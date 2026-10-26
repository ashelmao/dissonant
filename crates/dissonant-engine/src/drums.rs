//! Synthesized drums (no bundled samples). A [`DrumPatch`] is rendered once into a one-shot
//! buffer ([`render_drum`]) — body (swept oscillator + overtone), filtered noise (multi-burst for
//! claps), an 808-style metal cluster and a click, then drive / crush / cuts — and a
//! [`DrumKit`] plays those buffers back per pad, with choke groups.
//!
//! Rendering is the expensive part, so it stays off the audio thread: the sending side keeps a
//! [`DrumBank`] of rendered patches and ships it with each sequence, and a kit only renders a
//! pad itself when nothing has rendered that patch yet (offline renders, tests).

use dissonant_core::sound::{FilterMode, OscWave};
use dissonant_core::{DrumPatch, ResolvedPad, Sequence, MAX_DRUM_PADS};
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

/// Choke / retrigger fade: long enough not to click, short enough to read as a cut.
const FADE_SECS: f32 = 0.004;
/// Hits sounding at once per kit; the oldest goes first beyond this.
const MAX_VOICES: usize = 32;

/// The TR-808 hi-hat oscillators (205.3, 304.4, 369.6, 522.7, 540, 800 Hz) as ratios of the first.
const METAL_RATIOS: [f32; 6] = [1.0, 1.4827, 1.8003, 2.5460, 2.6303, 3.8967];

// ─── Rendering ───────────────────────────────────────────────────────────────────────────────

/// Tiny deterministic noise source so renders are reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

/// Topology-preserving state-variable filter (12 dB/oct): low, band and high outputs at once.
#[derive(Debug, Clone, Copy)]
struct Svf {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    fn new(sample_rate: f32, hz: f32, q: f32) -> Self {
        let g = (PI * (hz / sample_rate).clamp(1e-5, 0.49)).tan();
        let k = 1.0 / q.max(0.1);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Svf { k, a1, a2, a3: g * a2, ic1: 0.0, ic2: 0.0 }
    }

    /// (low, band, high)
    #[inline]
    fn tick(&mut self, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - self.k * v1 - v2)
    }
}

/// One [`FilterMode`] built from SVFs. The band-pass is normalized to unity gain at its centre.
struct ModeFilter {
    mode: FilterMode,
    a: Svf,
    b: Svf,
}

impl ModeFilter {
    fn new(mode: FilterMode, sample_rate: f32, hz: f32, resonance: f32) -> Self {
        // 0 → Butterworth, 1 → Q ≈ 16.
        let q = std::f32::consts::FRAC_1_SQRT_2 * 2f32.powf(resonance.clamp(0.0, 1.0) * 4.5);
        let a = Svf::new(sample_rate, hz, q);
        ModeFilter { mode, a, b: a }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        match self.mode {
            FilterMode::Off => x,
            FilterMode::Lowpass12 => self.a.tick(x).0,
            FilterMode::Lowpass24 => {
                let y = self.a.tick(x).0;
                self.b.tick(y).0
            }
            FilterMode::Highpass12 => self.a.tick(x).2,
            FilterMode::Bandpass => self.a.k * self.a.tick(x).1,
            FilterMode::Notch => {
                let (lo, _, hi) = self.a.tick(x);
                lo + hi
            }
        }
    }
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// One sample of `wave` at `phase` (0–1) advancing by `inc` per sample. Saw and squares are
/// PolyBLEP'd: a tuned-up body sweeping down from 4 kHz would otherwise alias hard.
#[inline]
fn osc(wave: OscWave, phase: f32, inc: f32) -> f32 {
    let pulse = |width: f32| {
        let mut v = if phase < width { 1.0 } else { -1.0 };
        v += poly_blep(phase, inc);
        v -= poly_blep((phase + 1.0 - width) % 1.0, inc);
        v
    };
    match wave {
        OscWave::Sine => (TAU * phase).sin(),
        OscWave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        OscWave::Saw => 2.0 * phase - 1.0 - poly_blep(phase, inc),
        OscWave::Square => pulse(0.5),
        OscWave::Pulse => pulse(0.25),
    }
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

/// Scale `x` toward `target` RMS, the gain clamped to `[lo, hi]`. Keeps a component's level
/// meaning roughly the same whatever its filter throws away.
fn normalize_rms(x: &mut [f32], target: f32, lo: f32, hi: f32) {
    let r = rms(x);
    if r > 1e-9 {
        let g = (target / r).clamp(lo, hi);
        x.iter_mut().for_each(|s| *s *= g);
    }
}

/// Render `patch` to a mono one-shot exactly `length` seconds long. Deterministic: the same
/// patch and rate always give the same samples. A single component peaks around `level`;
/// stacked ones can exceed it (the channel and master chains follow, as with the old kit).
pub fn render_drum(patch: &DrumPatch, sample_rate: f32) -> Vec<f32> {
    let p = patch.sanitized();
    let sr = sample_rate.max(1_000.0);
    let dt = 1.0 / sr;
    let frames = ((p.length * sr).round() as usize).max(1);
    let tune = 2f32.powf(p.tune / 12.0);
    let nyquist = sr * 0.45;
    let mut out = vec![0.0f32; frames];
    let mut scratch = vec![0.0f32; frames];

    // Body: exponential pitch sweep start → end, exponential amplitude decay, one overtone.
    let b = p.body;
    if b.level > 0.0 {
        let (start, end) = (b.start_hz * tune, b.end_hz * tune);
        let sweep_k = (-dt / b.pitch_decay).exp();
        let amp_k = (-dt / b.decay).exp();
        let (mut sweep, mut amp, mut ph, mut ph2) = (1.0f32, 1.0f32, 0.0f32, 0.0f32);
        for o in out.iter_mut() {
            let f = (end + (start - end) * sweep).min(nyquist);
            let (inc, inc2) = (f * dt, (f * b.overtone_ratio).min(nyquist) * dt);
            let mut s = osc(b.wave, ph, inc);
            if b.overtone_level > 0.0 {
                s += b.overtone_level * osc(b.wave, ph2, inc2);
            }
            *o += b.level * amp * s;
            ph = (ph + inc).fract();
            ph2 = (ph2 + inc2).fract();
            sweep *= sweep_k;
            amp *= amp_k;
        }
    }

    // Noise: filtered white noise. The envelope restarts `bursts` times `burst_spacing` apart;
    // the early bursts snap shut, the last decays over `decay` (a hand clap).
    let n = p.noise;
    if n.level > 0.0 {
        let mut rng = Lcg(0x9E3779B97F4A7C15);
        let mut filter = ModeFilter::new(n.mode, sr, n.cutoff, n.resonance);
        for s in scratch.iter_mut() {
            *s = filter.process(rng.next());
        }
        // A narrow filter is made up toward raw white noise's level, by at most +10 dB.
        normalize_rms(&mut scratch, 0.5, 1.0, 3.0);
        let short = (n.burst_spacing * 0.35).min(n.decay);
        for (i, (o, s)) in out.iter_mut().zip(&scratch).enumerate() {
            let t = i as f32 * dt;
            let k = ((t / n.burst_spacing) as u32).min(n.bursts - 1);
            let since = t - k as f32 * n.burst_spacing;
            let decay = if k + 1 == n.bursts { n.decay } else { short };
            let mut env = (-since / decay).exp();
            if n.attack > 0.0 {
                env *= (t / n.attack).min(1.0);
            }
            *o += n.level * env * s;
        }
    }

    // Metal: six squares at the 808 ratios through two band-passes in series, normalized since
    // the squares carry little energy up around the centre.
    let m = p.metal;
    if m.level > 0.0 {
        let base = m.base_hz * tune;
        // Spread the start phases so the cluster doesn't open on one big aligned edge.
        let mut phases: [f32; 6] = std::array::from_fn(|i| i as f32 * 0.17);
        let mut bp1 = ModeFilter::new(FilterMode::Bandpass, sr, m.cutoff, 0.25);
        let mut bp2 = ModeFilter::new(FilterMode::Bandpass, sr, m.cutoff, 0.25);
        for s in scratch.iter_mut() {
            let mut x = 0.0;
            for (ph, r) in phases.iter_mut().zip(METAL_RATIOS) {
                let inc = (base * r).min(nyquist) * dt;
                x += osc(OscWave::Square, *ph, inc);
                *ph = (*ph + inc).fract();
            }
            *s = bp2.process(bp1.process(x / 6.0));
        }
        normalize_rms(&mut scratch, 0.35, 0.0, 200.0);
        let amp_k = (-dt / m.decay).exp();
        let mut amp = 1.0f32;
        for (o, s) in out.iter_mut().zip(&scratch) {
            *o += m.level * amp * s;
            amp *= amp_k;
        }
    }

    // Click: a few ms of sine at `tone_hz` blended with noise band-passed around it.
    let c = p.click;
    if c.level > 0.0 {
        let tone = (c.tone_hz * tune).min(nyquist);
        let len = ((c.decay * 8.0 * sr) as usize).min(frames);
        let mut rng = Lcg(0xD1B54A32D192ED03);
        let mut bp = ModeFilter::new(FilterMode::Bandpass, sr, tone, 0.0);
        let amp_k = (-dt / c.decay).exp();
        let (mut amp, mut ph) = (1.0f32, 0.0f32);
        for o in out[..len].iter_mut() {
            let s = 0.5 * (TAU * ph).sin() + 0.8 * bp.process(rng.next());
            *o += c.level * amp * s;
            ph = (ph + tone * dt).fract();
            amp *= amp_k;
        }
    }

    // Drive: tanh, crossfaded in so 0 is clean and a full-scale peak stays put.
    if p.drive > 0.0 {
        let g = 1.0 + 9.0 * p.drive;
        let norm = g.tanh();
        for s in out.iter_mut() {
            *s = (1.0 - p.drive) * *s + p.drive * (g * *s).tanh() / norm;
        }
    }

    // Crush: fewer bits (16 → 4) and a sample-and-hold (1 → 16 samples).
    if p.crush > 0.0 {
        let levels = 2f32.powf(15.0 - p.crush * 12.0);
        let hold = 1 + (p.crush * p.crush * 15.0) as usize;
        let mut held = 0.0;
        for (i, s) in out.iter_mut().enumerate() {
            if i % hold == 0 {
                held = (*s * levels).round() / levels;
            }
            *s = held;
        }
    }

    if p.low_cut > 20.5 {
        let mut hp = ModeFilter::new(FilterMode::Highpass12, sr, p.low_cut, 0.0);
        out.iter_mut().for_each(|s| *s = hp.process(*s));
    }
    if p.high_cut < 19_990.0 {
        let mut lp = ModeFilter::new(FilterMode::Lowpass12, sr, p.high_cut, 0.0);
        out.iter_mut().for_each(|s| *s = lp.process(*s));
    }

    // Level, and a short fade at the very end so a tail cut off by `length` doesn't click.
    let fade = ((FADE_SECS * sr) as usize).min(frames / 4).max(1);
    for (i, s) in out.iter_mut().enumerate() {
        let left = frames - i;
        let f = if left <= fade { (left - 1) as f32 / fade as f32 } else { 1.0 };
        *s *= p.level * f;
    }
    out
}

/// A patch rendered at one sample rate. Shared (`Arc`) between the bank that rendered it, the
/// kit pads that play it and any hit still sounding after its pad changed.
#[derive(Clone, PartialEq)]
pub struct RenderedDrum {
    /// The patch as given (not sanitized), so it compares equal to the one that asked for it.
    pub patch: DrumPatch,
    pub sample_rate: f32,
    pub samples: Vec<f32>,
}

impl RenderedDrum {
    pub fn new(patch: &DrumPatch, sample_rate: f32) -> Self {
        RenderedDrum { patch: *patch, sample_rate, samples: render_drum(patch, sample_rate) }
    }
}

impl std::fmt::Debug for RenderedDrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderedDrum")
            .field("sample_rate", &self.sample_rate)
            .field("frames", &self.samples.len())
            .finish_non_exhaustive()
    }
}

/// Rendered drums keyed by patch. Built on the sending side (`AudioDevice::send`) and shipped to
/// the engine with each sequence, so the audio thread never synthesizes a drum.
#[derive(Debug, Clone, Default)]
pub struct DrumBank {
    sample_rate: f32,
    drums: Vec<Arc<RenderedDrum>>,
}

impl DrumBank {
    pub fn new(sample_rate: f32) -> Self {
        DrumBank { sample_rate, drums: Vec::new() }
    }

    /// Every pad of every drum track in `sequence`, rendered at `sample_rate`.
    pub fn for_sequence(sequence: &Sequence, sample_rate: f32) -> Self {
        let mut bank = DrumBank::new(sample_rate);
        bank.prepare(sequence, sample_rate);
        bank
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn len(&self) -> usize {
        self.drums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drums.is_empty()
    }

    pub fn get(&self, patch: &DrumPatch) -> Option<&Arc<RenderedDrum>> {
        self.drums.iter().find(|d| d.patch == *patch && d.sample_rate == self.sample_rate)
    }

    /// Hold exactly the drums `sequence`'s kits use, rendering only patches not already here.
    /// Runs on every model edit, so an unchanged kit costs a few patch comparisons.
    pub fn prepare(&mut self, sequence: &Sequence, sample_rate: f32) {
        if sample_rate != self.sample_rate {
            self.drums.clear();
            self.sample_rate = sample_rate;
        }
        let mut next: Vec<Arc<RenderedDrum>> = Vec::with_capacity(self.drums.len());
        for pad in sequence.tracks.iter().filter(|t| t.is_drum).flat_map(|t| &t.drum_pads) {
            if next.iter().any(|d| d.patch == pad.patch) {
                continue;
            }
            let drum = match self.get(&pad.patch) {
                Some(d) => Arc::clone(d),
                None => Arc::new(RenderedDrum::new(&pad.patch, sample_rate)),
            };
            next.push(drum);
        }
        self.drums = next;
    }
}

// ─── Playback ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Pad {
    pitch: u8,
    choke: u8,
    drum: Arc<RenderedDrum>,
}

/// One sounding hit. It holds its own buffer (an `Arc` clone), so it plays out even if its pad
/// is re-patched or removed meanwhile.
#[derive(Debug, Clone)]
struct Voice {
    drum: Arc<RenderedDrum>,
    pitch: u8,
    choke: u8,
    pos: usize,
    gain: f32,
    /// Samples of choke fade left; `None` while playing normally.
    fade: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct DrumKit {
    pads: Vec<Pad>,
    voices: Vec<Voice>,
    sample_rate: f32,
    fade_len: usize,
}

impl DrumKit {
    /// Render every pad now. The engine uses [`DrumKit::with_bank`] to reuse pre-rendered drums.
    pub fn new(pads: &[ResolvedPad], sample_rate: f32) -> Self {
        Self::with_bank(pads, sample_rate, None)
    }

    pub fn with_bank(pads: &[ResolvedPad], sample_rate: f32, bank: Option<&DrumBank>) -> Self {
        let mut kit = DrumKit::empty(sample_rate);
        kit.update(pads, bank);
        kit
    }

    /// One pad playing `drum` (the sound designer's preview); play it with [`DrumKit::trigger`].
    pub fn single(drum: Arc<RenderedDrum>) -> Self {
        let mut kit = DrumKit::empty(drum.sample_rate);
        kit.pads.push(Pad { pitch: 0, choke: 0, drum });
        kit
    }

    fn empty(sample_rate: f32) -> Self {
        DrumKit {
            pads: Vec::with_capacity(MAX_DRUM_PADS),
            voices: Vec::with_capacity(MAX_VOICES),
            sample_rate,
            fade_len: ((FADE_SECS * sample_rate) as usize).max(1),
        }
    }

    /// Follow a changed kit. A pad whose patch is unchanged (anywhere in the old kit) keeps its
    /// buffer; others come from `bank`, and only failing that are rendered here. Sounding hits
    /// play out.
    pub fn update(&mut self, pads: &[ResolvedPad], bank: Option<&DrumBank>) {
        let pads = &pads[..pads.len().min(MAX_DRUM_PADS)];
        let same = self.pads.len() == pads.len()
            && self.pads.iter().zip(pads).all(|(a, b)| a.pitch == b.pitch && a.choke == b.choke && a.drum.patch == b.patch);
        if same {
            return;
        }
        let sr = self.sample_rate;
        let next = pads
            .iter()
            .map(|p| {
                let drum = self
                    .pads
                    .iter()
                    .find(|old| old.drum.patch == p.patch)
                    .map(|old| Arc::clone(&old.drum))
                    .or_else(|| bank.filter(|b| b.sample_rate() == sr).and_then(|b| b.get(&p.patch)).cloned())
                    .unwrap_or_else(|| Arc::new(RenderedDrum::new(&p.patch, sr)));
                Pad { pitch: p.pitch, choke: p.choke, drum }
            })
            .collect();
        self.pads = next;
    }

    /// Swap the buffer one pad plays (hits already sounding keep theirs).
    pub fn set_drum(&mut self, pad: usize, drum: Arc<RenderedDrum>) {
        if let Some(p) = self.pads.get_mut(pad) {
            p.drum = drum;
        }
    }

    /// The buffer the pad at `pitch` plays.
    pub fn pad_drum(&self, pitch: u8) -> Option<&Arc<RenderedDrum>> {
        self.pads.iter().find(|p| p.pitch == pitch).map(|p| &p.drum)
    }

    pub fn pad_count(&self) -> usize {
        self.pads.len()
    }

    pub fn pitch_index(&self, pitch: u8) -> Option<usize> {
        self.pads.iter().position(|p| p.pitch == pitch)
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8) {
        if let Some(i) = self.pitch_index(pitch) {
            self.trigger(i, velocity);
        }
    }

    /// Play pad `index`. Retriggering a pad, or hitting another in its non-zero choke group,
    /// fades the earlier hit out over a few ms instead of cutting it (a cut clicks).
    pub fn trigger(&mut self, index: usize, velocity: u8) {
        let Some(pad) = self.pads.get(index) else { return };
        let (pitch, choke) = (pad.pitch, pad.choke);
        let fade = self.fade_len;
        for v in &mut self.voices {
            if v.fade.is_none() && (v.pitch == pitch || (choke != 0 && v.choke == choke)) {
                v.fade = Some(fade);
            }
        }
        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0);
        }
        let amount = pad.drum.patch.velocity.clamp(0.0, 1.0);
        let gain = 1.0 - amount * (1.0 - velocity.min(127) as f32 / 127.0);
        self.voices.push(Voice { drum: Arc::clone(&pad.drum), pitch, choke, pos: 0, gain, fade: None });
    }

    pub fn is_silent(&self) -> bool {
        self.voices.is_empty()
    }

    pub fn render_add(&mut self, out: &mut [f32]) {
        let fade_len = self.fade_len as f32;
        for v in &mut self.voices {
            let buf = &v.drum.samples;
            let n = out.len().min(buf.len().saturating_sub(v.pos));
            match v.fade.as_mut() {
                None => {
                    for (o, s) in out[..n].iter_mut().zip(&buf[v.pos..v.pos + n]) {
                        *o += s * v.gain;
                    }
                    v.pos += n;
                }
                Some(left) => {
                    let n = n.min(*left);
                    for (o, s) in out[..n].iter_mut().zip(&buf[v.pos..v.pos + n]) {
                        *left -= 1;
                        *o += s * v.gain * (*left as f32 / fade_len);
                    }
                    v.pos += n;
                }
            }
        }
        self.voices.retain(|v| v.pos < v.drum.samples.len() && v.fade != Some(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissonant_core::{factory_sound, factory_sounds, ProjectModel, SoundPatch};

    const SR: f32 = 44_100.0;

    fn factory_drum(id: &str) -> DrumPatch {
        match factory_sound(id).map(|s| &s.patch) {
            Some(SoundPatch::Drum(p)) => *p,
            _ => panic!("no factory drum {id}"),
        }
    }

    fn pad(pitch: u8, id: &str, choke: u8) -> ResolvedPad {
        ResolvedPad { pitch, choke, patch: factory_drum(id) }
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn every_factory_drum_renders_a_sane_one_shot() {
        for sound in factory_sounds() {
            let SoundPatch::Drum(patch) = &sound.patch else { continue };
            let buf = render_drum(patch, SR);
            let id = &sound.id;
            assert!(buf.iter().all(|s| s.is_finite()), "{id}: non-finite sample");
            let p = peak(&buf);
            assert!(p > 0.05, "{id}: silent (peak {p})");
            assert!(p < 2.0, "{id}: peak {p} blows up");
            assert_eq!(buf.len(), (patch.length * SR).round() as usize, "{id}: not `length` long");
            assert!(buf.last().unwrap().abs() < 1e-3, "{id}: ends on a step");
        }
    }

    #[test]
    fn rendering_is_deterministic_and_tune_raises_pitch() {
        let kick = factory_drum("kick");
        assert_eq!(render_drum(&kick, SR), render_drum(&kick, SR));
        // Zero crossings through the body's tail: an octave up about doubles them.
        let crossings = |p: &DrumPatch| {
            let b = render_drum(p, SR);
            b[2_000..14_000].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32
        };
        let up = DrumPatch { tune: 12.0, ..kick };
        let ratio = crossings(&up) / crossings(&kick);
        assert!((1.7..2.3).contains(&ratio), "octave up gave ×{ratio}");
    }

    /// The legacy kick keeps the old one's shape: a falling sine settling near 45 Hz, as long
    /// and as loud.
    #[test]
    fn legacy_kick_is_close_to_the_old_one() {
        let old: Vec<f32> = (0..(0.34 * SR) as usize)
            .map(|i| {
                let t = i as f32 / SR;
                (TAU * (110.0 * (-t * 28.0).exp() + 45.0) * t).sin() * (-t * 8.0).exp() * 0.7
            })
            .collect();
        let new = render_drum(&factory_drum("kick"), SR);
        assert_eq!(old.len(), new.len());
        let (po, pn) = (peak(&old), peak(&new));
        assert!((pn / po - 1.0).abs() < 0.15, "peak {pn} vs old {po}");
        let (ro, rn) = (rms(&old), rms(&new));
        assert!((rn / ro - 1.0).abs() < 0.25, "rms {rn} vs old {ro}");
    }

    #[test]
    fn velocity_scales_by_the_patch_amount() {
        let render = |vel: u8, amount: f32| {
            let patch = DrumPatch { velocity: amount, ..factory_drum("kick") };
            let mut kit = DrumKit::new(&[ResolvedPad { pitch: 36, choke: 0, patch }], SR);
            kit.note_on(36, vel);
            let mut buf = vec![0.0; 4_000];
            kit.render_add(&mut buf);
            peak(&buf)
        };
        assert!(render(127, 0.65) > render(30, 0.65) * 1.8);
        assert!((render(127, 0.0) - render(30, 0.0)).abs() < 1e-6, "amount 0 = fixed level");
    }

    #[test]
    fn kick_renders_and_finishes() {
        let mut kit = DrumKit::new(&[pad(36, "kick", 0)], SR);
        kit.note_on(36, 127);
        let mut buf = vec![0.0; 2048];
        kit.render_add(&mut buf);
        assert!(buf.iter().any(|x| x.abs() > 0.05));
        let mut rest = vec![0.0; 44_100];
        kit.render_add(&mut rest);
        assert!(kit.is_silent());
    }

    #[test]
    fn unknown_pitch_is_ignored_and_retrigger_fades_the_old_hit() {
        let mut kit = DrumKit::new(&[pad(38, "snare", 0)], SR);
        kit.note_on(99, 100);
        assert!(kit.is_silent());
        kit.note_on(38, 100);
        let mut buf = vec![0.0; 1000];
        kit.render_add(&mut buf);
        kit.note_on(38, 100);
        assert_eq!(kit.voices.len(), 2, "the old hit fades while the new one starts");
        assert_eq!(kit.voices[1].pos, 0);
        kit.render_add(&mut buf);
        assert_eq!(kit.voices.len(), 1, "the faded hit is gone after a few ms");
    }

    #[test]
    fn a_closed_hat_chokes_the_open_hat_with_a_fade() {
        let pads = [pad(46, "open-hat", 1), pad(42, "hat", 1), pad(36, "kick", 0)];
        let mut kit = DrumKit::new(&pads, SR);
        kit.note_on(46, 120);
        kit.note_on(36, 120);
        let mut a = vec![0.0; 2_000];
        kit.render_add(&mut a);
        let mut ringing = kit.clone();
        kit.note_on(42, 120);
        assert!(kit.voices.iter().any(|v| v.pitch == 36 && v.fade.is_none()), "the kick has no choke group");
        // Isolate the open hat, choked and not.
        let open_hat = |k: &mut DrumKit| {
            k.voices.retain(|v| v.pitch == 46);
            let mut b = vec![0.0; 2_000];
            k.render_add(&mut b);
            b
        };
        let (free, choked) = (open_hat(&mut ringing), open_hat(&mut kit));
        let fade = kit.fade_len;
        assert!(rms(&free[fade..]) > 0.01, "the open hat really was ringing");
        assert!(choked[fade..].iter().all(|s| *s == 0.0), "the open hat rang on past the choke");
        assert!(choked[..fade].iter().zip(&free).all(|(c, f)| c.abs() <= f.abs() + 1e-7), "the choke fades, it doesn't jump");
        assert!((choked[0] - free[0]).abs() <= free[0].abs() * 0.05 + 1e-6, "the fade starts at full level");
    }

    #[test]
    fn more_than_four_pads_with_custom_pitches_all_play() {
        let ids = ["kick", "snare", "clap", "hat", "open-hat", "tom-hi", "tom-lo", "crash"];
        let pads: Vec<ResolvedPad> = ids.iter().enumerate().map(|(i, id)| pad(60 + i as u8 * 3, id, 0)).collect();
        let mut kit = DrumKit::new(&pads, SR);
        assert_eq!(kit.pad_count(), 8);
        for p in &pads {
            kit.note_on(p.pitch, 110);
            let mut buf = vec![0.0; 1_500];
            kit.render_add(&mut buf);
            assert!(peak(&buf) > 0.01, "pad {} is silent", p.pitch);
            kit.voices.clear();
        }
    }

    #[test]
    fn changing_one_pad_keeps_the_other_buffers() {
        let mut pads = vec![pad(36, "kick", 0), pad(38, "snare", 0), pad(42, "hat", 1)];
        let mut kit = DrumKit::new(&pads, SR);
        let before: Vec<Arc<RenderedDrum>> = [36, 38, 42].iter().map(|p| Arc::clone(kit.pad_drum(*p).unwrap())).collect();
        pads[1].patch.tune = 3.0;
        kit.update(&pads, None);
        assert!(Arc::ptr_eq(&before[0], kit.pad_drum(36).unwrap()));
        assert!(!Arc::ptr_eq(&before[1], kit.pad_drum(38).unwrap()), "the changed pad re-rendered");
        assert!(Arc::ptr_eq(&before[2], kit.pad_drum(42).unwrap()));
        // A bank hit is used instead of rendering, and a pad moved to a new pitch keeps its buffer.
        let mut bank = DrumBank::new(SR);
        bank.drums.push(Arc::new(RenderedDrum::new(&factory_drum("clap"), SR)));
        pads[2] = pad(50, "clap", 0);
        pads[0].pitch = 35;
        kit.update(&pads, Some(&bank));
        assert!(Arc::ptr_eq(&bank.drums[0], kit.pad_drum(50).unwrap()));
        assert!(Arc::ptr_eq(&before[0], kit.pad_drum(35).unwrap()));
    }

    #[test]
    fn a_bank_renders_each_patch_once_and_drops_unused_ones() {
        let mut model = ProjectModel::starter();
        let drum = model.tracks.iter().position(|t| t.is_drum).unwrap();
        let mut bank = DrumBank::for_sequence(&Sequence::silent(&model, 4.0), SR);
        assert_eq!(bank.len(), model.tracks[drum].drum_pads.len());
        let kept = model.tracks[drum].drum_pads[0].sound.clone();
        let rendered = Arc::clone(bank.get(&factory_drum(&kept)).unwrap());
        model.tracks[drum].drum_pads.truncate(1);
        bank.prepare(&Sequence::silent(&model, 4.0), SR);
        assert_eq!(bank.len(), 1);
        assert!(Arc::ptr_eq(&rendered, bank.get(&factory_drum(&kept)).unwrap()), "not re-rendered");
        bank.prepare(&Sequence::silent(&model, 4.0), 48_000.0);
        assert!(!Arc::ptr_eq(&rendered, bank.get(&factory_drum(&kept)).unwrap()), "a new rate re-renders");
    }
}
