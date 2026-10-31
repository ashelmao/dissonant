//! Polyphonic subtractive + FM synth playing a [`SynthPatch`]: a pool of voices with velocity,
//! round-robin allocation and oldest-note stealing in Poly, or one shared voice in Mono and
//! Legato. A note always starts at its own pitch: nothing glides on its own. The one exception
//! is a slide (a rope from another note), which takes over the voice of the note it comes from
//! in any mode and glides it to the new pitch.
//!
//! Per voice: 3 PolyBLEP oscillators (unison, FM / ring / hard sync between osc 1 and 2) +
//! coloured noise → pre-drive → multimode SVF (key tracking, envelope, velocity, LFO) → the
//! note's own Mod X / Mod Y filter → amp envelope × velocity → post-drive → the note's pan.
//! The voice sum then runs through crush → level → chorus → echo, in stereo, because per-note
//! pan means the two sides are no longer the same signal. Envelopes tick per sample;
//! everything else (filter coefficients, LFOs, glide, pitch envelope, smoothed knobs) updates
//! once per [`BLOCK`], with pitch and amplitude ramped linearly across it so nothing steps
//! audibly. Nothing allocates after construction.
//!
//! A note that nobody has shaped ([`NoteShape::neutral`]) renders bit-for-bit as the patch
//! alone would: unity gains on both sides, no extra filter and the patch's own release.

use crate::dsp::{
    drive, drive_gain, midi_to_hz, note_filter, note_filter_tuning, note_pan_gains, soft_limit,
    wave, Adsr, AdsrParams, Chorus, Crusher, Echo, Lfo, NoteFilter, Rng, Svf,
};
use crate::engine::{NoteShape, Slide, VoiceTag};
use dissonant_core::sound::{Envelope, FilterMode, LfoTarget, OscWave, SynthPatch, VoiceMode};
use dissonant_core::MAX_SLIDE_SECS;
use std::f32::consts::{PI, SQRT_2};

/// Samples per control block.
const BLOCK: usize = 16;
const MAX_UNISON: usize = 8;
/// Osc 1 phase deviation, in cycles, at FM amount 1 (a modulation index of ~9.4).
const FM_DEPTH: f32 = 1.5;
/// The longest echo the patch allows; the buffer is allocated for it up front.
const MAX_DELAY_S: f32 = 1.5;
/// Held notes remembered by the one-voice modes (last-note priority).
const MONO_STACK: usize = 16;
/// Time constant for knob changes (levels, FM, ring, effect mixes) made while notes sound.
const SMOOTH_S: f32 = 0.005;
/// Output below this counts as silence when deciding the effects tail has died away.
const QUIET: f32 = 1e-5;

/// How far a note's `release` scales the patch's own tail, in octaves either way: 0 is a
/// quarter of it, 0.5 leaves it alone, 1 stretches it fourfold.
const RELEASE_OCTAVES: f32 = 2.0;

/// A note filter as good as absent, in the octave form a slide eases through: log2 of a cutoff
/// at the top of the range (20 kHz) and of a flat Q (0.707). A slide into or out of a note with
/// no filter of its own starts or ends here.
const OPEN_FILTER: (f32, f32) = (14.287_712, -0.500_217_9);

/// Where a synth sits when its track has told it nothing. Only bare test synths are ever in
/// that state; a channel pushes its own tone down on every `apply`.
const DEFAULT_TONE_HZ: f32 = 20_000.0;

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
    /// Per-sample rate (1/τ) for the pitch-envelope exponential.
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
                let spread = if n > 1 {
                    op.detune * (u as f32 / (n - 1) as f32 - 0.5)
                } else {
                    0.0
                };
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
            osc: [0, 1, 2].map(|o| {
                if p.osc[o].on {
                    p.osc[o].level * d.ugain[o]
                } else {
                    0.0
                }
            }),
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
    pitch_env_mul: f32,
    /// Free-running LFO values.
    free_lfo: [f32; 2],
}

/// `a` ramped toward `b`: the value at sample `i` of `n`, reaching `b` on the last one.
#[inline]
fn ramp(a: f32, b: f32, i: usize, n: usize) -> f32 {
    a + (b - a) * ((i + 1) as f32 / n as f32)
}

/// The rate (1/τ per sample) of an exponential that gets ~95% of the way in `secs` (τ = secs / 3);
/// 0, a jump, for no time at all.
fn slide_rate(secs: f32, sr: f32) -> f32 {
    if secs > 0.0 {
        3.0 / (secs * sr)
    } else {
        0.0
    }
}

/// What a note's `release` multiplies the patch's tail by.
fn release_scale(release: f32) -> f32 {
    ((release.clamp(0.0, 1.0) - 0.5) * 2.0 * RELEASE_OCTAVES).exp2()
}

/// A note filter's tuning as log2 cutoff and Q, the form a slide eases through:
/// [`OPEN_FILTER`] for a note with none.
fn filter_octaves(filtered: bool, tone_hz: f32, shape: NoteShape) -> (f32, f32) {
    if !filtered {
        return OPEN_FILTER;
    }
    let (cutoff, q) = note_filter_tuning(tone_hz, shape.mod_x, shape.mod_y);
    (cutoff.log2(), q.log2())
}

/// A slide easing the next note's expression in: where each part is heading, and how fast.
/// Everything travels on the same exponential as the pitch, so the new note's level, pan,
/// filter, detune and release all arrive together with it instead of on its first sample.
#[derive(Debug, Clone, Copy)]
struct Ease {
    rate: f32,
    /// How much of the way is still to go, from 1; the ease snaps home below [`EASE_DONE`].
    left: f32,
    velocity: f32,
    pan: f32,
    fine: f32,
    release_scale: f32,
    /// The note filter's tuning in octaves (see [`Voice::filter_at`]), and whether the filter
    /// goes once it gets there because the new note has none.
    filter_at: (f32, f32),
    filter_off: bool,
}

/// What is left of an ease when it snaps to its target: -60 dB of the step, about 7τ.
const EASE_DONE: f32 = 1e-3;

/// One entry on the one-voice modes' held-key stack.
#[derive(Debug, Clone, Copy, Default)]
struct Key {
    pitch: u8,
    /// A sequenced note's tag; `None` for a live key, which is matched by pitch.
    tag: Option<VoiceTag>,
}

#[derive(Debug, Clone)]
struct Voice {
    /// The held key; `None` once released.
    pitch: Option<u8>,
    /// The sequenced note this voice is playing, so that note's own end, or a slide out of it,
    /// finds exactly this voice. `None` for a live key (released by pitch) and once released.
    tag: Option<VoiceTag>,
    started_at: u64,
    /// 0–1
    velocity: f32,
    /// Note pitch in semitones and where the glide currently is.
    target: f32,
    glide: f32,
    /// How fast `glide` chases `target` (1/τ per sample): set by each slide from its own time.
    glide_rate: f32,
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
    /// Equal-power gains from the note's `pan`; (1, 1) when it is centred.
    gains: (f32, f32),
    /// The note's Mod X / Mod Y filter, downstream of the patch's own. `None` at neutral, and
    /// then nothing at all sits in that spot of the path.
    note_filter: Option<NoteFilter>,
    /// The note's `fine` detune in semitones, added to its pitch.
    fine: f32,
    /// What the note's `release` multiplies the patch's tail by; 1 at neutral. Kept so that a
    /// live `set_patch` can re-apply it over the new patch's release.
    release_scale: f32,
    /// The note's pan as the voice plays it, and its note filter's cutoff and Q as log2 values
    /// ([`OPEN_FILTER`] when it has none): what a slide eases from.
    pan: f32,
    filter_at: (f32, f32),
    /// Set while a slide eases the new note's expression in.
    ease: Option<Ease>,
    /// Velocity and pan gains at the start of the block being rendered, which `finish` ramps
    /// from. The same as the current ones unless a slide is easing them in.
    block_from: (f32, (f32, f32)),
}

impl Voice {
    fn new(p: &SynthPatch, sr: f32, seed: u32) -> Self {
        Voice {
            pitch: None,
            tag: None,
            started_at: 0,
            velocity: 0.0,
            target: 60.0,
            glide: 60.0,
            glide_rate: 0.0,
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
            gains: (1.0, 1.0),
            note_filter: None,
            fine: 0.0,
            release_scale: 1.0,
            pan: 0.5,
            filter_at: OPEN_FILTER,
            ease: None,
            block_from: (0.0, (1.0, 1.0)),
        }
    }

    /// Start a note, at its own pitch. A free voice (`reset`) starts clean; a stolen one keeps
    /// its phases, filter state and envelope levels — slamming any of them to zero while it
    /// still sounds would be a step, i.e. an audible click — and its attack ramps on from
    /// wherever the old note had got to.
    fn start(
        &mut self,
        p: &SynthPatch,
        pitch: u8,
        velocity: u8,
        tag: Option<VoiceTag>,
        reset: bool,
        at: u64,
    ) {
        if reset {
            self.amp_env.reset();
            self.filter_env.reset();
            for (o, op) in p.osc.iter().enumerate() {
                for ph in &mut self.phase[o] {
                    *ph = if op.random_phase {
                        self.rng.unit()
                    } else {
                        0.0
                    };
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
        self.tag = tag;
        self.started_at = at;
        self.velocity = velocity as f32 / 127.0;
        self.ease = None;
        self.jump_to(pitch);
        self.pitch_env = 1.0;
        self.amp_env.gate_on();
        self.filter_env.gate_on();
    }

    /// Move to `pitch` at once. The glide smoother then has nothing to cover; only a slide
    /// sets a target it has to travel to.
    fn jump_to(&mut self, pitch: u8) {
        self.target = pitch as f32;
        self.glide = self.target;
    }

    /// Point the voice at one note's own expression. Separate from [`Voice::start`] because
    /// Legato re-shapes a voice it deliberately does not restart.
    fn shape(&mut self, p: &SynthPatch, shape: NoteShape, tone_hz: f32, sr: f32) {
        self.ease = None;
        self.pan = shape.pan;
        self.gains = note_pan_gains(shape.pan);
        self.fine = shape.fine as f32 / 100.0;
        self.release_scale = release_scale(shape.release);
        self.amp_env
            .set_release(p.amp_env.release * self.release_scale);
        self.note_filter = note_filter(sr, tone_hz, shape.mod_x, shape.mod_y);
        self.filter_at = filter_octaves(self.note_filter.is_some(), tone_hz, shape);
    }

    /// Hand this voice to the next note of a rope: retag it, glide to `pitch` over `secs` and
    /// ease the new note's expression in over the same time. Nothing restarts — no envelope,
    /// LFO or pitch-envelope retrigger — so the two notes sound as one. At 0 seconds the pitch
    /// jumps and the expression lands at once, still without an attack.
    #[allow(clippy::too_many_arguments)]
    fn slide_to(
        &mut self,
        p: &SynthPatch,
        pitch: u8,
        velocity: u8,
        shape: NoteShape,
        tag: Option<VoiceTag>,
        secs: f32,
        at: u64,
        tone_hz: f32,
        sr: f32,
    ) {
        self.pitch = Some(pitch);
        self.tag = tag;
        self.started_at = at;
        let rate = slide_rate(secs, sr);
        if rate <= 0.0 {
            self.jump_to(pitch);
            self.velocity = velocity as f32 / 127.0;
            self.shape(p, shape, tone_hz, sr);
            return;
        }
        self.target = pitch as f32;
        self.glide_rate = rate;
        let filtered = note_filter(sr, tone_hz, shape.mod_x, shape.mod_y).is_some();
        if filtered && self.note_filter.is_none() {
            // No filter on the way out: fade one in from wide open, where it is as good as
            // absent.
            self.filter_at = OPEN_FILTER;
            self.note_filter = Some(NoteFilter::lowpass(
                sr,
                OPEN_FILTER.0.exp2(),
                OPEN_FILTER.1.exp2(),
            ));
        }
        self.ease = Some(Ease {
            rate,
            left: 1.0,
            velocity: velocity as f32 / 127.0,
            pan: shape.pan,
            fine: shape.fine as f32 / 100.0,
            release_scale: release_scale(shape.release),
            filter_at: filter_octaves(filtered, tone_hz, shape),
            filter_off: !filtered,
        });
    }

    /// Move a slide's ease on by one block of `n` samples, snapping home at the end.
    fn ease_block(&mut self, p: &SynthPatch, n: f32, sr: f32) {
        let Some(mut e) = self.ease else {
            return;
        };
        let k = 1.0 - (-n * e.rate).exp();
        e.left *= 1.0 - k;
        if e.left < EASE_DONE {
            self.velocity = e.velocity;
            self.pan = e.pan;
            self.fine = e.fine;
            self.release_scale = e.release_scale;
            self.filter_at = e.filter_at;
            if e.filter_off {
                self.note_filter = None;
            }
            self.ease = None;
        } else {
            let mv = |a: &mut f32, b: f32| *a += (b - *a) * k;
            mv(&mut self.velocity, e.velocity);
            mv(&mut self.pan, e.pan);
            mv(&mut self.fine, e.fine);
            mv(&mut self.release_scale, e.release_scale);
            mv(&mut self.filter_at.0, e.filter_at.0);
            mv(&mut self.filter_at.1, e.filter_at.1);
            self.ease = Some(e);
        }
        self.gains = note_pan_gains(self.pan);
        self.amp_env
            .set_release(p.amp_env.release * self.release_scale);
        if let Some(f) = &mut self.note_filter {
            f.retune(sr, self.filter_at.0.exp2(), self.filter_at.1.exp2());
        }
    }

    fn release(&mut self) {
        self.amp_env.gate_off();
        self.filter_env.gate_off();
        self.pitch = None;
        self.tag = None;
    }

    /// Render one block (`out_l.len()` ≤ [`BLOCK`]), adding into `out_l` / `out_r`.
    fn render(
        &mut self,
        p: &SynthPatch,
        d: &Derived,
        c: &Ctx,
        sr: f32,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = out_l.len();
        let nf = n as f32;
        self.block_from = (self.velocity, self.gains);
        self.ease_block(p, nf, sr);

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
        if self.glide != self.target {
            let k = if self.glide_rate > 0.0 {
                1.0 - (-nf * self.glide_rate).exp()
            } else {
                1.0
            };
            self.glide += (self.target - self.glide) * k;
            if (self.target - self.glide).abs() < 1e-4 {
                self.glide = self.target;
            }
        }
        let hz = midi_to_hz(self.glide + p.pitch_env_amount * self.pitch_env + m_pitch + self.fine);
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
                    pm[i] =
                        FM_DEPTH * (ramp(from.fm, to.fm, i, n) + m_fm).clamp(0.0, 1.0) * modsig[i];
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
            run_osc(
                p.osc[2].wave,
                pw[2],
                &mut self.phase[2][..u],
                &d.inc[2][..u],
                &hzs,
                None,
                None,
                &mut buf[..n],
            );
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
            FilterMode::Off => self.finish(p, d, &mix, m_amp, out_l, out_r, |_, x| x),
            FilterMode::Lowpass12 if open => {
                self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| {
                    f[0].tick(x);
                    x
                })
            }
            FilterMode::Lowpass24 if open => {
                self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| {
                    let y = f[0].tick(x).low;
                    f[1].tick(y);
                    x
                })
            }
            FilterMode::Lowpass12 => {
                self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| f[0].tick(x).low)
            }
            FilterMode::Lowpass24 => self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| {
                let y = f[0].tick(x).low;
                f[1].tick(y).low
            }),
            FilterMode::Highpass12 => {
                self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| f[0].tick(x).high)
            }
            FilterMode::Bandpass => {
                self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| f[0].tick(x).band)
            }
            FilterMode::Notch => self.finish(p, d, &mix, m_amp, out_l, out_r, |f, x| {
                let o = f[0].tick(x);
                o.low + o.high
            }),
        }
    }

    /// Per sample: pre-drive → patch filter → the note's own filter → safety limit → amp
    /// envelope × velocity × amp LFO → post-drive → the note's pan, added into the pair.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn finish(
        &mut self,
        p: &SynthPatch,
        d: &Derived,
        mix: &[f32; BLOCK],
        m_amp: f32,
        out_l: &mut [f32],
        out_r: &mut [f32],
        mut filt: impl FnMut(&mut [Svf; 2], f32) -> f32,
    ) {
        let n = out_l.len();
        // Both ends of the block: the same value unless a slide is easing them in, and then
        // `ramp` returns it exactly, so an unslid note renders as it always has.
        let vel_gain = |v: f32| 1.0 - p.velocity * (1.0 - v);
        let (v0, (gl0, gr0)) = self.block_from;
        let (vg0, vg) = (vel_gain(v0), vel_gain(self.velocity));
        let (pre, (pg, pn)) = (p.filter.drive, d.pre_drive);
        let (post, (qg, qn)) = (p.drive, d.post_drive);
        let a0 = self.last_amp;
        let (gl, gr) = self.gains;
        for i in 0..n {
            let mut x = mix[i];
            if pre > 0.0 {
                x = drive(x, pre, pg, pn);
            }
            let mut y = filt(&mut self.filter, x);
            if let Some(nf) = &mut self.note_filter {
                y = nf.process(y);
            }
            let y = soft_limit(y, 2.0);
            let mut y = y * self.amp_env.tick() * ramp(vg0, vg, i, n) * ramp(a0, m_amp, i, n);
            if post > 0.0 {
                y = drive(y, post, qg, qn);
            }
            // Unity gains at centre, so an unpanned note lands on both sides untouched.
            out_l[i] += y * ramp(gl0, gl, i, n);
            out_r[i] += y * ramp(gr0, gr, i, n);
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
    /// The one-voice modes' held keys, oldest first.
    held: [Key; MONO_STACK],
    held_len: usize,
    /// The track's tone cutoff, which is what a note's Mod X sweeps around.
    tone_hz: f32,
    /// One instance per side: the voices place themselves from their own pan, so the two
    /// sides stop being the same signal the moment a note is panned off centre.
    crusher: [Crusher; 2],
    chorus: [Chorus; 2],
    echo: [Echo; 2],
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
            .map(|i| {
                Voice::new(
                    &patch,
                    sample_rate,
                    0x2545_F491 ^ (i + 1).wrapping_mul(0x9E37_79B9),
                )
            })
            .collect();
        let mut lfo = [Lfo::default(); 2];
        lfo.iter_mut().for_each(|l| l.reset(&mut rng));
        let mut echo = [
            Echo::new(sample_rate, MAX_DELAY_S),
            Echo::new(sample_rate, MAX_DELAY_S),
        ];
        echo.iter_mut().for_each(|e| e.reset(patch.delay.time));
        PolySynth {
            knobs: Knobs::target(&patch, &derived),
            patch,
            derived,
            sample_rate,
            voices,
            counter: 0,
            lfo,
            rng,
            held: [Key::default(); MONO_STACK],
            held_len: 0,
            tone_hz: DEFAULT_TONE_HZ,
            crusher: [Crusher::default(); 2],
            chorus: [Chorus::new(sample_rate), Chorus::new(sample_rate)],
            echo,
            chorus_stale: false,
            echo_stale: false,
            quiet_run: usize::MAX / 2,
        }
    }

    pub fn patch(&self) -> &SynthPatch {
        &self.patch
    }

    /// The cutoff a note's Mod X is measured against. The channel pushes its tone down here
    /// whenever the track changes, so a note only ever carries its *offset* from the track's
    /// own voicing rather than an absolute frequency.
    pub fn set_tone_hz(&mut self, hz: f32) {
        self.tone_hz = hz;
    }

    /// Change the sound live. Sounding voices carry on with the new parameters (levels and
    /// effect mixes glide over a few ms); nothing is cut or retriggered.
    pub fn set_patch(&mut self, patch: &SynthPatch) {
        let patch = patch.sanitized();
        if patch.voice.is_mono() != self.patch.voice.is_mono() {
            // Keys held in the other mode are released through the poly path.
            self.held_len = 0;
        }
        self.patch = patch;
        self.derived = Derived::new(&patch, self.sample_rate);
        for v in &mut self.voices {
            v.amp_env.set_params(adsr(&patch.amp_env));
            v.filter_env.set_params(adsr(&patch.filter_env));
            // `set_params` just took the patch's own tail; put the note's scaling back over it.
            v.amp_env
                .set_release(patch.amp_env.release * v.release_scale);
        }
    }

    /// Start a live key: the controller or typing keyboard, the chord bed, an audition. It
    /// carries no tag, so [`PolySynth::note_off`] finds it again by pitch.
    pub fn note_on(&mut self, pitch: u8, velocity: u8, shape: NoteShape) {
        self.play(pitch, velocity, shape, None, None);
    }

    /// Start a sequenced note, which [`PolySynth::note_off_tagged`] releases by `tag`. With a
    /// `slide`, the note takes over the voice playing the note the rope comes from, in any
    /// voice mode, and glides it here; when no voice is playing that note any more (released,
    /// stolen or never started) it plays as an ordinary note.
    pub fn note_on_tagged(
        &mut self,
        pitch: u8,
        velocity: u8,
        shape: NoteShape,
        tag: VoiceTag,
        slide: Option<Slide>,
    ) {
        self.play(pitch, velocity, shape, Some(tag), slide);
    }

    fn play(
        &mut self,
        pitch: u8,
        velocity: u8,
        shape: NoteShape,
        tag: Option<VoiceTag>,
        slide: Option<Slide>,
    ) {
        self.counter += 1;
        if let Some(slide) = slide {
            if self.slide(pitch, velocity, shape, tag, slide) {
                return;
            }
        }
        if self.patch.voice.is_mono() {
            self.mono_on(pitch, velocity, shape, tag);
        } else {
            let idx = match self
                .voices
                .iter()
                .position(|v| v.pitch.is_none() && !v.amp_env.is_active())
            {
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
            let (tone_hz, sr) = (self.tone_hz, self.sample_rate);
            let v = &mut self.voices[idx];
            let reset = !v.amp_env.is_active();
            v.start(&self.patch, pitch, velocity, tag, reset, self.counter);
            v.shape(&self.patch, shape, tone_hz, sr);
        }
    }

    /// Hand the voice playing `slide.from` to this note; false when no voice is.
    fn slide(
        &mut self,
        pitch: u8,
        velocity: u8,
        shape: NoteShape,
        tag: Option<VoiceTag>,
        slide: Slide,
    ) -> bool {
        let mono = self.patch.voice.is_mono();
        if mono {
            // The scheduler drops the source's own note-off once a rope takes it over, so its
            // key comes off the stack here, whether or not there is still a voice to take.
            self.unhold(|k| k.tag == Some(slide.from));
        }
        let Some(i) = self
            .voices
            .iter()
            .position(|v| v.pitch.is_some() && v.tag == Some(slide.from))
        else {
            return false;
        };
        if mono {
            // The slide note is the key now held: its own end releases the voice, and a newer
            // key letting go falls back to it.
            self.hold(Key { pitch, tag });
        }
        let secs = slide
            .secs
            .unwrap_or(self.patch.glide)
            .clamp(0.0, MAX_SLIDE_SECS);
        let (tone_hz, sr, at) = (self.tone_hz, self.sample_rate, self.counter);
        self.voices[i].slide_to(
            &self.patch,
            pitch,
            velocity,
            shape,
            tag,
            secs,
            at,
            tone_hz,
            sr,
        );
        true
    }

    /// Mono and Legato: one voice, last-note priority, and the pitch always jumps. In Legato a
    /// key pressed while another is held (and still sounding) takes the voice over with no
    /// new attack; otherwise, and always in Mono, the note replays its attack from wherever
    /// the envelope had got to.
    fn mono_on(&mut self, pitch: u8, velocity: u8, shape: NoteShape, tag: Option<VoiceTag>) {
        self.unhold(|k| k.tag == tag && (tag.is_some() || k.pitch == pitch));
        let overlapping = self.held_len > 0;
        self.hold(Key { pitch, tag });
        let (tone_hz, sr) = (self.tone_hz, self.sample_rate);
        let v = &mut self.voices[0];
        let sounding = v.amp_env.is_active();
        if overlapping && sounding && self.patch.voice == VoiceMode::Legato {
            v.pitch = Some(pitch);
            v.tag = tag;
            v.started_at = self.counter;
            v.jump_to(pitch);
        } else {
            v.start(&self.patch, pitch, velocity, tag, !sounding, self.counter);
        }
        // A connected note takes the new key's expression too: it is the note that is sounding.
        v.shape(&self.patch, shape, tone_hz, sr);
    }

    /// Push a key on top of the held stack, forgetting the oldest when it is full.
    fn hold(&mut self, key: Key) {
        if self.held_len == MONO_STACK {
            self.held.copy_within(1.., 0);
            self.held_len -= 1;
        }
        self.held[self.held_len] = key;
        self.held_len += 1;
    }

    /// Drop the first held key that `is` picks; whether there was one.
    fn unhold(&mut self, is: impl Fn(&Key) -> bool) -> bool {
        match self.held[..self.held_len].iter().position(is) {
            Some(i) => {
                self.held.copy_within(i + 1..self.held_len, i);
                self.held_len -= 1;
                true
            }
            None => false,
        }
    }

    /// The one voice's key just let go: fall back to the most recent key still held, without
    /// a new attack, or release.
    fn fall_back(&mut self) {
        let v = &mut self.voices[0];
        if self.held_len > 0 {
            let prev = self.held[self.held_len - 1];
            v.pitch = Some(prev.pitch);
            v.tag = prev.tag;
            v.jump_to(prev.pitch);
        } else {
            v.release();
        }
    }

    /// Release a live key: *one* voice, the oldest untagged one still holding `pitch`. Note-ons
    /// and note-offs are 1:1, so releasing every matching voice would make two overlapping
    /// notes of the same pitch cut each other short — the first note's end would silence the
    /// second. Sequenced notes are left alone: they are released by tag.
    pub fn note_off(&mut self, pitch: u8) {
        let live = |tag: Option<VoiceTag>, p: Option<u8>| tag.is_none() && p == Some(pitch);
        if self.patch.voice.is_mono() && self.unhold(|k| live(k.tag, Some(k.pitch))) {
            if live(self.voices[0].tag, self.voices[0].pitch) {
                self.fall_back();
            }
            return;
        }
        let oldest = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| live(v.tag, v.pitch))
            .min_by_key(|(_, v)| v.started_at)
            .map(|(i, _)| i);
        if let Some(i) = oldest {
            self.voices[i].release();
        }
    }

    /// Release the sequenced note `tag`. Once a slide has taken its voice over the voice
    /// answers to the slide note instead, so a stray off for the old note releases nothing.
    pub fn note_off_tagged(&mut self, tag: VoiceTag) {
        if self.patch.voice.is_mono()
            && self.unhold(|k| k.tag == Some(tag))
            && self.voices[0].tag == Some(tag)
        {
            self.fall_back();
            return;
        }
        // Poly, or a voice left over from before a switch to a one-voice mode.
        if let Some(v) = self
            .voices
            .iter_mut()
            .find(|v| v.pitch.is_some() && v.tag == Some(tag))
        {
            v.release();
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
            t += self.chorus[0].tail();
        }
        if p.delay.mix > 0.0 || self.knobs.delay > 0.0 {
            t += self.echo[0].time_samples() as usize + 64;
        }
        t
    }

    /// Render into `out_l` / `out_r` (additive). Each voice is placed by its own note's pan;
    /// the channel's pan and tone sit downstream of this.
    pub fn render_add(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let frames = out_l.len().min(out_r.len());
        let pairs = out_l[..frames]
            .chunks_mut(BLOCK)
            .zip(out_r[..frames].chunks_mut(BLOCK));
        for (l, r) in pairs {
            let voices_on = !self.voices_idle();
            if !voices_on && self.quiet_run >= self.tail_len() {
                // Silent until the next note: settle the knobs so it starts at their levels.
                self.knobs = Knobs::target(&self.patch, &self.derived);
                return;
            }
            self.render_block(l, r, voices_on);
        }
    }

    fn render_block(&mut self, out_l: &mut [f32], out_r: &mut [f32], voices_on: bool) {
        let n = out_l.len();
        let nf = n as f32;
        let sr = self.sample_rate;
        let d = self.derived;
        let from = self.knobs;
        let mut to = from;
        to.approach(
            &Knobs::target(&self.patch, &d),
            (nf / (SMOOTH_S * sr)).min(1.0),
        );
        self.knobs = to;
        let ctx = Ctx {
            from,
            to,
            pitch_env_mul: (-nf * d.pitch_env_rate).exp(),
            free_lfo: [0, 1].map(|l| self.lfo[l].value(self.patch.lfo[l].wave)),
        };
        for l in 0..2 {
            self.lfo[l].advance(d.lfo_inc[l] * nf, &mut self.rng);
        }

        let mut sum_l = [0.0f32; BLOCK];
        let mut sum_r = [0.0f32; BLOCK];
        let p = &self.patch;
        if voices_on {
            for v in self.voices.iter_mut().filter(|v| v.amp_env.is_active()) {
                v.render(p, &d, &ctx, sr, &mut sum_l[..n], &mut sum_r[..n]);
            }
        }

        // ── Post: crush → level → chorus → echo, once per side ──
        if p.crush > 0.0 {
            let (ratio, steps) = d.crush;
            for i in 0..n {
                sum_l[i] = self.crusher[0].process(sum_l[i], ratio, steps);
                sum_r[i] = self.crusher[1].process(sum_r[i], ratio, steps);
            }
        }
        let chorus_on = from.chorus > 0.0 || to.chorus > 0.0;
        if !chorus_on {
            self.chorus_stale = true;
        } else if self.chorus_stale {
            self.chorus.iter_mut().for_each(Chorus::clear);
            self.chorus_stale = false;
        }
        let echo_on = from.delay > 0.0 || to.delay > 0.0;
        if !echo_on {
            self.echo_stale = true;
        } else if self.echo_stale {
            self.echo.iter_mut().for_each(|e| e.reset(p.delay.time));
            self.echo_stale = false;
        }
        let mut peak = 0.0f32;
        for i in 0..n {
            let level = ramp(from.level, to.level, i, n);
            let chorus_mix = ramp(from.chorus, to.chorus, i, n);
            let delay_mix = ramp(from.delay, to.delay, i, n);
            let mut side = [sum_l[i] * level, sum_r[i] * level];
            for (s, y) in side.iter_mut().enumerate() {
                if chorus_on {
                    *y = self.chorus[s].process(*y, p.chorus.rate, p.chorus.depth, chorus_mix);
                }
                if echo_on {
                    *y = self.echo[s].process(*y, p.delay.time, p.delay.feedback, delay_mix);
                }
                if !y.is_finite() {
                    // Never let a NaN reach the mix bus; drop whatever the effects were holding.
                    *y = 0.0;
                    self.chorus_stale = true;
                    self.echo_stale = true;
                }
                peak = peak.max(y.abs());
            }
            out_l[i] += side[0];
            out_r[i] += side[1];
        }
        self.quiet_run = if voices_on || peak >= QUIET {
            0
        } else {
            self.quiet_run.saturating_add(n)
        };
    }
}

/// One voice as a test sees it: where its pitch is, which note it plays and how loud its
/// envelope is. Lets the scheduler's tests watch ropes land in the synth.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct VoiceProbe {
    pub glide: f32,
    pub target: f32,
    pub tag: Option<VoiceTag>,
    pub level: f32,
}

#[cfg(test)]
impl PolySynth {
    /// Every voice holding a note (not releasing), oldest first.
    pub(crate) fn held_voices(&self) -> Vec<VoiceProbe> {
        let mut held: Vec<&Voice> = self.voices.iter().filter(|v| v.pitch.is_some()).collect();
        held.sort_by_key(|v| v.started_at);
        held.iter()
            .map(|v| VoiceProbe {
                glide: v.glide,
                target: v.target,
                tag: v.tag,
                level: v.amp_env.level(),
            })
            .collect()
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

    fn render_stereo(s: &mut PolySynth, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        // Odd-sized chunks, like the engine's event-split sub-blocks.
        for (cl, cr) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
            s.render_add(cl, cr);
        }
        (l, r)
    }

    /// The left side, which is the whole signal for the centred notes most tests play.
    fn render(s: &mut PolySynth, frames: usize) -> Vec<f32> {
        render_stereo(s, frames).0
    }

    fn zero_crossings(xs: &[f32]) -> usize {
        xs.windows(2)
            .filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0))
            .count()
    }

    /// Energy above roughly a quarter of Nyquist: a first difference is a crude high-pass, and
    /// that is all a "is this duller?" comparison needs.
    fn bright_rms(xs: &[f32]) -> f32 {
        rms(&xs.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>())
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
        s.note_on(60, 100, NoteShape::neutral());
        let buf = render(&mut s, 4800);
        assert!(buf.iter().any(|x| x.abs() > 0.01));
        s.note_off(60);
        render(&mut s, 48_000);
        assert!(s.is_silent());
    }

    #[test]
    fn steals_oldest_voice_when_full() {
        let mut s = PolySynth::new(&factory("pad"), SR, 2);
        s.note_on(60, 100, NoteShape::neutral());
        s.note_on(64, 100, NoteShape::neutral());
        s.note_on(67, 100, NoteShape::neutral());
        let sounding: Vec<u8> = s.voices.iter().filter_map(|v| v.pitch).collect();
        assert_eq!(sounding.len(), 2);
        assert!(!sounding.contains(&60));
    }

    #[test]
    fn velocity_scales_amplitude() {
        let render_at = |vel: u8| {
            let mut s = PolySynth::new(&factory("sine"), SR, 1);
            s.note_on(69, vel, NoteShape::neutral());
            peak(&render(&mut s, 9600))
        };
        assert!(render_at(127) > render_at(40) * 2.0);
    }

    #[test]
    fn renders_are_deterministic() {
        let go = || {
            let mut s = PolySynth::new(&kitchen_sink(), SR, 4);
            s.note_on(48, 110, NoteShape::neutral());
            s.note_on(55, 90, NoteShape::neutral());
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
            s.note_on(57, 100, NoteShape::neutral());
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
            let SoundPatch::Synth(p) = &sound.patch else {
                continue;
            };
            n += 1;
            let mut s = PolySynth::new(p, SR, 12);
            for pitch in [48, 55, 60, 64, 67] {
                s.note_on(pitch, 127, NoteShape::neutral());
            }
            let mut buf = render(&mut s, 48_000);
            for pitch in [48, 55, 60, 64, 67] {
                s.note_off(pitch);
            }
            buf.extend(render(&mut s, 24_000));
            assert!(
                buf.iter().all(|x| x.is_finite()),
                "{}: non-finite output",
                sound.id
            );
            assert!(rms(&buf) > 1e-3, "{}: silent", sound.id);
            assert!(peak(&buf) < 4.0, "{}: peak {}", sound.id, peak(&buf));
        }
        assert!(n >= 6);
    }

    #[test]
    fn set_patch_mid_note_neither_clicks_nor_cuts() {
        let mut p = factory("sine");
        let mut s = PolySynth::new(&p, SR, 4);
        s.note_on(57, 100, NoteShape::neutral());
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
        let max_step = joined
            .windows(2)
            .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(max_step < 0.06, "click: step {max_step}");
    }

    /// Zero-crossing rate of a sine voice in a window → its frequency.
    fn freq(buf: &[f32]) -> f32 {
        let ups = buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        ups as f32 * SR / buf.len() as f32
    }

    /// A sine patch whose envelope settles well below its peak, so a replayed attack shows up
    /// as the level climbing back towards 1. Glide is up, to prove nothing reads it.
    fn voiced(voice: VoiceMode) -> SynthPatch {
        let mut p = factory("sine");
        p.voice = voice;
        p.glide = 0.3;
        p.amp_env.attack = 0.02;
        p.amp_env.decay = 0.02;
        p.amp_env.sustain = 0.3;
        p
    }

    /// The amp envelope's highest level over the next `frames`, rendered in small steps.
    fn env_peak(s: &mut PolySynth, frames: usize) -> f32 {
        let mut top = 0.0f32;
        for _ in 0..frames / BLOCK {
            render(s, BLOCK);
            top = top.max(
                s.voices
                    .iter()
                    .map(|v| v.amp_env.level())
                    .fold(0.0, f32::max),
            );
        }
        top
    }

    #[test]
    fn legato_connects_held_notes_and_jumps_without_gliding() {
        let mut s = PolySynth::new(&voiced(VoiceMode::Legato), SR, 4);
        s.note_on(57, 100, NoteShape::neutral()); // 220 Hz
        let a = render(&mut s, 9_600);
        s.note_on(69, 100, NoteShape::neutral()); // 440 Hz, while 57 is held
        assert!(env_peak(&mut s, 2_400) < 0.35, "legato replayed the attack");
        // The pitch is already there: no portamento, even with glide set.
        let early = render(&mut s, 9_600);
        assert!((freq(&a) - 220.0).abs() < 10.0, "start {}", freq(&a));
        assert!(
            (freq(&early) - 440.0).abs() < 10.0,
            "early {}",
            freq(&early)
        );
        assert_eq!(s.voices.iter().filter(|v| v.amp_env.is_active()).count(), 1);
        // Releasing the top note falls back to the held one, at once and without an attack.
        s.note_off(69);
        assert!(
            env_peak(&mut s, 2_400) < 0.35,
            "falling back replayed the attack"
        );
        assert!((freq(&render(&mut s, 9_600)) - 220.0).abs() < 10.0);
        s.note_off(57);
        render(&mut s, 48_000);
        assert!(s.is_silent());
    }

    #[test]
    fn mono_replays_the_attack_on_every_note_and_jumps() {
        let mut s = PolySynth::new(&voiced(VoiceMode::Mono), SR, 4);
        s.note_on(57, 100, NoteShape::neutral());
        render(&mut s, 9_600);
        s.note_on(69, 100, NoteShape::neutral()); // held over 57, but Mono re-attacks
        assert!(
            env_peak(&mut s, 2_400) > 0.9,
            "mono did not replay the attack"
        );
        assert!((freq(&render(&mut s, 9_600)) - 440.0).abs() < 10.0);
        assert_eq!(
            s.voices.iter().filter(|v| v.amp_env.is_active()).count(),
            1,
            "one voice"
        );
        // The older key still held takes over when the newer one lets go.
        s.note_off(69);
        render(&mut s, 2_400);
        assert!((freq(&render(&mut s, 9_600)) - 220.0).abs() < 10.0);
        s.note_off(57);
        render(&mut s, 48_000);
        assert!(s.is_silent());
    }

    #[test]
    fn poly_notes_start_at_their_own_pitch_with_glide_set() {
        let mut s = PolySynth::new(&voiced(VoiceMode::Poly), SR, 4);
        s.note_on(57, 100, NoteShape::neutral());
        render(&mut s, 2_400);
        s.note_off(57);
        render(&mut s, 24_000);
        // Poly portamento would glide this in from 220 Hz over 0.3 s.
        s.note_on(69, 100, NoteShape::neutral());
        assert!(env_peak(&mut s, 1_200) > 0.9, "a poly note attacks");
        assert!((freq(&render(&mut s, 9_600)) - 440.0).abs() < 10.0);
    }

    /// The three modes start a lone note the same way, so only overlaps tell them apart.
    #[test]
    fn a_lone_note_sounds_the_same_in_every_mode() {
        let go = |voice| {
            let mut s = PolySynth::new(&voiced(voice), SR, 4);
            s.note_on(60, 100, NoteShape::neutral());
            let mut buf = render(&mut s, 4_800);
            s.note_off(60);
            buf.extend(render(&mut s, 4_800));
            buf
        };
        let poly = go(VoiceMode::Poly);
        assert_eq!(go(VoiceMode::Mono), poly);
        assert_eq!(go(VoiceMode::Legato), poly);
    }

    // ── Slides ──

    const A: VoiceTag = VoiceTag(1);
    const B: VoiceTag = VoiceTag(2);
    const C: VoiceTag = VoiceTag(3);

    fn slide(from: VoiceTag, secs: Option<f32>) -> Option<Slide> {
        Some(Slide { from, secs })
    }

    /// The pitch of the one voice holding `tag`.
    fn glide_of(s: &PolySynth, tag: VoiceTag) -> f32 {
        let v = s
            .voices
            .iter()
            .find(|v| v.tag == Some(tag))
            .expect("a voice plays it");
        v.glide
    }

    #[test]
    fn a_slide_takes_the_voice_over_and_glides_without_an_attack_in_every_mode() {
        for mode in [VoiceMode::Poly, VoiceMode::Mono, VoiceMode::Legato] {
            let mut s = PolySynth::new(&voiced(mode), SR, 4);
            s.note_on_tagged(57, 100, NoteShape::neutral(), A, None);
            render(&mut s, 9_600);
            s.note_on_tagged(69, 100, NoteShape::neutral(), B, slide(A, Some(0.2)));
            render(&mut s, BLOCK);
            let early = glide_of(&s, B);
            assert!(
                early > 57.0 && early < 60.0,
                "{mode:?}: starts from the source, {early}"
            );
            let mut last = early;
            let mut top = 0.0f32;
            for _ in 0..(SR as usize / 20) / BLOCK {
                render(&mut s, BLOCK);
                let g = glide_of(&s, B);
                assert!(
                    g >= last && g - last < 1.0,
                    "{mode:?}: moves gradually ({last} → {g})"
                );
                last = g;
                top = s
                    .voices
                    .iter()
                    .map(|v| v.amp_env.level())
                    .fold(top, f32::max);
            }
            assert!(
                last > 60.0 && last < 68.0,
                "{mode:?}: on its way after 50 ms, {last}"
            );
            assert!(
                top < 0.35,
                "{mode:?}: the slide replayed the attack ({top})"
            );
            render(&mut s, 19_200);
            assert!(
                (freq(&render(&mut s, 9_600)) - 440.0).abs() < 10.0,
                "{mode:?}: arrives"
            );
            assert_eq!(s.held_voices().len(), 1, "{mode:?}: one voice");
            // The source's off would be held back by the scheduler; a stray one is harmless.
            s.note_off_tagged(A);
            assert_eq!(
                s.held_voices().len(),
                1,
                "{mode:?}: the voice is the slide note's now"
            );
            s.note_off_tagged(B);
            render(&mut s, 48_000);
            assert!(s.is_silent(), "{mode:?}: the slide note's end releases it");
        }
    }

    #[test]
    fn a_chord_slides_only_the_voices_its_ropes_name() {
        let mut s = PolySynth::new(&voiced(VoiceMode::Poly), SR, 6);
        for (pitch, tag) in [(48, A), (52, B), (55, C)] {
            s.note_on_tagged(pitch, 100, NoteShape::neutral(), tag, None);
        }
        render(&mut s, 9_600);
        s.note_on_tagged(
            50,
            100,
            NoteShape::neutral(),
            VoiceTag(4),
            slide(A, Some(0.05)),
        );
        s.note_on_tagged(
            53,
            100,
            NoteShape::neutral(),
            VoiceTag(5),
            slide(B, Some(0.05)),
        );
        render(&mut s, 9_600);
        let held = s.held_voices();
        assert_eq!(held.len(), 3, "no new voice: {held:?}");
        let targets: Vec<(f32, Option<VoiceTag>)> =
            held.iter().map(|v| (v.target, v.tag)).collect();
        assert!(targets.contains(&(50.0, Some(VoiceTag(4)))));
        assert!(targets.contains(&(53.0, Some(VoiceTag(5)))));
        assert_eq!(
            glide_of(&s, C),
            55.0,
            "the third note keeps ringing where it was"
        );
        assert!(
            held.iter().all(|v| v.level < 0.35),
            "nothing re-attacked: {held:?}"
        );
    }

    #[test]
    fn a_slide_with_nothing_to_take_plays_as_an_ordinary_note() {
        for mode in [VoiceMode::Poly, VoiceMode::Mono, VoiceMode::Legato] {
            let mut s = PolySynth::new(&voiced(mode), SR, 4);
            s.note_on_tagged(57, 100, NoteShape::neutral(), A, None);
            render(&mut s, 2_400);
            s.note_off_tagged(A);
            render(&mut s, 2_400);
            // The source is releasing: its voice no longer answers to its tag.
            s.note_on_tagged(69, 100, NoteShape::neutral(), B, slide(A, Some(0.2)));
            assert_eq!(glide_of(&s, B), 69.0, "{mode:?}: starts at its own pitch");
            assert!(env_peak(&mut s, 1_200) > 0.9, "{mode:?}: attacks");
        }
    }

    #[test]
    fn slide_time_comes_from_the_note_or_the_patch_and_zero_jumps() {
        // How far from 57 towards 69 the slide has got after 0.1 s.
        let progress = |secs: Option<f32>| {
            let mut s = PolySynth::new(&voiced(VoiceMode::Poly), SR, 2);
            s.note_on_tagged(57, 100, NoteShape::neutral(), A, None);
            render(&mut s, 2_400);
            s.note_on_tagged(69, 100, NoteShape::neutral(), B, slide(A, secs));
            render(&mut s, 4_800);
            (glide_of(&s, B) - 57.0) / 12.0
        };
        // τ = time / 3: 0.1 s is a whole τ of the patch's 0.3 s glide, a third of one of 0.9 s.
        let patch = progress(None);
        assert!((patch - 0.63).abs() < 0.05, "patch glide: {patch}");
        let slow = progress(Some(0.9));
        assert!((slow - 0.28).abs() < 0.05, "override: {slow}");
        assert!(
            progress(Some(0.03)) > 0.99,
            "a short slide is there already"
        );
        assert!(
            progress(Some(5.0)) > progress(Some(MAX_SLIDE_SECS)) - 1e-6,
            "clamped"
        );

        let mut s = PolySynth::new(&voiced(VoiceMode::Poly), SR, 2);
        s.note_on_tagged(57, 100, NoteShape::neutral(), A, None);
        render(&mut s, 2_400);
        s.note_on_tagged(
            69,
            30,
            NoteShape {
                pan: 0.0,
                ..NoteShape::neutral()
            },
            B,
            slide(A, Some(0.0)),
        );
        assert_eq!(glide_of(&s, B), 69.0, "0 s jumps");
        let v = s.voices.iter().find(|v| v.tag == Some(B)).unwrap();
        assert!(
            v.ease.is_none() && v.gains == note_pan_gains(0.0),
            "and lands at once"
        );
        assert!(env_peak(&mut s, 1_200) < 0.35, "still without an attack");
    }

    #[test]
    fn a_slide_eases_the_new_notes_expression_in() {
        let mut p = voiced(VoiceMode::Poly);
        p.velocity = 1.0;
        p.amp_env.sustain = 1.0;
        let mut s = PolySynth::new(&p, SR, 2);
        s.set_tone_hz(4_000.0);
        s.note_on_tagged(57, 20, NoteShape::neutral(), A, None);
        render(&mut s, 9_600);
        let shape = NoteShape {
            pan: 1.0,
            mod_x: 0.2,
            fine: 50,
            release: 1.0,
            ..NoteShape::neutral()
        };
        s.note_on_tagged(57, 127, shape, B, slide(A, Some(0.2)));
        let voice = |s: &PolySynth| s.voices.iter().find(|v| v.tag == Some(B)).unwrap().clone();
        assert!(
            voice(&s).note_filter.is_some(),
            "a filter fades in from wide open"
        );
        // Over 0.1 s: the velocity block by block, and the right side's level (where the pan is
        // heading) about one cycle of the note at a time.
        let (mut vel, mut right) = (vec![voice(&s).velocity], vec![]);
        for _ in 0..20 {
            let mut r = vec![];
            for _ in 0..15 {
                r.extend(render_stereo(&mut s, BLOCK).1);
                vel.push(voice(&s).velocity);
            }
            right.push(rms(&r));
        }
        let v = voice(&s);
        let step = vel.windows(2).map(|w| w[1] - w[0]).fold(0.0, f32::max);
        assert!(
            vel.windows(2).all(|w| w[1] >= w[0]) && step < 0.01,
            "velocity eases: {step}"
        );
        assert!(
            v.velocity > 0.5 && v.velocity < 0.99,
            "velocity on its way: {}",
            v.velocity
        );
        assert!(v.pan > 0.6 && v.pan < 0.99, "pan on its way: {}", v.pan);
        assert!(
            v.fine > 0.25 && v.fine < 0.5,
            "detune on its way: {}",
            v.fine
        );
        assert!(
            v.release_scale > 1.5 && v.release_scale < 4.0,
            "release on its way"
        );
        // What you hear climbs too, without the jump a stepped level would make up front.
        let jump = right
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        let climb = right[right.len() - 1] - right[0];
        assert!(
            climb > 2.0 * right[0],
            "the right side gets louder: {right:?}"
        );
        assert!(jump < climb / 4.0, "no step: {jump} of {climb}");

        render(&mut s, SR as usize);
        let v = voice(&s);
        assert!(v.ease.is_none(), "arrived");
        assert_eq!(
            (v.velocity, v.pan, v.fine, v.release_scale),
            (1.0, 1.0, 0.5, 4.0)
        );
        assert_eq!(v.gains, note_pan_gains(1.0));

        // And back out to a note with no filter of its own: the filter goes once it is open.
        s.note_on_tagged(57, 127, NoteShape::neutral(), C, slide(B, Some(0.05)));
        render(&mut s, SR as usize / 2);
        let v = s.voices.iter().find(|v| v.tag == Some(C)).unwrap();
        assert!(v.note_filter.is_none() && v.ease.is_none());
        assert_eq!(v.gains, (1.0, 1.0));
    }

    /// The one-voice modes keep a stack of held keys. A rope's source never sends its own off,
    /// so the slide must take its key off the stack, or a later fall-back would land on it.
    #[test]
    fn a_slide_leaves_no_stale_key_on_the_one_voice_stack() {
        for mode in [VoiceMode::Mono, VoiceMode::Legato] {
            let mut s = PolySynth::new(&voiced(mode), SR, 4);
            s.note_on_tagged(57, 100, NoteShape::neutral(), A, None);
            render(&mut s, 2_400);
            s.note_on_tagged(64, 100, NoteShape::neutral(), B, slide(A, Some(0.05)));
            render(&mut s, 2_400);
            s.note_off_tagged(B);
            assert!(
                s.held_voices().is_empty(),
                "{mode:?}: released, not fallen back to 57"
            );

            // The source has already handed the voice to another key: the slide plays plainly
            // (connected or re-attacked as the mode says), and still clears the source's key.
            let mut s = PolySynth::new(&voiced(mode), SR, 4);
            s.note_on_tagged(57, 100, NoteShape::neutral(), A, None);
            s.note_on_tagged(60, 100, NoteShape::neutral(), C, None);
            render(&mut s, 2_400);
            s.note_on_tagged(64, 100, NoteShape::neutral(), B, slide(A, Some(0.05)));
            assert_eq!(glide_of(&s, B), 64.0, "{mode:?}");
            s.note_off_tagged(C);
            assert_eq!(s.held_voices()[0].tag, Some(B), "{mode:?}: B still sounds");
            s.note_off_tagged(B);
            assert!(
                s.held_voices().is_empty(),
                "{mode:?}: nothing left to fall back to"
            );
        }
    }

    /// Live keys and sequenced notes share a synth but never release each other.
    #[test]
    fn live_keys_and_tagged_notes_release_separately() {
        let mut s = PolySynth::new(&voiced(VoiceMode::Poly), SR, 4);
        s.note_on_tagged(60, 100, NoteShape::neutral(), A, None);
        s.note_on(60, 100, NoteShape::neutral());
        s.note_off(60);
        assert_eq!(s.held_voices().len(), 1);
        assert_eq!(
            s.held_voices()[0].tag,
            Some(A),
            "the key-up released the live key"
        );
        s.note_off_tagged(A);
        assert!(s.held_voices().is_empty());
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
        s.note_on(69, 127, NoteShape::neutral());
        render(&mut s, 4_800);
        let buf = render(&mut s, 48_000);
        let windows: Vec<f32> = buf.chunks(480).map(rms).collect();
        let (lo, hi) = windows
            .iter()
            .fold((f32::MAX, 0.0f32), |(lo, hi), &w| (lo.min(w), hi.max(w)));
        assert!(hi > 0.05 && lo < hi * 0.1, "tremolo range {lo}..{hi}");
    }

    #[test]
    fn everything_on_stays_finite_and_bounded() {
        let mut s = PolySynth::new(&kitchen_sink(), SR, 12);
        for (i, pitch) in [24, 36, 48, 55, 60, 64, 67, 72, 79, 84, 96, 108]
            .into_iter()
            .enumerate()
        {
            s.note_on(pitch, 40 + i as u8 * 7, NoteShape::neutral());
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
                s.note_on(pitch, 127, NoteShape::neutral());
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
        s.note_on(60, 127, NoteShape::neutral());
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
        for (i, pitch) in (48..60).enumerate() {
            // Spread across the image: every voice then takes the two-sided path.
            s.note_on(
                pitch,
                100,
                NoteShape {
                    pan: i as f32 / 11.0,
                    ..NoteShape::neutral()
                },
            );
        }
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let t = Instant::now();
        for _ in 0..(SR as usize / 512) {
            l.fill(0.0);
            r.fill(0.0);
            s.render_add(&mut l, &mut r);
        }
        let rtf = t.elapsed().as_secs_f64() / ((SR as usize / 512 * 512) as f64 / SR as f64);
        eprintln!(
            "synth worst case: real-time factor {rtf:.4} ({:.1}x headroom)",
            1.0 / rtf
        );
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        if !cfg!(debug_assertions) {
            assert!(rtf < 0.5, "worst-case synth too slow: rtf {rtf}");
        }
    }

    /// One note, one shape, rendered to both sides.
    fn shaped(patch: &str, pitch: u8, shape: NoteShape, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut s = PolySynth::new(&factory(patch), SR, 1);
        s.set_tone_hz(2_000.0);
        s.note_on(pitch, 100, shape);
        render_stereo(&mut s, frames)
    }

    #[test]
    fn fine_cents_retune_the_voice() {
        let frames = 9600;
        let plain = shaped("sine", 69, NoteShape::neutral(), frames).0;
        let up = shaped(
            "sine",
            69,
            NoteShape {
                fine: 1200,
                ..NoteShape::neutral()
            },
            frames,
        )
        .0;
        let (a, b) = (zero_crossings(&plain), zero_crossings(&up));
        assert!(a > 100, "the reference note must actually oscillate: {a}");
        let ratio = b as f32 / a as f32;
        assert!(
            (ratio - 2.0).abs() < 0.05,
            "+1200 cents gave {b} crossings against {a} ({ratio})"
        );
        // And the sign is right: a negative detune goes down, not up.
        let down = shaped(
            "sine",
            69,
            NoteShape {
                fine: -1200,
                ..NoteShape::neutral()
            },
            frames,
        )
        .0;
        assert!((zero_crossings(&down) as f32 / a as f32 - 0.5).abs() < 0.05);
    }

    #[test]
    fn pan_places_the_voice_and_centre_stays_balanced() {
        let (l, r) = shaped(
            "sine",
            69,
            NoteShape {
                pan: 0.0,
                ..NoteShape::neutral()
            },
            9600,
        );
        assert!(rms(&l) > 0.01, "hard left must still speak");
        assert!(
            rms(&r) < rms(&l) * 1e-6,
            "hard left leaked into the right: {}",
            rms(&r)
        );

        let (l, r) = shaped(
            "sine",
            69,
            NoteShape {
                pan: 1.0,
                ..NoteShape::neutral()
            },
            9600,
        );
        assert!(rms(&r) > 0.01);
        assert!(rms(&l) < rms(&r) * 1e-6);

        // A centred note is untouched by the pan law: the same bits on both sides.
        let (l, r) = shaped("sine", 69, NoteShape::neutral(), 9600);
        assert_eq!(l, r, "a centred note must be identical on both sides");
    }

    #[test]
    fn release_lengthens_the_tail() {
        let tail_frames = |release: f32| {
            let mut s = PolySynth::new(&factory("sine"), SR, 1);
            s.note_on(
                69,
                100,
                NoteShape {
                    release,
                    ..NoteShape::neutral()
                },
            );
            render(&mut s, 4800);
            s.note_off(69);
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            let mut n = 0;
            while !s.is_silent() && n < SR as usize * 8 {
                l.fill(0.0);
                r.fill(0.0);
                s.render_add(&mut l, &mut r);
                n += 256;
            }
            n
        };
        let short = tail_frames(0.0);
        let long = tail_frames(1.0);
        assert!(short > 0, "even the shortest release has to ring briefly");
        assert!(
            long > short * 4,
            "release 1.0 gave {long} frames against {short} at 0.0"
        );
    }

    #[test]
    fn a_live_patch_change_keeps_the_notes_own_release() {
        let mut p = factory("sine");
        let mut s = PolySynth::new(&p, SR, 1);
        s.note_on(
            69,
            100,
            NoteShape {
                release: 1.0,
                ..NoteShape::neutral()
            },
        );
        render(&mut s, 4800);
        // A patch edit that does not touch the release at all must not wipe the note's own.
        p.level = 0.5;
        s.set_patch(&p);
        assert_eq!(
            s.voices[0].release_scale, 4.0,
            "the note's release scaling was lost"
        );
        s.note_off(69);
        let mut n = 0;
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        while !s.is_silent() && n < SR as usize * 8 {
            l.fill(0.0);
            r.fill(0.0);
            s.render_add(&mut l, &mut r);
            n += 256;
        }
        let plain = (p.amp_env.release * SR) as usize;
        assert!(n > plain * 2, "tail {n} frames against a plain {plain}");
    }

    #[test]
    fn mod_x_closes_the_filter_and_neutral_leaves_it_open() {
        // The legacy "saw" patch has no filter of its own, so the note's is the only one.
        let render_at = |mod_x: f32| {
            shaped(
                "saw",
                69,
                NoteShape {
                    mod_x,
                    ..NoteShape::neutral()
                },
                9600,
            )
            .0
        };
        let dark = render_at(0.0);
        let open = render_at(1.0);
        assert!(rms(&open) > 0.001, "the open voice must speak");
        assert!(
            bright_rms(&dark) < bright_rms(&open) * 0.25,
            "mod_x 0 was not duller: {} vs {}",
            bright_rms(&dark),
            bright_rms(&open)
        );
        // Neutral is the bypass, so it keeps every bit of the raw saw's brightness.
        let neutral = render_at(0.5);
        assert!(bright_rms(&neutral) > bright_rms(&dark) * 4.0);
    }

    #[test]
    fn mod_y_rings_the_filter_at_its_cutoff() {
        let render_at = |mod_y: f32| {
            shaped(
                "saw",
                69,
                NoteShape {
                    mod_x: 0.4,
                    mod_y,
                    ..NoteShape::neutral()
                },
                9600,
            )
            .0
        };
        // Resonance peaks the band around the cutoff, so the same source comes out louder.
        assert!(rms(&render_at(1.0)) > rms(&render_at(0.0)) * 1.5);
    }
}
