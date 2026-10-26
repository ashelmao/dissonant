//! Polyphonic subtractive + FM synth playing a [`SynthPatch`]: a pool of voices with velocity,
//! round-robin allocation and oldest-note stealing, or one legato voice in mono mode.
//!
//! Per voice: 3 PolyBLEP oscillators (unison, FM / ring / hard sync between osc 1 and 2) +
//! coloured noise → pre-drive → multimode SVF (key tracking, envelope, velocity, LFO) → amp
//! envelope × velocity → post-drive. The voice sum then runs through crush → level → chorus →
//! echo. Envelopes tick per sample; everything else (filter coefficients, LFOs, glide, pitch
//! envelope, smoothed knobs) updates once per [`BLOCK`], with pitch and amplitude ramped
//! linearly across it so nothing steps audibly. Nothing allocates after construction.

use crate::dsp::{
    drive, drive_gain, midi_to_hz, soft_limit, wave, Adsr, AdsrParams, Chorus, Crusher, Echo, Lfo, Rng, Svf,
};
use dissonant_core::sound::{Envelope, FilterMode, LfoTarget, OscWave, SynthPatch};
use std::f32::consts::{PI, SQRT_2};

/// Samples per control block.
const BLOCK: usize = 16;
const MAX_UNISON: usize = 8;
/// Osc 1 phase deviation, in cycles, at FM amount 1 (a modulation index of ~9.4).
const FM_DEPTH: f32 = 1.5;
/// The longest echo the patch allows; the buffer is allocated for it up front.
const MAX_DELAY_S: f32 = 1.5;
/// Held notes remembered in mono mode (last-note priority).
const MONO_STACK: usize = 16;
/// Time constant for knob changes (levels, FM, ring, effect mixes) made while notes sound.
const SMOOTH_S: f32 = 0.005;
/// Output below this counts as silence when deciding the effects tail has died away.
const QUIET: f32 = 1e-5;

const ZEROS: [f32; BLOCK] = [0.0; BLOCK];

fn adsr(e: &Envelope) -> AdsrParams {
    AdsrParams {
        attack: e.attack,
        decay: e.decay,
        sustain: e.sustain,
        release: e.release,
    }
}

/// Everything about the patch that is costly to recompute per block: refreshed by
/// [`PolySynth::set_patch`].
#[derive(Debug, Clone, Copy)]
struct Derived {
    /// Phase increment per Hz of note frequency, per oscillator and unison voice.
    inc: [[f32; MAX_UNISON]; 3],
    unison: [usize; 3],
    /// 1/√n, so a unison stack is about as loud as one oscillator.
    ugain: [f32; 3],
    /// Phase increment per Hz of osc 1's undetuned pitch: the hard-sync master.
    sync_inc: f32,
    /// SVF damping (1/Q) for the resonance knob: Butterworth at 0, Q ≈ 24 at 1.
    k: f32,
    noise_coef: f32,
    /// Keeps darkened noise at white noise's rms.
    noise_gain: f32,
    pre_drive: (f32, f32),
    post_drive: (f32, f32),
    crush: (f32, f32),
    /// Per-sample rates (1/τ) for the glide and pitch-envelope exponentials; 0 = jump.
    glide_rate: f32,
    pitch_env_rate: f32,
    lfo_inc: [f32; 2],
    lfo_on: [bool; 2],
}

impl Derived {
    fn new(p: &SynthPatch, sr: f32) -> Self {
        let mut d = Derived {
            inc: [[0.0; MAX_UNISON]; 3],
            unison: [1; 3],
            ugain: [1.0; 3],
            sync_inc: 0.0,
            k: SQRT_2 * (1.0 - 0.97 * p.filter.resonance),
            noise_coef: 1.0,
            noise_gain: 1.0,
            pre_drive: drive_gain(p.filter.drive),
            post_drive: drive_gain(p.drive),
            crush: Crusher::params(p.crush),
            // The exponentials get ~95% of the way in the knob's time (τ = time / 3).
            glide_rate: if p.glide > 0.0 { 3.0 / (p.glide * sr) } else { 0.0 },
            pitch_env_rate: 3.0 / (p.pitch_env_decay.max(0.001) * sr),
            lfo_inc: [p.lfo[0].rate / sr, p.lfo[1].rate / sr],
            lfo_on: [0, 1].map(|l| p.lfo[l].depth > 0.0 && p.lfo[l].target != LfoTarget::None),
        };
        for (o, op) in p.osc.iter().enumerate() {
            let n = (op.unison as usize).clamp(1, MAX_UNISON);
            let base = (op.octave * 12 + op.semi) as f32 + op.fine / 100.0;
            d.unison[o] = n;
            d.ugain[o] = 1.0 / (n as f32).sqrt();
            for u in 0..n {
                // Spread evenly across ±detune/2 cents.
                let spread = if n > 1 { op.detune * (u as f32 / (n - 1) as f32 - 0.5) } else { 0.0 };
                d.inc[o][u] = ((base + spread / 100.0) / 12.0).exp2() / sr;
            }
            if o == 0 {
                d.sync_inc = (base / 12.0).exp2() / sr;
            }
        }
        let fc = 20_000.0 * (-7.0 * p.noise_color).exp2();
        let a = (1.0 - (-2.0 * PI * fc / sr).exp()).clamp(1e-4, 1.0);
        d.noise_coef = a;
        d.noise_gain = ((2.0 - a) / a).sqrt();
        d
    }
}

/// Knob values that are smoothed while notes sound, so turning them doesn't click.
#[derive(Debug, Clone, Copy, Default)]
struct Knobs {
    /// Oscillator level × unison gain; 0 when the oscillator is off.
    osc: [f32; 3],
    noise: f32,
    fm: f32,
    ring: f32,
    level: f32,
    chorus: f32,
    delay: f32,
}

impl Knobs {
    fn target(p: &SynthPatch, d: &Derived) -> Self {
        Knobs {
            osc: [0, 1, 2].map(|o| if p.osc[o].on { p.osc[o].level * d.ugain[o] } else { 0.0 }),
            noise: p.noise_level * d.noise_gain,
            fm: p.fm_amount,
            ring: p.ring_mod,
            level: p.level,
            chorus: p.chorus.mix,
            delay: p.delay.mix,
        }
    }

    fn approach(&mut self, t: &Knobs, c: f32) {
        let mv = |a: &mut f32, b: f32| *a += (b - *a) * c;
        for o in 0..3 {
            mv(&mut self.osc[o], t.osc[o]);
        }
        mv(&mut self.noise, t.noise);
        mv(&mut self.fm, t.fm);
        mv(&mut self.ring, t.ring);
        mv(&mut self.level, t.level);
        mv(&mut self.chorus, t.chorus);
        mv(&mut self.delay, t.delay);
    }
}

/// Shared by every voice for one block.
struct Ctx {
    /// Smoothed knobs at the start and end of the block.
    from: Knobs,
    to: Knobs,
    glide_mul: f32,
    pitch_env_mul: f32,
    /// Free-running LFO values.
    free_lfo: [f32; 2],
}

/// `a` ramped toward `b`: the value at sample `i` of `n`, reaching `b` on the last one.
#[inline]
fn ramp(a: f32, b: f32, i: usize, n: usize) -> f32 {
    a + (b - a) * ((i + 1) as f32 / n as f32)
}

#[derive(Debug, Clone)]
struct Voice {
    /// The held key; `None` once released.
    pitch: Option<u8>,
    started_at: u64,
    /// 0–1
    velocity: f32,
    /// Note pitch in semitones and where the glide currently is.
    target: f32,
    glide: f32,
    /// 1 at note-on, decaying to 0.
    pitch_env: f32,
    amp_env: Adsr,
    filter_env: Adsr,
    phase: [[f32; MAX_UNISON]; 3],
    sync_phase: f32,
    lfo: [Lfo; 2],
    filter: [Svf; 2],
    noise_lp: f32,
    rng: Rng,
    /// Frequency and amp-LFO gain at the end of the previous block (ramp start points).
    last_hz: f32,
    last_amp: f32,
    /// The next block starts without ramping from stale values.
    fresh: bool,
}

impl Voice {
    fn new(p: &SynthPatch, sr: f32, seed: u32) -> Self {
        Voice {
            pitch: None,
            started_at: 0,
            velocity: 0.0,
            target: 60.0,
            glide: 60.0,
            pitch_env: 0.0,
            amp_env: Adsr::new(adsr(&p.amp_env), sr),
            filter_env: Adsr::new(adsr(&p.filter_env), sr),
            phase: [[0.0; MAX_UNISON]; 3],
            sync_phase: 0.0,
            lfo: [Lfo::default(); 2],
            filter: [Svf::default(); 2],
            noise_lp: 0.0,
            rng: Rng::new(seed),
            last_hz: 0.0,
            last_amp: 1.0,
            fresh: true,
        }
    }

    /// Start a note gliding in from `from` semitones. A free voice (`reset`) starts clean; a
    /// stolen one keeps its phases, filter state and envelope levels — slamming any of them
    /// to zero while it still sounds would be a step, i.e. an audible click — and its attack
    /// ramps on from wherever the old note had got to.
    fn start(&mut self, p: &SynthPatch, pitch: u8, velocity: u8, from: f32, reset: bool, at: u64) {
        if reset {
            self.amp_env.reset();
            self.filter_env.reset();
            for (o, op) in p.osc.iter().enumerate() {
                for ph in &mut self.phase[o] {
                    *ph = if op.random_phase { self.rng.unit() } else { 0.0 };
                }
            }
            self.sync_phase = 0.0;
            self.filter.iter_mut().for_each(Svf::reset);
            self.noise_lp = 0.0;
            self.fresh = true;
        }
        for l in 0..2 {
            if reset || p.lfo[l].retrigger {
                self.lfo[l].reset(&mut self.rng);
            }
        }
        self.pitch = Some(pitch);
        self.started_at = at;
        self.velocity = velocity as f32 / 127.0;
        self.target = pitch as f32;
        self.glide = from;
        self.pitch_env = 1.0;
        self.amp_env.gate_on();
        self.filter_env.gate_on();
    }

    fn release(&mut self) {
        self.amp_env.gate_off();
        self.filter_env.gate_off();
        self.pitch = None;
    }

    /// Render one block (`out.len()` ≤ [`BLOCK`]), adding into `out`.
    fn render(&mut self, p: &SynthPatch, d: &Derived, c: &Ctx, sr: f32, out: &mut [f32]) {
        let n = out.len();
        let nf = n as f32;

        // ── Block-rate modulation ──
        let (mut m_pitch, mut m_cut, mut m_amp, mut m_pw, mut m_fm) = (0.0, 0.0, 1.0, 0.0, 0.0);
        for l in 0..2 {
            if !d.lfo_on[l] {
                continue;
            }
            let lp = &p.lfo[l];
            let v = if lp.retrigger {
                let v = self.lfo[l].value(lp.wave);
                self.lfo[l].advance(d.lfo_inc[l] * nf, &mut self.rng);
                v
            } else {
                c.free_lfo[l]
            };
            let x = lp.depth * v;
            match lp.target {
                LfoTarget::Pitch => m_pitch += 12.0 * x,
                LfoTarget::Cutoff => m_cut += 4.0 * x,
                LfoTarget::Amp => m_amp *= 1.0 - 0.5 * lp.depth * (1.0 - v),
                LfoTarget::PulseWidth => m_pw += 0.45 * x,
                LfoTarget::Fm => m_fm += x,
                LfoTarget::None => {}
            }
        }
        self.glide += (self.target - self.glide) * c.glide_mul;
        let hz = midi_to_hz(self.glide + p.pitch_env_amount * self.pitch_env + m_pitch);
        self.pitch_env *= c.pitch_env_mul;
        if self.fresh {
            self.last_hz = hz;
            self.last_amp = m_amp;
            self.fresh = false;
        }
        let mut hzs = [0.0f32; BLOCK];
        for (i, h) in hzs[..n].iter_mut().enumerate() {
            *h = ramp(self.last_hz, hz, i, n);
        }
        self.last_hz = hz;

        // ── Oscillators ──
        let (from, to) = (&c.from, &c.to);
        let mut mix = [0.0f32; BLOCK];
        let mut buf = [0.0f32; BLOCK];
        let mut modsig = [0.0f32; BLOCK];
        let fm_on = from.fm > 0.0 || to.fm > 0.0 || m_fm > 0.0;
        let ring_on = from.ring > 0.0 || to.ring > 0.0;
        let audible = |o: usize| from.osc[o] > 0.0 || to.osc[o] > 0.0;
        let pw = [0, 1, 2].map(|o| (p.osc[o].pulse_width + m_pw).clamp(0.05, 0.95));

        // Osc 2 first: it modulates osc 1. It runs as a silent modulator when it is off.
        if audible(1) || fm_on || ring_on {
            let mut sync = [-1.0f32; BLOCK];
            if p.sync {
                // Osc 2 restarts whenever a master phase at osc 1's pitch wraps, carrying the
                // fraction of a sample since the wrap.
                let mut mp = self.sync_phase;
                for i in 0..n {
                    let dm = (hzs[i] * d.sync_inc).min(0.45);
                    mp += dm;
                    if mp >= 1.0 {
                        mp -= 1.0;
                        sync[i] = mp / dm;
                    }
                }
                self.sync_phase = mp;
            }
            let u = d.unison[1];
            run_osc(
                p.osc[1].wave,
                pw[1],
                &mut self.phase[1][..u],
                &d.inc[1][..u],
                &hzs,
                None,
                p.sync.then_some(&sync),
                &mut buf[..n],
            );
            if fm_on || ring_on {
                let s = 1.0 / u as f32;
                for i in 0..n {
                    modsig[i] = buf[i] * s;
                }
            }
            if audible(1) {
                for i in 0..n {
                    mix[i] += buf[i] * ramp(from.osc[1], to.osc[1], i, n);
                }
            }
        }
        if audible(0) {
            buf = ZEROS;
            let mut pm = ZEROS;
            if fm_on {
                for i in 0..n {
                    pm[i] = FM_DEPTH * (ramp(from.fm, to.fm, i, n) + m_fm).clamp(0.0, 1.0) * modsig[i];
                }
            }
            let u = d.unison[0];
            run_osc(
                p.osc[0].wave,
                pw[0],
                &mut self.phase[0][..u],
                &d.inc[0][..u],
                &hzs,
                fm_on.then_some(&pm),
                None,
                &mut buf[..n],
            );
            for i in 0..n {
                let mut x = buf[i];
                if ring_on {
                    let r = ramp(from.ring, to.ring, i, n);
                    x *= 1.0 - r + r * modsig[i];
                }
                mix[i] += x * ramp(from.osc[0], to.osc[0], i, n);
            }
        }
        if audible(2) {
            buf = ZEROS;
            let u = d.unison[2];
            run_osc(p.osc[2].wave, pw[2], &mut self.phase[2][..u], &d.inc[2][..u], &hzs, None, None, &mut buf[..n]);
            for i in 0..n {
                mix[i] += buf[i] * ramp(from.osc[2], to.osc[2], i, n);
            }
        }
        if from.noise > 0.0 || to.noise > 0.0 {
            let a = d.noise_coef;
            for (i, m) in mix[..n].iter_mut().enumerate() {
                self.noise_lp += (self.rng.bipolar() - self.noise_lp) * a;
                *m += self.noise_lp * ramp(from.noise, to.noise, i, n);
            }
        }

        // ── Filter coefficients from the envelope's level at the block start ──
        let fenv = self.filter_env.level();
        for _ in 0..n {
            self.filter_env.tick();
        }
        let f = &p.filter;
        let octaves = f.env_amount * fenv + m_cut + f.key_track * (self.glide - 60.0) / 12.0
            - 4.0 * f.velocity * (1.0 - self.velocity);
        let cutoff = (f.cutoff * octaves.exp2()).clamp(20.0, 20_000.0);
        match f.mode {
            FilterMode::Off => {}
            FilterMode::Lowpass24 => {
                // Resonance on the second stage only: two resonant stages would square the peak.
                self.filter[0].set(sr, cutoff, SQRT_2);
                self.filter[1].set(sr, cutoff, d.k);
            }
            _ => self.filter[0].set(sr, cutoff, d.k),
        }

        // A lowpass all the way open is spectrally flat but its phase shift near Nyquist still
        // adds overshoot to saw / square edges, so pass the dry signal while it is fully open
        // (the filter keeps running, so it picks up seamlessly when the cutoff comes down).
        let open = cutoff >= 20_000.0 && f.resonance <= 0.0;
        match f.mode {
            FilterMode::Off => self.finish(p, d, &mix, m_amp, out, |_, x| x),
            FilterMode::Lowpass12 if open => self.finish(p, d, &mix, m_amp, out, |f, x| {
                f[0].tick(x);
                x
            }),
            FilterMode::Lowpass24 if open => self.finish(p, d, &mix, m_amp, out, |f, x| {
                let y = f[0].tick(x).low;
                f[1].tick(y);
                x
            }),
            FilterMode::Lowpass12 => self.finish(p, d, &mix, m_amp, out, |f, x| f[0].tick(x).low),
            FilterMode::Lowpass24 => self.finish(p, d, &mix, m_amp, out, |f, x| {
                let y = f[0].tick(x).low;
                f[1].tick(y).low
            }),
            FilterMode::Highpass12 => self.finish(p, d, &mix, m_amp, out, |f, x| f[0].tick(x).high),
            FilterMode::Bandpass => self.finish(p, d, &mix, m_amp, out, |f, x| f[0].tick(x).band),
            FilterMode::Notch => self.finish(p, d, &mix, m_amp, out, |f, x| {
                let o = f[0].tick(x);
                o.low + o.high
            }),
        }
    }

    /// Per sample: pre-drive → filter → safety limit → amp envelope × velocity × amp LFO →
    /// post-drive, added into `out`.
    #[inline(always)]
    fn finish(
        &mut self,
        p: &SynthPatch,
        d: &Derived,
        mix: &[f32; BLOCK],
        m_amp: f32,
        out: &mut [f32],
        mut filt: impl FnMut(&mut [Svf; 2], f32) -> f32,
    ) {
        let n = out.len();
        let vel_gain = 1.0 - p.velocity * (1.0 - self.velocity);
        let (pre, (pg, pn)) = (p.filter.drive, d.pre_drive);
        let (post, (qg, qn)) = (p.drive, d.post_drive);
        let a0 = self.last_amp;
        for i in 0..n {
            let mut x = mix[i];
            if pre > 0.0 {
                x = drive(x, pre, pg, pn);
            }
            let y = soft_limit(filt(&mut self.filter, x), 2.0);
            let mut y = y * self.amp_env.tick() * vel_gain * ramp(a0, m_amp, i, n);
            if post > 0.0 {
                y = drive(y, post, qg, qn);
            }
            out[i] += y;
        }
        self.last_amp = m_amp;
        self.filter.iter_mut().for_each(Svf::sanitize);
    }
}

/// Add one oscillator's unison stack into `out`, picking a specialized inner loop for the
/// waveform and for whether phase modulation / hard sync are in play.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn run_osc(
    w: OscWave,
    pw: f32,
    phases: &mut [f32],
    inc: &[f32],
    hz: &[f32; BLOCK],
    pm: Option<&[f32; BLOCK]>,
    sync: Option<&[f32; BLOCK]>,
    out: &mut [f32],
) {
    macro_rules! go {
        ($f:expr) => {
            match (pm, sync) {
                (None, None) => run::<false, false>(phases, inc, hz, &ZEROS, &ZEROS, out, $f),
                (Some(pm), None) => run::<true, false>(phases, inc, hz, pm, &ZEROS, out, $f),
                (None, Some(s)) => run::<false, true>(phases, inc, hz, &ZEROS, s, out, $f),
                (Some(pm), Some(s)) => run::<true, true>(phases, inc, hz, pm, s, out, $f),
            }
        };
    }
    match w {
        OscWave::Sine => go!(|t, _| wave::sine(t)),
        OscWave::Triangle => go!(wave::triangle),
        OscWave::Saw => go!(wave::saw),
        OscWave::Square => go!(|t, dt| wave::pulse(t, dt, 0.5)),
        OscWave::Pulse => go!(|t, dt| wave::pulse(t, dt, pw)),
    }
}

#[inline(always)]
fn run<const PM: bool, const SYNC: bool>(
    phases: &mut [f32],
    inc: &[f32],
    hz: &[f32; BLOCK],
    pm: &[f32; BLOCK],
    sync: &[f32; BLOCK],
    out: &mut [f32],
    f: impl Fn(f32, f32) -> f32,
) {
    let n = out.len();
    for (ph, &k) in phases.iter_mut().zip(inc) {
        let mut p = *ph;
        for i in 0..n {
            let dt = (hz[i] * k).min(0.45);
            let t = if PM {
                let x = p + pm[i];
                x - x.floor()
            } else {
                p
            };
            out[i] += f(t, dt);
            p += dt;
            if p >= 1.0 {
                p -= 1.0;
            }
            if SYNC && sync[i] >= 0.0 {
                p = sync[i] * dt;
            }
        }
        *ph = p;
    }
}

#[derive(Debug, Clone)]
pub struct PolySynth {
    patch: SynthPatch,
    derived: Derived,
    sample_rate: f32,
    voices: Vec<Voice>,
    counter: u64,
    knobs: Knobs,
    /// Free-running LFOs (retriggered ones live in each voice).
    lfo: [Lfo; 2],
    rng: Rng,
    /// Mono mode's held keys, oldest first.
    held: [u8; MONO_STACK],
    held_len: usize,
    /// Where the next glide starts from.
    last_note: Option<f32>,
    crusher: Crusher,
    chorus: Chorus,
    echo: Echo,
    /// An effect that was bypassed may hold stale audio; clear it when it comes back.
    chorus_stale: bool,
    echo_stale: bool,
    /// Consecutive samples with every voice idle and the output below [`QUIET`].
    quiet_run: usize,
}

impl PolySynth {
    pub fn new(patch: &SynthPatch, sample_rate: f32, polyphony: usize) -> Self {
        let patch = patch.sanitized();
        let derived = Derived::new(&patch, sample_rate);
        let mut rng = Rng::new(0x9E37_79B9);
        let voices = (0..polyphony.max(1) as u32)
            .map(|i| Voice::new(&patch, sample_rate, 0x2545_F491 ^ (i + 1).wrapping_mul(0x9E37_79B9)))
            .collect();
        let mut lfo = [Lfo::default(); 2];
        lfo.iter_mut().for_each(|l| l.reset(&mut rng));
        let mut echo = Echo::new(sample_rate, MAX_DELAY_S);
        echo.reset(patch.delay.time);
        PolySynth {
            knobs: Knobs::target(&patch, &derived),
            patch,
            derived,
            sample_rate,
            voices,
            counter: 0,
            lfo,
            rng,
            held: [0; MONO_STACK],
            held_len: 0,
            last_note: None,
            crusher: Crusher::default(),
            chorus: Chorus::new(sample_rate),
            echo,
            chorus_stale: false,
            echo_stale: false,
            quiet_run: usize::MAX / 2,
        }
    }

    pub fn patch(&self) -> &SynthPatch {
        &self.patch
    }

    /// Change the sound live. Sounding voices carry on with the new parameters (levels and
    /// effect mixes glide over a few ms); nothing is cut or retriggered.
    pub fn set_patch(&mut self, patch: &SynthPatch) {
        let patch = patch.sanitized();
        if patch.mono != self.patch.mono {
            // Keys held in the other mode are released through the poly path.
            self.held_len = 0;
        }
        self.patch = patch;
        self.derived = Derived::new(&patch, self.sample_rate);
        for v in &mut self.voices {
            v.amp_env.set_params(adsr(&patch.amp_env));
            v.filter_env.set_params(adsr(&patch.filter_env));
        }
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8) {
        self.counter += 1;
        if self.patch.mono {
            self.mono_on(pitch, velocity);
        } else {
            let idx = match self.voices.iter().position(|v| v.pitch.is_none() && !v.amp_env.is_active()) {
                Some(i) => i,
                None => match self.voices.iter().position(|v| v.pitch.is_none()) {
                    // Releasing voice: reuse the one that started longest ago.
                    Some(_) => self
                        .voices
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| v.pitch.is_none())
                        .min_by_key(|(_, v)| v.started_at)
                        .map(|(i, _)| i)
                        .unwrap(),
                    None => self
                        .voices
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, v)| v.started_at)
                        .map(|(i, _)| i)
                        .unwrap(),
                },
            };
            // Poly portamento glides each new note in from the last one played.
            let from = match self.last_note {
                Some(last) if self.patch.glide > 0.0 => last,
                _ => pitch as f32,
            };
            let v = &mut self.voices[idx];
            let reset = !v.amp_env.is_active();
            v.start(&self.patch, pitch, velocity, from, reset, self.counter);
        }
        self.last_note = Some(pitch as f32);
    }

    /// Mono: last-note priority. A key pressed while another is held (and still sounding)
    /// slides there legato — no envelope retrigger; otherwise the note starts afresh.
    fn mono_on(&mut self, pitch: u8, velocity: u8) {
        self.unhold(pitch);
        let legato = self.held_len > 0;
        if self.held_len == MONO_STACK {
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        self.held[self.held_len] = pitch;
        self.held_len += 1;
        let glide = self.patch.glide > 0.0;
        let v = &mut self.voices[0];
        let sounding = v.amp_env.is_active();
        if legato && sounding {
            v.pitch = Some(pitch);
            v.started_at = self.counter;
            v.target = pitch as f32;
            if !glide {
                v.glide = v.target;
            }
        } else {
            let from = if glide && sounding { v.glide } else { pitch as f32 };
            v.start(&self.patch, pitch, velocity, from, !sounding, self.counter);
        }
    }

    /// Drop `pitch` from the mono stack; whether it was there.
    fn unhold(&mut self, pitch: u8) -> bool {
        match self.held[..self.held_len].iter().position(|&h| h == pitch) {
            Some(i) => {
                self.held.copy_within(i + 1..self.held_len, i);
                self.held_len -= 1;
                true
            }
            None => false,
        }
    }

    /// Release *one* voice: the oldest one still holding `pitch`. Note-ons and note-offs are
    /// 1:1, so releasing every matching voice would make two overlapping notes of the same
    /// pitch cut each other short — the first note's end would silence the second.
    pub fn note_off(&mut self, pitch: u8) {
        if self.patch.mono && self.unhold(pitch) {
            let v = &mut self.voices[0];
            if v.pitch == Some(pitch) {
                if self.held_len > 0 {
                    // Fall back to the most recent key still held.
                    let prev = self.held[self.held_len - 1];
                    v.pitch = Some(prev);
                    v.target = prev as f32;
                    if self.patch.glide <= 0.0 {
                        v.glide = v.target;
                    }
                    self.last_note = Some(prev as f32);
                } else {
                    v.release();
                }
            }
            return;
        }
        let oldest = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.pitch == Some(pitch))
            .min_by_key(|(_, v)| v.started_at)
            .map(|(i, _)| i);
        if let Some(i) = oldest {
            self.voices[i].release();
        }
    }

    pub fn all_notes_off(&mut self) {
        self.held_len = 0;
        for v in &mut self.voices {
            if v.pitch.is_some() {
                v.release();
            }
        }
    }

    /// No voice sounding and the chorus / echo / crusher tail has died away.
    pub fn is_silent(&self) -> bool {
        self.voices_idle() && self.quiet_run >= self.tail_len()
    }

    fn voices_idle(&self) -> bool {
        self.voices.iter().all(|v| !v.amp_env.is_active())
    }

    /// How long the output must stay quiet, with no voice sounding, before the effects can
    /// hold nothing but silence.
    fn tail_len(&self) -> usize {
        let p = &self.patch;
        let mut t = 0;
        if p.crush > 0.0 {
            t += 64;
        }
        if p.chorus.mix > 0.0 || self.knobs.chorus > 0.0 {
            t += self.chorus.tail();
        }
        if p.delay.mix > 0.0 || self.knobs.delay > 0.0 {
            t += self.echo.time_samples() as usize + 64;
        }
        t
    }

    /// Render mono into `out` (additive).
    pub fn render_add(&mut self, out: &mut [f32]) {
        for chunk in out.chunks_mut(BLOCK) {
            let voices_on = !self.voices_idle();
            if !voices_on && self.quiet_run >= self.tail_len() {
                // Silent until the next note: settle the knobs so it starts at their levels.
                self.knobs = Knobs::target(&self.patch, &self.derived);
                return;
            }
            self.render_block(chunk, voices_on);
        }
    }

    fn render_block(&mut self, out: &mut [f32], voices_on: bool) {
        let n = out.len();
        let nf = n as f32;
        let sr = self.sample_rate;
        let d = self.derived;
        let from = self.knobs;
        let mut to = from;
        to.approach(&Knobs::target(&self.patch, &d), (nf / (SMOOTH_S * sr)).min(1.0));
        self.knobs = to;
        let ctx = Ctx {
            from,
            to,
            glide_mul: if d.glide_rate > 0.0 { 1.0 - (-nf * d.glide_rate).exp() } else { 1.0 },
            pitch_env_mul: (-nf * d.pitch_env_rate).exp(),
            free_lfo: [0, 1].map(|l| self.lfo[l].value(self.patch.lfo[l].wave)),
        };
        for l in 0..2 {
            self.lfo[l].advance(d.lfo_inc[l] * nf, &mut self.rng);
        }

        let mut sum = [0.0f32; BLOCK];
        let p = &self.patch;
        if voices_on {
            for v in self.voices.iter_mut().filter(|v| v.amp_env.is_active()) {
                v.render(p, &d, &ctx, sr, &mut sum[..n]);
            }
        }

        // ── Post: crush → level → chorus → echo ──
        if p.crush > 0.0 {
            let (ratio, steps) = d.crush;
            for x in &mut sum[..n] {
                *x = self.crusher.process(*x, ratio, steps);
            }
        }
        let chorus_on = from.chorus > 0.0 || to.chorus > 0.0;
        if !chorus_on {
            self.chorus_stale = true;
        } else if self.chorus_stale {
            self.chorus.clear();
            self.chorus_stale = false;
        }
        let echo_on = from.delay > 0.0 || to.delay > 0.0;
        if !echo_on {
            self.echo_stale = true;
        } else if self.echo_stale {
            self.echo.reset(p.delay.time);
            self.echo_stale = false;
        }
        let mut peak = 0.0f32;
        for i in 0..n {
            let mut y = sum[i] * ramp(from.level, to.level, i, n);
            if chorus_on {
                y = self.chorus.process(y, p.chorus.rate, p.chorus.depth, ramp(from.chorus, to.chorus, i, n));
            }
            if echo_on {
                y = self.echo.process(y, p.delay.time, p.delay.feedback, ramp(from.delay, to.delay, i, n));
            }
            if !y.is_finite() {
                // Never let a NaN reach the mix bus; drop whatever the effects were holding.
                y = 0.0;
                self.chorus_stale = true;
                self.echo_stale = true;
            }
            peak = peak.max(y.abs());
            out[i] += y;
        }
        self.quiet_run = if voices_on || peak >= QUIET { 0 } else { self.quiet_run.saturating_add(n) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{Oscillator, Waveform};
    use dissonant_core::sound::{factory_sound, factory_sounds, SoundPatch};
    use std::time::Instant;

    const SR: f32 = 48_000.0;

    fn factory(id: &str) -> SynthPatch {
        match factory_sound(id).map(|s| &s.patch) {
            Some(SoundPatch::Synth(p)) => *p,
            _ => panic!("no factory synth {id}"),
        }
    }

    fn render(s: &mut PolySynth, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0; frames];
        // Odd-sized chunks, like the engine's event-split sub-blocks.
        for chunk in buf.chunks_mut(333) {
            s.render_add(chunk);
        }
        buf
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
    }

    /// Every feature on at once.
    fn kitchen_sink() -> SynthPatch {
        let mut p = SynthPatch::default();
        for (i, o) in p.osc.iter_mut().enumerate() {
            o.on = true;
            o.unison = 8;
            o.detune = 40.0;
            o.random_phase = true;
            o.wave = [OscWave::Saw, OscWave::Pulse, OscWave::Triangle][i];
            o.octave = i as i32 - 1;
        }
        p.noise_level = 0.4;
        p.noise_color = 0.5;
        p.fm_amount = 0.5;
        p.ring_mod = 0.3;
        p.sync = true;
        p.filter.mode = FilterMode::Lowpass24;
        p.filter.cutoff = 1_200.0;
        p.filter.resonance = 1.0;
        p.filter.drive = 0.8;
        p.filter.key_track = 0.5;
        p.filter.env_amount = 5.0;
        p.filter.velocity = 0.5;
        p.pitch_env_amount = 12.0;
        p.pitch_env_decay = 0.08;
        p.lfo[0] = dissonant_core::sound::LfoParams {
            wave: dissonant_core::sound::LfoWave::SampleHold,
            rate: 7.0,
            depth: 0.6,
            target: LfoTarget::Cutoff,
            retrigger: true,
        };
        p.lfo[1] = dissonant_core::sound::LfoParams {
            wave: dissonant_core::sound::LfoWave::Triangle,
            rate: 0.7,
            depth: 0.3,
            target: LfoTarget::PulseWidth,
            retrigger: false,
        };
        p.drive = 0.6;
        p.crush = 0.3;
        p.chorus.mix = 0.6;
        p.delay.mix = 0.5;
        p.delay.feedback = 0.95;
        p.delay.time = 0.11;
        p.level = 0.3;
        p
    }

    #[test]
    fn plays_and_releases() {
        let mut s = PolySynth::new(&factory("saw"), SR, 4);
        assert!(s.is_silent());
        s.note_on(60, 100);
        let buf = render(&mut s, 4800);
        assert!(buf.iter().any(|x| x.abs() > 0.01));
        s.note_off(60);
        render(&mut s, 48_000);
        assert!(s.is_silent());
    }

    #[test]
    fn steals_oldest_voice_when_full() {
        let mut s = PolySynth::new(&factory("pad"), SR, 2);
        s.note_on(60, 100);
        s.note_on(64, 100);
        s.note_on(67, 100);
        let sounding: Vec<u8> = s.voices.iter().filter_map(|v| v.pitch).collect();
        assert_eq!(sounding.len(), 2);
        assert!(!sounding.contains(&60));
    }

    #[test]
    fn velocity_scales_amplitude() {
        let render_at = |vel: u8| {
            let mut s = PolySynth::new(&factory("sine"), SR, 1);
            s.note_on(69, vel);
            peak(&render(&mut s, 9600))
        };
        assert!(render_at(127) > render_at(40) * 2.0);
    }

    #[test]
    fn renders_are_deterministic() {
        let go = || {
            let mut s = PolySynth::new(&kitchen_sink(), SR, 4);
            s.note_on(48, 110);
            s.note_on(55, 90);
            render(&mut s, 12_000)
        };
        assert_eq!(go(), go());
    }

    /// The six legacy names must sound like the old fixed `VoicePreset`s: one oscillator ×
    /// ADSR × velocity × level, no filter.
    #[test]
    fn legacy_sounds_match_old_voices() {
        for id in ["saw", "square", "triangle", "sine", "pad", "pluck"] {
            let p = factory(id);
            let wf = match p.osc[0].wave {
                OscWave::Saw => Waveform::Saw,
                OscWave::Square => Waveform::Square,
                OscWave::Triangle => Waveform::Triangle,
                _ => Waveform::Sine,
            };
            let (hold, total) = (24_000, 72_000);
            let mut old = vec![0.0f32; total];
            let mut osc = Oscillator::new(wf);
            osc.set_frequency(midi_to_hz(57.0), SR);
            let mut env = Adsr::new(adsr(&p.amp_env), SR);
            env.gate_on();
            let amp = 100.0 / 127.0 * p.level;
            for (i, s) in old.iter_mut().enumerate() {
                if i == hold {
                    env.gate_off();
                }
                *s = osc.tick() * env.tick() * amp;
            }

            let mut s = PolySynth::new(&p, SR, 4);
            s.note_on(57, 100);
            let mut new = render(&mut s, hold);
            s.note_off(57);
            new.extend(render(&mut s, total - hold));

            let ratio = rms(&new) / rms(&old);
            let peaks = peak(&new) / peak(&old);
            eprintln!("{id}: rms ratio {ratio:.3}, peak ratio {peaks:.3}");
            // The old triangle was a leaky integrator that starts at 0 and clips flat for its
            // first half cycle (louder on short notes); the new one is a clean triangle.
            let tol = if wf == Waveform::Triangle { 0.12 } else { 0.03 };
            assert!((ratio - 1.0).abs() < tol, "{id}: rms ratio {ratio}");
            assert!((peaks - 1.0).abs() < tol, "{id}: peak ratio {peaks}");
            if wf != Waveform::Triangle {
                // Same waveform, same phase: the renders should line up sample for sample.
                let dot: f32 = old.iter().zip(&new).map(|(a, b)| a * b).sum();
                let corr = dot / (rms(&old) * rms(&new) * total as f32);
                assert!(corr > 0.98, "{id}: correlation {corr}");
            }
        }
    }

    #[test]
    fn every_factory_synth_renders_sanely() {
        let mut n = 0;
        for sound in factory_sounds() {
            let SoundPatch::Synth(p) = &sound.patch else { continue };
            n += 1;
            let mut s = PolySynth::new(p, SR, 12);
            for pitch in [48, 55, 60, 64, 67] {
                s.note_on(pitch, 127);
            }
            let mut buf = render(&mut s, 48_000);
            for pitch in [48, 55, 60, 64, 67] {
                s.note_off(pitch);
            }
            buf.extend(render(&mut s, 24_000));
            assert!(buf.iter().all(|x| x.is_finite()), "{}: non-finite output", sound.id);
            assert!(rms(&buf) > 1e-3, "{}: silent", sound.id);
            assert!(peak(&buf) < 4.0, "{}: peak {}", sound.id, peak(&buf));
        }
        assert!(n >= 6);
    }

    #[test]
    fn set_patch_mid_note_neither_clicks_nor_cuts() {
        let mut p = factory("sine");
        let mut s = PolySynth::new(&p, SR, 4);
        s.note_on(57, 100);
        let before = render(&mut s, 9_600);
        // Big jumps on every smoothed knob plus filter and envelope changes.
        p.level = 0.9;
        p.osc[0].level = 0.3;
        p.osc[1].on = true;
        p.filter.cutoff = 900.0;
        p.amp_env.sustain = 0.3;
        p.chorus.mix = 1.0;
        s.set_patch(&p);
        let after = render(&mut s, 9_600);
        assert!(!s.is_silent());
        assert!(rms(&after[4_800..]) > 0.02, "set_patch silenced the note");
        // The largest sample-to-sample step across the change stays in the range of the
        // waveform's own slope (a 220 Hz sine at this level steps ≤ ~0.03/sample).
        let mut joined = before[before.len() - 64..].to_vec();
        joined.extend(&after[..512]);
        let max_step = joined.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(max_step < 0.06, "click: step {max_step}");
    }

    /// Zero-crossing rate of a sine voice in a window → its frequency.
    fn freq(buf: &[f32]) -> f32 {
        let ups = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        ups as f32 * SR / buf.len() as f32
    }

    #[test]
    fn mono_glide_slides_between_held_notes() {
        let mut p = factory("sine");
        p.mono = true;
        p.glide = 0.3;
        let mut s = PolySynth::new(&p, SR, 4);
        s.note_on(57, 100); // 220 Hz
        let a = render(&mut s, 9_600);
        s.note_on(69, 100); // 440 Hz, legato
        let early = render(&mut s, 2_400);
        render(&mut s, 24_000);
        let late = render(&mut s, 9_600);
        assert!((freq(&a) - 220.0).abs() < 10.0, "start {}", freq(&a));
        assert!(freq(&early) > 225.0 && freq(&early) < 400.0, "early {}", freq(&early));
        assert!((freq(&late) - 440.0).abs() < 10.0, "late {}", freq(&late));
        // One voice, never retriggered.
        assert_eq!(s.voices.iter().filter(|v| v.amp_env.is_active()).count(), 1);
        // Releasing the top note falls back to the held one.
        s.note_off(69);
        render(&mut s, 30_000);
        assert!((freq(&render(&mut s, 9_600)) - 220.0).abs() < 10.0);
        s.note_off(57);
        render(&mut s, 48_000);
        assert!(s.is_silent());
    }

    #[test]
    fn lfo_amp_target_makes_tremolo() {
        let mut p = factory("sine");
        p.amp_env.sustain = 1.0;
        p.lfo[0] = dissonant_core::sound::LfoParams {
            wave: dissonant_core::sound::LfoWave::Sine,
            rate: 4.0,
            depth: 1.0,
            target: LfoTarget::Amp,
            retrigger: true,
        };
        let mut s = PolySynth::new(&p, SR, 1);
        s.note_on(69, 127);
        render(&mut s, 4_800);
        let buf = render(&mut s, 48_000);
        let windows: Vec<f32> = buf.chunks(480).map(rms).collect();
        let (lo, hi) = windows.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &w| (lo.min(w), hi.max(w)));
        assert!(hi > 0.05 && lo < hi * 0.1, "tremolo range {lo}..{hi}");
    }

    #[test]
    fn everything_on_stays_finite_and_bounded() {
        let mut s = PolySynth::new(&kitchen_sink(), SR, 12);
        for (i, pitch) in [24, 36, 48, 55, 60, 64, 67, 72, 79, 84, 96, 108].into_iter().enumerate() {
            s.note_on(pitch, 40 + i as u8 * 7);
        }
        let mut buf = render(&mut s, 96_000);
        s.all_notes_off();
        buf.extend(render(&mut s, 48_000));
        assert!(buf.iter().all(|x| x.is_finite()));
        assert!(rms(&buf) > 1e-3);
        assert!(peak(&buf) < 8.0, "peak {}", peak(&buf));
    }

    #[test]
    fn every_filter_mode_is_stable_at_full_resonance() {
        for mode in [
            FilterMode::Lowpass12,
            FilterMode::Lowpass24,
            FilterMode::Highpass12,
            FilterMode::Bandpass,
            FilterMode::Notch,
        ] {
            let mut p = factory("saw");
            p.filter.mode = mode;
            p.filter.resonance = 1.0;
            p.filter.drive = 1.0;
            p.filter.cutoff = 300.0;
            p.filter.env_amount = 8.0;
            p.filter_env.decay = 0.4;
            p.level = 1.0;
            let mut s = PolySynth::new(&p, SR, 4);
            for pitch in [36, 48, 60, 72] {
                s.note_on(pitch, 127);
            }
            let buf = render(&mut s, 48_000);
            assert!(buf.iter().all(|x| x.is_finite()), "{mode:?}");
            // Four voices, each limited to ±4 before velocity and envelope.
            assert!(peak(&buf) < 16.0, "{mode:?}: peak {}", peak(&buf));
            assert!(rms(&buf) > 1e-3, "{mode:?}: silent");
        }
    }

    #[test]
    fn echo_tail_keeps_the_synth_awake() {
        let mut p = factory("pluck");
        p.delay.mix = 0.6;
        p.delay.time = 0.25;
        p.delay.feedback = 0.5;
        let mut s = PolySynth::new(&p, SR, 2);
        s.note_on(60, 127);
        render(&mut s, 2_400);
        s.note_off(60);
        render(&mut s, 12_000);
        assert!(s.voices_idle());
        assert!(!s.is_silent(), "echo tail cut off");
        // An echo arrives ~0.25 s after the note.
        assert!(rms(&render(&mut s, 12_000)) > 1e-3);
        render(&mut s, 48_000 * 4);
        assert!(s.is_silent());
    }

    /// Worst case: 12 voices × 3 oscillators × 8 unison with every feature and effect on.
    /// Prints the real-time factor; measure with `cargo test --release -p dissonant-engine
    /// synth_performance -- --nocapture`.
    #[test]
    fn synth_performance() {
        let mut s = PolySynth::new(&kitchen_sink(), SR, 12);
        for pitch in 48..60 {
            s.note_on(pitch, 100);
        }
        let mut buf = vec![0.0f32; 512];
        let t = Instant::now();
        for _ in 0..(SR as usize / 512) {
            buf.fill(0.0);
            s.render_add(&mut buf);
        }
        let rtf = t.elapsed().as_secs_f64() / ((SR as usize / 512 * 512) as f64 / SR as f64);
        eprintln!("synth worst case: real-time factor {rtf:.4} ({:.1}x headroom)", 1.0 / rtf);
        assert!(buf.iter().all(|x| x.is_finite()));
        if !cfg!(debug_assertions) {
            assert!(rtf < 0.5, "worst-case synth too slow: rtf {rtf}");
        }
    }
}
