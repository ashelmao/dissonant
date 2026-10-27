//! Ready-to-play projects for a first session: drum grooves, and the "vibe" templates the
//! new-project screen offers (amen jungle, dubstep, rage, jersey club, …).
//!
//! A vibe sets tempo, swing, key and a progression, then builds its tracks from
//! `assets/vibes.json` (the UI's browser mock reads the same file): a lead left empty for the
//! player's own melody, a bass line on the chord roots, an optional chord part, and a drum
//! pattern on its own kit. Every sound is a factory sound. Everything is ordinary model data
//! afterwards.

use crate::chord_track::ChordTrack;
use crate::model::{tile_chords, KeyState, NoteEvent, ProjectModel, ScaleType, SongPattern, Track};
use crate::theory::harmony;
use crate::sound::DrumPad;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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

/// One hit of a bass or chord part, repeated every bar: `[offset, length]`, plus an optional
/// interval in semitones above the root for a bass hit (`[1.5, 0.5, 12]` is an octave pop).
pub type PartHit = Vec<f64>;

/// A bass line on the root of the chord under each hit, in the G♯1–G2 range (plus the hit's
/// interval), on a per-bar rhythm. Hits in a chord gap are skipped.
pub fn bass_line(chords: &ChordTrack, length: f64, bar: f64, rhythm: &[PartHit]) -> Vec<NoteEvent> {
    per_bar(length, bar, rhythm, |i, start, len| {
        let chord = chords.chord_at(start)?;
        let pc = crate::theory::normalize(*chord.pitch_classes.first()?);
        let interval = rhythm[i].get(2).copied().unwrap_or(0.0) as i32;
        let pitch = 36 + pc - if pc >= 8 { 12 } else { 0 } + interval;
        Some(vec![NoteEvent::new(start, len, pitch).with_velocity(if i == 0 { 105 } else { 92 })])
    })
}

/// The chord under each hit, voiced close from its root upward: the root on the lowest pitch
/// at or above `low`, every later chord tone stacked just above the one before.
pub fn chord_part(chords: &ChordTrack, length: f64, bar: f64, low: i32, rhythm: &[PartHit]) -> Vec<NoteEvent> {
    per_bar(length, bar, rhythm, |i, start, len| {
        let chord = chords.chord_at(start)?;
        let mut pitches: Vec<i32> = Vec::with_capacity(chord.pitch_classes.len());
        for &pc in &chord.pitch_classes {
            let below = pitches.last().map_or(low - 1, |&p| p);
            pitches.push(below + 1 + (pc - below - 1).rem_euclid(12));
        }
        let velocity = if i == 0 { 92 } else { 80 };
        Some(pitches.into_iter().map(|p| NoteEvent::new(start, len, p).with_velocity(velocity)).collect())
    })
}

/// Run `hit(index, start, length)` for every rhythm hit of every bar inside `length`, clipping
/// each note at the end.
fn per_bar(length: f64, bar: f64, rhythm: &[PartHit], hit: impl Fn(usize, f64, f64) -> Option<Vec<NoteEvent>>) -> Vec<NoteEvent> {
    let mut notes = Vec::new();
    let mut bar_start = 0.0;
    while bar_start < length - 1e-9 {
        for (i, h) in rhythm.iter().enumerate() {
            let (Some(&offset), Some(&len)) = (h.first(), h.get(1)) else { continue };
            let start = bar_start + offset;
            if start < length - 1e-9 {
                notes.extend(hit(i, start, len.min(length - start)).unwrap_or_default());
            }
        }
        bar_start += bar;
    }
    notes
}

/// One drum row of a vibe: a step string, one character per `step` beats (`X` accent, `x` hit,
/// `o` ghost, anything else a rest; spaces are only for reading), repeated to fill the pattern.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrumLane {
    pub pitch: i32,
    #[serde(default = "sixteenth")]
    pub step: f64,
    pub steps: String,
}

fn sixteenth() -> f64 {
    0.25
}

impl DrumLane {
    /// The lane's step characters, without the spaces.
    pub fn cells(&self) -> Vec<char> {
        self.steps.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// The hits in `length` beats.
    pub fn hits(&self, length: f64) -> Vec<NoteEvent> {
        let cells = self.cells();
        let period = cells.len() as f64 * self.step;
        let mut hits = Vec::new();
        if period <= 0.0 {
            return hits;
        }
        let mut at = 0.0;
        while at < length - 1e-9 {
            for (i, c) in cells.iter().enumerate() {
                let velocity = match c {
                    'X' => 120,
                    'x' => 96,
                    'o' => 64,
                    _ => continue,
                };
                let start = at + i as f64 * self.step;
                if start < length - 1e-9 {
                    hits.push(NoteEvent::new(start, self.step.min(length - start), self.pitch).with_velocity(velocity));
                }
            }
            at += period;
        }
        hits
    }
}

/// What a vibe track plays.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum VibeRole {
    /// Left empty for the player.
    Lead,
    Bass { rhythm: Vec<PartHit> },
    Chords { low: i32, rhythm: Vec<PartHit> },
    /// `kit` rows are `[pitch, sound, choke]`, top to bottom.
    Drums { kit: Vec<(u8, String, u8)>, lanes: Vec<DrumLane> },
}

fn full_tone() -> f64 {
    18_000.0
}
fn unity() -> f64 {
    1.0
}

/// One track of a vibe and its bus settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VibeTrack {
    pub name: String,
    /// A factory synth id; drum tracks take their sounds from the kit instead.
    #[serde(default)]
    pub sound: Option<String>,
    #[serde(default = "full_tone")]
    pub tone: f64,
    #[serde(default)]
    pub reverb: f64,
    #[serde(default = "unity")]
    pub volume: f64,
    #[serde(default)]
    pub pan: f64,
    #[serde(flatten)]
    pub role: VibeRole,
}

/// A vibe as `assets/vibes.json` writes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VibeSpec {
    pub id: String,
    pub name: String,
    pub blurb: String,
    pub tempo: f64,
    pub swing: f64,
    pub swing_grid: f64,
    pub root: i32,
    pub scale: ScaleType,
    pub degrees: Vec<i32>,
    pub chord_beats: f64,
    pub tracks: Vec<VibeTrack>,
}

const VIBES_JSON: &str = include_str!("../assets/vibes.json");

/// Every vibe spec, in file order. Panics at first use if the bundled JSON is malformed (a test
/// covers it).
pub fn vibe_specs() -> &'static [VibeSpec] {
    static SPECS: OnceLock<Vec<VibeSpec>> = OnceLock::new();
    SPECS.get_or_init(|| serde_json::from_str(VIBES_JSON).expect("vibes.json is valid"))
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

/// Every vibe, in the order the new-project screen lists them.
pub fn vibes() -> Vec<VibeInfo> {
    vibe_specs()
        .iter()
        .map(|v| VibeInfo {
            id: v.id.clone(),
            name: v.name.clone(),
            blurb: v.blurb.clone(),
            tempo: v.tempo,
            key: KeyState::locked(v.root, v.scale),
        })
        .collect()
}

impl ProjectModel {
    /// A new project from a vibe: the key locked, then the vibe's tracks in order (the empty
    /// lead first, so it is selected), each with one 4-bar 4/4 pattern placed from beat 0 to
    /// the end of a `song_bars`-bar song, with the progression tiled across it. `None` for an
    /// unknown id.
    pub fn from_vibe(id: &str, song_bars: u32) -> Option<Self> {
        let v = vibe_specs().iter().find(|v| v.id == id)?;
        let (length, bar) = (16.0, 4.0);
        let mut model = ProjectModel::empty();
        model.tempo = v.tempo;
        model.swing = v.swing;
        model.swing_grid = v.swing_grid;
        model.key = KeyState::locked(v.root, v.scale);
        model.song_length_beats = song_bars.max(1) as f64 * bar;
        let progression = harmony::progression(&v.degrees, v.root, v.scale, length, v.chord_beats);
        let chords = ChordTrack::new(progression.clone());
        model.chords = tile_chords(&progression, length, model.song_length_beats);

        model.tracks.clear();
        model.patterns.clear();
        for t in &v.tracks {
            let mut track = match t.role {
                VibeRole::Drums { .. } => Track::drums(t.name.as_str()),
                _ => Track::new(t.name.as_str()),
            };
            if let Some(sound) = &t.sound {
                track.voice = sound.clone();
            }
            track.tone = t.tone;
            track.reverb_send = t.reverb;
            track.volume = t.volume;
            track.pan = t.pan;
            let mut pattern = SongPattern::new(track.id, format!("{} 1", t.name));
            pattern.length_beats = length;
            pattern.notes = match &t.role {
                VibeRole::Lead => vec![],
                VibeRole::Bass { rhythm } => bass_line(&chords, length, bar, rhythm),
                VibeRole::Chords { low, rhythm } => chord_part(&chords, length, bar, *low, rhythm),
                VibeRole::Drums { kit, lanes } => {
                    track.drum_pads = kit.iter().map(|(pitch, sound, choke)| DrumPad { pitch: *pitch, sound: sound.clone(), choke: *choke }).collect();
                    let mut hits: Vec<NoteEvent> = lanes.iter().flat_map(|l| l.hits(length)).collect();
                    hits.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat).then(a.pitch.cmp(&b.pitch)));
                    hits
                }
            };
            model.tracks.push(track);
            model.patterns.push(pattern);
        }
        model.place_every_pattern_across_the_song();
        Some(model)
    }
}
