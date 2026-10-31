use dissonant_core::*;

const BEAT: i64 = TICKS_PER_BEAT;

#[test]
fn round_trip_preserves_all_state() {
    let mut model = ProjectModel::starter();
    let pid = model.patterns[0].id;
    model.tempo = 96.0;
    model.key = KeyState::locked(0, ScaleType::Major);
    model.tracks[0].soloed = true;
    model.tracks[0].pan = -0.3;
    model.master.reverb_wet = 0.25;
    model.patterns[0].notes = vec![NoteEvent::new(0, BEAT, 60).with_velocity(77)];
    model.clips = vec![
        Clip::new(pid, 0, 16 * BEAT),
        Clip::new(pid, 16 * BEAT, 8 * BEAT),
    ];
    model.sections = vec![Section::new("intro", 0)];
    model.song_length_ticks = 48 * BEAT;
    model.loop_region = Some((16 * BEAT, 32 * BEAT));

    let json = model.to_json().unwrap();
    let decoded = ProjectModel::from_json(&json).unwrap();
    assert_eq!(decoded, model);
}

#[test]
fn audio_tracks_clips_and_rigs_round_trip() {
    let mut model = ProjectModel::starter();
    let mut guitar = Track::audio("guitar");
    let rig = guitar
        .amp
        .as_mut()
        .expect("an audio track is created with a rig");
    rig.amp.model = AmpModel::Fuzz;
    rig.amp.gain = 9.5;
    rig.cab.model = CabModel::TwoByTwelve;
    rig.input_db = -3.0;
    rig.pedals = vec![
        Pedal::Comp {
            on: true,
            sustain: 7.0,
            level: 5.0,
        },
        Pedal::Delay {
            on: true,
            time_ms: 420.0,
            feedback: 4.0,
            mix: 3.0,
        },
    ];
    let gid = guitar.id;
    model.tracks.push(guitar);

    let source = AudioSource {
        id: uuid::Uuid::new_v4(),
        name: "riff.wav".into(),
        frames: 96_000,
        sample_rate: 48_000,
        channels: 2,
    };
    let mut clip = AudioClip::new(gid, source.id, 8 * BEAT, 2.0);
    clip.gain = 0.7;
    clip.fade_in_seconds = 0.2;
    clip.offset_seconds = 0.5;
    model.audio_sources = vec![source];
    model.audio_clips = vec![clip];

    let decoded = ProjectModel::from_json(&model.to_json().unwrap()).unwrap();
    assert_eq!(decoded, model);
}

#[test]
fn a_project_with_no_audio_writes_no_audio_keys() {
    // Opening and re-saving an old project must not grow it with empty collections.
    let json = ProjectModel::starter().to_json().unwrap();
    assert!(
        !json.contains("audioClips"),
        "empty audio clips were written:\n{json}"
    );
    assert!(
        !json.contains("audioSources"),
        "empty audio sources were written:\n{json}"
    );
}

#[test]
fn the_song_loop_survives_a_save_and_open_round_trip() {
    let mut model = ProjectModel::starter();
    model.loop_region = Some((32 * BEAT, 64 * BEAT));
    let json = model.to_json().unwrap();
    assert!(json.contains("loopRegion"), "no loop key written:\n{json}");
    let decoded = ProjectModel::from_json(&json).unwrap();
    assert_eq!(decoded.loop_region, Some((32 * BEAT, 64 * BEAT)));
}

#[test]
fn a_project_with_no_loop_writes_no_loop_key() {
    // A pair of ticks is a `#[serde(default)]` addition, not a schema bump: an unlooped project
    // has to come back out of the writer exactly as it went in.
    let json = ProjectModel::starter().to_json().unwrap();
    assert!(!json.contains("loopRegion"), "an empty loop was written:\n{json}");
    assert_eq!(ProjectModel::from_json(&json).unwrap().loop_region, None);
}

#[test]
fn a_loop_saved_past_the_song_end_opens_held_inside_it() {
    let mut model = ProjectModel::starter();
    model.song_length_ticks = 32 * BEAT;
    // Hand-edited (or written before the song was shortened): the brace cannot outlive the song.
    model.loop_region = Some((16 * BEAT, 400 * BEAT));
    let decoded = ProjectModel::from_json(&model.to_json().unwrap()).unwrap();
    assert_eq!(decoded.loop_region, Some((16 * BEAT, 32 * BEAT)));

    model.loop_region = Some((900 * BEAT, 950 * BEAT));
    let decoded = ProjectModel::from_json(&model.to_json().unwrap()).unwrap();
    assert_eq!(decoded.loop_region, None, "a brace entirely past the end is dropped");
}

#[test]
fn a_file_written_before_audio_tracks_existed_opens_with_no_audio() {
    let track = "11111111-1111-1111-1111-111111111111";
    let v7 = format!(
        r#"{{
        "schemaVersion": 7,
        "tempo": 110,
        "tracks": [{{"id": "{track}", "name": "lead", "voice": "saw"}}],
        "patterns": [{{
            "id": "22222222-2222-2222-2222-222222222222", "name": "p1", "lengthBeats": 16,
            "notesByTrack": {{ "{track}": [ {{ "id": "44444444-4444-4444-4444-444444444444", "startBeat": 0, "lengthBeats": 1, "pitch": 60 }} ] }}
        }}]
    }}"#
    );
    let m = ProjectModel::from_json(&v7).unwrap();
    assert_eq!(m.schema_version, SCHEMA_VERSION);
    assert!(!m.tracks[0].is_audio);
    assert_eq!(m.tracks[0].amp, None);
    assert!(m.audio_clips.is_empty());
    assert!(m.audio_sources.is_empty());
    assert_eq!(m.patterns[0].length_ticks, 16 * BEAT);
}

#[test]
fn loading_drops_audio_clips_whose_track_or_source_is_gone() {
    let mut m = ProjectModel::starter();
    let guitar = Track::audio("guitar");
    let gid = guitar.id;
    m.tracks.push(guitar);
    let source = AudioSource {
        id: uuid::Uuid::new_v4(),
        name: "take".into(),
        frames: 1_000,
        sample_rate: 48_000,
        channels: 1,
    };
    let sid = source.id;
    m.audio_sources = vec![source];
    m.audio_clips = vec![
        AudioClip::new(gid, sid, 0, 1.0),                  // keeper
        AudioClip::new(uuid::Uuid::new_v4(), sid, 0, 1.0), // no such track
        AudioClip::new(gid, uuid::Uuid::new_v4(), 0, 1.0), // no such source
        AudioClip::new(m.tracks[0].id, sid, 0, 1.0),       // not an audio track
        AudioClip::new(gid, sid, 0, 0.0),                  // zero length
    ];
    let decoded = ProjectModel::from_json(&m.to_json().unwrap()).unwrap();
    assert_eq!(decoded.audio_clips.len(), 1);
    assert_eq!(decoded.audio_clips[0].track_id, gid);
    // The source survives even when nothing points at it, so undo can bring a take back
    // without decoding the file again.
    assert_eq!(decoded.audio_sources.len(), 1);
}

#[test]
fn a_hand_edited_audio_track_cannot_also_be_a_drum_track() {
    let mut m = ProjectModel::starter();
    let mut t = Track::audio("guitar");
    t.is_drum = true;
    t.amp = None;
    m.tracks.push(t);
    let decoded = ProjectModel::from_json(&m.to_json().unwrap()).unwrap();
    let guitar = decoded.tracks.iter().find(|t| t.is_audio).unwrap();
    assert!(!guitar.is_drum);
    assert!(guitar.amp.is_some(), "an audio track always carries a rig");
    // …and a note track never carries one.
    assert_eq!(decoded.tracks[0].amp, None);
}

#[test]
fn empty_project_defaults() {
    let empty = ProjectModel::empty();
    assert_eq!(empty.tracks.len(), 1);
    assert_eq!(empty.patterns.len(), 1);
    assert_eq!(
        empty.patterns[0].track_id, empty.tracks[0].id,
        "the pattern belongs to the track"
    );
    assert_eq!(empty.patterns[0].name, "melody 1");
    assert!(empty.patterns[0].notes.is_empty());
    assert!(empty.chords.is_empty());
    assert_eq!(empty.song_length_ticks, DEFAULT_SONG_BARS as i64 * 4 * BEAT);
    assert_eq!(empty.tempo, 120.0);
    assert_eq!(empty.key, KeyState::NONE);
    assert_eq!(empty.patterns[0].length_ticks, 16 * BEAT);
    assert_eq!(empty.swing_grid_ticks, BEAT / 2);
}

#[test]
fn decoding_missing_fields_does_not_fail() {
    let decoded = ProjectModel::from_json(r#"{ "schemaVersion": 6, "tempo": 140 }"#).unwrap();
    assert_eq!(decoded.tempo, 140.0);
    assert_eq!(decoded.tracks.len(), 1);
    assert_eq!(decoded.patterns.len(), 1);
    assert_eq!(decoded.schema_version, SCHEMA_VERSION);
    assert_eq!(decoded.song_length_ticks, 128 * BEAT);

    let decoded = ProjectModel::from_json(r#"{ "schemaVersion": 6 }"#).unwrap();
    assert_eq!(decoded.tempo, 120.0);
    assert_eq!(decoded.master, MasterSettings::default());
}

/// A file from the future is refused rather than opened and re-saved without the fields this
/// build never knew about.
#[test]
fn a_file_from_a_newer_version_is_refused() {
    let err =
        ProjectModel::from_json(r#"{ "schemaVersion": 99 }"#).expect_err("a newer file was opened");
    assert!(
        err.to_string().contains("newer version of dissonant"),
        "{err}"
    );
    assert!(ProjectModel::from_json(r#"{ "schemaVersion": 6 }"#).is_ok());
    // A project is an object: a bare array would otherwise decode as a blank project and the
    // next save would destroy the file.
    assert!(ProjectModel::from_json("[]").is_err());
}

#[test]
fn notes_without_velocity_default_to_100_and_bad_values_are_clamped() {
    let track = "11111111-1111-1111-1111-111111111111";
    let json = format!(
        r#"{{
          "schemaVersion": 6,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "22222222-2222-2222-2222-222222222222", "trackId": "{track}", "name": "p",
            "notes": [ {{ "id": "44444444-4444-4444-4444-444444444444", "startBeat": 1, "lengthBeats": 1, "pitch": 300 }} ]
          }} ],
          "chords": [ {{ "id": "33333333-3333-3333-3333-333333333333", "startBeat": 0, "lengthBeats": 4, "pitchClasses": [0,4,7], "name": "C" }} ]
        }}"#
    );
    let decoded = ProjectModel::from_json(&json).unwrap();
    let notes = &decoded.patterns[0].notes;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].velocity, 100);
    assert_eq!(notes[0].pitch, 127);
    assert_eq!(notes[0].start_tick, BEAT);
    assert_eq!(decoded.tracks[0].voice, "saw");
    assert_eq!(decoded.tracks[0].tone, 18_000.0);
    assert_eq!(
        decoded.chords.chord_at(BEAT).unwrap().name.as_deref(),
        Some("C")
    );
}

#[test]
fn legacy_arrangement_lays_clips_back_to_back() {
    let track = "11111111-1111-1111-1111-111111111111";
    let (a, b) = (
        "22222222-2222-2222-2222-222222222222",
        "33333333-3333-3333-3333-333333333333",
    );
    let note =
        |id: &str| format!(r#"{{ "id": "{id}", "startBeat": 0, "lengthBeats": 1, "pitch": 60 }}"#);
    let json = format!(
        r#"{{
      "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
      "patterns": [ {{ "id": "{a}", "name": "a", "lengthBeats": 8, "notesByTrack": {{ "{track}": [ {} ] }} }},
                    {{ "id": "{b}", "name": "b", "lengthBeats": 16, "notesByTrack": {{ "{track}": [ {} ] }} }} ],
      "arrangement": [ "{a}", "{b}", "{a}" ]
    }}"#,
        note("44444444-4444-4444-4444-444444444444"),
        note("55555555-5555-5555-5555-555555555555")
    );
    let m = ProjectModel::from_json(&json).unwrap();
    let starts: Vec<i64> = m.clips.iter().map(|c| c.start_tick).collect();
    assert_eq!(starts, vec![0, 8 * BEAT, 24 * BEAT]);
    // The clips survive the v10 split: each one now plays the pattern of the track it fed.
    assert_eq!(m.patterns.len(), 2, "one pattern per track per old pattern");
    assert!(m.clips.iter().all(|c| m.pattern(&c.pattern_id).is_some()));
    assert!(!m.to_json().unwrap().contains("\"arrangement\""));
}

#[test]
fn loading_tidies_chords_song_length_and_clips() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    let json = format!(
        r#"{{
          "schemaVersion": 6,
          "songLengthTicks": 960,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{ "id": "{pattern}", "trackId": "{track}", "name": "p" }} ],
          "chords": [
            {{ "startBeat": 4, "lengthBeats": 4, "pitchClasses": [7, 11, 2] }},
            {{ "startBeat": 0, "lengthBeats": 8, "pitchClasses": [0, 4, 7] }},
            {{ "startBeat": -2, "lengthBeats": 4, "pitchClasses": [0] }}
          ],
          "clips": [
            {{ "patternId": "{pattern}", "startBeat": 2, "lengthBeats": 16 }},
            {{ "patternId": "{pattern}", "startBeat": 8, "lengthBeats": 4 }}
          ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    assert_eq!(m.song_length_ticks, 4 * BEAT, "at least one bar");
    let chords: Vec<(i64, i64)> = m
        .chords
        .chords()
        .iter()
        .map(|c| (c.start_tick, c.length_ticks))
        .collect();
    assert_eq!(
        chords,
        vec![(0, 4 * BEAT), (4 * BEAT, 4 * BEAT)],
        "sorted, the bad one dropped, no overlap"
    );
    let clips: Vec<(i64, i64)> = m
        .clips
        .iter()
        .map(|c| (c.start_tick, c.length_ticks))
        .collect();
    assert_eq!(
        clips,
        vec![(2 * BEAT, 2 * BEAT)],
        "the clip past the end is gone and the crossing one cut"
    );
}

#[test]
fn embedded_sounds_and_custom_drum_pads_round_trip() {
    let mut model = ProjectModel::starter();
    let synth = model.tracks.iter().position(|t| !t.is_drum).unwrap();
    let drums = model.tracks.iter().position(|t| t.is_drum).unwrap();
    let lead = SynthPatch {
        level: 0.5,
        mono: true,
        glide: 0.25,
        ..SynthPatch::default()
    };
    let boom = DrumPatch {
        tune: -2.0,
        length: 0.8,
        ..DrumPatch::default()
    };
    model.sounds = vec![
        Sound {
            id: "u-lead".into(),
            name: "lead".into(),
            category: "lead".into(),
            patch: SoundPatch::Synth(lead),
        },
        Sound {
            id: "u-boom".into(),
            name: "boom".into(),
            category: "kick".into(),
            patch: SoundPatch::Drum(boom),
        },
    ];
    model.tracks[synth].voice = "u-lead".into();
    model.tracks[drums].drum_pads = vec![
        DrumPad {
            pitch: 36,
            sound: "u-boom".into(),
            choke: 0,
        },
        DrumPad {
            pitch: 42,
            sound: "hat".into(),
            choke: 1,
        },
        DrumPad {
            pitch: 46,
            sound: "open-hat".into(),
            choke: 1,
        },
    ];

    let json = model.to_json().unwrap();
    assert!(json.contains("\"drumPads\"") && json.contains("\"sounds\""));
    let decoded = ProjectModel::from_json(&json).unwrap();
    assert_eq!(decoded, model);
    // And the copy in the file is what plays, on a machine whose library has never seen it.
    assert_eq!(decoded.resolve_synth("u-lead"), lead);
    assert_eq!(decoded.resolve_drum("u-boom"), boom);
}

#[test]
fn a_file_from_before_sounds_opens_with_the_default_kit() {
    let json = r#"{
      "schemaVersion": 6,
      "tracks": [
        { "id": "11111111-1111-1111-1111-111111111111", "name": "lead", "voice": "pluck" },
        { "id": "22222222-2222-2222-2222-222222222222", "name": "drums", "isDrum": true }
      ]
    }"#;
    let m = ProjectModel::from_json(json).unwrap();
    assert!(m.sounds.is_empty());
    assert_eq!(
        m.tracks[1].drum_pads,
        default_kit(),
        "a drum track without drumPads gets the default kit"
    );
    for (pitch, sound) in [(36, "kick"), (38, "snare"), (39, "clap"), (42, "hat")] {
        assert!(
            m.tracks[1]
                .drum_pads
                .iter()
                .any(|p| p.pitch == pitch && p.sound == sound),
            "old groove pitch {pitch} lost its {sound}"
        );
    }
    // The old voice name is a factory id, so it still plays the same patch.
    match &factory_sound("pluck").unwrap().patch {
        SoundPatch::Synth(p) => assert_eq!(m.resolve_synth(&m.tracks[0].voice), *p),
        _ => panic!("factory pluck is not a synth"),
    }
    assert!(
        !m.to_json().unwrap().contains("\"sounds\""),
        "re-saving added an empty sounds list"
    );
}

#[test]
fn json_uses_camel_case_tick_keys() {
    let json = ProjectModel::starter().to_json().unwrap();
    for key in [
        "schemaVersion",
        "trackId",
        "lengthTicks",
        "songLengthTicks",
        "swingGridTicks",
        "chords",
        "notes",
    ] {
        assert!(
            json.contains(&format!("\"{key}\"")),
            "missing {key}:\n{json}"
        );
    }
    assert!(
        !json.contains("notesByTrack"),
        "a pattern no longer holds every track's notes:\n{json}"
    );
    assert!(!json.contains("length_ticks"));
    assert!(json.contains(&format!("\"schemaVersion\": {SCHEMA_VERSION}")));
}

/// Every schema ≤ 8 time key folds into its tick field at exactly 960 PPQ, including the
/// fractional ones a quantize grid produces (0.25 beats → 240 ticks).
#[test]
fn a_schema_8_file_migrates_every_beat_field_to_ticks() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    let json = format!(
        r#"{{
          "schemaVersion": 8,
          "tempo": 120,
          "swing": 60,
          "swingGrid": 0.25,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "{pattern}", "name": "p", "lengthBeats": 8,
            "chords": [ {{ "id": "33333333-3333-3333-3333-333333333333", "startBeat": 0.5, "lengthBeats": 1.5, "pitchClasses": [0,4,7], "name": "C" }} ],
            "notesByTrack": {{ "{track}": [ {{ "id": "44444444-4444-4444-4444-444444444444", "startBeat": 0.25, "lengthBeats": 0.75, "pitch": 60, "shift": -0.125 }} ] }}
          }} ],
          "clips": [ {{ "id": "55555555-5555-5555-5555-555555555555", "patternId": "{pattern}", "startBeat": 16, "lengthBeats": 8, "offsetBeats": 1.5 }} ],
          "sections": [ {{ "id": "66666666-6666-6666-6666-666666666666", "name": "intro", "startBeat": 2.5 }} ],
          "tempoPoints": [ {{ "id": "77777777-7777-7777-7777-777777777777", "beat": 12.5, "bpm": 140, "ramp": true }} ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    assert_eq!(m.schema_version, SCHEMA_VERSION);

    let p = &m.patterns[0];
    assert_eq!(
        p.track_id, m.tracks[0].id,
        "the pattern found its owning track"
    );
    assert_eq!(p.length_ticks, 8 * BEAT);
    let note = &p.notes[0];
    assert_eq!(note.start_tick, 240, "0.25 beats is 240 ticks");
    assert_eq!(note.length_ticks, 720);
    assert_eq!(note.shift_ticks, -120);

    assert_eq!(m.clips[0].start_tick, 16 * BEAT);
    assert_eq!(m.clips[0].length_ticks, 8 * BEAT);
    assert_eq!(m.clips[0].offset_ticks, 1440);
    // The pattern's own chords moved onto the song, under the clip that played them.
    let chord = &m.chords.chords()[0];
    assert_eq!(
        chord.start_tick,
        16 * BEAT,
        "the chord that was sounding at the clip start begins there"
    );
    assert_eq!(chord.name.as_deref(), Some("C"));
    assert_eq!(m.sections[0].start_tick, 2400);
    assert_eq!(m.tempo_points[0].tick, 12_000);
    assert_eq!(m.swing_grid_ticks, BEAT / 4);
    assert_eq!(m.swing, 60.0);
}

/// Migration is one-way: what a migrated project writes back carries ticks only, so the next
/// load has nothing left to fold.
#[test]
fn a_migrated_project_is_written_back_without_legacy_keys() {
    let track = "11111111-1111-1111-1111-111111111111";
    let json = format!(
        r#"{{
          "schemaVersion": 8,
          "swingGrid": 0.5,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "22222222-2222-2222-2222-222222222222", "name": "p", "lengthBeats": 8,
            "chords": [ {{ "id": "33333333-3333-3333-3333-333333333333", "startBeat": 0, "lengthBeats": 4, "pitchClasses": [0,4,7] }} ],
            "notesByTrack": {{ "{track}": [ {{ "id": "44444444-4444-4444-4444-444444444444", "startBeat": 1, "lengthBeats": 1, "pitch": 60, "shift": 0.25 }} ] }},
            "trackLoopBeats": {{ "{track}": 4 }},
            "chordLoopBeats": 4
          }} ],
          "clips": [ {{ "id": "55555555-5555-5555-5555-555555555555", "patternId": "22222222-2222-2222-2222-222222222222", "startBeat": 0, "lengthBeats": 8, "offsetBeats": 2 }} ],
          "sections": [ {{ "id": "66666666-6666-6666-6666-666666666666", "name": "intro", "startBeat": 4 }} ],
          "tempoPoints": [ {{ "id": "77777777-7777-7777-7777-777777777777", "beat": 4, "bpm": 140 }} ]
        }}"#
    );
    let written = ProjectModel::from_json(&json).unwrap().to_json().unwrap();
    for key in [
        "\"startBeat\"",
        "\"lengthBeats\"",
        "\"offsetBeats\"",
        "\"trackLoopBeats\"",
        "\"chordLoopBeats\"",
        "\"trackLoopTicks\"",
        "\"chordLoopTicks\"",
        "\"notesByTrack\"",
        "\"swingGrid\"",
        "\"shift\"",
        "\"beat\"",
    ] {
        assert!(
            !written.contains(key),
            "{key} survived the migration:\n{written}"
        );
    }
    // Reloading what we wrote changes nothing.
    let once = ProjectModel::from_json(&written).unwrap();
    assert_eq!(
        once,
        ProjectModel::from_json(&once.to_json().unwrap()).unwrap()
    );
}

#[test]
fn imports_swift_flat_array_notes_by_track() {
    // The Swift app's Codable wrote `[UUID: [NoteEvent]]` as [key, value, key, value].
    let json = r#"{
      "schemaVersion": 2, "tempo": 110,
      "key": { "rootPitchClass": 7, "scale": "major", "isLocked": true },
      "tracks": [ { "id": "11111111-1111-1111-1111-111111111111", "name": "melody", "voice": "pluck", "muted": false, "soloed": false, "isDrum": false, "volume": 1, "reverbSend": 0.2, "tone": 9000, "pan": 0 } ],
      "patterns": [ { "id": "22222222-2222-2222-2222-222222222222", "name": "verse", "lengthBeats": 16,
        "chords": [],
        "notesByTrack": [ "11111111-1111-1111-1111-111111111111", [ { "id": "44444444-4444-4444-4444-444444444444", "startBeat": 2, "lengthBeats": 0.5, "pitch": 67 } ] ] } ],
      "arrangement": [ "22222222-2222-2222-2222-222222222222" ]
    }"#;
    let m = ProjectModel::from_json(json).unwrap();
    let notes = &m.patterns[0].notes;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].pitch, 67);
    assert_eq!(notes[0].velocity, 100);
    assert_eq!(notes[0].start_tick, 2 * BEAT);
    assert_eq!(notes[0].length_ticks, BEAT / 2);
    assert_eq!(m.patterns[0].track_id, m.tracks[0].id);
    assert_eq!(
        m.clips.len(),
        1,
        "the arrangement entry became a clip of the split pattern"
    );
    assert_eq!(m.key.root_pitch_class, Some(7));
    assert_eq!(m.time_signature, TimeSignature::default());
    assert_eq!(m.schema_version, SCHEMA_VERSION);
}

// ------------------------------------------------- the schema 9 → 10 split

/// The headline migration: one pattern that held two tracks' notes and its own progression
/// becomes one pattern per track, each clip of the old pattern is duplicated onto every split
/// pattern, and the chords move onto the song under those clips.
#[test]
fn a_schema_9_pattern_splits_into_one_pattern_per_track() {
    let (lead, drums) = (
        "11111111-1111-1111-1111-111111111111",
        "22222222-2222-2222-2222-222222222222",
    );
    let pattern = "33333333-3333-3333-3333-333333333333";
    let json = format!(
        r#"{{
          "schemaVersion": 9,
          "tracks": [ {{ "id": "{lead}", "name": "lead" }}, {{ "id": "{drums}", "name": "drums", "isDrum": true }} ],
          "patterns": [ {{
            "id": "{pattern}", "name": "verse", "lengthTicks": 15360,
            "chords": [
              {{ "id": "44444444-4444-4444-4444-444444444444", "startTick": 0, "lengthTicks": 7680, "pitchClasses": [0,4,7], "name": "C" }},
              {{ "id": "55555555-5555-5555-5555-555555555555", "startTick": 7680, "lengthTicks": 7680, "pitchClasses": [5,9,0], "name": "F" }}
            ],
            "notesByTrack": {{
              "{lead}": [ {{ "id": "66666666-6666-6666-6666-666666666666", "startTick": 960, "lengthTicks": 960, "pitch": 67 }} ],
              "{drums}": [ {{ "id": "77777777-7777-7777-7777-777777777777", "startTick": 0, "lengthTicks": 240, "pitch": 36 }} ]
            }}
          }} ],
          "clips": [
            {{ "id": "88888888-8888-8888-8888-888888888888", "patternId": "{pattern}", "startTick": 0, "lengthTicks": 15360 }},
            {{ "id": "99999999-9999-9999-9999-999999999999", "patternId": "{pattern}", "startTick": 15360, "lengthTicks": 15360 }}
          ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    let (lead_id, drums_id) = (m.tracks[0].id, m.tracks[1].id);

    let names: Vec<&str> = m.patterns.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["lead verse", "drums verse"],
        "one pattern per track, named after both"
    );
    let lead_pattern = m.patterns_of(&lead_id).next().unwrap();
    let drums_pattern = m.patterns_of(&drums_id).next().unwrap();
    assert_eq!(lead_pattern.length_ticks, 16 * BEAT);
    assert_eq!(
        lead_pattern
            .notes
            .iter()
            .map(|n| (n.start_tick, n.pitch))
            .collect::<Vec<_>>(),
        vec![(BEAT, 67)]
    );
    assert_eq!(
        drums_pattern
            .notes
            .iter()
            .map(|n| (n.start_tick, n.pitch))
            .collect::<Vec<_>>(),
        vec![(0, 36)]
    );

    // Both clips of the old pattern now play on both tracks, in their old places.
    assert_eq!(m.clips.len(), 4);
    let spans = |pid| {
        m.clips
            .iter()
            .filter(|c| c.pattern_id == pid)
            .map(|c| (c.start_tick, c.length_ticks))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        spans(lead_pattern.id),
        vec![(0, 16 * BEAT), (16 * BEAT, 16 * BEAT)]
    );
    assert_eq!(spans(drums_pattern.id), spans(lead_pattern.id));

    // The pattern's progression is now the song's, repeated under each clip.
    let chords: Vec<(i64, i64, &str)> = m
        .chords
        .chords()
        .iter()
        .map(|c| {
            (
                c.start_tick,
                c.length_ticks,
                c.name.as_deref().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        chords,
        vec![
            (0, 8 * BEAT, "C"),
            (8 * BEAT, 8 * BEAT, "F"),
            (16 * BEAT, 8 * BEAT, "C"),
            (24 * BEAT, 8 * BEAT, "F"),
        ]
    );
    assert_eq!(
        m.song_length_ticks,
        128 * BEAT,
        "the default song is long enough to hold both clips"
    );
    assert!(m.patterns.iter().all(|p| !p.track_id.is_nil()));
    // Nothing is left to migrate: a second pass through the file changes nothing.
    assert_eq!(m, ProjectModel::from_json(&m.to_json().unwrap()).unwrap());
}

/// A schema ≤ 9 pattern could loop one track (or its chords) faster than the pattern itself.
/// There is nowhere to keep that in v10, so the migration flattens it: the loop is written out
/// as repeated material.
#[test]
fn the_v10_split_flattens_the_old_per_track_and_chord_loops() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    let json = format!(
        r#"{{
          "schemaVersion": 9,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "{pattern}", "name": "p", "lengthTicks": 15360,
            "chords": [ {{ "id": "33333333-3333-3333-3333-333333333333", "startTick": 0, "lengthTicks": 3840, "pitchClasses": [0,4,7], "name": "C" }} ],
            "notesByTrack": {{ "{track}": [ {{ "id": "44444444-4444-4444-4444-444444444444", "startTick": 0, "lengthTicks": 960, "pitch": 60 }} ] }},
            "trackLoopTicks": {{ "{track}": 3840 }},
            "chordLoopTicks": 3840
          }} ],
          "clips": [ {{ "id": "55555555-5555-5555-5555-555555555555", "patternId": "{pattern}", "startTick": 0, "lengthTicks": 15360 }} ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    let starts: Vec<i64> = m.patterns[0].notes.iter().map(|n| n.start_tick).collect();
    assert_eq!(
        starts,
        vec![0, 4 * BEAT, 8 * BEAT, 12 * BEAT],
        "the 4-beat track loop is written out"
    );
    let ids: std::collections::HashSet<_> = m.patterns[0].notes.iter().map(|n| n.id).collect();
    assert_eq!(ids.len(), 4, "every repeat keeps its own id");
    let chords: Vec<i64> = m.chords.chords().iter().map(|c| c.start_tick).collect();
    assert_eq!(
        chords,
        vec![0, 4 * BEAT, 8 * BEAT, 12 * BEAT],
        "and so is the chord loop"
    );
}

/// A clip that started mid-pattern or ran past its end carried the chords with it; the lifted
/// progression lands where it was heard, cut at the clip's edges.
#[test]
fn lifted_chords_follow_an_offset_clip() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    let json = format!(
        r#"{{
          "schemaVersion": 9,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "{pattern}", "name": "p", "lengthTicks": 7680,
            "chords": [
              {{ "id": "33333333-3333-3333-3333-333333333333", "startTick": 0, "lengthTicks": 3840, "pitchClasses": [0,4,7], "name": "C" }},
              {{ "id": "44444444-4444-4444-4444-444444444444", "startTick": 3840, "lengthTicks": 3840, "pitchClasses": [5,9,0], "name": "F" }}
            ],
            "notesByTrack": {{ "{track}": [ {{ "id": "55555555-5555-5555-5555-555555555555", "startTick": 0, "lengthTicks": 960, "pitch": 60 }} ] }}
          }} ],
          "clips": [ {{ "id": "66666666-6666-6666-6666-666666666666", "patternId": "{pattern}", "startTick": 7680, "lengthTicks": 11520, "offsetTicks": 3840 }} ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    let chords: Vec<(i64, i64, &str)> = m
        .chords
        .chords()
        .iter()
        .map(|c| {
            (
                c.start_tick,
                c.length_ticks,
                c.name.as_deref().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        chords,
        vec![
            (8 * BEAT, 4 * BEAT, "F"),
            (12 * BEAT, 4 * BEAT, "C"),
            (16 * BEAT, 4 * BEAT, "F")
        ],
        "the clip starts on the second chord and loops the pattern once more"
    );
    assert_eq!(
        m.clips[0].offset_ticks,
        4 * BEAT,
        "the clip keeps its offset"
    );
}

/// Schema 9 wrote a pattern's chord track as the struct it was then — `{"chords": [...]}` —
/// while every other version writes a bare array. A v9 project must still open, chords and all.
#[test]
fn a_v9_pattern_chord_track_written_as_an_object_still_opens() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    let json = format!(
        r#"{{
          "schemaVersion": 9,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "{pattern}", "name": "p", "lengthTicks": 3840,
            "chords": {{ "chords": [ {{ "id": "33333333-3333-3333-3333-333333333333", "startTick": 0, "lengthTicks": 3840, "pitchClasses": [0,4,7], "name": "C" }} ] }},
            "notesByTrack": {{ "{track}": [ {{ "id": "44444444-4444-4444-4444-444444444444", "startTick": 0, "lengthTicks": 960, "pitch": 60 }} ] }}
          }} ],
          "clips": [ {{ "id": "55555555-5555-5555-5555-555555555555", "patternId": "{pattern}", "startTick": 0, "lengthTicks": 3840 }} ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    assert_eq!(m.patterns.len(), 1, "the pattern split onto its one track");
    assert_eq!(m.patterns[0].notes.len(), 1);
    let chords: Vec<(i64, i64)> = m
        .chords
        .chords()
        .iter()
        .map(|c| (c.start_tick, c.length_ticks))
        .collect();
    assert_eq!(
        chords,
        vec![(0, 4 * BEAT)],
        "the wrapped chord track was lifted onto the song"
    );
}

// ---------------------------------------------------------------- the starter project

/// Every melody note's tier against the project chord under it, with the starter's locked key.
fn melody_tiers(model: &ProjectModel) -> Vec<Tier> {
    let scale = model.key.scale_pitch_classes();
    model.patterns[0]
        .notes
        .iter()
        .map(|n| {
            let chord = model
                .chords
                .chord_at(n.start_tick)
                .expect("every melody note sits on a chord");
            TierClassifier.tier(n.pitch, &chord.pitch_classes, scale.as_deref())
        })
        .collect()
}

#[test]
fn starter_has_a_melody_a_groove_and_a_locked_key() {
    let m = ProjectModel::starter();
    assert_eq!(m.key, KeyState::locked(0, ScaleType::Major));
    assert_eq!(m.song_length_ticks, 128 * BEAT, "32 bars of 4/4");
    assert!(!m.tracks[0].is_drum && m.tracks[1].is_drum);
    let names: Vec<&str> = m.patterns.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["melody 1", "drums 1"], "one pattern per track");
    let (melody, drums) = (&m.patterns[0], &m.patterns[1]);
    assert_eq!(
        (melody.track_id, drums.track_id),
        (m.tracks[0].id, m.tracks[1].id)
    );
    assert_eq!(melody.length_ticks, 16 * BEAT);
    assert!(!melody.notes.is_empty(), "the melody track plays something");
    let count = |pitch| drums.notes.iter().filter(|n| n.pitch == pitch).count();
    assert_eq!(
        (count(36), count(38), count(42)),
        (8, 8, 32),
        "kick on 1 and 3, snare on 2 and 4, 8th hats, 4 bars"
    );

    // The progression tiles the whole song, and both patterns loop across it from tick 0.
    let starts: Vec<i64> = m.chords.chords().iter().map(|c| c.start_tick).collect();
    assert_eq!(starts, (0..32).map(|i| i * 4 * BEAT).collect::<Vec<_>>());
    assert_eq!(m.chords.chords()[4].name, m.chords.chords()[0].name);
    assert_eq!(m.clips.len(), 2, "one clip per track");
    for c in &m.clips {
        assert_eq!((c.start_tick, c.length_ticks), (0, 128 * BEAT));
    }

    let tiers = melody_tiers(&m);
    assert!(
        tiers.iter().all(|t| *t != Tier::Dissonance),
        "the starter melody is never flagged: {tiers:?}"
    );
    assert!(
        tiers.contains(&Tier::ChordTone) && tiers.contains(&Tier::Tension),
        "it shows more than one tier"
    );
}

#[test]
fn starter_shaped_fits_any_length_and_meter() {
    for (length, bar) in [
        (32 * BEAT, 4 * BEAT),
        (8 * BEAT, 4 * BEAT),
        (12 * BEAT, 3 * BEAT),
        (14 * BEAT, 7 * BEAT / 2),
        (8 * BEAT, 2 * BEAT),
    ] {
        let m = ProjectModel::starter_shaped(length, bar, 8);
        let song = 8 * bar;
        assert_eq!(m.song_length_ticks, song);
        for p in &m.patterns {
            assert_eq!(p.length_ticks, length);
            for n in &p.notes {
                assert!(
                    n.start_tick >= 0 && n.end_tick() <= length,
                    "{length}/{bar}: note past the end: {n:?}"
                );
            }
        }
        assert!(m
            .clips
            .iter()
            .all(|c| c.start_tick == 0 && c.length_ticks == song));
        let first_pass: Vec<&ChordEvent> = m
            .chords
            .chords()
            .iter()
            .filter(|c| c.start_tick < length)
            .collect();
        assert_eq!(
            first_pass.len(),
            4,
            "{length}/{bar}: one pass of the progression per pattern length"
        );
        assert_eq!(
            m.chords.chords().last().unwrap().end_tick(),
            song,
            "{length}/{bar}: chords reach the song end"
        );
        assert!(
            melody_tiers(&m).iter().all(|t| *t != Tier::Dissonance),
            "{length}/{bar}: the melody keeps its tiers"
        );
        let hits = &m.patterns[1].notes;
        let kicks: Vec<i64> = hits
            .iter()
            .filter(|n| n.pitch == 36)
            .map(|n| n.start_tick % bar)
            .collect();
        assert!(
            kicks.contains(&0),
            "{length}/{bar}: a kick on every downbeat"
        );
        assert!(
            hits.iter().any(|n| n.pitch == 38),
            "{length}/{bar}: there is a snare"
        );
    }
    // Nonsense input falls back to the default shape instead of an empty pattern.
    let fallback = ProjectModel::starter_shaped(0, -1, 0);
    assert_eq!(fallback.patterns[0].length_ticks, 16 * BEAT);
    assert_eq!(
        fallback.song_length_ticks,
        4 * BEAT,
        "a zero-bar song is one bar"
    );
}

// ---------------------------------------------------------------- vibes

#[test]
fn every_vibe_builds_a_playable_project() {
    let infos = vibes();
    assert_eq!(infos.len(), 7);
    for (info, spec) in infos.iter().zip(vibe_specs()) {
        let id = &info.id;
        let m =
            ProjectModel::from_vibe(id, DEFAULT_SONG_BARS).unwrap_or_else(|| panic!("{id} builds"));
        assert_eq!(m.tempo, info.tempo, "{id}");
        assert_eq!(m.key, info.key, "{id}: key locked as advertised");
        assert!(m.key.is_locked);
        assert_eq!(m.song_length_ticks, 128 * BEAT);
        assert_eq!(m.tracks.len(), spec.tracks.len(), "{id}");
        assert_eq!(
            m.patterns.len(),
            m.tracks.len(),
            "{id}: one pattern per track"
        );
        for (p, t) in m.patterns.iter().zip(&m.tracks) {
            assert_eq!(p.track_id, t.id);
            assert_eq!(p.name, format!("{} 1", t.name));
            assert_eq!(p.length_ticks, 16 * BEAT);
            for n in &p.notes {
                assert!(
                    n.start_tick >= 0 && n.end_tick() <= 16 * BEAT,
                    "{id}: {n:?} past the end"
                );
            }
        }
        assert_eq!(
            m.clips.len(),
            m.patterns.len(),
            "{id}: every pattern is in the song"
        );
        assert!(m
            .clips
            .iter()
            .all(|c| c.start_tick == 0 && c.length_ticks == 128 * BEAT));
        assert_eq!(
            m.chords.chords().last().unwrap().end_tick(),
            128 * BEAT,
            "{id}: the progression fills the song"
        );
        assert!(
            matches!(spec.tracks[0].role, VibeRole::Lead) && !m.tracks[0].is_drum,
            "{id}: the lead comes first"
        );
        assert!(
            m.patterns[0].notes.is_empty(),
            "{id}: the lead is left for the player"
        );

        let chords = &m.chords;
        for ((t, p), vt) in m.tracks.iter().zip(&m.patterns).zip(&spec.tracks) {
            let name = &t.name;
            if t.is_drum {
                assert!(sound::valid_kit(&t.drum_pads), "{id}/{name}: kit");
                for pad in &t.drum_pads {
                    assert!(
                        factory_sound(&pad.sound).is_some_and(Sound::is_drum),
                        "{id}/{name}: {} is not a factory drum",
                        pad.sound
                    );
                }
                for n in &p.notes {
                    assert!(
                        t.drum_pads.iter().any(|pad| pad.pitch as i32 == n.pitch),
                        "{id}/{name}: no pad for pitch {}",
                        n.pitch
                    );
                }
                assert!(
                    p.notes.iter().any(|n| n.pitch == 36 && n.start_tick == 0),
                    "{id}: a kick on the one"
                );
            } else {
                assert!(
                    factory_sound(&t.voice).is_some_and(|s| !s.is_drum()),
                    "{id}/{name}: {} is not a factory synth",
                    t.voice
                );
            }
            match &vt.role {
                VibeRole::Lead => assert!(p.notes.is_empty()),
                // The first hit of every bar is the chord root, low; the rest sit near it.
                VibeRole::Bass { rhythm } => {
                    assert!(!p.notes.is_empty(), "{id}: bass plays");
                    assert_eq!(
                        rhythm[0].get(2).copied().unwrap_or(0.0),
                        0.0,
                        "{id}: the bar starts on the root"
                    );
                    for n in &p.notes {
                        assert!(
                            (32..=55).contains(&n.pitch),
                            "{id}: bass pitch {} out of range",
                            n.pitch
                        );
                        if n.start_tick % (4 * BEAT) == 0 {
                            let chord = chords.chord_at(n.start_tick).unwrap();
                            assert_eq!(
                                n.pitch.rem_euclid(12),
                                chord.pitch_classes[0].rem_euclid(12),
                                "{id}: bass on the root"
                            );
                        }
                    }
                }
                // Chord parts play only chord tones.
                VibeRole::Chords { .. } => {
                    assert!(!p.notes.is_empty(), "{id}: chords play");
                    for n in &p.notes {
                        let chord = chords.chord_at(n.start_tick).unwrap();
                        assert!(
                            chord
                                .pitch_classes
                                .iter()
                                .any(|pc| pc.rem_euclid(12) == n.pitch.rem_euclid(12)),
                            "{id}: {} not in the chord",
                            n.pitch
                        );
                    }
                }
                // Every lane repeats evenly inside the 16-beat pattern. The lane's step is in
                // beats, as `assets/vibes.json` writes it.
                VibeRole::Drums { lanes, .. } => {
                    for lane in lanes {
                        let period = lane.cells().len() as f64 * lane.step;
                        assert!(
                            period > 0.0 && (16.0 / period).fract() == 0.0,
                            "{id}: lane {} lasts {period} beats",
                            lane.pitch
                        );
                    }
                }
            }
        }
    }
    assert!(ProjectModel::from_vibe("polka", DEFAULT_SONG_BARS).is_none());
    assert_eq!(
        ProjectModel::from_vibe("jungle", 8)
            .unwrap()
            .song_length_ticks,
        32 * BEAT
    );
}

/// The browser mock (ui/src/bridge.ts) builds vibes from the same `assets/vibes.json`;
/// `bridge.test.ts` asserts these same values, so the two builders can't drift apart. Chords
/// are the first pass of the progression (one pattern length); the rest of the song repeats it.
#[test]
fn vibe_values_shared_with_the_ui() {
    let described: Vec<String> = vibes()
        .iter()
        .map(|info| {
            let m = ProjectModel::from_vibe(&info.id, DEFAULT_SONG_BARS).unwrap();
            let names: Vec<&str> = m.tracks.iter().map(|t| t.name.as_str()).collect();
            let chords: Vec<&str> = m
                .chords
                .chords()
                .iter()
                .filter(|c| c.start_tick < 16 * BEAT)
                .map(|c| c.name.as_deref().unwrap_or(""))
                .collect();
            let counts: Vec<String> = m
                .patterns
                .iter()
                .map(|p| p.notes.len().to_string())
                .collect();
            let pitch_sum: i32 = m
                .patterns
                .iter()
                .flat_map(|p| &p.notes)
                .map(|n| n.pitch)
                .sum();
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                info.id,
                info.tempo,
                m.swing,
                names.join(","),
                chords.join(" "),
                m.tracks[0].voice,
                counts.join(","),
                pitch_sum,
            )
        })
        .collect();
    assert_eq!(
        described,
        [
            "jungle|172|50|bells,reese,pad,amen|Fm C# A#m Cm|fm-bell|0,12,12,67|4194",
            "dubstep|140|50|lead,wobble,pad,drums|Dm A# C Am|screamer-lead|0,24,12,62|4195",
            "blownout|138|54|voice,bass,stabs,machine|Em F#dim Em D|robot|0,24,36,90|7149",
            "rage|160|50|synth,808,chords,drums|C#m A B A|rage-lead|0,16,12,72|4273",
            "jersey|140|50|melody,sub,keys,drums|Am F C G|bright-pluck|0,20,36,81|6657",
            "hyperpop|160|50|lead,808,synths,drums|A B C#m E|chip-lead|0,16,60,108|9080",
            "pluggnb|150|50|flute,808,keys,drums|F# Fm A#m G#|whistle-lead|0,8,24,59|4319",
        ]
    );
}

// ---------------------------------------------------------------- insert chains

#[test]
fn insert_chains_round_trip_with_their_order_bypass_and_state() {
    let mut model = ProjectModel::starter();
    let mut delay = Insert::new(
        "clap:/CLAP/ValhallaDelay.clap:com.valhalladsp.delay",
        "Valhalla Delay",
    );
    delay.state = Some("ZGVsYXk=".into());
    let mut eq = Insert::new("clap:/CLAP/Pro-Q.clap:com.fabfilter.proq3", "Pro-Q");
    eq.enabled = false;
    model.tracks[0].inserts = vec![delay, eq.clone()];
    model.master_inserts = vec![eq];

    let decoded = ProjectModel::from_json(&model.to_json().unwrap()).unwrap();
    assert_eq!(decoded, model);
    // Order is meaning, not presentation: the chain must come back in the order it went in.
    assert_eq!(
        decoded.tracks[0]
            .inserts
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        ["Valhalla Delay", "Pro-Q"]
    );
}

#[test]
fn a_project_with_no_inserts_writes_no_insert_keys() {
    // Opening and re-saving a project that has never used an effect must not grow it.
    let json = ProjectModel::starter().to_json().unwrap();
    assert!(
        !json.contains("\"inserts\""),
        "an empty chain was written:\n{json}"
    );
    assert!(
        !json.contains("masterInserts"),
        "an empty master chain was written:\n{json}"
    );
}

#[test]
fn a_file_written_before_inserts_existed_opens_with_empty_chains() {
    let v9 = r#"{
        "schemaVersion": 9,
        "tempo": 110,
        "tracks": [{"id": "11111111-1111-1111-1111-111111111111", "name": "lead", "voice": "saw"}],
        "patterns": [{"id": "22222222-2222-2222-2222-222222222222", "trackId": "11111111-1111-1111-1111-111111111111", "name": "p1", "lengthTicks": 7680}]
    }"#;
    let m = ProjectModel::from_json(v9).unwrap();
    assert!(m.tracks[0].inserts.is_empty());
    assert!(m.master_inserts.is_empty());
}

#[test]
fn loading_drops_unusable_inserts_and_caps_the_chain() {
    // All three are only reachable by hand-editing a file, and all three would otherwise be
    // carried forever: a slot that can never load, two slots the plugin layer cannot tell
    // apart, and a chain deeper than the engine will process.
    let mut m = ProjectModel::starter();
    let dup = Insert::new("clap:/CLAP/Pro-Q.clap:com.fabfilter.proq3", "Pro-Q");
    let mut chain = vec![
        Insert {
            uid: "   ".into(),
            ..Insert::new("x", "nameless")
        },
        dup.clone(),
        dup.clone(),
    ];
    chain.extend((0..MAX_INSERTS).map(|i| Insert::new("clap:/CLAP/x.clap:x", format!("eq {i}"))));
    m.tracks[0].inserts = chain.clone();
    m.master_inserts = chain;

    let loaded = ProjectModel::from_json(&m.to_json().unwrap()).unwrap();
    for inserts in [&loaded.tracks[0].inserts, &loaded.master_inserts] {
        assert_eq!(inserts.len(), MAX_INSERTS, "the chain was not capped");
        assert!(
            inserts.iter().all(|i| !i.uid.trim().is_empty()),
            "a uid-less insert survived"
        );
        assert_eq!(
            inserts.iter().filter(|i| i.id == dup.id).count(),
            1,
            "a duplicated insert id survived"
        );
        assert_eq!(
            inserts[0].name, "Pro-Q",
            "the surviving order was disturbed"
        );
    }
}

#[test]
fn automation_lanes_round_trip() {
    let mut model = ProjectModel::starter();
    let track = model.tracks[0].id;
    let mut fade = AutomationLane::new(AutoTarget::Track(track, TrackParam::Volume));
    fade.points = vec![
        Breakpoint::new(0, 1.0, Curve::Step),
        Breakpoint::new(32 * BEAT, 0.0, Curve::Ramp),
    ];
    let mut sweep = AutomationLane::new(AutoTarget::Track(track, TrackParam::Pan));
    sweep.points = vec![Breakpoint::new(8 * BEAT, -0.75, Curve::Ramp)];
    let mut master = AutomationLane::new(AutoTarget::Master(MasterParam::Gain));
    master.points = vec![Breakpoint::new(0, 0.8, Curve::Step)];
    // Carried but not yet resolved by the engine: a file written here has to come back the
    // same on a build that does play it, or plugin automation would need a second format move.
    let mut plugin = AutomationLane::new(AutoTarget::TrackPlugin(track, 42));
    plugin.points = vec![Breakpoint::new(4 * BEAT, 0.33, Curve::Ramp)];
    let insert_id = uuid::Uuid::new_v4();
    let mut insert = AutomationLane::new(AutoTarget::Insert(InsertTarget::Master, insert_id, 7));
    insert.points = vec![Breakpoint::new(0, 0.5, Curve::Step)];
    model.automation = vec![fade, sweep, master, plugin, insert];

    let decoded = ProjectModel::from_json(&model.to_json().unwrap()).unwrap();
    assert_eq!(decoded, model);
    assert_eq!(decoded.schema_version, SCHEMA_VERSION);
    // The same fade has to come back out of the flattened sequence, not just out of the model.
    let seq = Sequence::from_song(&decoded);
    let volume = seq.tracks[0]
        .automation
        .volume
        .as_ref()
        .expect("the volume lane survived the round trip");
    assert_eq!(volume.value_at(0.0), 1.0);
    assert_eq!(volume.value_at(32.0 * BEAT as f64), 0.0);
    assert_eq!(
        seq.master_automation
            .gain
            .as_ref()
            .map(|c| c.value_at(0.0)),
        Some(0.8)
    );
}

#[test]
fn a_project_with_no_automation_writes_no_automation_key() {
    // The field defaults and skips when empty, which is what keeps it off the schema version:
    // a project that automates nothing is byte-identical to what schema 10 already wrote.
    let model = ProjectModel::starter();
    assert!(!model.to_json().unwrap().contains("automation"));
    assert_eq!(model.schema_version, SCHEMA_VERSION);
}

#[test]
fn loading_tidies_automation_and_drops_orphaned_lanes() {
    let mut m = ProjectModel::starter();
    let track = m.tracks[0].id;
    let gone = uuid::Uuid::new_v4();
    let mut messy = AutomationLane::new(AutoTarget::Track(track, TrackParam::Volume));
    // Out of order, out of range, one negative tick and two points on the same tick: all
    // reachable by hand-editing a file, none of them a function of time.
    messy.points = vec![
        Breakpoint::new(16 * BEAT, 9.0, Curve::Step),
        Breakpoint::new(-4, 0.5, Curve::Step),
        Breakpoint::new(0, 0.25, Curve::Step),
        Breakpoint::new(16 * BEAT, 0.6, Curve::Ramp),
    ];
    let orphan = AutomationLane::new(AutoTarget::Track(gone, TrackParam::Pan));
    let empty = AutomationLane::new(AutoTarget::Track(track, TrackParam::Tone));
    let mut duplicate = AutomationLane::new(AutoTarget::Track(track, TrackParam::Volume));
    duplicate.points = vec![Breakpoint::new(0, 1.0, Curve::Step)];
    m.automation = vec![messy, orphan, empty, duplicate];

    let loaded = ProjectModel::from_json(&m.to_json().unwrap()).unwrap();
    assert_eq!(loaded.automation.len(), 1, "one lane per target should survive");
    let lane = &loaded.automation[0];
    assert_eq!(lane.target, AutoTarget::Track(track, TrackParam::Volume));
    let shape: Vec<(i64, f64, Curve)> = lane
        .points
        .iter()
        .map(|p| (p.tick, p.value, p.curve))
        .collect();
    assert_eq!(
        shape,
        vec![(0, 0.25, Curve::Step), (16 * BEAT, 0.6, Curve::Ramp)]
    );
}

/// A rope and its slide time survive a save, and a project with no ropes writes no rope keys,
/// so files from before ropes existed are byte-for-byte what this version writes back.
#[test]
fn ropes_round_trip_and_are_absent_when_unset() {
    let mut model = ProjectModel::starter();
    let plain = model.to_json().unwrap();
    assert!(!plain.contains("slideFrom") && !plain.contains("slideSecs"), "{plain}");

    let a = NoteEvent::new(0, BEAT, 60);
    let mut b = NoteEvent::new(BEAT, BEAT, 67);
    b.slide_from = Some(a.id);
    b.slide_secs = Some(0.25);
    let mut c = NoteEvent::new(2 * BEAT, BEAT, 64);
    c.slide_from = Some(b.id);
    model.patterns[0].notes = vec![a.clone(), b.clone(), c.clone()];

    let json = model.to_json().unwrap();
    assert!(json.contains(&format!("\"slideFrom\": \"{}\"", a.id)), "{json}");
    assert!(json.contains("\"slideSecs\": 0.25"), "{json}");
    // `c` uses the patch's glide, so only one note writes a time.
    assert_eq!(json.matches("slideSecs").count(), 1);
    let decoded = ProjectModel::from_json(&json).unwrap();
    assert_eq!(decoded, model);
    assert_eq!(decoded.patterns[0].notes[2].slide_from, Some(b.id));
    assert_eq!(decoded.patterns[0].notes[2].slide_secs, None);
}

/// A hand-edited file can name a source that isn't there, loop, or ask for a silly time: the
/// load keeps what makes sense and cuts the rest.
#[test]
fn loading_cuts_bad_ropes_and_clamps_slide_times() {
    let track = "11111111-1111-1111-1111-111111111111";
    let (a, b, c, d) = (
        "a0000000-0000-4000-8000-000000000000",
        "b0000000-0000-4000-8000-000000000000",
        "c0000000-0000-4000-8000-000000000000",
        "d0000000-0000-4000-8000-000000000000",
    );
    let gone = "e0000000-0000-4000-8000-000000000000";
    let json = format!(
        r#"{{
          "schemaVersion": {SCHEMA_VERSION},
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{
            "id": "22222222-2222-2222-2222-222222222222", "trackId": "{track}", "name": "p",
            "notes": [
              {{ "id": "{a}", "startTick": 0, "lengthTicks": 960, "pitch": 60, "slideFrom": "{b}" }},
              {{ "id": "{b}", "startTick": 960, "lengthTicks": 960, "pitch": 62, "slideFrom": "{a}", "slideSecs": 9.0 }},
              {{ "id": "{c}", "startTick": 1920, "lengthTicks": 960, "pitch": 64, "slideFrom": "{gone}", "slideSecs": 0.5 }},
              {{ "id": "{d}", "startTick": 2880, "lengthTicks": 960, "pitch": 65, "slideSecs": -1.0 }}
            ]
          }} ]
        }}"#
    );
    let decoded = ProjectModel::from_json(&json).unwrap();
    let notes = &decoded.patterns[0].notes;
    let by = |id: &str| notes.iter().find(|n| n.id.to_string() == id).unwrap();
    // The loop is cut at its earliest note; the later one keeps its rope and its time, clamped.
    assert_eq!(by(a).slide_from, None);
    assert_eq!(by(b).slide_from, Some(by(a).id));
    assert_eq!(by(b).slide_secs, Some(MAX_SLIDE_SECS));
    // A missing source takes the rope's time with it, and a time with no rope means nothing.
    assert_eq!((by(c).slide_from, by(c).slide_secs), (None, None));
    assert_eq!(by(d).slide_secs, None);
}
