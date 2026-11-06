//! Monophonic pitch detection for the tuner.
//!
//! **YIN, not plain autocorrelation.** A guitar's low E is the worst case for autocorrelation:
//! the second harmonic is often louder than the fundamental, so the correlation peak at half the
//! period wins and the tuner says E3 while the player is tuning E2. YIN's cumulative mean
//! normalised difference function divides each candidate period by the running mean of the
//! shorter ones, which deliberately penalises those sub-multiples, and its absolute threshold
//! takes the *first* dip that is good enough rather than the deepest — the two things that make
//! octave errors rare instead of routine. The cost over raw autocorrelation is one extra pass.
//!
//! Detection runs on the audio thread, from [`crate::engine::Engine::process`], so it has to be
//! cheap and allocation-free. Two stages buy that:
//!
//! 1. The signal is low-passed and decimated to ~12 kHz, where a 2·W search over a 32 ms window
//!    costs ~150 k multiply-adds per hop — a guitar fundamental never goes near 6 kHz, so
//!    nothing that matters is thrown away.
//! 2. The winning period is then refined against the *undecimated* window over ±`decimation`
//!    samples with parabolic interpolation. At 12 kHz one sample of period is ~50 cents on the
//!    high E, which would be useless for tuning; at 48 kHz with interpolation it is well under
//!    one cent, which is what a tuner has to resolve.
//!
//! Naming the string, the A4 reference and the alternate tunings live in the UI (`ui/src/tuner.ts`):
//! this module only answers "what frequency is being played", which is the part that needs the
//! samples.
//!
//! The two stages are split out as [`Conditioner`] (DC block, anti-alias, decimate) and [`Yin`]
//! (the search over one window) so offline DI detection (`crate::detect`) runs exactly the
//! detector the tuner does, over a whole take instead of a ring buffer. Two YINs would drift.

/// Target rate for the search stage. A guitar's fundamental tops out around 1.3 kHz (high E,
/// 24th fret), so this leaves several harmonics and still cuts the work by 4× at 48 kHz.
pub(crate) const SEARCH_RATE: f32 = 12_000.0;

/// Half the search window, in decimated samples, and so also the longest period considered.
/// 384 at 12 kHz is 32 ms: two whole periods of 62 Hz, which covers a drop-D low string even
/// with the reference pulled down to 415 Hz.
pub(crate) const HALF_WINDOW: usize = 384;

/// How often detection runs, in device samples. ~43 ms at 48 kHz: faster than the eye needs and
/// slow enough that the search is a rounding error on the block budget.
const HOP: usize = 2048;

/// Below this the input is silence or hum, not a plucked string.
const MIN_PEAK: f32 = 0.003;

/// YIN's absolute threshold: the first dip under this wins, sub-multiples and all.
const THRESHOLD: f32 = 0.15;

/// A dip worse than this is not a note; the tuner shows nothing rather than a guess.
const MAX_DIP: f32 = 0.35;

/// What the tracker last heard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pitch {
    pub hz: f32,
    /// 0…1, how periodic the window was. Low means a chord, a mute or a room.
    pub clarity: f32,
}

/// The front end both detectors share: a DC blocker on the full-rate signal, then two cascaded
/// one-poles and a decimator down to ~[`SEARCH_RATE`].
#[derive(Debug, Clone)]
pub(crate) struct Conditioner {
    /// Device samples per search sample.
    decimation: usize,
    /// Samples into the current decimation group.
    phase: usize,
    /// DC blocker and the two-pole anti-alias filter, in that order.
    dc: (f32, f32),
    lp: (f32, f32),
    lp_coeffs: (f32, f32, f32),
}

impl Conditioner {
    pub(crate) fn new(sample_rate: f32) -> Conditioner {
        let decimation = ((sample_rate / SEARCH_RATE).round() as usize).max(1);
        // Two cascaded one-poles at a quarter of the decimated rate. A single pole would leak
        // the top string's upper harmonics past the new Nyquist, where they fold back down into
        // the band the search actually reads.
        let cutoff = (sample_rate / decimation as f32) * 0.25;
        Conditioner {
            decimation,
            phase: 0,
            dc: (0.0, 0.0),
            lp: (0.0, 0.0),
            lp_coeffs: one_pole_pair(cutoff, sample_rate),
        }
    }

    pub(crate) fn decimation(&self) -> usize {
        self.decimation
    }

    pub(crate) fn reset(&mut self) {
        self.phase = 0;
        self.dc = (0.0, 0.0);
        self.lp = (0.0, 0.0);
    }

    /// One input sample in: the DC-blocked full-rate sample, and the decimated one on the
    /// first sample of every decimation group.
    #[inline]
    pub(crate) fn push(&mut self, s: f32) -> (f32, Option<f32>) {
        // DC blocker: a cheap interface can sit a long way off zero, and an offset walks
        // straight into the difference function as a constant the dips have to climb.
        let x = s - self.dc.0 + 0.995 * self.dc.1;
        self.dc = (s, x);
        let (a, b1, b2) = self.lp_coeffs;
        let y1 = a * x + b1 * self.lp.0;
        let y2 = a * y1 + b2 * self.lp.1;
        self.lp = (y1, y2);
        let low = (self.phase == 0).then_some(y2);
        self.phase = (self.phase + 1) % self.decimation;
        (x, low)
    }
}

/// One YIN search: owns only its difference-function scratch, so it never allocates per call.
#[derive(Debug, Clone)]
pub(crate) struct Yin {
    diff: Vec<f32>,
}

impl Yin {
    pub(crate) fn new() -> Yin {
        Yin {
            diff: vec![0.0; HALF_WINDOW + 1],
        }
    }

    /// Search one window. `low` is `2 · HALF_WINDOW` decimated samples in time order; `full`
    /// is the same stretch undecimated, `low.len() · decimation` samples, used to refine the
    /// winning period. `None` for silence, or for a window with no period worth the name.
    pub(crate) fn search(
        &mut self,
        low: &[f32],
        full: &[f32],
        decimation: usize,
        sample_rate: f32,
    ) -> Option<Pitch> {
        let w = HALF_WINDOW;
        debug_assert_eq!(low.len(), 2 * w);
        let peak = low.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        if peak < MIN_PEAK {
            return None;
        }

        // Difference function over the first half, shifted by every candidate period.
        self.diff[0] = 1.0;
        for tau in 1..=w {
            let mut sum = 0.0f32;
            for j in 0..w {
                let d = low[j] - low[j + tau];
                sum += d * d;
            }
            self.diff[tau] = sum;
        }
        // Cumulative mean normalisation: this is the step that stops the octave below winning.
        let mut running = 0.0f32;
        for tau in 1..=w {
            running += self.diff[tau];
            self.diff[tau] = if running > 0.0 {
                self.diff[tau] * tau as f32 / running
            } else {
                1.0
            };
        }

        // The first dip under the threshold, followed down to its local minimum. Falling back
        // to the global minimum keeps a quiet but clean string from reading as nothing.
        let mut best = 2;
        let mut found = false;
        let mut tau = 2;
        while tau < w {
            if self.diff[tau] < THRESHOLD {
                while tau + 1 < w && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                best = tau;
                found = true;
                break;
            }
            tau += 1;
        }
        if !found {
            best = (2..w).min_by(|&a, &b| self.diff[a].total_cmp(&self.diff[b]))?;
        }
        let dip = self.diff[best];
        if dip > MAX_DIP {
            return None;
        }

        let period = self.refine(best, full, decimation)?;
        let hz = sample_rate / period;
        if !hz.is_finite() || !(20.0..=5_000.0).contains(&hz) {
            return None;
        }
        Some(Pitch {
            hz,
            clarity: (1.0 - dip).clamp(0.0, 1.0),
        })
    }

    /// The period in device samples, searched around `coarse · decimation` and interpolated.
    fn refine(&self, coarse: usize, full: &[f32], d: usize) -> Option<f32> {
        if d == 1 {
            return parabolic(
                self.diff[coarse.saturating_sub(1).max(1)],
                self.diff[coarse],
                self.diff[(coarse + 1).min(HALF_WINDOW)],
                coarse as f32,
            );
        }
        let w = full.len() / 2;
        let centre = coarse * d;
        let lo = centre.saturating_sub(d + 1).max(1);
        let hi = (centre + d + 1).min(w - 1);
        if hi <= lo {
            return None;
        }
        let at = |tau: usize| {
            let mut sum = 0.0f32;
            for j in 0..w {
                let diff = full[j] - full[j + tau];
                sum += diff * diff;
            }
            sum
        };
        let mut best = lo;
        let mut best_val = f32::INFINITY;
        for tau in lo..=hi {
            let v = at(tau);
            if v < best_val {
                best_val = v;
                best = tau;
            }
        }
        if best <= lo || best >= hi {
            // The dip sits on the edge of the search: trust the coarse period rather than
            // interpolate off a slope that is still falling.
            return Some(best as f32);
        }
        parabolic(at(best - 1), best_val, at(best + 1), best as f32)
    }
}

/// Rolling YIN detector over the live input.
///
/// Built once per engine (it owns its scratch), reset when the tuner opens so a stale reading
/// never shows up under a new string.
#[derive(Debug)]
pub struct PitchTracker {
    sample_rate: f32,
    conditioner: Conditioner,
    /// The undecimated window, circular, `2 · HALF_WINDOW · decimation` long.
    full: Vec<f32>,
    full_at: usize,
    /// The decimated window, circular, `2 · HALF_WINDOW` long.
    low: Vec<f32>,
    low_at: usize,
    /// Device samples since the last search.
    since: usize,
    /// How much of the window is real audio rather than the zeros it started with.
    filled: usize,
    /// Both windows unrolled into time order for the search.
    window: Vec<f32>,
    full_window: Vec<f32>,
    yin: Yin,
    last: Option<Pitch>,
}

impl PitchTracker {
    pub fn new(sample_rate: f32) -> PitchTracker {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 1_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        let conditioner = Conditioner::new(sample_rate);
        let low_len = 2 * HALF_WINDOW;
        let full_len = low_len * conditioner.decimation();
        PitchTracker {
            sample_rate,
            conditioner,
            full: vec![0.0; full_len],
            full_at: 0,
            low: vec![0.0; low_len],
            low_at: 0,
            since: 0,
            filled: 0,
            window: vec![0.0; low_len],
            full_window: vec![0.0; full_len],
            yin: Yin::new(),
            last: None,
        }
    }

    /// Forget everything heard so far. The window is zeroed, so the next reading waits for a
    /// whole new one rather than mixing in whatever was playing before the tuner opened.
    pub fn reset(&mut self) {
        self.full.fill(0.0);
        self.low.fill(0.0);
        self.full_at = 0;
        self.low_at = 0;
        self.since = 0;
        self.filled = 0;
        self.conditioner.reset();
        self.last = None;
    }

    /// The last search's result: `None` both before the first search and when the last one
    /// heard nothing with a pitch in it.
    pub fn last(&self) -> Option<Pitch> {
        self.last
    }

    /// Feed one block of mono input. True on the blocks where the search actually ran, so the
    /// caller knows a `None` in [`PitchTracker::last`] is fresh rather than simply unanswered.
    pub fn push(&mut self, block: &[f32]) -> bool {
        for &s in block {
            let (x, low) = self.conditioner.push(s);
            self.full[self.full_at] = x;
            self.full_at = (self.full_at + 1) % self.full.len();
            if let Some(y) = low {
                self.low[self.low_at] = y;
                self.low_at = (self.low_at + 1) % self.low.len();
            }
            self.filled = (self.filled + 1).min(self.full.len());
            self.since += 1;
        }
        if self.since < HOP || self.filled < self.full.len() {
            return false;
        }
        self.since = 0;
        let n = self.low.len();
        for i in 0..n {
            self.window[i] = self.low[(self.low_at + i) % n];
        }
        let n = self.full.len();
        for i in 0..n {
            self.full_window[i] = self.full[(self.full_at + i) % n];
        }
        self.last = self.yin.search(
            &self.window,
            &self.full_window,
            self.conditioner.decimation(),
            self.sample_rate,
        );
        true
    }
}

/// The minimum of the parabola through three evenly spaced points, as an offset from `x1`.
fn parabolic(y0: f32, y1: f32, y2: f32, x1: f32) -> Option<f32> {
    let denom = y0 + y2 - 2.0 * y1;
    let x = if denom.abs() < f32::EPSILON {
        x1
    } else {
        x1 + 0.5 * (y0 - y2) / denom
    };
    (x.is_finite() && x > 0.0).then_some(x)
}

/// Coefficients for two cascaded one-pole lowpasses at `cutoff`: `(gain, pole1, pole2)`.
fn one_pole_pair(cutoff: f32, sample_rate: f32) -> (f32, f32, f32) {
    let x = (-2.0 * std::f32::consts::PI * cutoff.min(sample_rate * 0.45) / sample_rate).exp();
    (1.0 - x, x, x)
}
