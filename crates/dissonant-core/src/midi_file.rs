//! Standard MIDI files: export a pattern or the whole song (one MIDI track per Dissonant
//! track, chord names as markers), and read a file into tracks of notes for
//! [`crate::Command::ImportTracks`].

use crate::arrangement::Arrangement;
use crate::chord_track::ChordTrack;
use crate::model::{NoteEvent, ProjectModel, Track};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use thiserror::Error;
use uuid::Uuid;

/// Ticks per quarter note written and assumed on import when the file says nothing.
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiScope {
    /// One pattern on its own track. Chord markers come from the project progression from
    /// song beat `chord_offset_beats` (what the pattern is being edited against).
    Pattern { id: Uuid, chord_offset_beats: f64 },
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

/// Everything an import yields; `length_beats` is the end of the last note rounded up to a bar.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedMidi {
    pub tracks: Vec<ImportedTrack>,
    pub tempo_bpm: Option<f64>,
    pub length_beats: f64,
}

fn ticks(beat: f64) -> u64 {
    (beat * PPQ as f64).round().max(0.0) as u64
}

type TrackNotes<'a> = Vec<(&'a Track, Vec<NoteEvent>)>;
type ChordMarkers = Vec<(f64, String)>;

/// Build the SMF (format 1) for a scope. Track 0 carries tempo, time signature and chord
/// markers; every Dissonant track in scope becomes one MIDI track (drums on channel 10): the
/// pattern's own track for a pattern, every track for the song.
pub fn export(model: &ProjectModel, scope: MidiScope) -> Result<Vec<u8>, MidiError> {
    let markers = |chords: &[crate::model::ChordEvent]| -> ChordMarkers {
        chords.iter().filter_map(|c| c.name.clone().map(|n| (c.start_beat, n))).collect()
    };
    // Gather (track, notes) and the chord list first; names must outlive the Smf.
    let (per_track, chords): (TrackNotes, ChordMarkers) = match scope {
        MidiScope::Pattern { id, chord_offset_beats } => {
            let p = model.pattern(&id).ok_or(MidiError::NoSuchPattern)?;
            let track = model.track(&p.track_id).ok_or(MidiError::NoSuchPattern)?;
            let offset = if chord_offset_beats.is_finite() { chord_offset_beats.max(0.0) } else { 0.0 };
            let window = ChordTrack::window(&model.chords, offset, p.length_beats.max(1.0));
            (vec![(track, p.notes.clone())], markers(window.chords()))
        }
        MidiScope::Song => {
            let end = model.song_length_beats;
            let in_song: Vec<_> = model.chords.iter().filter(|c| c.start_beat < end).cloned().collect();
            (
                model
                    .tracks
                    .iter()
                    .map(|t| (t, Arrangement::flattened_notes(&t.id, &model.patterns, &model.clips)))
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
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ))));

    // Conductor track.
    let mut conductor: Vec<(u64, TrackEventKind)> = vec![
        (0, TrackEventKind::Meta(MetaMessage::Tempo(u24::new((60_000_000.0 / model.tempo).round() as u32)))),
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
    for ((beat, _), name) in chords.iter().zip(marker_names.iter()) {
        conductor.push((ticks(*beat), TrackEventKind::Meta(MetaMessage::Marker(name.as_bytes()))));
    }
    smf.tracks.push(to_deltas(conductor));

    for ((track, notes), name) in per_track.iter().zip(names.iter()) {
        let channel = u4::new(if track.is_drum { 9 } else { 0 });
        let mut events: Vec<(u64, TrackEventKind)> = vec![(0, TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())))];
        for n in notes.iter().filter(|n| !n.muted) {
            let key = u7::new(n.pitch.clamp(0, 127) as u8);
            let vel = u7::new(n.velocity.clamp(1, 127) as u8);
            let on = ticks(n.start_beat);
            let off = ticks(n.end_beat()).max(on + 1);
            events.push((on, TrackEventKind::Midi { channel, message: MidiMessage::NoteOn { key, vel } }));
            events.push((off, TrackEventKind::Midi { channel, message: MidiMessage::NoteOff { key, vel: u7::new(0) } }));
        }
        smf.tracks.push(to_deltas(events));
    }

    let mut out = Vec::new();
    smf.write_std(&mut out).map_err(|e| MidiError::Write(e.to_string()))?;
    Ok(out)
}

/// Sort absolute-tick events (note-offs before note-ons at the same tick) and convert to deltas.
fn to_deltas(mut events: Vec<(u64, TrackEventKind<'_>)>) -> Vec<TrackEvent<'_>> {
    let rank = |k: &TrackEventKind| match k {
        TrackEventKind::Meta(_) => 0,
        TrackEventKind::Midi { message: MidiMessage::NoteOff { .. }, .. } => 1,
        _ => 2,
    };
    events.sort_by(|a, b| a.0.cmp(&b.0).then(rank(&a.1).cmp(&rank(&b.1))));
    let mut last = 0u64;
    let mut out: Vec<TrackEvent> = events
        .into_iter()
        .map(|(at, kind)| {
            let delta = at - last;
            last = at;
            TrackEvent { delta: u28::new(delta.min(u28::max_value().as_int() as u64) as u32), kind }
        })
        .collect();
    out.push(TrackEvent { delta: u28::new(0), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });
    out
}

/// Read a MIDI file into tracks of notes. Format 0 files (one track, many channels) are split
/// by channel; tracks without notes are dropped.
pub fn import(bytes: &[u8], beats_per_bar: f64) -> Result<ImportedMidi, MidiError> {
    let smf = Smf::parse(bytes).map_err(|e| MidiError::Parse(e.to_string()))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as f64,
        Timing::Timecode(..) => PPQ as f64,
    };
    let mut tempo_bpm = None;
    let mut tracks = Vec::new();
    for (ti, track) in smf.tracks.iter().enumerate() {
        let mut at = 0u64;
        let mut name: Option<String> = None;
        // Per (channel, key): start tick + velocity, for pairing note-offs.
        let mut held: std::collections::HashMap<(u8, u8), (u64, u8)> = std::collections::HashMap::new();
        let mut per_channel: std::collections::BTreeMap<u8, Vec<NoteEvent>> = std::collections::BTreeMap::new();
        for ev in track {
            at += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) => name = Some(String::from_utf8_lossy(n).trim().to_string()),
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
                                let start_beat = start as f64 / ppq;
                                let len = ((at - start) as f64 / ppq).max(1.0 / 64.0);
                                per_channel
                                    .entry(ch)
                                    .or_default()
                                    .push(NoteEvent::new(start_beat, len, key.as_int() as i32).with_velocity(vel as i32));
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
            per_channel
                .entry(ch)
                .or_default()
                .push(NoteEvent::new(start as f64 / ppq, 1.0, key as i32).with_velocity(vel as i32));
        }
        let multi = per_channel.len() > 1;
        for (ch, mut notes) in per_channel {
            notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat).then(a.pitch.cmp(&b.pitch)));
            let base = name.clone().unwrap_or_else(|| format!("midi {}", ti + 1));
            tracks.push(ImportedTrack {
                name: if multi { format!("{base} ch{}", ch + 1) } else { base },
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
        .flat_map(|t| t.notes.iter().map(NoteEvent::end_beat))
        .fold(0.0, f64::max);
    let bpb = beats_per_bar.max(1.0);
    let length_beats = ((end / bpb).ceil() * bpb).max(bpb);
    Ok(ImportedMidi { tracks, tempo_bpm, length_beats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Clip;

    fn pattern_scope(id: Uuid) -> MidiScope {
        MidiScope::Pattern { id, chord_offset_beats: 0.0 }
    }

    #[test]
    fn export_then_import_round_trips_notes_and_names() {
        let mut model = ProjectModel::starter();
        model.tempo = 100.0;
        let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
        model.tracks[0].name = "lead".into();
        let mut muted = NoteEvent::new(3.0, 1.0, 72);
        muted.muted = true;
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 0.5, 60).with_velocity(90), NoteEvent::new(1.5, 1.0, 67), muted];
        model.patterns[1].notes = vec![NoteEvent::new(0.0, 0.25, 36)];

        let bytes = export(&model, pattern_scope(melody)).unwrap();
        let back = import(&bytes, 4.0).unwrap();
        assert_eq!(back.tempo_bpm.map(|t| t.round()), Some(100.0));
        assert_eq!(back.tracks.len(), 1, "a pattern exports its own track only");
        let lead = &back.tracks[0];
        assert_eq!(lead.name, "lead");
        assert!(!lead.is_drum);
        assert_eq!(lead.notes.len(), 2, "muted notes are not exported");
        assert_eq!(lead.notes[0].pitch, 60);
        assert_eq!(lead.notes[0].velocity, 90);
        assert!((lead.notes[0].length_beats - 0.5).abs() < 1e-9);
        assert!((lead.notes[1].start_beat - 1.5).abs() < 1e-9);
        assert_eq!(back.length_beats, 4.0);

        let kit = import(&export(&model, pattern_scope(drums)).unwrap(), 4.0).unwrap();
        assert_eq!(kit.tracks.len(), 1);
        assert_eq!(kit.tracks[0].name, "drums");
        assert!(kit.tracks[0].is_drum, "channel 10 marks drums");
    }

    #[test]
    fn song_export_follows_clips_and_empty_is_an_error() {
        let mut model = ProjectModel::starter();
        let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 1.0, 60)];
        model.patterns[1].notes = vec![NoteEvent::new(1.0, 0.25, 36)];
        model.clips = vec![Clip::new(melody, 0.0, 16.0), Clip::new(melody, 16.0, 16.0), Clip::new(drums, 0.0, 16.0)];
        let back = import(&export(&model, MidiScope::Song).unwrap(), 4.0).unwrap();
        assert_eq!(back.tracks.len(), 2, "every track with notes in the song");
        let lead = &back.tracks[0];
        assert_eq!(lead.notes.len(), 2);
        assert!((lead.notes[1].start_beat - 16.0).abs() < 1e-9);
        assert_eq!(back.tracks[1].notes.len(), 1, "the drums play their own clip only");
        assert_eq!(back.length_beats, 20.0, "last note ends at 17 → rounded up to the bar");

        let empty = ProjectModel::empty();
        assert!(matches!(export(&empty, MidiScope::Song), Err(MidiError::Empty)));
        assert!(matches!(export(&empty, pattern_scope(empty.patterns[0].id)), Err(MidiError::Empty)));
        assert!(matches!(export(&empty, pattern_scope(Uuid::new_v4())), Err(MidiError::NoSuchPattern)));
        assert!(matches!(import(b"not midi", 4.0), Err(MidiError::Parse(_))));
    }
}
