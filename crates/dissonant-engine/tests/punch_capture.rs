//! Behavioural audit of punch recording: when the engine starts and stops writing a take, and
//! what an armed punch does to looping.
//!
//! Offline: the engine is driven block by block with a matched input stream, exactly as a
//! capture device running at the same rate would, and the take ring is read back afterwards.

use std::sync::Arc;

use dissonant_core::{MasterSettings, ProjectModel, Sequence, Track, TICKS_PER_BEAT};
use dissonant_engine::engine::{Engine, EngineCommand, Shared};

const SR: f32 = 48_000.0;
const B: i64 = TICKS_PER_BEAT;
/// 120 bpm: a beat is half a second, so a beat is 24 000 frames at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
const BLOCK: usize = 256;

/// A 16-beat song with one audio track and nothing making noise.
fn song() -> ProjectModel {
    let mut m = ProjectModel::empty();
    m.tempo = 120.0;
    m.tracks[0].muted = true;
    m.tracks.push(Track::audio("guitar"));
    m.song_length_ticks = 16 * B;
    m
}

struct Rig {
    engine: Engine,
    shared: Arc<Shared>,
    input: rtrb::Producer<f32>,
    take: rtrb::Consumer<f32>,
}

fn rig() -> Rig {
    let shared = Arc::new(Shared::default());
    let mut engine = Engine::new(SR, Arc::clone(&shared), &MasterSettings::default());
    let (input, input_rx) = rtrb::RingBuffer::<f32>::new(SR as usize);
    let (take_tx, take) = rtrb::RingBuffer::<f32>::new(SR as usize);
    engine.attach_io(input_rx, take_tx);
    engine.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(
        &song(),
    ))));
    engine.handle(EngineCommand::SetLooping(false));
    Rig {
        engine,
        shared,
        input,
        take,
    }
}

/// Run `blocks` blocks, feeding a full block of input to each one and emptying the take ring
/// after each — the writer thread's job in the real app, and a full ring would otherwise stop
/// capture early. Returns how many frames had been captured by the end of each block, and the
/// frames that actually came out of the ring.
fn run(r: &mut Rig, blocks: usize) -> (Vec<u64>, usize) {
    let mut out = vec![0.0f32; BLOCK * 2];
    let mut seen = Vec::with_capacity(blocks);
    let mut drained = 0usize;
    for _ in 0..blocks {
        for _ in 0..BLOCK {
            let _ = r.input.push(0.5);
        }
        r.engine.process(&mut out);
        while r.take.pop().is_ok() {
            drained += 1;
        }
        seen.push(r.shared.take_span().1);
    }
    (seen, drained)
}

#[test]
fn a_punch_drops_in_at_its_start_and_out_at_its_end() {
    let mut r = rig();
    // Punch beats 4 → 8: one second of take, two seconds into the song.
    r.engine.handle(EngineCommand::SetPunch {
        start: (4 * B) as f64,
        end: (8 * B) as f64,
    });
    r.engine.handle(EngineCommand::StartRecord);
    r.engine.handle(EngineCommand::Play);
    // Twelve beats of transport: four before the punch, four through it, four after.
    let blocks = 12 * FRAMES_PER_BEAT / BLOCK;
    let (captured, drained) = run(&mut r, blocks);

    let before = 4 * FRAMES_PER_BEAT / BLOCK;
    assert_eq!(
        captured[before - 2],
        0,
        "the take started before the punch did"
    );
    let total = *captured.last().unwrap();
    let wanted = (4 * FRAMES_PER_BEAT) as u64;
    assert!(
        total.abs_diff(wanted) <= BLOCK as u64,
        "captured {total} frames, wanted the punch's {wanted}"
    );
    // The clip has to land exactly on the punch, so the published start tick is punch-in
    // itself rather than wherever the block that crossed it began.
    assert_eq!(r.shared.take_span().0, (4 * B) as f64);
    // And the frames really went down the ring, not just onto a counter.
    assert_eq!(drained as u64, total);
}

#[test]
fn recording_without_a_punch_still_starts_with_the_transport() {
    let mut r = rig();
    r.engine.handle(EngineCommand::StartRecord);
    r.engine.handle(EngineCommand::Play);
    let (captured, _) = run(&mut r, 2 * FRAMES_PER_BEAT / BLOCK);
    assert_eq!(r.shared.take_span().0, 0.0);
    let total = *captured.last().unwrap();
    assert!(
        total.abs_diff((2 * FRAMES_PER_BEAT) as u64) <= BLOCK as u64,
        "{total}"
    );
}

#[test]
fn an_armed_punch_suppresses_the_loop() {
    // Comping by the back door is exactly what this stops: a loop would carry the playhead
    // back over the punch range and record it again.
    let mut r = rig();
    r.engine.handle(EngineCommand::SetLooping(true));
    r.engine.handle(EngineCommand::SetLoop {
        start: 0.0,
        end: (2 * B) as f64,
    });
    r.engine.handle(EngineCommand::SetPunch {
        start: (4 * B) as f64,
        end: (8 * B) as f64,
    });
    r.engine.handle(EngineCommand::Play);
    run(&mut r, 3 * FRAMES_PER_BEAT / BLOCK);
    assert!(
        r.engine.position() > (2 * B) as f64,
        "the loop wrapped under an armed punch: {}",
        r.engine.position()
    );

    // Cleared, the brace works again.
    r.engine.handle(EngineCommand::ClearPunch);
    r.engine.handle(EngineCommand::Seek { tick: 0.0 });
    run(&mut r, 3 * FRAMES_PER_BEAT / BLOCK);
    assert!(
        r.engine.position() < (2 * B) as f64,
        "the brace stopped working once the punch was cleared: {}",
        r.engine.position()
    );
}
