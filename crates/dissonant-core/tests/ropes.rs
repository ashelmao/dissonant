//! Ropes: a note that slides from another. When a rope is live, the commands that set and cut
//! it, and every path that removes or copies notes keeping ropes pointed at the right notes.

use dissonant_core::*;
use uuid::Uuid;

const BEAT: i64 = TICKS_PER_BEAT;

fn note(start: i64, len: i64, pitch: i32) -> NoteEvent {
    NoteEvent::new(start, len, pitch)
}

fn roped(mut n: NoteEvent, from: &NoteEvent) -> NoteEvent {
    n.slide_from = Some(from.id);
    n
}

/// The starter with its melody pattern replaced by `notes`; answers the pattern id.
fn doc_with(notes: Vec<NoteEvent>) -> (Document, Uuid) {
    let mut m = ProjectModel::starter();
    m.patterns[0].notes = notes;
    let p = m.patterns[0].id;
    (Document::new(m), p)
}

fn notes_of(d: &Document, p: Uuid) -> &[NoteEvent] {
    &d.model().pattern(&p).unwrap().notes
}

fn find(notes: &[NoteEvent], id: Uuid) -> &NoteEvent {
    notes.iter().find(|n| n.id == id).unwrap()
}

// ---------------------------------------------------------------- rope_is_live

#[test]
fn a_rope_is_live_while_the_source_still_sounds() {
    let a = note(0, BEAT, 60);
    assert!(rope_is_live(&a, &note(BEAT / 2, BEAT, 62)), "overlapping");
    assert!(rope_is_live(&a, &note(BEAT, BEAT, 62)), "touching counts");
    assert!(
        !rope_is_live(&a, &note(BEAT + 1, BEAT, 62)),
        "a gap, however small"
    );
    assert!(
        !rope_is_live(&a, &note(0, BEAT, 62)),
        "starting together is a chord, not a slide"
    );
    assert!(
        !rope_is_live(&note(BEAT, BEAT, 60), &note(0, BEAT, 62)),
        "the source comes later"
    );
}

#[test]
fn a_rope_is_measured_on_shifted_starts() {
    let a = note(0, BEAT, 60);
    let mut late = note(BEAT, BEAT, 62);
    late.shift_ticks = 1;
    assert!(!rope_is_live(&a, &late), "the shift pushes it off the end");
    let mut early = late.clone();
    early.shift_ticks = -BEAT / 2;
    assert!(rope_is_live(&a, &early));
    // A shift can't drag the target before tick 0, where it would meet a source starting there.
    let mut b = note(0, BEAT, 62);
    b.shift_ticks = -BEAT;
    assert!(!rope_is_live(&a, &b));
}

#[test]
fn a_muted_source_or_a_note_on_itself_is_never_live() {
    let mut a = note(0, BEAT, 60);
    let b = note(BEAT / 2, BEAT, 62);
    assert!(!rope_is_live(&a, &a));
    a.muted = true;
    assert!(!rope_is_live(&a, &b));
}

#[test]
fn a_rope_to_the_same_pitch_is_a_tie_and_stays_live() {
    let a = note(0, BEAT, 60);
    assert!(rope_is_live(&a, &note(BEAT, BEAT, 60)));
}

// ---------------------------------------------------------------- tidy_ropes

#[test]
fn tidying_keeps_chains_and_cuts_loops_self_ropes_and_dangling_ones() {
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    let c = roped(note(2 * BEAT, BEAT, 64), &b);
    let mut chain = vec![a.clone(), b.clone(), c.clone()];
    tidy_ropes(&mut chain);
    assert_eq!(
        chain,
        vec![a.clone(), b.clone(), c.clone()],
        "A → B → C is fine"
    );

    // A three-note loop is cut once, at its first note; the rest of it survives.
    let mut x = roped(a.clone(), &c);
    x.slide_secs = Some(0.3);
    let mut looped = vec![x, b.clone(), c.clone()];
    tidy_ropes(&mut looped);
    assert_eq!((looped[0].slide_from, looped[0].slide_secs), (None, None));
    assert_eq!(looped[1].slide_from, Some(a.id));
    assert_eq!(looped[2].slide_from, Some(b.id));

    let mut own = roped(a.clone(), &a);
    let mut gone = roped(note(BEAT, BEAT, 62), &note(0, 1, 1));
    gone.slide_secs = Some(1.0);
    let mut timed = note(2 * BEAT, BEAT, 64);
    timed.slide_secs = Some(1.0);
    own.slide_secs = Some(1.0);
    let mut bad = vec![own, gone, timed];
    tidy_ropes(&mut bad);
    for n in &bad {
        assert_eq!((n.slide_from, n.slide_secs), (None, None), "{n:?}");
    }
}

#[test]
fn tidying_stops_at_a_loop_further_up_without_cutting_the_note_below_it() {
    // D hangs off a B ⇄ C loop: the loop is cut at B (the first of it in order), D keeps its rope.
    let (b, c) = (note(BEAT, BEAT, 62), note(2 * BEAT, BEAT, 64));
    let d = roped(note(3 * BEAT, BEAT, 65), &c);
    let mut notes = vec![roped(b.clone(), &c), roped(c.clone(), &b), d.clone()];
    tidy_ropes(&mut notes);
    assert_eq!(notes[0].slide_from, None);
    assert_eq!(notes[1].slide_from, Some(b.id));
    assert_eq!(notes[2].slide_from, Some(c.id));
}

#[test]
fn sanitizing_clamps_the_slide_time_to_the_glide_range() {
    let mut n = note(0, BEAT, 60);
    n.slide_secs = Some(5.0);
    assert_eq!(n.clone().sanitized().slide_secs, Some(MAX_SLIDE_SECS));
    n.slide_secs = Some(-1.0);
    assert_eq!(n.clone().sanitized().slide_secs, Some(0.0));
    n.slide_secs = Some(f32::NAN);
    assert_eq!(
        n.sanitized().slide_secs,
        None,
        "garbage falls back to the patch's glide"
    );
}

// ---------------------------------------------------------------- commands

#[test]
fn set_rope_ropes_a_note_and_undo_takes_it_away() {
    let (a, b) = (note(0, BEAT, 60), note(BEAT, BEAT, 67));
    let (mut d, p) = doc_with(vec![a.clone(), b.clone()]);
    d.apply(
        Command::SetRope {
            pattern_id: p,
            note_id: b.id,
            source_id: a.id,
        },
        false,
    )
    .unwrap();
    assert_eq!(find(notes_of(&d, p), b.id).slide_from, Some(a.id));
    assert_eq!(d.undo_label(), Some("add rope"));
    assert!(d.undo());
    assert_eq!(find(notes_of(&d, p), b.id).slide_from, None);
    assert!(d.redo());
    assert_eq!(find(notes_of(&d, p), b.id).slide_from, Some(a.id));
}

#[test]
fn set_rope_replaces_an_existing_rope_and_allows_chains_and_chords() {
    let a = note(0, BEAT, 60);
    let b = note(0, BEAT, 64);
    let c = roped(note(BEAT, BEAT, 67), &a);
    let e = note(BEAT, BEAT, 71);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone(), c.clone(), e.clone()]);
    d.apply(
        Command::SetRope {
            pattern_id: p,
            note_id: c.id,
            source_id: b.id,
        },
        false,
    )
    .unwrap();
    assert_eq!(find(notes_of(&d, p), c.id).slide_from, Some(b.id));
    // Each note of the chord slides to its own target.
    d.apply(
        Command::SetRope {
            pattern_id: p,
            note_id: e.id,
            source_id: a.id,
        },
        false,
    )
    .unwrap();
    // And a chain: something can slide out of a slide note.
    let f = note(2 * BEAT, BEAT, 72);
    let mut notes = notes_of(&d, p).to_vec();
    notes.push(f.clone());
    d.apply(
        Command::SetNotes {
            pattern_id: p,
            notes,
        },
        false,
    )
    .unwrap();
    d.apply(
        Command::SetRope {
            pattern_id: p,
            note_id: f.id,
            source_id: c.id,
        },
        false,
    )
    .unwrap();
    let notes = notes_of(&d, p);
    assert_eq!(find(notes, e.id).slide_from, Some(a.id));
    assert_eq!(find(notes, f.id).slide_from, Some(c.id));
}

#[test]
fn set_rope_refuses_missing_notes_self_ropes_and_loops_and_changes_nothing() {
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    let c = roped(note(2 * BEAT, BEAT, 64), &b);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone(), c.clone()]);
    let before = d.model().clone();
    let rope = |note_id, source_id| Command::SetRope {
        pattern_id: p,
        note_id,
        source_id,
    };
    assert_eq!(
        d.apply(rope(a.id, Uuid::new_v4()), false),
        Err(EditError::NoSuchNote)
    );
    assert_eq!(
        d.apply(rope(Uuid::new_v4(), a.id), false),
        Err(EditError::NoSuchNote)
    );
    assert_eq!(d.apply(rope(a.id, a.id), false), Err(EditError::RopeLoop));
    assert_eq!(
        d.apply(rope(a.id, b.id), false),
        Err(EditError::RopeLoop),
        "A ⇄ B"
    );
    assert_eq!(
        d.apply(rope(a.id, c.id), false),
        Err(EditError::RopeLoop),
        "A → C → B → A"
    );
    assert_eq!(
        d.apply(
            Command::SetRope {
                pattern_id: Uuid::new_v4(),
                note_id: b.id,
                source_id: a.id
            },
            false
        ),
        Err(EditError::NoSuchPattern)
    );
    // A source on another pattern of the same track isn't in this pattern: ropes stay inside one.
    let other = d.model().patterns[1].id;
    assert_eq!(
        d.apply(
            Command::SetRope {
                pattern_id: other,
                note_id: b.id,
                source_id: a.id
            },
            false
        ),
        Err(EditError::NoSuchNote)
    );
    assert_eq!(d.model(), &before);
    assert!(!d.can_undo());
}

#[test]
fn clear_rope_cuts_the_rope_and_its_time() {
    let a = note(0, BEAT, 60);
    let mut b = roped(note(BEAT, BEAT, 62), &a);
    b.slide_secs = Some(0.4);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone()]);
    d.apply(
        Command::ClearRope {
            pattern_id: p,
            note_id: b.id,
        },
        false,
    )
    .unwrap();
    let got = find(notes_of(&d, p), b.id);
    assert_eq!((got.slide_from, got.slide_secs), (None, None));
    assert_eq!(d.undo_label(), Some("remove rope"));
    // Clearing a note with no rope is a no-op, not an error, and leaves no undo step.
    d.apply(
        Command::ClearRope {
            pattern_id: p,
            note_id: a.id,
        },
        false,
    )
    .unwrap();
    assert_eq!(d.undo_label(), Some("remove rope"));
    assert_eq!(
        d.apply(
            Command::ClearRope {
                pattern_id: p,
                note_id: Uuid::new_v4()
            },
            false
        ),
        Err(EditError::NoSuchNote)
    );
    assert!(d.undo());
    assert_eq!(find(notes_of(&d, p), b.id), &b);
}

#[test]
fn set_slide_time_clamps_resets_and_coalesces_a_drag() {
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone()]);
    let time = |secs| Command::SetSlideTime {
        pattern_id: p,
        note_id: b.id,
        secs,
    };
    for s in [0.1, 0.2, 0.3] {
        d.apply(time(Some(s)), true).unwrap();
    }
    d.commit_gesture();
    assert_eq!(find(notes_of(&d, p), b.id).slide_secs, Some(0.3));
    assert_eq!(d.undo_label(), Some("change slide time"));
    d.apply(time(Some(10.0)), false).unwrap();
    assert_eq!(find(notes_of(&d, p), b.id).slide_secs, Some(MAX_SLIDE_SECS));
    d.apply(time(None), false).unwrap();
    assert_eq!(
        find(notes_of(&d, p), b.id).slide_secs,
        None,
        "back to the patch's glide"
    );
    assert_eq!(
        d.apply(time(Some(f32::INFINITY)), false),
        Err(EditError::InvalidValue)
    );
    assert_eq!(
        d.apply(
            Command::SetSlideTime {
                pattern_id: p,
                note_id: a.id,
                secs: Some(0.5)
            },
            false
        ),
        Err(EditError::NoRope),
        "a time needs a rope to time"
    );
    // Undo walks back through None → 2 s → the one step the drag made.
    assert!(d.undo() && d.undo());
    assert_eq!(find(notes_of(&d, p), b.id).slide_secs, Some(0.3));
    assert!(d.undo());
    assert_eq!(find(notes_of(&d, p), b.id).slide_secs, None);
}

// ---------------------------------------------------------------- integrity

#[test]
fn deleting_a_note_cuts_the_ropes_that_slid_from_it() {
    let a = note(0, BEAT, 60);
    let mut b = roped(note(BEAT, BEAT, 62), &a);
    b.slide_secs = Some(0.2);
    let c = roped(note(2 * BEAT, BEAT, 64), &b);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone(), c.clone()]);
    // The roll deletes by committing the array without the note.
    d.apply(
        Command::SetNotes {
            pattern_id: p,
            notes: vec![b.clone(), c.clone()],
        },
        false,
    )
    .unwrap();
    let notes = notes_of(&d, p);
    assert_eq!((notes[0].slide_from, notes[0].slide_secs), (None, None));
    assert_eq!(
        notes[1].slide_from,
        Some(b.id),
        "the rope downstream is untouched"
    );
    assert!(d.undo());
    assert_eq!(find(notes_of(&d, p), b.id).slide_from, Some(a.id));
}

#[test]
fn moving_a_note_keeps_its_rope_even_when_it_goes_dead() {
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone()]);
    let moved = NoteEvent {
        start_tick: 4 * BEAT,
        ..b.clone()
    };
    d.apply(
        Command::SetNotes {
            pattern_id: p,
            notes: vec![a.clone(), moved],
        },
        false,
    )
    .unwrap();
    let got = find(notes_of(&d, p), b.id);
    assert_eq!(got.slide_from, Some(a.id));
    assert!(!rope_is_live(find(notes_of(&d, p), a.id), got));
}

#[test]
fn make_clip_unique_forks_the_ropes_into_the_copy() {
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    let (mut d, p) = doc_with(vec![a.clone(), b.clone()]);
    let clip = d
        .model()
        .clips
        .iter()
        .find(|c| c.pattern_id == p)
        .unwrap()
        .id;
    d.apply(Command::MakeClipUnique { id: clip }, false)
        .unwrap();
    let fork = d.model().clip(&clip).unwrap().pattern_id;
    let notes = notes_of(&d, fork);
    assert_ne!(fork, p);
    let (fa, fb) = (notes[0].id, &notes[1]);
    assert_ne!(fa, a.id);
    assert_eq!(fb.slide_from, Some(fa));
}

#[test]
fn ghost_repeats_rope_each_repetition_to_its_own_source() {
    let mut model = ProjectModel::starter();
    let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    model.patterns[0].length_ticks = 4 * BEAT;
    model.patterns[0].notes = vec![a.clone(), b.clone()];
    let seq = Sequence::from_pattern(&model, &drums, 0)
        .unwrap()
        .with_ghost(&model, &melody, 1.0);
    let ghost = &seq.tracks[0].notes;
    let drum_len = model.patterns[1].length_ticks;
    let turns = (drum_len + 4 * BEAT - 1) / (4 * BEAT);
    assert_eq!(ghost.len() as i64, 2 * turns);
    for pair in ghost.chunks(2) {
        let (src, slide) = (&pair[0], &pair[1]);
        assert_eq!(
            slide.slide_from,
            Some(src.id),
            "each turn's slide points at its own turn's source"
        );
        assert!(rope_is_live(src, slide));
    }
}

#[test]
fn a_ghost_rope_whose_source_never_plays_is_dropped() {
    let mut model = ProjectModel::starter();
    let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
    model.patterns[1].length_ticks = 4 * BEAT;
    model.patterns[1].notes.clear();
    // A 4-beat window under an 8-beat ghost: the slide plays, its source (beat 5) never does.
    let src = note(5 * BEAT, BEAT, 60);
    let slide = roped(note(2 * BEAT, BEAT, 62), &src);
    model.patterns[0].length_ticks = 8 * BEAT;
    model.patterns[0].notes = vec![slide.clone(), src];
    let seq = Sequence::from_pattern(&model, &drums, 0)
        .unwrap()
        .with_ghost(&model, &melody, 1.0);
    let ghost = &seq.tracks[0].notes;
    assert_eq!(ghost.len(), 1);
    assert_eq!(ghost[0].slide_from, None);
}

#[test]
fn song_flattening_ropes_each_occurrence_to_its_own_source() {
    let mut model = ProjectModel::starter();
    let p = model.patterns[0].id;
    let track = model.tracks[0].id;
    let a = note(0, BEAT, 60);
    let b = roped(note(BEAT, BEAT, 62), &a);
    model.patterns[0].length_ticks = 4 * BEAT;
    model.patterns[0].notes = vec![a, b];
    let mut first = Clip::new(p, 0, 8 * BEAT); // two repetitions
    first.id = Uuid::new_v4();
    let mut second = Clip::new(p, 8 * BEAT, 4 * BEAT);
    second.id = Uuid::new_v4();
    model.clips = vec![first, second];
    let flat = Arrangement::flattened_notes(&track, &model.patterns, &model.clips);
    assert_eq!(flat.len(), 6);
    let ids: std::collections::HashSet<_> = flat.iter().map(|n| n.id).collect();
    assert_eq!(ids.len(), 6);
    for pair in flat.chunks(2) {
        assert_eq!(pair[1].slide_from, Some(pair[0].id));
        assert_eq!(pair[0].slide_from, None);
    }

    // A clip that starts after the source cuts it off; the slide plays without its rope.
    let mut late = Clip::new(p, 0, 4 * BEAT);
    late.id = Uuid::new_v4();
    late.offset_ticks = BEAT / 2;
    model.clips = vec![late];
    let flat = Arrangement::flattened_notes(&track, &model.patterns, &model.clips);
    let slide = flat.iter().find(|n| n.pitch == 62).unwrap();
    assert_eq!(slide.slide_from, None);
}

#[test]
fn midi_import_writes_no_ropes() {
    use dissonant_core::midi_file::{export, import, MidiScope};
    let (a, b) = (note(0, BEAT, 60), note(BEAT / 2, BEAT, 64));
    let mut model = ProjectModel::starter();
    model.patterns[0].notes = vec![a.clone(), roped(b, &a)];
    let bytes = export(&model, MidiScope::Song).unwrap();
    let imported = import(&bytes, model.bar_ticks()).unwrap();
    let notes: Vec<&NoteEvent> = imported.tracks.iter().flat_map(|t| &t.notes).collect();
    assert!(!notes.is_empty());
    assert!(notes
        .iter()
        .all(|n| n.slide_from.is_none() && n.slide_secs.is_none()));
}
