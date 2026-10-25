//! The flattened, engine-facing view of what to play: one list of notes per track, the chord
//! track for the bed, and a loop length. Built from either one pattern (pattern mode) or the
//! whole song (song mode). The engine never looks at `ProjectModel` directly.

use crate::arrangement::Arrangement;
use crate::chord_track::ChordTrack;
use crate::model::{NoteEvent, ProjectModel, Track};
use crate::tempo::TempoMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct SequenceTrack {
    pub id: Uuid,
    pub voice: String,
    pub is_drum: bool,
    pub muted: bool,
    pub soloed: bool,
    pub volume: f64,
    pub reverb_send: f64,
    pub tone: f64,
    pub pan: f64,
    pub notes: Vec<NoteEvent>,
}

impl SequenceTrack {
    fn from_track(track: &Track, notes: Vec<NoteEvent>) -> Self {
        SequenceTrack {
            id: track.id,
            voice: track.voice.clone(),
            is_drum: track.is_drum,
            muted: track.muted,
            soloed: track.soloed,
            volume: track.volume,
            reverb_send: track.reverb_send,
            tone: track.tone,
            pan: track.pan,
            notes,
        }
    }

    /// Audible given the project-wide solo state.
    pub fn audible(&self, any_solo: bool) -> bool {
        if any_solo {
            self.soloed
        } else {
            !self.muted
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sequence {
    pub tracks: Vec<SequenceTrack>,
    pub chords: ChordTrack,
    pub length_beats: f64,
    pub tempo_bpm: f64,
    /// Swing percent (50 = none) and grid, applied by the scheduler to note starts and ends.
    pub swing: f64,
    pub swing_grid: f64,
    /// Quarter-note beats per bar, for the metronome accent.
    pub beats_per_bar: f64,
    /// Beats ⇄ seconds. Constant at `tempo_bpm` in pattern mode; the song's map in song mode.
    pub tempo_map: TempoMap,
}

impl Sequence {
    /// One pattern on its own: only the pattern's track has notes (every other track is still
    /// there, silent, so live input and auditions can reach it). The chords are the project's
    /// progression from song beat `chord_offset_beats` for the pattern's length, shifted to 0 —
    /// the bar the pattern is being edited against.
    pub fn from_pattern(model: &ProjectModel, pattern_id: &Uuid, chord_offset_beats: f64) -> Option<Self> {
        let pattern = model.pattern(pattern_id)?;
        let length = pattern.length_beats.max(1.0);
        let offset = if chord_offset_beats.is_finite() { chord_offset_beats.max(0.0) } else { 0.0 };
        let tracks = model
            .tracks
            .iter()
            .map(|t| SequenceTrack::from_track(t, if t.id == pattern.track_id { pattern.notes.clone() } else { vec![] }))
            .collect();
        Some(Sequence {
            tracks,
            chords: ChordTrack::window(&model.chords, offset, length),
            length_beats: length,
            ..Self::silent(model, length)
        })
    }

    /// Also play another pattern (the ghost) on its own track, looped from beat 0 under this
    /// sequence's length and cut at its end, so the pattern being edited can be heard against
    /// it (a melody over the drums). Repeats get stable, distinct ids derived from the note id
    /// and the repetition. An unknown ghost id leaves the sequence as it was.
    ///
    /// `level` (0…1.5, 1 = as the track is) sets how loud the ghost plays. On a track of its own
    /// it scales that track's volume, which the engine changes without cutting notes; on the
    /// track being edited (which already has the edited pattern's notes) it scales the ghost
    /// notes' velocities instead, so the notes being written keep their level.
    pub fn with_ghost(mut self, model: &ProjectModel, ghost_id: &Uuid, level: f64) -> Self {
        let Some(ghost) = model.pattern(ghost_id) else { return self };
        let Some(track) = self.tracks.iter_mut().find(|t| t.id == ghost.track_id) else { return self };
        if !(ghost.length_beats > 0.0) {
            return self;
        }
        let level = if level.is_finite() { level.clamp(0.0, 1.5) } else { 1.0 };
        let shared = !track.notes.is_empty();
        if shared && level <= 0.0 {
            return self;
        }
        if !shared {
            track.volume *= level;
        }
        let velocity = |v: i32| if shared { ((v as f64) * level).round().clamp(1.0, 127.0) as i32 } else { v };
        let end = self.length_beats;
        let mut base = 0.0;
        let mut rep = 0u64;
        while base < end {
            for note in &ghost.notes {
                let start = base + note.start_beat;
                if start >= end {
                    continue;
                }
                let mut salt = b"ghost".to_vec();
                salt.extend_from_slice(&rep.to_le_bytes());
                track.notes.push(NoteEvent {
                    id: Uuid::new_v5(&note.id, &salt),
                    start_beat: start,
                    length_beats: (note.length_beats.min(end - start)).max(1.0 / 64.0),
                    velocity: velocity(note.velocity),
                    ..note.clone()
                });
            }
            base += ghost.length_beats;
            rep += 1;
        }
        track.notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        self
    }

    /// Every track with no notes and no chords, for pattern mode with nothing to edit.
    pub fn silent(model: &ProjectModel, length_beats: f64) -> Self {
        Sequence {
            tracks: model.tracks.iter().map(|t| SequenceTrack::from_track(t, vec![])).collect(),
            chords: ChordTrack::default(),
            length_beats: if length_beats.is_finite() { length_beats.max(1.0) } else { 16.0 },
            tempo_bpm: model.tempo,
            swing: model.swing,
            swing_grid: model.swing_grid,
            beats_per_bar: model.time_signature.beats_per_bar(),
            tempo_map: TempoMap::constant(model.tempo),
        }
    }

    /// The whole song: per track, the clips of that track's patterns; the project chords; and
    /// the song's explicit length as the hard end.
    pub fn from_song(model: &ProjectModel) -> Self {
        let tracks = model
            .tracks
            .iter()
            .map(|t| {
                SequenceTrack::from_track(
                    t,
                    Arrangement::flattened_notes(&t.id, &model.patterns, &model.clips),
                )
            })
            .collect();
        // Same floor as `from_pattern`: a non-positive loop length stops song mode from ever
        // wrapping and makes offline render bail out.
        let length = if model.song_length_beats.is_finite() { model.song_length_beats.max(1.0) } else { 16.0 };
        Sequence {
            tracks,
            chords: ChordTrack::new(model.chords.clone()),
            length_beats: length,
            tempo_bpm: model.tempo,
            swing: model.swing,
            swing_grid: model.swing_grid,
            beats_per_bar: model.time_signature.beats_per_bar(),
            tempo_map: model.tempo_map(),
        }
    }

    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.soloed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChordEvent, Clip};

    #[test]
    fn pattern_mode_plays_only_its_own_track() {
        let model = ProjectModel::starter();
        let (melody, drums) = (&model.patterns[0], &model.patterns[1]);
        let p = Sequence::from_pattern(&model, &melody.id, 0.0).unwrap();
        assert_eq!(p.length_beats, 16.0);
        assert_eq!(p.tracks.len(), 2, "every track is there");
        assert_eq!(p.tracks[0].notes.len(), melody.notes.len());
        assert!(p.tracks[1].notes.is_empty(), "the drums are silent while the melody is edited");
        let d = Sequence::from_pattern(&model, &drums.id, 0.0).unwrap();
        assert!(d.tracks[0].notes.is_empty());
        assert_eq!(d.tracks[1].notes.len(), drums.notes.len());
    }

    #[test]
    fn a_ghost_pattern_loops_under_the_edited_one_on_its_own_track() {
        let mut model = ProjectModel::starter();
        let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
        model.patterns[1].length_beats = 4.0;
        model.patterns[1].notes = vec![NoteEvent::new(0.0, 0.25, 36), NoteEvent::new(3.0, 2.0, 38)];
        let p = Sequence::from_pattern(&model, &melody, 0.0).unwrap().with_ghost(&model, &drums, 1.0);
        let starts: Vec<f64> = p.tracks[1].notes.iter().map(|n| n.start_beat).collect();
        assert_eq!(starts, vec![0.0, 3.0, 4.0, 7.0, 8.0, 11.0, 12.0, 15.0], "a 4-beat ghost loops four times under 16 beats");
        assert_eq!(p.tracks[1].notes.last().unwrap().length_beats, 1.0, "cut at the loop end");
        let ids: std::collections::HashSet<_> = p.tracks[1].notes.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), 8, "every repeat has its own id");
        assert_eq!(p.tracks[0].notes.len(), model.patterns[0].notes.len(), "the edited pattern is untouched");
        let same = Sequence::from_pattern(&model, &melody, 0.0).unwrap();
        assert_eq!(same.clone().with_ghost(&model, &Uuid::new_v4(), 1.0), same, "an unknown ghost changes nothing");
    }

    #[test]
    fn the_ghost_level_scales_its_own_track_or_only_its_notes_on_a_shared_one() {
        let mut model = ProjectModel::starter();
        let (melody, drums) = (model.patterns[0].id, model.patterns[1].id);
        let base = Sequence::from_pattern(&model, &melody, 0.0).unwrap();
        let quiet = base.clone().with_ghost(&model, &drums, 0.5);
        assert_eq!(quiet.tracks[1].volume, base.tracks[1].volume * 0.5, "its own track: the track volume");
        assert_eq!(quiet.tracks[1].notes.iter().map(|n| n.velocity).collect::<Vec<_>>(), model.patterns[1].notes.iter().map(|n| n.velocity).collect::<Vec<_>>());
        assert_eq!(quiet.tracks[0], base.tracks[0], "the edited track is untouched");

        // A second melody pattern ghosted under the first: same track, so only its notes soften.
        let mut other = model.patterns[0].clone();
        other.id = Uuid::new_v4();
        other.notes = vec![NoteEvent::new(0.0, 1.0, 121)];
        other.notes[0].velocity = 100;
        let other_id = other.id;
        model.patterns.push(other);
        let base = Sequence::from_pattern(&model, &melody, 0.0).unwrap();
        let shared = base.clone().with_ghost(&model, &other_id, 0.5);
        assert_eq!(shared.tracks[0].volume, base.tracks[0].volume, "the track level stays");
        let ghost_notes: Vec<_> = shared.tracks[0].notes.iter().filter(|n| n.pitch == 121).map(|n| n.velocity).collect();
        assert!(!ghost_notes.is_empty() && ghost_notes.iter().all(|&v| v == 50), "{ghost_notes:?}");
        assert_eq!(base.clone().with_ghost(&model, &other_id, 0.0), base, "silent on a shared track: no ghost notes");
    }

    #[test]
    fn pattern_mode_chords_are_the_song_window_at_the_offset() {
        let mut model = ProjectModel::empty();
        model.patterns[0].length_beats = 8.0;
        model.chords = vec![
            ChordEvent::new(0.0, 4.0, vec![0, 4, 7], Some("C".into())),
            ChordEvent::new(4.0, 8.0, vec![5, 9, 0], Some("F".into())),
            ChordEvent::new(12.0, 4.0, vec![7, 11, 2], Some("G".into())),
        ];
        let pid = model.patterns[0].id;
        let at = |offset: f64| -> Vec<(f64, f64, String)> {
            Sequence::from_pattern(&model, &pid, offset)
                .unwrap()
                .chords
                .chords()
                .iter()
                .map(|c| (c.start_beat, c.length_beats, c.name.clone().unwrap()))
                .collect()
        };
        assert_eq!(at(0.0), vec![(0.0, 4.0, "C".into()), (4.0, 4.0, "F".into())], "from bar 1, cut at the pattern end");
        assert_eq!(at(8.0), vec![(0.0, 4.0, "F".into()), (4.0, 4.0, "G".into())], "a chord already sounding starts at 0");
        assert!(at(16.0).is_empty(), "past the progression: no chords");
        assert_eq!(at(f64::NAN), at(0.0), "a bad offset reads as bar 1");
    }

    #[test]
    fn song_mode_uses_the_explicit_length_and_project_chords() {
        let mut model = ProjectModel::starter();
        let melody = model.patterns[0].id;
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 1.0, 60)];
        model.clips = vec![Clip::new(melody, 0.0, 16.0), Clip::new(melody, 16.0, 16.0)];
        model.song_length_beats = 64.0;
        let s = Sequence::from_song(&model);
        assert_eq!(s.length_beats, 64.0, "the song ends where it says, not at the last clip");
        assert_eq!(s.tracks[0].notes.len(), 2);
        assert!(s.tracks[1].notes.is_empty(), "the drum pattern has no clips any more");
        assert_eq!(s.chords.chord_at(17.0).unwrap().name.as_deref(), Some("C"));
    }

    #[test]
    fn an_empty_song_still_has_its_length() {
        let mut model = ProjectModel::empty();
        model.song_length_beats = 8.0;
        assert_eq!(Sequence::from_song(&model).length_beats, 8.0);
        let silent = Sequence::silent(&model, 0.0);
        assert_eq!(silent.length_beats, 1.0);
        assert_eq!(silent.tracks.len(), 1);
    }
}
