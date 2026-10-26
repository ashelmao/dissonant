//! Behavioural audit of `Document` (undo/redo, transient gestures, dirty + path tracking),
//! command validation, referential integrity, sequence flattening and persistence.
//!
//! Every assertion here is about an observable contract a user can feel: "undo puts the project
//! back exactly where it was", "a refused edit changes nothing", "what I saved is what I load".

use dissonant_core::document::TrackParam;
use dissonant_core::*;
use uuid::Uuid;

// ---------------------------------------------------------------- fixtures

/// Two tracks (melody + drums), one pattern each (the melody's 16 beats with two notes, the
/// drums' 8 beats and empty), both placed, in the starter's 128-beat song with its chords.
/// Rich enough that every `Command` variant has something to bite on.
fn fixture() -> ProjectModel {
    let mut m = ProjectModel::starter();
    m.patterns[0].notes = vec![
        NoteEvent::new(0.0, 1.0, 60).with_velocity(90),
        NoteEvent::new(4.0, 2.0, 64).with_velocity(70),
    ];
    m.patterns[1].notes.clear(); // the starter's groove
    m.patterns[1].length_beats = 8.0;
    let p0 = m.patterns[0].id;
    let p1 = m.patterns[1].id;
    m.clips = vec![clip_at(p0, 0.0, 16.0), clip_at(p1, 16.0, 8.0)];
    // The melody plays an embedded user sound, so the sound commands have something to bite on.
    m.sounds = vec![user_sound(0.3)];
    m.tracks[0].voice = "u-test".into();
    m
}

fn user_sound(level: f32) -> Sound {
    Sound { id: "u-test".into(), name: "test".into(), category: "lead".into(), patch: SoundPatch::Synth(SynthPatch { level, ..SynthPatch::default() }) }
}

/// A second pattern on the melody track, so the song can switch melodies over the same beat.
fn add_melody_pattern(m: &mut ProjectModel, length: f64, notes: Vec<NoteEvent>) -> Uuid {
    let mut p = SongPattern::new(m.tracks[0].id, "melody 2");
    p.length_beats = length;
    p.notes = notes;
    let id = p.id;
    m.patterns.push(p);
    id
}

/// A clip placing `pattern` on the song timeline.
fn clip_at(pattern: Uuid, start: f64, length: f64) -> Clip {
    Clip { id: Uuid::new_v4(), pattern_id: pattern, start_beat: start, length_beats: length, offset_beats: 0.0, muted: false }
}

fn doc() -> Document {
    Document::new(fixture())
}

/// (melody track, drum track, melody pattern, drum pattern).
fn ids(m: &ProjectModel) -> (Uuid, Uuid, Uuid, Uuid) {
    (m.tracks[0].id, m.tracks[1].id, m.patterns[0].id, m.patterns[1].id)
}

/// Compile-time exhaustiveness guard: if a `Command` variant is added, this stops compiling
/// and whoever added it has to decide how it is covered below.
#[allow(dead_code)]
fn every_variant_is_accounted_for(c: Command) -> &'static str {
    match c {
        Command::SetTempo { .. } => "SetTempo",
        Command::SetKey { .. } => "SetKey",
        Command::AddTrack { .. } => "AddTrack",
        Command::DeleteTrack { .. } => "DeleteTrack",
        Command::RenameTrack { .. } => "RenameTrack",
        Command::SetTrackMuted { .. } => "SetTrackMuted",
        Command::SetTrackSoloed { .. } => "SetTrackSoloed",
        Command::MoveTrack { .. } => "MoveTrack",
        Command::SetTrackVoice { .. } => "SetTrackVoice",
        Command::SetTrackSound { .. } => "SetTrackSound",
        Command::SetDrumPads { .. } => "SetDrumPads",
        Command::UpsertSound { .. } => "UpsertSound",
        Command::SetTrackParam { .. } => "SetTrackParam",
        Command::SetTrackColor { .. } => "SetTrackColor",
        Command::AddPattern { .. } => "AddPattern",
        Command::DuplicatePattern { .. } => "DuplicatePattern",
        Command::DeletePattern { .. } => "DeletePattern",
        Command::RenamePattern { .. } => "RenamePattern",
        Command::SetPatternLength { .. } => "SetPatternLength",
        Command::SetNotes { .. } => "SetNotes",
        Command::SetChords { .. } => "SetChords",
        Command::AddClip { .. } => "AddClip",
        Command::UpdateClip { .. } => "UpdateClip",
        Command::UpdateClips { .. } => "UpdateClips",
        Command::RemoveClip { .. } => "RemoveClip",
        Command::RemoveClips { .. } => "RemoveClips",
        Command::MakeClipUnique { .. } => "MakeClipUnique",
        Command::SetSongLength { .. } => "SetSongLength",
        Command::AddSection { .. } => "AddSection",
        Command::UpdateSection { .. } => "UpdateSection",
        Command::RemoveSection { .. } => "RemoveSection",
        Command::SetMaster { .. } => "SetMaster",
        Command::SetTimeSignature { .. } => "SetTimeSignature",
        Command::SetSwing { .. } => "SetSwing",
        Command::ImportTracks { .. } => "ImportTracks",
        Command::AddTempoPoint { .. } => "AddTempoPoint",
        Command::UpdateTempoPoint { .. } => "UpdateTempoPoint",
        Command::RemoveTempoPoint { .. } => "RemoveTempoPoint",
    }
}

/// One mutating instance of every `Command` variant, bound to the ids of `m`.
fn one_of_every_variant(m: &ProjectModel) -> Vec<Command> {
    let (t0, _t1, p0, p1) = ids(m);
    let (c0, c1) = (m.clips[0].clone(), m.clips[1].clone());
    vec![
        Command::SetTempo { bpm: 90.0 },
        Command::SetKey { key: KeyState::locked(5, ScaleType::Minor) },
        Command::AddTrack { is_drum: false },
        Command::AddTrack { is_drum: true },
        Command::DeleteTrack { id: t0 },
        Command::RenameTrack { id: t0, name: "lead".into() },
        Command::SetTrackMuted { id: t0, muted: true },
        Command::SetTrackSoloed { id: t0, soloed: true },
        Command::MoveTrack { id: t0, up: false },
        Command::SetTrackVoice { id: t0, voice: "pad".into() },
        Command::SetTrackSound { id: t0, sound: factory_sound("sine").unwrap().clone() },
        Command::SetDrumPads { id: _t1, pads: default_kit().into_iter().rev().collect(), sounds: vec![] },
        Command::UpsertSound { sound: user_sound(0.6) },
        Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: 0.5 },
        Command::SetTrackParam { id: t0, param: TrackParam::ReverbSend, value: 0.25 },
        Command::SetTrackParam { id: t0, param: TrackParam::Tone, value: 4_000.0 },
        Command::SetTrackParam { id: t0, param: TrackParam::Pan, value: -0.75 },
        Command::SetTrackColor { id: t0, color: Some("#010203".into()) },
        Command::AddPattern { track_id: t0 },
        Command::DuplicatePattern { id: p0 },
        Command::DeletePattern { id: p1 },
        Command::RenamePattern { id: p0, name: "verse".into() },
        Command::SetPatternLength { id: p0, beats: 32.0 },
        Command::SetNotes { pattern_id: p1, notes: vec![NoteEvent::new(1.0, 1.0, 62)] },
        Command::SetChords { chords: vec![ChordEvent::new(0.0, 4.0, vec![2, 5, 9], Some("Dm".into()))] },
        Command::AddClip { pattern_id: p1, start_beat: 16.0, length_beats: None },
        Command::UpdateClip { clip: Clip { start_beat: 32.0, ..c1.clone() } },
        Command::UpdateClips { clips: vec![Clip { start_beat: 4.0, ..c0.clone() }, Clip { muted: true, ..c1.clone() }] },
        Command::RemoveClip { id: c0.id },
        Command::RemoveClips { ids: vec![c0.id, c1.id] },
        Command::MakeClipUnique { id: c0.id },
        Command::SetSongLength { beats: 20.0 },
        Command::ImportTracks {
            tracks: vec![ImportedTrack { name: "piano".into(), is_drum: false, notes: vec![NoteEvent::new(0.0, 1.0, 60)] }],
            length_beats: 16.0,
        },
        Command::AddSection { name: "chorus".into(), start_beat: 32.0 },
        Command::SetMaster { master: MasterSettings { gain: 0.5, ..MasterSettings::default() } },
        Command::SetTimeSignature { numerator: 3, denominator: 4 },
        Command::SetSwing { swing: 62.0, grid: 0.25 },
        Command::AddTempoPoint { beat: 8.0, bpm: 140.0, ramp: true },
    ]
}

// ---------------------------------------------------------------- 1. undo / redo

#[test]
fn every_command_variant_is_undoable_and_redoable() {
    let count = one_of_every_variant(&fixture()).len();
    for i in 0..count {
        let mut d = doc();
        let command = one_of_every_variant(d.model())[i].clone();
        let label = every_variant_is_accounted_for(command.clone());
        let before = d.model().clone();

        d.apply(command.clone(), false).unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        let after = d.model().clone();
        assert_ne!(after, before, "{label}: command did not change the model; fixture is too weak");
        assert!(d.can_undo(), "{label}: no undo step was recorded");

        assert!(d.undo(), "{label}: undo refused");
        assert_eq!(d.model(), &before, "{label}: undo did not restore the exact prior state");

        assert!(d.can_redo(), "{label}: redo unavailable after undo");
        assert!(d.redo(), "{label}: redo refused");
        assert_eq!(d.model(), &after, "{label}: redo did not re-apply the edit");
    }
}

#[test]
fn a_new_edit_after_undo_clears_the_redo_stack() {
    let mut d = doc();
    d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
    d.apply(Command::SetTempo { bpm: 95.0 }, false).unwrap();
    assert!(d.undo());
    assert!(d.can_redo());

    d.apply(Command::SetTempo { bpm: 100.0 }, false).unwrap();
    assert!(!d.can_redo(), "redo stack survived a new edit");
    assert!(!d.redo());
    assert_eq!(d.model().tempo, 100.0);
}

#[test]
fn undo_cap_drops_the_oldest_entry_without_corrupting_the_stack() {
    const MAX_UNDO: usize = 200;
    let mut d = doc();
    // 250 distinct, individually-undoable edits.
    let edits = MAX_UNDO + 50;
    for i in 0..edits {
        d.apply(Command::SetTempo { bpm: 21.0 + i as f64 }, false).unwrap();
    }
    assert_eq!(d.model().tempo, 21.0 + (edits - 1) as f64);

    // Exactly MAX_UNDO steps are available, and each one lands on the right tempo.
    for i in (0..MAX_UNDO).rev() {
        assert!(d.undo(), "undo #{} refused while the cap should still allow it", MAX_UNDO - i);
        let expected = 21.0 + (edits - 1 - (MAX_UNDO - i)) as f64;
        assert_eq!(d.model().tempo, expected, "undo walked to the wrong snapshot");
    }
    assert!(!d.undo(), "undo stack exceeded the {MAX_UNDO} cap");
    assert!(!d.can_undo());

    // The surviving history is intact: redo walks all the way forward again.
    for _ in 0..MAX_UNDO {
        assert!(d.redo(), "redo lost an entry the undo cap should have kept");
    }
    assert_eq!(d.model().tempo, 21.0 + (edits - 1) as f64);
    assert!(!d.redo());
}

#[test]
fn undo_and_redo_on_an_empty_stack_are_safe_no_ops() {
    let mut d = doc();
    let before = d.model().clone();
    assert!(!d.can_undo());
    assert!(!d.can_redo());
    assert!(!d.undo());
    assert!(!d.redo());
    assert!(!d.undo());
    assert_eq!(d.model(), &before);
    assert!(!d.is_dirty(), "a failed undo marked the document dirty");
}

#[test]
fn a_command_that_changes_nothing_records_no_undo_step() {
    let mut d = doc();
    let (t0, _, p0, _) = ids(d.model());
    let before = d.model().clone();

    d.apply(Command::SetTempo { bpm: before.tempo }, false).unwrap();
    d.apply(Command::RenameTrack { id: t0, name: before.tracks[0].name.clone() }, false).unwrap();
    d.apply(Command::MoveTrack { id: t0, up: true }, false).unwrap(); // already at the top
    if let Some(clip) = before.clips.first() {
        d.apply(Command::UpdateClip { clip: clip.clone() }, false).unwrap();
    }
    d.apply(Command::SetNotes { pattern_id: p0, notes: before.patterns[0].notes.clone() }, false).unwrap();
    d.apply(Command::SetChords { chords: before.chords.clone() }, false).unwrap();
    d.apply(Command::SetSongLength { beats: before.song_length_beats }, false).unwrap();
    d.apply(Command::RemoveClips { ids: vec![Uuid::new_v4()] }, false).unwrap();

    assert_eq!(d.model(), &before);
    assert!(!d.can_undo(), "a no-op edit polluted the undo stack");
    assert!(!d.is_dirty(), "a no-op edit marked the document dirty");
}

#[test]
fn replace_clears_all_history_and_sets_the_path() {
    let mut d = doc();
    d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
    assert!(d.undo());
    assert!(d.can_redo());

    let path = std::path::PathBuf::from("/tmp/song.dissonant");
    let opened = ProjectModel::empty();
    d.replace(opened.clone(), Some(path.clone()));
    assert!(!d.can_undo());
    assert!(!d.can_redo());
    assert!(!d.is_dirty());
    assert_eq!(d.path.as_deref(), Some(path.as_path()));
    assert_eq!(d.model(), &opened);
}

// ---------------------------------------------------------------- 2. transient gestures

#[test]
fn a_committed_transient_run_is_exactly_one_undo_step() {
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let before = d.model().clone();

    for v in [0.95, 0.9, 0.85, 0.8, 0.75, 0.7] {
        d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: v }, true).unwrap();
    }
    d.commit_gesture();
    assert_eq!(d.model().tracks[0].volume, 0.7);

    assert!(d.undo());
    assert_eq!(d.model(), &before, "the gesture did not rewind to the pre-gesture state");
    assert!(!d.undo(), "the gesture produced more than one undo step");

    assert!(d.redo());
    assert_eq!(d.model().tracks[0].volume, 0.7, "redo did not re-apply the whole gesture");
    assert!(!d.redo());
}

#[test]
fn an_uncommitted_transient_run_is_still_undone_as_one_step() {
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let before = d.model().clone();

    for v in [-0.2, -0.4, -0.6] {
        d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Pan, value: v }, true).unwrap();
    }
    // No commit_gesture() — the user let go of the mouse outside the window, say.
    assert!(d.can_undo());
    assert!(d.undo());
    assert_eq!(d.model(), &before);
    assert!(!d.undo(), "an uncommitted gesture left extra entries on the undo stack");
}

#[test]
fn a_non_transient_command_mid_gesture_closes_the_gesture_first() {
    // Documented behaviour: the gesture so far becomes one undo step, and the interleaved
    // command becomes a second one. Undoing twice walks back through both.
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let before = d.model().clone();

    d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Pan, value: -0.3 }, true).unwrap();
    d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Pan, value: -0.6 }, true).unwrap();
    d.apply(Command::SetTempo { bpm: 101.0 }, false).unwrap();
    d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Pan, value: -0.9 }, true).unwrap();
    d.commit_gesture();

    assert_eq!(d.model().tracks[0].pan, -0.9);
    assert_eq!(d.model().tempo, 101.0);

    assert!(d.undo()); // second gesture
    assert_eq!(d.model().tracks[0].pan, -0.6);
    assert_eq!(d.model().tempo, 101.0);
    assert!(d.undo()); // tempo
    assert_eq!(d.model().tempo, before.tempo);
    assert_eq!(d.model().tracks[0].pan, -0.6);
    assert!(d.undo()); // first gesture
    assert_eq!(d.model(), &before);
    assert!(!d.undo());
}

#[test]
fn commit_gesture_without_a_gesture_is_a_no_op() {
    let mut d = doc();
    d.commit_gesture();
    d.commit_gesture();
    assert!(!d.can_undo());
    assert!(!d.is_dirty());

    d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
    d.commit_gesture();
    d.commit_gesture();
    assert!(d.undo());
    assert!(!d.undo(), "commit_gesture duplicated a committed edit");
}

#[test]
fn a_gesture_that_returns_to_its_starting_value_records_no_undo_step() {
    // Drag a slider away and back again: the project is unchanged, so there is nothing to undo.
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let before = d.model().clone();

    for v in [0.8, 0.6, 1.0] {
        d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: v }, true).unwrap();
    }
    d.commit_gesture();
    assert_eq!(d.model(), &before);
    assert!(!d.can_undo(), "a round-trip gesture left an undo step behind");
}

#[test]
fn can_undo_only_promises_an_undo_that_will_happen() {
    // The Undo menu item is enabled from `can_undo()`. A transient gesture that has landed back
    // on its starting value has nothing to undo, so it must not light the menu item up.
    let mut d = doc();
    let (t0, ..) = ids(d.model());

    for v in [0.8, 0.6, 1.0] {
        d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: v }, true).unwrap();
    }
    // Gesture is still open (never committed) and has returned to its starting value.
    assert!(!d.can_undo(), "Undo was offered for a gesture that changed nothing");
    assert!(!d.undo());

    // An open gesture that *has* moved is undoable, and the promise is kept.
    d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: 0.4 }, true).unwrap();
    assert!(d.can_undo());
    assert!(d.undo(), "can_undo() promised an undo that undo() refused");
    assert_eq!(d.model().tracks[0].volume, 1.0);
}

// ---------------------------------------------------------------- 3. validation

#[test]
fn the_last_track_is_protected_but_a_project_may_have_no_patterns() {
    let mut d = Document::new(ProjectModel::empty());
    let (t0, p0) = (d.model().tracks[0].id, d.model().patterns[0].id);
    assert_eq!(d.apply(Command::DeleteTrack { id: t0 }, false), Err(EditError::LastTrack));
    assert_eq!(d.model().tracks.len(), 1);
    d.apply(Command::DeletePattern { id: p0 }, false).unwrap();
    assert!(d.model().patterns.is_empty());
    // Nothing to play in pattern mode, but the song still plays (silence for its length).
    assert!(Sequence::from_pattern(d.model(), &p0, 0.0).is_none());
    assert_eq!(Sequence::from_song(d.model()).length_beats, d.model().song_length_beats);
    // And the track can grow a new one.
    d.apply(Command::AddPattern { track_id: t0 }, false).unwrap();
    assert_eq!(d.model().patterns[0].name, "melody 1");
}

#[test]
fn unknown_ids_are_rejected() {
    let mut d = doc();
    let (_, _, p0, _) = ids(d.model());
    let ghost = Uuid::new_v4();

    use EditError::*;
    assert_eq!(d.apply(Command::DeleteTrack { id: ghost }, false), Err(NoSuchTrack));
    assert_eq!(d.apply(Command::RenameTrack { id: ghost, name: "x".into() }, false), Err(NoSuchTrack));
    assert_eq!(d.apply(Command::SetTrackMuted { id: ghost, muted: true }, false), Err(NoSuchTrack));
    assert_eq!(d.apply(Command::SetTrackSoloed { id: ghost, soloed: true }, false), Err(NoSuchTrack));
    assert_eq!(d.apply(Command::MoveTrack { id: ghost, up: true }, false), Err(NoSuchTrack));
    assert_eq!(d.apply(Command::SetTrackVoice { id: ghost, voice: "pad".into() }, false), Err(NoSuchTrack));
    assert_eq!(
        d.apply(Command::SetTrackParam { id: ghost, param: TrackParam::Pan, value: 0.5 }, false),
        Err(NoSuchTrack)
    );
    assert_eq!(d.apply(Command::SetTrackColor { id: ghost, color: None }, false), Err(NoSuchTrack));

    assert_eq!(d.apply(Command::DeletePattern { id: ghost }, false), Err(NoSuchPattern));
    assert_eq!(d.apply(Command::DuplicatePattern { id: ghost }, false), Err(NoSuchPattern));
    assert_eq!(d.apply(Command::RenamePattern { id: ghost, name: "x".into() }, false), Err(NoSuchPattern));
    assert_eq!(d.apply(Command::SetPatternLength { id: ghost, beats: 8.0 }, false), Err(NoSuchPattern));
    assert_eq!(d.apply(Command::SetNotes { pattern_id: ghost, notes: vec![] }, false), Err(NoSuchPattern));
    assert_eq!(d.apply(Command::AddPattern { track_id: ghost }, false), Err(NoSuchTrack));
    assert_eq!(
        d.apply(Command::AddClip { pattern_id: ghost, start_beat: 0.0, length_beats: None }, false),
        Err(NoSuchPattern)
    );

    let stray = Clip::new(p0, 0.0, 4.0);
    assert_eq!(d.apply(Command::UpdateClip { clip: stray.clone() }, false), Err(NoSuchClip));
    assert_eq!(d.apply(Command::UpdateClips { clips: vec![stray] }, false), Err(NoSuchClip));
    assert_eq!(d.apply(Command::RemoveClip { id: ghost }, false), Err(NoSuchClip));
    assert_eq!(d.apply(Command::MakeClipUnique { id: ghost }, false), Err(NoSuchClip));
}

#[test]
fn non_finite_and_out_of_range_values_are_rejected() {
    let mut d = doc();
    let (t0, _, p0, _) = ids(d.model());
    use EditError::InvalidValue;

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(d.apply(Command::SetTempo { bpm: bad }, false), Err(InvalidValue));
        assert_eq!(
            d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: bad }, false),
            Err(InvalidValue)
        );
        assert_eq!(d.apply(Command::SetPatternLength { id: p0, beats: bad }, false), Err(InvalidValue));
        assert_eq!(d.apply(Command::SetSongLength { beats: bad }, false), Err(InvalidValue));
        assert_eq!(
            d.apply(Command::AddClip { pattern_id: p0, start_beat: bad, length_beats: None }, false),
            Err(InvalidValue)
        );
        assert_eq!(
            d.apply(
                Command::SetMaster { master: MasterSettings { gain: bad, ..MasterSettings::default() } },
                false
            ),
            Err(InvalidValue)
        );
    }

    // A pattern shorter than one beat is not a pattern.
    assert_eq!(d.apply(Command::SetPatternLength { id: p0, beats: 0.0 }, false), Err(InvalidValue));
    assert_eq!(d.apply(Command::SetPatternLength { id: p0, beats: -4.0 }, false), Err(InvalidValue));
    assert_eq!(d.apply(Command::SetPatternLength { id: p0, beats: 0.999 }, false), Err(InvalidValue));

    // Colors must be `#rrggbb`.
    for bad in ["red", "#fff", "#ggghhh", "", "#1234567"] {
        assert_eq!(
            d.apply(Command::SetTrackColor { id: t0, color: Some(bad.into()) }, false),
            Err(InvalidValue),
            "color {bad:?} was accepted"
        );
    }
}

#[test]
fn blank_pattern_names_are_rejected() {
    let mut d = doc();
    let (_, _, p0, _) = ids(d.model());
    for blank in ["", "   ", "\t\n"] {
        assert_eq!(
            d.apply(Command::RenamePattern { id: p0, name: blank.into() }, false),
            Err(EditError::InvalidValue),
            "pattern accepted the blank name {blank:?}"
        );
    }
    assert_eq!(d.model().patterns[0].name, "melody 1");
}

#[test]
fn blank_track_names_are_rejected() {
    // Same contract as RenamePattern: clearing the name field must not leave an unlabelled,
    // unidentifiable track header behind (nor cost an undo step).
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let original = d.model().tracks[0].name.clone();
    for blank in ["", "   ", "\t\n"] {
        assert_eq!(
            d.apply(Command::RenameTrack { id: t0, name: blank.into() }, false),
            Err(EditError::InvalidValue),
            "track accepted the blank name {blank:?}"
        );
        assert_eq!(d.model().tracks[0].name, original);
    }
    assert!(!d.can_undo(), "a refused rename recorded an undo step");
    // A real name still goes through, including one with surrounding whitespace.
    d.apply(Command::RenameTrack { id: t0, name: " lead ".into() }, false).unwrap();
    assert_eq!(d.model().tracks[0].name, " lead ");
}

#[test]
fn out_of_range_values_that_are_accepted_are_clamped_not_stored_raw() {
    let mut d = doc();
    let (t0, ..) = ids(d.model());

    d.apply(Command::SetTempo { bpm: -5.0 }, false).unwrap();
    assert_eq!(d.model().tempo, Tempo::MIN_BPM);
    d.apply(Command::SetTempo { bpm: 10_000.0 }, false).unwrap();
    assert_eq!(d.model().tempo, Tempo::MAX_BPM);

    let clamps = [
        (TrackParam::Volume, 99.0, 1.5),
        (TrackParam::Volume, -99.0, 0.0),
        (TrackParam::ReverbSend, 99.0, 1.0),
        (TrackParam::ReverbSend, -1.0, 0.0),
        (TrackParam::Tone, 1.0, 200.0),
        (TrackParam::Tone, 1e9, 20_000.0),
        (TrackParam::Pan, -9.0, -1.0),
        (TrackParam::Pan, 9.0, 1.0),
    ];
    for (param, value, expected) in clamps {
        d.apply(Command::SetTrackParam { id: t0, param, value }, false).unwrap();
        let t = &d.model().tracks[0];
        let got = match param {
            TrackParam::Volume => t.volume,
            TrackParam::ReverbSend => t.reverb_send,
            TrackParam::Tone => t.tone,
            TrackParam::Pan => t.pan,
        };
        assert_eq!(got, expected, "{param:?} {value} was not clamped");
    }

    d.apply(
        Command::SetMaster {
            master: MasterSettings {
                gain: 99.0,
                reverb_wet: 99.0,
                low_cut_hz: 0.0,
                high_cut_hz: 1e9,
                low_eq: -1.0,
                mid_eq: 99.0,
                high_eq: -1.0,
            },
        },
        false,
    )
    .unwrap();
    let m = &d.model().master;
    assert_eq!((m.gain, m.reverb_wet), (1.5, 1.0));
    assert_eq!((m.low_cut_hz, m.high_cut_hz), (10.0, 20_000.0));
    assert_eq!((m.low_eq, m.mid_eq, m.high_eq), (0.0, 2.0, 0.0));
}

#[test]
fn a_refused_command_mutates_nothing_and_records_no_undo_step() {
    let mut d = doc();
    let (t0, _, p0, _) = ids(d.model());
    let ghost = Uuid::new_v4();

    // Establish one real edit so there is a history to corrupt.
    d.apply(Command::SetTempo { bpm: 140.0 }, false).unwrap();
    assert!(d.undo());
    let baseline = d.model().clone();
    assert!(d.can_redo());
    d.mark_saved();

    let refusals: Vec<Command> = vec![
        Command::SetTempo { bpm: f64::NAN },
        Command::DeleteTrack { id: ghost },
        Command::RenameTrack { id: ghost, name: "x".into() },
        Command::RenamePattern { id: p0, name: "   ".into() },
        Command::SetPatternLength { id: p0, beats: 0.5 },
        Command::SetTrackColor { id: t0, color: Some("nope".into()) },
        Command::SetTrackParam { id: t0, param: TrackParam::Tone, value: f64::NAN },
        Command::DuplicatePattern { id: ghost },
        Command::SetNotes { pattern_id: ghost, notes: vec![NoteEvent::new(0.0, 1.0, 60)] },
        Command::SetMaster { master: MasterSettings { high_eq: f64::NAN, ..MasterSettings::default() } },
        Command::AddClip { pattern_id: p0, start_beat: 500.0, length_beats: None },
        Command::UpdateClips { clips: vec![Clip { start_beat: 4.0, ..baseline.clips[0].clone() }, Clip::new(p0, 0.0, 4.0)] },
        Command::SetSongLength { beats: f64::INFINITY },
        Command::ImportTracks { tracks: vec![], length_beats: 16.0 },
    ];
    for command in refusals {
        assert!(d.apply(command.clone(), false).is_err(), "{command:?} was expected to be refused");
        assert_eq!(d.model(), &baseline, "{command:?} mutated the model despite being refused");
        assert!(d.can_redo(), "{command:?} cleared the redo stack despite being refused");
        assert!(!d.is_dirty(), "{command:?} marked the document dirty despite being refused");
    }
    assert!(!d.can_undo(), "refused commands pushed undo entries");
}

#[test]
fn a_refused_transient_command_does_not_open_a_gesture() {
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    assert!(d
        .apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: f64::NAN }, true)
        .is_err());
    assert!(!d.can_undo());
    d.commit_gesture();
    assert!(!d.can_undo());
}

// ---------------------------------------------------------------- 4. referential integrity

#[test]
fn deleting_a_track_purges_its_patterns_and_their_clips() {
    let mut d = doc();
    let (t0, _, p0, p1) = ids(d.model());
    d.apply(Command::AddPattern { track_id: t0 }, false).unwrap();
    let second = d.model().patterns_of(&t0).nth(1).unwrap().id;
    d.apply(Command::AddClip { pattern_id: second, start_beat: 32.0, length_beats: None }, false).unwrap();
    d.apply(Command::SetNotes { pattern_id: p1, notes: vec![NoteEvent::new(0.0, 0.25, 36)] }, false).unwrap();
    let before = d.model().clone();

    d.apply(Command::DeleteTrack { id: t0 }, false).unwrap();
    let m = d.model();
    assert!(m.patterns.iter().all(|p| p.track_id != t0), "a pattern of the deleted track survived");
    assert!(m.pattern(&p0).is_none() && m.pattern(&second).is_none());
    assert!(m.clips.iter().all(|c| c.pattern_id == p1), "a clip of the deleted track survived");
    assert_eq!(m.clips.len(), 1);
    assert_eq!(m.pattern(&p1).unwrap().notes.len(), 1, "the surviving track lost its notes");

    // And undo brings every pattern and clip back.
    assert!(d.undo());
    assert_eq!(d.model(), &before);
}

// ---------------------------------------------------------------- sounds and drum kits

fn drum_sound(id: &str) -> Sound {
    Sound { id: id.into(), name: id.into(), category: "kick".into(), patch: SoundPatch::Drum(DrumPatch::default()) }
}

fn pad(pitch: u8, sound: &str, choke: u8) -> DrumPad {
    DrumPad { pitch, sound: sound.into(), choke }
}

fn embedded(m: &ProjectModel) -> Vec<&str> {
    m.sounds.iter().map(|s| s.id.as_str()).collect()
}

/// `command` is refused with exactly `error`, and the document is left as it was.
fn refused(d: &mut Document, command: Command, error: EditError) {
    let before = d.model().clone();
    let could_undo = d.can_undo();
    assert_eq!(d.apply(command.clone(), false), Err(error), "{command:?}");
    assert_eq!(d.model(), &before, "{command:?} mutated the model despite being refused");
    assert_eq!(d.can_undo(), could_undo, "{command:?} pushed an undo entry despite being refused");
}

#[test]
fn set_track_sound_embeds_a_user_sound_and_a_factory_sound_prunes_it() {
    let mut d = doc();
    let (t0, t1, ..) = ids(d.model());
    let start = d.model().clone();

    // A new user sound replaces the old one on the track; the old one, now unused, is dropped.
    // The embedded copy is sanitized like everything else that enters the model.
    let other = Sound { id: "u-other".into(), name: "  ".into(), ..user_sound(9.0) };
    d.apply(Command::SetTrackSound { id: t0, sound: other.clone() }, false).unwrap();
    assert_eq!(d.model().tracks[0].voice, "u-other");
    assert_eq!(embedded(d.model()), ["u-other"], "the unused u-test was not pruned");
    assert_eq!(d.model().sounds[0].name, "untitled");
    assert_eq!(d.model().resolve_synth("u-other").level, 1.0, "the embedded copy was stored unclamped");

    // Sending the same id again replaces the copy in place.
    let quieter = Sound { patch: SoundPatch::Synth(SynthPatch { level: 0.5, ..SynthPatch::default() }), ..other };
    d.apply(Command::SetTrackSound { id: t0, sound: quieter }, false).unwrap();
    assert_eq!(embedded(d.model()), ["u-other"]);
    assert_eq!(d.model().resolve_synth("u-other").level, 0.5);

    // Back to a factory sound: nothing is embedded, and the file carries no `sounds` key.
    d.apply(Command::SetTrackSound { id: t0, sound: factory_sound("saw").unwrap().clone() }, false).unwrap();
    assert_eq!(d.model().tracks[0].voice, "saw");
    assert!(d.model().sounds.is_empty(), "a factory sound was embedded or a user sound kept");
    assert!(!d.model().to_json().unwrap().contains("\"sounds\""), "an empty sounds list was written");

    // Wrong kind either way round, and an unknown track, change nothing.
    refused(&mut d, Command::SetTrackSound { id: t1, sound: user_sound(0.5) }, EditError::InvalidValue);
    refused(&mut d, Command::SetTrackSound { id: t0, sound: drum_sound("u-boom") }, EditError::InvalidValue);
    refused(&mut d, Command::SetTrackSound { id: Uuid::new_v4(), sound: user_sound(0.5) }, EditError::NoSuchTrack);

    // Each change was one undo step, and undo brings the embedded sounds back.
    for _ in 0..3 {
        assert!(d.undo());
    }
    assert_eq!(d.model(), &start);
}

#[test]
fn set_drum_pads_refuses_a_bad_kit_and_changes_nothing() {
    let mut d = doc();
    let (t0, t1, ..) = ids(d.model());
    let seventeen: Vec<DrumPad> = (0..=MAX_DRUM_PADS as u8).map(|i| pad(30 + i, "kick", 0)).collect();
    assert_eq!(seventeen.len(), 17);
    let kit = |pads: Vec<DrumPad>| Command::SetDrumPads { id: t1, pads, sounds: vec![] };
    for (command, error) in [
        (kit(vec![]), EditError::InvalidValue),
        (kit(seventeen.clone()), EditError::InvalidValue),
        (kit(vec![pad(36, "kick", 0), pad(36, "snare", 0)]), EditError::InvalidValue),
        (kit(vec![pad(128, "kick", 0)]), EditError::InvalidValue),
        // A synth sound carried along to embed, a pad on a factory synth sound, and a pad on the
        // project's embedded synth sound.
        (Command::SetDrumPads { id: t1, pads: vec![pad(36, "u-test", 0)], sounds: vec![user_sound(0.5)] }, EditError::InvalidValue),
        (kit(vec![pad(36, "saw", 0)]), EditError::NoSuchSound),
        (kit(vec![pad(36, "u-test", 0)]), EditError::NoSuchSound),
        // A kit on a synth track, and on no track at all.
        (Command::SetDrumPads { id: t0, pads: default_kit(), sounds: vec![] }, EditError::InvalidValue),
        (Command::SetDrumPads { id: Uuid::new_v4(), pads: default_kit(), sounds: vec![] }, EditError::NoSuchTrack),
    ] {
        refused(&mut d, command, error);
    }
    assert_eq!(d.model().tracks[1].drum_pads, default_kit());
    assert!(!d.can_undo());

    // Sixteen is the limit, not one past it.
    d.apply(kit(seventeen[..MAX_DRUM_PADS].to_vec()), false).unwrap();
    assert_eq!(d.model().tracks[1].drum_pads.len(), MAX_DRUM_PADS);
}

#[test]
fn a_pad_on_an_unknown_sound_is_refused_and_embeds_nothing() {
    let mut d = doc();
    let (_, t1, ..) = ids(d.model());
    // The carried sound would be embedded on the way, but the refusal throws the whole edit away.
    refused(
        &mut d,
        Command::SetDrumPads { id: t1, pads: vec![pad(36, "u-boom", 0), pad(38, "u-missing", 0)], sounds: vec![drum_sound("u-boom")] },
        EditError::NoSuchSound,
    );
    assert_eq!(embedded(d.model()), ["u-test"]);
}

#[test]
fn drum_pads_embed_their_user_sounds_and_deleting_a_track_prunes_its_sounds() {
    let mut d = doc();
    let (t0, t1, ..) = ids(d.model());
    let pads = vec![pad(36, "u-boom", 0), pad(42, "hat", 1), pad(46, "open-hat", 1)];
    let sounds = vec![drum_sound("u-boom"), drum_sound("u-unused"), factory_sound("hat").unwrap().clone()];
    d.apply(Command::SetDrumPads { id: t1, pads: pads.clone(), sounds }, false).unwrap();
    assert_eq!(d.model().tracks[1].drum_pads, pads);
    // Only the user sound a pad uses is embedded: not the unused one, not the factory one.
    assert_eq!(embedded(d.model()), ["u-test", "u-boom"]);
    assert_eq!(d.model().resolve_drum("u-boom"), DrumPatch::default());

    // Deleting a track drops the sounds only it used.
    d.apply(Command::DeleteTrack { id: t1 }, false).unwrap();
    assert_eq!(embedded(d.model()), ["u-test"]);
    assert!(d.undo());
    d.apply(Command::DeleteTrack { id: t0 }, false).unwrap();
    assert_eq!(embedded(d.model()), ["u-boom"]);
    assert!(d.undo());

    // A sound another track still plays survives.
    d.apply(Command::AddTrack { is_drum: false }, false).unwrap();
    let t2 = d.model().tracks[2].id;
    d.apply(Command::SetTrackVoice { id: t2, voice: "u-test".into() }, false).unwrap();
    d.apply(Command::DeleteTrack { id: t0 }, false).unwrap();
    assert_eq!(embedded(d.model()), ["u-test", "u-boom"]);
}

#[test]
fn upsert_sound_only_replaces_a_copy_the_project_embeds() {
    let mut d = doc();
    refused(&mut d, Command::UpsertSound { sound: Sound { id: "u-nope".into(), ..user_sound(0.5) } }, EditError::NoSuchSound);
    refused(&mut d, Command::UpsertSound { sound: factory_sound("saw").unwrap().clone() }, EditError::NoSuchSound);
    // The embedded u-test is a synth; a drum patch under its id is the wrong kind.
    refused(&mut d, Command::UpsertSound { sound: drum_sound("u-test") }, EditError::InvalidValue);

    d.apply(Command::UpsertSound { sound: user_sound(0.6) }, false).unwrap();
    assert_eq!(embedded(d.model()), ["u-test"]);
    assert_eq!(d.model().resolve_synth("u-test").level, 0.6);
}

#[test]
fn deleting_a_pattern_purges_every_clip_of_it_from_the_song() {
    let mut d = doc();
    let (.., p0, p1) = ids(d.model());
    // Start from a clean song: the fixture already places one clip of each pattern.
    for id in d.model().clips.iter().map(|c| c.id).collect::<Vec<_>>() {
        d.apply(Command::RemoveClip { id }, false).unwrap();
    }
    for (pattern_id, start) in [(p0, 0.0), (p1, 16.0), (p0, 32.0), (p0, 48.0), (p1, 64.0)] {
        d.apply(Command::AddClip { pattern_id, start_beat: start, length_beats: None }, false).unwrap();
    }
    d.apply(Command::DeletePattern { id: p0 }, false).unwrap();
    assert_eq!(
        d.model().clips.iter().map(|c| c.pattern_id).collect::<Vec<_>>(),
        vec![p1, p1],
        "clips of the deleted pattern survived"
    );
    assert!(d.model().pattern(&p0).is_none());
}

#[test]
fn duplicating_a_pattern_deep_copies_its_notes_on_the_same_track_with_fresh_ids() {
    let mut d = doc();
    let (t0, _, p0, _) = ids(d.model());
    d.apply(Command::DuplicatePattern { id: p0 }, false).unwrap();

    let original = d.model().patterns[0].clone();
    let copy = d.model().patterns[1].clone();
    assert_ne!(original.id, copy.id);
    assert_eq!(copy.track_id, t0, "the copy belongs to the same track");
    assert_eq!(copy.name, format!("{} copy", original.name));
    assert_eq!(copy.length_beats, original.length_beats);

    assert_eq!(original.notes.len(), 2, "fixture should have two melody notes");
    assert_eq!(copy.notes.len(), original.notes.len(), "the copy lost notes");
    for n in &copy.notes {
        assert!(!original.notes.iter().any(|o| o.id == n.id), "copy shares note id {} with the original", n.id);
    }
    // Musical content is identical even though the ids are not.
    let musical = |p: &SongPattern| p.notes.iter().map(|n| (n.start_beat, n.pitch, n.velocity)).collect::<Vec<_>>();
    assert_eq!(musical(&original), musical(&copy), "duplicated notes differ musically");

    // Editing the copy must not reach back into the original.
    d.apply(Command::SetNotes { pattern_id: copy.id, notes: vec![] }, false).unwrap();
    assert_eq!(d.model().patterns[0].notes.len(), 2, "editing the copy mutated the original");
}

#[test]
fn the_song_only_ever_holds_clips_of_live_patterns() {
    let mut d = doc();
    let (.., p0, p1) = ids(d.model());
    d.apply(Command::AddClip { pattern_id: p0, start_beat: 0.0, length_beats: None }, false).unwrap();
    d.apply(Command::AddClip { pattern_id: p1, start_beat: 16.0, length_beats: None }, false).unwrap();
    // A clip can only be placed for a pattern that exists.
    assert_eq!(
        d.apply(Command::AddClip { pattern_id: Uuid::new_v4(), start_beat: 32.0, length_beats: None }, false),
        Err(EditError::NoSuchPattern)
    );

    let live: Vec<Uuid> = d.model().patterns.iter().map(|p| p.id).collect();
    for clip in &d.model().clips {
        assert!(live.contains(&clip.pattern_id), "clip points at a pattern that is gone");
    }
}

// ---------------------------------------------------------------- 5. sequence flattening

#[test]
fn from_pattern_uses_pattern_local_positions_and_honours_pattern_length() {
    let m = fixture();
    let (t0, t1, p0, p1) = ids(&m);

    let seq = Sequence::from_pattern(&m, &p0, 0.0).expect("pattern 1 exists");
    assert_eq!(seq.length_beats, 16.0);
    assert_eq!(seq.tempo_bpm, m.tempo);
    assert_eq!(seq.tracks.len(), m.tracks.len());
    assert_eq!(seq.tracks[0].id, t0);
    let starts: Vec<f64> = seq.tracks[0].notes.iter().map(|n| n.start_beat).collect();
    assert_eq!(starts, vec![0.0, 4.0], "pattern-mode notes must keep their pattern-local beats");
    assert!(seq.tracks[1].notes.is_empty(), "track {t1} is not this pattern's track");
    assert_eq!(seq.chords.chords().len(), 4, "the progression's first 16 beats");

    let short = Sequence::from_pattern(&m, &p1, 0.0).unwrap();
    assert_eq!(short.length_beats, 8.0);
    assert!(short.tracks.iter().all(|t| t.notes.is_empty()));
    assert_eq!(short.chords.end_beat(), 8.0, "chords are cut at the pattern end");

    assert!(Sequence::from_pattern(&m, &Uuid::new_v4(), 0.0).is_none(), "unknown pattern id must be None");
}

/// Pattern mode tiers against the bar the pattern was opened at: a clip at song beat 8 sees
/// the song's chords from beat 8, starting at its own beat 0.
#[test]
fn pattern_mode_chords_follow_the_chord_offset() {
    let mut m = fixture();
    let (_, _, p0, _) = ids(&m);
    m.chords = vec![
        ChordEvent::new(0.0, 8.0, vec![0, 4, 7], Some("C".into())),
        ChordEvent::new(8.0, 8.0, vec![9, 0, 4], Some("Am".into())),
        ChordEvent::new(16.0, 16.0, vec![5, 9, 0], Some("F".into())),
    ];
    let names = |offset: f64| -> Vec<(f64, f64, String)> {
        let seq = Sequence::from_pattern(&m, &p0, offset).unwrap();
        seq.chords.chords().iter().map(|c| (c.start_beat, c.length_beats, c.name.clone().unwrap())).collect()
    };
    assert_eq!(names(0.0), vec![(0.0, 8.0, "C".into()), (8.0, 8.0, "Am".into())]);
    assert_eq!(names(8.0), vec![(0.0, 8.0, "Am".into()), (8.0, 8.0, "F".into())]);
    assert_eq!(names(20.0), vec![(0.0, 12.0, "F".into())], "mid-chord offset, cut at the progression end");
    // The notes do not move with the offset; only the harmony under them does.
    let notes = |offset: f64| Sequence::from_pattern(&m, &p0, offset).unwrap().tracks[0].notes.clone();
    assert_eq!(notes(0.0), notes(8.0));
}

#[test]
fn song_flattening_produces_absolute_beat_positions() {
    let mut m = fixture();
    let (_, t1, p0, p1) = ids(&m);
    // p0 is 16 beats with notes at 0 and 4; melody 2 is 8 beats with a note at 2; the drums
    // play under all of it from their own pattern.
    let p2 = add_melody_pattern(&mut m, 8.0, vec![NoteEvent::new(2.0, 1.0, 67)]);
    m.patterns[1].notes = vec![NoteEvent::new(0.0, 0.25, 36)];
    m.clips = vec![clip_at(p0, 0.0, 16.0), clip_at(p2, 16.0, 8.0), clip_at(p0, 24.0, 16.0), clip_at(p1, 0.0, 40.0)];

    let seq = Sequence::from_song(&m);
    assert_eq!(seq.length_beats, m.song_length_beats, "the song ends at its explicit length");

    let starts: Vec<f64> = seq.tracks[0].notes.iter().map(|n| n.start_beat).collect();
    // p0 @0 → 0,4 ; melody 2 @16 → 18 ; p0 @24 → 24,28
    assert_eq!(starts, vec![0.0, 4.0, 18.0, 24.0, 28.0]);
    let drums: Vec<f64> = seq.tracks[1].notes.iter().map(|n| n.start_beat).collect();
    assert_eq!(drums, vec![0.0, 8.0, 16.0, 24.0, 32.0], "track {t1} loops its own 8-beat pattern");

    // Repeated occurrences get distinct but deterministic ids.
    let ids: Vec<Uuid> = seq.tracks[0].notes.iter().map(|n| n.id).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "repeated pattern occurrences reused note ids");
    assert_eq!(ids, Sequence::from_song(&m).tracks[0].notes.iter().map(|n| n.id).collect::<Vec<_>>());

    // The chords are the project's own, in song beats.
    assert_eq!(seq.chords.chords(), m.chords.as_slice());
}

#[test]
fn a_song_sequence_always_has_a_playable_loop_length() {
    // `SetPatternLength` refuses anything below 1 beat, but nothing clamps a pattern's
    // `lengthBeats` when a project is *loaded*, so a hand-edited or third-party file can carry 0
    // or a negative length. `Sequence::from_pattern` guards this with `.max(1.0)`, and the song
    // length is held to at least one bar on load; otherwise song mode never loops
    // (`len > 0.0` in the engine) and "Export" fails outright with `RenderError::Empty`.
    let base = r#"{
      "schemaVersion": 6,
      "songLengthBeats": SONG,
      "tracks": [ { "id": "11111111-1111-1111-1111-111111111111", "name": "lead" } ],
      "patterns": [ { "id": "22222222-2222-2222-2222-222222222222", "trackId": "11111111-1111-1111-1111-111111111111", "name": "p", "lengthBeats": LEN } ],
      "clips": [ { "patternId": "22222222-2222-2222-2222-222222222222", "startBeat": 0, "lengthBeats": 16 } ]
    }"#;
    for len in ["0", "-8", "0.25"] {
        let m = ProjectModel::from_json(&base.replace("LEN", len).replace("SONG", len)).unwrap();
        let pid = m.patterns[0].id;
        assert!(
            Sequence::from_pattern(&m, &pid, 0.0).unwrap().length_beats >= 1.0,
            "pattern mode produced an unplayable loop for lengthBeats {len}"
        );
        assert!(
            Sequence::from_song(&m).length_beats >= 4.0,
            "song mode produced an unplayable loop ({}) for songLengthBeats {len}",
            Sequence::from_song(&m).length_beats
        );
    }
}

#[test]
fn song_flattening_survives_empty_and_degenerate_input() {
    // Empty song: no notes, but the song is still as long as it says and keeps its chords.
    let mut m = fixture();
    m.clips = vec![];
    let seq = Sequence::from_song(&m);
    assert!(seq.tracks.iter().all(|t| t.notes.is_empty()));
    assert_eq!(seq.chords.chords(), m.chords.as_slice());
    assert_eq!(seq.length_beats, m.song_length_beats);

    // Clips of patterns that are gone: loading drops them.
    m.clips = vec![clip_at(Uuid::new_v4(), 0.0, 16.0), clip_at(Uuid::new_v4(), 16.0, 16.0)];
    let m = ProjectModel::from_json(&m.to_json().unwrap()).unwrap();
    assert!(m.clips.is_empty(), "clips of deleted patterns survived loading");
    let seq = Sequence::from_song(&m);
    assert!(seq.tracks.iter().all(|t| t.notes.is_empty()));
    let mut m = m;

    // A dead id in the middle must not shift the patterns after it.
    let (_, _, p0, _) = ids(&m);
    let p2 = add_melody_pattern(&mut m, 16.0, vec![NoteEvent::new(0.0, 1.0, 67)]);
    m.clips = vec![clip_at(p0, 0.0, 16.0), clip_at(p2, 16.0, 16.0)];
    let seq = Sequence::from_song(&m);
    let starts: Vec<f64> = seq.tracks[0].notes.iter().map(|n| n.start_beat).collect();
    assert_eq!(Arrangement::total_length(&m.clips), 32.0);
    assert!(starts.contains(&16.0), "the pattern after a dead id was misplaced: {starts:?}");

    // A pattern with no notes and a project with no chords: empty, not a panic.
    let mut bare = ProjectModel::empty();
    let bare_p = bare.patterns[0].id;
    bare.clips = vec![clip_at(bare_p, 0.0, 16.0)];
    let seq = Sequence::from_song(&bare);
    assert_eq!(seq.length_beats, bare.song_length_beats);
    assert!(seq.chords.is_empty());
    assert!(seq.tracks.iter().all(|t| t.notes.is_empty()));
    assert!(!seq.any_solo());
}

#[test]
fn sequence_carries_mute_and_solo_so_audibility_is_decidable() {
    let mut d = doc();
    let (t0, t1, p0, _) = ids(d.model());
    d.apply(Command::SetTrackMuted { id: t0, muted: true }, false).unwrap();
    let seq = Sequence::from_pattern(d.model(), &p0, 0.0).unwrap();
    assert!(!seq.any_solo());
    assert!(!seq.tracks[0].audible(false), "a muted track must not be audible");
    assert!(seq.tracks[1].audible(false));

    d.apply(Command::SetTrackSoloed { id: t1, soloed: true }, false).unwrap();
    let seq = Sequence::from_pattern(d.model(), &p0, 0.0).unwrap();
    assert!(seq.any_solo());
    assert!(!seq.tracks[0].audible(true));
    assert!(seq.tracks[1].audible(true));
}


// ---------------------------------------------------------------- 6. persistence / migration

#[test]
fn save_load_round_trip_preserves_every_edit() {
    let mut d = doc();
    let (t0, t1, p0, p1) = ids(d.model());
    d.apply(Command::SetTempo { bpm: 143.0 }, false).unwrap();
    d.apply(Command::SetKey { key: KeyState::locked(9, ScaleType::Minor) }, false).unwrap();
    d.apply(Command::RenameTrack { id: t0, name: "lead".into() }, false).unwrap();
    d.apply(Command::SetTrackVoice { id: t0, voice: "pluck".into() }, false).unwrap();
    d.apply(Command::SetTrackColor { id: t0, color: Some("#AABBCC".into()) }, false).unwrap();
    d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Pan, value: -0.42 }, false).unwrap();
    d.apply(Command::SetTrackMuted { id: t1, muted: true }, false).unwrap();
    d.apply(Command::SetPatternLength { id: p1, beats: 12.5 }, false).unwrap();
    d.apply(Command::RenamePattern { id: p1, name: "chorus".into() }, false).unwrap();
    d.apply(
        Command::SetNotes {
            pattern_id: p1,
            notes: vec![NoteEvent::new(1.5, 0.25, 36).with_velocity(1), NoteEvent::new(2.0, 0.5, 42)],
        },
        false,
    )
    .unwrap();
    d.apply(Command::SetChords { chords: vec![ChordEvent::new(0.0, 4.0, vec![9, 0, 4], None)] }, false).unwrap();
    d.apply(Command::SetSongLength { beats: 96.0 }, false).unwrap();
    for (pattern_id, start) in [(p1, 0.0), (p0, 16.0), (p1, 32.0)] {
        d.apply(Command::AddClip { pattern_id, start_beat: start, length_beats: None }, false).unwrap();
    }
    d.apply(Command::AddPattern { track_id: t0 }, false).unwrap();
    d.apply(
        Command::SetMaster {
            master: MasterSettings {
                gain: 0.8,
                reverb_wet: 0.33,
                low_cut_hz: 60.0,
                high_cut_hz: 12_000.0,
                low_eq: 1.2,
                mid_eq: 0.8,
                high_eq: 1.1,
            },
        },
        false,
    )
    .unwrap();

    let saved = d.model().clone();
    let json = saved.to_json().unwrap();
    let loaded = ProjectModel::from_json(&json).expect("a file we just wrote must load");
    assert_eq!(loaded, saved, "round trip lost or changed state");

    // And it is stable: saving the loaded project yields the same project again.
    assert_eq!(ProjectModel::from_json(&loaded.to_json().unwrap()).unwrap(), saved);

    // The flattened song is identical too, ids included.
    assert_eq!(Sequence::from_song(&loaded), Sequence::from_song(&saved));
}

#[test]
fn a_file_missing_newer_optional_fields_opens_with_documented_defaults() {
    let track = "11111111-1111-1111-1111-111111111111";
    let pattern = "22222222-2222-2222-2222-222222222222";
    // A minimal schema-6 file: no velocity, no master, no voice/tone/pan, no chords, no song
    // length.
    let json = format!(
        r#"{{
          "schemaVersion": 6,
          "tempo": 128,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [ {{ "id": "{pattern}", "trackId": "{track}", "name": "verse",
            "notes": [ {{ "startBeat": 0, "lengthBeats": 1, "pitch": 60 }} ]
          }} ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).expect("a minimal file must still open");

    assert_eq!(m.schema_version, SCHEMA_VERSION);
    assert_eq!(m.tempo, 128.0);
    assert_eq!(m.key, KeyState::NONE);
    assert_eq!(m.master, MasterSettings::default());
    assert_eq!(m.master.low_cut_hz, 20.0);
    assert_eq!(m.master.high_cut_hz, 18_000.0);
    assert!(m.chords.is_empty());
    assert_eq!(m.song_length_beats, DEFAULT_SONG_BARS as f64 * 4.0, "missing songLengthBeats is the default song");

    let t = &m.tracks[0];
    assert_eq!(t.voice, "saw");
    assert_eq!(t.volume, 1.0);
    assert_eq!(t.tone, 18_000.0);
    assert_eq!((t.pan, t.reverb_send), (0.0, 0.0));
    assert_eq!((t.muted, t.soloed, t.is_drum), (false, false, false));
    assert_eq!(t.color, None);

    let p = &m.patterns[0];
    assert_eq!(p.length_beats, 16.0, "missing lengthBeats must default to 16");
    assert_eq!(p.track_id, t.id);

    // A note with no velocity plays at the documented default, and gets a real id.
    let note = &p.notes[0];
    assert_eq!(note.velocity, 100, "a note without velocity must default to 100");
    assert_ne!(note.id, Uuid::nil(), "a note without an id must get a fresh one");
}

#[test]
fn an_empty_or_stub_file_still_opens_as_a_usable_project() {
    for json in [
        r#"{"schemaVersion": 6}"#,
        r#"{"schemaVersion": 6, "tracks": []}"#,
        r#"{"schemaVersion": 6, "patterns": []}"#,
        r#"{"schemaVersion": 6, "tracks": [], "patterns": []}"#,
    ] {
        let m = ProjectModel::from_json(json).unwrap_or_else(|e| panic!("{json} failed to open: {e}"));
        assert!(!m.tracks.is_empty(), "{json} produced a project with no tracks");
        assert!(!m.patterns.is_empty(), "{json} produced a project with nothing to write into");
        // And a usable project is one you can immediately play and edit.
        assert!(Sequence::from_pattern(&m, &m.patterns[0].id, 0.0).is_some());
        let mut d = Document::new(m);
        let p0 = d.model().patterns[0].id;
        d.apply(Command::SetNotes { pattern_id: p0, notes: vec![NoteEvent::new(0.0, 1.0, 60)] }, false).unwrap();
    }
}

#[test]
fn garbage_input_produces_an_error_not_a_panic() {
    let deep = "[".repeat(200);
    let garbage = [
        "",
        "   ",
        "null",
        "42",
        "\"a string\"",
        "{",
        "{ \"tempo\": }",
        r#"{ "schemaVersion": 6, "tempo": "fast" }"#,
        r#"{ "schemaVersion": 6, "tempo": null }"#,
        r#"{ "schemaVersion": 6, "tracks": "nope" }"#,
        r#"{ "schemaVersion": 6, "tracks": [ { "voice": "saw" } ] }"#, // track with no name
        r#"{ "schemaVersion": 6, "patterns": [ { "name": "p", "trackId": "11111111-1111-1111-1111-111111111111", "lengthBeats": "long" } ] }"#,
        r#"{ "schemaVersion": 6, "tracks": [ { "id": "not-a-uuid", "name": "x" } ] }"#,
        r#"{ "schemaVersion": 6, "patterns": [ { "name": "p", "trackId": "nope" } ] }"#,
        r#"{ "schemaVersion": 6, "patterns": [ { "name": "p" } ] }"#, // a pattern belongs to a track
        r#"{ "schemaVersion": 6, "chords": [ { "startBeat": 0 } ] }"#,
        r#"{ "schemaVersion": 6, "songLengthBeats": "long" }"#,
        "\u{feff}{}",
        deep.as_str(),
    ];
    for json in garbage {
        let short: String = json.chars().take(40).collect();
        assert!(ProjectModel::from_json(json).is_err(), "{short:?} was accepted as a project file");
    }
}

#[test]
fn a_bare_json_array_is_not_a_project() {
    // serde's derived struct impl also accepts the *sequence* form, and every `ProjectModel`
    // field defaults, so `[]` would otherwise deserialize into a full default project: "Open…"
    // on an unrelated `.json` file would silently hand the user a blank song, and saving over
    // it would destroy the original file.
    for not_a_project in ["[]", "[1,2,3]", r#"["tempo", 120]"#] {
        assert!(
            ProjectModel::from_json(not_a_project).is_err(),
            "{not_a_project} was accepted as a project file"
        );
    }
    // An object still opens, including the minimal one.
    assert_eq!(ProjectModel::from_json(r#"{"schemaVersion": 6}"#).unwrap().tempo, 120.0);
}

#[test]
fn files_from_another_schema_are_refused_rather_than_converted() {
    // Opening a future file here would drop every field this build doesn't know, and the next
    // Save would overwrite the user's richer project with the lossy version. Refuse instead.
    let future = SCHEMA_VERSION + 96;
    let json = format!(r#"{{ "schemaVersion": {future}, "tempo": 100, "unknownFutureField": [1,2,3] }}"#);
    let err = ProjectModel::from_json(&json).expect_err("a file from the future was opened anyway");
    assert!(
        err.to_string().contains("newer version"),
        "the error should tell the user why: {err}"
    );
    // Anything before schema 6 held every track's notes in each pattern: no migration, a
    // clear refusal.
    for version in [0, 1, 2, 3, 4, 5] {
        let json = format!(r#"{{ "schemaVersion": {version}, "tempo": 100 }}"#);
        let err = ProjectModel::from_json(&json).expect_err("an old file was opened");
        assert_eq!(err.to_string(), OLD_SCHEMA_ERROR);
    }
    let json = format!(r#"{{ "schemaVersion": {SCHEMA_VERSION}, "tempo": 100 }}"#);
    assert_eq!(ProjectModel::from_json(&json).unwrap().schema_version, SCHEMA_VERSION);
}

#[test]
fn loading_drops_patterns_of_tracks_that_do_not_exist() {
    // A pattern of a track that is not in the project is invisible in the UI and never played,
    // but would otherwise survive every load/save cycle and grow the file forever. Its clips go
    // with it.
    let ghost = "33333333-3333-3333-3333-333333333333";
    let track = "11111111-1111-1111-1111-111111111111";
    let json = format!(
        r#"{{
          "schemaVersion": 6,
          "tracks": [ {{ "id": "{track}", "name": "lead" }} ],
          "patterns": [
            {{ "id": "44444444-4444-4444-4444-444444444444", "trackId": "{track}", "name": "p",
               "notes": [ {{ "startBeat": 0, "lengthBeats": 1, "pitch": 60 }} ] }},
            {{ "id": "55555555-5555-5555-5555-555555555555", "trackId": "{ghost}", "name": "orphan",
               "notes": [ {{ "startBeat": 0, "lengthBeats": 1, "pitch": 72 }} ] }}
          ],
          "clips": [
            {{ "patternId": "44444444-4444-4444-4444-444444444444", "startBeat": 0, "lengthBeats": 16 }},
            {{ "patternId": "55555555-5555-5555-5555-555555555555", "startBeat": 0, "lengthBeats": 16 }}
          ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).unwrap();
    assert_eq!(m.patterns.len(), 1, "the orphan pattern survived loading");
    assert_eq!(m.patterns[0].name, "p");
    assert_eq!(m.patterns[0].notes[0].pitch, 60, "the live track's notes are untouched");
    assert_eq!(m.clips.len(), 1, "the orphan's clip survived loading");
}

#[test]
fn json_is_camel_case_on_every_nested_field() {
    let json = fixture().to_json().unwrap();
    for key in ["schemaVersion", "trackId", "lengthBeats", "startBeat", "pitchClasses", "isDrum", "songLengthBeats"] {
        assert!(json.contains(&format!("\"{key}\"")), "missing camelCase key {key}");
    }
    for snake in ["length_beats", "start_beat", "track_id", "pitch_classes", "is_drum", "reverb_send", "song_length_beats"] {
        assert!(!json.contains(snake), "snake_case key {snake} leaked into the file");
    }
    assert!(!json.contains("notesByTrack"), "the schema-5 layout leaked into the file");
}


// ---------------------------------------------------------------- dirty / path tracking

#[test]
fn dirty_tracking_follows_edits_and_saves() {
    let mut d = doc();
    assert!(!d.is_dirty());
    assert!(d.path.is_none());

    d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
    assert!(d.is_dirty(), "an edit did not mark the document dirty");

    d.mark_saved();
    assert!(!d.is_dirty());

    assert!(d.undo());
    assert!(d.is_dirty(), "undoing past the save point must mark the document dirty");
}

#[test]
fn returning_to_the_saved_state_reports_no_unsaved_changes() {
    // Edit, save, undo, redo: the project is byte-for-byte what is on disk, so the title bar
    // must drop its modified marker and quitting must not prompt about unsaved changes.
    let mut d = doc();
    d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
    d.mark_saved();
    let on_disk = d.model().clone();

    assert!(d.undo());
    assert!(d.is_dirty(), "undoing away from the saved state is an unsaved change");
    assert!(d.redo());
    assert_eq!(d.model(), &on_disk, "undo+redo did not return to the saved state");
    assert!(!d.is_dirty(), "the document matches the file on disk but still reports changes");

    // A gesture that lands back on its starting value is likewise not an unsaved change.
    let t0 = d.model().tracks[0].id;
    for v in [0.5, 0.2, 1.0] {
        d.apply(Command::SetTrackParam { id: t0, param: TrackParam::Volume, value: v }, true).unwrap();
    }
    d.commit_gesture();
    assert!(!d.is_dirty(), "a round-trip slider drag reported unsaved changes");
}

#[test]
fn a_factory_voice_prunes_the_user_sound_it_replaces() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    d.apply(Command::SetTrackVoice { id: t0, voice: "pad".into() }, false).unwrap();
    assert!(d.model().sounds.is_empty(), "u-test outlived the track that used it");
}

#[test]
fn loading_repairs_a_broken_kit_and_unclamped_sounds() {
    let mut m = fixture();
    m.tracks[1].drum_pads = vec![pad(36, "kick", 0), pad(36, "snare", 0)];
    m.sounds[0] = user_sound(9.0);
    m.sounds.push(drum_sound("u-orphan"));
    let m = m.normalized();
    assert_eq!(m.tracks[1].drum_pads, default_kit());
    assert_eq!(m.resolve_synth("u-test").level, 1.0);
    assert_eq!(embedded(&m), ["u-test"], "an unused sound survived loading");
}
