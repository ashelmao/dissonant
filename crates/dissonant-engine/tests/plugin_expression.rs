//! The per-note pitch envelope as a hosted CLAP instrument receives it.
//!
//! A plugin cannot be handed the engine's per-sample contour; it is sent
//! `CLAP_NOTE_EXPRESSION_TUNING` events addressed to the note's id. These tests run the whole
//! path — sequence, scheduler, channel, host — into a real plugin that writes down every note
//! event it receives, and check the log: each note sounds under a real id, its detune and its
//! contour arrive as tuning values on the samples where the envelope has them, the stream is
//! continuous across engine blocks and the host's own `process` splits, and a seek into a bend
//! starts the plugin note where the bend stands.

use dissonant_core::automation::Curve;
use dissonant_core::{NoteEvent, PitchPoint, ProjectModel, Sequence, TICKS_PER_BEAT};
use dissonant_engine::engine::{Engine, EngineCommand, Shared};
use dissonant_engine::Instrument;
use dissonant_plugins::{HostedPlugin, PluginDescriptor, PluginFormat, PluginKind};
use std::path::PathBuf;
use std::sync::Arc;

mod fixture {
    #![allow(dead_code)]

    include!("fixture/note_recorder_plugin.rs");
}

use fixture::{Kind, Received};

const SR: f32 = 44_100.0;
/// The engine's block, and a smaller ceiling for the plugin, so values cross both kinds of
/// boundary: one between two engine callbacks, one between two `process` calls inside a block.
const BLOCK: usize = 512;
const PLUGIN_MAX_FRAMES: u32 = 128;

/// At the default 120 bpm a tick is 735/32 samples, so ticks in multiples of 32 land on whole
/// samples and every expected position below is exact.
fn samples(ticks: i64) -> u64 {
    assert_eq!(ticks % 32, 0, "pick ticks that land on whole samples");
    (ticks / 32 * 735) as u64
}

const ANCHOR_KEY: i16 = 48;
const BENT_KEY: i16 = 60;
const BENT_START: i64 = 320;
const BENT_LENGTH: i64 = 2 * TICKS_PER_BEAT;
const BENT_FINE: i32 = 10;
/// The contour: a ramp up 200 cents over the first 480 ticks, then a step down to -100.
const RAMP_END: i64 = 480;
const STEP_AT: i64 = 736;

/// Where `cargo test` left a compiled fixture example.
fn example_dylib(stem: &str) -> PathBuf {
    let mut dir = std::env::current_exe().expect("test binary has a path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let file = if cfg!(target_os = "windows") {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    };
    let path = dir.join("examples").join(&file);
    assert!(
        path.is_file(),
        "fixture plugin {} was not built; `cargo test` is expected to build example targets",
        path.display()
    );
    path
}

fn recorder() -> PluginDescriptor {
    let path = example_dylib("dissonant_note_recorder_fixture");
    PluginDescriptor {
        uid: format!("clap:{}:{}", path.display(), fixture::PLUGIN_ID),
        format: PluginFormat::Clap,
        path,
        plugin_id: fixture::PLUGIN_ID.to_string(),
        name: fixture::PLUGIN_NAME.to_string(),
        vendor: fixture::PLUGIN_VENDOR.to_string(),
        version: fixture::PLUGIN_VERSION.to_string(),
        kind: PluginKind::Instrument,
        features: vec!["instrument".into()],
    }
}

/// Two notes: an unbent anchor from tick 0 with a detune of its own — it also wakes the plugin
/// on the very first sample, so the recorder's frame count is the engine's — and a bent note
/// starting part-way into an engine block.
fn model() -> ProjectModel {
    let mut m = ProjectModel::empty();
    m.patterns[0].length_ticks = 8 * TICKS_PER_BEAT;
    let mut anchor = NoteEvent::new(0, 4 * TICKS_PER_BEAT, ANCHOR_KEY as i32);
    anchor.fine = -20;
    let mut bent = NoteEvent::new(BENT_START, BENT_LENGTH, BENT_KEY as i32);
    bent.fine = BENT_FINE;
    bent.pitch_envelope = vec![
        PitchPoint::new(RAMP_END, 200.0, Curve::Ramp),
        PitchPoint::new(STEP_AT, -100.0, Curve::Step),
    ];
    m.patterns[0].notes = vec![anchor, bent];
    m
}

/// The cents the bent note should sound at, `rel` samples after it starts.
fn expected_cents(rel: u64) -> f64 {
    let (ramp_end, step_at) = (samples(RAMP_END), samples(STEP_AT));
    let bend = if rel < ramp_end {
        200.0 * rel as f64 / ramp_end as f64
    } else if rel < step_at {
        200.0
    } else {
        -100.0
    };
    f64::from(BENT_FINE) + bend
}

/// An engine playing `model()` through the recorder, with the app-thread half handed back.
fn engine_with_recorder() -> (Engine, HostedPlugin) {
    let m = model();
    let mut e = Engine::new(SR, Arc::new(Shared::default()), &m.master);
    let seq = Sequence::from_pattern(&m, &m.patterns[0].id, 0).expect("pattern");
    e.handle(EngineCommand::SetSequence(Arc::new(seq)));
    let (hosted, unit) =
        HostedPlugin::open(&recorder(), SR as f64, PLUGIN_MAX_FRAMES).expect("recorder opens");
    e.handle(EngineCommand::SetInstrument {
        track_id: m.tracks[0].id,
        instrument: Box::new(Instrument::Plugin(Box::new(unit))),
    });
    (e, hosted)
}

fn run_blocks(e: &mut Engine, frames: usize) {
    let mut out = vec![0.0f32; BLOCK * 2];
    for _ in 0..frames.div_ceil(BLOCK) {
        e.process(&mut out);
    }
}

fn log(hosted: &mut HostedPlugin) -> Vec<Received> {
    fixture::decode(&hosted.save_state().expect("recorder saves its log"))
}

fn note_on(log: &[Received], key: i16) -> (usize, Received) {
    log.iter()
        .copied()
        .enumerate()
        .find(|(_, r)| r.kind == Kind::NoteOn && r.key == key)
        .unwrap_or_else(|| panic!("no note-on for key {key} in {log:#?}"))
}

#[test]
fn a_bent_note_reaches_the_plugin_as_tuning_on_its_own_id_sample_by_sample() {
    let (mut e, mut hosted) = engine_with_recorder();
    e.handle(EngineCommand::Play);
    // Past the bent note's end, well short of the anchor's.
    run_blocks(&mut e, samples(BENT_START + BENT_LENGTH) as usize + 2 * BLOCK);
    let log = log(&mut hosted);

    assert!(
        log.iter().all(|r| r.kind != Kind::OtherExpression),
        "only tuning is sent: {log:#?}"
    );

    // The anchor: a real id, and its detune as a single value on its note-on's sample.
    let (anchor_at, anchor) = note_on(&log, ANCHOR_KEY);
    assert_eq!(anchor.at, 0);
    assert!(anchor.note_id >= 0, "a note-on must carry a real id, got {}", anchor.note_id);
    let anchor_tunings: Vec<_> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == Kind::Tuning && r.note_id == anchor.note_id)
        .collect();
    assert_eq!(anchor_tunings.len(), 1, "an unbent note is told its detune once: {anchor_tunings:#?}");
    let (i, t) = anchor_tunings[0];
    assert!(i > anchor_at, "the tuning follows the note-on it addresses");
    assert_eq!((t.at, t.key), (0, ANCHOR_KEY));
    assert!((t.value - -0.20).abs() < 1e-9, "fine of -20 cents is -0.2 semitones, got {}", t.value);

    // The bent note: its own id, on the sample the scheduler put it on — mid-block.
    let (on_index, on) = note_on(&log, BENT_KEY);
    let start = samples(BENT_START);
    assert_eq!(on.at, start);
    assert_ne!(start % BLOCK as u64, 0, "the note is meant to start inside a block");
    assert!(on.note_id >= 0 && on.note_id != anchor.note_id);

    let tunings: Vec<_> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| r.kind == Kind::Tuning && r.note_id == on.note_id)
        .map(|(i, r)| (i, *r))
        .collect();
    assert!(tunings.iter().all(|(_, r)| r.key == BENT_KEY), "addressed by key as well as id");
    assert!(tunings[0].0 > on_index, "the first value follows the note-on it addresses");

    // Every value is the envelope (plus fine) at the very sample it lands on.
    for (_, r) in &tunings {
        let rel = r.at - start;
        let want = expected_cents(rel);
        assert!(
            (r.value * 100.0 - want).abs() < 0.25,
            "at +{rel} samples the plugin was told {:.3} cents, the envelope says {want:.3}",
            r.value * 100.0
        );
    }

    // The shape of the stream: one value per 64 samples while the ramp moves, starting on the
    // note-on's own sample; then the ramp's end exactly where it lands; then the step exactly
    // where it was drawn; then nothing, because the note holds.
    let rel: Vec<u64> = tunings.iter().map(|(_, r)| r.at - start).collect();
    let (ramp_end, step_at) = (samples(RAMP_END), samples(STEP_AT));
    let ramp: Vec<u64> = (0..ramp_end).step_by(64).collect();
    let mut want = ramp.clone();
    want.push(ramp_end);
    want.push(step_at);
    assert_eq!(rel, want, "tuning offsets relative to the note-on");
    assert!((tunings[0].1.value - 0.10).abs() < 1e-6, "the note starts at its fine");
    assert!((tunings[ramp.len()].1.value - 2.10).abs() < 1e-6, "the ramp ends exactly on its point");
    assert!((tunings[ramp.len() + 1].1.value - -0.90).abs() < 1e-6, "the step lands exactly");

    // The stream does not stop at the first engine block boundary, nor at any `process` split.
    let first_boundary = start.next_multiple_of(BLOCK as u64);
    assert!(rel.iter().any(|&r| start + r < first_boundary));
    assert!(rel.iter().any(|&r| start + r >= first_boundary));
    assert!(
        rel.iter().any(|&r| !(start + r).is_multiple_of(u64::from(PLUGIN_MAX_FRAMES))
            && (start + r) / BLOCK as u64 != start / BLOCK as u64),
        "values land inside later blocks' `process` splits, not only on their edges"
    );

    // And the note-off names the same note.
    let off = log
        .iter()
        .find(|r| r.kind == Kind::NoteOff && r.key == BENT_KEY)
        .expect("the bent note is released");
    assert_eq!(off.at, samples(BENT_START + BENT_LENGTH));
    assert_eq!(off.note_id, on.note_id, "released under the id it was struck with");
}

#[test]
fn seeking_into_a_bend_starts_the_plugin_note_where_the_bend_stands() {
    let (mut e, mut hosted) = engine_with_recorder();
    let into = 256;
    e.handle(EngineCommand::Seek {
        tick: (BENT_START + into) as f64,
    });
    e.handle(EngineCommand::Play);
    run_blocks(&mut e, 2 * BLOCK);
    let log = log(&mut hosted);

    let (on_index, on) = note_on(&log, BENT_KEY);
    let first = log
        .iter()
        .enumerate()
        .find(|(_, r)| r.kind == Kind::Tuning && r.note_id == on.note_id)
        .expect("the resumed note is told where it stands");
    assert!(first.0 > on_index);
    assert_eq!(first.1.at, on.at, "on the note-on's own sample, before any audio");
    let want = expected_cents(samples(into));
    assert!(
        (first.1.value * 100.0 - want).abs() < 0.01,
        "resumed at {:.3} cents, the envelope stands at {want:.3}",
        first.1.value * 100.0
    );
}
