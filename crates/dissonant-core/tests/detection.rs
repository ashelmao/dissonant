//! What DI detection does to a document: a transcription lands as one undo step, detected
//! chords wait in their own lane until accepted or dismissed one at a time, and none of it
//! touches the song's progression until the player says so.

use dissonant_core::*;
use uuid::Uuid;

const BEAT: i64 = TICKS_PER_BEAT;

/// A starter song (its progression tiled across the song) with one guitar take at bar 2.
fn song() -> (Document, Uuid) {
    let mut m = ProjectModel::starter();
    let guitar = Track::audio("guitar");
    let source = AudioSource {
        id: Uuid::new_v4(),
        name: "take 3".into(),
        frames: 96_000,
        sample_rate: 48_000,
        channels: 1,
    };
    let clip = AudioClip::new(guitar.id, source.id, 4 * BEAT, 2.0);
    let clip_id = clip.id;
    m.tracks.push(guitar);
    m.audio_sources.push(source);
    m.audio_clips.push(clip);
    (Document::new(m), clip_id)
}

fn chord(start: i64, len: i64, pcs: &[i32], name: &str) -> DetectedChord {
    DetectedChord {
        id: Uuid::new_v4(),
        start_tick: start,
        length_ticks: len,
        pitch_classes: pcs.to_vec(),
        name: Some(name.into()),
        confidence: 0.9,
    }
}

fn bent_note() -> NoteEvent {
    let mut n = NoteEvent::new(BEAT, 2 * BEAT, 57).with_velocity(96);
    n.pitch_envelope = vec![
        PitchPoint::new(0, 0.0, Curve::Ramp),
        PitchPoint::new(BEAT / 2, 200.0, Curve::Ramp),
    ];
    n
}

fn transcription(clip_id: Uuid, notes: Vec<NoteEvent>, chords: Vec<DetectedChord>) -> Command {
    Command::AddTranscription {
        name: "take 3 notes".into(),
        start_tick: 4 * BEAT,
        length_ticks: 8 * BEAT,
        notes,
        detection: ClipDetection {
            clip_id,
            chords,
            chroma: [0.0; 12],
        },
    }
}

#[test]
fn a_transcription_is_a_new_synth_track_pattern_and_clip_in_one_undo_step() {
    let (mut d, clip_id) = song();
    let before = d.model().clone();
    d.apply(
        transcription(
            clip_id,
            vec![NoteEvent::new(0, BEAT, 52), bent_note()],
            vec![chord(12 * BEAT, 4 * BEAT, &[0, 4, 7], "C")],
        ),
        false,
    )
    .unwrap();
    let m = d.model();
    let track = m.tracks.last().unwrap();
    assert_eq!(track.name, "take 3 notes");
    assert_eq!(track.kind(), TrackKind::Midi);
    assert_eq!(track.voice, Track::new("x").voice, "the default synth sound");
    let pattern = m.patterns.iter().find(|p| p.track_id == track.id).unwrap();
    assert_eq!(pattern.name, "take 3 notes 1");
    assert_eq!(pattern.length_ticks, 8 * BEAT);
    assert_eq!(pattern.notes.len(), 2);
    assert_eq!(pattern.notes[1].pitch_envelope.len(), 2, "the bend survives");
    let clip = m.clips.iter().find(|c| c.pattern_id == pattern.id).unwrap();
    assert_eq!((clip.start_tick, clip.length_ticks), (4 * BEAT, 8 * BEAT));
    assert_eq!(m.detections.len(), 1);
    // The progression is untouched until a chord is accepted.
    assert_eq!(m.chords, before.chords);
    assert_eq!(d.undo_label(), Some("detect notes"));

    assert!(d.undo());
    assert_eq!(d.model(), &before, "one undo takes all of it back");
}

#[test]
fn an_all_strummed_take_writes_no_track_and_running_again_replaces_its_candidates() {
    let (mut d, clip_id) = song();
    let tracks = d.model().tracks.len();
    d.apply(
        transcription(clip_id, vec![], vec![chord(4 * BEAT, 4 * BEAT, &[7, 11, 2], "G")]),
        false,
    )
    .unwrap();
    assert_eq!(d.model().tracks.len(), tracks, "no notes, no track");
    assert_eq!(d.model().detections[0].chords.len(), 1);

    d.apply(
        transcription(
            clip_id,
            vec![],
            vec![
                chord(4 * BEAT, 2 * BEAT, &[7, 11, 2], "G"),
                chord(6 * BEAT, 2 * BEAT, &[2, 6, 9], "D"),
            ],
        ),
        false,
    )
    .unwrap();
    assert_eq!(d.model().detections.len(), 1, "one entry per clip");
    assert_eq!(d.model().detections[0].chords.len(), 2);
}

#[test]
fn a_transcription_for_a_take_that_is_gone_is_refused() {
    let (mut d, _) = song();
    let before = d.model().clone();
    assert_eq!(
        d.apply(transcription(Uuid::new_v4(), vec![NoteEvent::new(0, BEAT, 60)], vec![]), false),
        Err(EditError::NoSuchAudioClip)
    );
    assert_eq!(d.model(), &before);
}

#[test]
fn accepting_lays_the_chord_over_the_progression_and_undo_puts_both_lanes_back() {
    let (mut d, clip_id) = song();
    let am = chord(2 * BEAT, 4 * BEAT, &[9, 0, 4], "Am");
    let f = chord(8 * BEAT, 2 * BEAT, &[5, 9, 0], "F");
    d.apply(transcription(clip_id, vec![], vec![am.clone(), f.clone()]), false)
        .unwrap();
    let before = d.model().clone();

    d.apply(Command::AcceptDetectedChord { id: am.id }, false).unwrap();
    let m = d.model();
    let landed = m.chords.get(&am.id).expect("the chord is on the progression");
    assert_eq!((landed.start_tick, landed.length_ticks), (2 * BEAT, 4 * BEAT));
    assert_eq!(landed.pitch_classes, vec![9, 0, 4]);
    // The starter's first two chords (a bar each) are trimmed back around it.
    let spans: Vec<(i64, i64)> = m
        .chords
        .chords()
        .iter()
        .take(3)
        .map(|c| (c.start_tick, c.end_tick()))
        .collect();
    assert_eq!(spans, vec![(0, 2 * BEAT), (2 * BEAT, 6 * BEAT), (6 * BEAT, 8 * BEAT)]);
    // It left the detected lane; the other candidate is still waiting.
    let waiting: Vec<Uuid> = m.detections[0].chords.iter().map(|c| c.id).collect();
    assert_eq!(waiting, vec![f.id]);
    assert_eq!(d.undo_label(), Some("accept chord"));

    assert!(d.undo());
    assert_eq!(d.model(), &before);
}

#[test]
fn dismissing_drops_the_candidate_and_never_touches_the_progression() {
    let (mut d, clip_id) = song();
    let g = chord(4 * BEAT, 4 * BEAT, &[7, 11, 2], "G");
    d.apply(transcription(clip_id, vec![], vec![g.clone()]), false)
        .unwrap();
    let before = d.model().clone();
    d.apply(Command::DismissDetectedChord { id: g.id }, false).unwrap();
    assert_eq!(d.model().chords, before.chords);
    assert!(d.model().detections.is_empty(), "nothing left to show");
    assert_eq!(d.undo_label(), Some("dismiss chord"));
    assert!(d.undo());
    assert_eq!(d.model(), &before);

    assert_eq!(
        d.apply(Command::DismissDetectedChord { id: Uuid::new_v4() }, false),
        Err(EditError::NoSuchDetectedChord)
    );
    assert_eq!(
        d.apply(Command::AcceptDetectedChord { id: Uuid::new_v4() }, false),
        Err(EditError::NoSuchDetectedChord)
    );
}

#[test]
fn candidates_move_with_the_music_when_time_is_opened_or_cut() {
    let (mut d, clip_id) = song();
    let g = chord(8 * BEAT, 4 * BEAT, &[7, 11, 2], "G");
    d.apply(transcription(clip_id, vec![], vec![g.clone()]), false)
        .unwrap();
    d.apply(Command::InsertTime { at: 0, ticks: 4 * BEAT }, false)
        .unwrap();
    let moved = &d.model().detections[0].chords[0];
    assert_eq!((moved.id, moved.start_tick, moved.length_ticks), (g.id, 12 * BEAT, 4 * BEAT));
    // The take moved by the same four beats, so the detection still belongs to it.
    assert_eq!(d.model().audio_clips[0].start_tick, 8 * BEAT);
    d.apply(Command::DeleteTime { at: 12 * BEAT, ticks: 4 * BEAT }, false)
        .unwrap();
    assert!(d.model().detections[0].chords.is_empty(), "a chord wholly inside a cut goes with it");
}

#[test]
fn detections_are_saved_and_a_take_that_is_gone_takes_its_candidates_with_it() {
    let (mut d, clip_id) = song();
    let mut chroma = [0.0f32; 12];
    chroma[7] = 3.0;
    d.apply(
        Command::AddTranscription {
            name: "take 3 notes".into(),
            start_tick: 4 * BEAT,
            length_ticks: 8 * BEAT,
            notes: vec![],
            detection: ClipDetection {
                clip_id,
                chords: vec![chord(4 * BEAT, 4 * BEAT, &[7, 11, 2], "G")],
                chroma,
            },
        },
        false,
    )
    .unwrap();
    let json = d.model().to_json().unwrap();
    assert!(json.contains("\"detections\""));
    assert!(json.contains("\"clipId\"") && json.contains("\"confidence\""));
    let loaded = ProjectModel::from_json(&json).unwrap();
    assert_eq!(loaded.detections, d.model().detections);

    // A file whose take was deleted by hand loses the candidates on load.
    let mut orphaned = d.model().clone();
    orphaned.audio_clips.clear();
    let loaded = ProjectModel::from_json(&orphaned.to_json().unwrap()).unwrap();
    assert!(loaded.detections.is_empty());

    // A project that never ran detection does not mention it.
    assert!(!ProjectModel::starter().to_json().unwrap().contains("detections"));
}

#[test]
fn detected_chroma_feeds_the_key_detector_like_placed_notes() {
    // Three strums of G minor and two of D major, in note equivalents.
    let mut chroma = [0.0f32; 12];
    for (pc, w) in [(7, 3.0), (10, 3.0), (2, 5.0), (6, 2.0), (9, 2.0), (0, 1.0), (3, 1.0)] {
        chroma[pc] = w;
    }
    let r = KeyDetector::default().detect_with_chroma(&[], &chroma);
    let top = r.top().unwrap();
    assert_eq!((top.root_pitch_class, top.scale), (7, ScaleType::Minor));
    assert!(r.is_confident);
    // With nothing placed and nothing heard there is still nothing to say.
    assert!(KeyDetector::default().detect_with_chroma(&[], &[0.0; 12]).top().is_none());
    // Faint leakage is not a pitch class: two strong bins and ten whispers stay unconfident.
    let mut thin = [0.3f32; 12];
    thin[0] = 6.0;
    thin[7] = 6.0;
    assert!(!KeyDetector::default().detect_with_chroma(&[], &thin).is_confident);
}
