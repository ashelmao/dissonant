// Pure helpers behind the SETUP page: building and placing chords on the song progression,
// tiling a starter across the song (key by key), next-chord suggestions, and song length in
// bars ⇄ min:sec. No React, no store; everything here is unit-tested.

import { chordName, diatonicChords, NOTE_NAMES, noteName, normalize, progression } from "./theory";
import type { ChordEvent, KeyState, ScaleType, Section } from "./types";
import { uuid } from "./types";

// ─── Building a chord: root + quality ──────────────────────────────────────────────────────

export interface ChordQuality {
  id: string;
  /** What the picker shows. */
  label: string;
  /** Appended to the root in the chord's name ("m7" → "Am7"). */
  suffix: string;
  intervals: number[];
}

export const QUALITIES: ChordQuality[] = [
  { id: "maj", label: "maj", suffix: "", intervals: [0, 4, 7] },
  { id: "min", label: "min", suffix: "m", intervals: [0, 3, 7] },
  { id: "dim", label: "dim", suffix: "dim", intervals: [0, 3, 6] },
  { id: "aug", label: "aug", suffix: "aug", intervals: [0, 4, 8] },
  { id: "sus2", label: "sus2", suffix: "sus2", intervals: [0, 2, 7] },
  { id: "sus4", label: "sus4", suffix: "sus4", intervals: [0, 5, 7] },
  { id: "7", label: "7", suffix: "7", intervals: [0, 4, 7, 10] },
  { id: "maj7", label: "maj7", suffix: "maj7", intervals: [0, 4, 7, 11] },
  { id: "m7", label: "m7", suffix: "m7", intervals: [0, 3, 7, 10] },
];

const sortedSet = (pcs: number[]) => [...new Set(pcs.map(normalize))].sort((a, b) => a - b);
const sameSet = (a: number[], b: number[]) => {
  const x = sortedSet(a);
  const y = sortedSet(b);
  return x.length === y.length && x.every((v, i) => v === y[i]);
};

/** Pitch classes and name of `root` + `quality`. */
export function buildChord(root: number, quality: ChordQuality): { pitchClasses: number[]; name: string } {
  return { pitchClasses: sortedSet(quality.intervals.map((i) => root + i)), name: `${noteName(root)}${quality.suffix}` };
}

/**
 * The root and quality a set of pitch classes spells, if it is one of `QUALITIES`.
 * `preferRoot` is tried first, so a chord named "Csus2" doesn't come back as "Gsus4".
 */
export function identifyChord(pitchClasses: number[], preferRoot?: number | null): { root: number; quality: ChordQuality } | null {
  const pcs = sortedSet(pitchClasses);
  if (pcs.length < 3) return null;
  const roots = preferRoot != null && pcs.includes(normalize(preferRoot)) ? [normalize(preferRoot), ...pcs] : pcs;
  for (const root of roots) {
    for (const q of QUALITIES) if (sameSet(q.intervals.map((i) => root + i), pcs)) return { root, quality: q };
  }
  return null;
}

/** Best name for a free-built chord: "Am7" when it spells a known quality, else its notes. */
export function nameOf(pitchClasses: number[], preferRoot?: number | null): string {
  const id = identifyChord(pitchClasses, preferRoot);
  return id ? `${noteName(id.root)}${id.quality.suffix}` : chordName(pitchClasses);
}

/** The root a chord's name starts with ("F#m7" → 6), or null. */
export function rootOfName(name: string | null): number | null {
  const m = name ? /^[A-G]#?/.exec(name) : null;
  const i = m ? NOTE_NAMES.indexOf(m[0]) : -1;
  return i < 0 ? null : i;
}

// ─── Placing chords on the song ────────────────────────────────────────────────────────────

/**
 * `chord` laid onto `chords`: whatever it overlaps is trimmed back, removed when fully covered,
 * or split in two when it spans the new chord. The result is sorted and non-overlapping (given
 * a non-overlapping input), which is what the song progression must be.
 */
export function placeChord(chords: ChordEvent[], chord: ChordEvent): ChordEvent[] {
  const s = chord.startBeat;
  const e = chord.startBeat + chord.lengthBeats;
  const out: ChordEvent[] = [];
  for (const c of chords) {
    if (c.id === chord.id) continue;
    const cEnd = c.startBeat + c.lengthBeats;
    if (cEnd <= s + 1e-9 || c.startBeat >= e - 1e-9) {
      out.push(c);
      continue;
    }
    if (c.startBeat < s - 1e-9) out.push({ ...c, lengthBeats: s - c.startBeat });
    if (cEnd > e + 1e-9) out.push({ ...c, id: c.startBeat < s - 1e-9 ? uuid() : c.id, startBeat: e, lengthBeats: cEnd - e });
  }
  out.push(chord);
  return out.sort((a, b) => a.startBeat - b.startBeat);
}

/** The free stretch around `beat`: from the previous chord's end to the next chord's start. */
export function gapAt(chords: ChordEvent[], beat: number, songEnd: number): { start: number; end: number } {
  let start = 0;
  let end = songEnd;
  for (const c of chords) {
    const cEnd = c.startBeat + c.lengthBeats;
    if (cEnd <= beat + 1e-9) start = Math.max(start, cEnd);
    else if (c.startBeat >= beat - 1e-9) end = Math.min(end, c.startBeat);
  }
  return { start, end };
}

// ─── Starters across the song, one key per section ─────────────────────────────────────────

export interface KeySpan {
  start: number;
  end: number;
  root: number;
  scale: ScaleType;
}

/**
 * `[start, end)` split where the key changes: a section with its own key uses it, everything
 * else the song key. An unset root falls back to C, like the starters have always done.
 */
export function keySpans(key: KeyState, sections: Section[], start: number, end: number): KeySpan[] {
  const keyAt = (beat: number): KeyState => {
    let best: Section | null = null;
    for (const s of sections) if (s.startBeat <= beat && (!best || s.startBeat >= best.startBeat)) best = s;
    return best?.key ?? key;
  };
  const cuts = [start, ...sections.map((s) => s.startBeat).filter((b) => b > start && b < end), end].sort((a, b) => a - b);
  const spans: KeySpan[] = [];
  for (let i = 0; i + 1 < cuts.length; i++) {
    if (cuts[i + 1] - cuts[i] <= 1e-9) continue;
    const k = keyAt(cuts[i]);
    const root = k.rootPitchClass ?? 0;
    const last = spans[spans.length - 1];
    if (last && last.root === root && last.scale === k.scale) last.end = cuts[i + 1];
    else spans.push({ start: cuts[i], end: cuts[i + 1], root, scale: k.scale });
  }
  return spans;
}

/** A starter's degrees tiled over each key span (restarting at each key change), in song beats. */
export function tileStarter(degrees: number[], spans: KeySpan[], chordBeats: number): ChordEvent[] {
  return spans.flatMap((sp) =>
    progression(degrees, sp.root, sp.scale, sp.end - sp.start, chordBeats).map((c) => ({ ...c, startBeat: c.startBeat + sp.start })),
  );
}

/** True when `chords` inside `[start, end)` are exactly `tiled` (ids aside): the starter is "on". */
export function matchesWindow(chords: ChordEvent[], tiled: ChordEvent[], start: number, end: number): boolean {
  const inside = chords.filter((c) => c.startBeat < end - 1e-9 && c.startBeat + c.lengthBeats > start + 1e-9);
  return (
    inside.length === tiled.length &&
    inside.every(
      (c, i) => Math.abs(c.startBeat - tiled[i].startBeat) < 1e-6 && Math.abs(c.lengthBeats - tiled[i].lengthBeats) < 1e-6 && sameSet(c.pitchClasses, tiled[i].pitchClasses),
    )
  );
}

// ─── What could come next ──────────────────────────────────────────────────────────────────

/** Common next steps from each scale degree (0-based), strongest first. Roughly functional harmony. */
const NEXT: number[][] = [
  [3, 4, 5, 1], // I → IV, V, vi, ii
  [4, 6, 3], // ii → V, vii°, IV
  [5, 3, 1], // iii → vi, IV, ii
  [4, 0, 1, 5], // IV → V, I, ii, vi
  [0, 5, 3], // V → I, vi, IV
  [3, 1, 4, 2], // vi → IV, ii, V, iii
  [0, 2], // vii° → I, iii
];

/** How each degree feels arriving there, in plain words. */
const FEEL: Record<ScaleType, string[]> = {
  major: ["home", "gentle step", "soft shade", "lift", "tension, wants home", "sad turn", "edge, leans home"],
  minor: ["home", "uneasy step", "bright shift", "deeper", "tension, wants home", "warm lift", "rising push"],
};

/** Overrides for particular moves, where the move says more than the arrival. */
const MOVE: Record<string, string> = {
  "4>0": "resolve home",
  "4>5": "fake-out (deceptive)",
  "3>0": "settle home (plagal)",
  "1>4": "the classic setup",
};

export interface NextChord {
  degree: number;
  name: string;
  roman: string;
  pitchClasses: number[];
  why: string;
  /** One of the usual moves from the current chord. */
  common: boolean;
}

/** Which diatonic chord of the key `pitchClasses` is, or null for anything outside it. */
export function degreeOf(pitchClasses: number[], root: number, scale: ScaleType): number | null {
  const i = diatonicChords(root, scale).findIndex((d) => sameSet(d.pitchClasses, pitchClasses));
  return i < 0 ? null : i;
}

/**
 * All seven chords of the key, the usual moves from `fromDegree` first (marked `common`),
 * then the rest in scale order. With no current chord (or one outside the key) it's the key.
 */
export function nextChords(fromDegree: number | null, root: number, scale: ScaleType): NextChord[] {
  const diatonic = diatonicChords(root, scale);
  const order = fromDegree === null ? [] : NEXT[fromDegree];
  const rest = [0, 1, 2, 3, 4, 5, 6].filter((d) => !order.includes(d) && d !== fromDegree);
  return [...order, ...rest].map((d) => ({
    degree: d,
    name: diatonic[d].name,
    roman: diatonic[d].romanNumeral,
    pitchClasses: [...diatonic[d].pitchClasses],
    why: (fromDegree !== null && MOVE[`${fromDegree}>${d}`]) || FEEL[scale][d],
    common: order.includes(d),
  }));
}

// ─── Song length: bars ⇄ min:sec ───────────────────────────────────────────────────────────

/** `seconds` as m:ss (whole seconds, rounded). */
export function formatMinSec(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** "1:05", "1:05.5", "0:30", or plain seconds ("95"). Null for anything else. */
export function parseMinSec(text: string): number | null {
  const t = text.trim();
  const m = /^(\d+):(\d{1,2}(?:\.\d*)?)$/.exec(t);
  if (m) {
    const sec = parseFloat(m[2]);
    return sec < 60 ? parseInt(m[1], 10) * 60 + sec : null;
  }
  return /^\d+(\.\d*)?$/.test(t) ? parseFloat(t) : null;
}

/**
 * The whole number of bars (≥ 1) whose end lands nearest `seconds`, given the song's
 * `secondsAt(beat)` (the tempo map, which only ever increases).
 */
export function barsForSeconds(secondsAt: (beat: number) => number, seconds: number, bpb: number, maxBars = 999): number {
  let lo = 1;
  let hi = maxBars;
  // First bar count whose end is at or past the target…
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (secondsAt(mid * bpb) >= seconds) hi = mid;
    else lo = mid + 1;
  }
  // …or the one before it, if that's closer.
  if (lo > 1 && Math.abs(secondsAt((lo - 1) * bpb) - seconds) <= Math.abs(secondsAt(lo * bpb) - seconds)) return lo - 1;
  return lo;
}

/** Bars as text: whole numbers plain, a meter change's leftover as one decimal. */
export function barsText(beats: number, bpb: number): string {
  const bars = beats / bpb;
  return Math.abs(bars - Math.round(bars)) < 1e-6 ? String(Math.round(bars)) : bars.toFixed(1);
}

/** Beats as a length in words: "2 bars", "1 bar", "3 beats", "1.5 beats". */
export function lengthText(beats: number, bpb: number): string {
  const bars = Math.round(beats / bpb);
  if (bars >= 1 && Math.abs(beats - bars * bpb) < 1e-6) return `${bars} bar${bars === 1 ? "" : "s"}`;
  return `${+beats.toFixed(2)} beat${beats === 1 ? "" : "s"}`;
}
