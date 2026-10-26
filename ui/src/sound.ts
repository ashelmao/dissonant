// Sounds: synth and drum patches. Mirrors `dissonant_core::sound` — keep the defaults below
// identical to the Rust `Default` impls, because factory patches in the shared JSON may be
// partial and both sides fill the gaps the same way.

import factoryJson from "../../crates/dissonant-core/assets/factory_sounds.json";

export type OscWave = "sine" | "triangle" | "saw" | "square" | "pulse";
export type FilterMode = "off" | "lowpass12" | "lowpass24" | "highpass12" | "bandpass" | "notch";
export type LfoWave = "sine" | "triangle" | "saw" | "square" | "sampleHold";
/** Depth 1 means: pitch ±12 st, cutoff ±4 octaves, amp full tremolo, pulse width ±0.45, fm ±1. */
export type LfoTarget = "none" | "pitch" | "cutoff" | "amp" | "pulseWidth" | "fm";

export const OSC_WAVES: OscWave[] = ["sine", "triangle", "saw", "square", "pulse"];
export const FILTER_MODES: FilterMode[] = ["off", "lowpass12", "lowpass24", "highpass12", "bandpass", "notch"];
export const LFO_WAVES: LfoWave[] = ["sine", "triangle", "saw", "square", "sampleHold"];
export const LFO_TARGETS: LfoTarget[] = ["none", "pitch", "cutoff", "amp", "pulseWidth", "fm"];

/** Attack / decay / release in seconds, sustain 0–1. */
export interface Envelope {
  attack: number;
  decay: number;
  sustain: number;
  release: number;
}

export interface OscParams {
  on: boolean;
  wave: OscWave;
  /** -3…3 */
  octave: number;
  /** -12…12 */
  semi: number;
  /** cents, -100…100 */
  fine: number;
  level: number;
  /** 0.05–0.95 */
  pulseWidth: number;
  /** 1–8 */
  unison: number;
  /** total spread in cents, 0–100 */
  detune: number;
  randomPhase: boolean;
}

export interface FilterParams {
  mode: FilterMode;
  /** Hz, 20–20000 */
  cutoff: number;
  resonance: number;
  drive: number;
  keyTrack: number;
  /** octaves, -8…8 */
  envAmount: number;
  velocity: number;
}

export interface LfoParams {
  wave: LfoWave;
  /** Hz, 0.01–30 */
  rate: number;
  depth: number;
  target: LfoTarget;
  retrigger: boolean;
}

export interface SynthPatch {
  osc: [OscParams, OscParams, OscParams];
  noiseLevel: number;
  /** 0 white … 1 dark */
  noiseColor: number;
  /** osc 2 → osc 1 phase modulation, 0–1 */
  fmAmount: number;
  ringMod: number;
  /** osc 2 hard-synced to osc 1 */
  sync: boolean;
  filter: FilterParams;
  ampEnv: Envelope;
  filterEnv: Envelope;
  /** semitones, -48…48 */
  pitchEnvAmount: number;
  /** s, 0.001–2 */
  pitchEnvDecay: number;
  lfo: [LfoParams, LfoParams];
  mono: boolean;
  /** s, 0–2 */
  glide: number;
  velocity: number;
  drive: number;
  crush: number;
  /** rate Hz 0.05–5 */
  chorus: { mix: number; rate: number; depth: number };
  /** time s 0.01–1.5, feedback 0–0.95 */
  delay: { mix: number; time: number; feedback: number };
  level: number;
}

export interface DrumBody {
  level: number;
  wave: OscWave;
  /** Hz, 20–4000 */
  startHz: number;
  endHz: number;
  /** s */
  pitchDecay: number;
  /** s */
  decay: number;
  overtoneLevel: number;
  /** 0.5–8 */
  overtoneRatio: number;
}

export interface DrumNoise {
  level: number;
  mode: FilterMode;
  cutoff: number;
  resonance: number;
  /** s, 0–0.1 */
  attack: number;
  decay: number;
  /** 1–8 */
  bursts: number;
  /** s, 0.002–0.05 */
  burstSpacing: number;
}

export interface DrumMetal {
  level: number;
  /** Hz, 100–1000 */
  baseHz: number;
  decay: number;
  /** band-pass centre Hz */
  cutoff: number;
}

export interface DrumClick {
  level: number;
  /** s, 0.0005–0.02 */
  decay: number;
  /** Hz, 500–12000 */
  toneHz: number;
}

export interface DrumPatch {
  /** s, 0.02–3 */
  length: number;
  /** semitones, -24…24 */
  tune: number;
  body: DrumBody;
  noise: DrumNoise;
  metal: DrumMetal;
  click: DrumClick;
  drive: number;
  crush: number;
  /** Hz, 20–2000 */
  lowCut: number;
  /** Hz, 200–20000 */
  highCut: number;
  velocity: number;
  level: number;
}

export type SoundPatch = ({ kind: "synth" } & SynthPatch) | ({ kind: "drum" } & DrumPatch);

/** Factory ids are short slugs (`saw`, `kick-808`); user ids are `u-<uuid>`. */
export interface Sound {
  id: string;
  name: string;
  category: string;
  patch: SoundPatch;
}

export interface DrumPad {
  /** MIDI pitch, unique within the kit. */
  pitch: number;
  /** A drum sound id. */
  sound: string;
  /** Pads sharing a non-zero group cut each other off. */
  choke: number;
}

export const MAX_DRUM_PADS = 16;

// ─── Defaults (match the Rust `Default` impls) ──────────────────────────────────────────────

const DEFAULT_OSC: OscParams = {
  on: false,
  wave: "saw",
  octave: 0,
  semi: 0,
  fine: 0,
  level: 0.8,
  pulseWidth: 0.5,
  unison: 1,
  detune: 15,
  randomPhase: false,
};
const DEFAULT_LFO: LfoParams = { wave: "sine", rate: 5, depth: 0, target: "none", retrigger: false };

export const DEFAULT_SYNTH_PATCH: SynthPatch = {
  osc: [{ ...DEFAULT_OSC, on: true }, { ...DEFAULT_OSC }, { ...DEFAULT_OSC }],
  noiseLevel: 0,
  noiseColor: 0,
  fmAmount: 0,
  ringMod: 0,
  sync: false,
  filter: { mode: "lowpass12", cutoff: 20000, resonance: 0, drive: 0, keyTrack: 0, envAmount: 0, velocity: 0 },
  ampEnv: { attack: 0.008, decay: 0.1, sustain: 0.65, release: 0.14 },
  filterEnv: { attack: 0.005, decay: 0.3, sustain: 0, release: 0.2 },
  pitchEnvAmount: 0,
  pitchEnvDecay: 0.05,
  lfo: [{ ...DEFAULT_LFO }, { ...DEFAULT_LFO }],
  mono: false,
  glide: 0,
  velocity: 1,
  drive: 0,
  crush: 0,
  chorus: { mix: 0, rate: 0.8, depth: 0.5 },
  delay: { mix: 0, time: 0.3, feedback: 0.35 },
  level: 0.22,
};

export const DEFAULT_DRUM_PATCH: DrumPatch = {
  length: 0.34,
  tune: 0,
  body: { level: 1, wave: "sine", startHz: 160, endHz: 50, pitchDecay: 0.035, decay: 0.125, overtoneLevel: 0, overtoneRatio: 1.6 },
  noise: { level: 0, mode: "off", cutoff: 8000, resonance: 0, attack: 0, decay: 0.06, bursts: 1, burstSpacing: 0.01 },
  metal: { level: 0, baseHz: 205, decay: 0.05, cutoff: 9000 },
  click: { level: 0, decay: 0.003, toneHz: 3000 },
  drive: 0,
  crush: 0,
  lowCut: 20,
  highCut: 20000,
  velocity: 0.65,
  level: 0.7,
};

/** The kit a new drum track gets, top to bottom. Mirrors `dissonant_core::sound::default_kit`. */
export function defaultKit(): DrumPad[] {
  return [
    { pitch: 49, sound: "crash", choke: 0 },
    { pitch: 46, sound: "open-hat", choke: 1 },
    { pitch: 42, sound: "hat", choke: 1 },
    { pitch: 39, sound: "clap", choke: 0 },
    { pitch: 38, sound: "snare", choke: 0 },
    { pitch: 50, sound: "tom-hi", choke: 0 },
    { pitch: 45, sound: "tom-lo", choke: 0 },
    { pitch: 36, sound: "kick", choke: 0 },
  ];
}

// ─── Filling partial patches ────────────────────────────────────────────────────────────────

type Loose = Record<string, unknown>;
const isObj = (v: unknown): v is Loose => typeof v === "object" && v !== null && !Array.isArray(v);

/** Deep-merge `partial` over `base`; arrays merge element-wise (fixed-length param arrays). */
function fill<T>(base: T, partial: unknown): T {
  if (Array.isArray(base)) {
    const p = Array.isArray(partial) ? partial : [];
    return base.map((b, i) => fill(b, p[i])) as T;
  }
  if (isObj(base)) {
    const p = isObj(partial) ? partial : {};
    const out: Loose = {};
    for (const k of Object.keys(base)) out[k] = fill((base as Loose)[k], p[k]);
    return out as T;
  }
  return (partial === undefined || partial === null ? base : partial) as T;
}

/** A complete patch from a possibly partial one (what the factory JSON holds). */
export function filledPatch(patch: { kind: "synth" | "drum" } & Loose): SoundPatch {
  return patch.kind === "drum"
    ? { kind: "drum", ...fill(DEFAULT_DRUM_PATCH, patch) }
    : { kind: "synth", ...fill(DEFAULT_SYNTH_PATCH, patch) };
}

export function filledSound(s: { id: string; name: string; category?: string; patch: { kind: "synth" | "drum" } & Loose }): Sound {
  return { id: s.id, name: s.name, category: s.category ?? "", patch: filledPatch(s.patch) };
}

/** Every factory sound, complete, in file order. */
export const FACTORY_SOUNDS: Sound[] = (factoryJson as unknown as Parameters<typeof filledSound>[0][]).map(filledSound);

export function factorySound(id: string): Sound | undefined {
  return FACTORY_SOUNDS.find((s) => s.id === id);
}

export function isFactory(id: string): boolean {
  return FACTORY_SOUNDS.some((s) => s.id === id);
}

export const isDrumSound = (s: Sound) => s.patch.kind === "drum";

/** A fresh user-sound id. */
export function userSoundId(): string {
  const r = typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `${Date.now().toString(16)}-${Math.random().toString(16).slice(2)}`;
  return `u-${r}`;
}

/** A sound by id from the project's embedded sounds, then the factory. */
export function lookupSound(projectSounds: Sound[], id: string): Sound | undefined {
  return projectSounds.find((s) => s.id === id) ?? factorySound(id);
}
