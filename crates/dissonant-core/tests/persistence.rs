use dissonant_core::*;

#[test]
fn round_trip_preserves_all_state() {
    let mut model = ProjectModel::starter();
    let pid = model.patterns[0].id;
    model.tempo = 96.0;
    model.key = KeyState::locked(0, ScaleType::Major);
    model.tracks[0].soloed = true;
    model.tracks[0].pan = -0.3;
    model.master.reverb_wet = 0.25;
    model.patterns[0].notes = vec![NoteEvent::new(0.0, 1.0, 60).with_velocity(77)];
    model.clips = vec![Clip::new(pid, 0.0, 16.0), Clip::new(pid, 16.0, 8.0)];
    model.sections = vec![Section::new("intro", 0.0)];
    model.song_length_beats = 48.0;

    let json = model.to_json().unwrap();
    let decoded = ProjectModel::from_json(&json).unwrap();
    assert_eq!(decoded, model);
}

#[test]
fn empty_project_defaults() {
    let empty = ProjectModel::empty();
    assert_eq!(empty.tracks.len(), 1);
    assert_eq!(empty.patterns.len(), 1);
    assert_eq!(empty.patterns[0].track_id, empty.tracks[0].id, "the pattern belongs to the track");
    assert_eq!(empty.patterns[0].name, "melody 1");
    assert!(empty.patterns[0].notes.is_empty());
    assert!(empty.chords.is_empty());
    assert_eq!(empty.song_length_beats, DEFAULT_SONG_BARS as f64 * 4.0);
    assert_eq!(empty.tempo, 120.0);
    assert_eq!(empty.key, KeyState::NONE);
}

#[test]
fn decoding_missing_fields_does_not_fail() {
    let decoded = ProjectModel::from_json(r#"{ "schemaVersion": 6, "tempo": 140 }"#).unwrap();
    assert_eq!(decoded.tempo, 140.0);
    assert_eq!(decoded.tracks.len(), 1);
    assert_eq!(decoded.patterns.len(), 1);
    assert_eq!(decoded.schema_version, SCHEMA_VERSION);
    assert_eq!(decoded.song_length_beats, 128.0);

    let decoded = ProjectModel::from_json(r#"{ "schemaVersion": 6 }"#).unwrap();
    assert_eq!(decoded.tempo, 120.0);
    assert_eq!(decoded.master, MasterSettings::default());
}

/// There is no migration: anything from before schema 6 (or without a version, which every
/// schema-6 file has) is refused with the one message the UI shows.
#[test]
fn files_older_than_schema_6_are_refused_with_a_clear_message() {
    let old = r#"{ "schemaVersion": 5, "tracks": [ { "id": "11111111-1111-1111-1111-111111111111", "name": "lead" } ],
                   "patterns": [ { "id": "22222222-2222-2222-2222-222222222222", "name": "p",
                     "notesByTrack": { "11111111-1111-1111-1111-111111111111": [] } } ] }"#;
    for json in [old, r#"{ "schemaVersion": 2 }"#, r#"{ "schemaVersion": 0 }"#, r#"{ "tempo": 120 }"#, "{}"] {
        let err = ProjectModel::from_json(json).expect_err("an old file was opened");
        assert_eq!(err.to_string(), "This project was made with an older version of dissonant and can't be opened.");
        assert_eq!(err.to_string(), OLD_SCHEMA_ERROR);
    }
    assert!(ProjectModel::from_json(r#"{ "schemaVersion": 6 }"#).is_ok());
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
    assert_eq!(decoded.tracks[0].voice, "saw");
    assert_eq!(decoded.tracks[0].tone, 18_000.0);
    assert_eq!(ChordTrack::new(decoded.chords.clone()).chord_at(1.0).unwrap().name.as_deref(), Some("C"));
}

#[test]
fn loading_tidies_chords_song_length_and_clips() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    let json = format!(
        r#"{{
          "schemaVersion": 6,
          "songLengthBeats": 1,
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
    assert_eq!(m.song_length_beats, 4.0, "at least one bar");
    let chords: Vec<(f64, f64)> = m.chords.iter().map(|c| (c.start_beat, c.length_beats)).collect();
    assert_eq!(chords, vec![(0.0, 4.0), (4.0, 4.0)], "sorted, the bad one dropped, no overlap");
    let clips: Vec<(f64, f64)> = m.clips.iter().map(|c| (c.start_beat, c.length_beats)).collect();
    assert_eq!(clips, vec![(2.0, 2.0)], "the clip past the end is gone and the crossing one cut");
}

#[test]
fn embedded_sounds_and_custom_drum_pads_round_trip() {
    let mut model = ProjectModel::starter();
    let synth = model.tracks.iter().position(|t| !t.is_drum).unwrap();
    let drums = model.tracks.iter().position(|t| t.is_drum).unwrap();
    let lead = SynthPatch { level: 0.5, mono: true, glide: 0.25, ..SynthPatch::default() };
    let boom = DrumPatch { tune: -2.0, length: 0.8, ..DrumPatch::default() };
    model.sounds = vec![
        Sound { id: "u-lead".into(), name: "lead".into(), category: "lead".into(), patch: SoundPatch::Synth(lead) },
        Sound { id: "u-boom".into(), name: "boom".into(), category: "kick".into(), patch: SoundPatch::Drum(boom) },
    ];
    model.tracks[synth].voice = "u-lead".into();
    model.tracks[drums].drum_pads = vec![
        DrumPad { pitch: 36, sound: "u-boom".into(), choke: 0 },
        DrumPad { pitch: 42, sound: "hat".into(), choke: 1 },
        DrumPad { pitch: 46, sound: "open-hat".into(), choke: 1 },
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
    assert_eq!(m.tracks[1].drum_pads, default_kit(), "a drum track without drumPads gets the default kit");
    for (pitch, sound) in [(36, "kick"), (38, "snare"), (39, "clap"), (42, "hat")] {
        assert!(m.tracks[1].drum_pads.iter().any(|p| p.pitch == pitch && p.sound == sound), "old groove pitch {pitch} lost its {sound}");
    }
    // The old voice name is a factory id, so it still plays the same patch.
    match &factory_sound("pluck").unwrap().patch {
        SoundPatch::Synth(p) => assert_eq!(m.resolve_synth(&m.tracks[0].voice), *p),
        _ => panic!("factory pluck is not a synth"),
    }
    assert!(!m.to_json().unwrap().contains("\"sounds\""), "re-saving added an empty sounds list");
}

#[test]
fn json_uses_camel_case_keys() {
    let json = ProjectModel::starter().to_json().unwrap();
    for key in ["schemaVersion", "trackId", "lengthBeats", "songLengthBeats", "chords", "notes"] {
        assert!(json.contains(&format!("\"{key}\"")), "missing {key}");
    }
    assert!(!json.contains("notesByTrack"));
    assert!(!json.contains("length_beats"));
    assert!(json.contains("\"schemaVersion\": 6"));
}

// ---------------------------------------------------------------- the starter project

/// Every melody note's tier against the project chord under it, with the starter's locked key.
fn melody_tiers(model: &ProjectModel) -> Vec<Tier> {
    let chords = ChordTrack::new(model.chords.clone());
    let scale = model.key.scale_pitch_classes();
    model.patterns[0]
        .notes
        .iter()
        .map(|n| {
            let chord = chords.chord_at(n.start_beat).expect("every melody note sits on a chord");
            TierClassifier.tier(n.pitch, &chord.pitch_classes, scale.as_deref())
        })
        .collect()
}

#[test]
fn starter_has_a_melody_a_groove_and_a_locked_key() {
    let m = ProjectModel::starter();
    assert_eq!(m.key, KeyState::locked(0, ScaleType::Major));
    assert_eq!(m.song_length_beats, 128.0, "32 bars of 4/4");
    assert!(!m.tracks[0].is_drum && m.tracks[1].is_drum);
    let names: Vec<&str> = m.patterns.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["melody 1", "drums 1"], "one pattern per track");
    let (melody, drums) = (&m.patterns[0], &m.patterns[1]);
    assert_eq!((melody.track_id, drums.track_id), (m.tracks[0].id, m.tracks[1].id));
    assert_eq!(melody.length_beats, 16.0);
    assert!(!melody.notes.is_empty(), "the melody track plays something");
    let count = |pitch| drums.notes.iter().filter(|n| n.pitch == pitch).count();
    assert_eq!((count(36), count(38), count(42)), (8, 8, 32), "kick on 1 and 3, snare on 2 and 4, 8th hats, 4 bars");

    // The progression tiles the whole song, and both patterns loop across it from beat 0.
    let starts: Vec<f64> = m.chords.iter().map(|c| c.start_beat).collect();
    assert_eq!(starts, (0..32).map(|i| i as f64 * 4.0).collect::<Vec<_>>());
    assert_eq!(m.chords[4].name, m.chords[0].name);
    assert_eq!(m.clips.len(), 2, "one clip per track");
    for c in &m.clips {
        assert_eq!((c.start_beat, c.length_beats), (0.0, 128.0));
    }

    let tiers = melody_tiers(&m);
    assert!(tiers.iter().all(|t| *t != Tier::Dissonance), "the starter melody is never flagged: {tiers:?}");
    assert!(tiers.contains(&Tier::ChordTone) && tiers.contains(&Tier::Tension), "it shows more than one tier");
}

#[test]
fn starter_shaped_fits_any_length_and_meter() {
    for (length, bar) in [(32.0, 4.0), (8.0, 4.0), (12.0, 3.0), (14.0, 3.5), (8.0, 2.0)] {
        let m = ProjectModel::starter_shaped(length, bar, 8);
        let song = 8.0 * bar;
        assert_eq!(m.song_length_beats, song);
        for p in &m.patterns {
            assert_eq!(p.length_beats, length);
            for n in &p.notes {
                assert!(n.start_beat >= 0.0 && n.end_beat() <= length + 1e-9, "{length}/{bar}: note past the end: {n:?}");
            }
        }
        assert!(m.clips.iter().all(|c| c.start_beat == 0.0 && c.length_beats == song));
        let first_pass: Vec<&ChordEvent> = m.chords.iter().filter(|c| c.start_beat < length).collect();
        assert_eq!(first_pass.len(), 4, "{length}/{bar}: one pass of the progression per pattern length");
        assert!((m.chords.last().unwrap().end_beat() - song).abs() < 1e-9, "{length}/{bar}: chords reach the song end");
        assert!(melody_tiers(&m).iter().all(|t| *t != Tier::Dissonance), "{length}/{bar}: the melody keeps its tiers");
        let hits = &m.patterns[1].notes;
        let kicks: Vec<f64> = hits.iter().filter(|n| n.pitch == 36).map(|n| n.start_beat % bar).collect();
        assert!(kicks.contains(&0.0), "{length}/{bar}: a kick on every downbeat");
        assert!(hits.iter().any(|n| n.pitch == 38), "{length}/{bar}: there is a snare");
    }
    // Nonsense input falls back to the default shape instead of an empty pattern.
    let fallback = ProjectModel::starter_shaped(0.0, f64::NAN, 0);
    assert_eq!(fallback.patterns[0].length_beats, 16.0);
    assert_eq!(fallback.song_length_beats, 4.0, "a zero-bar song is one bar");
}

// ---------------------------------------------------------------- vibes

#[test]
fn every_vibe_builds_a_playable_project() {
    let infos = vibes();
    assert_eq!(infos.len(), 7);
    for (info, spec) in infos.iter().zip(vibe_specs()) {
        let id = &info.id;
        let m = ProjectModel::from_vibe(id, DEFAULT_SONG_BARS).unwrap_or_else(|| panic!("{id} builds"));
        assert_eq!(m.tempo, info.tempo, "{id}");
        assert_eq!(m.key, info.key, "{id}: key locked as advertised");
        assert!(m.key.is_locked);
        assert_eq!(m.song_length_beats, 128.0);
        assert_eq!(m.tracks.len(), spec.tracks.len(), "{id}");
        assert_eq!(m.patterns.len(), m.tracks.len(), "{id}: one pattern per track");
        for (p, t) in m.patterns.iter().zip(&m.tracks) {
            assert_eq!(p.track_id, t.id);
            assert_eq!(p.name, format!("{} 1", t.name));
            assert_eq!(p.length_beats, 16.0);
            for n in &p.notes {
                assert!(n.start_beat >= 0.0 && n.end_beat() <= 16.0 + 1e-9, "{id}: {n:?} past the end");
            }
        }
        assert_eq!(m.clips.len(), m.patterns.len(), "{id}: every pattern is in the song");
        assert!(m.clips.iter().all(|c| c.start_beat == 0.0 && c.length_beats == 128.0));
        assert!((m.chords.last().unwrap().end_beat() - 128.0).abs() < 1e-9, "{id}: the progression fills the song");
        assert!(matches!(spec.tracks[0].role, VibeRole::Lead) && !m.tracks[0].is_drum, "{id}: the lead comes first");
        assert!(m.patterns[0].notes.is_empty(), "{id}: the lead is left for the player");

        let chords = ChordTrack::new(m.chords.clone());
        for ((t, p), vt) in m.tracks.iter().zip(&m.patterns).zip(&spec.tracks) {
            let name = &t.name;
            if t.is_drum {
                assert!(sound::valid_kit(&t.drum_pads), "{id}/{name}: kit");
                for pad in &t.drum_pads {
                    assert!(factory_sound(&pad.sound).is_some_and(Sound::is_drum), "{id}/{name}: {} is not a factory drum", pad.sound);
                }
                for n in &p.notes {
                    assert!(t.drum_pads.iter().any(|pad| pad.pitch as i32 == n.pitch), "{id}/{name}: no pad for pitch {}", n.pitch);
                }
                assert!(p.notes.iter().any(|n| n.pitch == 36 && n.start_beat == 0.0), "{id}: a kick on the one");
            } else {
                assert!(factory_sound(&t.voice).is_some_and(|s| !s.is_drum()), "{id}/{name}: {} is not a factory synth", t.voice);
            }
            match &vt.role {
                VibeRole::Lead => assert!(p.notes.is_empty()),
                // The first hit of every bar is the chord root, low; the rest sit near it.
                VibeRole::Bass { rhythm } => {
                    assert!(!p.notes.is_empty(), "{id}: bass plays");
                    assert_eq!(rhythm[0].get(2).copied().unwrap_or(0.0), 0.0, "{id}: the bar starts on the root");
                    for n in &p.notes {
                        assert!((32..=55).contains(&n.pitch), "{id}: bass pitch {} out of range", n.pitch);
                        if n.start_beat.fract() == 0.0 && (n.start_beat / 4.0).fract() == 0.0 {
                            let chord = chords.chord_at(n.start_beat).unwrap();
                            assert_eq!(n.pitch.rem_euclid(12), chord.pitch_classes[0].rem_euclid(12), "{id}: bass on the root");
                        }
                    }
                }
                // Chord parts play only chord tones.
                VibeRole::Chords { .. } => {
                    assert!(!p.notes.is_empty(), "{id}: chords play");
                    for n in &p.notes {
                        let chord = chords.chord_at(n.start_beat).unwrap();
                        assert!(chord.pitch_classes.iter().any(|pc| pc.rem_euclid(12) == n.pitch.rem_euclid(12)), "{id}: {} not in the chord", n.pitch);
                    }
                }
                // Every lane repeats evenly inside the 16-beat pattern.
                VibeRole::Drums { lanes, .. } => {
                    for lane in lanes {
                        let period = lane.cells().len() as f64 * lane.step;
                        assert!(period > 0.0 && (16.0 / period).fract() == 0.0, "{id}: lane {} lasts {period} beats", lane.pitch);
                    }
                }
            }
        }
    }
    assert!(ProjectModel::from_vibe("polka", DEFAULT_SONG_BARS).is_none());
    assert_eq!(ProjectModel::from_vibe("jungle", 8).unwrap().song_length_beats, 32.0);
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
            let chords: Vec<&str> =
                m.chords.iter().filter(|c| c.start_beat < 16.0).map(|c| c.name.as_deref().unwrap_or("")).collect();
            let counts: Vec<String> = m.patterns.iter().map(|p| p.notes.len().to_string()).collect();
            let pitch_sum: i32 = m.patterns.iter().flat_map(|p| &p.notes).map(|n| n.pitch).sum();
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
