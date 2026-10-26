//! Small, allocation-free DSP building blocks: biquad and state-variable filters, an ADSR,
//! PolyBLEP oscillators and waveforms, LFOs, a seeded RNG, delay lines (chorus, echo), a bit
//! crusher, a Freeverb-style reverb and a constant-power pan law.

use dissonant_core::sound::LfoWave;
use std::f32::consts::PI;

// ─── Biquad ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn identity() -> Self {
        Biquad {
            b0: 1.0,
            ..Default::default()
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // Transposed direct form II.
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    fn set(&mut self, b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) {
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    pub fn lowpass(&mut self, sample_rate: f32, cutoff: f32, q: f32) {
        let fc = cutoff.clamp(10.0, sample_rate * 0.49);
        let w0 = 2.0 * PI * fc / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        self.set((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha);
    }

    pub fn highpass(&mut self, sample_rate: f32, cutoff: f32, q: f32) {
        let fc = cutoff.clamp(5.0, sample_rate * 0.49);
        let w0 = 2.0 * PI * fc / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        self.set((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha);
    }

    /// `gain_db` boost/cut at `freq`, bandwidth from `q`.
    pub fn peaking(&mut self, sample_rate: f32, freq: f32, gain_db: f32, q: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq.clamp(10.0, sample_rate * 0.49) / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        self.set(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        );
    }

    pub fn low_shelf(&mut self, sample_rate: f32, freq: f32, gain_db: f32, q: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq.clamp(10.0, sample_rate * 0.49) / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) - (a - 1.0) * cos + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - sa),
            (a + 1.0) + (a - 1.0) * cos + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - sa,
        );
    }

    pub fn high_shelf(&mut self, sample_rate: f32, freq: f32, gain_db: f32, q: f32) {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq.clamp(10.0, sample_rate * 0.49) / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let sa = 2.0 * a.sqrt() * alpha;
        self.set(
            a * ((a + 1.0) + (a - 1.0) * cos + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - sa),
            (a + 1.0) - (a - 1.0) * cos + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - sa,
        );
    }
}

/// Linear amplitude (1.0 = flat) → dB, guarding against -inf.
pub fn linear_to_db(gain: f32) -> f32 {
    20.0 * gain.max(0.02).log10()
}

/// A stereo pair of identical biquads.
#[derive(Debug, Clone, Copy, Default)]
pub struct StereoBiquad {
    pub l: Biquad,
    pub r: Biquad,
}

impl StereoBiquad {
    pub fn identity() -> Self {
        StereoBiquad {
            l: Biquad::identity(),
            r: Biquad::identity(),
        }
    }
    pub fn configure(&mut self, f: impl Fn(&mut Biquad)) {
        f(&mut self.l);
        f(&mut self.r);
    }
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.l.process(l), self.r.process(r))
    }
}

// ─── ADSR ────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrParams {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
pub struct Adsr {
    params: AdsrParams,
    sample_rate: f32,
    stage: Stage,
    level: f32,
}

impl Adsr {
    pub fn new(params: AdsrParams, sample_rate: f32) -> Self {
        Adsr {
            params,
            sample_rate,
            stage: Stage::Idle,
            level: 0.0,
        }
    }

    pub fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// Change the shape live; the current stage and level carry on from where they are.
    pub fn set_params(&mut self, params: AdsrParams) {
        self.params = params;
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    /// Hard reset (voice stealing).
    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    #[inline]
    fn rate(&self, seconds: f32) -> f32 {
        1.0 / (seconds.max(0.0005) * self.sample_rate)
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => 0.0,
            Stage::Attack => {
                self.level += self.rate(self.params.attack);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
                self.level
            }
            Stage::Decay => {
                let target = self.params.sustain;
                self.level -= (1.0 - target).max(0.0) * self.rate(self.params.decay);
                if self.level <= target + 1e-5 {
                    self.level = target;
                    self.stage = if target <= 1e-4 { Stage::Idle } else { Stage::Sustain };
                }
                self.level
            }
            Stage::Sustain => self.level,
            Stage::Release => {
                // Exponential-feeling tail: constant fraction of the remaining level per sample
                // would never reach zero, so use a linear ramp scaled by the start level.
                self.level -= self.rate(self.params.release);
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
                self.level
            }
        }
    }
}

// ─── Oscillator ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Saw,
    Square,
    Triangle,
    Sine,
}

#[derive(Debug, Clone, Copy)]
pub struct Oscillator {
    pub waveform: Waveform,
    phase: f32,
    inc: f32,
    tri_state: f32,
}

/// PolyBLEP residual for a downward step of 2 at phase 0 (`t` = phase, `dt` = increment).
#[inline]
pub fn poly_blep(t: f32, dt: f32) -> f32 {
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

impl Oscillator {
    pub fn new(waveform: Waveform) -> Self {
        Oscillator {
            waveform,
            phase: 0.0,
            inc: 0.0,
            tri_state: 0.0,
        }
    }

    pub fn set_frequency(&mut self, hz: f32, sample_rate: f32) {
        self.inc = (hz / sample_rate).clamp(0.0, 0.5);
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.tri_state = 0.0;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        let t = self.phase;
        let dt = self.inc;
        let out = match self.waveform {
            Waveform::Sine => (2.0 * PI * t).sin(),
            Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Waveform::Square => {
                let mut v = if t < 0.5 { 1.0 } else { -1.0 };
                v += poly_blep(t, dt);
                v -= poly_blep((t + 0.5) % 1.0, dt);
                v
            }
            Waveform::Triangle => {
                // Leaky-integrated PolyBLEP square → anti-aliased triangle.
                let mut sq = if t < 0.5 { 1.0 } else { -1.0 };
                sq += poly_blep(t, dt);
                sq -= poly_blep((t + 0.5) % 1.0, dt);
                self.tri_state = 0.999 * self.tri_state + 4.0 * dt * sq;
                self.tri_state.clamp(-1.0, 1.0)
            }
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        out
    }
}

pub fn midi_to_hz(pitch: f32) -> f32 {
    440.0 * 2f32.powf((pitch - 69.0) / 12.0)
}

/// PolyBLAMP residual for a slope change of +1 per sample at phase 0: `(1 - |d|)³ / 6`, `d`
/// being the distance to the corner in samples. The integral of the BLEP residual.
#[inline]
pub fn poly_blamp(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = 1.0 - t / dt;
        x * x * x * (1.0 / 6.0)
    } else if t > 1.0 - dt {
        let x = 1.0 - (1.0 - t) / dt;
        x * x * x * (1.0 / 6.0)
    } else {
        0.0
    }
}

/// Stateless band-limited waveforms of a phase `t` in [0, 1) advancing `dt` per sample. Being
/// pure functions of phase, they take phase modulation and hard sync directly.
pub mod wave {
    use super::{poly_blamp, poly_blep, sin_cycles};

    #[inline]
    pub fn sine(t: f32) -> f32 {
        sin_cycles(t)
    }

    #[inline]
    pub fn saw(t: f32, dt: f32) -> f32 {
        2.0 * t - 1.0 - poly_blep(t, dt)
    }

    /// Duty cycle `pw`, DC removed so a narrow pulse doesn't thump when the envelope opens.
    /// At `pw` 0.5 this is exactly [`super::Oscillator`]'s square.
    #[inline]
    pub fn pulse(t: f32, dt: f32, pw: f32) -> f32 {
        let mut v = if t < pw { 1.0 } else { -1.0 };
        v += poly_blep(t, dt);
        let t2 = t - pw;
        v -= poly_blep(if t2 < 0.0 { t2 + 1.0 } else { t2 }, dt);
        v - (2.0 * pw - 1.0)
    }

    /// -1 at phase 0, +1 at phase 0.5 (the same phase as [`super::Oscillator`]'s triangle),
    /// PolyBLAMP-corrected corners.
    #[inline]
    pub fn triangle(t: f32, dt: f32) -> f32 {
        let t2 = t + 0.5;
        let t2 = if t2 >= 1.0 { t2 - 1.0 } else { t2 };
        1.0 - 4.0 * (t - 0.5).abs() + 8.0 * dt * (poly_blamp(t, dt) - poly_blamp(t2, dt))
    }
}

// ─── Fast math ───────────────────────────────────────────────────────────────────────────────

/// `sin(2πt)` for `t` in [0, 1): a 9th-order odd polynomial on a folded quarter wave (error
/// under 4e-6), several times cheaper than `f32::sin` in the oscillator loops.
#[inline]
pub fn sin_cycles(t: f32) -> f32 {
    // sin(2πt) = sin(2πx) with x = 0.5 - t in (-0.5, 0.5]; fold x into [-0.25, 0.25].
    let mut x = 0.5 - t;
    if x > 0.25 {
        x = 0.5 - x;
    } else if x < -0.25 {
        x = -0.5 - x;
    }
    let z = 2.0 * PI * x;
    let z2 = z * z;
    z * (1.0 + z2 * (-1.0 / 6.0 + z2 * (1.0 / 120.0 + z2 * (-1.0 / 5040.0 + z2 * (1.0 / 362_880.0)))))
}

/// Rational tanh approximation; exactly ±1 beyond |x| = 3, where it joins smoothly.
#[inline]
pub fn fast_tanh(x: f32) -> f32 {
    if x <= -3.0 {
        -1.0
    } else if x >= 3.0 {
        1.0
    } else {
        let x2 = x * x;
        x * (27.0 + x2) / (27.0 + 9.0 * x2)
    }
}

/// Identity up to `knee`, then a smooth (C¹) bend toward a ceiling of `2 × knee`. A safety net
/// against resonance and feedback blow-ups that never touches normal levels.
#[inline]
pub fn soft_limit(x: f32, knee: f32) -> f32 {
    let a = x.abs();
    if a <= knee {
        x
    } else {
        let e = (a - knee) / knee;
        (knee * (1.0 + e / (1.0 + e))).copysign(x)
    }
}

/// Saturation that is exactly the identity at `amount` 0 and morphs continuously into a
/// normalized `tanh` with gain 10 at 1. `(gain, norm)` come from [`drive_gain`].
#[inline]
pub fn drive(x: f32, amount: f32, gain: f32, norm: f32) -> f32 {
    x + amount * (fast_tanh(gain * x) * norm - x)
}

/// `(gain, 1 / tanh(gain))` for [`drive`].
pub fn drive_gain(amount: f32) -> (f32, f32) {
    let g = 1.0 + 9.0 * amount;
    (g, 1.0 / g.tanh())
}

#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 {
        0.0
    } else {
        x
    }
}

// ─── RNG ─────────────────────────────────────────────────────────────────────────────────────

/// xorshift32: tiny and seeded, so random phases and noise render identically every time.
#[derive(Debug, Clone, Copy)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed.max(1))
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// [0, 1)
    #[inline]
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// [-1, 1)
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

// ─── State-variable filter ───────────────────────────────────────────────────────────────────

/// Trapezoidal (zero-delay-feedback) state-variable filter after Simper: stable under fast
/// cutoff modulation, low / band / high / notch from one structure. `k` = 1/Q.
#[derive(Debug, Clone, Copy)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl Default for Svf {
    fn default() -> Self {
        Svf {
            ic1: 0.0,
            ic2: 0.0,
            a1: 1.0,
            a2: 0.0,
            a3: 0.0,
            k: std::f32::consts::SQRT_2,
        }
    }
}

/// One SVF tick's outputs; notch = `low + high`.
#[derive(Debug, Clone, Copy)]
pub struct SvfOut {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

impl Svf {
    /// Cutoff is clamped to 10 Hz … 0.45 × sample rate.
    pub fn set(&mut self, sample_rate: f32, cutoff: f32, k: f32) {
        let fc = cutoff.clamp(10.0, sample_rate * 0.45);
        let g = (PI * fc / sample_rate).tan();
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline]
    pub fn tick(&mut self, v0: f32) -> SvfOut {
        let v3 = v0 - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut {
            low: v2,
            band: v1,
            high: v0 - self.k * v1 - v2,
        }
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// Once per block: recover from NaN/inf and flush denormals out of the integrators.
    pub fn sanitize(&mut self) {
        if !(self.ic1.is_finite() && self.ic2.is_finite()) {
            self.reset();
        }
        self.ic1 = flush_denormal(self.ic1);
        self.ic2 = flush_denormal(self.ic2);
    }
}

// ─── LFO ─────────────────────────────────────────────────────────────────────────────────────

/// A bipolar (-1…1) low-frequency oscillator. Sine and triangle start at 0 heading up, so a
/// retriggered LFO doesn't jump at note-on.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lfo {
    phase: f32,
    held: f32,
}

impl Lfo {
    pub fn reset(&mut self, rng: &mut Rng) {
        self.phase = 0.0;
        self.held = rng.bipolar();
    }

    #[inline]
    pub fn value(&self, wave: LfoWave) -> f32 {
        let p = self.phase;
        match wave {
            LfoWave::Sine => sin_cycles(p),
            LfoWave::Triangle => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            LfoWave::Saw => 2.0 * p - 1.0,
            LfoWave::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWave::SampleHold => self.held,
        }
    }

    /// Move on by `cycles`; sample-and-hold picks a new value each time the phase wraps.
    #[inline]
    pub fn advance(&mut self, cycles: f32, rng: &mut Rng) {
        self.phase += cycles;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            self.held = rng.bipolar();
        }
    }
}

// ─── Delay line, chorus, echo, crusher ───────────────────────────────────────────────────────

/// A power-of-two ring buffer with linear-interpolated fractional reads. Allocates once.
#[derive(Debug, Clone)]
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    pos: usize,
}

impl DelayLine {
    /// Holds at least `max_samples` of history.
    pub fn new(max_samples: usize) -> Self {
        let len = (max_samples + 4).next_power_of_two();
        DelayLine {
            buf: vec![0.0; len],
            mask: len - 1,
            pos: 0,
        }
    }

    /// The longest delay [`DelayLine::read`] supports, in samples.
    pub fn capacity(&self) -> f32 {
        (self.buf.len() - 3) as f32
    }

    /// The input `d` samples ago (1 = the last [`DelayLine::write`]). Call before writing.
    #[inline]
    pub fn read(&self, d: f32) -> f32 {
        let d = d.clamp(1.0, self.capacity());
        let rp = self.pos as f32 + self.buf.len() as f32 - d;
        let i = rp as usize;
        let f = rp - i as f32;
        let a = self.buf[i & self.mask];
        let b = self.buf[(i + 1) & self.mask];
        a + (b - a) * f
    }

    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) & self.mask;
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

const CHORUS_BASE_S: f32 = 0.012;
const CHORUS_SWEEP_S: f32 = 0.006;

/// Mono chorus: two taps swept in anti-phase around 12 ms. `mix` 1 = equal dry and wet (the
/// strongest chorus); `depth` 1 = ±6 ms of sweep.
#[derive(Debug, Clone)]
pub struct Chorus {
    line: DelayLine,
    phase: f32,
    sample_rate: f32,
}

impl Chorus {
    pub fn new(sample_rate: f32) -> Self {
        Chorus {
            line: DelayLine::new(Self::tail_for(sample_rate)),
            phase: 0.0,
            sample_rate,
        }
    }

    fn tail_for(sample_rate: f32) -> usize {
        ((CHORUS_BASE_S + CHORUS_SWEEP_S) * sample_rate) as usize + 4
    }

    /// How many samples the effect can keep sounding after its input stops.
    pub fn tail(&self) -> usize {
        Self::tail_for(self.sample_rate)
    }

    #[inline]
    pub fn process(&mut self, x: f32, rate: f32, depth: f32, mix: f32) -> f32 {
        let base = CHORUS_BASE_S * self.sample_rate;
        let sweep = CHORUS_SWEEP_S * self.sample_rate * depth;
        let p2 = if self.phase >= 0.5 { self.phase - 0.5 } else { self.phase + 0.5 };
        let wet = 0.5
            * (self.line.read(base + sweep * sin_cycles(self.phase)) + self.line.read(base + sweep * sin_cycles(p2)));
        self.line.write(x);
        self.phase += rate / self.sample_rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        x + 0.5 * mix * (wet - x)
    }

    pub fn clear(&mut self) {
        self.line.clear();
    }
}

/// Feedback echo with damped repeats. A live time change glides (tape-style) instead of
/// jumping, and the feedback path is soft-limited so 0.95 feedback can't run away. Output =
/// dry + `mix` × echoes.
#[derive(Debug, Clone)]
pub struct Echo {
    line: DelayLine,
    time: f32,
    damp: f32,
    damp_coef: f32,
    glide_coef: f32,
    sample_rate: f32,
}

impl Echo {
    pub fn new(sample_rate: f32, max_seconds: f32) -> Self {
        Echo {
            line: DelayLine::new((max_seconds * sample_rate) as usize + 4),
            time: 0.3 * sample_rate,
            damp: 0.0,
            damp_coef: 1.0 - (-2.0 * PI * 6_500.0 / sample_rate).exp(),
            glide_coef: 1.0 / (0.05 * sample_rate),
            sample_rate,
        }
    }

    /// Forget the old repeats and jump straight to `seconds` (no glide).
    pub fn reset(&mut self, seconds: f32) {
        self.line.clear();
        self.damp = 0.0;
        self.time = (seconds * self.sample_rate).clamp(1.0, self.line.capacity());
    }

    /// The current delay in samples.
    pub fn time_samples(&self) -> f32 {
        self.time
    }

    #[inline]
    pub fn process(&mut self, x: f32, seconds: f32, feedback: f32, mix: f32) -> f32 {
        let target = (seconds * self.sample_rate).clamp(1.0, self.line.capacity());
        self.time += (target - self.time) * self.glide_coef;
        let y = self.line.read(self.time);
        self.damp = flush_denormal(self.damp + (y - self.damp) * self.damp_coef);
        self.line.write(flush_denormal(x + soft_limit(self.damp * feedback, 1.0)));
        x + mix * self.damp
    }
}

/// Sample-rate reduction (a new sample every `1 / ratio` samples, held) plus quantization to
/// `steps` levels per unit.
#[derive(Debug, Clone, Copy, Default)]
pub struct Crusher {
    phase: f32,
    held: f32,
}

impl Crusher {
    /// `(ratio, steps)` for an amount 0–1: down to 1/32 of the rate and roughly 4 bits.
    pub fn params(amount: f32) -> (f32, f32) {
        ((-5.0 * amount).exp2(), (15.0 - 12.0 * amount).exp2())
    }

    #[inline]
    pub fn process(&mut self, x: f32, ratio: f32, steps: f32) -> f32 {
        self.phase += ratio;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held = (x * steps).round() / steps;
        }
        self.held
    }
}

// ─── Reverb ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    idx: usize,
    filter: f32,
    feedback: f32,
    damp: f32,
}

impl Comb {
    fn new(len: usize, feedback: f32, damp: f32) -> Self {
        Comb {
            buf: vec![0.0; len.max(1)],
            idx: 0,
            filter: 0.0,
            feedback,
            damp,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.idx];
        self.filter = y * (1.0 - self.damp) + self.filter * self.damp;
        self.buf[self.idx] = x + self.filter * self.feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Allpass {
            buf: vec![0.0; len.max(1)],
            idx: 0,
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let y = -x + b;
        self.buf[self.idx] = x + b * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}

/// Brings the comb/allpass network to unity against the dry signal. Its own broadband rms
/// gain measures 0.652 at 44.1 kHz and 0.645 at 48 kHz, so one constant covers both.
const WET_MAKEUP: f32 = 1.53;

/// Freeverb-style stereo reverb. `wet` is a 0–1 dry/wet mix, crossfaded at constant power.
#[derive(Debug, Clone)]
pub struct Reverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    allpass_l: Vec<Allpass>,
    allpass_r: Vec<Allpass>,
    pub wet: f32,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let comb_lens = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        let allpass_lens = [556, 441, 341, 225];
        let spread = 23;
        let s = |n: usize| ((n as f32) * scale) as usize;
        Reverb {
            combs_l: comb_lens.iter().map(|&n| Comb::new(s(n), 0.84, 0.2)).collect(),
            combs_r: comb_lens.iter().map(|&n| Comb::new(s(n + spread), 0.84, 0.2)).collect(),
            allpass_l: allpass_lens.iter().map(|&n| Allpass::new(s(n))).collect(),
            allpass_r: allpass_lens.iter().map(|&n| Allpass::new(s(n + spread))).collect(),
            wet: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.wet <= 0.0001 {
            return (l, r);
        }
        let input = (l + r) * 0.015;
        let mut wl = 0.0;
        let mut wr = 0.0;
        for c in &mut self.combs_l {
            wl += c.process(input);
        }
        for c in &mut self.combs_r {
            wr += c.process(input);
        }
        for a in &mut self.allpass_l {
            wl = a.process(wl);
        }
        for a in &mut self.allpass_r {
            wr = a.process(wr);
        }
        // Dry and wet are uncorrelated, so crossfade with constant power — the same law the
        // pan knob uses. With a unity-calibrated wet path this makes the mix knob change the
        // amount of ambience without changing how loud the bus is.
        let dry_g = (1.0 - self.wet).sqrt();
        let wet_g = self.wet.sqrt() * WET_MAKEUP;
        (l * dry_g + wl * wet_g, r * dry_g + wr * wet_g)
    }
}

// ─── Pan ─────────────────────────────────────────────────────────────────────────────────────

/// Constant-power pan law. `pan` in -1 (left) … 1 (right) → (left gain, right gain).
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_is_constant_power() {
        let (l, r) = pan_gains(0.0);
        assert!((l - r).abs() < 1e-6);
        assert!((l * l + r * r - 1.0).abs() < 1e-5);
        let (l, r) = pan_gains(-1.0);
        assert!(l > 0.999 && r.abs() < 1e-6);
        let (l, r) = pan_gains(1.0);
        assert!(r > 0.999 && l.abs() < 1e-6);
    }

    #[test]
    fn lowpass_attenuates_high_frequencies() {
        let sr = 44_100.0;
        let mut lp = Biquad::identity();
        lp.lowpass(sr, 500.0, 0.707);
        let mut osc = Oscillator::new(Waveform::Sine);
        osc.set_frequency(8_000.0, sr);
        let mut peak = 0.0f32;
        for i in 0..4410 {
            let y = lp.process(osc.tick());
            if i > 2000 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak < 0.01, "peak {peak}");
    }

    #[test]
    fn adsr_shape() {
        let mut env = Adsr::new(
            AdsrParams {
                attack: 0.01,
                decay: 0.01,
                sustain: 0.5,
                release: 0.01,
            },
            1000.0,
        );
        env.gate_on();
        let mut peak = 0.0;
        for _ in 0..40 {
            peak = env.tick().max(peak);
        }
        assert!((peak - 1.0).abs() < 1e-5);
        assert!((env.tick() - 0.5).abs() < 1e-5);
        env.gate_off();
        for _ in 0..40 {
            env.tick();
        }
        assert!(!env.is_active());
    }

    #[test]
    fn sin_cycles_matches_sin() {
        for i in 0..10_000 {
            let t = i as f32 / 10_000.0;
            let err = (sin_cycles(t) - (2.0 * PI * t).sin()).abs();
            assert!(err < 1e-5, "t {t}: err {err}");
        }
    }

    #[test]
    fn waves_stay_in_range_and_pulse_has_no_dc() {
        let dt = 1_000.0 / 48_000.0;
        let (mut dc, n) = (0.0, 48_000);
        for i in 0..n {
            let t = (i as f32 * dt).fract();
            for v in [wave::saw(t, dt), wave::triangle(t, dt), wave::pulse(t, dt, 0.5), wave::sine(t)] {
                assert!(v.abs() <= 1.3, "{v}");
            }
            let p = wave::pulse(t, dt, 0.1);
            assert!(p.abs() <= 2.2, "{p}");
            dc += p;
        }
        assert!((dc / n as f32).abs() < 0.01, "pulse dc {}", dc / n as f32);
    }

    #[test]
    fn svf_lowpass_and_highpass_split_the_spectrum() {
        let sr = 48_000.0;
        let level = |hz: f32, high: bool| {
            let mut f = Svf::default();
            f.set(sr, 1_000.0, std::f32::consts::SQRT_2);
            let mut peak = 0.0f32;
            for i in 0..9_600 {
                let o = f.tick(sin_cycles((i as f32 * hz / sr).fract()));
                if i > 4_800 {
                    peak = peak.max(if high { o.high } else { o.low }.abs());
                }
            }
            peak
        };
        assert!(level(100.0, false) > 0.95 && level(10_000.0, false) < 0.02);
        assert!(level(10_000.0, true) > 0.95 && level(100.0, true) < 0.02);
    }

    #[test]
    fn delay_line_reads_back_what_it_wrote() {
        let mut d = DelayLine::new(100);
        for i in 0..200 {
            d.write(i as f32);
        }
        assert_eq!(d.read(1.0), 199.0);
        assert_eq!(d.read(50.0), 150.0);
        assert!((d.read(10.5) - 189.5).abs() < 1e-4);
    }

    #[test]
    fn oscillators_stay_in_range() {
        for wf in [Waveform::Saw, Waveform::Square, Waveform::Triangle, Waveform::Sine] {
            let mut o = Oscillator::new(wf);
            o.set_frequency(440.0, 44_100.0);
            for _ in 0..10_000 {
                let v = o.tick();
                assert!(v.abs() <= 1.3, "{wf:?} produced {v}");
            }
        }
    }
}
