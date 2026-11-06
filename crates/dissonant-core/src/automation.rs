//! Automation: project parameters that move along the song timeline.
//!
//! A lane is a target plus a list of breakpoints in song ticks. The rule between two points is
//! the tempo lane's, so the same widget can draw both: a [`Curve::Ramp`] point glides linearly
//! from the previous point's value into itself, a [`Curve::Step`] point holds the previous
//! value and jumps on arrival. Before the first point the first point's value holds, and past
//! the last the last one does — a lane is an *override* of the static control for as long as
//! it exists, which is what makes "draw a fade and hear a fade" true without the fader having
//! to agree with the lane's first point.

use crate::document::{InsertTarget, TrackParam};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which parameter of the master bus a lane writes to.
///
/// An enum rather than a bare marker so the master's other controls (reverb send, ceiling) can
/// be automated later without the file format moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MasterParam {
    Gain,
}

/// What a lane writes to.
///
/// `Insert` and `TrackPlugin` name a CLAP parameter by its stable `ParamInfo.id` (the key a
/// project can store across sessions). They are part of the file format from the start so that
/// plugin automation needs no second schema move, but nothing resolves them onto a [`Sequence`]
/// yet — see [`crate::sequence::TrackAutomation`].
///
/// [`Sequence`]: crate::sequence::Sequence
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoTarget {
    Track(Uuid, TrackParam),
    Master(MasterParam),
    /// An insert's CLAP parameter: which chain, which insert, which parameter id.
    Insert(InsertTarget, Uuid, u32),
    /// A track instrument plugin's CLAP parameter.
    TrackPlugin(Uuid, u32),
}

impl AutoTarget {
    /// The track this lane belongs to, if any. Drives the UI's grouping and the normalizer that
    /// drops lanes left behind by a deleted track.
    pub fn track_id(&self) -> Option<Uuid> {
        match self {
            AutoTarget::Track(id, _) | AutoTarget::TrackPlugin(id, _) => Some(*id),
            AutoTarget::Insert(InsertTarget::Track(id), _, _) => Some(*id),
            AutoTarget::Master(_) | AutoTarget::Insert(InsertTarget::Master, _, _) => None,
        }
    }

    /// The value range for this target, or `None` for a plugin parameter whose range only the
    /// plugin knows.
    pub fn range(&self) -> Option<(f64, f64)> {
        match self {
            AutoTarget::Track(_, param) => Some(track_param_range(*param)),
            AutoTarget::Master(MasterParam::Gain) => Some((0.0, 1.5)),
            AutoTarget::Insert(..) | AutoTarget::TrackPlugin(..) => None,
        }
    }

    /// `value` held inside [`AutoTarget::range`]; unbounded targets pass through untouched.
    pub fn clamp(&self, value: f64) -> f64 {
        match self.range() {
            Some((lo, hi)) => value.clamp(lo, hi),
            None => value,
        }
    }
}

/// The range a track parameter's control allows. One table, shared by
/// [`crate::document::Command::SetTrackParam`] and by the lane that writes the same parameter,
/// so a value drawn on a lane and one dragged on a fader are bounded identically.
pub fn track_param_range(param: TrackParam) -> (f64, f64) {
    match param {
        TrackParam::Volume => (0.0, 1.5),
        TrackParam::ReverbSend => (0.0, 1.0),
        TrackParam::Tone => (200.0, 20_000.0),
        TrackParam::Pan => (-1.0, 1.0),
    }
}

/// How a breakpoint is reached from the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Curve {
    /// The previous value holds all the way to this tick, then jumps.
    #[default]
    Step,
    /// Glide linearly from the previous point's value into this one.
    Ramp,
}

/// One point on a lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub tick: i64,
    pub value: f64,
    #[serde(default)]
    pub curve: Curve,
}

impl Breakpoint {
    pub fn new(tick: i64, value: f64, curve: Curve) -> Self {
        Breakpoint {
            id: crate::ids::mint(),
            tick,
            value,
            curve,
        }
    }
}

/// One automated parameter: a target and its points, sorted by tick with at most one per tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationLane {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub target: AutoTarget,
    #[serde(default)]
    pub points: Vec<Breakpoint>,
}

impl AutomationLane {
    pub fn new(target: AutoTarget) -> Self {
        AutomationLane {
            id: crate::ids::mint(),
            target,
            points: Vec::new(),
        }
    }

    /// Sort by tick, drop points at a negative tick and keep only the last of any that share a
    /// tick, so the stored lane always reads as a function of time. Called after every edit and
    /// on decode, because a hand-written or merged file can carry anything.
    pub fn tidy(&mut self) {
        self.points.retain(|p| p.tick >= 0 && p.value.is_finite());
        for p in &mut self.points {
            p.value = self.target.clamp(p.value);
        }
        self.points.sort_by_key(|p| p.tick);
        // `dedup_by` keeps the *first* of a run; the later edit is the one the user just made,
        // so walk it backwards to keep that one instead.
        self.points.reverse();
        self.points.dedup_by_key(|p| p.tick);
        self.points.reverse();
    }

    /// The value in force at `tick`, or `None` when the lane has no points (nothing to
    /// override the static control with).
    pub fn value_at(&self, tick: f64) -> Option<f64> {
        value_at(&self.points, tick)
    }
}

/// A lane flattened onto the timeline the engine plays, in that timeline's ticks.
///
/// Never empty: an automation-free parameter is `None` rather than a curve with no points, so
/// the audio thread's question is "is there a curve?" and not "does this curve say anything?".
#[derive(Debug, Clone, PartialEq)]
pub struct AutomationCurve {
    points: Vec<Breakpoint>,
}

impl AutomationCurve {
    /// `None` when `points` is empty. Assumes the points are already tidy.
    pub fn new(points: Vec<Breakpoint>) -> Option<Self> {
        (!points.is_empty()).then_some(AutomationCurve { points })
    }

    pub fn points(&self) -> &[Breakpoint] {
        &self.points
    }

    /// The value in force at `tick`. Allocation- and branch-light: the audio thread calls this
    /// twice per parameter per sub-block.
    pub fn value_at(&self, tick: f64) -> f64 {
        value_at(&self.points, tick).unwrap_or(0.0)
    }
}

/// The lane rule, shared by the stored lane and the flattened curve.
fn value_at(points: &[Breakpoint], tick: f64) -> Option<f64> {
    let first = points.first()?;
    // Index of the first point strictly after `tick`.
    let i = points.partition_point(|p| (p.tick as f64) <= tick);
    if i == 0 {
        return Some(first.value);
    }
    let prev = &points[i - 1];
    match points.get(i) {
        Some(next) if next.curve == Curve::Ramp && next.tick > prev.tick => {
            let t = (tick - prev.tick as f64) / (next.tick - prev.tick) as f64;
            Some(prev.value + (next.value - prev.value) * t.clamp(0.0, 1.0))
        }
        _ => Some(prev.value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticks::TICKS_PER_BEAT as BEAT;

    fn lane(points: Vec<Breakpoint>) -> AutomationCurve {
        AutomationCurve::new(points).expect("points")
    }

    #[test]
    fn a_ramp_interpolates_and_a_step_holds() {
        let c = lane(vec![
            Breakpoint::new(0, 1.0, Curve::Step),
            Breakpoint::new(4 * BEAT, 0.0, Curve::Ramp),
            Breakpoint::new(8 * BEAT, 1.0, Curve::Step),
        ]);
        assert_eq!(c.value_at(0.0), 1.0);
        assert!((c.value_at(2.0 * BEAT as f64) - 0.5).abs() < 1e-12);
        assert_eq!(c.value_at(4.0 * BEAT as f64), 0.0);
        // Step: the ramp's value holds right up to the step's tick, then jumps.
        assert_eq!(c.value_at(6.0 * BEAT as f64), 0.0);
        assert_eq!(c.value_at(8.0 * BEAT as f64), 1.0);
        // Past the last point the last value stands.
        assert_eq!(c.value_at(99.0 * BEAT as f64), 1.0);
    }

    #[test]
    fn before_the_first_point_the_first_value_holds() {
        let c = lane(vec![Breakpoint::new(4 * BEAT, 0.25, Curve::Ramp)]);
        assert_eq!(c.value_at(0.0), 0.25);
        assert_eq!(c.value_at(10.0 * BEAT as f64), 0.25);
    }

    #[test]
    fn tidy_sorts_clamps_and_keeps_the_last_point_on_a_tick() {
        let track = Uuid::new_v4();
        let mut l = AutomationLane::new(AutoTarget::Track(track, TrackParam::Volume));
        l.points = vec![
            Breakpoint::new(8 * BEAT, 9.0, Curve::Step),
            Breakpoint::new(-1, 0.5, Curve::Step),
            Breakpoint::new(0, 0.2, Curve::Step),
            Breakpoint::new(8 * BEAT, 0.75, Curve::Ramp),
        ];
        l.tidy();
        let shape: Vec<(i64, f64, Curve)> =
            l.points.iter().map(|p| (p.tick, p.value, p.curve)).collect();
        assert_eq!(
            shape,
            vec![(0, 0.2, Curve::Step), (8 * BEAT, 0.75, Curve::Ramp)]
        );
    }

    #[test]
    fn targets_clamp_to_their_control_s_range() {
        let t = Uuid::new_v4();
        assert_eq!(AutoTarget::Track(t, TrackParam::Volume).clamp(9.0), 1.5);
        assert_eq!(AutoTarget::Track(t, TrackParam::Pan).clamp(-9.0), -1.0);
        assert_eq!(AutoTarget::Track(t, TrackParam::Tone).clamp(0.0), 200.0);
        assert_eq!(AutoTarget::Master(MasterParam::Gain).clamp(2.0), 1.5);
        // A plugin parameter's range belongs to the plugin, so the host leaves it alone.
        assert_eq!(AutoTarget::TrackPlugin(t, 7).clamp(42.0), 42.0);
        assert_eq!(AutoTarget::TrackPlugin(t, 7).track_id(), Some(t));
        assert_eq!(
            AutoTarget::Insert(InsertTarget::Track(t), Uuid::nil(), 3).track_id(),
            Some(t)
        );
        assert_eq!(AutoTarget::Master(MasterParam::Gain).track_id(), None);
    }
}
