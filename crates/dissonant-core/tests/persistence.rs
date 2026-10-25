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
    assert_eq!(infos.len(), 5);
    for info in &infos {
        let m = ProjectModel::from_vibe(&info.id, DEFAULT_SONG_BARS).unwrap_or_else(|| panic!("{} builds", info.id));
        assert_eq!(m.tempo, info.tempo, "{}", info.id);
        assert_eq!(m.key, info.key, "{}: key locked as advertised", info.id);
        assert!(m.key.is_locked);
        assert_eq!(m.song_length_beats, 128.0);
        assert_eq!(m.patterns.len(), 3, "{}: one pattern per track", info.id);
        for (p, t) in m.patterns.iter().zip(&m.tracks) {
            assert_eq!(p.track_id, t.id);
            assert_eq!(p.name, format!("{} 1", t.name));
            assert_eq!(p.length_beats, 16.0);
            for n in &p.notes {
                assert!(n.start_beat >= 0.0 && n.end_beat() <= 16.0 + 1e-9, "{}: {n:?} past the end", info.id);
            }
        }
        assert_eq!(m.clips.len(), 3, "{}: every pattern is in the song", info.id);
        assert!(m.clips.iter().all(|c| c.start_beat == 0.0 && c.length_beats == 128.0));
        assert!((m.chords.last().unwrap().end_beat() - 128.0).abs() < 1e-9, "{}: the progression fills the song", info.id);
        assert!(m.patterns[0].notes.is_empty(), "{}: the lead is left for the player", info.id);
        // Every bass note is the root of the chord it sits on, low.
        let chords = ChordTrack::new(m.chords.clone());
        let bass = &m.patterns[1].notes;
        assert!(!bass.is_empty());
        for n in bass {
            let chord = chords.chord_at(n.start_beat).unwrap();
            assert_eq!(n.pitch.rem_euclid(12), chord.pitch_classes[0].rem_euclid(12), "{}: bass plays roots", info.id);
            assert!((32..=43).contains(&n.pitch), "{}: bass pitch {} out of range", info.id, n.pitch);
        }
        // A kick on every downbeat, whatever the groove.
        let kicks: Vec<f64> = m.patterns[2].notes.iter().filter(|n| n.pitch == 36).map(|n| n.start_beat).collect();
        for bar in 0..4 {
            assert!(kicks.contains(&(bar as f64 * 4.0)), "{}: kick on bar {bar}", info.id);
        }
    }
    assert!(ProjectModel::from_vibe("polka", DEFAULT_SONG_BARS).is_none());
    assert_eq!(ProjectModel::from_vibe("lofi", 8).unwrap().song_length_beats, 32.0);
}

/// The browser mock (ui/src/bridge.ts) mirrors the vibes; `bridge.test.ts` asserts these same
/// values, so a change on either side fails a test. Chords are the first pass of the
/// progression (one pattern length); the rest of the song repeats it.
#[test]
fn vibe_values_shared_with_the_ui() {
    let described: Vec<String> = vibes()
        .iter()
        .map(|info| {
            let m = ProjectModel::from_vibe(&info.id, DEFAULT_SONG_BARS).unwrap();
            let names: Vec<&str> = m.tracks.iter().map(|t| t.name.as_str()).collect();
            let chords: Vec<&str> =
                m.chords.iter().filter(|c| c.start_beat < 16.0).map(|c| c.name.as_deref().unwrap_or("")).collect();
            let count = |track: usize, pitch: Option<i32>| {
                m.patterns_of(&m.tracks[track].id).flat_map(|p| &p.notes).filter(|n| pitch.is_none_or(|x| n.pitch == x)).count()
            };
            format!(
                "{}|{}|{}|{}|{}|{}|kick {}|bass {}|{}",
                info.id,
                info.tempo,
                m.swing,
                names.join(","),
                chords.join(" "),
                m.tracks[0].voice,
                count(2, Some(36)),
                count(1, None),
                count(2, None),
            )
        })
        .collect();
    assert_eq!(
        described,
        [
            "lofi|80|60|keys,bass,drums|Gm C F Dm|triangle|kick 12|bass 12|52",
            "postpunk|148|50|lead,bass,drums|Em C G D|square|kick 12|bass 32|52",
            "ambient|70|50|pad,bass,drums|D G|pad|kick 4|bass 4|8",
            "songwriter|96|50|melody,bass,drums|G D Em C|pluck|kick 8|bass 8|48",
            "club|124|50|lead,bass,drums|Am G F G|square|kick 16|bass 16|40",
        ]
    );
}
