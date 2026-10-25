//! Ready-to-play projects for a first session: drum grooves, generated bass lines, and the
//! "vibe" templates the new-project screen offers (lo-fi, post-punk, ambient, …).
//!
//! A vibe sets tempo, swing, key, a progression, a drum groove and a bass line, and leaves one
//! track empty for the player's own melody. Everything is ordinary model data afterwards.

use crate::chord_track::ChordTrack;
use crate::model::{tile_chords, KeyState, NoteEvent, ProjectModel, ScaleType, SongPattern, Track};
use crate::theory::harmony;
use serde::{Deserialize, Serialize};

const KICK: i32 = 36;
const SNARE: i32 = 38;
const CLAP: i32 = 39;
const HAT: i32 = 42;

/// A drum pattern, generated one bar at a time. Every groove is on 8th steps so it shows on the
/// drum grid at any snap. Only [`Groove::Backbeat`] adapts to odd meters; the others are 4/4
/// patterns and fall back to the backbeat in any other bar size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Groove {
    /// Kick on the downbeat (and mid-bar in even meters), snare on the other beats, 8th hats.
    Backbeat,
    /// Kick on every beat, clap on 2 and 4, hats on the offbeats.
    FourOnFloor,
    /// Lazy hip-hop: kick on 1, the and of 2 and the and of 3; snare on 2 and 4.
    BoomBap,
    /// Snare on 3 only, so the bar feels twice as long.
    HalfTime,
    /// Driving straight 8ths with a push into beat 3.
    Motorik,
    /// A soft kick and one hat a bar: pulse without a beat.
    Sparse,
}

impl Groove {
    /// Hits for `length` beats in bars of `bar` beats. Anything past the end is dropped.
    pub fn hits(self, length: f64, bar: f64) -> Vec<NoteEvent> {
        let four_four = (bar - 4.0).abs() < 1e-9;
        let groove = if four_four { self } else { Groove::Backbeat };
        let mut hits = Vec::new();
        let mut bar_start = 0.0;
        while bar_start < length - 1e-9 {
            for (offset, pitch, velocity) in groove.bar(bar) {
                let start = bar_start + offset;
                if offset < bar - 1e-9 && start < length - 1e-9 {
                    hits.push(NoteEvent::new(start, 0.25_f64.min(length - start), pitch).with_velocity(velocity));
                }
            }
            bar_start += bar;
        }
        hits.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat).then(a.pitch.cmp(&b.pitch)));
        hits
    }

    /// One bar as `(offset, pitch, velocity)`.
    fn bar(self, bar: f64) -> Vec<(f64, i32, i32)> {
        let eighths = |vel_on: i32, vel_off: i32| -> Vec<(f64, i32, i32)> {
            (0..8).map(|i| (i as f64 * 0.5, HAT, if i % 2 == 0 { vel_on } else { vel_off })).collect()
        };
        match self {
            Groove::Backbeat => {
                let whole_beats = bar.ceil() as i32;
                // A second kick mid-bar only in whole, even meters of 4+ beats (4/4, 6/4): in
                // 2/4 it would take the snare's place.
                let mid_kick = (bar.fract() == 0.0 && whole_beats >= 4 && whole_beats % 2 == 0).then_some(whole_beats / 2);
                let mut out: Vec<_> = (0..whole_beats)
                    .map(|beat| {
                        if beat == 0 || Some(beat) == mid_kick {
                            (beat as f64, KICK, 120)
                        } else {
                            (beat as f64, SNARE, 100)
                        }
                    })
                    .collect();
                let mut step = 0.0;
                while step < bar - 1e-9 {
                    out.push((step, HAT, if step.fract() == 0.0 { 80 } else { 60 }));
                    step += 0.5;
                }
                out
            }
            Groove::FourOnFloor => {
                let mut out = vec![(0.0, KICK, 120), (1.0, KICK, 115), (2.0, KICK, 120), (3.0, KICK, 115)];
                out.extend([(1.0, CLAP, 100), (3.0, CLAP, 100)]);
                out.extend([0.5, 1.5, 2.5, 3.5].map(|o| (o, HAT, 85)));
                out
            }
            Groove::BoomBap => {
                let mut out = vec![(0.0, KICK, 120), (1.5, KICK, 100), (2.5, KICK, 110), (1.0, SNARE, 105), (3.0, SNARE, 105)];
                out.extend(eighths(70, 50));
                out
            }
            Groove::HalfTime => {
                let mut out = vec![(0.0, KICK, 120), (2.5, KICK, 100), (2.0, SNARE, 110)];
                out.extend(eighths(70, 55));
                out
            }
            Groove::Motorik => {
                let mut out = vec![(0.0, KICK, 115), (2.0, KICK, 115), (2.5, KICK, 95), (1.0, SNARE, 100), (3.0, SNARE, 100)];
                out.extend(eighths(80, 75));
                out
            }
            Groove::Sparse => vec![(0.0, KICK, 90), (2.0, HAT, 45)],
        }
    }
}

/// A bass line that plays the root of the chord under each hit, in the G♯1–G2 range, on a
/// per-bar rhythm of `(offset, length)` pairs. Hits in a chord gap are skipped.
pub fn bass_line(chords: &ChordTrack, length: f64, bar: f64, rhythm: &[(f64, f64)]) -> Vec<NoteEvent> {
    let mut notes = Vec::new();
    let mut bar_start = 0.0;
    while bar_start < length - 1e-9 {
        for (i, &(offset, len)) in rhythm.iter().enumerate() {
            let start = bar_start + offset;
            if start >= length - 1e-9 {
                continue;
            }
            let Some(chord) = chords.chord_at(start) else { continue };
            let Some(&root) = chord.pitch_classes.first() else { continue };
            let pc = crate::theory::normalize(root);
            let pitch = 36 + pc - if pc >= 8 { 12 } else { 0 };
            let velocity = if i == 0 { 105 } else { 92 };
            notes.push(NoteEvent::new(start, len.min(length - start), pitch).with_velocity(velocity));
        }
        bar_start += bar;
    }
    notes
}

/// What the new-project screen shows for a vibe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VibeInfo {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub tempo: f64,
    pub key: KeyState,
}

/// Voice, tone and reverb. The values match `SOUND_PRESETS` in `ui/src/types.ts`.
#[derive(Clone, Copy)]
struct Sound {
    voice: &'static str,
    tone: f64,
    reverb: f64,
}

const SOFT_KEYS: Sound = Sound { voice: "triangle", tone: 6000.0, reverb: 0.25 };
const WARM_PAD: Sound = Sound { voice: "pad", tone: 4000.0, reverb: 0.45 };
const GLASS_PLUCK: Sound = Sound { voice: "pluck", tone: 12000.0, reverb: 0.3 };
const BUZZY_BASS: Sound = Sound { voice: "saw", tone: 1200.0, reverb: 0.0 };
const ROUND_BASS: Sound = Sound { voice: "sine", tone: 2000.0, reverb: 0.0 };
const LEAD_WITH_BITE: Sound = Sound { voice: "square", tone: 9000.0, reverb: 0.15 };

struct VibeSpec {
    id: &'static str,
    name: &'static str,
    blurb: &'static str,
    tempo: f64,
    swing: f64,
    swing_grid: f64,
    root: i32,
    scale: ScaleType,
    degrees: &'static [i32],
    chord_beats: f64,
    groove: Groove,
    bass_rhythm: &'static [(f64, f64)],
    bass: Sound,
    lead_name: &'static str,
    lead: Sound,
}

const EIGHTHS: [(f64, f64); 8] = [(0.0, 0.45), (0.5, 0.45), (1.0, 0.45), (1.5, 0.45), (2.0, 0.45), (2.5, 0.45), (3.0, 0.45), (3.5, 0.45)];

const VIBES: [VibeSpec; 5] = [
    VibeSpec {
        id: "lofi",
        name: "lo-fi bedroom",
        blurb: "dusty, swung and slow: jazzy chords, lazy drums, room for a melody",
        tempo: 80.0,
        swing: 60.0,
        swing_grid: 0.25,
        root: 5,
        scale: ScaleType::Major,
        degrees: &[1, 4, 0, 5],
        chord_beats: 4.0,
        groove: Groove::BoomBap,
        bass_rhythm: &[(0.0, 1.25), (1.5, 1.5), (3.5, 0.5)],
        bass: ROUND_BASS,
        lead_name: "keys",
        lead: SOFT_KEYS,
    },
    VibeSpec {
        id: "postpunk",
        name: "post-punk",
        blurb: "fast, tense and driving: 8th-note bass, motorik drums, minor chords",
        tempo: 148.0,
        swing: 50.0,
        swing_grid: 0.5,
        root: 4,
        scale: ScaleType::Minor,
        degrees: &[0, 5, 2, 6],
        chord_beats: 4.0,
        groove: Groove::Motorik,
        bass_rhythm: &EIGHTHS,
        bass: BUZZY_BASS,
        lead_name: "lead",
        lead: LEAD_WITH_BITE,
    },
    VibeSpec {
        id: "ambient",
        name: "ambient texture",
        blurb: "a slow two-chord drift: barely a beat, long notes, lots of space",
        tempo: 70.0,
        swing: 50.0,
        swing_grid: 0.5,
        root: 2,
        scale: ScaleType::Major,
        degrees: &[0, 3],
        chord_beats: 8.0,
        groove: Groove::Sparse,
        bass_rhythm: &[(0.0, 4.0)],
        bass: ROUND_BASS,
        lead_name: "pad",
        lead: WARM_PAD,
    },
    VibeSpec {
        id: "songwriter",
        name: "songwriter demo",
        blurb: "four friendly chords and a plain beat to sing or play over",
        tempo: 96.0,
        swing: 50.0,
        swing_grid: 0.5,
        root: 7,
        scale: ScaleType::Major,
        degrees: &[0, 4, 5, 3],
        chord_beats: 4.0,
        groove: Groove::Backbeat,
        bass_rhythm: &[(0.0, 2.0), (2.0, 2.0)],
        bass: ROUND_BASS,
        lead_name: "melody",
        lead: GLASS_PLUCK,
    },
    VibeSpec {
        id: "club",
        name: "club loop",
        blurb: "four-on-the-floor, offbeat bass and a minor loop that keeps going",
        tempo: 124.0,
        swing: 50.0,
        swing_grid: 0.5,
        root: 9,
        scale: ScaleType::Minor,
        degrees: &[0, 6, 5, 6],
        chord_beats: 4.0,
        groove: Groove::FourOnFloor,
        bass_rhythm: &[(0.5, 0.4), (1.5, 0.4), (2.5, 0.4), (3.5, 0.4)],
        bass: BUZZY_BASS,
        lead_name: "lead",
        lead: LEAD_WITH_BITE,
    },
];

/// Every vibe, in the order the new-project screen lists them.
pub fn vibes() -> Vec<VibeInfo> {
    VIBES
        .iter()
        .map(|v| VibeInfo {
            id: v.id.into(),
            name: v.name.into(),
            blurb: v.blurb.into(),
            tempo: v.tempo,
            key: KeyState::locked(v.root, v.scale),
        })
        .collect()
}

fn apply_sound(track: &mut Track, sound: Sound) {
    track.voice = sound.voice.into();
    track.tone = sound.tone;
    track.reverb_send = sound.reverb;
}

impl ProjectModel {
    /// A new project from a vibe: the key locked, an empty lead track (selected first, for the
    /// player's own melody), a bass line and a groove, each in its own 4-bar 4/4 pattern placed
    /// from beat 0 to the end of a `song_bars`-bar song, with the progression tiled across it.
    /// `None` for an unknown id.
    pub fn from_vibe(id: &str, song_bars: u32) -> Option<Self> {
        let v = VIBES.iter().find(|v| v.id == id)?;
        let (length, bar) = (16.0, 4.0);
        let mut model = ProjectModel::empty();
        model.tempo = v.tempo;
        model.swing = v.swing;
        model.swing_grid = v.swing_grid;
        model.key = KeyState::locked(v.root, v.scale);
        model.song_length_beats = song_bars.max(1) as f64 * bar;
        let progression = harmony::progression(v.degrees, v.root, v.scale, length, v.chord_beats);
        let chords = ChordTrack::new(progression.clone());
        model.chords = tile_chords(&progression, length, model.song_length_beats);

        model.tracks[0].name = v.lead_name.into();
        apply_sound(&mut model.tracks[0], v.lead);
        model.patterns[0].name = format!("{} 1", v.lead_name);
        model.patterns[0].length_beats = length;
        let mut bass = Track::new("bass");
        apply_sound(&mut bass, v.bass);
        let drums = Track::drums("drums");

        let mut bass_pattern = SongPattern::new(bass.id, "bass 1");
        bass_pattern.length_beats = length;
        bass_pattern.notes = bass_line(&chords, length, bar, v.bass_rhythm);
        let mut groove = SongPattern::new(drums.id, "drums 1");
        groove.length_beats = length;
        groove.notes = v.groove.hits(length, bar);
        model.tracks.push(bass);
        model.tracks.push(drums);
        model.patterns.push(bass_pattern);
        model.patterns.push(groove);
        model.place_every_pattern_across_the_song();
        Some(model)
    }
}
