//! The Tauri shell: owns the [`Document`] and the [`AudioDevice`], exposes commands to the UI,
//! and streams the playhead back as events. All model edits go through `apply`, which keeps
//! undo/redo in one place and re-feeds the engine a fresh [`Sequence`] after every change.

pub mod audio_files;
pub mod library;
pub mod plugins;
pub mod recording;
pub mod settings;

use dissonant_catalog::{install, CatalogEntry, InstallEvent, Platform};
use dissonant_core::{
    bar_ticks, midi_file, Command, Document, Insert, InsertTarget, MasterSettings, MidiScope, NativeKind, NoteEvent,
    ProjectModel, Sequence, SongPattern, TrackPlugin, TICKS_PER_BEAT,
};
use dissonant_engine::audio::{AudioBuffer, AudioPool};
use dissonant_engine::{
    default_output_name, list_output_devices, render_wav, AudioConfig, AudioDevice, AudioInput, EngineCommand, EngineIo,
    InputDeviceInfo, InsertSlot, InsertUnit, NativeEffect, RenderOptions, RenderPlugins,
};
use dissonant_plugins::{ParamInfo, PluginDescriptor, PluginFormat};
use plugins::PluginHostThread;
use recording::TakeWriter;
use settings::{AudioSettings, NewProjectKind, Settings};
use midir::{MidiInput, MidiInputConnection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    /// The song tick the edited pattern's tick 0 sits on, for the chords it hears and tiers
    /// against (0 = from song bar 1).
    chord_offset_ticks: i64,
    /// Another pattern played along in pattern mode (the ghost the user chose to hear), and
    /// how loud (1 = the track's own level).
    ghost_pattern_id: Option<Uuid>,
    ghost_level: f64,
    hear_chords: bool,
    /// The track live input (MIDI / typing keyboard) plays and records into.
    live_track: Option<Uuid>,
    /// Mirrors of engine transport settings, re-sent when the audio device restarts.
    looping: bool,
    /// Loop sub-range in ticks.
    loop_region: Option<(f64, f64)>,
    /// Armed punch range in ticks, if any.
    punch_region: Option<(f64, f64)>,
    metronome: bool,
    metronome_volume: f32,
}

impl Default for PlaybackContext {
    fn default() -> Self {
        PlaybackContext {
            mode: PlayMode::Pattern,
            pattern_id: None,
            chord_offset_ticks: 0,
            ghost_pattern_id: None,
            ghost_level: 1.0,
            hear_chords: false,
            live_track: None,
            looping: true,
            loop_region: None,
            punch_region: None,
            metronome: false,
            metronome_volume: 0.6,
        }
    }
}

/// Record-arm state. Held notes are keyed by (track, pitch) with their start tick + velocity.
#[derive(Default)]
struct Recorder {
    armed: bool,
    /// Grid in ticks recorded starts snap to; `None` keeps the take exactly as it was played.
    quantize: Option<i64>,
    held: HashMap<(Uuid, u8), (f64, u8)>,
    /// The pass being recorded right now.
    take: Take,
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
    /// Every loaded plugin, pinned to its own thread; see `plugins` for why.
    plugin_host: PluginHostThread,
    /// What is loaded under each key — a track id for its instrument, an insert id for an
    /// effect — so reconciling after undo or a project open can skip what is already right:
    /// loading a plugin costs hundreds of milliseconds.
    loaded_plugin_uids: Mutex<HashMap<Uuid, String>>,
    /// The insert ids the engine is holding for each chain, in the order it holds them: `None`
    /// is the master's chain, `Some(track)` a track's. Without it a reconcile could not tell a
    /// reorder from an add, and would reload plugins that only moved.
    loaded_insert_chains: Mutex<HashMap<Option<Uuid>, Vec<Uuid>>>,
    /// The live capture stream, when one is open. Independent of the output device: losing
    /// the input must not take playback down with it.
    input: Mutex<Option<AudioInput>>,
    input_error: Mutex<Option<String>>,
    /// Drains the engine's capture ring to disk.
    takes: Mutex<Option<TakeWriter>>,
    /// Decoded audio sources, shared by `Arc` with the engine's pool.
    audio_cache: Mutex<HashMap<Uuid, Arc<AudioBuffer>>>,
    /// Waveform envelopes, computed once per source.
    peak_cache: Mutex<HashMap<Uuid, Arc<Vec<[f32; 2]>>>>,
    /// Which audio track hears the input, and whether it is monitored.
    armed_audio: Mutex<(Option<Uuid>, bool)>,
    /// The source id the current take is being written to.
    recording_source: Mutex<Option<Uuid>>,
    /// What the last take's latency correction came to. Kept here rather than on the clip:
    /// it describes the recording session, not the project, and the REC page is the only
    /// thing that reads it.
    last_take_align: Mutex<Option<TakeAlign>>,
    /// The tuner strip is open, so a rebuilt engine has to be told again.
    tuner_open: AtomicBool,
    importing: AtomicBool,
    /// A DI analysis is running: one at a time, since each one ends in a new track.
    detecting: AtomicBool,
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

/// What the UI's progress bar reads while a catalog plugin installs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub entry_id: String,
    /// "downloading" | "verifying" | "unpacking" | "installed" | "failed"
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub message: Option<String>,
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
/// What a project file is called, everywhere: the save dialog, the open filter, templates.
const PROJECT_EXT: &str = "dissonant";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayheadEvent {
    pub tick: f64,
    pub playing: bool,
    pub master_peak: [f32; 2],
    pub track_peaks: Vec<f32>,
    /// Beats of count-in left (0 when not counting in).
    pub count_in: f32,
    /// Peak of the live input (0 when nothing is captured).
    pub input_peak: f32,
    /// True while a take is actually being captured.
    pub recording: bool,
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
    /// The capture device in use, if one opened.
    pub input_device: Option<String>,
    /// Why the capture device did not open, if it did not.
    pub input_error: Option<String>,
    /// Input latency implied by the block size, in ms.
    pub input_latency_ms: f64,
    /// What a guitarist actually feels: in plus out.
    pub round_trip_ms: f64,
    /// Blocks in which the input ring ran dry.
    pub input_underruns: u32,
    /// Latched since the last `clear_clip`: the mix asked the master for more than its ceiling.
    pub master_over: bool,
    /// The input meter's peak hold, and whether the input has hit full scale, both since the
    /// last `clear_input_peak`. Pre-rig: this is the signal the interface is sending.
    pub input_peak_hold: f32,
    pub input_over: bool,
    /// The manual trim in force for the open input device, in ms.
    pub input_trim_ms: f64,
    /// What the next take will be pulled back by: block + backlog + trim.
    pub alignment_ms: f64,
    /// What the last take was actually corrected by, if one has been recorded this session.
    pub last_take_align: Option<TakeAlign>,
}

/// How far the last take moved, and how much of its head was cut to keep it on the grid.
///
/// Two numbers rather than one because they are not interchangeable: a shift is free, a trim
/// threw audio away, and the REC page says so.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeAlign {
    pub shifted_ms: f64,
    pub trimmed_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDevice {
    pub name: String,
    pub is_default: bool,
    pub sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
}

impl Default for AppState {
    fn default() -> Self {
        AppState::new()
    }
}

impl AppState {
    pub fn new() -> Self {
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
            // The scan cache path needs the app-data dir, which `setup` discovers later; the
            // thread starts cacheless and `set_data_dir` is not worth the churn for one file.
            plugin_host: PluginHostThread::spawn(dirs::cache_dir().map(|d| d.join("dissonant").join("plugin-scan.json"))),
            loaded_plugin_uids: Mutex::new(HashMap::new()),
            loaded_insert_chains: Mutex::new(HashMap::new()),
            input: Mutex::new(None),
            input_error: Mutex::new(None),
            takes: Mutex::new(None),
            audio_cache: Mutex::new(HashMap::new()),
            peak_cache: Mutex::new(HashMap::new()),
            armed_audio: Mutex::new((None, false)),
            recording_source: Mutex::new(None),
            last_take_align: Mutex::new(None),
            tuner_open: AtomicBool::new(false),
            importing: AtomicBool::new(false),
            detecting: AtomicBool::new(false),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// The manual latency trim set for `device`, in ms. Unknown devices read as 0.
    fn input_trim_ms(&self, device: &str) -> f64 {
        self.settings
            .lock()
            .unwrap()
            .audio
            .input_trims
            .get(device)
            .copied()
            .unwrap_or(0.0)
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
        audio.as_ref().map(|a| (a.shared().position_ticks(), a.shared().is_playing()))
    }

    pub fn snapshot(&self) -> Snapshot {
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
        let latency_ms = if sr > 0 { block as f64 * 1000.0 / sr as f64 } else { 0.0 };
        let input = self.input.lock().unwrap();
        // What the player actually feels: the capture block, whatever the engine is still
        // holding in the ring, and the playback block. Reporting only the two block sizes
        // understates it by however much slack the ring is carrying.
        let queued = shared.map(|s| s.input_backlog()).unwrap_or(0);
        let input_latency_ms = if input.is_some() && sr > 0 {
            (block as f64 + queued as f64) * 1000.0 / sr as f64
        } else {
            0.0
        };
        // The same figure with the device's manual trim on top: what a take gets moved by,
        // as against what the round trip measures.
        let input_trim_ms = input.as_ref().map(|i| self.input_trim_ms(&i.device_name)).unwrap_or(0.0);
        let alignment_ms = if input.is_some() {
            dissonant_engine::input_latency_ms(block, queued, sr, input_trim_ms)
        } else {
            0.0
        };
        AudioStatus {
            running: audio.is_some(),
            sample_rate: audio.as_ref().map(|a| a.sample_rate),
            error: self.audio_error.lock().unwrap().clone(),
            device_name: audio.as_ref().map(|a| a.device_name.clone()),
            buffer_size: audio.as_ref().and_then(|a| a.buffer_size),
            block_frames: block,
            latency_ms,
            load: shared.map(|s| s.load()).unwrap_or(0.0),
            xruns: shared.map(|s| s.xruns()).unwrap_or(0),
            input_device: input.as_ref().map(|i| i.device_name.clone()),
            input_error: self.input_error.lock().unwrap().clone(),
            input_latency_ms,
            round_trip_ms: latency_ms + input_latency_ms,
            input_underruns: shared.map(|s| s.input_underruns()).unwrap_or(0),
            master_over: shared.map(|s| s.master_over()).unwrap_or(false),
            input_peak_hold: shared.map(|s| s.input_peak_hold()).unwrap_or(0.0),
            input_over: shared.map(|s| s.input_over()).unwrap_or(false),
            input_trim_ms,
            alignment_ms,
            last_take_align: *self.last_take_align.lock().unwrap(),
        }
    }

    /// Re-send the transport settings the engine forgets when its device is rebuilt.
    fn resend_transport(&self) {
        let (hear_chords, looping, loop_region, punch_region, metronome, volume) = {
            let ctx = self.playback.lock().unwrap();
            (ctx.hear_chords, ctx.looping, ctx.loop_region, ctx.punch_region, ctx.metronome, ctx.metronome_volume)
        };
        self.send(EngineCommand::SetHearChords(hear_chords));
        self.send(EngineCommand::SetLooping(looping));
        self.send(EngineCommand::SetMetronome { on: metronome, volume });
        match loop_region {
            Some((start, end)) => self.send(EngineCommand::SetLoop { start, end }),
            None => self.send(EngineCommand::ClearLoop),
        }
        match punch_region {
            Some((start, end)) => self.send(EngineCommand::SetPunch { start, end }),
            None => self.send(EngineCommand::ClearPunch),
        }
    }

    /// Tear the device down and open the current default again, keeping the transport where
    /// it was. Returns the new status.
    fn restart_audio(&self) -> AudioStatus {
        let was = self.engine_position();
        // Order matters: the input and take writer hold the far ends of the engine's rings,
        // so they go before the device that owns the engine.
        *self.input.lock().unwrap() = None;
        *self.takes.lock().unwrap() = None;
        *self.audio.lock().unwrap() = None;
        self.start_audio();
        self.sync_engine();
        self.sync_audio_pool();
        // The new engine starts with no instruments and empty chains, and every plugin has to
        // be re-opened for whatever rate the new device came up at — a unit opened at 44.1 k
        // cannot be handed to a 48 k engine. Forgetting what the old engine held is what makes
        // the reconcile below rebuild everything instead of thinking it is already right.
        self.loaded_insert_chains.lock().unwrap().clear();
        self.loaded_plugin_uids.lock().unwrap().clear();
        // `sync_plugins` ends in `sync_inserts(true)`, so this covers both.
        self.sync_plugins();
        self.resend_transport();
        let (armed, monitor) = *self.armed_audio.lock().unwrap();
        self.send(EngineCommand::SetAudioArm { track_id: armed, monitor });
        if self.tuner_open.load(Ordering::SeqCst) {
            self.send(EngineCommand::SetTuner(true));
        }
        if let Some((tick, playing)) = was {
            self.send(EngineCommand::Seek { tick });
            if playing {
                self.send(EngineCommand::Play);
            }
        }
        self.audio_status()
    }

    /// Hand the engine every decoded source the model refers to.
    ///
    /// Modelled on `sync_plugins`: the cache is keyed by source id, a miss is filled from the
    /// audio store, and a file that is gone is logged and skipped — the clip stays visible and
    /// plays silence rather than taking the project down.
    fn sync_audio_pool(&self) {
        let wanted: Vec<Uuid> = self.doc.lock().unwrap().model().audio_sources.iter().map(|s| s.id).collect();
        if wanted.is_empty() {
            self.send(EngineCommand::SetAudioPool(Arc::new(AudioPool::default())));
            return;
        }
        let Some(dir) = self.data_dir().map(|d| audio_files::store_dir(&d)) else { return };
        let mut cache = self.audio_cache.lock().unwrap();
        for id in &wanted {
            if cache.contains_key(id) {
                continue;
            }
            match audio_files::read_source(&dir, *id) {
                Ok(buffer) => {
                    cache.insert(*id, Arc::new(buffer));
                }
                Err(e) => log::warn!("audio source {id} is missing: {e}"),
            }
        }
        // Sources nothing refers to any more stay on disk (undo may want them back) but need
        // not stay in memory.
        cache.retain(|id, _| wanted.contains(id));
        let sources: HashMap<Uuid, Arc<AudioBuffer>> = cache.iter().map(|(k, v)| (*k, Arc::clone(v))).collect();
        drop(cache);
        self.send(EngineCommand::SetAudioPool(Arc::new(AudioPool::new(sources))));
    }

    /// The pool as the offline renderer wants it, for export.
    fn audio_pool(&self) -> Arc<AudioPool> {
        let cache = self.audio_cache.lock().unwrap();
        Arc::new(AudioPool::new(cache.iter().map(|(k, v)| (*k, Arc::clone(v))).collect()))
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

    /// Write the document to `target`, and only if that lands treat it as saved. The safety
    /// net — the autosave beside the file, the dirty flag, the undo history's save point —
    /// stays until there is a real file on disk to fall back on.
    pub fn save_to(&self, target: &std::path::Path) -> Result<(), String> {
        let json = self.doc.lock().unwrap().model().to_json().map_err(|e| e.to_string())?;
        write_project_atomically(target, &json).map_err(|e| format!("could not write {}: {e}", target.display()))?;
        {
            let mut doc = self.doc.lock().unwrap();
            doc.path = Some(target.to_path_buf());
            doc.mark_saved();
        }
        self.remember_recent(target);
        self.clear_autosave();
        // A stale autosave next to the file would otherwise be offered on the next launch.
        let _ = std::fs::remove_file(autosave_path_for(target));
        Ok(())
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
            ctx.chord_offset_ticks = 0;
            ctx.ghost_pattern_id = None;
        }
        self.send(EngineCommand::Stop);
        self.send(EngineCommand::Seek { tick: 0.0 });
        self.sync_engine();
        self.sync_plugins();
        // A freshly opened project's sources are still on disk, not in memory.
        self.audio_cache.lock().unwrap().clear();
        self.peak_cache.lock().unwrap().clear();
        self.sync_audio_pool();
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
            build_sequence(doc.model(), ctx.mode, ctx.pattern_id, ctx.chord_offset_ticks, ctx.ghost_pattern_id.map(|g| (g, ctx.ghost_level)))
        };
        let master = self.doc.lock().unwrap().model().master.clone();
        self.send(EngineCommand::SetSequence(Arc::new(seq)));
        self.send(EngineCommand::SetMaster(master));
    }

    /// The rate the engine activates plugins for: what it is actually running at, or — with no
    /// device open yet — the rate it will come up on. 48k is cpal's own fallback.
    fn plugin_sample_rate(&self) -> f64 {
        self.audio_status()
            .sample_rate
            .or(self.settings().audio.sample_rate)
            .unwrap_or(48_000) as f64
    }

    /// Make the plugin host match what the document says each track should be hosting, as an
    /// instrument and as a chain of effects.
    ///
    /// Called after anything that can rewrite tracks wholesale — opening a project, undo, redo,
    /// restoring an autosave. Loading is slow, so a track already hosting the right plugin is
    /// only re-fed its state blob (which is what makes undo of a parameter move actually reach
    /// the plugin rather than just the document).
    fn sync_plugins(&self) {
        let wanted: Vec<(Uuid, Option<TrackPlugin>)> = {
            let doc = self.doc.lock().unwrap();
            doc.model().tracks.iter().map(|t| (t.id, t.plugin.clone())).collect()
        };
        let sample_rate = self.plugin_sample_rate();
        for (track_id, plugin) in wanted {
            let Some(plugin) = plugin else {
                self.plugin_host.close(track_id);
                self.send(EngineCommand::ClearInstrument { track_id });
                continue;
            };
            let already = self.loaded_plugin_uids.lock().unwrap().get(&track_id).cloned();
            if already.as_deref() != Some(plugin.uid.as_str()) {
                match self.plugin_host.open(track_id, &plugin.uid, sample_rate) {
                    Ok((unit, _)) => {
                        self.send(EngineCommand::SetInstrument {
                            track_id,
                            instrument: Box::new(dissonant_engine::Instrument::Plugin(Box::new(unit))),
                        });
                        self.loaded_plugin_uids.lock().unwrap().insert(track_id, plugin.uid.clone());
                    }
                    Err(e) => {
                        // A project made on another machine, or a plugin since uninstalled. The
                        // track keeps its built-in voice and the UI shows the cached name as
                        // missing; losing the reference would be worse than failing loudly.
                        log::warn!("could not load plugin {}: {e}", plugin.uid);
                        self.loaded_plugin_uids.lock().unwrap().remove(&track_id);
                        continue;
                    }
                }
            }
            if let Some(state) = plugin.state.as_deref().and_then(b64_decode) {
                if let Err(e) = self.plugin_host.load_state(track_id, state) {
                    log::warn!("could not restore plugin state for {}: {e}", plugin.uid);
                }
            }
        }
        self.sync_inserts(true);
    }

    /// Make the engine's insert chains — every track's, and the master's — match the document.
    ///
    /// Diffs by slot id rather than rebuilding: a chain that only moved an effect sends a
    /// `MoveInsert` and keeps the plugin exactly where it was, parameters and all. A slot whose
    /// plugin is not installed keeps its place with no unit, so the order of everything after
    /// it survives, and is retried on the next reconcile in case it has since been installed.
    ///
    /// `restore_state` pushes each slot's saved blob back into its plugin, which is what undo,
    /// redo and opening a project need. The insert commands pass `false`: re-feeding a stored
    /// blob mid-session would throw away a knob move that has not been committed yet.
    fn sync_inserts(&self, restore_state: bool) {
        let wanted: Vec<(Option<Uuid>, Vec<Insert>)> = {
            let doc = self.doc.lock().unwrap();
            let model = doc.model();
            let mut chains: Vec<(Option<Uuid>, Vec<Insert>)> =
                model.tracks.iter().map(|t| (Some(t.id), t.inserts.clone())).collect();
            chains.push((None, model.master_inserts.clone()));
            chains
        };
        let sample_rate = self.plugin_sample_rate();

        self.prune_insert_chains();

        for (key, inserts) in wanted {
            let target = key.map_or(InsertTarget::Master, InsertTarget::Track);
            let mut have = self.loaded_insert_chains.lock().unwrap().get(&key).cloned().unwrap_or_default();

            // Slots the document no longer has.
            have.retain(|id| {
                if inserts.iter().any(|i| i.id == *id) {
                    return true;
                }
                self.send(EngineCommand::RemoveInsert { target, id: *id });
                self.release_insert(*id);
                false
            });

            for (index, insert) in inserts.iter().enumerate() {
                // A slot whose plugin never loaded is torn down and tried again: installing the
                // effect and reopening the project is then enough to get the sound back.
                let loaded = self.loaded_plugin_uids.lock().unwrap().get(&insert.id).cloned();
                if loaded.as_deref() != Some(insert.uid.as_str()) {
                    if let Some(at) = have.iter().position(|id| *id == insert.id) {
                        self.send(EngineCommand::RemoveInsert { target, id: insert.id });
                        self.release_insert(insert.id);
                        have.remove(at);
                    }
                }
                match have.iter().position(|id| *id == insert.id) {
                    Some(at) if at == index => {}
                    Some(at) => {
                        self.send(EngineCommand::MoveInsert { target, id: insert.id, to: index });
                        let id = have.remove(at);
                        have.insert(index.min(have.len()), id);
                    }
                    None => {
                        let unit = self.open_insert(insert, sample_rate);
                        let mut slot = InsertSlot::new(insert.id, unit);
                        slot.enabled = insert.enabled;
                        self.send(EngineCommand::AddInsert { target, index, slot });
                        have.insert(index.min(have.len()), insert.id);
                    }
                }
                self.send(EngineCommand::SetInsertEnabled { target, id: insert.id, on: insert.enabled });
                if restore_state {
                    // Undo, redo and opening a project all land here. A native effect's values
                    // are the document's own, so they are pushed straight at the engine; a
                    // plugin's are an opaque blob only it can read.
                    match insert.native_kind() {
                        Some(kind) => {
                            for param in kind.params() {
                                self.send(EngineCommand::SetInsertParam {
                                    id: insert.id,
                                    param_id: param.id,
                                    value: kind.value_of(&insert.params, param.id),
                                });
                            }
                        }
                        None => {
                            if let Some(state) = insert.state.as_deref().and_then(b64_decode) {
                                if let Err(e) = self.plugin_host.load_state(insert.id, state) {
                                    log::warn!("could not restore insert state for {}: {e}", insert.uid);
                                }
                            }
                        }
                    }
                }
            }
            self.loaded_insert_chains.lock().unwrap().insert(key, have);
        }
    }

    /// Forget the chains of tracks that no longer exist, releasing their plugins.
    ///
    /// A deleted track takes its channel — and with it the engine's copy of its chain — away on
    /// the next sequence rebuild, so the record of what the engine holds has to go too.
    /// Otherwise undoing the delete would find the chain "already loaded" and bring the track
    /// back silent.
    fn prune_insert_chains(&self) {
        let live: Vec<Uuid> = self.doc.lock().unwrap().model().tracks.iter().map(|t| t.id).collect();
        let dropped: Vec<Uuid> = {
            let mut chains = self.loaded_insert_chains.lock().unwrap();
            let mut gone = Vec::new();
            chains.retain(|key, ids| match key {
                Some(track_id) if !live.contains(track_id) => {
                    gone.extend(ids.iter().copied());
                    false
                }
                _ => true,
            });
            gone
        };
        for id in dropped {
            self.release_insert(id);
        }
    }

    /// Build one insert's unit: a stock effect, or a hosted plugin — or report why the plugin
    /// could not be loaded and leave the slot empty.
    ///
    /// A project naming an effect this machine does not have keeps the slot and its name, the
    /// same bargain a missing instrument gets: losing the reference would be worse than playing
    /// the chain without it. A native effect is never in that position.
    fn open_insert(&self, insert: &Insert, sample_rate: f64) -> Option<InsertUnit> {
        if let Some(kind) = insert.native_kind() {
            // Built here, on the app thread, with its stored values already dialled in: the
            // engine is handed a finished effect exactly as it is handed a loaded plugin.
            let effect = NativeEffect::with_params(kind, sample_rate as f32, &insert.params);
            self.loaded_plugin_uids.lock().unwrap().insert(insert.id, insert.uid.clone());
            return Some(InsertUnit::Native(Box::new(effect)));
        }
        match self.plugin_host.open(insert.id, &insert.uid, sample_rate) {
            Ok((unit, _)) => {
                self.loaded_plugin_uids.lock().unwrap().insert(insert.id, insert.uid.clone());
                Some(InsertUnit::Plugin(Box::new(unit)))
            }
            Err(e) => {
                log::warn!("could not load insert {} ({}): {e}", insert.name, insert.uid);
                self.loaded_plugin_uids.lock().unwrap().remove(&insert.id);
                None
            }
        }
    }

    /// Drop whatever an insert id was holding on the plugin thread.
    fn release_insert(&self, id: Uuid) {
        self.plugin_host.close(id);
        self.loaded_plugin_uids.lock().unwrap().remove(&id);
    }

    /// Open the output device, then the capture device and the take writer that hang off it.
    ///
    /// The input is secondary: if it fails, playback still runs and the guitar screen shows
    /// why. The reverse is not true — the engine owns both rings, so there is no input
    /// without an output device.
    fn start_audio(&self) {
        let master = self.doc.lock().unwrap().model().master.clone();
        let config = self.audio_config();
        // Half a second of input slack and four seconds of take buffer: enough that a stalled
        // writer thread never costs recorded audio, small enough to be a rounding error in RAM.
        let nominal = config.sample_rate.unwrap_or(48_000) as usize;
        let (in_prod, in_cons) = rtrb::RingBuffer::<f32>::new(nominal / 2);
        let (cap_prod, cap_cons) = rtrb::RingBuffer::<f32>::new(nominal * 4);
        let io = EngineIo { input: in_cons, capture: cap_prod };

        match AudioDevice::start_with(master, config.clone(), Some(io)) {
            Ok(device) => {
                let rate = device.sample_rate;
                let block = device.buffer_size;
                *self.audio.lock().unwrap() = Some(device);
                *self.audio_error.lock().unwrap() = None;

                let settings = self.settings().audio;
                match AudioInput::start_with(settings.input_device.as_deref(), rate, block, settings.input_channel, in_prod) {
                    Ok(input) => {
                        log::info!("capturing from {} at {rate} Hz", input.device_name);
                        *self.input.lock().unwrap() = Some(input);
                        *self.input_error.lock().unwrap() = None;
                    }
                    Err(e) => {
                        log::warn!("no audio input: {e}");
                        *self.input.lock().unwrap() = None;
                        *self.input_error.lock().unwrap() = Some(e.to_string());
                    }
                }
                *self.takes.lock().unwrap() = Some(TakeWriter::spawn(cap_cons));
            }
            Err(e) => {
                log::error!("audio device failed to start: {e}");
                *self.audio_error.lock().unwrap() = Some(e.to_string());
                *self.input_error.lock().unwrap() = Some("no audio device".into());
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

/// `song.dissonant` + `.tmp` → `song.dissonant.tmp`. Appending rather than replacing the
/// extension keeps the sibling unambiguous: two projects in one folder cannot collide.
fn sibling_path(file: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name = file.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "project".into());
    name.push(suffix);
    file.with_file_name(name)
}

/// The open dialog filters on `.dissonant`, so a file saved under a bare name could never be
/// picked again.
pub fn with_project_extension(target: PathBuf) -> PathBuf {
    match target.extension() {
        Some(_) => target,
        None => target.with_extension(PROJECT_EXT),
    }
}

/// Write the project the way every other writer in the app does: a sibling `.tmp`, flushed to
/// the disk, then renamed over the target. A crash or a full disk leaves the previous file
/// whole instead of a half-written one — and that previous file is kept as `<target>.bak`,
/// one rolling generation, before the rename takes it.
pub fn write_project_atomically(target: &std::path::Path, json: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = sibling_path(target, ".tmp");
    let mut file = std::fs::File::create(&tmp)?;
    let flushed = file.write_all(json.as_bytes()).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = flushed {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if target.exists() {
        // Not fatal: a backup that cannot be made is worth less than the save itself.
        if let Err(e) = std::fs::copy(target, sibling_path(target, ".bak")) {
            log::warn!("could not back up {}: {e}", target.display());
        }
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Song mode (SETUP and SONG in the UI) plays the whole song, empty or not. Pattern mode plays
/// the context pattern alone against the chords from `chord_offset_ticks`; with no pattern (or
/// one that is gone) it plays nothing, but every track is still there for live input. A ghost
/// pattern, when given, loops along on its own track (live listening only; exports pass none).
fn build_sequence(
    model: &ProjectModel,
    mode: PlayMode,
    pattern_id: Option<Uuid>,
    chord_offset_ticks: i64,
    ghost: Option<(Uuid, f64)>,
) -> Sequence {
    match mode {
        PlayMode::Song => Sequence::from_song(model),
        PlayMode::Pattern => pattern_id
            .and_then(|id| Sequence::from_pattern(model, &id, chord_offset_ticks))
            .map(|seq| match ghost.filter(|(g, _)| Some(*g) != pattern_id) {
                Some((id, level)) => seq.with_ghost(model, &id, level),
                None => seq,
            })
            .unwrap_or_else(|| Sequence::silent(model, model.bar_ticks() * 4)),
    }
}

/// The message for a project file that would not open.
fn open_error(e: serde_json::Error) -> String {
    format!("not a dissonant project: {e}")
}

// ─── Document commands ───────────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_state(state: State<'_, AppState>) -> Snapshot {
    state.snapshot()
}

#[tauri::command]
fn apply(state: State<'_, AppState>, command: Command, transient: Option<bool>, label: Option<String>) -> Result<Snapshot, String> {
    // Only `AddAudioTake` can introduce a source; everything else leaves the pool alone, and
    // re-reading the store on every knob drag would be absurd.
    let touches_audio = matches!(command, Command::AddAudioTake { .. });
    // The dedicated insert commands below reconcile themselves, but an insert edit can also
    // arrive here, and deleting a track takes a whole chain with it.
    let touches_chains = matches!(
        command,
        Command::AddInsert { .. }
            | Command::RemoveInsert { .. }
            | Command::MoveInsert { .. }
            | Command::SetInsertEnabled { .. }
            | Command::SetInsertParam { .. }
            | Command::DeleteTrack { .. }
    );
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(command, transient.unwrap_or(false), label)
        .map_err(|e| e.to_string())?;
    state.sync_engine();
    if touches_audio {
        state.sync_audio_pool();
    }
    if touches_chains {
        state.sync_inserts(false);
    }
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
    state.sync_plugins();
    // Undo can bring a take back or take one away, so the pool is re-derived rather than
    // guessed at.
    state.sync_audio_pool();
    state.snapshot()
}

#[tauri::command]
fn redo(state: State<'_, AppState>) -> Snapshot {
    state.doc.lock().unwrap().redo();
    state.sync_engine();
    state.sync_plugins();
    state.sync_audio_pool();
    state.snapshot()
}

/// A new project shaped by the editing defaults (tempo, time signature, pattern length, song
/// length, starter vs empty). An explicit `starter` overrides the setting.
#[tauri::command]
fn new_project(state: State<'_, AppState>, starter: Option<bool>) -> Snapshot {
    let e = state.settings().editing;
    let starter = starter.unwrap_or(e.new_project == NewProjectKind::Starter);
    let bar = bar_ticks(e.default_time_signature);
    let ticks = e.default_pattern_bars.max(1) as i64 * bar;
    let song_bars = e.default_song_bars.max(1);
    // The starter is built at the default size, so its melody and groove fill the pattern.
    let mut model = if starter { ProjectModel::starter_shaped(ticks, bar, song_bars) } else { ProjectModel::empty() };
    model.tempo = e.default_tempo;
    model.time_signature = e.default_time_signature;
    model.song_length_ticks = song_bars as i64 * bar;
    for p in &mut model.patterns {
        p.length_ticks = ticks;
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
    let target = match path {
        Some(p) => with_project_extension(PathBuf::from(p)),
        None => {
            let doc = state.doc.lock().unwrap();
            doc.path.clone().ok_or_else(|| "no file path yet — use Save As".to_string())?
        }
    };
    state.save_to(&target)?;
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

/// What ▶ plays: the song, or one pattern heard against the project chords from song tick
/// `chord_offset_ticks` (default 0, song bar 1), with `ghost_pattern_id` looping along when the
/// user wants to hear it, at `ghost_level` (default 1). Switching mode or pattern rewinds;
/// moving only the chord offset or changing the ghost does not.
#[tauri::command]
fn set_playback_context(
    state: State<'_, AppState>,
    mode: PlayMode,
    pattern_id: Option<Uuid>,
    chord_offset_ticks: Option<i64>,
    ghost_pattern_id: Option<Uuid>,
    ghost_level: Option<f64>,
) {
    let offset = chord_offset_ticks.unwrap_or(0).max(0);
    let changed = {
        let mut ctx = state.playback.lock().unwrap();
        let changed = ctx.mode != mode || ctx.pattern_id != pattern_id;
        ctx.mode = mode;
        ctx.pattern_id = pattern_id;
        ctx.chord_offset_ticks = offset;
        ctx.ghost_pattern_id = ghost_pattern_id;
        ctx.ghost_level = ghost_level.filter(|l| l.is_finite()).unwrap_or(1.0).clamp(0.0, 1.5);
        if changed {
            state.send(EngineCommand::Seek { tick: 0.0 });
        }
        changed
    };
    // A take belongs to the pattern it was played into: switching ends it.
    if changed {
        state.commit_take();
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
    // The pass is over, whether or not the player disarms: one undo takes it back.
    state.commit_take();
}

#[tauri::command]
fn transport_seek(state: State<'_, AppState>, tick: f64) {
    state.send(EngineCommand::Seek { tick });
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
        if let Some((tick, true)) = state.engine_position() {
            rec.held.insert((track_id, pitch), (tick, velocity));
        }
    }
    let _ = app.emit("midi-activity", pitch);
}

/// Where a note the player held from tick `start` to tick `end` belongs in the pattern being
/// recorded into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Pattern-local start and length, in ticks.
    At { start_tick: i64, length_ticks: i64 },
    /// The playhead is outside every clip of this pattern, so a song tick says nothing about
    /// pattern time. Folding it in with a modulo — what the recorder used to do — files the
    /// note somewhere it was never played.
    NotPlaying,
}

/// Resolve a played note into pattern time. In song mode that is done through the clip the
/// player is actually hearing: the pattern tick is how far into that clip the note fell, plus
/// whatever the clip is offset by, wrapped by the pattern's own length (a clip longer than its
/// pattern loops it). `quantize` is the grid starts and lengths snap to; `None` records the
/// take as it was played.
pub fn place_recorded_note(
    model: &ProjectModel,
    mode: PlayMode,
    pattern: &SongPattern,
    start: f64,
    end: f64,
    quantize: Option<i64>,
) -> Placement {
    let length_ticks = pattern.length_ticks.max(1);
    let mut len = end - start;
    if len < 0.0 {
        len += length_ticks as f64; // the loop wrapped while the key was held
    }
    let start_in = match mode {
        // Pattern mode plays the pattern alone, so the engine's tick is already pattern time.
        PlayMode::Pattern => start.rem_euclid(length_ticks as f64),
        PlayMode::Song => {
            // Clips can overlap; the one that starts last is the one on top, and the one heard.
            let clip = model
                .clips
                .iter()
                .filter(|c| c.pattern_id == pattern.id && start >= c.start_tick as f64 && start < c.end_tick() as f64)
                .max_by_key(|c| c.start_tick);
            let Some(clip) = clip else { return Placement::NotPlaying };
            (start - clip.start_tick as f64 + clip.offset_ticks as f64).rem_euclid(length_ticks as f64)
        }
    };
    let (start_tick, note_len) = match quantize.filter(|g| *g > 0) {
        Some(grid) => (
            (start_in / grid as f64).round() as i64 * grid,
            ((len / grid as f64).round() as i64 * grid).max(grid),
        ),
        None => (start_in.round() as i64, (len.round() as i64).max(1)),
    };
    // Rounding up can land a start on the pattern's end; keep it inside, a slot short.
    let last = (length_ticks - quantize.filter(|g| *g > 0).unwrap_or(1)).max(0);
    Placement::At { start_tick: start_tick.clamp(0, last), length_ticks: note_len }
}

/// One recorded pass. It remembers what it has written, which is what lets a second pass over
/// the same loop layer onto the first instead of erasing it, and what lets the whole take
/// collapse into a single undo step.
#[derive(Default)]
pub struct Take {
    written: HashSet<Uuid>,
}

impl Take {
    /// Write `note` into `pattern_id`, transient the way a knob drag is: nothing becomes an
    /// undo step until [`Take::finish`] closes the take. `slot` is the window within which a
    /// repeat of the same pitch counts as a double trigger rather than a second layer.
    pub fn write(&mut self, doc: &mut Document, pattern_id: Uuid, note: NoteEvent, slot: i64) -> bool {
        let Some(pattern) = doc.model().pattern(&pattern_id) else { return false };
        let mut notes = pattern.notes.clone();
        // Only this pass's own notes are swallowed: an earlier pass is another overdub layer.
        notes.retain(|n| {
            !(self.written.contains(&n.id) && n.pitch == note.pitch && (n.start_tick - note.start_tick).abs() < slot)
        });
        self.written.insert(note.id);
        notes.push(note);
        doc.apply_labeled(Command::SetNotes { pattern_id, notes }, true, Some("record".into())).is_ok()
    }

    /// Close the take: what it wrote becomes one undo step, and the next pass starts clean.
    pub fn finish(&mut self, doc: &mut Document) {
        self.written.clear();
        doc.commit_gesture();
    }
}

/// What became of a note the player held while armed.
pub enum Recorded {
    Written,
    /// Nothing of the pattern is placed under the playhead. The name is for the player.
    NotPlaying { pattern: String },
    /// Nothing to record into: no context pattern, or it belongs to another track.
    Nowhere,
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
    match state.record_note(track_id, pitch, velocity, start, end) {
        Recorded::Written => {
            state.sync_engine();
            let _ = app.emit("document", state.snapshot());
        }
        Recorded::NotPlaying { pattern } => {
            let _ = app.emit("notice", format!("nothing of {pattern} is playing here — note not recorded"));
        }
        Recorded::Nowhere => {}
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

    /// Write a note the player held into the context pattern. Where it goes is
    /// [`place_recorded_note`]'s decision; this is the document side of it.
    pub fn record_note(&self, track_id: Uuid, pitch: u8, velocity: u8, start: f64, end: f64) -> Recorded {
        let (mode, context) = {
            let ctx = self.playback.lock().unwrap();
            (ctx.mode, ctx.pattern_id)
        };
        let quantize = self.recorder.lock().unwrap().quantize;
        let (pattern_id, name, placement) = {
            let doc = self.doc.lock().unwrap();
            let model = doc.model();
            let Some(p) = context.and_then(|id| model.pattern(&id)).filter(|p| p.track_id == track_id) else {
                return Recorded::Nowhere;
            };
            (p.id, p.name.clone(), place_recorded_note(model, mode, p, start, end, quantize))
        };
        let Placement::At { start_tick, length_ticks } = placement else {
            return Recorded::NotPlaying { pattern: name };
        };
        let note = NoteEvent::new(start_tick, length_ticks, pitch as i32).with_velocity(velocity as i32);
        // Half a grid step either side of the start, or a 128th of a beat when recording as
        // played: close enough to be the same key, struck twice.
        let slot = (quantize.unwrap_or(TICKS_PER_BEAT / 32) / 2).max(1);
        let mut rec = self.recorder.lock().unwrap();
        let mut doc = self.doc.lock().unwrap();
        if rec.take.write(&mut doc, pattern_id, note, slot) {
            Recorded::Written
        } else {
            Recorded::Nowhere
        }
    }

    /// End the take, so the pass the player just made takes one undo to take back. Called on
    /// disarm, on stop, and when the mode or the pattern being recorded into changes.
    pub fn commit_take(&self) {
        let mut rec = self.recorder.lock().unwrap();
        rec.take.finish(&mut self.doc.lock().unwrap());
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

/// Arm or disarm recording. `quantize` is the grid in ticks recorded notes snap to; `None`
/// (or 0) records the take exactly as played.
#[tauri::command]
fn set_record(state: State<'_, AppState>, armed: bool, quantize: Option<i64>) -> MidiStatus {
    let mut rec = state.recorder.lock().unwrap();
    rec.armed = armed;
    rec.quantize = quantize.filter(|q| *q > 0);
    if !armed {
        rec.held.clear();
    }
    drop(rec);
    // Disarming ends the pass: one undo takes the whole take back.
    if !armed {
        state.commit_take();
    }
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

/// Arm the punch range. The engine holds capture until the playhead reaches `start` and drops
/// it at `end`, and stops looping for as long as one is armed.
#[tauri::command]
fn set_punch(state: State<'_, AppState>, start: f64, end: f64) {
    state.playback.lock().unwrap().punch_region = if end > start { Some((start, end)) } else { None };
    state.send(EngineCommand::SetPunch { start, end });
}

#[tauri::command]
fn clear_punch(state: State<'_, AppState>) {
    state.playback.lock().unwrap().punch_region = None;
    state.send(EngineCommand::ClearPunch);
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
    /// Plugins the render could not open, by name. The files were written without them, and
    /// saying so is the whole point: a silently substituted voice is a wrong mix that looks
    /// finished.
    pub missing_plugins: Vec<String>,
}

/// One render pass's plugin instances, plus what the caller owes afterwards.
///
/// Not `Clone` and not reusable: [`RenderPlugins`] holds live units the engine takes ownership
/// of, so each pass opens its own set and closes it again.
#[derive(Default)]
pub struct RenderRig {
    pub plugins: RenderPlugins,
    /// The fresh ids the plugin host now holds, to close once the pass has rendered.
    pub opened: Vec<Uuid>,
    /// Names of the plugins that would not open.
    pub missing: Vec<String>,
}

/// Open one offline render pass's worth of plugins: every audible track's instrument and every
/// slot of every chain, at the render sample rate.
///
/// The engine consumes the units it is handed, so a pass cannot borrow the live ones and two
/// passes cannot share a set — hence open-per-pass, with [`RenderRig::opened`] naming what the
/// caller has to close afterwards.
///
/// Free of [`AppState`] so a test can drive it with a host of its own.
pub fn open_render_plugins(host: &PluginHostThread, model: &ProjectModel, sample_rate: f64) -> RenderRig {
    let mut rig = RenderRig::default();
    // A stem pass mutes every track but one, and a muted track is never fed a note: opening its
    // plugins would cost a load each for a channel that renders silence.
    let any_solo = model.tracks.iter().any(|t| t.soloed);
    let audible = |t: &dissonant_core::Track| if any_solo { t.soloed } else { !t.muted };
    for track in model.tracks.iter().filter(|t| audible(t)) {
        let Some(plugin) = &track.plugin else { continue };
        match open_for_render(host, track.id, &plugin.uid, plugin.state.as_deref(), sample_rate) {
            Some((id, unit)) => {
                rig.opened.push(id);
                rig.plugins
                    .instruments
                    .push((track.id, dissonant_engine::Instrument::Plugin(Box::new(unit))));
            }
            // Rendering the built-in voice here would sound like a finished export of a mix the
            // user never made; the name goes back to the UI instead.
            None => rig.missing.push(plugin.name.clone()),
        }
    }
    let chains = model
        .tracks
        .iter()
        .filter(|t| audible(t))
        .map(|t| (InsertTarget::Track(t.id), &t.inserts))
        .chain(std::iter::once((InsertTarget::Master, &model.master_inserts)));
    for (target, inserts) in chains {
        if inserts.is_empty() {
            continue;
        }
        let mut chain = Vec::with_capacity(inserts.len());
        for insert in inserts {
            // The same effect, built the same way, from the same stored values: an offline
            // bounce of a native chain is the render of what plays, not an approximation.
            let unit = match insert.native_kind() {
                Some(kind) => Some(InsertUnit::Native(Box::new(NativeEffect::with_params(
                    kind,
                    sample_rate as f32,
                    &insert.params,
                )))),
                None => match open_for_render(host, insert.id, &insert.uid, insert.state.as_deref(), sample_rate) {
                    Some((id, unit)) => {
                        rig.opened.push(id);
                        Some(InsertUnit::Plugin(Box::new(unit)))
                    }
                    None => {
                        rig.missing.push(insert.name.clone());
                        None
                    }
                },
            };
            // An empty slot still takes its place, so everything after it keeps its index.
            let mut slot = InsertSlot::new(insert.id, unit);
            slot.enabled = insert.enabled;
            chain.push(slot);
        }
        rig.plugins.inserts.push((target, chain));
    }
    rig
}

/// Open one plugin for a render pass under a **fresh** id — never the live key, or this instance
/// and the live one would collide in the host's table and in the reconcile caches, and closing
/// the render copy would silence playback.
///
/// State comes from the live instance first: `save_state` is the sound the user is hearing right
/// now, while the model's blob is only as fresh as the last edit that wrote it.
fn open_for_render(
    host: &PluginHostThread,
    live_id: Uuid,
    uid: &str,
    stored: Option<&str>,
    sample_rate: f64,
) -> Option<(Uuid, dissonant_plugins::PluginAudioUnit)> {
    let id = Uuid::new_v4();
    let unit = match host.open(id, uid, sample_rate) {
        Ok((unit, _)) => unit,
        Err(e) => {
            log::warn!("export: could not open {uid}: {e}");
            return None;
        }
    };
    let state = host
        .save_state(live_id)
        .map(|s| (s, "the live instance"))
        .or_else(|| stored.and_then(b64_decode).map(|s| (s, "the project")));
    match state {
        Some((state, from)) => {
            log::debug!("export: {uid} state from {from}");
            if let Err(e) = host.load_state(id, state) {
                log::warn!("export: could not restore state for {uid}: {e}");
            }
        }
        None => log::debug!("export: {uid} renders with its default state"),
    }
    Some((id, unit))
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
        (doc.model().clone(), ctx.pattern_id, ctx.chord_offset_ticks)
    };
    // Audio clips make it a song export even when no pattern is placed, or a guitar-only
    // project would export silence.
    let song = request.scope != "pattern" && !(model.clips.is_empty() && model.audio_clips.is_empty());
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
        // Without the pool the renderer has no samples and every guitar take exports silent.
        audio_pool: Some(state.audio_pool()),
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
        // The managed state outlives the app, so the blocking pass can reach the plugin host the
        // same way a command would.
        let state = handle.state::<AppState>();
        let sample_rate = options.sample_rate as f64;
        let mut files = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for (i, (path, m)) in jobs.into_iter().enumerate() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let _ = handle.emit("export-progress", ExportProgress { done: i as u32, total, current: name });
            // One set per pass: the engine consumes the units, so they cannot be reused, and
            // closing them before the next pass keeps peak memory at one set.
            let rig = open_render_plugins(&state.plugin_host, &m, sample_rate);
            let written = render_wav(Arc::new(build(&m)), &path, &options, rig.plugins).map_err(|e| e.to_string());
            for id in rig.opened {
                state.plugin_host.close(id);
            }
            written?;
            for name in rig.missing {
                if !missing.contains(&name) {
                    missing.push(name);
                }
            }
            files.push(path.display().to_string());
        }
        let _ = handle.emit("export-progress", ExportProgress { done: total, total, current: String::new() });
        Ok::<_, String>((files, missing))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    state.exporting.store(false, Ordering::SeqCst);
    result.map(|(files, missing_plugins)| ExportResult { files, missing_plugins })
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
        MidiScope::Pattern { id, chord_offset_ticks: ctx.chord_offset_ticks }
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
    let bar = bar_ticks(state.doc.lock().unwrap().model().time_signature);
    let imported = midi_file::import(&bytes, bar).map_err(|e| e.to_string())?;
    let count = imported.tracks.len();
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(
            Command::ImportTracks { tracks: imported.tracks, length_ticks: imported.length_ticks },
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

// ─── Sound library ───────────────────────────────────────────────────────────────────────────

impl AppState {
    fn library_path(&self) -> Option<PathBuf> {
        self.data_dir().map(|d| d.join(library::FILE_NAME))
    }
}

/// The global user sound library (`sound-library.json` in app data), shared by every project.
#[tauri::command]
fn library_sounds(state: State<'_, AppState>) -> Vec<dissonant_core::Sound> {
    state.library_path().map(|p| library::load(&p)).unwrap_or_default()
}

/// Insert or replace a user sound by id; returns the whole library.
#[tauri::command]
fn save_library_sound(state: State<'_, AppState>, sound: dissonant_core::Sound) -> Result<Vec<dissonant_core::Sound>, String> {
    library::save(&state.library_path().ok_or("no app data dir")?, sound)
}

#[tauri::command]
fn delete_library_sound(state: State<'_, AppState>, id: String) -> Result<Vec<dissonant_core::Sound>, String> {
    library::delete(&state.library_path().ok_or("no app data dir")?, &id)
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
        // Only the fields the streams are actually built from. The manual latency trim lives
        // in this section too and is nudged a millisecond at a time while a take is being
        // checked; restarting both devices under the player for that would be absurd.
        let device_shape = |a: &AudioSettings| {
            (
                a.device.clone(),
                a.sample_rate,
                a.buffer_size,
                a.input_device.clone(),
                a.input_channel,
            )
        };
        let audio_changed = device_shape(&current.audio) != device_shape(&settings.audio);
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

/// Acknowledge the latched clip indicator; the next over lights it again.
#[tauri::command]
fn clear_clip(state: State<'_, AppState>) {
    state.send(EngineCommand::ClearClip);
}

/// Acknowledge the input meter's peak hold and its clip latch, together: they are one reading
/// of the same signal and clearing half of it would leave the strip lying.
#[tauri::command]
fn clear_input_peak(state: State<'_, AppState>) -> AudioStatus {
    state.send(EngineCommand::ClearInputPeak);
    state.audio_status()
}

/// What the tuner hears: a frequency and how periodic the window was.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunerReading {
    pub hz: f32,
    /// 0…1. A chord or a muted string reads low, and the strip says "—" rather than guessing.
    pub clarity: f32,
}

/// Open or close the tuner. While it is open the engine is silent and the live input is run
/// through pitch detection on every stopped block.
#[tauri::command]
fn set_tuner(state: State<'_, AppState>, on: bool) {
    // Remembered as well as sent: a rebuilt engine starts with the tuner closed, and the strip
    // would be left showing a needle over an engine that was no longer listening.
    state.tuner_open.store(on, Ordering::SeqCst);
    state.send(EngineCommand::SetTuner(on));
}

/// The tuner's latest reading, polled by the strip. `None` before it has heard a pitch, and
/// again once the string has died away.
#[tauri::command]
fn tuner_reading(state: State<'_, AppState>) -> Option<TunerReading> {
    state
        .audio
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|a| a.shared().tuner_reading())
        .map(|(hz, clarity)| TunerReading { hz, clarity })
}

// ─── Guitar: input, recording and audio import ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
    pub channels: u16,
    pub default_sample_rate: u32,
}

impl From<InputDeviceInfo> for InputDevice {
    fn from(d: InputDeviceInfo) -> Self {
        InputDevice {
            name: d.name,
            is_default: d.is_default,
            channels: d.channels,
            default_sample_rate: d.default_sample_rate,
        }
    }
}

/// What the UI's progress strip reads while a file imports.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioImportProgress {
    pub name: String,
    pub fraction: f32,
}

#[tauri::command]
fn input_devices() -> Vec<InputDevice> {
    dissonant_engine::list_input_devices().into_iter().map(Into::into).collect()
}

/// Point the live input at one audio track, and say whether it is monitored through the rig.
#[tauri::command]
fn set_audio_arm(state: State<'_, AppState>, track_id: Option<Uuid>, monitor: bool) -> Result<AudioStatus, String> {
    if let Some(id) = track_id {
        let doc = state.doc.lock().unwrap();
        match doc.model().track(&id) {
            Some(t) if t.is_audio => {}
            Some(_) => return Err("not an audio track".into()),
            None => return Err("no such track".into()),
        }
    }
    *state.armed_audio.lock().unwrap() = (track_id, monitor);
    state.send(EngineCommand::SetAudioArm { track_id, monitor });
    Ok(state.audio_status())
}

/// Arm the take writer and tell the engine to start capturing.
///
/// This does *not* start the transport: the UI calls `play` next, and the engine only captures
/// once it is actually playing, so a count-in is excluded without anyone having to subtract it.
#[tauri::command]
fn start_audio_record(state: State<'_, AppState>) -> Result<(), String> {
    let (armed, _) = *state.armed_audio.lock().unwrap();
    let track_id = armed.ok_or("arm an audio track first")?;
    if !state.doc.lock().unwrap().model().track(&track_id).is_some_and(|t| t.is_audio) {
        return Err("the armed track is gone".into());
    }
    let rate = state.audio.lock().unwrap().as_ref().map(|a| a.sample_rate).ok_or("no audio device")?;
    if state.input.lock().unwrap().is_none() {
        return Err(state.input_error.lock().unwrap().clone().unwrap_or_else(|| "no audio input".into()));
    }
    let dir = state
        .data_dir()
        .map(|d| audio_files::store_dir(&d))
        .ok_or("the audio store is not available")?;

    let source_id = Uuid::new_v4();
    state
        .takes
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("the take writer is not running")?
        .start(audio_files::source_path(&dir, source_id), rate)?;
    *state.recording_source.lock().unwrap() = Some(source_id);
    state.send(EngineCommand::StartRecord);
    Ok(())
}

/// Finish the take and land it as a clip, in one undo step.
#[tauri::command]
fn stop_audio_record(state: State<'_, AppState>) -> Result<Snapshot, String> {
    state.send(EngineCommand::StopRecord);
    let Some(source_id) = state.recording_source.lock().unwrap().take() else {
        return Ok(state.snapshot());
    };
    // Give the audio thread one block to see the stop before the writer closes the file.
    std::thread::sleep(Duration::from_millis(30));

    let (start_tick, captured, block, backlog) = state
        .audio
        .lock()
        .unwrap()
        .as_ref()
        .map(|a| {
            let s = a.shared();
            let (tick, frames) = s.take_span();
            (tick, frames, s.block_frames(), s.input_backlog())
        })
        .unwrap_or((0.0, 0, 0, 0));
    let rate = state.audio.lock().unwrap().as_ref().map(|a| a.sample_rate).unwrap_or(48_000);
    let frames = state
        .takes
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("the take writer is not running")?
        .stop()?
        .min(captured.max(1));

    let dir = state
        .data_dir()
        .map(|d| audio_files::store_dir(&d))
        .ok_or("the audio store is not available")?;
    if frames == 0 {
        // Nothing was played: leave no trace rather than a zero-length clip to clean up.
        let _ = std::fs::remove_file(audio_files::source_path(&dir, source_id));
        return Ok(state.snapshot());
    }

    let (track_id, _) = *state.armed_audio.lock().unwrap();
    let track_id = track_id.ok_or("the armed track went away mid-take")?;
    let take_number = state.doc.lock().unwrap().model().audio_clips_on(track_id).count() + 1;

    // Everything between the string and the take ring is delay, so the take is pulled back by
    // it before it lands. The tempo map is the song's, read at the take's own position: a take
    // recorded into a ritardando is worth fewer ticks per millisecond than the base tempo says.
    let trim_ms = state
        .input
        .lock()
        .unwrap()
        .as_ref()
        .map(|i| state.input_trim_ms(&i.device_name))
        .unwrap_or(0.0);
    let latency_ms = dissonant_engine::input_latency_ms(block, backlog, rate, trim_ms);
    let map = state.doc.lock().unwrap().model().tempo_map();
    let aligned = dissonant_engine::align_take(&map, start_tick, latency_ms);
    let duration = frames as f64 / rate as f64;
    if duration <= aligned.offset_seconds {
        // The whole take is inside the latency: there is no audio left once the head is cut,
        // so this was a stray click rather than a performance.
        let _ = std::fs::remove_file(audio_files::source_path(&dir, source_id));
        *state.last_take_align.lock().unwrap() = None;
        return Ok(state.snapshot());
    }

    let source = dissonant_core::AudioSource {
        id: source_id,
        name: format!("take {take_number}"),
        frames,
        sample_rate: rate,
        channels: 1,
    };
    let mut clip = dissonant_core::AudioClip::new(
        track_id,
        source_id,
        aligned.start_tick,
        duration - aligned.offset_seconds,
    );
    clip.offset_seconds = aligned.offset_seconds;
    *state.last_take_align.lock().unwrap() = Some(TakeAlign {
        shifted_ms: aligned.shifted_ms,
        trimmed_ms: aligned.trimmed_ms,
    });

    match audio_files::read_source(&dir, source_id) {
        Ok(buffer) => {
            state.audio_cache.lock().unwrap().insert(source_id, Arc::new(buffer));
        }
        Err(e) => log::warn!("the take was written but could not be read back: {e}"),
    }

    // With a punch armed the engine captured exactly the punch range, so the take lands on
    // top of what is already there and the clips underneath are split and crossfaded against
    // it — one command, one undo step, because half-undoing a punch leaves a hole.
    let punched = state.doc.lock().unwrap().model().punch_region.is_some();
    let command = if punched {
        Command::PunchAudioTake {
            track_id,
            source,
            clip,
            crossfade_seconds: dissonant_core::DEFAULT_PUNCH_CROSSFADE_SECONDS,
        }
    } else {
        Command::AddAudioTake { track_id, source, clip }
    };
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(
            command,
            false,
            Some(if punched { "punch in".into() } else { "record take".into() }),
        )
        .map_err(|e| e.to_string())?;
    state.sync_engine();
    state.sync_audio_pool();
    Ok(state.snapshot())
}

/// Decode a file into the audio store and place it on `track_id`.
///
/// With no track, a new audio track named after the file is added first — two commands, so the
/// Edit menu shows "import audio" twice. Honest, and better than one command that has to know
/// how to make tracks.
#[tauri::command]
async fn import_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    track_id: Option<Uuid>,
    start_tick: Option<i64>,
) -> Result<Snapshot, String> {
    if state.importing.swap(true, Ordering::SeqCst) {
        return Err("an import is already running".into());
    }
    let result = import_audio_inner(&app, &state, path, track_id, start_tick.unwrap_or(0)).await;
    state.importing.store(false, Ordering::SeqCst);
    result
}

async fn import_audio_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    path: String,
    track_id: Option<Uuid>,
    start_tick: i64,
) -> Result<Snapshot, String> {
    let source_path = PathBuf::from(&path);
    let name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio")
        .to_string();
    let dir = state
        .data_dir()
        .map(|d| audio_files::store_dir(&d))
        .ok_or("the audio store is not available")?;
    let source_id = Uuid::new_v4();

    let handle = app.clone();
    let progress_name = name.clone();
    // Decoding a long file takes seconds; the UI thread must not wait on it.
    let decoded = tauri::async_runtime::spawn_blocking(move || {
        let mut last = -1.0f32;
        let mut audio = audio_files::decode(&source_path, &mut |fraction| {
            // One event per percent: enough for a smooth bar, few enough not to flood the webview.
            if fraction - last >= 0.01 {
                last = fraction;
                let _ = handle.emit(
                    "audio-import",
                    AudioImportProgress { name: progress_name.clone(), fraction },
                );
            }
        })?;
        audio.channels = audio.channels.min(2);
        audio_files::write_source(&dir, source_id, &audio)?;
        Ok::<_, String>((audio, dir))
    })
    .await
    .map_err(|e| format!("the import thread failed: {e}"))??;
    let (audio, dir) = decoded;

    let frames = audio.frames() as u64;
    if frames == 0 {
        let _ = std::fs::remove_file(audio_files::source_path(&dir, source_id));
        return Err("that file holds no audio".into());
    }
    let seconds = frames as f64 / audio.sample_rate as f64;

    let track_id = match track_id {
        Some(id) if state.doc.lock().unwrap().model().track(&id).is_some_and(|t| t.is_audio) => id,
        _ => {
            let stem = source_path_stem(&path);
            state
                .doc
                .lock()
                .unwrap()
                .apply_labeled(Command::AddAudioTrack { name: stem }, false, Some("import audio".into()))
                .map_err(|e| e.to_string())?;
            state
                .doc
                .lock()
                .unwrap()
                .model()
                .tracks
                .last()
                .map(|t| t.id)
                .ok_or("could not add an audio track")?
        }
    };

    match audio_files::read_source(&dir, source_id) {
        Ok(buffer) => {
            state.audio_cache.lock().unwrap().insert(source_id, Arc::new(buffer));
        }
        Err(e) => log::warn!("the import was written but could not be read back: {e}"),
    }

    let source = dissonant_core::AudioSource {
        id: source_id,
        name,
        frames,
        sample_rate: audio.sample_rate,
        channels: audio.channels,
    };
    let clip = dissonant_core::AudioClip::new(track_id, source_id, start_tick.max(0), seconds);
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(
            Command::AddAudioTake { track_id, source, clip },
            false,
            Some("import audio".into()),
        )
        .map_err(|e| e.to_string())?;
    state.sync_engine();
    state.sync_audio_pool();
    Ok(state.snapshot())
}

fn source_path_stem(path: &str) -> String {
    PathBuf::from(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string()
}

/// What a DI analysis did, for the UI to say so.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetectionReport {
    snapshot: Snapshot,
    /// The new note track, when any notes were written.
    track_id: Option<Uuid>,
    notes: usize,
    chords: usize,
    /// Regions refused as polyphonic, `[start, end]` in song ticks: no notes there.
    refused: Vec<(i64, i64)>,
}

/// Transcribe one audio clip's raw DI: notes onto a new synth track, chord candidates into the
/// detected lane, one undo step for both. The analysis runs on a blocking worker over samples
/// already in memory, so neither the UI nor the audio thread waits on it; the document is only
/// locked to read the clip at the start and to land the result at the end.
#[tauri::command]
async fn detect_audio_clip(state: State<'_, AppState>, clip_id: Uuid) -> Result<DetectionReport, String> {
    if state.detecting.swap(true, Ordering::SeqCst) {
        return Err("a detection is already running".into());
    }
    let result = detect_audio_clip_inner(&state, clip_id).await;
    state.detecting.store(false, Ordering::SeqCst);
    result
}

async fn detect_audio_clip_inner(state: &State<'_, AppState>, clip_id: Uuid) -> Result<DetectionReport, String> {
    // The clip and tempo map as they are now: a take moved while it is being analysed lands
    // its notes where it was when the button was pressed.
    let (clip, source, map) = {
        let doc = state.doc.lock().unwrap();
        let m = doc.model();
        let clip = m.audio_clips.iter().find(|c| c.id == clip_id).cloned().ok_or("that take is not on the song any more")?;
        let source = m.audio_source(&clip.source_id).cloned().ok_or("that take's audio is missing")?;
        (clip, source, m.tempo_map())
    };
    let cached = state.audio_cache.lock().unwrap().get(&source.id).cloned();
    let buffer = match cached {
        Some(b) => b,
        None => {
            let dir = state.data_dir().map(|d| audio_files::store_dir(&d)).ok_or("the audio store is not available")?;
            Arc::new(audio_files::read_source(&dir, source.id)?)
        }
    };
    let (start_tick, offset, length) = (clip.start_tick, clip.offset_seconds, clip.length_seconds);
    let t = tauri::async_runtime::spawn_blocking(move || {
        let mono = dissonant_engine::clip_mono(&buffer, offset, length);
        let heard = dissonant_engine::hear(&mono, buffer.sample_rate as f32);
        dissonant_engine::place_on_song(&heard, clip_id, start_tick, length, &map)
    })
    .await
    .map_err(|e| format!("the detection thread failed: {e}"))?;
    if t.notes.is_empty() && t.detection.chords.is_empty() && t.refused.is_empty() {
        return Err("nothing with a pitch in it in that take".into());
    }

    let (notes, chords, refused) = (t.notes.len(), t.detection.chords.len(), t.refused);
    let label = if notes > 0 { "detect notes" } else { "detect chords" };
    let track_id = {
        let mut doc = state.doc.lock().unwrap();
        let before = doc.model().tracks.len();
        doc.apply_labeled(
            Command::AddTranscription {
                name: format!("{} notes", source_path_stem(&source.name)),
                start_tick,
                length_ticks: t.length_ticks,
                notes: t.notes,
                detection: t.detection,
            },
            false,
            Some(label.into()),
        )
        .map_err(|e| e.to_string())?;
        let m = doc.model();
        (m.tracks.len() > before).then(|| m.tracks.last().map(|t| t.id)).flatten()
    };
    state.sync_engine();
    Ok(DetectionReport { snapshot: state.snapshot(), track_id, notes, chords, refused })
}

/// The waveform envelope for one source: `buckets` min/max pairs over the mono sum.
#[tauri::command]
fn audio_peaks(state: State<'_, AppState>, source_id: Uuid, buckets: u32) -> Vec<[f32; 2]> {
    let buckets = buckets.clamp(1, 8192) as usize;
    // Cached at a fixed resolution and resampled down: a lane that resizes must not redo the
    // whole file on every frame.
    const CACHED: usize = 4096;
    let cached = {
        let cache = state.peak_cache.lock().unwrap();
        cache.get(&source_id).cloned()
    };
    let cached = match cached {
        Some(c) => c,
        None => {
            let Some(buffer) = state.audio_cache.lock().unwrap().get(&source_id).cloned() else {
                return vec![[0.0, 0.0]; buckets];
            };
            let computed = Arc::new(audio_files::peaks(&buffer, CACHED));
            state.peak_cache.lock().unwrap().insert(source_id, Arc::clone(&computed));
            computed
        }
    };
    if buckets >= cached.len() {
        return cached.as_ref().clone();
    }
    let per = cached.len() as f64 / buckets as f64;
    (0..buckets)
        .map(|b| {
            let from = (b as f64 * per) as usize;
            let to = (((b + 1) as f64 * per) as usize).clamp(from + 1, cached.len());
            cached[from..to]
                .iter()
                .fold([f32::MAX, f32::MIN], |[lo, hi], [a, b]| [lo.min(*a), hi.max(*b)])
        })
        .map(|[lo, hi]| if lo > hi { [0.0, 0.0] } else { [lo, hi] })
        .collect()
}

// ─── Plugins ─────────────────────────────────────────────────────────────────────────────────

/// Base64 without a dependency: the plugin state blob is the only thing that needs it, and it
/// crosses one JSON boundary. Standard alphabet, padded, matching what `dissonant-core` validates.
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for c in text.bytes().filter(|c| *c != b'=') {
        let v = B64.iter().position(|b| *b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[tauri::command]
fn scan_plugins(state: State<'_, AppState>, rescan: Option<bool>) -> Vec<PluginDescriptor> {
    state.plugin_host.scan(rescan.unwrap_or(false))
}

/// Put a plugin on a track, or `None` to fall back to the track's built-in voice. The plugin is
/// loaded on the plugin thread, its audio half is handed to the engine, and only then is the
/// document updated — a load that fails leaves the track exactly as it was.
#[tauri::command]
fn set_track_plugin(app: AppHandle, state: State<'_, AppState>, track_id: Uuid, uid: Option<String>) -> Result<Snapshot, String> {
    let Some(uid) = uid else {
        state.plugin_host.close(track_id);
        state.send(EngineCommand::ClearInstrument { track_id });
        state.loaded_plugin_uids.lock().unwrap().remove(&track_id);
        state.doc.lock().unwrap().apply(Command::SetTrackPlugin { id: track_id, plugin: None }, false).map_err(|e| e.to_string())?;
        // Its editor is now looking at a plugin that is not there any more.
        if let Some(w) = app.get_webview_window(&plugin_window_label(track_id)) {
            let _ = w.close();
        }
        return Ok(state.snapshot());
    };
    // The engine activates plugins for the rate it is actually running at; with no device yet,
    // the configured rate is what it will come up on, and 48k is cpal's own fallback.
    let sample_rate = state
        .audio_status()
        .sample_rate
        .or(state.settings().audio.sample_rate)
        .unwrap_or(48_000) as f64;
    let (unit, name) = state.plugin_host.open(track_id, &uid, sample_rate)?;
    state.send(EngineCommand::SetInstrument {
        track_id,
        instrument: Box::new(dissonant_engine::Instrument::Plugin(Box::new(unit))),
    });
    state.loaded_plugin_uids.lock().unwrap().insert(track_id, uid.clone());
    let plugin = TrackPlugin { uid, name, state: None };
    state
        .doc
        .lock()
        .unwrap()
        .apply(Command::SetTrackPlugin { id: track_id, plugin: Some(plugin) }, false)
        .map_err(|e| e.to_string())?;
    Ok(state.snapshot())
}

#[tauri::command]
fn plugin_params(state: State<'_, AppState>, track_id: Uuid) -> Vec<ParamInfo> {
    state.plugin_host.params(track_id)
}

// ─── Insert chains ───────────────────────────────────────────────────────────────────────────

/// Add an effect to a chain. The name comes from the scan, so the document carries something
/// readable even on a machine where the plugin later goes missing; the plugin itself is loaded
/// by the reconcile below, which is also what undo and redo go through.
///
/// `index` appends when absent. `label` is the undo label the UI is showing ("add Consolidated");
/// without one the core's own wording is used.
#[tauri::command]
fn add_insert(
    state: State<'_, AppState>,
    target: InsertTarget,
    uid: String,
    index: Option<usize>,
    label: Option<String>,
) -> Result<Snapshot, String> {
    // A stock effect names itself; only a plugin has to be found in the scan.
    let name = match NativeKind::from_uid(&uid) {
        Some(kind) => kind.display_name().to_string(),
        None => state
            .plugin_host
            .scan(false)
            .into_iter()
            .find(|d| d.uid == uid)
            .map(|d| d.name)
            .ok_or("no such plugin")?,
    };
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(Command::AddInsert { target, uid, name, index }, false, label)
        .map_err(|e| e.to_string())?;
    state.sync_inserts(false);
    Ok(state.snapshot())
}

#[tauri::command]
fn remove_insert(
    state: State<'_, AppState>,
    target: InsertTarget,
    insert_id: Uuid,
    label: Option<String>,
) -> Result<Snapshot, String> {
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(Command::RemoveInsert { target, insert_id }, false, label)
        .map_err(|e| e.to_string())?;
    state.sync_inserts(false);
    Ok(state.snapshot())
}

#[tauri::command]
fn move_insert(
    state: State<'_, AppState>,
    target: InsertTarget,
    insert_id: Uuid,
    to_index: usize,
    label: Option<String>,
) -> Result<Snapshot, String> {
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(Command::MoveInsert { target, insert_id, to_index }, false, label)
        .map_err(|e| e.to_string())?;
    state.sync_inserts(false);
    Ok(state.snapshot())
}

#[tauri::command]
fn set_insert_enabled(
    state: State<'_, AppState>,
    target: InsertTarget,
    insert_id: Uuid,
    on: bool,
    label: Option<String>,
) -> Result<Snapshot, String> {
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(Command::SetInsertEnabled { target, insert_id, on }, false, label)
        .map_err(|e| e.to_string())?;
    state.sync_inserts(false);
    Ok(state.snapshot())
}

/// One insert's parameters. An id nothing is loaded under answers with an empty list rather than
/// an error: the UI fetches these as the selection moves, and a fetch can land after a removal.
///
/// A native effect answers from the catalogue and the document, in the same shape a plugin
/// answers in, so one column of knobs draws either kind.
#[tauri::command]
fn insert_params(state: State<'_, AppState>, insert_id: Uuid) -> Vec<ParamInfo> {
    let native = {
        let doc = state.doc.lock().unwrap();
        insert_target_of(doc.model(), insert_id)
            .and_then(|t| insert_in(doc.model(), t, insert_id))
            .and_then(|i| i.native_kind().map(|k| (k, i.params.clone())))
    };
    match native {
        Some((kind, params)) => kind
            .params()
            .iter()
            .map(|p| ParamInfo {
                id: p.id,
                name: p.name.to_string(),
                module: p.module.to_string(),
                min: p.min,
                max: p.max,
                default: p.default,
                value: kind.value_of(&params, p.id),
                is_stepped: p.stepped,
            })
            .collect(),
        None => state.plugin_host.params(insert_id),
    }
}

/// A parameter move on an insert, with the same bargain as `set_plugin_param`: while the knob is
/// being dragged (`commit` false) it only reaches the audio thread, and on release the value is
/// recorded as a single undo step.
///
/// A plugin's value is recovered from its own state blob; a native effect's is the document's
/// already, so it is written straight in.
#[tauri::command]
fn set_insert_param(
    app: AppHandle,
    state: State<'_, AppState>,
    insert_id: Uuid,
    param_id: u32,
    value: f64,
    commit: Option<bool>,
) -> Result<Option<Snapshot>, String> {
    state.send(EngineCommand::SetInsertParam { id: insert_id, param_id, value });
    if !commit.unwrap_or(false) {
        return Ok(None);
    }
    // Which chain the slot lives in is the document's business, not the UI's: a knob knows its
    // own insert and nothing else.
    let (target, name, native) = {
        let doc = state.doc.lock().unwrap();
        let target = insert_target_of(doc.model(), insert_id).ok_or("no such insert")?;
        let insert = insert_in(doc.model(), target, insert_id).ok_or("no such insert")?;
        (target, insert.name.clone(), insert.native_kind().is_some())
    };
    let command = if native {
        Command::SetInsertParam { target, insert_id, param_id, value }
    } else {
        let Some(bytes) = state.plugin_host.save_state(insert_id) else {
            // An effect with no state extension still sounds right; there is simply nothing to
            // record, so the move is not undoable. Saying so beats inventing a blob.
            return Ok(None);
        };
        Command::SetInsertState { target, insert_id, state: b64_encode(&bytes) }
    };
    state
        .doc
        .lock()
        .unwrap()
        .apply_labeled(command, false, Some(format!("{name} settings")))
        .map_err(|e| e.to_string())?;
    let snapshot = state.snapshot();
    // Mirrors `set_plugin_param`: the edit can come from a window that is not the one holding
    // the undo history, so everyone is told.
    let _ = app.emit("document", snapshot.clone());
    Ok(Some(snapshot))
}

/// The chain an insert id belongs to, searched the only way the app can: by looking.
fn insert_target_of(model: &ProjectModel, insert_id: Uuid) -> Option<InsertTarget> {
    if model.master_inserts.iter().any(|i| i.id == insert_id) {
        return Some(InsertTarget::Master);
    }
    model
        .tracks
        .iter()
        .find(|t| t.inserts.iter().any(|i| i.id == insert_id))
        .map(|t| InsertTarget::Track(t.id))
}

fn insert_in(model: &ProjectModel, target: InsertTarget, insert_id: Uuid) -> Option<&Insert> {
    let chain = match target {
        InsertTarget::Master => &model.master_inserts,
        InsertTarget::Track(id) => &model.track(&id)?.inserts,
    };
    chain.iter().find(|i| i.id == insert_id)
}

/// Plugin editor windows are labelled `plugin-<track id>`, so one track can only ever have one.
///
/// Everything that is not an editor is the project window. Testing for the editor rather than
/// for `"main"` keeps the save-on-close prompt working even if the main window is ever
/// relabelled — getting that backwards would skip the prompt and lose work silently.
const PLUGIN_WINDOW_PREFIX: &str = "plugin-";

fn plugin_window_label(track_id: Uuid) -> String {
    format!("{PLUGIN_WINDOW_PREFIX}{}", track_id.simple())
}

fn is_plugin_window(label: &str) -> bool {
    label.starts_with(PLUGIN_WINDOW_PREFIX)
}

/// Shut every plugin editor. Used when the project they are looking at goes away.
fn close_plugin_windows(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if is_plugin_window(&label) {
            let _ = window.close();
        }
    }
}

/// Open (or raise) a separate window holding one track's plugin controls.
///
/// A plugin like Dexed publishes 156 parameters; a 300px inspector column cannot show that, and
/// cramming it in is what makes hosted plugins miserable to use. The editor gets its own OS
/// window so it can be sized, moved to another monitor, or tiled by a window manager. One window
/// per track, reused: asking twice raises the one already open rather than stacking duplicates.
#[tauri::command]
fn open_plugin_window(app: AppHandle, state: State<'_, AppState>, track_id: Uuid) -> Result<(), String> {
    let (track_name, plugin_name) = {
        let doc = state.doc.lock().unwrap();
        let track = doc.model().track(&track_id).ok_or("no such track")?;
        let plugin = track.plugin.as_ref().ok_or("that track has no plugin")?;
        (track.name.clone(), plugin.name.clone())
    };
    let label = plugin_window_label(track_id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.unminimize();
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?plugin={track_id}").into()),
    )
    .title(format!("{plugin_name} — {track_name}"))
    .inner_size(900.0, 620.0)
    .min_inner_size(560.0, 360.0)
    .resizable(true)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// A parameter move. While a knob is being dragged (`commit` false) this only reaches the audio
/// thread, so the sound follows the finger without serialising plugin state every frame. On
/// release the state is read once and recorded, producing exactly one undo step whose baseline
/// is the value the drag started from.
#[tauri::command]
fn set_plugin_param(
    app: AppHandle,
    state: State<'_, AppState>,
    track_id: Uuid,
    param_id: u32,
    value: f64,
    commit: Option<bool>,
) -> Result<Option<Snapshot>, String> {
    state.send(EngineCommand::SetPluginParam { track_id, param_id, value });
    if !commit.unwrap_or(false) {
        return Ok(None);
    }
    let Some(bytes) = state.plugin_host.save_state(track_id) else {
        // A plugin with no state extension still sounds right; there is simply nothing to
        // record, so the move is not undoable. Saying so beats inventing a blob.
        return Ok(None);
    };
    state
        .doc
        .lock()
        .unwrap()
        .apply(Command::SetPluginState { id: track_id, state: b64_encode(&bytes) }, false)
        .map_err(|e| e.to_string())?;
    let snapshot = state.snapshot();
    // This edit usually comes from the plugin's own window. Only the caller gets the return
    // value, so without the event the main window's undo history would silently fall behind.
    let _ = app.emit("document", snapshot.clone());
    Ok(Some(snapshot))
}

#[tauri::command]
fn plugin_catalog() -> Vec<CatalogEntry> {
    dissonant_catalog::builtin().to_vec()
}

/// Download, verify and install a catalog entry, streaming `plugin-install` events as it goes.
#[tauri::command]
async fn install_plugin(app: AppHandle, entry_id: String) -> Result<(), String> {
    let entry = dissonant_catalog::builtin()
        .iter()
        .find(|e| e.id == entry_id)
        .cloned()
        .ok_or_else(|| format!("no catalog entry {entry_id}"))?;
    let asset = entry
        .asset_for(Platform::current(), PluginFormat::Clap)
        .cloned()
        .ok_or_else(|| format!("{} has no CLAP build for this computer", entry.name))?;
    let dest = dissonant_catalog::user_plugin_dir(PluginFormat::Clap).ok_or("no plugin folder on this computer")?;

    // Blocking I/O: a 180 MB download would otherwise stall the Tauri runtime thread.
    let result = tauri::async_runtime::spawn_blocking(move || {
        let emit = |phase: &str, done: u64, total: u64, message: Option<String>| {
            let _ = app.emit(
                "plugin-install",
                InstallProgress { entry_id: entry.id.clone(), phase: phase.into(), done, total, message },
            );
        };
        let mut on_event = |e: InstallEvent| match e {
            InstallEvent::Started { bytes } => emit("downloading", 0, bytes, None),
            InstallEvent::Progress { done, total } => emit("downloading", done, total, None),
            InstallEvent::Verifying => emit("verifying", 0, 0, None),
            InstallEvent::Unpacking => emit("unpacking", 0, 0, None),
            InstallEvent::Installed { ref paths } => {
                emit("installed", 0, 0, paths.first().map(|p| p.display().to_string()))
            }
            InstallEvent::Failed { ref message } => emit("failed", 0, 0, Some(message.clone())),
        };
        install(&entry, &asset, &dest, &mut on_event).map(|_| ()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

/// Open a vendor page in the user's browser, for plugins that only ship a `.dmg` or `.pkg`.
/// Only http(s) — handing an arbitrary string to the OS opener is a shell-adjacent surface.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) links can be opened".into());
    }
    open::that(url).map_err(|e| e.to_string())
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
                    let tick = shared.position_ticks();
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
                        tick,
                        playing,
                        master_peak: [l, r],
                        track_peaks,
                        count_in: shared.count_in_beats(),
                        input_peak: shared.input_peak(),
                        recording: shared.is_recording(),
                    }
                };
                let key = (payload.tick.to_bits(), payload.playing);
                // The input meter and the recording light ride this event too, so a block
                // that moved neither the playhead nor the meters but has signal on the input
                // still has to go out.
                let quiet = payload.master_peak == [0.0, 0.0] && payload.count_in == 0.0 && payload.input_peak == 0.0 && !payload.recording;
                if last == Some(key) && quiet {
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
        // Reopen where it was last closed, on whatever monitor that was. Without this the OS
        // places the window itself, which under `tauri dev` means it lands on the primary
        // display again after every rebuild.
        .plugin(tauri_plugin_window_state::Builder::default().build())
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
            // Only the main window is the project. A plugin editor is a view onto it: closing
            // one must not prompt to save, must not clear the autosave, and must certainly not
            // quit. `Window::emit` reaches every window, so without this guard closing an editor
            // put the save/discard prompt in front of the main window, which then called `quit`.
            WindowEvent::CloseRequested { api, .. } if !is_plugin_window(window.label()) => {
                let state = window.state::<AppState>();
                let dirty = state.doc.lock().unwrap().is_dirty();
                if dirty && !state.force_close.load(Ordering::SeqCst) {
                    // Keep the window; the UI asks save / discard / cancel and calls `quit`.
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                } else {
                    state.clear_autosave();
                    // The editors are views onto a project that is going away.
                    close_plugin_windows(window.app_handle());
                }
            }
            // A project dropped on a plugin editor is almost certainly a misfire, and opening it
            // would replace the document the editor is looking at.
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) if !is_plugin_window(window.label()) => {
                let project = paths.iter().find(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
                        Some("dissonant") | Some("json")
                    )
                });
                if let Some(p) = project {
                    let _ = window.emit("file-dropped", p.display().to_string());
                    return;
                }
                // Audio gets its own event: routing it through `file-dropped` would make the
                // UI try to open a WAV as a project.
                if let Some(p) = paths.iter().find(|p| audio_files::is_audio_path(p)) {
                    let _ = window.emit("audio-dropped", p.display().to_string());
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
            set_punch,
            clear_punch,
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
            clear_clip,
            clear_input_peak,
            set_tuner,
            tuner_reading,
            export_midi,
            import_midi,
            list_templates,
            save_template,
            delete_template,
            new_from_template,
            scan_plugins,
            set_track_plugin,
            plugin_params,
            set_plugin_param,
            add_insert,
            remove_insert,
            move_insert,
            set_insert_enabled,
            insert_params,
            set_insert_param,
            plugin_catalog,
            install_plugin,
            open_url,
            open_plugin_window,
            input_devices,
            set_audio_arm,
            start_audio_record,
            stop_audio_record,
            import_audio,
            detect_audio_clip,
            audio_peaks,
            library_sounds,
            save_library_sound,
            delete_library_sound,
            list_vibes,
            new_from_vibe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running dissonant");
}

#[allow(dead_code)]
fn _master_type_is_used(_: MasterSettings) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// This predicate decides whether a closing window runs the project's save prompt. Getting
    /// it backwards either loses unsaved work silently, or — as it did once — lets closing a
    /// plugin editor put the prompt in front of the main window and take the whole app down
    /// with it, because `Emitter::emit` reaches every window rather than just the sender.
    #[test]
    fn only_plugin_editors_are_treated_as_plugin_windows() {
        let id = Uuid::new_v4();
        assert!(is_plugin_window(&plugin_window_label(id)));
        assert!(!is_plugin_window("main"));
        // Any future window that is not an editor must still get the save prompt.
        for other in ["settings", "about", "", "export"] {
            assert!(!is_plugin_window(other), "{other:?} was mistaken for a plugin editor");
        }
    }

    #[test]
    fn each_track_gets_exactly_one_editor_label() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(plugin_window_label(a), plugin_window_label(a), "a track's editor label must be stable");
        assert_ne!(plugin_window_label(a), plugin_window_label(b));
    }
}
