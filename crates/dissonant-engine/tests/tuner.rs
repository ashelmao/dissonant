//! Pitch detection, against tones we know the frequency of.
//!
//! A tuner is only worth having if it is right to a cent or two, and the failure that actually
//! bites on a guitar is the octave error — the low strings put more energy in the second
//! harmonic than the first. Every test here is the same shape: synthesize a string-like tone at
//! a known frequency, feed it in blocks the way the audio callback does, and assert the reading
//! in cents.

use dissonant_engine::PitchTracker;

/// Cents between two frequencies — the unit the tuner is judged in.
fn cents(got: f32, want: f32) -> f32 {
    1200.0 * (got / want).log2()
}

/// A plucked string: a fundamental under a louder second harmonic, which is what makes a low E
/// read an octave high on a naive autocorrelation.
fn pluck(phase: f32) -> f32 {
    0.30 * phase.sin() + 0.50 * (2.0 * phase).sin() + 0.22 * (3.0 * phase).sin() + 0.10 * (4.0 * phase).sin()
}

/// Feed `seconds` of `hz` through the tracker in 256-frame blocks and return the last reading.
fn read(hz: f32, rate: f32, seconds: f32) -> Option<(f32, f32)> {
    let mut tracker = PitchTracker::new(rate);
    let mut phase = 0.0f32;
    let mut block = [0.0f32; 256];
    let step = 2.0 * std::f32::consts::PI * hz / rate;
    let mut done = 0;
    let frames = (rate * seconds) as usize;
    while done < frames {
        for s in block.iter_mut() {
            *s = pluck(phase);
            phase = (phase + step) % (2.0 * std::f32::consts::PI);
        }
        tracker.push(&block);
        done += block.len();
    }
    tracker.last().map(|p| (p.hz, p.clarity))
}

#[test]
fn every_open_string_in_standard_tuning_reads_within_two_cents() {
    // E2 A2 D3 G3 B3 E4 at A4 = 440.
    for &want in &[82.41, 110.0, 146.83, 196.0, 246.94, 329.63] {
        let (hz, clarity) = read(want, 48_000.0, 0.6).unwrap_or_else(|| panic!("no reading at {want} Hz"));
        assert!(
            cents(hz, want).abs() < 2.0,
            "{want} Hz read as {hz} Hz ({:.1} cents off)",
            cents(hz, want)
        );
        assert!(clarity > 0.8, "{want} Hz read with clarity {clarity}");
    }
}

#[test]
fn a_dropped_low_string_at_a_flat_reference_still_reads_as_itself() {
    // The lowest note the tuner has to cover: drop D with A4 pulled down to 415 Hz.
    let want = 73.42 * 415.0 / 440.0;
    let (hz, _) = read(want, 48_000.0, 0.8).expect("no reading");
    assert!(cents(hz, want).abs() < 3.0, "{want} Hz read as {hz} Hz");
}

#[test]
fn the_top_string_resolves_finely_enough_to_tune_by() {
    // The hardest case for the decimated search: one sample of period at 12 kHz is ~50 cents
    // up here, so this is really a test that the full-rate refinement runs.
    for &(want, name) in &[(329.63, "E4 in tune"), (331.0, "E4 sharp"), (328.0, "E4 flat")] {
        let (hz, _) = read(want, 48_000.0, 0.6).unwrap_or_else(|| panic!("no reading for {name}"));
        assert!(cents(hz, want).abs() < 2.0, "{name}: {want} Hz read as {hz} Hz");
    }
}

#[test]
fn the_reading_holds_up_at_44_1_khz() {
    let (hz, _) = read(146.83, 44_100.0, 0.6).expect("no reading");
    assert!(cents(hz, 146.83).abs() < 2.0, "D3 read as {hz} Hz");
}

#[test]
fn silence_and_noise_read_as_nothing_rather_than_a_guess() {
    let mut tracker = PitchTracker::new(48_000.0);
    let silence = [0.0f32; 512];
    for _ in 0..40 {
        tracker.push(&silence);
    }
    assert!(tracker.last().is_none(), "silence produced a reading");

    // Deterministic hash noise: loud, broadband, no period to find.
    let mut tracker = PitchTracker::new(48_000.0);
    let mut state = 0x12345678u32;
    let mut block = [0.0f32; 512];
    for _ in 0..40 {
        for s in block.iter_mut() {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s = (state >> 8) as f32 / 8_388_608.0 - 1.0;
        }
        tracker.push(&block);
    }
    let clarity = tracker.last().map(|p| p.clarity).unwrap_or(0.0);
    assert!(clarity < 0.75, "noise read as a pitch with clarity {clarity}");
}

#[test]
fn closing_the_tuner_clears_what_it_heard() {
    let mut tracker = PitchTracker::new(48_000.0);
    let mut phase = 0.0f32;
    let mut block = [0.0f32; 256];
    let step = 2.0 * std::f32::consts::PI * 110.0 / 48_000.0;
    for _ in 0..200 {
        for s in block.iter_mut() {
            *s = pluck(phase);
            phase = (phase + step) % (2.0 * std::f32::consts::PI);
        }
        tracker.push(&block);
    }
    assert!(tracker.last().is_some());
    tracker.reset();
    assert!(tracker.last().is_none());
    // And a reset really does start the window over: one short block is not enough to answer.
    assert!(!tracker.push(&block));
}
