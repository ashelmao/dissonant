// Mirrors the Rust model in crates/dissonant-core (serde camelCase). Keep in sync.

import type { DrumPad, Sound } from "./sound";

/** Ticks per quarter-note beat (PPQ). Mirrors `dissonant_core::ticks::TICKS_PER_BEAT`. */
export const TICKS_PER_BEAT = 960;

/** Beats → ticks, rounded half away from zero. Non-finite input gives 0. */
export function beatsToTicks(beats: number): number {
  if (!Number.isFinite(beats)) return 0;
  const t = beats * TICKS_PER_BEAT;
  return t < 0 ? -Math.round(-t) : Math.round(t);
}

/** Ticks → beats as a real number. */
export function ticksToBeats(ticks: number): number {
  return ticks / TICKS_PER_BEAT;
}

export type ScaleType = "major" | "minor";

export interface KeyState {
  rootPitchClass: number | null;
  scale: ScaleType;
  isLocked: boolean;
}

export const NO_KEY: KeyState = {
  rootPitchClass: null,
  scale: "major",
  isLocked: false,
};

export interface ChordEvent {
  id: string;
  startTick: number;
  lengthTicks: number;
  pitchClasses: number[];
  name: string | null;
}

/**
 * One note. The properties after `velocity` are per-note performance controls modelled on
 * FL Studio's note record; a drum step is the same record, which is why one graph editor
 * drives both editors. All are optional on the wire: Rust fills the neutral value and skips
 * it again when serialising, so untouched notes stay byte-identical in saved projects.
 */
export interface NoteEvent {
  id: string;
  startTick: number;
  lengthTicks: number;
  pitch: number;
  velocity: number;
  /** The player marked this dissonance as deliberate. */
  intentional?: boolean;
  /** Kept in the pattern but not played (drawn hollow). */
  muted?: boolean;
  /** 0 hard left · 0.5 centre · 1 hard right. */
  pan?: number;
  /** Scales the voice's release: 0.5 is the instrument's own tail. */
  release?: number;
  /** Detune in cents, ±1200. Doubles as the drum grid's step pitch. */
  fine?: number;
  /** Filter cutoff for this note: 0.5 is the track's tone. */
  modX?: number;
  /** Filter resonance for this note: 0.5 is flat. */
  modY?: number;
  /** Start offset in ticks; the grid position stays the note's identity. */
  shiftTicks?: number;
  /** Retriggers across the note's own length; 0 plays it once. */
  repeats?: number;
  /** Colour group 0–15; the MIDI channel on export. */
  color?: number;
  /** Id of the note on the same pattern this one slides from (a rope); see `ropes.ts`. */
  slideFrom?: string;
  /** The rope's slide time in seconds, 0–2; absent uses the patch's glide. */
  slideSecs?: number;
  /**
   * The note's pitch contour, as an offset *on top of* `fine`: ticks are measured from the
   * note's start, at most {@link MAX_PITCH_POINTS} of them. Absent on the overwhelming
   * majority of notes, which sound at `pitch + fine` throughout.
   */
  pitchEnvelope?: PitchPoint[];
}

/** Most points one note's pitch envelope may carry. Mirrors `model::MAX_PITCH_POINTS`. */
export const MAX_PITCH_POINTS = 16;

/** One point on a note's pitch envelope. Mirrors `dissonant_core::model::PitchPoint`. */
export interface PitchPoint {
  /** Ticks after the note's start. */
  tick: number;
  /** Offset from the note's pitch and its `fine`, in cents. */
  cents: number;
  curve: Curve;
}

/** The per-note properties the roll's lane and the drum graph editor can draw. */
export type NoteProp = "velocity" | "pan" | "fine" | "release" | "modX" | "modY" | "shiftTicks" | "repeats";

export interface NotePropSpec {
  /** Lane label, matching FL's naming. */
  label: string;
  min: number;
  max: number;
  /** Value a note has when the property was never touched. */
  neutral: number;
  /** Whether the lane draws from its centre (pan) or from the floor (velocity). */
  bipolar: boolean;
  format: (v: number) => string;
}

export const NOTE_PROPS: Record<NoteProp, NotePropSpec> = {
  velocity: { label: "velocity", min: 1, max: 127, neutral: 100, bipolar: false, format: (v) => String(Math.round(v)) },
  pan: { label: "panning", min: 0, max: 1, neutral: 0.5, bipolar: true, format: panText },
  fine: { label: "fine pitch", min: -1200, max: 1200, neutral: 0, bipolar: true, format: (v) => `${Math.round(v)} ct` },
  release: { label: "release", min: 0, max: 1, neutral: 0.5, bipolar: true, format: (v) => v.toFixed(2) },
  modX: { label: "mod X", min: 0, max: 1, neutral: 0.5, bipolar: true, format: (v) => v.toFixed(2) },
  modY: { label: "mod Y", min: 0, max: 1, neutral: 0.5, bipolar: true, format: (v) => v.toFixed(2) },
  shiftTicks: { label: "shift", min: -TICKS_PER_BEAT / 2, max: TICKS_PER_BEAT / 2, neutral: 0, bipolar: true, format: (v) => `${v >= 0 ? "+" : ""}${Math.round(v)} t` },
  repeats: { label: "repeats", min: 0, max: 14, neutral: 0, bipolar: false, format: (v) => (v < 1 ? "none" : `×${Math.round(v) + 1}`) },
};

function panText(v: number): string {
  const off = Math.round((v - 0.5) * 200);
  if (off === 0) return "centre";
  return `${Math.abs(off)}% ${off < 0 ? "L" : "R"}`;
}

/** The value a note carries for `prop`, falling back to the neutral default. */
export function noteProp(note: NoteEvent, prop: NoteProp): number {
  const v = note[prop];
  return v === undefined ? NOTE_PROPS[prop].neutral : v;
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

/** Ticks per bar. Mirrors `dissonant_core::ticks::bar_ticks`. */
export function barTicks(ts: TimeSignature | undefined): number {
  return beatsToTicks(beatsPerBar(ts));
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
  /** An audio track holds recorded or imported audio and a guitar rig instead of notes. */
  isAudio: boolean;
  volume: number;
  reverbSend: number;
  tone: number;
  pan: number;
  /** `#rrggbb`, or null to use the palette by position. */
  color: string | null;
  /** The CLAP plugin that replaces the built-in voice, or null for the built-in voice. */
  plugin: TrackPlugin | null;
  /** This track's effect chain, in order, applied pre-fader. Absent when empty, like Rust's
   *  `#[serde(skip_serializing_if = "Vec::is_empty")]`. */
  inserts?: Insert[];
  /** The guitar rig, on audio tracks only. */
  amp: AmpRig | null;
  /** A drum track's kit, top to bottom (1–16 pads, unique pitches). Ignored on synth tracks. */
  drumPads: DrumPad[];
}

export type TrackKind = "midi" | "drum" | "audio";

/** Audio wins over drum; the model guarantees they are never both set. */
export function trackKind(t: Track): TrackKind {
  return t.isAudio ? "audio" : t.isDrum ? "drum" : "midi";
}

/**
 * An audio file the project uses. The samples live in the app's audio store; the project only
 * carries this description.
 */
export interface AudioSource {
  id: string;
  /** The imported file's name, or "take 3". */
  name: string;
  frames: number;
  sampleRate: number;
  channels: number;
}

/**
 * A piece of an {@link AudioSource} on an audio track's timeline. Audio never warps: it is
 * anchored at a tick and then plays for a length in seconds.
 */
export interface AudioClip {
  id: string;
  trackId: string;
  sourceId: string;
  startTick: number;
  lengthSeconds: number;
  offsetSeconds: number;
  /** Linear, 0…{@link MAX_CLIP_GAIN}. */
  gain: number;
  fadeInSeconds: number;
  fadeOutSeconds: number;
  muted: boolean;
}

export const MAX_CLIP_GAIN = 4;
export const MAX_FADE_SECONDS = 5;
/**
 * The crossfade a punch lays over each of its two seams, in seconds. Mirrors
 * `dissonant_core::DEFAULT_PUNCH_CROSSFADE_SECONDS`.
 */
export const DEFAULT_PUNCH_CROSSFADE_SECONDS = 0.015;
/** Most pedals a rig will hold. Mirrors `dissonant_core::MAX_PEDALS`. */
export const MAX_PEDALS = 4;

/** The guitar rig: input strip → pedals → amp head → cabinet → rack. */
export interface AmpRig {
  enabled: boolean;
  /** Input trim, -24…+24 dB. */
  inputDb: number;
  gate: Gate;
  /** In signal order, at most {@link MAX_PEDALS}. */
  pedals: Pedal[];
  amp: AmpHead;
  /** The tone stack, picked apart from the amp: the same voicing through another topology is a
   *  different guitar sound, which is the whole point of splitting them. */
  eq: EqModel;
  cab: Cabinet;
  rack: Rack;
}

export interface Gate {
  on: boolean;
  /** -80…0 dBFS. */
  thresholdDb: number;
  /** 5…500 ms. */
  releaseMs: number;
}

export type PedalKind =
  "comp" | "drive" | "wah" | "chorus" | "phaser" | "tremolo" | "delay";

export const PEDAL_KINDS: PedalKind[] = [
  "comp",
  "drive",
  "wah",
  "chorus",
  "phaser",
  "tremolo",
  "delay",
];

/** One stompbox. Knobs are 0…10 like the real thing; times and rates are in their own units. */
export type Pedal =
  | { kind: "comp"; on: boolean; sustain: number; level: number }
  | { kind: "drive"; on: boolean; drive: number; tone: number; level: number }
  | { kind: "wah"; on: boolean; sensitivity: number; range: number }
  | { kind: "chorus"; on: boolean; rateHz: number; depth: number; mix: number }
  | {
      kind: "phaser";
      on: boolean;
      rateHz: number;
      depth: number;
      feedback: number;
    }
  | { kind: "tremolo"; on: boolean; rateHz: number; depth: number }
  | {
      kind: "delay";
      on: boolean;
      timeMs: number;
      feedback: number;
      mix: number;
    };

export type AmpModel = "clean" | "crunch" | "lead" | "fuzz";
export const AMP_MODELS: AmpModel[] = ["clean", "crunch", "lead", "fuzz"];

/** The amp head: which voicing, the knob row across its face (all 0…10), and the circuit
 *  switches on the models that have them. */
export interface AmpHead {
  model: AmpModel;
  gain: number;
  bass: number;
  mid: number;
  treble: number;
  presence: number;
  master: number;
  /** Treble bypass across the gain pot. Only real on the models that offer it; the core turns
   *  it off again on the ones that do not. */
  brightCap: boolean;
  /** Half the output tubes: less headroom, and a supply that gives up sooner. */
  halfPower: boolean;
  /** Drive the tone stack from a cathode follower rather than straight off a plate. */
  cathodeFollower: boolean;
}

/** The circuit switches an amp head can carry. */
export type AmpSwitchKey = "brightCap" | "halfPower" | "cathodeFollower";

/** Which models' circuits actually have each switch. Mirrors `AmpModel::has_bright_cap` and its
 *  neighbours in `model.rs`: "more models" means switches rather than more entries in the list,
 *  but a switch bolted onto every amp is only a knob with two positions. The core turns off the
 *  ones a model never had, so this table and that one have to agree. */
export const AMP_SWITCH_MODELS: Record<AmpSwitchKey, AmpModel[]> = {
  brightCap: ["clean", "crunch"],
  halfPower: ["clean"],
  cathodeFollower: ["crunch", "lead"],
};

/** The tone-stack topology, which is what actually separates one classic amp from another once
 *  the gain stage is equal. Named for where the circuit comes from, not for a brand. */
export type EqModel = "usClassic" | "british" | "britishBright" | "boutique";
export const EQ_MODELS: EqModel[] = [
  "usClassic",
  "british",
  "britishBright",
  "boutique",
];

/** What each tone stack is called on screen. */
export const EQ_LABELS: Record<EqModel, string> = {
  usClassic: "us classic",
  british: "british",
  britishBright: "british bright",
  boutique: "boutique",
};

export type CabModel =
  "fourByTwelve" | "twoByTwelve" | "oneByTwelve" | "direct";
export const CAB_MODELS: CabModel[] = [
  "fourByTwelve",
  "twoByTwelve",
  "oneByTwelve",
  "direct",
];

/** What each cabinet is called on screen. */
export const CAB_LABELS: Record<CabModel, string> = {
  fourByTwelve: "4×12",
  twoByTwelve: "2×12",
  oneByTwelve: "1×12",
  direct: "direct",
};

/** The microphone in front of the cabinet. Each type hears the same speaker differently. */
export type MicModel = "dynamic" | "condenser" | "ribbon";
export const MIC_MODELS: MicModel[] = ["dynamic", "condenser", "ribbon"];

/** What each microphone is called on screen. */
export const MIC_LABELS: Record<MicModel, string> = {
  dynamic: "dynamic",
  condenser: "condenser",
  ribbon: "ribbon",
};

export interface Cabinet {
  model: CabModel;
  micModel: MicModel;
  /** Across the cone, 0…10: 0 = cone edge, dark and mellow; 10 = dust cap, bright and aggressive. */
  posX: number;
  /** Away from the cab, 0…10: 0 = on the grille, proximity bass and tight; 10 = backed off and roomier. */
  posZ: number;
}

export interface Rack {
  /** Reverb mix, 0…10. */
  reverb: number;
  /** Reverb decay, 0…10. */
  decay: number;
  /** -24…+12 dB. */
  outputDb: number;
}

/** A fresh pedal of `kind`, switched on, every knob at noon. Mirrors `Pedal::of`. */
export function newPedal(kind: PedalKind): Pedal {
  switch (kind) {
    case "comp":
      return { kind, on: true, sustain: 5, level: 5 };
    case "drive":
      return { kind, on: true, drive: 5, tone: 5, level: 5 };
    case "wah":
      return { kind, on: true, sensitivity: 5, range: 5 };
    case "chorus":
      return { kind, on: true, rateHz: 1.2, depth: 5, mix: 5 };
    case "phaser":
      return { kind, on: true, rateHz: 0.8, depth: 5, feedback: 4 };
    case "tremolo":
      return { kind, on: true, rateHz: 5, depth: 5 };
    case "delay":
      return { kind, on: true, timeMs: 350, feedback: 3, mix: 3 };
  }
}

/** The rig an audio track is created with. Mirrors `AmpRig::default`. */
export function defaultAmpRig(): AmpRig {
  return {
    enabled: true,
    inputDb: 0,
    gate: { on: true, thresholdDb: -55, releaseMs: 120 },
    pedals: [{ kind: "drive", on: false, drive: 5, tone: 5, level: 5 }],
    amp: {
      model: "crunch",
      gain: 5,
      bass: 5,
      mid: 5,
      treble: 6,
      presence: 5,
      master: 5,
      brightCap: false,
      halfPower: false,
      cathodeFollower: false,
    },
    eq: "usClassic",
    cab: { model: "fourByTwelve", micModel: "dynamic", posX: 5, posZ: 2 },
    rack: { reverb: 2, decay: 5, outputDb: 0 },
  };
}

/** A pattern belongs to one track; its clips play on that track's lane. */
export interface SongPattern {
  id: string;
  trackId: string;
  name: string;
  lengthTicks: number;
  notes: NoteEvent[];
}

/** Shortest note the model keeps. Mirrors `dissonant_core::model::MIN_NOTE_TICKS`. */
export const MIN_NOTE_TICKS = TICKS_PER_BEAT / 16;

/** Largest start offset a note may carry. Mirrors `dissonant_core::model::MAX_SHIFT_TICKS`. */
export const MAX_SHIFT_TICKS = 4 * TICKS_PER_BEAT;

/**
 * Lay `events` out across `totalTicks` in turns of `turnTicks`, trimming each turn at its own
 * end. Mirrors how the engine loops a pattern inside a longer clip; repeats carry a suffixed
 * id so React keys stay unique and hit-testing can tell them from the source.
 */
export function layOutLoop<
  T extends { id: string; startTick: number; lengthTicks: number },
>(events: T[], turnTicks: number, totalTicks: number): T[] {
  if (turnTicks >= totalTicks || events.length === 0) return events;
  const out: T[] = [];
  for (let rep = 0, base = 0; base < totalTicks; rep++, base += turnTicks) {
    const end = Math.min(base + turnTicks, totalTicks);
    for (const e of events) {
      const startTick = base + e.startTick;
      if (startTick >= end) continue;
      out.push({
        ...e,
        id: rep === 0 ? e.id : `${e.id}~${rep}`,
        startTick,
        lengthTicks: Math.max(
          MIN_NOTE_TICKS,
          Math.min(startTick + e.lengthTicks, end) - startTick,
        ),
      });
    }
  }
  return out;
}

/** A pattern placed on the song; loops from `offsetTicks` for `lengthTicks`. Its track is
 *  its pattern's track. */
export interface Clip {
  id: string;
  patternId: string;
  startTick: number;
  lengthTicks: number;
  offsetTicks: number;
  muted: boolean;
}

/**
 * A song marker, optionally with its own key (modulation), and with an extent: see
 * {@link sectionSpan}. Without one a marker would only ever be a label hung in front of music
 * it does not own, and moving or copying "the chorus" could not move the chorus.
 */
export interface Section {
  id: string;
  name: string;
  startTick: number;
  /**
   * How far the section reaches. Absent (or 0) means "up to the next section, then the song's
   * end" — the implicit extent a marker dropped on the ruler still gets, and the only thing an
   * older project can mean. The backend skips it when implicit, so it really is absent.
   */
  lengthTicks?: number;
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
  /** Ceiling the limiter holds the output under, in dBFS. */
  ceilingDb: number;
  /** Off restores a hard clamp at ±1.0. */
  limiter: boolean;
}

export interface ProjectModel {
  /** 10 for everything this build reads or writes; older files are migrated on load. */
  schemaVersion: number;
  tempo: number;
  key: KeyState;
  tracks: Track[];
  patterns: SongPattern[];
  clips: Clip[];
  /** Every audio file the project refers to; kept even when nothing points at one. */
  audioSources?: AudioSource[];
  /** Audio placed on the song timeline, each in its own track's lane. */
  audioClips?: AudioClip[];
  sections: Section[];
  master: MasterSettings;
  timeSignature?: TimeSignature;
  /** 50 = straight … 75 = hard shuffle. */
  swing?: number;
  /** Swing grid in ticks: 480 (eighths) or 240 (sixteenths). */
  swingGridTicks?: number;
  /** Tempo changes along the song (absent / empty = constant). */
  tempoPoints?: TempoPoint[];
  /** The master effect chain, ahead of the master bus. Absent when empty. */
  masterInserts?: Insert[];
  /** Automated parameters, one lane each. Absent when empty, so an automation-free project
   *  is byte-identical to one saved before lanes existed. */
  automation?: AutomationLane[];
  /** The project-wide progression in song ticks: sorted, non-overlapping. */
  chords: ChordEvent[];
  /** The song's hard end in ticks: clips can't start at or run past it. At least one bar. */
  songLengthTicks: number;
  /** The song's loop brace in song ticks, `[start, end]`; absent / null = loop the whole song. */
  loopRegion?: [number, number] | null;
  /** The armed punch range in song ticks, `[in, out]`; absent / null = nothing armed. Looping
   *  is blocked while it is set. */
  punchRegion?: [number, number] | null;
  /** Copies of the non-factory sounds the tracks use. Absent when empty. */
  sounds?: Sound[];
  /** What DI analysis heard besides notes, one entry per analysed audio clip. Absent when
   *  empty. */
  detections?: ClipDetection[];
}

/**
 * A chord heard in a recorded DI, waiting in the detected lane. It joins the progression only
 * when accepted (`acceptDetectedChord`), one at a time.
 */
export interface DetectedChord {
  id: string;
  startTick: number;
  lengthTicks: number;
  /** Root first. */
  pitchClasses: number[];
  name: string | null;
  /** 0…1: how closely the heard chroma matched this chord. */
  confidence: number;
}

/** What one analysis of one audio clip found besides notes. */
export interface ClipDetection {
  clipId: string;
  chords: DetectedChord[];
  /** Per pitch class (0 = C), in note equivalents: sums with a histogram of placed notes for
   *  key detection. */
  chroma: number[];
}

/** Most chord candidates one analysis may leave in the detected lane. Mirrors
 *  `dissonant_core::MAX_DETECTED_CHORDS`. */
export const MAX_DETECTED_CHORDS = 512;

export interface TempoPoint {
  id: string;
  tick: number;
  bpm: number;
  /** Glide linearly from the previous point's tempo to this one. */
  ramp: boolean;
}

/** The grid a project swings on when it carries none. Mirrors `default_swing_grid`. */
export const DEFAULT_SWING_GRID_TICKS = TICKS_PER_BEAT / 2;

/** Mirror of `dissonant_core::tempo::TempoMap`: tempo and elapsed seconds along the song.
 *  Positions are ticks (fractional ticks allowed for a sub-tick playhead). */
export function tempoMap(baseBpm: number, points: TempoPoint[] | undefined) {
  const anchors: { tick: number; bpm: number; ramp: boolean }[] = [
    { tick: 0, bpm: baseBpm, ramp: false },
  ];
  for (const p of [...(points ?? [])].sort((a, b) => a.tick - b.tick)) {
    if (p.tick <= 0) anchors[0] = { tick: 0, bpm: p.bpm, ramp: false };
    else if (anchors[anchors.length - 1].tick !== p.tick)
      anchors.push({ tick: p.tick, bpm: p.bpm, ramp: p.ramp });
  }
  const segSeconds = (t0: number, bpm0: number, t1: number, bpm1: number) => {
    const len = (t1 - t0) / TICKS_PER_BEAT;
    if (len <= 0) return 0;
    return Math.abs(bpm1 - bpm0) < 1e-9
      ? (60 * len) / bpm0
      : ((60 * len) / (bpm1 - bpm0)) * Math.log(bpm1 / bpm0);
  };
  const starts: number[] = [0];
  for (let i = 1; i < anchors.length; i++) {
    const a = anchors[i - 1];
    const n = anchors[i];
    starts.push(
      starts[i - 1] + segSeconds(a.tick, a.bpm, n.tick, n.ramp ? n.bpm : a.bpm),
    );
  }
  const segAt = (tick: number) => {
    let i = 0;
    while (i + 1 < anchors.length && anchors[i + 1].tick <= tick) i++;
    return i;
  };
  const bpmAt = (tick: number) => {
    const i = segAt(tick);
    const a = anchors[i];
    const n = anchors[i + 1];
    if (!n || !n.ramp) return a.bpm;
    const t = Math.min(1, Math.max(0, (tick - a.tick) / (n.tick - a.tick)));
    return a.bpm + (n.bpm - a.bpm) * t;
  };
  const secondsAt = (tick: number) => {
    const i = segAt(Math.max(0, tick));
    const a = anchors[i];
    return (
      starts[i] + segSeconds(a.tick, a.bpm, Math.max(0, tick), bpmAt(tick))
    );
  };
  return { bpmAt, secondsAt, isConstant: anchors.length === 1 };
}

/**
 * Swing as a piecewise-linear warp of the tick line (mirrors `dissonant_core::tempo::swing_warp_ticks`):
 * the second half of each `2·grid` pair is pushed so it lands at `2·grid·swing/100`.
 */
export function swingWarp(tick: number, swing: number, gridTicks: number): number {
  if (!(gridTicks > 0) || !(swing > 50) || swing > 75) return tick;
  const pair = 2 * gridTicks;
  const late = (pair * swing) / 100;
  const base = Math.floor(tick / pair) * pair;
  const u = tick - base;
  return (
    base +
    (u <= gridTicks
      ? u * (late / gridTicks)
      : late + (u - gridTicks) * ((pair - late) / gridTicks))
  );
}

/** The song's explicit length, in ticks. */
export function songLength(m: ProjectModel): number {
  return m.songLengthTicks;
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
    const s = Math.max(c.startTick, offset);
    const e = Math.min(c.startTick + c.lengthTicks, end);
    if (e > s) out.push({ ...c, startTick: s - offset, lengthTicks: e - s });
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
    const cEnd = c.startTick + c.lengthTicks;
    if (c.startTick < offset) out.push({ ...c, lengthTicks: Math.min(cEnd, offset) - c.startTick });
    if (cEnd > end) {
      const s = Math.max(c.startTick, end);
      // A chord spanning the whole window splits in two: give the right half its own id.
      out.push({ ...c, id: c.startTick < offset ? uuid() : c.id, startTick: s, lengthTicks: cEnd - s });
    }
  }
  for (const c of local) {
    const s = Math.max(0, c.startTick);
    const e = Math.min(length, c.startTick + c.lengthTicks);
    if (e > s) out.push({ ...c, startTick: offset + s, lengthTicks: e - s });
  }
  return out.filter((c) => c.lengthTicks > 0).sort((a, b) => a.startTick - b.startTick);
}

/**
 * Where a section stops, or null when nothing bounds it: with no explicit extent and no later
 * marker, the last section names the music to the end of time. Mirrors `section_end`.
 */
function sectionEnd(sections: Section[], section: Section): number | null {
  if (section.lengthTicks && section.lengthTicks > 0) return section.startTick + section.lengthTicks;
  let next: number | null = null;
  for (const s of sections)
    if (s.startTick > section.startTick && (next === null || s.startTick < next)) next = s.startTick;
  return next;
}

/**
 * A section's extent in song ticks, `[start, end)`: its own `lengthTicks`, else up to the next
 * marker, else the rest of the song. This is the music the section owns — what the arrangement
 * draws as its band, and what moving or copying it moves. Mirrors `ProjectModel::section_span`.
 */
export function sectionSpan(m: ProjectModel, section: Section): [number, number] {
  const end = sectionEnd(m.sections, section);
  return [section.startTick, Math.max(section.startTick, end ?? songLength(m))];
}

/** The section whose extent contains `tick` (the last one, where two overlap). */
export function sectionAt(m: ProjectModel, tick: number): Section | null {
  let best: Section | null = null;
  for (const s of m.sections) {
    const end = sectionEnd(m.sections, s);
    if (s.startTick <= tick && (end === null || tick < end) && (!best || s.startTick >= best.startTick)) best = s;
  }
  return best;
}

export function keyAt(m: ProjectModel, tick: number): KeyState {
  return sectionAt(m, tick)?.key ?? m.key;
}

export interface Snapshot {
  model: ProjectModel;
  /** Goes up with every change; sent back with an edit to say what it was built from. */
  revision?: number;
  /** Sources the project uses that aren't on this machine. */
  missingAudio?: string[];
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

export function fileNameOf(
  path: string | null | undefined,
  fallback = "untitled.dissonant",
): string {
  if (!path) return fallback;
  return path.split(/[\\/]/).pop() || fallback;
}

// ─── Plugins (mirrors crates/dissonant-plugins and crates/dissonant-catalog) ────────────────

/** The only format the app hosts. Kept as a union so a descriptor still round-trips as JSON. */
export type PluginFormat = "clap";
export type PluginKind = "instrument" | "effect";

/** One plugin found on this computer. `uid` is `clap:<bundle path>:<plugin id>`. */
export interface PluginDescriptor {
  uid: string;
  format: PluginFormat;
  path: string;
  pluginId: string;
  name: string;
  vendor: string;
  version: string;
  kind: PluginKind;
  features: string[];
}

/** One automatable parameter of a hosted plugin. `value` is the plugin's current value, read
 *  afresh on every `pluginParams` call, so a reopened project shows its restored state;
 *  `default` is what a double-click resets to. */
export interface ParamInfo {
  id: number;
  name: string;
  module: string;
  min: number;
  max: number;
  default: number;
  value: number;
  isStepped: boolean;
}

/** What a track remembers about its plugin. `name` is cached so a plugin that is no longer
 *  installed can still be named; `state` is base64 of the plugin's own opaque state blob. */
export interface TrackPlugin {
  uid: string;
  name: string;
  state: string | null;
}

/** Most inserts one chain will hold. Mirrors `dissonant_core::MAX_INSERTS`. */
export const MAX_INSERTS = 8;

/**
 * One effect slot in a chain, on a track or on the master. `uid` names a scanned plugin or —
 * with the `native:` prefix — one of the stock effects in `effects.ts`; `name` is cached so an
 * uninstalled effect can still be listed; `state` is a plugin's own base64 blob and `params` a
 * native effect's knobs, keyed by parameter id. Mirrors `dissonant_core::Insert`.
 */
export interface Insert {
  id: string;
  uid: string;
  name: string;
  enabled: boolean;
  state?: string | null;
  /** Absent ids are at their defaults, so a slot nobody has touched carries no map at all. */
  params?: Record<string, number>;
}

/** Which chain an insert command addresses: one track's, or the master's. */
export type InsertTarget = { track: string } | "master";

/** The chain a target names; empty when the track is gone or has never held an effect. */
export function insertChain(m: ProjectModel, target: InsertTarget): Insert[] {
  if (target === "master") return m.masterInserts ?? [];
  return m.tracks.find((t) => t.id === target.track)?.inserts ?? [];
}

export type Platform = "macArm" | "macIntel" | "windows" | "linux";
export type AssetKind = "zip" | "tarGz" | "installer";

/** One downloadable file of a catalog entry, pinned by hash so a tampered download is caught. */
export interface CatalogAsset {
  platform: Platform;
  format: PluginFormat;
  kind: AssetKind;
  url: string;
  sha256: string;
  bytes: number;
}

/** One curated free plugin. `summary` is one plain line; `tags` are what it sounds like. */
export interface CatalogEntry {
  id: string;
  name: string;
  vendor: string;
  summary: string;
  tags: string[];
  kind: PluginKind;
  license: string;
  homepage: string;
  assets: CatalogAsset[];
}

export type InstallPhase =
  "downloading" | "verifying" | "unpacking" | "installed" | "failed";

/** The `plugin-install` event: one entry's progress through the install. `done`/`total` are
 *  bytes while downloading and steps otherwise, so a bar can be drawn from them throughout. */
export interface InstallProgress {
  entryId: string;
  phase: InstallPhase;
  done: number;
  total: number;
  message: string;
}

/**
 * The platforms this computer can load a plugin built for. A webview cannot tell an Apple
 * Silicon Mac from an Intel one — both report "Intel Mac OS X" — so a Mac accepts either Mac
 * build and the installer, which knows the real target arch, picks between them.
 */
export function hostPlatforms(): Platform[] {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/Win/i.test(ua)) return ["windows"];
  if (/Mac/i.test(ua)) return ["macArm", "macIntel"];
  return ["linux"];
}

export type TrackParam = "volume" | "reverbSend" | "tone" | "pan";
/** What the engine plays: one pattern (looped) or the whole song. */
export type PlayMode = "pattern" | "song";
/** What the window shows. SETUP and SONG both play the song. */
/** `sound` is the sound designer; it plays like song mode. */
export type ViewMode = "setup" | "pattern" | "song" | "sound";
export type Tier = "chordTone" | "tension" | "dissonance";

/** Which parameter of the master bus a lane writes to. */
export type MasterParam = "gain";

/** What a lane writes to. Mirrors `dissonant_core::automation::AutoTarget`; `insert` and
 *  `trackPlugin` name a CLAP parameter by its stable id. */
export type AutoTarget =
  | { track: [string, TrackParam] }
  | { master: MasterParam }
  | { insert: [InsertTarget, string, number] }
  | { trackPlugin: [string, number] };

/** How a breakpoint is reached from the one before it. */
export type Curve = "step" | "ramp";

/** One point on a lane. */
export interface Breakpoint {
  id: string;
  tick: number;
  value: number;
  curve: Curve;
}

/** One automated parameter: sorted by tick, at most one point per tick. */
export interface AutomationLane {
  id: string;
  target: AutoTarget;
  points: Breakpoint[];
}

/** Mirror of `AutoTarget::clamp` / `track_param_range`. */
export function trackParamRange(param: TrackParam): [number, number] {
  switch (param) {
    case "volume":
      return [0, 1.5];
    case "reverbSend":
      return [0, 1];
    case "tone":
      return [200, 20_000];
    case "pan":
      return [-1, 1];
  }
}

/** `null` for plugin/insert targets, whose range only the plugin knows. */
export function autoTargetRange(target: AutoTarget): [number, number] | null {
  if ("track" in target) return trackParamRange(target.track[1]);
  if ("master" in target) return [0, 1.5];
  return null;
}

export function sameAutoTarget(a: AutoTarget, b: AutoTarget): boolean {
  if ("track" in a && "track" in b)
    return a.track[0] === b.track[0] && a.track[1] === b.track[1];
  if ("master" in a && "master" in b) return a.master === b.master;
  if ("insert" in a && "insert" in b) {
    const [ca, ia, pa] = a.insert;
    const [cb, ib, pb] = b.insert;
    const sameChain =
      ca === "master" || cb === "master" ? ca === cb : ca.track === cb.track;
    return sameChain && ia === ib && pa === pb;
  }
  if ("trackPlugin" in a && "trackPlugin" in b)
    return a.trackPlugin[0] === b.trackPlugin[0] && a.trackPlugin[1] === b.trackPlugin[1];
  return false;
}

export function autoTrackId(target: AutoTarget): string | null {
  if ("track" in target) return target.track[0];
  if ("trackPlugin" in target) return target.trackPlugin[0];
  if ("insert" in target)
    return target.insert[0] === "master" ? null : target.insert[0].track;
  return null;
}

/** The lane rule: before the first point the first value holds; a `ramp` point glides
 *  linearly from the previous point's value into itself; a `step` point holds the previous
 *  value until its own tick then jumps; past the last point the last value holds.
 *  `null` when `points` is empty. */
export function automationValueAt(points: Breakpoint[], tick: number): number | null {
  const first = points[0];
  if (!first) return null;
  // Index of the first point strictly after `tick`.
  let i = 0;
  while (i < points.length && points[i].tick <= tick) i++;
  if (i === 0) return first.value;
  const prev = points[i - 1];
  const next = points[i];
  if (next && next.curve === "ramp" && next.tick > prev.tick) {
    const t = (tick - prev.tick) / (next.tick - prev.tick);
    return prev.value + (next.value - prev.value) * Math.min(1, Math.max(0, t));
  }
  return prev.value;
}

/** The lane for `target`, or undefined. */
export function automationLane(
  m: ProjectModel,
  target: AutoTarget,
): AutomationLane | undefined {
  return (m.automation ?? []).find((l) => sameAutoTarget(l.target, target));
}

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
  /** Synth tracks only; a non-factory sound is embedded in the project. */
  | { type: "setTrackSound"; id: string; sound: Sound }
  /** Drum tracks only: the whole kit, plus the non-factory drum sounds its pads use. */
  | { type: "setDrumPads"; id: string; pads: DrumPad[]; sounds: Sound[] }
  /** Replace the project's embedded copy of a sound. */
  | { type: "upsertSound"; sound: Sound }
  | { type: "setTrackParam"; id: string; param: TrackParam; value: number }
  | { type: "setTrackColor"; id: string; color: string | null }
  | { type: "setTrackPlugin"; id: string; plugin: TrackPlugin | null }
  | { type: "setPluginState"; id: string; state: string }
  | {
      type: "addInsert";
      target: InsertTarget;
      uid: string;
      name: string;
      /** Where in the chain, or null to append. */
      index: number | null;
    }
  | { type: "removeInsert"; target: InsertTarget; insertId: string }
  | { type: "moveInsert"; target: InsertTarget; insertId: string; toIndex: number }
  | { type: "setInsertEnabled"; target: InsertTarget; insertId: string; on: boolean }
  | { type: "setInsertState"; target: InsertTarget; insertId: string; state: string }
  | {
      type: "setInsertParam";
      target: InsertTarget;
      insertId: string;
      paramId: number;
      value: number;
    }
  | { type: "addPattern"; trackId: string }
  | { type: "duplicatePattern"; id: string }
  | { type: "deletePattern"; id: string }
  | { type: "renamePattern"; id: string; name: string }
  | { type: "setPatternLength"; id: string; ticks: number }
  | { type: "setNotes"; patternId: string; notes: NoteEvent[] }
  /** Rope `noteId` to `sourceId` (same pattern), replacing any rope it had. Refused when the
   *  rope would loop back on itself. */
  | { type: "setRope"; patternId: string; noteId: string; sourceId: string }
  /** Cut `noteId`'s rope and its slide time; a note with no rope is left alone. */
  | { type: "clearRope"; patternId: string; noteId: string }
  /** The rope's slide time in seconds (clamped 0–2), or null for the patch's glide. Send it
   *  transient while the midpoint is dragged. Refused on a note with no rope. */
  | { type: "setSlideTime"; patternId: string; noteId: string; secs: number | null }
  | { type: "setChords"; chords: ChordEvent[] }
  | {
      type: "addClip";
      patternId: string;
      startTick: number;
      lengthTicks: number | null;
    }
  | { type: "updateClip"; clip: Clip }
  | { type: "updateClips"; clips: Clip[] }
  | { type: "removeClip"; id: string }
  | { type: "removeClips"; ids: string[] }
  | { type: "makeClipUnique"; id: string }
  | { type: "setSongLength"; ticks: number }
  | { type: "setLoopRegion"; region: [number, number] | null }
  /** Arm the punch range, or `null` to disarm it. Looping is blocked while it is set. */
  | { type: "setPunchRegion"; region: [number, number] | null }
  | { type: "addSection"; name: string; startTick: number }
  | { type: "updateSection"; section: Section }
  | { type: "removeSection"; id: string }
  | { type: "setMaster"; master: MasterSettings }
  | { type: "setTimeSignature"; numerator: number; denominator: number }
  | { type: "setSwing"; swing: number; gridTicks: number }
  | { type: "addTempoPoint"; tick: number; bpm: number; ramp: boolean }
  | { type: "updateTempoPoint"; point: TempoPoint }
  | { type: "removeTempoPoint"; id: string }
  /** Draw or redraw the point on `tick`: one drag is one undo step like a knob's. */
  | { type: "addAutomationPoint"; target: AutoTarget; tick: number; value: number; curve: Curve }
  /** Move / retune / re-curve a point (matched by id inside `lane`). */
  | { type: "updateAutomationPoint"; lane: string; point: Breakpoint }
  | { type: "removeAutomationPoint"; lane: string; id: string }
  /** Drop the whole lane for `target`: the parameter goes back to its static control. */
  | { type: "clearAutomation"; target: AutoTarget }
  | { type: "addAudioTrack"; name: string }
  | { type: "setTrackAmp"; id: string; amp: AmpRig }
  | {
      type: "addAudioTake";
      trackId: string;
      source: AudioSource;
      clip: AudioClip;
    }
  /**
   * A take that lands inside what was already there: the clips under it are split and
   * crossfaded against it — head fading out across `crossfadeSeconds` from the take's start,
   * tail fading in across the same at its end — all in one undo step.
   */
  | {
      type: "punchAudioTake";
      trackId: string;
      source: AudioSource;
      clip: AudioClip;
      crossfadeSeconds: number;
    }
  | { type: "updateAudioClip"; clip: AudioClip }
  | { type: "removeAudioClip"; id: string }
  /**
   * Open `ticks` of empty time at song tick `at`: every clip, take, marker, tempo point, chord
   * and automation breakpoint from `at` on moves that much later, in one undo step, and the
   * song grows to match. A clip the cut falls inside is split, never stretched.
   */
  | { type: "insertTime"; at: number; ticks: number }
  /** Cut `[at, at + ticks)` out of the song and close the gap, in one undo step. */
  | { type: "deleteTime"; at: number; ticks: number }
  /** Copy everything in `[from, to)` to `at`, pushing what was there along. */
  | { type: "duplicateRange"; from: number; to: number; at: number }
  /**
   * What DI analysis heard in one audio clip, as one undo step: a new synth track `name` with
   * one `lengthTicks` pattern of `notes` placed at `startTick` (no track when `notes` is empty),
   * and `detection` replacing whatever an earlier analysis of the same clip left.
   */
  | {
      type: "addTranscription";
      name: string;
      startTick: number;
      lengthTicks: number;
      notes: NoteEvent[];
      detection: ClipDetection;
    }
  /** Lay one detected chord onto the progression over its own span; it leaves the lane. */
  | { type: "acceptDetectedChord"; id: string }
  /** Drop one detected chord without touching the progression. */
  | { type: "dismissDetectedChord"; id: string };

/** `detect_audio_clip`'s answer: the new state and what the analysis did. */
export interface DetectionReport {
  snapshot: Snapshot;
  /** The new note track, when any notes were written. */
  trackId: string | null;
  notes: number;
  chords: number;
  /** Regions refused as polyphonic, `[start, end]` in song ticks: no notes were written there. */
  refused: [number, number][];
}

export interface PlayheadEvent {
  tick: number;
  playing: boolean;
  masterPeak: [number, number];
  trackPeaks: number[];
  /** Beats of count-in left; 0 when not counting in. */
  countIn?: number;
  /** Peak of the live input; 0 when nothing is captured. */
  inputPeak?: number;
  /** True while a take is actually being captured. */
  recording?: boolean;
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
  /** The capture device in use, if one opened. */
  inputDevice?: string | null;
  /** Why the capture device did not open, if it did not. */
  inputError?: string | null;
  inputLatencyMs?: number;
  /** What a guitarist actually feels: in plus out. */
  roundTripMs?: number;
  inputUnderruns?: number;
  /** Latched until cleared: the mix asked the master for more than its ceiling. */
  masterOver?: boolean;
  /** The input meter's peak hold and clip latch, both since the last `clearInputPeak`.
   *  Pre-rig: this is what the interface is sending, whatever the rig does with it. */
  inputPeakHold?: number;
  inputOver?: boolean;
  /** The manual latency trim in force for the open input device, in ms. */
  inputTrimMs?: number;
  /** What the next take will be pulled back by: block + backlog + trim. */
  alignmentMs?: number;
  /** What the last take of this session was actually corrected by. */
  lastTakeAlign?: TakeAlign | null;
}

/** `dissonant_app::TakeAlign`: how far the last take moved, and how much of its head was cut. */
export interface TakeAlign {
  shiftedMs: number;
  trimmedMs: number;
}

/** `dissonant_app::TunerReading`: what the tuner hears, straight off the input. */
export interface TunerReading {
  hz: number;
  /** 0…1. A chord or a muted string reads low. */
  clarity: number;
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
  /** Plugins the render could not open. The files were written without them. */
  missingPlugins: string[];
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

export interface InputDevice {
  name: string;
  isDefault: boolean;
  channels: number;
  defaultSampleRate: number;
}

/** What the rec page's progress strip reads while a file imports. */
export interface AudioImportProgress {
  name: string;
  fraction: number;
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
  audio: {
    device: string | null;
    sampleRate: number | null;
    bufferSize: number | null;
    /** Capture device name; null follows the system default. */
    inputDevice: string | null;
    /** Which of the input device's channels the guitar is plugged into. */
    inputChannel: number;
    /** Manual latency trim per input device, in ms, keyed by device name. A device nobody
     *  has trimmed is simply absent. */
    inputTrims: Record<string, number>;
  };
  midi: {
    defaultInput: string | null;
    autoReconnect: boolean;
    velocityCurve: VelocityCurve;
    channel: number | null;
    octaveOffset: number;
  };
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
  export: {
    sampleRate: number;
    bitDepth: 16 | 24 | 32;
    dither: boolean;
    normalize: boolean;
    normalizeDb: number;
    tailSeconds: number;
  };
  appearance: {
    uiScale: number;
    rowHeight: number;
    reducedMotion: boolean;
    tierColors: TierColors;
    accent: string;
  };
  metronome: {
    on: boolean;
    volume: number;
    countInBars: number;
    preRollBars: number;
    duringPlayback: boolean;
  };
  tuner: {
    /** What A4 is worth, in Hz. */
    a4Hz: number;
    /** Which named tuning the readout names its strings from; see `tuner.ts`. */
    tuning: string;
  };
}

export const DEFAULT_TIER_COLORS: TierColors = {
  chordTone: "#3dffb0",
  tension: "#ff9d2a",
  dissonance: "#ff3b30",
};

/** Tier palettes: the default, and an Okabe–Ito set that survives red-green and blue-yellow CVD. */
export const TIER_PRESETS: { name: string; colors: TierColors }[] = [
  { name: "default", colors: DEFAULT_TIER_COLORS },
  {
    name: "okabe–ito (color-blind safe)",
    colors: { chordTone: "#56b4e9", tension: "#f0e442", dissonance: "#d55e00" },
  },
  {
    name: "cool",
    colors: { chordTone: "#4dd2ff", tension: "#c9b3ff", dissonance: "#ff5f9e" },
  },
];

export const DEFAULT_SETTINGS: Settings = {
  audio: {
    device: null,
    sampleRate: null,
    bufferSize: null,
    inputDevice: null,
    inputChannel: 0,
    inputTrims: {},
  },
  midi: {
    defaultInput: null,
    autoReconnect: true,
    velocityCurve: "linear",
    channel: null,
    octaveOffset: 0,
  },
  editing: {
    defaultGrid: 240,
    defaultNoteLength: 240,
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
  export: {
    sampleRate: 44100,
    bitDepth: 16,
    dither: true,
    normalize: false,
    normalizeDb: -1,
    tailSeconds: 1.5,
  },
  appearance: {
    uiScale: 1,
    rowHeight: 18,
    reducedMotion: false,
    tierColors: DEFAULT_TIER_COLORS,
    accent: "#b48cff",
  },
  metronome: {
    on: false,
    volume: 0.6,
    countInBars: 1,
    preRollBars: 0,
    duringPlayback: true,
  },
  tuner: {
    a4Hz: 440,
    tuning: "standard",
  },
};

/** Deep-merge a (possibly partial / older) settings object over the defaults. */
export function withDefaults(
  s: Partial<Settings> | null | undefined,
): Settings {
  const d = DEFAULT_SETTINGS;
  return {
    audio: {
      ...d.audio,
      ...(s?.audio ?? {}),
      inputTrims: { ...(s?.audio?.inputTrims ?? {}) },
    },
    midi: { ...d.midi, ...(s?.midi ?? {}) },
    editing: {
      ...d.editing,
      ...(s?.editing ?? {}),
      defaultTimeSignature: {
        ...d.editing.defaultTimeSignature,
        ...(s?.editing?.defaultTimeSignature ?? {}),
      },
    },
    export: { ...d.export, ...(s?.export ?? {}) },
    appearance: {
      ...d.appearance,
      ...(s?.appearance ?? {}),
      tierColors: {
        ...d.appearance.tierColors,
        ...(s?.appearance?.tierColors ?? {}),
      },
    },
    metronome: { ...d.metronome, ...(s?.metronome ?? {}) },
    tuner: { ...d.tuner, ...(s?.tuner ?? {}) },
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

/** The built-in voices offered as chips beside the plugin list. Each is a factory sound id. */
export const VOICES = [
  "saw",
  "square",
  "triangle",
  "sine",
  "pad",
  "pluck",
] as const;

/** A sound named by character: a voice (factory synth sound id) plus tone (low-pass Hz) and reverb send. */
export interface SoundPreset {
  name: string;
  voice: string;
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

export function uuid(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto)
    return crypto.randomUUID();
  // Fallback for very old webviews.
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
  });
}
