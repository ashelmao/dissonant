//! The document: a [`ProjectModel`] plus undo/redo, mutated only through [`Command`]s.
//!
//! Every command is validated and applied here, so the UI, tests and any future scripting or
//! collaboration layer all go through one door. Undo is snapshot-based (the model is small);
//! continuous controls (sliders) send `transient` commands which coalesce into one undo step
//! per gesture.

use crate::midi_file::ImportedTrack;
use crate::model::{tidy_chords, ChordEvent, Clip, KeyState, MasterSettings, NoteEvent, ProjectModel, Section, SongPattern, Track};
use crate::sound::{is_factory, valid_kit, DrumPad, Sound};
use crate::tempo::TempoPoint;
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

/// Every edit the UI can make. Serialized as `{"type": "...", ...}` over Tauri IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    SetTempo { bpm: f64 },
    SetKey { key: KeyState },
    /// A new track, with one empty pattern ("<track name> 1") to write into.
    AddTrack { is_drum: bool },
    /// Removes the track, its patterns and their clips.
    DeleteTrack { id: Uuid },
    RenameTrack { id: Uuid, name: String },
    SetTrackMuted { id: Uuid, muted: bool },
    SetTrackSoloed { id: Uuid, soloed: bool },
    MoveTrack { id: Uuid, up: bool },
    SetTrackVoice { id: Uuid, voice: String },
    /// Point a synth track at a sound. A non-factory sound is embedded in the project (replacing
    /// any copy with the same id); embedded sounds no track uses any more are dropped.
    SetTrackSound { id: Uuid, sound: Sound },
    /// Replace a drum track's whole kit (1–16 pads, unique pitches). `sounds` carries the
    /// non-factory drum sounds the pads use, to embed. Unused embedded sounds are dropped.
    SetDrumPads { id: Uuid, pads: Vec<DrumPad>, sounds: Vec<Sound> },
    /// Replace the project's embedded copy of a sound (after it was re-saved in the library).
    UpsertSound { sound: Sound },
    SetTrackParam { id: Uuid, param: TrackParam, value: f64 },
    /// `#rrggbb`, or `None` to fall back to the palette.
    SetTrackColor { id: Uuid, color: Option<String> },
    /// A new empty pattern on a track, named "<track name> <n>".
    AddPattern { track_id: Uuid },
    /// A copy on the same track, with fresh note ids.
    DuplicatePattern { id: Uuid },
    /// Removes the pattern and its clips. A track may end up with no patterns.
    DeletePattern { id: Uuid },
    RenamePattern { id: Uuid, name: String },
    SetPatternLength { id: Uuid, beats: f64 },
    /// Replace a pattern's notes (the roll commits whole arrays).
    SetNotes { pattern_id: Uuid, notes: Vec<NoteEvent> },
    /// Replace the project's chord progression (song beats).
    SetChords { chords: Vec<ChordEvent> },
    /// Place a pattern on the song (on its own track). `length_beats` defaults to the pattern
    /// length. Refused at or past the song end; a clip crossing it is cut there.
    AddClip { pattern_id: Uuid, start_beat: f64, length_beats: Option<f64> },
    /// Move / resize / re-offset / mute a clip (matched by id). The clip keeps its pattern
    /// (a changed `pattern_id` is ignored) and is held inside the song like `AddClip`.
    UpdateClip { clip: Clip },
    /// Several `UpdateClip`s as one undo step (a group move). All or nothing.
    UpdateClips { clips: Vec<Clip> },
    RemoveClip { id: Uuid },
    /// Remove several clips as one undo step. Unknown ids are skipped.
    RemoveClips { ids: Vec<Uuid> },
    /// Fork the clip's pattern into a copy on the same track and point this one clip at it.
    MakeClipUnique { id: Uuid },
    /// Move the song's hard end (at least one bar). Clips starting at or after it are
    /// removed; clips crossing it are cut.
    SetSongLength { beats: f64 },
    AddSection { name: String, start_beat: f64 },
    UpdateSection { section: Section },
    RemoveSection { id: Uuid },
    SetMaster { master: MasterSettings },
    SetTimeSignature { numerator: u32, denominator: u32 },
    /// Global swing in percent (50–75) and its grid in beats (0.5 or 0.25).
    SetSwing { swing: f64, grid: f64 },
    /// Add one track per imported MIDI track, each with one `length_beats` pattern holding its
    /// notes and one clip of it at beat 0, in one undo step. The song grows to fit.
    ImportTracks { tracks: Vec<ImportedTrack>, length_beats: f64 },
    /// A tempo change on the song timeline (replaces any point already on that beat).
    AddTempoPoint { beat: f64, bpm: f64, ramp: bool },
    UpdateTempoPoint { point: TempoPoint },
    RemoveTempoPoint { id: Uuid },
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
    #[error("invalid value")]
    InvalidValue,
    #[error("that is past the end of the song")]
    PastSongEnd,
    #[error("no such clip")]
    NoSuchClip,
    #[error("no such section")]
    NoSuchSection,
    #[error("no such tempo point")]
    NoSuchTempoPoint,
}

/// One undo step: the model before the edit, and what the edit was ("move notes").
#[derive(Debug, Clone)]
struct HistoryEntry {
    model: ProjectModel,
    label: String,
}

#[derive(Debug, Clone)]
pub struct Document {
    model: ProjectModel,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// Snapshot taken at the start of a transient gesture, pushed to `undo` when it ends.
    pending: Option<HistoryEntry>,
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
        !self.undo.is_empty() || self.pending.as_ref().is_some_and(|p| p.model != self.model)
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
    pub fn apply_labeled(&mut self, command: Command, transient: bool, label: Option<String>) -> Result<(), EditError> {
        let label = label.filter(|l| !l.trim().is_empty()).unwrap_or_else(|| command.label());
        let before = self.model.clone();
        let mut next = self.model.clone();
        apply_to(&mut next, command)?;
        if next == before {
            return Ok(());
        }
        if transient {
            if self.pending.is_none() {
                self.pending = Some(HistoryEntry { model: before, label });
            }
        } else {
            self.commit_gesture();
            self.push_undo(HistoryEntry { model: before, label });
        }
        self.redo.clear();
        self.model = next;
        self.touch();
        Ok(())
    }

    /// End a run of transient edits, making it one undo step.
    pub fn commit_gesture(&mut self) {
        if let Some(entry) = self.pending.take() {
            if entry.model != self.model {
                self.push_undo(entry);
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        self.commit_gesture();
        match self.undo.pop() {
            Some(entry) => {
                let current = std::mem::replace(&mut self.model, entry.model);
                self.redo.push(HistoryEntry { model: current, label: entry.label });
                self.touch();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.commit_gesture();
        match self.redo.pop() {
            Some(entry) => {
                let current = std::mem::replace(&mut self.model, entry.model);
                self.undo.push(HistoryEntry { model: current, label: entry.label });
                self.touch();
                true
            }
            None => false,
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

impl Command {
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
            AddPattern { .. } => "add pattern",
            DuplicatePattern { .. } => "duplicate pattern",
            DeletePattern { .. } => "delete pattern",
            RenamePattern { .. } => "rename pattern",
            SetPatternLength { .. } => "change pattern length",
            SetNotes { .. } => "edit notes",
            SetChords { .. } => "edit chords",
            AddClip { .. } => "add clip",
            UpdateClip { .. } => "edit clip",
            UpdateClips { .. } => "edit clips",
            RemoveClip { .. } => "remove clip",
            RemoveClips { .. } => "remove clips",
            MakeClipUnique { .. } => "make clip unique",
            SetSongLength { .. } => "set song length",
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
        }
        .to_string()
    }
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
            let track = if is_drum { Track::drums("drums") } else { Track::new(format!("track {n}")) };
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
            m.prune_sounds();
        }
        RenameTrack { id, name } => {
            // Matches RenamePattern: a blank name makes the track unidentifiable everywhere.
            if name.trim().is_empty() {
                return Err(EditError::InvalidValue);
            }
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.name = name;
        }
        SetTrackMuted { id, muted } => m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.muted = muted,
        SetTrackSoloed { id, soloed } => m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.soloed = soloed,
        MoveTrack { id, up } => {
            let i = m.tracks.iter().position(|t| t.id == id).ok_or(EditError::NoSuchTrack)?;
            let j = if up { i.checked_sub(1) } else { Some(i + 1) };
            if let Some(j) = j.filter(|&j| j < m.tracks.len()) {
                m.tracks.swap(i, j);
            }
        }
        SetTrackVoice { id, voice } => m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.voice = voice,
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
            match param {
                TrackParam::Volume => t.volume = value.clamp(0.0, 1.5),
                TrackParam::ReverbSend => t.reverb_send = value.clamp(0.0, 1.0),
                TrackParam::Tone => t.tone = value.clamp(200.0, 20_000.0),
                TrackParam::Pan => t.pan = value.clamp(-1.0, 1.0),
            }
        }
        SetTrackColor { id, color } => {
            if let Some(c) = &color {
                let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
                if !ok {
                    return Err(EditError::InvalidValue);
                }
            }
            m.track_mut(&id).ok_or(EditError::NoSuchTrack)?.color = color.map(|c| c.to_lowercase());
        }
        AddPattern { track_id } => {
            let track = m.track(&track_id).ok_or(EditError::NoSuchTrack)?;
            let pattern = SongPattern::new(track_id, next_pattern_name(m, track));
            // Right after the track's last pattern, so the tree's order is creation order.
            let at = m.patterns.iter().rposition(|p| p.track_id == track_id).map_or(m.patterns.len(), |i| i + 1);
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
        SetPatternLength { id, beats } => {
            if !beats.is_finite() || beats < 1.0 {
                return Err(EditError::InvalidValue);
            }
            m.pattern_mut(&id).ok_or(EditError::NoSuchPattern)?.length_beats = beats;
        }
        SetNotes { pattern_id, notes } => {
            let p = m.pattern_mut(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let mut notes: Vec<NoteEvent> = notes.into_iter().map(NoteEvent::sanitized).collect();
            notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat).then(a.pitch.cmp(&b.pitch)));
            p.notes = notes;
        }
        SetChords { chords } => m.chords = tidy_chords(chords),
        AddClip { pattern_id, start_beat, length_beats } => {
            let p = m.pattern(&pattern_id).ok_or(EditError::NoSuchPattern)?;
            let len = length_beats.unwrap_or(p.length_beats);
            if !start_beat.is_finite() || start_beat < 0.0 || !len.is_finite() || len <= 0.0 {
                return Err(EditError::InvalidValue);
            }
            if start_beat >= m.song_length_beats {
                return Err(EditError::PastSongEnd);
            }
            let len = len.min(m.song_length_beats - start_beat);
            m.clips.push(Clip::new(pattern_id, start_beat, len));
            m.clips.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        }
        UpdateClip { clip } => {
            update_clip(m, clip)?;
            m.clips.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        }
        UpdateClips { clips } => {
            for clip in clips {
                update_clip(m, clip)?;
            }
            m.clips.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        }
        RemoveClip { id } => {
            let before = m.clips.len();
            m.clips.retain(|c| c.id != id);
            if m.clips.len() == before {
                return Err(EditError::NoSuchClip);
            }
        }
        RemoveClips { ids } => m.clips.retain(|c| !ids.contains(&c.id)),
        MakeClipUnique { id } => {
            let pattern_id = m.clip(&id).ok_or(EditError::NoSuchClip)?.pattern_id;
            let copy = duplicate_pattern(m, &pattern_id)?;
            if let Some(c) = m.clips.iter_mut().find(|c| c.id == id) {
                c.pattern_id = copy;
            }
        }
        SetSongLength { beats } => {
            if !beats.is_finite() {
                return Err(EditError::InvalidValue);
            }
            m.song_length_beats = beats.max(m.bar_beats());
            m.fit_clips_to_song();
        }
        AddSection { name, start_beat } => {
            if !start_beat.is_finite() || start_beat < 0.0 {
                return Err(EditError::InvalidValue);
            }
            let name = if name.trim().is_empty() { format!("section {}", m.sections.len() + 1) } else { name };
            m.sections.push(Section::new(name, start_beat));
            m.sections.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        }
        UpdateSection { section } => {
            if !section.start_beat.is_finite() || section.start_beat < 0.0 || section.name.trim().is_empty() {
                return Err(EditError::InvalidValue);
            }
            let slot = m.sections.iter_mut().find(|s| s.id == section.id).ok_or(EditError::NoSuchSection)?;
            *slot = section;
            m.sections.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        }
        RemoveSection { id } => {
            let before = m.sections.len();
            m.sections.retain(|s| s.id != id);
            if m.sections.len() == before {
                return Err(EditError::NoSuchSection);
            }
        }
        SetTimeSignature { numerator, denominator } => {
            if !(1..=16).contains(&numerator) || ![2, 4, 8, 16].contains(&denominator) {
                return Err(EditError::InvalidValue);
            }
            m.time_signature = crate::model::TimeSignature { numerator, denominator };
            // The song stays at least one (new) bar long.
            m.song_length_beats = m.song_length_beats.max(m.bar_beats());
        }
        ImportTracks { tracks, length_beats } => {
            if tracks.is_empty() || !length_beats.is_finite() || length_beats < 1.0 {
                return Err(EditError::InvalidValue);
            }
            for t in tracks {
                let mut track = if t.is_drum { Track::drums(&t.name) } else { Track::new(&t.name) };
                if track.name.trim().is_empty() {
                    track.name = format!("track {}", m.tracks.len() + 1);
                }
                let mut pattern = SongPattern::new(track.id, next_pattern_name(m, &track));
                pattern.length_beats = length_beats;
                pattern.notes = t.notes.into_iter().map(NoteEvent::sanitized).collect();
                pattern.notes.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat).then(a.pitch.cmp(&b.pitch)));
                m.clips.push(Clip::new(pattern.id, 0.0, length_beats));
                m.tracks.push(track);
                m.patterns.push(pattern);
            }
            m.song_length_beats = m.song_length_beats.max(length_beats);
            m.clips.sort_by(|a, b| a.start_beat.total_cmp(&b.start_beat));
        }
        AddTempoPoint { beat, bpm, ramp } => {
            if !beat.is_finite() || beat < 0.0 || !bpm.is_finite() {
                return Err(EditError::InvalidValue);
            }
            let bpm = bpm.clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM);
            m.tempo_points.retain(|p| (p.beat - beat).abs() > 1e-9);
            m.tempo_points.push(TempoPoint::new(beat, bpm, ramp));
            m.tempo_points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        }
        UpdateTempoPoint { point } => {
            if !point.beat.is_finite() || point.beat < 0.0 || !point.bpm.is_finite() {
                return Err(EditError::InvalidValue);
            }
            let slot = m.tempo_points.iter_mut().find(|p| p.id == point.id).ok_or(EditError::NoSuchTempoPoint)?;
            *slot = TempoPoint { bpm: point.bpm.clamp(crate::tempo::Tempo::MIN_BPM, crate::tempo::Tempo::MAX_BPM), ..point };
            // Keep one point per beat: the edited one wins.
            let (id, beat) = (slot.id, slot.beat);
            m.tempo_points.retain(|p| p.id == id || (p.beat - beat).abs() > 1e-9);
            m.tempo_points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        }
        RemoveTempoPoint { id } => {
            let before = m.tempo_points.len();
            m.tempo_points.retain(|p| p.id != id);
            if m.tempo_points.len() == before {
                return Err(EditError::NoSuchTempoPoint);
            }
        }
        SetSwing { swing, grid } => {
            if !swing.is_finite() || !(grid == 0.5 || grid == 0.25) {
                return Err(EditError::InvalidValue);
            }
            m.swing = swing.clamp(50.0, 75.0);
            m.swing_grid = grid;
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
            };
        }
    }
    Ok(())
}

/// "<track name> <n>" with the first n past the track's pattern count that no pattern of that
/// track already uses ("bass 1", "bass 2", …).
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

/// Insert a copy of a pattern right after it (same track, fresh ids, "<name> copy") and return
/// the copy's id.
fn duplicate_pattern(m: &mut ProjectModel, id: &Uuid) -> Result<Uuid, EditError> {
    let i = m.patterns.iter().position(|p| &p.id == id).ok_or(EditError::NoSuchPattern)?;
    let copy = m.patterns[i].duplicated();
    let copy_id = copy.id;
    m.patterns.insert(i + 1, copy);
    Ok(copy_id)
}

/// Replace a clip's placement, keeping its pattern and holding it inside the song: refused at
/// or past the end, cut where it crosses it. The caller re-sorts.
fn update_clip(m: &mut ProjectModel, clip: Clip) -> Result<(), EditError> {
    let end = m.song_length_beats;
    let slot = m.clips.iter_mut().find(|c| c.id == clip.id).ok_or(EditError::NoSuchClip)?;
    let bad = [clip.start_beat, clip.length_beats, clip.offset_beats].iter().any(|v| !v.is_finite());
    if bad || clip.start_beat < 0.0 || clip.length_beats <= 0.0 || clip.offset_beats < 0.0 {
        return Err(EditError::InvalidValue);
    }
    if clip.start_beat >= end {
        return Err(EditError::PastSongEnd);
    }
    *slot = Clip {
        pattern_id: slot.pattern_id,
        length_beats: clip.length_beats.min(end - clip.start_beat),
        ..clip
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ScaleType;

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
            d.apply(Command::SetTrackParam { id, param: TrackParam::Volume, value: v }, true).unwrap();
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
        d.apply(Command::SetTrackParam { id, param: TrackParam::Pan, value: -0.5 }, true).unwrap();
        d.apply(Command::SetTempo { bpm: 100.0 }, false).unwrap();
        assert!(d.undo()); // tempo
        assert_eq!(d.model().tracks[0].pan, -0.5);
        assert!(d.undo()); // pan gesture
        assert_eq!(d.model().tracks[0].pan, 0.0);
    }

    #[test]
    fn add_track_comes_with_an_empty_pattern() {
        let mut d = doc();
        d.apply(Command::AddTrack { is_drum: false }, false).unwrap();
        let t = d.model().tracks.last().unwrap().clone();
        assert_eq!(t.name, "track 3");
        let mine: Vec<&SongPattern> = d.model().patterns_of(&t.id).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].name, "track 3 1");
        assert!(mine[0].notes.is_empty());
        assert_eq!(d.undo_label(), Some("add track"));
        d.undo();
        assert_eq!(d.model().patterns.len(), 2, "undo takes the pattern with the track");
    }

    #[test]
    fn delete_track_takes_its_patterns_and_clips_and_guards_last() {
        let mut d = doc();
        let (melody, drums) = (d.model().tracks[0].id, d.model().tracks[1].id);
        d.apply(Command::AddPattern { track_id: drums }, false).unwrap();
        let fill = d.model().patterns_of(&drums).nth(1).unwrap().id;
        d.apply(Command::AddClip { pattern_id: fill, start_beat: 0.0, length_beats: None }, false).unwrap();
        assert_eq!(d.model().clips.len(), 3);

        d.apply(Command::DeleteTrack { id: drums }, false).unwrap();
        assert_eq!(d.model().tracks.len(), 1);
        assert!(d.model().patterns.iter().all(|p| p.track_id == melody), "the drum patterns went with the track");
        assert_eq!(d.model().clips.len(), 1, "and so did their clips");
        assert_eq!(d.model().clips[0].pattern_id, d.model().patterns[0].id);
        assert_eq!(d.apply(Command::DeleteTrack { id: melody }, false), Err(EditError::LastTrack));
        assert_eq!(d.apply(Command::DeleteTrack { id: drums }, false), Err(EditError::NoSuchTrack));

        assert!(d.undo());
        assert_eq!(d.model().patterns_of(&drums).count(), 2);
        assert_eq!(d.model().clips.len(), 3);
    }

    #[test]
    fn add_pattern_names_after_the_track_and_skips_taken_names() {
        let mut d = doc();
        let melody = d.model().tracks[0].id;
        d.apply(Command::AddPattern { track_id: melody }, false).unwrap();
        let names: Vec<&str> = d.model().patterns.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["melody 1", "melody 2", "drums 1"], "next to its siblings");
        let second = d.model().patterns[1].clone();
        assert_eq!(second.track_id, melody);
        assert_eq!(second.length_beats, 16.0);
        d.apply(Command::RenamePattern { id: second.id, name: "melody 3".into() }, false).unwrap();
        d.apply(Command::AddPattern { track_id: melody }, false).unwrap();
        assert_eq!(d.model().patterns[2].name, "melody 4");
        assert_eq!(d.apply(Command::AddPattern { track_id: Uuid::new_v4() }, false), Err(EditError::NoSuchTrack));
    }

    #[test]
    fn set_notes_sanitizes_and_sorts() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        let notes = vec![
            NoteEvent::new(4.0, 1.0, 200).with_velocity(999),
            NoteEvent::new(-3.0, 0.0, 60),
        ];
        d.apply(Command::SetNotes { pattern_id: pid, notes }, false).unwrap();
        let stored = &d.model().patterns[0].notes;
        assert_eq!(stored[0].start_beat, 0.0);
        assert!(stored[0].length_beats > 0.0);
        assert_eq!(stored[1].pitch, 127);
        assert_eq!(stored[1].velocity, 127);
    }

    #[test]
    fn set_chords_sorts_and_removes_overlaps() {
        let mut d = doc();
        let chords = vec![
            ChordEvent::new(8.0, 4.0, vec![19, 11, 2, 2], Some("G".into())),
            ChordEvent::new(0.0, 12.0, vec![0, 4, 7], Some("C".into())),
            ChordEvent::new(-1.0, 4.0, vec![0], None),
            ChordEvent::new(12.0, 0.0, vec![0], None),
        ];
        d.apply(Command::SetChords { chords }, false).unwrap();
        let spans: Vec<(f64, f64, Vec<i32>)> =
            d.model().chords.iter().map(|c| (c.start_beat, c.length_beats, c.pitch_classes.clone())).collect();
        assert_eq!(spans, vec![(0.0, 8.0, vec![0, 4, 7]), (8.0, 4.0, vec![7, 11, 2])], "root first, folded, no repeats");
        assert_eq!(d.undo_label(), Some("edit chords"));
    }

    #[test]
    fn deleting_patterns_removes_their_clips_down_to_none() {
        let mut d = doc();
        let (melody, drums) = (d.model().patterns[0].id, d.model().patterns[1].id);
        d.apply(Command::DeletePattern { id: melody }, false).unwrap();
        assert_eq!(d.model().clips.len(), 1);
        assert_eq!(d.model().clips[0].pattern_id, drums);
        d.apply(Command::DeletePattern { id: drums }, false).unwrap();
        assert!(d.model().patterns.is_empty(), "a project may have no patterns at all");
        assert!(d.model().clips.is_empty());
        assert_eq!(d.model().tracks.len(), 2, "the tracks stay");
        assert_eq!(d.apply(Command::DeletePattern { id: drums }, false), Err(EditError::NoSuchPattern));
    }

    #[test]
    fn add_clip_is_held_inside_the_song() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        assert_eq!(d.model().song_length_beats, 128.0);
        assert_eq!(
            d.apply(Command::AddClip { pattern_id: Uuid::new_v4(), start_beat: 0.0, length_beats: None }, false),
            Err(EditError::NoSuchPattern)
        );
        assert_eq!(d.apply(Command::AddClip { pattern_id: pid, start_beat: -1.0, length_beats: None }, false), Err(EditError::InvalidValue));
        assert_eq!(d.apply(Command::AddClip { pattern_id: pid, start_beat: 128.0, length_beats: None }, false), Err(EditError::PastSongEnd));
        d.apply(Command::AddClip { pattern_id: pid, start_beat: 120.0, length_beats: None }, false).unwrap();
        let last = d.model().clips.last().unwrap();
        assert_eq!((last.start_beat, last.length_beats), (120.0, 8.0), "the 16-beat pattern is cut at the end");
        d.apply(Command::AddClip { pattern_id: pid, start_beat: 32.0, length_beats: Some(8.0) }, false).unwrap();
        let starts: Vec<f64> = d.model().clips.iter().map(|c| c.start_beat).collect();
        assert_eq!(starts, vec![0.0, 0.0, 32.0, 120.0], "sorted");
    }

    #[test]
    fn update_clip_is_clamped_and_keeps_its_pattern() {
        let mut d = doc();
        let drums = d.model().patterns[1].id;
        let mut c = d.model().clips[0].clone();
        let melody = c.pattern_id;
        c.length_beats = 0.0;
        assert_eq!(d.apply(Command::UpdateClip { clip: c.clone() }, false), Err(EditError::InvalidValue));
        c.start_beat = 128.0;
        c.length_beats = 8.0;
        assert_eq!(d.apply(Command::UpdateClip { clip: c.clone() }, false), Err(EditError::PastSongEnd));
        c.start_beat = 112.0;
        c.length_beats = 64.0;
        c.offset_beats = 4.0;
        c.pattern_id = drums;
        d.apply(Command::UpdateClip { clip: c.clone() }, false).unwrap();
        let stored = d.model().clip(&c.id).unwrap();
        assert_eq!((stored.start_beat, stored.length_beats, stored.offset_beats), (112.0, 16.0, 4.0));
        assert_eq!(stored.pattern_id, melody, "a clip never changes pattern (or track) by update");
        let ghost = Clip::new(melody, 0.0, 4.0);
        assert_eq!(d.apply(Command::UpdateClip { clip: ghost }, false), Err(EditError::NoSuchClip));
    }

    #[test]
    fn update_clips_moves_a_group_in_one_step_or_not_at_all() {
        let mut d = doc();
        let before = d.model().clone();
        let mut moved: Vec<Clip> = d.model().clips.clone();
        for c in &mut moved {
            c.start_beat = 16.0;
            c.length_beats = 32.0;
        }
        d.apply(Command::UpdateClips { clips: moved.clone() }, false).unwrap();
        assert!(d.model().clips.iter().all(|c| c.start_beat == 16.0 && c.length_beats == 32.0));
        assert_eq!(d.undo_label(), Some("edit clips"));
        assert!(d.undo());
        assert_eq!(d.model(), &before, "one undo puts the whole group back");

        moved[1].start_beat = 500.0;
        assert_eq!(d.apply(Command::UpdateClips { clips: moved }, false), Err(EditError::PastSongEnd));
        assert_eq!(d.model(), &before, "one bad clip refuses the whole move");
    }

    #[test]
    fn remove_clips_takes_a_selection_in_one_step() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        d.apply(Command::AddClip { pattern_id: pid, start_beat: 64.0, length_beats: None }, false).unwrap();
        let ids: Vec<Uuid> = d.model().clips.iter().take(2).map(|c| c.id).collect();
        d.apply(Command::RemoveClips { ids: [ids.clone(), vec![Uuid::new_v4()]].concat() }, false).unwrap();
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
        let first = d.model().clips.iter().find(|c| c.pattern_id == melody.id).unwrap().id;
        d.apply(Command::UpdateClip { clip: Clip { length_beats: 16.0, ..d.model().clip(&first).unwrap().clone() } }, false).unwrap();
        d.apply(Command::AddClip { pattern_id: melody.id, start_beat: 16.0, length_beats: None }, false).unwrap();
        let second = d.model().clips.iter().find(|c| c.start_beat == 16.0).unwrap().id;

        d.apply(Command::MakeClipUnique { id: second }, false).unwrap();
        assert_eq!(d.undo_label(), Some("make clip unique"));
        let m = d.model();
        let fork = m.pattern(&m.clip(&second).unwrap().pattern_id).unwrap();
        assert_ne!(fork.id, melody.id);
        assert_eq!(fork.track_id, melody.track_id, "the fork stays on the same track");
        assert_eq!(fork.name, "melody 1 copy");
        assert_eq!(m.patterns[1].id, fork.id, "listed right after the original");
        let musical = |p: &SongPattern| p.notes.iter().map(|n| (n.start_beat, n.pitch)).collect::<Vec<_>>();
        assert_eq!(musical(fork), musical(&melody));
        assert!(fork.notes.iter().all(|n| melody.notes.iter().all(|o| o.id != n.id)), "fresh note ids");
        assert_eq!(m.clip(&first).unwrap().pattern_id, melody.id, "the other clip still plays the original");
        assert_eq!(d.apply(Command::MakeClipUnique { id: Uuid::new_v4() }, false), Err(EditError::NoSuchClip));
    }

    #[test]
    fn set_song_length_removes_and_trims_clips_past_the_end() {
        let mut d = doc();
        let (melody, drums) = (d.model().patterns[0].id, d.model().patterns[1].id);
        d.apply(Command::AddClip { pattern_id: melody, start_beat: 48.0, length_beats: Some(16.0) }, false).unwrap();
        d.apply(Command::AddClip { pattern_id: drums, start_beat: 32.0, length_beats: Some(16.0) }, false).unwrap();
        d.apply(Command::SetSongLength { beats: 40.0 }, false).unwrap();
        let m = d.model();
        assert_eq!(m.song_length_beats, 40.0);
        let spans: Vec<(f64, f64)> = m.clips.iter().map(|c| (c.start_beat, c.end_beat())).collect();
        assert_eq!(spans, vec![(0.0, 40.0), (0.0, 40.0), (32.0, 40.0)], "the clip at 48 is gone, the rest end at 40");
        assert_eq!(d.undo_label(), Some("set song length"));

        d.apply(Command::SetSongLength { beats: 1.0 }, false).unwrap();
        assert_eq!(d.model().song_length_beats, 4.0, "at least one bar");
        assert!(d.model().clips.iter().all(|c| c.end_beat() <= 4.0));
        assert_eq!(d.apply(Command::SetSongLength { beats: f64::NAN }, false), Err(EditError::InvalidValue));
        d.undo();
        d.undo();
        assert_eq!(d.model().clips.len(), 4, "undo brings removed clips back");
    }

    #[test]
    fn sections_carry_their_own_key() {
        let mut d = doc();
        d.apply(Command::SetKey { key: KeyState::locked(0, ScaleType::Major) }, false).unwrap();
        d.apply(Command::AddSection { name: "verse".into(), start_beat: 0.0 }, false).unwrap();
        d.apply(Command::AddSection { name: "".into(), start_beat: 16.0 }, false).unwrap();
        assert_eq!(d.model().sections[1].name, "section 2");
        let mut bridge = d.model().sections[1].clone();
        bridge.key = Some(KeyState::locked(9, ScaleType::Minor));
        d.apply(Command::UpdateSection { section: bridge.clone() }, false).unwrap();
        assert_eq!(d.model().key_at(3.0).root_pitch_class, Some(0));
        assert_eq!(d.model().key_at(20.0).root_pitch_class, Some(9));
        assert_eq!(d.model().section_at(20.0).map(|s| s.name.as_str()), Some("section 2"));
        d.apply(Command::RemoveSection { id: bridge.id }, false).unwrap();
        assert_eq!(d.model().key_at(20.0).root_pitch_class, Some(0));
    }

    #[test]
    fn duplicate_pattern_gets_fresh_ids_on_the_same_track() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        d.apply(Command::SetNotes { pattern_id: pid, notes: vec![NoteEvent::new(0.0, 1.0, 60)] }, false).unwrap();
        d.apply(Command::DuplicatePattern { id: pid }, false).unwrap();
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
        d.apply(Command::SetTrackColor { id, color: Some("#FF3B30".into()) }, false).unwrap();
        assert_eq!(d.model().tracks[0].color.as_deref(), Some("#ff3b30"));
        assert_eq!(
            d.apply(Command::SetTrackColor { id, color: Some("red".into()) }, false),
            Err(EditError::InvalidValue)
        );
        d.apply(Command::SetTrackColor { id, color: None }, false).unwrap();
        assert!(d.model().tracks[0].color.is_none());
    }

    #[test]
    fn time_signature_is_validated() {
        let mut d = doc();
        d.apply(Command::SetSongLength { beats: 4.0 }, false).unwrap();
        d.apply(Command::SetTimeSignature { numerator: 7, denominator: 4 }, false).unwrap();
        assert_eq!(d.model().time_signature.beats_per_bar(), 7.0);
        assert_eq!(d.model().song_length_beats, 7.0, "the song stays at least one bar");
        assert_eq!(d.apply(Command::SetTimeSignature { numerator: 0, denominator: 4 }, false), Err(EditError::InvalidValue));
        assert_eq!(d.apply(Command::SetTimeSignature { numerator: 4, denominator: 3 }, false), Err(EditError::InvalidValue));
    }

    #[test]
    fn undo_labels_follow_the_history() {
        let mut d = doc();
        assert_eq!(d.undo_label(), None);
        d.apply(Command::SetTempo { bpm: 90.0 }, false).unwrap();
        assert_eq!(d.undo_label(), Some("set tempo"));
        let (pid, tid) = (d.model().patterns[0].id, d.model().tracks[0].id);
        d.apply_labeled(
            Command::SetNotes { pattern_id: pid, notes: vec![NoteEvent::new(0.0, 1.0, 60)] },
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
        let id = tid;
        d.apply(Command::SetTrackParam { id, param: TrackParam::Pan, value: 0.3 }, true).unwrap();
        assert_eq!(d.undo_label(), Some("change pan"));
        d.commit_gesture();
        assert_eq!(d.undo_label(), Some("change pan"));
        // An empty label falls back to the command's own.
        d.apply_labeled(Command::SetTempo { bpm: 95.0 }, false, Some("  ".into())).unwrap();
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
                    ImportedTrack { name: "piano".into(), is_drum: false, notes: vec![NoteEvent::new(0.0, 1.0, 60)] },
                    ImportedTrack { name: "kit".into(), is_drum: true, notes: vec![NoteEvent::new(0.0, 0.25, 36)] },
                ],
                length_beats: 160.0,
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
        assert_eq!(kit_patterns[0].length_beats, 160.0);
        assert_eq!(kit_patterns[0].notes.len(), 1);
        let clip = m.clips.iter().find(|c| c.pattern_id == kit_patterns[0].id).unwrap();
        assert_eq!((clip.start_beat, clip.length_beats), (0.0, 160.0));
        assert_eq!(m.song_length_beats, 160.0, "the song grows to fit");
        assert_eq!(d.undo_label(), Some("import MIDI"));
        d.undo();
        assert_eq!(d.model(), &before);
        assert_eq!(
            d.apply(Command::ImportTracks { tracks: vec![], length_beats: 16.0 }, false),
            Err(EditError::InvalidValue)
        );
    }

    #[test]
    fn tempo_points_are_edited_one_per_beat() {
        let mut d = doc();
        d.apply(Command::AddTempoPoint { beat: 16.0, bpm: 90.0, ramp: false }, false).unwrap();
        d.apply(Command::AddTempoPoint { beat: 16.0, bpm: 95.0, ramp: true }, false).unwrap();
        assert_eq!(d.model().tempo_points.len(), 1);
        assert_eq!(d.model().tempo_points[0].bpm, 95.0);
        assert!(d.model().tempo_points[0].ramp);
        assert_eq!(d.model().tempo_map().bpm_at(20.0), 95.0);
        let mut p = d.model().tempo_points[0].clone();
        p.beat = 8.0;
        p.bpm = 999.0;
        d.apply(Command::UpdateTempoPoint { point: p.clone() }, false).unwrap();
        assert_eq!(d.model().tempo_points[0].bpm, 300.0);
        assert_eq!(d.model().tempo_points[0].beat, 8.0);
        d.apply(Command::RemoveTempoPoint { id: p.id }, false).unwrap();
        assert!(d.model().tempo_points.is_empty());
        assert_eq!(d.apply(Command::RemoveTempoPoint { id: p.id }, false), Err(EditError::NoSuchTempoPoint));
        assert_eq!(d.apply(Command::AddTempoPoint { beat: -1.0, bpm: 100.0, ramp: false }, false), Err(EditError::InvalidValue));
    }

    #[test]
    fn swing_is_validated() {
        let mut d = doc();
        d.apply(Command::SetSwing { swing: 90.0, grid: 0.25 }, false).unwrap();
        assert_eq!(d.model().swing, 75.0);
        assert_eq!(d.model().swing_grid, 0.25);
        assert_eq!(d.apply(Command::SetSwing { swing: 60.0, grid: 0.3 }, false), Err(EditError::InvalidValue));
    }

    #[test]
    fn intentional_flag_survives_set_notes() {
        let mut d = doc();
        let pid = d.model().patterns[0].id;
        let mut n = NoteEvent::new(0.0, 1.0, 65);
        n.intentional = true;
        d.apply(Command::SetNotes { pattern_id: pid, notes: vec![n] }, false).unwrap();
        assert!(d.model().patterns[0].notes[0].intentional);
    }
}
