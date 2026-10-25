//! The chord track: ordered chords over time, and the lookup the rest of the engine depends
//! on — "what chord is sounding at beat T". This is the authoritative harmonic context that
//! drives the piano-roll tiering (R4, R10).
//!
//! Chords occupy half-open beat intervals `[start, start + length)`. Where two chords overlap,
//! the later-starting one wins (so a freshly dropped chord takes precedence).

use crate::model::ChordEvent;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordTrack {
    #[serde(default)]
    chords: Vec<ChordEvent>,
}

impl ChordTrack {
    pub fn new(mut chords: Vec<ChordEvent>) -> Self {
        chords.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        ChordTrack { chords }
    }

    pub fn chords(&self) -> &[ChordEvent] {
        &self.chords
    }

    /// The part of a song-beat progression inside `[from, from + length)`, shifted so `from`
    /// lands on beat 0 and cut to the window. A chord that started before the window counts
    /// from the window's start. This is what pattern mode hears and tiers against.
    pub fn window(chords: &[ChordEvent], from: f64, length: f64) -> ChordTrack {
        let end = from + length;
        let inside = chords
            .iter()
            .filter_map(|c| {
                let start = c.start_beat.max(from);
                let stop = c.end_beat().min(end);
                (stop > start).then(|| ChordEvent { start_beat: start - from, length_beats: stop - start, ..c.clone() })
            })
            .collect();
        ChordTrack::new(inside)
    }

    pub fn is_empty(&self) -> bool {
        self.chords.is_empty()
    }

    /// The chord sounding at `beat`, or `None` in a gap or empty track (drives cold start).
    pub fn chord_at(&self, beat: f64) -> Option<&ChordEvent> {
        self.chords.iter().rev().find(|c| c.contains(beat))
    }

    pub fn get(&self, id: &Uuid) -> Option<&ChordEvent> {
        self.chords.iter().find(|c| &c.id == id)
    }

    pub fn add(&mut self, chord: ChordEvent) {
        self.chords.push(chord);
        self.resort();
    }

    pub fn remove(&mut self, id: &Uuid) {
        self.chords.retain(|c| &c.id != id);
    }

    /// Replace a chord (matched by id) in place; re-sorts if its start moved.
    pub fn update(&mut self, chord: ChordEvent) {
        if let Some(existing) = self.chords.iter_mut().find(|c| c.id == chord.id) {
            *existing = chord;
            self.resort();
        }
    }

    /// Beat of the last chord's end (0 for an empty track).
    pub fn end_beat(&self) -> f64 {
        self.chords.iter().map(ChordEvent::end_beat).fold(0.0, f64::max)
    }

    fn resort(&mut self) {
        self.chords.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> ChordEvent {
        ChordEvent::new(0.0, 4.0, vec![0, 4, 7], Some("C".into()))
    }
    fn f() -> ChordEvent {
        ChordEvent::new(4.0, 4.0, vec![5, 9, 0], Some("F".into()))
    }

    #[test]
    fn lookup_across_boundaries_is_half_open() {
        let t = ChordTrack::new(vec![f(), c()]);
        assert_eq!(t.chord_at(0.0).unwrap().name.as_deref(), Some("C"));
        assert_eq!(t.chord_at(3.999).unwrap().name.as_deref(), Some("C"));
        assert_eq!(t.chord_at(4.0).unwrap().name.as_deref(), Some("F"));
        assert!(t.chord_at(8.0).is_none());
        assert!(t.chord_at(-1.0).is_none());
    }

    #[test]
    fn empty_track_is_none_everywhere() {
        let t = ChordTrack::default();
        assert!(t.is_empty());
        assert!(t.chord_at(0.0).is_none());
    }

    #[test]
    fn overlap_later_start_wins() {
        let mut t = ChordTrack::new(vec![c()]);
        t.add(ChordEvent::new(2.0, 4.0, vec![7, 11, 2], Some("G".into())));
        assert_eq!(t.chord_at(3.0).unwrap().name.as_deref(), Some("G"));
        assert_eq!(t.chord_at(1.0).unwrap().name.as_deref(), Some("C"));
    }

    #[test]
    fn window_shifts_to_zero_and_clips_at_both_edges() {
        let song = [c(), f(), ChordEvent::new(8.0, 4.0, vec![7, 11, 2], Some("G".into()))];
        let w = ChordTrack::window(&song, 2.0, 8.0);
        let spans: Vec<(f64, f64, &str)> =
            w.chords().iter().map(|c| (c.start_beat, c.length_beats, c.name.as_deref().unwrap())).collect();
        assert_eq!(spans, vec![(0.0, 2.0, "C"), (2.0, 4.0, "F"), (6.0, 2.0, "G")]);
        assert!(ChordTrack::window(&song, 20.0, 4.0).is_empty());
    }

    #[test]
    fn update_and_remove() {
        let mut t = ChordTrack::new(vec![c(), f()]);
        let id = t.chords()[1].id;
        let mut moved = t.get(&id).unwrap().clone();
        moved.start_beat = 8.0;
        t.update(moved);
        assert!(t.chord_at(5.0).is_none());
        assert_eq!(t.chord_at(9.0).unwrap().name.as_deref(), Some("F"));
        t.remove(&id);
        assert_eq!(t.chords().len(), 1);
        assert_eq!(t.end_beat(), 4.0);
    }
}
