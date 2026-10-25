//! Flattens the song's clips into one continuous timeline per track. A clip plays its pattern
//! looped from `offset_beats` for `length_beats`, starting at `start_beat`, on the pattern's
//! own track; notes that would run past the clip's end are cut there. Pure and testable; song
//! playback and the song-mode playhead read from these.

use crate::model::{Clip, NoteEvent, SongPattern};
use uuid::Uuid;

pub struct Arrangement;

impl Arrangement {
    /// End of the last clip (0 for an empty song). The song itself ends at the model's
    /// `song_length_beats`; this is only where the placed material stops.
    pub fn total_length(clips: &[Clip]) -> f64 {
        clips.iter().map(Clip::end_beat).fold(0.0, f64::max)
    }

    /// Visit every (clip, repetition index, pattern, timeline offset of that repetition) for
    /// the clips whose pattern belongs to `track_id`.
    fn repetitions<'a>(
        track_id: &'a Uuid,
        patterns: &'a [SongPattern],
        clips: &'a [Clip],
    ) -> impl Iterator<Item = (&'a Clip, u64, &'a SongPattern, f64)> + 'a {
        clips.iter().filter(|c| !c.muted).flat_map(move |clip| {
            let pattern = patterns.iter().find(|p| p.id == clip.pattern_id && &p.track_id == track_id);
            let mut out = Vec::new();
            if let Some(p) = pattern {
                if p.length_beats > 0.0 {
                    let mut rep = 0u64;
                    let mut pos = -clip.offset_beats.rem_euclid(p.length_beats);
                    while pos < clip.length_beats {
                        out.push((clip, rep, p, clip.start_beat + pos));
                        pos += p.length_beats;
                        rep += 1;
                    }
                }
            }
            out
        })
    }

    /// Notes for one track across the whole song: the clips of that track's patterns, and only
    /// those. Occurrence ids are derived deterministically from (note id, clip id, repetition)
    /// so repeats produce stable, distinct ids the engine can key on.
    pub fn flattened_notes(track_id: &Uuid, patterns: &[SongPattern], clips: &[Clip]) -> Vec<NoteEvent> {
        let mut result = Vec::new();
        for (clip, rep, pattern, base) in Self::repetitions(track_id, patterns, clips) {
            let clip_end = clip.end_beat();
            for note in &pattern.notes {
                let start = base + note.start_beat;
                if start < clip.start_beat || start >= clip_end {
                    continue;
                }
                let end = (start + note.length_beats).min(clip_end);
                let mut salt = clip.id.as_bytes().to_vec();
                salt.extend_from_slice(&rep.to_le_bytes());
                result.push(NoteEvent {
                    id: Uuid::new_v5(&note.id, &salt),
                    start_beat: start,
                    length_beats: (end - start).max(1.0 / 64.0),
                    pitch: note.pitch,
                    velocity: note.velocity,
                    intentional: note.intentional,
                    muted: note.muted,
                });
            }
        }
        result.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn melody() -> Uuid {
        Uuid::from_u128(1)
    }
    fn drums() -> Uuid {
        Uuid::from_u128(2)
    }

    fn pattern_a() -> SongPattern {
        let mut p = SongPattern::new(melody(), "A");
        p.notes = vec![NoteEvent::new(2.0, 1.0, 60)];
        p
    }
    fn pattern_b() -> SongPattern {
        let mut p = SongPattern::new(melody(), "B");
        p.notes = vec![NoteEvent::new(4.0, 1.0, 64)];
        p
    }

    #[test]
    fn total_length_is_the_last_clip_end() {
        let (a, b) = (pattern_a(), pattern_b());
        let clips = vec![Clip::new(a.id, 0.0, 16.0), Clip::new(b.id, 16.0, 16.0), Clip::new(a.id, 40.0, 8.0)];
        assert_eq!(Arrangement::total_length(&clips), 48.0);
        assert_eq!(Arrangement::total_length(&[]), 0.0);
    }

    #[test]
    fn notes_land_at_clip_positions() {
        let (a, b) = (pattern_a(), pattern_b());
        let clips = vec![Clip::new(a.id, 0.0, 16.0), Clip::new(b.id, 16.0, 16.0)];
        let notes = Arrangement::flattened_notes(&melody(), &[a.clone(), b.clone()], &clips);
        assert_eq!(notes.iter().map(|n| n.start_beat).collect::<Vec<_>>(), vec![2.0, 20.0]);
    }

    /// The point of track-owned patterns: a melody pattern sits over a different drum pattern,
    /// and each track hears only the clips of its own patterns.
    #[test]
    fn each_track_flattens_only_its_own_patterns() {
        let p1 = pattern_a();
        let mut p2 = SongPattern::new(drums(), "beat");
        p2.length_beats = 4.0;
        p2.notes = vec![NoteEvent::new(0.0, 0.25, 36)];
        let patterns = [p1.clone(), p2.clone()];
        // P1 once over 16 beats, P2 looping under it.
        let clips = vec![Clip::new(p1.id, 0.0, 16.0), Clip::new(p2.id, 0.0, 16.0)];
        let m = Arrangement::flattened_notes(&melody(), &patterns, &clips);
        let d = Arrangement::flattened_notes(&drums(), &patterns, &clips);
        assert_eq!(m.iter().map(|n| (n.start_beat, n.pitch)).collect::<Vec<_>>(), vec![(2.0, 60)]);
        assert_eq!(d.iter().map(|n| n.start_beat).collect::<Vec<_>>(), vec![0.0, 4.0, 8.0, 12.0]);
        assert!(d.iter().all(|n| n.pitch == 36));
        assert!(Arrangement::flattened_notes(&Uuid::from_u128(3), &patterns, &clips).is_empty());
    }

    #[test]
    fn a_long_clip_loops_its_pattern_and_a_short_one_trims() {
        let a = pattern_a();
        let long = vec![Clip::new(a.id, 0.0, 40.0)];
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &long);
        assert_eq!(notes.iter().map(|n| n.start_beat).collect::<Vec<_>>(), vec![2.0, 18.0, 34.0]);
        let short = vec![Clip::new(a.id, 0.0, 2.5)];
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &short);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].length_beats, 0.5, "cut at the clip end");
        let none = vec![Clip::new(a.id, 0.0, 1.0)];
        assert!(Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &none).is_empty());
    }

    #[test]
    fn offset_starts_mid_pattern_and_muted_clips_are_silent() {
        let a = pattern_a();
        let mut c = Clip::new(a.id, 0.0, 16.0);
        c.offset_beats = 2.0; // pattern beat 2 lands at clip start
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &[c.clone()]);
        assert_eq!(notes.iter().map(|n| n.start_beat).collect::<Vec<_>>(), vec![0.0]);
        c.muted = true;
        assert!(Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &[c]).is_empty());
    }

    #[test]
    fn repeats_get_distinct_stable_ids() {
        let a = pattern_a();
        let twice = vec![Clip::new(a.id, 0.0, 32.0)];
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &twice);
        assert_eq!(notes.len(), 2);
        assert_ne!(notes[0].id, notes[1].id);
        let again = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &twice);
        assert_eq!(notes[1].id, again[1].id);
    }
}
