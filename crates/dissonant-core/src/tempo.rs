//! Ticks ⇄ seconds ⇄ samples, for a constant tempo ([`Tempo`]) and for a song with tempo
//! changes ([`TempoMap`]).
//!
//! Musical time is ticks everywhere else, but the integration here is real-valued — a ramp
//! spends a non-integral number of seconds in every tick — so the map keeps working in f64
//! beats internally and converts at its edges. Positions arrive as `f64` ticks because the
//! playhead sits between ticks within a block.

use crate::ticks::{beats_to_ticks, ticks_to_beats, TICKS_PER_BEAT};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f64,
}

impl Tempo {
    pub const MIN_BPM: f64 = 20.0;
    pub const MAX_BPM: f64 = 300.0;

    pub fn new(bpm: f64) -> Self {
        Tempo {
            bpm: bpm.clamp(Self::MIN_BPM, Self::MAX_BPM),
        }
    }

    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.bpm
    }

    pub fn seconds_for_beats(&self, beats: f64) -> f64 {
        beats * self.seconds_per_beat()
    }

    pub fn beats_for_seconds(&self, seconds: f64) -> f64 {
        seconds / self.seconds_per_beat()
    }

    pub fn samples_per_tick(&self, sample_rate: f64) -> f64 {
        self.seconds_per_beat() * sample_rate / TICKS_PER_BEAT as f64
    }

    pub fn ticks_for_samples(&self, samples: f64, sample_rate: f64) -> f64 {
        samples / self.samples_per_tick(sample_rate)
    }
}

/// A tempo change on the song timeline. With `ramp`, the tempo glides linearly from the
/// previous point's value to this one over the ticks in between; otherwise it jumps here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TempoPoint {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub tick: i64,
    pub bpm: f64,
    #[serde(default)]
    pub ramp: bool,
    /// Schema ≤ 8 only; folded into `tick` by [`crate::model::ProjectModel::normalized`].
    #[serde(default, rename = "beat", skip_serializing)]
    pub legacy_beat: Option<f64>,
}

impl TempoPoint {
    pub fn new(tick: i64, bpm: f64, ramp: bool) -> Self {
        TempoPoint {
            id: crate::ids::mint(),
            tick,
            bpm,
            ramp,
            legacy_beat: None,
        }
    }

    /// The beat field from an older file folded into `tick`.
    pub(crate) fn migrated(&mut self) {
        if let Some(beat) = self.legacy_beat.take() {
            if self.tick == 0 {
                self.tick = beats_to_ticks(beat);
            }
        }
    }
}

/// One segment of the map: from `beat` the tempo is `bpm`, and if `ramp_to` is set it glides
/// linearly to that bpm at the next segment's start.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    beat: f64,
    seconds: f64,
    bpm: f64,
    /// `(end_beat, end_bpm)` for a ramp segment.
    ramp_to: Option<(f64, f64)>,
}

/// Ticks ⇄ seconds over a song with tempo changes. The base tempo holds from tick 0 until
/// the first point. Linear-in-bpm ramps integrate exactly (`60·Δb/Δbpm · ln(bpm1/bpm0)`).
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMap {
    segments: Vec<Segment>,
}

impl Default for TempoMap {
    fn default() -> Self {
        TempoMap::constant(120.0)
    }
}

impl TempoMap {
    pub fn constant(bpm: f64) -> Self {
        TempoMap::new(bpm, &[])
    }

    pub fn new(base_bpm: f64, points: &[TempoPoint]) -> Self {
        let clamp = |b: f64| {
            if b.is_finite() {
                b.clamp(Tempo::MIN_BPM, Tempo::MAX_BPM)
            } else {
                120.0
            }
        };
        let mut pts: Vec<&TempoPoint> = points.iter().filter(|p| p.tick >= 0).collect();
        pts.sort_by_key(|p| p.tick);
        pts.dedup_by(|a, b| a.tick == b.tick);
        // Anchors: (beat, bpm, ramp_into_this_point)
        let mut anchors: Vec<(f64, f64, bool)> = vec![(0.0, clamp(base_bpm), false)];
        for p in pts {
            if p.tick == 0 {
                anchors[0] = (0.0, clamp(p.bpm), false);
            } else {
                anchors.push((ticks_to_beats(p.tick), clamp(p.bpm), p.ramp));
            }
        }
        let mut segments = Vec::with_capacity(anchors.len());
        let mut seconds = 0.0;
        for (i, &(beat, bpm, _)) in anchors.iter().enumerate() {
            let ramp_to = anchors.get(i + 1).filter(|n| n.2).map(|n| (n.0, n.1));
            segments.push(Segment {
                beat,
                seconds,
                bpm,
                ramp_to,
            });
            if let Some(&(nb, nbpm, ramp)) = anchors.get(i + 1) {
                seconds += segment_seconds(beat, bpm, nb, if ramp { nbpm } else { bpm });
            }
        }
        TempoMap { segments }
    }

    /// True when the map is a single constant tempo.
    pub fn is_constant(&self) -> bool {
        self.segments.len() == 1
    }

    pub fn base_bpm(&self) -> f64 {
        self.segments[0].bpm
    }

    fn segment_at_beat(&self, beat: f64) -> &Segment {
        let i = self
            .segments
            .partition_point(|s| s.beat <= beat)
            .saturating_sub(1);
        &self.segments[i]
    }

    fn segment_at_seconds(&self, seconds: f64) -> &Segment {
        let i = self
            .segments
            .partition_point(|s| s.seconds <= seconds)
            .saturating_sub(1);
        &self.segments[i]
    }

    /// Tempo in force at `tick`.
    pub fn bpm_at_tick(&self, tick: f64) -> f64 {
        let beat = tick / TICKS_PER_BEAT as f64;
        let s = self.segment_at_beat(beat);
        match s.ramp_to {
            Some((eb, ebpm)) if eb > s.beat => {
                let t = ((beat - s.beat) / (eb - s.beat)).clamp(0.0, 1.0);
                s.bpm + (ebpm - s.bpm) * t
            }
            _ => s.bpm,
        }
    }

    /// Seconds from tick 0 to `tick`.
    pub fn seconds_at_tick(&self, tick: f64) -> f64 {
        let beat = (tick / TICKS_PER_BEAT as f64).max(0.0);
        let s = self.segment_at_beat(beat);
        let end_bpm = match s.ramp_to {
            Some((eb, ebpm)) if eb > s.beat => {
                let t = ((beat - s.beat) / (eb - s.beat)).clamp(0.0, 1.0);
                s.bpm + (ebpm - s.bpm) * t
            }
            _ => s.bpm,
        };
        s.seconds + segment_seconds(s.beat, s.bpm, beat, end_bpm)
    }

    /// The tick reached after `seconds` from tick 0.
    pub fn tick_at_seconds(&self, seconds: f64) -> f64 {
        let s = self.segment_at_seconds(seconds.max(0.0));
        let dt = seconds.max(0.0) - s.seconds;
        let beat = match s.ramp_to {
            Some((eb, ebpm)) if eb > s.beat && (ebpm - s.bpm).abs() > 1e-9 => {
                // bpm(b) grows linearly in b; seconds(b) = 60·len/Δbpm · ln(bpm(b)/bpm0).
                let len = eb - s.beat;
                let k = (ebpm - s.bpm) / len; // bpm per beat
                let bpm = s.bpm * (dt * k / 60.0).exp();
                let b = s.beat + (bpm - s.bpm) / k;
                b.min(eb).max(s.beat)
                    + if b > eb {
                        (dt - segment_seconds(s.beat, s.bpm, eb, ebpm)) * ebpm / 60.0
                    } else {
                        0.0
                    }
            }
            _ => s.beat + dt * s.bpm / 60.0,
        };
        beat * TICKS_PER_BEAT as f64
    }

    pub fn seconds_for_span(&self, from_tick: f64, to_tick: f64) -> f64 {
        self.seconds_at_tick(to_tick) - self.seconds_at_tick(from_tick)
    }
}

/// Seconds to go from `b0` (at `bpm0`) to `b1`, with the tempo moving linearly to `bpm1`.
fn segment_seconds(b0: f64, bpm0: f64, b1: f64, bpm1: f64) -> f64 {
    let len = b1 - b0;
    if len <= 0.0 {
        return 0.0;
    }
    if (bpm1 - bpm0).abs() < 1e-9 {
        60.0 * len / bpm0
    } else {
        60.0 * len / (bpm1 - bpm0) * (bpm1 / bpm0).ln()
    }
}

/// Swing as a piecewise-linear warp of the tick line. Ticks are grouped in pairs of
/// `grid_ticks` (480 = eighth-note swing); the second half of each pair is pushed later so that
/// it lands at `2·grid·swing/100` instead of `grid`. 50 % is straight, 66.7 % the triplet feel,
/// 75 % a hard shuffle. Monotonic, so note order (and the lengths of notes that straddle a
/// pair) survives.
pub fn swing_warp_ticks(tick: f64, swing_percent: f64, grid_ticks: i64) -> f64 {
    if grid_ticks <= 0 || !(50.0..=75.0).contains(&swing_percent) || swing_percent == 50.0 {
        return tick;
    }
    let grid = grid_ticks as f64;
    let pair = 2.0 * grid;
    let late = pair * swing_percent / 100.0; // where the off-subdivision ends up
    let base = (tick / pair).floor() * pair;
    let u = tick - base;
    let warped = if u <= grid {
        u * (late / grid)
    } else {
        late + (u - grid) * ((pair - late) / grid)
    };
    base + warped
}

impl Default for Tempo {
    fn default() -> Self {
        Tempo { bpm: 120.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_seconds_and_samples() {
        let t = Tempo::new(120.0);
        assert!((t.seconds_for_beats(1.0) - 0.5).abs() < 1e-12);
        assert!((t.beats_for_seconds(2.0) - 4.0).abs() < 1e-12);
        assert!((t.samples_per_tick(44_100.0) - 22_050.0 / TICKS_PER_BEAT as f64).abs() < 1e-9);
        assert!((t.ticks_for_samples(22_050.0, 44_100.0) - TICKS_PER_BEAT as f64).abs() < 1e-9);
    }

    #[test]
    fn swing_warp_moves_offbeats_and_keeps_downbeats() {
        let eighth = TICKS_PER_BEAT / 2;
        let sixteenth = TICKS_PER_BEAT / 4;
        // Straight: identity.
        assert_eq!(swing_warp_ticks(480.0, 50.0, eighth), 480.0);
        // Triplet feel on eighths: the "and" lands at 2/3 of the beat.
        assert!((swing_warp_ticks(480.0, 200.0 / 3.0, eighth) - 640.0).abs() < 1e-9);
        // Downbeats never move.
        for b in [0, 1, 2, 7] {
            let tick = (b * TICKS_PER_BEAT) as f64;
            assert!((swing_warp_ticks(tick, 66.0, eighth) - tick).abs() < 1e-12);
        }
        // Hard shuffle on sixteenths: the second sixteenth of each eighth lands at 3/8 of the beat.
        assert!((swing_warp_ticks(240.0, 75.0, sixteenth) - 360.0).abs() < 1e-12);
        // Monotonic.
        let mut last = -1.0;
        for i in 0..400 {
            let w = swing_warp_ticks((i * 10) as f64, 70.0, eighth);
            assert!(w >= last);
            last = w;
        }
        // Out-of-range swing is ignored.
        assert_eq!(swing_warp_ticks(480.0, 90.0, eighth), 480.0);
    }

    #[test]
    fn tempo_map_steps_and_ramps() {
        let beat = |b: f64| b * TICKS_PER_BEAT as f64;
        let constant = TempoMap::constant(120.0);
        assert!(constant.is_constant());
        assert!((constant.seconds_at_tick(beat(4.0)) - 2.0).abs() < 1e-12);
        assert!((constant.tick_at_seconds(2.0) - beat(4.0)).abs() < 1e-9);

        // 120 for 4 beats, then a jump to 60.
        let step = TempoMap::new(120.0, &[TempoPoint::new(4 * TICKS_PER_BEAT, 60.0, false)]);
        assert_eq!(step.bpm_at_tick(beat(4.0) - 1.0), 120.0);
        assert_eq!(step.bpm_at_tick(beat(4.0)), 60.0);
        assert!((step.seconds_at_tick(beat(8.0)) - (2.0 + 4.0)).abs() < 1e-12);
        assert!((step.tick_at_seconds(5.0) - beat(7.0)).abs() < 1e-9);

        // 120 → 60 ramped over 4 beats: exact integral 60·4/(−60)·ln(0.5) = 4·ln 2 s.
        let ramp = TempoMap::new(120.0, &[TempoPoint::new(4 * TICKS_PER_BEAT, 60.0, true)]);
        assert!((ramp.bpm_at_tick(beat(2.0)) - 90.0).abs() < 1e-12);
        let expected = 4.0 * std::f64::consts::LN_2;
        assert!((ramp.seconds_at_tick(beat(4.0)) - expected).abs() < 1e-9);
        // Inverse round-trips inside and past the ramp.
        for b in [0.0, 1.0, 2.5, 4.0, 6.0, 9.5] {
            let t = ramp.seconds_at_tick(beat(b));
            assert!(
                (ramp.tick_at_seconds(t) - beat(b)).abs() < 1e-4,
                "tick {} → {t} s",
                beat(b)
            );
        }
        // Unsorted and duplicate points are tolerated; a point at 0 replaces the base.
        let messy = TempoMap::new(
            100.0,
            &[
                TempoPoint::new(8 * TICKS_PER_BEAT, 140.0, false),
                TempoPoint::new(0, 90.0, false),
                TempoPoint::new(8 * TICKS_PER_BEAT, 150.0, false),
            ],
        );
        assert_eq!(messy.base_bpm(), 90.0);
        assert_eq!(messy.bpm_at_tick(beat(9.0)), 140.0);
    }

    #[test]
    fn clamps_bpm() {
        assert_eq!(Tempo::new(5.0).bpm, Tempo::MIN_BPM);
        assert_eq!(Tempo::new(999.0).bpm, Tempo::MAX_BPM);
    }
}
