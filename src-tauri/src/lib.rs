//! The Tauri shell: owns the [`Document`] and the [`AudioDevice`], exposes commands to the UI,
//! and streams the playhead back as events. All model edits go through `apply`, which keeps
//! undo/redo in one place and re-feeds the engine a fresh [`Sequence`] after every change.

pub mod settings;

use dissonant_core::{midi_file, Command, Document, MasterSettings, MidiScope, NoteEvent, ProjectModel, Sequence};
use dissonant_engine::{default_output_name, list_output_devices, render_wav, AudioConfig, AudioDevice, EngineCommand, RenderOptions};
use settings::{NewProjectKind, Settings};
use midir::{MidiInput, MidiInputConnection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WindowEvent};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PlayMode {
    #[default]
    Pattern,
    Song,
}

struct PlaybackContext {
    mode: PlayMode,
    /// The pattern being edited (pattern mode plays it; recording writes into it).
    pattern_id: Option<Uuid>,
    /// The song beat the edited pattern's beat 0 sits on, for the chords it hears and tiers
    /// against (0 = from song bar 1).
    chord_offset_beats: f64,
    /// Another pattern played along in pattern mode (the ghost the user chose to hear), and
    /// how loud (1 = the track's own level).
    ghost_pattern_id: Option<Uuid>,
    ghost_level: f64,
    hear_chords: bool,
    /// The track live input (MIDI / typing keyboard) plays and records into.
    live_track: Option<Uuid>,
    /// Mirrors of engine transport settings, re-sent when the audio device restarts.
    looping: bool,
    loop_region: Option<(f64, f64)>,
    metronome: bool,
    metronome_volume: f32,
}

impl Default for PlaybackContext {
    fn default() -> Self {
        PlaybackContext {
            mode: PlayMode::Pattern,
            pattern_id: None,
            chord_offset_beats: 0.0,
            ghost_pattern_id: None,
            ghost_level: 1.0,
            hear_chords: false,
            live_track: None,
            looping: true,
            loop_region: None,
            metronome: false,
            metronome_volume: 0.6,
        }
    }
}

/// Record-arm state. Held notes are keyed by (track, pitch) with their start beat + velocity.
#[derive(Default)]
struct Recorder {
    armed: bool,
    /// Grid to snap recorded note starts to (0 = off).
    quantize: f64,
    held: HashMap<(Uuid, u8), (f64, u8)>,
}

pub struct AppState {
    doc: Mutex<Document>,
    audio: Mutex<Option<AudioDevice>>,
    audio_error: Mutex<Option<String>>,
    playback: Mutex<PlaybackContext>,
    exporting: AtomicBool,
    midi: Mutex<Option<MidiInputConnection<()>>>,
    midi_port: Mutex<Option<String>>,
    midi_last_ms: AtomicU64,
    recorder: Mutex<Recorder>,
    /// Tauri app-data dir (recent files, untitled autosaves). Set once in `setup`.
    data_dir: Mutex<Option<PathBuf>>,
    /// Document revision the last autosave captured.
    autosaved_revision: AtomicU64,
    /// The autosave file currently on disk for this document, if any.
    autosave_path: Mutex<Option<PathBuf>>,
    /// Set by `quit` so the close handler lets the window go.
    force_close: AtomicBool,
    settings: Mutex<Settings>,
}

/// What every editing command returns to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub model: ProjectModel,
    pub can_undo: bool,
    pub can_redo: bool,
    pub dirty: bool,
    pub path: Option<String>,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
}

/// An autosave that may hold work newer than its project file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCandidate {
    pub autosave_path: String,
    pub project_path: Option<String>,
    pub saved_at_ms: u64,
}

/// The on-disk autosave format: the model plus where it belongs. The model is kept as raw
/// JSON so an autosave from an older schema is still listed, and refused on restore with the
/// same message as opening an old file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutosaveFile {
    project_path: Option<String>,
    saved_at_ms: u64,
    model: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatusEvent {
    pub status: AudioStatus,
    pub message: String,
}

const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);
const MAX_RECENT: usize = 12;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayheadEvent {
    pub beat: f64,
    pub playing: bool,
    pub master_peak: [f32; 2],
    pub track_peaks: Vec<f32>,
    /// Beats of count-in left (0 when not counting in).
    pub count_in: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStatus {
    pub running: bool,
    pub sample_rate: Option<u32>,
    pub error: Option<String>,
    pub device_name: Option<String>,
    /// The fixed buffer size in force, if one was requested and accepted.
    pub buffer_size: Option<u32>,
    /// Frames per callback as last observed (what latency really is).
    pub block_frames: u32,
    /// Output latency implied by the block size, in ms.
    pub latency_ms: f64,
    /// 0…1+: render time over block time, smoothed.
    pub load: f32,
    pub xruns: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    pub name: String,
    pub is_default: bool,
    pub sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
}

impl AppState {
    fn new() -> Self {
        AppState {
            doc: Mutex::new(Document::new(ProjectModel::starter())),
            audio: Mutex::new(None),
            audio_error: Mutex::new(None),
            playback: Mutex::new(PlaybackContext::default()),
            exporting: AtomicBool::new(false),
            midi: Mutex::new(None),
            midi_port: Mutex::new(None),
            midi_last_ms: AtomicU64::new(0),
            recorder: Mutex::new(Recorder::default()),
            data_dir: Mutex::new(None),
            autosaved_revision: AtomicU64::new(0),
            autosave_path: Mutex::new(None),
            force_close: AtomicBool::new(false),
            settings: Mutex::new(Settings::default()),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn settings_path(&self) -> Option<PathBuf> {
        self.data_dir().map(|d| d.join("settings.json"))
    }

    fn audio_config(&self) -> AudioConfig {
        let a = self.settings.lock().unwrap().audio.clone();
        AudioConfig {
            device: a.device,
            sample_rate: a.sample_rate,
            buffer_size: a.buffer_size,
        }
    }

    fn live_track(&self) -> Option<Uuid> {
        let ctx = self.playback.lock().unwrap();
        ctx.live_track
            .filter(|id| self.doc.lock().unwrap().model().track(id).is_some())
            .or_else(|| self.doc.lock().unwrap().model().tracks.first().map(|t| t.id))
    }

    fn engine_position(&self) -> Option<(f64, bool)> {
        let audio = self.audio.lock().unwrap();
        audio.as_ref().map(|a| (a.shared().position_beats(), a.shared().is_playing()))
    }

    fn snapshot(&self) -> Snapshot {
        let doc = self.doc.lock().unwrap();
        Snapshot {
            model: doc.model().clone(),
            can_undo: doc.can_undo(),
            can_redo: doc.can_redo(),
            dirty: doc.is_dirty(),
            path: doc.path.as_ref().map(|p| p.display().to_string()),
            undo_label: doc.undo_label().map(str::to_string),
            redo_label: doc.redo_label().map(str::to_string),
        }
    }

    fn audio_status(&self) -> AudioStatus {
        let audio = self.audio.lock().unwrap();
        let shared = audio.as_ref().map(|a| a.shared());
        let block = shared.map(|s| s.block_frames()).unwrap_or(0);
        let sr = audio.as_ref().map(|a| a.sample_rate).unwrap_or(0);
        AudioStatus {
            running: audio.is_some(),
            sample_rate: audio.as_ref().map(|a| a.sample_rate),
            error: self.audio_error.lock().unwrap().clone(),
            device_name: audio.as_ref().map(|a| a.device_name.clone()),
            buffer_size: audio.as_ref().and_then(|a| a.buffer_size),
            block_frames: block,
            latency_ms: if sr > 0 { block as f64 * 1000.0 / sr as f64 } else { 0.0 },
            load: shared.map(|s| s.load()).unwrap_or(0.0),
            xruns: shared.map(|s| s.xruns()).unwrap_or(0),
        }
    }

    /// Re-send the transport settings the engine forgets when its device is rebuilt.
    fn resend_transport(&self) {
        let (hear_chords, looping, loop_region, metronome, volume) = {
            let ctx = self.playback.lock().unwrap();
            (ctx.hear_chords, ctx.looping, ctx.loop_region, ctx.metronome, ctx.metronome_volume)
        };
        self.send(EngineCommand::SetHearChords(hear_chords));
        self.send(EngineCommand::SetLooping(looping));
        self.send(EngineCommand::SetMetronome { on: metronome, volume });
        match loop_region {
            Some((start, end)) => self.send(EngineCommand::SetLoop { start, end }),
            None => self.send(EngineCommand::ClearLoop),
        }
    }

    /// Tear the device down and open the current default again, keeping the transport where
    /// it was. Returns the new status.
    fn restart_audio(&self) -> AudioStatus {
        let was = self.engine_position();
        *self.audio.lock().unwrap() = None;
        self.start_audio();
        self.sync_engine();
        self.resend_transport();
        if let Some((beat, playing)) = was {
            self.send(EngineCommand::Seek { beat });
            if playing {
                self.send(EngineCommand::Play);
            }
        }
        self.audio_status()
    }

    // ── Files on disk: recent list and autosave ───────────────────────────────────────────

    fn data_dir(&self) -> Option<PathBuf> {
        self.data_dir.lock().unwrap().clone()
    }

    fn recent_path(&self) -> Option<PathBuf> {
        self.data_dir().map(|d| d.join("recent.json"))
    }

    fn load_recent(&self) -> Vec<String> {
        let Some(path) = self.recent_path() else { return vec![] };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
            .unwrap_or_default()
    }

    fn save_recent(&self, list: &[String]) {
        if let Some(path) = self.recent_path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_string_pretty(list) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    fn remember_recent(&self, file: &std::path::Path) {
        let shown = file.display().to_string();
        let mut list = self.load_recent();
        list.retain(|p| p != &shown);
        list.insert(0, shown);
        list.truncate(MAX_RECENT);
        self.save_recent(&list);
    }

    /// Where this document's autosave goes: next to the project, or in app data for untitled.
    fn autosave_target(&self, project: Option<&std::path::Path>) -> Option<PathBuf> {
        match project {
            Some(p) => Some(autosave_path_for(p)),
            None => self.data_dir().map(|d| d.join("autosave").join("untitled.autosave.json")),
        }
    }

    /// Write an autosave if the document changed since the last one. Returns the path written.
    fn autosave(&self) -> Option<PathBuf> {
        let (json, path, revision) = {
            let doc = self.doc.lock().unwrap();
            if !doc.is_dirty() || doc.revision() == self.autosaved_revision.load(Ordering::Relaxed) {
                return None;
            }
            let target = self.autosave_target(doc.path.as_deref())?;
            let file = AutosaveFile {
                project_path: doc.path.as_ref().map(|p| p.display().to_string()),
                saved_at_ms: now_ms(),
                model: serde_json::to_value(doc.model()).ok()?,
            };
            (serde_json::to_string(&file).ok()?, target, doc.revision())
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
            let _ = std::fs::remove_file(&tmp);
            log::warn!("autosave failed: {}", path.display());
            return None;
        }
        self.autosaved_revision.store(revision, Ordering::Relaxed);
        *self.autosave_path.lock().unwrap() = Some(path.clone());
        Some(path)
    }

    /// Delete this document's autosave (after a real save, or when the document is replaced).
    fn clear_autosave(&self) {
        if let Some(path) = self.autosave_path.lock().unwrap().take() {
            let _ = std::fs::remove_file(path);
        }
        let revision = self.doc.lock().unwrap().revision();
        self.autosaved_revision.store(revision, Ordering::Relaxed);
    }

    /// Autosaves lying around from an earlier run: the untitled slot plus one next to any
    /// recent project, when it is newer than the project itself.
    fn recovery_candidates(&self) -> Vec<RecoveryCandidate> {
        let mut out = Vec::new();
        let mut consider = |autosave: PathBuf| {
            let Ok(json) = std::fs::read_to_string(&autosave) else { return };
            let Ok(file) = serde_json::from_str::<AutosaveFile>(&json) else { return };
            if let Some(project) = &file.project_path {
                // Skip autosaves older than the project they belong to (a stale leftover).
                let project_ms = std::fs::metadata(project)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64);
                if matches!(project_ms, Some(ms) if ms >= file.saved_at_ms) {
                    let _ = std::fs::remove_file(&autosave);
                    return;
                }
            }
            out.push(RecoveryCandidate {
                autosave_path: autosave.display().to_string(),
                project_path: file.project_path,
                saved_at_ms: file.saved_at_ms,
            });
        };
        if let Some(untitled) = self.autosave_target(None) {
            if untitled.exists() {
                consider(untitled);
            }
        }
        for recent in self.load_recent() {
            let path = autosave_path_for(std::path::Path::new(&recent));
            if path.exists() {
                consider(path);
            }
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.saved_at_ms));
        out
    }

    /// Replace the document and forget the previous one's autosave.
    fn load_model(&self, model: ProjectModel, path: Option<PathBuf>, keep_dirty: bool) {
        self.clear_autosave();
        {
            let mut doc = self.doc.lock().unwrap();
            if keep_dirty {
                doc.restore(model, path);
            } else {
                doc.replace(model, path);
            }
        }
        {
            let mut ctx = self.playback.lock().unwrap();
            ctx.pattern_id = None;
            ctx.chord_offset_beats = 0.0;
            ctx.ghost_pattern_id = None;
        }
        self.send(EngineCommand::Stop);
        self.send(EngineCommand::Seek { beat: 0.0 });
        self.sync_engine();
    }

    fn send(&self, cmd: EngineCommand) {
        if let Some(audio) = self.audio.lock().unwrap().as_ref() {
            if let Err(e) = audio.send(cmd) {
                log::warn!("engine command dropped: {e}");
            }
        }
    }

    /// Rebuild the engine's sequence from the current model and playback context.
    fn sync_engine(&self) {
        let seq = {
            let doc = self.doc.lock().unwrap();
            let ctx = self.playback.lock().unwrap();
            build_sequence(doc.model(), ctx.mode, ctx.pattern_id, ctx.chord_offset_beats, ctx.ghost_pattern_id.map(|g| (g, ctx.ghost_level)))
        };
        let master = self.doc.lock().unwrap().model().master.clone();
        self.send(EngineCommand::SetSequence(Arc::new(seq)));
        self.send(EngineCommand::SetMaster(master));
    }

    fn start_audio(&self) {
        let master = self.doc.lock().unwrap().model().master.clone();
        match AudioDevice::start_with(master, self.audio_config()) {
            Ok(device) => {
                *self.audio.lock().unwrap() = Some(device);
                *self.audio_error.lock().unwrap() = None;
            }
            Err(e) => {
                log::error!("audio device failed to start: {e}");
                *self.audio_error.lock().unwrap() = Some(e.to_string());
            }
        }
    }
}

/// `song.dissonant` → `song.dissonant.autosave.json`, next to the project.
fn autosave_path_for(project: &std::path::Path) -> PathBuf {
    let mut name = project.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "project".into());
    name.push(".autosave.json");
    project.with_file_name(name)
}

/// Song mode (SETUP and SONG in the UI) plays the whole song, empty or not. Pattern mode plays
/// the context pattern alone against the chords from `chord_offset_beats`; with no pattern (or
/// one that is gone) it plays nothing, but every track is still there for live input. A ghost
/// pattern, when given, loops along on its own track (live listening only; exports pass none).
fn build_sequence(
    model: &ProjectModel,
    mode: PlayMode,
    pattern_id: Option<Uuid>,
    chord_offset_beats: f64,
    ghost: Option<(Uuid, f64)>,
) -> Sequence {
    match mode {
        PlayMode::Song => Sequence::from_song(model),
        PlayMode::Pattern => pattern_id
            .and_then(|id| Sequence::from_pattern(model, &id, chord_offset_beats))
            .map(|seq| match ghost.filter(|(g, _)| Some(*g) != pattern_id) {
                Some((id, level)) => seq.with_ghost(model, &id, level),
                None => seq,
            })
            .unwrap_or_else(|| Sequence::silent(model, model.bar_beats() * 4.0)),
    }
}

/// The message for a project file that would not open. The old-schema refusal is shown as is.
fn open_error(e: serde_json::Error) -> String {
    let message = e.to_string();
    if message == dissonant_core::OLD_SCHEMA_ERROR {
        message
    } else {
        format!("not a dissonant project: {message}")
    }
}

// ─── Document commands ───────────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_state(state: State<'_, AppState>) -> Snapshot {
    state.snapshot()
}

#[tauri::command]
fn apply(state: State<'_, AppState>, command: Command, transient: Option<bool>, label: Option<String>) -> Result<Snapshot, String> {
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(command, transient.unwrap_or(false), label)
        .map_err(|e| e.to_string())?;
    state.sync_engine();
    Ok(state.snapshot())
}

#[tauri::command]
fn commit_gesture(state: State<'_, AppState>) -> Snapshot {
    state.doc.lock().unwrap().commit_gesture();
    state.snapshot()
}

#[tauri::command]
fn undo(state: State<'_, AppState>) -> Snapshot {
    state.doc.lock().unwrap().undo();
    state.sync_engine();
    state.snapshot()
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> Snapshot {
    state.doc.lock().unwrap().redo();
    state.sync_engine();
    state.snapshot()
}

/// A new project shaped by the editing defaults (tempo, time signature, pattern length, song
/// length, starter vs empty). An explicit `starter` overrides the setting.
#[tauri::command]
fn new_project(state: State<'_, AppState>, starter: Option<bool>) -> Snapshot {
    let e = state.settings().editing;
    let starter = starter.unwrap_or(e.new_project == NewProjectKind::Starter);
    let bar = e.default_time_signature.beats_per_bar();
    let beats = (e.default_pattern_bars.max(1) as f64) * bar;
    let song_bars = e.default_song_bars.max(1);
    // The starter is built at the default size, so its melody and groove fill the pattern.
    let mut model = if starter { ProjectModel::starter_shaped(beats, bar, song_bars) } else { ProjectModel::empty() };
    model.tempo = e.default_tempo;
    model.time_signature = e.default_time_signature;
    model.song_length_beats = song_bars as f64 * bar;
    for p in &mut model.patterns {
        p.length_beats = beats;
    }
    state.load_model(model, None, false);
    state.snapshot()
}

#[tauri::command]
fn open_project(state: State<'_, AppState>, path: String) -> Result<Snapshot, String> {
    let json = std::fs::read_to_string(&path).map_err(|e| format!("could not read {path}: {e}"))?;
    let model = ProjectModel::from_json(&json).map_err(open_error)?;
    let file = PathBuf::from(&path);
    state.remember_recent(&file);
    state.load_model(model, Some(file), false);
    Ok(state.snapshot())
}

#[tauri::command]
fn save_project(state: State<'_, AppState>, path: Option<String>) -> Result<Snapshot, String> {
    let mut doc = state.doc.lock().unwrap();
    let target = match path {
        Some(p) => PathBuf::from(p),
        None => doc.path.clone().ok_or_else(|| "no file path yet — use Save As".to_string())?,
    };
    let json = doc.model().to_json().map_err(|e| e.to_string())?;
    std::fs::write(&target, json).map_err(|e| format!("could not write {}: {e}", target.display()))?;
    doc.path = Some(target.clone());
    doc.mark_saved();
    drop(doc);
    state.remember_recent(&target);
    state.clear_autosave();
    // A stale autosave next to the file would otherwise be offered on the next launch.
    let _ = std::fs::remove_file(autosave_path_for(&target));
    Ok(state.snapshot())
}

// ─── Window, recent files, recovery ─────────────────────────────────────────────────────────

/// Close the app for real (the close handler asks first while the document is dirty).
#[tauri::command]
fn quit(app: AppHandle, state: State<'_, AppState>) {
    state.force_close.store(true, Ordering::SeqCst);
    state.clear_autosave();
    app.exit(0);
}

#[tauri::command]
fn recent_files(state: State<'_, AppState>) -> Vec<String> {
    state.load_recent().into_iter().filter(|p| std::path::Path::new(p).exists()).collect()
}

#[tauri::command]
fn clear_recent(state: State<'_, AppState>) {
    state.save_recent(&[]);
}

#[tauri::command]
fn recovery_candidates(state: State<'_, AppState>) -> Vec<RecoveryCandidate> {
    state.recovery_candidates()
}

/// Load an autosave as the current (dirty) document, bound to its original path if it had one.
#[tauri::command]
fn restore_autosave(state: State<'_, AppState>, autosave_path: String) -> Result<Snapshot, String> {
    let json = std::fs::read_to_string(&autosave_path).map_err(|e| format!("could not read autosave: {e}"))?;
    let file: AutosaveFile = serde_json::from_str(&json).map_err(|e| format!("not an autosave: {e}"))?;
    let model = ProjectModel::from_value(file.model).map_err(open_error)?;
    let path = file.project_path.map(PathBuf::from);
    state.load_model(model, path, true);
    // The restored document now owns this autosave file.
    *state.autosave_path.lock().unwrap() = Some(PathBuf::from(autosave_path));
    Ok(state.snapshot())
}

#[tauri::command]
fn discard_autosave(autosave_path: String) {
    let _ = std::fs::remove_file(autosave_path);
}

/// Write the autosave now (the UI calls this before risky operations; the timer covers the rest).
#[tauri::command]
fn autosave_now(state: State<'_, AppState>) -> Option<String> {
    state.autosave().map(|p| p.display().to_string())
}

// ─── Transport ───────────────────────────────────────────────────────────────────────────────

/// What ▶ plays: the song, or one pattern heard against the project chords from song beat
/// `chord_offset_beats` (default 0, song bar 1), with `ghost_pattern_id` looping along when the
/// user wants to hear it, at `ghost_level` (default 1). Switching mode or pattern rewinds;
/// moving only the chord offset or changing the ghost does not.
#[tauri::command]
fn set_playback_context(
    state: State<'_, AppState>,
    mode: PlayMode,
    pattern_id: Option<Uuid>,
    chord_offset_beats: Option<f64>,
    ghost_pattern_id: Option<Uuid>,
    ghost_level: Option<f64>,
) {
    let offset = chord_offset_beats.filter(|b| b.is_finite()).unwrap_or(0.0).max(0.0);
    {
        let mut ctx = state.playback.lock().unwrap();
        let changed = ctx.mode != mode || ctx.pattern_id != pattern_id;
        ctx.mode = mode;
        ctx.pattern_id = pattern_id;
        ctx.chord_offset_beats = offset;
        ctx.ghost_pattern_id = ghost_pattern_id;
        ctx.ghost_level = ghost_level.filter(|l| l.is_finite()).unwrap_or(1.0).clamp(0.0, 1.5);
        if changed {
            state.send(EngineCommand::Seek { beat: 0.0 });
        }
    }
    state.sync_engine();
}

/// Start playing. With `count_in_bars > 0` the metronome counts that many bars first and
/// the transport starts on the downbeat after.
#[tauri::command]
fn transport_play(state: State<'_, AppState>, count_in_bars: Option<u32>) {
    match count_in_bars {
        Some(bars) if bars > 0 => state.send(EngineCommand::PlayWithCountIn { bars }),
        _ => state.send(EngineCommand::Play),
    }
}

#[tauri::command]
fn set_metronome(state: State<'_, AppState>, on: bool, volume: Option<f32>) {
    let (on, volume) = {
        let mut ctx = state.playback.lock().unwrap();
        ctx.metronome = on;
        if let Some(v) = volume {
            ctx.metronome_volume = v.clamp(0.0, 1.0);
        }
        (ctx.metronome, ctx.metronome_volume)
    };
    state.send(EngineCommand::SetMetronome { on, volume });
}

#[tauri::command]
fn transport_stop(state: State<'_, AppState>) {
    state.send(EngineCommand::Stop);
}

#[tauri::command]
fn transport_seek(state: State<'_, AppState>, beat: f64) {
    state.send(EngineCommand::Seek { beat });
}

#[tauri::command]
fn set_hear_chords(state: State<'_, AppState>, on: bool) {
    state.playback.lock().unwrap().hear_chords = on;
    state.send(EngineCommand::SetHearChords(on));
}

#[tauri::command]
fn audition(state: State<'_, AppState>, track_id: Uuid, pitch: u8, velocity: Option<u8>, seconds: Option<f32>) {
    state.send(EngineCommand::Audition {
        track_id,
        pitch,
        velocity: velocity.unwrap_or(100),
        seconds: seconds.unwrap_or(0.25),
    });
}

#[tauri::command]
fn audition_off(state: State<'_, AppState>, track_id: Uuid, pitch: u8) {
    state.send(EngineCommand::AuditionOff { track_id, pitch });
}

// ─── Sound preview (the SOUND page) ──────────────────────────────────────────────────────────

/// Load a sound into the engine's preview channel, or with `None` release and close it. A drum
/// is rendered here, at the device rate, so the audio thread only swaps a buffer.
#[tauri::command]
fn preview_sound(state: State<'_, AppState>, sound: Option<dissonant_core::Sound>) {
    let Some(rate) = state.audio.lock().unwrap().as_ref().map(|a| a.sample_rate as f32) else { return };
    let patch = sound.map(|s| dissonant_engine::PreviewPatch::from_sound(&s.sanitized().patch, rate));
    state.send(EngineCommand::SetPreview(patch));
}

#[tauri::command]
fn preview_note(state: State<'_, AppState>, pitch: u8, velocity: u8, on: bool) {
    let pitch = pitch.min(127);
    state.send(if on {
        EngineCommand::PreviewNoteOn { pitch, velocity: velocity.clamp(1, 127) }
    } else {
        EngineCommand::PreviewNoteOff { pitch }
    });
}

/// ~512 recent samples of the preview channel, aligned to a rising zero crossing.
#[tauri::command]
fn preview_scope(state: State<'_, AppState>) -> Vec<f32> {
    match state.audio.lock().unwrap().as_ref() {
        Some(audio) => audio.shared().preview_scope(),
        None => vec![0.0; dissonant_engine::SCOPE_SAMPLES],
    }
}

#[tauri::command]
fn audio_status(state: State<'_, AppState>) -> AudioStatus {
    state.audio_status()
}

#[tauri::command]
fn restart_audio(state: State<'_, AppState>) -> AudioStatus {
    state.restart_audio()
}

// ─── Live input: MIDI + typing keyboard, with record-arm ──────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiStatus {
    pub inputs: Vec<String>,
    pub open: Option<String>,
    pub armed: bool,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn midi_inputs() -> Vec<String> {
    match MidiInput::new("dissonant-scan") {
        Ok(input) => input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect(),
        Err(_) => vec![],
    }
}

/// A note started (controller or typing keyboard): sound it on the live track and, when
/// armed and playing, remember when it started.
fn live_note_on(app: &AppHandle, pitch: u8, velocity: u8) {
    let state = app.state::<AppState>();
    let Some(track_id) = state.live_track() else { return };
    let velocity = {
        let s = state.settings.lock().unwrap();
        s.midi.velocity_curve.apply(velocity, s.editing.default_velocity)
    };
    state.send(EngineCommand::NoteOn { track_id, pitch, velocity });
    state.midi_last_ms.store(now_ms(), Ordering::Relaxed);
    let mut rec = state.recorder.lock().unwrap();
    if rec.armed {
        if let Some((beat, true)) = state.engine_position() {
            rec.held.insert((track_id, pitch), (beat, velocity));
        }
    }
    let _ = app.emit("midi-activity", pitch);
}

/// A note ended: release it and, if it was being recorded, write it into the context pattern.
/// Only a pattern of the live track can take it; playing another track records nothing.
fn live_note_off(app: &AppHandle, pitch: u8) {
    let state = app.state::<AppState>();
    let Some(track_id) = state.live_track() else { return };
    state.send(EngineCommand::NoteOff { track_id, pitch });
    let held = state.recorder.lock().unwrap().held.remove(&(track_id, pitch));
    let Some((start, velocity)) = held else { return };
    let Some((end, _)) = state.engine_position() else { return };
    let quantize = state.recorder.lock().unwrap().quantize;

    let (pattern_id, length_beats, existing) = {
        let ctx = state.playback.lock().unwrap();
        let doc = state.doc.lock().unwrap();
        let model = doc.model();
        let Some(p) = ctx.pattern_id.and_then(|id| model.pattern(&id)).filter(|p| p.track_id == track_id) else {
            return;
        };
        (p.id, p.length_beats, p.notes.clone())
    };
    // In song mode the engine position is a song position; fold it into the pattern.
    let start_in = start % length_beats;
    let mut len = end - start;
    if len < 0.0 {
        len += length_beats; // the loop wrapped while the key was held
    }
    let grid = if quantize > 0.0 { quantize } else { 1.0 / 32.0 };
    let snapped_start = (start_in / grid).round() * grid;
    let snapped_len = ((len / grid).round() * grid).max(grid);
    let note = NoteEvent::new(snapped_start.min(length_beats - grid).max(0.0), snapped_len, pitch as i32).with_velocity(velocity as i32);

    let mut notes = existing;
    // Replace a note that starts on the same pitch and grid slot instead of stacking.
    notes.retain(|n| !(n.pitch == note.pitch && (n.start_beat - note.start_beat).abs() < grid / 2.0));
    notes.push(note);
    let applied = state
        .doc
        .lock()
        .unwrap()
        .apply(Command::SetNotes { pattern_id, notes }, false);
    if applied.is_ok() {
        state.sync_engine();
        let _ = app.emit("document", state.snapshot());
    }
}

impl AppState {
    fn midi_status(&self) -> MidiStatus {
        MidiStatus {
            inputs: midi_inputs(),
            open: self.midi_port.lock().unwrap().clone(),
            armed: self.recorder.lock().unwrap().armed,
        }
    }
}

#[tauri::command]
fn midi_status(state: State<'_, AppState>) -> MidiStatus {
    state.midi_status()
}

#[tauri::command]
fn open_midi_input(app: AppHandle, state: State<'_, AppState>, name: Option<String>) -> Result<MidiStatus, String> {
    open_midi(&app, &state, name)
}

fn open_midi(app: &AppHandle, state: &AppState, name: Option<String>) -> Result<MidiStatus, String> {
    let input = MidiInput::new("dissonant").map_err(|e| e.to_string())?;
    let ports = input.ports();
    let port = match &name {
        Some(n) => ports.iter().find(|p| input.port_name(p).ok().as_deref() == Some(n.as_str())),
        None => ports.first(),
    }
    .cloned()
    .ok_or_else(|| "no MIDI input found".to_string())?;
    let port_name = input.port_name(&port).unwrap_or_else(|_| "midi".into());
    let handle = app.clone();
    let conn = input
        .connect(
            &port,
            "dissonant-in",
            move |_stamp, message, _| {
                if message.len() < 3 {
                    return;
                }
                let (channel_filter, octave) = {
                    let s = handle.state::<AppState>();
                    let s = s.settings.lock().unwrap();
                    (s.midi.channel, s.midi.octave_offset)
                };
                if let Some(ch) = channel_filter {
                    if (message[0] & 0x0F) + 1 != ch {
                        return;
                    }
                }
                let pitch = (message[1] as i32 + octave as i32 * 12).clamp(0, 127) as u8;
                match message[0] & 0xF0 {
                    0x90 if message[2] > 0 => live_note_on(&handle, pitch, message[2]),
                    0x80 | 0x90 => live_note_off(&handle, pitch),
                    _ => {}
                }
            },
            (),
        )
        .map_err(|e| e.to_string())?;
    *state.midi.lock().unwrap() = Some(conn);
    *state.midi_port.lock().unwrap() = Some(port_name);
    Ok(state.midi_status())
}

#[tauri::command]
fn close_midi_input(state: State<'_, AppState>) -> MidiStatus {
    *state.midi.lock().unwrap() = None;
    *state.midi_port.lock().unwrap() = None;
    midi_status(state)
}

/// Keeps the default MIDI input open: reopens it when it reappears and drops the connection
/// (telling the UI) when the device goes away. Runs every few seconds.
fn midi_housekeeping(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (wanted, auto) = {
        let s = state.settings.lock().unwrap();
        (s.midi.default_input.clone(), s.midi.auto_reconnect)
    };
    let open = state.midi_port.lock().unwrap().clone();
    let inputs = midi_inputs();
    if let Some(name) = &open {
        if !inputs.contains(name) {
            *state.midi.lock().unwrap() = None;
            *state.midi_port.lock().unwrap() = None;
            let _ = app.emit("midi-status", state.midi_status());
            return;
        }
    }
    if auto && open.is_none() {
        if let Some(name) = wanted.filter(|n| inputs.contains(n)) {
            if open_midi(app, &state, Some(name)).is_ok() {
                let _ = app.emit("midi-status", state.midi_status());
            }
        }
    }
}

#[tauri::command]
fn set_live_track(state: State<'_, AppState>, track_id: Option<Uuid>) {
    state.playback.lock().unwrap().live_track = track_id;
}

#[tauri::command]
fn set_record(state: State<'_, AppState>, armed: bool, quantize: Option<f64>) -> MidiStatus {
    let mut rec = state.recorder.lock().unwrap();
    rec.armed = armed;
    if let Some(q) = quantize {
        rec.quantize = q.max(0.0);
    }
    if !armed {
        rec.held.clear();
    }
    drop(rec);
    midi_status(state)
}

#[tauri::command]
fn note_on(app: AppHandle, pitch: u8, velocity: Option<u8>) {
    live_note_on(&app, pitch, velocity.unwrap_or(100));
}

#[tauri::command]
fn note_off(app: AppHandle, pitch: u8) {
    live_note_off(&app, pitch);
}

#[tauri::command]
fn set_loop(state: State<'_, AppState>, start: f64, end: f64) {
    state.playback.lock().unwrap().loop_region = if end > start { Some((start, end)) } else { None };
    state.send(EngineCommand::SetLoop { start, end });
}

#[tauri::command]
fn clear_loop(state: State<'_, AppState>) {
    state.playback.lock().unwrap().loop_region = None;
    state.send(EngineCommand::ClearLoop);
}

#[tauri::command]
fn set_looping(state: State<'_, AppState>, on: bool) {
    state.playback.lock().unwrap().looping = on;
    state.send(EngineCommand::SetLooping(on));
}

// ─── Export ──────────────────────────────────────────────────────────────────────────────────

/// Everything the export dialog decides. Fields left `None` fall back to the export settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub path: String,
    /// "song" or "pattern" (the selected pattern).
    pub scope: String,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u16>,
    pub dither: Option<bool>,
    /// `Some(db)` normalizes to that peak; `None` leaves the level alone.
    pub normalize_db: Option<f32>,
    pub tail_seconds: Option<f64>,
    pub loops: Option<u32>,
    /// Also write one WAV per track next to the mix (`<name> - <track>.wav`).
    pub stems: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub done: u32,
    pub total: u32,
    pub current: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub files: Vec<String>,
}

/// Render to WAV faster than real time on a worker thread: the mix, and with `stems` one file
/// per track with everything else muted (for a pattern, just its own track). The chord bed is
/// a monitor and is never rendered. Progress goes out as `export-progress` events.
#[tauri::command]
async fn export_wav(app: AppHandle, state: State<'_, AppState>, request: ExportRequest) -> Result<ExportResult, String> {
    if state.exporting.swap(true, Ordering::SeqCst) {
        return Err("an export is already running".into());
    }
    let x = state.settings().export;
    let (model, pattern_id, offset) = {
        let doc = state.doc.lock().unwrap();
        let ctx = state.playback.lock().unwrap();
        (doc.model().clone(), ctx.pattern_id, ctx.chord_offset_beats)
    };
    let song = request.scope != "pattern" && !model.clips.is_empty();
    let mode = if song { PlayMode::Song } else { PlayMode::Pattern };
    let build = move |m: &ProjectModel| build_sequence(m, mode, pattern_id, offset, None);
    let pattern_track = pattern_id.and_then(|id| model.pattern(&id)).map(|p| p.track_id);
    if !song && pattern_track.is_none() {
        state.exporting.store(false, Ordering::SeqCst);
        return Err("no pattern to export".into());
    }
    let options = RenderOptions {
        sample_rate: request.sample_rate.unwrap_or(x.sample_rate),
        tail_seconds: request.tail_seconds.unwrap_or(x.tail_seconds),
        master: model.master.clone(),
        bit_depth: request.bit_depth.unwrap_or(x.bit_depth),
        normalize_db: request.normalize_db,
        dither: request.dither.unwrap_or(x.dither),
        loops: request.loops.unwrap_or(1).clamp(1, 64),
    };
    let mix_path = PathBuf::from(&request.path);
    // Stem jobs: (file, model with only that track audible).
    let mut jobs: Vec<(PathBuf, ProjectModel)> = vec![(mix_path.clone(), model.clone())];
    if request.stems {
        let stem = mix_path.file_stem().and_then(|s| s.to_str()).unwrap_or("export").to_string();
        let dir = mix_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        for (i, t) in model.tracks.iter().enumerate() {
            // A pattern only sounds on its own track; the other stems would be silent files.
            if !song && pattern_track != Some(t.id) {
                continue;
            }
            let mut m = model.clone();
            for (j, mt) in m.tracks.iter_mut().enumerate() {
                mt.muted = j != i;
                mt.soloed = false;
            }
            let safe: String = t.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
            jobs.push((dir.join(format!("{stem} - {:02} {}.wav", i + 1, safe.trim())), m));
        }
    }
    let total = jobs.len() as u32;
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut files = Vec::new();
        for (i, (path, m)) in jobs.into_iter().enumerate() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let _ = handle.emit("export-progress", ExportProgress { done: i as u32, total, current: name });
            render_wav(Arc::new(build(&m)), &path, &options).map_err(|e| e.to_string())?;
            files.push(path.display().to_string());
        }
        let _ = handle.emit("export-progress", ExportProgress { done: total, total, current: String::new() });
        Ok::<_, String>(files)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    state.exporting.store(false, Ordering::SeqCst);
    result.map(|files| ExportResult { files })
}

// ─── MIDI files ──────────────────────────────────────────────────────────────────────────────

/// Write the context pattern on its own track (`scope` = "pattern") or every track over the
/// song length as a Standard MIDI File.
#[tauri::command]
fn export_midi(state: State<'_, AppState>, path: String, scope: String) -> Result<String, String> {
    let doc = state.doc.lock().unwrap();
    let ctx = state.playback.lock().unwrap();
    let midi_scope = if scope == "pattern" || doc.model().clips.is_empty() {
        let id = ctx.pattern_id.filter(|id| doc.model().pattern(id).is_some()).ok_or("no pattern to export")?;
        MidiScope::Pattern { id, chord_offset_beats: ctx.chord_offset_beats }
    } else {
        MidiScope::Song
    };
    let bytes = midi_file::export(doc.model(), midi_scope).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| format!("could not write {path}: {e}"))?;
    Ok(path)
}

/// Read a MIDI file and add each of its tracks as a new track with one pattern holding its
/// notes and one clip of it at beat 0 (the song grows to fit), in one undo step.
#[tauri::command]
fn import_midi(state: State<'_, AppState>, path: String) -> Result<Snapshot, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("could not read {path}: {e}"))?;
    let bpb = state.doc.lock().unwrap().model().time_signature.beats_per_bar();
    let imported = midi_file::import(&bytes, bpb).map_err(|e| e.to_string())?;
    let count = imported.tracks.len();
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(
            Command::ImportTracks { tracks: imported.tracks, length_beats: imported.length_beats },
            false,
            Some(format!("import {count} MIDI track{}", if count == 1 { "" } else { "s" })),
        )
        .map_err(|e| e.to_string())?;
    state.sync_engine();
    Ok(state.snapshot())
}

// ─── Templates ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    pub name: String,
    pub path: String,
}

impl AppState {
    fn templates_dir(&self) -> Option<PathBuf> {
        self.data_dir().map(|d| d.join("templates"))
    }

    fn list_templates(&self) -> Vec<TemplateInfo> {
        let Some(dir) = self.templates_dir() else { return vec![] };
        let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
        let mut out: Vec<TemplateInfo> = entries
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("dissonant"))
            .map(|e| TemplateInfo {
                name: e.path().file_stem().and_then(|n| n.to_str()).unwrap_or("template").to_string(),
                path: e.path().display().to_string(),
            })
            .collect();
        out.sort_by_key(|t| t.name.to_lowercase());
        out
    }
}

#[tauri::command]
fn list_templates(state: State<'_, AppState>) -> Vec<TemplateInfo> {
    state.list_templates()
}

/// Save the current project as a template (notes, chords, tracks, master — everything).
#[tauri::command]
fn save_template(state: State<'_, AppState>, name: String) -> Result<TemplateInfo, String> {
    let safe: String = name.trim().chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    if safe.is_empty() {
        return Err("a template needs a name".into());
    }
    let dir = state.templates_dir().ok_or("no app data dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{safe}.dissonant"));
    let json = state.doc.lock().unwrap().model().to_json().map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("could not write template: {e}"))?;
    Ok(TemplateInfo { name: safe, path: path.display().to_string() })
}

#[tauri::command]
fn delete_template(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let dir = state.templates_dir().ok_or("no app data dir")?;
    std::fs::remove_file(dir.join(format!("{name}.dissonant"))).map_err(|e| e.to_string())
}

/// A new untitled project from a template: fresh ids are not needed (a template is a
/// snapshot), but the document gets no path so Save asks where.
#[tauri::command]
fn new_from_template(state: State<'_, AppState>, name: String) -> Result<Snapshot, String> {
    let dir = state.templates_dir().ok_or("no app data dir")?;
    let json = std::fs::read_to_string(dir.join(format!("{name}.dissonant"))).map_err(|e| format!("could not read template: {e}"))?;
    let model = ProjectModel::from_json(&json).map_err(open_error)?;
    state.load_model(model, None, false);
    Ok(state.snapshot())
}

/// The built-in vibe templates, for the new-project screen.
#[tauri::command]
fn list_vibes() -> Vec<dissonant_core::VibeInfo> {
    dissonant_core::vibes()
}

/// A new, untitled project from a vibe (`ProjectModel::from_vibe`), as long as the default
/// song length setting says.
#[tauri::command]
fn new_from_vibe(state: State<'_, AppState>, id: String) -> Result<Snapshot, String> {
    let song_bars = state.settings().editing.default_song_bars.max(1);
    let model = ProjectModel::from_vibe(&id, song_bars).ok_or_else(|| format!("no vibe called {id}"))?;
    state.load_model(model, None, false);
    Ok(state.snapshot())
}

// ─── Settings ────────────────────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

/// Store new settings and apply the ones that need the backend: the audio device config
/// (restarting the engine when it changed) and the default MIDI input.
#[tauri::command]
fn set_settings(app: AppHandle, state: State<'_, AppState>, settings: Settings) -> Result<AudioStatus, String> {
    let settings = settings.sanitized();
    let (audio_changed, midi_changed) = {
        let mut current = state.settings.lock().unwrap();
        let audio_changed = current.audio != settings.audio;
        let midi_changed = current.midi.default_input != settings.midi.default_input;
        *current = settings.clone();
        (audio_changed, midi_changed)
    };
    if let Some(path) = state.settings_path() {
        settings.save(&path).map_err(|e| format!("could not save settings: {e}"))?;
    }
    if audio_changed {
        state.restart_audio();
    }
    {
        let mut ctx = state.playback.lock().unwrap();
        ctx.metronome = settings.metronome.on;
        ctx.metronome_volume = settings.metronome.volume;
        let (on, volume) = (ctx.metronome, ctx.metronome_volume);
        drop(ctx);
        state.send(EngineCommand::SetMetronome { on, volume });
    }
    if midi_changed {
        if let Some(name) = &settings.midi.default_input {
            let _ = open_midi(&app, &state, Some(name.clone()));
        }
    }
    Ok(state.audio_status())
}

#[tauri::command]
fn output_devices() -> Vec<OutputDevice> {
    list_output_devices()
        .into_iter()
        .map(|d| OutputDevice {
            name: d.name,
            is_default: d.is_default,
            sample_rates: d.sample_rates,
            default_sample_rate: d.default_sample_rate,
        })
        .collect()
}

#[tauri::command]
fn test_tone(state: State<'_, AppState>) {
    state.send(EngineCommand::TestTone);
}

// ─── Wiring ──────────────────────────────────────────────────────────────────────────────────

fn spawn_playhead_emitter(app: AppHandle) {
    std::thread::Builder::new()
        .name("dissonant-playhead".into())
        .spawn(move || {
            let mut last: Option<(u64, bool)> = None;
            loop {
                std::thread::sleep(Duration::from_millis(16));
                let state = app.state::<AppState>();
                let payload = {
                    let audio = state.audio.lock().unwrap();
                    let Some(device) = audio.as_ref() else { continue };
                    let shared = device.shared();
                    let beat = shared.position_beats();
                    let playing = shared.is_playing();
                    let (l, r) = shared.master_peak();
                    let track_count = state
                        .doc
                        .lock()
                        .unwrap()
                        .model()
                        .tracks
                        .len()
                        .min(dissonant_engine::MAX_TRACKS);
                    let track_peaks = (0..track_count).map(|i| shared.track_peak(i)).collect();
                    PlayheadEvent {
                        beat,
                        playing,
                        master_peak: [l, r],
                        track_peaks,
                        count_in: shared.count_in_beats(),
                    }
                };
                let key = (payload.beat.to_bits(), payload.playing);
                if last == Some(key) && payload.master_peak == [0.0, 0.0] && payload.count_in == 0.0 {
                    continue; // nothing moved; don't spam the webview
                }
                last = Some(key);
                let _ = app.emit("playhead", &payload);
            }
        })
        .expect("playhead thread");
}

/// Watches the output device: restarts on the new default when the stream errors (device
/// unplugged) or the system default changes (headphones plugged in), and keeps retrying while
/// no device is available. Tells the UI through the `audio-status` event.
fn spawn_audio_watchdog(app: AppHandle) {
    std::thread::Builder::new()
        .name("dissonant-audio-watchdog".into())
        .spawn(move || {
            let mut last_attempt = std::time::Instant::now();
            let mut tick = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                tick = tick.wrapping_add(1);
                if tick.is_multiple_of(3) {
                    midi_housekeeping(&app);
                }
                let state = app.state::<AppState>();
                let (running, failed, current) = {
                    let audio = state.audio.lock().unwrap();
                    (
                        audio.is_some(),
                        audio.as_ref().map(|a| a.has_failed()).unwrap_or(false),
                        audio.as_ref().map(|a| a.device_name.clone()),
                    )
                };
                let default = default_output_name();
                let reason = if failed {
                    Some("audio device lost".to_string())
                } else if running && default.is_some() && default != current {
                    Some(format!("output switched to {}", default.clone().unwrap_or_default()))
                } else if !running && default.is_some() && last_attempt.elapsed() >= Duration::from_secs(3) {
                    Some("audio device available".to_string())
                } else {
                    None
                };
                let Some(reason) = reason else { continue };
                last_attempt = std::time::Instant::now();
                log::info!("{reason}: restarting audio");
                let status = state.restart_audio();
                let message = if status.running {
                    format!("{reason} — now on {}", status.device_name.clone().unwrap_or_default())
                } else {
                    format!("{reason} — {}", status.error.clone().unwrap_or_else(|| "no output".into()))
                };
                let _ = app.emit("audio-status", AudioStatusEvent { status, message });
            }
        })
        .expect("audio watchdog thread");
}

/// Writes `<project>.autosave.json` while the document is dirty and changing.
fn spawn_autosaver(app: AppHandle) {
    std::thread::Builder::new()
        .name("dissonant-autosave".into())
        .spawn(move || {
            let mut last_write = std::time::Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(5));
                if last_write.elapsed() < AUTOSAVE_INTERVAL {
                    continue;
                }
                let state = app.state::<AppState>();
                if let Some(path) = state.autosave() {
                    log::debug!("autosaved to {}", path.display());
                    last_write = std::time::Instant::now();
                }
            }
        })
        .expect("autosave thread");
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .setup(|app| {
            let state = app.state::<AppState>();
            match app.path().app_data_dir() {
                Ok(dir) => {
                    let _ = std::fs::create_dir_all(&dir);
                    *state.settings.lock().unwrap() = Settings::load(&dir.join("settings.json"));
                    *state.data_dir.lock().unwrap() = Some(dir);
                }
                Err(e) => log::warn!("no app data dir: {e}"),
            }
            {
                let m = state.settings().metronome;
                let mut ctx = state.playback.lock().unwrap();
                ctx.metronome = m.on;
                ctx.metronome_volume = m.volume;
            }
            state.start_audio();
            state.sync_engine();
            state.resend_transport();
            if let Some(name) = state.settings().midi.default_input {
                if let Err(e) = open_midi(app.handle(), &state, Some(name)) {
                    log::info!("default MIDI input not opened: {e}");
                }
            }
            spawn_playhead_emitter(app.handle().clone());
            spawn_audio_watchdog(app.handle().clone());
            spawn_autosaver(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let state = window.state::<AppState>();
                let dirty = state.doc.lock().unwrap().is_dirty();
                if dirty && !state.force_close.load(Ordering::SeqCst) {
                    // Keep the window; the UI asks save / discard / cancel and calls `quit`.
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                } else {
                    state.clear_autosave();
                }
            }
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                let project = paths.iter().find(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
                        Some("dissonant") | Some("json")
                    )
                });
                if let Some(p) = project {
                    let _ = window.emit("file-dropped", p.display().to_string());
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            apply,
            commit_gesture,
            undo,
            redo,
            new_project,
            open_project,
            save_project,
            set_playback_context,
            transport_play,
            set_metronome,
            transport_stop,
            transport_seek,
            set_hear_chords,
            audition,
            audition_off,
            preview_sound,
            preview_note,
            preview_scope,
            audio_status,
            restart_audio,
            export_wav,
            midi_status,
            open_midi_input,
            close_midi_input,
            set_live_track,
            set_record,
            note_on,
            note_off,
            set_loop,
            clear_loop,
            set_looping,
            quit,
            recent_files,
            clear_recent,
            recovery_candidates,
            restore_autosave,
            discard_autosave,
            autosave_now,
            get_settings,
            set_settings,
            output_devices,
            test_tone,
            export_midi,
            import_midi,
            list_templates,
            save_template,
            delete_template,
            new_from_template,
            list_vibes,
            new_from_vibe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running dissonant");
}

#[allow(dead_code)]
fn _master_type_is_used(_: MasterSettings) {}
