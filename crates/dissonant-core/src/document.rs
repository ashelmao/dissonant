//! The document: a [`ProjectModel`] plus undo/redo, mutated only through [`Command`]s.
//!
//! Every command is validated and applied here, so the UI, tests and any future scripting or
//! collaboration layer all go through one door. An undo step is a [`Patch`], so it can be taken
//! back without touching anything somebody else did since; continuous controls (sliders) send
//! `transient` commands which coalesce into one undo step per gesture.

use crate::automation::{AutoTarget, AutomationLane, Breakpoint, Curve};
use crate::midi_file::ImportedTrack;
use crate::patch::Patch;
use crate::model::{
    AmpRig, AudioClip, AudioSource, ChordEvent, Clip, ClipDetection, Insert, KeyState,
    MasterSettings, NoteEvent, ProjectModel, Section, SongPattern, Track, TrackPlugin,
    DEFAULT_PUNCH_CROSSFADE_SECONDS, MAX_FADE_SECONDS, MAX_INSERTS,
};
use crate::sound::{is_factory, valid_kit, DrumPad, Sound};
use crate::tempo::{TempoMap, TempoPoint};
use crate::ticks::TICKS_PER_BEAT;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackParam {
    Volume,
    ReverbSend,
    Tone,
    Pan,
}

/// Which effect chain an insert command addresses: one track's, or the master's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsertTarget {
    Track(Uuid),
    Master,
}

/// Every edit the UI can make. Serialized as `{"type": "...", ...}` over Tauri IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Command {
    SetTempo {
        bpm: f64,
    },
    SetKey {
        key: KeyState,
    },
    /// A new track, with one empty pattern ("<track name> 1") to write into.
    AddTrack {
        is_drum: bool,
    },
    /// Removes the track, its patterns and their clips.
    DeleteTrack {
        id: Uuid,
    },
    RenameTrack {
        id: Uuid,
        name: String,
    },
    SetTrackMuted {
        id: Uuid,
        muted: bool,
    },
    SetTrackSoloed {
        id: Uuid,
        soloed: bool,
    },
    MoveTrack {
        id: Uuid,
        up: bool,
    },
    SetTrackVoice {
        id: Uuid,
        voice: String,
    },
    /// Point a synth track at a sound. A non-factory sound is embedded in the project (replacing
    /// any copy with the same id); embedded sounds no track uses any more are dropped.
    SetTrackSound {
        id: Uuid,
        sound: Sound,
    },
    /// Replace a drum track's whole kit (1–16 pads, unique pitches). `sounds` carries the
    /// non-factory drum sounds the pads use, to embed. Unused embedded sounds are dropped.
    SetDrumPads {
        id: Uuid,
        pads: Vec<DrumPad>,
        sounds: Vec<Sound>,
    },
    /// Replace the project's embedded copy of a sound (after it was re-saved in the library).
    UpsertSound {
        sound: Sound,
    },
    SetTrackParam {
        id: Uuid,
        param: TrackParam,
        value: f64,
    },
    /// `#rrggbb`, or `None` to fall back to the palette.
    SetTrackColor {
        id: Uuid,
        color: Option<String>,
    },
    /// Load a plugin on a track, or `None` to go back to the built-in voice.
    SetTrackPlugin {
        id: Uuid,
        plugin: Option<TrackPlugin>,
    },
    /// The plugin's own state blob (base64) after the user edited it in the plugin's window.
    SetPluginState {
        id: Uuid,
        state: String,
    },
    /// Add an effect to a chain, at `index` or appended. The slot's id is minted here.
    AddInsert {
        target: InsertTarget,
        uid: String,
        name: String,
        index: Option<usize>,
    },
    RemoveInsert {
        target: InsertTarget,
        insert_id: Uuid,
    },
    /// Move an insert within its chain; `to_index` is clamped to the chain.
    MoveInsert {
        target: InsertTarget,
        insert_id: Uuid,
        to_index: usize,
    },
    /// Bypass (`on: false`) or re-enable an insert, leaving it in place in the chain.
    SetInsertEnabled {
        target: InsertTarget,
        insert_id: Uuid,
        on: bool,
    },
    /// The effect's own state blob (base64) after the user edited it. Sent `transient` while
    /// a plugin window is open, so a run of them coalesces into one undo step like a knob drag.
    SetInsertState {
        target: InsertTarget,
        insert_id: Uuid,
        state: String,
    },
    /// One knob on a native effect. Sent `transient` while the knob is being dragged, so the
    /// whole gesture coalesces into one undo step — the same bargain [`Command::SetInsertState`]
    /// makes for a plugin, except that here the value is the document's own and there is no
    /// opaque blob to read back.
    SetInsertParam {
        target: InsertTarget,
        insert_id: Uuid,
        param_id: u32,
        value: f64,
    },
    /// A new empty pattern on a track, named "<track name> <n>".
    AddPattern {
        track_id: Uuid,
    },
    /// A copy on the same track, with fresh note ids.
    DuplicatePattern {
        id: Uuid,
    },
    /// Removes the pattern and its clips. A track may end up with no patterns.
    DeletePattern {
        id: Uuid,
    },
    RenamePattern {
        id: Uuid,
        name: String,
    },
    SetPatternLength {
        id: Uuid,
        ticks: i64,
    },
    /// Replace a pattern's notes (the roll commits whole arrays).
    SetNotes {
        pattern_id: Uuid,
        notes: Vec<NoteEvent>,
    },
    /// Change some of a pattern's notes and leave the rest alone. This is what a shared session
    /// turns [`Command::SetNotes`] into, so two people in one pattern don't erase each other: an
    /// id in `remove` that is already gone is not an error, and a note in `upsert` replaces the
    /// note with its id or joins the pattern.
    PatchNotes {
        pattern_id: Uuid,
        upsert: Vec<NoteEvent>,
        remove: Vec<Uuid>,
    },
    /// Rope `note_id` to `source_id`, both in `pattern_id`: the source's voice will slide into
    /// this note while the rope is live (see [`crate::model::rope_is_live`]). Replaces any rope
    /// the note had. A note can't slide from itself or from a note downstream of it.
    SetRope {
        pattern_id: Uuid,
        note_id: Uuid,
        source_id: Uuid,
    },
    /// Cut `note_id`'s rope, and its slide time with it. A note with no rope is left alone.
    ClearRope {
        pattern_id: Uuid,
        note_id: Uuid,
    },
    /// The slide time of `note_id`'s rope in seconds (clamped to 0–2), or `None` to go back to
    /// the patch's glide. Sent `transient` while the rope's midpoint is dragged.
    SetSlideTime {
        pattern_id: Uuid,
        note_id: Uuid,
        secs: Option<f32>,
    },
    /// Replace the project's chord progression (song ticks).
    SetChords {
        chords: Vec<ChordEvent>,
    },
    /// Change some chords and leave the rest alone: [`Command::PatchNotes`] for the progression.
    PatchChords {
        upsert: Vec<ChordEvent>,
        remove: Vec<Uuid>,
    },
    /// Place a pattern on the song (on its own track). `length_ticks` defaults to the pattern
    /// length. Refused at or past the song end; a clip crossing it is cut there.
    AddClip {
        pattern_id: Uuid,
        start_tick: i64,
        length_ticks: Option<i64>,
    },
    /// Move / resize / re-offset / mute a clip (matched by id). The clip keeps its pattern
    /// (a changed `pattern_id` is ignored) and is held inside the song like `AddClip`.
    UpdateClip {
        clip: Clip,
    },
    /// Several `UpdateClip`s as one undo step (a group move). All or nothing.
    UpdateClips {
        clips: Vec<Clip>,
    },
    RemoveClip {
        id: Uuid,
    },
    /// Remove several clips as one undo step. Unknown ids are skipped.
    RemoveClips {
        ids: Vec<Uuid>,
    },
    /// Fork the clip's pattern into a copy on the same track and point this one clip at it.
    MakeClipUnique {
        id: Uuid,
    },
    /// Move the song's hard end (at least one bar). Clips starting at or after it are
    /// removed; clips crossing it are cut.
    SetSongLength {
        ticks: i64,
    },
    /// The song's loop brace as `[start, end]` in song ticks, or `None` to clear it. It is an
    /// edit like any other so that setting the chorus loop is undoable and saved with the song.
    SetLoopRegion {
        region: Option<(i64, i64)>,
    },
    /// The punch range as `[in, out]` in song ticks, or `None` to disarm it. Undoable and
    /// saved with the song for the same reasons the loop brace is.
    SetPunchRegion {
        region: Option<(i64, i64)>,
    },
    AddSection {
        name: String,
        start_tick: i64,
    },
    UpdateSection {
        section: Section,
    },
    RemoveSection {
        id: Uuid,
    },
    SetMaster {
        master: MasterSettings,
    },
    SetTimeSignature {
        numerator: u32,
        denominator: u32,
    },
    /// Global swing in percent (50–75) and its grid in ticks (480 or 240).
    SetSwing {
        swing: f64,
        grid_ticks: i64,
    },
    /// Add one track per imported MIDI track, each with one `length_ticks` pattern holding its
    /// notes and one clip of it at tick 0, in one undo step. The song grows to fit.
    ImportTracks {
        tracks: Vec<ImportedTrack>,
        length_ticks: i64,
    },
    /// A tempo change on the song timeline (replaces any point already on that tick).
    AddTempoPoint {
        tick: i64,
        bpm: f64,
        ramp: bool,
    },
    UpdateTempoPoint {
        point: TempoPoint,
    },
    RemoveTempoPoint {
        id: Uuid,
    },
    /// A breakpoint on `target`'s lane, creating the lane if this is its first point and
    /// replacing any point already on that tick. Sent `transient` while drawing, so a freehand
    /// drag is one undo step like a knob's.
    AddAutomationPoint {
        target: AutoTarget,
        tick: i64,
        value: f64,
        curve: Curve,
    },
    /// Move / retune / re-curve a point (matched by id inside `lane`).
    UpdateAutomationPoint {
        lane: Uuid,
        point: Breakpoint,
    },
    RemoveAutomationPoint {
        lane: Uuid,
        id: Uuid,
    },
    /// Drop the whole lane for `target`: the parameter goes back to its static control.
    ClearAutomation {
        target: AutoTarget,
    },
    /// A track that holds audio and a guitar rig instead of notes.
    AddAudioTrack {
        name: String,
    },
    /// Replace an audio track's whole rig (one knob drag is one of these).
    SetTrackAmp {
        id: Uuid,
        amp: AmpRig,
    },
    /// One undo step for "a new recording appeared": the source and its first clip together,
    /// since a source with nothing pointing at it is invisible and a clip without its source
    /// is dropped on load.
    AddAudioTake {
        track_id: Uuid,
        source: AudioSource,
        clip: AudioClip,
    },
    /// A take that lands *inside* what was already there: the clips under it are split and
    /// crossfaded against it, and the whole repair is one undo step, because a punch the user
    /// has to undo three times is a punch that eats the original take.
    ///
    /// `crossfade_seconds` is the overlap at each seam: the head keeps playing that long past
    /// the take's start while fading out, the tail starts that long before the take's end
    /// while fading in, and the take fades in and out across both. Equal-power on both sides,
    /// so the join holds its level.
    PunchAudioTake {
        track_id: Uuid,
        source: AudioSource,
        clip: AudioClip,
        crossfade_seconds: f64,
    },
    /// Move / trim / fade / mute an audio clip (matched by id).
    UpdateAudioClip {
        clip: AudioClip,
    },
    RemoveAudioClip {
        id: Uuid,
    },
    /// Open `ticks` of empty time at song tick `at`: every clip, take, marker, tempo point,
    /// chord and automation breakpoint from `at` on moves that much later, in one undo step,
    /// and the song grows to match. See [`crate::ripple`] for what happens to a clip the cut
    /// falls inside.
    InsertTime {
        at: i64,
        ticks: i64,
    },
    /// Cut `[at, at + ticks)` out of the song and close the gap, in one undo step.
    DeleteTime {
        at: i64,
        ticks: i64,
    },
    /// Copy everything in `[from, to)` to `at`, pushing what was there along: verse 1 into
    /// verse 2 in one gesture.
    DuplicateRange {
        from: i64,
        to: i64,
        at: i64,
    },
    /// What DI analysis heard in one audio clip, landed as one undo step: a new synth track
    /// called `name` with the default sound, holding one `length_ticks` pattern of `notes`
    /// placed at `start_tick` (no track at all when `notes` is empty — every region was
    /// refused as polyphonic), and `detection` replacing whatever an earlier analysis of the
    /// same clip left in the detected lane. The song grows to fit, as an import does.
    AddTranscription {
        name: String,
        start_tick: i64,
        length_ticks: i64,
        notes: Vec<NoteEvent>,
        detection: ClipDetection,
    },
    /// Lay one detected chord onto the song's progression over its own span: whatever it lands
    /// on is trimmed back, removed or split, as a chord drawn in the lane would be. It leaves
    /// the detected lane in the same step.
    AcceptDetectedChord {
        id: Uuid,
    },
    /// Drop one detected chord without touching the progression.
    DismissDetectedChord {
        id: Uuid,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("no such track")]
    NoSuchTrack,
    #[error("no such pattern")]
    NoSuchPattern,
    #[error("no such sound")]
    NoSuchSound,
    #[error("a project needs at least one track")]
    LastTrack,
    #[error("that is past the end of the song")]
    PastSongEnd,
    #[error("invalid value")]
    InvalidValue,
    #[error("track has no plugin")]
    NoPluginOnTrack,
    #[error("no such clip")]
    NoSuchClip,
    #[error("no such section")]
    NoSuchSection,
    #[error("no such tempo point")]
    NoSuchTempoPoint,
    #[error("no such automation lane")]
    NoSuchAutomationLane,
    #[error("no such automation point")]
    NoSuchAutomationPoint,
    #[error("no such audio clip")]
    NoSuchAudioClip,
    #[error("no such audio source")]
    NoSuchAudioSource,
    #[error("not an audio track")]
    NotAnAudioTrack,
    #[error("no such insert")]
    NoSuchInsert,
    #[error("an insert chain holds at most {MAX_INSERTS} effects")]
    TooManyInserts,
    #[error("that effect has no parameters of its own")]
    NotANativeEffect,
    #[error("no such parameter")]
    NoSuchParam,
    #[error("no such note")]
    NoSuchNote,
    #[error("a note can't slide from itself, or from a note that slides from it")]
    RopeLoop,
    #[error("that note has no rope")]
    NoRope,
    #[error("no such detected chord")]
    NoSuchDetectedChord,
}

/// One undo step: the model before the edit, and what the edit was ("move notes").
#[derive(Debug, Clone)]
struct HistoryEntry {
    /// The edit as it was made: applying it reversed undoes it.
    patch: Patch,
    label: String,
}

/// A transient gesture in progress: the model when it began, to diff against when it ends.
#[derive(Debug, Clone)]
struct Gesture {
    before: ProjectModel,
    label: String,
}

/// What came of an undo or a redo.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// There was nothing to undo or redo.
    Nothing,
    /// The model changed by this patch.
    Applied(Patch),
    /// Somebody else's work got in the way, so nothing changed and the step is gone.
    Blocked,
}

#[derive(Debug, Clone)]
pub struct Document {
    model: ProjectModel,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// The transient gesture in progress, which becomes one undo step when it ends.
    pending: Option<Gesture>,
    pub path: Option<std::path::PathBuf>,
    /// The model as it was at the last save point; `is_dirty` is a comparison against it rather
    /// than a sticky flag, so undoing back to the saved state really is "no unsaved changes".
    saved: Option<ProjectModel>,
    /// Bumps on every change to the model (edits, undo, redo, replace). Autosave compares it.
    revision: u64,
}

const MAX_UNDO: usize = 200;

impl Document {
    pub fn new(model: ProjectModel) -> Self {
        Document {
            saved: Some(model.clone()),
            model,
            undo: vec![],
            redo: vec![],
            pending: None,
            path: None,
            revision: 0,
        }
    }

    pub fn model(&self) -> &ProjectModel {
        &self.model
    }

    /// Unsaved changes, measured against the last save point rather than "an edit happened" —
    /// undoing back to the saved state (or a gesture that returns to its starting value) leaves
    /// the document clean, so the quit prompt only fires when the file really differs.
    pub fn is_dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.model)
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.model.clone());
    }

    /// True only when an undo would actually change something: an open gesture that has not
    /// moved off its starting value is not undoable, and promising one lights a dead menu item.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.pending.as_ref().is_some_and(|p| p.before != self.model)
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What the next undo would revert ("move notes"), if anything.
    pub fn undo_label(&self) -> Option<&str> {
        self.pending
            .as_ref()
            .map(|e| e.label.as_str())
            .or_else(|| self.undo.last().map(|e| e.label.as_str()))
    }

    /// What the next redo would re-apply, if anything.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// Monotonic change counter; two equal revisions mean the model has not changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Apply a command. `transient` edits (slider drags) don't create their own undo step;
    /// the first one in a run snapshots the model and the next non-transient command (or
    /// [`Document::commit_gesture`]) turns that snapshot into a single undo entry.
    pub fn apply(&mut self, command: Command, transient: bool) -> Result<(), EditError> {
        self.apply_labeled(command, transient, None)
    }

    /// [`Document::apply`] with an explicit undo label. The UI knows *what* an edit was
    /// ("resize notes") better than the command does ("edit notes"); when it does not say,
    /// [`Command::label`] is used.
    pub fn apply_labeled(
        &mut self,
        command: Command,
        transient: bool,
        label: Option<String>,
    ) -> Result<(), EditError> {
        let label = label
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| command.label());
        let mut next = self.model.clone();
        apply_to(&mut next, command)?;
        if next == self.model {
            return Ok(());
        }
        if transient {
            if self.pending.is_none() {
                self.pending = Some(Gesture { before: self.model.clone(), label });
            }
        } else {
            self.commit_gesture();
            let patch = Patch::between(&self.model, &next);
            self.push_undo(HistoryEntry { patch, label });
        }
        self.redo.clear();
        self.model = next;
        self.touch();
        Ok(())
    }

    /// End a run of transient edits, making it one undo step.
    pub fn commit_gesture(&mut self) {
        if let Some(gesture) = self.pending.take() {
            if gesture.before != self.model {
                let patch = Patch::between(&gesture.before, &self.model);
                self.push_undo(HistoryEntry { patch, label: gesture.label });
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        matches!(self.undo_step(), StepOutcome::Applied(_))
    }

    pub fn redo(&mut self) -> bool {
        matches!(self.redo_step(), StepOutcome::Applied(_))
    }

    /// [`Document::undo`], saying what changed. Anything somebody else changed since the edit is
    /// left as they left it.
    pub fn undo_step(&mut self) -> StepOutcome {
        self.commit_gesture();
        let Some(entry) = self.undo.pop() else { return StepOutcome::Nothing };
        match self.settle_patch(&entry.patch.reversed()) {
            Some(change) => {
                self.redo.push(HistoryEntry { patch: change.reversed(), label: entry.label });
                StepOutcome::Applied(change)
            }
            None => StepOutcome::Blocked,
        }
    }

    /// [`Document::redo`], saying what changed.
    pub fn redo_step(&mut self) -> StepOutcome {
        self.commit_gesture();
        let Some(entry) = self.redo.pop() else { return StepOutcome::Nothing };
        match self.settle_patch(&entry.patch) {
            Some(change) => {
                self.undo.push(HistoryEntry { patch: change.clone(), label: entry.label });
                StepOutcome::Applied(change)
            }
            None => StepOutcome::Blocked,
        }
    }

    /// Apply as much of `patch` as still holds, returning the change actually made. `None` when
    /// there was nothing left to do or the patch was refused.
    fn settle_patch(&mut self, patch: &Patch) -> Option<Patch> {
        let usable = patch.applicable_to(&self.model).ok()?;
        let mut next = usable.apply(&self.model).ok()?;
        // Only a step cut short can leave anything to mend: a whole one puts back a model that
        // was already consistent.
        if &usable != patch {
            next.mend();
        }
        let change = Patch::between(&self.model, &next);
        if change.is_empty() {
            return None;
        }
        self.model = next;
        self.touch();
        Some(change)
    }

    /// Run a command for somebody else: the model changes but this document's history doesn't.
    /// Returns what changed.
    pub fn apply_external(&mut self, command: Command) -> Result<Patch, EditError> {
        let mut next = self.model.clone();
        apply_to(&mut next, command)?;
        let change = Patch::between(&self.model, &next);
        self.adopt(next, &change);
        Ok(change)
    }

    /// Apply somebody else's undo or redo: as much of `patch` as still holds, outside this
    /// document's history. Returns what changed, which may be nothing.
    pub fn apply_external_patch(&mut self, patch: &Patch) -> Patch {
        let Ok(usable) = patch.applicable_to(&self.model) else { return Patch::default() };
        let Ok(mut next) = usable.apply(&self.model) else { return Patch::default() };
        next.mend();
        let change = Patch::between(&self.model, &next);
        self.adopt(next, &change);
        change
    }

    /// Take a new model that differs by `change`, which is somebody else's doing.
    fn adopt(&mut self, next: ProjectModel, change: &Patch) {
        if change.is_empty() {
            return;
        }
        self.keep_gesture_current(change);
        self.model = next;
        self.touch();
    }

    /// Somebody else's `change` landed in the middle of a gesture. Move the gesture's starting
    /// point along with it, so ending the gesture records this person's edit and not theirs.
    pub fn keep_gesture_current(&mut self, change: &Patch) {
        if let Some(gesture) = &mut self.pending {
            if let Ok(before) = change.apply(&gesture.before) {
                gesture.before = before;
            }
        }
    }

    /// Show `model` in place of the current one, keeping history. A guest's view of a session is
    /// rebuilt this way whenever the host answers.
    pub fn set_view(&mut self, model: ProjectModel) {
        if model != self.model {
            self.model = model;
            self.touch();
        }
    }

    /// Replace the whole model (new / open). Clears history.
    pub fn replace(&mut self, model: ProjectModel, path: Option<std::path::PathBuf>) {
        self.saved = Some(model.clone());
        self.model = model;
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.path = path;
        self.revision += 1;
    }

    /// Replace the model but keep it marked dirty — used when restoring an autosave, which is
    /// by definition unsaved work. Dropping the save point is what makes it dirty: there is no
    /// file this model is known to match.
    pub fn restore(&mut self, model: ProjectModel, path: Option<std::path::PathBuf>) {
        self.replace(model, path);
        self.saved = None;
    }

    /// Note a change to the model. Dirtiness is derived from the save point, so this only has
    /// to move the revision counter autosave watches.
    fn touch(&mut self) {
        self.revision += 1;
    }

    fn push_undo(&mut self, entry: HistoryEntry) {
        self.undo.push(entry);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }
}

/// Insert or replace an embedded sound by id.
fn embed_sound(m: &mut ProjectModel, sound: Sound) {
    match m.sounds.iter_mut().find(|s| s.id == sound.id) {
        Some(slot) => *slot = sound,
        None => m.sounds.push(sound),
    }
}

/// A change to the song's timeline that moves whatever marks a place in the music. Guests apply
/// it to their own loop and punch ranges, which never travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TimeEdit {
    Insert { at: i64, ticks: i64 },
    Delete { at: i64, ticks: i64 },
}

impl TimeEdit {
    /// `region` after this edit, by the same rule the document applies to its own.
    pub fn shift(&self, region: Option<(i64, i64)>) -> Option<(i64, i64)> {
        match *self {
            TimeEdit::Insert { at, ticks } => crate::ripple::shift_region(region, at, ticks),
            TimeEdit::Delete { at, ticks } => crate::ripple::pull_region(region, at, ticks),
        }
    }
}

impl Command {
    /// Whether this edit belongs to one person in a shared session rather than to the project.
    /// See [`crate::patch::Personal`].
    pub fn is_personal(&self) -> bool {
        matches!(
            self,
            Command::SetTrackMuted { .. }
                | Command::SetTrackSoloed { .. }
                | Command::SetLoopRegion { .. }
                | Command::SetPunchRegion { .. }
        )
    }

    /// Whether this edit brings in audio that exists only on the machine that made it.
    pub fn adds_audio(&self) -> bool {
        matches!(self, Command::AddAudioTake { .. } | Command::PunchAudioTake { .. })
    }

    /// How this edit moves the timeline, if it does.
    pub fn time_edit(&self) -> Option<TimeEdit> {
        match *self {
            Command::InsertTime { at, ticks } => Some(TimeEdit::Insert { at, ticks }),
            Command::DeleteTime { at, ticks } => Some(TimeEdit::Delete { at, ticks }),
            Command::DuplicateRange { from, to, at } => Some(TimeEdit::Insert { at, ticks: to - from }),
            _ => None,
        }
    }

    /// A short, lower-case description for the Edit menu ("Undo set tempo").
    pub fn label(&self) -> String {
        use Command::*;
        match self {
            SetTempo { .. } => "set tempo",
            SetKey { .. } => "set key",
            AddTrack { is_drum: true } => "add drum track",
            AddTrack { .. } => "add track",
            DeleteTrack { .. } => "delete track",
            RenameTrack { .. } => "rename track",
            SetTrackMuted { muted: true, .. } => "mute track",
            SetTrackMuted { .. } => "unmute track",
            SetTrackSoloed { soloed: true, .. } => "solo track",
            SetTrackSoloed { .. } => "unsolo track",
            MoveTrack { .. } => "reorder tracks",
            SetTrackVoice { .. } | SetTrackSound { .. } => "change instrument",
            SetDrumPads { .. } => "edit drum kit",
            UpsertSound { .. } => "update sound",
            SetTrackParam { param, .. } => match param {
                TrackParam::Volume => "change volume",
                TrackParam::ReverbSend => "change reverb send",
                TrackParam::Tone => "change tone",
                TrackParam::Pan => "change pan",
            },
            SetTrackColor { .. } => "color track",
            SetTrackPlugin { .. } => "change plugin",
            SetPluginState { .. } => "edit plugin",
            // Only `AddInsert` carries the effect's name; for the rest the UI passes the name
            // it is showing through `apply_labeled`, the same as it does for note edits.
            AddInsert { name, .. } => {
                let name = name.trim();
                return if name.is_empty() {
                    "add insert".to_string()
                } else {
                    format!("add {name}")
                };
            }
            RemoveInsert { .. } => "remove insert",
            MoveInsert { .. } => "reorder insert",
            SetInsertEnabled { on: true, .. } => "enable insert",
            SetInsertEnabled { .. } => "bypass insert",
            SetInsertState { .. } => "insert settings",
            SetInsertParam { .. } => "insert settings",
            AddPattern { .. } => "add pattern",
            DuplicatePattern { .. } => "duplicate pattern",
            DeletePattern { .. } => "delete pattern",
            RenamePattern { .. } => "rename pattern",
            SetPatternLength { .. } => "change pattern length",
            SetNotes { .. } | PatchNotes { .. } => "edit notes",
            SetRope { .. } => "add rope",
            ClearRope { .. } => "remove rope",
            SetSlideTime { .. } => "change slide time",
            SetChords { .. } | PatchChords { .. } => "edit chords",
            AddClip { .. } => "add clip",
            UpdateClip { .. } => "edit clip",
            UpdateClips { .. } => "move clips",
            RemoveClip { .. } => "remove clip",
            RemoveClips { .. } => "remove clips",
            MakeClipUnique { .. } => "make clip unique",
            SetSongLength { .. } => "set song length",
            SetLoopRegion { region: Some(_) } => "set loop",
            SetLoopRegion { .. } => "clear loop",
            SetPunchRegion { region: Some(_) } => "set punch",
            SetPunchRegion { .. } => "clear punch",
            AddSection { .. } => "add section",
            UpdateSection { .. } => "edit section",
            RemoveSection { .. } => "remove section",
            SetMaster { .. } => "change master",
            SetTimeSignature { .. } => "set time signature",
            SetSwing { .. } => "set swing",
            ImportTracks { .. } => "import MIDI",
            AddTempoPoint { .. } => "add tempo change",
            UpdateTempoPoint { .. } => "edit tempo change",
            RemoveTempoPoint { .. } => "remove tempo change",
            AddAutomationPoint { .. } => "add automation point",
            UpdateAutomationPoint { .. } => "move automation point",
            RemoveAutomationPoint { .. } => "remove automation point",
            ClearAutomation { .. } => "clear automation",
            AddAudioTrack { .. } => "add audio track",
            SetTrackAmp { .. } => "edit rig",
            AddAudioTake { .. } => "add audio",
            PunchAudioTake { .. } => "punch in",
            UpdateAudioClip { .. } => "edit audio clip",
            RemoveAudioClip { .. } => "remove audio clip",
            InsertTime { .. } => "insert time",
            DeleteTime { .. } => "delete time",
            DuplicateRange { .. } => "duplicate range",
            AddTranscription { .. } => "detect notes",
            AcceptDetectedChord { .. } => "accept chord",
            DismissDetectedChord { .. } => "dismiss chord",
        }
        .to_string()
    }
}

/// The next free "<track name> <n>" for a new pattern on that track.
fn next_pattern_name(m: &ProjectModel, track: &Track) -> String {
    let mut n = m.patterns_of(&track.id).count() + 1;
    loop {
        let name = format!("{} {n}", track.name);
        if !m.patterns_of(&track.id).any(|p| p.name == name) {
            return name;
        }
        n += 1;
    }
}

/// Copy a pattern in place, right after the original, and answer the copy's id.
fn duplicate_pattern(m: &mut ProjectModel, id: &Uuid) -> Result<Uuid, EditError> {
    let i = m
        .patterns
        .iter()
        .position(|p| &p.id == id)
        .ok_or(EditError::NoSuchPattern)?;
    let copy = m.patterns[i].duplicated();
    let copy_id = copy.id;
    m.patterns.insert(i + 1, copy);
    Ok(copy_id)
}

/// Write one clip back, keeping its pattern and holding it inside the song.
fn update_clip(m: &mut ProjectModel, clip: Clip) -> Result<(), EditError> {
    let end = m.song_length_ticks;
    let slot = m
        .clips
        .iter_mut()
        .find(|c| c.id == clip.id)
        .ok_or(EditError::NoSuchClip)?;
    if clip.start_tick < 0 || clip.length_ticks <= 0 || clip.offset_ticks < 0 {
        return Err(EditError::InvalidValue);
    }
    if clip.start_tick >= end {
        return Err(EditError::PastSongEnd);
    }
    *slot = Clip {
        pattern_id: slot.pattern_id,
        length_ticks: clip.length_ticks.min(end - clip.start_tick),
        ..clip
    };
    Ok(())
}

/// Shape-only base64 check: the standard alphabet, padded to a multiple of four with at most
/// two trailing `=`. Core never decodes a plugin's state — it has no business knowing what the
/// bytes mean — but a blob that cannot possibly be base64 is a bug at the edit, not at load,
/// and refusing it here keeps unreadable garbage out of saved projects.
fn is_base64(s: &str) -> bool {
    if !s.len().is_multiple_of(4) {
        return false;
    }
    let body = s.trim_end_matches('=');
    if s.len() - body.len() > 2 {
        return false;
    }
    body.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// The effect chain an [`InsertTarget`] names. The master's chain always exists; a track's
/// only exists while the track does.
fn chain_mut(m: &mut ProjectModel, target: InsertTarget) -> Result<&mut Vec<Insert>, EditError> {
    match target {
        InsertTarget::Track(id) => Ok(&mut m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.inserts),
        InsertTarget::Master => Ok(&mut m.master_inserts),
    }
}

/// Reject a lane whose target names a track that is not in the project. Insert and plugin
/// parameter ids are not checked: a plugin can be missing on this machine and still be the
/// right thing to automate on the machine that has it, so those lanes survive like the insert
/// entries themselves do.
fn check_auto_target(m: &ProjectModel, target: AutoTarget) -> Result<(), EditError> {
    match target.track_id() {
        Some(id) if m.track(&id).is_none() => Err(EditError::NoSuchTrack),
        _ => Ok(()),
    }
}

fn note_mut<'a>(p: &'a mut SongPattern, id: &Uuid) -> Result<&'a mut NoteEvent, EditError> {
    p.notes.iter_mut().find(|n| &n.id == id).ok_or(EditError::NoSuchNote)
}

fn insert_mut(
    m: &mut ProjectModel,
    target: InsertTarget,
    insert_id: Uuid,
) -> Result<&mut Insert, EditError> {
    chain_mut(m, target)?
        .iter_mut()
        .find(|i| i.id == insert_id)
        .ok_or(EditError::NoSuchInsert)
}

/// Run a command on a bare model, outside any history. A guest replays what the host hasn't
/// answered yet this way.
pub fn apply_command(m: &mut ProjectModel, command: Command) -> Result<(), EditError> {
    apply_to(m, command)
}

fn apply_to(m: &mut ProjectModel, command: Command) -> Result<(), EditError> {
    use Command::*;
    match command {
        SetTempo { bpm } => {
            if !bpm.is_finite() {
                return Err(EditError::InvalidValue);
            }
            m.tempo = bpm.clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM);
        }
        SetKey { key } => m.key = key,
        AddTrack { is_drum } => {
            let n = m.tracks.len() + 1;
            let track = if is_drum {
                Track::drums("drums")
            } else {
                Track::new(format!("track {n}"))
            };
            let pattern = SongPattern::new(track.id, next_pattern_name(m, &track));
            m.tracks.push(track);
            m.patterns.push(pattern);
        }
        DeleteTrack { id } => {
            if m.track(&id).is_none() {
                return Err(EditError::NoSuchTrack);
            }
            if m.tracks.len() <= 1 {
                return Err(EditError::LastTrack);
            }
            m.tracks.retain(|t| t.id != id);
            let gone: Vec<Uuid> = m.patterns_of(&id).map(|p| p.id).collect();
            m.patterns.retain(|p| p.track_id != id);
            m.clips.retain(|c| !gone.contains(&c.pattern_id));
            m.audio_clips.retain(|c| c.track_id != id);
            m.automation.retain(|l| l.target.track_id() != Some(id));
            m.prune_sounds();
        }
        RenameTrack { id, name } => {
            // Matches RenamePattern: a blank name makes the track unidentifiable everywhere.
            if name.trim().is_empty() {
                return Err(EditError::InvalidValue);
            }
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.name = name;
        }
        SetTrackMuted { id, muted } => {
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.muted = muted
        }
        SetTrackSoloed { id, soloed } => {
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.soloed = soloed
        }
        MoveTrack { id, up } => {
            let i = m
                .tracks
                .iter()
                .position(|t| t.id == id)
                .ok_or(EditError::NoSuchTrack)?;
            let j = if up { i.checked_sub(1) } else { Some(i + 1) };
            if let Some(j) = j.filter(|&j| j < m.tracks.len()) {
                m.tracks.swap(i, j);
            }
        }
        SetTrackVoice { id, voice } => {
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.voice = voice;
            m.prune_sounds();
        }
        SetTrackSound { id, sound } => {
            let sound = sound.sanitized();
            let t = m.track_mut(&id).ok_or(EditError::NoSuchTrack)?;
            if t.is_drum || sound.is_drum() {
                return Err(EditError::InvalidValue);
            }
            t.voice = sound.id.clone();
            if !is_factory(&sound.id) {
                embed_sound(m, sound);
            }
            m.prune_sounds();
        }
        SetDrumPads { id, pads, sounds } => {
            if !valid_kit(&pads) || sounds.iter().any(|s| !s.is_drum()) {
                return Err(EditError::InvalidValue);
            }
            if !m.track(&id).ok_or(EditError::NoSuchTrack)?.is_drum {
                return Err(EditError::InvalidValue);
            }
            for s in sounds.into_iter().filter(|s| !is_factory(&s.id)) {
                embed_sound(m, s.sanitized());
            }
            if pads.iter().any(|p| !m.sound(&p.sound).is_some_and(Sound::is_drum)) {
                return Err(EditError::NoSuchSound);
            }
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.drum_pads = pads;
            m.prune_sounds();
        }
        UpsertSound { sound } => {
            let sound = sound.sanitized();
            let slot = m.sounds.iter_mut().find(|s| s.id == sound.id).ok_or(EditError::NoSuchSound)?;
            if slot.is_drum() != sound.is_drum() {
                return Err(EditError::InvalidValue);
            }
            *slot = sound;
        }
        SetTrackParam { id, param, value } => {
            if !value.is_finite() {
                return Err(EditError::InvalidValue);
            }
            let t = m.track_mut(&id).ok_or(EditError::NoSuchTrack)?;
            let (lo, hi) = crate::automation::track_param_range(param);
            let value = value.clamp(lo, hi);
            match param {
                TrackParam::Volume => t.volume = value,
                TrackParam::ReverbSend => t.reverb_send = value,
                TrackParam::Tone => t.tone = value,
                TrackParam::Pan => t.pan = value,
            }
        }
        SetTrackColor { id, color } => {
            if let Some(c) = &color {
                let ok = c.len() == 7
                    && c.starts_with('#')
                    && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
                if !ok {
                    return Err(EditError::InvalidValue);
                }
            }
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.color = color.map(|c| c.to_lowercase());
        }
        SetTrackPlugin { id, plugin } => {
            if let Some(p) = &plugin {
                // A blank uid names nothing: the track would claim to be plugin-driven and
                // silently fall back to its voice on every load.
                if p.uid.trim().is_empty() {
                    return Err(EditError::InvalidValue);
                }
                if p.state.as_deref().is_some_and(|s| !is_base64(s)) {
                    return Err(EditError::InvalidValue);
                }
            }
            let t = m.track_mut(&id).ok_or(EditError::NoSuchTrack)?;
            let previous = t.plugin.as_ref().map(|p| p.uid.clone());
            t.plugin = plugin.map(|mut p| {
                // A state blob only means anything to the plugin that wrote it; carrying it
                // across a swap feeds one plugin another's bytes.
                if previous.as_deref() != Some(p.uid.as_str()) {
                    p.state = None;
                }
                p
            });
        }
        SetPluginState { id, state } => {
            if !is_base64(&state) {
                return Err(EditError::InvalidValue);
            }
            let t = m.track_mut(&id).ok_or(EditError::NoSuchTrack)?;
            // State with no plugin to own it would be resurrected onto whatever plugin is
            // loaded next, which is exactly the mismatch the uid check above prevents.
            t.plugin.as_mut().ok_or(EditError::NoPluginOnTrack)?.state = Some(state);
        }
        AddInsert {
            target,
            uid,
            name,
            index,
        } => {
            // A blank uid names nothing: the chain would carry a slot that can never load.
            if uid.trim().is_empty() {
                return Err(EditError::InvalidValue);
            }
            let chain = chain_mut(m, target)?;
            if chain.len() >= MAX_INSERTS {
                return Err(EditError::TooManyInserts);
            }
            let at = index.unwrap_or(chain.len()).min(chain.len());
            chain.insert(at, Insert::new(uid, name));
        }
        RemoveInsert { target, insert_id } => {
            let chain = chain_mut(m, target)?;
            let before = chain.len();
            chain.retain(|i| i.id != insert_id);
            if chain.len() == before {
                return Err(EditError::NoSuchInsert);
            }
        }
        MoveInsert {
            target,
            insert_id,
            to_index,
        } => {
            let chain = chain_mut(m, target)?;
            let from = chain
                .iter()
                .position(|i| i.id == insert_id)
                .ok_or(EditError::NoSuchInsert)?;
            // Dragging past either end lands on that end rather than failing: the UI's drop
            // targets and the chain's bounds do not have to agree.
            let to = to_index.min(chain.len() - 1);
            let slot = chain.remove(from);
            chain.insert(to, slot);
        }
        SetInsertEnabled {
            target,
            insert_id,
            on,
        } => {
            insert_mut(m, target, insert_id)?.enabled = on;
        }
        SetInsertState {
            target,
            insert_id,
            state,
        } => {
            if !is_base64(&state) {
                return Err(EditError::InvalidValue);
            }
            insert_mut(m, target, insert_id)?.state = Some(state);
        }
        SetInsertParam {
            target,
            insert_id,
            param_id,
            value,
        } => {
            let insert = insert_mut(m, target, insert_id)?;
            let kind = insert.native_kind().ok_or(EditError::NotANativeEffect)?;
            let param = kind.param(param_id).ok_or(EditError::NoSuchParam)?;
            if !value.is_finite() {
                return Err(EditError::InvalidValue);
            }
            insert.params.insert(param_id, param.clamp(value));
        }
        AddPattern { track_id } => {
            let track = m.track(&track_id).ok_or(EditError::NoSuchTrack)?;
            let pattern = SongPattern::new(track_id, next_pattern_name(m, track));
            // Right after the track's last pattern, so the tree's order is creation order.
            let at = m
                .patterns
                .iter()
                .rposition(|p| p.track_id == track_id)
                .map_or(m.patterns.len(), |i| i + 1);
            m.patterns.insert(at, pattern);
        }
        DuplicatePattern { id } => {
            duplicate_pattern(m, &id)?;
        }
        DeletePattern { id } => {
            let before = m.patterns.len();
            m.patterns.retain(|p| p.id != id);
            if m.patterns.len() == before {
                return Err(EditError::NoSuchPattern);
            }
            m.clips.retain(|c| c.pattern_id != id);
        }
        RenamePattern { id, name } => {
            if name.trim().is_empty() {
                return Err(EditError::InvalidValue);
            }
            m.pattern_mut(&id).ok_or(EditError::NoSuchPattern)?.name = name;
        }
        SetPatternLength { id, ticks } => {
            if ticks < TICKS_PER_BEAT {
                return Err(EditError::InvalidValue);
            }
            m.pattern_mut(&id)
                .ok_or(EditError::NoSuchPattern)?
                .length_ticks = ticks;
        }
        SetNotes { pattern_id, notes } => {
            let p = m.pattern_mut(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let mut notes: Vec<NoteEvent> = notes.into_iter().map(NoteEvent::sanitized).collect();
            notes.sort_by(|a, b| a.start_tick.cmp(&b.start_tick).then(a.pitch.cmp(&b.pitch)));
            // The roll deletes a note by committing the array without it; this is what cuts the
            // ropes that pointed at it. Ropes the UI copied wrongly are caught here too.
            crate::model::tidy_ropes(&mut notes);
            p.notes = notes;
        }
        PatchNotes { pattern_id, upsert, remove } => {
            let p = m.pattern_mut(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let replaced: Vec<Uuid> = upsert.iter().map(|n| n.id).collect();
            let mut notes = std::mem::take(&mut p.notes);
            notes.retain(|n| !remove.contains(&n.id) && !replaced.contains(&n.id));
            notes.extend(upsert.into_iter().map(NoteEvent::sanitized));
            notes.sort_by(|a, b| a.start_tick.cmp(&b.start_tick).then(a.pitch.cmp(&b.pitch)));
            crate::model::tidy_ropes(&mut notes);
            p.notes = notes;
        }
        SetRope {
            pattern_id,
            note_id,
            source_id,
        } => {
            let p = m.pattern_mut(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            if !p.notes.iter().any(|n| n.id == source_id) {
                return Err(EditError::NoSuchNote);
            }
            // Walk upstream from the source: meeting the target on the way means the new rope
            // would close a loop. Commands keep a pattern loop-free, but the walk is bounded
            // anyway so a model built by hand can't hang it.
            let mut from = Some(source_id);
            for _ in 0..=p.notes.len() {
                let Some(id) = from else { break };
                if id == note_id {
                    return Err(EditError::RopeLoop);
                }
                from = p.notes.iter().find(|n| n.id == id).and_then(|n| n.slide_from);
            }
            note_mut(p, &note_id)?.slide_from = Some(source_id);
        }
        ClearRope {
            pattern_id,
            note_id,
        } => {
            let p = m.pattern_mut(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let n = note_mut(p, &note_id)?;
            n.slide_from = None;
            n.slide_secs = None;
        }
        SetSlideTime {
            pattern_id,
            note_id,
            secs,
        } => {
            if secs.is_some_and(|s| !s.is_finite()) {
                return Err(EditError::InvalidValue);
            }
            let p = m.pattern_mut(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let n = note_mut(p, &note_id)?;
            // A time with no rope to time would sit in the file meaning nothing.
            if n.slide_from.is_none() {
                return Err(EditError::NoRope);
            }
            n.slide_secs = secs.map(|s| s.clamp(0.0, crate::model::MAX_SLIDE_SECS));
        }
        SetChords { chords } => {
            m.chords = crate::chord_track::ChordTrack::new(crate::model::tidy_chords(chords))
        }
        PatchChords { upsert, remove } => {
            let replaced: Vec<Uuid> = upsert.iter().map(|c| c.id).collect();
            let mut chords = m.chords.take();
            chords.retain(|c| !remove.contains(&c.id) && !replaced.contains(&c.id));
            chords.extend(upsert);
            m.chords = crate::chord_track::ChordTrack::new(crate::model::tidy_chords(chords));
        }
        AddClip {
            pattern_id,
            start_tick,
            length_ticks,
        } => {
            let p = m.pattern(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let len = length_ticks.unwrap_or(p.length_ticks);
            if start_tick < 0 || len <= 0 {
                return Err(EditError::InvalidValue);
            }
            if start_tick >= m.song_length_ticks {
                return Err(EditError::PastSongEnd);
            }
            let len = len.min(m.song_length_ticks - start_tick);
            m.clips.push(Clip::new(pattern_id, start_tick, len));
            m.clips.sort_by_key(|c| c.start_tick);
        }
        UpdateClip { clip } => {
            update_clip(m, clip)?;
            m.clips.sort_by_key(|c| c.start_tick);
        }
        UpdateClips { clips } => {
            for clip in clips {
                update_clip(m, clip)?;
            }
            m.clips.sort_by_key(|c| c.start_tick);
        }
        RemoveClips { ids } => m.clips.retain(|c| !ids.contains(&c.id)),
        MakeClipUnique { id } => {
            let pattern_id = m.clip(&id).ok_or(EditError::NoSuchClip)?.pattern_id;
            let copy = duplicate_pattern(m, &pattern_id)?;
            if let Some(c) = m.clips.iter_mut().find(|c| c.id == id) {
                c.pattern_id = copy;
            }
        }
        SetSongLength { ticks } => {
            m.song_length_ticks = ticks.max(m.bar_ticks());
            m.fit_clips_to_song();
        }
        SetLoopRegion { region } => {
            m.loop_region = match region {
                None => None,
                Some((start, end)) => {
                    // An inverted or empty brace is a bug in the caller, not a way to clear:
                    // clearing says so with `None`.
                    if start < 0 || end <= start {
                        return Err(EditError::InvalidValue);
                    }
                    Some(
                        crate::model::clamp_loop(start, end, m.song_length_ticks)
                            .ok_or(EditError::InvalidValue)?,
                    )
                }
            };
        }
        SetPunchRegion { region } => {
            m.punch_region = match region {
                None => None,
                Some((start, end)) => {
                    // Same rule as the loop brace: an inverted or empty range is a caller bug,
                    // and disarming says so with `None`.
                    if start < 0 || end <= start {
                        return Err(EditError::InvalidValue);
                    }
                    Some(
                        crate::model::clamp_loop(start, end, m.song_length_ticks)
                            .ok_or(EditError::InvalidValue)?,
                    )
                }
            };
        }
        RemoveClip { id } => {
            let before = m.clips.len();
            m.clips.retain(|c| c.id != id);
            if m.clips.len() == before {
                return Err(EditError::NoSuchClip);
            }
        }
        AddSection { name, start_tick } => {
            if start_tick < 0 {
                return Err(EditError::InvalidValue);
            }
            let name = if name.trim().is_empty() {
                format!("section {}", m.sections.len() + 1)
            } else {
                name
            };
            m.sections.push(Section::new(name, start_tick));
            m.sections.sort_by_key(|s| s.start_tick);
        }
        UpdateSection { section } => {
            // A negative extent is nonsense; 0 is the implicit one (to the next marker).
            if section.start_tick < 0 || section.length_ticks < 0 || section.name.trim().is_empty() {
                return Err(EditError::InvalidValue);
            }
            let slot = m
                .sections
                .iter_mut()
                .find(|s| s.id == section.id)
                .ok_or(EditError::NoSuchSection)?;
            *slot = section;
            m.sections.sort_by_key(|s| s.start_tick);
        }
        RemoveSection { id } => {
            let before = m.sections.len();
            m.sections.retain(|s| s.id != id);
            if m.sections.len() == before {
                return Err(EditError::NoSuchSection);
            }
        }
        SetTimeSignature {
            numerator,
            denominator,
        } => {
            if !(1..=16).contains(&numerator) || ![2, 4, 8, 16].contains(&denominator) {
                return Err(EditError::InvalidValue);
            }
            m.time_signature = crate::model::TimeSignature {
                numerator,
                denominator,
            };
            // The song stays at least one (new) bar long.
            m.song_length_ticks = m.song_length_ticks.max(m.bar_ticks());
        }
        ImportTracks {
            tracks,
            length_ticks,
        } => {
            if tracks.is_empty() || length_ticks < TICKS_PER_BEAT {
                return Err(EditError::InvalidValue);
            }
            for t in tracks {
                let mut track = if t.is_drum {
                    Track::drums(&t.name)
                } else {
                    Track::new(&t.name)
                };
                if track.name.trim().is_empty() {
                    track.name = format!("track {}", m.tracks.len() + 1);
                }
                let mut pattern = SongPattern::new(track.id, next_pattern_name(m, &track));
                pattern.length_ticks = length_ticks;
                pattern.notes = t.notes.into_iter().map(NoteEvent::sanitized).collect();
                pattern
                    .notes
                    .sort_by(|a, b| a.start_tick.cmp(&b.start_tick).then(a.pitch.cmp(&b.pitch)));
                crate::model::tidy_ropes(&mut pattern.notes);
                m.clips.push(Clip::new(pattern.id, 0, length_ticks));
                m.tracks.push(track);
                m.patterns.push(pattern);
            }
            m.song_length_ticks = m.song_length_ticks.max(length_ticks);
            m.clips.sort_by_key(|c| c.start_tick);
        }
        AddTempoPoint { tick, bpm, ramp } => {
            if tick < 0 || !bpm.is_finite() {
                return Err(EditError::InvalidValue);
            }
            let bpm = bpm.clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM);
            m.tempo_points.retain(|p| p.tick != tick);
            m.tempo_points.push(TempoPoint::new(tick, bpm, ramp));
            m.tempo_points.sort_by_key(|p| p.tick);
        }
        UpdateTempoPoint { point } => {
            if point.tick < 0 || !point.bpm.is_finite() {
                return Err(EditError::InvalidValue);
            }
            let slot = m
                .tempo_points
                .iter_mut()
                .find(|p| p.id == point.id)
                .ok_or(EditError::NoSuchTempoPoint)?;
            *slot = TempoPoint {
                bpm: point
                    .bpm
                    .clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM),
                ..point
            };
            // Keep one point per tick: the edited one wins.
            let (id, tick) = (slot.id, slot.tick);
            m.tempo_points.retain(|p| p.id == id || p.tick != tick);
            m.tempo_points.sort_by_key(|p| p.tick);
        }
        RemoveTempoPoint { id } => {
            let before = m.tempo_points.len();
            m.tempo_points.retain(|p| p.id != id);
            if m.tempo_points.len() == before {
                return Err(EditError::NoSuchTempoPoint);
            }
        }
        AddAutomationPoint {
            target,
            tick,
            value,
            curve,
        } => {
            if tick < 0 || !value.is_finite() {
                return Err(EditError::InvalidValue);
            }
            check_auto_target(m, target)?;
            let lane = match m.automation.iter_mut().position(|l| l.target == target) {
                Some(i) => &mut m.automation[i],
                None => {
                    m.automation.push(AutomationLane::new(target));
                    m.automation.last_mut().expect("just pushed")
                }
            };
            lane.points.retain(|p| p.tick != tick);
            lane.points
                .push(Breakpoint::new(tick, target.clamp(value), curve));
            lane.tidy();
        }
        UpdateAutomationPoint { lane, point } => {
            if point.tick < 0 || !point.value.is_finite() {
                return Err(EditError::InvalidValue);
            }
            let lane = m
                .automation
                .iter_mut()
                .find(|l| l.id == lane)
                .ok_or(EditError::NoSuchAutomationLane)?;
            let slot = lane
                .points
                .iter_mut()
                .find(|p| p.id == point.id)
                .ok_or(EditError::NoSuchAutomationPoint)?;
            let id = slot.id;
            *slot = Breakpoint { id, ..point };
            // Keep one point per tick: dragging a point onto a neighbour swallows it, exactly
            // as the tempo lane does, so a drag can never leave two values on one tick.
            let tick = slot.tick;
            lane.points.retain(|p| p.id == id || p.tick != tick);
            lane.tidy();
        }
        RemoveAutomationPoint { lane, id } => {
            let i = m
                .automation
                .iter()
                .position(|l| l.id == lane)
                .ok_or(EditError::NoSuchAutomationLane)?;
            let before = m.automation[i].points.len();
            m.automation[i].points.retain(|p| p.id != id);
            if m.automation[i].points.len() == before {
                return Err(EditError::NoSuchAutomationPoint);
            }
            // An empty lane is no automation at all; leaving it behind would make the file
            // grow and the sequence carry a curve with nothing to say.
            if m.automation[i].points.is_empty() {
                m.automation.remove(i);
            }
        }
        ClearAutomation { target } => {
            let before = m.automation.len();
            m.automation.retain(|l| l.target != target);
            if m.automation.len() == before {
                return Err(EditError::NoSuchAutomationLane);
            }
        }
        SetSwing { swing, grid_ticks } => {
            if !swing.is_finite()
                || !(grid_ticks == TICKS_PER_BEAT / 2 || grid_ticks == TICKS_PER_BEAT / 4)
            {
                return Err(EditError::InvalidValue);
            }
            m.swing = swing.clamp(50.0, 75.0);
            m.swing_grid_ticks = grid_ticks;
        }
        SetMaster { master } => {
            let v = [
                master.gain,
                master.reverb_wet,
                master.low_cut_hz,
                master.high_cut_hz,
                master.low_eq,
                master.mid_eq,
                master.high_eq,
                master.ceiling_db,
            ];
            if v.iter().any(|x| !x.is_finite()) {
                return Err(EditError::InvalidValue);
            }
            m.master = MasterSettings {
                gain: master.gain.clamp(0.0, 1.5),
                reverb_wet: master.reverb_wet.clamp(0.0, 1.0),
                low_cut_hz: master.low_cut_hz.clamp(10.0, 2_000.0),
                high_cut_hz: master.high_cut_hz.clamp(500.0, 20_000.0),
                low_eq: master.low_eq.clamp(0.0, 2.0),
                mid_eq: master.mid_eq.clamp(0.0, 2.0),
                high_eq: master.high_eq.clamp(0.0, 2.0),
                // Above 0 dBFS the limiter would be asked to hold a ceiling the device cannot
                // pass; -12 is as far down as the control is useful.
                ceiling_db: master.ceiling_db.clamp(-12.0, 0.0),
                limiter: master.limiter,
            };
        }
        AddAudioTrack { name } => {
            let name = if name.trim().is_empty() {
                "audio".to_string()
            } else {
                name
            };
            m.tracks.push(Track::audio(name));
        }
        SetTrackAmp { id, amp } => {
            let amp = amp.sanitized().ok_or(EditError::InvalidValue)?;
            let t = m.track_mut(&id).ok_or(EditError::NoSuchTrack)?;
            if !t.is_audio {
                return Err(EditError::NotAnAudioTrack);
            }
            t.amp = Some(amp);
        }
        AddAudioTake {
            track_id,
            source,
            clip,
        } => {
            // The clip must name the track and source it arrived with: a take that points
            // somewhere else would be dropped by `normalized` and silently vanish.
            if clip.track_id != track_id || clip.source_id != source.id {
                return Err(EditError::InvalidValue);
            }
            if source.sample_rate == 0 || source.channels == 0 || source.frames == 0 {
                return Err(EditError::InvalidValue);
            }
            let t = m.track(&track_id).ok_or(EditError::NoSuchTrack)?;
            if !t.is_audio {
                return Err(EditError::NotAnAudioTrack);
            }
            let clip = clip.sanitized().ok_or(EditError::InvalidValue)?;
            match m.audio_sources.iter_mut().find(|s| s.id == source.id) {
                Some(slot) => *slot = source,
                None => m.audio_sources.push(source),
            }
            m.audio_clips.push(clip);
            m.audio_clips.sort_by_key(|c| c.start_tick);
        }
        PunchAudioTake {
            track_id,
            source,
            clip,
            crossfade_seconds,
        } => {
            if clip.track_id != track_id || clip.source_id != source.id {
                return Err(EditError::InvalidValue);
            }
            if source.sample_rate == 0 || source.channels == 0 || source.frames == 0 {
                return Err(EditError::InvalidValue);
            }
            let t = m.track(&track_id).ok_or(EditError::NoSuchTrack)?;
            if !t.is_audio {
                return Err(EditError::NotAnAudioTrack);
            }
            let mut take = clip.sanitized().ok_or(EditError::InvalidValue)?;
            // A crossfade longer than half the take would overlap itself; a nonsense one falls
            // back to the default rather than refusing a take the player has already performed.
            let xf = if crossfade_seconds.is_finite() && crossfade_seconds >= 0.0 {
                crossfade_seconds
            } else {
                DEFAULT_PUNCH_CROSSFADE_SECONDS
            }
            .min(MAX_FADE_SECONDS)
            .min(take.length_seconds * 0.5);
            // The punch owns both of the take's edges: whatever fades it arrived with are the
            // seams, not a shape the user chose.
            take.fade_in_seconds = xf;
            take.fade_out_seconds = xf;
            let map = m.tempo_map();
            m.audio_clips = punch_split(&map, std::mem::take(&mut m.audio_clips), &take, xf);
            match m.audio_sources.iter_mut().find(|s| s.id == source.id) {
                Some(slot) => *slot = source,
                None => m.audio_sources.push(source),
            }
            m.audio_clips.push(take);
            m.audio_clips.sort_by_key(|c| c.start_tick);
        }
        UpdateAudioClip { clip } => {
            if m.audio_source(&clip.source_id).is_none() {
                return Err(EditError::NoSuchAudioSource);
            }
            if !m.track(&clip.track_id).is_some_and(|t| t.is_audio) {
                return Err(EditError::NotAnAudioTrack);
            }
            let clip = clip.sanitized().ok_or(EditError::InvalidValue)?;
            let slot = m
                .audio_clips
                .iter_mut()
                .find(|c| c.id == clip.id)
                .ok_or(EditError::NoSuchAudioClip)?;
            *slot = clip;
            m.audio_clips.sort_by_key(|c| c.start_tick);
        }
        RemoveAudioClip { id } => {
            let before = m.audio_clips.len();
            m.audio_clips.retain(|c| c.id != id);
            if m.audio_clips.len() == before {
                return Err(EditError::NoSuchAudioClip);
            }
        }
        InsertTime { at, ticks } => {
            if at < 0 || ticks <= 0 {
                return Err(EditError::InvalidValue);
            }
            crate::ripple::insert_time(m, at, ticks);
        }
        DeleteTime { at, ticks } => {
            if at < 0 || ticks <= 0 {
                return Err(EditError::InvalidValue);
            }
            crate::ripple::delete_time(m, at, ticks);
        }
        DuplicateRange { from, to, at } => {
            if from < 0 || to <= from || at < 0 {
                return Err(EditError::InvalidValue);
            }
            crate::ripple::duplicate_range(m, from, to, at);
        }
        AddTranscription {
            name,
            start_tick,
            length_ticks,
            notes,
            detection,
        } => {
            if start_tick < 0 || length_ticks <= 0 {
                return Err(EditError::InvalidValue);
            }
            // Results for a take that is not on the song would be dropped on the next load.
            if !m.audio_clips.iter().any(|c| c.id == detection.clip_id) {
                return Err(EditError::NoSuchAudioClip);
            }
            if !notes.is_empty() {
                let name = name.trim();
                let track = Track::new(if name.is_empty() { "notes" } else { name });
                let length = length_ticks.max(TICKS_PER_BEAT);
                let mut pattern = SongPattern::new(track.id, next_pattern_name(m, &track));
                pattern.length_ticks = length;
                // Inside the pattern or not at all: a note past its end would never sound.
                pattern.notes = notes
                    .into_iter()
                    .map(NoteEvent::sanitized)
                    .filter(|n| n.start_tick < length)
                    .map(|mut n| {
                        n.length_ticks = n.length_ticks.min(length - n.start_tick);
                        n
                    })
                    .collect();
                pattern
                    .notes
                    .sort_by(|a, b| a.start_tick.cmp(&b.start_tick).then(a.pitch.cmp(&b.pitch)));
                m.clips.push(Clip::new(pattern.id, start_tick, length));
                m.clips.sort_by_key(|c| c.start_tick);
                m.song_length_ticks = m.song_length_ticks.max(start_tick + length);
                m.tracks.push(track);
                m.patterns.push(pattern);
            }
            let detection = detection.sanitized();
            m.detections.retain(|d| d.clip_id != detection.clip_id);
            if !detection.chords.is_empty() || detection.chroma.iter().any(|&c| c > 0.0) {
                m.detections.push(detection);
            }
        }
        AcceptDetectedChord { id } => {
            let chord = take_detected_chord(m, id)?;
            let end = m.song_length_ticks;
            if chord.start_tick >= end {
                return Err(EditError::PastSongEnd);
            }
            let mut chord = chord.to_chord();
            chord.length_ticks = chord.length_ticks.min(end - chord.start_tick);
            let placed = place_chord(m.chords.take(), chord);
            m.chords = crate::chord_track::ChordTrack::new(crate::model::tidy_chords(placed));
        }
        DismissDetectedChord { id } => {
            take_detected_chord(m, id)?;
        }
    }
    Ok(())
}

/// Take one chord out of the detected lane by id. A detection left with no chords and no
/// chroma has nothing more to say and goes with it.
fn take_detected_chord(
    m: &mut ProjectModel,
    id: Uuid,
) -> Result<crate::model::DetectedChord, EditError> {
    let (d, i) = m
        .detections
        .iter()
        .enumerate()
        .find_map(|(d, det)| det.chords.iter().position(|c| c.id == id).map(|i| (d, i)))
        .ok_or(EditError::NoSuchDetectedChord)?;
    let chord = m.detections[d].chords.remove(i);
    let det = &m.detections[d];
    if det.chords.is_empty() && det.chroma.iter().all(|&c| c <= 0.0) {
        m.detections.remove(d);
    }
    Ok(chord)
}

/// `chord` laid onto a progression: whatever it overlaps is trimmed back, removed when fully
/// covered, or split in two when it spans the new chord (the far half gets a fresh id). The
/// same rule as drawing a chord in the lane (`ui/src/songSetup.ts` `placeChord`), so an
/// accepted detection lands exactly as the hand-drawn equivalent would.
fn place_chord(chords: Vec<ChordEvent>, chord: ChordEvent) -> Vec<ChordEvent> {
    let (s, e) = (chord.start_tick, chord.end_tick());
    let mut out = Vec::with_capacity(chords.len() + 2);
    for c in chords {
        if c.id == chord.id {
            continue;
        }
        let c_end = c.end_tick();
        if c_end <= s || c.start_tick >= e {
            out.push(c);
            continue;
        }
        if c_end > e {
            let mut tail = c.clone();
            if c.start_tick < s {
                tail.id = crate::ids::mint();
            }
            tail.start_tick = e;
            tail.length_ticks = c_end - e;
            out.push(tail);
        }
        if c.start_tick < s {
            let mut head = c;
            head.length_ticks = s - head.start_tick;
            out.push(head);
        }
    }
    out.push(chord);
    out.sort_by_key(|c| c.start_tick);
    out
}

/// `clips` with everything the punch covers on the take's track cut back around it.
///
/// A clip the take lands inside becomes at most two: a head that runs from its own start to
/// `xf` seconds *past* the take's start, fading out across that overlap, and a tail that
/// starts `xf` seconds *before* the take's end, fading in across that one. The overlaps are
/// the crossfades — the engine sums overlapping clips, so both sides sounding at once is the
/// whole mechanism. A clip buried entirely under the take leaves nothing and is dropped.
///
/// Audio never warps, so each new edge is carried through the tempo map: the piece is placed
/// on the nearest whole tick and its offset into the file is taken from *that* tick, so the
/// material under it never slides even when the edge falls between ticks.
fn punch_split(map: &TempoMap, clips: Vec<AudioClip>, take: &AudioClip, xf: f64) -> Vec<AudioClip> {
    let from = map.seconds_at_tick(take.start_tick as f64);
    let to = from + take.length_seconds;
    let mut out = Vec::with_capacity(clips.len() + 1);
    for c in clips {
        let start = map.seconds_at_tick(c.start_tick as f64);
        let end = start + c.length_seconds;
        if c.track_id != take.track_id || end <= from || start >= to {
            out.push(c);
            continue;
        }
        // Only a clip that was already sounding when the punch dropped in has a head; one that
        // starts inside the take has nothing in front of it to fade out.
        let head_end = (from + xf).min(end);
        let head = (start < from).then(|| AudioClip {
            length_seconds: head_end - start,
            fade_out_seconds: xf,
            ..c.clone()
        });
        let tail = (end > to).then(|| {
            let tick = map.tick_at_seconds((to - xf).max(start)).round() as i64;
            let at = map.seconds_at_tick(tick as f64).clamp(start, end);
            AudioClip {
                // The head keeps the clip's identity when there is one, the way a ripple split
                // does; with the take butted against the clip's front, the tail is all that is
                // left of it and keeps the id instead.
                id: if head.is_some() { crate::ids::mint() } else { c.id },
                start_tick: tick,
                length_seconds: end - at,
                offset_seconds: c.offset_seconds + (at - start),
                fade_in_seconds: xf,
                ..c.clone()
            }
        });
        out.extend(head.and_then(AudioClip::sanitized));
        out.extend(tail.and_then(AudioClip::sanitized));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ScaleType;

    const BEAT: i64 = TICKS_PER_BEAT;

    /// The starter: melody + drums, one pattern each ("melody 1", "drums 1"), each placed once
    /// across the whole 128-beat song.
    fn doc() -> Document {
        Document::new(ProjectModel::starter())
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut d = doc();
        let original = d.model().clone();
        d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
        assert_eq!(d.model().tempo, 90.0);
        assert!(d.undo());
        assert_eq!(d.model(), &original);
        assert!(d.redo());
        assert_eq!(d.model().tempo, 90.0);
        assert!(!d.redo());
    }

    #[test]
    fn noop_command_does_not_create_undo_step() {
        let mut d = doc();
        d.apply(Command::SetTempo { bpm: 120.0 }, false).unwrap();
        assert!(!d.can_undo());
    }

    #[test]
    fn transient_edits_coalesce_into_one_undo_step() {
        let mut d = doc();
        let id = d.model().tracks[0].id;
        for v in [0.9, 0.8, 0.7, 0.6] {
            d.apply(
                Command::SetTrackParam {
                    id,
                    param: TrackParam::Volume,
                    value: v,
                },
                true,
            )
            .unwrap();
        }
        d.commit_gesture();
        assert_eq!(d.model().tracks[0].volume, 0.6);
        assert!(d.undo());
        assert_eq!(d.model().tracks[0].volume, 1.0);
        assert!(!d.undo());
    }

    #[test]
    fn transient_run_is_closed_by_next_committed_command() {
        let mut d = doc();
        let id = d.model().tracks[0].id;
        d.apply(
            Command::SetTrackParam {
                id,
                param: TrackParam::Pan,
                value: -0.5,
            },
            true,
        )
        .unwrap();
        d.apply(Command::SetTempo { bpm: 100.0 }, false).unwrap();
        assert!(d.undo()); // tempo
        assert_eq!(d.model().tracks[0].pan, -0.5);
        assert!(d.undo()); // pan gesture
        assert_eq!(d.model().tracks[0].pan, 0.0);
    }

    #[test]
    fn add_track_comes_with_an_empty_pattern() {
        let mut d = doc();
        d.apply(Command::AddTrack { is_drum: false }, false)
            .unwrap();
        let t = d.model().tracks.last().unwrap().clone();
        assert_eq!(t.name, "track 3");
        let mine: Vec<&SongPattern> = d.model().patterns_of(&t.id).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].name, "track 3 1");
        assert!(mine[0].notes.is_empty());
        assert_eq!(d.undo_label(), Some("add track"));
        d.undo();
        assert_eq!(
            d.model().patterns.len(),
            2,
            "undo takes the pattern with the track"
        );
    }

    #[test]
    fn delete_track_takes_its_patterns_and_clips_and_guards_last() {
        let mut d = doc();
        let (melody, drums) = (d.model().tracks[0].id, d.model().tracks[1].id);
        d.apply(Command::AddPattern { track_id: drums }, false)
            .unwrap();
        let fill = d.model().patterns_of(&drums).nth(1).unwrap().id;
        d.apply(
            Command::AddClip {
                pattern_id: fill,
                start_tick: 0,
                length_ticks: None,
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().clips.len(), 3);

        d.apply(Command::DeleteTrack { id: drums }, false).unwrap();
        assert_eq!(d.model().tracks.len(), 1);
        assert!(
            d.model().patterns.iter().all(|p| p.track_id == melody),
            "the drum patterns went with the track"
        );
        assert_eq!(d.model().clips.len(), 1, "and so did their clips");
        assert_eq!(d.model().clips[0].pattern_id, d.model().patterns[0].id);
        assert_eq!(
            d.apply(Command::DeleteTrack { id: melody }, false),
            Err(EditError::LastTrack)
        );
        assert_eq!(
            d.apply(Command::DeleteTrack { id: drums }, false),
            Err(EditError::NoSuchTrack)
        );

        assert!(d.undo());
        assert_eq!(d.model().patterns_of(&drums).count(), 2);
        assert_eq!(d.model().clips.len(), 3);
    }

    #[test]
    fn add_pattern_names_after_the_track_and_skips_taken_names() {
        let mut d = doc();
        let melody = d.model().tracks[0].id;
        d.apply(Command::AddPattern { track_id: melody }, false)
            .unwrap();
        let names: Vec<&str> = d.model().patterns.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["melody 1", "melody 2", "drums 1"],
            "next to its siblings"
        );
        let second = d.model().patterns[1].clone();
        assert_eq!(second.track_id, melody);
        assert_eq!(second.length_ticks, 16 * BEAT);
        d.apply(
            Command::RenamePattern {
                id: second.id,
                name: "melody 3".into(),
            },
            false,
        )
        .unwrap();
        d.apply(Command::AddPattern { track_id: melody }, false)
            .unwrap();
        assert_eq!(d.model().patterns[2].name, "melody 4");
        assert_eq!(
            d.apply(
                Command::AddPattern {
                    track_id: Uuid::new_v4()
                },
                false
            ),
            Err(EditError::NoSuchTrack)
        );
    }

    #[test]
    fn set_notes_sanitizes_and_sorts() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        let notes = vec![
            NoteEvent::new(4 * BEAT, BEAT, 200).with_velocity(999),
            NoteEvent::new(-3 * BEAT, 0, 60),
        ];
        d.apply(
            Command::SetNotes {
                pattern_id: pid,
                notes,
            },
            false,
        )
        .unwrap();
        let stored = &d.model().patterns[0].notes;
        assert_eq!(stored[0].start_tick, 0);
        assert!(stored[0].length_ticks > 0);
        assert_eq!(stored[1].pitch, 127);
        assert_eq!(stored[1].velocity, 127);
    }

    #[test]
    fn set_chords_sorts_and_removes_overlaps() {
        let mut d = doc();
        let chords = vec![
            ChordEvent::new(8 * BEAT, 4 * BEAT, vec![19, 11, 2, 2], Some("G".into())),
            ChordEvent::new(0, 12 * BEAT, vec![0, 4, 7], Some("C".into())),
            ChordEvent::new(-BEAT, 4 * BEAT, vec![0], None),
            ChordEvent::new(12 * BEAT, 0, vec![0], None),
        ];
        d.apply(Command::SetChords { chords }, false).unwrap();
        let spans: Vec<(i64, i64, Vec<i32>)> = d
            .model()
            .chords
            .chords()
            .iter()
            .map(|c| (c.start_tick, c.length_ticks, c.pitch_classes.clone()))
            .collect();
        assert_eq!(
            spans,
            vec![
                (0, 8 * BEAT, vec![0, 4, 7]),
                (8 * BEAT, 4 * BEAT, vec![7, 11, 2])
            ],
            "root first, folded, no repeats"
        );
        assert_eq!(d.undo_label(), Some("edit chords"));
    }

    #[test]
    fn deleting_patterns_removes_their_clips_down_to_none() {
        let mut d = doc();
        let (melody, drums) = (d.model().patterns[0].id, d.model().patterns[1].id);
        d.apply(Command::DeletePattern { id: melody }, false)
            .unwrap();
        assert_eq!(d.model().clips.len(), 1);
        assert_eq!(d.model().clips[0].pattern_id, drums);
        d.apply(Command::DeletePattern { id: drums }, false)
            .unwrap();
        assert!(
            d.model().patterns.is_empty(),
            "a project may have no patterns at all"
        );
        assert!(d.model().clips.is_empty());
        assert_eq!(d.model().tracks.len(), 2, "the tracks stay");
        assert_eq!(
            d.apply(Command::DeletePattern { id: drums }, false),
            Err(EditError::NoSuchPattern)
        );
    }

    #[test]
    fn add_clip_is_held_inside_the_song() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        assert_eq!(d.model().song_length_ticks, 128 * BEAT);
        assert_eq!(
            d.apply(
                Command::AddClip {
                    pattern_id: Uuid::new_v4(),
                    start_tick: 0,
                    length_ticks: None
                },
                false
            ),
            Err(EditError::NoSuchPattern)
        );
        assert_eq!(
            d.apply(
                Command::AddClip {
                    pattern_id: pid,
                    start_tick: -1,
                    length_ticks: None
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
        assert_eq!(
            d.apply(
                Command::AddClip {
                    pattern_id: pid,
                    start_tick: 128 * BEAT,
                    length_ticks: None
                },
                false
            ),
            Err(EditError::PastSongEnd)
        );
        d.apply(
            Command::AddClip {
                pattern_id: pid,
                start_tick: 120 * BEAT,
                length_ticks: None,
            },
            false,
        )
        .unwrap();
        let last = d.model().clips.last().unwrap();
        assert_eq!(
            (last.start_tick, last.length_ticks),
            (120 * BEAT, 8 * BEAT),
            "the 16-beat pattern is cut at the end"
        );
        d.apply(
            Command::AddClip {
                pattern_id: pid,
                start_tick: 32 * BEAT,
                length_ticks: Some(8 * BEAT),
            },
            false,
        )
        .unwrap();
        let starts: Vec<i64> = d.model().clips.iter().map(|c| c.start_tick).collect();
        assert_eq!(starts, vec![0, 0, 32 * BEAT, 120 * BEAT], "sorted");
    }

    #[test]
    fn update_clip_is_clamped_and_keeps_its_pattern() {
        let mut d = doc();
        let drums = d.model().patterns[1].id;
        let mut c = d.model().clips[0].clone();
        let melody = c.pattern_id;
        c.length_ticks = 0;
        assert_eq!(
            d.apply(Command::UpdateClip { clip: c.clone() }, false),
            Err(EditError::InvalidValue)
        );
        c.start_tick = 128 * BEAT;
        c.length_ticks = 8 * BEAT;
        assert_eq!(
            d.apply(Command::UpdateClip { clip: c.clone() }, false),
            Err(EditError::PastSongEnd)
        );
        c.start_tick = 112 * BEAT;
        c.length_ticks = 64 * BEAT;
        c.offset_ticks = 4 * BEAT;
        c.pattern_id = drums;
        d.apply(Command::UpdateClip { clip: c.clone() }, false)
            .unwrap();
        let stored = d.model().clip(&c.id).unwrap();
        assert_eq!(
            (stored.start_tick, stored.length_ticks, stored.offset_ticks),
            (112 * BEAT, 16 * BEAT, 4 * BEAT)
        );
        assert_eq!(
            stored.pattern_id, melody,
            "a clip never changes pattern (or track) by update"
        );
        let ghost = Clip::new(melody, 0, 4 * BEAT);
        assert_eq!(
            d.apply(Command::UpdateClip { clip: ghost }, false),
            Err(EditError::NoSuchClip)
        );
    }

    #[test]
    fn update_clips_moves_a_group_in_one_step_or_not_at_all() {
        let mut d = doc();
        let before = d.model().clone();
        let mut moved: Vec<Clip> = d.model().clips.clone();
        for c in &mut moved {
            c.start_tick = 16 * BEAT;
            c.length_ticks = 32 * BEAT;
        }
        d.apply(
            Command::UpdateClips {
                clips: moved.clone(),
            },
            false,
        )
        .unwrap();
        assert!(d
            .model()
            .clips
            .iter()
            .all(|c| c.start_tick == 16 * BEAT && c.length_ticks == 32 * BEAT));
        assert_eq!(d.undo_label(), Some("move clips"));
        assert!(d.undo());
        assert_eq!(d.model(), &before, "one undo puts the whole group back");

        moved[1].start_tick = 500 * BEAT;
        assert_eq!(
            d.apply(Command::UpdateClips { clips: moved }, false),
            Err(EditError::PastSongEnd)
        );
        assert_eq!(d.model(), &before, "one bad clip refuses the whole move");
    }

    #[test]
    fn remove_clips_takes_a_selection_in_one_step() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        d.apply(
            Command::AddClip {
                pattern_id: pid,
                start_tick: 64 * BEAT,
                length_ticks: None,
            },
            false,
        )
        .unwrap();
        let ids: Vec<Uuid> = d.model().clips.iter().take(2).map(|c| c.id).collect();
        d.apply(
            Command::RemoveClips {
                ids: [ids.clone(), vec![Uuid::new_v4()]].concat(),
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().clips.len(), 1);
        assert!(!ids.contains(&d.model().clips[0].id));
        assert_eq!(d.undo_label(), Some("remove clips"));
        d.undo();
        assert_eq!(d.model().clips.len(), 3);
    }

    #[test]
    fn make_clip_unique_forks_the_pattern_for_that_clip_only() {
        let mut d = doc();
        let melody = d.model().patterns[0].clone();
        let first = d
            .model()
            .clips
            .iter()
            .find(|c| c.pattern_id == melody.id)
            .unwrap()
            .id;
        d.apply(
            Command::UpdateClip {
                clip: Clip {
                    length_ticks: 16 * BEAT,
                    ..d.model().clip(&first).unwrap().clone()
                },
            },
            false,
        )
        .unwrap();
        d.apply(
            Command::AddClip {
                pattern_id: melody.id,
                start_tick: 16 * BEAT,
                length_ticks: None,
            },
            false,
        )
        .unwrap();
        let second = d
            .model()
            .clips
            .iter()
            .find(|c| c.start_tick == 16 * BEAT)
            .unwrap()
            .id;

        d.apply(Command::MakeClipUnique { id: second }, false)
            .unwrap();
        assert_eq!(d.undo_label(), Some("make clip unique"));
        let m = d.model();
        let fork = m.pattern(&m.clip(&second).unwrap().pattern_id).unwrap();
        assert_ne!(fork.id, melody.id);
        assert_eq!(
            fork.track_id, melody.track_id,
            "the fork stays on the same track"
        );
        assert_eq!(fork.name, "melody 1 copy");
        assert_eq!(m.patterns[1].id, fork.id, "listed right after the original");
        let musical = |p: &SongPattern| {
            p.notes
                .iter()
                .map(|n| (n.start_tick, n.pitch))
                .collect::<Vec<_>>()
        };
        assert_eq!(musical(fork), musical(&melody));
        assert!(
            fork.notes
                .iter()
                .all(|n| melody.notes.iter().all(|o| o.id != n.id)),
            "fresh note ids"
        );
        assert_eq!(
            m.clip(&first).unwrap().pattern_id,
            melody.id,
            "the other clip still plays the original"
        );
        assert_eq!(
            d.apply(Command::MakeClipUnique { id: Uuid::new_v4() }, false),
            Err(EditError::NoSuchClip)
        );
    }

    #[test]
    fn set_song_length_removes_and_trims_clips_past_the_end() {
        let mut d = doc();
        let (melody, drums) = (d.model().patterns[0].id, d.model().patterns[1].id);
        d.apply(
            Command::AddClip {
                pattern_id: melody,
                start_tick: 48 * BEAT,
                length_ticks: Some(16 * BEAT),
            },
            false,
        )
        .unwrap();
        d.apply(
            Command::AddClip {
                pattern_id: drums,
                start_tick: 32 * BEAT,
                length_ticks: Some(16 * BEAT),
            },
            false,
        )
        .unwrap();
        d.apply(Command::SetSongLength { ticks: 40 * BEAT }, false)
            .unwrap();
        let m = d.model();
        assert_eq!(m.song_length_ticks, 40 * BEAT);
        let spans: Vec<(i64, i64)> = m
            .clips
            .iter()
            .map(|c| (c.start_tick, c.end_tick()))
            .collect();
        assert_eq!(
            spans,
            vec![(0, 40 * BEAT), (0, 40 * BEAT), (32 * BEAT, 40 * BEAT)],
            "the clip at beat 48 is gone, the rest end at 40"
        );
        assert_eq!(d.undo_label(), Some("set song length"));

        d.apply(Command::SetSongLength { ticks: BEAT }, false)
            .unwrap();
        assert_eq!(d.model().song_length_ticks, 4 * BEAT, "at least one bar");
        assert!(d.model().clips.iter().all(|c| c.end_tick() <= 4 * BEAT));
        d.undo();
        d.undo();
        assert_eq!(d.model().clips.len(), 4, "undo brings removed clips back");
    }

    #[test]
    fn set_loop_region_sets_clears_and_stays_inside_the_song() {
        let mut d = doc();
        d.apply(
            Command::SetLoopRegion {
                region: Some((16 * BEAT, 32 * BEAT)),
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().loop_region, Some((16 * BEAT, 32 * BEAT)));
        assert_eq!(d.undo_label(), Some("set loop"));

        // Past the end: held back to it rather than refused, like a clip.
        let end = d.model().song_length_ticks;
        d.apply(
            Command::SetLoopRegion {
                region: Some((8 * BEAT, end + 100 * BEAT)),
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().loop_region, Some((8 * BEAT, end)));

        for bad in [(8 * BEAT, 8 * BEAT), (32 * BEAT, 16 * BEAT), (-BEAT, 4 * BEAT)] {
            assert_eq!(
                d.apply(
                    Command::SetLoopRegion {
                        region: Some(bad)
                    },
                    false
                ),
                Err(EditError::InvalidValue),
                "{bad:?}"
            );
        }
        assert_eq!(
            d.apply(
                Command::SetLoopRegion {
                    region: Some((end, end + BEAT))
                },
                false
            ),
            Err(EditError::InvalidValue),
            "a brace entirely past the song end has nothing left to loop"
        );
        assert_eq!(d.model().loop_region, Some((8 * BEAT, end)), "refused edits change nothing");

        d.apply(Command::SetLoopRegion { region: None }, false)
            .unwrap();
        assert_eq!(d.model().loop_region, None);
        assert_eq!(d.undo_label(), Some("clear loop"));
        d.undo();
        assert_eq!(d.model().loop_region, Some((8 * BEAT, end)), "undo brings the loop back");
    }

    #[test]
    fn shortening_the_song_pulls_the_loop_back_inside_it() {
        let mut d = doc();
        d.apply(
            Command::SetLoopRegion {
                region: Some((16 * BEAT, 48 * BEAT)),
            },
            false,
        )
        .unwrap();
        d.apply(Command::SetSongLength { ticks: 32 * BEAT }, false)
            .unwrap();
        assert_eq!(d.model().loop_region, Some((16 * BEAT, 32 * BEAT)));
        d.apply(Command::SetSongLength { ticks: 8 * BEAT }, false)
            .unwrap();
        assert_eq!(d.model().loop_region, None, "nothing of it is left inside the song");
    }

    #[test]
    fn sections_carry_their_own_key() {
        let mut d = doc();
        d.apply(
            Command::SetKey {
                key: KeyState::locked(0, ScaleType::Major),
            },
            false,
        )
        .unwrap();
        d.apply(
            Command::AddSection {
                name: "verse".into(),
                start_tick: 0,
            },
            false,
        )
        .unwrap();
        d.apply(
            Command::AddSection {
                name: "".into(),
                start_tick: 16 * BEAT,
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().sections[1].name, "section 2");
        let mut bridge = d.model().sections[1].clone();
        bridge.key = Some(KeyState::locked(9, ScaleType::Minor));
        d.apply(
            Command::UpdateSection {
                section: bridge.clone(),
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().key_at(3 * BEAT).root_pitch_class, Some(0));
        assert_eq!(d.model().key_at(20 * BEAT).root_pitch_class, Some(9));
        assert_eq!(
            d.model().section_at(20 * BEAT).map(|s| s.name.as_str()),
            Some("section 2")
        );
        d.apply(Command::RemoveSection { id: bridge.id }, false)
            .unwrap();
        assert_eq!(d.model().key_at(20 * BEAT).root_pitch_class, Some(0));
    }

    #[test]
    fn duplicate_pattern_gets_fresh_ids_on_the_same_track() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        d.apply(
            Command::SetNotes {
                pattern_id: pid,
                notes: vec![NoteEvent::new(0, BEAT, 60)],
            },
            false,
        )
        .unwrap();
        d.apply(Command::DuplicatePattern { id: pid }, false)
            .unwrap();
        let (a, b) = (&d.model().patterns[0], &d.model().patterns[1]);
        assert_ne!(a.id, b.id);
        assert_eq!(a.track_id, b.track_id);
        assert_ne!(a.notes[0].id, b.notes[0].id);
        assert_eq!(b.name, "melody 1 copy");
    }

    #[test]
    fn track_color_is_validated() {
        let mut d = doc();
        let id = d.model().tracks[0].id;
        d.apply(
            Command::SetTrackColor {
                id,
                color: Some("#FF3B30".into()),
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().tracks[0].color.as_deref(), Some("#ff3b30"));
        assert_eq!(
            d.apply(
                Command::SetTrackColor {
                    id,
                    color: Some("red".into())
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
        d.apply(Command::SetTrackColor { id, color: None }, false)
            .unwrap();
        assert!(d.model().tracks[0].color.is_none());
    }

    #[test]
    fn time_signature_is_validated_and_keeps_the_song_a_bar_long() {
        let mut d = doc();
        d.apply(Command::SetSongLength { ticks: 4 * BEAT }, false)
            .unwrap();
        d.apply(
            Command::SetTimeSignature {
                numerator: 7,
                denominator: 4,
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().time_signature.beats_per_bar(), 7.0);
        assert_eq!(
            d.model().song_length_ticks,
            7 * BEAT,
            "the song stays at least one bar"
        );
        d.apply(
            Command::SetTimeSignature {
                numerator: 7,
                denominator: 8,
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().time_signature.beats_per_bar(), 3.5);
        assert_eq!(
            d.apply(
                Command::SetTimeSignature {
                    numerator: 0,
                    denominator: 4
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
        assert_eq!(
            d.apply(
                Command::SetTimeSignature {
                    numerator: 4,
                    denominator: 3
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn undo_labels_follow_the_history() {
        let mut d = doc();
        assert_eq!(d.undo_label(), None);
        d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
        assert_eq!(d.undo_label(), Some("set tempo"));
        let (pid, tid) = (d.model().patterns[0].id, d.model().tracks[0].id);
        d.apply_labeled(
            Command::SetNotes {
                pattern_id: pid,
                notes: vec![NoteEvent::new(0, BEAT, 60)],
            },
            false,
            Some("place note".into()),
        )
        .unwrap();
        assert_eq!(d.undo_label(), Some("place note"));
        assert_eq!(d.redo_label(), None);
        d.undo();
        assert_eq!(d.undo_label(), Some("set tempo"));
        assert_eq!(d.redo_label(), Some("place note"));
        d.redo();
        assert_eq!(d.undo_label(), Some("place note"));
        // A transient gesture carries its label while it is still open.
        d.apply(
            Command::SetTrackParam {
                id: tid,
                param: TrackParam::Pan,
                value: 0.3,
            },
            true,
        )
        .unwrap();
        assert_eq!(d.undo_label(), Some("change pan"));
        d.commit_gesture();
        assert_eq!(d.undo_label(), Some("change pan"));
        // An empty label falls back to the command's own.
        d.apply_labeled(Command::SetTempo { bpm: 95.0 }, false, Some("  ".into()))
            .unwrap();
        assert_eq!(d.undo_label(), Some("set tempo"));
    }

    #[test]
    fn revision_bumps_on_every_change_and_restore_stays_dirty() {
        let mut d = doc();
        let r0 = d.revision();
        d.apply(Command::SetTempo { bpm: 120.0 }, false).unwrap(); // no-op
        assert_eq!(d.revision(), r0);
        d.apply(Command::SetTempo { bpm: 99.0 }, false).unwrap();
        assert!(d.revision() > r0);
        let r1 = d.revision();
        d.undo();
        assert!(d.revision() > r1);
        d.mark_saved();
        assert!(!d.is_dirty());
        d.restore(ProjectModel::starter(), None);
        assert!(d.is_dirty());
        assert!(!d.can_undo());
    }

    #[test]
    fn import_tracks_adds_a_track_pattern_and_clip_each_in_one_undo_step() {
        let mut d = doc();
        let before = d.model().clone();
        d.apply(
            Command::ImportTracks {
                tracks: vec![
                    ImportedTrack {
                        name: "piano".into(),
                        is_drum: false,
                        notes: vec![NoteEvent::new(0, BEAT, 60)],
                    },
                    ImportedTrack {
                        name: "kit".into(),
                        is_drum: true,
                        notes: vec![NoteEvent::new(0, BEAT / 4, 36)],
                    },
                ],
                length_ticks: 160 * BEAT,
            },
            false,
        )
        .unwrap();
        let m = d.model();
        assert_eq!(m.tracks.len(), before.tracks.len() + 2);
        let kit = m.tracks.last().unwrap();
        assert!(kit.is_drum);
        let kit_patterns: Vec<&SongPattern> = m.patterns_of(&kit.id).collect();
        assert_eq!(kit_patterns.len(), 1);
        assert_eq!(kit_patterns[0].name, "kit 1");
        assert_eq!(kit_patterns[0].length_ticks, 160 * BEAT);
        assert_eq!(kit_patterns[0].notes.len(), 1);
        let clip = m
            .clips
            .iter()
            .find(|c| c.pattern_id == kit_patterns[0].id)
            .unwrap();
        assert_eq!((clip.start_tick, clip.length_ticks), (0, 160 * BEAT));
        assert_eq!(m.song_length_ticks, 160 * BEAT, "the song grows to fit");
        assert_eq!(d.undo_label(), Some("import MIDI"));
        d.undo();
        assert_eq!(d.model(), &before);
        assert_eq!(
            d.apply(
                Command::ImportTracks {
                    tracks: vec![],
                    length_ticks: 16 * BEAT
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn tempo_points_are_edited_one_per_tick() {
        let mut d = doc();
        d.apply(
            Command::AddTempoPoint {
                tick: 16 * BEAT,
                bpm: 90.0,
                ramp: false,
            },
            false,
        )
        .unwrap();
        d.apply(
            Command::AddTempoPoint {
                tick: 16 * BEAT,
                bpm: 95.0,
                ramp: true,
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().tempo_points.len(), 1);
        assert_eq!(d.model().tempo_points[0].bpm, 95.0);
        assert!(d.model().tempo_points[0].ramp);
        assert_eq!(d.model().tempo_map().bpm_at_tick((20 * BEAT) as f64), 95.0);
        let mut p = d.model().tempo_points[0].clone();
        p.tick = 8 * BEAT;
        p.bpm = 999.0;
        d.apply(Command::UpdateTempoPoint { point: p.clone() }, false)
            .unwrap();
        assert_eq!(d.model().tempo_points[0].bpm, 300.0);
        assert_eq!(d.model().tempo_points[0].tick, 8 * BEAT);
        d.apply(Command::RemoveTempoPoint { id: p.id }, false)
            .unwrap();
        assert!(d.model().tempo_points.is_empty());
        assert_eq!(
            d.apply(Command::RemoveTempoPoint { id: p.id }, false),
            Err(EditError::NoSuchTempoPoint)
        );
        assert_eq!(
            d.apply(
                Command::AddTempoPoint {
                    tick: -1,
                    bpm: 100.0,
                    ramp: false
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn swing_is_validated() {
        let mut d = doc();
        d.apply(
            Command::SetSwing {
                swing: 90.0,
                grid_ticks: BEAT / 4,
            },
            false,
        )
        .unwrap();
        assert_eq!(d.model().swing, 75.0);
        assert_eq!(d.model().swing_grid_ticks, BEAT / 4);
        assert_eq!(
            d.apply(
                Command::SetSwing {
                    swing: 60.0,
                    grid_ticks: 300
                },
                false
            ),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn intentional_flag_survives_set_notes() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        let mut n = NoteEvent::new(0, BEAT, 65);
        n.intentional = true;
        d.apply(
            Command::SetNotes {
                pattern_id: pid,
                notes: vec![n],
            },
            false,
        )
        .unwrap();
        assert!(d.model().patterns[0].notes[0].intentional);
    }
}
