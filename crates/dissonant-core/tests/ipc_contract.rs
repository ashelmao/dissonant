//! The UI↔core wire contract. `ui/src/types.ts` declares `Command` as a camelCase tagged
//! union; this file pins every variant's JSON exactly as the UI sends it over Tauri IPC, so a
//! field-name drift fails here instead of silently rejecting edits at runtime.

use dissonant_core::document::{Command, InsertTarget, TrackParam};
use dissonant_core::{
    AmpModel, AudioClip, AutoTarget, CabModel, Cabinet, Curve, DrumPad, EqModel, Insert,
    MasterParam, MicModel, Pedal, TrackPlugin,
};
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
            "voice": "legato", "glide": 0.0625, "velocity": 0.5, "drive": 0.375, "crush": 0.125,
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

    assert_eq!(
        parse(json!({"type": "setTempo", "bpm": 128.0})),
        Command::SetTempo { bpm: 128.0 }
    );

    assert!(matches!(
        parse(
            json!({"type": "setKey", "key": {"rootPitchClass": 2, "scale": "minor", "isLocked": true}})
        ),
        Command::SetKey { .. }
    ));

    assert_eq!(
        parse(json!({"type": "addTrack", "isDrum": true})),
        Command::AddTrack { is_drum: true }
    );
    assert_eq!(
        parse(json!({"type": "deleteTrack", "id": id})),
        Command::DeleteTrack { id }
    );
    assert_eq!(
        parse(json!({"type": "renameTrack", "id": id, "name": "bass"})),
        Command::RenameTrack {
            id,
            name: "bass".into()
        }
    );
    assert_eq!(
        parse(json!({"type": "setTrackMuted", "id": id, "muted": true})),
        Command::SetTrackMuted { id, muted: true }
    );
    assert_eq!(
        parse(json!({"type": "setTrackSoloed", "id": id, "soloed": true})),
        Command::SetTrackSoloed { id, soloed: true }
    );
    assert_eq!(
        parse(json!({"type": "moveTrack", "id": id, "up": false})),
        Command::MoveTrack { id, up: false }
    );
    assert_eq!(
        parse(json!({"type": "setTrackVoice", "id": id, "voice": "pluck"})),
        Command::SetTrackVoice {
            id,
            voice: "pluck".into()
        }
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
        Command::SetDrumPads {
            id: t,
            pads,
            sounds,
        } => {
            assert_eq!(t, id);
            assert_eq!(
                pads,
                vec![
                    DrumPad {
                        pitch: 36,
                        sound: "u-66666666-7777-4888-8999-aaaaaaaaaaaa".into(),
                        choke: 0
                    },
                    DrumPad {
                        pitch: 42,
                        sound: "hat".into(),
                        choke: 1
                    },
                ]
            );
            assert_eq!(
                serde_json::to_value(&sounds).unwrap(),
                json!([drum_sound_json()])
            );
        }
        other => panic!("expected SetDrumPads, got {other:?}"),
    }
    match parse(json!({"type": "upsertSound", "sound": drum_sound_json()})) {
        Command::UpsertSound { sound } => {
            assert_eq!(serde_json::to_value(&sound).unwrap(), drum_sound_json())
        }
        other => panic!("expected UpsertSound, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "setTrackParam", "id": id, "param": "reverbSend", "value": 0.3})),
        Command::SetTrackParam {
            id,
            param: TrackParam::ReverbSend,
            value: 0.3
        }
    );
    assert_eq!(
        parse(json!({"type": "setTrackColor", "id": id, "color": "#ff8800"})),
        Command::SetTrackColor {
            id,
            color: Some("#ff8800".into())
        }
    );
    assert_eq!(
        parse(json!({"type": "setTrackColor", "id": id, "color": null})),
        Command::SetTrackColor { id, color: None }
    );

    // The plugin picker sends the whole record; `state` is omitted when the plugin is new.
    assert_eq!(
        parse(json!({
            "type": "setTrackPlugin", "id": id,
            "plugin": {"uid": "clap:/CLAP/Vital.clap:audio.vital.synth", "name": "Vital", "state": null}
        })),
        Command::SetTrackPlugin {
            id,
            plugin: Some(TrackPlugin {
                uid: "clap:/CLAP/Vital.clap:audio.vital.synth".into(),
                name: "Vital".into(),
                state: None,
            })
        }
    );
    assert_eq!(
        parse(json!({
            "type": "setTrackPlugin", "id": id,
            "plugin": {"uid": "clap:/CLAP/Vital.clap:audio.vital.synth", "name": "Vital"}
        })),
        Command::SetTrackPlugin {
            id,
            plugin: Some(TrackPlugin {
                uid: "clap:/CLAP/Vital.clap:audio.vital.synth".into(),
                name: "Vital".into(),
                state: None,
            })
        }
    );
    assert_eq!(
        parse(json!({"type": "setTrackPlugin", "id": id, "plugin": null})),
        Command::SetTrackPlugin { id, plugin: None }
    );
    assert_eq!(
        parse(json!({"type": "setPluginState", "id": id, "state": "QUJDRA=="})),
        Command::SetPluginState {
            id,
            state: "QUJDRA==".into()
        }
    );

    // Insert chains. The target is `{"track": "<uuid>"}` or the bare string `"master"`, and it
    // travels on every insert command — the UI panel shows one chain at a time and names it.
    let insert_id = Uuid::new_v4();
    let uid = "clap:/CLAP/FabFilter Pro-Q 3.clap:com.fabfilter.proq3";
    assert_eq!(
        parse(
            json!({"type": "addInsert", "target": {"track": track_id}, "uid": uid, "name": "Pro-Q", "index": null})
        ),
        Command::AddInsert {
            target: InsertTarget::Track(track_id),
            uid: uid.into(),
            name: "Pro-Q".into(),
            index: None
        }
    );
    assert_eq!(
        parse(
            json!({"type": "addInsert", "target": "master", "uid": uid, "name": "Pro-Q", "index": 2})
        ),
        Command::AddInsert {
            target: InsertTarget::Master,
            uid: uid.into(),
            name: "Pro-Q".into(),
            index: Some(2)
        }
    );
    assert_eq!(
        parse(
            json!({"type": "removeInsert", "target": {"track": track_id}, "insertId": insert_id})
        ),
        Command::RemoveInsert {
            target: InsertTarget::Track(track_id),
            insert_id
        }
    );
    assert_eq!(
        parse(
            json!({"type": "moveInsert", "target": "master", "insertId": insert_id, "toIndex": 1})
        ),
        Command::MoveInsert {
            target: InsertTarget::Master,
            insert_id,
            to_index: 1
        }
    );
    assert_eq!(
        parse(
            json!({"type": "setInsertEnabled", "target": {"track": track_id}, "insertId": insert_id, "on": false})
        ),
        Command::SetInsertEnabled {
            target: InsertTarget::Track(track_id),
            insert_id,
            on: false
        }
    );
    assert_eq!(
        parse(
            json!({"type": "setInsertState", "target": "master", "insertId": insert_id, "state": "QUJDRA=="})
        ),
        Command::SetInsertState {
            target: InsertTarget::Master,
            insert_id,
            state: "QUJDRA==".into()
        }
    );

    // And the same shape on the way out, since the UI reads chains straight off the project.
    assert_eq!(
        serde_json::to_value(Insert {
            id: insert_id,
            uid: uid.into(),
            name: "Pro-Q".into(),
            enabled: false,
            state: None,
            params: Default::default()
        })
        .unwrap(),
        json!({"id": insert_id, "uid": uid, "name": "Pro-Q", "enabled": false})
    );
    assert_eq!(
        serde_json::to_value(InsertTarget::Track(track_id)).unwrap(),
        json!({"track": track_id})
    );
    assert_eq!(
        serde_json::to_value(InsertTarget::Master).unwrap(),
        json!("master")
    );

    // A pattern belongs to one track, so the UI names it when adding one.
    assert_eq!(
        parse(json!({"type": "addPattern", "trackId": track_id})),
        Command::AddPattern { track_id }
    );
    assert_eq!(
        parse(json!({"type": "duplicatePattern", "id": id})),
        Command::DuplicatePattern { id }
    );
    assert_eq!(
        parse(json!({"type": "deletePattern", "id": id})),
        Command::DeletePattern { id }
    );
    assert_eq!(
        parse(json!({"type": "renamePattern", "id": id, "name": "chorus"})),
        Command::RenamePattern {
            id,
            name: "chorus".into()
        }
    );
    assert_eq!(
        parse(json!({"type": "setPatternLength", "id": id, "ticks": 30_720})),
        Command::SetPatternLength { id, ticks: 30_720 }
    );

    // The piano roll's commit path — one `setNotes` per gesture. The performance properties
    // are optional on the wire: a bare note takes the neutral value for each.
    let source_id = Uuid::new_v4();
    let notes = parse(json!({
        "type": "setNotes",
        "patternId": pattern_id,
        "notes": [
            {"id": Uuid::new_v4(), "startTick": 1_440, "lengthTicks": 240, "pitch": 60, "velocity": 100},
            {
                "id": Uuid::new_v4(), "startTick": 1_920, "lengthTicks": 480, "pitch": 64, "velocity": 90,
                "pan": 0.25, "release": 0.75, "fine": -50, "modX": 0.1, "modY": 0.9,
                "shiftTicks": -120, "repeats": 3, "color": 5,
                "slideFrom": source_id, "slideSecs": 0.25
            },
        ],
    }));
    match notes {
        Command::SetNotes {
            pattern_id: p,
            notes,
        } => {
            assert_eq!(p, pattern_id);
            assert_eq!(notes.len(), 2);
            assert_eq!(notes[0].start_tick, 1_440);
            assert_eq!(notes[0].length_ticks, 240);
            assert_eq!(notes[0].pitch, 60);
            assert_eq!(notes[0].velocity, 100);
            // Neutral defaults, so projects written before these properties existed load unchanged.
            assert_eq!(notes[0].pan, 0.5);
            assert_eq!(notes[0].release, 0.5);
            assert_eq!(notes[0].mod_x, 0.5);
            assert_eq!(notes[0].mod_y, 0.5);
            assert_eq!(
                (
                    notes[0].fine,
                    notes[0].shift_ticks,
                    notes[0].repeats,
                    notes[0].color
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(notes[1].pan, 0.25);
            assert_eq!(notes[1].release, 0.75);
            assert_eq!(notes[1].fine, -50);
            assert_eq!(notes[1].mod_x, 0.1);
            assert_eq!(notes[1].mod_y, 0.9);
            assert_eq!(notes[1].shift_ticks, -120);
            assert_eq!((notes[1].repeats, notes[1].color), (3, 5));
            assert_eq!(notes[1].shifted_start(), 1_800);
            // No rope unless the UI sends one.
            assert_eq!((notes[0].slide_from, notes[0].slide_secs), (None, None));
            assert_eq!(notes[1].slide_from, Some(source_id));
            assert_eq!(notes[1].slide_secs, Some(0.25));
        }
        other => panic!("expected SetNotes, got {other:?}"),
    }

    // Ropes, drawn in the piano roll.
    let note_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "setRope", "patternId": pattern_id, "noteId": note_id, "sourceId": source_id})),
        Command::SetRope { pattern_id, note_id, source_id }
    );
    assert_eq!(
        parse(json!({"type": "clearRope", "patternId": pattern_id, "noteId": note_id})),
        Command::ClearRope { pattern_id, note_id }
    );
    assert_eq!(
        parse(json!({"type": "setSlideTime", "patternId": pattern_id, "noteId": note_id, "secs": 0.375})),
        Command::SetSlideTime { pattern_id, note_id, secs: Some(0.375) }
    );
    assert_eq!(
        parse(json!({"type": "setSlideTime", "patternId": pattern_id, "noteId": note_id, "secs": null})),
        Command::SetSlideTime { pattern_id, note_id, secs: None }
    );

    // The project progression, in song ticks.
    let chords = parse(json!({
        "type": "setChords",
        "chords": [{"id": Uuid::new_v4(), "startTick": 0, "lengthTicks": 3_840, "pitchClasses": [0, 4, 7], "name": "C"}],
    }));
    match chords {
        Command::SetChords { chords } => assert_eq!(chords[0].pitch_classes, vec![0, 4, 7]),
        other => panic!("expected SetChords, got {other:?}"),
    }

    // Song timeline: clips, sections and the tempo map.
    let clip_id = Uuid::new_v4();
    assert_eq!(
        parse(
            json!({"type": "addClip", "patternId": pattern_id, "startTick": 15_360, "lengthTicks": null})
        ),
        Command::AddClip {
            pattern_id,
            start_tick: 15_360,
            length_ticks: None
        }
    );
    assert_eq!(
        parse(
            json!({"type": "addClip", "patternId": pattern_id, "startTick": 0, "lengthTicks": 7_680})
        ),
        Command::AddClip {
            pattern_id,
            start_tick: 0,
            length_ticks: Some(7_680)
        }
    );
    match parse(json!({
        "type": "updateClip",
        "clip": {"id": clip_id, "patternId": pattern_id, "startTick": 3_840, "lengthTicks": 7_680, "offsetTicks": 1_920, "muted": true},
    })) {
        Command::UpdateClip { clip } => {
            assert_eq!((clip.id, clip.pattern_id), (clip_id, pattern_id));
            assert_eq!(
                (clip.start_tick, clip.length_ticks, clip.offset_ticks),
                (3_840, 7_680, 1_920)
            );
            assert!(clip.muted);
        }
        other => panic!("expected UpdateClip, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "removeClip", "id": clip_id})),
        Command::RemoveClip { id: clip_id }
    );
    match parse(json!({
        "type": "updateClips",
        "clips": [{"id": clip_id, "patternId": pattern_id, "startTick": 7_680, "lengthTicks": 3_840, "offsetTicks": 0, "muted": false}],
    })) {
        Command::UpdateClips { clips } => {
            assert_eq!((clips[0].id, clips[0].start_tick), (clip_id, 7_680))
        }
        other => panic!("expected UpdateClips, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "removeClips", "ids": [clip_id, id]})),
        Command::RemoveClips {
            ids: vec![clip_id, id]
        }
    );
    assert_eq!(
        parse(json!({"type": "makeClipUnique", "id": clip_id})),
        Command::MakeClipUnique { id: clip_id }
    );
    assert_eq!(
        parse(json!({"type": "setSongLength", "ticks": 92_160})),
        Command::SetSongLength { ticks: 92_160 }
    );
    assert_eq!(
        parse(json!({"type": "setLoopRegion", "region": [15_360, 30_720]})),
        Command::SetLoopRegion {
            region: Some((15_360, 30_720))
        }
    );
    assert_eq!(
        parse(json!({"type": "setLoopRegion", "region": null})),
        Command::SetLoopRegion { region: None }
    );
    match parse(json!({
        "type": "importTracks",
        "tracks": [{"name": "piano", "isDrum": false, "notes": []}],
        "lengthTicks": 15_360,
    })) {
        Command::ImportTracks {
            tracks,
            length_ticks,
        } => assert_eq!((tracks[0].name.as_str(), length_ticks), ("piano", 15_360)),
        other => panic!("expected ImportTracks, got {other:?}"),
    }

    let section_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "addSection", "name": "chorus", "startTick": 30_720})),
        Command::AddSection {
            name: "chorus".into(),
            start_tick: 30_720
        }
    );
    match parse(json!({
        "type": "updateSection",
        "section": {"id": section_id, "name": "verse", "startTick": 15_360, "key": {"rootPitchClass": 7, "scale": "major", "isLocked": true}, "color": "#ff8800"},
    })) {
        Command::UpdateSection { section } => {
            assert_eq!(section.id, section_id);
            assert_eq!(section.start_tick, 15_360);
            assert_eq!(section.key.map(|k| k.root_pitch_class), Some(Some(7)));
        }
        other => panic!("expected UpdateSection, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "removeSection", "id": section_id})),
        Command::RemoveSection { id: section_id }
    );

    assert!(matches!(
        parse(json!({
            "type": "setMaster",
            "master": {"gain": 1.0, "reverbWet": 0.0, "lowCutHz": 20.0, "highCutHz": 18000.0, "lowEq": 1.0, "midEq": 1.0, "highEq": 1.0},
        })),
        Command::SetMaster { .. }
    ));

    assert_eq!(
        parse(json!({"type": "setTimeSignature", "numerator": 3, "denominator": 4})),
        Command::SetTimeSignature {
            numerator: 3,
            denominator: 4
        }
    );
    assert_eq!(
        parse(json!({"type": "setSwing", "swing": 62.0, "gridTicks": 240})),
        Command::SetSwing {
            swing: 62.0,
            grid_ticks: 240
        }
    );

    let point_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "addTempoPoint", "tick": 7_680, "bpm": 140.0, "ramp": true})),
        Command::AddTempoPoint {
            tick: 7_680,
            bpm: 140.0,
            ramp: true
        }
    );
    match parse(
        json!({"type": "updateTempoPoint", "point": {"id": point_id, "tick": 11_520, "bpm": 90.0, "ramp": false}}),
    ) {
        Command::UpdateTempoPoint { point } => {
            assert_eq!(
                (point.id, point.tick, point.bpm, point.ramp),
                (point_id, 11_520, 90.0, false)
            );
        }
        other => panic!("expected UpdateTempoPoint, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "removeTempoPoint", "id": point_id})),
        Command::RemoveTempoPoint { id: point_id }
    );

    // ---- automation lanes

    let track_id = Uuid::new_v4();
    let lane_id = Uuid::new_v4();
    let bp_id = Uuid::new_v4();
    assert_eq!(
        parse(json!({"type": "addAutomationPoint", "target": {"track": [track_id, "reverbSend"]},
                     "tick": 3_840, "value": 0.4, "curve": "ramp"})),
        Command::AddAutomationPoint {
            target: AutoTarget::Track(track_id, TrackParam::ReverbSend),
            tick: 3_840,
            value: 0.4,
            curve: Curve::Ramp
        }
    );
    assert_eq!(
        parse(json!({"type": "addAutomationPoint", "target": {"master": "gain"},
                     "tick": 0, "value": 1.0, "curve": "step"})),
        Command::AddAutomationPoint {
            target: AutoTarget::Master(MasterParam::Gain),
            tick: 0,
            value: 1.0,
            curve: Curve::Step
        }
    );
    // The plugin and insert targets are on the wire from the start even though nothing
    // resolves them yet, so plugin automation will not need a second wire change.
    assert_eq!(
        parse(json!({"type": "addAutomationPoint", "target": {"trackPlugin": [track_id, 17]},
                     "tick": 0, "value": 0.25, "curve": "step"})),
        Command::AddAutomationPoint {
            target: AutoTarget::TrackPlugin(track_id, 17),
            tick: 0,
            value: 0.25,
            curve: Curve::Step
        }
    );
    assert_eq!(
        parse(json!({"type": "addAutomationPoint",
                     "target": {"insert": [{"track": track_id}, lane_id, 3]},
                     "tick": 0, "value": 0.25, "curve": "step"})),
        Command::AddAutomationPoint {
            target: AutoTarget::Insert(InsertTarget::Track(track_id), lane_id, 3),
            tick: 0,
            value: 0.25,
            curve: Curve::Step
        }
    );
    match parse(json!({"type": "updateAutomationPoint", "lane": lane_id,
                       "point": {"id": bp_id, "tick": 1_920, "value": 0.6, "curve": "ramp"}})) {
        Command::UpdateAutomationPoint { lane, point } => {
            assert_eq!(lane, lane_id);
            assert_eq!(
                (point.id, point.tick, point.value, point.curve),
                (bp_id, 1_920, 0.6, Curve::Ramp)
            );
        }
        other => panic!("expected UpdateAutomationPoint, got {other:?}"),
    }
    assert_eq!(
        parse(json!({"type": "removeAutomationPoint", "lane": lane_id, "id": bp_id})),
        Command::RemoveAutomationPoint {
            lane: lane_id,
            id: bp_id
        }
    );
    assert_eq!(
        parse(json!({"type": "clearAutomation", "target": {"track": [track_id, "volume"]}})),
        Command::ClearAutomation {
            target: AutoTarget::Track(track_id, TrackParam::Volume)
        }
    );

    // ---- audio tracks, clips and the guitar rig

    assert_eq!(
        parse(json!({"type": "addAudioTrack", "name": "guitar"})),
        Command::AddAudioTrack {
            name: "guitar".into()
        }
    );

    let source_id = Uuid::new_v4();
    let clip_id = Uuid::new_v4();
    let clip_json = json!({
        "id": clip_id, "trackId": track_id, "sourceId": source_id,
        "startTick": 3_840, "lengthSeconds": 2.5, "offsetSeconds": 0.25,
        "gain": 0.8, "fadeInSeconds": 0.1, "fadeOutSeconds": 0.2, "muted": false,
    });
    let expected_clip = AudioClip {
        id: clip_id,
        track_id,
        source_id,
        start_tick: 3_840,
        length_seconds: 2.5,
        offset_seconds: 0.25,
        gain: 0.8,
        fade_in_seconds: 0.1,
        fade_out_seconds: 0.2,
        muted: false,
        legacy_start_beat: None,
    };
    assert_eq!(
        parse(json!({"type": "updateAudioClip", "clip": clip_json})),
        Command::UpdateAudioClip {
            clip: expected_clip.clone()
        }
    );
    assert_eq!(
        parse(json!({"type": "removeAudioClip", "id": clip_id})),
        Command::RemoveAudioClip { id: clip_id }
    );

    match parse(json!({
        "type": "addAudioTake", "trackId": track_id,
        "source": {"id": source_id, "name": "take 1", "frames": 48000, "sampleRate": 48000, "channels": 1},
        "clip": clip_json,
    })) {
        Command::AddAudioTake {
            track_id: t,
            source,
            clip,
        } => {
            assert_eq!(t, track_id);
            assert_eq!(
                (
                    source.id,
                    source.frames,
                    source.sample_rate,
                    source.channels
                ),
                (source_id, 48_000, 48_000, 1)
            );
            assert_eq!(source.name, "take 1");
            assert_eq!(clip, expected_clip);
        }
        other => panic!("expected AddAudioTake, got {other:?}"),
    }

    // The rig is one whole record per edit, and `pedals` is an internally tagged union — the
    // part most likely to drift between `ui/src/types.ts` and here.
    match parse(json!({
        "type": "setTrackAmp", "id": id,
        "amp": {
            "enabled": true,
            "inputDb": 3.0,
            "gate": {"on": true, "thresholdDb": -55.0, "releaseMs": 120.0},
            "pedals": [
                {"kind": "comp", "on": true, "sustain": 6.0, "level": 5.0},
                {"kind": "drive", "on": true, "drive": 7.0, "tone": 4.0, "level": 5.0},
                {"kind": "wah", "on": false, "sensitivity": 5.0, "range": 6.0},
                {"kind": "delay", "on": true, "timeMs": 350.0, "feedback": 3.0, "mix": 2.0},
            ],
            "amp": {"model": "lead", "gain": 8.0, "bass": 5.0, "mid": 4.0, "treble": 6.0, "presence": 5.0, "master": 5.0},
            "eq": "britishBright",
            "cab": {"model": "fourByTwelve", "micModel": "ribbon", "posX": 7.0, "posZ": 1.5},
            "rack": {"reverb": 2.0, "decay": 5.0, "outputDb": -1.5},
        },
    })) {
        Command::SetTrackAmp { id: t, amp } => {
            assert_eq!(t, id);
            assert_eq!(amp.input_db, 3.0);
            assert_eq!(amp.amp.model, AmpModel::Lead);
            assert_eq!(amp.eq, EqModel::BritishBright);
            assert_eq!(amp.cab.model, CabModel::FourByTwelve);
            assert_eq!(amp.cab.mic_model, MicModel::Ribbon);
            assert_eq!((amp.cab.pos_x, amp.cab.pos_z), (7.0, 1.5));
            assert_eq!(amp.rack.output_db, -1.5);
            assert_eq!(
                amp.pedals,
                vec![
                    Pedal::Comp {
                        on: true,
                        sustain: 6.0,
                        level: 5.0
                    },
                    Pedal::Drive {
                        on: true,
                        drive: 7.0,
                        tone: 4.0,
                        level: 5.0
                    },
                    Pedal::Wah {
                        on: false,
                        sensitivity: 5.0,
                        range: 6.0
                    },
                    Pedal::Delay {
                        on: true,
                        time_ms: 350.0,
                        feedback: 3.0,
                        mix: 2.0
                    },
                ]
            );
        }
        other => panic!("expected SetTrackAmp, got {other:?}"),
    }

    // A rig saved before the amp was broken into components still opens: the missing pieces take
    // their defaults rather than sinking the command.
    match parse(json!({
        "type": "setTrackAmp", "id": id,
        "amp": {
            "enabled": true, "inputDb": 0.0,
            "gate": {"on": true, "thresholdDb": -55.0, "releaseMs": 120.0},
            "pedals": [],
            "amp": {"model": "crunch", "gain": 5.0, "bass": 5.0, "mid": 5.0, "treble": 6.0, "presence": 5.0, "master": 5.0},
            "cab": {"model": "twoByTwelve"},
            "rack": {"reverb": 2.0, "decay": 5.0, "outputDb": 0.0},
        },
    })) {
        Command::SetTrackAmp { amp, .. } => {
            assert_eq!(amp.eq, EqModel::default());
            assert_eq!(amp.cab.model, CabModel::TwoByTwelve);
            assert_eq!(amp.cab.mic_model, MicModel::default());
            assert_eq!(
                (amp.cab.pos_x, amp.cab.pos_z),
                (Cabinet::default().pos_x, Cabinet::default().pos_z)
            );
        }
        other => panic!("expected SetTrackAmp, got {other:?}"),
    }

    // Every remaining pedal kind, so no variant's field names go unpinned.
    for (payload, expected) in [
        (
            json!({"kind": "chorus", "on": true, "rateHz": 1.2, "depth": 5.0, "mix": 4.0}),
            Pedal::Chorus {
                on: true,
                rate_hz: 1.2,
                depth: 5.0,
                mix: 4.0,
            },
        ),
        (
            json!({"kind": "phaser", "on": true, "rateHz": 0.8, "depth": 5.0, "feedback": 4.0}),
            Pedal::Phaser {
                on: true,
                rate_hz: 0.8,
                depth: 5.0,
                feedback: 4.0,
            },
        ),
        (
            json!({"kind": "tremolo", "on": false, "rateHz": 5.0, "depth": 6.0}),
            Pedal::Tremolo {
                on: false,
                rate_hz: 5.0,
                depth: 6.0,
            },
        ),
    ] {
        let got: Pedal = serde_json::from_value(payload.clone())
            .unwrap_or_else(|e| panic!("UI pedal rejected: {payload} — {e}"));
        assert_eq!(got, expected);
    }
}
