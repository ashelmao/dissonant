//! Pulling a recorded take back by the capture path's latency.
//!
//! The arithmetic is small and the consequences are not: get it wrong and every take sits a few
//! milliseconds behind the grid, which is exactly the complaint this exists to answer. The
//! cases that matter are a constant tempo, a tempo that moves under the correction, and a take
//! that starts close enough to zero that the correction runs off the front of the song.

use dissonant_core::{TempoMap, TempoPoint, TICKS_PER_BEAT};
use dissonant_engine::{align_take, input_latency_ms};

const BEAT: f64 = TICKS_PER_BEAT as f64;

#[test]
fn latency_is_the_block_the_backlog_and_the_devices_trim() {
    // 512 frames out, 256 still in the ring, at 48 kHz: 16 ms, plus a 4 ms measured trim.
    let ms = input_latency_ms(512, 256, 48_000, 4.0);
    assert!((ms - 20.0).abs() < 1e-9, "{ms} ms");
    // No device, no latency — rather than dividing by a zero sample rate.
    assert_eq!(input_latency_ms(512, 256, 0, 4.0), 0.0);
    // A trim that would pull the take earlier than the real latency is still allowed; one that
    // would make the whole correction negative is not.
    assert_eq!(input_latency_ms(0, 0, 48_000, -5.0), 0.0);
    assert_eq!(input_latency_ms(480, 0, 48_000, f64::NAN), 10.0);
}

#[test]
fn a_take_moves_back_by_the_latency_at_one_tempo() {
    // 120 bpm: a beat is 500 ms, so 20 ms is 4 % of a beat.
    let map = TempoMap::constant(120.0);
    let a = align_take(&map, 8.0 * BEAT, 20.0);
    assert_eq!(a.start_tick, (8.0 * BEAT - 0.04 * BEAT).round() as i64);
    assert_eq!(a.offset_seconds, 0.0);
    assert_eq!(a.trimmed_ms, 0.0);
    // Within a tick: the clip lands on a whole tick, and `shifted_ms` reports where it really
    // went rather than the ideal it was aiming at. A tick at 120 bpm is 0.52 ms.
    assert!((a.shifted_ms - 20.0).abs() < 0.6, "{} ms", a.shifted_ms);
}

#[test]
fn the_correction_follows_the_tempo_map_rather_than_the_base_tempo() {
    // Base 120, stepping to 60 at bar 2. A take that starts inside the slow half is worth half
    // as many ticks per millisecond as the base tempo would claim.
    let map = TempoMap::new(120.0, &[TempoPoint::new(4 * TICKS_PER_BEAT, 60.0, false)]);
    let start = 8.0 * BEAT;
    let a = align_take(&map, start, 100.0);
    let moved = start - a.start_tick as f64;
    // At 60 bpm a beat is 1 s, so 100 ms is a tenth of a beat.
    assert!((moved - 0.1 * BEAT).abs() < 1.0, "moved {moved} ticks");
    // The base tempo would have moved it twice as far, which is the bug this guards.
    assert!(moved < 0.15 * BEAT, "moved {moved} ticks — read the base tempo?");
    assert!((a.shifted_ms - 100.0).abs() < 1.0, "{} ms", a.shifted_ms);
}

#[test]
fn a_correction_past_the_start_trims_the_head_instead_of_moving_the_clip() {
    // 10 ms into the song with 25 ms of latency: the sound started before the song did.
    let map = TempoMap::constant(120.0);
    let start = map.tick_at_seconds(0.010);
    let a = align_take(&map, start, 25.0);
    assert_eq!(a.start_tick, 0, "the clip has to stay on the grid");
    assert!((a.offset_seconds - 0.015).abs() < 1e-4, "{} s", a.offset_seconds);
    assert!((a.trimmed_ms - 15.0).abs() < 0.1, "{} ms", a.trimmed_ms);
    // What moved and what was cut add up to the whole latency, so the readout stays honest.
    assert!((a.shifted_ms + a.trimmed_ms - 25.0).abs() < 0.1);
}

#[test]
fn a_take_at_tick_zero_is_all_trim() {
    let map = TempoMap::constant(90.0);
    let a = align_take(&map, 0.0, 12.0);
    assert_eq!(a.start_tick, 0);
    assert_eq!(a.shifted_ms, 0.0);
    assert!((a.trimmed_ms - 12.0).abs() < 1e-6, "{} ms", a.trimmed_ms);
    assert!((a.offset_seconds - 0.012).abs() < 1e-9);
}

#[test]
fn no_latency_leaves_the_take_exactly_where_it_was_played() {
    let map = TempoMap::new(140.0, &[TempoPoint::new(2 * TICKS_PER_BEAT, 90.0, true)]);
    let a = align_take(&map, 5.0 * BEAT, 0.0);
    assert_eq!(a.start_tick, (5.0 * BEAT) as i64);
    assert_eq!(a.offset_seconds, 0.0);
    assert!(a.shifted_ms.abs() < 0.001, "{} ms", a.shifted_ms);
}
