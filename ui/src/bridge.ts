// The one place the UI talks to the backend. In Tauri it calls Rust commands; in a plain
// browser (`pnpm --dir ui dev`) it runs an in-memory mock so the interface can be designed and
// exercised without audio. Keep the mock's command semantics aligned with dissonant-core.

import { SOLO, type PeerLocation, type PeerPresence, type SessionState } from "./collab";
import type {
  AmpRig,
  AudioClip,
  AudioImportProgress,
  AudioStatus,
  AudioStatusEvent,
  AutomationLane,
  AutoTarget,
  Breakpoint,
  CatalogEntry,
  ChordEvent,
  Clip,
  ClipDetection,
  DetectedChord,
  DetectionReport,
  ExportProgress,
  ExportRequest,
  ExportResult,
  Command,
  InputDevice,
  Insert,
  InsertTarget,
  InstallProgress,
  MasterSettings,
  MidiStatus,
  NoteEvent,
  Pedal,
  OutputDevice,
  ParamInfo,
  PluginDescriptor,
  Settings,
  PlayheadEvent,
  PlayMode,
  ProjectModel,
  RecoveryCandidate,
  Section,
  Snapshot,
  Track,
  TrackPlugin,
  SongPattern,
  TrackParam,
  TemplateInfo,
  TunerReading,
  KeyState,
  ScaleType,
  VibeInfo,
} from "./types";
import {
  AMP_SWITCH_MODELS,
  autoTargetRange,
  autoTrackId,
  beatsPerBar,
  beatsToTicks,
  defaultAmpRig,
  DEFAULT_PUNCH_CROSSFADE_SECONDS,
  DEFAULT_SWING_GRID_TICKS,
  EQ_MODELS,
  hostPlatforms,
  layOutLoop,
  MAX_CLIP_GAIN,
  MAX_DETECTED_CHORDS,
  MAX_FADE_SECONDS,
  MAX_INSERTS,
  MAX_PEDALS,
  MIC_MODELS,
  MIN_NOTE_TICKS,
  sameAutoTarget,
  sectionSpan,
  tempoMap,
  TICKS_PER_BEAT,
  uuid,
  withDefaults,
} from "./types";
import {
  clampNativeParam,
  nativeEffect,
  nativeValue,
  sanitizeNativeParams,
} from "./effects";
import { deletePieces, deletePoint, insertPieces, insertPoint, punchSplit, windowPiece, type Piece } from "./clipEditing";
import { defaultKit, filledSound, isFactory, lookupSound, validKit, type Sound } from "./sound";
import { chordAt, normalize, progression } from "./theory";
import { MAX_SLIDE_SECS, remapRopes, tidyRopes } from "./ropes";
import { placeChord } from "./songSetup";
import vibesJson from "../../crates/dissonant-core/assets/vibes.json";

export interface Bridge {
  readonly isTauri: boolean;
  getState(): Promise<Snapshot>;
  /** `label` names the edit for the Edit menu ("move notes"); the command's own name otherwise. */
  apply(
    command: Command,
    transient?: boolean,
    label?: string,
    baseRevision?: number,
  ): Promise<Snapshot>;
  shareStart(name: string, color: string): Promise<SessionState>;
  shareStop(): Promise<SessionState>;
  shareResetLink(): Promise<SessionState>;
  removePeer(peer: string): Promise<void>;
  joinSession(code: string, name: string, color: string): Promise<SessionState>;
  leaveSession(): Promise<Snapshot>;
  getSession(): Promise<SessionState>;
  sendPresence(location: PeerLocation): void;
  onSession(cb: (s: SessionState) => void): () => void;
  onPeerPresence(cb: (p: PeerPresence) => void): () => void;
  /** The session ended from the far side; the reason is for the person. */
  onSessionEnded(cb: (reason: string) => void): () => void;
  commitGesture(): Promise<Snapshot>;
  undo(): Promise<Snapshot>;
  redo(): Promise<Snapshot>;
  /** `starter` picks starter or empty; leave it out to follow the "new project opens" setting. */
  newProject(starter?: boolean): Promise<Snapshot>;
  openProject(path: string): Promise<Snapshot>;
  saveProject(path?: string): Promise<Snapshot>;
  /**
   * What ▶ plays. In pattern mode `chordOffsetTicks` is the song tick the pattern's bar 1 sits
   * on: the chord bed and tiering use the project chords from there.
   * `ghostPatternId`: another pattern looped along in pattern mode, heard while you write, at
   * `ghostLevel` (0…1.5, 1 = its track's own level).
   */
  setPlaybackContext(
    mode: PlayMode,
    patternId: string | null,
    chordOffsetTicks: number,
    ghostPatternId?: string | null,
    ghostLevel?: number,
  ): Promise<void>;
  /** Start playing; with `countInBars` the metronome counts first and the transport starts after. */
  play(countInBars?: number): Promise<void>;
  stop(): Promise<void>;
  setMetronome(on: boolean, volume?: number): Promise<void>;
  seek(tick: number): Promise<void>;
  setHearChords(on: boolean): Promise<void>;
  /** `seconds` is how long the note is held before its automatic release. */
  audition(
    trackId: string,
    pitch: number,
    velocity?: number,
    seconds?: number,
  ): Promise<void>;
  auditionOff(trackId: string, pitch: number): Promise<void>;
  audioStatus(): Promise<AudioStatus>;
  restartAudio(): Promise<AudioStatus>;
  exportWav(request: ExportRequest): Promise<ExportResult>;
  onExportProgress(cb: (p: ExportProgress) => void): () => void;
  exportMidi(path: string, scope: "song" | "pattern"): Promise<string>;
  /** Each MIDI track becomes a new track with one pattern and one clip at beat 0. */
  importMidi(path: string): Promise<Snapshot>;
  listTemplates(): Promise<TemplateInfo[]>;
  saveTemplate(name: string): Promise<TemplateInfo>;
  deleteTemplate(name: string): Promise<void>;
  newFromTemplate(name: string): Promise<Snapshot>;
  /** The built-in vibes (`list_vibes`). */
  listVibes(): Promise<VibeInfo[]>;
  /** A new untitled project from a vibe (`new_from_vibe`). */
  newFromVibe(id: string): Promise<Snapshot>;
  onPlayhead(cb: (e: PlayheadEvent) => void): () => void;
  /** The backend changed the document on its own (e.g. a recorded note landed). */
  onDocument(cb: (s: Snapshot) => void): () => void;
  onMidiActivity(cb: (pitch: number) => void): () => void;
  /** The backend has something to tell the player (a note it could not place, say). */
  onNotice(cb: (message: string) => void): () => void;
  midiStatus(): Promise<MidiStatus>;
  openMidiInput(name?: string): Promise<MidiStatus>;
  closeMidiInput(): Promise<MidiStatus>;
  setLiveTrack(trackId: string | null): Promise<void>;
  /** `quantize` is the recording grid in ticks; `null` records the take as played. */
  setRecord(armed: boolean, quantize: number | null): Promise<MidiStatus>;
  noteOn(pitch: number, velocity?: number): Promise<void>;
  noteOff(pitch: number): Promise<void>;
  /** Loop bounds in ticks. */
  setLoop(start: number, end: number): Promise<void>;
  clearLoop(): Promise<void>;
  setLooping(on: boolean): Promise<void>;
  /** Arm the punch range in ticks; the engine drops in and out on those points. */
  setPunch(start: number, end: number): Promise<void>;
  clearPunch(): Promise<void>;
  /** Native open dialog; `extensions` defaults to the project formats. */
  pickOpenPath(extensions?: string[], label?: string): Promise<string | null>;
  pickSavePath(defaultName: string, extension: string): Promise<string | null>;
  /** OS window title (file name + dirty mark). */
  setTitle(title: string): Promise<void>;
  /** Close the app for real; the backend only asks first while the document is dirty. */
  quit(): Promise<void>;
  /** The window's close button was pressed while the document was dirty. */
  onCloseRequested(cb: () => void): () => void;
  /** A `.dissonant` file was dropped onto the window. */
  onFileDropped(cb: (path: string) => void): () => void;
  /** The audio device changed under us (lost, switched, reappeared). */
  onAudioStatus(cb: (e: AudioStatusEvent) => void): () => void;
  recentFiles(): Promise<string[]>;
  clearRecent(): Promise<void>;
  recoveryCandidates(): Promise<RecoveryCandidate[]>;
  restoreAutosave(autosavePath: string): Promise<Snapshot>;
  discardAutosave(autosavePath: string): Promise<void>;
  autosaveNow(): Promise<string | null>;
  getSettings(): Promise<Settings>;
  /** Persist settings; the backend restarts audio / reopens MIDI as needed and returns the audio status. */
  setSettings(settings: Settings): Promise<AudioStatus>;
  outputDevices(): Promise<OutputDevice[]>;
  testTone(): Promise<void>;
  /** Acknowledge the latched master clip indicator. */
  clearClip(): Promise<void>;
  /** The backend opened or dropped a MIDI input on its own. */
  onMidiStatus(cb: (s: MidiStatus) => void): () => void;
  /** Plugins found on this computer. `rescan` walks the folders again instead of answering
   *  from the cache the backend built at startup. */
  scanPlugins(rescan?: boolean): Promise<PluginDescriptor[]>;
  /** Put a scanned plugin on a track, or `null` to go back to the built-in voice. The backend
   *  opens the plugin and edits the document, so the new state arrives on the `document` event. */
  setTrackPlugin(trackId: string, uid: string | null): Promise<void>;
  /** The track plugin's parameters, with the values it currently holds. */
  pluginParams(trackId: string): Promise<ParamInfo[]>;
  /**
   * Write one parameter. While dragging, `commit` is false and the value only reaches the
   * plugin; on release, `commit` is true and the backend folds the plugin's new state into
   * one undo step. There is no transient document command to close, so `commitGesture` plays
   * no part here.
   */
  setPluginParam(
    trackId: string,
    paramId: number,
    value: number,
    commit?: boolean,
  ): Promise<void>;
  /** The curated free-plugin list. */
  pluginCatalog(): Promise<CatalogEntry[]>;
  /** Download, verify and unpack one catalog entry; progress arrives on `onPluginInstall`. */
  installPlugin(entryId: string): Promise<void>;
  onPluginInstall(cb: (p: InstallProgress) => void): () => void;
  /** Hand a link to the user's own browser. Needed by catalog entries that ship an installer
   *  rather than a plugin folder: those have to be run by hand from the vendor's page. */
  openUrl(url: string): Promise<void>;
  /** Open (or raise) the separate window holding one track's plugin controls. */
  openPluginWindow(trackId: string): Promise<void>;

  // ── Effect inserts ──────────────────────────────────────────────────────────────────────
  // Each of these is a document edit plus the hosting work that has to go with it (opening or
  // closing the effect, moving it in the engine's chain), which is why they are commands of
  // their own rather than an `apply` from the UI. `label` is the undo label the user will see
  // in the Edit menu; omitted, the core names the edit itself.
  /** Put a scanned effect in a chain, at `index` or appended. */
  addInsert(
    target: InsertTarget,
    uid: string,
    index?: number | null,
    label?: string,
  ): Promise<Snapshot>;
  removeInsert(
    target: InsertTarget,
    insertId: string,
    label?: string,
  ): Promise<Snapshot>;
  /** Move an insert within its chain; `toIndex` is clamped to the chain. */
  moveInsert(
    target: InsertTarget,
    insertId: string,
    toIndex: number,
    label?: string,
  ): Promise<Snapshot>;
  /** Bypass (`on: false`) or re-enable one insert, leaving it where it is. */
  setInsertEnabled(
    target: InsertTarget,
    insertId: string,
    on: boolean,
    label?: string,
  ): Promise<Snapshot>;
  /** One insert's parameters, with the values its effect currently holds. Empty for an insert
   *  that is gone or whose plugin is not installed, so a fetch racing a removal is harmless. */
  insertParams(insertId: string): Promise<ParamInfo[]>;
  /** Write one insert parameter. Like `setPluginParam`: dragging (`commit` false) only reaches
   *  the effect and answers null; committing folds its state into one undo step. */
  setInsertParam(
    insertId: string,
    paramId: number,
    value: number,
    commit?: boolean,
  ): Promise<Snapshot | null>;

  // ── Guitar ──────────────────────────────────────────────────────────────────────────────
  /** Capture devices this computer offers. */
  inputDevices(): Promise<InputDevice[]>;
  /** Point the live input at one audio track, and whether it is monitored through the rig. */
  setAudioArm(trackId: string | null, monitor: boolean): Promise<AudioStatus>;
  /** Arm the recorder. Does not start the transport — call `play` next, so a count-in is
   *  excluded from the take without anyone having to subtract it. */
  startAudioRecord(): Promise<void>;
  /** Finish the take; the new clip arrives in the returned snapshot as one undo step. */
  stopAudioRecord(): Promise<Snapshot>;
  /** Decode a file into the audio store and place it. With no track, one is added first. */
  importAudio(
    path: string,
    trackId: string | null,
    startTick?: number,
  ): Promise<Snapshot>;
  /** Transcribe one audio clip's raw DI: notes onto a new synth track, chord candidates into
   *  the detected lane, as one undo step. Runs off the UI thread; resolves when it lands. */
  detectAudioClip(clipId: string): Promise<DetectionReport>;
  /** `buckets` min/max pairs over one source, for the waveform lane. */
  audioPeaks(sourceId: string, buckets: number): Promise<[number, number][]>;
  /** Acknowledge the input meter's peak hold and clip latch together. */
  clearInputPeak(): Promise<AudioStatus>;
  /** Open or close the tuner. While it is open the engine is silent and the live input is
   *  run through pitch detection on every block the transport is stopped for. */
  setTuner(on: boolean): Promise<void>;
  /** The tuner's latest reading; `null` before it hears a pitch and once one dies away. */
  tunerReading(): Promise<TunerReading | null>;
  onAudioImport(cb: (p: AudioImportProgress) => void): () => void;
  /** An audio file was dropped onto the window. */
  onAudioDropped(cb: (path: string) => void): () => void;
  /** The global user sound library (`sound-library.json` in app data). */
  librarySounds(): Promise<Sound[]>;
  /** Insert or replace by id; returns the whole library. */
  saveLibrarySound(sound: Sound): Promise<Sound[]>;
  deleteLibrarySound(id: string): Promise<Sound[]>;
  /** Load a patch into the preview channel (SOUND page); `null` stops and clears it. */
  previewSound(sound: Sound | null): Promise<void>;
  previewNote(pitch: number, velocity: number, on: boolean): Promise<void>;
  /** ~512 recent samples of the preview channel, aligned to a rising zero crossing. */
  previewScope(): Promise<number[]>;
}

const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ─── Tauri implementation ──────────────────────────────────────────────────────────────────

async function tauriBridge(): Promise<Bridge> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const dialog = await import("@tauri-apps/plugin-dialog");

  const subscribe = <T>(
    name: string,
    cb: (payload: T) => void,
  ): (() => void) => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<T>(name, (e) => cb(e.payload)).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  };

  return {
    isTauri: true,
    getState: () => invoke<Snapshot>("get_state"),
    apply: (command, transient = false, label, baseRevision) =>
      invoke<Snapshot>("apply", { command, transient, label: label ?? null, baseRevision: baseRevision ?? null }),
    shareStart: (name, color) => invoke<SessionState>("share_start", { name, color }),
    shareStop: () => invoke<SessionState>("share_stop"),
    shareResetLink: () => invoke<SessionState>("share_reset_link"),
    removePeer: (peer) => invoke<void>("remove_peer", { peer }),
    joinSession: (code, name, color) => invoke<SessionState>("join_session", { code, name, color }),
    leaveSession: () => invoke<Snapshot>("leave_session"),
    getSession: () => invoke<SessionState>("get_session"),
    sendPresence: (location) => void invoke("send_presence", { location }),
    onSession: (cb) => subscribe<SessionState>("session", cb),
    onPeerPresence: (cb) => subscribe<PeerPresence>("peer-presence", cb),
    onSessionEnded: (cb) => subscribe<string>("session-ended", cb),
    commitGesture: () => invoke<Snapshot>("commit_gesture"),
    undo: () => invoke<Snapshot>("undo"),
    redo: () => invoke<Snapshot>("redo"),
    newProject: (starter) =>
      invoke<Snapshot>("new_project", { starter: starter ?? null }),
    openProject: (path) => invoke<Snapshot>("open_project", { path }),
    saveProject: (path) =>
      invoke<Snapshot>("save_project", { path: path ?? null }),
    setPlaybackContext: (
      mode,
      patternId,
      chordOffsetTicks,
      ghostPatternId = null,
      ghostLevel = 1,
    ) =>
      invoke("set_playback_context", {
        mode,
        patternId,
        chordOffsetTicks,
        ghostPatternId,
        ghostLevel,
      }),
    play: (countInBars) =>
      invoke("transport_play", { countInBars: countInBars ?? null }),
    stop: () => invoke("transport_stop"),
    setMetronome: (on, volume) =>
      invoke("set_metronome", { on, volume: volume ?? null }),
    seek: (tick) => invoke("transport_seek", { tick }),
    setHearChords: (on) => invoke("set_hear_chords", { on }),
    audition: (trackId, pitch, velocity = 100, seconds) =>
      invoke("audition", { trackId, pitch, velocity, seconds }),
    auditionOff: (trackId, pitch) => invoke("audition_off", { trackId, pitch }),
    audioStatus: () => invoke<AudioStatus>("audio_status"),
    restartAudio: () => invoke<AudioStatus>("restart_audio"),
    exportWav: (request) => invoke<ExportResult>("export_wav", { request }),
    onExportProgress: (cb) => subscribe<ExportProgress>("export-progress", cb),
    exportMidi: (path, scope) => invoke<string>("export_midi", { path, scope }),
    importMidi: (path) => invoke<Snapshot>("import_midi", { path }),
    listTemplates: () => invoke<TemplateInfo[]>("list_templates"),
    saveTemplate: (name) => invoke<TemplateInfo>("save_template", { name }),
    deleteTemplate: (name) => invoke("delete_template", { name }),
    newFromTemplate: (name) => invoke<Snapshot>("new_from_template", { name }),
    listVibes: () => invoke<VibeInfo[]>("list_vibes"),
    newFromVibe: (id) => invoke<Snapshot>("new_from_vibe", { id }),
    onPlayhead: (cb) => subscribe<PlayheadEvent>("playhead", cb),
    onDocument: (cb) => subscribe<Snapshot>("document", cb),
    onMidiActivity: (cb) => subscribe<number>("midi-activity", cb),
    onNotice: (cb) => subscribe<string>("notice", cb),
    midiStatus: () => invoke<MidiStatus>("midi_status"),
    openMidiInput: (name) =>
      invoke<MidiStatus>("open_midi_input", { name: name ?? null }),
    closeMidiInput: () => invoke<MidiStatus>("close_midi_input"),
    setLiveTrack: (trackId) => invoke("set_live_track", { trackId }),
    setRecord: (armed, quantize) =>
      invoke<MidiStatus>("set_record", { armed, quantize }),
    noteOn: (pitch, velocity = 100) => invoke("note_on", { pitch, velocity }),
    noteOff: (pitch) => invoke("note_off", { pitch }),
    setLoop: (start, end) => invoke("set_loop", { start, end }),
    clearLoop: () => invoke("clear_loop"),
    setLooping: (on) => invoke("set_looping", { on }),
    setPunch: (start, end) => invoke("set_punch", { start, end }),
    clearPunch: () => invoke("clear_punch"),
    pickOpenPath: async (
      extensions = ["dissonant", "json"],
      label = "dissonant project",
    ) => {
      const r = await dialog.open({
        multiple: false,
        directory: false,
        filters: [{ name: label, extensions }],
      });
      return typeof r === "string" ? r : null;
    },
    pickSavePath: async (defaultName, extension) => {
      const r = await dialog.save({
        defaultPath: defaultName,
        filters: [{ name: extension, extensions: [extension] }],
      });
      return r ?? null;
    },
    setTitle: (title) => getCurrentWindow().setTitle(title),
    quit: () => invoke("quit"),
    onCloseRequested: (cb) => subscribe<null>("close-requested", () => cb()),
    onFileDropped: (cb) => subscribe<string>("file-dropped", cb),
    onAudioStatus: (cb) => subscribe<AudioStatusEvent>("audio-status", cb),
    recentFiles: () => invoke<string[]>("recent_files"),
    clearRecent: () => invoke("clear_recent"),
    recoveryCandidates: () =>
      invoke<RecoveryCandidate[]>("recovery_candidates"),
    restoreAutosave: (autosavePath) =>
      invoke<Snapshot>("restore_autosave", { autosavePath }),
    discardAutosave: (autosavePath) =>
      invoke("discard_autosave", { autosavePath }),
    autosaveNow: () => invoke<string | null>("autosave_now"),
    getSettings: async () =>
      withDefaults(await invoke<Partial<Settings>>("get_settings")),
    setSettings: (settings) =>
      invoke<AudioStatus>("set_settings", { settings }),
    outputDevices: () => invoke<OutputDevice[]>("output_devices"),
    testTone: () => invoke("test_tone"),
    clearClip: () => invoke("clear_clip"),
    librarySounds: () => invoke<Sound[]>("library_sounds"),
    saveLibrarySound: (sound) => invoke<Sound[]>("save_library_sound", { sound }),
    deleteLibrarySound: (id) => invoke<Sound[]>("delete_library_sound", { id }),
    previewSound: (sound) => invoke("preview_sound", { sound }),
    previewNote: (pitch, velocity, on) => invoke("preview_note", { pitch, velocity, on }),
    previewScope: () => invoke<number[]>("preview_scope"),
    onMidiStatus: (cb) => subscribe<MidiStatus>("midi-status", cb),
    scanPlugins: (rescan = false) =>
      invoke<PluginDescriptor[]>("scan_plugins", { rescan }),
    setTrackPlugin: (trackId, uid) =>
      invoke("set_track_plugin", { trackId, uid }),
    pluginParams: (trackId) =>
      invoke<ParamInfo[]>("plugin_params", { trackId }),
    setPluginParam: (trackId, paramId, value, commit = false) =>
      invoke("set_plugin_param", { trackId, paramId, value, commit }),
    pluginCatalog: () => invoke<CatalogEntry[]>("plugin_catalog"),
    installPlugin: (entryId) => invoke("install_plugin", { entryId }),
    onPluginInstall: (cb) => subscribe<InstallProgress>("plugin-install", cb),
    openUrl: (url) => invoke("open_url", { url }),
    openPluginWindow: (trackId) => invoke("open_plugin_window", { trackId }),
    addInsert: (target, uid, index = null, label) =>
      invoke<Snapshot>("add_insert", {
        target,
        uid,
        index,
        label: label ?? null,
      }),
    removeInsert: (target, insertId, label) =>
      invoke<Snapshot>("remove_insert", {
        target,
        insertId,
        label: label ?? null,
      }),
    moveInsert: (target, insertId, toIndex, label) =>
      invoke<Snapshot>("move_insert", {
        target,
        insertId,
        toIndex,
        label: label ?? null,
      }),
    setInsertEnabled: (target, insertId, on, label) =>
      invoke<Snapshot>("set_insert_enabled", {
        target,
        insertId,
        on,
        label: label ?? null,
      }),
    insertParams: (insertId) =>
      invoke<ParamInfo[]>("insert_params", { insertId }),
    setInsertParam: (insertId, paramId, value, commit = false) =>
      invoke<Snapshot | null>("set_insert_param", {
        insertId,
        paramId,
        value,
        commit,
      }),
    inputDevices: () => invoke<InputDevice[]>("input_devices"),
    setAudioArm: (trackId, monitor) =>
      invoke<AudioStatus>("set_audio_arm", { trackId, monitor }),
    startAudioRecord: () => invoke("start_audio_record"),
    stopAudioRecord: () => invoke<Snapshot>("stop_audio_record"),
    importAudio: (path, trackId, startTick = 0) =>
      invoke<Snapshot>("import_audio", { path, trackId, startTick }),
    detectAudioClip: (clipId) => invoke<DetectionReport>("detect_audio_clip", { clipId }),
    audioPeaks: (sourceId, buckets) =>
      invoke<[number, number][]>("audio_peaks", { sourceId, buckets }),
    clearInputPeak: () => invoke<AudioStatus>("clear_input_peak"),
    setTuner: (on) => invoke("set_tuner", { on }),
    tunerReading: () => invoke<TunerReading | null>("tuner_reading"),
    onAudioImport: (cb) => subscribe<AudioImportProgress>("audio-import", cb),
    onAudioDropped: (cb) => subscribe<string>("audio-dropped", cb),
  };
}

// ─── Browser mock ──────────────────────────────────────────────────────────────────────────

/**
 * `dissonant_core::document::EditError`. Over Tauri IPC `apply` maps the error with
 * `to_string`, so `invoke` rejects with the bare message — the mock rejects with the same
 * value so a rejected command looks identical in both bridges.
 */
const NO_SUCH_TRACK = "no such track";
const NO_SUCH_PATTERN = "no such pattern";
const NO_SUCH_CLIP = "no such clip";
const NO_SUCH_SECTION = "no such section";
const NO_SUCH_SOUND = "no such sound";
const LAST_TRACK = "a project needs at least one track";
const INVALID_VALUE = "invalid value";
const NO_PLUGIN = "track has no plugin";
const NO_SUCH_PLUGIN = "no such plugin";
const NO_SUCH_PARAM = "no such parameter";
const NO_SUCH_ENTRY = "no such catalog entry";
const NO_ASSET = "no download for this computer";
const NO_SUCH_AUDIO_CLIP = "no such audio clip";
const NO_SUCH_AUDIO_SOURCE = "no such audio source";
const NOT_AN_AUDIO_TRACK = "not an audio track";
const NO_SUCH_INSERT = "no such insert";
const NOT_A_NATIVE_EFFECT = "that effect has no parameters of its own";
const TOO_MANY_INSERTS = `an insert chain holds at most ${MAX_INSERTS} effects`;
const PAST_SONG_END = "that is past the end of the song";
const NO_SUCH_AUTOMATION_LANE = "no such automation lane";
const NO_SUCH_AUTOMATION_POINT = "no such automation point";
const NO_SUCH_NOTE = "no such note";
const ROPE_LOOP = "a note can't slide from itself, or from a note that slides from it";
const NO_ROPE = "that note has no rope";
const NO_SUCH_DETECTED_CHORD = "no such detected chord";
/** `ProjectModel` decoding refuses a schema it cannot migrate with this text. */
export const OLD_SCHEMA = "This project was made with an older version of dissonant and can't be opened.";

function fail(message: string): never {
  // A plain string, not an `Error`: that is what Tauri's `invoke` rejects with.
  throw message;
}

const clamp = (x: number, lo: number, hi: number) =>
  Math.min(hi, Math.max(lo, x));
const allFinite = (...xs: number[]) => xs.every((x) => Number.isFinite(x));

/**
 * `document.rs::chain_mut` — the chain a target names, created on demand so a project that has
 * never held an effect carries no empty arrays. Throws `NO_SUCH_TRACK` like the Rust version.
 */
function chainMut(m: ProjectModel, target: InsertTarget): Insert[] {
  if (target === "master") return (m.masterInserts ??= []);
  const t = m.tracks.find((x) => x.id === target.track) ?? fail(NO_SUCH_TRACK);
  return (t.inserts ??= []);
}

/** `document.rs::insert_mut`. */
function insertMut(
  m: ProjectModel,
  target: InsertTarget,
  insertId: string,
): Insert {
  return (
    chainMut(m, target).find((i) => i.id === insertId) ?? fail(NO_SUCH_INSERT)
  );
}

/** `document.rs::is_base64`: shape only — the standard alphabet, padded to a multiple of four
 *  with at most two trailing `=`. Neither side ever decodes a plugin's blob. */
function isBase64(s: string): boolean {
  if (s.length % 4 !== 0) return false;
  const body = s.replace(/=+$/, "");
  if (s.length - body.length > 2) return false;
  return /^[A-Za-z0-9+/]*$/.test(body);
}

/** `model.rs::sanitize_inserts`: a slot that names nothing can never load, a duplicate id makes
 *  two slots indistinguishable, a chain past the cap is more than the engine will run, and a
 *  native effect's knobs are held inside their ranges (a plugin has none of ours at all). */
function sanitizeInserts(chain: Insert[]): Insert[] {
  const seen = new Set<string>();
  return chain
    .filter((i) => {
      if (!i.uid?.trim() || seen.has(i.id)) return false;
      seen.add(i.id);
      return true;
    })
    .slice(0, MAX_INSERTS)
    .map((i) => {
      const effect = nativeEffect(i.uid);
      const params = effect ? sanitizeNativeParams(effect, i.params) : undefined;
      const out: Insert = { ...i, enabled: i.enabled ?? true };
      if (params) out.params = params;
      else delete out.params;
      return out;
    });
}

/** `TrackParam` ranges, from `apply_to`'s `SetTrackParam` arm. */
const TRACK_PARAM_RANGE: Record<TrackParam, [number, number]> = {
  volume: [0, 1.5],
  reverbSend: [0, 1],
  tone: [200, 20_000],
  pan: [-1, 1],
};

/**
 * `AutomationLane::tidy` — drop points that are not a position in time, hold each value inside
 * its target's range, sort by tick and keep only the last point on any tick (the later edit is
 * the one the user just made).
 */
function tidyLane(lane: AutomationLane): void {
  const range = autoTargetRange(lane.target);
  const byTick = new Map<number, Breakpoint>();
  for (const p of lane.points) {
    if (p.tick < 0 || !Number.isFinite(p.value)) continue;
    byTick.set(p.tick, range ? { ...p, value: clamp(p.value, range[0], range[1]) } : { ...p });
  }
  lane.points = [...byTick.values()].sort((a, b) => a.tick - b.tick);
}

/** `document.rs::check_auto_target` — a lane may only name a track the project still has. */
function checkAutoTarget(m: ProjectModel, target: AutoTarget): void {
  const id = autoTrackId(target);
  if (id !== null && !m.tracks.some((t) => t.id === id)) fail(NO_SUCH_TRACK);
}

function newTrack(isDrum: boolean, name: string): Track {
  return {
    id: uuid(),
    name,
    voice: "saw",
    muted: false,
    soloed: false,
    isDrum,
    isAudio: false,
    volume: 1,
    reverbSend: 0,
    tone: 18000,
    pan: 0,
    color: null,
    plugin: null,
    amp: null,
    drumPads: defaultKit(),
  };
}

/** `Track::audio`: no voice, a default rig. */
function newAudioTrack(name: string): Track {
  return { ...newTrack(false, name), isAudio: true, amp: defaultAmpRig() };
}

/** `AudioClip::sanitized`: every number in range, or null when the clip is nonsense. */
function sanitizeAudioClip(c: AudioClip): AudioClip | null {
  if (!allFinite(c.startTick, c.lengthSeconds)) return null;
  if (c.startTick < 0 || c.lengthSeconds <= 0) return null;
  const half = c.lengthSeconds / 2;
  const fade = (v: number) =>
    Number.isFinite(v) ? clamp(v, 0, Math.min(MAX_FADE_SECONDS, half)) : 0;
  return {
    ...c,
    // `start_tick` is an `i64` in Rust: a stored tick is always whole.
    startTick: Math.round(c.startTick),
    offsetSeconds: Number.isFinite(c.offsetSeconds)
      ? Math.max(0, c.offsetSeconds)
      : 0,
    gain: Number.isFinite(c.gain) ? clamp(c.gain, 0, MAX_CLIP_GAIN) : 1,
    fadeInSeconds: fade(c.fadeInSeconds),
    fadeOutSeconds: fade(c.fadeOutSeconds),
  };
}

const knob = (v: number) => (Number.isFinite(v) ? clamp(v, 0, 10) : null);
const ranged = (v: number, lo: number, hi: number) =>
  Number.isFinite(v) ? clamp(v, lo, hi) : null;

/** `Pedal::sanitized`. Returns null when any knob was NaN, like the Rust version. */
function sanitizePedal(p: Pedal): Pedal | null {
  switch (p.kind) {
    case "comp": {
      const [sustain, level] = [knob(p.sustain), knob(p.level)];
      return sustain === null || level === null
        ? null
        : { ...p, sustain, level };
    }
    case "drive": {
      const [drive, tone, level] = [knob(p.drive), knob(p.tone), knob(p.level)];
      return drive === null || tone === null || level === null
        ? null
        : { ...p, drive, tone, level };
    }
    case "wah": {
      const [sensitivity, range] = [knob(p.sensitivity), knob(p.range)];
      return sensitivity === null || range === null
        ? null
        : { ...p, sensitivity, range };
    }
    case "chorus": {
      const [rateHz, depth, mix] = [
        ranged(p.rateHz, 0.1, 8),
        knob(p.depth),
        knob(p.mix),
      ];
      return rateHz === null || depth === null || mix === null
        ? null
        : { ...p, rateHz, depth, mix };
    }
    case "phaser": {
      const [rateHz, depth, feedback] = [
        ranged(p.rateHz, 0.1, 8),
        knob(p.depth),
        knob(p.feedback),
      ];
      return rateHz === null || depth === null || feedback === null
        ? null
        : { ...p, rateHz, depth, feedback };
    }
    case "tremolo": {
      const [rateHz, depth] = [ranged(p.rateHz, 0.1, 14), knob(p.depth)];
      return rateHz === null || depth === null ? null : { ...p, rateHz, depth };
    }
    case "delay": {
      const [timeMs, feedback, mix] = [
        ranged(p.timeMs, 20, 1200),
        knob(p.feedback),
        knob(p.mix),
      ];
      return timeMs === null || feedback === null || mix === null
        ? null
        : { ...p, timeMs, feedback, mix };
    }
  }
}

/** `AmpRig::sanitized`. Null when a value was NaN — a rig that cannot be made sane is
 *  rejected rather than guessed at. */
function sanitizeRig(r: AmpRig): AmpRig | null {
  const inputDb = ranged(r.inputDb, -24, 24);
  const thresholdDb = ranged(r.gate.thresholdDb, -80, 0);
  const releaseMs = ranged(r.gate.releaseMs, 5, 500);
  if (inputDb === null || thresholdDb === null || releaseMs === null)
    return null;
  const pedals: Pedal[] = [];
  for (const p of r.pedals.slice(0, MAX_PEDALS)) {
    const clean = sanitizePedal(p);
    if (clean === null) return null;
    pedals.push(clean);
  }
  const a = [
    r.amp.gain,
    r.amp.bass,
    r.amp.mid,
    r.amp.treble,
    r.amp.presence,
    r.amp.master,
  ].map(knob);
  // A key the file never carried takes its default, the way `#[serde(default)]` does on the Rust
  // side — a project saved before the mic moved is opened, not rejected.
  const posX = knob(r.cab.posX ?? defaultAmpRig().cab.posX);
  const posZ = knob(r.cab.posZ ?? defaultAmpRig().cab.posZ);
  const reverb = knob(r.rack.reverb);
  const decay = knob(r.rack.decay);
  const outputDb = ranged(r.rack.outputDb, -24, 12);
  if (
    a.some((v) => v === null) ||
    posX === null ||
    posZ === null ||
    reverb === null ||
    decay === null ||
    outputDb === null
  )
    return null;
  const [gain, bass, mid, treble, presence, master] = a as number[];
  return {
    enabled: r.enabled,
    inputDb,
    gate: { on: r.gate.on, thresholdDb, releaseMs },
    pedals,
    // A switch the circuit never had is not a switch, exactly as `AmpRig::sanitized` decides it.
    amp: {
      model: r.amp.model,
      gain,
      bass,
      mid,
      treble,
      presence,
      master,
      brightCap: !!r.amp.brightCap && AMP_SWITCH_MODELS.brightCap.includes(r.amp.model),
      halfPower: !!r.amp.halfPower && AMP_SWITCH_MODELS.halfPower.includes(r.amp.model),
      cathodeFollower:
        !!r.amp.cathodeFollower && AMP_SWITCH_MODELS.cathodeFollower.includes(r.amp.model),
    },
    // An unrecognised component falls back to its default instead of sinking the rig: only a
    // number that cannot be made sane is worth rejecting a command over.
    eq: EQ_MODELS.includes(r.eq) ? r.eq : "usClassic",
    cab: {
      model: r.cab.model,
      micModel: MIC_MODELS.includes(r.cab.micModel) ? r.cab.micModel : "dynamic",
      posX,
      posZ,
    },
    rack: { reverb, decay, outputDb },
  };
}

function newPattern(trackId: string, name: string): SongPattern {
  return { id: uuid(), trackId, name, lengthTicks: 16 * TICKS_PER_BEAT, notes: [] };
}

/** `"<track name> <n>"`: the next free number among that track's patterns. */
function nextPatternName(m: ProjectModel, track: Track): string {
  const names = new Set(m.patterns.filter((p) => p.trackId === track.id).map((p) => p.name));
  let n = names.size + 1;
  while (names.has(`${track.name} ${n}`)) n++;
  return `${track.name} ${n}`;
}

/** A copy of a pattern on the same track: fresh ids, `"<name> copy"`. */
function copyPattern(p: SongPattern): SongPattern {
  const copy = clone(p);
  copy.id = uuid();
  copy.name = `${p.name} copy`;
  const copyOf = new Map<string, string>();
  for (const n of copy.notes) {
    const fresh = uuid();
    copyOf.set(n.id, fresh);
    n.id = fresh;
  }
  // Every source is in the copy, so every rope survives, pointing inside the copy.
  copy.notes = remapRopes(copy.notes, copyOf);
  return copy;
}

/** A clip of `patternId` from tick 0 to the song end (loop-extended). */
function songClip(patternId: string, songLengthTicks: number): Clip {
  return { id: uuid(), patternId, startTick: 0, lengthTicks: songLengthTicks, offsetTicks: 0, muted: false };
}

/** Ticks per bar of the model's time signature. */
const barOf = (m: ProjectModel) => beatsToTicks(beatsPerBar(m.timeSignature));

/** The 4/4 bar and the four-bar pattern the starter and the vibes are written in. */
const BAR_TICKS = 4 * TICKS_PER_BEAT;
const PATTERN_TICKS = 4 * BAR_TICKS;

/** The mock's stand-in for `settings.json`. */
const SETTINGS_KEY = "dissonant.mock.settings";

function mockSettings(): Settings {
  try {
    return withDefaults(JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "null"));
  } catch {
    return withDefaults(null);
  }
}

/** `editing.defaultSongBars`: how long every fresh project's song is. */
const defaultSongBars = () => Math.max(1, Math.round(mockSettings().editing.defaultSongBars) || 32);

/** `MasterSettings::default()`: what a file that predates a field opens with. */
const DEFAULT_MASTER: MasterSettings = {
  gain: 1,
  reverbWet: 0,
  lowCutHz: 20,
  highCutHz: 18000,
  lowEq: 1,
  midEq: 1,
  highEq: 1,
  ceilingDb: -0.3,
  limiter: true,
};

/** `ProjectModel::empty()`: one melody track with one empty pattern, no key, nothing arranged. */
function emptyModel(songBars = defaultSongBars()): ProjectModel {
  const melody = newTrack(false, "melody");
  return {
    schemaVersion: SCHEMA_VERSION,
    tempo: 120,
    key: { rootPitchClass: null, scale: "major", isLocked: false },
    tracks: [melody],
    patterns: [newPattern(melody.id, "melody 1")],
    clips: [],
    audioSources: [],
    audioClips: [],
    sections: [],
    master: { ...DEFAULT_MASTER },
    chords: [],
    songLengthTicks: songBars * BAR_TICKS,
  };
}

/** `STARTER_MELODY` in model.rs: (start, length, pitch, velocity) per 4-beat chord slot, in beats. */
const STARTER_MELODY: [number, number, number, number][][] = [
  [[0, 1.5, 76, 100], [1.5, 0.5, 74, 80], [2, 2, 72, 96]], // C:  E  D  C
  [[0, 1, 69, 100], [1, 1, 72, 88], [2, 2, 77, 96]], // F:  A  C  F
  [[0, 1.5, 74, 100], [1.5, 0.5, 71, 80], [2, 2, 67, 96]], // G:  D  B  G
  [[0, 1.5, 72, 100], [1.5, 0.5, 71, 80], [2, 2, 69, 96]], // Am: C  B  A
];

type Groove = "backbeat" | "fourOnFloor" | "boomBap" | "halfTime" | "motorik" | "sparse";
const [KICK, SNARE, CLAP, HAT] = [36, 38, 39, 42];
const eighths = (on: number, off: number): [number, number, number][] => Array.from({ length: 8 }, (_, i) => [i * 0.5, HAT, i % 2 === 0 ? on : off]);

/** `Groove::bar` in vibes.rs for a 4/4 bar: (offset in beats, pitch, velocity). */
const GROOVE_BARS: Record<Groove, [number, number, number][]> = {
  backbeat: [[0, KICK, 120], [1, SNARE, 100], [2, KICK, 120], [3, SNARE, 100], ...eighths(80, 60)],
  fourOnFloor: [[0, KICK, 120], [1, KICK, 115], [2, KICK, 120], [3, KICK, 115], [1, CLAP, 100], [3, CLAP, 100], ...[0.5, 1.5, 2.5, 3.5].map((o): [number, number, number] => [o, HAT, 85])],
  boomBap: [[0, KICK, 120], [1.5, KICK, 100], [2.5, KICK, 110], [1, SNARE, 105], [3, SNARE, 105], ...eighths(70, 50)],
  halfTime: [[0, KICK, 120], [2.5, KICK, 100], [2, SNARE, 110], ...eighths(70, 55)],
  motorik: [[0, KICK, 115], [2, KICK, 115], [2.5, KICK, 95], [1, SNARE, 100], [3, SNARE, 100], ...eighths(80, 75)],
  sparse: [[0, KICK, 90], [2, HAT, 45]],
};

/** `Groove::hits(PATTERN_TICKS, BAR_TICKS)`: four bars of 4/4, sorted by time then pitch. */
function grooveHits(groove: Groove): NoteEvent[] {
  const hits: NoteEvent[] = [];
  for (let bar = 0; bar < PATTERN_TICKS; bar += BAR_TICKS) {
    for (const [offset, pitch, velocity] of GROOVE_BARS[groove])
      hits.push({ id: uuid(), startTick: bar + beatsToTicks(offset), lengthTicks: TICKS_PER_BEAT / 4, pitch, velocity });
  }
  return hits.sort((a, b) => a.startTick - b.startTick || a.pitch - b.pitch);
}

/** One hit of a bass or chord part, every bar: `[offset, length, interval?]`, in beats. */
type PartHit = number[];
/** `vibes::DrumLane`: a step string, one character per `step` beats (X accent, x hit, o ghost). */
interface DrumLane {
  pitch: number;
  step?: number;
  steps: string;
}
/** `vibes::VibeTrack`. */
type VibeTrackSpec = { name: string; sound?: string; tone?: number; reverb?: number; volume?: number; pan?: number } & (
  | { role: "lead" }
  | { role: "bass"; rhythm: PartHit[] }
  | { role: "chords"; low: number; rhythm: PartHit[] }
  | { role: "drums"; kit: [number, string, number][]; lanes: DrumLane[] }
);
/** `vibes::VibeSpec`. The JSON writes musical time in beats; everything built from it is ticks. */
interface VibeSpec {
  id: string;
  name: string;
  blurb: string;
  tempo: number;
  swing: number;
  /** The swing grid in beats (0.5 = eighths). */
  swingGrid: number;
  root: number;
  scale: ScaleType;
  degrees: number[];
  chordBeats: number;
  tracks: VibeTrackSpec[];
}

/** The same `assets/vibes.json` `ProjectModel::from_vibe` builds from. */
const VIBE_SPECS = vibesJson as unknown as VibeSpec[];

const lockedKey = (rootPitchClass: number, scale: ScaleType): KeyState => ({ rootPitchClass, scale, isLocked: true });
const mod12 = (x: number) => ((x % 12) + 12) % 12;

/** `vibes::per_bar` over the four 4/4 bars of a pattern: every rhythm hit of every bar, in
 *  ticks, clipped at the pattern's end. */
function perBar(rhythm: PartHit[], hit: (i: number, startTick: number, lengthTicks: number) => NoteEvent[]): NoteEvent[] {
  const notes: NoteEvent[] = [];
  for (let bar = 0; bar < PATTERN_TICKS; bar += BAR_TICKS) {
    rhythm.forEach(([offset, len], i) => {
      if (offset === undefined || len === undefined) return;
      const start = bar + beatsToTicks(offset);
      if (start >= PATTERN_TICKS) return;
      notes.push(...hit(i, start, Math.max(1, Math.min(beatsToTicks(len), PATTERN_TICKS - start))));
    });
  }
  return notes;
}

/** `vibes::bass_line`: the chord root under each hit, G♯1–G2, plus the hit's interval. */
function bassLine(chords: ChordEvent[], rhythm: PartHit[]): NoteEvent[] {
  return perBar(rhythm, (i, start, len) => {
    const chord = chordAt(chords, start);
    if (!chord || !chord.pitchClasses.length) return [];
    const pc = normalize(chord.pitchClasses[0]);
    const pitch = 36 + pc - (pc >= 8 ? 12 : 0) + (rhythm[i][2] ?? 0);
    return [{ id: uuid(), startTick: start, lengthTicks: len, pitch, velocity: i === 0 ? 105 : 92 }];
  });
}

/** `vibes::chord_part`: the chord voiced close from its root, the root at or above `low`. */
function chordPart(chords: ChordEvent[], low: number, rhythm: PartHit[]): NoteEvent[] {
  return perBar(rhythm, (i, start, len) => {
    const chord = chordAt(chords, start);
    if (!chord) return [];
    let below = low - 1;
    return chord.pitchClasses.map((pc) => {
      below = below + 1 + mod12(pc - below - 1);
      return { id: uuid(), startTick: start, lengthTicks: len, pitch: below, velocity: i === 0 ? 92 : 80 };
    });
  });
}

/** `DrumLane::hits(PATTERN_TICKS)`. */
function laneHits(lane: DrumLane): NoteEvent[] {
  const cells = [...lane.steps].filter((c) => !/\s/.test(c));
  const step = beatsToTicks(lane.step ?? 0.25);
  const period = cells.length * step;
  const velocities: Record<string, number> = { X: 120, x: 96, o: 64 };
  const hits: NoteEvent[] = [];
  if (period <= 0) return hits;
  for (let at = 0; at < PATTERN_TICKS; at += period) {
    cells.forEach((c, i) => {
      const start = at + i * step;
      if (velocities[c] && start < PATTERN_TICKS) hits.push({ id: uuid(), startTick: start, lengthTicks: Math.min(step, PATTERN_TICKS - start), pitch: lane.pitch, velocity: velocities[c] });
    });
  }
  return hits;
}

/**
 * `ProjectModel::from_vibe`: the vibe's tracks in order (the empty lead first), one pattern
 * each ("bells 1", "reese 1", …), the progression tiled across the whole song, one clip per
 * track from 0 to the song end.
 */
function vibeModel(id: string, songBars = defaultSongBars()): ProjectModel | null {
  const v = VIBE_SPECS.find((x) => x.id === id);
  if (!v) return null;
  const m = emptyModel(songBars);
  m.tempo = v.tempo;
  m.swing = v.swing;
  m.swingGridTicks = beatsToTicks(v.swingGrid);
  m.key = lockedKey(v.root, v.scale);
  m.chords = progression(v.degrees, v.root, v.scale, m.songLengthTicks, beatsToTicks(v.chordBeats));
  m.tracks = [];
  m.patterns = [];
  for (const t of v.tracks) {
    const track: Track = { ...newTrack(t.role === "drums", t.name), voice: t.sound ?? "saw", tone: t.tone ?? 18000, reverbSend: t.reverb ?? 0, volume: t.volume ?? 1, pan: t.pan ?? 0 };
    let notes: NoteEvent[] = [];
    if (t.role === "bass") notes = bassLine(m.chords, t.rhythm);
    else if (t.role === "chords") notes = chordPart(m.chords, t.low, t.rhythm);
    else if (t.role === "drums") {
      track.drumPads = t.kit.map(([pitch, sound, choke]) => ({ pitch, sound, choke }));
      notes = t.lanes.flatMap(laneHits).sort((a, b) => a.startTick - b.startTick || a.pitch - b.pitch);
    }
    m.tracks.push(track);
    m.patterns.push({ ...newPattern(track.id, `${t.name} 1`), notes });
  }
  m.clips = m.patterns.map((p) => songClip(p.id, m.songLengthTicks));
  return m;
}

/**
 * `ProjectModel::starter()`: I–IV–V–vi in C (key locked) across the whole song, a pluck melody
 * pattern and a drum pattern, each placed from 0 to the song end so ▶ plays music at once.
 */
function starterModel(songBars = defaultSongBars()): ProjectModel {
  const m = emptyModel(songBars);
  m.key = { rootPitchClass: 0, scale: "major", isLocked: true };
  m.chords = progression([0, 3, 4, 5], 0, "major", m.songLengthTicks);
  const melody = m.tracks[0];
  melody.voice = "pluck";
  melody.reverbSend = 0.2;
  const drums = newTrack(true, "drums");
  m.tracks.push(drums);
  m.patterns[0].notes = STARTER_MELODY.flatMap((phrase, i) =>
    phrase.map(([start, lengthBeats, pitch, velocity]) => ({
      id: uuid(),
      startTick: i * BAR_TICKS + beatsToTicks(start),
      lengthTicks: beatsToTicks(lengthBeats),
      pitch,
      velocity,
    })),
  );
  m.patterns.push({ ...newPattern(drums.id, "drums 1"), notes: grooveHits("backbeat") });
  m.clips = m.patterns.map((p) => songClip(p.id, m.songLengthTicks));
  return m;
}

/**
 * `NoteEvent::sanitized`. Pitch and velocity are `i32` in Rust and every number crosses the IPC
 * boundary as JSON, so a non-finite or fractional value is rejected outright rather than
 * silently stored — in the desktop app serde would refuse the whole payload. The tick fields
 * are `i64` there, so a fractional one is rounded to the tick it names.
 */
function sanitizeNote(n: NoteEvent): NoteEvent {
  if (
    !allFinite(n.startTick, n.lengthTicks) ||
    !Number.isInteger(n.pitch) ||
    !Number.isInteger(n.velocity)
  ) {
    fail(INVALID_VALUE);
  }
  const out: NoteEvent = {
    ...n,
    pitch: clamp(n.pitch, 0, 127),
    velocity: clamp(n.velocity, 1, 127),
    // `start_tick` / `length_ticks` are `i64` in Rust: a stored tick is always whole.
    startTick: Math.max(0, Math.round(n.startTick)),
    lengthTicks: Math.max(MIN_NOTE_TICKS, Math.round(n.lengthTicks)),
  };
  // Same range as the patch's glide; garbage means "use the glide", as in `sanitized`.
  if (out.slideSecs !== undefined) {
    if (Number.isFinite(out.slideSecs)) out.slideSecs = clamp(out.slideSecs, 0, MAX_SLIDE_SECS);
    else delete out.slideSecs;
  }
  return out;
}

/** The schema this build writes; mirrors `SCHEMA_VERSION` in crates/dissonant-core/src/model.rs.
 *  7 added `Track::plugin`; 8 added audio tracks, sources and clips; 9 moved musical time from
 *  beats to ticks; 10 gave every pattern one track and moved the chords onto the song. */
const SCHEMA_VERSION = 10;

/** A stored pattern from a schema ≤ 9 file: one pattern held every track and its own chords. */
interface LegacyPattern {
  trackId?: string;
  notesByTrack?: Record<string, NoteEvent[]>;
  trackLoopTicks?: Record<string, number>;
  chordLoopTicks?: number | null;
  /** Written as `{ chords: [...] }` by schema ≤ 9 and as a bare array by the Rust side today. */
  chords?: { chords?: ChordEvent[] } | ChordEvent[];
}

/** The legacy fields of `p`, which the current `SongPattern` no longer declares. */
const legacy = (p: SongPattern): LegacyPattern => p as SongPattern & LegacyPattern;

/** A legacy pattern's own chord progression, whatever shape it was stored in. */
function legacyChords(p: SongPattern): ChordEvent[] {
  const c = legacy(p).chords;
  return (Array.isArray(c) ? c : (c?.chords ?? [])) ?? [];
}

/**
 * `migrate_beats` in crates/dissonant-core/src/model.rs: a project written before schema 9 keeps
 * its musical time in `f64` beats. The real backend folds those keys on load; the in-memory
 * backend reads the same files out of local storage, so it has to fold them too.
 */
function migrateBeats(m: ProjectModel): ProjectModel {
  // Legacy keys are gone from every model type, so the migration reads them through an
  // index signature: a stored file is untyped JSON until it has been folded.
  const loose = (o: object): Record<string, unknown> => o as Record<string, unknown>;
  const beats = (o: object, key: string): number | null => {
    const v = loose(o)[key];
    return typeof v === "number" && Number.isFinite(v) ? v : null;
  };
  const fold = (o: object, from: string, to: string) => {
    const l = loose(o);
    const was = beats(o, from);
    if (l[to] == null && was !== null) l[to] = beatsToTicks(was);
    delete l[from];
  };
  for (const p of m.patterns) {
    fold(p, "lengthBeats", "lengthTicks");
    fold(p, "chordLoopBeats", "chordLoopTicks");
    const old = legacy(p);
    const loops = loose(p).trackLoopBeats;
    if (loops && typeof loops === "object" && !old.trackLoopTicks) {
      old.trackLoopTicks = Object.fromEntries(
        Object.entries(loops).map(([id, v]) => [id, beatsToTicks(Number(v))]),
      );
    }
    delete loose(p).trackLoopBeats;
    for (const notes of [...Object.values(old.notesByTrack ?? {}), p.notes ?? []]) {
      for (const n of notes) {
        fold(n, "startBeat", "startTick");
        fold(n, "lengthBeats", "lengthTicks");
        fold(n, "shift", "shiftTicks");
      }
    }
    for (const ch of legacyChords(p)) {
      fold(ch, "startBeat", "startTick");
      fold(ch, "lengthBeats", "lengthTicks");
    }
  }
  for (const ch of m.chords ?? []) {
    fold(ch, "startBeat", "startTick");
    fold(ch, "lengthBeats", "lengthTicks");
  }
  for (const c of m.clips ?? []) {
    fold(c, "startBeat", "startTick");
    fold(c, "lengthBeats", "lengthTicks");
    fold(c, "offsetBeats", "offsetTicks");
  }
  for (const s of m.sections ?? []) fold(s, "startBeat", "startTick");
  for (const c of m.audioClips ?? []) fold(c, "startBeat", "startTick");
  for (const t of m.tempoPoints ?? []) fold(t, "beat", "tick");
  fold(m, "songLengthBeats", "songLengthTicks");
  fold(m, "swingGrid", "swingGridTicks");
  return m;
}

/** A progression written over `[0, period)` repeated up to `end`, cut there. Mirrors `tile_chords`. */
function tileChords(progression: ChordEvent[], period: number, end: number): ChordEvent[] {
  const out: ChordEvent[] = [];
  if (period <= 0) return out;
  for (let at = 0; at < end; at += period) {
    for (const c of progression) {
      const start = at + c.startTick;
      const stop = Math.min(start + c.lengthTicks, end);
      if (stop > start) out.push({ ...c, id: uuid(), startTick: start, lengthTicks: stop - start });
    }
  }
  return out;
}

/**
 * `migrate_v10`: a pattern used to hold every track's notes and its own looping chord track.
 * Each one becomes a pattern per track (clips repointed, loops flattened), and its chords are
 * lifted onto the song wherever a clip of it played.
 */
function migrateV10(m: ProjectModel): void {
  if (!m.patterns.some((p) => !legacy(p).trackId)) return;
  const oldClips = m.clips ?? [];
  const patterns: SongPattern[] = [];
  const clips: Clip[] = [];
  const lifted: ChordEvent[] = [];
  for (const old of m.patterns) {
    const mine = oldClips.filter((c) => c.patternId === old.id);
    if (legacy(old).trackId) {
      clips.push(...mine);
      patterns.push(old);
      continue;
    }
    const period = Math.max(1, old.lengthTicks);
    const turn = loopTurn(legacy(old).chordLoopTicks, period);
    const chords = layOutLoop(legacyChords(old), turn, period);
    for (const clip of mine) {
      for (const c of tileChords(chords, period, clip.offsetTicks + clip.lengthTicks)) {
        const start = Math.max(clip.startTick + c.startTick - clip.offsetTicks, clip.startTick);
        const cut = Math.min(clip.startTick + c.startTick - clip.offsetTicks + c.lengthTicks, clip.startTick + clip.lengthTicks);
        if (cut > start) lifted.push({ ...c, startTick: start, lengthTicks: cut - start });
      }
    }
    for (const t of m.tracks) {
      const own = legacy(old).notesByTrack?.[t.id] ?? [];
      if (own.length === 0) continue;
      const split: SongPattern = {
        ...newPattern(t.id, `${old.name} ${t.name}`),
        lengthTicks: old.lengthTicks,
        notes: layOutLoop(own, loopTurn(legacy(old).trackLoopTicks?.[t.id], period), period),
      };
      clips.push(...mine.map((c) => ({ ...c, id: uuid(), patternId: split.id })));
      patterns.push(split);
    }
  }
  m.patterns = patterns;
  m.clips = clips;
  m.chords = [...(m.chords ?? []), ...lifted];
  // A pre-v10 file never said how long the song was: it ended with its last clip.
  const end = clips.reduce((max, c) => Math.max(max, c.startTick + c.lengthTicks), 0);
  m.songLengthTicks = Math.max(m.songLengthTicks ?? 0, end);
}

/** A legacy loop length made usable: inside the pattern, or the whole pattern when unset. */
function loopTurn(ticks: number | null | undefined, period: number): number {
  return ticks != null && Number.isFinite(ticks) && ticks > 0 ? Math.min(ticks, period) : period;
}

/**
 * Decoding plus `ProjectModel::normalized`, run on anything read back from storage. A file
 * older than schema 6 is refused: there is no migration from patterns that hold every track.
 */
function normalizedModel(m: ProjectModel): ProjectModel {
  // Only ever bump forward, like `normalized()`: relabelling a newer file as ours would make the
  // next save overwrite it with the fields this build dropped.
  migrateBeats(m);
  migrateV10(m);
  m.schemaVersion = Math.max(m.schemaVersion ?? 0, SCHEMA_VERSION);
  if (m.tracks.length === 0) m.tracks = [newTrack(false, "melody")];
  // `#[serde(default)]` on the Rust side: a project saved before plugins existed has no field.
  for (const t of m.tracks) t.plugin ??= null;
  // Same `#[serde(default)]` story for the master: a project saved before the limiter existed
  // opens with it on, not off, because a bare `false` here would be a different mix.
  m.master = { ...DEFAULT_MASTER, ...m.master };
  const trackIds = new Set(m.tracks.map((t) => t.id));
  // A pattern of a track that no longer exists is invisible and never played.
  m.patterns = (m.patterns ?? []).filter((p) => trackIds.has(p.trackId));
  for (const p of m.patterns) p.notes = tidyRopes((p.notes ?? []).map(sanitizeNote));
  // An audio track is never also a drum track, and always carries a rig; a note track never
  // does. Hand-edited files can carry either combination.
  for (const t of m.tracks) {
    t.isAudio ??= false;
    if (t.isAudio) {
      t.isDrum = false;
      t.amp = sanitizeRig(t.amp ?? defaultAmpRig()) ?? defaultAmpRig();
    } else {
      t.amp = null;
    }
  }
  // A hand-edited kit the commands would refuse (empty, too big, repeated pitches) falls back
  // to the default kit rather than playing half-broken, and a partial embedded sound is filled.
  for (const t of m.tracks) if (!validKit(t.drumPads)) t.drumPads = defaultKit();
  if (m.sounds) m.sounds = m.sounds.map(sanitizedSound);
  pruneSounds(m);
  m.chords = m.chords ?? [];
  m.songLengthTicks = Number.isFinite(m.songLengthTicks)
    ? Math.max(barOf(m), m.songLengthTicks)
    : defaultSongBars() * barOf(m);
  m.clips = (m.clips ?? [])
    .filter((c) => m.patterns.some((p) => p.id === c.patternId))
    .filter((c) => c.startTick < m.songLengthTicks)
    .map((c) => ({ ...c, lengthTicks: Math.min(c.lengthTicks, m.songLengthTicks - c.startTick) }));
  // A brace left hanging past the end (hand-edited, or saved before the song was shortened)
  // would loop over silence.
  m.loopRegion = clampLoop(m.loopRegion, m.songLengthTicks);
  m.punchRegion = clampLoop(m.punchRegion, m.songLengthTicks);
  foldImplicitExtent(m.sections ?? []);
  // Unreferenced sources stay: undoing the clip that used one must not force a re-decode.
  m.audioSources ??= [];
  const audioTrackIds = new Set(
    m.tracks.filter((t) => t.isAudio).map((t) => t.id),
  );
  const sourceIds = new Set(m.audioSources.map((s) => s.id));
  m.audioClips = (m.audioClips ?? [])
    .filter((c) => audioTrackIds.has(c.trackId) && sourceIds.has(c.sourceId))
    .map(sanitizeAudioClip)
    .filter((c): c is AudioClip => c !== null)
    .sort((a, b) => a.startTick - b.startTick);
  // Detection results of a take that is gone describe nothing on the song any more.
  const clipIds = new Set(m.audioClips.map((c) => c.id));
  const seen = new Set<string>();
  const detections = (m.detections ?? [])
    .filter((d) => clipIds.has(d.clipId) && !seen.has(d.clipId) && (seen.add(d.clipId), true))
    .map(sanitizeDetection);
  if (detections.length) m.detections = detections;
  else delete m.detections;
  // Only when the key is there: an absent chain is how a project with no effects is written,
  // and filling it in would put `"inserts": []` on every track of every saved file.
  for (const t of m.tracks) if (t.inserts) t.inserts = sanitizeInserts(t.inserts);
  if (m.masterInserts) m.masterInserts = sanitizeInserts(m.masterInserts);
  // A lane of a deleted track is invisible and never played, but would survive every save;
  // the same rule patterns get. One lane per target, so the UI never has two drawings of one
  // parameter fighting.
  if (m.automation) {
    const lanes = m.automation.filter((l) => {
      const id = autoTrackId(l.target);
      return id === null || trackIds.has(id);
    });
    for (const l of lanes) {
      l.id ??= uuid();
      l.points ??= [];
      tidyLane(l);
    }
    const seen: AutoTarget[] = [];
    const kept = lanes.filter((l) => {
      if (l.points.length === 0 || seen.some((t) => sameAutoTarget(t, l.target))) return false;
      seen.push(l.target);
      return true;
    });
    if (kept.length) m.automation = kept;
    else delete m.automation;
  }
  return m;
}

/**
 * `AddClip` / `UpdateClip` clamping: a clip may not start at or after the song end, and its end
 * is cut back to it. Returns the clamped length in ticks.
 */
function clampToSong(m: ProjectModel, startTick: number, lengthTicks: number): number {
  if (startTick >= m.songLengthTicks) fail(PAST_SONG_END);
  return Math.min(lengthTicks, m.songLengthTicks - startTick);
}

/** `UpdateClip`: the clip id, then values; the pattern never changes (a clip stays on its track). */
function updateClip(m: ProjectModel, clip: Clip): void {
  const i = m.clips.findIndex((cl) => cl.id === clip.id);
  if (i < 0) fail(NO_SUCH_CLIP);
  if (!allFinite(clip.startTick, clip.lengthTicks, clip.offsetTicks)) fail(INVALID_VALUE);
  if (clip.startTick < 0 || clip.lengthTicks <= 0 || clip.offsetTicks < 0) fail(INVALID_VALUE);
  const lengthTicks = clampToSong(m, clip.startTick, clip.lengthTicks);
  m.clips[i] = { ...clip, patternId: m.clips[i].patternId, lengthTicks };
}

/** Mirrors `clamp_loop`: a brace held inside `[0, songEnd]`, or null when nothing is left of it. */
function clampLoop(region: [number, number] | null | undefined, songEnd: number): [number, number] | null {
  if (!region) return null;
  const start = Math.min(Math.max(region[0], 0), songEnd);
  const end = Math.min(Math.max(region[1], 0), songEnd);
  return end > start ? [start, end] : null;
}

/**
 * `Section::length_ticks` is skipped when it is the implicit extent, so an absent field and a
 * zero mean the same thing — and a project whose sections all run to the next marker is
 * byte-identical to one saved before extents existed. The mock's model *is* its file, so it
 * drops the key the same way.
 */
function foldImplicitExtent(sections: Section[]): void {
  for (const s of sections) if (!s.lengthTicks || s.lengthTicks < 0) delete s.lengthTicks;
}

/**
 * Mirrors `tidy_chords`: drop the nonsense, fold each chord's pitch classes into 0–11 keeping
 * their order (root first), sort by start, and cut each chord at the next one's start so none
 * overlap.
 */
function tidyChords(chords: ChordEvent[]): ChordEvent[] {
  return chords
    .filter((ch) => allFinite(ch.startTick, ch.lengthTicks) && ch.lengthTicks > 0 && ch.startTick >= 0)
    .map((ch) => ({ ...ch, pitchClasses: [...new Set(ch.pitchClasses.map(normalize))] }))
    .sort((a, b) => a.startTick - b.startTick)
    .map((ch, i, all) => (all[i + 1] ? { ...ch, lengthTicks: Math.min(ch.lengthTicks, all[i + 1].startTick - ch.startTick) } : ch))
    .filter((ch) => ch.lengthTicks > 0);
}

// ─── Ripple edits ──────────────────────────────────────────────────────────────────────────
// `crates/dissonant-core/src/ripple.rs`, rule for rule. The tick math itself is shared with
// the song view through `clipEditing`, so the preview a menu shows and the edit the backend
// makes cannot disagree.

/** `ripple.rs::split_clip`: the first piece keeps the clip's identity, a second one picks the
 *  pattern up exactly where the first left it. */
function splitClip(clip: Clip, pieces: Piece[]): Clip[] {
  return pieces.map((p, i) => ({
    ...clip,
    id: i === 0 ? clip.id : uuid(),
    startTick: p.start,
    lengthTicks: p.len,
    offsetTicks: clip.offsetTicks + p.skip,
  }));
}

/** `ripple.rs::split_chord`: both halves still spell the same notes. */
function splitChord(chord: ChordEvent, pieces: Piece[]): ChordEvent[] {
  return pieces.map((p, i) => ({ ...chord, id: i === 0 ? chord.id : uuid(), startTick: p.start, lengthTicks: p.len }));
}

/** `ripple.rs::split_detected`: a candidate stays over the music it was heard in. */
function splitDetected(m: ProjectModel, pieces: (c: DetectedChord) => Piece[]): void {
  for (const d of m.detections ?? []) {
    d.chords = d.chords.flatMap((c) =>
      pieces(c).map((p, i) => ({ ...c, id: i === 0 ? c.id : uuid(), startTick: p.start, lengthTicks: p.len })),
    );
  }
}

/** `ClipDetection::sanitized`: chords sorted, cut so none overlap, confidence in 0…1. */
function sanitizeDetection(d: ClipDetection): ClipDetection {
  const chords = (d.chords ?? [])
    .filter((c) => allFinite(c.startTick, c.lengthTicks) && c.startTick >= 0 && c.lengthTicks > 0)
    .map((c) => ({
      ...c,
      startTick: Math.round(c.startTick),
      lengthTicks: Math.round(c.lengthTicks),
      pitchClasses: [...new Set(c.pitchClasses.map(normalize))],
      confidence: Number.isFinite(c.confidence) ? clamp(c.confidence, 0, 1) : 0,
    }))
    .filter((c) => c.pitchClasses.length > 0)
    .sort((a, b) => a.startTick - b.startTick);
  for (let i = 0; i + 1 < chords.length; i++)
    chords[i].lengthTicks = Math.min(chords[i].lengthTicks, chords[i + 1].startTick - chords[i].startTick);
  const chroma = Array.from({ length: 12 }, (_, i) => {
    const v = d.chroma?.[i] ?? 0;
    return Number.isFinite(v) && v > 0 ? v : 0;
  });
  return { clipId: d.clipId, chords: chords.filter((c) => c.lengthTicks > 0).slice(0, MAX_DETECTED_CHORDS), chroma };
}

/** `document.rs::take_detected_chord`: out of the lane by id; an emptied detection goes too. */
function takeDetectedChord(m: ProjectModel, id: string): DetectedChord {
  const list = m.detections ?? [];
  const d = list.findIndex((x) => x.chords.some((c) => c.id === id));
  if (d < 0) fail(NO_SUCH_DETECTED_CHORD);
  const det = list[d];
  const chord = det.chords.find((c) => c.id === id)!;
  det.chords = det.chords.filter((c) => c.id !== id);
  if (det.chords.length === 0 && det.chroma.every((v) => v <= 0)) list.splice(d, 1);
  if (list.length === 0) delete m.detections;
  return chord;
}

/**
 * `ripple.rs::split_audio`. Audio never warps: the split point goes through the tempo map (the
 * one from before the edit) to find how many seconds of the file each piece holds. The leading
 * fade stays on the first piece and the trailing one on the last.
 */
function splitAudio(secondsAt: (tick: number) => number, clip: AudioClip, pieces: Piece[], len: number): AudioClip[] {
  const anchor = secondsAt(clip.startTick);
  const into = (tick: number) => clamp(secondsAt(tick) - anchor, 0, clip.lengthSeconds);
  const last = pieces.length - 1;
  return pieces
    .map((p, i) => {
      const tail = p.skip + p.len >= len;
      const from = into(clip.startTick + p.skip);
      return {
        ...clip,
        id: i === 0 ? clip.id : uuid(),
        startTick: p.start,
        lengthSeconds: (tail ? clip.lengthSeconds : into(clip.startTick + p.skip + p.len)) - from,
        offsetSeconds: clip.offsetSeconds + from,
        fadeInSeconds: p.skip === 0 ? clip.fadeInSeconds : 0,
        fadeOutSeconds: i === last && tail ? clip.fadeOutSeconds : 0,
      };
    })
    .map(sanitizeAudioClip)
    .filter((c): c is AudioClip => c !== null);
}

/**
 * How long a take is in ticks — the only thing a timeline edit can reason about. The map is
 * monotonic, so its inverse is a bisection; `tempoMap` only goes one way.
 */
function audioLenTicks(secondsAt: (tick: number) => number, clip: AudioClip): number {
  const target = secondsAt(clip.startTick) + clip.lengthSeconds;
  let lo = clip.startTick;
  let hi = clip.startTick + TICKS_PER_BEAT;
  while (secondsAt(hi) < target) hi *= 2;
  for (let i = 0; i < 60; i++) {
    const mid = (lo + hi) / 2;
    if (secondsAt(mid) < target) lo = mid;
    else hi = mid;
  }
  return Math.max(0, Math.round(hi) - clip.startTick);
}

/** `ripple.rs::settle`: everything back in order, nothing reaching past the song's end. */
function settle(m: ProjectModel): void {
  m.clips = m.clips
    .filter((c) => c.startTick < m.songLengthTicks)
    .map((c) => (c.startTick + c.lengthTicks > m.songLengthTicks ? { ...c, lengthTicks: m.songLengthTicks - c.startTick } : c))
    .sort((a, b) => a.startTick - b.startTick);
  m.audioClips?.sort((a, b) => a.startTick - b.startTick);
  m.sections.sort((a, b) => a.startTick - b.startTick);
  foldImplicitExtent(m.sections);
  m.tempoPoints?.sort((a, b) => a.tick - b.tick);
  m.loopRegion = clampLoop(m.loopRegion, m.songLengthTicks);
  m.punchRegion = clampLoop(m.punchRegion, m.songLengthTicks);
}

/** `ripple.rs::insert_time`. */
function insertTime(m: ProjectModel, at: number, ticks: number): void {
  const { secondsAt } = tempoMap(m.tempo, m.tempoPoints);
  m.clips = m.clips.flatMap((c) => splitClip(c, insertPieces(c.startTick, c.lengthTicks, at, ticks)));
  m.audioClips = (m.audioClips ?? []).flatMap((c) => {
    const len = audioLenTicks(secondsAt, c);
    return splitAudio(secondsAt, c, insertPieces(c.startTick, len, at, ticks), len);
  });
  m.sections = m.sections.map((s) =>
    // Time opened inside a section is more of that section: a marker names a stretch of music,
    // not a piece of it.
    (s.lengthTicks ?? 0) > 0 && s.startTick < at && at < s.startTick + (s.lengthTicks ?? 0)
      ? { ...s, lengthTicks: (s.lengthTicks ?? 0) + ticks }
      : { ...s, startTick: insertPoint(s.startTick, at, ticks) },
  );
  m.tempoPoints = (m.tempoPoints ?? []).map((p) => ({ ...p, tick: insertPoint(p.tick, at, ticks) }));
  m.chords = tidyChords(m.chords.flatMap((ch) => splitChord(ch, insertPieces(ch.startTick, ch.lengthTicks, at, ticks))));
  splitDetected(m, (c) => insertPieces(c.startTick, c.lengthTicks, at, ticks));
  for (const lane of m.automation ?? []) {
    lane.points = lane.points.map((p) => ({ ...p, tick: insertPoint(p.tick, at, ticks) }));
    tidyLane(lane);
  }
  // Time opened inside the brace lengthens it; a brace ending where the time opens does not.
  // The punch range is a place in the music too, so it moves by the same rule.
  const grow = (r: [number, number] | null | undefined): [number, number] | null =>
    r ? [insertPoint(r[0], at, ticks), r[1] > at ? r[1] + ticks : r[1]] : null;
  m.loopRegion = grow(m.loopRegion);
  m.punchRegion = grow(m.punchRegion);
  m.songLengthTicks = Math.max(m.songLengthTicks, at) + ticks;
  settle(m);
}

/** `ripple.rs::delete_time`. */
function deleteTime(m: ProjectModel, at: number, ticks: number): void {
  const { secondsAt } = tempoMap(m.tempo, m.tempoPoints);
  const cutEnd = at + ticks;
  m.clips = m.clips.flatMap((c) => splitClip(c, deletePieces(c.startTick, c.lengthTicks, at, ticks)));
  m.audioClips = (m.audioClips ?? []).flatMap((c) => {
    const len = audioLenTicks(secondsAt, c);
    return splitAudio(secondsAt, c, deletePieces(c.startTick, len, at, ticks), len);
  });
  const spans = m.sections.map((s) => sectionSpan(m, s));
  m.sections = m.sections.flatMap((s, i) => {
    const [s0, s1] = spans[i];
    // A section whose whole extent was inside the cut goes with its music.
    if (s0 >= at && s0 < cutEnd && s1 <= cutEnd) return [];
    const startTick = deletePoint(s0, at, ticks);
    const explicit = (s.lengthTicks ?? 0) > 0;
    const lengthTicks = explicit ? Math.max(0, deletePoint(s1, at, ticks) - startTick) : 0;
    // An explicit extent cut away to nothing is a section with no music left in it.
    return explicit && lengthTicks === 0 ? [] : [{ ...s, startTick, lengthTicks }];
  });
  // A tempo change inside the cut goes with it: what was in force before it plays on.
  m.tempoPoints = (m.tempoPoints ?? []).filter((p) => p.tick < at || p.tick >= cutEnd).map((p) => ({ ...p, tick: deletePoint(p.tick, at, ticks) }));
  m.chords = tidyChords(m.chords.flatMap((ch) => splitChord(ch, deletePieces(ch.startTick, ch.lengthTicks, at, ticks))));
  splitDetected(m, (c) => deletePieces(c.startTick, c.lengthTicks, at, ticks));
  for (const lane of m.automation ?? []) {
    lane.points = lane.points.filter((p) => p.tick < at || p.tick >= cutEnd).map((p) => ({ ...p, tick: deletePoint(p.tick, at, ticks) }));
    tidyLane(lane);
  }
  // A lane with every point inside the cut has nothing left to say.
  m.automation = (m.automation ?? []).filter((l) => l.points.length > 0);
  const pull = (r: [number, number] | null | undefined): [number, number] | null =>
    r ? [deletePoint(r[0], at, ticks), deletePoint(r[1], at, ticks)] : null;
  m.loopRegion = pull(m.loopRegion);
  m.punchRegion = pull(m.punchRegion);
  m.songLengthTicks = Math.max(barOf(m), m.songLengthTicks - Math.max(0, Math.min(m.songLengthTicks, cutEnd) - at));
  settle(m);
}

/** `ripple.rs::duplicate_range`. */
function duplicateRange(m: ProjectModel, from: number, to: number, at: number): void {
  const span = to - from;
  const { secondsAt } = tempoMap(m.tempo, m.tempoPoints);
  // Taken before the time is opened, so duplicating a range onto its own start still
  // reproduces what was there rather than what the insert just moved.
  const clips = m.clips.map((c) => ({ ...c }));
  const audio = (m.audioClips ?? []).map((c) => ({ ...c }));
  const sections = m.sections.filter((s) => s.startTick >= from && s.startTick < to).map((s) => ({ s: { ...s }, span: sectionSpan(m, s) }));
  const tempo = (m.tempoPoints ?? []).filter((p) => p.tick >= from && p.tick < to).map((p) => ({ ...p }));
  const chords = m.chords.map((c) => ({ ...c }));
  const lanes = (m.automation ?? []).map((l) => ({ target: l.target, points: l.points.filter((p) => p.tick >= from && p.tick < to).map((p) => ({ ...p })) }));

  insertTime(m, at, span);

  for (const c of clips) {
    const p = windowPiece(c.startTick, c.lengthTicks, from, to, at);
    if (p) m.clips.push({ ...c, id: uuid(), startTick: p.start, lengthTicks: p.len, offsetTicks: c.offsetTicks + p.skip });
  }
  for (const c of audio) {
    const len = audioLenTicks(secondsAt, c);
    const p = windowPiece(c.startTick, len, from, to, at);
    // A copy is a second take of the same source, not the same clip twice.
    if (p) m.audioClips = [...(m.audioClips ?? []), ...splitAudio(secondsAt, c, [p], len).map((copy) => ({ ...copy, id: uuid() }))];
  }
  for (const { s, span: was } of sections) {
    m.sections.push({
      ...s,
      id: uuid(),
      startTick: at + (was[0] - from),
      // An explicit extent is cut at the range's end; an implicit one stays implicit, so the
      // copy runs to whatever marker follows it, exactly as the original did.
      lengthTicks: (s.lengthTicks ?? 0) > 0 ? Math.max(1, Math.min(was[1], to) - was[0]) : 0,
    });
  }
  for (const p of tempo) {
    const tick = at + (p.tick - from);
    m.tempoPoints = [...(m.tempoPoints ?? []).filter((x) => x.tick !== tick), { ...p, id: uuid(), tick }];
  }
  const all = [...m.chords];
  for (const c of chords) {
    const p = windowPiece(c.startTick, c.lengthTicks, from, to, at);
    if (p) all.push({ ...c, id: uuid(), startTick: p.start, lengthTicks: p.len });
  }
  m.chords = tidyChords(all);
  for (const { target, points } of lanes) {
    const lane = (m.automation ?? []).find((l) => sameAutoTarget(l.target, target));
    if (!lane || points.length === 0) continue;
    lane.points = [...lane.points, ...points.map((p) => ({ ...p, id: uuid(), tick: at + (p.tick - from) }))];
    tidyLane(lane);
  }
  settle(m);
}

function clone<T>(x: T): T {
  return JSON.parse(JSON.stringify(x));
}

/**
 * Mirrors `apply_to` in crates/dissonant-core/src/document.rs: same validation, same clamping,
 * same errors. It mutates a throwaway clone, so a thrown error leaves the model untouched —
 * exactly like the Rust version, which discards `next` on `Err`.
 */
function reduce(m: ProjectModel, c: Command): void {
  const track = (id: string) =>
    m.tracks.find((t) => t.id === id) ?? fail(NO_SUCH_TRACK);
  const pattern = (id: string) =>
    m.patterns.find((p) => p.id === id) ?? fail(NO_SUCH_PATTERN);
  switch (c.type) {
    case "setTempo":
      if (!allFinite(c.bpm)) fail(INVALID_VALUE);
      m.tempo = clamp(c.bpm, 20, 300);
      break;
    case "setKey":
      m.key = { ...c.key };
      break;
    case "addTrack": {
      // A new track comes with one empty pattern, so there is something to open right away.
      const t = newTrack(c.isDrum, c.isDrum ? "drums" : `track ${m.tracks.length + 1}`);
      m.tracks.push(t);
      m.patterns.push(newPattern(t.id, `${t.name} 1`));
      break;
    }
    case "setTrackColor":
      if (c.color !== null && !/^#[0-9a-f]{6}$/i.test(c.color))
        fail(INVALID_VALUE);
      track(c.id).color = c.color === null ? null : c.color.toLowerCase();
      break;
    case "deleteTrack": {
      // Like Rust: an unknown id is "no such track" even when only one track is left.
      track(c.id);
      if (m.tracks.length <= 1) fail(LAST_TRACK);
      m.tracks = m.tracks.filter((t) => t.id !== c.id);
      // Its patterns go with it, and their clips with them.
      const gone = new Set(m.patterns.filter((p) => p.trackId === c.id).map((p) => p.id));
      m.patterns = m.patterns.filter((p) => !gone.has(p.id));
      m.clips = m.clips.filter((cl) => !gone.has(cl.patternId));
      m.audioClips = (m.audioClips ?? []).filter((a) => a.trackId !== c.id);
      const lanes = (m.automation ?? []).filter((l) => autoTrackId(l.target) !== c.id);
      if (lanes.length) m.automation = lanes;
      else delete m.automation;
      pruneSounds(m);
      break;
    }
    case "renameTrack":
      track(c.id).name = c.name;
      break;
    case "setTrackMuted":
      track(c.id).muted = c.muted;
      break;
    case "setTrackSoloed":
      track(c.id).soloed = c.soloed;
      break;
    case "moveTrack": {
      const i = m.tracks.findIndex((t) => t.id === c.id);
      if (i < 0) fail(NO_SUCH_TRACK);
      const j = c.up ? i - 1 : i + 1;
      if (j >= 0 && j < m.tracks.length)
        [m.tracks[i], m.tracks[j]] = [m.tracks[j], m.tracks[i]];
      break;
    }
    case "setTrackVoice":
      track(c.id).voice = c.voice;
      pruneSounds(m);
      break;
    case "setTrackSound": {
      const sound = sanitizedSound(c.sound);
      const t = track(c.id);
      if (t.isDrum || sound.patch.kind !== "synth") fail(INVALID_VALUE);
      t.voice = sound.id;
      if (!isFactory(sound.id)) embedSound(m, sound);
      pruneSounds(m);
      break;
    }
    case "setDrumPads": {
      const sounds = c.sounds.map(sanitizedSound);
      // Pitch and choke are `u8` in Rust: anything else fails to decode before `apply_to` runs.
      const byte = (x: number) => Number.isInteger(x) && x >= 0 && x <= 255;
      if (!c.pads.every((p) => byte(p.pitch) && byte(p.choke) && typeof p.sound === "string")) fail(INVALID_VALUE);
      if (!validKit(c.pads) || sounds.some((s) => s.patch.kind !== "drum")) fail(INVALID_VALUE);
      if (!track(c.id).isDrum) fail(INVALID_VALUE);
      for (const s of sounds) if (!isFactory(s.id)) embedSound(m, s);
      if (c.pads.some((p) => lookupSound(m.sounds ?? [], p.sound)?.patch.kind !== "drum")) fail(NO_SUCH_SOUND);
      track(c.id).drumPads = c.pads.map((p) => ({ pitch: p.pitch, sound: p.sound, choke: p.choke }));
      pruneSounds(m);
      break;
    }
    case "upsertSound": {
      const sound = sanitizedSound(c.sound);
      const i = (m.sounds ?? []).findIndex((s) => s.id === sound.id);
      if (i < 0) fail(NO_SUCH_SOUND);
      if (m.sounds![i].patch.kind !== sound.patch.kind) fail(INVALID_VALUE);
      m.sounds![i] = sound;
      break;
    }
    case "setTrackParam": {
      if (!allFinite(c.value)) fail(INVALID_VALUE);
      const [lo, hi] = TRACK_PARAM_RANGE[c.param];
      track(c.id)[c.param] = clamp(c.value, lo, hi);
      break;
    }
    case "setTrackPlugin": {
      const t = track(c.id);
      if (c.plugin === null) {
        t.plugin = null;
        break;
      }
      const uid = c.plugin.uid.trim();
      if (!uid) fail(INVALID_VALUE);
      // A state blob belongs to the plugin that wrote it, so it survives re-picking the same
      // plugin and is dropped the moment the uid changes.
      const kept = t.plugin?.uid === uid ? t.plugin.state : null;
      t.plugin = {
        uid,
        name: c.plugin.name.trim() || uid,
        state: c.plugin.state ?? kept,
      };
      break;
    }
    case "setPluginState": {
      const t = track(c.id);
      if (!t.plugin) fail(NO_PLUGIN);
      t.plugin = { ...t.plugin, state: c.state };
      break;
    }
    case "addInsert": {
      // A blank uid names nothing: the chain would carry a slot that can never load.
      if (!c.uid.trim()) fail(INVALID_VALUE);
      const chain = chainMut(m, c.target);
      if (chain.length >= MAX_INSERTS) fail(TOO_MANY_INSERTS);
      const at = clamp(c.index ?? chain.length, 0, chain.length);
      chain.splice(at, 0, {
        id: uuid(),
        uid: c.uid,
        name: c.name,
        enabled: true,
      });
      break;
    }
    case "removeInsert": {
      const chain = chainMut(m, c.target);
      const at = chain.findIndex((i) => i.id === c.insertId);
      if (at < 0) fail(NO_SUCH_INSERT);
      chain.splice(at, 1);
      break;
    }
    case "moveInsert": {
      const chain = chainMut(m, c.target);
      const from = chain.findIndex((i) => i.id === c.insertId);
      if (from < 0) fail(NO_SUCH_INSERT);
      // Dragging past either end lands on that end rather than failing: the UI's drop targets
      // and the chain's bounds do not have to agree.
      const to = clamp(c.toIndex, 0, chain.length - 1);
      chain.splice(to, 0, ...chain.splice(from, 1));
      break;
    }
    case "setInsertEnabled":
      insertMut(m, c.target, c.insertId).enabled = c.on;
      break;
    case "setInsertState": {
      if (!isBase64(c.state)) fail(INVALID_VALUE);
      insertMut(m, c.target, c.insertId).state = c.state;
      break;
    }
    case "setInsertParam": {
      const insert = insertMut(m, c.target, c.insertId);
      const effect = nativeEffect(insert.uid) ?? fail(NOT_A_NATIVE_EFFECT);
      const param =
        effect.params.find((x) => x.id === c.paramId) ?? fail(NO_SUCH_PARAM);
      if (!allFinite(c.value)) fail(INVALID_VALUE);
      insert.params = {
        ...insert.params,
        [String(c.paramId)]: clampNativeParam(param, c.value),
      };
      break;
    }
    case "addPattern":
      m.patterns.push(newPattern(c.trackId, nextPatternName(m, track(c.trackId))));
      break;
    case "duplicatePattern": {
      const i = m.patterns.findIndex((p) => p.id === c.id);
      if (i < 0) fail(NO_SUCH_PATTERN);
      m.patterns.splice(i + 1, 0, copyPattern(m.patterns[i]));
      break;
    }
    case "deletePattern": {
      // No last-pattern guard: a track may end up with no patterns at all.
      const before = m.patterns.length;
      m.patterns = m.patterns.filter((p) => p.id !== c.id);
      if (m.patterns.length === before) fail(NO_SUCH_PATTERN);
      m.clips = m.clips.filter((cl) => cl.patternId !== c.id);
      break;
    }
    case "renamePattern":
      if (!c.name.trim()) fail(INVALID_VALUE);
      pattern(c.id).name = c.name;
      break;
    case "setPatternLength":
      if (!allFinite(c.ticks) || c.ticks < TICKS_PER_BEAT) fail(INVALID_VALUE);
      pattern(c.id).lengthTicks = c.ticks;
      break;
    case "setNotes":
      // Deleting a note is committing the array without it; tidying is what cuts the ropes
      // that pointed at it.
      pattern(c.patternId).notes = tidyRopes(
        c.notes.map(sanitizeNote).sort((a, b) => a.startTick - b.startTick || a.pitch - b.pitch),
      );
      break;
    case "setRope": {
      const p = pattern(c.patternId);
      if (!p.notes.some((n) => n.id === c.sourceId)) fail(NO_SUCH_NOTE);
      // Meeting the target upstream of the source means the rope would close a loop.
      let from: string | undefined = c.sourceId;
      for (let i = 0; i <= p.notes.length && from !== undefined; i++) {
        if (from === c.noteId) fail(ROPE_LOOP);
        const id: string = from;
        from = p.notes.find((n) => n.id === id)?.slideFrom;
      }
      const target = p.notes.find((n) => n.id === c.noteId) ?? fail(NO_SUCH_NOTE);
      target.slideFrom = c.sourceId;
      break;
    }
    case "clearRope": {
      const target = pattern(c.patternId).notes.find((n) => n.id === c.noteId) ?? fail(NO_SUCH_NOTE);
      delete target.slideFrom;
      delete target.slideSecs;
      break;
    }
    case "setSlideTime": {
      if (c.secs !== null && !Number.isFinite(c.secs)) fail(INVALID_VALUE);
      const target = pattern(c.patternId).notes.find((n) => n.id === c.noteId) ?? fail(NO_SUCH_NOTE);
      if (!target.slideFrom) fail(NO_ROPE);
      if (c.secs === null) delete target.slideSecs;
      else target.slideSecs = clamp(c.secs, 0, MAX_SLIDE_SECS);
      break;
    }
    case "setChords":
      // The project progression, in song ticks.
      m.chords = tidyChords(c.chords);
      break;
    case "addClip": {
      const p = pattern(c.patternId);
      const length = c.lengthTicks ?? p.lengthTicks;
      if (!allFinite(c.startTick, length) || c.startTick < 0 || length <= 0) fail(INVALID_VALUE);
      const lengthTicks = clampToSong(m, c.startTick, length);
      m.clips.push({ id: uuid(), patternId: p.id, startTick: c.startTick, lengthTicks, offsetTicks: 0, muted: false });
      m.clips.sort((a, b) => a.startTick - b.startTick);
      break;
    }
    case "updateClip":
      updateClip(m, c.clip);
      m.clips.sort((a, b) => a.startTick - b.startTick);
      break;
    case "updateClips":
      // One command, so one undo step: a group move.
      for (const clip of c.clips) updateClip(m, clip);
      m.clips.sort((a, b) => a.startTick - b.startTick);
      break;
    case "removeClip": {
      const before = m.clips.length;
      m.clips = m.clips.filter((cl) => cl.id !== c.id);
      if (m.clips.length === before) fail(NO_SUCH_CLIP);
      break;
    }
    case "removeClips": {
      // Unknown ids are skipped, as in the Rust document.
      const ids = new Set(c.ids);
      m.clips = m.clips.filter((cl) => !ids.has(cl.id));
      break;
    }
    case "makeClipUnique": {
      // Fork the clip's pattern (like duplicatePattern) and point just this clip at the copy.
      const clip = m.clips.find((cl) => cl.id === c.id) ?? fail(NO_SUCH_CLIP);
      const i = m.patterns.findIndex((p) => p.id === clip.patternId);
      if (i < 0) fail(NO_SUCH_PATTERN);
      const copy = copyPattern(m.patterns[i]);
      m.patterns.splice(i + 1, 0, copy);
      clip.patternId = copy.id;
      break;
    }
    case "setSongLength": {
      if (!allFinite(c.ticks)) fail(INVALID_VALUE);
      const end = Math.max(barOf(m), Math.round(c.ticks));
      m.songLengthTicks = end;
      // Clips starting at or after the new end go; clips crossing it are trimmed.
      m.clips = m.clips
        .filter((cl) => cl.startTick < end)
        .map((cl) => (cl.startTick + cl.lengthTicks > end ? { ...cl, lengthTicks: end - cl.startTick } : cl));
      m.loopRegion = clampLoop(m.loopRegion, end);
      m.punchRegion = clampLoop(m.punchRegion, end);
      break;
    }
    case "setLoopRegion": {
      if (c.region === null) {
        m.loopRegion = null;
        break;
      }
      const [start, end] = c.region;
      // An inverted or empty brace is a bug in the caller, not a way to clear: clearing says
      // so with `null`.
      if (!allFinite(start, end) || start < 0 || end <= start) fail(INVALID_VALUE);
      m.loopRegion = clampLoop([start, end], m.songLengthTicks) ?? fail(INVALID_VALUE);
      break;
    }
    case "setPunchRegion": {
      if (c.region === null) {
        m.punchRegion = null;
        break;
      }
      const [start, end] = c.region;
      // Same rule as the loop brace: disarming says so with `null`.
      if (!allFinite(start, end) || start < 0 || end <= start) fail(INVALID_VALUE);
      m.punchRegion = clampLoop([start, end], m.songLengthTicks) ?? fail(INVALID_VALUE);
      break;
    }
    case "addSection":
      if (!allFinite(c.startTick) || c.startTick < 0) fail(INVALID_VALUE);
      m.sections.push({
        id: uuid(),
        name: c.name.trim() || `section ${m.sections.length + 1}`,
        startTick: Math.max(0, c.startTick),
        // No extent: a marker dropped on the ruler runs to the next one.
        key: null,
        color: null,
      });
      m.sections.sort((a, b) => a.startTick - b.startTick);
      break;
    case "updateSection": {
      // A negative extent is nonsense; 0 is the implicit one (to the next marker).
      if (
        !allFinite(c.section.startTick, c.section.lengthTicks ?? 0) ||
        c.section.startTick < 0 ||
        (c.section.lengthTicks ?? 0) < 0 ||
        !c.section.name.trim()
      )
        fail(INVALID_VALUE);
      const i = m.sections.findIndex((x) => x.id === c.section.id);
      if (i < 0) fail(NO_SUCH_SECTION);
      m.sections[i] = { ...c.section };
      foldImplicitExtent(m.sections);
      m.sections.sort((a, b) => a.startTick - b.startTick);
      break;
    }
    case "removeSection": {
      const before = m.sections.length;
      m.sections = m.sections.filter((x) => x.id !== c.id);
      if (m.sections.length === before) fail(NO_SUCH_SECTION);
      break;
    }
    case "setMaster": {
      const v = c.master;
      if (
        !allFinite(
          v.gain,
          v.reverbWet,
          v.lowCutHz,
          v.highCutHz,
          v.lowEq,
          v.midEq,
          v.highEq,
          v.ceilingDb,
        )
      )
        fail(INVALID_VALUE);
      const master: MasterSettings = {
        gain: clamp(v.gain, 0, 1.5),
        reverbWet: clamp(v.reverbWet, 0, 1),
        lowCutHz: clamp(v.lowCutHz, 10, 2_000),
        highCutHz: clamp(v.highCutHz, 500, 20_000),
        lowEq: clamp(v.lowEq, 0, 2),
        midEq: clamp(v.midEq, 0, 2),
        highEq: clamp(v.highEq, 0, 2),
        ceilingDb: clamp(v.ceilingDb, -12, 0),
        limiter: v.limiter,
      };
      m.master = master;
      break;
    }
    case "setTimeSignature":
      m.timeSignature = { numerator: c.numerator, denominator: c.denominator };
      break;
    case "setSwing":
      m.swing = Math.min(75, Math.max(50, c.swing));
      m.swingGridTicks =
        c.gridTicks === TICKS_PER_BEAT / 4
          ? TICKS_PER_BEAT / 4
          : DEFAULT_SWING_GRID_TICKS;
      break;
    case "addTempoPoint":
      if (c.tick < 0) break;
      m.tempoPoints = [
        ...(m.tempoPoints ?? []).filter((p) => p.tick !== c.tick),
        {
          id: uuid(),
          tick: c.tick,
          bpm: Math.min(300, Math.max(20, c.bpm)),
          ramp: c.ramp,
        },
      ].sort((a, b) => a.tick - b.tick);
      break;
    case "updateTempoPoint": {
      const i = (m.tempoPoints ?? []).findIndex((p) => p.id === c.point.id);
      if (i < 0 || c.point.tick < 0) break;
      m.tempoPoints = (m.tempoPoints ?? [])
        .filter((p) => p.id === c.point.id || p.tick !== c.point.tick)
        .map((p) =>
          p.id === c.point.id
            ? { ...c.point, bpm: Math.min(300, Math.max(20, c.point.bpm)) }
            : p,
        )
        .sort((a, b) => a.tick - b.tick);
      break;
    }
    case "removeTempoPoint":
      m.tempoPoints = (m.tempoPoints ?? []).filter((p) => p.id !== c.id);
      break;
    case "addAutomationPoint": {
      if (c.tick < 0 || !allFinite(c.value)) fail(INVALID_VALUE);
      checkAutoTarget(m, c.target);
      const lanes = (m.automation ??= []);
      let lane = lanes.find((l) => sameAutoTarget(l.target, c.target));
      if (!lane) {
        lane = { id: uuid(), target: c.target, points: [] };
        lanes.push(lane);
      }
      const range = autoTargetRange(c.target);
      lane.points = lane.points.filter((p) => p.tick !== c.tick);
      lane.points.push({
        id: uuid(),
        tick: c.tick,
        value: range ? clamp(c.value, range[0], range[1]) : c.value,
        curve: c.curve,
      });
      tidyLane(lane);
      break;
    }
    case "updateAutomationPoint": {
      if (c.point.tick < 0 || !allFinite(c.point.value)) fail(INVALID_VALUE);
      const lane =
        (m.automation ?? []).find((l) => l.id === c.lane) ??
        fail(NO_SUCH_AUTOMATION_LANE);
      const at = lane.points.findIndex((p) => p.id === c.point.id);
      if (at < 0) fail(NO_SUCH_AUTOMATION_POINT);
      lane.points[at] = { ...c.point, id: lane.points[at].id };
      // Keep one point per tick: dragging a point onto a neighbour swallows it.
      lane.points = lane.points.filter(
        (p) => p.id === c.point.id || p.tick !== c.point.tick,
      );
      tidyLane(lane);
      break;
    }
    case "removeAutomationPoint": {
      const lanes = m.automation ?? [];
      const i = lanes.findIndex((l) => l.id === c.lane);
      if (i < 0) fail(NO_SUCH_AUTOMATION_LANE);
      const before = lanes[i].points.length;
      lanes[i].points = lanes[i].points.filter((p) => p.id !== c.id);
      if (lanes[i].points.length === before) fail(NO_SUCH_AUTOMATION_POINT);
      // An empty lane is no automation at all; leaving it behind would make the file grow and
      // the sequence carry a curve with nothing to say.
      if (lanes[i].points.length === 0) lanes.splice(i, 1);
      if (lanes.length === 0) delete m.automation;
      break;
    }
    case "clearAutomation": {
      const lanes = m.automation ?? [];
      const kept = lanes.filter((l) => !sameAutoTarget(l.target, c.target));
      if (kept.length === lanes.length) fail(NO_SUCH_AUTOMATION_LANE);
      if (kept.length) m.automation = kept;
      else delete m.automation;
      break;
    }
    case "addAudioTrack":
      m.tracks.push(newAudioTrack(c.name.trim() === "" ? "audio" : c.name));
      break;
    case "setTrackAmp": {
      const rig = sanitizeRig(c.amp);
      if (rig === null) fail(INVALID_VALUE);
      const t = track(c.id);
      if (!t.isAudio) fail(NOT_AN_AUDIO_TRACK);
      t.amp = rig;
      break;
    }
    case "addAudioTake": {
      // The clip must name the track and source it arrived with, or `normalized` would drop
      // it and the take would silently vanish.
      if (c.clip.trackId !== c.trackId || c.clip.sourceId !== c.source.id)
        fail(INVALID_VALUE);
      if (
        c.source.sampleRate === 0 ||
        c.source.channels === 0 ||
        c.source.frames === 0
      )
        fail(INVALID_VALUE);
      const t = track(c.trackId);
      if (!t.isAudio) fail(NOT_AN_AUDIO_TRACK);
      const clip = sanitizeAudioClip(c.clip) ?? fail(INVALID_VALUE);
      m.audioSources ??= [];
      const at = m.audioSources.findIndex((s) => s.id === c.source.id);
      if (at >= 0) m.audioSources[at] = { ...c.source };
      else m.audioSources.push({ ...c.source });
      m.audioClips = [...(m.audioClips ?? []), clip].sort(
        (a, b) => a.startTick - b.startTick,
      );
      break;
    }
    case "punchAudioTake": {
      if (c.clip.trackId !== c.trackId || c.clip.sourceId !== c.source.id)
        fail(INVALID_VALUE);
      if (
        c.source.sampleRate === 0 ||
        c.source.channels === 0 ||
        c.source.frames === 0
      )
        fail(INVALID_VALUE);
      const t = track(c.trackId);
      if (!t.isAudio) fail(NOT_AN_AUDIO_TRACK);
      const clip = sanitizeAudioClip(c.clip) ?? fail(INVALID_VALUE);
      // A nonsense crossfade falls back to the default rather than refusing a take the player
      // has already performed.
      const asked = Number.isFinite(c.crossfadeSeconds) && c.crossfadeSeconds >= 0 ? c.crossfadeSeconds : DEFAULT_PUNCH_CROSSFADE_SECONDS;
      const xf = Math.min(asked, MAX_FADE_SECONDS, clip.lengthSeconds / 2);
      // The punch owns both of the take's edges.
      const take = { ...clip, fadeInSeconds: xf, fadeOutSeconds: xf };
      const { secondsAt } = tempoMap(m.tempo, m.tempoPoints);
      m.audioSources ??= [];
      const at = m.audioSources.findIndex((s) => s.id === c.source.id);
      if (at >= 0) m.audioSources[at] = { ...c.source };
      else m.audioSources.push({ ...c.source });
      m.audioClips = [...punchSplit(m.audioClips ?? [], take, xf, secondsAt, uuid), take].sort(
        (a, b) => a.startTick - b.startTick,
      );
      break;
    }
    case "updateAudioClip": {
      if (!(m.audioSources ?? []).some((s) => s.id === c.clip.sourceId))
        fail(NO_SUCH_AUDIO_SOURCE);
      const t = m.tracks.find((x) => x.id === c.clip.trackId);
      if (!t?.isAudio) fail(NOT_AN_AUDIO_TRACK);
      const clip = sanitizeAudioClip(c.clip) ?? fail(INVALID_VALUE);
      const at = (m.audioClips ?? []).findIndex((x) => x.id === clip.id);
      if (at < 0) fail(NO_SUCH_AUDIO_CLIP);
      m.audioClips![at] = clip;
      m.audioClips!.sort((a, b) => a.startTick - b.startTick);
      break;
    }
    case "removeAudioClip": {
      const before = (m.audioClips ?? []).length;
      m.audioClips = (m.audioClips ?? []).filter((x) => x.id !== c.id);
      if (m.audioClips.length === before) fail(NO_SUCH_AUDIO_CLIP);
      break;
    }
    case "insertTime":
      if (!allFinite(c.at, c.ticks) || c.at < 0 || c.ticks <= 0) fail(INVALID_VALUE);
      insertTime(m, c.at, c.ticks);
      break;
    case "deleteTime":
      if (!allFinite(c.at, c.ticks) || c.at < 0 || c.ticks <= 0) fail(INVALID_VALUE);
      deleteTime(m, c.at, c.ticks);
      break;
    case "duplicateRange":
      if (!allFinite(c.from, c.to, c.at) || c.from < 0 || c.to <= c.from || c.at < 0) fail(INVALID_VALUE);
      duplicateRange(m, c.from, c.to, c.at);
      break;
    case "addTranscription": {
      if (!allFinite(c.startTick, c.lengthTicks) || c.startTick < 0 || c.lengthTicks <= 0) fail(INVALID_VALUE);
      // Results for a take that is not on the song would be dropped on the next load.
      if (!(m.audioClips ?? []).some((a) => a.id === c.detection.clipId)) fail(NO_SUCH_AUDIO_CLIP);
      if (c.notes.length > 0) {
        const name = c.name.trim();
        const t = newTrack(false, name === "" ? "notes" : name);
        const length = Math.max(TICKS_PER_BEAT, Math.round(c.lengthTicks));
        const p: SongPattern = { ...newPattern(t.id, nextPatternName(m, t)), lengthTicks: length };
        // Inside the pattern or not at all: a note past its end would never sound.
        p.notes = c.notes
          .map(sanitizeNote)
          .filter((n) => n.startTick < length)
          .map((n) => ({ ...n, lengthTicks: Math.min(n.lengthTicks, length - n.startTick) }))
          .sort((a, b) => a.startTick - b.startTick || a.pitch - b.pitch);
        const startTick = Math.round(c.startTick);
        m.clips.push({ id: uuid(), patternId: p.id, startTick, lengthTicks: length, offsetTicks: 0, muted: false });
        m.clips.sort((a, b) => a.startTick - b.startTick);
        m.songLengthTicks = Math.max(m.songLengthTicks, startTick + length);
        m.tracks.push(t);
        m.patterns.push(p);
      }
      const d = sanitizeDetection(c.detection);
      const rest = (m.detections ?? []).filter((x) => x.clipId !== d.clipId);
      if (d.chords.length > 0 || d.chroma.some((v) => v > 0)) rest.push(d);
      if (rest.length) m.detections = rest;
      else delete m.detections;
      break;
    }
    case "acceptDetectedChord": {
      const found = takeDetectedChord(m, c.id);
      if (found.startTick >= m.songLengthTicks) fail(PAST_SONG_END);
      const chord: ChordEvent = {
        id: found.id,
        startTick: found.startTick,
        lengthTicks: Math.min(found.lengthTicks, m.songLengthTicks - found.startTick),
        pitchClasses: [...found.pitchClasses],
        name: found.name,
      };
      m.chords = tidyChords(placeChord(m.chords, chord));
      break;
    }
    case "dismissDetectedChord":
      takeDetectedChord(m, c.id);
      break;
  }
}

/** `document::MAX_UNDO`. */
const MAX_UNDO = 200;

/** Insert or replace an embedded sound by id (mirrors `embed_sound`). */
function embedSound(m: ProjectModel, sound: Sound) {
  const sounds = (m.sounds ??= []);
  const i = sounds.findIndex((s) => s.id === sound.id);
  if (i >= 0) sounds[i] = structuredClone(sound);
  else sounds.push(structuredClone(sound));
}

/** Drop embedded sounds no track uses (mirrors `ProjectModel::prune_sounds`). */
function pruneSounds(m: ProjectModel) {
  const used = (s: Sound) => m.tracks.some((t) => (t.isDrum ? t.drumPads.some((p) => p.sound === s.id) : t.voice === s.id));
  const kept = (m.sounds ?? []).filter(used);
  if (kept.length) m.sounds = kept;
  else delete m.sounds;
}

// ─── Sound sanitizing (mirrors `Sound::sanitized` in crates/dissonant-core/src/sound.rs) ────

/** `[path, lo, hi, integer]`; `*` walks every element of a fixed-length array. */
type Range = [path: string, lo: number, hi: number, integer?: true];
const ENVELOPE: Range[] = [["attack", 0, 10], ["decay", 0.001, 10], ["sustain", 0, 1], ["release", 0.001, 10]];
const under = (prefix: string, ranges: Range[]): Range[] => ranges.map(([p, ...r]) => [prefix + "." + p, ...r]);
const SYNTH_RANGES: Range[] = [
  ["osc.*.octave", -3, 3, true],
  ["osc.*.semi", -12, 12, true],
  ["osc.*.fine", -100, 100],
  ["osc.*.level", 0, 1],
  ["osc.*.pulseWidth", 0.05, 0.95],
  ["osc.*.unison", 1, 8, true],
  ["osc.*.detune", 0, 100],
  ["noiseLevel", 0, 1],
  ["noiseColor", 0, 1],
  ["fmAmount", 0, 1],
  ["ringMod", 0, 1],
  ["filter.cutoff", 20, 20000],
  ["filter.resonance", 0, 1],
  ["filter.drive", 0, 1],
  ["filter.keyTrack", 0, 1],
  ["filter.envAmount", -8, 8],
  ["filter.velocity", 0, 1],
  ...under("ampEnv", ENVELOPE),
  ...under("filterEnv", ENVELOPE),
  ["pitchEnvAmount", -48, 48],
  ["pitchEnvDecay", 0.001, 2],
  ["lfo.*.rate", 0.01, 30],
  ["lfo.*.depth", 0, 1],
  ["glide", 0, 2],
  ["velocity", 0, 1],
  ["drive", 0, 1],
  ["crush", 0, 1],
  ["chorus.mix", 0, 1],
  ["chorus.rate", 0.05, 5],
  ["chorus.depth", 0, 1],
  ["delay.mix", 0, 1],
  ["delay.time", 0.01, 1.5],
  ["delay.feedback", 0, 0.95],
  ["level", 0, 1],
];
const DRUM_RANGES: Range[] = [
  ["length", 0.02, 3],
  ["tune", -24, 24],
  ["body.level", 0, 1],
  ["body.startHz", 20, 4000],
  ["body.endHz", 20, 4000],
  ["body.pitchDecay", 0.001, 2],
  ["body.decay", 0.001, 3],
  ["body.overtoneLevel", 0, 1],
  ["body.overtoneRatio", 0.5, 8],
  ["noise.level", 0, 1],
  ["noise.cutoff", 20, 20000],
  ["noise.resonance", 0, 1],
  ["noise.attack", 0, 0.1],
  ["noise.decay", 0.001, 3],
  ["noise.bursts", 1, 8, true],
  ["noise.burstSpacing", 0.002, 0.05],
  ["metal.level", 0, 1],
  ["metal.baseHz", 100, 1000],
  ["metal.decay", 0.001, 3],
  ["metal.cutoff", 200, 20000],
  ["click.level", 0, 1],
  ["click.decay", 0.0005, 0.02],
  ["click.toneHz", 500, 12000],
  ["drive", 0, 1],
  ["crush", 0, 1],
  ["lowCut", 20, 2000],
  ["highCut", 200, 20000],
  ["velocity", 0, 1],
  ["level", 0, 1],
];

function clampPath(node: unknown, keys: string[], lo: number, hi: number, integer: boolean): void {
  const [k, ...rest] = keys;
  if (k === "*") {
    for (const item of node as unknown[]) clampPath(item, rest, lo, hi, integer);
    return;
  }
  const obj = node as Record<string, unknown>;
  if (rest.length) return clampPath(obj[k], rest, lo, hi, integer);
  const v = obj[k];
  // Serde refuses a non-number (or a fraction for an integer field) before the edit runs.
  if (typeof v !== "number" || (integer && !Number.isInteger(v))) fail(INVALID_VALUE);
  obj[k] = Number.isFinite(v) ? clamp(v, lo, hi) : lo;
}

/**
 * What Rust makes of a sound arriving over IPC: missing fields take their defaults (serde
 * `default`), unknown ones are dropped, then `Sound::sanitized` clamps every parameter and
 * names a blank sound "untitled". Always a fresh object.
 */
export function sanitizedSound(sound: Sound): Sound {
  const s = filledSound(structuredClone(sound) as unknown as Parameters<typeof filledSound>[0]);
  const ranges = s.patch.kind === "drum" ? DRUM_RANGES : SYNTH_RANGES;
  for (const [path, lo, hi, integer] of ranges) clampPath(s.patch, path.split("."), lo, hi, integer === true);
  if (!s.name.trim()) s.name = "untitled";
  return s;
}

// ─── Mock sound library ─────────────────────────────────────────────────────────────────────

/** The mock's stand-in for the global sound library (`src-tauri/src/library.rs`). */
const LIBRARY_KEY = "dissonant.mock.soundLibrary";

/** `library::check_id`: the same refusals, word for word. */
function libraryIdError(id: string): string | null {
  if (isFactory(id)) return `"${id}" is a factory sound; save a copy under a new id`;
  if (!id.startsWith("u-")) return `a library sound id must start with "u-" (got "${id}")`;
  return null;
}

/** Tolerant like `library::load`: unreadable storage is an empty library, bad entries drop. */
function loadLibrary(): Sound[] {
  let raw: unknown;
  try {
    raw = JSON.parse(localStorage.getItem(LIBRARY_KEY) ?? "[]");
  } catch {
    return [];
  }
  if (!Array.isArray(raw)) return [];
  const lib: Sound[] = [];
  for (const item of raw) {
    try {
      const s = sanitizedSound(item as Sound);
      if (libraryIdError(s.id) === null && !lib.some((k) => k.id === s.id)) lib.push(s);
    } catch {
      // Not a sound this build can read: dropped, like a bad entry in the file.
    }
  }
  return lib;
}

function storeLibrary(lib: Sound[]) {
  try {
    localStorage.setItem(LIBRARY_KEY, JSON.stringify(lib));
  } catch {
    // Private window / storage blocked: the library just doesn't persist.
  }
}

/** The mock's stand-in for the filesystem: one slot, written by save, read by open. */
const STORAGE_KEY = "dissonant.mock.project";
const TEMPLATES_KEY = "dissonant.mock.templates";

// ─── The mock's plugin world ───────────────────────────────────────────────────────────────

/**
 * Parameters the mock's plugins publish.
 *
 * Two shapes on purpose, because real plugins come in both and they need completely different
 * UI. A small, well-described one that names its modules sits beside a large one that publishes
 * a hundred-odd controls with no modules at all and leans on a naming convention instead —
 * which is exactly what Dexed does with its 156. Designing against only the tidy one is how the
 * parameter list ended up unusable in a side column.
 */
const MOCK_PARAMS_TIDY: Omit<ParamInfo, "value">[] = [
  {
    id: 0,
    name: "cutoff",
    module: "filter",
    min: 20,
    max: 20_000,
    default: 8_000,
    isStepped: false,
  },
  {
    id: 1,
    name: "resonance",
    module: "filter",
    min: 0,
    max: 1,
    default: 0.2,
    isStepped: false,
  },
  {
    id: 2,
    name: "attack",
    module: "envelope",
    min: 0,
    max: 4,
    default: 0.01,
    isStepped: false,
  },
  {
    id: 3,
    name: "release",
    module: "envelope",
    min: 0,
    max: 8,
    default: 0.4,
    isStepped: false,
  },
  {
    id: 4,
    name: "drift",
    module: "voice",
    min: -1,
    max: 1,
    default: 0,
    isStepped: false,
  },
  {
    id: 5,
    name: "voices",
    module: "voice",
    min: 1,
    max: 16,
    default: 8,
    isStepped: true,
  },
];

/** An FM synth's shape: six operators of the same controls, a shared pitch envelope and an LFO,
 *  a handful of globals, and not one module string — copied from what Dexed actually reports. */
const MOCK_PARAMS_BIG: Omit<ParamInfo, "value">[] = (() => {
  const unit = (id: number, name: string, def = 0.5) =>
    ({
      id,
      name,
      module: "",
      min: 0,
      max: 1,
      default: def,
      isStepped: false,
    }) as Omit<ParamInfo, "value">;
  const out = [
    "Cutoff",
    "Resonance",
    "Output",
    "MonoMode",
    "MASTER TUNE ADJ",
    "ALGORITHM",
    "FEEDBACK",
    "OSC KEY SYNC",
    "TRANSPOSE",
    "P MODE SENS.",
  ].map((n, i) => unit(i, n));
  for (const n of [
    "SPEED",
    "DELAY",
    "PM DEPTH",
    "AM DEPTH",
    "KEY SYNC",
    "WAVE",
  ])
    out.push(unit(out.length, `LFO ${n}`));
  for (let i = 1; i <= 4; i++) out.push(unit(out.length, `PITCH EG RATE ${i}`));
  for (let i = 1; i <= 4; i++)
    out.push(unit(out.length, `PITCH EG LEVEL ${i}`));
  for (let op = 1; op <= 6; op++) {
    for (let i = 1; i <= 4; i++)
      out.push(unit(out.length, `OP${op} EG RATE ${i}`));
    for (let i = 1; i <= 4; i++)
      out.push(unit(out.length, `OP${op} EG LEVEL ${i}`));
    for (const n of [
      "OUTPUT LEVEL",
      "MODE",
      "F COARSE",
      "F FINE",
      "OSC DETUNE",
      "BREAK POINT",
      "L SCALE DEPTH",
    ]) {
      out.push(unit(out.length, `OP${op} ${n}`));
    }
  }
  return out;
})();

/** Which set a plugin publishes. Anything FM-shaped in the fixtures gets the big one. */
function mockParamsFor(uid: string): Omit<ParamInfo, "value">[] {
  return /dexed|fm/i.test(uid) ? MOCK_PARAMS_BIG : MOCK_PARAMS_TIDY;
}

/** `HostedPlugin::save_state` stands in for the plugin's opaque blob: here, its parameters. */
function encodeState(values: Map<number, number>): string {
  return btoa(JSON.stringify(Object.fromEntries(values)));
}

function decodeState(uid: string, state: string | null): Map<number, number> {
  const values = new Map(mockParamsFor(uid).map((p) => [p.id, p.default]));
  if (!state) return values;
  try {
    for (const [id, v] of Object.entries(
      JSON.parse(atob(state)) as Record<string, number>,
    )) {
      if (Number.isFinite(v)) values.set(Number(id), v);
    }
  } catch {
    // A blob written by some other plugin: its defaults stand, exactly as a real host would
    // end up when `load_state` is refused.
  }
  return values;
}

/** Plugins the mock pretends were already on this computer before the app was ever run. Only
 *  some of them are in the catalog, so the browser has both "installed" and installable rows. */
const MOCK_INSTALLED: PluginDescriptor[] = [
  {
    uid: "clap:/Library/Audio/Plug-Ins/CLAP/Surge XT.clap:org.surge-synth-team.surge-xt",
    format: "clap",
    path: "/Library/Audio/Plug-Ins/CLAP/Surge XT.clap",
    pluginId: "org.surge-synth-team.surge-xt",
    name: "Surge XT",
    vendor: "Surge Synth Team",
    version: "1.3.1",
    kind: "instrument",
    features: ["instrument", "synthesizer", "pad", "bass"],
  },
  {
    uid: "clap:/Library/Audio/Plug-Ins/CLAP/Tunefish4.clap:de.braincontrol.tunefish4",
    format: "clap",
    path: "/Library/Audio/Plug-Ins/CLAP/Tunefish4.clap",
    pluginId: "de.braincontrol.tunefish4",
    name: "Tunefish 4",
    vendor: "Brain Control",
    version: "4.3.0",
    kind: "instrument",
    features: ["instrument", "synthesizer", "pad", "lead"],
  },
  {
    uid: "clap:/Library/Audio/Plug-Ins/CLAP/Airwindows Consolidated.clap:com.airwindows.consolidated",
    format: "clap",
    path: "/Library/Audio/Plug-Ins/CLAP/Airwindows Consolidated.clap",
    pluginId: "com.airwindows.consolidated",
    name: "Airwindows Consolidated",
    vendor: "Airwindows",
    version: "2.1.0",
    kind: "effect",
    features: ["audio-effect", "saturation", "lo-fi"],
  },
  // A second effect, so a chain in the browser mock can be built out of two distinguishable
  // ones and the order in the card means something.
  {
    uid: "clap:/Library/Audio/Plug-Ins/CLAP/CHOWTapeModel.clap:com.chowdsp.CHOWTapeModel",
    format: "clap",
    path: "/Library/Audio/Plug-Ins/CLAP/CHOWTapeModel.clap",
    pluginId: "com.chowdsp.CHOWTapeModel",
    name: "CHOW Tape Model",
    vendor: "chowdsp",
    version: "2.11.0",
    kind: "effect",
    features: ["audio-effect", "distortion", "tape"],
  },
];

/**
 * The mock's curated list. The real one lives in dissonant-catalog; these entries carry real
 * names and homepages so the browser can be judged on real text, with stand-in download urls
 * and hashes because the mock never fetches anything.
 */
const MOCK_CATALOG: CatalogEntry[] = [
  {
    id: "surge-xt",
    name: "Surge XT",
    vendor: "Surge Synth Team",
    summary:
      "A big synth that covers almost any sound, with hundreds of ready-made presets.",
    tags: ["pad", "bass", "lead", "presets"],
    kind: "instrument",
    license: "GPL-3.0",
    homepage: "https://surge-synthesizer.github.io",
    assets: [
      {
        platform: "macArm",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/surge-xt-mac.zip",
        sha256: "a1".repeat(32),
        bytes: 128_000_000,
      },
      {
        platform: "macIntel",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/surge-xt-mac.zip",
        sha256: "a1".repeat(32),
        bytes: 128_000_000,
      },
      {
        platform: "windows",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/surge-xt-win.zip",
        sha256: "a2".repeat(32),
        bytes: 120_000_000,
      },
      {
        platform: "linux",
        format: "clap",
        kind: "tarGz",
        url: "https://example.invalid/surge-xt-linux.tar.gz",
        sha256: "a3".repeat(32),
        bytes: 110_000_000,
      },
    ],
  },
  {
    id: "dexed",
    name: "Dexed",
    vendor: "Digital Suburban",
    summary:
      "Eighties FM sounds: glassy bells, electric pianos and brassy stabs.",
    tags: ["bell", "electric piano", "retro"],
    kind: "instrument",
    license: "GPL-3.0",
    homepage: "https://asb2m10.github.io/dexed",
    assets: [
      {
        platform: "macArm",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/dexed-mac.zip",
        sha256: "b1".repeat(32),
        bytes: 18_000_000,
      },
      {
        platform: "macIntel",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/dexed-mac.zip",
        sha256: "b1".repeat(32),
        bytes: 18_000_000,
      },
      {
        platform: "windows",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/dexed-win.zip",
        sha256: "b2".repeat(32),
        bytes: 17_000_000,
      },
      {
        platform: "linux",
        format: "clap",
        kind: "tarGz",
        url: "https://example.invalid/dexed-linux.tar.gz",
        sha256: "b3".repeat(32),
        bytes: 16_000_000,
      },
    ],
  },
  {
    id: "vital",
    name: "Vital",
    vendor: "Matt Tytel",
    summary:
      "Moving, modern sounds — good for wobbling basses and slow evolving pads.",
    tags: ["pad", "bass", "modern", "presets"],
    kind: "instrument",
    license: "freeware",
    homepage: "https://vital.audio",
    assets: [
      {
        platform: "macArm",
        format: "clap",
        kind: "installer",
        url: "https://vital.audio/#download",
        sha256: "c1".repeat(32),
        bytes: 1_100_000_000,
      },
      {
        platform: "macIntel",
        format: "clap",
        kind: "installer",
        url: "https://vital.audio/#download",
        sha256: "c1".repeat(32),
        bytes: 1_100_000_000,
      },
      {
        platform: "windows",
        format: "clap",
        kind: "installer",
        url: "https://vital.audio/#download",
        sha256: "c2".repeat(32),
        bytes: 1_000_000_000,
      },
      {
        platform: "linux",
        format: "clap",
        kind: "installer",
        url: "https://vital.audio/#download",
        sha256: "c3".repeat(32),
        bytes: 900_000_000,
      },
    ],
  },
  {
    id: "airwindows-consolidated",
    name: "Airwindows Consolidated",
    vendor: "Airwindows",
    summary:
      "Hundreds of small effects in one: warmth, grit, tape wobble and tape hiss.",
    tags: ["fx", "lo-fi", "saturation"],
    kind: "effect",
    license: "MIT",
    homepage: "https://www.airwindows.com",
    assets: [
      {
        platform: "macArm",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/airwindows-mac.zip",
        sha256: "d1".repeat(32),
        bytes: 24_000_000,
      },
      {
        platform: "macIntel",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/airwindows-mac.zip",
        sha256: "d1".repeat(32),
        bytes: 24_000_000,
      },
      {
        platform: "windows",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/airwindows-win.zip",
        sha256: "d2".repeat(32),
        bytes: 23_000_000,
      },
      {
        platform: "linux",
        format: "clap",
        kind: "tarGz",
        url: "https://example.invalid/airwindows-linux.tar.gz",
        sha256: "d3".repeat(32),
        bytes: 22_000_000,
      },
    ],
  },
  {
    id: "odin-2",
    name: "Odin 2",
    vendor: "TheWaveWarden",
    summary:
      "Three oscillators and a lot of knobs — raw analogue-style bass and leads.",
    tags: ["bass", "lead", "analogue"],
    kind: "instrument",
    license: "GPL-3.0",
    homepage: "https://www.thewavewarden.com/odin2",
    assets: [
      {
        platform: "windows",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/odin2-win.zip",
        sha256: "e1".repeat(32),
        bytes: 60_000_000,
      },
    ],
  },
  {
    id: "bespoke-drums",
    name: "Bespoke Drums",
    vendor: "Ryan Challinor",
    summary: "A kit of sampled drums you can swap one pad at a time.",
    tags: ["drums", "kit", "samples"],
    kind: "instrument",
    license: "GPL-3.0",
    homepage: "https://www.bespokesynth.com",
    assets: [
      {
        platform: "macArm",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/bespoke-mac.zip",
        sha256: "f1".repeat(32),
        bytes: 40_000_000,
      },
      {
        platform: "macIntel",
        format: "clap",
        kind: "zip",
        url: "https://example.invalid/bespoke-mac.zip",
        sha256: "f1".repeat(32),
        bytes: 40_000_000,
      },
      {
        platform: "linux",
        format: "clap",
        kind: "tarGz",
        url: "https://example.invalid/bespoke-linux.tar.gz",
        sha256: "f2".repeat(32),
        bytes: 38_000_000,
      },
    ],
  },
];

/** What `scanPlugins` reports once an entry has been unpacked into the plugin folder. */
function installedDescriptor(entry: CatalogEntry): PluginDescriptor {
  const path = `/Library/Audio/Plug-Ins/CLAP/${entry.name}.clap`;
  return {
    uid: `clap:${path}:${entry.id}`,
    format: "clap",
    path,
    pluginId: entry.id,
    name: entry.name,
    vendor: entry.vendor,
    version: "1.0.0",
    kind: entry.kind,
    features: entry.tags,
  };
}

/**
 * The one document every window on this origin shares.
 *
 * In the desktop app both the main window and a plugin editor talk to the same Rust state, so a
 * mock that kept its model per window would diverge from the thing it stands in for — the editor
 * popup would open onto a different project and find no plugin at all. Keeping the model in
 * `localStorage` reproduces the shared backend, and the `storage` event carries another window's
 * edits (including undo) back here, which is what the real `document` event does.
 */

/**
 * A deterministic waveform envelope seeded from a source id, so the rec page can be
 * designed under `pnpm --dir ui dev` without any audio at all. Shaped like a strummed take:
 * a pick attack decaying into a body, repeated.
 */
function syntheticPeaks(sourceId: string, buckets: number): [number, number][] {
  let seed = 0;
  for (let i = 0; i < sourceId.length; i++)
    seed = (seed * 31 + sourceId.charCodeAt(i)) >>> 0;
  const rand = () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 0xffffffff;
  };
  const n = Math.max(1, Math.min(8192, Math.floor(buckets)));
  const strums = 8;
  return Array.from({ length: n }, (_, i) => {
    const phase = ((i / n) * strums) % 1;
    const envelope = Math.exp(-phase * 3.5) * (0.35 + rand() * 0.5);
    const skew = 0.85 + rand() * 0.3;
    return [-envelope * skew, envelope] as [number, number];
  });
}
const LIVE_MODEL_KEY = "dissonant.mock.live";

/**
 * `place_recorded_note` in src-tauri/src/lib.rs: where a note held from `start` to `end` lands
 * in the pattern being recorded into. In song mode that is resolved through the clip actually
 * being heard; played where the pattern is not placed, the note has nowhere to go and `null`
 * says so rather than folding it in with a modulo. `quantize` is the grid, `null` as played.
 */
function placeRecordedNote(
  model: ProjectModel,
  mode: PlayMode,
  pattern: SongPattern,
  start: number,
  end: number,
  quantize: number | null,
): { startTick: number; lengthTicks: number } | null {
  const loop = Math.max(1, pattern.lengthTicks);
  let len = end - start;
  if (len < 0) len += loop; // the loop wrapped while the key was held
  let startIn: number;
  if (mode === "pattern") {
    startIn = ((start % loop) + loop) % loop;
  } else {
    const clips = model.clips.filter((c) => c.patternId === pattern.id && start >= c.startTick && start < c.startTick + c.lengthTicks);
    if (!clips.length) return null;
    const clip = clips.reduce((a, b) => (b.startTick >= a.startTick ? b : a));
    const into = start - clip.startTick + clip.offsetTicks;
    startIn = ((into % loop) + loop) % loop;
  }
  const grid = quantize && quantize > 0 ? quantize : null;
  const startTick = grid ? Math.round(startIn / grid) * grid : Math.round(startIn);
  const lengthTicks = grid ? Math.max(grid, Math.round(len / grid) * grid) : Math.max(1, Math.round(len));
  const last = Math.max(0, loop - (grid ?? 1));
  return { startTick: Math.min(last, Math.max(0, startTick)), lengthTicks };
}

function mockBridge(): Bridge {
  // Adopt the shared document only in a browser, where a second window can exist. The test
  // runner drives the mock directly and builds a fresh one per case; restoring there would leak
  // one test's project into the next.
  const restored = (() => {
    if (typeof window === "undefined") return null;
    try {
      const raw = localStorage.getItem(LIVE_MODEL_KEY);
      return raw ? normalizedModel(JSON.parse(raw) as ProjectModel) : null;
    } catch {
      return null;
    }
  })();
  let model = restored ?? starterModel();
  const undo: ProjectModel[] = [];
  const redo: ProjectModel[] = [];
  let pending: ProjectModel | null = null;
  // Mirrors Document's save point: dirty is "differs from what was saved", not a sticky flag,
  // so undoing back to the saved state (or a round-trip slider drag) leaves the project clean.
  // `Document::new` takes a save point immediately, so a fresh project is clean until edited.
  let saved: string | null = JSON.stringify(model);
  let path: string | null = null;
  let mode: PlayMode = "pattern";
  let patternId: string | null = null;
  let playing = false;
  let playTick = 0;
  let last = performance.now();
  let loop: [number, number] | null = null;
  let looping = true;
  /** Armed punch range in ticks; blocks looping, exactly as the engine does. */
  let punch: [number, number] | null = null;
  let armed = false;
  let quantize: number | null = TICKS_PER_BEAT / 4;
  let liveTrack: string | null = null;
  const held = new Map<number, { start: number; velocity: number }>();
  /** Ids the current take wrote, so a second pass over a loop layers instead of erasing. */
  const take = new Set<string>();
  const listeners = new Set<(e: PlayheadEvent) => void>();
  const docListeners = new Set<(s: Snapshot) => void>();
  const midiListeners = new Set<(p: number) => void>();
  const noticeListeners = new Set<(m: string) => void>();

  const undoLabels: string[] = [];
  const redoLabels: string[] = [];
  let pendingLabel = "edit";
  const closeListeners = new Set<() => void>();
  const isDirty = () => saved !== JSON.stringify(model);
  const snapshot = (): Snapshot => ({
    model: clone(model),
    // An open gesture sitting on its starting value is not undoable, matching `Document::can_undo`.
    canUndo:
      undo.length > 0 ||
      (pending !== null && JSON.stringify(pending) !== JSON.stringify(model)),
    canRedo: redo.length > 0,
    dirty: isDirty(),
    path,
    undoLabel: pending
      ? pendingLabel
      : (undoLabels[undoLabels.length - 1] ?? null),
    redoLabel: redoLabels[redoLabels.length - 1] ?? null,
  });

  /** Publish the document: to this window's listeners, and to the other windows through storage. */
  const notifyDoc = () => {
    try {
      localStorage.setItem(LIVE_MODEL_KEY, JSON.stringify(model));
    } catch {
      // Private mode, or the quota is full. The mock still works; it just stops being shared.
    }
    for (const cb of docListeners) cb(snapshot());
  };
  // Another window changed the shared document — a plugin editor moving a parameter, or an undo
  // in the main window. Adopt it and tell our own listeners, which is what the real `document`
  // event does for every webview. There is no `window` under the test runner, where the mock is
  // driven directly and a second window cannot exist.
  //
  // Only the model travels. The desktop app has one `Document` behind both windows, so its undo
  // history is genuinely shared; here each window keeps its own stack, and an edit made in the
  // editor is undoable from the editor rather than from the main window.
  if (typeof window !== "undefined") {
    window.addEventListener("storage", (e) => {
      if (e.key !== LIVE_MODEL_KEY || !e.newValue) return;
      try {
        model = JSON.parse(e.newValue) as ProjectModel;
      } catch {
        return;
      }
      for (const cb of docListeners) cb(snapshot());
    });
  }
  const defaultLabel = (c: Command) => {
    // `document.rs::label_for` names the insert edits by hand; every other command's label
    // falls out of its own name.
    switch (c.type) {
      case "addInsert":
        return c.name.trim() ? `add ${c.name.trim()}` : "add insert";
      case "moveInsert":
        return "reorder insert";
      case "setInsertEnabled":
        return c.on ? "enable insert" : "bypass insert";
      case "setInsertState":
      case "setInsertParam":
        return "insert settings";
      case "updateAutomationPoint":
        return "move automation point";
      case "setRope":
        return "add rope";
      case "clearRope":
        return "remove rope";
      case "setSlideTime":
        return "change slide time";
      default:
        return c.type
          .replace(/([A-Z])/g, " $1")
          .toLowerCase()
          .trim();
    }
  };
  const recentKey = "dissonant.mock.recent";
  const loadRecent = (): string[] => {
    try {
      return JSON.parse(localStorage.getItem(recentKey) ?? "[]") as string[];
    } catch {
      return [];
    }
  };
  const remember = (p: string) => {
    const list = [p, ...loadRecent().filter((x) => x !== p)].slice(0, 12);
    try {
      localStorage.setItem(recentKey, JSON.stringify(list));
    } catch {
      /* storage unavailable */
    }
  };
  // The browser stands in for the OS close button. Guarded: the mock is also constructed under
  // the test runner, where there is no DOM.
  if (typeof window !== "undefined") {
    window.addEventListener("beforeunload", (e) => {
      if (isDirty()) {
        e.preventDefault();
        for (const cb of closeListeners) cb();
      }
    });
  }

  /** Song mode ends at the song's hard end; pattern mode loops the context pattern. */
  const loopLength = () => {
    if (mode === "song") return model.songLengthTicks;
    return model.patterns.find((x) => x.id === patternId)?.lengthTicks ?? 16 * TICKS_PER_BEAT;
  };

  const frame = () => {
    const now = performance.now();
    if (playing) {
      playTick += ((now - last) / 1000) * (model.tempo / 60) * TICKS_PER_BEAT;
      const len = loopLength();
      const [ls, le] =
        loop && loop[1] > loop[0]
          ? [Math.min(loop[0], len), Math.min(loop[1], len)]
          : [0, len];
      if (looping && !punch && playTick >= le && le > ls)
        playTick = ls + ((playTick - le) % (le - ls));
    }
    last = now;
    const e: PlayheadEvent = {
      tick: playTick,
      playing,
      masterPeak: [0, 0],
      trackPeaks: [],
    };
    for (const cb of listeners) cb(e);
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  // A tiny WebAudio blip so placing notes gives feedback in the browser mock.
  let ctx: AudioContext | null = null;
  const blip = (pitch: number, velocity: number, seconds = 0.25) => {
    try {
      ctx ??= new AudioContext();
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      const hold = Math.min(4, Math.max(0.12, seconds));
      osc.type = "sawtooth";
      osc.frequency.value = 440 * Math.pow(2, (pitch - 69) / 12);
      gain.gain.setValueAtTime(0.0001, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(
        0.15 * (velocity / 127),
        ctx.currentTime + 0.01,
      );
      gain.gain.setValueAtTime(0.15 * (velocity / 127), ctx.currentTime + hold);
      gain.gain.exponentialRampToValueAtTime(
        0.0001,
        ctx.currentTime + hold + 0.08,
      );
      osc.connect(gain).connect(ctx.destination);
      osc.start();
      osc.stop(ctx.currentTime + hold + 0.1);
    } catch {
      /* no audio in this browser */
    }
  };

  /** `Document::push_undo` — the Rust history is capped at `MAX_UNDO` labelled snapshots. */
  const pushUndo = (snap: ProjectModel, label: string) => {
    undo.push(snap);
    undoLabels.push(label);
    if (undo.length > MAX_UNDO) {
      undo.shift();
      undoLabels.shift();
    }
  };

  /** `Document::replace` — new/open install a model and drop the whole history, pending included. */
  const replace = (next: ProjectModel, to: string | null) => {
    model = next;
    undo.length = 0;
    redo.length = 0;
    undoLabels.length = 0;
    redoLabels.length = 0;
    pending = null;
    path = to;
    saved = JSON.stringify(model);
    playTick = 0;
    notifyDoc();
  };

  const commitGesture = () => {
    if (pending && JSON.stringify(pending) !== JSON.stringify(model))
      pushUndo(pending, pendingLabel);
    pending = null;
  };

  /** End the recorded pass: what it wrote becomes one undo step and the next pass starts clean. */
  const endTake = () => {
    take.clear();
    commitGesture();
  };

  /** Named so the plugin commands can reach it too: a committed param edit is an ordinary
   *  undoable document edit, it just does not originate from a UI dispatch. */
  const applyCommand = async (
    command: Command,
    transient = false,
    label?: string,
  ): Promise<Snapshot> => {
    const before = clone(model);
    const next = clone(model);
    reduce(next, command);
    if (JSON.stringify(next) === JSON.stringify(before)) return snapshot();
    const name = label?.trim() || defaultLabel(command);
    if (transient) {
      if (!pending) {
        pending = before;
        pendingLabel = name;
      }
    } else {
      commitGesture();
      pushUndo(before, name);
    }
    redo.length = 0;
    redoLabels.length = 0;
    model = next;
    notifyDoc();
    return snapshot();
  };

  // The mock's plugin folder: what shipped with the fake system plus whatever `installPlugin`
  // unpacked this session. Scanning is therefore always instant and `rescan` has nothing to do.
  const installed: PluginDescriptor[] = MOCK_INSTALLED.map((d) => ({ ...d }));
  const installListeners = new Set<(p: InstallProgress) => void>();
  /**
   * Live parameter values per track, the mock's stand-in for the values a `HostedPlugin` holds
   * on the app thread. Keyed by uid and by the blob they were last reconciled with, so swapping
   * the plugin or undoing a committed edit reseeds them instead of showing stale knobs.
   */
  const paramValues = new Map<
    string,
    { uid: string; state: string | null; values: Map<number, number> }
  >();

  const pluginFor = (uid: string): TrackPlugin => {
    const d = installed.find((x) => x.uid === uid) ?? fail(NO_SUCH_PLUGIN);
    return { uid: d.uid, name: d.name, state: null };
  };

  const liveParams = (trackId: string) => {
    const t = model.tracks.find((x) => x.id === trackId) ?? fail(NO_SUCH_TRACK);
    if (!t.plugin) return null;
    const held = paramValues.get(trackId);
    if (held && held.uid === t.plugin.uid && held.state === t.plugin.state)
      return held;
    const fresh = {
      uid: t.plugin.uid,
      state: t.plugin.state,
      values: decodeState(t.plugin.uid, t.plugin.state),
    };
    paramValues.set(trackId, fresh);
    return fresh;
  };

  /** The insert with this id and the chain it sits in, or null once it has been removed. */
  const findInsert = (
    insertId: string,
  ): { target: InsertTarget; insert: Insert } | null => {
    for (const t of model.tracks) {
      const i = (t.inserts ?? []).find((x) => x.id === insertId);
      if (i) return { target: { track: t.id }, insert: i };
    }
    const i = (model.masterInserts ?? []).find((x) => x.id === insertId);
    return i ? { target: "master", insert: i } : null;
  };

  /** `liveParams` for an insert: the same live values a hosted effect would hold, reseeded
   *  whenever the document's blob moves out from under them (a swap, an undo, a reopen). */
  const liveInsert = (insertId: string) => {
    const found = findInsert(insertId);
    if (!found) return null;
    const state = found.insert.state ?? null;
    const held = paramValues.get(insertId);
    if (held && held.uid === found.insert.uid && held.state === state)
      return held;
    const fresh = {
      uid: found.insert.uid,
      state,
      values: decodeState(found.insert.uid, state),
    };
    paramValues.set(insertId, fresh);
    return fresh;
  };

  // A pretend session for working on the UI in a browser: one person who wanders around whatever
  // you are looking at and now and then writes a note. Nothing like it runs in the real app.
  const sessionListeners = new Set<(s: SessionState) => void>();
  const presenceListeners = new Set<(p: PeerPresence) => void>();
  const endedListeners = new Set<(reason: string) => void>();
  let session: SessionState = SOLO;
  let fakePeer: ReturnType<typeof setInterval> | null = null;
  let youAre: PeerLocation = { screen: "setup" };
  const setSession = (next: SessionState) => {
    session = next;
    for (const cb of sessionListeners) cb(next);
    return next;
  };
  const startFakePeer = (host: boolean) => {
    const id = "fake-peer";
    let t = 0;
    fakePeer = setInterval(() => {
      t += 1;
      const wave = (period: number) => (Math.sin(t / period) + 1) / 2;
      let location: PeerLocation = youAre;
      if (youAre.screen === "pattern") {
        location = { screen: "pattern", patternId: youAre.patternId, cursor: { tick: wave(40) * 3840, pitch: 60 + Math.round(wave(23) * 12), frac: 0.5 } };
      } else if (youAre.screen === "song") {
        location = { screen: "song", cursor: { tick: wave(40) * 30000, trackId: model.tracks[0].id, lane: wave(23) } };
      }
      for (const cb of presenceListeners) cb({ peer: id, location });
      if (youAre.screen === "pattern" && t % 150 === 0) {
        const p = model.patterns.find((x) => youAre.screen === "pattern" && x.id === youAre.patternId);
        if (p) {
          const notes = [...p.notes, { id: uuid(), startTick: Math.round(wave(40) * 3840), lengthTicks: 240, pitch: 60 + Math.round(wave(23) * 12), velocity: 100 }];
          void applyCommand({ type: "setNotes", patternId: p.id, notes }).then((s) => {
            for (const cb of docListeners) cb(s);
          });
        }
      }
    }, 33);
    const peer = { id, name: "robin", color: "#f0a6ca", isHost: !host };
    return peer;
  };
  const endFakePeer = () => {
    if (fakePeer !== null) clearInterval(fakePeer);
    fakePeer = null;
  };

  return {
    isTauri: false,
    getState: async () => snapshot(),
    apply: applyCommand,
    shareStart: async (name, color) => {
      const you = { id: "you", name, color, isHost: true };
      setSession({ ...SOLO, role: "host", live: true, you: you.id, code: "dissonant-pretendinvitecode", peers: [you] });
      setTimeout(() => setSession({ ...session, peers: [you, startFakePeer(true)] }), 600);
      return session;
    },
    shareStop: async () => {
      endFakePeer();
      return setSession(SOLO);
    },
    shareResetLink: async () => setSession({ ...session, code: `dissonant-pretendinvitecode${Date.now()}` }),
    removePeer: async () => {
      endFakePeer();
      setSession({ ...session, peers: session.peers.filter((p) => p.id !== "fake-peer") });
    },
    joinSession: async (code, name, color) => {
      if (!code.startsWith("dissonant-")) throw new Error("that doesn't look like a dissonant invite.");
      const you = { id: "you", name, color, isHost: false };
      setSession({ ...SOLO, role: "guest", you: you.id, peers: [] });
      setTimeout(() => setSession({ ...session, live: true, path: "direct", peers: [startFakePeer(false), you] }), 600);
      return session;
    },
    leaveSession: async () => {
      endFakePeer();
      setSession(SOLO);
      return snapshot();
    },
    getSession: async () => session,
    sendPresence: (location) => {
      youAre = location;
    },
    onSession: (cb) => {
      sessionListeners.add(cb);
      return () => sessionListeners.delete(cb);
    },
    onPeerPresence: (cb) => {
      presenceListeners.add(cb);
      return () => presenceListeners.delete(cb);
    },
    onSessionEnded: (cb) => {
      endedListeners.add(cb);
      return () => endedListeners.delete(cb);
    },
    commitGesture: async () => {
      commitGesture();
      return snapshot();
    },
    undo: async () => {
      commitGesture();
      const prev = undo.pop();
      if (prev) {
        redo.push(model);
        redoLabels.push(undoLabels.pop() ?? "edit");
        model = prev;
      }
      return snapshot();
    },
    redo: async () => {
      commitGesture();
      const next = redo.pop();
      if (next) {
        pushUndo(model, redoLabels.pop() ?? "edit");
        model = next;
      }
      return snapshot();
    },
    newProject: async (starter) => {
      // Like `new_project`: no explicit choice means the "new project opens" setting.
      let useStarter = starter;
      if (useStarter === undefined) {
        try {
          useStarter = mockSettings().editing.newProject === "starter";
        } catch {
          useStarter = true;
        }
      }
      replace(useStarter ? starterModel() : emptyModel(), null);
      return snapshot();
    },
    openProject: async (p) => {
      const stored = localStorage.getItem(STORAGE_KEY);
      if (!stored) fail(`could not read ${p}`);
      replace(normalizedModel(JSON.parse(stored) as ProjectModel), p);
      remember(p);
      return snapshot();
    },
    saveProject: async (p) => {
      const target = p ?? path ?? fail("no file path yet — use Save As");
      path = target;
      remember(target);
      saved = JSON.stringify(model);
      try {
        localStorage.setItem(STORAGE_KEY, saved);
      } catch {
        /* storage unavailable */
      }
      return snapshot();
    },
    // The mock plays no sound, so the chord offset and the ghost only matter to the engine.
    setPlaybackContext: async (m, pid) => {
      if (m !== mode || pid !== patternId) {
        playTick = 0;
        // A take belongs to the pattern it was played into: switching ends it.
        endTake();
      }
      mode = m;
      patternId = pid;
    },
    play: async () => {
      playing = true;
      last = performance.now();
    },
    setMetronome: async () => {},
    stop: async () => {
      playing = false;
      endTake();
    },
    seek: async (t) => {
      playTick = Math.max(0, t);
    },
    setHearChords: async () => {},
    audition: async (_t, pitch, velocity = 100, seconds) =>
      blip(pitch, velocity, seconds),
    auditionOff: async () => {},
    audioStatus: async () => ({
      running: false,
      sampleRate: null,
      error: "browser mock: no engine",
    }),
    restartAudio: async () => ({
      running: false,
      sampleRate: null,
      error: "browser mock: no engine",
    }),
    exportWav: async () => {
      throw new Error(
        "export needs the desktop app — the browser mock renders nothing",
      );
    },
    onExportProgress: () => () => {},
    exportMidi: async () => {
      throw new Error("MIDI export needs the desktop app");
    },
    importMidi: async () => {
      throw new Error("MIDI import needs the desktop app");
    },
    listTemplates: async () => {
      try {
        return Object.keys(JSON.parse(localStorage.getItem(TEMPLATES_KEY) ?? "{}")).map((name) => ({ name, path: name }));
      } catch {
        return [];
      }
    },
    saveTemplate: async (name) => {
      const all = JSON.parse(localStorage.getItem(TEMPLATES_KEY) ?? "{}") as Record<string, ProjectModel>;
      all[name] = clone(model);
      localStorage.setItem(TEMPLATES_KEY, JSON.stringify(all));
      return { name, path: name };
    },
    deleteTemplate: async (name) => {
      const all = JSON.parse(localStorage.getItem(TEMPLATES_KEY) ?? "{}") as Record<string, ProjectModel>;
      delete all[name];
      localStorage.setItem(TEMPLATES_KEY, JSON.stringify(all));
    },
    listVibes: async () => VIBE_SPECS.map(({ id, name, blurb, tempo, root, scale }) => ({ id, name, blurb, tempo, key: lockedKey(root, scale) })),
    newFromVibe: async (id) => {
      replace(vibeModel(id) ?? fail(`no vibe called ${id}`), null);
      return snapshot();
    },
    newFromTemplate: async (name) => {
      const all = JSON.parse(localStorage.getItem(TEMPLATES_KEY) ?? "{}") as Record<string, ProjectModel>;
      // A template is a project file: an old one is refused like any other.
      if (all[name]) replace(normalizedModel(clone(all[name])), null);
      return snapshot();
    },
    onPlayhead: (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    onDocument: (cb) => {
      docListeners.add(cb);
      return () => docListeners.delete(cb);
    },
    onMidiActivity: (cb) => {
      midiListeners.add(cb);
      return () => midiListeners.delete(cb);
    },
    onNotice: (cb) => {
      noticeListeners.add(cb);
      return () => noticeListeners.delete(cb);
    },
    midiStatus: async () => ({ inputs: [], open: null, armed }),
    openMidiInput: async () => ({ inputs: [], open: null, armed }),
    closeMidiInput: async () => ({ inputs: [], open: null, armed }),
    setLiveTrack: async (id) => {
      liveTrack = id;
    },
    setRecord: async (a, q) => {
      armed = a;
      quantize = q && q > 0 ? q : null;
      if (!a) {
        held.clear();
        endTake();
      }
      return { inputs: [], open: null, armed };
    },
    noteOn: async (pitch, velocity = 100) => {
      blip(pitch, velocity);
      for (const cb of midiListeners) cb(pitch);
      if (armed && playing) held.set(pitch, { start: playTick, velocity });
    },
    noteOff: async (pitch) => {
      const h = held.get(pitch);
      held.delete(pitch);
      if (!h) return;
      // Recording lands in the context pattern, and only while the live track is its track.
      const p = model.patterns.find((x) => x.id === patternId);
      if (!p || (liveTrack ?? model.tracks[0]?.id) !== p.trackId) return;
      const placed = placeRecordedNote(model, mode, p, h.start, playTick, quantize);
      if (!placed) {
        for (const cb of noticeListeners) cb(`nothing of ${p.name} is playing here — note not recorded`);
        return;
      }
      const slot = Math.max(1, Math.round((quantize ?? TICKS_PER_BEAT / 32) / 2));
      const note = { id: uuid(), startTick: placed.startTick, lengthTicks: placed.lengthTicks, pitch, velocity: h.velocity };
      // Only this take's own notes are swallowed; an earlier pass is another overdub layer.
      const notes = p.notes.filter((n) => !(take.has(n.id) && n.pitch === pitch && Math.abs(n.startTick - note.startTick) < slot));
      take.add(note.id);
      notes.push(note);
      // Transient, so the whole take is one undo step; `endTake` closes it.
      await applyCommand({ type: "setNotes", patternId: p.id, notes }, true, "record");
    },
    setLoop: async (s, e) => {
      loop = [s, e];
    },
    clearLoop: async () => {
      loop = null;
    },
    setLooping: async (on) => {
      looping = on;
    },
    setPunch: async (s, e) => {
      punch = [s, e];
    },
    clearPunch: async () => {
      punch = null;
    },
    pickOpenPath: async () => null,
    pickSavePath: async (name, ext) =>
      name.endsWith(`.${ext}`) ? name : `${name}.${ext}`,
    setTitle: async (title) => {
      document.title = title;
    },
    quit: async () => {
      // Quitting discards unsaved work, so the save point is whatever is in memory.
      saved = JSON.stringify(model);
      window.close();
    },
    onCloseRequested: (cb) => {
      closeListeners.add(cb);
      return () => closeListeners.delete(cb);
    },
    onFileDropped: () => () => {},
    onAudioStatus: () => () => {},
    recentFiles: async () => loadRecent(),
    clearRecent: async () => {
      try {
        localStorage.removeItem(recentKey);
      } catch {
        /* storage unavailable */
      }
    },
    recoveryCandidates: async () => [],
    restoreAutosave: async () => snapshot(),
    discardAutosave: async () => {},
    autosaveNow: async () => null,
    getSettings: async () => mockSettings(),
    setSettings: async (settings) => {
      try {
        localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
      } catch {
        /* storage unavailable */
      }
      return {
        running: false,
        sampleRate: null,
        error: "browser mock: no engine",
      };
    },
    outputDevices: async () => [
      {
        name: "System default",
        isDefault: true,
        sampleRates: [44100, 48000, 96000],
        defaultSampleRate: 48000,
      },
      {
        name: "Mock interface",
        isDefault: false,
        sampleRates: [44100, 48000, 88200, 96000, 192000],
        defaultSampleRate: 44100,
      },
    ],
    testTone: async () => blip(69, 100),
    // No engine in the browser, so nothing ever latches and there is nothing to clear.
    clearClip: async () => {},
    librarySounds: async () => loadLibrary(),
    // `library::save` / `library::delete`: factory and non-`u-` ids are refused, the sound is
    // sanitized and replaces its id in place; deleting an unknown id changes nothing.
    saveLibrarySound: async (sound) => {
      const refusal = libraryIdError(sound.id);
      if (refusal) fail(refusal);
      const clean = sanitizedSound(sound);
      const lib = loadLibrary();
      const i = lib.findIndex((s) => s.id === clean.id);
      if (i >= 0) lib[i] = clean;
      else lib.push(clean);
      storeLibrary(lib);
      return clone(lib);
    },
    deleteLibrarySound: async (id) => {
      const lib = loadLibrary().filter((s) => s.id !== id);
      storeLibrary(lib);
      return clone(lib);
    },
    // The mock has no synth engine: previews are silent and the scope is flat.
    previewSound: async () => {},
    previewNote: async () => {},
    previewScope: async () => new Array(512).fill(0),
    onMidiStatus: () => () => {},
    scanPlugins: async () => installed.map((d) => ({ ...d })),
    setTrackPlugin: async (trackId, uid) => {
      // The track is checked first, like the Rust command: an unknown track is the user's
      // mistake, an unknown uid is a stale list.
      model.tracks.find((t) => t.id === trackId) ?? fail(NO_SUCH_TRACK);
      const plugin = uid === null ? null : pluginFor(uid);
      await applyCommand(
        { type: "setTrackPlugin", id: trackId, plugin },
        false,
        "change plugin",
      );
      paramValues.delete(trackId);
      notifyDoc();
    },
    pluginParams: async (trackId) => {
      const held = liveParams(trackId);
      return held
        ? mockParamsFor(held.uid).map((p) => ({
            ...p,
            value: held.values.get(p.id) ?? p.default,
          }))
        : [];
    },
    setPluginParam: async (trackId, paramId, value, commit = false) => {
      const held = liveParams(trackId) ?? fail(NO_PLUGIN);
      const info =
        mockParamsFor(held.uid).find((p) => p.id === paramId) ??
        fail(NO_SUCH_PARAM);
      if (!allFinite(value)) fail(INVALID_VALUE);
      const v = clamp(value, info.min, info.max);
      held.values.set(paramId, info.isStepped ? Math.round(v) : v);
      // Only the end of the gesture touches the document: a dragging value goes straight to
      // the plugin, which is what `commit: false` means on the Rust side.
      if (commit) {
        const state = encodeState(held.values);
        await applyCommand(
          { type: "setPluginState", id: trackId, state },
          false,
          "edit plugin",
        );
        held.state = state;
        notifyDoc();
      }
    },
    pluginCatalog: async () => MOCK_CATALOG.map(clone),
    installPlugin: async (entryId) => {
      const entry =
        MOCK_CATALOG.find((e) => e.id === entryId) ?? fail(NO_SUCH_ENTRY);
      const emit = (
        phase: InstallProgress["phase"],
        done: number,
        total: number,
        message: string,
      ) => {
        for (const cb of installListeners)
          cb({ entryId, phase, done, total, message });
      };
      const platforms = hostPlatforms();
      const asset = entry.assets.find(
        (a) => platforms.includes(a.platform) && a.kind !== "installer",
      );
      if (!asset) {
        emit("failed", 0, 1, NO_ASSET);
        fail(NO_ASSET);
      }
      // A real download is slow enough that the UI has to show progress; the mock keeps the
      // shape and compresses the time, so a bar that only renders on the last event still
      // looks wrong here. (`Promise.withResolvers` needs a newer lib than this project targets.)
      const pause = () => new Promise((r) => setTimeout(r, 12));
      for (let i = 1; i <= 5; i++) {
        await pause();
        emit(
          "downloading",
          Math.round((asset.bytes * i) / 5),
          asset.bytes,
          `downloading ${entry.name}`,
        );
      }
      await pause();
      emit("verifying", 1, 2, "checking the download");
      await pause();
      emit("unpacking", 2, 2, `unpacking ${entry.name}`);
      await pause();
      const d = installedDescriptor(entry);
      if (!installed.some((x) => x.uid === d.uid)) installed.push(d);
      emit("installed", 2, 2, `${entry.name} is ready to use`);
    },
    onPluginInstall: (cb) => {
      installListeners.add(cb);
      return () => installListeners.delete(cb);
    },
    openUrl: async (url) => {
      window.open(url, "_blank", "noreferrer");
    },
    openPluginWindow: async (trackId) => {
      // No OS windows in a browser; a popup on the same origin runs the very same editor.
      window.open(
        `${window.location.pathname}?plugin=${trackId}`,
        `plugin-${trackId}`,
        "width=900,height=620",
      );
    },

    // ── Effect inserts ────────────────────────────────────────────────────────────────────
    addInsert: async (target, uid, index = null, label) => {
      // The chain is checked first, like the Rust command: an unknown track is the user's
      // mistake, an unknown uid is a stale list.
      if (target !== "master" && !model.tracks.some((t) => t.id === target.track))
        fail(NO_SUCH_TRACK);
      // A stock effect names itself; only a plugin has to be found in the scan.
      const native = nativeEffect(uid);
      const d = native ?? installed.find((x) => x.uid === uid) ?? fail(NO_SUCH_PLUGIN);
      return applyCommand(
        { type: "addInsert", target, uid: d.uid, name: d.name, index },
        false,
        label,
      );
    },
    removeInsert: async (target, insertId, label) => {
      const s = await applyCommand(
        { type: "removeInsert", target, insertId },
        false,
        label,
      );
      // The effect is closed with the slot; a new one at the same index is a new effect.
      paramValues.delete(insertId);
      return s;
    },
    moveInsert: async (target, insertId, toIndex, label) =>
      applyCommand({ type: "moveInsert", target, insertId, toIndex }, false, label),
    setInsertEnabled: async (target, insertId, on, label) =>
      applyCommand(
        { type: "setInsertEnabled", target, insertId, on },
        false,
        label,
      ),
    insertParams: async (insertId) => {
      const found = findInsert(insertId);
      const effect = found && nativeEffect(found.insert.uid);
      if (found && effect) {
        return effect.params.map((p) => ({
          id: p.id,
          name: p.name,
          module: p.module,
          min: p.min,
          max: p.max,
          default: p.default,
          value: nativeValue(effect, found.insert.params, p.id),
          isStepped: p.stepped,
        }));
      }
      const held = liveInsert(insertId);
      return held
        ? mockParamsFor(held.uid).map((p) => ({
            ...p,
            value: held.values.get(p.id) ?? p.default,
          }))
        : [];
    },
    setInsertParam: async (insertId, paramId, value, commit = false) => {
      const found = findInsert(insertId) ?? fail(NO_SUCH_INSERT);
      const native = nativeEffect(found.insert.uid);
      if (native) {
        // A native effect's value is the document's own, so the drag reaches the engine and
        // nothing else, and the release writes it straight in as one undo step.
        native.params.find((x) => x.id === paramId) ?? fail(NO_SUCH_PARAM);
        if (!allFinite(value)) fail(INVALID_VALUE);
        if (!commit) return null;
        return applyCommand(
          { type: "setInsertParam", target: found.target, insertId, paramId, value },
          false,
          `${found.insert.name} settings`,
        );
      }
      const held = liveInsert(insertId) ?? fail(NO_SUCH_INSERT);
      const info =
        mockParamsFor(held.uid).find((p) => p.id === paramId) ??
        fail(NO_SUCH_PARAM);
      if (!allFinite(value)) fail(INVALID_VALUE);
      const v = clamp(value, info.min, info.max);
      held.values.set(paramId, info.isStepped ? Math.round(v) : v);
      // Only the end of the gesture touches the document, exactly as for a track plugin.
      if (!commit) return null;
      const state = encodeState(held.values);
      const s = await applyCommand(
        { type: "setInsertState", target: found.target, insertId, state },
        false,
        `${found.insert.name} settings`,
      );
      held.state = state;
      return s;
    },

    // ── Guitar ────────────────────────────────────────────────────────────────────────────
    // There is no audio device in a browser, so anything that needs one says so plainly
    // rather than pretending to work. Everything that is pure data still does.
    inputDevices: async () => [],
    setAudioArm: async () => ({
      running: false,
      sampleRate: null,
      error: "browser mock: no engine",
    }),
    startAudioRecord: async () => {
      throw new Error("recording needs the desktop app");
    },
    stopAudioRecord: async () => {
      throw new Error("recording needs the desktop app");
    },
    importAudio: async () => {
      throw new Error("importing audio needs the desktop app");
    },
    detectAudioClip: async () => {
      throw new Error("detecting notes needs the desktop app");
    },
    audioPeaks: async (sourceId, buckets) => syntheticPeaks(sourceId, buckets),
    clearInputPeak: async () => ({
      running: false,
      sampleRate: null,
      error: "browser mock: no engine",
    }),
    // No input stream means nothing to detect: the strip stays at rest rather than inventing
    // a string for someone to tune to.
    setTuner: async () => {},
    tunerReading: async () => null,
    onAudioImport: () => () => {},
    onAudioDropped: () => () => {},
  };
}

let bridgePromise: Promise<Bridge> | null = null;

export function getBridge(): Promise<Bridge> {
  bridgePromise ??= isTauri ? tauriBridge() : Promise.resolve(mockBridge());
  return bridgePromise;
}
