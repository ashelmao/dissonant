// SOUND page logic that doesn't touch React: parameter tapers and readouts, randomize / mutate
// over a list of parameter definitions, plausibility fix-ups, the draft's undo history and the
// drawn drum sketch. The parameter lists themselves live in components/sound/soundParams.ts.

import type { DrumPatch, Envelope, SynthPatch } from "./sound";

// ─── Parameter definitions ──────────────────────────────────────────────────────────────────

/**
 * How a knob's 0–1 travel maps onto a value. `log` for Hz and most times (needs min > 0),
 * `pow` for times that start at 0 (cubic: fine control near zero), `int` for steps.
 */
export type Taper = "lin" | "log" | "pow" | "int";
export type Path = readonly (string | number)[];

/** What randomize draws. `chance` is the odds the param is rolled at all; otherwise it gets its default. */
export type NumRand = { range: readonly [number, number]; chance?: number } | { pick: readonly number[]; chance?: number };
export type ChoiceRand = { pick: readonly string[]; chance?: number };
export type BoolRand = { p: number };

interface ParamBase {
  /** Unique within a patch kind, e.g. `osc2.detune`. */
  id: string;
  /** Short knob label. */
  label: string;
  /** Longer description for the tooltip. */
  hint?: string;
  path: Path;
}
export interface NumParam extends ParamBase {
  type: "num";
  min: number;
  max: number;
  taper: Taper;
  format: (v: number) => string;
  rand?: NumRand;
  /** Left alone by mutate (output level, velocity). */
  noMutate?: boolean;
}
export interface ChoiceParam extends ParamBase {
  type: "choice";
  options: readonly string[];
  /** Display text per option; the option itself otherwise. */
  labels?: Readonly<Record<string, string>>;
  rand?: ChoiceRand;
}
export interface BoolParam extends ParamBase {
  type: "bool";
  rand?: BoolRand;
}
export type ParamDef = NumParam | ChoiceParam | BoolParam;

// ─── Tapers ─────────────────────────────────────────────────────────────────────────────────

const POW = 3;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Value → knob position 0–1. */
export function toNorm(d: Pick<NumParam, "min" | "max" | "taper">, v: number): number {
  const { min, max } = d;
  let p: number;
  switch (d.taper) {
    case "log":
      p = Math.log(clamp(v, min, max) / min) / Math.log(max / min);
      break;
    case "pow":
      p = Math.pow(clamp((v - min) / (max - min), 0, 1), 1 / POW);
      break;
    default:
      p = (v - min) / (max - min);
  }
  return clamp(Number.isFinite(p) ? p : 0, 0, 1);
}

/** Knob position 0–1 → value (rounded for `int`). */
export function fromNorm(d: Pick<NumParam, "min" | "max" | "taper">, p: number): number {
  const { min, max } = d;
  const q = clamp(p, 0, 1);
  let v: number;
  switch (d.taper) {
    case "log":
      v = min * Math.pow(max / min, q);
      break;
    case "pow":
      v = min + Math.pow(q, POW) * (max - min);
      break;
    case "int":
      v = Math.round(min + q * (max - min));
      break;
    default:
      v = min + q * (max - min);
  }
  return clamp(v, min, max);
}

// ─── Readouts ───────────────────────────────────────────────────────────────────────────────

const signed = (v: number, digits = 0) => `${v > 0 ? "+" : v < 0 ? "−" : ""}${Math.abs(v).toFixed(digits)}`;

export const fmt = {
  pct: (v: number) => `${Math.round(v * 100)}%`,
  hz: (v: number) => (v >= 10000 ? `${(v / 1000).toFixed(1)}k` : v >= 1000 ? `${(v / 1000).toFixed(2)}k` : v >= 100 ? `${Math.round(v)}` : v >= 10 ? v.toFixed(1) : v.toFixed(2)) + (v >= 1000 ? "Hz" : " Hz"),
  secs: (v: number) => (v <= 0 ? "0 ms" : v < 0.01 ? `${(v * 1000).toFixed(1)} ms` : v < 1 ? `${Math.round(v * 1000)} ms` : `${v.toFixed(2)} s`),
  semis: (v: number) => `${signed(v, Number.isInteger(v) ? 0 : 1)} st`,
  cents: (v: number) => `${signed(Math.round(v))} ct`,
  octaves: (v: number) => `${signed(v, 1)} oct`,
  ratio: (v: number) => `×${v.toFixed(2)}`,
  int: (v: number) => `${signed(Math.round(v))}`,
  count: (v: number) => `${Math.round(v)}`,
  voices: (v: number) => `${Math.round(v)} v`,
  db: (v: number) => (v <= 0.0005 ? "−inf" : `${signed(20 * Math.log10(v), 1)} dB`),
};

// ─── Paths ──────────────────────────────────────────────────────────────────────────────────

export function getPath(obj: unknown, path: Path): unknown {
  let o = obj as Record<string | number, unknown> | undefined;
  for (const k of path) o = o?.[k] as Record<string | number, unknown> | undefined;
  return o;
}

/** An immutable copy of `obj` with the value at `path` replaced. */
export function setPath<T>(obj: T, path: Path, value: unknown): T {
  if (path.length === 0) return value as T;
  const [k, ...rest] = path;
  const src = obj as unknown as Record<string | number, unknown>;
  const copy = (Array.isArray(obj) ? [...obj] : { ...src }) as Record<string | number, unknown>;
  copy[k] = setPath(src[k], rest, value);
  return copy as T;
}

// ─── Randomize / mutate ─────────────────────────────────────────────────────────────────────

export type Rng = () => number;

/** A small seedable generator (mulberry32), for tests and repeatable rolls. */
export function seededRng(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const pickOf = <T,>(list: readonly T[], rng: Rng): T => list[Math.min(list.length - 1, Math.floor(rng() * list.length))];

/** One roll of a param's value; `undefined` means "use the default". */
function roll(d: ParamDef, rand: NumRand | ChoiceRand | BoolRand, rng: Rng): unknown {
  if ("chance" in rand && rand.chance !== undefined && rng() >= rand.chance) return undefined;
  if (d.type === "bool") return rng() < (rand as BoolRand).p;
  if (d.type === "choice") return pickOf((rand as ChoiceRand).pick, rng);
  const r = rand as NumRand;
  if ("pick" in r) return clamp(pickOf(r.pick, rng), d.min, d.max);
  const lo = toNorm(d, r.range[0]);
  const hi = toNorm(d, r.range[1]);
  return fromNorm(d, lo + rng() * (hi - lo));
}

/**
 * A rolled patch: every param with a `rand` (or an override) is drawn from its range, the rest
 * keep `defaults`. Not uniform noise: the ranges are narrower than the knobs and many params
 * only roll some of the time.
 */
export function randomize<P>(defaults: P, defs: readonly ParamDef[], rng: Rng, overrides: Readonly<Record<string, NumRand | ChoiceRand | BoolRand>> = {}): P {
  let out = defaults;
  for (const d of defs) {
    const rand = overrides[d.id] ?? d.rand;
    if (!rand) continue;
    const v = roll(d, rand, rng);
    if (v !== undefined) out = setPath(out, d.path, v);
  }
  return out;
}

/**
 * Nudge roughly `amount` of the numeric params that are audible right now, each by 3–8% of its
 * knob travel (one step for stepped params). Always changes at least one, never all of them.
 */
export function mutate<P>(patch: P, defs: readonly ParamDef[], rng: Rng, relevant: (patch: P, d: ParamDef) => boolean = () => true, amount = 0.2 + 0.1 * rng()): P {
  const pool = defs.filter((d): d is NumParam => d.type === "num" && !d.noMutate && relevant(patch, d));
  if (pool.length === 0) return patch;
  const n = clamp(Math.round(pool.length * amount), 1, Math.max(1, pool.length - 1));
  // Fisher–Yates on a copy, then take the first n.
  const order = [...pool];
  for (let i = order.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    [order[i], order[j]] = [order[j], order[i]];
  }
  let out = patch;
  for (const d of order.slice(0, n)) {
    const v = getPath(out, d.path) as number;
    const dir = rng() < 0.5 ? -1 : 1;
    const step = (v2: number, s: number) => (d.taper === "int" ? clamp(Math.round(v2) + s, d.min, d.max) : fromNorm(d, toNorm(d, v2) + s * (0.03 + rng() * 0.05)));
    let next = step(v, dir);
    if (Math.abs(next - v) < 1e-9) next = step(v, -dir); // pinned at an end: go the other way
    out = setPath(out, d.path, next);
  }
  return out;
}

/** Fix-ups that keep a rolled synth patch playable and sensible. */
export function plausibleSynth(p: SynthPatch): SynthPatch {
  const q: SynthPatch = structuredClone(p);
  q.osc[0].on = true;
  const osc2 = q.osc[1].on;
  if (!osc2) {
    q.sync = false;
    q.fmAmount = 0;
    q.ringMod = 0;
  }
  const hasPulse = q.osc.some((o) => o.on && o.wave === "pulse");
  for (const l of q.lfo) {
    if (l.target === "pitch") l.depth = Math.min(l.depth, 0.05); // vibrato, not a siren
    if (l.target === "pulseWidth" && !hasPulse) l.target = "cutoff";
    if (l.target === "fm" && q.fmAmount === 0) l.target = "cutoff";
    if (l.target === "none") l.depth = 0;
  }
  const f = q.filter;
  if (f.mode === "lowpass12" || f.mode === "lowpass24") {
    // Keep some top end open somewhere in the note, or the roll is a muffled thud.
    const opens = Math.max(0, f.envAmount) * Math.max(0.3, q.filterEnv.sustain);
    const need = 450 / Math.pow(2, opens);
    if (f.cutoff < need) f.cutoff = Math.min(20000, need);
  } else if (f.mode === "highpass12") {
    f.cutoff = Math.min(f.cutoff, 1200);
  } else if (f.mode === "bandpass" || f.mode === "notch") {
    f.cutoff = clamp(f.cutoff, 300, 4000);
    f.resonance = Math.min(f.resonance, 0.6);
  }
  const a = q.ampEnv;
  if (a.attack > 0.4) a.sustain = Math.max(a.sustain, 0.5); // a slow swell should arrive somewhere
  if (a.sustain < 0.15) a.decay = Math.max(a.decay, 0.18);
  if (!q.mono) q.glide = 0;
  return q;
}

/** Fix-ups for a rolled drum: something is audible and the hit fits in its length. */
export function plausibleDrum(p: DrumPatch): DrumPatch {
  const q: DrumPatch = structuredClone(p);
  if (q.body.level < 0.1 && q.noise.level < 0.1 && q.metal.level < 0.1 && q.click.level < 0.1) q.body.level = 0.8;
  const tail = Math.max(
    q.body.level > 0.05 ? q.body.decay : 0,
    q.noise.level > 0.05 ? q.noise.attack + q.noise.decay + (q.noise.bursts - 1) * q.noise.burstSpacing : 0,
    q.metal.level > 0.05 ? q.metal.decay : 0,
    q.click.level > 0.05 ? q.click.decay : 0,
  );
  q.length = clamp(Math.max(q.length, tail * 3), 0.05, 3);
  if (q.highCut < q.lowCut * 2) q.highCut = Math.min(20000, q.lowCut * 4);
  return q;
}

// ─── Draft history ──────────────────────────────────────────────────────────────────────────

/**
 * The page's own undo stack for the draft (document undo is untouched). Edits that share a
 * `key` coalesce until `seal` — a knob drag is one step, not two hundred.
 */
export interface History<T> {
  past: T[];
  present: T;
  future: T[];
  key: string | null;
}

const HISTORY_LIMIT = 200;

export const historyOf = <T,>(present: T): History<T> => ({ past: [], present, future: [], key: null });

export function record<T>(h: History<T>, next: T, key: string | null = null): History<T> {
  if (next === h.present) return h;
  if (key !== null && key === h.key) return { ...h, present: next, future: [] };
  return { past: [...h.past, h.present].slice(-HISTORY_LIMIT), present: next, future: [], key };
}

export const seal = <T,>(h: History<T>): History<T> => (h.key === null ? h : { ...h, key: null });

export function undo<T>(h: History<T>): History<T> {
  if (h.past.length === 0) return h;
  return { past: h.past.slice(0, -1), present: h.past[h.past.length - 1], future: [h.present, ...h.future], key: null };
}

export function redo<T>(h: History<T>): History<T> {
  if (h.future.length === 0) return h;
  return { past: [...h.past, h.present], present: h.future[0], future: h.future.slice(1), key: null };
}

// ─── Drawn shapes ───────────────────────────────────────────────────────────────────────────

/**
 * An ADSR as points (seconds, level) for drawing: attack, decay to sustain, a held stretch of
 * `hold` seconds, release. The curves are exponential-ish like the engine's.
 */
export function adsrPoints(env: Envelope, hold: number, steps = 24): [number, number][] {
  const pts: [number, number][] = [[0, 0]];
  const a = Math.max(0, env.attack);
  for (let i = 1; i <= steps; i++) pts.push([(a * i) / steps, i / steps]);
  const d = Math.max(0.001, env.decay);
  for (let i = 1; i <= steps; i++) {
    const t = (d * i) / steps;
    pts.push([a + t, env.sustain + (1 - env.sustain) * Math.exp((-4 * t) / d)]);
  }
  const t0 = a + d + hold;
  pts.push([t0, env.sustain]);
  const r = Math.max(0.001, env.release);
  for (let i = 1; i <= steps; i++) {
    const t = (r * i) / steps;
    pts.push([t0 + t, env.sustain * Math.exp((-4 * t) / r) * (1 - i / steps / 8)]);
  }
  pts.push([t0 + r, 0]);
  return pts;
}

/** A drum hit sketch: per-layer amplitude, their sum and the body pitch over `length`, `n` samples. */
export function drumSketch(p: DrumPatch, n = 160): { t: number[]; amp: number[]; pitch: number[]; layers: Record<"body" | "noise" | "metal" | "click", number[]> } {
  const len = Math.max(0.02, p.length);
  const t: number[] = [];
  const amp: number[] = [];
  const pitch: number[] = [];
  const layers = { body: [] as number[], noise: [] as number[], metal: [] as number[], click: [] as number[] };
  const tune = Math.pow(2, p.tune / 12);
  const ex = (x: number, d: number) => Math.exp((-x * 4.6) / Math.max(0.0005, d)); // ~-40 dB at `d`
  for (let i = 0; i < n; i++) {
    const x = (len * i) / (n - 1);
    const body = p.body.level * ex(x, p.body.decay);
    let noise = 0;
    for (let b = 0; b < p.noise.bursts; b++) {
      const xb = x - b * p.noise.burstSpacing;
      if (xb < 0) continue;
      const atk = p.noise.attack > 0 ? Math.min(1, xb / p.noise.attack) : 1;
      const last = b === p.noise.bursts - 1;
      noise = Math.max(noise, atk * ex(Math.max(0, xb - p.noise.attack), last ? p.noise.decay : Math.min(p.noise.decay, p.noise.burstSpacing)));
    }
    noise *= p.noise.level;
    const metal = p.metal.level * ex(x, p.metal.decay);
    const click = p.click.level * ex(x, p.click.decay);
    // A short fade at the end of the buffer, like the renderer's.
    const fade = Math.min(1, (len - x) / Math.min(0.01, len * 0.1));
    t.push(x);
    layers.body.push(body * fade);
    layers.noise.push(noise * fade);
    layers.metal.push(metal * fade);
    layers.click.push(click * fade);
    amp.push(Math.min(1.5, Math.hypot(body, noise, metal, click)) * fade);
    pitch.push((p.body.endHz + (p.body.startHz - p.body.endHz) * ex(x, p.body.pitchDecay)) * tune);
  }
  return { t, amp, pitch, layers };
}

// ─── Misc ───────────────────────────────────────────────────────────────────────────────────

/** Leading + trailing throttle: at most one call per `ms`, the latest arguments win. */
export function throttle<A extends unknown[]>(fn: (...args: A) => void, ms: number): ((...args: A) => void) & { flush(): void; cancel(): void } {
  let last = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending: A | null = null;
  const run = () => {
    timer = null;
    last = Date.now();
    if (pending) {
      const a = pending;
      pending = null;
      fn(...a);
    }
  };
  const call = (...args: A) => {
    pending = args;
    const wait = ms - (Date.now() - last);
    if (wait <= 0 && !timer) run();
    else if (!timer) timer = setTimeout(run, Math.max(0, wait));
  };
  call.flush = () => {
    if (timer) clearTimeout(timer);
    run();
  };
  call.cancel = () => {
    if (timer) clearTimeout(timer);
    timer = null;
    pending = null;
  };
  return call;
}
