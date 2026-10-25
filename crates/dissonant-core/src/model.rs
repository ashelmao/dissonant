//! The serializable project. Schema v6: patterns belong to one track, the chord progression
//! lives on the song timeline, and the song has an explicit length.
//!
//! Files older than schema 6 are refused (there is no migration). Within schema 6 decoding is
//! tolerant: every field has a `serde(default)` so a file missing a later-added field still
//! opens. JSON keys are camelCase.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 6;

/// What opening a pre-v6 file (or template, or autosave) says. The UI shows it verbatim.
pub const OLD_SCHEMA_ERROR: &str = "This project was made with an older version of dissonant and can't be opened.";

/// Song length of a new project when nothing else says, in bars.
pub const DEFAULT_SONG_BARS: u32 = 32;

/// A diatonic scale family. Expanded later (modes, harmonic minor, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ScaleType {
    #[default]
    Major,
    Minor,
}

/// The project's key. `root_pitch_class == None` means no key is locked yet (cold start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyState {
    /// Pitch class of the tonic, 0 = C … 11 = B. `None` when no key is set.
    pub root_pitch_class: Option<i32>,
    pub scale: ScaleType,
    /// A user-confirmed key, as opposed to a merely inferred suggestion.
    pub is_locked: bool,
}

impl KeyState {
    pub const NONE: KeyState = KeyState {
        root_pitch_class: None,
        scale: ScaleType::Major,
        is_locked: false,
    };

    pub fn locked(root: i32, scale: ScaleType) -> Self {
        KeyState {
            root_pitch_class: Some(root),
            scale,
            is_locked: true,
        }
    }

    /// The scale's pitch classes, if a key is set.
    pub fn scale_pitch_classes(&self) -> Option<Vec<i32>> {
        self.root_pitch_class
            .map(|root| crate::theory::harmony::scale_pitch_classes(root, self.scale))
    }
}

/// One chord on the chord track, stored as bare pitch classes. `name` is display-only and
/// never required (R9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordEvent {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub start_beat: f64,
    pub length_beats: f64,
    pub pitch_classes: Vec<i32>,
    #[serde(default)]
    pub name: Option<String>,
}

impl ChordEvent {
    pub fn new(start_beat: f64, length_beats: f64, pitch_classes: Vec<i32>, name: Option<String>) -> Self {
        ChordEvent {
            id: Uuid::new_v4(),
            start_beat,
            length_beats,
            pitch_classes,
            name,
        }
    }

    pub fn end_beat(&self) -> f64 {
        self.start_beat + self.length_beats
    }

    pub fn contains(&self, beat: f64) -> bool {
        beat >= self.start_beat && beat < self.end_beat()
    }
}

fn default_velocity() -> i32 {
    100
}

/// One note in the piano roll. `pitch` is a MIDI note number (0–127), `velocity` 1–127.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEvent {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub start_beat: f64,
    pub length_beats: f64,
    pub pitch: i32,
    #[serde(default = "default_velocity")]
    pub velocity: i32,
    /// The player marked this (dissonant) note as deliberate: the tier cue stays, the flag
    /// stops nagging. Guidance, never a gate (R11).
    #[serde(default)]
    pub intentional: bool,
    /// Kept in the pattern but skipped by the scheduler (drawn hollow).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
}

impl NoteEvent {
    pub fn new(start_beat: f64, length_beats: f64, pitch: i32) -> Self {
        NoteEvent {
            id: Uuid::new_v4(),
            start_beat,
            length_beats,
            pitch,
            velocity: default_velocity(),
            intentional: false,
            muted: false,
        }
    }

    pub fn with_velocity(mut self, velocity: i32) -> Self {
        self.velocity = velocity;
        self
    }

    pub fn end_beat(&self) -> f64 {
        self.start_beat + self.length_beats
    }

    /// Clamp into the ranges the engine accepts. Used when applying edits from the UI.
    pub fn sanitized(mut self) -> Self {
        self.pitch = self.pitch.clamp(0, 127);
        self.velocity = self.velocity.clamp(1, 127);
        if !self.start_beat.is_finite() || self.start_beat < 0.0 {
            self.start_beat = 0.0;
        }
        if !self.length_beats.is_finite() || self.length_beats < 1.0 / 64.0 {
            self.length_beats = 1.0 / 64.0;
        }
        self
    }
}

fn default_voice() -> String {
    "saw".to_string()
}
fn one() -> f64 {
    1.0
}
fn default_tone() -> f64 {
    18_000.0
}

/// One instrument track: name, voice, mute/solo and its own bus settings. Its notes live in
/// the patterns that name it as their `track_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    /// A synth voice preset name (`saw`, `square`, `triangle`, `sine`, `pad`, `pluck`).
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub soloed: bool,
    /// A drum track uses the procedural kit and a step grid instead of a synth + piano roll.
    #[serde(default)]
    pub is_drum: bool,
    /// Bus gain (0–1.5).
    #[serde(default = "one")]
    pub volume: f64,
    /// Reverb wet mix (0–1).
    #[serde(default)]
    pub reverb_send: f64,
    /// Low-pass cutoff in Hz.
    #[serde(default = "default_tone")]
    pub tone: f64,
    /// -1 (left) … 1 (right).
    #[serde(default)]
    pub pan: f64,
    /// Display color (`#rrggbb`). `None` means "pick from the palette by position".
    #[serde(default)]
    pub color: Option<String>,
}

impl Track {
    pub fn new(name: impl Into<String>) -> Self {
        Track {
            id: Uuid::new_v4(),
            name: name.into(),
            voice: default_voice(),
            muted: false,
            soloed: false,
            is_drum: false,
            volume: 1.0,
            reverb_send: 0.0,
            tone: default_tone(),
            pan: 0.0,
            color: None,
        }
    }

    pub fn drums(name: impl Into<String>) -> Self {
        let mut t = Track::new(name);
        t.is_drum = true;
        t
    }
}

fn default_pattern_length() -> f64 {
    16.0
}

/// A loop on one track ("bass 1", "drums 2"). Its notes sit at pattern-local beats; clips place
/// it on the song, always on its own track's lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongPattern {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub track_id: Uuid,
    pub name: String,
    #[serde(default = "default_pattern_length")]
    pub length_beats: f64,
    #[serde(default)]
    pub notes: Vec<NoteEvent>,
}

impl SongPattern {
    pub fn new(track_id: Uuid, name: impl Into<String>) -> Self {
        SongPattern {
            id: Uuid::new_v4(),
            track_id,
            name: name.into(),
            length_beats: default_pattern_length(),
            notes: vec![],
        }
    }

    /// A deep copy on the same track with fresh pattern and note ids, named "<name> copy".
    pub fn duplicated(&self) -> Self {
        let mut copy = self.clone();
        copy.id = Uuid::new_v4();
        copy.name = format!("{} copy", self.name);
        for n in &mut copy.notes {
            n.id = Uuid::new_v4();
        }
        copy
    }
}

fn default_low_cut() -> f64 {
    20.0
}

/// The master chain's persisted settings (previously lived in UI state and was lost on save).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MasterSettings {
    pub gain: f64,
    pub reverb_wet: f64,
    pub low_cut_hz: f64,
    pub high_cut_hz: f64,
    pub low_eq: f64,
    pub mid_eq: f64,
    pub high_eq: f64,
}

impl Default for MasterSettings {
    fn default() -> Self {
        MasterSettings {
            gain: 1.0,
            reverb_wet: 0.0,
            low_cut_hz: default_low_cut(),
            high_cut_hz: default_tone(),
            low_eq: 1.0,
            mid_eq: 1.0,
            high_eq: 1.0,
        }
    }
}

/// A pattern placed on the song timeline. The pattern loops from `offset_beats` for
/// `length_beats`, so a clip can be trimmed, extended or started mid-pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub pattern_id: Uuid,
    pub start_beat: f64,
    pub length_beats: f64,
    #[serde(default)]
    pub offset_beats: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Clip {
    pub fn new(pattern_id: Uuid, start_beat: f64, length_beats: f64) -> Self {
        Clip {
            id: Uuid::new_v4(),
            pattern_id,
            start_beat,
            length_beats,
            offset_beats: 0.0,
            muted: false,
        }
    }

    pub fn end_beat(&self) -> f64 {
        self.start_beat + self.length_beats
    }
}

/// A named marker on the song (intro, verse, drop…). With a `key`, the section modulates:
/// patterns placed inside it are tiered against that key instead of the project key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub start_beat: f64,
    #[serde(default)]
    pub key: Option<KeyState>,
    #[serde(default)]
    pub color: Option<String>,
}

impl Section {
    pub fn new(name: impl Into<String>, start_beat: f64) -> Self {
        Section {
            id: Uuid::new_v4(),
            name: name.into(),
            start_beat,
            key: None,
            color: None,
        }
    }
}

/// Beats per bar and the beat unit. Only the numerator changes grids and bar math; the
/// engine counts in quarter-note beats regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TimeSignature {
    pub numerator: u32,
    pub denominator: u32,
}

impl Default for TimeSignature {
    fn default() -> Self {
        TimeSignature { numerator: 4, denominator: 4 }
    }
}

impl TimeSignature {
    /// Quarter-note beats in one bar (3/4 → 3, 6/8 → 3, 7/8 → 3.5).
    pub fn beats_per_bar(&self) -> f64 {
        self.numerator as f64 * 4.0 / self.denominator.max(1) as f64
    }
}

fn default_tempo() -> f64 {
    120.0
}
fn default_schema() -> u32 {
    SCHEMA_VERSION
}
/// The starter melody: one phrase per chord of I–IV–V–vi in C, as `(start, length, pitch,
/// velocity)` inside a 4-beat chord slot. Mostly chord tones, plus two tensions (D over C,
/// B leaning into C over Am) so the roll shows more than one tier on the first look.
const STARTER_MELODY: [[(f64, f64, i32, i32); 3]; 4] = [
    [(0.0, 1.5, 76, 100), (1.5, 0.5, 74, 80), (2.0, 2.0, 72, 96)], // C:  E  D  C
    [(0.0, 1.0, 69, 100), (1.0, 1.0, 72, 88), (2.0, 2.0, 77, 96)], // F:  A  C  F
    [(0.0, 1.5, 74, 100), (1.5, 0.5, 71, 80), (2.0, 2.0, 67, 96)], // G:  D  B  G
    [(0.0, 1.5, 72, 100), (1.5, 0.5, 71, 80), (2.0, 2.0, 69, 96)], // Am: C  B  A
];

fn default_song_length() -> f64 {
    DEFAULT_SONG_BARS as f64 * TimeSignature::default().beats_per_bar()
}

/// The full serializable project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModel {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default = "default_tempo")]
    pub tempo: f64,
    #[serde(default)]
    pub key: KeyState,
    #[serde(default)]
    pub tracks: Vec<Track>,
    /// Every pattern of every track (a track may have none).
    #[serde(default)]
    pub patterns: Vec<SongPattern>,
    /// The song: clips placed on the timeline. A clip plays on its pattern's track.
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Song markers, optionally with their own key.
    #[serde(default)]
    pub sections: Vec<Section>,
    #[serde(default)]
    pub master: MasterSettings,
    #[serde(default)]
    pub time_signature: TimeSignature,
    /// Global swing: 50 = straight, 66.7 = triplet feel, 75 = hard shuffle. See
    /// [`crate::tempo::swing_warp`].
    #[serde(default = "default_swing")]
    pub swing: f64,
    /// The subdivision swing acts on, in beats (0.5 = eighths, 0.25 = sixteenths).
    #[serde(default = "default_swing_grid")]
    pub swing_grid: f64,
    /// Tempo changes along the song (empty = `tempo` throughout). Pattern mode plays at the
    /// base tempo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tempo_points: Vec<crate::tempo::TempoPoint>,
    /// The project's chord progression in song beats: sorted, non-overlapping. It drives the
    /// tiering everywhere and the (monitor-only) chord bed.
    #[serde(default)]
    pub chords: Vec<ChordEvent>,
    /// The hard end of the song, at least one bar. Clips never reach past it.
    #[serde(default = "default_song_length")]
    pub song_length_beats: f64,
}

fn default_swing() -> f64 {
    50.0
}
fn default_swing_grid() -> f64 {
    0.5
}

impl Default for ProjectModel {
    fn default() -> Self {
        ProjectModel::empty()
    }
}

/// Clean up a chord progression: drop chords with a bad position or length, fold the pitch
/// classes into 0–11 without duplicates (keeping their order, so the root stays first), sort
/// by start, and cut each chord at the next one's start (the later chord wins, as in
/// [`crate::chord_track::ChordTrack::chord_at`]) so none overlap.
pub fn tidy_chords(chords: Vec<ChordEvent>) -> Vec<ChordEvent> {
    let mut chords: Vec<ChordEvent> = chords
        .into_iter()
        .filter(|c| c.start_beat.is_finite() && c.start_beat >= 0.0 && c.length_beats.is_finite() && c.length_beats > 0.0)
        .map(|mut c| {
            let mut seen = Vec::with_capacity(c.pitch_classes.len());
            for pc in c.pitch_classes.iter().map(|&pc| crate::theory::normalize(pc)) {
                if !seen.contains(&pc) {
                    seen.push(pc);
                }
            }
            c.pitch_classes = seen;
            c
        })
        .collect();
    chords.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
    let starts: Vec<f64> = chords.iter().map(|c| c.start_beat).collect();
    for (i, c) in chords.iter_mut().enumerate() {
        if let Some(&next) = starts.get(i + 1) {
            c.length_beats = c.length_beats.min(next - c.start_beat);
        }
    }
    chords.retain(|c| c.length_beats > 1e-9);
    chords
}

/// Repeat a progression written over `[0, period)` back to back up to `end`, cutting the last
/// pass there. Every copy gets fresh chord ids.
pub fn tile_chords(progression: &[ChordEvent], period: f64, end: f64) -> Vec<ChordEvent> {
    let mut out = Vec::new();
    if !period.is_finite() || period <= 0.0 {
        return out;
    }
    let mut at = 0.0;
    while at < end - 1e-9 {
        for c in progression {
            let start = at + c.start_beat;
            if start >= end - 1e-9 {
                continue;
            }
            let len = c.length_beats.min(end - start);
            out.push(ChordEvent::new(start, len, c.pitch_classes.clone(), c.name.clone()));
        }
        at += period;
    }
    out
}

impl ProjectModel {
    /// A brand-new project: one melody track with one empty pattern, no key, no chords, and a
    /// [`DEFAULT_SONG_BARS`]-bar song.
    pub fn empty() -> Self {
        let track = Track::new("melody");
        let pattern = SongPattern::new(track.id, "melody 1");
        ProjectModel {
            schema_version: SCHEMA_VERSION,
            tempo: default_tempo(),
            key: KeyState::NONE,
            tracks: vec![track],
            patterns: vec![pattern],
            clips: vec![],
            sections: vec![],
            master: MasterSettings::default(),
            time_signature: TimeSignature::default(),
            swing: default_swing(),
            swing_grid: default_swing_grid(),
            tempo_points: vec![],
            chords: vec![],
            song_length_beats: default_song_length(),
        }
    }

    /// The song's tempo map (base tempo plus points).
    pub fn tempo_map(&self) -> crate::tempo::TempoMap {
        crate::tempo::TempoMap::new(self.tempo, &self.tempo_points)
    }

    /// Quarter-note beats in one bar of the project's meter.
    pub fn bar_beats(&self) -> f64 {
        self.time_signature.beats_per_bar()
    }

    /// A new project that plays music the moment space is pressed: I–IV–V–vi in C (key
    /// locked), a short melody over it and a drum groove, each in its own 4-bar 4/4 pattern
    /// looping through a [`DEFAULT_SONG_BARS`]-bar song. See [`ProjectModel::starter_shaped`].
    pub fn starter() -> Self {
        Self::starter_shaped(16.0, 4.0, DEFAULT_SONG_BARS)
    }

    /// The starter at any pattern length, bar size and song length (the new-project defaults).
    /// The four chords split the pattern evenly and the melody stretches with them, so every
    /// note keeps its tier; the progression is tiled across the whole song. The groove repeats
    /// per bar at its own speed instead of stretching. Each track's pattern is placed once from
    /// beat 0 to the song end.
    pub fn starter_shaped(length_beats: f64, beats_per_bar: f64, song_bars: u32) -> Self {
        let length = if length_beats.is_finite() && length_beats > 0.0 { length_beats } else { default_pattern_length() };
        let bar = if beats_per_bar.is_finite() && beats_per_bar > 0.0 { beats_per_bar } else { 4.0 };
        let mut model = ProjectModel::empty();
        model.key = KeyState::locked(0, ScaleType::Major);
        model.song_length_beats = song_bars.max(1) as f64 * bar;
        let slot = length / STARTER_MELODY.len() as f64;
        let progression = crate::theory::harmony::progression(&[0, 3, 4, 5], 0, ScaleType::Major, length, slot);
        model.chords = tile_chords(&progression, length, model.song_length_beats);

        model.tracks[0].voice = "pluck".into();
        model.tracks[0].reverb_send = 0.2;
        let scale = slot / 4.0;
        model.patterns[0].length_beats = length;
        model.patterns[0].notes = STARTER_MELODY
            .iter()
            .enumerate()
            .flat_map(|(i, phrase)| {
                phrase.iter().map(move |&(start, len, pitch, vel)| {
                    NoteEvent::new(i as f64 * slot + start * scale, len * scale, pitch).with_velocity(vel)
                })
            })
            .collect();

        let drums = Track::drums("drums");
        let mut groove = SongPattern::new(drums.id, "drums 1");
        groove.length_beats = length;
        groove.notes = crate::vibes::Groove::Backbeat.hits(length, bar);
        model.tracks.push(drums);
        model.patterns.push(groove);
        model.place_every_pattern_across_the_song();
        model
    }

    /// One clip per pattern from beat 0 to the song end (the pattern loops to fill it). Used by
    /// the starter and the vibes, where every track has exactly one pattern.
    pub(crate) fn place_every_pattern_across_the_song(&mut self) {
        let end = self.song_length_beats;
        self.clips = self.patterns.iter().map(|p| Clip::new(p.id, 0.0, end)).collect();
    }

    pub fn clip(&self, id: &Uuid) -> Option<&Clip> {
        self.clips.iter().find(|c| &c.id == id)
    }

    pub fn section(&self, id: &Uuid) -> Option<&Section> {
        self.sections.iter().find(|s| &s.id == id)
    }

    /// The section containing `beat` (the last one starting at or before it).
    pub fn section_at(&self, beat: f64) -> Option<&Section> {
        self.sections
            .iter()
            .filter(|s| s.start_beat <= beat)
            .max_by(|a, b| a.start_beat.total_cmp(&b.start_beat))
    }

    /// The key in force at `beat`: the section's key if it has one, else the project key.
    pub fn key_at(&self, beat: f64) -> KeyState {
        self.section_at(beat).and_then(|s| s.key).unwrap_or(self.key)
    }

    pub fn track(&self, id: &Uuid) -> Option<&Track> {
        self.tracks.iter().find(|t| &t.id == id)
    }

    pub fn track_mut(&mut self, id: &Uuid) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| &t.id == id)
    }

    pub fn pattern(&self, id: &Uuid) -> Option<&SongPattern> {
        self.patterns.iter().find(|p| &p.id == id)
    }

    pub fn pattern_mut(&mut self, id: &Uuid) -> Option<&mut SongPattern> {
        self.patterns.iter_mut().find(|p| &p.id == id)
    }

    /// The patterns that belong to one track, in project order.
    pub fn patterns_of<'a>(&'a self, track_id: &'a Uuid) -> impl Iterator<Item = &'a SongPattern> + 'a {
        self.patterns.iter().filter(move |p| &p.track_id == track_id)
    }

    /// Hold the song's hard end: clips starting at or after it go, clips crossing it are cut.
    pub(crate) fn fit_clips_to_song(&mut self) {
        let end = self.song_length_beats;
        self.clips.retain(|c| c.start_beat < end);
        for c in &mut self.clips {
            if c.end_beat() > end {
                c.length_beats = end - c.start_beat;
            }
        }
    }

    /// Normalize after decoding: bump the schema, make sure there is at least one track, clamp
    /// every note, and drop things that point at what isn't in the project any more (patterns
    /// of a missing track, clips of a missing pattern). Tidies the chords and holds the song
    /// length to at least one bar, with every clip inside it.
    pub fn normalized(mut self) -> Self {
        // Only ever bump forward: relabelling a newer file as ours would make the next save
        // overwrite it with fields this version dropped.
        self.schema_version = self.schema_version.max(SCHEMA_VERSION);
        if self.tracks.is_empty() {
            let fresh = ProjectModel::empty();
            self.tracks = fresh.tracks;
            self.patterns = fresh.patterns;
        }
        let track_ids: Vec<Uuid> = self.tracks.iter().map(|t| t.id).collect();
        // A pattern of a track that no longer exists is invisible and never played, but would
        // survive every save; drop it instead of growing the file forever.
        self.patterns.retain(|p| track_ids.contains(&p.track_id));
        for pattern in &mut self.patterns {
            for n in pattern.notes.iter_mut() {
                *n = n.clone().sanitized();
            }
        }
        let pattern_ids: Vec<Uuid> = self.patterns.iter().map(|p| p.id).collect();
        if !self.swing.is_finite() {
            self.swing = default_swing();
        }
        self.swing = self.swing.clamp(50.0, 75.0);
        if !(self.swing_grid == 0.5 || self.swing_grid == 0.25) {
            self.swing_grid = default_swing_grid();
        }
        self.tempo_points.retain(|p| p.beat.is_finite() && p.beat >= 0.0 && p.bpm.is_finite());
        for p in &mut self.tempo_points {
            p.bpm = p.bpm.clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM);
        }
        self.tempo_points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        self.chords = tidy_chords(std::mem::take(&mut self.chords));
        if !self.song_length_beats.is_finite() {
            self.song_length_beats = default_song_length();
        }
        self.song_length_beats = self.song_length_beats.max(self.bar_beats());
        self.clips.retain(|c| {
            let finite = [c.start_beat, c.length_beats, c.offset_beats].iter().all(|v| v.is_finite());
            finite && pattern_ids.contains(&c.pattern_id) && c.length_beats > 0.0 && c.start_beat >= 0.0
        });
        for c in &mut self.clips {
            c.offset_beats = c.offset_beats.max(0.0);
        }
        self.fit_clips_to_song();
        self.clips.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        self.sections.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        self
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        Self::from_value(serde_json::from_str(json)?)
    }

    /// Decode an already-parsed project (an autosave carries one inside its own JSON). Refuses
    /// a non-object, a file from before schema 6 ([`OLD_SCHEMA_ERROR`]) and one from a newer
    /// build.
    pub fn from_value(value: serde_json::Value) -> serde_json::Result<Self> {
        use serde::de::Error;
        // Every field defaults, and serde also accepts a struct in sequence form — so `[]` and
        // `[1,2]` would both decode as a blank project and the next save would destroy whatever
        // file the user actually opened. A project is an object.
        if !value.is_object() {
            return Err(serde_json::Error::custom("not a dissonant project: expected a JSON object"));
        }
        // Every file schema 6 writes says its version, so one that doesn't is from before it.
        // Older layouts (patterns holding every track) are refused, not migrated.
        let version = value.get("schemaVersion").and_then(serde_json::Value::as_u64).unwrap_or(0);
        if version < SCHEMA_VERSION as u64 {
            return Err(serde_json::Error::custom(OLD_SCHEMA_ERROR));
        }
        let decoded: ProjectModel = serde_json::from_value(value)?;
        if decoded.schema_version > SCHEMA_VERSION {
            return Err(serde_json::Error::custom(format!(
                "project was saved by a newer version of dissonant (schema {}, this build reads {})",
                decoded.schema_version, SCHEMA_VERSION
            )));
        }
        Ok(decoded.normalized())
    }
}
