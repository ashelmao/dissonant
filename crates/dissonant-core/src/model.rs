//! The serializable project. Schema v10 gives every pattern one owning track, moves the chord
//! progression onto the song timeline and gives the song an explicit length, on top of v9's
//! integer ticks at [`crate::ticks::TICKS_PER_BEAT`] PPQ and ordered effect [`Insert`] chains,
//! v8's audio tracks — audio sources, audio clips and a guitar rig per track — v7's optional
//! plugin per track and v5's velocity and persisted master settings.
//!
//! Decoding is tolerant: every field has a `serde(default)` so a file missing a later-added
//! field still opens. JSON keys are camelCase to match the previous app's files. Files written
//! before v9 carry the old `…Beat`/`…Beats` keys; each of those decodes into a `legacy_*` twin
//! that [`ProjectModel::normalized`] converts to ticks and clears, so nothing downstream ever
//! sees beats and nothing is ever written back out in them. Files written before v10 carry
//! patterns that hold every track's notes and their own chords; [`ProjectModel::migrated_v10`]
//! splits each into one pattern per track and lifts its chords onto the song.

use crate::effects::{NativeKind, ParamMap};
use crate::ticks::{beats_to_ticks, TICKS_PER_BEAT};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 10;

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
    #[serde(default)]
    pub start_tick: i64,
    #[serde(default)]
    pub length_ticks: i64,
    pub pitch_classes: Vec<i32>,
    #[serde(default)]
    pub name: Option<String>,
    /// Schema ≤ 8 only; folded into `start_tick` by [`ProjectModel::normalized`].
    #[serde(default, rename = "startBeat", skip_serializing)]
    pub legacy_start_beat: Option<f64>,
    /// Schema ≤ 8 only; folded into `length_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "lengthBeats", skip_serializing)]
    pub legacy_length_beats: Option<f64>,
}

impl ChordEvent {
    pub fn new(
        start_tick: i64,
        length_ticks: i64,
        pitch_classes: Vec<i32>,
        name: Option<String>,
    ) -> Self {
        ChordEvent {
            id: Uuid::new_v4(),
            start_tick,
            length_ticks,
            pitch_classes,
            name,
            legacy_start_beat: None,
            legacy_length_beats: None,
        }
    }

    pub fn end_tick(&self) -> i64 {
        self.start_tick + self.length_ticks
    }

    pub fn contains(&self, tick: i64) -> bool {
        tick >= self.start_tick && tick < self.end_tick()
    }

    /// Beat fields from an older file folded into the tick fields.
    pub(crate) fn migrated(&mut self) {
        migrate_tick(&mut self.start_tick, &mut self.legacy_start_beat);
        migrate_tick(&mut self.length_ticks, &mut self.legacy_length_beats);
    }
}

/// The schema ≤ 8 fold: a stored beat value wins only where the tick field says nothing, so a
/// hand-merged file carrying both keys keeps the newer one.
fn migrate_tick(tick: &mut i64, legacy: &mut Option<f64>) {
    if let Some(beats) = legacy.take() {
        if *tick == 0 {
            *tick = beats_to_ticks(beats);
        }
    }
}

fn default_velocity() -> i32 {
    100
}

/// The neutral value of a bipolar per-note property: centred pan, the instrument's own
/// release, no filter offset.
fn half() -> f32 {
    0.5
}

fn is_half(v: &f32) -> bool {
    *v == 0.5
}
fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}
fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

/// Widest detune the fine-pitch property may ask for, in cents (one octave each way).
pub const MAX_FINE_CENTS: i32 = 1200;
/// Most retriggers a single note may fire, matching FL Studio's repeat range.
pub const MAX_REPEATS: u8 = 14;
/// Furthest a note may be nudged off its grid position, in ticks (four beats each way).
pub const MAX_SHIFT_TICKS: i64 = 4 * TICKS_PER_BEAT;

/// Longest slide a rope may ask for, in seconds; the patch's glide knob tops out here too.
pub const MAX_SLIDE_SECS: f32 = 2.0;

/// The shortest note the engine will schedule (a 1/64 note); trimming never produces less
/// than this.
pub const MIN_NOTE_TICKS: i64 = TICKS_PER_BEAT / 16;

/// One note in the piano roll. `pitch` is a MIDI note number (0–127), `velocity` 1–127.
///
/// The properties below the first five are per-note performance controls modelled on FL
/// Studio's note record (`number/time/length/pan/velocity/release/color/fcut/fres/pitchofs/
/// repeats`, observed in the `flpianoroll` script reference shipped with FL Studio 2026).
/// A step in the drum grid is the same record, which is what lets one graph editor drive
/// both editors. Every one of them is read by the scheduler or a voice; none is decorative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteEvent {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub start_tick: i64,
    #[serde(default)]
    pub length_ticks: i64,
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
    /// Stereo position of this note alone: 0 hard left, 0.5 centre, 1 hard right.
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub pan: f32,
    /// Scales the voice's release: 0.5 leaves the instrument's own tail, 0 cuts it short,
    /// 1 stretches it.
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub release: f32,
    /// Detune in cents, ±[`MAX_FINE_CENTS`]. Drives the drum grid's step pitch as well.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub fine: i32,
    /// Filter cutoff for this note: 0.5 is the track's tone, below closes, above opens.
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub mod_x: f32,
    /// Filter resonance for this note: 0.5 is flat, above rings.
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub mod_y: f32,
    /// Start offset in ticks, applied when scheduling. The grid position stays the note's
    /// identity, so a step keeps its cell while playing early or late.
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub shift_ticks: i64,
    /// Retriggers spread across the note's own length; 0 plays it once.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub repeats: u8,
    /// Colour group 0–15. Groups the roll by colour and becomes the MIDI channel on export.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub color: u8,
    /// The note on the same pattern this one slides from: a rope. It is stored on the
    /// destination so each note of a chord can slide to its own target, and only takes effect
    /// while [`rope_is_live`] holds. Every path that copies notes remaps it to the copy of its
    /// source, or drops it when the source was not copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_from: Option<Uuid>,
    /// How long the slide takes, in seconds (0–[`MAX_SLIDE_SECS`]). `None` uses the patch's
    /// glide, so changing the sound changes every rope that never had its own time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_secs: Option<f32>,
    /// Schema ≤ 8 only; folded into `start_tick` by [`ProjectModel::normalized`].
    #[serde(default, rename = "startBeat", skip_serializing)]
    pub legacy_start_beat: Option<f64>,
    /// Schema ≤ 8 only; folded into `length_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "lengthBeats", skip_serializing)]
    pub legacy_length_beats: Option<f64>,
    /// Schema ≤ 8 only; folded into `shift_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "shift", skip_serializing)]
    pub legacy_shift: Option<f64>,
}

impl NoteEvent {
    pub fn new(start_tick: i64, length_ticks: i64, pitch: i32) -> Self {
        NoteEvent {
            id: Uuid::new_v4(),
            start_tick,
            length_ticks,
            pitch,
            velocity: default_velocity(),
            intentional: false,
            muted: false,
            pan: 0.5,
            release: 0.5,
            fine: 0,
            mod_x: 0.5,
            mod_y: 0.5,
            shift_ticks: 0,
            repeats: 0,
            color: 0,
            slide_from: None,
            slide_secs: None,
            legacy_start_beat: None,
            legacy_length_beats: None,
            legacy_shift: None,
        }
    }

    pub fn with_velocity(mut self, velocity: i32) -> Self {
        self.velocity = velocity;
        self
    }

    pub fn end_tick(&self) -> i64 {
        self.start_tick + self.length_ticks
    }

    /// Where the note actually sounds once its shift is taken into account. Never negative.
    pub fn shifted_start(&self) -> i64 {
        (self.start_tick + self.shift_ticks).max(0)
    }

    /// Clamp into the ranges the engine accepts. Used when applying edits from the UI.
    pub fn sanitized(mut self) -> Self {
        self.pitch = self.pitch.clamp(0, 127);
        self.velocity = self.velocity.clamp(1, 127);
        self.start_tick = self.start_tick.max(0);
        self.length_ticks = self.length_ticks.max(MIN_NOTE_TICKS);
        self.pan = unit(self.pan);
        self.release = unit(self.release);
        self.mod_x = unit(self.mod_x);
        self.mod_y = unit(self.mod_y);
        self.fine = self.fine.clamp(-MAX_FINE_CENTS, MAX_FINE_CENTS);
        self.shift_ticks = self.shift_ticks.clamp(-MAX_SHIFT_TICKS, MAX_SHIFT_TICKS);
        self.repeats = self.repeats.min(MAX_REPEATS);
        self.color = self.color.min(15);
        // Same range as the patch's glide; garbage means "use the glide" rather than a guess.
        self.slide_secs = self
            .slide_secs
            .filter(|s| s.is_finite())
            .map(|s| s.clamp(0.0, MAX_SLIDE_SECS));
        self
    }

    /// Beat fields from an older file folded into the tick fields.
    fn migrated(&mut self) {
        migrate_tick(&mut self.start_tick, &mut self.legacy_start_beat);
        migrate_tick(&mut self.length_ticks, &mut self.legacy_length_beats);
        migrate_tick(&mut self.shift_ticks, &mut self.legacy_shift);
    }
}

/// Whether `target`'s rope from `source` actually slides: the source must still be sounding
/// when the target starts. Touching counts (`target` starting exactly at the source's end), so a
/// legato line drawn on the grid connects. Both starts are the shifted ones, because that is
/// when the notes really sound. A muted source has no voice to slide out of, and a note can't
/// slide from itself. Equal pitches are fine: the rope then acts as a tie.
///
/// A rope that isn't live is kept (moving a note never breaks its rope) and the target plays as
/// an ordinary note. `ui/src/ropes.ts` mirrors this so the roll can draw live and dead ropes
/// without asking; the two must stay identical.
pub fn rope_is_live(source: &NoteEvent, target: &NoteEvent) -> bool {
    if source.id == target.id || source.muted {
        return false;
    }
    let from = source.shifted_start();
    let at = target.shifted_start();
    from < at && at <= from + source.length_ticks
}

/// How far a rope's source is held past its slide note's start wherever the slide can only be
/// asked for with plain MIDI: on a track hosting a plugin, and in an exported MIDI file. A
/// synth in legato or mono mode glides with its own portamento when a new key arrives while
/// the last one is still down, so a source that merely touches its slide note is stretched to
/// overlap it by this much.
///
/// Five milliseconds is long enough that the two keys overlap on any host that keeps event
/// order (it is a couple of hundred samples at 44.1 kHz, wider than any sub-block a plugin
/// splits its events into, and five SMF ticks at 480 PPQ and 120 bpm) and short enough that a
/// polyphonic patch, which simply plays both notes, never lets the source be heard ringing on.
/// The slide time itself is not sent: that is the synth's own portamento setting.
pub const ROPE_OVERLAP_SECS: f64 = 0.005;

/// Make one pattern's ropes valid: a rope whose source isn't in `notes` (deleted, or not
/// copied), that points at its own note, or that closes a loop (A from B from A) is cut, and a
/// slide time left without a rope goes with it. A loop is cut at its first note in slice
/// order, which for a sorted pattern is the earliest, the one that could never have slid
/// anyway. Chains (A → B → C) are fine.
pub fn tidy_ropes(notes: &mut [NoteEvent]) {
    let index: HashMap<Uuid, usize> = notes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    for i in 0..notes.len() {
        let id = notes[i].id;
        let Some(first) = notes[i].slide_from else {
            notes[i].slide_secs = None;
            continue;
        };
        // A missing source cuts this rope. Otherwise walk the chain upstream: reaching this note
        // again is a loop through it; reaching a note seen before is a loop further up, cut when
        // its own first note comes round; a missing link further up is that note's to cut.
        let cut = !index.contains_key(&first) || {
            let mut from = first;
            let mut seen = HashSet::new();
            loop {
                if from == id {
                    break true;
                }
                if !seen.insert(from) {
                    break false;
                }
                match index.get(&from).and_then(|&j| notes[j].slide_from) {
                    Some(next) => from = next,
                    None => break false,
                }
            }
        };
        if cut {
            notes[i].slide_from = None;
            notes[i].slide_secs = None;
        }
    }
}

/// Point copied ropes at the copies: `copy_of` maps an original note id to its copy's id. A rope
/// whose source has a copy follows it; one whose source was left behind is dropped with its
/// slide time, since a copy sliding out of the original would tie two unrelated phrases.
pub fn remap_ropes(notes: &mut [NoteEvent], copy_of: impl Fn(&Uuid) -> Option<Uuid>) {
    for n in notes {
        n.slide_from = n.slide_from.as_ref().and_then(&copy_of);
        if n.slide_from.is_none() {
            n.slide_secs = None;
        }
    }
}

/// A 0–1 property, with anything non-finite falling back to the neutral centre.
fn unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.5
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

/// A third-party plugin loaded on a track, in place of the built-in voice.
///
/// Core stores this but never interprets it: `uid` is opaque here, minted and resolved by the
/// plugin layer (it encodes format, bundle path and plugin id). Keeping core free of that
/// meaning is what lets the model stay a pure data crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPlugin {
    /// The plugin layer's identifier for one plugin inside one bundle.
    pub uid: String,
    /// Cached display name, so a project opened on a machine where the plugin is gone can
    /// still say *which* plugin is missing instead of showing a bare uid.
    pub name: String,
    /// Base64 of the plugin's own opaque state blob. `None` means "whatever the plugin loads
    /// with" — a freshly chosen plugin, or one that does not implement state.
    #[serde(default)]
    pub state: Option<String>,
}

/// Most inserts a chain will hold. Eight is deeper than any realistic chain and bounds the
/// per-block cost and the UI height.
pub const MAX_INSERTS: usize = 8;

/// One effect slot in a chain, on a track or on the master.
///
/// `uid` is opaque to core, exactly like [`TrackPlugin::uid`]: minted and resolved by the
/// plugin layer — except for the `native:` corner of it, which names one of the stock effects
/// in [`crate::effects`]. `id` is this slot's identity — it survives reordering, and the plugin
/// layer keys the hosted instance by it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Insert {
    pub id: Uuid,
    /// The plugin layer's identifier for one plugin inside one bundle, or `native:<slug>`.
    pub uid: String,
    /// Cached display name, so a project opened where the plugin is gone still says which one.
    pub name: String,
    /// A bypassed insert stays in the chain, in order, and costs nothing to process.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Base64 of the plugin's own opaque state blob. `None` means "whatever the plugin loads
    /// with". A native effect has no blob of its own; its knobs are in `params`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// A native effect's knobs, by [`crate::effects::NativeParam::id`]. Absent ids are at
    /// their defaults, so a freshly added effect stores nothing at all and a file only carries
    /// what someone actually moved.
    ///
    /// `default` rather than a schema bump: every older file is a chain of plugin slots, which
    /// is exactly an empty map, so there is nothing to migrate and nothing new to refuse.
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub params: ParamMap,
}

impl Insert {
    /// A freshly chosen effect: enabled, with no state of its own yet.
    pub fn new(uid: impl Into<String>, name: impl Into<String>) -> Self {
        Insert {
            id: Uuid::new_v4(),
            uid: uid.into(),
            name: name.into(),
            enabled: true,
            state: None,
            params: ParamMap::new(),
        }
    }

    /// Which stock effect this is, or `None` when the slot names a plugin.
    pub fn native_kind(&self) -> Option<NativeKind> {
        NativeKind::from_uid(&self.uid)
    }
}

/// Drop slots that name nothing, keep one slot per id, cap the chain, and hold every native
/// effect's knobs inside their ranges. Reachable by hand-editing a file, so it is resolved once
/// on load instead of guarded at every read.
fn sanitize_inserts(inserts: &mut Vec<Insert>) {
    let mut seen: Vec<Uuid> = Vec::with_capacity(inserts.len());
    inserts.retain(|i| {
        if i.uid.trim().is_empty() || seen.contains(&i.id) {
            return false;
        }
        seen.push(i.id);
        true
    });
    inserts.truncate(MAX_INSERTS);
    for insert in inserts.iter_mut() {
        match insert.native_kind() {
            Some(kind) => crate::effects::sanitize_params(kind, &mut insert.params),
            // A plugin has no parameters of ours; a map on one is a file someone edited.
            None => insert.params.clear(),
        }
    }
}

/// One instrument track: name, voice, mute/solo and its own bus settings. Notes live in
/// patterns keyed by track id, so a track is reused across every pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    /// The synth sound id (a factory slug like `saw`, or an embedded `u-<uuid>`). See
    /// [`ProjectModel::resolve_synth`].
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub soloed: bool,
    /// A drum track uses the procedural kit and a step grid instead of a synth + piano roll.
    #[serde(default)]
    pub is_drum: bool,
    /// An audio track holds recorded or imported audio and a guitar rig instead of notes.
    #[serde(default)]
    pub is_audio: bool,
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
    /// A plugin that replaces the built-in voice for this track. `None` plays the synth or
    /// drum kit named by `voice`.
    #[serde(default)]
    pub plugin: Option<TrackPlugin>,
    /// The guitar rig on an audio track. `None` on note tracks.
    #[serde(default)]
    pub amp: Option<AmpRig>,
    /// This track's effect chain, in order, applied pre-fader. Empty on a new track.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserts: Vec<Insert>,
    /// A drum track's kit, top to bottom (1–16 pads, unique pitches). Ignored on synth tracks.
    #[serde(default = "crate::sound::default_kit")]
    pub drum_pads: Vec<crate::sound::DrumPad>,
}

/// What a track plays. Derived from the booleans rather than replacing them: `isDrum` is in
/// every file ever written and pinned by the IPC contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Midi,
    Drum,
    Audio,
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
            is_audio: false,
            volume: 1.0,
            reverb_send: 0.0,
            tone: default_tone(),
            pan: 0.0,
            color: None,
            plugin: None,
            amp: None,
            inserts: vec![],
            drum_pads: crate::sound::default_kit(),
        }
    }

    pub fn drums(name: impl Into<String>) -> Self {
        let mut t = Track::new(name);
        t.is_drum = true;
        t
    }

    /// A new audio track: no voice, a default rig.
    pub fn audio(name: impl Into<String>) -> Self {
        let mut t = Track::new(name);
        t.is_audio = true;
        t.amp = Some(AmpRig::default());
        t
    }

    /// Audio wins over drum; [`ProjectModel::normalized`] guarantees they are never both set.
    pub fn kind(&self) -> TrackKind {
        if self.is_audio {
            TrackKind::Audio
        } else if self.is_drum {
            TrackKind::Drum
        } else {
            TrackKind::Midi
        }
    }
}

/// An audio file the project uses. The samples live in the app's audio store as
/// `<app data>/audio/<id>.wav`; the project only carries the description, so core stays free
/// of file paths and decoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSource {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    /// What the UI calls it: the imported file's name, or "take 3".
    pub name: String,
    pub frames: u64,
    pub sample_rate: u32,
    pub channels: u8,
}

impl AudioSource {
    /// Length at the source's own rate. 0 for a degenerate source.
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / self.sample_rate as f64
        }
    }
}

/// Longest fade a clip may carry, in seconds. Past this it is an automation curve, not a fade.
pub const MAX_FADE_SECONDS: f64 = 5.0;
/// Loudest a clip may be boosted, linear.
pub const MAX_CLIP_GAIN: f64 = 4.0;

/// A piece of an [`AudioSource`] on an audio track's timeline. Audio never warps: it is
/// anchored at a beat and then plays for a length in seconds, so a tempo change moves where
/// it starts but never what it sounds like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioClip {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub track_id: Uuid,
    pub source_id: Uuid,
    #[serde(default)]
    pub start_tick: i64,
    pub length_seconds: f64,
    /// How far into the source playback begins.
    #[serde(default)]
    pub offset_seconds: f64,
    /// Linear, 0…[`MAX_CLIP_GAIN`].
    #[serde(default = "one")]
    pub gain: f64,
    #[serde(default)]
    pub fade_in_seconds: f64,
    #[serde(default)]
    pub fade_out_seconds: f64,
    #[serde(default)]
    pub muted: bool,
    /// Schema ≤ 8 only; folded into `start_tick` by [`ProjectModel::normalized`].
    #[serde(default, rename = "startBeat", skip_serializing)]
    pub legacy_start_beat: Option<f64>,
}

impl AudioClip {
    pub fn new(track_id: Uuid, source_id: Uuid, start_tick: i64, length_seconds: f64) -> Self {
        AudioClip {
            id: Uuid::new_v4(),
            track_id,
            source_id,
            start_tick,
            length_seconds,
            offset_seconds: 0.0,
            gain: 1.0,
            fade_in_seconds: 0.0,
            fade_out_seconds: 0.0,
            muted: false,
            legacy_start_beat: None,
        }
    }

    /// Every number in range and finite, or `None` if the clip is nonsense.
    pub fn sanitized(mut self) -> Option<Self> {
        if !self.length_seconds.is_finite() {
            return None;
        }
        if self.start_tick < 0 || self.length_seconds <= 0.0 {
            return None;
        }
        self.offset_seconds = if self.offset_seconds.is_finite() {
            self.offset_seconds.max(0.0)
        } else {
            0.0
        };
        self.gain = if self.gain.is_finite() {
            self.gain.clamp(0.0, MAX_CLIP_GAIN)
        } else {
            1.0
        };
        let half = self.length_seconds * 0.5;
        self.fade_in_seconds = clamp_fade(self.fade_in_seconds, half);
        self.fade_out_seconds = clamp_fade(self.fade_out_seconds, half);
        Some(self)
    }

    /// Beat fields from an older file folded into the tick fields.
    fn migrated(&mut self) {
        migrate_tick(&mut self.start_tick, &mut self.legacy_start_beat);
    }
}

/// Fades may not overlap, so each is capped at half the clip.
fn clamp_fade(v: f64, half: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, MAX_FADE_SECONDS.min(half))
    } else {
        0.0
    }
}

/// Most pedals a rig will hold. Four fills the screen and bounds the per-sample cost.
pub const MAX_PEDALS: usize = 4;

/// A guitar rig on an audio track: input strip → pedals → amp head → cabinet → rack.
///
/// Plain numbers in real units; `dissonant-engine` turns them into DSP. Core never interprets
/// them beyond clamping, the same deal as [`TrackPlugin`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmpRig {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Input trim, -24…+24 dB.
    #[serde(default)]
    pub input_db: f64,
    #[serde(default)]
    pub gate: Gate,
    /// In signal order, at most [`MAX_PEDALS`].
    #[serde(default)]
    pub pedals: Vec<Pedal>,
    #[serde(default)]
    pub amp: AmpHead,
    /// The tone stack the Bass/Mid/Treble knobs drive. A separate choice from the amp: the same
    /// preamp through a British stack and an American one are two different amps, which is why
    /// Amp Designer makes you pick them independently.
    #[serde(default)]
    pub eq: EqModel,
    #[serde(default)]
    pub cab: Cabinet,
    #[serde(default)]
    pub rack: Rack,
}

fn yes() -> bool {
    true
}

/// The noise gate on the input strip — single-coils into a high-gain amp are unusable without
/// one, so it is on by default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Gate {
    pub on: bool,
    /// -80…0 dBFS.
    pub threshold_db: f64,
    /// 5…500 ms.
    pub release_ms: f64,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            on: true,
            threshold_db: -55.0,
            release_ms: 120.0,
        }
    }
}

/// One stompbox. Every knob is 0…10 like the real thing, except times and rates, which are in
/// their own units so the UI can print them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Pedal {
    Comp {
        on: bool,
        sustain: f64,
        level: f64,
    },
    Drive {
        on: bool,
        drive: f64,
        tone: f64,
        level: f64,
    },
    Wah {
        on: bool,
        sensitivity: f64,
        range: f64,
    },
    Chorus {
        on: bool,
        rate_hz: f64,
        depth: f64,
        mix: f64,
    },
    Phaser {
        on: bool,
        rate_hz: f64,
        depth: f64,
        feedback: f64,
    },
    Tremolo {
        on: bool,
        rate_hz: f64,
        depth: f64,
    },
    Delay {
        on: bool,
        time_ms: f64,
        feedback: f64,
        mix: f64,
    },
}

/// Which stompbox, without its settings. The UI builds its "add pedal" menu from this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PedalKind {
    Comp,
    Drive,
    Wah,
    Chorus,
    Phaser,
    Tremolo,
    Delay,
}

impl Pedal {
    pub const KINDS: [PedalKind; 7] = [
        PedalKind::Comp,
        PedalKind::Drive,
        PedalKind::Wah,
        PedalKind::Chorus,
        PedalKind::Phaser,
        PedalKind::Tremolo,
        PedalKind::Delay,
    ];

    pub fn kind(&self) -> PedalKind {
        match self {
            Pedal::Comp { .. } => PedalKind::Comp,
            Pedal::Drive { .. } => PedalKind::Drive,
            Pedal::Wah { .. } => PedalKind::Wah,
            Pedal::Chorus { .. } => PedalKind::Chorus,
            Pedal::Phaser { .. } => PedalKind::Phaser,
            Pedal::Tremolo { .. } => PedalKind::Tremolo,
            Pedal::Delay { .. } => PedalKind::Delay,
        }
    }

    pub fn on(&self) -> bool {
        match self {
            Pedal::Comp { on, .. }
            | Pedal::Drive { on, .. }
            | Pedal::Wah { on, .. }
            | Pedal::Chorus { on, .. }
            | Pedal::Phaser { on, .. }
            | Pedal::Tremolo { on, .. }
            | Pedal::Delay { on, .. } => *on,
        }
    }

    /// A fresh pedal of `kind`, switched on, every knob at noon.
    pub fn of(kind: PedalKind) -> Pedal {
        match kind {
            PedalKind::Comp => Pedal::Comp {
                on: true,
                sustain: 5.0,
                level: 5.0,
            },
            PedalKind::Drive => Pedal::Drive {
                on: true,
                drive: 5.0,
                tone: 5.0,
                level: 5.0,
            },
            PedalKind::Wah => Pedal::Wah {
                on: true,
                sensitivity: 5.0,
                range: 5.0,
            },
            PedalKind::Chorus => Pedal::Chorus {
                on: true,
                rate_hz: 1.2,
                depth: 5.0,
                mix: 5.0,
            },
            PedalKind::Phaser => Pedal::Phaser {
                on: true,
                rate_hz: 0.8,
                depth: 5.0,
                feedback: 4.0,
            },
            PedalKind::Tremolo => Pedal::Tremolo {
                on: true,
                rate_hz: 5.0,
                depth: 5.0,
            },
            PedalKind::Delay => Pedal::Delay {
                on: true,
                time_ms: 350.0,
                feedback: 3.0,
                mix: 3.0,
            },
        }
    }

    /// Every knob finite and in range, or `None` if any was NaN.
    pub fn sanitized(self) -> Option<Pedal> {
        let p = match self {
            Pedal::Comp { on, sustain, level } => Pedal::Comp {
                on,
                sustain: knob(sustain)?,
                level: knob(level)?,
            },
            Pedal::Drive {
                on,
                drive,
                tone,
                level,
            } => Pedal::Drive {
                on,
                drive: knob(drive)?,
                tone: knob(tone)?,
                level: knob(level)?,
            },
            Pedal::Wah {
                on,
                sensitivity,
                range,
            } => Pedal::Wah {
                on,
                sensitivity: knob(sensitivity)?,
                range: knob(range)?,
            },
            Pedal::Chorus {
                on,
                rate_hz,
                depth,
                mix,
            } => Pedal::Chorus {
                on,
                rate_hz: ranged(rate_hz, 0.1, 8.0)?,
                depth: knob(depth)?,
                mix: knob(mix)?,
            },
            Pedal::Phaser {
                on,
                rate_hz,
                depth,
                feedback,
            } => Pedal::Phaser {
                on,
                rate_hz: ranged(rate_hz, 0.1, 8.0)?,
                depth: knob(depth)?,
                feedback: knob(feedback)?,
            },
            Pedal::Tremolo { on, rate_hz, depth } => Pedal::Tremolo {
                on,
                rate_hz: ranged(rate_hz, 0.1, 14.0)?,
                depth: knob(depth)?,
            },
            Pedal::Delay {
                on,
                time_ms,
                feedback,
                mix,
            } => Pedal::Delay {
                on,
                time_ms: ranged(time_ms, 20.0, 1200.0)?,
                feedback: knob(feedback)?,
                mix: knob(mix)?,
            },
        };
        Some(p)
    }
}

fn knob(v: f64) -> Option<f64> {
    ranged(v, 0.0, 10.0)
}

fn ranged(v: f64, lo: f64, hi: f64) -> Option<f64> {
    v.is_finite().then(|| v.clamp(lo, hi))
}

/// The amp head: which voicing, and the knob row across its face. All 0…10.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AmpHead {
    pub model: AmpModel,
    pub gain: f64,
    pub bass: f64,
    pub mid: f64,
    pub treble: f64,
    pub presence: f64,
    pub master: f64,
}

impl Default for AmpHead {
    fn default() -> Self {
        AmpHead {
            model: AmpModel::Crunch,
            gain: 5.0,
            bass: 5.0,
            mid: 5.0,
            treble: 6.0,
            presence: 5.0,
            master: 5.0,
        }
    }
}

/// How hard the preamp bites, in four steps from a pedal platform to a wall of fuzz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AmpModel {
    Clean,
    #[default]
    Crunch,
    Lead,
    Fuzz,
}

impl AmpModel {
    pub const ALL: [AmpModel; 4] = [
        AmpModel::Clean,
        AmpModel::Crunch,
        AmpModel::Lead,
        AmpModel::Fuzz,
    ];
}

/// Which passive tone-stack topology sits between the preamp and the power amp. These are
/// circuits, not three independent EQ bands: the knobs load each other, so turning the mid down
/// on a British stack also moves the bass and treble. That interaction is most of what people
/// mean by an amp's "voice".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum EqModel {
    /// The American tone stack: scooped mids, slopes gently.
    #[default]
    UsClassic,
    /// The British stack: the same network with a bigger treble cap, so it is browner and the
    /// mid control bites harder.
    British,
    /// British with the bright cap across the volume pot — glassy, cuts through.
    BritishBright,
    /// A modern boutique voicing: flatter, far less scoop, usable everywhere.
    Boutique,
}

impl EqModel {
    pub const ALL: [EqModel; 4] = [
        EqModel::UsClassic,
        EqModel::British,
        EqModel::BritishBright,
        EqModel::Boutique,
    ];
}

/// The speaker cabinet, which microphone is in front of it, and where that microphone sits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cabinet {
    pub model: CabModel,
    pub mic_model: MicModel,
    /// Across the cone: 0 = out at the edge, dark and mellow; 10 = on the dust cap, bright and
    /// aggressive. The axis every engineer actually uses to dial a guitar tone.
    pub pos_x: f64,
    /// Away from the grille: 0 = touching it, with the proximity bass bump and a tight transient;
    /// 10 = backed off, rounder and softer.
    pub pos_z: f64,
}

impl Default for Cabinet {
    fn default() -> Self {
        Cabinet {
            model: CabModel::FourByTwelve,
            mic_model: MicModel::Dynamic,
            pos_x: 5.0,
            pos_z: 2.0,
        }
    }
}

/// Which microphone is on the grille. Three archetypes, the ones every studio owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum MicModel {
    /// Moving coil: presence peak around 5 kHz, rolled off on top, strong proximity effect.
    #[default]
    Dynamic,
    /// Large-diaphragm condenser: flat and extended, hears the fizz.
    Condenser,
    /// Ribbon: figure-of-eight, dark above 6 kHz, the softest of the three.
    Ribbon,
}

impl MicModel {
    pub const ALL: [MicModel; 3] = [MicModel::Dynamic, MicModel::Condenser, MicModel::Ribbon];
}

/// Cabinet size. `Direct` bypasses the speaker entirely — a DI sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CabModel {
    #[default]
    FourByTwelve,
    TwoByTwelve,
    OneByTwelve,
    Direct,
}

impl CabModel {
    pub const ALL: [CabModel; 4] = [
        CabModel::FourByTwelve,
        CabModel::TwoByTwelve,
        CabModel::OneByTwelve,
        CabModel::Direct,
    ];
}

/// What sits after the cabinet: room and output level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rack {
    /// Reverb mix, 0…10.
    pub reverb: f64,
    /// Reverb decay, 0…10.
    pub decay: f64,
    /// -24…+12 dB.
    pub output_db: f64,
}

impl Default for Rack {
    fn default() -> Self {
        Rack {
            reverb: 2.0,
            decay: 5.0,
            output_db: 0.0,
        }
    }
}

impl Default for AmpRig {
    fn default() -> Self {
        AmpRig {
            enabled: true,
            input_db: 0.0,
            gate: Gate::default(),
            pedals: vec![Pedal::Drive {
                on: false,
                drive: 5.0,
                tone: 5.0,
                level: 5.0,
            }],
            amp: AmpHead::default(),
            eq: EqModel::default(),
            cab: Cabinet::default(),
            rack: Rack::default(),
        }
    }
}

impl AmpRig {
    /// Every number finite and in range, pedals truncated to [`MAX_PEDALS`]. `None` when a
    /// value was NaN or infinite — a rig that cannot be made sane is rejected, not guessed at.
    pub fn sanitized(mut self) -> Option<AmpRig> {
        self.input_db = ranged(self.input_db, -24.0, 24.0)?;
        self.gate.threshold_db = ranged(self.gate.threshold_db, -80.0, 0.0)?;
        self.gate.release_ms = ranged(self.gate.release_ms, 5.0, 500.0)?;
        self.pedals.truncate(MAX_PEDALS);
        self.pedals = self
            .pedals
            .into_iter()
            .map(Pedal::sanitized)
            .collect::<Option<Vec<_>>>()?;
        self.amp.gain = knob(self.amp.gain)?;
        self.amp.bass = knob(self.amp.bass)?;
        self.amp.mid = knob(self.amp.mid)?;
        self.amp.treble = knob(self.amp.treble)?;
        self.amp.presence = knob(self.amp.presence)?;
        self.amp.master = knob(self.amp.master)?;
        self.cab.pos_x = knob(self.cab.pos_x)?;
        self.cab.pos_z = knob(self.cab.pos_z)?;
        self.rack.reverb = knob(self.rack.reverb)?;
        self.rack.decay = knob(self.rack.decay)?;
        self.rack.output_db = ranged(self.rack.output_db, -24.0, 12.0)?;
        Some(self)
    }
}

fn default_pattern_length() -> i64 {
    16 * TICKS_PER_BEAT
}

/// Shortest loop a pattern will subdivide into. Anything finer is a note, not a loop, and a
/// hand-edited file carrying a tick or two would otherwise tile thousands of copies. Only the
/// schema ≤ 9 migration still uses it, to flatten the loops those patterns carried.
const MIN_LOOP_TICKS: i64 = TICKS_PER_BEAT;

/// A loop on one track ("bass 1", "drums 2"). Its notes sit at pattern-local ticks; clips place
/// it on the song, always on its own track's lane, and a clip longer than the pattern loops it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongPattern {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    /// The track whose notes these are. Nil only in a schema ≤ 9 file, where one pattern held
    /// every track; [`ProjectModel::migrated_v10`] splits those and fills this in.
    #[serde(default)]
    pub track_id: Uuid,
    pub name: String,
    /// Zero only in a file that predates ticks or names no length at all; `normalized` fills
    /// it in from the legacy beats or the default.
    #[serde(default)]
    pub length_ticks: i64,
    #[serde(default)]
    pub notes: Vec<NoteEvent>,
    /// Schema ≤ 8 only; folded into `length_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "lengthBeats", skip_serializing)]
    pub legacy_length_beats: Option<f64>,
    /// Schema ≤ 9 only: the pattern's own chord progression, lifted onto the song by
    /// [`ProjectModel::migrated_v10`].
    #[serde(default, rename = "chords", skip_serializing, deserialize_with = "de_legacy_chords")]
    pub legacy_chords: crate::chord_track::ChordTrack,
    /// Schema ≤ 9 only: notes per track, split into one pattern each by
    /// [`ProjectModel::migrated_v10`].
    #[serde(
        default,
        rename = "notesByTrack",
        skip_serializing,
        deserialize_with = "de_notes_by_track"
    )]
    pub legacy_notes_by_track: HashMap<Uuid, Vec<NoteEvent>>,
    /// Schema ≤ 9 only: per-track loop length inside the pattern, flattened by the migration.
    #[serde(default, rename = "trackLoopTicks", skip_serializing)]
    pub legacy_track_loop_ticks: HashMap<Uuid, i64>,
    /// Schema ≤ 9 only: chord-track loop length, flattened by the migration.
    #[serde(default, rename = "chordLoopTicks", skip_serializing)]
    pub legacy_chord_loop_ticks: Option<i64>,
    /// Schema ≤ 8 only; folded into `legacy_track_loop_ticks` by the migration.
    #[serde(default, rename = "trackLoopBeats", skip_serializing)]
    pub legacy_track_loop_beats: Option<HashMap<Uuid, f64>>,
    /// Schema ≤ 8 only; folded into `legacy_chord_loop_ticks` by the migration.
    #[serde(default, rename = "chordLoopBeats", skip_serializing)]
    pub legacy_chord_loop_beats: Option<f64>,
}

/// `notesByTrack` is a JSON object keyed by track id — except in files written by the
/// original Swift app, whose `Codable` encoded a `[UUID: [NoteEvent]]` as a flat array of
/// alternating keys and values. Accept both.
#[derive(Deserialize)]
#[serde(untagged)]
enum NotesByTrackRepr {
    Map(HashMap<Uuid, Vec<NoteEvent>>),
    Flat(Vec<serde_json::Value>),
}

fn de_notes_by_track<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<HashMap<Uuid, Vec<NoteEvent>>, D::Error> {
    match NotesByTrackRepr::deserialize(d)? {
        NotesByTrackRepr::Map(m) => Ok(m),
        NotesByTrackRepr::Flat(values) => {
            let mut out = HashMap::new();
            for pair in values.chunks(2) {
                if pair.len() == 2 {
                    let key: Uuid = serde_json::from_value(pair[0].clone())
                        .map_err(serde::de::Error::custom)?;
                    let notes: Vec<NoteEvent> = serde_json::from_value(pair[1].clone())
                        .map_err(serde::de::Error::custom)?;
                    out.insert(key, notes);
                }
            }
            Ok(out)
        }
    }
}

/// A schema ≤ 9 pattern's `chords`. Schema 9 wrote the chord track as an object
/// (`{"chords": [...]}`) because `ChordTrack` was a plain struct then; older files and the
/// current one write a bare array. Accept both, or every v9 project would fail to open.
#[derive(Deserialize)]
#[serde(untagged)]
enum LegacyChordsRepr {
    Flat(Vec<ChordEvent>),
    Wrapped { chords: Vec<ChordEvent> },
}

fn de_legacy_chords<'de, D: Deserializer<'de>>(d: D) -> Result<crate::chord_track::ChordTrack, D::Error> {
    let chords = match LegacyChordsRepr::deserialize(d)? {
        LegacyChordsRepr::Flat(c) => c,
        LegacyChordsRepr::Wrapped { chords } => chords,
    };
    Ok(crate::chord_track::ChordTrack::new(chords))
}

impl SongPattern {
    pub fn new(track_id: Uuid, name: impl Into<String>) -> Self {
        SongPattern {
            id: Uuid::new_v4(),
            track_id,
            name: name.into(),
            length_ticks: default_pattern_length(),
            notes: vec![],
            legacy_length_beats: None,
            legacy_chords: Default::default(),
            legacy_notes_by_track: HashMap::new(),
            legacy_track_loop_ticks: HashMap::new(),
            legacy_chord_loop_ticks: None,
            legacy_track_loop_beats: None,
            legacy_chord_loop_beats: None,
        }
    }

    /// A deep copy on the same track with fresh pattern and note ids, named "<name> copy".
    pub fn duplicated(&self) -> Self {
        let mut copy = self.clone();
        copy.id = Uuid::new_v4();
        copy.name = format!("{} copy", self.name);
        let mut copy_of = HashMap::new();
        for n in &mut copy.notes {
            let fresh = Uuid::new_v4();
            copy_of.insert(n.id, fresh);
            n.id = fresh;
        }
        // Every source is in the copy, so every rope survives, pointing inside the copy.
        remap_ropes(&mut copy.notes, |id| copy_of.get(id).copied());
        copy
    }

    /// A stored loop length made usable: clamped into `[MIN_LOOP_TICKS, length_ticks]`, with
    /// anything missing or nonsensical meaning "fills the pattern".
    fn usable_loop(&self, ticks: Option<i64>) -> i64 {
        let len = self.length_ticks.max(1);
        match ticks {
            Some(t) if t > 0 => t.clamp(MIN_LOOP_TICKS.min(len), len),
            _ => len,
        }
    }

    /// Schema ≤ 9 only: one track's notes laid out across the whole pattern, the stored notes
    /// then one copy per further turn of that track's loop, each turn trimmed at its own end.
    fn legacy_looped_notes(&self, track_id: &Uuid) -> Vec<NoteEvent> {
        let empty = Vec::new();
        let src = self.legacy_notes_by_track.get(track_id).unwrap_or(&empty);
        let total = self.length_ticks.max(1);
        let turn = self.usable_loop(self.legacy_track_loop_ticks.get(track_id).copied());
        if turn >= total || src.is_empty() {
            return src.clone();
        }
        let mut out = Vec::with_capacity(src.len() * ((total + turn - 1) / turn) as usize);
        for (rep, base) in turns(turn, total) {
            let end = (base + turn).min(total);
            for note in src {
                let start = base + note.start_tick;
                if start >= end {
                    continue;
                }
                out.push(NoteEvent {
                    id: repeat_id(&note.id, rep),
                    // A rope inside the loop points at its source's copy in the same turn.
                    slide_from: note.slide_from.map(|s| repeat_id(&s, rep)),
                    start_tick: start,
                    length_ticks: ((start + note.length_ticks).min(end) - start)
                        .max(MIN_NOTE_TICKS),
                    ..note.clone()
                });
            }
        }
        out.sort_by_key(|n| n.start_tick);
        out
    }

    /// Schema ≤ 9 only: the pattern's chord track laid out across the whole pattern, the same
    /// way as [`SongPattern::legacy_looped_notes`].
    fn legacy_looped_chords(&self) -> Vec<ChordEvent> {
        let total = self.length_ticks.max(1);
        let turn = self.usable_loop(self.legacy_chord_loop_ticks);
        if turn >= total || self.legacy_chords.is_empty() {
            return self.legacy_chords.chords().to_vec();
        }
        let mut out = Vec::new();
        for (rep, base) in turns(turn, total) {
            let end = (base + turn).min(total);
            for chord in self.legacy_chords.chords() {
                let start = base + chord.start_tick;
                let cut = (start + chord.length_ticks).min(end);
                if cut <= start {
                    continue;
                }
                out.push(ChordEvent {
                    id: repeat_id(&chord.id, rep),
                    start_tick: start,
                    length_ticks: cut - start,
                    pitch_classes: chord.pitch_classes.clone(),
                    name: chord.name.clone(),
                    legacy_start_beat: None,
                    legacy_length_beats: None,
                });
            }
        }
        out
    }

    /// Beat fields from an older file folded into the tick fields, this pattern's own and
    /// those of everything it holds.
    fn migrated(&mut self) {
        migrate_tick(&mut self.length_ticks, &mut self.legacy_length_beats);
        if self.length_ticks <= 0 {
            self.length_ticks = default_pattern_length();
        }
        if let Some(legacy) = self.legacy_track_loop_beats.take() {
            if self.legacy_track_loop_ticks.is_empty() {
                self.legacy_track_loop_ticks = legacy
                    .into_iter()
                    .map(|(id, b)| (id, beats_to_ticks(b)))
                    .collect();
            }
        }
        if let Some(beats) = self.legacy_chord_loop_beats.take() {
            if self.legacy_chord_loop_ticks.is_none() {
                self.legacy_chord_loop_ticks = Some(beats_to_ticks(beats));
            }
        }
        for n in self.notes.iter_mut() {
            n.migrated();
        }
        for notes in self.legacy_notes_by_track.values_mut() {
            for n in notes.iter_mut() {
                n.migrated();
            }
        }
        self.legacy_chords.migrated();
    }
}

/// `(repetition, start tick)` for every turn of a `turn`-long loop inside `total` ticks.
fn turns(turn: i64, total: i64) -> impl Iterator<Item = (u64, i64)> {
    (0u64..)
        .map(move |rep| (rep, rep as i64 * turn))
        .take_while(move |&(_, base)| base < total)
}

/// Repetition 0 keeps the source id so an unlooped pattern is byte-identical to its notes;
/// later turns get a derived id that is stable across rebuilds and distinct per turn.
fn repeat_id(source: &Uuid, rep: u64) -> Uuid {
    if rep == 0 {
        *source
    } else {
        Uuid::new_v5(source, &rep.to_le_bytes())
    }
}

fn default_low_cut() -> f64 {
    20.0
}

/// The master chain's persisted settings (previously lived in UI state and was lost on save).
///
/// The container-level `default` is what keeps new fields off the schema version: a file written
/// before `ceiling_db` / `limiter` existed deserializes them from [`MasterSettings::default`],
/// not from the field type's default — which matters for `limiter`, whose honest value for an
/// old project is on, not `false`.
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
    /// Ceiling the limiter holds the output under, in dBFS.
    pub ceiling_db: f64,
    /// Off restores the old behaviour: a hard clamp at ±1.0.
    pub limiter: bool,
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
            ceiling_db: -0.3,
            limiter: true,
        }
    }
}

/// A pattern placed on the song timeline. The pattern loops from `offset_ticks` for
/// `length_ticks`, so a clip can be trimmed, extended or started mid-pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub pattern_id: Uuid,
    #[serde(default)]
    pub start_tick: i64,
    #[serde(default)]
    pub length_ticks: i64,
    #[serde(default)]
    pub offset_ticks: i64,
    #[serde(default)]
    pub muted: bool,
    /// Schema ≤ 8 only; folded into `start_tick` by [`ProjectModel::normalized`].
    #[serde(default, rename = "startBeat", skip_serializing)]
    pub legacy_start_beat: Option<f64>,
    /// Schema ≤ 8 only; folded into `length_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "lengthBeats", skip_serializing)]
    pub legacy_length_beats: Option<f64>,
    /// Schema ≤ 8 only; folded into `offset_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "offsetBeats", skip_serializing)]
    pub legacy_offset_beats: Option<f64>,
}

impl Clip {
    pub fn new(pattern_id: Uuid, start_tick: i64, length_ticks: i64) -> Self {
        Clip {
            id: Uuid::new_v4(),
            pattern_id,
            start_tick,
            length_ticks,
            offset_ticks: 0,
            muted: false,
            legacy_start_beat: None,
            legacy_length_beats: None,
            legacy_offset_beats: None,
        }
    }

    pub fn end_tick(&self) -> i64 {
        self.start_tick + self.length_ticks
    }

    /// Beat fields from an older file folded into the tick fields.
    fn migrated(&mut self) {
        migrate_tick(&mut self.start_tick, &mut self.legacy_start_beat);
        migrate_tick(&mut self.length_ticks, &mut self.legacy_length_beats);
        migrate_tick(&mut self.offset_ticks, &mut self.legacy_offset_beats);
    }
}

/// A named marker on the song (intro, verse, drop…). With a `key`, the section modulates:
/// patterns placed inside it are tiered against that key instead of the project key.
///
/// A section has an extent, not just a position: see [`ProjectModel::section_span`]. Without
/// one it would only ever be a label hung in front of music it does not own, and moving or
/// copying "the chorus" could not move the chorus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub start_tick: i64,
    /// How far the section reaches, or 0 for "up to the next section, then the song's end" —
    /// the implicit extent every section had before this field existed, and still the one a
    /// marker dropped on the ruler gets.
    ///
    /// `#[serde(default)]` plus skipping the implicit 0 rather than a schema bump: an older
    /// file has no explicit extent, which is exactly what 0 means, and a project whose
    /// sections all run to the next marker serializes byte-identically to what schema 10
    /// already wrote. Those are the two things a schema version exists to signal.
    #[serde(default, skip_serializing_if = "is_implicit_extent")]
    pub length_ticks: i64,
    #[serde(default)]
    pub key: Option<KeyState>,
    #[serde(default)]
    pub color: Option<String>,
    /// Schema ≤ 8 only; folded into `start_tick` by [`ProjectModel::normalized`].
    #[serde(default, rename = "startBeat", skip_serializing)]
    pub legacy_start_beat: Option<f64>,
}

impl Section {
    /// A marker with the implicit extent: it runs to the next section, then to the song's end.
    pub fn new(name: impl Into<String>, start_tick: i64) -> Self {
        Section {
            id: Uuid::new_v4(),
            name: name.into(),
            start_tick,
            length_ticks: 0,
            key: None,
            color: None,
            legacy_start_beat: None,
        }
    }

    /// Beat fields from an older file folded into the tick fields.
    fn migrated(&mut self) {
        migrate_tick(&mut self.start_tick, &mut self.legacy_start_beat);
    }
}

/// Serde: an implicit extent is the absence of one, so it is not written.
fn is_implicit_extent(length_ticks: &i64) -> bool {
    *length_ticks <= 0
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
        TimeSignature {
            numerator: 4,
            denominator: 4,
        }
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

/// The song's hard end for a new project, in ticks.
fn default_song_length() -> i64 {
    DEFAULT_SONG_BARS as i64 * beats_to_ticks(TimeSignature::default().beats_per_bar())
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
    /// Every audio file the project refers to. Kept even when nothing points at one, so an
    /// undone take can be redone without decoding it again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_sources: Vec<AudioSource>,
    /// Audio placed on the song timeline, each in its own track's lane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_clips: Vec<AudioClip>,
    /// Song markers, optionally with their own key.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Legacy (schema ≤ 3): an ordered list of pattern ids played back to back. Folded into
    /// `clips` by [`ProjectModel::normalized`] and never written again.
    #[serde(default, skip_serializing)]
    pub arrangement: Vec<Uuid>,
    #[serde(default)]
    pub master: MasterSettings,
    #[serde(default)]
    pub time_signature: TimeSignature,
    /// Global swing: 50 = straight, 66.7 = triplet feel, 75 = hard shuffle. See
    /// [`crate::tempo::swing_warp_ticks`].
    #[serde(default = "default_swing")]
    pub swing: f64,
    /// The subdivision swing acts on, in ticks (480 = eighths, 240 = sixteenths).
    #[serde(default)]
    pub swing_grid_ticks: i64,
    /// Tempo changes along the song (empty = `tempo` throughout). Pattern mode plays at the
    /// base tempo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tempo_points: Vec<crate::tempo::TempoPoint>,
    /// The project's chord progression in song ticks: sorted, non-overlapping. It drives the
    /// tiering everywhere and the (monitor-only) chord bed.
    #[serde(default)]
    pub chords: crate::chord_track::ChordTrack,
    /// The hard end of the song, at least one bar. Clips never reach past it.
    #[serde(default = "default_song_length")]
    pub song_length_ticks: i64,
    /// The song's loop brace in song ticks, `(start, end)` with `end > start`; `None` loops the
    /// whole song. It lives on the model, not in UI state, because it marks a place in the
    /// *music* — "the chorus" — and the project file is the only thing that outlives a session:
    /// there is no per-project UI sidecar, and inventing one to hold a pair of ticks would be a
    /// second place a song can live. `#[serde(default)]` lets older files open, and skipping
    /// `None` keeps a project without a loop byte-identical, so the schema does not move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_region: Option<(i64, i64)>,
    /// The master effect chain, in order, ahead of the master bus processing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub master_inserts: Vec<Insert>,
    /// Parameters that move along the song timeline, one lane per target.
    ///
    /// No schema bump: `default` lets every older file open with no lanes, and skipping an
    /// empty vector keeps a project that automates nothing byte-identical to what schema 10
    /// already wrote — the two things a schema version exists to signal. A reader of this
    /// version that meets a lane it cannot resolve (a plugin parameter) ignores it rather than
    /// failing, so the format does not have to move again when those land either.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<crate::automation::AutomationLane>,
    /// Copies of the non-factory sounds the tracks use, so the project plays anywhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounds: Vec<crate::sound::Sound>,
    /// Schema ≤ 8 only; folded into `swing_grid_ticks` by [`ProjectModel::normalized`].
    #[serde(default, rename = "swingGrid", skip_serializing)]
    pub legacy_swing_grid: Option<f64>,
}

fn default_swing() -> f64 {
    50.0
}
fn default_swing_grid() -> i64 {
    TICKS_PER_BEAT / 2
}

impl Default for ProjectModel {
    fn default() -> Self {
        ProjectModel::empty()
    }
}

/// A loop brace held inside `[0, song_end]`, or `None` when nothing is left of it. One rule for
/// both the editing command and the file fixups, so a hand-edited file and a drag agree.
pub(crate) fn clamp_loop(start: i64, end: i64, song_end: i64) -> Option<(i64, i64)> {
    let start = start.clamp(0, song_end);
    let end = end.clamp(0, song_end);
    (end > start).then_some((start, end))
}

/// Clean up a chord progression: drop chords with a bad position or length, fold the pitch
/// classes into 0–11 without duplicates (keeping their order, so the root stays first), sort
/// by start, and cut each chord at the next one's start (the later chord wins, as in
/// [`crate::chord_track::ChordTrack::chord_at`]) so none overlap.
pub fn tidy_chords(chords: Vec<ChordEvent>) -> Vec<ChordEvent> {
    let mut chords: Vec<ChordEvent> = chords
        .into_iter()
        .filter(|c| c.start_tick >= 0 && c.length_ticks > 0)
        .map(|mut c| {
            let mut seen = Vec::with_capacity(c.pitch_classes.len());
            for pc in c
                .pitch_classes
                .iter()
                .map(|&pc| crate::theory::normalize(pc))
            {
                if !seen.contains(&pc) {
                    seen.push(pc);
                }
            }
            c.pitch_classes = seen;
            c
        })
        .collect();
    chords.sort_by_key(|c| c.start_tick);
    let starts: Vec<i64> = chords.iter().map(|c| c.start_tick).collect();
    for (i, c) in chords.iter_mut().enumerate() {
        if let Some(&next) = starts.get(i + 1) {
            c.length_ticks = c.length_ticks.min(next - c.start_tick);
        }
    }
    chords.retain(|c| c.length_ticks > 0);
    chords
}

/// Repeat a progression written over `[0, period)` back to back up to `end`, cutting the last
/// pass there. Every copy gets fresh chord ids.
pub fn tile_chords(progression: &[ChordEvent], period: i64, end: i64) -> Vec<ChordEvent> {
    let mut out = Vec::new();
    if period <= 0 {
        return out;
    }
    let mut at = 0;
    while at < end {
        for c in progression {
            let start = at + c.start_tick;
            if start >= end {
                continue;
            }
            out.push(ChordEvent::new(
                start,
                c.length_ticks.min(end - start),
                c.pitch_classes.clone(),
                c.name.clone(),
            ));
        }
        at += period;
    }
    out
}

/// The starter melody: one phrase per chord of I–IV–V–vi in C, as `(start, length, pitch,
/// velocity)` in beats inside a 4-beat chord slot. Mostly chord tones, plus two tensions (D
/// over C, B leaning into C over Am) so the roll shows more than one tier on the first look.
const STARTER_MELODY: [[(f64, f64, i32, i32); 3]; 4] = [
    [(0.0, 1.5, 76, 100), (1.5, 0.5, 74, 80), (2.0, 2.0, 72, 96)], // C:  E  D  C
    [(0.0, 1.0, 69, 100), (1.0, 1.0, 72, 88), (2.0, 2.0, 77, 96)], // F:  A  C  F
    [(0.0, 1.5, 74, 100), (1.5, 0.5, 71, 80), (2.0, 2.0, 67, 96)], // G:  D  B  G
    [(0.0, 1.5, 72, 100), (1.5, 0.5, 71, 80), (2.0, 2.0, 69, 96)], // Am: C  B  A
];

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
            audio_sources: vec![],
            audio_clips: vec![],
            sections: vec![],
            arrangement: vec![],
            master: MasterSettings::default(),
            time_signature: TimeSignature::default(),
            swing: default_swing(),
            swing_grid_ticks: default_swing_grid(),
            tempo_points: vec![],
            chords: Default::default(),
            song_length_ticks: default_song_length(),
            loop_region: None,
            master_inserts: vec![],
            automation: vec![],
            sounds: vec![],
            legacy_swing_grid: None,
        }
    }

    /// The song's tempo map (base tempo plus points).
    pub fn tempo_map(&self) -> crate::tempo::TempoMap {
        crate::tempo::TempoMap::new(self.tempo, &self.tempo_points)
    }

    /// Ticks in one bar of the project's meter.
    pub fn bar_ticks(&self) -> i64 {
        beats_to_ticks(self.time_signature.beats_per_bar()).max(1)
    }

    /// A new project that plays music the moment space is pressed: I–IV–V–vi in C (key
    /// locked), a short melody over it and a drum groove, each in its own 4-bar 4/4 pattern
    /// looping through a [`DEFAULT_SONG_BARS`]-bar song. See [`ProjectModel::starter_shaped`].
    pub fn starter() -> Self {
        Self::starter_shaped(16 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT, DEFAULT_SONG_BARS)
    }

    /// The starter at any pattern length, bar size and song length (the new-project defaults).
    /// The four chords split the pattern evenly and the melody stretches with them, so every
    /// note keeps its tier; the progression is tiled across the whole song. The groove repeats
    /// per bar at its own speed instead of stretching. Each track's pattern is placed once from
    /// tick 0 to the song end.
    pub fn starter_shaped(length_ticks: i64, bar_ticks: i64, song_bars: u32) -> Self {
        let length = if length_ticks > 0 {
            length_ticks
        } else {
            default_pattern_length()
        };
        let bar = if bar_ticks > 0 {
            bar_ticks
        } else {
            4 * TICKS_PER_BEAT
        };
        let mut model = ProjectModel::empty();
        model.key = KeyState::locked(0, ScaleType::Major);
        model.song_length_ticks = song_bars.max(1) as i64 * bar;
        let slot = length / STARTER_MELODY.len() as i64;
        let progression =
            crate::theory::harmony::progression(&[0, 3, 4, 5], 0, ScaleType::Major, length, slot);
        model.chords = crate::chord_track::ChordTrack::new(tile_chords(
            &progression,
            length,
            model.song_length_ticks,
        ));

        model.tracks[0].voice = "pluck".into();
        model.tracks[0].reverb_send = 0.2;
        let scale = slot as f64 / (4.0 * TICKS_PER_BEAT as f64);
        model.patterns[0].length_ticks = length;
        model.patterns[0].notes = STARTER_MELODY
            .iter()
            .enumerate()
            .flat_map(|(i, phrase)| {
                phrase.iter().map(move |&(start, len, pitch, vel)| {
                    NoteEvent::new(
                        i as i64 * slot + beats_to_ticks(start * scale),
                        beats_to_ticks(len * scale),
                        pitch,
                    )
                    .with_velocity(vel)
                })
            })
            .collect();

        let drums = Track::drums("drums");
        let mut groove = SongPattern::new(drums.id, "drums 1");
        groove.length_ticks = length;
        groove.notes = crate::vibes::Groove::Backbeat.hits(length, bar);
        model.tracks.push(drums);
        model.patterns.push(groove);
        model.place_every_pattern_across_the_song();
        model
    }

    /// One clip per pattern from tick 0 to the song end (the pattern loops to fill it). Used by
    /// the starter and the vibes, where every track has exactly one pattern.
    pub(crate) fn place_every_pattern_across_the_song(&mut self) {
        let end = self.song_length_ticks;
        self.clips = self
            .patterns
            .iter()
            .map(|p| Clip::new(p.id, 0, end))
            .collect();
    }

    pub fn clip(&self, id: &Uuid) -> Option<&Clip> {
        self.clips.iter().find(|c| &c.id == id)
    }

    pub fn section(&self, id: &Uuid) -> Option<&Section> {
        self.sections.iter().find(|s| &s.id == id)
    }

    /// The patterns that belong to one track, in project order.
    pub fn patterns_of<'a>(
        &'a self,
        track_id: &'a Uuid,
    ) -> impl Iterator<Item = &'a SongPattern> + 'a {
        self.patterns
            .iter()
            .filter(move |p| &p.track_id == track_id)
    }

    /// Hold the song's hard end: clips starting at or after it go, clips crossing it are cut,
    /// and the loop brace is pulled back inside (a brace left hanging past the end would loop
    /// over silence).
    pub(crate) fn fit_clips_to_song(&mut self) {
        let end = self.song_length_ticks;
        self.clips.retain(|c| c.start_tick < end);
        for c in &mut self.clips {
            if c.end_tick() > end {
                c.length_ticks = end - c.start_tick;
            }
        }
        self.loop_region = self.loop_region.and_then(|(s, e)| clamp_loop(s, e, end));
    }

    /// Where a section stops, or `None` when nothing bounds it: with no explicit extent and no
    /// later marker, the last section names the music to the end of time, which is what every
    /// section did before extents existed.
    fn section_end(&self, section: &Section) -> Option<i64> {
        if section.length_ticks > 0 {
            return Some(section.start_tick + section.length_ticks);
        }
        self.sections
            .iter()
            .map(|s| s.start_tick)
            .filter(|&t| t > section.start_tick)
            .min()
    }

    /// A section's extent in song ticks, `[start, end)`: its own `length_ticks`, else the
    /// stretch up to the next marker, else the rest of the song. This is the music the section
    /// owns — what the arrangement draws as its band, and what moving or copying it moves.
    pub fn section_span(&self, section: &Section) -> (i64, i64) {
        (
            section.start_tick,
            self.section_end(section)
                .unwrap_or(self.song_length_ticks)
                .max(section.start_tick),
        )
    }

    /// The section whose extent contains `tick` (the last one, where two overlap).
    pub fn section_at(&self, tick: i64) -> Option<&Section> {
        self.sections
            .iter()
            .filter(|s| s.start_tick <= tick && self.section_end(s).is_none_or(|e| tick < e))
            .max_by_key(|s| s.start_tick)
    }

    /// The key in force at `tick`: the section's key if it has one, else the project key.
    pub fn key_at(&self, tick: i64) -> KeyState {
        self.section_at(tick)
            .and_then(|s| s.key)
            .unwrap_or(self.key)
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

    pub fn audio_clip(&self, id: &Uuid) -> Option<&AudioClip> {
        self.audio_clips.iter().find(|c| &c.id == id)
    }

    pub fn audio_source(&self, id: &Uuid) -> Option<&AudioSource> {
        self.audio_sources.iter().find(|s| &s.id == id)
    }

    /// Every audio clip on one track, in timeline order.
    pub fn audio_clips_on(&self, track_id: Uuid) -> impl Iterator<Item = &AudioClip> {
        self.audio_clips
            .iter()
            .filter(move |c| c.track_id == track_id)
    }

    /// Normalize after decoding a foreign/older file: fold schema ≤ 8 beat fields into ticks,
    /// split schema ≤ 9 patterns into one per track, bump the schema, make sure there is at
    /// least one track, clamp every note, tidy the chords, hold the song length to at least a
    /// bar with every clip inside it, and drop references to things that aren't in the project
    /// any more (patterns of a missing track, clips of a missing pattern).
    pub fn normalized(mut self) -> Self {
        self.migrate_beats();
        // Before the v10 split, while the arrangement's pattern ids still name patterns that
        // exist: the split mints new ids and repoints the clips it finds.
        self.fold_legacy_arrangement();
        self.migrate_v10();
        // Only ever bump forward: relabelling a newer file as ours would make the next save
        // overwrite it with fields this version dropped.
        self.schema_version = self.schema_version.max(SCHEMA_VERSION);
        if self.tracks.is_empty() {
            let fresh = ProjectModel::empty();
            self.tracks = fresh.tracks;
            self.patterns = fresh.patterns;
        }
        // A hand-edited kit the commands would refuse (empty, too big, repeated pitches) falls
        // back to the default kit rather than playing half-broken.
        for t in &mut self.tracks {
            if !crate::sound::valid_kit(&t.drum_pads) {
                t.drum_pads = crate::sound::default_kit();
            }
        }
        self.sounds = std::mem::take(&mut self.sounds).into_iter().map(crate::sound::Sound::sanitized).collect();
        self.prune_sounds();
        let track_ids: Vec<Uuid> = self.tracks.iter().map(|t| t.id).collect();
        // A pattern of a track that no longer exists is invisible and never played, but would
        // survive every save; drop it instead of growing the file forever.
        self.patterns.retain(|p| track_ids.contains(&p.track_id));
        for pattern in &mut self.patterns {
            for n in pattern.notes.iter_mut() {
                *n = n.clone().sanitized();
            }
            // A hand-edited file can name a source that isn't there, or loop.
            tidy_ropes(&mut pattern.notes);
        }
        let pattern_ids: Vec<Uuid> = self.patterns.iter().map(|p| p.id).collect();
        if !self.swing.is_finite() {
            self.swing = default_swing();
        }
        self.swing = self.swing.clamp(50.0, 75.0);
        if !(self.swing_grid_ticks == TICKS_PER_BEAT / 2
            || self.swing_grid_ticks == TICKS_PER_BEAT / 4)
        {
            self.swing_grid_ticks = default_swing_grid();
        }
        self.tempo_points
            .retain(|p| p.tick >= 0 && p.bpm.is_finite());
        for p in &mut self.tempo_points {
            p.bpm = p
                .bpm
                .clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM);
        }
        self.tempo_points.sort_by_key(|p| p.tick);
        self.chords = crate::chord_track::ChordTrack::new(tidy_chords(self.chords.take()));
        if self.song_length_ticks <= 0 {
            self.song_length_ticks = default_song_length();
        }
        self.song_length_ticks = self.song_length_ticks.max(self.bar_ticks());
        self.clips.retain(|c| {
            pattern_ids.contains(&c.pattern_id) && c.length_ticks > 0 && c.start_tick >= 0
        });
        for c in &mut self.clips {
            c.offset_ticks = c.offset_ticks.max(0);
        }
        self.fit_clips_to_song();
        self.clips.sort_by_key(|c| c.start_tick);
        // An audio track is never also a drum track, and always carries a rig; a note track
        // never does. The combinations the UI cannot produce are still reachable by hand-editing
        // a file, so they are resolved here rather than guarded at every read.
        for t in &mut self.tracks {
            if t.is_audio {
                t.is_drum = false;
                let rig = t.amp.take().unwrap_or_default();
                t.amp = Some(rig.sanitized().unwrap_or_default());
            } else {
                t.amp = None;
            }
            sanitize_inserts(&mut t.inserts);
        }
        sanitize_inserts(&mut self.master_inserts);
        // A lane of a deleted track is invisible and never played, but would survive every
        // save; the same rule patterns get. One lane per target, so the UI never has two
        // drawings of one parameter fighting.
        self.automation.retain(|l| match l.target.track_id() {
            Some(id) => track_ids.contains(&id),
            None => true,
        });
        for lane in &mut self.automation {
            lane.tidy();
        }
        self.automation.retain(|l| !l.points.is_empty());
        let mut seen: Vec<crate::automation::AutoTarget> = Vec::new();
        self.automation.retain(|l| {
            let fresh = !seen.contains(&l.target);
            if fresh {
                seen.push(l.target);
            }
            fresh
        });
        let audio_track_ids: Vec<Uuid> = self
            .tracks
            .iter()
            .filter(|t| t.is_audio)
            .map(|t| t.id)
            .collect();
        let source_ids: Vec<Uuid> = self.audio_sources.iter().map(|s| s.id).collect();
        // Unreferenced sources stay: undoing the clip that used one must not force a re-decode.
        self.audio_clips = std::mem::take(&mut self.audio_clips)
            .into_iter()
            .filter(|c| audio_track_ids.contains(&c.track_id) && source_ids.contains(&c.source_id))
            .filter_map(AudioClip::sanitized)
            .collect();
        self.audio_clips.sort_by_key(|c| c.start_tick);
        // A negative extent is nonsense; 0 is the implicit one, so that is where it lands.
        for s in &mut self.sections {
            s.length_ticks = s.length_ticks.max(0);
        }
        self.sections.sort_by_key(|s| s.start_tick);
        self
    }

    /// Fold every schema ≤ 8 beat field into its tick field, then forget the beats. Runs
    /// before the other fixups so they only ever see ticks — a legacy loop length has to be
    /// in ticks before it can be clamped against a pattern length that is also in ticks.
    fn migrate_beats(&mut self) {
        migrate_tick(&mut self.swing_grid_ticks, &mut self.legacy_swing_grid);
        for pattern in &mut self.patterns {
            pattern.migrated();
        }
        for clip in &mut self.clips {
            clip.migrated();
        }
        for clip in &mut self.audio_clips {
            clip.migrated();
        }
        for section in &mut self.sections {
            section.migrated();
        }
        for point in &mut self.tempo_points {
            point.migrated();
        }
        // The song progression is written in beats by every build before ticks; without this
        // its chords would decode with no position or length and be dropped as nonsense.
        self.chords.migrated();
    }

    /// Schema ≤ 3 named the song as an ordered list of pattern ids; lay those out as clips,
    /// back to back. A file that already has clips keeps them.
    fn fold_legacy_arrangement(&mut self) {
        if self.clips.is_empty() && !self.arrangement.is_empty() {
            let mut at = 0;
            for pid in &self.arrangement {
                if let Some(p) = self.patterns.iter().find(|p| &p.id == pid) {
                    self.clips.push(Clip::new(p.id, at, p.length_ticks));
                    at += p.length_ticks;
                }
            }
        }
        self.arrangement.clear();
    }

    /// Split every schema ≤ 9 pattern — one pattern held every track's notes and its own chord
    /// track — into one pattern per track that had notes, repoint that pattern's clips at each
    /// of them, and lift its chords onto the song timeline under those same clips (tiling the
    /// pattern across a longer clip, as playback did). A file already on v10 passes through.
    fn migrate_v10(&mut self) {
        if !self.patterns.iter().any(|p| p.track_id.is_nil()) {
            return;
        }
        let track_order: Vec<(Uuid, String)> =
            self.tracks.iter().map(|t| (t.id, t.name.clone())).collect();
        let old_clips = std::mem::take(&mut self.clips);
        let mut patterns = Vec::new();
        let mut clips = Vec::new();
        let mut lifted = Vec::new();
        for old in std::mem::take(&mut self.patterns) {
            if !old.track_id.is_nil() {
                clips.extend(old_clips.iter().filter(|c| c.pattern_id == old.id).cloned());
                patterns.push(old);
                continue;
            }
            let mine: Vec<&Clip> = old_clips
                .iter()
                .filter(|c| c.pattern_id == old.id)
                .collect();
            let period = old.length_ticks.max(1);
            let chords = old.legacy_looped_chords();
            for clip in &mine {
                let span = clip.offset_ticks + clip.length_ticks;
                for c in tile_chords(&chords, period, span) {
                    let start = clip.start_tick + c.start_tick - clip.offset_ticks;
                    let cut = (start + c.length_ticks).min(clip.end_tick());
                    if cut > start.max(clip.start_tick) {
                        let start = start.max(clip.start_tick);
                        lifted.push(ChordEvent::new(
                            start,
                            cut - start,
                            c.pitch_classes.clone(),
                            c.name.clone(),
                        ));
                    }
                }
            }
            for (track_id, track_name) in &track_order {
                let notes = old.legacy_looped_notes(track_id);
                if notes.is_empty() {
                    continue;
                }
                let mut split = SongPattern::new(*track_id, format!("{} {}", track_name, old.name));
                split.length_ticks = old.length_ticks;
                split.notes = notes;
                clips.extend(mine.iter().map(|c| Clip {
                    id: Uuid::new_v4(),
                    pattern_id: split.id,
                    ..(*c).clone()
                }));
                patterns.push(split);
            }
        }
        self.patterns = patterns;
        self.clips = clips;
        let mut all = self.chords.take();
        all.extend(lifted);
        self.chords = crate::chord_track::ChordTrack::new(tidy_chords(all));
        // A pre-v10 file never said how long the song was: it ended with its last clip.
        let end = self.clips.iter().map(Clip::end_tick).max().unwrap_or(0);
        self.song_length_ticks = self.song_length_ticks.max(end);
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        Self::from_value(serde_json::from_str(json)?)
    }

    /// Decode an already-parsed project (an autosave carries one inside its own JSON).
    pub fn from_value(value: serde_json::Value) -> serde_json::Result<Self> {
        use serde::de::Error;
        // Every field defaults, and serde also accepts a struct in sequence form — so `[]` and
        // `[1,2]` would both decode as a blank project and the next save would destroy whatever
        // file the user actually opened. A project is an object.
        if !value.is_object() {
            return Err(serde_json::Error::custom(
                "not a dissonant project: expected a JSON object",
            ));
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
