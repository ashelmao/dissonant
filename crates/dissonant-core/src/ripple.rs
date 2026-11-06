//! Ripple edits: opening time, cutting time out, and copying a stretch of song.
//!
//! "The intro needs four more bars" touches every kind of thing that lives on the song
//! timeline — clips, audio takes, section markers, tempo points, the chord progression,
//! automation breakpoints, the loop brace and the song's own end. One module holds the rule
//! each of them moves by so the three commands cannot drift apart, and so a bounce made after
//! the edit is the same music as the one made before it, four bars later.
//!
//! The rule for material a cut falls inside is **split, never stretch**: the part before the
//! cut stays exactly where it was, the part after it keeps playing exactly what it played, by
//! moving into a second clip whose `offset_ticks` is advanced to the split point. Nothing is
//! resampled, retimed or re-looped; the only thing that changes is where the music sits.

use crate::automation::Breakpoint;
use crate::model::{AudioClip, ChordEvent, Clip, DetectedChord, ProjectModel, Section};
use crate::tempo::{TempoMap, TempoPoint};
use uuid::Uuid;

/// A surviving piece of a span an edit cut: where it lands, how far into the original span it
/// begins (what the piece has to skip to keep playing the same material), and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub start: i64,
    pub skip: i64,
    pub len: i64,
}

/// Where a point on the timeline lands when `ticks` of empty time open at `at`. A point *on*
/// `at` moves: the material that starts there is what the new time is opened in front of.
pub fn insert_point(tick: i64, at: i64, ticks: i64) -> i64 {
    if tick >= at {
        tick + ticks
    } else {
        tick
    }
}

/// Where a point lands when `[at, at + ticks)` is cut out. Anything inside the cut collapses
/// onto `at`, which is both where the gap closes and where an end that reached into the cut
/// now stops.
pub fn delete_point(tick: i64, at: i64, ticks: i64) -> i64 {
    if tick < at {
        tick
    } else if tick < at + ticks {
        at
    } else {
        tick - ticks
    }
}

/// The pieces of material `[start, start + len)` after `ticks` open at `at`: at most two, and
/// together they still play every tick the original did.
pub fn insert_pieces(start: i64, len: i64, at: i64, ticks: i64) -> Vec<Piece> {
    if len <= 0 {
        return vec![];
    }
    let end = start + len;
    if end <= at {
        return vec![Piece { start, skip: 0, len }];
    }
    if start >= at {
        return vec![Piece {
            start: start + ticks,
            skip: 0,
            len,
        }];
    }
    vec![
        Piece {
            start,
            skip: 0,
            len: at - start,
        },
        Piece {
            start: at + ticks,
            skip: at - start,
            len: end - at,
        },
    ]
}

/// The pieces of material `[start, start + len)` after `[at, at + ticks)` is cut out: whatever
/// was inside the cut is gone and what followed closes up against what came before. Material
/// wholly inside the cut answers nothing — it goes with the time it lived in.
pub fn delete_pieces(start: i64, len: i64, at: i64, ticks: i64) -> Vec<Piece> {
    if len <= 0 {
        return vec![];
    }
    let (end, cut_end) = (start + len, at + ticks);
    let mut out = Vec::new();
    if start < at {
        out.push(Piece {
            start,
            skip: 0,
            len: at.min(end) - start,
        });
    }
    if end > cut_end {
        let s = start.max(cut_end);
        out.push(Piece {
            start: s - ticks,
            skip: s - start,
            len: end - s,
        });
    }
    out
}

/// The part of material `[start, start + len)` that lies inside the window `[from, to)`, moved
/// so the window's own start lands on `at`. `None` when the material misses the window.
pub fn window_piece(start: i64, len: i64, from: i64, to: i64, at: i64) -> Option<Piece> {
    let (s, e) = (start.max(from), (start + len).min(to));
    (e > s).then(|| Piece {
        start: at + (s - from),
        skip: s - start,
        len: e - s,
    })
}

/// `clip` cut into its pieces. The first keeps the clip's identity; a second one is a new clip
/// that starts the pattern where the first left off, so a split is inaudible.
fn split_clip(clip: &Clip, pieces: Vec<Piece>) -> impl Iterator<Item = Clip> + '_ {
    pieces.into_iter().enumerate().map(move |(i, p)| Clip {
        id: if i == 0 { clip.id } else { Uuid::new_v4() },
        start_tick: p.start,
        length_ticks: p.len,
        offset_ticks: clip.offset_ticks + p.skip,
        ..clip.clone()
    })
}

/// `chord` cut into its pieces, each still spelling the same notes.
fn split_chord(chord: &ChordEvent, pieces: Vec<Piece>) -> impl Iterator<Item = ChordEvent> + '_ {
    pieces.into_iter().enumerate().map(move |(i, p)| ChordEvent {
        id: if i == 0 { chord.id } else { Uuid::new_v4() },
        start_tick: p.start,
        length_ticks: p.len,
        ..chord.clone()
    })
}

/// Detected chords cut by the same pieces as the progression: a candidate stays over the music
/// it was heard in, so accepting it after a time edit still lands on the right bars. One wholly
/// inside a cut goes with it.
fn split_detected(m: &mut ProjectModel, pieces: impl Fn(&DetectedChord) -> Vec<Piece>) {
    for d in &mut m.detections {
        let mut out = Vec::with_capacity(d.chords.len());
        for c in std::mem::take(&mut d.chords) {
            for (i, p) in pieces(&c).into_iter().enumerate() {
                out.push(DetectedChord {
                    id: if i == 0 { c.id } else { Uuid::new_v4() },
                    start_tick: p.start,
                    length_ticks: p.len,
                    ..c.clone()
                });
            }
        }
        d.chords = out;
    }
}

/// `clip` cut into its pieces, in seconds.
///
/// Audio never warps: a take is anchored at a tick and then plays for a length in seconds, so
/// the split point has to be carried through the tempo map (the one in force *before* the
/// edit, since the edit moves tempo points too) to find how many seconds of the file each
/// piece holds. The leading fade stays on the first piece and the trailing one on the last,
/// because a fade belongs to an edge of the sound and a split makes no new edges to shape.
fn split_audio(map: &TempoMap, clip: &AudioClip, pieces: Vec<Piece>, len: i64) -> Vec<AudioClip> {
    let anchor = map.seconds_at_tick(clip.start_tick as f64);
    // Seconds from the clip's own beginning to song tick `tick`.
    let into = |tick: i64| (map.seconds_at_tick(tick as f64) - anchor).clamp(0.0, clip.length_seconds);
    let last = pieces.len().saturating_sub(1);
    pieces
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let head = p.skip == 0;
            let tail = p.skip + p.len >= len;
            let from = into(clip.start_tick + p.skip);
            let until = if tail {
                clip.length_seconds
            } else {
                into(clip.start_tick + p.skip + p.len)
            };
            AudioClip {
                id: if i == 0 { clip.id } else { Uuid::new_v4() },
                start_tick: p.start,
                length_seconds: until - from,
                offset_seconds: clip.offset_seconds + from,
                fade_in_seconds: if head { clip.fade_in_seconds } else { 0.0 },
                fade_out_seconds: if i == last && tail {
                    clip.fade_out_seconds
                } else {
                    0.0
                },
                ..clip.clone()
            }
            .sanitized()
        })
        .collect()
}

/// How long an audio clip is in ticks, which is the only thing a timeline edit can reason
/// about. Rounded: a take's seconds rarely land on a whole tick, and the clip keeps its
/// seconds either way.
fn audio_len_ticks(map: &TempoMap, clip: &AudioClip) -> i64 {
    let end = map.seconds_at_tick(clip.start_tick as f64) + clip.length_seconds;
    (map.tick_at_seconds(end).round() as i64 - clip.start_tick).max(0)
}

/// Open `ticks` of empty time at song tick `at`: everything from `at` on moves that much
/// later, the song grows by the same amount, and anything the cut falls inside is split.
///
/// A section the time opens *inside* grows instead of splitting: a marker names a stretch of
/// music rather than a piece of it, so four bars added in the middle of the chorus are four
/// more bars of chorus.
pub fn insert_time(m: &mut ProjectModel, at: i64, ticks: i64) {
    let map = m.tempo_map();
    m.clips = std::mem::take(&mut m.clips)
        .iter()
        .flat_map(|c| split_clip(c, insert_pieces(c.start_tick, c.length_ticks, at, ticks)))
        .collect();
    m.audio_clips = std::mem::take(&mut m.audio_clips)
        .iter()
        .flat_map(|c| {
            let len = audio_len_ticks(&map, c);
            split_audio(&map, c, insert_pieces(c.start_tick, len, at, ticks), len)
        })
        .collect();
    for s in &mut m.sections {
        if s.length_ticks > 0 && s.start_tick < at && at < s.start_tick + s.length_ticks {
            s.length_ticks += ticks;
        } else {
            s.start_tick = insert_point(s.start_tick, at, ticks);
        }
    }
    for p in &mut m.tempo_points {
        p.tick = insert_point(p.tick, at, ticks);
    }
    let chords = m.chords.take();
    m.chords = crate::chord_track::ChordTrack::new(crate::model::tidy_chords(
        chords
            .iter()
            .flat_map(|c| split_chord(c, insert_pieces(c.start_tick, c.length_ticks, at, ticks)))
            .collect(),
    ));
    split_detected(m, |c| insert_pieces(c.start_tick, c.length_ticks, at, ticks));
    for lane in &mut m.automation {
        for p in &mut lane.points {
            p.tick = insert_point(p.tick, at, ticks);
        }
        lane.tidy();
    }
    // Time opened inside the brace lengthens it: the loop is a place in the music, and that
    // place just got longer. A brace that ends exactly where the time opens does not grow.
    // The punch range is a place in the music too, so it moves by the same rule.
    let shift = |r: Option<(i64, i64)>| {
        r.map(|(s, e)| {
            (
                insert_point(s, at, ticks),
                if e > at { e + ticks } else { e },
            )
        })
    };
    m.loop_region = shift(m.loop_region);
    m.punch_region = shift(m.punch_region);
    // `at` past the end means "open time there", so the song reaches it either way.
    m.song_length_ticks = m.song_length_ticks.max(at) + ticks;
    settle(m);
}

/// Cut `[at, at + ticks)` out of the song and close the gap: material inside it is gone,
/// everything after it moves earlier by `ticks`, and the song shortens to match.
///
/// Material the cut falls inside is split, not stretched: the part before the cut keeps its
/// length and the part after it starts where the cut ends, playing on from exactly there. A
/// section whose whole extent was inside the cut goes with its music.
pub fn delete_time(m: &mut ProjectModel, at: i64, ticks: i64) {
    let map = m.tempo_map();
    let cut_end = at + ticks;
    m.clips = std::mem::take(&mut m.clips)
        .iter()
        .flat_map(|c| split_clip(c, delete_pieces(c.start_tick, c.length_ticks, at, ticks)))
        .collect();
    m.audio_clips = std::mem::take(&mut m.audio_clips)
        .iter()
        .flat_map(|c| {
            let len = audio_len_ticks(&map, c);
            split_audio(&map, c, delete_pieces(c.start_tick, len, at, ticks), len)
        })
        .collect();
    let spans: Vec<(i64, i64)> = m.sections.iter().map(|s| m.section_span(s)).collect();
    m.sections = std::mem::take(&mut m.sections)
        .into_iter()
        .zip(spans)
        .filter_map(|(s, (s0, s1))| {
            if s0 >= at && s0 < cut_end && s1 <= cut_end {
                return None;
            }
            let start = delete_point(s0, at, ticks);
            let length_ticks = if s.length_ticks > 0 {
                (delete_point(s1, at, ticks) - start).max(0)
            } else {
                0
            };
            // An explicit extent cut away to nothing is a section with no music left in it.
            (s.length_ticks <= 0 || length_ticks > 0).then_some(Section {
                start_tick: start,
                length_ticks,
                ..s
            })
        })
        .collect();
    // A tempo change inside the cut goes with it: whatever was in force before the cut plays
    // on through what follows.
    m.tempo_points.retain(|p| p.tick < at || p.tick >= cut_end);
    for p in &mut m.tempo_points {
        p.tick = delete_point(p.tick, at, ticks);
    }
    let chords = m.chords.take();
    m.chords = crate::chord_track::ChordTrack::new(crate::model::tidy_chords(
        chords
            .iter()
            .flat_map(|c| split_chord(c, delete_pieces(c.start_tick, c.length_ticks, at, ticks)))
            .collect(),
    ));
    split_detected(m, |c| delete_pieces(c.start_tick, c.length_ticks, at, ticks));
    for lane in &mut m.automation {
        lane.points.retain(|p| p.tick < at || p.tick >= cut_end);
        for p in &mut lane.points {
            p.tick = delete_point(p.tick, at, ticks);
        }
        lane.tidy();
    }
    // A lane with every point inside the cut has nothing left to say; the parameter goes back
    // to its static control, as `ClearAutomation` leaves it.
    m.automation.retain(|l| !l.points.is_empty());
    let pull = |r: Option<(i64, i64)>| {
        r.map(|(s, e): (i64, i64)| (delete_point(s, at, ticks), delete_point(e, at, ticks)))
    };
    m.loop_region = pull(m.loop_region);
    m.punch_region = pull(m.punch_region);
    let gone = (m.song_length_ticks.min(cut_end) - at).max(0);
    m.song_length_ticks = (m.song_length_ticks - gone).max(m.bar_ticks());
    settle(m);
}

/// Copy everything in `[from, to)` to `at`, pushing what was there along: verse 1 becomes
/// verse 2 in one gesture.
///
/// The copy is taken before the time is opened, so duplicating a range onto its own start (or
/// onto itself) still reproduces what was there rather than what the insert just moved. Every
/// copied thing gets a fresh id; a section marker is copied when its *start* is inside the
/// range, since a section that began earlier already names the music the range was cut from.
pub fn duplicate_range(m: &mut ProjectModel, from: i64, to: i64, at: i64) {
    let span = to - from;
    let map = m.tempo_map();
    let clips = m.clips.clone();
    let audio = m.audio_clips.clone();
    let sections: Vec<(Section, (i64, i64))> = m
        .sections
        .iter()
        .filter(|s| s.start_tick >= from && s.start_tick < to)
        .map(|s| (s.clone(), m.section_span(s)))
        .collect();
    let tempo: Vec<TempoPoint> = m
        .tempo_points
        .iter()
        .filter(|p| p.tick >= from && p.tick < to)
        .cloned()
        .collect();
    let chords = m.chords.chords().to_vec();
    let lanes: Vec<(crate::automation::AutoTarget, Vec<Breakpoint>)> = m
        .automation
        .iter()
        .map(|l| {
            (
                l.target,
                l.points
                    .iter()
                    .filter(|p| p.tick >= from && p.tick < to)
                    .cloned()
                    .collect(),
            )
        })
        .collect();

    insert_time(m, at, span);

    for c in &clips {
        if let Some(p) = window_piece(c.start_tick, c.length_ticks, from, to, at) {
            m.clips.push(Clip {
                id: Uuid::new_v4(),
                start_tick: p.start,
                length_ticks: p.len,
                offset_ticks: c.offset_ticks + p.skip,
                ..c.clone()
            });
        }
    }
    for c in &audio {
        let len = audio_len_ticks(&map, c);
        if let Some(p) = window_piece(c.start_tick, len, from, to, at) {
            // The piece is placed by `split_audio`'s rules, then given a new identity: a copy
            // is a second take of the same source, not the same clip twice.
            m.audio_clips.extend(split_audio(&map, c, vec![p], len).into_iter().map(|copy| {
                AudioClip {
                    id: Uuid::new_v4(),
                    ..copy
                }
            }));
        }
    }
    for (s, (s0, s1)) in sections {
        m.sections.push(Section {
            id: Uuid::new_v4(),
            start_tick: at + (s0 - from),
            // An explicit extent is cut at the range's end; an implicit one stays implicit, so
            // the copy runs to whatever marker follows it, exactly as the original did.
            length_ticks: if s.length_ticks > 0 {
                (s1.min(to) - s0).max(1)
            } else {
                0
            },
            ..s
        });
    }
    for p in tempo {
        let tick = at + (p.tick - from);
        m.tempo_points.retain(|x| x.tick != tick);
        m.tempo_points.push(TempoPoint::new(tick, p.bpm, p.ramp));
    }
    let mut all = m.chords.take();
    for c in &chords {
        if let Some(p) = window_piece(c.start_tick, c.length_ticks, from, to, at) {
            all.push(ChordEvent::new(
                p.start,
                p.len,
                c.pitch_classes.clone(),
                c.name.clone(),
            ));
        }
    }
    m.chords = crate::chord_track::ChordTrack::new(crate::model::tidy_chords(all));
    for (target, points) in lanes {
        if points.is_empty() {
            continue;
        }
        let Some(lane) = m.automation.iter_mut().find(|l| l.target == target) else {
            continue;
        };
        lane.points.extend(points.into_iter().map(|p| Breakpoint {
            id: Uuid::new_v4(),
            tick: at + (p.tick - from),
            ..p
        }));
        lane.tidy();
    }
    settle(m);
}

/// Put the timeline back in order after a ripple: everything sorted by start, nothing reaching
/// past the song's end.
fn settle(m: &mut ProjectModel) {
    m.clips.sort_by_key(|c| c.start_tick);
    m.audio_clips.sort_by_key(|c| c.start_tick);
    m.sections.sort_by_key(|s| s.start_tick);
    m.tempo_points.sort_by_key(|p| p.tick);
    m.fit_clips_to_song();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_before_the_cut_never_moves() {
        assert_eq!(
            insert_pieces(0, 100, 100, 40),
            vec![Piece {
                start: 0,
                skip: 0,
                len: 100
            }]
        );
        assert_eq!(
            delete_pieces(0, 100, 100, 40),
            vec![Piece {
                start: 0,
                skip: 0,
                len: 100
            }]
        );
    }

    #[test]
    fn an_insert_splits_what_it_falls_inside() {
        assert_eq!(
            insert_pieces(0, 100, 40, 10),
            vec![
                Piece {
                    start: 0,
                    skip: 0,
                    len: 40
                },
                Piece {
                    start: 50,
                    skip: 40,
                    len: 60
                },
            ]
        );
    }

    #[test]
    fn a_delete_keeps_both_sides_and_drops_the_middle() {
        assert_eq!(
            delete_pieces(0, 100, 20, 30),
            vec![
                Piece {
                    start: 0,
                    skip: 0,
                    len: 20
                },
                Piece {
                    start: 20,
                    skip: 50,
                    len: 50
                },
            ]
        );
        assert!(delete_pieces(25, 10, 20, 30).is_empty());
        assert_eq!(
            delete_pieces(60, 10, 20, 30),
            vec![Piece {
                start: 30,
                skip: 0,
                len: 10
            }]
        );
    }

    #[test]
    fn a_window_takes_only_what_overlaps_it() {
        assert_eq!(
            window_piece(10, 100, 50, 80, 200),
            Some(Piece {
                start: 200,
                skip: 40,
                len: 30
            })
        );
        assert_eq!(window_piece(0, 10, 50, 80, 200), None);
    }
}
