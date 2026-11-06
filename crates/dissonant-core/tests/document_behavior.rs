//! Behavioural audit of `Document` (undo/redo, transient gestures, dirty + path tracking),
//! command validation, referential integrity, sequence flattening and persistence.
//!
//! Every assertion here is about an observable contract a user can feel: "undo puts the project
//! back exactly where it was", "a refused edit changes nothing", "what I saved is what I load".

use dissonant_core::document::TrackParam;
use dissonant_core::*;
use uuid::Uuid;

const BEAT: i64 = TICKS_PER_BEAT;

/// Opaque plugin-layer uids, in the shape the plugin layer mints them.
const DELAY_UID: &str =
    "clap:/Library/Audio/Plug-Ins/CLAP/ValhallaDelay.clap:com.valhalladsp.delay";
const EQ_UID: &str = "clap:/Library/Audio/Plug-Ins/CLAP/FabFilter Pro-Q 3.clap:com.fabfilter.proq3";

// ---------------------------------------------------------------- fixtures

/// Three tracks (melody + drums + guitar), one pattern on each note track (the melody's 16
/// beats with three notes, the last roped to the second, the drums' 8 beats and empty), both placed in the starter's 128-beat
/// song with its chords, one audio take on the guitar track, and effect chains on the melody
/// track and the master. Rich enough that every `Command` variant has something to bite on.
fn fixture() -> ProjectModel {
    let mut m = ProjectModel::starter();
    let second = NoteEvent::new(4 * BEAT, 2 * BEAT, 64).with_velocity(70);
    // Touching the second, so the rope is live; the rope commands have something to bite on.
    let mut third = NoteEvent::new(6 * BEAT, BEAT, 67);
    third.slide_from = Some(second.id);
    m.patterns[0].notes = vec![NoteEvent::new(0, BEAT, 60).with_velocity(90), second, third];
    m.patterns[1].notes.clear(); // the starter's groove
    m.patterns[1].length_ticks = 8 * BEAT;
    let p0 = m.patterns[0].id;
    let p1 = m.patterns[1].id;
    // The melody track is plugin-driven, so the plugin commands have something to bite on.
    m.tracks[0].plugin = Some(TrackPlugin {
        uid: "clap:/Library/Audio/Plug-Ins/CLAP/Surge XT.clap:org.surge-synth-team.surge-xt".into(),
        name: "Surge XT".into(),
        state: Some("c3VyZ2U=".into()),
    });
    // A guitar track with one take on it, so the audio commands have something to bite on.
    let guitar = Track::audio("guitar");
    let gid = guitar.id;
    m.tracks.push(guitar);
    let source = take_source();
    m.audio_clips = vec![AudioClip::new(gid, source.id, 0, 2.0)];
    m.audio_sources = vec![source];
    // Two chords waiting in the detected lane, so accepting and dismissing have something to
    // bite on.
    m.detections = vec![ClipDetection {
        clip_id: m.audio_clips[0].id,
        chords: vec![
            DetectedChord {
                id: Uuid::new_v4(),
                start_tick: 2 * BEAT,
                length_ticks: 4 * BEAT,
                pitch_classes: vec![9, 0, 4],
                name: Some("Am".into()),
                confidence: 0.9,
            },
            DetectedChord {
                id: Uuid::new_v4(),
                start_tick: 6 * BEAT,
                length_ticks: 2 * BEAT,
                pitch_classes: vec![5, 9, 0],
                name: Some("F".into()),
                confidence: 0.8,
            },
        ],
        chroma: [0.0; 12],
    }];
    // Effect chains on a track and on the master, so the insert commands have something to
    // bite on. Two on the track: one insert cannot be reordered. The master's second slot is
    // a native effect, which is the only kind `SetInsertParam` applies to.
    m.tracks[0].inserts = vec![
        Insert::new(DELAY_UID, "Valhalla Delay"),
        Insert::new(EQ_UID, "Pro-Q"),
    ];
    m.master_inserts = vec![
        Insert::new(EQ_UID, "Pro-Q"),
        Insert::new(NativeKind::Compressor.uid(), "compressor"),
    ];
    m.clips = vec![clip_at(p0, 0, 16 * BEAT), clip_at(p1, 16 * BEAT, 8 * BEAT)];
    // The melody plays an embedded user sound, so the sound commands have something to bite on.
    m.sounds = vec![user_sound(0.3)];
    m.tracks[0].voice = "u-test".into();
    m
}

fn user_sound(level: f32) -> Sound {
    Sound {
        id: "u-test".into(),
        name: "test".into(),
        category: "lead".into(),
        patch: SoundPatch::Synth(SynthPatch {
            level,
            ..SynthPatch::default()
        }),
    }
}

/// A second pattern on the melody track, so the song can switch melodies over the same beat.
fn add_melody_pattern(m: &mut ProjectModel, length_ticks: i64, notes: Vec<NoteEvent>) -> Uuid {
    let mut p = SongPattern::new(m.tracks[0].id, "melody 2");
    p.length_ticks = length_ticks;
    p.notes = notes;
    let id = p.id;
    m.patterns.push(p);
    id
}

/// A clip placing `pattern` on the song timeline.
fn clip_at(pattern: Uuid, start: i64, length: i64) -> Clip {
    let mut clip = Clip::new(pattern, start, length);
    clip.id = Uuid::new_v4();
    clip
}

/// A one-second 48 kHz mono source, as a recorded take would arrive.
fn take_source() -> AudioSource {
    AudioSource {
        id: Uuid::new_v4(),
        name: "take 1".into(),
        frames: 48_000,
        sample_rate: 48_000,
        channels: 1,
    }
}

/// The fixture's guitar track.
fn audio_track(m: &ProjectModel) -> Uuid {
    m.tracks
        .iter()
        .find(|t| t.is_audio)
        .expect("fixture has an audio track")
        .id
}

fn doc() -> Document {
    Document::new(fixture())
}

/// (melody track, drum track, melody pattern, drum pattern).
fn ids(m: &ProjectModel) -> (Uuid, Uuid, Uuid, Uuid) {
    (
        m.tracks[0].id,
        m.tracks[1].id,
        m.patterns[0].id,
        m.patterns[1].id,
    )
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
        Command::SetTrackPlugin { .. } => "SetTrackPlugin",
        Command::SetPluginState { .. } => "SetPluginState",
        Command::AddPattern { .. } => "AddPattern",
        Command::DuplicatePattern { .. } => "DuplicatePattern",
        Command::DeletePattern { .. } => "DeletePattern",
        Command::RenamePattern { .. } => "RenamePattern",
        Command::SetPatternLength { .. } => "SetPatternLength",
        Command::SetNotes { .. } => "SetNotes",
        Command::SetRope { .. } => "SetRope",
        Command::ClearRope { .. } => "ClearRope",
        Command::SetSlideTime { .. } => "SetSlideTime",
        Command::SetChords { .. } => "SetChords",
        Command::AddClip { .. } => "AddClip",
        Command::UpdateClip { .. } => "UpdateClip",
        Command::UpdateClips { .. } => "UpdateClips",
        Command::RemoveClip { .. } => "RemoveClip",
        Command::RemoveClips { .. } => "RemoveClips",
        Command::MakeClipUnique { .. } => "MakeClipUnique",
        Command::SetSongLength { .. } => "SetSongLength",
        Command::SetLoopRegion { .. } => "SetLoopRegion",
        Command::SetPunchRegion { .. } => "SetPunchRegion",
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
        Command::AddAutomationPoint { .. } => "AddAutomationPoint",
        Command::UpdateAutomationPoint { .. } => "UpdateAutomationPoint",
        Command::RemoveAutomationPoint { .. } => "RemoveAutomationPoint",
        Command::ClearAutomation { .. } => "ClearAutomation",
        Command::AddAudioTrack { .. } => "AddAudioTrack",
        Command::SetTrackAmp { .. } => "SetTrackAmp",
        Command::AddAudioTake { .. } => "AddAudioTake",
        Command::PunchAudioTake { .. } => "PunchAudioTake",
        Command::UpdateAudioClip { .. } => "UpdateAudioClip",
        Command::RemoveAudioClip { .. } => "RemoveAudioClip",
        Command::AddInsert { .. } => "AddInsert",
        Command::RemoveInsert { .. } => "RemoveInsert",
        Command::MoveInsert { .. } => "MoveInsert",
        Command::SetInsertEnabled { .. } => "SetInsertEnabled",
        Command::SetInsertState { .. } => "SetInsertState",
        Command::SetInsertParam { .. } => "SetInsertParam",
        Command::InsertTime { .. } => "InsertTime",
        Command::DeleteTime { .. } => "DeleteTime",
        Command::DuplicateRange { .. } => "DuplicateRange",
        Command::AddTranscription { .. } => "AddTranscription",
        Command::AcceptDetectedChord { .. } => "AcceptDetectedChord",
        Command::DismissDetectedChord { .. } => "DismissDetectedChord",
    }
}

/// One mutating instance of every `Command` variant, bound to the ids of `m`.
fn one_of_every_variant(m: &ProjectModel) -> Vec<Command> {
    let (t0, t1, p0, p1) = ids(m);
    let gid = audio_track(m);
    let take = take_source();
    let punch_take = take_source();
    let existing = m.audio_clips[0].clone();
    let (chain, master) = (InsertTarget::Track(t0), InsertTarget::Master);
    let (first, second) = (m.tracks[0].inserts[0].id, m.tracks[0].inserts[1].id);
    let (c0, c1) = (m.clips[0].clone(), m.clips[1].clone());
    vec![
        Command::SetTempo { bpm: 90.0 },
        Command::SetKey {
            key: KeyState::locked(5, ScaleType::Minor),
        },
        Command::AddTrack { is_drum: false },
        Command::AddTrack { is_drum: true },
        Command::DeleteTrack { id: t0 },
        Command::RenameTrack {
            id: t0,
            name: "lead".into(),
        },
        Command::SetTrackMuted {
            id: t0,
            muted: true,
        },
        Command::SetTrackSoloed {
            id: t0,
            soloed: true,
        },
        Command::MoveTrack { id: t0, up: false },
        Command::SetTrackVoice {
            id: t0,
            voice: "pad".into(),
        },
        Command::SetTrackSound {
            id: t0,
            sound: factory_sound("sine").unwrap().clone(),
        },
        Command::SetDrumPads {
            id: t1,
            pads: default_kit().into_iter().rev().collect(),
            sounds: vec![],
        },
        Command::UpsertSound {
            sound: user_sound(0.6),
        },
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Volume,
            value: 0.5,
        },
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::ReverbSend,
            value: 0.25,
        },
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Tone,
            value: 4_000.0,
        },
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Pan,
            value: -0.75,
        },
        Command::SetTrackColor {
            id: t0,
            color: Some("#010203".into()),
        },
        Command::SetTrackPlugin {
            id: t0,
            plugin: Some(TrackPlugin {
                uid: "clap:/CLAP/Vital.clap:audio.vital.synth".into(),
                name: "Vital".into(),
                state: None,
            }),
        },
        Command::SetPluginState {
            id: t0,
            state: "ZGlmZmVyZW50".into(),
        },
        Command::AddPattern { track_id: t0 },
        Command::DuplicatePattern { id: p0 },
        Command::DeletePattern { id: p1 },
        Command::RenamePattern {
            id: p0,
            name: "verse".into(),
        },
        Command::SetPatternLength {
            id: p0,
            ticks: 32 * BEAT,
        },
        Command::SetNotes {
            pattern_id: p1,
            notes: vec![NoteEvent::new(BEAT, BEAT, 62)],
        },
        Command::SetRope {
            pattern_id: p0,
            note_id: m.patterns[0].notes[1].id,
            source_id: m.patterns[0].notes[0].id,
        },
        Command::ClearRope {
            pattern_id: p0,
            note_id: m.patterns[0].notes[2].id,
        },
        Command::SetSlideTime {
            pattern_id: p0,
            note_id: m.patterns[0].notes[2].id,
            secs: Some(0.5),
        },
        Command::SetChords {
            chords: vec![ChordEvent::new(
                0,
                4 * BEAT,
                vec![2, 5, 9],
                Some("Dm".into()),
            )],
        },
        Command::AddClip {
            pattern_id: p1,
            start_tick: 16 * BEAT,
            length_ticks: None,
        },
        Command::UpdateClip {
            clip: Clip {
                start_tick: 32 * BEAT,
                ..c1.clone()
            },
        },
        Command::UpdateClips {
            clips: vec![
                Clip {
                    start_tick: 4 * BEAT,
                    ..c0.clone()
                },
                Clip {
                    muted: true,
                    ..c1.clone()
                },
            ],
        },
        Command::RemoveClip { id: c0.id },
        Command::RemoveClips {
            ids: vec![c0.id, c1.id],
        },
        Command::MakeClipUnique { id: c0.id },
        Command::SetSongLength { ticks: 20 * BEAT },
        Command::SetLoopRegion {
            region: Some((4 * BEAT, 12 * BEAT)),
        },
        Command::SetPunchRegion {
            region: Some((16 * BEAT, 20 * BEAT)),
        },
        Command::ImportTracks {
            tracks: vec![ImportedTrack {
                name: "piano".into(),
                is_drum: false,
                notes: vec![NoteEvent::new(0, BEAT, 60)],
            }],
            length_ticks: 16 * BEAT,
        },
        Command::AddSection {
            name: "chorus".into(),
            start_tick: 32 * BEAT,
        },
        Command::SetMaster {
            master: MasterSettings {
                gain: 0.5,
                ..MasterSettings::default()
            },
        },
        Command::SetTimeSignature {
            numerator: 3,
            denominator: 4,
        },
        Command::SetSwing {
            swing: 62.0,
            grid_ticks: BEAT / 4,
        },
        Command::AddTempoPoint {
            tick: 8 * BEAT,
            bpm: 140.0,
            ramp: true,
        },
        Command::AddAutomationPoint {
            target: AutoTarget::Track(t0, TrackParam::Volume),
            tick: 8 * BEAT,
            value: 0.25,
            curve: Curve::Ramp,
        },
        Command::AddAudioTrack {
            name: "gtr 2".into(),
        },
        Command::SetTrackAmp {
            id: gid,
            amp: AmpRig {
                amp: AmpHead {
                    model: AmpModel::Lead,
                    gain: 9.0,
                    ..AmpHead::default()
                },
                ..AmpRig::default()
            },
        },
        Command::AddAudioTake {
            track_id: gid,
            clip: AudioClip::new(gid, take.id, 8 * BEAT, 1.0),
            source: take,
        },
        Command::PunchAudioTake {
            track_id: gid,
            clip: AudioClip::new(gid, punch_take.id, BEAT, 0.5),
            source: punch_take,
            crossfade_seconds: DEFAULT_PUNCH_CROSSFADE_SECONDS,
        },
        Command::UpdateAudioClip {
            clip: AudioClip {
                start_tick: 12 * BEAT,
                ..existing.clone()
            },
        },
        Command::RemoveAudioClip { id: existing.id },
        Command::AddInsert {
            target: chain,
            uid: EQ_UID.into(),
            name: "Pro-Q".into(),
            index: None,
        },
        Command::AddInsert {
            target: master,
            uid: DELAY_UID.into(),
            name: "Valhalla Delay".into(),
            index: Some(0),
        },
        Command::RemoveInsert {
            target: chain,
            insert_id: first,
        },
        Command::MoveInsert {
            target: chain,
            insert_id: first,
            to_index: 1,
        },
        Command::SetInsertEnabled {
            target: chain,
            insert_id: second,
            on: false,
        },
        Command::SetInsertState {
            target: master,
            insert_id: m.master_inserts[0].id,
            state: "cHJvcQ==".into(),
        },
        Command::SetInsertParam {
            target: master,
            insert_id: m.master_inserts[1].id,
            param_id: 201,
            value: 8.0,
        },
        Command::InsertTime {
            at: 4 * BEAT,
            ticks: 8 * BEAT,
        },
        Command::DeleteTime {
            at: 4 * BEAT,
            ticks: 8 * BEAT,
        },
        Command::DuplicateRange {
            from: 0,
            to: 16 * BEAT,
            at: 16 * BEAT,
        },
        Command::AddTranscription {
            name: "take 1 notes".into(),
            start_tick: 0,
            length_ticks: 8 * BEAT,
            notes: vec![NoteEvent::new(0, BEAT, 52), NoteEvent::new(BEAT, BEAT, 55)],
            detection: ClipDetection {
                clip_id: existing.id,
                chords: vec![],
                chroma: [0.0; 12],
            },
        },
        Command::AcceptDetectedChord {
            id: m.detections[0].chords[0].id,
        },
        Command::DismissDetectedChord {
            id: m.detections[0].chords[1].id,
        },
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

        d.apply(command.clone(), false)
            .unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        let after = d.model().clone();
        assert_ne!(
            after, before,
            "{label}: command did not change the model; fixture is too weak"
        );
        assert!(d.can_undo(), "{label}: no undo step was recorded");

        assert!(d.undo(), "{label}: undo refused");
        assert_eq!(
            d.model(),
            &before,
            "{label}: undo did not restore the exact prior state"
        );

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
        d.apply(
            Command::SetTempo {
                bpm: 21.0 + i as f64,
            },
            false,
        )
        .unwrap();
    }
    assert_eq!(d.model().tempo, 21.0 + (edits - 1) as f64);

    // Exactly MAX_UNDO steps are available, and each one lands on the right tempo.
    for i in (0..MAX_UNDO).rev() {
        assert!(
            d.undo(),
            "undo #{} refused while the cap should still allow it",
            MAX_UNDO - i
        );
        let expected = 21.0 + (edits - 1 - (MAX_UNDO - i)) as f64;
        assert_eq!(
            d.model().tempo,
            expected,
            "undo walked to the wrong snapshot"
        );
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

    d.apply(Command::SetTempo { bpm: before.tempo }, false)
        .unwrap();
    d.apply(
        Command::RenameTrack {
            id: t0,
            name: before.tracks[0].name.clone(),
        },
        false,
    )
    .unwrap();
    d.apply(Command::MoveTrack { id: t0, up: true }, false)
        .unwrap(); // already at the top
    if let Some(clip) = before.clips.first() {
        d.apply(Command::UpdateClip { clip: clip.clone() }, false)
            .unwrap();
    }
    d.apply(
        Command::SetNotes {
            pattern_id: p0,
            notes: before.patterns[0].notes.clone(),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetChords {
            chords: before.chords.chords().to_vec(),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetSongLength {
            ticks: before.song_length_ticks,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::RemoveClips {
            ids: vec![Uuid::new_v4()],
        },
        false,
    )
    .unwrap();

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
        d.apply(
            Command::SetTrackParam {
                id: t0,
                param: TrackParam::Volume,
                value: v,
            },
            true,
        )
        .unwrap();
    }
    d.commit_gesture();
    assert_eq!(d.model().tracks[0].volume, 0.7);

    assert!(d.undo());
    assert_eq!(
        d.model(),
        &before,
        "the gesture did not rewind to the pre-gesture state"
    );
    assert!(!d.undo(), "the gesture produced more than one undo step");

    assert!(d.redo());
    assert_eq!(
        d.model().tracks[0].volume,
        0.7,
        "redo did not re-apply the whole gesture"
    );
    assert!(!d.redo());
}

#[test]
fn an_uncommitted_transient_run_is_still_undone_as_one_step() {
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let before = d.model().clone();

    for v in [-0.2, -0.4, -0.6] {
        d.apply(
            Command::SetTrackParam {
                id: t0,
                param: TrackParam::Pan,
                value: v,
            },
            true,
        )
        .unwrap();
    }
    // No commit_gesture() — the user let go of the mouse outside the window, say.
    assert!(d.can_undo());
    assert!(d.undo());
    assert_eq!(d.model(), &before);
    assert!(
        !d.undo(),
        "an uncommitted gesture left extra entries on the undo stack"
    );
}

#[test]
fn a_non_transient_command_mid_gesture_closes_the_gesture_first() {
    // Documented behaviour: the gesture so far becomes one undo step, and the interleaved
    // command becomes a second one. Undoing twice walks back through both.
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    let before = d.model().clone();

    d.apply(
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Pan,
            value: -0.3,
        },
        true,
    )
    .unwrap();
    d.apply(
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Pan,
            value: -0.6,
        },
        true,
    )
    .unwrap();
    d.apply(Command::SetTempo { bpm: 101.0 }, false).unwrap();
    d.apply(
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Pan,
            value: -0.9,
        },
        true,
    )
    .unwrap();
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
        d.apply(
            Command::SetTrackParam {
                id: t0,
                param: TrackParam::Volume,
                value: v,
            },
            true,
        )
        .unwrap();
    }
    d.commit_gesture();
    assert_eq!(d.model(), &before);
    assert!(
        !d.can_undo(),
        "a round-trip gesture left an undo step behind"
    );
}

#[test]
fn can_undo_only_promises_an_undo_that_will_happen() {
    // The Undo menu item is enabled from `can_undo()`. A transient gesture that has landed back
    // on its starting value has nothing to undo, so it must not light the menu item up.
    let mut d = doc();
    let (t0, ..) = ids(d.model());

    for v in [0.8, 0.6, 1.0] {
        d.apply(
            Command::SetTrackParam {
                id: t0,
                param: TrackParam::Volume,
                value: v,
            },
            true,
        )
        .unwrap();
    }
    // Gesture is still open (never committed) and has returned to its starting value.
    assert!(
        !d.can_undo(),
        "Undo was offered for a gesture that changed nothing"
    );
    assert!(!d.undo());

    // An open gesture that *has* moved is undoable, and the promise is kept.
    d.apply(
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Volume,
            value: 0.4,
        },
        true,
    )
    .unwrap();
    assert!(d.can_undo());
    assert!(d.undo(), "can_undo() promised an undo that undo() refused");
    assert_eq!(d.model().tracks[0].volume, 1.0);
}

// ---------------------------------------------------------------- 3. validation

#[test]
fn the_last_track_is_protected_but_a_project_may_have_no_patterns() {
    let mut d = Document::new(ProjectModel::empty());
    let (t0, p0) = (d.model().tracks[0].id, d.model().patterns[0].id);
    assert_eq!(
        d.apply(Command::DeleteTrack { id: t0 }, false),
        Err(EditError::LastTrack)
    );
    assert_eq!(d.model().tracks.len(), 1);
    d.apply(Command::DeletePattern { id: p0 }, false).unwrap();
    assert!(d.model().patterns.is_empty());
    // Nothing to play in pattern mode, but the song still plays (silence for its length).
    assert!(Sequence::from_pattern(d.model(), &p0, 0).is_none());
    assert_eq!(
        Sequence::from_song(d.model()).length_ticks,
        d.model().song_length_ticks
    );
    // And the track can grow a new one.
    d.apply(Command::AddPattern { track_id: t0 }, false)
        .unwrap();
    assert_eq!(d.model().patterns[0].name, "melody 1");
}

#[test]
fn unknown_ids_are_rejected() {
    let mut d = doc();
    let (_, _, p0, _) = ids(d.model());
    let ghost = Uuid::new_v4();

    use EditError::*;
    assert_eq!(
        d.apply(Command::DeleteTrack { id: ghost }, false),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::RenameTrack {
                id: ghost,
                name: "x".into()
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetTrackMuted {
                id: ghost,
                muted: true
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetTrackSoloed {
                id: ghost,
                soloed: true
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::MoveTrack {
                id: ghost,
                up: true
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetTrackVoice {
                id: ghost,
                voice: "pad".into()
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetTrackParam {
                id: ghost,
                param: TrackParam::Pan,
                value: 0.5
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetTrackColor {
                id: ghost,
                color: None
            },
            false
        ),
        Err(NoSuchTrack)
    );

    assert_eq!(
        d.apply(Command::DeletePattern { id: ghost }, false),
        Err(NoSuchPattern)
    );
    assert_eq!(
        d.apply(Command::DuplicatePattern { id: ghost }, false),
        Err(NoSuchPattern)
    );
    assert_eq!(
        d.apply(
            Command::RenamePattern {
                id: ghost,
                name: "x".into()
            },
            false
        ),
        Err(NoSuchPattern)
    );
    assert_eq!(
        d.apply(
            Command::SetPatternLength {
                id: ghost,
                ticks: 8 * BEAT
            },
            false
        ),
        Err(NoSuchPattern)
    );
    assert_eq!(
        d.apply(
            Command::SetNotes {
                pattern_id: ghost,
                notes: vec![]
            },
            false
        ),
        Err(NoSuchPattern)
    );
    assert_eq!(
        d.apply(Command::AddPattern { track_id: ghost }, false),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::AddClip {
                pattern_id: ghost,
                start_tick: 0,
                length_ticks: None
            },
            false
        ),
        Err(NoSuchPattern)
    );

    let stray = Clip::new(p0, 0, 4 * BEAT);
    assert_eq!(
        d.apply(
            Command::UpdateClip {
                clip: stray.clone()
            },
            false
        ),
        Err(NoSuchClip)
    );
    assert_eq!(
        d.apply(Command::UpdateClips { clips: vec![stray] }, false),
        Err(NoSuchClip)
    );
    assert_eq!(
        d.apply(Command::RemoveClip { id: ghost }, false),
        Err(NoSuchClip)
    );
    assert_eq!(
        d.apply(Command::MakeClipUnique { id: ghost }, false),
        Err(NoSuchClip)
    );
}

#[test]
fn non_finite_and_out_of_range_values_are_rejected() {
    let mut d = doc();
    let (t0, _, p0, _) = ids(d.model());
    use EditError::InvalidValue;

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            d.apply(Command::SetTempo { bpm: bad }, false),
            Err(InvalidValue)
        );
        assert_eq!(
            d.apply(
                Command::SetTrackParam {
                    id: t0,
                    param: TrackParam::Volume,
                    value: bad
                },
                false
            ),
            Err(InvalidValue)
        );
        assert_eq!(
            d.apply(
                Command::SetSwing {
                    swing: bad,
                    grid_ticks: BEAT / 2
                },
                false
            ),
            Err(InvalidValue)
        );
        assert_eq!(
            d.apply(
                Command::SetMaster {
                    master: MasterSettings {
                        gain: bad,
                        ..MasterSettings::default()
                    }
                },
                false
            ),
            Err(InvalidValue)
        );
    }

    // A pattern shorter than one beat is not a pattern.
    assert_eq!(
        d.apply(Command::SetPatternLength { id: p0, ticks: 0 }, false),
        Err(InvalidValue)
    );
    assert_eq!(
        d.apply(
            Command::SetPatternLength {
                id: p0,
                ticks: -4 * BEAT
            },
            false
        ),
        Err(InvalidValue)
    );
    assert_eq!(
        d.apply(
            Command::SetPatternLength {
                id: p0,
                ticks: BEAT - 1
            },
            false
        ),
        Err(InvalidValue)
    );

    // Colors must be `#rrggbb`.
    for bad in ["red", "#fff", "#ggghhh", "", "#1234567"] {
        assert_eq!(
            d.apply(
                Command::SetTrackColor {
                    id: t0,
                    color: Some(bad.into())
                },
                false
            ),
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
            d.apply(
                Command::RenamePattern {
                    id: p0,
                    name: blank.into()
                },
                false
            ),
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
            d.apply(
                Command::RenameTrack {
                    id: t0,
                    name: blank.into()
                },
                false
            ),
            Err(EditError::InvalidValue),
            "track accepted the blank name {blank:?}"
        );
        assert_eq!(d.model().tracks[0].name, original);
    }
    assert!(!d.can_undo(), "a refused rename recorded an undo step");
    // A real name still goes through, including one with surrounding whitespace.
    d.apply(
        Command::RenameTrack {
            id: t0,
            name: " lead ".into(),
        },
        false,
    )
    .unwrap();
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
        d.apply(
            Command::SetTrackParam {
                id: t0,
                param,
                value,
            },
            false,
        )
        .unwrap();
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
                ceiling_db: 6.0,
                limiter: false,
            },
        },
        false,
    )
    .unwrap();
    let m = &d.model().master;
    assert_eq!((m.gain, m.reverb_wet), (1.5, 1.0));
    assert_eq!((m.low_cut_hz, m.high_cut_hz), (10.0, 20_000.0));
    assert_eq!((m.low_eq, m.mid_eq, m.high_eq), (0.0, 2.0, 0.0));
    assert_eq!((m.ceiling_db, m.limiter), (0.0, false));
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
        Command::RenameTrack {
            id: ghost,
            name: "x".into(),
        },
        Command::RenamePattern {
            id: p0,
            name: "   ".into(),
        },
        Command::SetPatternLength {
            id: p0,
            ticks: BEAT / 2,
        },
        Command::SetTrackColor {
            id: t0,
            color: Some("nope".into()),
        },
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Tone,
            value: f64::NAN,
        },
        Command::DuplicatePattern { id: ghost },
        Command::SetNotes {
            pattern_id: ghost,
            notes: vec![NoteEvent::new(0, BEAT, 60)],
        },
        Command::SetMaster {
            master: MasterSettings {
                high_eq: f64::NAN,
                ..MasterSettings::default()
            },
        },
        Command::AddClip {
            pattern_id: p0,
            start_tick: 500 * BEAT,
            length_ticks: None,
        },
        Command::UpdateClips {
            clips: vec![
                Clip {
                    start_tick: 4 * BEAT,
                    ..baseline.clips[0].clone()
                },
                Clip::new(p0, 0, 4 * BEAT),
            ],
        },
        Command::ImportTracks {
            tracks: vec![],
            length_ticks: 16 * BEAT,
        },
    ];
    for command in refusals {
        assert!(
            d.apply(command.clone(), false).is_err(),
            "{command:?} was expected to be refused"
        );
        assert_eq!(
            d.model(),
            &baseline,
            "{command:?} mutated the model despite being refused"
        );
        assert!(
            d.can_redo(),
            "{command:?} cleared the redo stack despite being refused"
        );
        assert!(
            !d.is_dirty(),
            "{command:?} marked the document dirty despite being refused"
        );
    }
    assert!(!d.can_undo(), "refused commands pushed undo entries");
}

#[test]
fn a_refused_transient_command_does_not_open_a_gesture() {
    let mut d = doc();
    let (t0, ..) = ids(d.model());
    assert!(d
        .apply(
            Command::SetTrackParam {
                id: t0,
                param: TrackParam::Volume,
                value: f64::NAN
            },
            true
        )
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
    d.apply(Command::AddPattern { track_id: t0 }, false)
        .unwrap();
    let second = d.model().patterns_of(&t0).nth(1).unwrap().id;
    d.apply(
        Command::AddClip {
            pattern_id: second,
            start_tick: 32 * BEAT,
            length_ticks: None,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetNotes {
            pattern_id: p1,
            notes: vec![NoteEvent::new(0, BEAT / 4, 36)],
        },
        false,
    )
    .unwrap();
    let before = d.model().clone();

    d.apply(Command::DeleteTrack { id: t0 }, false).unwrap();
    let m = d.model();
    assert!(
        m.patterns.iter().all(|p| p.track_id != t0),
        "a pattern of the deleted track survived"
    );
    assert!(m.pattern(&p0).is_none() && m.pattern(&second).is_none());
    assert!(
        m.clips.iter().all(|c| c.pattern_id == p1),
        "a clip of the deleted track survived"
    );
    assert_eq!(m.clips.len(), 1);
    assert_eq!(
        m.pattern(&p1).unwrap().notes.len(),
        1,
        "the surviving track lost its notes"
    );

    // And undo brings every pattern and clip back.
    assert!(d.undo());
    assert_eq!(d.model(), &before);
}

// ---------------------------------------------------------------- sounds and drum kits

fn drum_sound(id: &str) -> Sound {
    Sound {
        id: id.into(),
        name: id.into(),
        category: "kick".into(),
        patch: SoundPatch::Drum(DrumPatch::default()),
    }
}

fn pad(pitch: u8, sound: &str, choke: u8) -> DrumPad {
    DrumPad {
        pitch,
        sound: sound.into(),
        choke,
    }
}

fn embedded(m: &ProjectModel) -> Vec<&str> {
    m.sounds.iter().map(|s| s.id.as_str()).collect()
}

/// `command` is refused with exactly `error`, and the document is left as it was.
fn refused(d: &mut Document, command: Command, error: EditError) {
    let before = d.model().clone();
    let could_undo = d.can_undo();
    assert_eq!(d.apply(command.clone(), false), Err(error), "{command:?}");
    assert_eq!(
        d.model(),
        &before,
        "{command:?} mutated the model despite being refused"
    );
    assert_eq!(
        d.can_undo(),
        could_undo,
        "{command:?} pushed an undo entry despite being refused"
    );
}

#[test]
fn set_track_sound_embeds_a_user_sound_and_a_factory_sound_prunes_it() {
    let mut d = doc();
    let (t0, t1, ..) = ids(d.model());
    let start = d.model().clone();

    // A new user sound replaces the old one on the track; the old one, now unused, is dropped.
    // The embedded copy is sanitized like everything else that enters the model.
    let other = Sound {
        id: "u-other".into(),
        name: "  ".into(),
        ..user_sound(9.0)
    };
    d.apply(
        Command::SetTrackSound {
            id: t0,
            sound: other.clone(),
        },
        false,
    )
    .unwrap();
    assert_eq!(d.model().tracks[0].voice, "u-other");
    assert_eq!(
        embedded(d.model()),
        ["u-other"],
        "the unused u-test was not pruned"
    );
    assert_eq!(d.model().sounds[0].name, "untitled");
    assert_eq!(
        d.model().resolve_synth("u-other").level,
        1.0,
        "the embedded copy was stored unclamped"
    );

    // Sending the same id again replaces the copy in place.
    let quieter = Sound {
        patch: SoundPatch::Synth(SynthPatch {
            level: 0.5,
            ..SynthPatch::default()
        }),
        ..other
    };
    d.apply(
        Command::SetTrackSound {
            id: t0,
            sound: quieter,
        },
        false,
    )
    .unwrap();
    assert_eq!(embedded(d.model()), ["u-other"]);
    assert_eq!(d.model().resolve_synth("u-other").level, 0.5);

    // Back to a factory sound: nothing is embedded, and the file carries no `sounds` key.
    d.apply(
        Command::SetTrackSound {
            id: t0,
            sound: factory_sound("saw").unwrap().clone(),
        },
        false,
    )
    .unwrap();
    assert_eq!(d.model().tracks[0].voice, "saw");
    assert!(
        d.model().sounds.is_empty(),
        "a factory sound was embedded or a user sound kept"
    );
    assert!(
        !d.model().to_json().unwrap().contains("\"sounds\""),
        "an empty sounds list was written"
    );

    // Wrong kind either way round, and an unknown track, change nothing.
    refused(
        &mut d,
        Command::SetTrackSound {
            id: t1,
            sound: user_sound(0.5),
        },
        EditError::InvalidValue,
    );
    refused(
        &mut d,
        Command::SetTrackSound {
            id: t0,
            sound: drum_sound("u-boom"),
        },
        EditError::InvalidValue,
    );
    refused(
        &mut d,
        Command::SetTrackSound {
            id: Uuid::new_v4(),
            sound: user_sound(0.5),
        },
        EditError::NoSuchTrack,
    );

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
    let seventeen: Vec<DrumPad> = (0..=MAX_DRUM_PADS as u8)
        .map(|i| pad(30 + i, "kick", 0))
        .collect();
    assert_eq!(seventeen.len(), 17);
    let kit = |pads: Vec<DrumPad>| Command::SetDrumPads {
        id: t1,
        pads,
        sounds: vec![],
    };
    for (command, error) in [
        (kit(vec![]), EditError::InvalidValue),
        (kit(seventeen.clone()), EditError::InvalidValue),
        (
            kit(vec![pad(36, "kick", 0), pad(36, "snare", 0)]),
            EditError::InvalidValue,
        ),
        (kit(vec![pad(128, "kick", 0)]), EditError::InvalidValue),
        // A synth sound carried along to embed, a pad on a factory synth sound, and a pad on the
        // project's embedded synth sound.
        (
            Command::SetDrumPads {
                id: t1,
                pads: vec![pad(36, "u-test", 0)],
                sounds: vec![user_sound(0.5)],
            },
            EditError::InvalidValue,
        ),
        (kit(vec![pad(36, "saw", 0)]), EditError::NoSuchSound),
        (kit(vec![pad(36, "u-test", 0)]), EditError::NoSuchSound),
        // A kit on a synth track, and on no track at all.
        (
            Command::SetDrumPads {
                id: t0,
                pads: default_kit(),
                sounds: vec![],
            },
            EditError::InvalidValue,
        ),
        (
            Command::SetDrumPads {
                id: Uuid::new_v4(),
                pads: default_kit(),
                sounds: vec![],
            },
            EditError::NoSuchTrack,
        ),
    ] {
        refused(&mut d, command, error);
    }
    assert_eq!(d.model().tracks[1].drum_pads, default_kit());
    assert!(!d.can_undo());

    // Sixteen is the limit, not one past it.
    d.apply(kit(seventeen[..MAX_DRUM_PADS].to_vec()), false)
        .unwrap();
    assert_eq!(d.model().tracks[1].drum_pads.len(), MAX_DRUM_PADS);
}

#[test]
fn a_pad_on_an_unknown_sound_is_refused_and_embeds_nothing() {
    let mut d = doc();
    let (_, t1, ..) = ids(d.model());
    // The carried sound would be embedded on the way, but the refusal throws the whole edit away.
    refused(
        &mut d,
        Command::SetDrumPads {
            id: t1,
            pads: vec![pad(36, "u-boom", 0), pad(38, "u-missing", 0)],
            sounds: vec![drum_sound("u-boom")],
        },
        EditError::NoSuchSound,
    );
    assert_eq!(embedded(d.model()), ["u-test"]);
}

#[test]
fn drum_pads_embed_their_user_sounds_and_deleting_a_track_prunes_its_sounds() {
    let mut d = doc();
    let (t0, t1, ..) = ids(d.model());
    let pads = vec![
        pad(36, "u-boom", 0),
        pad(42, "hat", 1),
        pad(46, "open-hat", 1),
    ];
    let sounds = vec![
        drum_sound("u-boom"),
        drum_sound("u-unused"),
        factory_sound("hat").unwrap().clone(),
    ];
    d.apply(
        Command::SetDrumPads {
            id: t1,
            pads: pads.clone(),
            sounds,
        },
        false,
    )
    .unwrap();
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
    d.apply(Command::AddTrack { is_drum: false }, false)
        .unwrap();
    let t2 = d.model().tracks.last().unwrap().id;
    d.apply(
        Command::SetTrackVoice {
            id: t2,
            voice: "u-test".into(),
        },
        false,
    )
    .unwrap();
    d.apply(Command::DeleteTrack { id: t0 }, false).unwrap();
    assert_eq!(embedded(d.model()), ["u-test", "u-boom"]);
}

#[test]
fn upsert_sound_only_replaces_a_copy_the_project_embeds() {
    let mut d = doc();
    refused(
        &mut d,
        Command::UpsertSound {
            sound: Sound {
                id: "u-nope".into(),
                ..user_sound(0.5)
            },
        },
        EditError::NoSuchSound,
    );
    refused(
        &mut d,
        Command::UpsertSound {
            sound: factory_sound("saw").unwrap().clone(),
        },
        EditError::NoSuchSound,
    );
    // The embedded u-test is a synth; a drum patch under its id is the wrong kind.
    refused(
        &mut d,
        Command::UpsertSound {
            sound: drum_sound("u-test"),
        },
        EditError::InvalidValue,
    );

    d.apply(
        Command::UpsertSound {
            sound: user_sound(0.6),
        },
        false,
    )
    .unwrap();
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
    for (pattern_id, start) in [
        (p0, 0),
        (p1, 16 * BEAT),
        (p0, 32 * BEAT),
        (p0, 48 * BEAT),
        (p1, 64 * BEAT),
    ] {
        d.apply(
            Command::AddClip {
                pattern_id,
                start_tick: start,
                length_ticks: None,
            },
            false,
        )
        .unwrap();
    }
    d.apply(Command::DeletePattern { id: p0 }, false).unwrap();
    assert_eq!(
        d.model()
            .clips
            .iter()
            .map(|c| c.pattern_id)
            .collect::<Vec<_>>(),
        vec![p1, p1],
        "clips of the deleted pattern survived"
    );
    assert!(d.model().pattern(&p0).is_none());
}

#[test]
fn duplicating_a_pattern_deep_copies_its_notes_on_the_same_track_with_fresh_ids() {
    let mut d = doc();
    let (t0, _, p0, _) = ids(d.model());
    d.apply(Command::DuplicatePattern { id: p0 }, false)
        .unwrap();

    let original = d.model().patterns[0].clone();
    let copy = d.model().patterns[1].clone();
    assert_ne!(original.id, copy.id);
    assert_eq!(copy.track_id, t0, "the copy belongs to the same track");
    assert_eq!(copy.name, format!("{} copy", original.name));
    assert_eq!(copy.length_ticks, original.length_ticks);

    assert_eq!(
        original.notes.len(),
        3,
        "fixture should have three melody notes"
    );
    assert_eq!(
        copy.notes.len(),
        original.notes.len(),
        "the copy lost notes"
    );
    for n in &copy.notes {
        assert!(
            !original.notes.iter().any(|o| o.id == n.id),
            "copy shares note id {} with the original",
            n.id
        );
    }
    // Musical content is identical even though the ids are not.
    let musical = |p: &SongPattern| {
        p.notes
            .iter()
            .map(|n| (n.start_tick, n.pitch, n.velocity))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        musical(&original),
        musical(&copy),
        "duplicated notes differ musically"
    );
    // The rope comes along, pointing at the copy of its source rather than back into the
    // original pattern.
    assert_eq!(copy.notes[2].slide_from, Some(copy.notes[1].id));

    // Editing the copy must not reach back into the original.
    d.apply(
        Command::SetNotes {
            pattern_id: copy.id,
            notes: vec![],
        },
        false,
    )
    .unwrap();
    assert_eq!(
        d.model().patterns[0].notes.len(),
        3,
        "editing the copy mutated the original"
    );
}

#[test]
fn the_song_only_ever_holds_clips_of_live_patterns() {
    let mut d = doc();
    let (.., p0, p1) = ids(d.model());
    d.apply(
        Command::AddClip {
            pattern_id: p0,
            start_tick: 0,
            length_ticks: None,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::AddClip {
            pattern_id: p1,
            start_tick: 16 * BEAT,
            length_ticks: None,
        },
        false,
    )
    .unwrap();
    // A clip can only be placed for a pattern that exists.
    assert_eq!(
        d.apply(
            Command::AddClip {
                pattern_id: Uuid::new_v4(),
                start_tick: 32 * BEAT,
                length_ticks: None
            },
            false
        ),
        Err(EditError::NoSuchPattern)
    );

    let live: Vec<Uuid> = d.model().patterns.iter().map(|p| p.id).collect();
    for clip in &d.model().clips {
        assert!(
            live.contains(&clip.pattern_id),
            "clip points at a pattern that is gone"
        );
    }
}

// ---------------------------------------------------------------- 5. sequence flattening

#[test]
fn from_pattern_uses_pattern_local_positions_and_honours_pattern_length() {
    let m = fixture();
    let (t0, t1, p0, p1) = ids(&m);

    let seq = Sequence::from_pattern(&m, &p0, 0).expect("pattern 1 exists");
    assert_eq!(seq.length_ticks, 16 * BEAT);
    assert_eq!(seq.tempo_bpm, m.tempo);
    assert_eq!(seq.tracks.len(), m.tracks.len());
    assert_eq!(seq.tracks[0].id, t0);
    let starts: Vec<i64> = seq.tracks[0].notes.iter().map(|n| n.start_tick).collect();
    assert_eq!(
        starts,
        vec![0, 4 * BEAT, 6 * BEAT],
        "pattern-mode notes must keep their pattern-local ticks"
    );
    assert!(
        seq.tracks[1].notes.is_empty(),
        "track {t1} is not this pattern's track"
    );
    assert_eq!(
        seq.chords.chords().len(),
        4,
        "the progression's first 16 beats"
    );

    let short = Sequence::from_pattern(&m, &p1, 0).unwrap();
    assert_eq!(short.length_ticks, 8 * BEAT);
    assert!(short.tracks.iter().all(|t| t.notes.is_empty()));
    assert_eq!(
        short.chords.end_tick(),
        8 * BEAT,
        "chords are cut at the pattern end"
    );

    assert!(
        Sequence::from_pattern(&m, &Uuid::new_v4(), 0).is_none(),
        "unknown pattern id must be None"
    );
}

/// Pattern mode tiers against the bar the pattern was opened at: a clip at song beat 8 sees
/// the song's chords from beat 8, starting at its own beat 0.
#[test]
fn pattern_mode_chords_follow_the_chord_offset() {
    let mut m = fixture();
    let (_, _, p0, _) = ids(&m);
    m.chords = ChordTrack::new(vec![
        ChordEvent::new(0, 8 * BEAT, vec![0, 4, 7], Some("C".into())),
        ChordEvent::new(8 * BEAT, 8 * BEAT, vec![9, 0, 4], Some("Am".into())),
        ChordEvent::new(16 * BEAT, 16 * BEAT, vec![5, 9, 0], Some("F".into())),
    ]);
    let names = |offset: i64| -> Vec<(i64, i64, String)> {
        let seq = Sequence::from_pattern(&m, &p0, offset).unwrap();
        seq.chords
            .chords()
            .iter()
            .map(|c| (c.start_tick, c.length_ticks, c.name.clone().unwrap()))
            .collect()
    };
    assert_eq!(
        names(0),
        vec![(0, 8 * BEAT, "C".into()), (8 * BEAT, 8 * BEAT, "Am".into())]
    );
    assert_eq!(
        names(8 * BEAT),
        vec![(0, 8 * BEAT, "Am".into()), (8 * BEAT, 8 * BEAT, "F".into())]
    );
    assert_eq!(
        names(20 * BEAT),
        vec![(0, 12 * BEAT, "F".into())],
        "mid-chord offset, cut at the progression end"
    );
    // The notes do not move with the offset; only the harmony under them does.
    let notes = |offset: i64| {
        Sequence::from_pattern(&m, &p0, offset).unwrap().tracks[0]
            .notes
            .clone()
    };
    assert_eq!(notes(0), notes(8 * BEAT));
}

#[test]
fn song_flattening_produces_absolute_tick_positions() {
    let mut m = fixture();
    let (_, t1, p0, p1) = ids(&m);
    // p0 is 16 beats with notes at 0, 4 and 6; melody 2 is 8 beats with a note at 2; the drums
    // play under all of it from their own pattern.
    let p2 = add_melody_pattern(&mut m, 8 * BEAT, vec![NoteEvent::new(2 * BEAT, BEAT, 67)]);
    m.patterns[1].notes = vec![NoteEvent::new(0, BEAT / 4, 36)];
    m.clips = vec![
        clip_at(p0, 0, 16 * BEAT),
        clip_at(p2, 16 * BEAT, 8 * BEAT),
        clip_at(p0, 24 * BEAT, 16 * BEAT),
        clip_at(p1, 0, 40 * BEAT),
    ];

    let seq = Sequence::from_song(&m);
    assert_eq!(
        seq.length_ticks, m.song_length_ticks,
        "the song ends at its explicit length"
    );

    let starts: Vec<i64> = seq.tracks[0].notes.iter().map(|n| n.start_tick).collect();
    // p0 @0 → 0,4,6 ; melody 2 @16 → 18 ; p0 @24 → 24,28,30
    assert_eq!(
        starts,
        vec![0, 4 * BEAT, 6 * BEAT, 18 * BEAT, 24 * BEAT, 28 * BEAT, 30 * BEAT]
    );
    // Each placement of p0 ropes its own third note to its own second.
    let notes = &seq.tracks[0].notes;
    assert_eq!(notes[2].slide_from, Some(notes[1].id));
    assert_eq!(notes[6].slide_from, Some(notes[5].id));
    let drums: Vec<i64> = seq.tracks[1].notes.iter().map(|n| n.start_tick).collect();
    assert_eq!(
        drums,
        vec![0, 8 * BEAT, 16 * BEAT, 24 * BEAT, 32 * BEAT],
        "track {t1} loops its own 8-beat pattern"
    );

    // Repeated occurrences get distinct but deterministic ids.
    let ids: Vec<Uuid> = seq.tracks[0].notes.iter().map(|n| n.id).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "repeated pattern occurrences reused note ids"
    );
    assert_eq!(
        ids,
        Sequence::from_song(&m).tracks[0]
            .notes
            .iter()
            .map(|n| n.id)
            .collect::<Vec<_>>()
    );

    // The chords are the project's own, in song ticks.
    assert_eq!(seq.chords, m.chords);
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
            Sequence::from_pattern(&m, &pid, 0).unwrap().length_ticks >= BEAT,
            "pattern mode produced an unplayable loop for lengthBeats {len}"
        );
        assert!(
            Sequence::from_song(&m).length_ticks >= 4 * BEAT,
            "song mode produced an unplayable loop ({}) for songLengthBeats {len}",
            Sequence::from_song(&m).length_ticks
        );
    }
}

#[test]
fn a_song_that_is_nothing_but_audio_loops_over_the_audio() {
    // Without this the loop would stop at the song's end, cutting off a take that runs past it.
    let mut m = fixture();
    m.clips = vec![];
    m.song_length_ticks = 4 * BEAT;
    m.tempo = 120.0;
    let gid = audio_track(&m);
    let source = m.audio_sources[0].id;
    // 6 seconds at 120 bpm is 12 beats.
    m.audio_clips = vec![AudioClip::new(gid, source, 0, 6.0)];
    let seq = Sequence::from_song(&m);
    assert_eq!(seq.length_ticks, 12 * BEAT);
    assert_eq!(seq.audio.len(), 1);
    assert_eq!(seq.audio[0].start_seconds, 0.0);

    // The song's own length wins once it is the longer of the two.
    m.song_length_ticks = 16 * BEAT;
    assert_eq!(Sequence::from_song(&m).length_ticks, 16 * BEAT);

    // A muted clip is left out of the sequence entirely rather than rendered silently.
    m.audio_clips[0].muted = true;
    assert!(Sequence::from_song(&m).audio.is_empty());

    // Pattern mode never carries audio: audio lives on the song timeline.
    m.audio_clips[0].muted = false;
    let pattern = Sequence::from_pattern(&m, &m.patterns[0].id, 0).unwrap();
    assert!(pattern.audio.is_empty());
}

#[test]
fn song_flattening_survives_empty_and_degenerate_input() {
    // Empty song: no notes, but the song is still as long as it says and keeps its chords.
    let mut m = fixture();
    m.clips = vec![];
    m.audio_clips = vec![];
    let seq = Sequence::from_song(&m);
    assert!(seq.tracks.iter().all(|t| t.notes.is_empty()));
    assert_eq!(seq.chords, m.chords);
    assert_eq!(seq.length_ticks, m.song_length_ticks);

    // Clips of patterns that are gone: loading drops them.
    m.clips = vec![
        clip_at(Uuid::new_v4(), 0, 16 * BEAT),
        clip_at(Uuid::new_v4(), 16 * BEAT, 16 * BEAT),
    ];
    let m = ProjectModel::from_json(&m.to_json().unwrap()).unwrap();
    assert!(
        m.clips.is_empty(),
        "clips of deleted patterns survived loading"
    );
    let seq = Sequence::from_song(&m);
    assert!(seq.tracks.iter().all(|t| t.notes.is_empty()));
    let mut m = m;

    // A dead id in the middle must not shift the patterns after it.
    let (_, _, p0, _) = ids(&m);
    let p2 = add_melody_pattern(&mut m, 16 * BEAT, vec![NoteEvent::new(0, BEAT, 67)]);
    m.clips = vec![clip_at(p0, 0, 16 * BEAT), clip_at(p2, 16 * BEAT, 16 * BEAT)];
    let seq = Sequence::from_song(&m);
    let starts: Vec<i64> = seq.tracks[0].notes.iter().map(|n| n.start_tick).collect();
    assert_eq!(Arrangement::total_length(&m.clips), 32 * BEAT);
    assert!(
        starts.contains(&(16 * BEAT)),
        "the pattern after a dead id was misplaced: {starts:?}"
    );

    // A pattern with no notes and a project with no chords: empty, not a panic.
    let mut bare = ProjectModel::empty();
    let bare_p = bare.patterns[0].id;
    bare.clips = vec![clip_at(bare_p, 0, 16 * BEAT)];
    let seq = Sequence::from_song(&bare);
    assert_eq!(seq.length_ticks, bare.song_length_ticks);
    assert!(seq.chords.is_empty());
    assert!(seq.tracks.iter().all(|t| t.notes.is_empty()));
    assert!(!seq.any_solo());
}

#[test]
fn sequence_carries_mute_and_solo_so_audibility_is_decidable() {
    let mut d = doc();
    let (t0, t1, p0, _) = ids(d.model());
    d.apply(
        Command::SetTrackMuted {
            id: t0,
            muted: true,
        },
        false,
    )
    .unwrap();
    let seq = Sequence::from_pattern(d.model(), &p0, 0).unwrap();
    assert!(!seq.any_solo());
    assert!(
        !seq.tracks[0].audible(false),
        "a muted track must not be audible"
    );
    assert!(seq.tracks[1].audible(false));

    d.apply(
        Command::SetTrackSoloed {
            id: t1,
            soloed: true,
        },
        false,
    )
    .unwrap();
    let seq = Sequence::from_pattern(d.model(), &p0, 0).unwrap();
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
    d.apply(
        Command::SetKey {
            key: KeyState::locked(9, ScaleType::Minor),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::RenameTrack {
            id: t0,
            name: "lead".into(),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetTrackVoice {
            id: t0,
            voice: "pluck".into(),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetTrackColor {
            id: t0,
            color: Some("#AABBCC".into()),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetTrackParam {
            id: t0,
            param: TrackParam::Pan,
            value: -0.42,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetTrackMuted {
            id: t1,
            muted: true,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetPatternLength {
            id: p1,
            ticks: 12 * BEAT + BEAT / 2,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::RenamePattern {
            id: p1,
            name: "chorus".into(),
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetNotes {
            pattern_id: p1,
            notes: vec![
                NoteEvent::new(3 * BEAT / 2, BEAT / 4, 36).with_velocity(1),
                NoteEvent::new(2 * BEAT, BEAT / 2, 42),
            ],
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetChords {
            chords: vec![ChordEvent::new(0, 4 * BEAT, vec![9, 0, 4], None)],
        },
        false,
    )
    .unwrap();
    d.apply(Command::SetSongLength { ticks: 96 * BEAT }, false)
        .unwrap();
    for (pattern_id, start) in [(p1, 0), (p0, 16 * BEAT), (p1, 32 * BEAT)] {
        d.apply(
            Command::AddClip {
                pattern_id,
                start_tick: start,
                length_ticks: None,
            },
            false,
        )
        .unwrap();
    }
    d.apply(Command::AddPattern { track_id: t0 }, false)
        .unwrap();
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
                ceiling_db: -1.0,
                limiter: false,
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
    assert_eq!(
        ProjectModel::from_json(&loaded.to_json().unwrap()).unwrap(),
        saved
    );

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
    assert_eq!(
        m.song_length_ticks,
        DEFAULT_SONG_BARS as i64 * 4 * BEAT,
        "missing songLengthTicks is the default song"
    );

    let t = &m.tracks[0];
    assert_eq!(t.voice, "saw");
    assert_eq!(t.volume, 1.0);
    assert_eq!(t.tone, 18_000.0);
    assert_eq!((t.pan, t.reverb_send), (0.0, 0.0));
    assert_eq!((t.muted, t.soloed, t.is_drum), (false, false, false));
    assert_eq!(t.color, None);

    let p = &m.patterns[0];
    assert_eq!(
        p.length_ticks,
        16 * BEAT,
        "missing lengthBeats must default to 16 beats of ticks"
    );
    assert_eq!(p.track_id, t.id);

    // A note with no velocity plays at the documented default, and gets a real id.
    let note = &p.notes[0];
    assert_eq!(
        note.velocity, 100,
        "a note without velocity must default to 100"
    );
    assert_ne!(
        note.id,
        Uuid::nil(),
        "a note without an id must get a fresh one"
    );
}

#[test]
fn an_empty_or_stub_file_still_opens_as_a_usable_project() {
    for json in [
        r#"{"schemaVersion": 6}"#,
        r#"{"schemaVersion": 6, "tracks": []}"#,
        r#"{"schemaVersion": 6, "patterns": []}"#,
        r#"{"schemaVersion": 6, "tracks": [], "patterns": []}"#,
    ] {
        let m =
            ProjectModel::from_json(json).unwrap_or_else(|e| panic!("{json} failed to open: {e}"));
        assert!(
            !m.tracks.is_empty(),
            "{json} produced a project with no tracks"
        );
        assert!(
            !m.patterns.is_empty(),
            "{json} produced a project with nothing to write into"
        );
        // And a usable project is one you can immediately play and edit.
        assert!(Sequence::from_pattern(&m, &m.patterns[0].id, 0).is_some());
        let mut d = Document::new(m);
        let p0 = d.model().patterns[0].id;
        d.apply(
            Command::SetNotes {
                pattern_id: p0,
                notes: vec![NoteEvent::new(0, BEAT, 60)],
            },
            false,
        )
        .unwrap();
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
        r#"{ "schemaVersion": 6, "chords": [ { "startBeat": 0 } ] }"#,
        r#"{ "schemaVersion": 6, "songLengthTicks": "long" }"#,
        "\u{feff}{}",
        deep.as_str(),
    ];
    for json in garbage {
        let short: String = json.chars().take(40).collect();
        assert!(
            ProjectModel::from_json(json).is_err(),
            "{short:?} was accepted as a project file"
        );
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
    assert_eq!(
        ProjectModel::from_json(r#"{"schemaVersion": 6}"#)
            .unwrap()
            .tempo,
        120.0
    );
}

#[test]
fn files_from_another_schema_are_refused_rather_than_converted() {
    // Opening a future file here would drop every field this build doesn't know, and the next
    // Save would overwrite the user's richer project with the lossy version. Refuse instead.
    let future = SCHEMA_VERSION + 96;
    let json =
        format!(r#"{{ "schemaVersion": {future}, "tempo": 100, "unknownFutureField": [1,2,3] }}"#);
    let err = ProjectModel::from_json(&json).expect_err("a file from the future was opened anyway");
    assert!(
        err.to_string().contains("newer version"),
        "the error should tell the user why: {err}"
    );
    // Anything older opens: a pre-v10 file whose patterns held every track's notes is split
    // into one pattern per track instead of being refused.
    for version in [0, 1, 2, 3, 4, 5] {
        let json = format!(r#"{{ "schemaVersion": {version}, "tempo": 100 }}"#);
        let m = ProjectModel::from_json(&json).expect("an old file must still open");
        assert_eq!(m.schema_version, SCHEMA_VERSION);
    }
    let json = format!(r#"{{ "schemaVersion": {SCHEMA_VERSION}, "tempo": 100 }}"#);
    assert_eq!(
        ProjectModel::from_json(&json).unwrap().schema_version,
        SCHEMA_VERSION
    );
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
    assert_eq!(
        m.patterns[0].notes[0].pitch, 60,
        "the live track's notes are untouched"
    );
    assert_eq!(m.clips.len(), 1, "the orphan's clip survived loading");
}

#[test]
fn json_is_camel_case_on_every_nested_field() {
    let json = fixture().to_json().unwrap();
    for key in [
        "schemaVersion",
        "trackId",
        "lengthTicks",
        "startTick",
        "pitchClasses",
        "isDrum",
        "songLengthTicks",
    ] {
        assert!(
            json.contains(&format!("\"{key}\"")),
            "missing camelCase key {key}"
        );
    }
    for snake in [
        "length_ticks",
        "start_tick",
        "track_id",
        "pitch_classes",
        "is_drum",
        "reverb_send",
        "song_length_ticks",
    ] {
        assert!(
            !json.contains(snake),
            "snake_case key {snake} leaked into the file"
        );
    }
    assert!(
        !json.contains("notesByTrack"),
        "the schema-5 layout leaked into the file"
    );
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
    assert!(
        d.is_dirty(),
        "undoing past the save point must mark the document dirty"
    );
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
    assert!(
        d.is_dirty(),
        "undoing away from the saved state is an unsaved change"
    );
    assert!(d.redo());
    assert_eq!(
        d.model(),
        &on_disk,
        "undo+redo did not return to the saved state"
    );
    assert!(
        !d.is_dirty(),
        "the document matches the file on disk but still reports changes"
    );

    // A gesture that lands back on its starting value is likewise not an unsaved change.
    let t0 = d.model().tracks[0].id;
    for v in [0.5, 0.2, 1.0] {
        d.apply(
            Command::SetTrackParam {
                id: t0,
                param: TrackParam::Volume,
                value: v,
            },
            true,
        )
        .unwrap();
    }
    d.commit_gesture();
    assert!(
        !d.is_dirty(),
        "a round-trip slider drag reported unsaved changes"
    );
}

// ---------------------------------------------------------------- 7. track plugins

/// The uid the fixture's melody track is loaded with.
fn fixture_uid() -> String {
    fixture().tracks[0].plugin.as_ref().unwrap().uid.clone()
}

fn vital() -> TrackPlugin {
    TrackPlugin {
        uid: "clap:/CLAP/Vital.clap:audio.vital.synth".into(),
        name: "Vital".into(),
        state: None,
    }
}

#[test]
fn a_track_plugin_survives_a_save_and_load_with_its_state_blob() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    d.apply(
        Command::SetPluginState {
            id: t0,
            state: "QUJDRA==".into(),
        },
        false,
    )
    .unwrap();

    let saved = d.model().clone();
    let loaded =
        ProjectModel::from_json(&saved.to_json().unwrap()).expect("a file we just wrote must load");
    let plugin = loaded.tracks[0]
        .plugin
        .as_ref()
        .expect("the track lost its plugin on reload");
    assert_eq!(plugin.uid, fixture_uid());
    assert_eq!(
        plugin.name, "Surge XT",
        "the cached name must survive, so a missing plugin is still nameable"
    );
    assert_eq!(
        plugin.state.as_deref(),
        Some("QUJDRA=="),
        "the plugin's own state did not survive the file"
    );
    assert_eq!(loaded, saved);
}

#[test]
fn a_file_written_before_plugins_existed_opens_with_no_plugin() {
    // A v6 track: every field this build knows except `plugin`. It must open playing its own
    // built-in voice rather than refusing to decode.
    let track = "11111111-1111-1111-1111-111111111111";
    let json = format!(
        r#"{{
          "schemaVersion": 6, "tempo": 120,
          "tracks": [ {{ "id": "{track}", "name": "lead", "voice": "pluck", "muted": false, "soloed": false,
            "isDrum": false, "volume": 1, "reverbSend": 0, "tone": 18000, "pan": 0 }} ],
          "patterns": [ {{ "name": "verse", "lengthBeats": 16 }} ]
        }}"#
    );
    let m = ProjectModel::from_json(&json).expect("a pre-plugin file must still open");
    assert_eq!(m.schema_version, SCHEMA_VERSION);
    assert_eq!(
        m.tracks[0].plugin, None,
        "a file with no plugin key must decode as no plugin"
    );
    assert_eq!(
        m.tracks[0].voice, "pluck",
        "the built-in voice must be left alone"
    );
}

#[test]
fn switching_to_a_different_plugin_drops_the_previous_plugins_state() {
    // A state blob is only meaningful to the plugin that wrote it: handing Surge's bytes to
    // Vital is at best ignored and at worst a crash inside someone else's code.
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    let before = d.model().tracks[0].plugin.clone().unwrap();
    assert!(
        before.state.is_some(),
        "fixture must start with a state blob for this to mean anything"
    );

    d.apply(
        Command::SetTrackPlugin {
            id: t0,
            plugin: Some(vital()),
        },
        false,
    )
    .unwrap();
    let now = d.model().tracks[0].plugin.as_ref().unwrap();
    assert_eq!(now.uid, vital().uid);
    assert_eq!(
        now.state, None,
        "the previous plugin's state was carried over to a different plugin"
    );

    // Undo restores the old plugin *and* the state it had.
    assert!(d.undo());
    assert_eq!(
        d.model().tracks[0].plugin.as_ref(),
        Some(&before),
        "undo did not restore the plugin and its state"
    );
    assert!(d.redo());
    assert_eq!(d.model().tracks[0].plugin.as_ref().unwrap().state, None);
}

#[test]
fn reloading_the_same_plugin_keeps_its_state() {
    // Re-sending the plugin a track already has (a rescan, a reconnect) must not wipe the
    // user's patch.
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    let same = TrackPlugin {
        uid: fixture_uid(),
        name: "Surge XT".into(),
        state: Some("c3VyZ2U=".into()),
    };

    d.apply(
        Command::SetTrackPlugin {
            id: t0,
            plugin: Some(same.clone()),
        },
        false,
    )
    .unwrap();
    assert_eq!(d.model().tracks[0].plugin.as_ref(), Some(&same));
}

#[test]
fn clearing_a_tracks_plugin_restores_the_built_in_voice() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    let voice = d.model().tracks[0].voice.clone();

    d.apply(
        Command::SetTrackPlugin {
            id: t0,
            plugin: None,
        },
        false,
    )
    .unwrap();
    assert_eq!(d.model().tracks[0].plugin, None);
    assert_eq!(
        d.model().tracks[0].voice,
        voice,
        "clearing a plugin must not disturb the voice it falls back to"
    );

    // And with no plugin to own it, there is nothing for a state blob to attach to.
    assert_eq!(
        d.apply(
            Command::SetPluginState {
                id: t0,
                state: "QUJDRA==".into()
            },
            false
        ),
        Err(EditError::NoPluginOnTrack)
    );
}

#[test]
fn a_plugin_without_a_uid_or_with_a_malformed_state_is_refused() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    let ghost = Uuid::new_v4();
    let before = d.model().clone();
    use EditError::*;

    // A uid is the only thing that can find the plugin again on the next load.
    for blank in ["", "   ", "\t\n"] {
        assert_eq!(
            d.apply(
                Command::SetTrackPlugin {
                    id: t0,
                    plugin: Some(TrackPlugin {
                        uid: blank.into(),
                        ..vital()
                    })
                },
                false
            ),
            Err(InvalidValue),
            "a plugin with the uid {blank:?} was accepted"
        );
    }

    // Shapes that cannot be base64, so they could never be handed back to a plugin.
    for bad in ["not base64!", "ab*d", "QQ=", "====", "=QUJ", "QUJDRA==="] {
        assert_eq!(
            d.apply(
                Command::SetPluginState {
                    id: t0,
                    state: bad.into()
                },
                false
            ),
            Err(InvalidValue),
            "state {bad:?} was accepted"
        );
        assert_eq!(
            d.apply(
                Command::SetTrackPlugin {
                    id: t0,
                    plugin: Some(TrackPlugin {
                        state: Some(bad.into()),
                        ..vital()
                    })
                },
                false
            ),
            Err(InvalidValue),
            "a plugin carrying state {bad:?} was accepted"
        );
    }

    assert_eq!(
        d.apply(
            Command::SetTrackPlugin {
                id: ghost,
                plugin: Some(vital())
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetTrackPlugin {
                id: ghost,
                plugin: None
            },
            false
        ),
        Err(NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::SetPluginState {
                id: ghost,
                state: "QUJDRA==".into()
            },
            false
        ),
        Err(NoSuchTrack)
    );

    assert_eq!(
        d.model(),
        &before,
        "a refused plugin edit changed the project"
    );
    assert!(!d.can_undo(), "a refused plugin edit recorded an undo step");

    // Well-formed blobs of every padding length still go through.
    for good in ["", "QUJD", "ZGlmZmVyZW50", "QQ==", "QUI=", "a+/9"] {
        d.apply(
            Command::SetPluginState {
                id: t0,
                state: good.into(),
            },
            false,
        )
        .unwrap_or_else(|e| panic!("valid base64 {good:?} was refused: {e}"));
    }
}

#[test]
fn the_sequence_carries_each_tracks_plugin_through_from_the_model() {
    // This is how the engine learns a track is plugin-driven; losing it here silently plays the
    // built-in synth instead of the user's plugin.
    let m = fixture();
    let expected = m.tracks[0].plugin.clone();
    assert!(expected.is_some());

    for sequence in [
        Sequence::from_pattern(&m, &m.patterns[0].id, 0).unwrap(),
        Sequence::from_song(&m),
    ] {
        assert_eq!(
            sequence.tracks[0].plugin, expected,
            "the sequence dropped the track's plugin"
        );
        assert_eq!(
            sequence.tracks[1].plugin, None,
            "a track with no plugin gained one"
        );
    }
}

#[test]
fn out_of_range_note_properties_are_clamped_on_the_way_in() {
    let m = fixture();
    let p0 = m.patterns[0].id;
    let mut doc = Document::new(m);

    let mut wild = NoteEvent::new(BEAT, BEAT, 60);
    wild.pan = 9.0;
    wild.release = -4.0;
    wild.mod_x = f32::NAN;
    wild.mod_y = 0.25;
    wild.fine = 9_000;
    wild.shift_ticks = 99 * BEAT;
    wild.repeats = 200;
    wild.color = 99;
    doc.apply(
        Command::SetNotes {
            pattern_id: p0,
            notes: vec![wild],
        },
        false,
    )
    .unwrap();

    let got = &doc.model().patterns[0].notes[0];
    assert_eq!(got.pan, 1.0, "pan clamps to the top of its range");
    assert_eq!(
        got.release, 0.0,
        "release clamps to the bottom of its range"
    );
    assert_eq!(
        got.mod_x, 0.5,
        "a non-finite property falls back to neutral"
    );
    assert_eq!(got.mod_y, 0.25, "a valid property is left alone");
    assert_eq!(got.fine, 1200, "fine pitch clamps to one octave");
    assert_eq!(
        got.shift_ticks, MAX_SHIFT_TICKS,
        "an out-of-range shift clamps to four beats"
    );
    assert_eq!(got.repeats, 14, "repeats clamp to FL's range");
    assert_eq!(got.color, 15, "colour clamps to the 16 groups");
}

#[test]
fn a_neutral_note_serialises_without_the_performance_properties() {
    // Projects written before these properties existed must round trip byte-identically, and a
    // note nobody has touched must not bloat the file.
    let json = serde_json::to_string(&NoteEvent::new(0, BEAT, 60)).unwrap();
    for absent in [
        "pan",
        "release",
        "fine",
        "modX",
        "modY",
        "shiftTicks",
        "repeats",
        "color",
    ] {
        assert!(
            !json.contains(absent),
            "neutral note serialised {absent}: {json}"
        );
    }

    let mut touched = NoteEvent::new(0, BEAT, 60);
    touched.pan = 0.0;
    let json = serde_json::to_string(&touched).unwrap();
    assert!(
        json.contains("\"pan\":0.0"),
        "a set property must survive the round trip: {json}"
    );
}

// ---------------------------------------------------------------- 8. effect inserts

/// The chain on a target, as names, with a `*` on anything bypassed.
fn chain(m: &ProjectModel, target: InsertTarget) -> Vec<String> {
    let inserts = match target {
        InsertTarget::Track(id) => &m.track(&id).expect("no such track").inserts,
        InsertTarget::Master => &m.master_inserts,
    };
    inserts
        .iter()
        .map(|i| {
            if i.enabled {
                i.name.clone()
            } else {
                format!("{}*", i.name)
            }
        })
        .collect()
}

#[test]
fn an_effect_chain_adds_reorders_bypasses_and_removes_one_undo_step_at_a_time() {
    let mut d = Document::new(ProjectModel::starter());
    let t0 = d.model().tracks[0].id;
    let target = InsertTarget::Track(t0);
    assert!(
        d.model().tracks[0].inserts.is_empty(),
        "a new track starts with no effects"
    );

    // Appended in the order they are added, then one squeezed in at the front.
    for (uid, name) in [(DELAY_UID, "Valhalla Delay"), (EQ_UID, "Pro-Q")] {
        d.apply(
            Command::AddInsert {
                target,
                uid: uid.into(),
                name: name.into(),
                index: None,
            },
            false,
        )
        .unwrap();
    }
    d.apply(
        Command::AddInsert {
            target,
            uid: EQ_UID.into(),
            name: "gate".into(),
            index: Some(0),
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), target),
        ["gate", "Valhalla Delay", "Pro-Q"]
    );

    // Order is the whole point of a chain: moving a slot keeps its identity and its state.
    let delay = d.model().tracks[0].inserts[1].id;
    d.apply(
        Command::MoveInsert {
            target,
            insert_id: delay,
            to_index: 2,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), target),
        ["gate", "Pro-Q", "Valhalla Delay"]
    );

    d.apply(
        Command::SetInsertEnabled {
            target,
            insert_id: delay,
            on: false,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), target),
        ["gate", "Pro-Q", "Valhalla Delay*"]
    );
    assert_eq!(
        d.model().tracks[0].inserts.len(),
        3,
        "a bypassed insert stays in the chain"
    );

    d.apply(
        Command::SetInsertState {
            target,
            insert_id: delay,
            state: "ZGVsYXk=".into(),
        },
        false,
    )
    .unwrap();
    assert_eq!(
        d.model().tracks[0].inserts[2].state.as_deref(),
        Some("ZGVsYXk=")
    );

    d.apply(
        Command::RemoveInsert {
            target,
            insert_id: delay,
        },
        false,
    )
    .unwrap();
    assert_eq!(chain(d.model(), target), ["gate", "Pro-Q"]);

    // Six edits, six undo steps, walking back to an empty chain and forward again.
    for expected in [
        vec!["gate", "Pro-Q", "Valhalla Delay*"],
        vec!["gate", "Pro-Q", "Valhalla Delay*"],
        vec!["gate", "Pro-Q", "Valhalla Delay"],
        vec!["gate", "Valhalla Delay", "Pro-Q"],
        vec!["Valhalla Delay", "Pro-Q"],
        vec!["Valhalla Delay"],
    ] {
        assert!(d.undo());
        assert_eq!(chain(d.model(), target), expected);
    }
    assert!(d.undo());
    assert!(d.model().tracks[0].inserts.is_empty());
    while d.redo() {}
    assert_eq!(chain(d.model(), target), ["gate", "Pro-Q"]);
}

#[test]
fn the_master_has_its_own_chain_independent_of_every_track() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    let before = chain(d.model(), InsertTarget::Track(t0));

    d.apply(
        Command::AddInsert {
            target: InsertTarget::Master,
            uid: DELAY_UID.into(),
            name: "glue".into(),
            index: Some(0),
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), InsertTarget::Master),
        ["glue", "Pro-Q", "compressor"]
    );
    assert_eq!(
        chain(d.model(), InsertTarget::Track(t0)),
        before,
        "a master insert leaked onto a track"
    );

    // Deleting every track leaves the master chain alone: it belongs to the project.
    let master_id = d.model().master_inserts[0].id;
    d.apply(
        Command::SetInsertEnabled {
            target: InsertTarget::Master,
            insert_id: master_id,
            on: false,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), InsertTarget::Master),
        ["glue*", "Pro-Q", "compressor"]
    );
}

#[test]
fn a_chain_is_capped_at_max_inserts() {
    let mut d = Document::new(ProjectModel::starter());
    let target = InsertTarget::Track(d.model().tracks[0].id);
    for i in 0..MAX_INSERTS {
        d.apply(
            Command::AddInsert {
                target,
                uid: EQ_UID.into(),
                name: format!("eq {i}"),
                index: None,
            },
            false,
        )
        .unwrap_or_else(|e| panic!("insert {i} refused: {e}"));
    }
    let full = d.model().clone();
    assert_eq!(
        d.apply(
            Command::AddInsert {
                target,
                uid: EQ_UID.into(),
                name: "one too many".into(),
                index: None
            },
            false
        ),
        Err(EditError::TooManyInserts)
    );
    assert_eq!(d.model(), &full, "a refused insert changed the chain");

    // The master chain has the same ceiling, counted separately.
    for i in 0..MAX_INSERTS {
        d.apply(
            Command::AddInsert {
                target: InsertTarget::Master,
                uid: EQ_UID.into(),
                name: format!("m {i}"),
                index: None,
            },
            false,
        )
        .unwrap_or_else(|e| panic!("master insert {i} refused: {e}"));
    }
    assert_eq!(
        d.apply(
            Command::AddInsert {
                target: InsertTarget::Master,
                uid: EQ_UID.into(),
                name: "nope".into(),
                index: None
            },
            false
        ),
        Err(EditError::TooManyInserts)
    );
}

#[test]
fn an_insert_without_a_uid_is_refused() {
    // A uid is the only thing that can find the effect again on the next load; a slot without
    // one is a permanent hole in the chain.
    let mut d = doc();
    let target = InsertTarget::Track(d.model().tracks[0].id);
    let before = d.model().clone();
    for blank in ["", "   ", "\t\n"] {
        for t in [target, InsertTarget::Master] {
            assert_eq!(
                d.apply(
                    Command::AddInsert {
                        target: t,
                        uid: blank.into(),
                        name: "ghost".into(),
                        index: None
                    },
                    false
                ),
                Err(EditError::InvalidValue),
                "an insert with the uid {blank:?} was accepted"
            );
        }
    }
    assert_eq!(d.model(), &before, "a refused insert changed the project");
    assert!(!d.can_undo(), "a refused insert recorded an undo step");
}

#[test]
fn insert_positions_are_clamped_rather_than_refused() {
    // The UI's drop targets and the chain's bounds do not have to agree: dragging past either
    // end lands on that end.
    let mut d = doc();
    let target = InsertTarget::Track(d.model().tracks[0].id);
    let first = d.model().tracks[0].inserts[0].id;

    d.apply(
        Command::AddInsert {
            target,
            uid: EQ_UID.into(),
            name: "tail".into(),
            index: Some(99),
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), target),
        ["Valhalla Delay", "Pro-Q", "tail"],
        "an out-of-range index must append"
    );

    d.apply(
        Command::MoveInsert {
            target,
            insert_id: first,
            to_index: 99,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), target),
        ["Pro-Q", "tail", "Valhalla Delay"]
    );

    d.apply(
        Command::MoveInsert {
            target,
            insert_id: first,
            to_index: 0,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        chain(d.model(), target),
        ["Valhalla Delay", "Pro-Q", "tail"]
    );

    // A move onto its own position changes nothing, so it costs no undo step.
    let steps = d.model().clone();
    d.apply(
        Command::MoveInsert {
            target,
            insert_id: first,
            to_index: 0,
        },
        false,
    )
    .unwrap();
    assert_eq!(d.model(), &steps);
}

#[test]
fn insert_state_is_base64_checked_and_coalesces_like_a_knob_drag() {
    let mut d = doc();
    let target = InsertTarget::Track(d.model().tracks[0].id);
    let id = d.model().tracks[0].inserts[0].id;
    let before = d.model().clone();

    for bad in ["not base64!", "ab*d", "QQ=", "====", "=QUJ", "QUJDRA==="] {
        assert_eq!(
            d.apply(
                Command::SetInsertState {
                    target,
                    insert_id: id,
                    state: bad.into()
                },
                false
            ),
            Err(EditError::InvalidValue),
            "insert state {bad:?} was accepted"
        );
    }
    assert_eq!(d.model(), &before);

    // A plugin window streaming state while the user drags a knob is one undo step, exactly
    // like `SetTrackParam`.
    for state in ["QUJD", "ZGlmZmVyZW50", "QQ==", "a+/9"] {
        d.apply(
            Command::SetInsertState {
                target,
                insert_id: id,
                state: state.into(),
            },
            true,
        )
        .unwrap();
    }
    d.commit_gesture();
    assert_eq!(
        d.model().tracks[0].inserts[0].state.as_deref(),
        Some("a+/9")
    );
    assert!(d.undo());
    assert_eq!(
        d.model(),
        &before,
        "a run of transient state edits was not one undo step"
    );
    assert!(!d.can_undo());
}

#[test]
fn insert_commands_reject_unknown_tracks_and_unknown_slots() {
    let mut d = doc();
    let target = InsertTarget::Track(d.model().tracks[0].id);
    let ghost_track = InsertTarget::Track(Uuid::new_v4());
    let ghost = Uuid::new_v4();
    let before = d.model().clone();

    assert_eq!(
        d.apply(
            Command::AddInsert {
                target: ghost_track,
                uid: EQ_UID.into(),
                name: "eq".into(),
                index: None
            },
            false
        ),
        Err(EditError::NoSuchTrack)
    );
    assert_eq!(
        d.apply(
            Command::RemoveInsert {
                target: ghost_track,
                insert_id: ghost
            },
            false
        ),
        Err(EditError::NoSuchTrack)
    );

    for target in [target, InsertTarget::Master] {
        assert_eq!(
            d.apply(
                Command::RemoveInsert {
                    target,
                    insert_id: ghost
                },
                false
            ),
            Err(EditError::NoSuchInsert)
        );
        assert_eq!(
            d.apply(
                Command::MoveInsert {
                    target,
                    insert_id: ghost,
                    to_index: 0
                },
                false
            ),
            Err(EditError::NoSuchInsert)
        );
        assert_eq!(
            d.apply(
                Command::SetInsertEnabled {
                    target,
                    insert_id: ghost,
                    on: false
                },
                false
            ),
            Err(EditError::NoSuchInsert)
        );
        assert_eq!(
            d.apply(
                Command::SetInsertState {
                    target,
                    insert_id: ghost,
                    state: "QUJD".into()
                },
                false
            ),
            Err(EditError::NoSuchInsert)
        );
    }

    // An insert id only means anything inside its own chain.
    let on_track = d.model().tracks[0].inserts[0].id;
    assert_eq!(
        d.apply(
            Command::RemoveInsert {
                target: InsertTarget::Master,
                insert_id: on_track
            },
            false
        ),
        Err(EditError::NoSuchInsert)
    );

    assert_eq!(
        d.model(),
        &before,
        "a refused insert command changed the project"
    );
    assert!(
        !d.can_undo(),
        "a refused insert command recorded an undo step"
    );
}

#[test]
fn deleting_a_track_takes_its_chain_with_it_and_undo_brings_it_back() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    let before = d.model().tracks[0].inserts.clone();
    assert_eq!(before.len(), 2);

    d.apply(Command::DeleteTrack { id: t0 }, false).unwrap();
    assert!(d.model().track(&t0).is_none());
    assert!(d.undo());
    assert_eq!(
        d.model().tracks[0].inserts,
        before,
        "undo lost the track's effect chain"
    );
}

#[test]
fn a_factory_voice_prunes_the_user_sound_it_replaces() {
    let mut d = doc();
    let t0 = d.model().tracks[0].id;
    d.apply(
        Command::SetTrackVoice {
            id: t0,
            voice: "pad".into(),
        },
        false,
    )
    .unwrap();
    assert!(
        d.model().sounds.is_empty(),
        "u-test outlived the track that used it"
    );
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

// ---------------------------------------------------------------- 9. automation

/// `(document, track id, volume target)` for the automation tests.
fn auto_doc() -> (Document, Uuid, AutoTarget) {
    let d = doc();
    let t = d.model().tracks[0].id;
    (d, t, AutoTarget::Track(t, TrackParam::Volume))
}

fn add_point(d: &mut Document, target: AutoTarget, tick: i64, value: f64, curve: Curve) {
    d.apply(
        Command::AddAutomationPoint {
            target,
            tick,
            value,
            curve,
        },
        false,
    )
    .unwrap();
}

#[test]
fn a_lane_appears_with_its_first_point_and_one_point_holds_each_tick() {
    let (mut d, t, target) = auto_doc();
    add_point(&mut d, target, 8 * BEAT, 0.5, Curve::Ramp);
    assert_eq!(d.model().automation.len(), 1);
    assert_eq!(d.model().automation[0].target, target);
    assert_eq!(d.model().automation[0].points.len(), 1);

    // A second point on the same tick replaces the first rather than stacking on it.
    add_point(&mut d, target, 8 * BEAT, 0.25, Curve::Step);
    assert_eq!(d.model().automation[0].points.len(), 1);
    assert_eq!(d.model().automation[0].points[0].value, 0.25);
    assert_eq!(d.model().automation[0].points[0].curve, Curve::Step);

    // A second parameter is a second lane, not a second entry on the first.
    add_point(
        &mut d,
        AutoTarget::Track(t, TrackParam::Pan),
        0,
        -0.5,
        Curve::Step,
    );
    assert_eq!(d.model().automation.len(), 2);
}

#[test]
fn a_point_moves_keeps_its_id_and_swallows_the_neighbour_it_lands_on() {
    let (mut d, _, target) = auto_doc();
    add_point(&mut d, target, 0, 1.0, Curve::Step);
    add_point(&mut d, target, 8 * BEAT, 0.0, Curve::Ramp);
    let lane = d.model().automation[0].id;
    let moved = d.model().automation[0].points[1].clone();
    let id = moved.id;

    d.apply(
        Command::UpdateAutomationPoint {
            lane,
            point: Breakpoint {
                tick: 0,
                value: 0.4,
                ..moved
            },
        },
        false,
    )
    .unwrap();
    let points = &d.model().automation[0].points;
    assert_eq!(points.len(), 1, "the point it landed on should be gone");
    assert_eq!(points[0].id, id, "the dragged point kept its identity");
    assert_eq!(points[0].value, 0.4);
}

#[test]
fn removing_the_last_point_takes_the_lane_with_it() {
    let (mut d, _, target) = auto_doc();
    add_point(&mut d, target, 0, 1.0, Curve::Step);
    add_point(&mut d, target, 4 * BEAT, 0.5, Curve::Ramp);
    let lane = d.model().automation[0].id;
    let (first, second) = (
        d.model().automation[0].points[0].id,
        d.model().automation[0].points[1].id,
    );
    d.apply(Command::RemoveAutomationPoint { lane, id: first }, false)
        .unwrap();
    assert_eq!(d.model().automation.len(), 1);
    d.apply(Command::RemoveAutomationPoint { lane, id: second }, false)
        .unwrap();
    assert!(
        d.model().automation.is_empty(),
        "an empty lane is no automation at all"
    );
    assert_eq!(
        d.apply(Command::RemoveAutomationPoint { lane, id: second }, false),
        Err(EditError::NoSuchAutomationLane)
    );
}

#[test]
fn automation_edits_are_validated_and_a_refused_one_changes_nothing() {
    let (mut d, t, target) = auto_doc();
    add_point(&mut d, target, 0, 1.0, Curve::Step);
    let before = d.model().clone();
    let lane = before.automation[0].id;
    let point = before.automation[0].points[0].clone();

    for bad in [
        Command::AddAutomationPoint {
            target,
            tick: -1,
            value: 0.5,
            curve: Curve::Step,
        },
        Command::AddAutomationPoint {
            target,
            tick: 0,
            value: f64::NAN,
            curve: Curve::Step,
        },
        Command::AddAutomationPoint {
            target: AutoTarget::Track(Uuid::new_v4(), TrackParam::Volume),
            tick: 0,
            value: 0.5,
            curve: Curve::Step,
        },
        Command::UpdateAutomationPoint {
            lane: Uuid::new_v4(),
            point: point.clone(),
        },
        Command::UpdateAutomationPoint {
            lane,
            point: Breakpoint::new(0, 0.5, Curve::Step),
        },
        Command::ClearAutomation {
            target: AutoTarget::Track(t, TrackParam::Tone),
        },
    ] {
        assert!(d.apply(bad.clone(), false).is_err(), "{bad:?} was accepted");
        assert_eq!(d.model(), &before, "{bad:?} changed the project");
    }

    // Values are held to the control's own range, the same table the fader uses.
    add_point(&mut d, target, 2 * BEAT, 99.0, Curve::Step);
    assert_eq!(d.model().automation[0].points[1].value, 1.5);
    add_point(
        &mut d,
        AutoTarget::Track(t, TrackParam::Pan),
        0,
        -99.0,
        Curve::Step,
    );
    assert_eq!(
        d.model()
            .automation
            .iter()
            .find(|l| l.target == AutoTarget::Track(t, TrackParam::Pan))
            .unwrap()
            .points[0]
            .value,
        -1.0
    );
}

#[test]
fn a_freehand_drag_is_one_undo_step() {
    let (mut d, _, target) = auto_doc();
    add_point(&mut d, target, 0, 1.0, Curve::Step);
    add_point(&mut d, target, 8 * BEAT, 1.0, Curve::Ramp);
    let before = d.model().clone();
    let lane = d.model().automation[0].id;
    let dragged = d.model().automation[0].points[1].clone();

    // What the lane sends while the pointer moves: one transient command per frame.
    for v in [0.8, 0.6, 0.4, 0.2, 0.0] {
        d.apply(
            Command::UpdateAutomationPoint {
                lane,
                point: Breakpoint {
                    value: v,
                    ..dragged.clone()
                },
            },
            true,
        )
        .unwrap();
    }
    d.commit_gesture();
    assert_eq!(d.model().automation[0].points[1].value, 0.0);
    assert_eq!(d.undo_label(), Some("move automation point"));
    d.undo();
    assert_eq!(
        d.model(),
        &before,
        "a drag should undo in one step, back to where the point started"
    );
}

#[test]
fn clearing_a_lane_and_deleting_its_track_both_remove_it() {
    let (mut d, t, target) = auto_doc();
    add_point(&mut d, target, 0, 0.5, Curve::Step);
    add_point(
        &mut d,
        AutoTarget::Master(MasterParam::Gain),
        0,
        0.9,
        Curve::Step,
    );
    d.apply(Command::ClearAutomation { target }, false).unwrap();
    assert_eq!(
        d.model().automation.len(),
        1,
        "clearing one target must leave the others"
    );

    add_point(&mut d, target, 0, 0.5, Curve::Step);
    d.apply(Command::DeleteTrack { id: t }, false).unwrap();
    assert!(
        d.model()
            .automation
            .iter()
            .all(|l| l.target.track_id() != Some(t)),
        "a deleted track left its lanes behind"
    );
    assert_eq!(
        d.model().automation.len(),
        1,
        "the master lane is not a track's and must survive"
    );
}

#[test]
fn song_mode_flattens_lanes_as_they_are_and_pattern_mode_windows_them() {
    let (mut d, _, target) = auto_doc();
    // A fade across bars 3 → 5 of the song, and a master lane alongside it.
    add_point(&mut d, target, 8 * BEAT, 1.0, Curve::Step);
    add_point(&mut d, target, 16 * BEAT, 0.0, Curve::Ramp);
    add_point(
        &mut d,
        AutoTarget::Master(MasterParam::Gain),
        0,
        0.75,
        Curve::Step,
    );

    let song = Sequence::from_song(d.model());
    let volume = song.tracks[0]
        .automation
        .volume
        .as_ref()
        .expect("song mode carries the lane");
    assert_eq!(volume.value_at(0.0), 1.0, "before the first point it holds");
    assert_eq!(volume.value_at(12.0 * BEAT as f64), 0.5, "a ramp interpolates");
    assert_eq!(volume.value_at(99.0 * BEAT as f64), 0.0, "past the end it holds");
    assert_eq!(
        song.master_automation.gain.as_ref().map(|c| c.value_at(0.0)),
        Some(0.75)
    );
    assert!(song.tracks[0].automation.tone.is_none());

    // Pattern mode hears the pattern where it sits in the song, so the lane is windowed from
    // the offset: the value in force there becomes the window's tick 0.
    let pid = d.model().patterns[0].id;
    let windowed = Sequence::from_pattern(d.model(), &pid, 12 * BEAT).unwrap();
    let w = windowed.tracks[0]
        .automation
        .volume
        .as_ref()
        .expect("pattern mode carries the window");
    assert_eq!(w.value_at(0.0), 0.5, "the window opens where the song was");
    assert_eq!(w.value_at(4.0 * BEAT as f64), 0.0, "and the rest shifts back");
    assert_eq!(
        windowed
            .master_automation
            .gain
            .as_ref()
            .map(|c| c.value_at(0.0)),
        Some(0.75)
    );

    // Two builds of the same sequence must compare equal, window or no window.
    assert_eq!(
        Sequence::from_pattern(d.model(), &pid, 12 * BEAT).unwrap(),
        windowed
    );
}

// ---------------------------------------------------------------- 10. ripple edits

/// 4/4, so one bar is four beats.
const BAR: i64 = 4 * BEAT;

/// The fixture with one of everything a ripple has to carry past bar 17: a clip that starts on
/// the cut and one that straddles it, three markers (two running to the next one, one with its
/// own extent), a tempo change, a volume lane and a loop brace.
fn rippled() -> Document {
    let mut m = fixture();
    let (t0, _, p0, p1) = ids(&m);
    m.clips.push(clip_at(p0, 16 * BAR, 4 * BAR));
    m.clips.push(clip_at(p1, 15 * BAR, 7 * BAR));
    m.clips.sort_by_key(|c| c.start_tick);
    m.sections = vec![
        Section::new("intro", 0),
        Section::new("chorus", 16 * BAR),
        Section {
            length_ticks: 4 * BAR,
            ..Section::new("outro", 24 * BAR)
        },
    ];
    m.tempo_points = vec![TempoPoint::new(16 * BAR, 140.0, false)];
    let mut lane = AutomationLane::new(AutoTarget::Track(t0, TrackParam::Volume));
    lane.points = vec![
        Breakpoint::new(8 * BAR, 0.5, Curve::Step),
        Breakpoint::new(20 * BAR, 0.25, Curve::Ramp),
    ];
    m.automation = vec![lane];
    m.loop_region = Some((16 * BAR, 20 * BAR));
    Document::new(m)
}

/// Everything on the timeline inside `[from, to)` as plain values measured from `from`, so two
/// stretches of a song can be compared without caring which ids they wear.
#[derive(Debug, PartialEq)]
struct Window {
    /// (pattern, start, length, offset)
    clips: Vec<(Uuid, i64, i64, i64)>,
    /// (name, start, extent)
    sections: Vec<(String, i64, i64)>,
    tempo: Vec<(i64, f64, bool)>,
    chords: Vec<(i64, i64, Vec<i32>)>,
    automation: Vec<(i64, f64)>,
}

fn window(m: &ProjectModel, from: i64, to: i64) -> Window {
    let inside = |tick: i64| tick >= from && tick < to;
    Window {
        clips: m
            .clips
            .iter()
            .filter(|c| inside(c.start_tick))
            .map(|c| {
                (
                    c.pattern_id,
                    c.start_tick - from,
                    c.length_ticks,
                    c.offset_ticks,
                )
            })
            .collect(),
        sections: m
            .sections
            .iter()
            .filter(|s| inside(s.start_tick))
            .map(|s| (s.name.clone(), s.start_tick - from, s.length_ticks))
            .collect(),
        tempo: m
            .tempo_points
            .iter()
            .filter(|p| inside(p.tick))
            .map(|p| (p.tick - from, p.bpm, p.ramp))
            .collect(),
        chords: m
            .chords
            .chords()
            .iter()
            .filter(|c| inside(c.start_tick))
            .map(|c| (c.start_tick - from, c.length_ticks, c.pitch_classes.clone()))
            .collect(),
        automation: m
            .automation
            .iter()
            .flat_map(|l| {
                l.points
                    .iter()
                    .filter(|p| inside(p.tick))
                    .map(|p| (p.tick - from, p.value))
            })
            .collect(),
    }
}

#[test]
fn inserting_time_moves_everything_after_it_by_exactly_that_much() {
    let mut d = rippled();
    let before = d.model().clone();
    let (at, ticks) = (16 * BAR, 4 * BAR);
    let on_cut = before
        .clips
        .iter()
        .find(|c| c.start_tick == at)
        .unwrap()
        .clone();
    let across = before
        .clips
        .iter()
        .find(|c| c.start_tick == 15 * BAR)
        .unwrap()
        .clone();
    let notes_before: Vec<i64> = Sequence::from_song(&before).tracks[0]
        .notes
        .iter()
        .map(|n| n.start_tick)
        .collect();

    d.apply(Command::InsertTime { at, ticks }, false).unwrap();
    let m = d.model();

    // A clip starting on the cut moves whole; one the cut falls inside is split, and the tail
    // picks the pattern up exactly where the head left it.
    assert_eq!(m.clip(&on_cut.id).unwrap().start_tick, at + ticks);
    let head = m.clip(&across.id).unwrap();
    assert_eq!((head.start_tick, head.length_ticks), (15 * BAR, BAR));
    let tail = m
        .clips
        .iter()
        .find(|c| c.pattern_id == across.pattern_id && c.id != across.id && c.start_tick == at + ticks)
        .expect("the part past the cut lives on as its own clip");
    assert_eq!(
        (tail.start_tick, tail.length_ticks, tail.offset_ticks),
        (at + ticks, 6 * BAR, BAR)
    );

    assert_eq!(
        m.sections
            .iter()
            .map(|s| (s.name.as_str(), s.start_tick, s.length_ticks))
            .collect::<Vec<_>>(),
        vec![
            ("intro", 0, 0),
            ("chorus", 20 * BAR, 0),
            ("outro", 28 * BAR, 4 * BAR)
        ]
    );
    assert_eq!(m.tempo_points[0].tick, 20 * BAR);
    assert_eq!(
        m.automation[0]
            .points
            .iter()
            .map(|p| p.tick)
            .collect::<Vec<_>>(),
        vec![8 * BAR, 24 * BAR]
    );
    assert_eq!(m.loop_region, Some((20 * BAR, 24 * BAR)));
    assert_eq!(m.song_length_ticks, before.song_length_ticks + ticks);
    // The clips and markers are asserted above one by one: a clip the cut split now starts
    // inside this window and did not before, which is the point of the split.
    assert_eq!(
        window(m, 20 * BAR, m.song_length_ticks).chords,
        window(&before, at, before.song_length_ticks).chords,
        "the progression past the cut is the same progression, four bars later"
    );

    // What is played moves with it: the bounce and the transport read this one sequence.
    let notes_after: Vec<i64> = Sequence::from_song(m).tracks[0]
        .notes
        .iter()
        .map(|n| n.start_tick)
        .collect();
    assert_eq!(
        notes_after,
        notes_before
            .iter()
            .map(|&t| if t >= at { t + ticks } else { t })
            .collect::<Vec<_>>()
    );

    // One gesture, one undo step.
    assert!(d.undo());
    assert_eq!(d.model(), &before);
    assert!(!d.can_undo());
}

#[test]
fn time_opened_inside_a_section_makes_the_section_longer() {
    let mut d = rippled();
    d.apply(
        Command::InsertTime {
            at: 26 * BAR,
            ticks: 2 * BAR,
        },
        false,
    )
    .unwrap();
    let outro = d.model().sections.last().unwrap();
    assert_eq!(
        (outro.start_tick, outro.length_ticks),
        (24 * BAR, 6 * BAR),
        "bars added inside the outro are more outro, not a split marker"
    );
}

#[test]
fn deleting_time_closes_the_gap_and_splits_what_straddles_the_cut() {
    let mut d = rippled();
    let before = d.model().clone();
    let (at, ticks) = (16 * BAR, 4 * BAR);
    let inside = before.clips.iter().find(|c| c.start_tick == at).unwrap().id;
    let across = before
        .clips
        .iter()
        .find(|c| c.start_tick == 15 * BAR)
        .unwrap()
        .clone();

    d.apply(Command::DeleteTime { at, ticks }, false).unwrap();
    let m = d.model();

    assert!(
        m.clip(&inside).is_none(),
        "a clip wholly inside the cut goes with the time it lived in"
    );
    let head = m.clip(&across.id).unwrap();
    assert_eq!((head.start_tick, head.length_ticks), (15 * BAR, BAR));
    let tail = m
        .clips
        .iter()
        .find(|c| c.pattern_id == across.pattern_id && c.id != across.id && c.start_tick == at)
        .expect("what was past the cut closes up against what was before it");
    assert_eq!(
        (tail.start_tick, tail.length_ticks, tail.offset_ticks),
        (at, 2 * BAR, 5 * BAR)
    );

    assert!(
        m.tempo_points.is_empty(),
        "a tempo change inside the cut goes with it"
    );
    assert_eq!(
        m.sections
            .iter()
            .map(|s| (s.name.as_str(), s.start_tick, s.length_ticks))
            .collect::<Vec<_>>(),
        vec![("intro", 0, 0), ("chorus", at, 0), ("outro", 20 * BAR, 4 * BAR)]
    );
    assert_eq!(
        m.automation[0]
            .points
            .iter()
            .map(|p| p.tick)
            .collect::<Vec<_>>(),
        vec![8 * BAR, at]
    );
    assert_eq!(
        m.loop_region, None,
        "a brace with nothing left inside it is no brace"
    );
    assert_eq!(m.song_length_ticks, before.song_length_ticks - ticks);
    assert_eq!(
        window(m, at, m.song_length_ticks).chords,
        window(&before, at + ticks, before.song_length_ticks).chords,
        "the progression past the cut is the same progression, four bars earlier"
    );

    assert!(d.undo());
    assert_eq!(d.model(), &before);
}

#[test]
fn a_section_whose_whole_extent_is_cut_goes_with_its_music() {
    let mut m = fixture();
    m.sections = vec![
        Section::new("verse", 0),
        Section {
            length_ticks: 2 * BAR,
            ..Section::new("break", 8 * BAR)
        },
        Section::new("chorus", 12 * BAR),
    ];
    let mut d = Document::new(m);
    d.apply(
        Command::DeleteTime {
            at: 8 * BAR,
            ticks: 4 * BAR,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        d.model()
            .sections
            .iter()
            .map(|s| (s.name.as_str(), s.start_tick))
            .collect::<Vec<_>>(),
        vec![("verse", 0), ("chorus", 8 * BAR)]
    );
}

#[test]
fn a_lane_whose_every_point_is_cut_away_stops_overriding_its_control() {
    let mut d = rippled();
    d.apply(
        Command::DeleteTime {
            at: 4 * BAR,
            ticks: 20 * BAR,
        },
        false,
    )
    .unwrap();
    assert!(
        d.model().automation.is_empty(),
        "a lane with no points left is no lane"
    );
}

#[test]
fn duplicating_a_range_reproduces_it_whole_at_the_target() {
    let mut m = fixture();
    let (t0, _, p0, p1) = ids(&m);
    m.clips = vec![clip_at(p0, 0, 8 * BAR), clip_at(p1, 8 * BAR, 8 * BAR)];
    m.sections = vec![Section::new("verse", 0), Section::new("after", 16 * BAR)];
    m.tempo_points = vec![TempoPoint::new(4 * BAR, 100.0, true)];
    let mut lane = AutomationLane::new(AutoTarget::Track(t0, TrackParam::Volume));
    lane.points = vec![Breakpoint::new(2 * BAR, 0.4, Curve::Ramp)];
    m.automation = vec![lane];
    let mut d = Document::new(m);
    let before = d.model().clone();

    d.apply(
        Command::DuplicateRange {
            from: 0,
            to: 16 * BAR,
            at: 16 * BAR,
        },
        false,
    )
    .unwrap();
    let m = d.model();

    assert_eq!(m.song_length_ticks, before.song_length_ticks + 16 * BAR);
    assert_eq!(
        window(m, 0, 16 * BAR),
        window(&before, 0, 16 * BAR),
        "the source bars are left alone"
    );
    assert_eq!(
        window(m, 16 * BAR, 32 * BAR),
        window(&before, 0, 16 * BAR),
        "bars 1-16 again at bar 17, marker and all"
    );
    assert_eq!(
        window(m, 32 * BAR, m.song_length_ticks),
        window(&before, 16 * BAR, before.song_length_ticks),
        "what followed was pushed along, not written over"
    );
    // Copies are copies: the originals keep their ids and nothing is counted twice.
    assert_eq!(m.clips.len(), before.clips.len() * 2);
    assert_eq!(
        m.clips.iter().filter(|c| c.id == before.clips[0].id).count(),
        1
    );

    assert!(d.undo());
    assert_eq!(d.model(), &before);
    assert!(!d.can_undo(), "one gesture, one undo step");
}

#[test]
fn a_take_split_by_a_ripple_keeps_the_seconds_it_played() {
    // At 120 bpm a beat is half a second, so the fixture's two-second take is four beats long.
    let mut d = doc();
    let source = d.model().audio_clips[0].source_id;
    d.apply(
        Command::InsertTime {
            at: 2 * BEAT,
            ticks: 4 * BEAT,
        },
        false,
    )
    .unwrap();
    let takes = &d.model().audio_clips;
    assert_eq!(takes.len(), 2, "the take is split, not stretched");
    assert_eq!(
        (
            takes[0].start_tick,
            takes[0].offset_seconds,
            takes[0].length_seconds
        ),
        (0, 0.0, 1.0)
    );
    assert_eq!(
        (
            takes[1].start_tick,
            takes[1].offset_seconds,
            takes[1].length_seconds
        ),
        (6 * BEAT, 1.0, 1.0),
        "the second half plays on from exactly where the first stopped"
    );
    assert!(takes.iter().all(|c| c.source_id == source));
    assert_ne!(takes[0].id, takes[1].id);
}

#[test]
fn a_ripple_refuses_nonsense_and_leaves_the_song_alone() {
    for command in [
        Command::InsertTime { at: -1, ticks: BAR },
        Command::InsertTime { at: 0, ticks: 0 },
        Command::DeleteTime { at: 0, ticks: -1 },
        Command::DuplicateRange {
            from: 4 * BAR,
            to: 4 * BAR,
            at: 0,
        },
        Command::DuplicateRange {
            from: -1,
            to: BAR,
            at: 0,
        },
    ] {
        let mut d = rippled();
        let before = d.model().clone();
        assert_eq!(
            d.apply(command.clone(), false),
            Err(EditError::InvalidValue),
            "{command:?} should be refused"
        );
        assert_eq!(d.model(), &before);
        assert!(!d.can_undo());
    }
}

#[test]
fn punching_the_middle_of_a_take_splits_it_and_undoes_in_one_step() {
    // At 120 bpm a beat is half a second, so the fixture's two-second take at tick 0 runs to
    // beat 4. The punch covers one second to one and a half: the middle of what was played.
    let mut d = doc();
    let gid = audio_track(d.model());
    let before = d.model().clone();
    let fix = take_source();
    let xf = DEFAULT_PUNCH_CROSSFADE_SECONDS;
    let punch_in = 2 * BEAT;
    d.apply(
        Command::PunchAudioTake {
            track_id: gid,
            clip: AudioClip::new(gid, fix.id, punch_in, 0.5),
            source: fix.clone(),
            crossfade_seconds: xf,
        },
        false,
    )
    .unwrap();

    let clips = d.model().audio_clips.clone();
    assert_eq!(clips.len(), 3, "head, take, tail");
    let (head, take, tail) = (&clips[0], &clips[1], &clips[2]);
    assert_eq!(take.source_id, fix.id, "the new take is the middle clip");
    assert!(
        head.source_id == tail.source_id && head.source_id != fix.id,
        "the head and tail are both the original take"
    );

    // The head keeps playing across the crossfade and fades out over it; the tail starts one
    // crossfade early and fades in over it. The seams are the overlaps.
    let map = d.model().tempo_map();
    let secs = |tick: i64| map.seconds_at_tick(tick as f64);
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert_eq!(head.id, before.audio_clips[0].id, "the head keeps the clip's identity");
    assert_eq!(head.start_tick, 0);
    assert!(close(head.length_seconds, 1.0 + xf), "{}", head.length_seconds);
    assert!(close(head.fade_out_seconds, xf) && close(head.fade_in_seconds, 0.0));

    assert_eq!(take.start_tick, punch_in);
    assert!(close(take.fade_in_seconds, xf) && close(take.fade_out_seconds, xf));

    // The tail is placed on the nearest whole tick, so its edge can sit up to half a tick off
    // the exact crossfade start; what it may never do is slide against the file under it.
    let tail_at = secs(tail.start_tick);
    assert!((tail_at - (1.5 - xf)).abs() <= secs(1) * 0.5, "{tail_at}");
    assert!(close(tail.offset_seconds, tail_at), "the tail plays the material that lives there");
    assert!(close(tail.offset_seconds + tail.length_seconds, 2.0), "and runs to the take's end");
    assert!(close(tail.fade_in_seconds, xf));

    assert_eq!(d.undo_label(), Some("punch in"));
    d.undo();
    assert_eq!(d.model().audio_clips, before.audio_clips, "one ⌘Z puts the take back whole");
}

#[test]
fn a_punch_over_a_whole_clip_replaces_it_and_one_beside_it_is_untouched() {
    let mut d = doc();
    let gid = audio_track(d.model());
    // A second take further along, well clear of the punch.
    let other = take_source();
    d.apply(
        Command::AddAudioTake {
            track_id: gid,
            clip: AudioClip::new(gid, other.id, 16 * BEAT, 1.0),
            source: other.clone(),
        },
        false,
    )
    .unwrap();
    let fix = take_source();
    d.apply(
        Command::PunchAudioTake {
            track_id: gid,
            // The fixture's take runs 0…2 s; this covers all of it and then some.
            clip: AudioClip::new(gid, fix.id, 0, 3.0),
            source: fix.clone(),
            crossfade_seconds: DEFAULT_PUNCH_CROSSFADE_SECONDS,
        },
        false,
    )
    .unwrap();
    let clips = &d.model().audio_clips;
    assert_eq!(clips.len(), 2, "nothing is left of the buried take");
    assert_eq!(clips[0].source_id, fix.id);
    assert_eq!(clips[1].source_id, other.id, "the take beside the punch is untouched");
    assert_eq!(clips[1].start_tick, 16 * BEAT);
}
