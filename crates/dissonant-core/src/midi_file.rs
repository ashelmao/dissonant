//! Standard MIDI files: export a pattern or the whole song (one MIDI track per Dissonant
//! track, chord names as markers), and import a file into a pattern as new tracks.
//!
//! A track whose notes bend — a pitch envelope or a `fine` detune — exports MPE-style: plain
//! MIDI has one pitch wheel per channel, so the only faithful way to give each note its own
//! contour is to give each note its own channel. Tracks that don't bend export exactly as they
//! always did; MPE costs a reader fifteen channels and is only worth it when it carries pitch.

use crate::arrangement::Arrangement;
use crate::automation::Curve;
use crate::chord_track::ChordTrack;
use crate::model::{NoteEvent, PitchPoint, ProjectModel, Track, MIN_NOTE_TICKS, ROPE_OVERLAP_SECS};
use crate::sequence::live_ropes;
use crate::ticks::TICKS_PER_BEAT;
use midly::num::{u15, u24, u28, u4, u7};
use midly::{
    Format, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent, TrackEventKind,
};
use thiserror::Error;
use uuid::Uuid;

/// Pitch-bend range set on every MPE member channel, in semitones. ±48 is the MPE spec's
/// default, so a receiver that ignores the RPN still reads the bends right; it also covers the
/// widest bend the model can hold (`fine` ±12 plus an envelope ±12) with room to spare, at
/// 4800 / 8192 ≈ 0.59 cents per wheel step — finer than anything the envelope stores.
pub const MPE_BEND_RANGE_SEMITONES: u8 = 48;

/// How often a ramp is sampled into pitch-bend messages, in project ticks: 1/96 beat, five
/// file ticks, about 5 ms at 120 BPM. MIDI has no ramps, only wheel positions, so a glide has
/// to be drawn as a staircase; at this rate a semitone ramp over one beat steps about one cent
/// at a time and even a fast whole-tone slide over a sixteenth stays within the 5 ms a synth's
/// own bend smoothing covers. Coarser is audible as zipper on slow bends; finer only bloats the
/// file. Holds and steps cost one message each, and unchanged values are never written.
pub const BEND_RAMP_STEP_TICKS: i64 = TICKS_PER_BEAT / 96;

/// MPE member channels as zero-based channel numbers: 2–16. Channel 1 is the lower zone's
/// master channel and is left alone.
const MPE_MEMBER_CHANNELS: u8 = 15;

/// Ticks per quarter note written and assumed on import when the file says nothing. This is
/// the file format's own resolution, not the project's: 960 % 480 == 0, so both directions are
/// exact and a round trip loses nothing.
pub const PPQ: u16 = 480;

#[derive(Debug, Error)]
pub enum MidiError {
    #[error("no such pattern")]
    NoSuchPattern,
    #[error("nothing to export")]
    Empty,
    #[error("not a MIDI file: {0}")]
    Parse(String),
    #[error("could not write MIDI: {0}")]
    Write(String),
}

/// What to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiScope {
    /// One pattern on its own track. Chord markers come from the project progression from
    /// song tick `chord_offset_ticks` (what the pattern is being edited against).
    Pattern { id: Uuid, chord_offset_ticks: i64 },
    /// Every track over the song length.
    Song,
}

/// One track read from a MIDI file, ready to become a Dissonant track.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedTrack {
    pub name: String,
    pub is_drum: bool,
    pub notes: Vec<NoteEvent>,
}

/// Everything an import yields; `length_ticks` is the end of the last note rounded up to a bar.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedMidi {
    pub tracks: Vec<ImportedTrack>,
    pub tempo_bpm: Option<f64>,
    pub length_ticks: i64,
}

/// A project tick on the file's own grid.
fn smf_tick(tick: i64) -> u64 {
    (tick * PPQ as i64 / TICKS_PER_BEAT).max(0) as u64
}

/// A file tick (at `ppq`) on the project's grid.
fn project_tick(tick: u64, ppq: i64) -> i64 {
    tick as i64 * TICKS_PER_BEAT / ppq
}

type TrackNotes<'a> = Vec<(&'a Track, Vec<NoteEvent>)>;
type ChordMarkers = Vec<(i64, String)>;

/// Build the SMF (format 1) for a scope. Track 0 carries tempo, time signature and chord
/// markers; every Dissonant track becomes one MIDI track (drums on channel 10).
pub fn export(model: &ProjectModel, scope: MidiScope) -> Result<Vec<u8>, MidiError> {
    // Gather (track, notes) and the chord list first; names must outlive the Smf.
    let markers = |chords: &[crate::model::ChordEvent]| -> ChordMarkers {
        chords
            .iter()
            .filter_map(|c| c.name.clone().map(|n| (c.start_tick, n)))
            .collect()
    };
    let (per_track, chords): (TrackNotes, ChordMarkers) = match scope {
        MidiScope::Pattern {
            id,
            chord_offset_ticks,
        } => {
            let p = model.pattern(&id).ok_or(MidiError::NoSuchPattern)?;
            let track = model.track(&p.track_id).ok_or(MidiError::NoSuchPattern)?;
            let window = ChordTrack::window(
                model.chords.chords(),
                chord_offset_ticks.max(0),
                p.length_ticks.max(1),
            );
            (vec![(track, p.notes.clone())], markers(window.chords()))
        }
        MidiScope::Song => {
            let end = model.song_length_ticks;
            let in_song: Vec<_> = model
                .chords
                .chords()
                .iter()
                .filter(|c| c.start_tick < end)
                .cloned()
                .collect();
            (
                model
                    .tracks
                    .iter()
                    .map(|t| {
                        (
                            t,
                            Arrangement::flattened_notes(&t.id, &model.patterns, &model.clips),
                        )
                    })
                    .collect(),
                markers(&in_song),
            )
        }
    };
    if per_track.iter().all(|(_, n)| n.is_empty()) {
        return Err(MidiError::Empty);
    }

    let names: Vec<String> = per_track.iter().map(|(t, _)| t.name.clone()).collect();
    let marker_names: Vec<String> = chords.iter().map(|(_, n)| n.clone()).collect();
    let mut smf = Smf::new(Header::new(
        Format::Parallel,
        Timing::Metrical(u15::new(PPQ)),
    ));

    // Conductor track.
    let mut conductor: Vec<(u64, TrackEventKind)> = vec![
        (
            0,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(
                (60_000_000.0 / model.tempo).round() as u32,
            ))),
        ),
        (
            0,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                model.time_signature.numerator as u8,
                (model.time_signature.denominator as f64).log2() as u8,
                24,
                8,
            )),
        ),
    ];
    for ((tick, _), name) in chords.iter().zip(marker_names.iter()) {
        conductor.push((
            smf_tick(*tick),
            TrackEventKind::Meta(MetaMessage::Marker(name.as_bytes())),
        ));
    }
    smf.tracks.push(to_deltas(conductor));

    let overlap = overlap_ticks(model.tempo);
    for ((track, notes), name) in per_track.iter().zip(names.iter()) {
        let mut events: Vec<(u64, TrackEventKind)> = vec![(
            0,
            TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
        )];
        // Everything in the file is heard, so no rope is cut for running past an end.
        let ropes = if track.is_drum {
            vec![]
        } else {
            live_ropes(notes, i64::MAX)
        };
        // Each note keeps its index in `notes`, which is what a rope points at: MPE reorders.
        let mut live: Vec<(usize, &NoteEvent)> =
            notes.iter().enumerate().filter(|(_, n)| !n.muted).collect();
        // Drums never go MPE: channel 10 *is* what makes them drums to every reader, and a
        // bent hit on channel 2 would come back as a pitched note.
        let mpe = !track.is_drum && live.iter().any(|(_, n)| bends(n));
        if mpe {
            for ch in 1..=MPE_MEMBER_CHANNELS {
                for message in bend_range_rpn() {
                    events.push((
                        0,
                        TrackEventKind::Midi {
                            channel: u4::new(ch),
                            message,
                        },
                    ));
                }
            }
            // Round-robin in time order, so a channel comes back round only after the
            // fourteen notes started since — the longest a channel can sit free.
            live.sort_by_key(|(_, n)| n.shifted_start());
        }
        for (turn, (i, n)) in live.into_iter().enumerate() {
            // A note's colour group is its MIDI channel, the way FL Studio treats colour.
            // Drums stay on channel 10 whatever colour they carry. MPE spends the channels
            // on pitch instead, so a bending track's colours do not survive export.
            let channel = u4::new(if track.is_drum {
                9
            } else if mpe {
                1 + (turn % MPE_MEMBER_CHANNELS as usize) as u8
            } else {
                n.color.min(15)
            });
            let key = u7::new(n.pitch.clamp(0, 127) as u8);
            let vel = u7::new(n.velocity.clamp(1, 127) as u8);
            let on = smf_tick(n.shifted_start());
            let mut off = smf_tick(n.shifted_start() + n.length_ticks).max(on + 1);
            if let Some(target) = ropes.get(i).and_then(|r| r.to) {
                off = rope_off(notes, i, target, off, overlap);
            }
            if mpe {
                // Pushed before the note-on: the sort is stable, so the channel is already
                // where the note starts when it sounds, rather than gliding there from the
                // previous note's last value.
                for (at, bend) in bend_events(n, off) {
                    events.push((
                        at,
                        TrackEventKind::Midi {
                            channel,
                            message: MidiMessage::PitchBend {
                                bend: PitchBend::from_int(bend),
                            },
                        },
                    ));
                }
            }
            events.push((
                on,
                TrackEventKind::Midi {
                    channel,
                    message: MidiMessage::NoteOn { key, vel },
                },
            ));
            events.push((
                off,
                TrackEventKind::Midi {
                    channel,
                    message: MidiMessage::NoteOff {
                        key,
                        vel: u7::new(0),
                    },
                },
            ));
        }
        smf.tracks.push(to_deltas(events));
    }

    let mut out = Vec::new();
    smf.write_std(&mut out)
        .map_err(|e| MidiError::Write(e.to_string()))?;
    Ok(out)
}

/// [`ROPE_OVERLAP_SECS`] in file ticks at the tempo the file is written at, never less than one
/// tick: a file has no finer way to say "still held".
fn overlap_ticks(bpm: f64) -> u64 {
    let beats = ROPE_OVERLAP_SECS * bpm.max(1.0) / 60.0;
    (beats * PPQ as f64).round().max(1.0) as u64
}

/// Where an exported note ends when a live rope slides out of it into `notes[target]`.
///
/// A file can't say "slide", but a synth in legato or mono mode glides when a key arrives
/// while the last one is still down. A source that only touches its slide note (ending at or
/// before its start, once on the file's grid) is held `overlap` ticks past that start, so the
/// file plays back sliding in another DAW; a source that already overlaps is left as written.
/// No portamento controllers (CC 5 / CC 65) are written: the slide time belongs to the synth
/// that plays the file, and turning portamento on for every note would glide notes that have no
/// rope.
///
/// The overlap needs two different keys to mean anything, so a rope between equal pitches (a
/// tie) is left alone, as is a source whose stretched end would swallow the next note of its
/// own key on its own channel: the note-off would end that note instead, or be paired with it
/// on import. A chord's ropes are each stretched, and a polyphonic synth simply hears every
/// pair overlap for a moment: one file can't glide several voices separately. On a bending
/// (MPE) track every note has a channel of its own, so the overlap only glides on a reader
/// that plays the whole zone as one voice.
fn rope_off(notes: &[NoteEvent], source: usize, target: usize, off: u64, overlap: u64) -> u64 {
    let (src, tgt) = (&notes[source], &notes[target]);
    let target_on = smf_tick(tgt.shifted_start());
    if off > target_on || src.pitch == tgt.pitch {
        return off;
    }
    let late = target_on + overlap;
    let clash = notes.iter().enumerate().any(|(k, n)| {
        k != source
            && !n.muted
            && n.pitch == src.pitch
            && n.color == src.color
            && (off..=late).contains(&smf_tick(n.shifted_start()))
    });
    if clash {
        off
    } else {
        late
    }
}

/// Whether a note needs its own pitch wheel. `fine` counts: plain MIDI has nowhere else to put
/// a detune, and dropping it would export a deliberately out-of-tune note in tune.
fn bends(n: &NoteEvent) -> bool {
    n.fine != 0 || !n.pitch_envelope.is_empty()
}

/// RPN 0 (pitch-bend sensitivity) set to [`MPE_BEND_RANGE_SEMITONES`], then the null RPN so a
/// stray data-entry message later cannot retune the range.
fn bend_range_rpn() -> [MidiMessage; 6] {
    let cc = |controller: u8, value: u8| MidiMessage::Controller {
        controller: u7::new(controller),
        value: u7::new(value),
    };
    [
        cc(101, 0),
        cc(100, 0),
        cc(6, MPE_BEND_RANGE_SEMITONES),
        cc(38, 0),
        cc(101, 127),
        cc(100, 127),
    ]
}

/// A note's envelope offset at `tick` after its start, in cents, by the envelope's reading
/// rule: zero before the first point, ramps glide from the previous point (or from zero at the
/// note's start), steps jump on arrival, the last value holds.
fn envelope_cents(env: &[PitchPoint], tick: i64) -> f32 {
    let i = env.partition_point(|p| p.tick <= tick);
    let (prev_tick, prev) = match i {
        0 => (0, 0.0),
        _ => (env[i - 1].tick, env[i - 1].cents),
    };
    match env.get(i) {
        Some(next) if next.curve == Curve::Ramp && next.tick > prev_tick => {
            let t = (tick - prev_tick) as f32 / (next.tick - prev_tick) as f32;
            prev + (next.cents - prev) * t.clamp(0.0, 1.0)
        }
        _ => prev,
    }
}

/// Cents to a signed 14-bit wheel position at [`MPE_BEND_RANGE_SEMITONES`].
fn cents_to_bend(cents: f32) -> i16 {
    let full = MPE_BEND_RANGE_SEMITONES as f32 * 100.0;
    (cents / full * 8192.0).round().clamp(-8192.0, 8191.0) as i16
}

/// The pitch-bend messages that trace one note's `fine` plus envelope, as (file tick, wheel)
/// in time order, all before the note-off at `off`.
///
/// The first message lands on the note-on tick and is always written, even when it is zero: the
/// channel still holds whatever the previous note left on it. After that, points become one
/// message each, ramps a message every [`BEND_RAMP_STEP_TICKS`], and anything that would not
/// move the wheel is skipped. Movement after the note-off is dropped — the channel is about to
/// be handed to another note, and a release tail bending under it would detune that note.
fn bend_events(n: &NoteEvent, off: u64) -> Vec<(u64, i16)> {
    let env = &n.pitch_envelope;
    let mut ticks = vec![0i64];
    let mut prev = 0i64;
    for p in env {
        if p.curve == Curve::Ramp {
            let mut t = prev + BEND_RAMP_STEP_TICKS;
            while t < p.tick {
                ticks.push(t);
                t += BEND_RAMP_STEP_TICKS;
            }
        }
        ticks.push(p.tick);
        prev = p.tick;
    }

    let start = n.shifted_start();
    let on = smf_tick(start);
    let mut out: Vec<(u64, i16)> = Vec::with_capacity(ticks.len());
    for t in ticks {
        let at = smf_tick(start + t).max(on);
        if at >= off {
            break;
        }
        let bend = cents_to_bend(n.fine as f32 + envelope_cents(env, t));
        match out.last_mut() {
            // Two project ticks share one file tick: the later value is where the wheel is.
            Some(last) if last.0 == at => last.1 = bend,
            Some(last) if last.1 == bend => {}
            _ => out.push((at, bend)),
        }
    }
    out
}

/// Sort absolute-tick events (note-offs before note-ons at the same tick) and convert to deltas.
fn to_deltas(mut events: Vec<(u64, TrackEventKind<'_>)>) -> Vec<TrackEvent<'_>> {
    let rank = |k: &TrackEventKind| match k {
        TrackEventKind::Meta(_) => 0,
        TrackEventKind::Midi {
            message: MidiMessage::NoteOff { .. },
            ..
        } => 1,
        _ => 2,
    };
    events.sort_by(|a, b| a.0.cmp(&b.0).then(rank(&a.1).cmp(&rank(&b.1))));
    let mut last = 0u64;
    let mut out: Vec<TrackEvent> = events
        .into_iter()
        .map(|(at, kind)| {
            let delta = at - last;
            last = at;
            TrackEvent {
                delta: u28::new(delta.min(u28::max_value().as_int() as u64) as u32),
                kind,
            }
        })
        .collect();
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    out
}

/// The channel a note arrived on becomes its colour group, so an export/import round trip
/// keeps the grouping. Channel 10 is percussion and carries no colour.
fn colored(mut note: NoteEvent, channel: u8) -> NoteEvent {
    note.color = if channel == 9 { 0 } else { channel.min(15) };
    note
}

/// Read a MIDI file into tracks of notes. Format 0 files (one track, many channels) are split
/// by channel; tracks without notes are dropped.
pub fn import(bytes: &[u8], bar_ticks: i64) -> Result<ImportedMidi, MidiError> {
    let smf = Smf::parse(bytes).map_err(|e| MidiError::Parse(e.to_string()))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as i64,
        Timing::Timecode(..) => PPQ as i64,
    }
    .max(1);
    let mut tempo_bpm = None;
    let mut tracks = Vec::new();
    for (ti, track) in smf.tracks.iter().enumerate() {
        let mut at = 0u64;
        let mut name: Option<String> = None;
        // Per (channel, key): start tick + velocity, for pairing note-offs.
        let mut held: std::collections::HashMap<(u8, u8), (u64, u8)> =
            std::collections::HashMap::new();
        let mut per_channel: std::collections::BTreeMap<u8, Vec<NoteEvent>> =
            std::collections::BTreeMap::new();
        for ev in track {
            at += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) => {
                    name = Some(String::from_utf8_lossy(n).trim().to_string())
                }
                TrackEventKind::Meta(MetaMessage::Tempo(t)) if tempo_bpm.is_none() => {
                    tempo_bpm = Some(60_000_000.0 / t.as_int() as f64);
                }
                TrackEventKind::Midi { channel, message } => {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            held.insert((ch, key.as_int()), (at, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            if let Some((start, vel)) = held.remove(&(ch, key.as_int())) {
                                let start_tick = project_tick(start, ppq);
                                let len = project_tick(at - start, ppq).max(MIN_NOTE_TICKS);
                                per_channel.entry(ch).or_default().push(
                                    colored(
                                        NoteEvent::new(start_tick, len, key.as_int() as i32),
                                        ch,
                                    )
                                    .with_velocity(vel as i32),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Anything still held at the end gets a beat.
        for ((ch, key), (start, vel)) in held {
            per_channel.entry(ch).or_default().push(
                colored(
                    NoteEvent::new(project_tick(start, ppq), TICKS_PER_BEAT, key as i32),
                    ch,
                )
                .with_velocity(vel as i32),
            );
        }
        let multi = per_channel.len() > 1;
        for (ch, mut notes) in per_channel {
            notes.sort_by(|a, b| a.start_tick.cmp(&b.start_tick).then(a.pitch.cmp(&b.pitch)));
            let base = name.clone().unwrap_or_else(|| format!("midi {}", ti + 1));
            tracks.push(ImportedTrack {
                name: if multi {
                    format!("{base} ch{}", ch + 1)
                } else {
                    base
                },
                is_drum: ch == 9,
                notes,
            });
        }
    }
    if tracks.is_empty() {
        return Err(MidiError::Empty);
    }
    let end = tracks
        .iter()
        .flat_map(|t| t.notes.iter().map(NoteEvent::end_tick))
        .max()
        .unwrap_or(0);
    let bar = bar_ticks.max(1);
    let length_ticks = ((end + bar - 1) / bar * bar).max(bar);
    Ok(ImportedMidi {
        tracks,
        tempo_bpm,
        length_ticks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Clip;

    const BEAT: i64 = TICKS_PER_BEAT;
    const BAR: i64 = 4 * BEAT;

    fn pattern_scope(id: Uuid) -> MidiScope {
        MidiScope::Pattern {
            id,
            chord_offset_ticks: 0,
        }
    }

    #[test]
    fn export_then_import_round_trips_notes_and_names() {
        let mut model = ProjectModel::starter();
        model.tempo = 100.0;
        let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
        model.tracks[0].name = "lead".into();
        let mut muted = NoteEvent::new(3 * BEAT, BEAT, 72);
        muted.muted = true;
        model.patterns[0].notes = vec![
            NoteEvent::new(0, BEAT / 2, 60).with_velocity(90),
            NoteEvent::new(3 * BEAT / 2, BEAT, 67),
            muted,
        ];
        model.patterns[1].notes = vec![NoteEvent::new(0, BEAT / 4, 36)];

        let bytes = export(&model, pattern_scope(melody)).unwrap();
        let back = import(&bytes, BAR).unwrap();
        assert_eq!(back.tempo_bpm.map(|t| t.round()), Some(100.0));
        assert_eq!(back.tracks.len(), 1, "a pattern exports its own track only");
        let lead = &back.tracks[0];
        assert_eq!(lead.name, "lead");
        assert!(!lead.is_drum);
        assert_eq!(lead.notes.len(), 2, "muted notes are not exported");
        assert_eq!(lead.notes[0].pitch, 60);
        assert_eq!(lead.notes[0].velocity, 90);
        assert_eq!(
            lead.notes[0].length_ticks,
            BEAT / 2,
            "960 → 480 → 960 is exact"
        );
        assert_eq!(lead.notes[1].start_tick, 3 * BEAT / 2);
        assert_eq!(back.length_ticks, BAR);

        let kit = import(&export(&model, pattern_scope(drums)).unwrap(), BAR).unwrap();
        assert_eq!(kit.tracks.len(), 1);
        assert_eq!(kit.tracks[0].name, "drums");
        assert!(kit.tracks[0].is_drum, "channel 10 marks drums");
    }

    #[test]
    fn song_export_follows_clips_and_empty_is_an_error() {
        let mut model = ProjectModel::starter();
        let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
        model.patterns[0].notes = vec![NoteEvent::new(0, BEAT, 60)];
        model.patterns[1].notes = vec![NoteEvent::new(BEAT, BEAT / 4, 36)];
        model.clips = vec![
            Clip::new(melody, 0, 16 * BEAT),
            Clip::new(melody, 16 * BEAT, 16 * BEAT),
            Clip::new(drums, 0, 16 * BEAT),
        ];
        let back = import(&export(&model, MidiScope::Song).unwrap(), BAR).unwrap();
        assert_eq!(back.tracks.len(), 2, "every track with notes in the song");
        let lead = &back.tracks[0];
        assert_eq!(lead.notes.len(), 2);
        assert_eq!(lead.notes[1].start_tick, 16 * BEAT);
        assert_eq!(
            back.tracks[1].notes.len(),
            1,
            "the drums play their own clip only"
        );
        assert_eq!(
            back.length_ticks,
            20 * BEAT,
            "last note ends at beat 17 → rounded up to the bar"
        );

        let empty = ProjectModel::empty();
        assert!(matches!(
            export(&empty, MidiScope::Song),
            Err(MidiError::Empty)
        ));
        assert!(matches!(
            export(&empty, pattern_scope(empty.patterns[0].id)),
            Err(MidiError::Empty)
        ));
        assert!(matches!(
            export(&empty, pattern_scope(Uuid::new_v4())),
            Err(MidiError::NoSuchPattern)
        ));
        assert!(matches!(import(b"not midi", BAR), Err(MidiError::Parse(_))));
    }

    /// Every note on the first exported track as `(key, on, off)` in file ticks, in the order
    /// the notes start.
    fn exported_notes(bytes: &[u8]) -> Vec<(u8, u64, u64)> {
        let smf = Smf::parse(bytes).unwrap();
        let mut at = 0u64;
        let mut held = std::collections::HashMap::new();
        let mut out = vec![];
        for ev in &smf.tracks[1] {
            at += ev.delta.as_int() as u64;
            if let TrackEventKind::Midi { message, .. } = ev.kind {
                match message {
                    MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                        held.insert(key.as_int(), at);
                    }
                    MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                        let on = held.remove(&key.as_int()).expect("an off pairs with an on");
                        out.push((key.as_int(), on, at));
                    }
                    _ => {}
                }
            }
        }
        out.sort_by_key(|&(key, on, _)| (on, key));
        out
    }

    #[test]
    fn a_rope_on_a_bending_track_finds_its_notes_after_mpe_reorders_them() {
        let mut model = ProjectModel::empty();
        model.tempo = 120.0;
        let pid = model.patterns[0].id;
        let mut muted = NoteEvent::new(0, BEAT, 72);
        muted.muted = true;
        let mut bent = NoteEvent::new(3 * BEAT, BEAT, 67);
        bent.fine = 20;
        let source = NoteEvent::new(BEAT, BEAT, 60);
        let mut slide = NoteEvent::new(2 * BEAT, BEAT, 64);
        slide.slide_from = Some(source.id);
        model.patterns[0].length_ticks = 4 * BEAT;
        // Stored out of time order, so the export's sort moves every note off its index.
        model.patterns[0].notes = vec![muted, bent, slide, source];
        let notes = exported_notes(&export(&model, pattern_scope(pid)).unwrap());
        let q = PPQ as u64;
        assert_eq!(
            notes,
            vec![(60, q, 2 * q + 5), (64, 2 * q, 3 * q), (67, 3 * q, 4 * q)],
            "the source is held into its own slide note, and no other note is"
        );
    }

    #[test]
    fn a_touching_roped_note_is_held_into_its_slide_note_in_the_file() {
        let mut model = ProjectModel::empty();
        model.tempo = 120.0;
        let pid = model.patterns[0].id;
        let roped = |source: &NoteEvent, mut n: NoteEvent| {
            n.slide_from = Some(source.id);
            n
        };
        let touching = NoteEvent::new(0, BEAT, 60);
        let overlapping = NoteEvent::new(2 * BEAT, 2 * BEAT, 60);
        let plain = NoteEvent::new(5 * BEAT, BEAT, 62);
        let gap = NoteEvent::new(8 * BEAT, BEAT, 60);
        let tie = NoteEvent::new(12 * BEAT, BEAT, 69);
        model.patterns[0].length_ticks = 16 * BEAT;
        model.patterns[0].notes = vec![
            touching.clone(),
            roped(&touching, NoteEvent::new(BEAT, BEAT, 64)),
            overlapping.clone(),
            roped(&overlapping, NoteEvent::new(3 * BEAT, BEAT, 67)),
            plain,
            NoteEvent::new(6 * BEAT, BEAT, 65), // touches `plain`, with no rope
            gap.clone(),
            roped(&gap, NoteEvent::new(10 * BEAT, BEAT, 64)), // dead: starts after `gap` ends
            tie.clone(),
            roped(&tie, NoteEvent::new(13 * BEAT, BEAT, 69)), // live, but the same key
        ];
        let notes = exported_notes(&export(&model, pattern_scope(pid)).unwrap());
        // 5 ms at 120 bpm is 4.8 file ticks at 480 PPQ.
        assert_eq!(overlap_ticks(120.0), 5);
        let q = PPQ as u64;
        assert_eq!(
            notes,
            vec![
                (60, 0, q + 5),
                (64, q, 2 * q),
                (60, 2 * q, 4 * q),
                (67, 3 * q, 4 * q),
                (62, 5 * q, 6 * q),
                (65, 6 * q, 7 * q),
                (60, 8 * q, 9 * q),
                (64, 10 * q, 11 * q),
                (69, 12 * q, 13 * q),
                (69, 13 * q, 14 * q),
            ],
            "only the touching source is held past its slide note's start"
        );

        // A stretched end that would swallow the next note of the same key is left as written.
        model.patterns[0].notes = vec![
            touching.clone(),
            roped(&touching, NoteEvent::new(BEAT, BEAT, 64)),
            NoteEvent::new(BEAT, BEAT / 2, 60),
        ];
        let notes = exported_notes(&export(&model, pattern_scope(pid)).unwrap());
        assert_eq!(notes[0], (60, 0, q), "{notes:?}");
    }

    #[test]
    fn colour_groups_round_trip_as_midi_channels_and_shift_moves_the_export() {
        let mut model = ProjectModel::starter();
        let melody = model.patterns[0].id;
        model.tracks[0].name = "lead".into();
        let mut red = NoteEvent::new(0, BEAT, 60);
        red.color = 3;
        let mut late = NoteEvent::new(2 * BEAT, BEAT, 64);
        late.shift_ticks = BEAT / 4;
        model.patterns[0].notes = vec![red, late];

        let back = import(&export(&model, pattern_scope(melody)).unwrap(), BAR).unwrap();
        // Two colour groups on one track come back as one track per channel.
        assert_eq!(back.tracks.len(), 2);
        let coloured = back.tracks.iter().find(|t| t.notes[0].pitch == 60).unwrap();
        assert_eq!(coloured.notes[0].color, 3, "channel 4 is colour group 3");
        let shifted = back.tracks.iter().find(|t| t.notes[0].pitch == 64).unwrap();
        assert_eq!(
            shifted.notes[0].start_tick,
            9 * BEAT / 4,
            "shift is baked into the exported time, not dropped"
        );
    }
}
