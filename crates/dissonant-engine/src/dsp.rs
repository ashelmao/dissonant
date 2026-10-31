//! Small, allocation-free DSP building blocks: biquad and state-variable filters, an ADSR,
//! PolyBLEP oscillators and waveforms, LFOs, a seeded RNG, delay lines (chorus, echo), a bit
//! crusher, a Freeverb-style reverb, a constant-power pan law, and the guitar-rig primitives
//! (delay line, envelope follower, gate, compressor and oversampling wrappers).

use dissonant_core::sound::LfoWave;
use std::f32::consts::{PI, SQRT_2};
use std::sync::LazyLock;

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
        self.set(
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        );
    }

    pub fn highpass(&mut self, sample_rate: f32, cutoff: f32, q: f32) {
        let fc = cutoff.clamp(5.0, sample_rate * 0.49);
        let w0 = 2.0 * PI * fc / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        self.set(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        );
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

    /// Constant-peak-gain bandpass: unity at `freq`, `q` sets how narrow.
    pub fn bandpass(&mut self, sample_rate: f32, freq: f32, q: f32) {
        let w0 = 2.0 * PI * freq.clamp(10.0, sample_rate * 0.49) / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        self.set(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha);
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

/// dB → linear amplitude. The inverse of [`linear_to_db`] above its floor.
#[inline]
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
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

    /// Retune the release alone. A note's `release` property scales the preset's tail, and a
    /// voice that is already sounding must keep its stage and level while that happens —
    /// rebuilding the envelope would restart it and click.
    pub fn set_release(&mut self, seconds: f32) {
        self.params.release = seconds;
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
                    self.stage = if target <= 1e-4 {
                        Stage::Idle
                    } else {
                        Stage::Sustain
                    };
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
    z * (1.0
        + z2 * (-1.0 / 6.0 + z2 * (1.0 / 120.0 + z2 * (-1.0 / 5040.0 + z2 * (1.0 / 362_880.0)))))
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
        let p2 = if self.phase >= 0.5 {
            self.phase - 0.5
        } else {
            self.phase + 0.5
        };
        let wet = 0.5
            * (self.line.read(base + sweep * sin_cycles(self.phase))
                + self.line.read(base + sweep * sin_cycles(p2)));
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
        self.line
            .write(flush_denormal(x + soft_limit(self.damp * feedback, 1.0)));
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
            combs_l: comb_lens
                .iter()
                .map(|&n| Comb::new(s(n), 0.84, 0.2))
                .collect(),
            combs_r: comb_lens
                .iter()
                .map(|&n| Comb::new(s(n + spread), 0.84, 0.2))
                .collect(),
            allpass_l: allpass_lens.iter().map(|&n| Allpass::new(s(n))).collect(),
            allpass_r: allpass_lens
                .iter()
                .map(|&n| Allpass::new(s(n + spread)))
                .collect(),
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

    /// How long the tail rings, 0 (tight room) … 1 (long hall). The constructor's hard-coded
    /// 0.84 feedback / 0.2 damping is exactly `set_decay(0.5)`, so a reverb that never calls
    /// this sounds the same as it always has.
    pub fn set_decay(&mut self, decay01: f32) {
        let d = decay01.clamp(0.0, 1.0);
        let feedback = 0.70 + d * 0.28;
        let damp = 0.40 - d * 0.40;
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.feedback = feedback;
            c.damp = damp;
        }
    }

    /// Drop the tail. Needed when a rig is rebuilt mid-playback.
    pub fn reset(&mut self) {
        for c in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            c.buf.fill(0.0);
            c.filter = 0.0;
        }
        for a in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            a.buf.fill(0.0);
        }
    }
}

// ─── Pan ─────────────────────────────────────────────────────────────────────────────────────

/// Constant-power pan law. `pan` in -1 (left) … 1 (right) → (left gain, right gain).
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
    (angle.cos(), angle.sin())
}

/// Per-note pan law: `pan` in 0 (hard left) … 1 (hard right) → (left gain, right gain).
///
/// Equal power like [`pan_gains`], but normalised so the centre is unity on both sides rather
/// than -3 dB. A note carries no pan of its own until someone moves it, so centre has to leave
/// the signal exactly as it was — the track's own pan is what places it in the image.
#[inline]
pub fn note_pan_gains(pan: f32) -> (f32, f32) {
    if pan == 0.5 {
        // Not just an optimisation: cos(π/4)·√2 is 0.99999994, and this path has to be exact
        // so an untouched note renders bit-for-bit as it did before notes could be panned.
        return (1.0, 1.0);
    }
    let angle = pan.clamp(0.0, 1.0) * 0.5 * PI;
    (angle.cos() * SQRT_2, angle.sin() * SQRT_2)
}

/// A 2-pole topology-preserving state-variable low-pass: one per sounding note.
///
/// Mod X / Mod Y need a filter that is retuned on every note-on, at a different cutoff and Q
/// per voice. The TPT form suits that exactly: its coefficients are a closed form of cutoff
/// and Q with no bilinear frequency warping to correct, and both integrator states stay
/// bounded however far the cutoff jumps between notes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoteFilter {
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

impl NoteFilter {
    pub fn lowpass(sample_rate: f32, cutoff: f32, q: f32) -> Self {
        let mut f = NoteFilter::default();
        f.retune(sample_rate, cutoff, q);
        f
    }

    /// New cutoff and Q, keeping the integrator states: a slide sweeps the filter from one
    /// note's setting to the next while it rings, and starting over would click.
    pub fn retune(&mut self, sample_rate: f32, cutoff: f32, q: f32) {
        let g = (PI * cutoff.clamp(10.0, sample_rate * 0.45) / sample_rate).tan();
        let k = 1.0 / q.max(0.05);
        let a1 = 1.0 / (1.0 + g * (g + k));
        self.a1 = a1;
        self.a2 = g * a1;
        self.a3 = g * g * a1;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let hp = x - self.ic2;
        let bp = self.a1 * self.ic1 + self.a2 * hp;
        let lp = self.ic2 + self.a2 * self.ic1 + self.a3 * hp;
        self.ic1 = 2.0 * bp - self.ic1;
        self.ic2 = 2.0 * lp - self.ic2;
        lp
    }
}

/// Octaves either side of the track's own tone that `mod_x` sweeps.
const MOD_X_OCTAVES: f32 = 4.0;
/// How far `mod_y` moves the filter's Q above and below flat, in octaves.
const MOD_Y_OCTAVES: f32 = 2.5;

/// The filter a note's Mod X / Mod Y ask for, against the track's own tone frequency.
///
/// Neutral on both axes is `None`, not a wide-open filter: the voice then runs with no filter
/// in its path at all, so a project that never touches these two knobs costs nothing extra and
/// sounds exactly as it did.
pub fn note_filter(sample_rate: f32, tone_hz: f32, mod_x: f32, mod_y: f32) -> Option<NoteFilter> {
    if mod_x == 0.5 && mod_y == 0.5 {
        return None;
    }
    let (cutoff, q) = note_filter_tuning(tone_hz, mod_x, mod_y);
    Some(NoteFilter::lowpass(sample_rate, cutoff, q))
}

/// The cutoff and Q behind [`note_filter`], for retuning one that is already running.
pub fn note_filter_tuning(tone_hz: f32, mod_x: f32, mod_y: f32) -> (f32, f32) {
    let octaves = (mod_x.clamp(0.0, 1.0) - 0.5) * 2.0 * MOD_X_OCTAVES;
    let cutoff = (tone_hz * octaves.exp2()).clamp(200.0, 20_000.0);
    let q = 0.707 * ((mod_y.clamp(0.0, 1.0) - 0.5) * 2.0 * MOD_Y_OCTAVES).exp2();
    (cutoff, q)
}

// ─── Guitar rig primitives ───────────────────────────────────────────────────────────────────

/// A one-pole allpass: flat magnitude, frequency-dependent phase. Four in series swept by an
/// LFO make a phaser.
#[derive(Debug, Clone, Copy, Default)]
pub struct Allpass1 {
    a: f32,
    z: f32,
}

impl Allpass1 {
    pub fn set_freq(&mut self, sample_rate: f32, hz: f32) {
        let t = (PI * hz.clamp(10.0, sample_rate * 0.49) / sample_rate).tan();
        self.a = (t - 1.0) / (t + 1.0);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.z;
        self.z = x - self.a * y;
        y
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// A fractional-delay line with linear interpolation. Sized once; `read` past the buffer wraps
/// to the oldest sample rather than panicking. Distinct from the synth's [`DelayLine`]: this
/// one is read from 0 (the sample just written) and interpolates toward the *older* neighbour,
/// which is what the rig's modulated delays are tuned against.
#[derive(Debug, Clone)]
pub struct RigDelay {
    buf: Vec<f32>,
    idx: usize,
}

impl RigDelay {
    pub fn new(max_samples: usize) -> Self {
        RigDelay {
            buf: vec![0.0; max_samples.max(2)],
            idx: 0,
        }
    }

    /// Write one sample and advance. Call once per sample, after every `read` for that sample.
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.idx] = x;
        self.idx = if self.idx + 1 == self.buf.len() {
            0
        } else {
            self.idx + 1
        };
    }

    /// The sample `delay_samples` ago, interpolated. 0 reads what was just written.
    #[inline]
    pub fn read(&self, delay_samples: f32) -> f32 {
        let len = self.buf.len();
        let d = delay_samples.clamp(0.0, (len - 2) as f32);
        let whole = d as usize;
        let frac = d - whole as f32;
        // `idx` points at the next write slot, so the newest sample is at `idx - 1`.
        let i0 = (self.idx + len - 1 - whole) % len;
        let i1 = (i0 + len - 1) % len;
        self.buf[i0] + (self.buf[i1] - self.buf[i0]) * frac
    }

    pub fn reset(&mut self) {
        self.buf.fill(0.0);
        self.idx = 0;
    }
}

/// One-pole attack/release envelope over `|x|`.
#[derive(Debug, Clone, Copy)]
pub struct EnvFollower {
    a_coef: f32,
    r_coef: f32,
    level: f32,
}

impl EnvFollower {
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self {
        let mut e = EnvFollower {
            a_coef: 0.0,
            r_coef: 0.0,
            level: 0.0,
        };
        e.set(sample_rate, attack_ms, release_ms);
        e
    }

    pub fn set(&mut self, sample_rate: f32, attack_ms: f32, release_ms: f32) {
        self.a_coef = time_coef(sample_rate, attack_ms);
        self.r_coef = time_coef(sample_rate, release_ms);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let rectified = x.abs();
        let coef = if rectified > self.level {
            self.a_coef
        } else {
            self.r_coef
        };
        self.level = rectified + coef * (self.level - rectified);
        self.level
    }

    #[inline]
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn reset(&mut self) {
        self.level = 0.0;
    }
}

/// The one-pole coefficient that decays to 1/e in `ms`.
#[inline]
fn time_coef(sample_rate: f32, ms: f32) -> f32 {
    (-1.0 / (ms.max(0.01) * 0.001 * sample_rate)).exp()
}

/// A sine LFO, -1 … 1, ticked per sample. The synth's multi-waveform [`Lfo`] is a different
/// animal: it is advanced a block at a time and read without being ticked.
#[derive(Debug, Clone, Copy, Default)]
pub struct SineLfo {
    phase: f32,
    inc: f32,
}

impl SineLfo {
    pub fn set_rate(&mut self, sample_rate: f32, hz: f32) {
        self.inc = hz.max(0.0) / sample_rate;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        let y = (2.0 * PI * self.phase).sin();
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        y
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// A noise gate: opens fast so a pick attack is never clipped, closes over `release_ms` so a
/// decaying note is not chopped off.
#[derive(Debug, Clone, Copy)]
pub struct NoiseGate {
    env: EnvFollower,
    gain: f32,
    threshold: f32,
    open_coef: f32,
    close_coef: f32,
}

/// How fast the gate opens once the signal crosses the threshold.
const GATE_OPEN_MS: f32 = 2.0;

impl NoiseGate {
    pub fn new(sample_rate: f32) -> Self {
        NoiseGate {
            env: EnvFollower::new(sample_rate, 1.0, 20.0),
            gain: 0.0,
            threshold: db_to_linear(-55.0),
            open_coef: time_coef(sample_rate, GATE_OPEN_MS),
            close_coef: time_coef(sample_rate, 120.0),
        }
    }

    pub fn set(&mut self, sample_rate: f32, threshold_db: f32, release_ms: f32) {
        self.threshold = db_to_linear(threshold_db);
        self.open_coef = time_coef(sample_rate, GATE_OPEN_MS);
        self.close_coef = time_coef(sample_rate, release_ms);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let level = self.env.process(x);
        let target = if level > self.threshold { 1.0 } else { 0.0 };
        let coef = if target > self.gain {
            self.open_coef
        } else {
            self.close_coef
        };
        self.gain = target + coef * (self.gain - target);
        x * self.gain
    }

    pub fn reset(&mut self) {
        self.env.reset();
        self.gain = 0.0;
    }
}

/// A feed-forward compressor with a soft knee, in the log domain.
#[derive(Debug, Clone, Copy)]
pub struct Compressor {
    env: EnvFollower,
    threshold_db: f32,
    ratio: f32,
    makeup: f32,
    knee_db: f32,
}

/// Width of the soft knee, in dB either side of the threshold, unless a caller sets its own.
const KNEE_DB: f32 = 6.0;

impl Compressor {
    pub fn new(sample_rate: f32) -> Self {
        Compressor {
            env: EnvFollower::new(sample_rate, 5.0, 120.0),
            threshold_db: -20.0,
            ratio: 4.0,
            makeup: 1.0,
            knee_db: KNEE_DB,
        }
    }

    pub fn set(&mut self, threshold_db: f32, ratio: f32, makeup_db: f32) {
        self.threshold_db = threshold_db;
        self.ratio = ratio.max(1.0);
        self.makeup = db_to_linear(makeup_db);
    }

    /// Attack and release of the level detector. The rig leaves these at the constructor's
    /// 5 / 120 ms; a compressor with its own knobs sets them.
    pub fn set_times(&mut self, sample_rate: f32, attack_ms: f32, release_ms: f32) {
        self.env.set(sample_rate, attack_ms, release_ms);
    }

    /// How wide the bend around the threshold is, in dB either side of it. Zero is a hard
    /// corner, which is what a drum bus often wants and a vocal never does.
    pub fn set_knee(&mut self, knee_db: f32) {
        self.knee_db = knee_db.max(0.0);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        x * self.gain_for(x) * self.makeup
    }

    /// The gain reduction one sample calls for, before makeup. Split out so a stereo-linked
    /// compressor can detect on one signal and apply the result to both sides.
    #[inline]
    pub fn gain_for(&mut self, x: f32) -> f32 {
        let level = self.env.process(x);
        // 1e-7 is about -140 dBFS: below it the gain computer is in the noise anyway, and
        // log10(0) would be -inf.
        let level_db = 20.0 * level.max(1e-7).log10();
        let over = level_db - self.threshold_db;
        let knee = self.knee_db;
        let reduction_db = if over <= -knee {
            0.0
        } else if over >= knee {
            over * (1.0 - 1.0 / self.ratio)
        } else {
            // Quadratic knee: the slope eases in across the knee instead of cornering.
            let t = (over + knee) / (2.0 * knee);
            (1.0 - 1.0 / self.ratio) * knee * t * t
        };
        db_to_linear(-reduction_db)
    }

    /// The makeup gain in use, so a stereo pair can apply it once alongside `gain_for`.
    #[inline]
    pub fn makeup(&self) -> f32 {
        self.makeup
    }

    pub fn reset(&mut self) {
        self.env.reset();
    }
}

// ─── Limiter ─────────────────────────────────────────────────────────────────────────────────

/// Lookahead, in seconds: long enough that the gain ramp has settled by the time the peak it was
/// computed from reaches the output, short enough to stay inaudible against the rest of the chain.
pub const LOOKAHEAD_S: f32 = 0.0015;

/// Nominal release of the gain stage. Program dependence comes from the two-stage detector, not
/// from this number.
pub const LIMITER_RELEASE_MS: f32 = 60.0;

/// One-pole coefficient for a time constant of `samples`.
#[inline]
fn pole(samples: f32) -> f32 {
    (-1.0 / samples.max(1.0)).exp()
}

/// Lookahead peak limiter, stereo-linked.
///
/// The gain requirement is computed from the signal as it arrives and applied to the same signal
/// a lookahead later, so the ramp is already down when the peak gets there: no clipping, and no
/// step in the gain to click. The ceiling is then guaranteed outright by taking the smoothed gain
/// against what the outgoing sample actually needs — the smoother converges to within a fraction
/// of a dB over the lookahead, so that floor binds by a hair on an attack and never otherwise.
#[derive(Debug, Clone)]
pub struct Limiter {
    left: DelayLine,
    right: DelayLine,
    /// Lookahead in samples, as a delay read distance.
    delay: f32,
    /// Instant-attack gain requirement, released toward unity.
    hold: f32,
    /// `hold` smoothed over the lookahead; what actually multiplies the signal.
    gain: f32,
    release_coef: f32,
    smooth_coef: f32,
    sample_rate: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32) -> Self {
        let delay = (LOOKAHEAD_S * sample_rate).round().max(1.0);
        Limiter {
            left: DelayLine::new(delay as usize + 4),
            right: DelayLine::new(delay as usize + 4),
            delay,
            hold: 1.0,
            gain: 1.0,
            // A fifth of the lookahead leaves the ramp within ~0.7% of its target by the time
            // the peak lands, which is the error the output-side floor then absorbs.
            smooth_coef: pole(delay / 5.0),
            release_coef: pole(LIMITER_RELEASE_MS * 0.001 * sample_rate),
            sample_rate,
        }
    }

    /// Samples between a sample going in and coming out: the lookahead, exactly.
    pub fn latency(&self) -> usize {
        self.delay as usize
    }

    /// How fast the gain creeps back to unity. The master bus leaves this at
    /// [`LIMITER_RELEASE_MS`]; the insert effect puts it on a knob.
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_coef = pole(ms.max(1.0) * 0.001 * self.sample_rate);
    }

    #[inline]
    pub fn process(&mut self, a: f32, b: f32, ceiling: f32) -> (f32, f32) {
        let peak = a.abs().max(b.abs());
        let req = if peak > ceiling { ceiling / peak } else { 1.0 };
        // Smooth-decoupled detector in the gain domain: the first stage drops to the requirement
        // at once and creeps back toward unity with the release time, so a run of transients
        // releases as one gesture instead of once per hit.
        self.hold = req.min(self.release_coef * self.hold + (1.0 - self.release_coef) * req);
        self.gain = self.smooth_coef * self.gain + (1.0 - self.smooth_coef) * self.hold;

        let (da, db) = (self.left.read(self.delay), self.right.read(self.delay));
        self.left.write(a);
        self.right.write(b);
        let out_peak = da.abs().max(db.abs());
        let g = if out_peak * self.gain > ceiling {
            ceiling / out_peak
        } else {
            self.gain
        };
        (da * g, db * g)
    }
}

// ─── Oversampling ────────────────────────────────────────────────────────────────────────────

/// Cutoff of the up/down-sampling filters as a fraction of the *base* rate.
const OS_CUTOFF: f32 = 0.45;

/// Two cascaded biquads make a 4th-order Butterworth: Q = 1/(2·cos(π/8)) and 1/(2·cos(3π/8)).
fn butterworth4(sample_rate: f32, cutoff: f32) -> [Biquad; 2] {
    let mut a = Biquad::default();
    let mut b = Biquad::default();
    a.lowpass(sample_rate, cutoff, 0.541_196_1);
    b.lowpass(sample_rate, cutoff, 1.306_563);
    [a, b]
}

/// 2× oversampling around a per-sample nonlinearity: zero-stuff, filter, apply `f` twice,
/// filter, drop one.
///
/// Phase is not linear — these are IIR filters, not half-band FIRs — which for a guitar amp is
/// inaudible and costs no latency, unlike a linear-phase design.
#[derive(Debug, Clone)]
pub struct Over2 {
    up: [Biquad; 2],
    down: [Biquad; 2],
}

impl Over2 {
    pub fn new(sample_rate: f32) -> Self {
        let inner = sample_rate * 2.0;
        Over2 {
            up: butterworth4(inner, sample_rate * OS_CUTOFF),
            down: butterworth4(inner, sample_rate * OS_CUTOFF),
        }
    }

    pub fn reset(&mut self) {
        for b in self.up.iter_mut().chain(self.down.iter_mut()) {
            b.reset();
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32, f: &mut impl FnMut(f32) -> f32) -> f32 {
        // Zero-stuffing halves the amplitude, so the interpolation filter is fed 2x. Both
        // inner samples run through the whole chain — the first result is dropped, which is
        // the decimation; dropping it before the down-filter would defeat the point.
        self.run(x * 2.0, f);
        self.run(0.0, f)
    }

    /// One inner-rate sample through up-filter → nonlinearity → down-filter.
    #[inline]
    fn run(&mut self, x: f32, f: &mut impl FnMut(f32) -> f32) -> f32 {
        let mut v = x;
        for b in &mut self.up {
            v = b.process(v);
        }
        v = f(v);
        for b in &mut self.down {
            v = b.process(v);
        }
        v
    }
}

/// 4× oversampling: two [`Over2`] nested, so the inner function runs at 4× the base rate.
#[derive(Debug, Clone)]
pub struct Over4 {
    outer: Over2,
    inner: Over2,
}

impl Over4 {
    pub fn new(sample_rate: f32) -> Self {
        Over4 {
            outer: Over2::new(sample_rate),
            inner: Over2::new(sample_rate * 2.0),
        }
    }

    pub fn reset(&mut self) {
        self.outer.reset();
        self.inner.reset();
    }

    #[inline]
    pub fn process(&mut self, x: f32, f: &mut impl FnMut(f32) -> f32) -> f32 {
        let Over4 { outer, inner } = self;
        outer.process(x, &mut |y| inner.process(y, f))
    }
}

// ─── Amp modelling ───────────────────────────────────────────────────────────────────────────

/// Points in each tabulated triode curve. 512 across a 10 V grid swing is a point every 20 mV,
/// finer than the curvature, so interpolating between neighbours is inaudible.
const TRIODE_POINTS: usize = 512;
/// Grid swing the tables cover either side of the operating point, in volts. Past this the tube
/// is fully cut off or fully saturated and the curve is flat, so the ends simply clamp.
const TRIODE_SPAN: f32 = 5.0;

/// The two curves that describe one 12AX7 gain stage, solved once for the whole process.
///
/// `plate` is the normalised plate swing against grid voltage — the stage's actual transfer
/// curve, not a `tanh` standing in for one. It is scaled so a fully clipped stage lands at about
/// ±1, and within that scale it is deeply asymmetric: the tube cuts off abruptly a little over one
/// unit below the operating point and saturates gently three units above it. That asymmetry is
/// where a real amp's even harmonics come from, and a symmetric soft-clipper cannot produce them
/// at all.
///
/// `small_signal` is the slope at the operating point, which that scaling leaves well under unity.
/// Callers ask for the stage gain they want and this converts it into grid volts, so the voicing
/// table stays readable.
///
/// `grid` is grid current. It is zero while the grid sits negative and climbs steeply once the
/// grid swings positive. It never shapes the waveform directly; it charges the coupling capacitor,
/// which drags the bias down, so a hard-picked chord chokes and then recovers.
#[derive(Debug)]
struct TriodeCurves {
    plate: [f32; TRIODE_POINTS],
    grid: [f32; TRIODE_POINTS],
    small_signal: f32,
}

static TRIODE: LazyLock<TriodeCurves> = LazyLock::new(TriodeCurves::solve);

impl TriodeCurves {
    /// Norman Koren's triode equations for a 12AX7, walked along a 100 kΩ plate load off a 250 V
    /// supply. Done in `f64` at startup, once: the result is a table, and the audio thread only
    /// ever indexes it.
    fn solve() -> TriodeCurves {
        const MU: f64 = 100.0;
        const EX: f64 = 1.4;
        const KG1: f64 = 1060.0;
        const KP: f64 = 600.0;
        const KVB: f64 = 300.0;
        const SUPPLY: f64 = 250.0;
        const PLATE_R: f64 = 100_000.0;
        /// Cathode bias: how far below conduction the grid idles, and so how much clean headroom
        /// there is before the stage cuts off.
        const BIAS: f64 = -1.5;
        /// Grid-to-cathode resistance once the grid conducts, in ohms.
        const GRID_R: f64 = 1_500.0;

        /// Plate current in amps for a grid and plate voltage.
        fn current(vgk: f64, vpk: f64) -> f64 {
            if vpk <= 0.0 {
                return 0.0;
            }
            let inner = KP * (1.0 / MU + vgk / (KVB + vpk * vpk).sqrt());
            // `ln(1+e^x)` saturates to `x`; past 50 the exponential would overflow for nothing.
            let e1 = (vpk / KP)
                * if inner < 50.0 {
                    (inner.exp()).ln_1p()
                } else {
                    inner
                };
            if e1 <= 0.0 {
                0.0
            } else {
                2.0 * e1.powf(EX) / KG1
            }
        }

        /// Where the load line crosses the tube's curve for this grid voltage. Plate current
        /// falls as plate voltage rises, so the crossing is unique and bisection always lands.
        fn plate_volts(vgk: f64) -> f64 {
            let (mut lo, mut hi) = (0.0, SUPPLY);
            for _ in 0..48 {
                let mid = 0.5 * (lo + hi);
                if SUPPLY - current(vgk, mid) * PLATE_R > mid {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            0.5 * (lo + hi)
        }

        let quiescent = plate_volts(BIAS);
        // Cutoff and saturation, the two walls the stage swings between. Normalising by half the
        // distance between them lands a fully clipped stage at roughly ±1, which keeps the rest of
        // the rig's gain structure readable; the walls stay where the tube put them, so the curve
        // is still lopsided — 2.2 units of headroom one way, 1.1 the other.
        let cutoff = quiescent - plate_volts(BIAS - TRIODE_SPAN as f64);
        let saturation = quiescent - plate_volts(BIAS + TRIODE_SPAN as f64);
        let swing = 0.5 * (saturation - cutoff);

        let mut plate = [0.0f32; TRIODE_POINTS];
        let mut grid = [0.0f32; TRIODE_POINTS];
        for (i, (p, g)) in plate.iter_mut().zip(grid.iter_mut()).enumerate() {
            let offset = TRIODE_SPAN as f64 * (2.0 * i as f64 / (TRIODE_POINTS - 1) as f64 - 1.0);
            let vgk = BIAS + offset;
            // The plate inverts; negate so the table is the stage's non-inverted output.
            *p = ((quiescent - plate_volts(vgk)) / swing) as f32;
            // Grid current, in amps. Only exists once the grid goes positive against the cathode.
            *g = (vgk.max(0.0) / GRID_R) as f32;
        }
        let small_signal =
            ((plate_volts(BIAS - 0.05) - plate_volts(BIAS + 0.05)) / 0.1 / swing) as f32;
        TriodeCurves {
            plate,
            grid,
            small_signal,
        }
    }

    /// Read one curve at `v` volts off the operating point, interpolating between table points
    /// and clamping to the flat ends.
    #[inline]
    fn read(table: &[f32; TRIODE_POINTS], v: f32) -> f32 {
        let pos = (v / TRIODE_SPAN * 0.5 + 0.5) * (TRIODE_POINTS - 1) as f32;
        if pos <= 0.0 {
            return table[0];
        }
        if pos >= (TRIODE_POINTS - 1) as f32 {
            return table[TRIODE_POINTS - 1];
        }
        let i = pos as usize;
        let frac = pos - i as f32;
        table[i] + (table[i + 1] - table[i]) * frac
    }
}

/// Volts of bias droop per amp of grid current: the impedance the grid is driven from. The
/// coupling capacitor charges through it whenever the grid conducts, and until it bleeds back the
/// stage idles colder than it was built to. This is why a hard-strummed chord on a cranked amp
/// chokes for a moment and then blooms back.
const BLOCKING_OHMS: f32 = 2_000.0;

/// One triode gain stage: DC block, pre-emphasis, the tabulated tube curve at 4× rate, then the
/// coupling capacitor's bias droop and a roll-off of the fizz the curve just created.
#[derive(Debug, Clone)]
pub struct TubeStage {
    curves: &'static TriodeCurves,
    dc: Biquad,
    emphasis: Biquad,
    post: Biquad,
    os: Over4,
    drive: f32,
    /// Where the stage idles on the curve. Negative sits it nearer cutoff, which clips the
    /// negative half first and thickens the even harmonics.
    bias: f32,
    /// Charge on the coupling capacitor, in grid volts.
    blocking: f32,
    charge: f32,
    bleed: f32,
}

impl TubeStage {
    pub fn new(sample_rate: f32) -> Self {
        let mut stage = TubeStage {
            curves: &TRIODE,
            dc: Biquad::default(),
            emphasis: Biquad::identity(),
            post: Biquad::default(),
            os: Over4::new(sample_rate),
            drive: 1.0,
            bias: 0.0,
            blocking: 0.0,
            // Conduction charges the cap in a couple of ms; the grid leak resistor takes tens of
            // ms to bleed it back.
            charge: 1.0 - (-1.0f32 / (0.002 * sample_rate)).exp(),
            bleed: 1.0 - (-1.0f32 / (0.060 * sample_rate)).exp(),
        };
        stage.set(sample_rate, 1.0, 0.0, 0.0);
        stage
    }

    /// `gain` is the stage's small-signal voltage gain — one means a clean unity stage, ten means
    /// it is running ten times over and is nowhere near clean. `bias` shifts the idle point along
    /// the curve, and `emphasis_hz` is the band this stage distorts first, which is the single
    /// biggest difference between one amp's preamp and another's.
    pub fn set(&mut self, sample_rate: f32, gain: f32, bias: f32, emphasis_hz: f32) {
        self.drive = gain.max(1e-4) / self.curves.small_signal;
        self.bias = bias;
        self.dc.highpass(sample_rate, 30.0, 0.707);
        if emphasis_hz > 0.0 {
            self.emphasis.peaking(sample_rate, emphasis_hz, 5.0, 0.8);
        } else {
            self.emphasis = Biquad::identity();
        }
        self.post.lowpass(sample_rate, 11_000.0, 0.707);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let v = self.emphasis.process(self.dc.process(x));
        let (curves, drive, bias) = (self.curves, self.drive, self.bias);
        let (charge, bleed) = (self.charge, self.bleed);
        let blocking = &mut self.blocking;
        let y = self.os.process(v, &mut |s| {
            let vg = s * drive + bias - *blocking;
            let droop = TriodeCurves::read(&curves.grid, vg) * BLOCKING_OHMS;
            // Charge fast while the grid conducts, bleed back whenever it does not.
            *blocking += if droop > *blocking {
                (droop - *blocking) * charge
            } else {
                -*blocking * bleed
            };
            TriodeCurves::read(&curves.plate, vg) - TriodeCurves::read(&curves.plate, bias)
        });
        self.post.process(y)
    }

    pub fn reset(&mut self) {
        self.dc.reset();
        self.emphasis.reset();
        self.post.reset();
        self.os.reset();
        self.blocking = 0.0;
    }
}

/// The range a guitar actually occupies, used to measure how loud a tone stack is overall.
const BAND_LOW: f32 = 80.0;
const BAND_HIGH: f32 = 6_400.0;
const BAND_POINTS: usize = 24;

/// The passive Bass/Mid/Treble network between the preamp and the power amp.
///
/// This is the circuit, not three independent bands: one third-order RC network whose knobs load
/// each other, so turning the mid down also moves the treble and drops the overall level. The
/// continuous-time transfer function is the closed form Yeh and Smith derived for the '59 Bassman
/// stack (DAFx-06), discretised here by the bilinear transform. Every amp family in the list is
/// the same topology with different parts.
#[derive(Debug, Clone, Copy)]
pub struct ToneStack {
    b: [f32; 4],
    a: [f32; 3],
    z: [f32; 3],
    /// Make-up for the network's insertion loss at its own mid setting, so switching topology
    /// changes the voice and not the volume.
    makeup: f32,
}

/// One tone stack's parts: treble, bass and mid caps, then treble, bass and mid pots and the
/// slope resistor. Farads and ohms.
#[derive(Debug, Clone, Copy)]
pub struct ToneStackParts {
    pub c1: f64,
    pub c2: f64,
    pub c3: f64,
    pub r1: f64,
    pub r2: f64,
    pub r3: f64,
    pub r4: f64,
}

impl ToneStack {
    pub fn new() -> Self {
        ToneStack {
            b: [1.0, 0.0, 0.0, 0.0],
            a: [0.0; 3],
            z: [0.0; 3],
            makeup: 1.0,
        }
    }

    /// `treble`, `mid` and `bass` are 0…1 pot positions. The bass pot is a log taper on a real
    /// amp, and without that its whole useful range crowds into the first tenth of the sweep.
    pub fn set(
        &mut self,
        sample_rate: f32,
        parts: ToneStackParts,
        treble: f32,
        mid: f32,
        bass: f32,
    ) {
        let l = (10f64.powf(bass.clamp(0.0, 1.0) as f64) - 1.0) / 9.0;
        let (t, m) = (treble.clamp(0.0, 1.0) as f64, mid.clamp(0.0, 1.0) as f64);
        let (b, a) = Self::discretise(sample_rate as f64, parts, t, m, l);
        self.b = b;
        self.a = a;
        // Level every topology by what it does across the guitar's range with its pots at noon,
        // not by one frequency: pinning the scoop's own floor to unity would boost everything
        // either side of it by ten decibels. Switching stack then changes the voice, not the fader.
        let (nb, na) = Self::discretise(
            sample_rate as f64,
            parts,
            0.5,
            0.5,
            (10f64.sqrt() - 1.0) / 9.0,
        );
        let mut sum = 0.0;
        for i in 0..BAND_POINTS {
            let f = BAND_LOW * (BAND_HIGH / BAND_LOW).powf(i as f32 / (BAND_POINTS - 1) as f32);
            let g = Self::magnitude(&nb, &na, f, sample_rate);
            sum += g * g;
        }
        self.makeup = 1.0 / (sum / BAND_POINTS as f32).sqrt().max(1e-3);
    }

    fn discretise(sr: f64, p: ToneStackParts, t: f64, m: f64, l: f64) -> ([f32; 4], [f32; 3]) {
        let ToneStackParts {
            c1,
            c2,
            c3,
            r1,
            r2,
            r3,
            r4,
        } = p;
        let b1 = t * c1 * r1 + m * c3 * r3 + l * (c1 * r2 + c2 * r2) + (c1 * r3 + c2 * r3);
        let b2 = t * (c1 * c2 * r1 * r4 + c1 * c3 * r1 * r4)
            - m * m * (c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
            + m * (c1 * c3 * r1 * r3 + c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
            + l * (c1 * c2 * r1 * r2 + c1 * c2 * r2 * r4 + c1 * c3 * r2 * r4)
            + l * m * (c1 * c3 * r2 * r3 + c2 * c3 * r2 * r3)
            + (c1 * c2 * r1 * r3 + c1 * c2 * r3 * r4 + c1 * c3 * r3 * r4);
        let b3 = l * m * (c1 * c2 * c3 * r1 * r2 * r3 + c1 * c2 * c3 * r2 * r3 * r4)
            - m * m * (c1 * c2 * c3 * r1 * r3 * r3 + c1 * c2 * c3 * r3 * r3 * r4)
            + m * (c1 * c2 * c3 * r1 * r3 * r3 + c1 * c2 * c3 * r3 * r3 * r4)
            + t * c1 * c2 * c3 * r1 * r3 * r4
            - t * m * c1 * c2 * c3 * r1 * r3 * r4
            + t * l * c1 * c2 * c3 * r1 * r2 * r4;
        let a0 = 1.0;
        let a1 = (c1 * r1 + c1 * r3 + c2 * r3 + c2 * r4 + c3 * r4)
            + m * c3 * r3
            + l * (c1 * r2 + c2 * r2);
        let a2 = m
            * (c1 * c3 * r1 * r3 - c2 * c3 * r3 * r4 + c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
            + l * m * (c1 * c3 * r2 * r3 + c2 * c3 * r2 * r3)
            - m * m * (c1 * c3 * r3 * r3 + c2 * c3 * r3 * r3)
            + l * (c1 * c2 * r2 * r4 + c1 * c2 * r1 * r2 + c1 * c3 * r2 * r4 + c2 * c3 * r2 * r4)
            + (c1 * c2 * r1 * r4
                + c1 * c3 * r1 * r4
                + c1 * c2 * r3 * r4
                + c1 * c2 * r1 * r3
                + c1 * c3 * r3 * r4
                + c2 * c3 * r3 * r4);
        let a3 = l * m * (c1 * c2 * c3 * r1 * r2 * r3 + c1 * c2 * c3 * r2 * r3 * r4)
            - m * m * (c1 * c2 * c3 * r1 * r3 * r3 + c1 * c2 * c3 * r3 * r3 * r4)
            + m * (c1 * c2 * c3 * r3 * r3 * r4 + c1 * c2 * c3 * r1 * r3 * r3
                - c1 * c2 * c3 * r1 * r3 * r4)
            + l * c1 * c2 * c3 * r1 * r2 * r4
            + c1 * c2 * c3 * r1 * r3 * r4;

        let c = 2.0 * sr;
        let (c2_, c3_) = (c * c, c * c * c);
        let bz = [
            -b1 * c - b2 * c2_ - b3 * c3_,
            -b1 * c + b2 * c2_ + 3.0 * b3 * c3_,
            b1 * c + b2 * c2_ - 3.0 * b3 * c3_,
            b1 * c - b2 * c2_ + b3 * c3_,
        ];
        let az = [
            -a0 - a1 * c - a2 * c2_ - a3 * c3_,
            -3.0 * a0 - a1 * c + a2 * c2_ + 3.0 * a3 * c3_,
            -3.0 * a0 + a1 * c + a2 * c2_ - 3.0 * a3 * c3_,
            -a0 + a1 * c - a2 * c2_ + a3 * c3_,
        ];
        let n = az[0];
        (
            [
                (bz[0] / n) as f32,
                (bz[1] / n) as f32,
                (bz[2] / n) as f32,
                (bz[3] / n) as f32,
            ],
            [(az[1] / n) as f32, (az[2] / n) as f32, (az[3] / n) as f32],
        )
    }

    /// `|H(e^jw)|` of the discretised stack, used to measure its own insertion loss.
    fn magnitude(b: &[f32; 4], a: &[f32; 3], freq: f32, sample_rate: f32) -> f32 {
        let w = -2.0 * PI * freq / sample_rate;
        let (mut nr, mut ni, mut dr, mut di) = (0.0f32, 0.0f32, 1.0f32, 0.0f32);
        for k in 0..4 {
            let (s, c) = (w * k as f32).sin_cos();
            nr += b[k] * c;
            ni += b[k] * s;
            if k > 0 {
                dr += a[k - 1] * c;
                di += a[k - 1] * s;
            }
        }
        ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-20)).sqrt()
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // Transposed direct form II, third order.
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y + self.z[2];
        self.z[2] = self.b[3] * x - self.a[2] * y;
        y * self.makeup
    }

    pub fn reset(&mut self) {
        self.z = [0.0; 3];
    }
}

impl Default for ToneStack {
    fn default() -> Self {
        ToneStack::new()
    }
}

/// Taps in a cabinet impulse response. At 48 kHz this is 10.7 ms — a guitar speaker's response
/// has decayed into the noise well before that, and a short response keeps the convolution cheap
/// and, more importantly, latency-free, which matters when you are monitoring a live DI.
pub const CAB_TAPS: usize = 512;
/// Mic positions modelled across the cone, from the edge to the dust cap.
const CAB_X: usize = 5;
/// Mic distances modelled, from touching the grille to backed off.
const CAB_Z: usize = 2;
/// How far back the far row of the grid sits, in seconds of propagation delay. The sound leaves
/// the cone at one moment and reaches the capsule at another, and that gap is audible as a
/// softening of the pick attack long before it is audible as a delay.
const CAB_FAR_SECONDS: f32 = 0.0005;
/// Where the cabinet is pinned to unity. The guitar's own register, so switching cabinets or
/// moving the mic changes the colour and not the fader.
const CAB_REFERENCE_HZ: f32 = 1_000.0;

/// How one speaker cabinet behaves, in the few numbers that actually separate a 4×12 from a 1×12.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerVoice {
    /// Where the box stops producing bass, and how resonantly it gives up.
    pub resonance_hz: f32,
    pub resonance_db: f32,
    /// The lower-mid thump of the enclosure.
    pub body_hz: f32,
    pub body_db: f32,
    /// The cone break-up peak: the hard, nasal edge a guitar speaker is famous for.
    pub breakup_hz: f32,
    /// How far above break-up the cone still radiates when the mic is on the dust cap.
    pub top_hz: f32,
}

/// How one microphone colours what it hears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MicVoice {
    pub presence_hz: f32,
    pub presence_db: f32,
    pub presence_q: f32,
    pub top_hz: f32,
}

/// A cabinet and the microphone in front of it, as a grid of impulse responses morphed in two
/// dimensions and convolved with the signal.
///
/// This is how a modelling amp gets a speaker right: a cabinet is a wildly non-minimum-phase,
/// finely structured response that no reasonable stack of biquads reproduces, and where the mic
/// sits in front of it changes that response more than any other single decision in the chain. So
/// the response is sampled over the positions that matter — five across the cone, two distances —
/// and the mic position bilinearly blends the four responses around it into the one actually
/// convolved. Moving the mic is then continuous and cheap: the grid is only rebuilt when the
/// cabinet or the microphone changes.
#[derive(Debug, Clone)]
pub struct CabIr {
    /// Indexed `[distance][across the cone]`.
    grid: [[[f32; CAB_TAPS]; CAB_X]; CAB_Z],
    active: [f32; CAB_TAPS],
    /// Twice the response length, with every sample written in both halves, so the history under
    /// the convolution is always one contiguous slice.
    ring: [f32; CAB_TAPS * 2],
    write: usize,
}

impl CabIr {
    pub fn new() -> Self {
        let mut cab = CabIr {
            grid: [[[0.0; CAB_TAPS]; CAB_X]; CAB_Z],
            active: [0.0; CAB_TAPS],
            ring: [0.0; CAB_TAPS * 2],
            write: 0,
        };
        cab.bypass();
        cab
    }

    /// Pass the signal through untouched — a direct sound, with no speaker in the way.
    pub fn bypass(&mut self) {
        self.active = [0.0; CAB_TAPS];
        self.active[0] = 1.0;
    }

    /// Re-measure the whole grid. Only needed when the cabinet, the microphone or the sample rate
    /// changes; moving the mic re-blends what is already here.
    pub fn bake(&mut self, sample_rate: f32, speaker: SpeakerVoice, mic: MicVoice) {
        for (iz, row) in self.grid.iter_mut().enumerate() {
            let z = iz as f32 / (CAB_Z - 1) as f32;
            for (ix, ir) in row.iter_mut().enumerate() {
                let x = ix as f32 / (CAB_X - 1) as f32;
                measure(ir, sample_rate, speaker, mic, x, z);
            }
        }
    }

    /// Blend the four grid points around `(x, z)`, both 0…1, into the response in use.
    pub fn morph(&mut self, x: f32, z: f32) {
        let fx = x.clamp(0.0, 1.0) * (CAB_X - 1) as f32;
        let fz = z.clamp(0.0, 1.0) * (CAB_Z - 1) as f32;
        let (x0, z0) = (fx as usize, fz as usize);
        let (x1, z1) = ((x0 + 1).min(CAB_X - 1), (z0 + 1).min(CAB_Z - 1));
        let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
        let w = [
            (1.0 - tz) * (1.0 - tx),
            (1.0 - tz) * tx,
            tz * (1.0 - tx),
            tz * tx,
        ];
        let corners = [
            &self.grid[z0][x0],
            &self.grid[z0][x1],
            &self.grid[z1][x0],
            &self.grid[z1][x1],
        ];
        for (i, tap) in self.active.iter_mut().enumerate() {
            *tap = w[0] * corners[0][i]
                + w[1] * corners[1][i]
                + w[2] * corners[2][i]
                + w[3] * corners[3][i];
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let w = self.write;
        self.ring[w] = x;
        self.ring[w + CAB_TAPS] = x;
        self.write = if w == 0 { CAB_TAPS - 1 } else { w - 1 };
        // `history[0]` is the sample just written and the rest run backwards in time, which is
        // exactly the order the taps are in.
        let history = &self.ring[w..w + CAB_TAPS];
        let mut y = 0.0;
        for (h, tap) in history.iter().zip(self.active.iter()) {
            y += h * tap;
        }
        y
    }

    pub fn reset(&mut self) {
        self.ring = [0.0; CAB_TAPS * 2];
        self.write = 0;
    }
}

impl Default for CabIr {
    fn default() -> Self {
        CabIr::new()
    }
}

/// Measure one point of the grid: build the speaker and microphone as a filter cascade, push an
/// impulse through it, and keep the first [`CAB_TAPS`] samples.
///
/// `x` runs 0 at the cone's edge to 1 on the dust cap, `z` runs 0 against the grille to 1 backed
/// off. Those two axes carry the whole craft of micing a guitar cab: the edge is dark and round
/// because the cone beams its highs straight ahead, the dust cap is bright and aggressive because
/// that beam is pointed at the capsule, and close in there is a proximity bass lift that
/// disappears as you pull away.
fn measure(
    ir: &mut [f32; CAB_TAPS],
    sample_rate: f32,
    speaker: SpeakerVoice,
    mic: MicVoice,
    x: f32,
    z: f32,
) {
    let mut chain = [Biquad::identity(); 10];
    chain[0].highpass(sample_rate, speaker.resonance_hz, 0.9);
    chain[1].peaking(sample_rate, speaker.resonance_hz, speaker.resonance_db, 1.4);
    chain[2].peaking(sample_rate, speaker.body_hz, speaker.body_db, 1.0);
    // On the dust cap the break-up peak is in your face; out at the edge it barely shows.
    chain[3].peaking(sample_rate, speaker.breakup_hz, 1.5 + 5.0 * x, 1.3);
    // A cone beams its highs straight ahead, so everything above the break-up region falls away as
    // the capsule moves out towards the surround. A broad shelf, not a dip: it is a radiation
    // pattern, not a resonance — and it leaves the midrange alone, which is why moving the mic
    // changes the brightness of a tone without changing which note you are hearing.
    chain[4].high_shelf(sample_rate, 2_500.0, -10.0 * (1.0 - x), 0.707);
    chain[5].lowpass(
        sample_rate,
        speaker.top_hz * (0.5 + 0.5 * x) * (1.0 - 0.15 * z),
        0.9,
    );
    chain[6].low_shelf(sample_rate, 140.0, 6.0 * (1.0 - z), 0.707);
    chain[7].high_shelf(sample_rate, 7_000.0, -3.5 * z, 0.707);
    chain[8].peaking(
        sample_rate,
        mic.presence_hz,
        mic.presence_db,
        mic.presence_q,
    );
    chain[9].lowpass(sample_rate, mic.top_hz, 0.707);

    let delay = (z * CAB_FAR_SECONDS * sample_rate).round() as usize;
    *ir = [0.0; CAB_TAPS];
    for (i, tap) in ir.iter_mut().enumerate() {
        let mut s = if i == delay { 1.0 } else { 0.0 };
        for f in &mut chain {
            s = f.process(s);
        }
        *tap = s;
    }

    // Truncating a response rings; fade the last quarter out so it does not.
    let fade = CAB_TAPS / 4;
    for (k, tap) in ir[CAB_TAPS - fade..].iter_mut().enumerate() {
        *tap *= 0.5 * (1.0 + (PI * k as f32 / fade as f32).cos());
    }

    // Pin the response to unity in the guitar's own register.
    let w = 2.0 * PI * CAB_REFERENCE_HZ / sample_rate;
    let (mut re, mut im) = (0.0f32, 0.0f32);
    for (i, tap) in ir.iter().enumerate() {
        let (s, c) = (w * i as f32).sin_cos();
        re += tap * c;
        im -= tap * s;
    }
    let scale = 1.0 / (re * re + im * im).sqrt().max(1e-6);
    for tap in ir.iter_mut() {
        *tap *= scale;
    }
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
            for v in [
                wave::saw(t, dt),
                wave::triangle(t, dt),
                wave::pulse(t, dt, 0.5),
                wave::sine(t),
            ] {
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
        for wf in [
            Waveform::Saw,
            Waveform::Square,
            Waveform::Triangle,
            Waveform::Sine,
        ] {
            let mut o = Oscillator::new(wf);
            o.set_frequency(440.0, 44_100.0);
            for _ in 0..10_000 {
                let v = o.tick();
                assert!(v.abs() <= 1.3, "{wf:?} produced {v}");
            }
        }
    }

    /// Peak amplitude at `hz` in `samples`, by one Goertzel bin.
    fn tone_amplitude(samples: &[f32], hz: f32, sample_rate: f32) -> f32 {
        let w = 2.0 * PI * hz / sample_rate;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (n, &s) in samples.iter().enumerate() {
            let p = w * n as f32;
            re += s * p.cos();
            im += s * p.sin();
        }
        2.0 * (re * re + im * im).sqrt() / samples.len() as f32
    }

    #[test]
    fn bandpass_passes_its_centre_and_rejects_the_rest() {
        let sr = 48_000.0;
        let amplitude_at = |hz: f32| {
            let mut f = Biquad::default();
            f.bandpass(sr, 1_000.0, 4.0);
            let out: Vec<f32> = (0..8_000)
                .map(|n| f.process((2.0 * PI * hz * n as f32 / sr).sin()))
                .collect();
            tone_amplitude(&out[4_000..], hz, sr)
        };
        assert!(
            (amplitude_at(1_000.0) - 1.0).abs() < 0.05,
            "centre is not unity"
        );
        assert!(amplitude_at(100.0) < 0.15);
        assert!(amplitude_at(10_000.0) < 0.15);
    }

    #[test]
    fn rig_delay_reads_back_what_it_wrote() {
        let mut d = RigDelay::new(64);
        d.write(1.0);
        for _ in 0..9 {
            d.write(0.0);
        }
        // The impulse was written 10 samples ago, so it sits at delay 9.
        assert!((d.read(9.0) - 1.0).abs() < 1e-6, "got {}", d.read(9.0));
        assert!(d.read(0.0).abs() < 1e-6);
        // Halfway between the impulse and its neighbour.
        assert!(
            (d.read(8.5) - 0.5).abs() < 1e-6,
            "interpolation is off: {}",
            d.read(8.5)
        );
        // Reading past the buffer clamps instead of panicking.
        let _ = d.read(1e9);
        d.reset();
        assert_eq!(d.read(9.0), 0.0);
    }

    #[test]
    fn the_gate_closes_on_hiss_and_opens_for_a_note() {
        let sr = 48_000.0;
        let mut g = NoiseGate::new(sr);
        g.set(sr, -40.0, 50.0);
        // -60 dBFS of hiss: well under the threshold, so it is squashed.
        let quiet = db_to_linear(-60.0);
        let mut out = 0.0f32;
        for n in 0..48_000 {
            out = g
                .process(quiet * (2.0 * PI * 220.0 * n as f32 / sr).sin())
                .abs()
                .max(out);
        }
        assert!(out < quiet * 0.05, "gate leaked {out} of {quiet}");

        // -12 dBFS: it opens, and fast enough that the first 5 ms are mostly through.
        let loud = db_to_linear(-12.0);
        let mut peak = 0.0f32;
        for n in 0..240 {
            peak = g
                .process(loud * (2.0 * PI * 220.0 * n as f32 / sr).sin())
                .abs()
                .max(peak);
        }
        assert!(
            peak > loud * 0.5,
            "gate was too slow to open: {peak} of {loud}"
        );
    }

    #[test]
    fn the_compressor_shrinks_the_range_above_its_threshold() {
        let sr = 48_000.0;
        let steady = |level_db: f32| {
            let mut c = Compressor::new(sr);
            c.set(-20.0, 4.0, 0.0);
            let a = db_to_linear(level_db);
            let mut peak = 0.0f32;
            for n in 0..48_000 {
                let y = c.process(a * (2.0 * PI * 220.0 * n as f32 / sr).sin());
                if n > 24_000 {
                    peak = peak.max(y.abs());
                }
            }
            20.0 * peak.max(1e-7).log10()
        };
        // 24 dB in becomes far less than 24 dB out at 4:1 above the threshold.
        let span = steady(-4.0) - steady(-28.0);
        assert!(
            span > 2.0 && span < 16.0,
            "24 dB of input became {span} dB of output"
        );
        // Below the threshold it is a wire.
        assert!((steady(-40.0) + 40.0).abs() < 1.0);
    }

    #[test]
    fn oversampling_keeps_a_clipped_tone_free_of_fold_back() {
        // 7 kHz hard-driven at 44.1 kHz: the 5th harmonic at 35 kHz folds back to 9.1 kHz,
        // which is inharmonic and plainly audible. Oversampling filters it before it folds.
        let sr = 44_100.0;
        let tone: Vec<f32> = (0..16_384)
            .map(|n| 0.8 * (2.0 * PI * 7_000.0 * n as f32 / sr).sin())
            .collect();
        let mut shape = |x: f32| (x * 10.0).tanh();

        let naive: Vec<f32> = tone.iter().map(|&x| shape(x)).collect();
        let mut os = Over4::new(sr);
        let oversampled: Vec<f32> = tone.iter().map(|&x| os.process(x, &mut shape)).collect();

        let tail = 8_192;
        let alias_naive = tone_amplitude(&naive[tail..], 9_100.0, sr);
        let alias_os = tone_amplitude(&oversampled[tail..], 9_100.0, sr);
        let fundamental = tone_amplitude(&oversampled[tail..], 7_000.0, sr);

        assert!(
            fundamental > 0.3,
            "oversampling swallowed the signal: {fundamental}"
        );
        assert!(
            alias_naive > fundamental * 0.05,
            "the naive case should alias audibly: {alias_naive}"
        );
        assert!(
            alias_os < fundamental * 0.01,
            "fold-back survived oversampling: {alias_os} against {fundamental} (naive {alias_naive})"
        );
    }

    #[test]
    fn reverb_decay_lengthens_the_tail() {
        let tail_energy = |decay: f32| {
            let mut r = Reverb::new(48_000.0);
            r.wet = 1.0;
            r.set_decay(decay);
            let (mut l, mut rr) = r.process(1.0, 1.0);
            for _ in 0..48_000 {
                let (a, b) = r.process(0.0, 0.0);
                l = a;
                rr = b;
            }
            // One more second, then measure what is left.
            let mut energy = 0.0f32;
            for _ in 0..48_000 {
                let (a, b) = r.process(0.0, 0.0);
                energy += a * a + b * b;
            }
            let _ = (l, rr);
            energy
        };
        assert!(
            tail_energy(1.0) > tail_energy(0.2) * 10.0,
            "decay did not change the tail"
        );
    }

    fn test_speaker() -> SpeakerVoice {
        SpeakerVoice {
            resonance_hz: 85.0,
            resonance_db: 3.0,
            body_hz: 180.0,
            body_db: 2.0,
            breakup_hz: 2_300.0,
            top_hz: 5_200.0,
        }
    }

    fn test_mic() -> MicVoice {
        MicVoice {
            presence_hz: 4_500.0,
            presence_db: 5.0,
            presence_q: 1.1,
            top_hz: 7_500.0,
        }
    }

    #[test]
    fn the_mic_position_morph_is_a_blend_of_the_responses_around_it() {
        let mut cab = CabIr::new();
        cab.bake(48_000.0, test_speaker(), test_mic());

        // Halfway between two measured positions is the average of the two, which is what makes
        // dragging the mic continuous instead of a four-step switch.
        cab.morph(0.0, 0.0);
        let edge = cab.active;
        cab.morph(0.25, 0.0);
        let quarter = cab.active;
        cab.morph(0.125, 0.0);
        for i in 0..CAB_TAPS {
            let want = 0.5 * (edge[i] + quarter[i]);
            assert!(
                (cab.active[i] - want).abs() < 1e-6,
                "tap {i} morphed to {} rather than {want}",
                cab.active[i]
            );
        }
    }

    #[test]
    fn every_measured_cabinet_response_is_level_matched_and_finite() {
        let mut cab = CabIr::new();
        cab.bake(48_000.0, test_speaker(), test_mic());
        for z in 0..CAB_Z {
            for x in 0..CAB_X {
                let ir = &cab.grid[z][x];
                assert!(
                    ir.iter().all(|t| t.is_finite()),
                    "position ({x}, {z}) measured a non-finite tap"
                );
                // Pinned at 1 kHz, so moving the mic is a tone control and never a fader.
                let w = 2.0 * PI * CAB_REFERENCE_HZ / 48_000.0;
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (i, tap) in ir.iter().enumerate() {
                    let (s, c) = (w * i as f32).sin_cos();
                    re += tap * c;
                    im -= tap * s;
                }
                let gain = (re * re + im * im).sqrt();
                assert!(
                    (gain - 1.0).abs() < 0.01,
                    "position ({x}, {z}) is {gain} at 1 kHz, not unity"
                );
            }
        }
    }

    #[test]
    fn the_cabinet_convolves_against_the_response_it_was_handed() {
        let mut cab = CabIr::new();
        cab.bake(48_000.0, test_speaker(), test_mic());
        cab.morph(0.6, 0.3);
        let ir = cab.active;

        // An impulse has to come back as the response itself, in order and in phase. This is the
        // one place a ring-buffer off-by-one would hide.
        let mut out = Vec::with_capacity(CAB_TAPS);
        out.push(cab.process(1.0));
        for _ in 1..CAB_TAPS {
            out.push(cab.process(0.0));
        }
        for i in 0..CAB_TAPS {
            assert!(
                (out[i] - ir[i]).abs() < 1e-7,
                "tap {i} came back as {} rather than {}",
                out[i],
                ir[i]
            );
        }
    }

    #[test]
    fn a_tone_stack_scoops_its_mids_and_its_knobs_interact() {
        // The '59 Bassman parts. At noon the network digs a hole in the upper mids; closing the
        // mid pot deepens it, and because it is one network that also moves the treble.
        let parts = ToneStackParts {
            c1: 0.25e-9,
            c2: 20e-9,
            c3: 20e-9,
            r1: 250e3,
            r2: 1e6,
            r3: 25e3,
            r4: 56e3,
        };
        let response = |mid: f32, freq: f32| {
            let mut stack = ToneStack::new();
            stack.set(48_000.0, parts, 0.5, mid, 0.5);
            ToneStack::magnitude(&stack.b, &stack.a, freq, 48_000.0)
        };
        let db = |a: f32, b: f32| 20.0 * (a / b).log10();

        assert!(
            db(response(0.5, 800.0), response(0.5, 200.0)) < -4.0,
            "the stack has no mid scoop at all"
        );
        assert!(
            db(response(0.0, 800.0), response(0.5, 800.0)) < -6.0,
            "closing the mid pot barely moved the mids"
        );
        assert!(
            db(response(0.0, 2_500.0), response(0.5, 2_500.0)) < -1.5,
            "the mid pot left the treble band alone, so the knobs are not loading each other"
        );
        // Passive and capacitor-coupled: a steady offset has to leak away rather than ride along.
        // Measured in the filter, not in `magnitude` — at 1 Hz both sides of that ratio cancel to
        // nothing in `f32` and the answer is noise.
        let mut stack = ToneStack::new();
        stack.set(48_000.0, parts, 0.5, 0.5, 0.5);
        let mut last = 0.0;
        for _ in 0..96_000 {
            last = stack.process(1.0);
        }
        // 1e-2 against a full-scale offset is 40 dB of rejection; the rest is `f32` rounding on a
        // pole that sits a ten-thousandth away from the unit circle.
        assert!(
            last.abs() < 1e-2,
            "the stack passes DC, which a coupling capacitor cannot: {last}"
        );
    }
}
