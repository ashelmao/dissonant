//! Flattens the song's clips into one continuous timeline per track. A clip plays its pattern
//! looped from `offset_ticks` for `length_ticks`, starting at `start_tick`, on the pattern's
//! own track; notes that would run past the clip's end are cut there. Pure and testable; song
//! playback and the song-mode playhead read from these.

use crate::model::{tidy_ropes, Clip, NoteEvent, SongPattern, MIN_NOTE_TICKS};
use uuid::Uuid;

pub struct Arrangement;

impl Arrangement {
    /// End of the last clip (0 for an empty song). The song itself ends at the model's
    /// `song_length_ticks`; this is only where the placed material stops.
    pub fn total_length(clips: &[Clip]) -> i64 {
        clips.iter().map(Clip::end_tick).max().unwrap_or(0)
    }

    /// Visit every (clip, repetition index, pattern, timeline offset of that repetition) for
    /// the clips whose pattern belongs to `track_id`.
    fn repetitions<'a>(
        track_id: &'a Uuid,
        patterns: &'a [SongPattern],
        clips: &'a [Clip],
    ) -> impl Iterator<Item = (&'a Clip, u64, &'a SongPattern, i64)> + 'a {
        clips.iter().filter(|c| !c.muted).flat_map(move |clip| {
            let pattern = patterns
                .iter()
                .find(|p| p.id == clip.pattern_id && &p.track_id == track_id);
            let mut out = Vec::new();
            if let Some(p) = pattern {
                if p.length_ticks > 0 {
                    let mut rep = 0u64;
                    let mut pos = -clip.offset_ticks.rem_euclid(p.length_ticks);
                    while pos < clip.length_ticks {
                        out.push((clip, rep, p, clip.start_tick + pos));
                        pos += p.length_ticks;
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
    pub fn flattened_notes(
        track_id: &Uuid,
        patterns: &[SongPattern],
        clips: &[Clip],
    ) -> Vec<NoteEvent> {
        let mut result = Vec::new();
        for (clip, rep, pattern, base) in Self::repetitions(track_id, patterns, clips) {
            let clip_end = clip.end_tick();
            for note in &pattern.notes {
                let start = base + note.start_tick;
                if start < clip.start_tick || start >= clip_end {
                    continue;
                }
                let end = (start + note.length_ticks).min(clip_end);
                let mut salt = clip.id.as_bytes().to_vec();
                salt.extend_from_slice(&rep.to_le_bytes());
                result.push(NoteEvent {
                    id: Uuid::new_v5(&note.id, &salt),
                    // Same salt, so a rope points at its source's occurrence in this very
                    // repetition of this very clip.
                    slide_from: note.slide_from.map(|s| Uuid::new_v5(&s, &salt)),
                    start_tick: start,
                    length_ticks: (end - start).max(MIN_NOTE_TICKS),
                    ..note.clone()
                });
            }
        }
        // A source the clip cut off was never emitted; its rope goes rather than dangle.
        tidy_ropes(&mut result);
        result.sort_by_key(|n| n.start_tick);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ticks::TICKS_PER_BEAT;

    const BEAT: i64 = TICKS_PER_BEAT;

    fn melody() -> Uuid {
        Uuid::from_u128(1)
    }
    fn drums() -> Uuid {
        Uuid::from_u128(2)
    }

    fn pattern_a() -> SongPattern {
        let mut p = SongPattern::new(melody(), "A");
        p.notes = vec![NoteEvent::new(2 * BEAT, BEAT, 60)];
        p
    }
    fn pattern_b() -> SongPattern {
        let mut p = SongPattern::new(melody(), "B");
        p.notes = vec![NoteEvent::new(4 * BEAT, BEAT, 64)];
        p
    }

    #[test]
    fn total_length_is_the_last_clip_end() {
        let (a, b) = (pattern_a(), pattern_b());
        let clips = vec![
            Clip::new(a.id, 0, 16 * BEAT),
            Clip::new(b.id, 16 * BEAT, 16 * BEAT),
            Clip::new(a.id, 40 * BEAT, 8 * BEAT),
        ];
        assert_eq!(Arrangement::total_length(&clips), 48 * BEAT);
        assert_eq!(Arrangement::total_length(&[]), 0);
    }

    #[test]
    fn notes_land_at_clip_positions() {
        let (a, b) = (pattern_a(), pattern_b());
        let clips = vec![
            Clip::new(a.id, 0, 16 * BEAT),
            Clip::new(b.id, 16 * BEAT, 16 * BEAT),
        ];
        let notes = Arrangement::flattened_notes(&melody(), &[a.clone(), b.clone()], &clips);
        assert_eq!(
            notes.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            vec![2 * BEAT, 20 * BEAT]
        );
    }

    /// The point of track-owned patterns: a melody pattern sits over a different drum pattern,
    /// and each track hears only the clips of its own patterns.
    #[test]
    fn each_track_flattens_only_its_own_patterns() {
        let p1 = pattern_a();
        let mut p2 = SongPattern::new(drums(), "beat");
        p2.length_ticks = 4 * BEAT;
        p2.notes = vec![NoteEvent::new(0, BEAT / 4, 36)];
        let patterns = [p1.clone(), p2.clone()];
        // P1 once over 16 beats, P2 looping under it.
        let clips = vec![
            Clip::new(p1.id, 0, 16 * BEAT),
            Clip::new(p2.id, 0, 16 * BEAT),
        ];
        let m = Arrangement::flattened_notes(&melody(), &patterns, &clips);
        let d = Arrangement::flattened_notes(&drums(), &patterns, &clips);
        assert_eq!(
            m.iter()
                .map(|n| (n.start_tick, n.pitch))
                .collect::<Vec<_>>(),
            vec![(2 * BEAT, 60)]
        );
        assert_eq!(
            d.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            vec![0, 4 * BEAT, 8 * BEAT, 12 * BEAT]
        );
        assert!(d.iter().all(|n| n.pitch == 36));
        assert!(Arrangement::flattened_notes(&Uuid::from_u128(3), &patterns, &clips).is_empty());
    }

    #[test]
    fn a_long_clip_loops_its_pattern_and_a_short_one_trims() {
        let a = pattern_a();
        let long = vec![Clip::new(a.id, 0, 40 * BEAT)];
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &long);
        assert_eq!(
            notes.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            vec![2 * BEAT, 18 * BEAT, 34 * BEAT]
        );
        let short = vec![Clip::new(a.id, 0, 5 * BEAT / 2)];
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &short);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].length_ticks, BEAT / 2, "cut at the clip end");
        let none = vec![Clip::new(a.id, 0, BEAT)];
        assert!(
            Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &none).is_empty()
        );
    }

    #[test]
    fn offset_starts_mid_pattern_and_muted_clips_are_silent() {
        let a = pattern_a();
        let mut c = Clip::new(a.id, 0, 16 * BEAT);
        c.offset_ticks = 2 * BEAT; // pattern tick 2·BEAT lands at clip start
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &[c.clone()]);
        assert_eq!(
            notes.iter().map(|n| n.start_tick).collect::<Vec<_>>(),
            vec![0]
        );
        c.muted = true;
        assert!(Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &[c]).is_empty());
    }

    #[test]
    fn repeats_get_distinct_stable_ids() {
        let a = pattern_a();
        let twice = vec![Clip::new(a.id, 0, 32 * BEAT)];
        let notes = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &twice);
        assert_eq!(notes.len(), 2);
        assert_ne!(notes[0].id, notes[1].id);
        let again = Arrangement::flattened_notes(&melody(), std::slice::from_ref(&a), &twice);
        assert_eq!(notes[1].id, again[1].id);
    }
}
