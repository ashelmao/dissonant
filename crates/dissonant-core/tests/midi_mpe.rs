//! MIDI export of bending notes as MPE (dis-4zy), checked by parsing the exported bytes rather
//! than round-tripping them: import does not read bends back, so the file itself is the spec.

use dissonant_core::automation::Curve;
use dissonant_core::midi_file::{export, MidiScope};
use dissonant_core::{Clip, NoteEvent, PitchPoint, ProjectModel, TICKS_PER_BEAT};
use midly::{MidiMessage, Smf, TrackEventKind};

const BEAT: i64 = TICKS_PER_BEAT;
/// Project ticks per file tick (960 project PPQ over the file's 480).
const PER_FILE_TICK: i64 = 2;
/// MPE's default member-channel range, which the export sets explicitly.
const RANGE_CENTS: f32 = 4800.0;

fn pattern_scope(model: &ProjectModel) -> MidiScope {
    MidiScope::Pattern {
        id: model.patterns[0].id,
        chord_offset_ticks: 0,
    }
}

/// The MTrk chunks of an SMF, header and length prefix included, in file order.
fn chunks(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 14; // MThd + length + 6-byte header
    while at < bytes.len() {
        let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        out.push(&bytes[at..at + 8 + len]);
        at += 8 + len;
    }
    out
}

/// Every channel event of track `index`, at absolute file ticks, in file order.
fn channel_events(bytes: &[u8], index: usize) -> Vec<(i64, u8, MidiMessage)> {
    let smf = Smf::parse(bytes).unwrap();
    let mut tick = 0i64;
    let mut out = Vec::new();
    for e in &smf.tracks[index] {
        tick += e.delta.as_int() as i64;
        if let TrackEventKind::Midi { channel, message } = e.kind {
            out.push((tick, channel.as_int(), message));
        }
    }
    out
}

/// The envelope reading rule, written out independently of the exporter: zero before the
/// first point, ramps glide from the previous point (or zero at the start), steps jump, the
/// last value holds.
fn envelope_at(env: &[PitchPoint], tick: i64) -> f32 {
    let i = env.partition_point(|p| p.tick <= tick);
    let (prev_tick, prev) = if i == 0 {
        (0, 0.0)
    } else {
        (env[i - 1].tick, env[i - 1].cents)
    };
    match env.get(i) {
        Some(next) if next.curve == Curve::Ramp && next.tick > prev_tick => {
            prev + (next.cents - prev) * (tick - prev_tick) as f32 / (next.tick - prev_tick) as f32
        }
        _ => prev,
    }
}

fn bend_free() -> ProjectModel {
    let mut model = ProjectModel::starter();
    model.tempo = 120.0;
    model.tracks[0].name = "lead".into();
    let mut coloured = NoteEvent::new(BEAT, BEAT / 2, 64);
    coloured.color = 2;
    let mut late = NoteEvent::new(2 * BEAT, BEAT, 67);
    late.shift_ticks = BEAT / 4;
    model.patterns[0].notes = vec![
        NoteEvent::new(0, BEAT, 60).with_velocity(90),
        coloured,
        late,
    ];
    model
}

/// The lead track's chunk as the exporter wrote it before MPE existed: colour groups as
/// channels, no controllers, no bends.
const BEND_FREE_LEAD: &[u8] = &[
    77, 84, 114, 107, 0, 0, 0, 40, 0, 255, 3, 4, 108, 101, 97, 100, 0, 144, 60, 90, 131, 96, 128,
    60, 0, 0, 146, 64, 100, 129, 112, 130, 64, 0, 130, 104, 144, 67, 100, 131, 96, 128, 67, 0, 0,
    255, 47, 0,
];

#[test]
fn a_track_without_bends_exports_byte_for_byte_as_before() {
    let model = bend_free();
    let bytes = export(&model, pattern_scope(&model)).unwrap();
    let tracks = chunks(&bytes);
    assert_eq!(tracks.len(), 2, "conductor + lead");
    assert_eq!(tracks[1], BEND_FREE_LEAD);
}

#[test]
fn a_bending_track_leaves_its_neighbours_untouched() {
    let mut model = bend_free();
    let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
    model.patterns[1].notes = vec![NoteEvent::new(0, BEAT / 4, 36)];
    model.clips = vec![
        Clip::new(melody, 0, 4 * BEAT),
        Clip::new(drums, 0, 4 * BEAT),
    ];
    let before = export(&model, MidiScope::Song).unwrap();

    model.patterns[0].notes[0].pitch_envelope = vec![PitchPoint::new(BEAT / 2, 100.0, Curve::Ramp)];
    // A detuned drum hit stays a drum: channel 10, no MPE.
    model.patterns[1].notes[0].fine = 50;
    let after = export(&model, MidiScope::Song).unwrap();

    let (b, a) = (chunks(&before), chunks(&after));
    assert_eq!(b.len(), a.len());
    assert_ne!(b[1], a[1], "the bending lead went MPE");
    for (i, (b, a)) in b.iter().zip(&a).enumerate().filter(|(i, _)| *i != 1) {
        assert_eq!(b, a, "chunk {i} is unchanged");
    }
}

#[test]
fn bending_notes_export_one_per_member_channel_with_their_contour() {
    let mut model = ProjectModel::starter();
    let mut bent = NoteEvent::new(0, BEAT, 60);
    bent.pitch_envelope = vec![
        PitchPoint::new(BEAT / 2, 200.0, Curve::Ramp),
        PitchPoint::new(3 * BEAT / 4, -100.0, Curve::Step),
        // Past the note's end: never written.
        PitchPoint::new(2 * BEAT, 900.0, Curve::Step),
    ];
    bent.color = 5; // MPE spends the channels on pitch, so colour does not pick one.
    let mut detuned = NoteEvent::new(BEAT / 2, BEAT, 61);
    detuned.fine = -30;
    detuned.pitch_envelope = vec![PitchPoint::new(BEAT / 4, 50.0, Curve::Step)];
    let mut notes = vec![detuned, bent];
    // Fifteen plain notes after them, so the round-robin wraps past channel 16.
    for k in 0..15 {
        notes.push(NoteEvent::new(
            2 * BEAT + k * BEAT / 4,
            BEAT / 8,
            70 + k as i32,
        ));
    }
    let mut muted = NoteEvent::new(0, BEAT, 100);
    muted.muted = true;
    muted.fine = 10;
    notes.push(muted);
    model.patterns[0].notes = notes.clone();

    let bytes = export(&model, pattern_scope(&model)).unwrap();
    let events = channel_events(&bytes, 1);
    assert!(
        events.iter().all(|(_, ch, _)| *ch != 0),
        "channel 1 is the master channel and stays silent"
    );

    // RPN 0 = 48 semitones on every member channel, at the very start.
    for ch in 1..=15u8 {
        let ccs: Vec<(u8, u8)> = events
            .iter()
            .filter(|(t, c, _)| *t == 0 && *c == ch)
            .filter_map(|(_, _, m)| match m {
                MidiMessage::Controller { controller, value } => {
                    Some((controller.as_int(), value.as_int()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            ccs,
            vec![(101, 0), (100, 0), (6, 48), (38, 0), (101, 127), (100, 127)],
            "bend range on channel {}",
            ch + 1
        );
        let first = events.iter().position(|(_, c, _)| *c == ch).unwrap();
        assert!(
            matches!(events[first].2, MidiMessage::Controller { .. }),
            "the range is set before anything else on channel {}",
            ch + 1
        );
    }

    // One note per channel, round-robin over 2–16 in time order.
    let ons: Vec<(i64, u8, u8)> = events
        .iter()
        .filter_map(|(t, c, m)| match m {
            MidiMessage::NoteOn { key, .. } => Some((*t, *c, key.as_int())),
            _ => None,
        })
        .collect();
    assert_eq!(ons.len(), 17, "muted notes are still not exported");
    for (i, (_, ch, _)) in ons.iter().enumerate() {
        assert_eq!(*ch as usize, 1 + i % 15, "note {i}'s member channel");
    }
    assert_eq!(
        (ons[0].2, ons[1].2),
        (60, 61),
        "time order picks the channel"
    );

    // Every bend traces fine + envelope at its tick, from the note-on up to the note-off.
    let mut ramp_samples = 0;
    for (i, &(on, ch, key)) in ons.iter().enumerate() {
        let note = notes.iter().find(|n| n.pitch == key as i32).unwrap();
        let start = note.shifted_start();
        let off = events
            .iter()
            .find_map(|(t, c, m)| match m {
                MidiMessage::NoteOff { key: k, .. } if *c == ch && k.as_int() == key && *t > on => {
                    Some(*t)
                }
                _ => None,
            })
            .unwrap();
        // The note-on index, then the bends on this channel inside the note.
        let on_at = events
            .iter()
            .position(|(t, c, m)| *t == on && *c == ch && matches!(m, MidiMessage::NoteOn { .. }))
            .unwrap();
        let wheel = |m: &MidiMessage| match m {
            MidiMessage::PitchBend { bend } => Some(bend.as_int() as f32 * RANGE_CENTS / 8192.0),
            _ => None,
        };
        let onset = events[..on_at]
            .iter()
            .rev()
            .find(|(_, c, m)| *c == ch && wheel(m).is_some())
            .expect("a bend is written for the note's start");
        assert_eq!(
            onset.0, on,
            "note {i}: the onset bend lands on the note-on tick"
        );

        let bends: Vec<(i64, f32)> = events
            .iter()
            .filter(|(t, c, _)| *c == ch && *t >= on && *t < off)
            .filter_map(|(t, _, m)| wheel(m).map(|cents| (*t, cents)))
            .collect();
        assert!(!bends.is_empty());
        for (t, cents) in &bends {
            let offset = t * PER_FILE_TICK - start;
            let expected = note.fine as f32 + envelope_at(&note.pitch_envelope, offset);
            assert!(
                (cents - expected).abs() <= 1.0,
                "note {i} at offset {offset}: wheel {cents} vs envelope {expected}"
            );
        }
        if key == 60 {
            ramp_samples = bends
                .iter()
                .filter(|(t, _)| t * PER_FILE_TICK < BEAT / 2)
                .count();
            let at = |tick: i64| {
                bends
                    .iter()
                    .find(|(t, _)| t * PER_FILE_TICK == tick)
                    .unwrap()
                    .1
            };
            assert!((at(BEAT / 2) - 200.0).abs() <= 1.0, "the ramp arrives");
            assert!(
                (at(3 * BEAT / 4) + 100.0).abs() <= 1.0,
                "the step jumps on arrival"
            );
            assert!(
                bends.iter().all(|(_, c)| (c - 900.0).abs() > 1.0),
                "nothing past the note-off"
            );
        }
        if key == 61 {
            assert_eq!(
                bends.len(),
                2,
                "a held detune and one step: two messages, no resampling of flats"
            );
            assert!((bends[0].1 + 30.0).abs() <= 1.0, "fine alone at the start");
            assert!((bends[1].1 - 20.0).abs() <= 1.0, "fine + the step");
        }
        if note.pitch_envelope.is_empty() && note.fine == 0 {
            assert_eq!(
                bends,
                vec![(on, 0.0)],
                "a plain note resets its channel to centre"
            );
        }
    }
    // A half-beat ramp at 1/96 beat comes out as one message per step.
    assert_eq!(ramp_samples, 48, "the ramp is sampled every 1/96 beat");
}
