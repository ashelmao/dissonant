//! The UI↔core wire contract. `ui/src/types.ts` declares `Command` as a camelCase tagged
//! union; this file pins every variant's JSON exactly as the UI sends it over Tauri IPC, so a
//! field-name drift fails here instead of silently rejecting edits at runtime.

use dissonant_core::document::{Command, TrackParam};
use dissonant_core::DrumPad;
use serde_json::json;
use uuid::Uuid;

fn parse(v: serde_json::Value) -> Command {
    serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("UI payload rejected: {v} — {e}"))
}

/// A complete synth `Sound` as `ui/src/sound.ts` builds it, every field off its default (and
/// exactly representable as f32), so a renamed or missing field shows up in the round trip.
fn synth_sound_json() -> serde_json::Value {
    let osc = |on: bool, wave: &str, octave: i32| {
        json!({"on": on, "wave": wave, "octave": octave, "semi": 7, "fine": -12.5, "level": 0.625, "pulseWidth": 0.25,
               "unison": 3, "detune": 22.5, "randomPhase": true})
    };
    let lfo = |wave: &str, target: &str| json!({"wave": wave, "rate": 2.5, "depth": 0.375, "target": target, "retrigger": true});
    json!({
        "id": "u-11111111-2222-4333-8444-555555555555", "name": "glass lead", "category": "lead",
        "patch": {
            "kind": "synth",
            "osc": [osc(true, "pulse", -1), osc(true, "square", 1), osc(false, "triangle", 2)],
            "noiseLevel": 0.125, "noiseColor": 0.75, "fmAmount": 0.25, "ringMod": 0.5, "sync": true,
            "filter": {"mode": "lowpass24", "cutoff": 2400.0, "resonance": 0.625, "drive": 0.25, "keyTrack": 0.5,
                       "envAmount": -2.5, "velocity": 0.75},
            "ampEnv": {"attack": 0.015625, "decay": 0.25, "sustain": 0.5, "release": 0.75},
            "filterEnv": {"attack": 0.0078125, "decay": 0.5, "sustain": 0.25, "release": 1.5},
            "pitchEnvAmount": 12.0, "pitchEnvDecay": 0.125,
            "lfo": [lfo("sampleHold", "cutoff"), lfo("triangle", "pulseWidth")],
            "mono": true, "glide": 0.0625, "velocity": 0.5, "drive": 0.375, "crush": 0.125,
            "chorus": {"mix": 0.25, "rate": 1.5, "depth": 0.75},
            "delay": {"mix": 0.125, "time": 0.375, "feedback": 0.5},
            "level": 0.3125,
        },
    })
}

/// A complete drum `Sound`, like [`synth_sound_json`].
fn drum_sound_json() -> serde_json::Value {
    json!({
        "id": "u-66666666-7777-4888-8999-aaaaaaaaaaaa", "name": "boom", "category": "kick",
        "patch": {
            "kind": "drum",
            "length": 0.75, "tune": -3.5,
            "body": {"level": 0.875, "wave": "triangle", "startHz": 320.0, "endHz": 45.0, "pitchDecay": 0.0625, "decay": 0.25,
                     "overtoneLevel": 0.125, "overtoneRatio": 2.5},
            "noise": {"level": 0.25, "mode": "bandpass", "cutoff": 3200.0, "resonance": 0.375, "attack": 0.00390625,
                      "decay": 0.125, "bursts": 3, "burstSpacing": 0.0078125},
            "metal": {"level": 0.0625, "baseHz": 410.0, "decay": 0.03125, "cutoff": 7500.0},
            "click": {"level": 0.5, "decay": 0.001953125, "toneHz": 4500.0},
            "drive": 0.25, "crush": 0.0625, "lowCut": 40.0, "highCut": 12000.0, "velocity": 0.875, "level": 0.625,
        },
    })
}

#[test]
fn every_command_variant_round_trips_from_the_ui_payload() {
    let id = Uuid::new_v4();
    let pattern_id = Uuid::new_v4();
    let track_id = Uuid::new_v4();

    assert_eq!(parse(json!({"type": "setTempo", "bpm": 128.0})), Command::SetTempo { bpm: 128.0 });

    assert!(matches!(
        parse(json!({"type": "setKey", "key": {"rootPitchClass": 2, "scale": "minor", "isLocked": true}})),
        Command::SetKey { .. }
    ));

    assert_eq!(parse(json!({"type": "addTrack", "isDrum": true})), Command::AddTrack { is_drum: true });
    assert_eq!(parse(json!({"type": "deleteTrack", "id": id})), Command::DeleteTrack { id });
    assert_eq!(
        parse(json!({"type": "renameTrack", "id": id, "name": "bass"})),
        Command::RenameTrack { id, name: "bass".into() }
    );
    assert_eq!(
        parse(json!({"type": "setTrackMuted", "id": id, "muted": true})),
        Command::SetTrackMuted { id, muted: true }
    );
    assert_eq!(
        parse(json!({"type": "setTrackSoloed", "id": id, "soloed": true})),
        Command::SetTrackSoloed { id, soloed: true }
    );
    assert_eq!(parse(json!({"type": "moveTrack", "id": id, "up": false})), Command::MoveTrack { id, up: false });
    assert_eq!(
        parse(json!({"type": "setTrackVoice", "id": id, "voice": "pluck"})),
        Command::SetTrackVoice { id, voice: "pluck".into() }
    );
    // The SOUND page and the Devices panel. A sound must come back out exactly as it went in:
    // every field the UI sets reaches the patch, none is silently left at its default.
    match parse(json!({"type": "setTrackSound", "id": id, "sound": synth_sound_json()})) {
        Command::SetTrackSound { id: t, sound } => {
            assert_eq!(t, id);
            assert_eq!(serde_json::to_value(&sound).unwrap(), synth_sound_json());
        }
        other => panic!("expected SetTrackSound, got {other:?}"),
    }
    match parse(json!({
        "type": "setDrumPads",
        "id": id,
        "pads": [{"pitch": 36, "sound": drum_sound_json()["id"], "choke": 0}, {"pitch": 42, "sound": "hat", "choke": 1}],
        "sounds": [drum_sound_json()],
    })) {
        Command::SetDrumPads { id: t, pads, sounds } => {
            assert_eq!(t, id);
            assert_eq!(
                pads,
                vec![
                    DrumPad { pitch: 36, sound: "u-66666666-7777-4888-8999-aaaaaaaaaaaa".into(), choke: 0 },
                    DrumPad { pitch: 42, sound: "hat".into(), choke: 1 },
                ]
            );
            assert_eq!(serde_json::to_value(&sounds).unwrap(), json!([drum_sound_json()]));
        }
        other => panic!("expected SetDrumPads, got {other:?}"),
    }
    match parse(json!({"type": "upsertSound", "sound": drum_sound_json()})) {
        Command::UpsertSound { sound } => assert_eq!(serde_json::to_value(&sound).unwrap(), drum_sound_json()),
        other => panic!("expected UpsertSound, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "setTrackParam", "id": id, "param": "reverbSend", "value": 0.3})),
        Command::SetTrackParam { id, param: TrackParam::ReverbSend, value: 0.3 }
    );
    assert_eq!(
        parse(json!({"type": "setTrackColor", "id": id, "color": "#ff8800"})),
        Command::SetTrackColor { id, color: Some("#ff8800".into()) }
    );
    assert_eq!(parse(json!({"type": "setTrackColor", "id": id, "color": null})), Command::SetTrackColor { id, color: None });

    assert_eq!(parse(json!({"type": "addPattern", "trackId": track_id})), Command::AddPattern { track_id });
    assert_eq!(parse(json!({"type": "duplicatePattern", "id": id})), Command::DuplicatePattern { id });
    assert_eq!(parse(json!({"type": "deletePattern", "id": id})), Command::DeletePattern { id });
    assert_eq!(
        parse(json!({"type": "renamePattern", "id": id, "name": "chorus"})),
        Command::RenamePattern { id, name: "chorus".into() }
    );
    assert_eq!(
        parse(json!({"type": "setPatternLength", "id": id, "beats": 32.0})),
        Command::SetPatternLength { id, beats: 32.0 }
    );

    // The piano roll's commit path — one `setNotes` per gesture.
    let notes = parse(json!({
        "type": "setNotes",
        "patternId": pattern_id,
        "notes": [{"id": Uuid::new_v4(), "startBeat": 1.5, "lengthBeats": 0.25, "pitch": 60, "velocity": 100}],
    }));
    match notes {
        Command::SetNotes { pattern_id: p, notes } => {
            assert_eq!(p, pattern_id);
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].start_beat, 1.5);
            assert_eq!(notes[0].length_beats, 0.25);
            assert_eq!(notes[0].pitch, 60);
            assert_eq!(notes[0].velocity, 100);
        }
        other => panic!("expected SetNotes, got {other:?}"),
    }

    // The project progression, in song beats.
    let chords = parse(json!({
        "type": "setChords",
        "chords": [{"id": Uuid::new_v4(), "startBeat": 0.0, "lengthBeats": 4.0, "pitchClasses": [0, 4, 7], "name": "C"}],
    }));
    match chords {
        Command::SetChords { chords } => assert_eq!(chords[0].pitch_classes, vec![0, 4, 7]),
        other => panic!("expected SetChords, got {other:?}"),
    }

    // Song timeline: clips, sections and the tempo map.
    let clip_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "addClip", "patternId": pattern_id, "startBeat": 16.0, "lengthBeats": null})),
        Command::AddClip { pattern_id, start_beat: 16.0, length_beats: None }
    );
    assert_eq!(
        parse(json!({"type": "addClip", "patternId": pattern_id, "startBeat": 0.0, "lengthBeats": 8.0})),
        Command::AddClip { pattern_id, start_beat: 0.0, length_beats: Some(8.0) }
    );
    match parse(json!({
        "type": "updateClip",
        "clip": {"id": clip_id, "patternId": pattern_id, "startBeat": 4.0, "lengthBeats": 8.0, "offsetBeats": 2.0, "muted": true},
    })) {
        Command::UpdateClip { clip } => {
            assert_eq!((clip.id, clip.pattern_id), (clip_id, pattern_id));
            assert_eq!((clip.start_beat, clip.length_beats, clip.offset_beats), (4.0, 8.0, 2.0));
            assert!(clip.muted);
        }
        other => panic!("expected UpdateClip, got {other:?}"),
    }
    assert_eq!(parse(json!({"type": "removeClip", "id": clip_id})), Command::RemoveClip { id: clip_id });
    match parse(json!({
        "type": "updateClips",
        "clips": [{"id": clip_id, "patternId": pattern_id, "startBeat": 8.0, "lengthBeats": 4.0, "offsetBeats": 0.0, "muted": false}],
    })) {
        Command::UpdateClips { clips } => assert_eq!((clips[0].id, clips[0].start_beat), (clip_id, 8.0)),
        other => panic!("expected UpdateClips, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "removeClips", "ids": [clip_id, id]})),
        Command::RemoveClips { ids: vec![clip_id, id] }
    );
    assert_eq!(parse(json!({"type": "makeClipUnique", "id": clip_id})), Command::MakeClipUnique { id: clip_id });
    assert_eq!(parse(json!({"type": "setSongLength", "beats": 96.0})), Command::SetSongLength { beats: 96.0 });
    match parse(json!({
        "type": "importTracks",
        "tracks": [{"name": "piano", "isDrum": false, "notes": []}],
        "lengthBeats": 16.0,
    })) {
        Command::ImportTracks { tracks, length_beats } => assert_eq!((tracks[0].name.as_str(), length_beats), ("piano", 16.0)),
        other => panic!("expected ImportTracks, got {other:?}"),
    }

    let section_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "addSection", "name": "chorus", "startBeat": 32.0})),
        Command::AddSection { name: "chorus".into(), start_beat: 32.0 }
    );
    match parse(json!({
        "type": "updateSection",
        "section": {"id": section_id, "name": "verse", "startBeat": 16.0, "key": {"rootPitchClass": 7, "scale": "major", "isLocked": true}, "color": "#ff8800"},
    })) {
        Command::UpdateSection { section } => {
            assert_eq!(section.id, section_id);
            assert_eq!(section.start_beat, 16.0);
            assert_eq!(section.key.map(|k| k.root_pitch_class), Some(Some(7)));
        }
        other => panic!("expected UpdateSection, got {other:?}"),
    }
    assert_eq!(parse(json!({"type": "removeSection", "id": section_id})), Command::RemoveSection { id: section_id });

    assert!(matches!(
        parse(json!({
            "type": "setMaster",
            "master": {"gain": 1.0, "reverbWet": 0.0, "lowCutHz": 20.0, "highCutHz": 18000.0, "lowEq": 1.0, "midEq": 1.0, "highEq": 1.0},
        })),
        Command::SetMaster { .. }
    ));

    assert_eq!(
        parse(json!({"type": "setTimeSignature", "numerator": 3, "denominator": 4})),
        Command::SetTimeSignature { numerator: 3, denominator: 4 }
    );
    assert_eq!(
        parse(json!({"type": "setSwing", "swing": 62.0, "grid": 0.25})),
        Command::SetSwing { swing: 62.0, grid: 0.25 }
    );

    let point_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "addTempoPoint", "beat": 8.0, "bpm": 140.0, "ramp": true})),
        Command::AddTempoPoint { beat: 8.0, bpm: 140.0, ramp: true }
    );
    match parse(json!({"type": "updateTempoPoint", "point": {"id": point_id, "beat": 12.0, "bpm": 90.0, "ramp": false}})) {
        Command::UpdateTempoPoint { point } => {
            assert_eq!((point.id, point.beat, point.bpm, point.ramp), (point_id, 12.0, 90.0, false));
        }
        other => panic!("expected UpdateTempoPoint, got {other:?}"),
    }
    assert_eq!(parse(json!({"type": "removeTempoPoint", "id": point_id})), Command::RemoveTempoPoint { id: point_id });
}
