// Mirrors the Rust model in crates/dissonant-core (serde camelCase). Keep in sync.

export type ScaleType = "major" | "minor";

export interface KeyState {
  rootPitchClass: number | null;
  scale: ScaleType;
  isLocked: boolean;
}

export const NO_KEY: KeyState = { rootPitchClass: null, scale: "major", isLocked: false };

export interface ChordEvent {
  id: string;
  startBeat: number;
  lengthBeats: number;
  pitchClasses: number[];
  name: string | null;
}

export interface NoteEvent {
  id: string;
  startBeat: number;
  lengthBeats: number;
  pitch: number;
  velocity: number;
  /** The player marked this dissonance as deliberate. */
  intentional?: boolean;
  /** Kept in the pattern but not played (drawn hollow). */
  muted?: boolean;
}

export interface TimeSignature {
  numerator: number;
  denominator: number;
}

/** Quarter-note beats per bar. */
export function beatsPerBar(ts: TimeSignature | undefined): number {
  if (!ts) return 4;
  return (ts.numerator * 4) / Math.max(1, ts.denominator);
}

export const TIME_SIGNATURES: TimeSignature[] = [
  { numerator: 2, denominator: 4 },
  { numerator: 3, denominator: 4 },
  { numerator: 4, denominator: 4 },
  { numerator: 5, denominator: 4 },
  { numerator: 6, denominator: 8 },
  { numerator: 7, denominator: 8 },
];

export interface Track {
  id: string;
  name: string;
  voice: string;
  muted: boolean;
  soloed: boolean;
  isDrum: boolean;
  volume: number;
  reverbSend: number;
  tone: number;
  pan: number;
  /** `#rrggbb`, or null to use the palette by position. */
  color: string | null;
}

/** A pattern belongs to one track; its clips play on that track's lane. */
export interface SongPattern {
  id: string;
  trackId: string;
  name: string;
  lengthBeats: number;
  notes: NoteEvent[];
}

/** A pattern placed on the song; loops from `offsetBeats` for `lengthBeats`. Its track is its pattern's track. */
export interface Clip {
  id: string;
  patternId: string;
  startBeat: number;
  lengthBeats: number;
  offsetBeats: number;
  muted: boolean;
}

/** A song marker, optionally with its own key (modulation). */
export interface Section {
  id: string;
  name: string;
  startBeat: number;
  key: KeyState | null;
  color: string | null;
}

export interface MasterSettings {
  gain: number;
  reverbWet: number;
  lowCutHz: number;
  highCutHz: number;
  lowEq: number;
  midEq: number;
  highEq: number;
}

export interface ProjectModel {
  /** 6 for everything this build reads or writes; older files are refused. */
  schemaVersion: number;
  tempo: number;
  key: KeyState;
  tracks: Track[];
  patterns: SongPattern[];
  clips: Clip[];
  sections: Section[];
  master: MasterSettings;
  timeSignature?: TimeSignature;
  /** 50 = straight … 75 = hard shuffle. */
  swing?: number;
  /** 0.5 (eighths) or 0.25 (sixteenths). */
  swingGrid?: number;
  /** Tempo changes along the song (absent / empty = constant). */
  tempoPoints?: TempoPoint[];
  /** The project-wide progression in song beats: sorted, non-overlapping. Monitor only. */
  chords: ChordEvent[];
  /** The song's hard end: clips can't start at or run past it. At least one bar. */
  songLengthBeats: number;
}

export interface TempoPoint {
  id: string;
  beat: number;
  bpm: number;
  /** Glide linearly from the previous point's tempo to this one. */
  ramp: boolean;
}

/** Mirror of `dissonant_core::tempo::TempoMap`: tempo and elapsed seconds along the song. */
export function tempoMap(baseBpm: number, points: TempoPoint[] | undefined) {
  const anchors: { beat: number; bpm: number; ramp: boolean }[] = [{ beat: 0, bpm: baseBpm, ramp: false }];
  for (const p of [...(points ?? [])].sort((a, b) => a.beat - b.beat)) {
    if (p.beat <= 0) anchors[0] = { beat: 0, bpm: p.bpm, ramp: false };
    else if (anchors[anchors.length - 1].beat !== p.beat) anchors.push({ beat: p.beat, bpm: p.bpm, ramp: p.ramp });
  }
  const segSeconds = (b0: number, bpm0: number, b1: number, bpm1: number) => {
    const len = b1 - b0;
    if (len <= 0) return 0;
    return Math.abs(bpm1 - bpm0) < 1e-9 ? (60 * len) / bpm0 : ((60 * len) / (bpm1 - bpm0)) * Math.log(bpm1 / bpm0);
  };
  const starts: number[] = [0];
  for (let i = 1; i < anchors.length; i++) {
    const a = anchors[i - 1];
    const n = anchors[i];
    starts.push(starts[i - 1] + segSeconds(a.beat, a.bpm, n.beat, n.ramp ? n.bpm : a.bpm));
  }
  const segAt = (beat: number) => {
    let i = 0;
    while (i + 1 < anchors.length && anchors[i + 1].beat <= beat) i++;
    return i;
  };
  const bpmAt = (beat: number) => {
    const i = segAt(beat);
    const a = anchors[i];
    const n = anchors[i + 1];
    if (!n || !n.ramp) return a.bpm;
    const t = Math.min(1, Math.max(0, (beat - a.beat) / (n.beat - a.beat)));
    return a.bpm + (n.bpm - a.bpm) * t;
  };
  const secondsAt = (beat: number) => {
    const i = segAt(Math.max(0, beat));
    const a = anchors[i];
    return starts[i] + segSeconds(a.beat, a.bpm, Math.max(0, beat), bpmAt(beat));
  };
  return { bpmAt, secondsAt, isConstant: anchors.length === 1 };
}

/**
 * Swing as a piecewise-linear warp of the beat line (mirrors `dissonant_core::tempo::swing_warp`):
 * the second half of each `2·grid` pair is pushed so it lands at `2·grid·swing/100`.
 */
export function swingWarp(beat: number, swing: number, grid: number): number {
  if (!(grid > 0) || !(swing > 50) || swing > 75) return beat;
  const pair = 2 * grid;
  const late = (pair * swing) / 100;
  const base = Math.floor(beat / pair) * pair;
  const u = beat - base;
  return base + (u <= grid ? u * (late / grid) : late + (u - grid) * ((pair - late) / grid));
}

/** The song's explicit length in beats. */
export function songLength(m: ProjectModel): number {
  return m.songLengthBeats;
}

/** The track a clip plays on (its pattern's track), or null for a dangling clip. */
export function clipTrackId(m: ProjectModel, clip: Clip): string | null {
  return m.patterns.find((p) => p.id === clip.patternId)?.trackId ?? null;
}

/**
 * The song chords inside `[offset, offset + length)`, shifted to start at 0 and clipped to the
 * window: what a pattern edited at `offset` is tiered against (the engine's pattern-mode bed).
 */
export function chordsInWindow(chords: ChordEvent[], offset: number, length: number): ChordEvent[] {
  const end = offset + length;
  const out: ChordEvent[] = [];
  for (const c of chords) {
    const s = Math.max(c.startBeat, offset);
    const e = Math.min(c.startBeat + c.lengthBeats, end);
    if (e - s > 1e-9) out.push({ ...c, startBeat: s - offset, lengthBeats: e - s });
  }
  return out;
}

/**
 * Song chords with the window `[offset, offset + length)` replaced by `local` (window-relative,
 * clipped to the window). Chords crossing the window's edges are trimmed to the outside part.
 */
export function replaceChordWindow(chords: ChordEvent[], offset: number, length: number, local: ChordEvent[]): ChordEvent[] {
  const end = offset + length;
  const out: ChordEvent[] = [];
  for (const c of chords) {
    const cEnd = c.startBeat + c.lengthBeats;
    if (c.startBeat < offset) out.push({ ...c, lengthBeats: Math.min(cEnd, offset) - c.startBeat });
    if (cEnd > end) {
      const s = Math.max(c.startBeat, end);
      // A chord spanning the whole window splits in two: give the right half its own id.
      out.push({ ...c, id: c.startBeat < offset ? uuid() : c.id, startBeat: s, lengthBeats: cEnd - s });
    }
  }
  for (const c of local) {
    const s = Math.max(0, c.startBeat);
    const e = Math.min(length, c.startBeat + c.lengthBeats);
    if (e - s > 1e-9) out.push({ ...c, startBeat: offset + s, lengthBeats: e - s });
  }
  return out.filter((c) => c.lengthBeats > 1e-9).sort((a, b) => a.startBeat - b.startBeat);
}

export function sectionAt(m: ProjectModel, beat: number): Section | null {
  let best: Section | null = null;
  for (const s of m.sections) if (s.startBeat <= beat && (!best || s.startBeat >= best.startBeat)) best = s;
  return best;
}

export function keyAt(m: ProjectModel, beat: number): KeyState {
  return sectionAt(m, beat)?.key ?? m.key;
}

export interface Snapshot {
  model: ProjectModel;
  canUndo: boolean;
  canRedo: boolean;
  dirty: boolean;
  path: string | null;
  /** What the next undo reverts ("move notes"). */
  undoLabel: string | null;
  redoLabel: string | null;
}

/** An autosave from an earlier run that may hold unsaved work. */
export interface RecoveryCandidate {
  autosavePath: string;
  projectPath: string | null;
  savedAtMs: number;
}

export function fileNameOf(path: string | null | undefined, fallback = "untitled.dissonant"): string {
  if (!path) return fallback;
  return path.split(/[\\/]/).pop() || fallback;
}

export type TrackParam = "volume" | "reverbSend" | "tone" | "pan";
/** What the engine plays: one pattern (looped) or the whole song. */
export type PlayMode = "pattern" | "song";
/** What the window shows. SETUP and SONG both play the song. */
export type ViewMode = "setup" | "pattern" | "song";
export type Tier = "chordTone" | "tension" | "dissonance";

export type Command =
  | { type: "setTempo"; bpm: number }
  | { type: "setKey"; key: KeyState }
  | { type: "addTrack"; isDrum: boolean }
  | { type: "deleteTrack"; id: string }
  | { type: "renameTrack"; id: string; name: string }
  | { type: "setTrackMuted"; id: string; muted: boolean }
  | { type: "setTrackSoloed"; id: string; soloed: boolean }
  | { type: "moveTrack"; id: string; up: boolean }
  | { type: "setTrackVoice"; id: string; voice: string }
  | { type: "setTrackParam"; id: string; param: TrackParam; value: number }
  | { type: "setTrackColor"; id: string; color: string | null }
  | { type: "addPattern"; trackId: string }
  | { type: "duplicatePattern"; id: string }
  | { type: "deletePattern"; id: string }
  | { type: "renamePattern"; id: string; name: string }
  | { type: "setPatternLength"; id: string; beats: number }
  | { type: "setNotes"; patternId: string; notes: NoteEvent[] }
  | { type: "setChords"; chords: ChordEvent[] }
  | { type: "addClip"; patternId: string; startBeat: number; lengthBeats: number | null }
  | { type: "updateClip"; clip: Clip }
  | { type: "updateClips"; clips: Clip[] }
  | { type: "removeClip"; id: string }
  | { type: "removeClips"; ids: string[] }
  | { type: "makeClipUnique"; id: string }
  | { type: "setSongLength"; beats: number }
  | { type: "addSection"; name: string; startBeat: number }
  | { type: "updateSection"; section: Section }
  | { type: "removeSection"; id: string }
  | { type: "setMaster"; master: MasterSettings }
  | { type: "setTimeSignature"; numerator: number; denominator: number }
  | { type: "setSwing"; swing: number; grid: number }
  | { type: "addTempoPoint"; beat: number; bpm: number; ramp: boolean }
  | { type: "updateTempoPoint"; point: TempoPoint }
  | { type: "removeTempoPoint"; id: string };

export interface PlayheadEvent {
  beat: number;
  playing: boolean;
  masterPeak: [number, number];
  trackPeaks: number[];
  /** Beats of count-in left; 0 when not counting in. */
  countIn?: number;
}

export interface AudioStatus {
  running: boolean;
  sampleRate: number | null;
  error: string | null;
  deviceName?: string | null;
  bufferSize?: number | null;
  blockFrames?: number;
  latencyMs?: number;
  load?: number;
  xruns?: number;
}

/** What the export dialog sends; `null` fields fall back to the export settings. */
export interface ExportRequest {
  path: string;
  scope: "song" | "pattern";
  sampleRate: number | null;
  bitDepth: 16 | 24 | 32 | null;
  dither: boolean | null;
  normalizeDb: number | null;
  tailSeconds: number | null;
  loops: number | null;
  stems: boolean;
}

export interface ExportProgress {
  done: number;
  total: number;
  current: string;
}

export interface ExportResult {
  files: string[];
}

export interface TemplateInfo {
  name: string;
  path: string;
}

/** `dissonant_core::VibeInfo`: a built-in project to start from (lo-fi, post-punk, …). */
export interface VibeInfo {
  id: string;
  name: string;
  blurb: string;
  tempo: number;
  key: KeyState;
}

export interface OutputDevice {
  name: string;
  isDefault: boolean;
  sampleRates: number[];
  defaultSampleRate: number;
}

// ─── Settings (mirrors src-tauri/src/settings.rs) ──────────────────────────────────────────

export type VelocityCurve = "linear" | "soft" | "hard" | "fixed";
export type NewProjectKind = "starter" | "empty";
export type AltKey = "noSnap" | "paint";

export interface TierColors {
  chordTone: string;
  tension: string;
  dissonance: string;
}

export interface Settings {
  audio: { device: string | null; sampleRate: number | null; bufferSize: number | null };
  midi: { defaultInput: string | null; autoReconnect: boolean; velocityCurve: VelocityCurve; channel: number | null; octaveOffset: number };
  editing: {
    defaultGrid: number;
    defaultNoteLength: number;
    defaultVelocity: number;
    defaultPatternBars: number;
    /** Length of a new project's song, in bars. */
    defaultSongBars: number;
    defaultTempo: number;
    defaultTimeSignature: TimeSignature;
    newProject: NewProjectKind;
    auditionOnPlace: boolean;
    confirmDestructive: boolean;
    altKey: AltKey;
    snapToChords: boolean;
    showWelcome: boolean;
  };
  export: { sampleRate: number; bitDepth: 16 | 24 | 32; dither: boolean; normalize: boolean; normalizeDb: number; tailSeconds: number };
  appearance: { uiScale: number; rowHeight: number; reducedMotion: boolean; tierColors: TierColors; accent: string };
  metronome: { on: boolean; volume: number; countInBars: number; preRollBars: number; duringPlayback: boolean };
}

export const DEFAULT_TIER_COLORS: TierColors = { chordTone: "#3dffb0", tension: "#ff9d2a", dissonance: "#ff3b30" };

/** Tier palettes: the default, and an Okabe–Ito set that survives red-green and blue-yellow CVD. */
export const TIER_PRESETS: { name: string; colors: TierColors }[] = [
  { name: "default", colors: DEFAULT_TIER_COLORS },
  { name: "okabe–ito (color-blind safe)", colors: { chordTone: "#56b4e9", tension: "#f0e442", dissonance: "#d55e00" } },
  { name: "cool", colors: { chordTone: "#4dd2ff", tension: "#c9b3ff", dissonance: "#ff5f9e" } },
];

export const DEFAULT_SETTINGS: Settings = {
  audio: { device: null, sampleRate: null, bufferSize: null },
  midi: { defaultInput: null, autoReconnect: true, velocityCurve: "linear", channel: null, octaveOffset: 0 },
  editing: {
    defaultGrid: 0.25,
    defaultNoteLength: 0.25,
    defaultVelocity: 100,
    defaultPatternBars: 4,
    defaultSongBars: 32,
    defaultTempo: 120,
    defaultTimeSignature: { numerator: 4, denominator: 4 },
    newProject: "starter",
    auditionOnPlace: true,
    confirmDestructive: true,
    altKey: "noSnap",
    snapToChords: false,
    showWelcome: true,
  },
  export: { sampleRate: 44100, bitDepth: 16, dither: true, normalize: false, normalizeDb: -1, tailSeconds: 1.5 },
  appearance: { uiScale: 1, rowHeight: 18, reducedMotion: false, tierColors: DEFAULT_TIER_COLORS, accent: "#b48cff" },
  metronome: { on: false, volume: 0.6, countInBars: 1, preRollBars: 0, duringPlayback: true },
};

/** Deep-merge a (possibly partial / older) settings object over the defaults. */
export function withDefaults(s: Partial<Settings> | null | undefined): Settings {
  const d = DEFAULT_SETTINGS;
  return {
    audio: { ...d.audio, ...(s?.audio ?? {}) },
    midi: { ...d.midi, ...(s?.midi ?? {}) },
    editing: { ...d.editing, ...(s?.editing ?? {}), defaultTimeSignature: { ...d.editing.defaultTimeSignature, ...(s?.editing?.defaultTimeSignature ?? {}) } },
    export: { ...d.export, ...(s?.export ?? {}) },
    appearance: { ...d.appearance, ...(s?.appearance ?? {}), tierColors: { ...d.appearance.tierColors, ...(s?.appearance?.tierColors ?? {}) } },
    metronome: { ...d.metronome, ...(s?.metronome ?? {}) },
  };
}

export interface AudioStatusEvent {
  status: AudioStatus;
  message: string;
}

export interface MidiStatus {
  inputs: string[];
  open: string | null;
  armed: boolean;
}

export const VOICES = ["saw", "square", "triangle", "sine", "pad", "pluck"] as const;

/** A sound named by character: a voice plus tone (low-pass Hz) and reverb send. */
export interface SoundPreset {
  name: string;
  voice: (typeof VOICES)[number];
  tone: number;
  reverbSend: number;
}

/** Instrument presets by character. The vibe templates in `dissonant-core` use the same values. */
export const SOUND_PRESETS: SoundPreset[] = [
  { name: "warm pad", voice: "pad", tone: 4000, reverbSend: 0.45 },
  { name: "soft keys", voice: "triangle", tone: 6000, reverbSend: 0.25 },
  { name: "glass pluck", voice: "pluck", tone: 12000, reverbSend: 0.3 },
  { name: "buzzy bass", voice: "saw", tone: 1200, reverbSend: 0 },
  { name: "round bass", voice: "sine", tone: 2000, reverbSend: 0 },
  { name: "lead with bite", voice: "square", tone: 9000, reverbSend: 0.15 },
];

/** The preset a track currently sounds like, if any. */
export function soundPresetOf(track: { voice: string; tone: number; reverbSend: number }): SoundPreset | undefined {
  return SOUND_PRESETS.find((p) => p.voice === track.voice && Math.abs(p.tone - track.tone) < 1 && Math.abs(p.reverbSend - track.reverbSend) < 1e-6);
}

/** The handoff's 12-color track palette (last one is reserved for returns/master). */
export const TRACK_PALETTE = [
  "#ff3b30",
  "#ff6a2a",
  "#ff9d2a",
  "#ffd02a",
  "#e2ec3e",
  "#b8f53a",
  "#3dffb0",
  "#3dc8ff",
  "#5a8cff",
  "#9a7bff",
  "#ff4fd8",
  "#8a8a94",
];

export function trackColor(track: Track, index: number): string {
  return track.color ?? TRACK_PALETTE[index % 11];
}

/** Live tier colors. Mutated in place by the appearance settings so every canvas reads the current set. */
export const TIER_COLORS: Record<Tier, string> = { ...DEFAULT_TIER_COLORS };

/** `#rrggbb` → [r, g, b] */
export function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** Blend `hex` toward `toward` (another hex) by `t` in 0…1. */
export function mixHex(hex: string, toward: string, t: number): string {
  const a = hexToRgb(hex);
  const b = hexToRgb(toward);
  const c = a.map((v, i) => Math.round(v + (b[i] - v) * t));
  return `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

/** Drum kit rows, top to bottom. Mirrors `dissonant_engine::drums::KIT`. */
export const DRUM_KIT: { name: string; pitch: number }[] = [
  { name: "hat", pitch: 42 },
  { name: "clap", pitch: 39 },
  { name: "snare", pitch: 38 },
  { name: "kick", pitch: 36 },
];

export function uuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) return crypto.randomUUID();
  // Fallback for very old webviews.
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}
