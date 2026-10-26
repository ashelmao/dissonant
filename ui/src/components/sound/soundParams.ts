// Every SOUND page parameter: label, path into the patch, range, taper, readout and how
// randomize rolls it. The editors render knobs from these lists and randomize / mutate walk
// the same lists, so a param added here is editable and rollable at once.

import { DEFAULT_DRUM_PATCH, DEFAULT_SYNTH_PATCH, FILTER_MODES, LFO_TARGETS, LFO_WAVES, OSC_WAVES, type DrumPatch, type SoundPatch, type SynthPatch } from "../../sound";
import { fmt, getPath, mutate, plausibleDrum, plausibleSynth, randomize, type BoolParam, type ChoiceParam, type ChoiceRand, type NumParam, type NumRand, type BoolRand, type ParamDef, type Path, type Rng, type Taper } from "../../soundDesign";

export interface Section {
  id: string;
  title: string;
  params: ParamDef[];
}

const num = (id: string, label: string, path: Path, min: number, max: number, taper: Taper, format: (v: number) => string, rand?: NumRand, extra: Partial<NumParam> = {}): NumParam => ({
  type: "num",
  id,
  label,
  path,
  min,
  max,
  taper,
  format,
  rand,
  ...extra,
});
const choice = (id: string, label: string, path: Path, options: readonly string[], rand?: ChoiceRand, labels?: Record<string, string>): ChoiceParam => ({ type: "choice", id, label, path, options, rand, labels });
const bool = (id: string, label: string, path: Path, rand?: BoolRand, hint?: string): BoolParam => ({ type: "bool", id, label, path, rand, hint });
const unit = (id: string, label: string, path: Path, rand?: NumRand, extra?: Partial<NumParam>) => num(id, label, path, 0, 1, "lin", fmt.pct, rand, extra);

export const WAVE_LABELS: Record<string, string> = { sine: "sin", triangle: "tri", saw: "saw", square: "sqr", pulse: "pls" };
export const FILTER_LABELS: Record<string, string> = { off: "off", lowpass12: "lp12", lowpass24: "lp24", highpass12: "hp12", bandpass: "bp", notch: "notch" };
export const LFO_WAVE_LABELS: Record<string, string> = { sine: "sin", triangle: "tri", saw: "saw", square: "sqr", sampleHold: "s&h" };
export const LFO_TARGET_LABELS: Record<string, string> = { none: "none", pitch: "pitch", cutoff: "cutoff", amp: "amp", pulseWidth: "pw", fm: "fm" };

// ─── Synth ──────────────────────────────────────────────────────────────────────────────────

function oscSection(i: 0 | 1 | 2): Section {
  const p = (k: string): Path => ["osc", i, k];
  const id = (k: string) => `osc${i + 1}.${k}`;
  return {
    id: `osc${i + 1}`,
    title: `osc ${i + 1}`,
    params: [
      bool(id("on"), "on", p("on"), i === 0 ? undefined : { p: i === 1 ? 0.65 : 0.3 }),
      choice(id("wave"), "wave", p("wave"), OSC_WAVES, { pick: ["saw", "saw", "square", "pulse", "triangle", "sine"] }, WAVE_LABELS),
      num(id("octave"), "oct", p("octave"), -3, 3, "int", fmt.int, { pick: i === 0 ? [-1, 0, 0] : [-1, 0, 0, 1] }),
      num(id("semi"), "semi", p("semi"), -12, 12, "int", fmt.semis, { pick: i === 0 ? [0] : [0, 0, 0, 0, 7, 12, -12, 5, -5] }),
      num(id("fine"), "fine", p("fine"), -100, 100, "lin", fmt.cents, { range: [-12, 12] }),
      unit(id("level"), "level", p("level"), { range: [0.45, 1] }),
      num(id("pulseWidth"), "pw", p("pulseWidth"), 0.05, 0.95, "lin", fmt.pct, { range: [0.2, 0.8] }, { hint: "pulse width (pulse wave)" }),
      num(id("unison"), "unison", p("unison"), 1, 8, "int", fmt.voices, { pick: [1, 1, 1, 1, 2, 3, 4] }),
      num(id("detune"), "detune", p("detune"), 0, 100, "pow", fmt.cents, { range: [4, 35] }, { hint: "unison spread" }),
      bool(id("randomPhase"), "rnd phase", p("randomPhase"), { p: 0.5 }, "start each voice at a random phase"),
    ],
  };
}

function envParams(prefix: string, key: "ampEnv" | "filterEnv", sustainRand: [number, number], attackMax: number): ParamDef[] {
  return [
    num(`${prefix}.attack`, "atk", [key, "attack"], 0, 10, "pow", fmt.secs, { range: [0, attackMax] }),
    num(`${prefix}.decay`, "dec", [key, "decay"], 0.001, 10, "log", fmt.secs, { range: [0.05, 2.5] }),
    unit(`${prefix}.sustain`, "sus", [key, "sustain"], { range: sustainRand }),
    num(`${prefix}.release`, "rel", [key, "release"], 0.001, 10, "log", fmt.secs, { range: [0.02, 2.5] }),
  ];
}

function lfoSection(i: 0 | 1): Section {
  const p = (k: string): Path => ["lfo", i, k];
  const id = (k: string) => `lfo${i + 1}.${k}`;
  return {
    id: `lfo${i + 1}`,
    title: `lfo ${i + 1}`,
    params: [
      choice(id("wave"), "wave", p("wave"), LFO_WAVES, { pick: LFO_WAVES }, LFO_WAVE_LABELS),
      choice(id("target"), "target", p("target"), LFO_TARGETS, { pick: ["none", "none", "cutoff", "cutoff", "pitch", "amp", "pulseWidth"] }, LFO_TARGET_LABELS),
      num(id("rate"), "rate", p("rate"), 0.01, 30, "log", fmt.hz, { range: [0.1, 8] }),
      unit(id("depth"), "depth", p("depth"), { range: [0, 0.5] }, { hint: "pitch ±12 st · cutoff ±4 oct · amp tremolo · pw ±0.45 · fm ±1" }),
      bool(id("retrigger"), "retrig", p("retrigger"), { p: 0.4 }, "restart the LFO on every note"),
    ],
  };
}

export const SYNTH_SECTIONS: Section[] = [
  oscSection(0),
  oscSection(1),
  oscSection(2),
  {
    id: "noise",
    title: "noise",
    params: [unit("noise.level", "level", ["noiseLevel"], { range: [0.02, 0.3], chance: 0.3 }), unit("noise.color", "color", ["noiseColor"], { range: [0, 1] }, { hint: "0 white … 1 dark" })],
  },
  {
    id: "mod",
    title: "mod",
    params: [
      unit("mod.fm", "fm", ["fmAmount"], { range: [0.05, 0.5], chance: 0.2 }, { hint: "osc 2 → osc 1 phase" }),
      unit("mod.ring", "ring", ["ringMod"], { range: [0.1, 0.6], chance: 0.12 }, { hint: "osc 1 × osc 2" }),
      bool("mod.sync", "sync", ["sync"], { p: 0.12 }, "osc 2 hard-synced to osc 1"),
    ],
  },
  {
    id: "filter",
    title: "filter",
    params: [
      choice("filter.mode", "mode", ["filter", "mode"], FILTER_MODES, { pick: ["lowpass24", "lowpass24", "lowpass24", "lowpass12", "lowpass12", "lowpass12", "bandpass", "highpass12"] }, FILTER_LABELS),
      num("filter.cutoff", "cutoff", ["filter", "cutoff"], 20, 20000, "log", fmt.hz, { range: [180, 9000] }),
      unit("filter.resonance", "reso", ["filter", "resonance"], { range: [0, 0.7] }),
      unit("filter.drive", "drive", ["filter", "drive"], { range: [0, 0.4], chance: 0.4 }),
      unit("filter.keyTrack", "key", ["filter", "keyTrack"], { range: [0, 0.7] }, { hint: "cutoff follows the note" }),
      num("filter.envAmount", "env", ["filter", "envAmount"], -8, 8, "lin", fmt.octaves, { range: [-1, 5], chance: 0.75 }, { hint: "filter envelope depth" }),
      unit("filter.velocity", "vel", ["filter", "velocity"], { range: [0, 0.6] }, { hint: "velocity → cutoff" }),
    ],
  },
  { id: "ampEnv", title: "amp env", params: envParams("ampEnv", "ampEnv", [0, 1], 1.5) },
  { id: "filterEnv", title: "filter env", params: envParams("filterEnv", "filterEnv", [0, 0.8], 0.8) },
  {
    id: "pitchEnv",
    title: "pitch env",
    params: [
      num("pitchEnv.amount", "amount", ["pitchEnvAmount"], -48, 48, "lin", fmt.semis, { range: [-12, 24], chance: 0.15 }),
      num("pitchEnv.decay", "decay", ["pitchEnvDecay"], 0.001, 2, "log", fmt.secs, { range: [0.005, 0.3] }),
    ],
  },
  lfoSection(0),
  lfoSection(1),
  {
    id: "voice",
    title: "voice",
    params: [
      bool("voice.mono", "mono", ["mono"], { p: 0.2 }, "one note at a time"),
      num("voice.glide", "glide", ["glide"], 0, 2, "pow", fmt.secs, { range: [0, 0.2] }, { hint: "portamento (mono)" }),
      unit("voice.velocity", "vel", ["velocity"], undefined, { noMutate: true, hint: "velocity → loudness" }),
    ],
  },
  {
    id: "character",
    title: "character",
    params: [unit("character.drive", "drive", ["drive"], { range: [0, 0.35], chance: 0.5 }), unit("character.crush", "crush", ["crush"], { range: [0, 0.35], chance: 0.1 }, { hint: "bit / rate crush" })],
  },
  {
    id: "chorus",
    title: "chorus",
    params: [
      unit("chorus.mix", "mix", ["chorus", "mix"], { range: [0.15, 0.6], chance: 0.35 }),
      num("chorus.rate", "rate", ["chorus", "rate"], 0.05, 5, "log", fmt.hz, { range: [0.1, 2] }),
      unit("chorus.depth", "depth", ["chorus", "depth"], { range: [0.2, 0.9] }),
    ],
  },
  {
    id: "delay",
    title: "delay",
    params: [
      unit("delay.mix", "mix", ["delay", "mix"], { range: [0.1, 0.35], chance: 0.25 }),
      num("delay.time", "time", ["delay", "time"], 0.01, 1.5, "log", fmt.secs, { range: [0.08, 0.7] }),
      num("delay.feedback", "fdbk", ["delay", "feedback"], 0, 0.95, "lin", fmt.pct, { range: [0.1, 0.55] }),
    ],
  },
  { id: "output", title: "output", params: [num("output.level", "level", ["level"], 0, 1, "pow", fmt.db, undefined, { noMutate: true })] },
];

export const SYNTH_PARAMS: ParamDef[] = SYNTH_SECTIONS.flatMap((s) => s.params);

// ─── Drum ───────────────────────────────────────────────────────────────────────────────────

export const DRUM_SECTIONS: Section[] = [
  {
    id: "body",
    title: "body",
    params: [
      unit("body.level", "level", ["body", "level"], { range: [0, 1] }),
      choice("body.wave", "wave", ["body", "wave"], OSC_WAVES, { pick: ["sine", "sine", "sine", "triangle", "square"] }, WAVE_LABELS),
      num("body.startHz", "start", ["body", "startHz"], 20, 4000, "log", fmt.hz, { range: [60, 800] }, { hint: "pitch at the hit" }),
      num("body.endHz", "end", ["body", "endHz"], 20, 4000, "log", fmt.hz, { range: [30, 300] }, { hint: "pitch it sweeps down to" }),
      num("body.pitchDecay", "sweep", ["body", "pitchDecay"], 0.001, 2, "log", fmt.secs, { range: [0.005, 0.1] }, { hint: "pitch sweep time" }),
      num("body.decay", "decay", ["body", "decay"], 0.001, 3, "log", fmt.secs, { range: [0.04, 0.6] }),
      unit("body.overtoneLevel", "ovt lvl", ["body", "overtoneLevel"], { range: [0, 0.5], chance: 0.4 }, { hint: "overtone level" }),
      num("body.overtoneRatio", "ovt ×", ["body", "overtoneRatio"], 0.5, 8, "log", fmt.ratio, { range: [1.2, 3.5] }, { hint: "overtone frequency ratio" }),
    ],
  },
  {
    id: "noise",
    title: "noise",
    params: [
      unit("noise.level", "level", ["noise", "level"], { range: [0, 1], chance: 0.6 }),
      choice("noise.mode", "filter", ["noise", "mode"], FILTER_MODES, { pick: ["highpass12", "bandpass", "lowpass12", "off"] }, FILTER_LABELS),
      num("noise.cutoff", "cutoff", ["noise", "cutoff"], 20, 20000, "log", fmt.hz, { range: [600, 12000] }),
      unit("noise.resonance", "reso", ["noise", "resonance"], { range: [0, 0.5] }),
      num("noise.attack", "atk", ["noise", "attack"], 0, 0.1, "pow", fmt.secs, { range: [0, 0.01] }),
      num("noise.decay", "decay", ["noise", "decay"], 0.001, 3, "log", fmt.secs, { range: [0.02, 0.4] }),
      num("noise.bursts", "bursts", ["noise", "bursts"], 1, 8, "int", fmt.count, { pick: [1, 1, 1, 1, 2, 3, 4] }, { hint: "clap-style repeats" }),
      num("noise.burstSpacing", "spacing", ["noise", "burstSpacing"], 0.002, 0.05, "log", fmt.secs, { range: [0.005, 0.02] }),
    ],
  },
  {
    id: "metal",
    title: "metal",
    params: [
      unit("metal.level", "level", ["metal", "level"], { range: [0.2, 1], chance: 0.25 }, { hint: "808-style square cluster" }),
      num("metal.baseHz", "base", ["metal", "baseHz"], 100, 1000, "log", fmt.hz, { range: [150, 600] }),
      num("metal.decay", "decay", ["metal", "decay"], 0.001, 3, "log", fmt.secs, { range: [0.02, 0.8] }),
      num("metal.cutoff", "tone", ["metal", "cutoff"], 200, 20000, "log", fmt.hz, { range: [4000, 14000] }, { hint: "band-pass centre" }),
    ],
  },
  {
    id: "click",
    title: "click",
    params: [
      unit("click.level", "level", ["click", "level"], { range: [0.1, 0.6], chance: 0.4 }),
      num("click.decay", "decay", ["click", "decay"], 0.0005, 0.02, "log", fmt.secs, { range: [0.001, 0.008] }),
      num("click.toneHz", "tone", ["click", "toneHz"], 500, 12000, "log", fmt.hz, { range: [1500, 8000] }),
    ],
  },
  {
    id: "global",
    title: "global",
    params: [
      num("global.length", "length", ["length"], 0.02, 3, "log", fmt.secs, undefined, { hint: "buffer length; the hit is cut here" }),
      num("global.tune", "tune", ["tune"], -24, 24, "lin", fmt.semis, { pick: [0] }),
      unit("global.drive", "drive", ["drive"], { range: [0, 0.5], chance: 0.4 }),
      unit("global.crush", "crush", ["crush"], { range: [0, 0.4], chance: 0.1 }),
      num("global.lowCut", "low cut", ["lowCut"], 20, 2000, "log", fmt.hz, { pick: [20] }),
      num("global.highCut", "high cut", ["highCut"], 200, 20000, "log", fmt.hz, { range: [6000, 20000] }),
      unit("global.velocity", "vel", ["velocity"], undefined, { noMutate: true, hint: "velocity → loudness" }),
      num("global.level", "level", ["level"], 0, 1, "pow", fmt.db, undefined, { noMutate: true }),
    ],
  },
];

export const DRUM_PARAMS: ParamDef[] = DRUM_SECTIONS.flatMap((s) => s.params);

type Overrides = Record<string, NumRand | ChoiceRand | BoolRand>;
const off = (layer: string): Overrides => ({ [`${layer}.level`]: { pick: [0] } });

/**
 * What a drum of each kind rolls from: narrower ranges over the generic ones, and the layers
 * that kind doesn't use switched off. A roll keeps the draft's kind when its category is one.
 */
export const DRUM_ARCHETYPES: Record<string, Overrides> = {
  kick: {
    "body.level": { pick: [1] },
    "body.wave": { pick: ["sine", "sine", "sine", "triangle"] },
    "body.startHz": { range: [90, 320] },
    "body.endHz": { range: [35, 65] },
    "body.pitchDecay": { range: [0.01, 0.07] },
    "body.decay": { range: [0.12, 0.7] },
    "body.overtoneLevel": { range: [0, 0.3], chance: 0.3 },
    "noise.level": { range: [0.03, 0.2], chance: 0.3 },
    "noise.mode": { pick: ["lowpass12"] },
    "noise.cutoff": { range: [1000, 5000] },
    "noise.bursts": { pick: [1] },
    ...off("metal"),
    "click.level": { range: [0.1, 0.5], chance: 0.6 },
    "global.highCut": { range: [4000, 20000] },
  },
  snare: {
    "body.level": { range: [0.4, 0.8] },
    "body.wave": { pick: ["sine", "triangle"] },
    "body.startHz": { range: [180, 400] },
    "body.endHz": { range: [120, 170] },
    "body.pitchDecay": { range: [0.005, 0.04] },
    "body.decay": { range: [0.05, 0.2] },
    "body.overtoneLevel": { range: [0.1, 0.5] },
    "body.overtoneRatio": { range: [1.4, 2.4] },
    "noise.level": { range: [0.6, 1] },
    "noise.mode": { pick: ["bandpass", "highpass12", "lowpass12"] },
    "noise.cutoff": { range: [2000, 9000] },
    "noise.resonance": { range: [0, 0.4] },
    "noise.attack": { pick: [0] },
    "noise.decay": { range: [0.08, 0.3] },
    "noise.bursts": { pick: [1] },
    "metal.level": { range: [0.1, 0.3], chance: 0.15 },
    "global.lowCut": { range: [60, 200] },
  },
  clap: {
    ...off("body"),
    "noise.level": { range: [0.8, 1] },
    "noise.mode": { pick: ["bandpass"] },
    "noise.cutoff": { range: [800, 2500] },
    "noise.resonance": { range: [0.1, 0.5] },
    "noise.decay": { range: [0.08, 0.35] },
    "noise.bursts": { pick: [2, 3, 3, 4] },
    "noise.burstSpacing": { range: [0.006, 0.016] },
    ...off("metal"),
    ...off("click"),
    "global.lowCut": { range: [200, 600] },
  },
  hat: {
    ...off("body"),
    "noise.level": { range: [0.3, 0.8], chance: 0.7 },
    "noise.mode": { pick: ["highpass12"] },
    "noise.cutoff": { range: [6000, 12000] },
    "noise.decay": { range: [0.02, 0.4] },
    "noise.bursts": { pick: [1] },
    "metal.level": { range: [0.5, 1] },
    "metal.baseHz": { range: [180, 500] },
    "metal.decay": { range: [0.03, 0.4] },
    "metal.cutoff": { range: [7000, 12000] },
    ...off("click"),
    "global.lowCut": { range: [800, 2000] },
  },
  cymbal: {
    ...off("body"),
    "noise.level": { range: [0.2, 0.6] },
    "noise.mode": { pick: ["highpass12"] },
    "noise.cutoff": { range: [5000, 10000] },
    "noise.decay": { range: [0.4, 1.5] },
    "noise.bursts": { pick: [1] },
    "metal.level": { range: [0.7, 1] },
    "metal.baseHz": { range: [200, 600] },
    "metal.decay": { range: [0.4, 1.5] },
    "metal.cutoff": { range: [6000, 14000] },
    ...off("click"),
    "global.lowCut": { range: [300, 1200] },
  },
  tom: {
    "body.level": { pick: [1] },
    "body.wave": { pick: ["sine", "sine", "triangle"] },
    "body.startHz": { range: [150, 400] },
    "body.endHz": { range: [60, 140] },
    "body.pitchDecay": { range: [0.02, 0.15] },
    "body.decay": { range: [0.15, 0.6] },
    "noise.level": { range: [0.05, 0.15], chance: 0.3 },
    "noise.bursts": { pick: [1] },
    ...off("metal"),
  },
  perc: {
    "body.level": { range: [0.6, 1] },
    "body.wave": { pick: ["square", "triangle", "pulse", "sine"] },
    "body.startHz": { range: [400, 2000] },
    "body.endHz": { range: [300, 1500] },
    "body.pitchDecay": { range: [0.005, 0.05] },
    "body.decay": { range: [0.03, 0.15] },
    "body.overtoneLevel": { range: [0.2, 0.7] },
    "body.overtoneRatio": { range: [1.5, 3] },
    "noise.level": { range: [0.05, 0.3], chance: 0.3 },
    "noise.bursts": { pick: [1] },
    ...off("metal"),
    "click.level": { range: [0.1, 0.5], chance: 0.5 },
  },
  zap: {
    "body.level": { pick: [1] },
    "body.wave": { pick: ["sine", "saw", "square", "triangle"] },
    "body.startHz": { range: [1500, 4000] },
    "body.endHz": { range: [40, 200] },
    "body.pitchDecay": { range: [0.01, 0.08] },
    "body.decay": { range: [0.05, 0.3] },
    ...off("noise"),
    ...off("metal"),
  },
};

export const DRUM_KINDS = Object.keys(DRUM_ARCHETYPES);

/** The archetype a category name suggests (`open hat` → hat), if any. */
export function archetypeFor(category: string): string | null {
  const c = category.toLowerCase();
  if (DRUM_ARCHETYPES[c]) return c;
  if (/hat|hh/.test(c)) return "hat";
  if (/crash|ride|cym/.test(c)) return "cymbal";
  if (/rim|cow|block|perc|conga|bongo/.test(c)) return "perc";
  return DRUM_KINDS.find((k) => c.includes(k)) ?? null;
}

// ─── Defaults, relevance, rolls ─────────────────────────────────────────────────────────────

export const paramsFor = (kind: "synth" | "drum") => (kind === "synth" ? SYNTH_PARAMS : DRUM_PARAMS);
export const sectionsFor = (kind: "synth" | "drum") => (kind === "synth" ? SYNTH_SECTIONS : DRUM_SECTIONS);
const defaultsFor = (kind: "synth" | "drum") => (kind === "synth" ? DEFAULT_SYNTH_PATCH : DEFAULT_DRUM_PATCH);

/** A param's default value (what double-click resets to). */
export function defaultOf(kind: "synth" | "drum", d: ParamDef): unknown {
  return getPath(defaultsFor(kind), d.path);
}

/** Whether changing a param would be heard right now (mutate skips silent ones). */
export function isRelevant(patch: SoundPatch, d: ParamDef): boolean {
  const [head, idx] = d.path;
  if (patch.kind === "synth") {
    if (head === "osc") return patch.osc[idx as number].on;
    if (head === "lfo") return patch.lfo[idx as number].target !== "none";
    if (head === "chorus" && idx !== "mix") return patch.chorus.mix > 0;
    if (head === "delay" && idx !== "mix") return patch.delay.mix > 0;
    if (head === "pitchEnvDecay") return patch.pitchEnvAmount !== 0;
    if (head === "noiseColor") return patch.noiseLevel > 0;
    if (head === "glide") return patch.mono;
    if (head === "filter" && idx !== "mode") return patch.filter.mode !== "off";
    if (head === "filterEnv") return patch.filter.mode !== "off" && patch.filter.envAmount !== 0;
    return true;
  }
  const layer = head === "body" || head === "noise" || head === "metal" || head === "click" ? patch[head] : null;
  if (layer && idx !== "level") return layer.level > 0;
  return true;
}

/** A fresh, musically plausible patch of the same kind; output level and velocity are kept. */
export function rollPatch(current: SoundPatch, rng: Rng, drumKind: string | null = null): SoundPatch {
  if (current.kind === "synth") {
    const rolled = randomize<SynthPatch>({ ...DEFAULT_SYNTH_PATCH, level: current.level, velocity: current.velocity }, SYNTH_PARAMS, rng);
    return { kind: "synth", ...plausibleSynth(rolled) };
  }
  const kind = drumKind && DRUM_ARCHETYPES[drumKind] ? drumKind : DRUM_KINDS[Math.floor(rng() * DRUM_KINDS.length)];
  const rolled = randomize<DrumPatch>({ ...DEFAULT_DRUM_PATCH, level: current.level, velocity: current.velocity }, DRUM_PARAMS, rng, DRUM_ARCHETYPES[kind]);
  return { kind: "drum", ...plausibleDrum(rolled) };
}

/** The same patch, a few audible params nudged. */
export function mutatePatch(current: SoundPatch, rng: Rng): SoundPatch {
  return mutate<SoundPatch>(current, paramsFor(current.kind), rng, isRelevant);
}
