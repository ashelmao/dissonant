//! The per-note pitch envelope: what a note may store, what a file may carry into it, and
//! what a dense detected contour decimates down to.
//!
//! The envelope is an *offset* on top of `fine`, so every assertion here is paired with the
//! fact that `fine` itself is untouched: adding a bend must not retune the note it rides on.

use dissonant_core::automation::Curve;
use dissonant_core::document::Command;
use dissonant_core::*;

const BEAT: i64 = TICKS_PER_BEAT;

fn bent(points: Vec<PitchPoint>) -> NoteEvent {
    let mut n = NoteEvent::new(0, BEAT, 60);
    n.pitch_envelope = points;
    n
}

#[test]
fn a_note_starts_with_no_envelope_and_costs_nothing_in_the_file() {
    let n = NoteEvent::new(0, BEAT, 60);
    assert!(n.pitch_envelope.is_empty());
    let json = serde_json::to_string(&n).unwrap();
    assert!(
        !json.contains("pitchEnvelope"),
        "an unbent note must not write the key at all: {json}"
    );
}

#[test]
fn a_file_written_before_envelopes_existed_opens_with_an_empty_one() {
    let old = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "startTick": 0, "lengthTicks": 480, "pitch": 60, "velocity": 100, "fine": -25,
    });
    let n: NoteEvent = serde_json::from_value(old).unwrap();
    assert!(n.pitch_envelope.is_empty());
    assert_eq!(n.fine, -25, "the base detune is untouched by the new field");
}

#[test]
fn the_envelope_round_trips_through_json() {
    let n = bent(vec![
        PitchPoint::new(0, 0.0, Curve::Step),
        PitchPoint::new(240, 203.5, Curve::Ramp),
        PitchPoint::new(480, 0.0, Curve::Ramp),
    ]);
    let back: NoteEvent = serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
    assert_eq!(back.pitch_envelope, n.pitch_envelope);
}

#[test]
fn sanitizing_clamps_cents_to_the_fine_range() {
    let n = bent(vec![
        PitchPoint::new(0, 5_000.0, Curve::Ramp),
        PitchPoint::new(240, -5_000.0, Curve::Ramp),
    ])
    .sanitized();
    let max = MAX_FINE_CENTS as f32;
    assert_eq!(
        n.pitch_envelope.iter().map(|p| p.cents).collect::<Vec<_>>(),
        vec![max, -max]
    );
}

#[test]
fn sanitizing_sorts_drops_junk_and_keeps_the_later_of_two_points_on_one_tick() {
    let n = bent(vec![
        PitchPoint::new(480, 100.0, Curve::Ramp),
        PitchPoint::new(-10, 50.0, Curve::Ramp),
        PitchPoint::new(0, f32::NAN, Curve::Ramp),
        PitchPoint::new(240, 25.0, Curve::Step),
        PitchPoint::new(240, 75.0, Curve::Ramp),
    ])
    .sanitized();
    assert_eq!(
        n.pitch_envelope,
        vec![
            PitchPoint::new(240, 75.0, Curve::Ramp),
            PitchPoint::new(480, 100.0, Curve::Ramp),
        ]
    );
}

#[test]
fn sanitizing_fits_an_oversized_envelope_under_the_cap_without_losing_its_ends() {
    // A triangle: up for twenty points, down for twenty. Far over the cap.
    let points: Vec<PitchPoint> = (0..40)
        .map(|i| {
            let cents = if i <= 20 { i as f32 * 10.0 } else { (40 - i) as f32 * 10.0 };
            PitchPoint::new(i as i64 * 24, cents, Curve::Ramp)
        })
        .collect();
    let n = bent(points).sanitized();
    let env = &n.pitch_envelope;
    assert!(env.len() <= MAX_PITCH_POINTS, "{} points", env.len());
    assert_eq!((env[0].tick, env[0].cents), (0, 0.0));
    assert_eq!(env.last().unwrap().tick, 39 * 24);
    // The peak is the single most significant point and must survive.
    assert!(env.iter().any(|p| p.cents == 200.0));
    assert!(env.windows(2).all(|w| w[0].tick < w[1].tick));
}

#[test]
fn an_envelope_survives_the_edit_path_and_leaves_fine_alone() {
    let mut m = ProjectModel::starter();
    m.patterns[0].notes = vec![NoteEvent::new(0, BEAT, 60)];
    let pattern_id = m.patterns[0].id;
    let mut d = Document::new(m);

    let mut note = d.model().patterns[0].notes[0].clone();
    note.fine = -30;
    note.pitch_envelope = vec![PitchPoint::new(120, 200.0, Curve::Ramp)];
    d.apply_labeled(
        Command::SetNotes {
            pattern_id,
            notes: vec![note],
        },
        false,
        Some("bend note".into()),
    )
    .unwrap();

    let stored = &d.model().patterns[0].notes[0];
    assert_eq!(stored.fine, -30, "the envelope is an offset, not a replacement");
    assert_eq!(stored.pitch_envelope.len(), 1);

    d.undo();
    assert!(d.model().patterns[0].notes[0].pitch_envelope.is_empty());
}

// ---------------------------------------------------------------- decimation

/// A dense contour sampled every `step` ticks from a function of ticks, the way a detector
/// hands one over: one reading per analysis hop.
fn dense(step: i64, count: usize, f: impl Fn(i64) -> f32) -> Vec<(i64, f32)> {
    (0..count as i64).map(|i| (i * step, f(i * step))).collect()
}

/// Worst vertical error, in cents, between the decimated line and the contour it came from.
fn worst_error(dense: &[(i64, f32)], env: &[PitchPoint]) -> f32 {
    let at = |tick: i64| -> f32 {
        // The reading rule: zero before the first point, ramps between, last value after.
        let i = env.partition_point(|p| p.tick <= tick);
        let (prev_tick, prev) = if i == 0 {
            (0, 0.0)
        } else {
            (env[i - 1].tick, env[i - 1].cents)
        };
        match env.get(i) {
            Some(next) if next.curve == Curve::Ramp && next.tick > prev_tick => {
                let t = (tick - prev_tick) as f32 / (next.tick - prev_tick) as f32;
                prev + (next.cents - prev) * t
            }
            _ => prev,
        }
    };
    dense
        .iter()
        .map(|&(tick, cents)| (cents - at(tick)).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_straight_contour_decimates_to_its_two_ends() {
    let line = dense(10, 60, |t| t as f32 / 10.0);
    let env = decimate_pitch_envelope(&line, PITCH_DECIMATE_CENTS);
    assert_eq!(
        env,
        vec![
            PitchPoint::new(0, 0.0, Curve::Ramp),
            PitchPoint::new(590, 59.0, Curve::Ramp),
        ]
    );
}

#[test]
fn a_contour_shorter_than_two_readings_is_kept_whole() {
    assert!(decimate_pitch_envelope(&[], PITCH_DECIMATE_CENTS).is_empty());
    assert_eq!(
        decimate_pitch_envelope(&[(0, 12.0)], PITCH_DECIMATE_CENTS),
        vec![PitchPoint::new(0, 12.0, Curve::Ramp)]
    );
}

#[test]
fn a_bend_and_release_keeps_the_turn_and_stays_inside_the_tolerance() {
    // Up a whole tone over half a beat, held, then released: the shape of a blues bend.
    let contour = dense(5, 193, |t| {
        let x = t as f32;
        if x < 240.0 {
            200.0 * (x / 240.0)
        } else if x < 480.0 {
            200.0
        } else {
            200.0 * (1.0 - (x - 480.0) / 240.0).max(0.0)
        }
    });
    let env = decimate_pitch_envelope(&contour, PITCH_DECIMATE_CENTS);
    assert!(env.len() <= MAX_PITCH_POINTS);
    assert!(
        env.len() <= 6,
        "a four-corner shape should not need {} points",
        env.len()
    );
    assert!(
        worst_error(&contour, &env) <= PITCH_DECIMATE_CENTS,
        "worst error {} cents",
        worst_error(&contour, &env)
    );
    assert_eq!(env[0].tick, 0);
    assert_eq!(env.last().unwrap().tick, 960);
}

#[test]
fn noise_under_the_tolerance_is_thrown_away() {
    // A flat note with ±2 cents of detector jitter — under four cents peak to peak, so no
    // straight line through it is ever five cents out and nothing is worth a point.
    let contour = dense(5, 100, |t| if (t / 5) % 2 == 0 { 2.0 } else { -2.0 });
    let env = decimate_pitch_envelope(&contour, PITCH_DECIMATE_CENTS);
    assert_eq!(env.len(), 2, "{env:?}");
}

#[test]
fn a_vibrato_denser_than_the_cap_spends_its_budget_on_the_biggest_swings() {
    // Eight cycles of ±60 cent vibrato over two beats, sampled every 5 ticks: far more turns
    // than sixteen points can describe.
    let contour = dense(5, 385, |t| {
        (t as f32 / 1_920.0 * 8.0 * std::f32::consts::TAU).sin() * 60.0
    });
    let env = decimate_pitch_envelope(&contour, 1.0);
    assert_eq!(env.len(), MAX_PITCH_POINTS, "the cap is the budget");
    assert!(env.windows(2).all(|w| w[0].tick < w[1].tick));
    assert_eq!(env[0].tick, 0);
    assert_eq!(env.last().unwrap().tick, 1_920);
    // Every kept point sits near a crest or a trough rather than on the flat crossings.
    assert!(
        env[1..env.len() - 1].iter().any(|p| p.cents.abs() > 50.0),
        "{env:?}"
    );
}

#[test]
fn a_decimated_contour_is_already_tidy() {
    let contour = dense(5, 193, |t| (t as f32 / 960.0) * 200.0);
    let env = decimate_pitch_envelope(&contour, PITCH_DECIMATE_CENTS);
    assert_eq!(tidy_pitch_envelope(env.clone()), env);
}
