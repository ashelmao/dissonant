//! The per-note pitch envelope as the voice hears it.
//!
//! The promise the feature rests on is that a bend is retuned *per sample*. Deciding a new
//! frequency once per control block and holding it for sixteen samples is what makes a fast
//! bend zipper, so the trajectory is asserted sample by sample: a ramp never steps, a step
//! steps exactly once and exactly where it was written, and the sound that comes out of the
//! synth actually glides between the two pitches.

use dissonant_core::automation::Curve;
use dissonant_core::sound::{factory_sound, SoundPatch, SynthPatch};
use dissonant_core::PitchPoint;
use dissonant_engine::engine::NoteShape;
use dissonant_engine::synth::{Bend, PolySynth};

const SR: f32 = 48_000.0;
/// 480 ticks (an eighth note at 960 PPQ) per 10 000 samples: a plain, constant tempo.
const SAMPLES_PER_TICK: f64 = 10_000.0 / 480.0;

fn resolved(points: &[PitchPoint], from: u32) -> Bend {
    Bend::new(points, |tick| tick as f64 * SAMPLES_PER_TICK, from)
}

/// The cents the bend hands the oscillator for each of `n` samples.
fn trajectory(mut bend: Bend, n: usize) -> Vec<f32> {
    (0..n).map(|_| bend.next_cents()).collect()
}

/// The largest jump between two neighbouring samples.
fn worst_step(xs: &[f32]) -> f32 {
    xs.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
}

#[test]
fn a_note_with_no_envelope_is_not_a_bend_at_all() {
    let bend = resolved(&[], 0);
    assert!(
        !bend.active(),
        "an unbent note must take the cheap path through the voice"
    );
    assert_eq!(trajectory(bend, 64), vec![0.0; 64]);
}

#[test]
fn a_ramp_never_steps_between_two_samples() {
    // Up a whole tone across 480 ticks = 10 000 samples.
    let env = [
        PitchPoint::new(0, 0.0, Curve::Ramp),
        PitchPoint::new(480, 200.0, Curve::Ramp),
    ];
    let t = trajectory(resolved(&env, 0), 10_001);
    let ideal = 200.0 / 10_000.0;
    assert!(
        worst_step(&t) <= ideal * 1.001,
        "worst jump {} cents against an ideal slope of {ideal}",
        worst_step(&t)
    );
    // And it is a *ramp*, not a slow staircase: every sample moves.
    assert!(t.windows(2).all(|w| w[1] > w[0]));
    assert!((t[0] - 0.0).abs() < 1e-4);
    assert!((t[10_000] - 200.0).abs() < 1e-3, "landed on {}", t[10_000]);
}

#[test]
fn the_trajectory_is_continuous_across_control_block_boundaries() {
    // The voice renders in sixteen-sample blocks; a bend resolved per block would show its
    // seams as a jump every sixteenth sample. Nothing here is special about index 16k.
    let env = [
        PitchPoint::new(0, 0.0, Curve::Ramp),
        PitchPoint::new(120, 150.0, Curve::Ramp),
        PitchPoint::new(240, -150.0, Curve::Ramp),
    ];
    let t = trajectory(resolved(&env, 0), 5_001);
    // Steepest leg: 300 cents across 120 ticks = 2 500 samples.
    let steepest = 300.0 / 2_500.0;
    for (i, w) in t.windows(2).enumerate() {
        assert!(
            (w[1] - w[0]).abs() <= steepest * 1.001,
            "sample {i} jumped {} cents",
            w[1] - w[0]
        );
    }
}

#[test]
fn a_step_point_jumps_once_and_exactly_where_it_was_written() {
    let env = [
        PitchPoint::new(0, 0.0, Curve::Step),
        PitchPoint::new(240, 100.0, Curve::Step),
    ];
    let t = trajectory(resolved(&env, 0), 8_000);
    let at = (240.0 * SAMPLES_PER_TICK) as usize;
    assert!(t[..at].iter().all(|&c| c == 0.0));
    assert!(t[at..].iter().all(|&c| c == 100.0));
    assert_eq!(
        t.windows(2).filter(|w| w[0] != w[1]).count(),
        1,
        "a step holds then jumps once; nothing else moves"
    );
}

#[test]
fn before_the_first_point_the_note_is_simply_in_tune() {
    // A lane holds its first value backwards; an envelope must not — a note that has not bent
    // yet sounds at its own pitch, or every detected bend would start out of tune.
    let env = [PitchPoint::new(240, 200.0, Curve::Ramp)];
    let t = trajectory(resolved(&env, 0), 5_001);
    assert_eq!(t[0], 0.0);
    let at = (240.0 * SAMPLES_PER_TICK) as usize;
    assert!((t[at] - 200.0).abs() < 1e-3);
    // And past the last point it holds rather than snapping back.
    assert!((t[5_000] - 200.0).abs() < 1e-3);
}

#[test]
fn seeking_into_a_note_picks_the_bend_up_mid_flight() {
    let env = [
        PitchPoint::new(0, 0.0, Curve::Ramp),
        PitchPoint::new(480, 200.0, Curve::Ramp),
    ];
    let from_start = trajectory(resolved(&env, 0), 10_001);
    let resumed = trajectory(resolved(&env, 4_000), 2_000);
    // Not bit-identical: walking the ramp accumulates a few thousandths of a cent of f32
    // error over four thousand samples, which is some twelve orders of magnitude below
    // audible. Landing in the same place is the contract, not reproducing the rounding.
    for (i, &c) in resumed.iter().enumerate() {
        assert!(
            (c - from_start[4_000 + i]).abs() < 0.01,
            "sample {i} after a seek: {c} against {}",
            from_start[4_000 + i]
        );
    }
}

#[test]
fn more_points_than_the_cap_are_ignored_rather_than_overflowing() {
    // `sanitized` fits an envelope under the cap before it ever reaches here; the voice is
    // the last line and must not be the one that panics.
    let env: Vec<PitchPoint> = (0..40)
        .map(|i| PitchPoint::new(i * 10, i as f32, Curve::Ramp))
        .collect();
    let t = trajectory(resolved(&env, 0), 4_000);
    assert!(t.iter().all(|c| c.is_finite()));
}

// ---------------------------------------------------------------- the sound

fn sine_patch() -> SynthPatch {
    match factory_sound("sine").map(|s| &s.patch) {
        Some(SoundPatch::Synth(p)) => *p,
        _ => panic!("no factory sine"),
    }
}

fn render(shape: NoteShape, frames: usize) -> Vec<f32> {
    let mut s = PolySynth::new(&sine_patch(), SR, 1);
    s.set_tone_hz(20_000.0);
    s.note_on(69, 100, shape);
    let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
    for (cl, cr) in l.chunks_mut(333).zip(r.chunks_mut(333)) {
        s.render_add(cl, cr);
    }
    l
}

/// Upward zero crossings in a window: one per cycle, so the count is the pitch.
fn cycles(xs: &[f32]) -> usize {
    xs.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
}

#[test]
fn a_held_offset_retunes_the_voice_like_fine_does() {
    // One point at tick 0 is a constant offset, which must sound exactly like the same
    // detune written into `fine` — the envelope is an offset in the same units, not a
    // second, differently scaled control.
    let frames = 9_600;
    let by_fine = render(
        NoteShape {
            fine: 700,
            ..NoteShape::neutral()
        },
        frames,
    );
    let by_envelope = render(
        NoteShape::neutral().bent(resolved(&[PitchPoint::new(0, 700.0, Curve::Step)], 0)),
        frames,
    );
    assert!(cycles(&by_fine) > 100);
    assert_eq!(cycles(&by_fine), cycles(&by_envelope));
}

#[test]
fn the_envelope_rides_on_top_of_fine_rather_than_replacing_it() {
    let frames = 9_600;
    let plain = render(NoteShape::neutral(), frames);
    let both = render(
        NoteShape {
            fine: 1_200,
            ..NoteShape::neutral()
        }
        .bent(resolved(&[PitchPoint::new(0, -1_200.0, Curve::Step)], 0)),
        frames,
    );
    // An octave up and an octave of bend back down is the note itself.
    let ratio = cycles(&both) as f32 / cycles(&plain) as f32;
    assert!((ratio - 1.0).abs() < 0.02, "ratio {ratio}");
}

#[test]
fn a_bend_glides_through_the_pitches_between_and_lands_on_the_far_one() {
    // Up a whole tone across 960 ticks = 20 000 samples, then held for the rest of the note.
    let frames = 96_000;
    let out = render(
        NoteShape::neutral().bent(resolved(
            &[
                PitchPoint::new(0, 0.0, Curve::Ramp),
                PitchPoint::new(960, 200.0, Curve::Ramp),
            ],
            0,
        )),
        frames,
    );
    let plain = render(NoteShape::neutral(), frames);
    let bent = render(
        NoteShape {
            fine: 200,
            ..NoteShape::neutral()
        },
        frames,
    );

    // Halfway through the ramp it sits between the two pitches — not still on the first and
    // not already on the second, which is what a jump at either end would look like.
    let (a, b, mid) = (
        cycles(&plain[8_000..12_000]),
        cycles(&bent[8_000..12_000]),
        cycles(&out[8_000..12_000]),
    );
    assert!(a + 1 < mid && mid + 1 < b, "{a} < {mid} < {b}");

    // It got there gradually: the second half of the ramp runs sharper than the first.
    assert!(cycles(&out[10_000..20_000]) > cycles(&out[0..10_000]));

    // And once the last point is past, it is the bent note and stays there.
    let tail = cycles(&out[48_000..]);
    assert!(
        tail.abs_diff(cycles(&bent[48_000..])) <= 2,
        "held at {tail} against {}",
        cycles(&bent[48_000..])
    );
}
