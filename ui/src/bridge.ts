// The one place the UI talks to the backend. In Tauri it calls Rust commands; in a plain
// browser (`pnpm --dir ui dev`) it runs an in-memory mock so the interface can be designed and
// exercised without audio. Keep the mock's command semantics aligned with dissonant-core.

import type {
  AudioStatus,
  AudioStatusEvent,
  ChordEvent,
  Clip,
  ExportProgress,
  ExportRequest,
  ExportResult,
  Command,
  MasterSettings,
  MidiStatus,
  NoteEvent,
  OutputDevice,
  Settings,
  PlayheadEvent,
  PlayMode,
  ProjectModel,
  RecoveryCandidate,
  Snapshot,
  Track,
  SongPattern,
  TrackParam,
  TemplateInfo,
  KeyState,
  ScaleType,
  VibeInfo,
} from "./types";
import { beatsPerBar, uuid, withDefaults } from "./types";
import { chordAt, normalize, progression } from "./theory";

export interface Bridge {
  readonly isTauri: boolean;
  getState(): Promise<Snapshot>;
  /** `label` names the edit for the Edit menu ("move notes"); the command's own name otherwise. */
  apply(command: Command, transient?: boolean, label?: string): Promise<Snapshot>;
  commitGesture(): Promise<Snapshot>;
  undo(): Promise<Snapshot>;
  redo(): Promise<Snapshot>;
  /** `starter` picks starter or empty; leave it out to follow the "new project opens" setting. */
  newProject(starter?: boolean): Promise<Snapshot>;
  openProject(path: string): Promise<Snapshot>;
  saveProject(path?: string): Promise<Snapshot>;
  /**
   * What ▶ plays. In pattern mode `chordOffsetBeats` is the song beat the pattern's bar 1 sits
   * on: the chord bed and tiering use the project chords from there.
   */
  setPlaybackContext(mode: PlayMode, patternId: string | null, chordOffsetBeats: number): Promise<void>;
  /** Start playing; with `countInBars` the metronome counts first and the transport starts after. */
  play(countInBars?: number): Promise<void>;
  stop(): Promise<void>;
  setMetronome(on: boolean, volume?: number): Promise<void>;
  seek(beat: number): Promise<void>;
  setHearChords(on: boolean): Promise<void>;
  /** `seconds` is how long the note is held before its automatic release. */
  audition(trackId: string, pitch: number, velocity?: number, seconds?: number): Promise<void>;
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
  midiStatus(): Promise<MidiStatus>;
  openMidiInput(name?: string): Promise<MidiStatus>;
  closeMidiInput(): Promise<MidiStatus>;
  setLiveTrack(trackId: string | null): Promise<void>;
  setRecord(armed: boolean, quantize?: number): Promise<MidiStatus>;
  noteOn(pitch: number, velocity?: number): Promise<void>;
  noteOff(pitch: number): Promise<void>;
  setLoop(start: number, end: number): Promise<void>;
  clearLoop(): Promise<void>;
  setLooping(on: boolean): Promise<void>;
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
  /** The backend opened or dropped a MIDI input on its own. */
  onMidiStatus(cb: (s: MidiStatus) => void): () => void;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// ─── Tauri implementation ──────────────────────────────────────────────────────────────────

async function tauriBridge(): Promise<Bridge> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const dialog = await import("@tauri-apps/plugin-dialog");

  const subscribe = <T,>(name: string, cb: (payload: T) => void): (() => void) => {
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
    apply: (command, transient = false, label) => invoke<Snapshot>("apply", { command, transient, label: label ?? null }),
    commitGesture: () => invoke<Snapshot>("commit_gesture"),
    undo: () => invoke<Snapshot>("undo"),
    redo: () => invoke<Snapshot>("redo"),
    newProject: (starter) => invoke<Snapshot>("new_project", { starter: starter ?? null }),
    openProject: (path) => invoke<Snapshot>("open_project", { path }),
    saveProject: (path) => invoke<Snapshot>("save_project", { path: path ?? null }),
    setPlaybackContext: (mode, patternId, chordOffsetBeats) => invoke("set_playback_context", { mode, patternId, chordOffsetBeats }),
    play: (countInBars) => invoke("transport_play", { countInBars: countInBars ?? null }),
    stop: () => invoke("transport_stop"),
    setMetronome: (on, volume) => invoke("set_metronome", { on, volume: volume ?? null }),
    seek: (beat) => invoke("transport_seek", { beat }),
    setHearChords: (on) => invoke("set_hear_chords", { on }),
    audition: (trackId, pitch, velocity = 100, seconds) => invoke("audition", { trackId, pitch, velocity, seconds }),
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
    midiStatus: () => invoke<MidiStatus>("midi_status"),
    openMidiInput: (name) => invoke<MidiStatus>("open_midi_input", { name: name ?? null }),
    closeMidiInput: () => invoke<MidiStatus>("close_midi_input"),
    setLiveTrack: (trackId) => invoke("set_live_track", { trackId }),
    setRecord: (armed, quantize) => invoke<MidiStatus>("set_record", { armed, quantize: quantize ?? null }),
    noteOn: (pitch, velocity = 100) => invoke("note_on", { pitch, velocity }),
    noteOff: (pitch) => invoke("note_off", { pitch }),
    setLoop: (start, end) => invoke("set_loop", { start, end }),
    clearLoop: () => invoke("clear_loop"),
    setLooping: (on) => invoke("set_looping", { on }),
    pickOpenPath: async (extensions = ["dissonant", "json"], label = "dissonant project") => {
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
    recoveryCandidates: () => invoke<RecoveryCandidate[]>("recovery_candidates"),
    restoreAutosave: (autosavePath) => invoke<Snapshot>("restore_autosave", { autosavePath }),
    discardAutosave: (autosavePath) => invoke("discard_autosave", { autosavePath }),
    autosaveNow: () => invoke<string | null>("autosave_now"),
    getSettings: async () => withDefaults(await invoke<Partial<Settings>>("get_settings")),
    setSettings: (settings) => invoke<AudioStatus>("set_settings", { settings }),
    outputDevices: () => invoke<OutputDevice[]>("output_devices"),
    testTone: () => invoke("test_tone"),
    onMidiStatus: (cb) => subscribe<MidiStatus>("midi-status", cb),
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
const LAST_TRACK = "a project needs at least one track";
const INVALID_VALUE = "invalid value";
/** `ProjectModel` decoding refuses schema < 6 with this text (no migration). */
export const OLD_SCHEMA = "This project was made with an older version of dissonant and can't be opened.";

function fail(message: string): never {
  // A plain string, not an `Error`: that is what Tauri's `invoke` rejects with.
  throw message;
}

const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));
const allFinite = (...xs: number[]) => xs.every((x) => Number.isFinite(x));

/** `TrackParam` ranges, from `apply_to`'s `SetTrackParam` arm. */
const TRACK_PARAM_RANGE: Record<TrackParam, [number, number]> = {
  volume: [0, 1.5],
  reverbSend: [0, 1],
  tone: [200, 20_000],
  pan: [-1, 1],
};

function newTrack(isDrum: boolean, name: string): Track {
  return {
    id: uuid(),
    name,
    voice: "saw",
    muted: false,
    soloed: false,
    isDrum,
    volume: 1,
    reverbSend: 0,
    tone: 18000,
    pan: 0,
    color: null,
  };
}

function newPattern(trackId: string, name: string): SongPattern {
  return { id: uuid(), trackId, name, lengthBeats: 16, notes: [] };
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
  for (const n of copy.notes) n.id = uuid();
  return copy;
}

/** A clip of `patternId` from beat 0 to the song end (loop-extended). */
function songClip(patternId: string, songLengthBeats: number): Clip {
  return { id: uuid(), patternId, startBeat: 0, lengthBeats: songLengthBeats, offsetBeats: 0, muted: false };
}

/** Beats per bar of the model's time signature. */
const barOf = (m: ProjectModel) => beatsPerBar(m.timeSignature);

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
    sections: [],
    master: { gain: 1, reverbWet: 0, lowCutHz: 20, highCutHz: 18000, lowEq: 1, midEq: 1, highEq: 1 },
    chords: [],
    songLengthBeats: songBars * 4,
  };
}

/** `STARTER_MELODY` in model.rs: (start, length, pitch, velocity) per 4-beat chord slot. */
const STARTER_MELODY: [number, number, number, number][][] = [
  [[0, 1.5, 76, 100], [1.5, 0.5, 74, 80], [2, 2, 72, 96]], // C:  E  D  C
  [[0, 1, 69, 100], [1, 1, 72, 88], [2, 2, 77, 96]], // F:  A  C  F
  [[0, 1.5, 74, 100], [1.5, 0.5, 71, 80], [2, 2, 67, 96]], // G:  D  B  G
  [[0, 1.5, 72, 100], [1.5, 0.5, 71, 80], [2, 2, 69, 96]], // Am: C  B  A
];

type Groove = "backbeat" | "fourOnFloor" | "boomBap" | "halfTime" | "motorik" | "sparse";
const [KICK, SNARE, CLAP, HAT] = [36, 38, 39, 42];
const eighths = (on: number, off: number): [number, number, number][] => Array.from({ length: 8 }, (_, i) => [i * 0.5, HAT, i % 2 === 0 ? on : off]);

/** `Groove::bar` in vibes.rs for a 4/4 bar: (offset, pitch, velocity). */
const GROOVE_BARS: Record<Groove, [number, number, number][]> = {
  backbeat: [[0, KICK, 120], [1, SNARE, 100], [2, KICK, 120], [3, SNARE, 100], ...eighths(80, 60)],
  fourOnFloor: [[0, KICK, 120], [1, KICK, 115], [2, KICK, 120], [3, KICK, 115], [1, CLAP, 100], [3, CLAP, 100], ...[0.5, 1.5, 2.5, 3.5].map((o): [number, number, number] => [o, HAT, 85])],
  boomBap: [[0, KICK, 120], [1.5, KICK, 100], [2.5, KICK, 110], [1, SNARE, 105], [3, SNARE, 105], ...eighths(70, 50)],
  halfTime: [[0, KICK, 120], [2.5, KICK, 100], [2, SNARE, 110], ...eighths(70, 55)],
  motorik: [[0, KICK, 115], [2, KICK, 115], [2.5, KICK, 95], [1, SNARE, 100], [3, SNARE, 100], ...eighths(80, 75)],
  sparse: [[0, KICK, 90], [2, HAT, 45]],
};

/** `Groove::hits(16.0, 4.0)`: four bars of 4/4, sorted by time then pitch. */
function grooveHits(groove: Groove): NoteEvent[] {
  const hits: NoteEvent[] = [];
  for (let bar = 0; bar < 16; bar += 4) {
    for (const [offset, pitch, velocity] of GROOVE_BARS[groove]) hits.push({ id: uuid(), startBeat: bar + offset, lengthBeats: 0.25, pitch, velocity });
  }
  return hits.sort((a, b) => a.startBeat - b.startBeat || a.pitch - b.pitch);
}

/** `vibes::bass_line` over 16 beats of 4/4: the chord root under each hit, G♯1–G2. */
function bassLine(chords: ChordEvent[], rhythm: [number, number][]): NoteEvent[] {
  const notes: NoteEvent[] = [];
  for (let bar = 0; bar < 16; bar += 4) {
    rhythm.forEach(([offset, len], i) => {
      const start = bar + offset;
      const chord = chordAt(chords, start);
      if (!chord || !chord.pitchClasses.length) return;
      const pc = normalize(chord.pitchClasses[0]);
      notes.push({ id: uuid(), startBeat: start, lengthBeats: Math.min(len, 16 - start), pitch: 36 + pc - (pc >= 8 ? 12 : 0), velocity: i === 0 ? 105 : 92 });
    });
  }
  return notes;
}

type Sound = [voice: string, tone: number, reverb: number];
const SOUND: Record<string, Sound> = {
  softKeys: ["triangle", 6000, 0.25],
  warmPad: ["pad", 4000, 0.45],
  glassPluck: ["pluck", 12000, 0.3],
  buzzyBass: ["saw", 1200, 0],
  roundBass: ["sine", 2000, 0],
  leadWithBite: ["square", 9000, 0.15],
};

interface MockVibe extends VibeInfo {
  swing: number;
  swingGrid: number;
  degrees: number[];
  chordBeats: number;
  groove: Groove;
  bass: [number, number][];
  bassSound: Sound;
  leadName: string;
  leadSound: Sound;
}

const lockedKey = (rootPitchClass: number, scale: ScaleType): KeyState => ({ rootPitchClass, scale, isLocked: true });
const EIGHTH_BASS: [number, number][] = Array.from({ length: 8 }, (_, i) => [i * 0.5, 0.45]);

/** `VIBES` in vibes.rs. `bridge.test.ts` pins the same values `vibe_values_shared_with_the_ui` does. */
const MOCK_VIBES: MockVibe[] = [
  { id: "lofi", name: "lo-fi bedroom", blurb: "dusty, swung and slow: jazzy chords, lazy drums, room for a melody", tempo: 80, key: lockedKey(5, "major"), swing: 60, swingGrid: 0.25, degrees: [1, 4, 0, 5], chordBeats: 4, groove: "boomBap", bass: [[0, 1.25], [1.5, 1.5], [3.5, 0.5]], bassSound: SOUND.roundBass, leadName: "keys", leadSound: SOUND.softKeys },
  { id: "postpunk", name: "post-punk", blurb: "fast, tense and driving: 8th-note bass, motorik drums, minor chords", tempo: 148, key: lockedKey(4, "minor"), swing: 50, swingGrid: 0.5, degrees: [0, 5, 2, 6], chordBeats: 4, groove: "motorik", bass: EIGHTH_BASS, bassSound: SOUND.buzzyBass, leadName: "lead", leadSound: SOUND.leadWithBite },
  { id: "ambient", name: "ambient texture", blurb: "a slow two-chord drift: barely a beat, long notes, lots of space", tempo: 70, key: lockedKey(2, "major"), swing: 50, swingGrid: 0.5, degrees: [0, 3], chordBeats: 8, groove: "sparse", bass: [[0, 4]], bassSound: SOUND.roundBass, leadName: "pad", leadSound: SOUND.warmPad },
  { id: "songwriter", name: "songwriter demo", blurb: "four friendly chords and a plain beat to sing or play over", tempo: 96, key: lockedKey(7, "major"), swing: 50, swingGrid: 0.5, degrees: [0, 4, 5, 3], chordBeats: 4, groove: "backbeat", bass: [[0, 2], [2, 2]], bassSound: SOUND.roundBass, leadName: "melody", leadSound: SOUND.glassPluck },
  { id: "club", name: "club loop", blurb: "four-on-the-floor, offbeat bass and a minor loop that keeps going", tempo: 124, key: lockedKey(9, "minor"), swing: 50, swingGrid: 0.5, degrees: [0, 6, 5, 6], chordBeats: 4, groove: "fourOnFloor", bass: [[0.5, 0.4], [1.5, 0.4], [2.5, 0.4], [3.5, 0.4]], bassSound: SOUND.buzzyBass, leadName: "lead", leadSound: SOUND.leadWithBite },
];

const withSound = (t: Track, [voice, tone, reverb]: Sound): Track => ({ ...t, voice, tone, reverbSend: reverb });

/**
 * `ProjectModel::from_vibe`: one pattern per track ("keys 1", "bass 1", "drums 1"), the
 * progression tiled across the whole song, one clip per track from 0 to the song end.
 */
function vibeModel(id: string, songBars = defaultSongBars()): ProjectModel | null {
  const v = MOCK_VIBES.find((x) => x.id === id);
  if (!v) return null;
  const m = emptyModel(songBars);
  m.tempo = v.tempo;
  m.swing = v.swing;
  m.swingGrid = v.swingGrid;
  m.key = { ...v.key };
  m.chords = progression(v.degrees, v.key.rootPitchClass!, v.key.scale, m.songLengthBeats, v.chordBeats);
  const lead = withSound({ ...m.tracks[0], name: v.leadName }, v.leadSound);
  const bass = withSound(newTrack(false, "bass"), v.bassSound);
  const drums = newTrack(true, "drums");
  m.tracks = [lead, bass, drums];
  m.patterns = [
    newPattern(lead.id, `${lead.name} 1`),
    { ...newPattern(bass.id, "bass 1"), notes: bassLine(m.chords, v.bass) },
    { ...newPattern(drums.id, "drums 1"), notes: grooveHits(v.groove) },
  ];
  m.clips = m.patterns.map((p) => songClip(p.id, m.songLengthBeats));
  return m;
}

/**
 * `ProjectModel::starter()`: I–IV–V–vi in C (key locked) across the whole song, a pluck melody
 * pattern and a drum pattern, each placed from 0 to the song end so ▶ plays music at once.
 */
function starterModel(songBars = defaultSongBars()): ProjectModel {
  const m = emptyModel(songBars);
  m.key = { rootPitchClass: 0, scale: "major", isLocked: true };
  m.chords = progression([0, 3, 4, 5], 0, "major", m.songLengthBeats);
  const melody = m.tracks[0];
  melody.voice = "pluck";
  melody.reverbSend = 0.2;
  const drums = newTrack(true, "drums");
  m.tracks.push(drums);
  m.patterns[0].notes = STARTER_MELODY.flatMap((phrase, i) =>
    phrase.map(([start, lengthBeats, pitch, velocity]) => ({ id: uuid(), startBeat: i * 4 + start, lengthBeats, pitch, velocity })),
  );
  m.patterns.push({ ...newPattern(drums.id, "drums 1"), notes: grooveHits("backbeat") });
  m.clips = m.patterns.map((p) => songClip(p.id, m.songLengthBeats));
  return m;
}

/**
 * `NoteEvent::sanitized`. Pitch and velocity are `i32` in Rust and every number crosses the IPC
 * boundary as JSON, so a non-finite or fractional value is rejected outright rather than
 * silently stored — in the desktop app serde would refuse the whole payload.
 */
function sanitizeNote(n: NoteEvent): NoteEvent {
  if (!allFinite(n.startBeat, n.lengthBeats) || !Number.isInteger(n.pitch) || !Number.isInteger(n.velocity)) {
    fail(INVALID_VALUE);
  }
  return {
    ...n,
    pitch: clamp(n.pitch, 0, 127),
    velocity: clamp(n.velocity, 1, 127),
    startBeat: Math.max(0, n.startBeat),
    lengthBeats: Math.max(1 / 64, n.lengthBeats),
  };
}

/** The schema this build writes; mirrors `SCHEMA_VERSION` in crates/dissonant-core/src/model.rs. */
const SCHEMA_VERSION = 6;

/**
 * Decoding plus `ProjectModel::normalized`, run on anything read back from storage. A file
 * older than schema 6 is refused: there is no migration from patterns that hold every track.
 */
function normalizedModel(m: ProjectModel): ProjectModel {
  if (!((m.schemaVersion ?? 0) >= SCHEMA_VERSION)) fail(OLD_SCHEMA);
  if (m.tracks.length === 0) m.tracks = [newTrack(false, "melody")];
  const trackIds = new Set(m.tracks.map((t) => t.id));
  // A pattern of a track that no longer exists is invisible and never played.
  m.patterns = (m.patterns ?? []).filter((p) => trackIds.has(p.trackId));
  for (const p of m.patterns) p.notes = (p.notes ?? []).map(sanitizeNote);
  m.clips = (m.clips ?? []).filter((c) => m.patterns.some((p) => p.id === c.patternId));
  m.chords = m.chords ?? [];
  m.songLengthBeats = Number.isFinite(m.songLengthBeats) ? Math.max(barOf(m), m.songLengthBeats) : defaultSongBars() * barOf(m);
  return m;
}

/**
 * `AddClip` / `UpdateClip` clamping: a clip may not start at or after the song end, and its end
 * is cut back to it. Returns the clamped length.
 */
function clampToSong(m: ProjectModel, startBeat: number, lengthBeats: number): number {
  if (startBeat >= m.songLengthBeats) fail(INVALID_VALUE);
  return Math.min(lengthBeats, m.songLengthBeats - startBeat);
}

/** `UpdateClip`: values, then the clip id; the pattern never changes (a clip stays on its track). */
function updateClip(m: ProjectModel, clip: Clip): void {
  if (!allFinite(clip.startBeat, clip.lengthBeats, clip.offsetBeats)) fail(INVALID_VALUE);
  if (clip.startBeat < 0 || clip.lengthBeats <= 0 || clip.offsetBeats < 0) fail(INVALID_VALUE);
  const i = m.clips.findIndex((cl) => cl.id === clip.id);
  if (i < 0) fail(NO_SUCH_CLIP);
  const lengthBeats = clampToSong(m, clip.startBeat, clip.lengthBeats);
  m.clips[i] = { ...clip, patternId: m.clips[i].patternId, lengthBeats };
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
  const track = (id: string) => m.tracks.find((t) => t.id === id) ?? fail(NO_SUCH_TRACK);
  const pattern = (id: string) => m.patterns.find((p) => p.id === id) ?? fail(NO_SUCH_PATTERN);
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
      if (c.color !== null && !/^#[0-9a-f]{6}$/i.test(c.color)) fail(INVALID_VALUE);
      track(c.id).color = c.color === null ? null : c.color.toLowerCase();
      break;
    case "deleteTrack": {
      if (m.tracks.length <= 1) fail(LAST_TRACK);
      const before = m.tracks.length;
      m.tracks = m.tracks.filter((t) => t.id !== c.id);
      if (m.tracks.length === before) fail(NO_SUCH_TRACK);
      // Its patterns go with it, and their clips with them.
      const gone = new Set(m.patterns.filter((p) => p.trackId === c.id).map((p) => p.id));
      m.patterns = m.patterns.filter((p) => !gone.has(p.id));
      m.clips = m.clips.filter((cl) => !gone.has(cl.patternId));
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
      if (j >= 0 && j < m.tracks.length) [m.tracks[i], m.tracks[j]] = [m.tracks[j], m.tracks[i]];
      break;
    }
    case "setTrackVoice":
      track(c.id).voice = c.voice;
      break;
    case "setTrackParam": {
      if (!allFinite(c.value)) fail(INVALID_VALUE);
      const [lo, hi] = TRACK_PARAM_RANGE[c.param];
      track(c.id)[c.param] = clamp(c.value, lo, hi);
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
      if (!allFinite(c.beats) || c.beats < 1) fail(INVALID_VALUE);
      pattern(c.id).lengthBeats = c.beats;
      break;
    case "setNotes":
      pattern(c.patternId).notes = c.notes.map(sanitizeNote).sort((a, b) => a.startBeat - b.startBeat || a.pitch - b.pitch);
      break;
    case "setChords":
      // The project progression, in song beats.
      m.chords = c.chords
        .filter((ch) => ch.lengthBeats > 0 && ch.startBeat >= 0)
        .map((ch) => ({ ...ch, pitchClasses: [...new Set(ch.pitchClasses.map(normalize))].sort((a, b) => a - b) }))
        .sort((a, b) => a.startBeat - b.startBeat);
      break;
    case "addClip": {
      const p = pattern(c.patternId);
      const length = c.lengthBeats ?? p.lengthBeats;
      if (!allFinite(c.startBeat, length) || c.startBeat < 0 || length <= 0) fail(INVALID_VALUE);
      const lengthBeats = clampToSong(m, c.startBeat, length);
      m.clips.push({ id: uuid(), patternId: p.id, startBeat: c.startBeat, lengthBeats, offsetBeats: 0, muted: false });
      m.clips.sort((a, b) => a.startBeat - b.startBeat);
      break;
    }
    case "updateClip":
      updateClip(m, c.clip);
      m.clips.sort((a, b) => a.startBeat - b.startBeat);
      break;
    case "updateClips":
      // One command, so one undo step: a group move.
      for (const clip of c.clips) updateClip(m, clip);
      m.clips.sort((a, b) => a.startBeat - b.startBeat);
      break;
    case "removeClip": {
      const before = m.clips.length;
      m.clips = m.clips.filter((cl) => cl.id !== c.id);
      if (m.clips.length === before) fail(NO_SUCH_CLIP);
      break;
    }
    case "removeClips": {
      const ids = new Set(c.ids);
      for (const id of ids) if (!m.clips.some((cl) => cl.id === id)) fail(NO_SUCH_CLIP);
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
      if (!allFinite(c.beats)) fail(INVALID_VALUE);
      const end = Math.max(barOf(m), c.beats);
      m.songLengthBeats = end;
      // Clips starting at or after the new end go; clips crossing it are trimmed.
      m.clips = m.clips.filter((cl) => cl.startBeat < end).map((cl) => (cl.startBeat + cl.lengthBeats > end ? { ...cl, lengthBeats: end - cl.startBeat } : cl));
      break;
    }
    case "addSection":
      if (!allFinite(c.startBeat) || c.startBeat < 0) fail(INVALID_VALUE);
      m.sections.push({ id: uuid(), name: c.name.trim() || `section ${m.sections.length + 1}`, startBeat: Math.max(0, c.startBeat), key: null, color: null });
      m.sections.sort((a, b) => a.startBeat - b.startBeat);
      break;
    case "updateSection": {
      if (!allFinite(c.section.startBeat) || c.section.startBeat < 0 || !c.section.name.trim()) fail(INVALID_VALUE);
      const i = m.sections.findIndex((x) => x.id === c.section.id);
      if (i < 0) fail(NO_SUCH_SECTION);
      m.sections[i] = { ...c.section };
      m.sections.sort((a, b) => a.startBeat - b.startBeat);
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
      if (!allFinite(v.gain, v.reverbWet, v.lowCutHz, v.highCutHz, v.lowEq, v.midEq, v.highEq)) fail(INVALID_VALUE);
      const master: MasterSettings = {
        gain: clamp(v.gain, 0, 1.5),
        reverbWet: clamp(v.reverbWet, 0, 1),
        lowCutHz: clamp(v.lowCutHz, 10, 2_000),
        highCutHz: clamp(v.highCutHz, 500, 20_000),
        lowEq: clamp(v.lowEq, 0, 2),
        midEq: clamp(v.midEq, 0, 2),
        highEq: clamp(v.highEq, 0, 2),
      };
      m.master = master;
      break;
    }
    case "setTimeSignature":
      m.timeSignature = { numerator: c.numerator, denominator: c.denominator };
      break;
    case "setSwing":
      m.swing = Math.min(75, Math.max(50, c.swing));
      m.swingGrid = c.grid === 0.25 ? 0.25 : 0.5;
      break;
    case "addTempoPoint":
      if (c.beat < 0) break;
      m.tempoPoints = [...(m.tempoPoints ?? []).filter((p) => Math.abs(p.beat - c.beat) > 1e-9), { id: uuid(), beat: c.beat, bpm: Math.min(300, Math.max(20, c.bpm)), ramp: c.ramp }].sort((a, b) => a.beat - b.beat);
      break;
    case "updateTempoPoint": {
      const i = (m.tempoPoints ?? []).findIndex((p) => p.id === c.point.id);
      if (i < 0 || c.point.beat < 0) break;
      m.tempoPoints = (m.tempoPoints ?? []).filter((p) => p.id === c.point.id || Math.abs(p.beat - c.point.beat) > 1e-9).map((p) => (p.id === c.point.id ? { ...c.point, bpm: Math.min(300, Math.max(20, c.point.bpm)) } : p)).sort((a, b) => a.beat - b.beat);
      break;
    }
    case "removeTempoPoint":
      m.tempoPoints = (m.tempoPoints ?? []).filter((p) => p.id !== c.id);
      break;
  }
}

/** `document::MAX_UNDO`. */
const MAX_UNDO = 200;

/** The mock's stand-in for the filesystem: one slot, written by save, read by open. */
const STORAGE_KEY = "dissonant.mock.project";
const TEMPLATES_KEY = "dissonant.mock.templates";

function mockBridge(): Bridge {
  let model = starterModel();
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
  let beat = 0;
  let last = performance.now();
  let loop: [number, number] | null = null;
  let looping = true;
  let armed = false;
  let quantize = 0.25;
  let liveTrack: string | null = null;
  const held = new Map<number, { start: number; velocity: number }>();
  const listeners = new Set<(e: PlayheadEvent) => void>();
  const docListeners = new Set<(s: Snapshot) => void>();
  const midiListeners = new Set<(p: number) => void>();

  const undoLabels: string[] = [];
  const redoLabels: string[] = [];
  let pendingLabel = "edit";
  const closeListeners = new Set<() => void>();
  const isDirty = () => saved !== JSON.stringify(model);
  const snapshot = (): Snapshot => ({
    model: clone(model),
    // An open gesture sitting on its starting value is not undoable, matching `Document::can_undo`.
    canUndo: undo.length > 0 || (pending !== null && JSON.stringify(pending) !== JSON.stringify(model)),
    canRedo: redo.length > 0,
    dirty: isDirty(),
    path,
    undoLabel: pending ? pendingLabel : (undoLabels[undoLabels.length - 1] ?? null),
    redoLabel: redoLabels[redoLabels.length - 1] ?? null,
  });
  const defaultLabel = (c: Command) => c.type.replace(/([A-Z])/g, " $1").toLowerCase().trim();
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
    if (mode === "song") return model.songLengthBeats;
    return model.patterns.find((x) => x.id === patternId)?.lengthBeats ?? 16;
  };

  const tick = () => {
    const now = performance.now();
    if (playing) {
      beat += ((now - last) / 1000) * (model.tempo / 60);
      const len = loopLength();
      const [ls, le] = loop && loop[1] > loop[0] ? [Math.min(loop[0], len), Math.min(loop[1], len)] : [0, len];
      if (looping && beat >= le && le > ls) beat = ls + ((beat - le) % (le - ls));
    }
    last = now;
    const e: PlayheadEvent = { beat, playing, masterPeak: [0, 0], trackPeaks: [] };
    for (const cb of listeners) cb(e);
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

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
      gain.gain.exponentialRampToValueAtTime(0.15 * (velocity / 127), ctx.currentTime + 0.01);
      gain.gain.setValueAtTime(0.15 * (velocity / 127), ctx.currentTime + hold);
      gain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + hold + 0.08);
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
    beat = 0;
  };

  const commitGesture = () => {
    if (pending && JSON.stringify(pending) !== JSON.stringify(model)) pushUndo(pending, pendingLabel);
    pending = null;
  };

  return {
    isTauri: false,
    getState: async () => snapshot(),
    apply: async (command, transient = false, label) => {
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
      return snapshot();
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
    // The mock plays no chord bed, so the chord offset only matters to the engine.
    setPlaybackContext: async (m, pid) => {
      if (m !== mode || pid !== patternId) beat = 0;
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
    },
    seek: async (b) => {
      beat = Math.max(0, b);
    },
    setHearChords: async () => {},
    audition: async (_t, pitch, velocity = 100, seconds) => blip(pitch, velocity, seconds),
    auditionOff: async () => {},
    audioStatus: async () => ({ running: false, sampleRate: null, error: "browser mock: no engine" }),
    restartAudio: async () => ({ running: false, sampleRate: null, error: "browser mock: no engine" }),
    exportWav: async () => {
      throw new Error("export needs the desktop app — the browser mock renders nothing");
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
    listVibes: async () => MOCK_VIBES.map(({ id, name, blurb, tempo, key }) => ({ id, name, blurb, tempo, key: { ...key } })),
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
    midiStatus: async () => ({ inputs: [], open: null, armed }),
    openMidiInput: async () => ({ inputs: [], open: null, armed }),
    closeMidiInput: async () => ({ inputs: [], open: null, armed }),
    setLiveTrack: async (id) => {
      liveTrack = id;
    },
    setRecord: async (a, q) => {
      armed = a;
      if (q !== undefined) quantize = q;
      if (!a) held.clear();
      return { inputs: [], open: null, armed };
    },
    noteOn: async (pitch, velocity = 100) => {
      blip(pitch, velocity);
      for (const cb of midiListeners) cb(pitch);
      if (armed && playing) held.set(pitch, { start: beat, velocity });
    },
    noteOff: async (pitch) => {
      const h = held.get(pitch);
      held.delete(pitch);
      if (!h) return;
      // Recording lands in the context pattern, and only while the live track is its track.
      const p = model.patterns.find((x) => x.id === patternId);
      if (!p || (liveTrack ?? model.tracks[0]?.id) !== p.trackId) return;
      const grid = quantize > 0 ? quantize : 1 / 32;
      let len = beat - h.start;
      if (len < 0) len += p.lengthBeats;
      const start = Math.round((h.start % p.lengthBeats) / grid) * grid;
      const notes = p.notes.filter((n) => !(n.pitch === pitch && Math.abs(n.startBeat - start) < grid / 2));
      notes.push({ id: uuid(), startBeat: Math.min(start, p.lengthBeats - grid), lengthBeats: Math.max(grid, Math.round(len / grid) * grid), pitch, velocity: h.velocity });
      pushUndo(clone(model), "record notes");
      redo.length = 0;
      redoLabels.length = 0;
      reduce(model, { type: "setNotes", patternId: p.id, notes });
      for (const cb of docListeners) cb(snapshot());
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
    pickOpenPath: async () => null,
    pickSavePath: async (name, ext) => (name.endsWith(`.${ext}`) ? name : `${name}.${ext}`),
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
      return { running: false, sampleRate: null, error: "browser mock: no engine" };
    },
    outputDevices: async () => [
      { name: "System default", isDefault: true, sampleRates: [44100, 48000, 96000], defaultSampleRate: 48000 },
      { name: "Mock interface", isDefault: false, sampleRates: [44100, 48000, 88200, 96000, 192000], defaultSampleRate: 44100 },
    ],
    testTone: async () => blip(69, 100),
    onMidiStatus: () => () => {},
  };
}

let bridgePromise: Promise<Bridge> | null = null;

export function getBridge(): Promise<Bridge> {
  bridgePromise ??= isTauri ? tauriBridge() : Promise.resolve(mockBridge());
  return bridgePromise;
}
