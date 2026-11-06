// Pure note-editing math for the piano roll. Geometry-free: works in ticks and MIDI pitches.
// The roll previews with these during a drag and commits one `setNotes` on release.

import type { Curve, NoteEvent, NoteProp, PitchPoint } from "./types";
import { MAX_PITCH_POINTS, NOTE_PROPS, noteProp, TICKS_PER_BEAT, uuid } from "./types";
import { remapRopes, repointRopes, tidyRopes, unroped } from "./ropes";

export const MIN_LENGTH_TICKS = TICKS_PER_BEAT / 16;

/** The grid a snap runs on: a whole number of ticks, never below one. */
const gridOf = (gridTicks: number): number => Math.max(1, Math.round(gridTicks));

export const snapFloor = (tick: number, gridTicks: number): number => {
  const g = gridOf(gridTicks);
  return Math.floor(Math.round(tick) / g) * g;
};
export const snapRound = (tick: number, gridTicks: number): number => {
  const g = gridOf(gridTicks);
  return Math.round(Math.round(tick) / g) * g;
};

export interface PitchRange {
  min: number;
  max: number;
}

export function noteAt(notes: NoteEvent[], tick: number, pitch: number): NoteEvent | null {
  for (let i = notes.length - 1; i >= 0; i--) {
    const n = notes[i];
    if (n.pitch === pitch && tick >= n.startTick && tick < n.startTick + n.lengthTicks) return n;
  }
  return null;
}

/** New note at a snapped slot, or null if a same-pitch note already covers it. */
export function placeNote(
  notes: NoteEvent[],
  tick: number,
  pitch: number,
  lengthTicks: number,
  gridTicks: number,
  velocity = 100,
): NoteEvent | null {
  const start = snapFloor(tick, gridTicks);
  const length = Math.round(lengthTicks);
  const overlaps = notes.some(
    (n) => n.pitch === pitch && n.startTick < start + length && start < n.startTick + n.lengthTicks,
  );
  if (overlaps) return null;
  return { id: uuid(), startTick: start, lengthTicks: length, pitch, velocity };
}

/**
 * Move `ids` by a delta, clamping the delta so no moved note leaves the pitch range or starts
 * before 0 (the whole selection shifts together, so relative positions survive).
 */
export function moveNotes(
  notes: NoteEvent[],
  ids: Set<string>,
  deltaTicks: number,
  deltaPitch: number,
  range: PitchRange,
  maxEnd?: number,
): NoteEvent[] {
  const sel = notes.filter((n) => ids.has(n.id));
  if (sel.length === 0) return notes;
  const minStart = Math.min(...sel.map((n) => n.startTick));
  const maxEndSel = Math.max(...sel.map((n) => n.startTick + n.lengthTicks));
  const minPitch = Math.min(...sel.map((n) => n.pitch));
  const maxPitch = Math.max(...sel.map((n) => n.pitch));
  let dt = Math.max(Math.round(deltaTicks), -minStart);
  if (maxEnd !== undefined) dt = Math.min(dt, Math.max(0, Math.round(maxEnd) - maxEndSel));
  const dp = Math.min(Math.max(deltaPitch, range.min - minPitch), range.max - maxPitch);
  if (dt === 0 && dp === 0) return notes;
  return notes.map((n) => (ids.has(n.id) ? { ...n, startTick: n.startTick + dt, pitch: n.pitch + dp } : n));
}

export function resizeNotes(notes: NoteEvent[], ids: Set<string>, deltaTicks: number, minLengthTicks = MIN_LENGTH_TICKS): NoteEvent[] {
  const dt = Math.round(deltaTicks);
  const min = Math.round(minLengthTicks);
  return notes.map((n) => (ids.has(n.id) ? { ...n, lengthTicks: Math.max(min, n.lengthTicks + dt) } : n));
}

export type NoteEdge = "start" | "end";

/**
 * Drag one edge of `ids` by the same delta: the end edge moves the end, the start edge trims
 * the start and leaves the end where it is. Each note keeps at least `minLengthTicks` (or its
 * own length, if it was already shorter, so a shrink never makes a short note longer) and no
 * start goes before 0.
 */
export function resizeEdge(notes: NoteEvent[], ids: Set<string>, edge: NoteEdge, deltaTicks: number, minLengthTicks = MIN_LENGTH_TICKS): NoteEvent[] {
  const dt = Math.round(deltaTicks);
  if (dt === 0) return notes;
  const min = Math.round(minLengthTicks);
  return notes.map((n) => {
    if (!ids.has(n.id)) return n;
    const floor = Math.min(min, n.lengthTicks);
    if (edge === "end") return { ...n, lengthTicks: Math.max(floor, n.lengthTicks + dt) };
    const end = n.startTick + n.lengthTicks;
    const start = Math.min(Math.max(0, n.startTick + dt), end - floor);
    return { ...n, startTick: start, lengthTicks: end - start };
  });
}

/** Plain note values, longest first: whole … 1/64. */
const NOTE_VALUES: [string, number][] = [
  ["1/1", 4 * TICKS_PER_BEAT],
  ["1/2", 2 * TICKS_PER_BEAT],
  ["1/4", TICKS_PER_BEAT],
  ["1/8", TICKS_PER_BEAT / 2],
  ["1/16", TICKS_PER_BEAT / 4],
  ["1/32", TICKS_PER_BEAT / 8],
  ["1/64", TICKS_PER_BEAT / 16],
];
/** What a length under a whole note is summed from, longest first: the plain values and their dotted forms. */
const SUM_TERMS: [string, number][] = NOTE_VALUES.slice(1)
  .flatMap(([label, ticks]): [string, number][] => [[label, ticks], [`${label}.`, ticks * 1.5]])
  .sort((a, b) => b[1] - a[1]);

/**
 * A length in note values, for the roll's drag readout. One value reads as itself ("1/8",
 * dotted "1/4.", triplet "1/8T", spelled the way the grid menu spells them); anything else is
 * a sum of plain and dotted values, longest first ("1/4 + 1/16", "1/2. + 1/8."), with whole
 * notes counted ("2×1/1") and whatever is finer than 1/64 left in ticks.
 */
export function noteValueLabel(lengthTicks: number): string {
  const t = Math.round(lengthTicks);
  if (t <= 0) return "0";
  const single = (v: number): string | null => {
    for (const [label, ticks] of NOTE_VALUES) {
      if (v === ticks) return label;
      if (v * 2 === ticks * 3) return `${label}.`;
      if (v * 3 === ticks * 2) return `${label}T`;
    }
    return null;
  };
  const one = single(t);
  if (one) return one;
  const whole = NOTE_VALUES[0][1];
  const parts: string[] = [];
  let rest = t;
  const wholes = Math.floor(rest / whole);
  if (wholes) {
    parts.push(wholes === 1 ? "1/1" : `${wholes}×1/1`);
    rest -= wholes * whole;
  }
  const tail = rest ? single(rest) : null;
  if (tail) parts.push(tail);
  else {
    for (const [label, ticks] of SUM_TERMS) {
      if (rest >= ticks) {
        parts.push(label);
        rest -= ticks;
      }
    }
    if (rest) parts.push(`${rest}t`);
  }
  return parts.join(" + ");
}

/** Ids of notes overlapping a tick × pitch rectangle (inclusive pitch bounds). */
export function notesInRect(notes: NoteEvent[], tick0: number, tick1: number, pitch0: number, pitch1: number): Set<string> {
  const [t0, t1] = tick0 <= tick1 ? [tick0, tick1] : [tick1, tick0];
  const [p0, p1] = pitch0 <= pitch1 ? [pitch0, pitch1] : [pitch1, pitch0];
  const out = new Set<string>();
  for (const n of notes) {
    if (n.pitch >= p0 && n.pitch <= p1 && n.startTick < t1 && n.startTick + n.lengthTicks > t0) out.add(n.id);
  }
  return out;
}

/**
 * Fresh-id copies of `originals`, each shaped by `place` (null leaves that one out), with the
 * ropes among them pointing at each other's copies: a phrase copied whole slides the same way,
 * and a rope whose source stayed behind is dropped. Every note-copying gesture goes through here.
 */
export function copyNotes(originals: NoteEvent[], place: (n: NoteEvent) => Partial<NoteEvent> | null = () => ({})): NoteEvent[] {
  const copyOf = new Map<string, string>();
  const copies: NoteEvent[] = [];
  for (const n of originals) {
    const at = place(n);
    if (!at) continue;
    const copy = { ...n, ...at, id: uuid() };
    copyOf.set(n.id, copy.id);
    copies.push(copy);
  }
  return remapRopes(copies, copyOf);
}

export function removeNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  return notes.filter((n) => !ids.has(n.id));
}

export function adjustVelocity(notes: NoteEvent[], ids: Set<string>, delta: number): NoteEvent[] {
  return notes.map((n) => (ids.has(n.id) ? { ...n, velocity: Math.min(127, Math.max(1, n.velocity + delta)) } : n));
}

/** Copies of `ids` placed right after the selection, on the grid. Returns new notes + their ids. */
export function duplicateNotes(notes: NoteEvent[], ids: Set<string>, gridTicks: number): { notes: NoteEvent[]; newIds: Set<string> } {
  const sel = notes.filter((n) => ids.has(n.id));
  if (sel.length === 0) return { notes, newIds: new Set() };
  const g = gridOf(gridTicks);
  const start = Math.min(...sel.map((n) => n.startTick));
  const end = Math.max(...sel.map((n) => n.startTick + n.lengthTicks));
  const offset = Math.max(g, Math.ceil((end - start) / g) * g);
  const copies = copyNotes(sel, (n) => ({ startTick: n.startTick + offset }));
  return { notes: [...notes, ...copies], newIds: new Set(copies.map((c) => c.id)) };
}

/**
 * Paste `clipboard` so its earliest note lands at `atTick`. With `maxTick` (the drum grid
 * pastes into a pattern of a fixed length) copies that would start past the end are dropped
 * and the rest are cut to it, so a paste never grows the pattern behind the writer's back.
 */
export function pasteNotes(notes: NoteEvent[], clipboard: NoteEvent[], atTick: number, maxTick?: number): { notes: NoteEvent[]; newIds: Set<string> } {
  if (clipboard.length === 0) return { notes, newIds: new Set() };
  const at = Math.round(atTick);
  const start = Math.min(...clipboard.map((n) => n.startTick));
  const copies = copyNotes(clipboard, (n) => {
    const startTick = n.startTick - start + at;
    if (maxTick !== undefined && startTick >= maxTick) return null;
    const lengthTicks = maxTick === undefined ? n.lengthTicks : Math.min(n.lengthTicks, maxTick - startTick);
    return { startTick, lengthTicks };
  });
  if (!copies.length) return { notes, newIds: new Set() };
  return { notes: [...notes, ...copies], newIds: new Set(copies.map((c) => c.id)) };
}

export function selectionBounds(notes: NoteEvent[], ids: Set<string>): { start: number; end: number } | null {
  const sel = notes.filter((n) => ids.has(n.id));
  if (sel.length === 0) return null;
  return {
    start: Math.min(...sel.map((n) => n.startTick)),
    end: Math.max(...sel.map((n) => n.startTick + n.lengthTicks)),
  };
}

// ─── Transform tools (FL-style, with the harmony twist where it helps) ─────────────────────

export type Rng = () => number;

/** Deterministic PRNG for tests (mulberry32). */
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

function sel(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  return ids.size ? notes.filter((n) => ids.has(n.id)) : notes;
}

/** Snap starts (and lengths) to the grid. `strength` 0..1 blends toward the grid. */
export function quantizeNotes(notes: NoteEvent[], ids: Set<string>, gridTicks: number, strength = 1): NoteEvent[] {
  const g = gridOf(gridTicks);
  const target = new Set(sel(notes, ids).map((n) => n.id));
  return notes.map((n) => {
    if (!target.has(n.id)) return n;
    const snapped = snapRound(n.startTick, g);
    const startTick = Math.round(n.startTick + (snapped - n.startTick) * strength);
    const lengthTicks = Math.max(g, snapRound(n.lengthTicks, g));
    return { ...n, startTick: Math.max(0, startTick), lengthTicks };
  });
}

/**
 * Humanize timing and velocity. Chord tones stay tighter (a third of the timing jitter) so
 * the strong notes keep the groove; tensions and dissonances loosen more. `amount.timing` is
 * the jitter's reach in ticks.
 */
export function humanizeNotes(
  notes: NoteEvent[],
  ids: Set<string>,
  amount: { timing: number; velocity: number },
  tierOf: (n: NoteEvent) => "chordTone" | "tension" | "dissonance" | null,
  rng: Rng = Math.random,
): NoteEvent[] {
  const target = new Set(sel(notes, ids).map((n) => n.id));
  return notes.map((n) => {
    if (!target.has(n.id)) return n;
    const tight = tierOf(n) === "chordTone" ? 0.33 : 1;
    const dt = Math.round((rng() * 2 - 1) * amount.timing * tight);
    const dv = Math.round((rng() * 2 - 1) * amount.velocity);
    return { ...n, startTick: Math.max(0, n.startTick + dt), velocity: Math.min(127, Math.max(1, n.velocity + dv)) };
  });
}

/** Spread stacked notes into a sequence of `gridTicks`-length steps across the selection's span. */
export function arpeggiateNotes(
  notes: NoteEvent[],
  ids: Set<string>,
  gridTicks: number,
  mode: "up" | "down" | "updown" | "random",
  rng: Rng = Math.random,
): NoteEvent[] {
  const s = sel(notes, ids);
  if (s.length < 2) return notes;
  const g = gridOf(gridTicks);
  const start = Math.min(...s.map((n) => n.startTick));
  const end = Math.max(...s.map((n) => n.startTick + n.lengthTicks));
  let order = [...s].sort((a, b) => a.pitch - b.pitch || a.startTick - b.startTick);
  if (mode === "down") order = order.reverse();
  if (mode === "updown" && order.length > 2) order = [...order, ...order.slice(1, -1).reverse()];
  const removed = new Set(s.map((n) => n.id));
  const out = notes.filter((n) => !removed.has(n.id));
  let i = 0;
  for (let t = start; t < end; t += g) {
    const src = mode === "random" ? order[Math.floor(rng() * order.length)] : order[i % order.length];
    // Copied alone, so a step never inherits its source note's rope.
    out.push(...copyNotes([src], () => ({ startTick: t, lengthTicks: Math.min(g, end - t) })));
    i++;
  }
  // Ropes that slid out of the stack had their source replaced; they go too.
  return tidyRopes(out);
}

/** Offset the notes of each stack (same start) by `offsetTicks` per voice, low to high. */
export function strumNotes(notes: NoteEvent[], ids: Set<string>, offsetTicks: number): NoteEvent[] {
  const s = sel(notes, ids);
  const stacks = new Map<number, NoteEvent[]>();
  for (const n of s) {
    const k = n.startTick;
    stacks.set(k, [...(stacks.get(k) ?? []), n]);
  }
  const moved = new Map<string, number>();
  for (const stack of stacks.values()) {
    stack.sort((a, b) => a.pitch - b.pitch).forEach((n, i) => moved.set(n.id, Math.round(i * offsetTicks)));
  }
  return notes.map((n) => {
    const d = moved.get(n.id);
    if (!d) return n;
    return { ...n, startTick: n.startTick + d, lengthTicks: Math.max(MIN_LENGTH_TICKS, n.lengthTicks - d) };
  });
}

/**
 * Time-stretch the selection by `factor` around its first note (2 = half speed, 0.5 = double
 * speed). Starts and lengths scale together; lengths never drop below the grid floor.
 */
export function scaleNotes(notes: NoteEvent[], ids: Set<string>, factor: number, minLengthTicks = MIN_LENGTH_TICKS): NoteEvent[] {
  const target = sel(notes, ids);
  if (!target.length || !(factor > 0) || factor === 1) return notes;
  const min = Math.round(minLengthTicks);
  const origin = Math.min(...target.map((n) => n.startTick));
  const set = new Set(target.map((n) => n.id));
  return notes.map((n) =>
    set.has(n.id)
      ? {
          ...n,
          startTick: Math.round(origin + (n.startTick - origin) * factor),
          lengthTicks: Math.max(min, Math.round(n.lengthTicks * factor)),
        }
      : n,
  );
}

/** Split each note into `gridTicks`-length pieces. */
export function chopNotes(notes: NoteEvent[], ids: Set<string>, gridTicks: number): NoteEvent[] {
  const g = gridOf(gridTicks);
  const target = new Set(sel(notes, ids).map((n) => n.id));
  const out: NoteEvent[] = [];
  const pieces = new Map<string, NoteEvent[]>();
  for (const n of notes) {
    if (!target.has(n.id) || n.lengthTicks <= g) {
      out.push(n);
      continue;
    }
    const end = n.startTick + n.lengthTicks;
    const parts: NoteEvent[] = [];
    for (let t = n.startTick; t < end; t += g) {
      // The first piece keeps the note's id and its rope; the rest start fresh.
      const piece = t === n.startTick ? { ...n, lengthTicks: Math.min(g, end - t) } : unroped({ ...n, id: uuid(), startTick: t, lengthTicks: Math.min(g, end - t) });
      parts.push(piece);
    }
    pieces.set(n.id, parts);
    out.push(...parts);
  }
  return repointRopes(out, pieces);
}

/** Extend each note up to the next note's start (any pitch), FL legato. */
export function legatoNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  const s = sel(notes, ids).sort((a, b) => a.startTick - b.startTick);
  const starts = [...new Set(s.map((n) => n.startTick))].sort((a, b) => a - b);
  const target = new Map(s.map((n) => [n.id, n]));
  return notes.map((n) => {
    if (!target.has(n.id)) return n;
    const next = starts.find((t) => t > n.startTick);
    return next === undefined ? n : { ...n, lengthTicks: Math.max(MIN_LENGTH_TICKS, next - n.startTick) };
  });
}

/** Voice a chord's pitch classes upward from `basePitch` and return the notes to add. */
export function stampChord(notes: NoteEvent[], tick: number, pitchClasses: number[], basePitch: number, lengthTicks: number, gridTicks: number): NoteEvent[] {
  const start = snapFloor(tick, gridTicks);
  const length = Math.round(lengthTicks);
  const base = basePitch - (((basePitch % 12) + 12) % 12);
  const added: NoteEvent[] = [];
  for (const pc of pitchClasses) {
    let pitch = base + (((pc % 12) + 12) % 12);
    if (pitch < basePitch) pitch += 12;
    const clash = notes.some((n) => n.pitch === pitch && n.startTick < start + length && start < n.startTick + n.lengthTicks);
    if (!clash) added.push({ id: uuid(), startTick: start, lengthTicks: length, pitch, velocity: 100 });
  }
  return added;
}

/**
 * The tier magnet: pull a pitch to the nearest chord tone (then tension) within half an
 * octave. Only used when the player turns the magnet on — guidance, never a gate.
 */
export function magnetPitch(pitch: number, tiers: ("chordTone" | "tension" | "dissonance" | null)[]): number {
  const t = (p: number) => tiers[((p % 12) + 12) % 12];
  if (t(pitch) === "chordTone") return pitch;
  for (const want of ["chordTone", "tension"] as const) {
    for (let d = 1; d <= 6; d++) {
      if (t(pitch + d) === want) return pitch + d;
      if (t(pitch - d) === want) return pitch - d;
    }
  }
  return pitch;
}

/** Nearest chord tones below and above a pitch. */
export function resolveTargets(pitch: number, tiers: ("chordTone" | "tension" | "dissonance" | null)[]): { down: number | null; up: number | null } {
  const t = (p: number) => tiers[((p % 12) + 12) % 12];
  let down: number | null = null;
  let up: number | null = null;
  for (let d = 1; d <= 12; d++) {
    if (down === null && t(pitch - d) === "chordTone") down = pitch - d;
    if (up === null && t(pitch + d) === "chordTone") up = pitch + d;
  }
  return { down, up };
}

// ─── Editing commands (roll QOL) ───────────────────────────────────────────────────────────

/** Cut every selected note that straddles `tick` into two at `tick`. */
export function splitNotes(notes: NoteEvent[], ids: Set<string>, tick: number, minLengthTicks = MIN_LENGTH_TICKS): NoteEvent[] {
  const at = Math.round(tick);
  const min = Math.round(minLengthTicks);
  const target = new Set(sel(notes, ids).map((n) => n.id));
  const out: NoteEvent[] = [];
  const pieces = new Map<string, NoteEvent[]>();
  for (const n of notes) {
    const end = n.startTick + n.lengthTicks;
    if (target.has(n.id) && at >= n.startTick + min && at <= end - min) {
      // The left half keeps the id and the rope into it; a rope out of the note moves to
      // whichever half is sounding when its slide note starts.
      const left = { ...n, lengthTicks: at - n.startTick };
      const right = unroped({ ...n, id: uuid(), startTick: at, lengthTicks: end - at });
      pieces.set(n.id, [left, right]);
      out.push(left, right);
    } else {
      out.push(n);
    }
  }
  return repointRopes(out, pieces);
}

/**
 * Join selected same-pitch notes that touch or overlap into one (the first keeps its id and
 * velocity). Notes separated by a gap stay apart.
 */
export function glueNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  const target = sel(notes, ids);
  if (target.length < 2) return notes;
  const targetIds = new Set(target.map((n) => n.id));
  const byPitch = new Map<number, NoteEvent[]>();
  for (const n of target) byPitch.set(n.pitch, [...(byPitch.get(n.pitch) ?? []), n]);
  const merged = new Map<string, NoteEvent>();
  const dropped = new Set<string>();
  for (const group of byPitch.values()) {
    group.sort((a, b) => a.startTick - b.startTick);
    let cur = { ...group[0] };
    for (const n of group.slice(1)) {
      if (n.startTick <= cur.startTick + cur.lengthTicks) {
        cur.lengthTicks = Math.max(cur.lengthTicks, n.startTick + n.lengthTicks - cur.startTick);
        dropped.add(n.id);
      } else {
        merged.set(cur.id, cur);
        cur = { ...n };
      }
    }
    merged.set(cur.id, cur);
  }
  if (dropped.size === 0) return notes;
  const glued = notes.filter((n) => !dropped.has(n.id)).map((n) => (targetIds.has(n.id) ? (merged.get(n.id) ?? n) : n));
  // A rope out of a swallowed note now comes out of the note that swallowed it; one that ran
  // between two glued notes would be a note sliding from itself, and is cut.
  const into = new Map<string, NoteEvent[]>();
  for (const group of byPitch.values()) {
    let keeper = group[0];
    for (const n of group) {
      if (merged.has(n.id)) keeper = merged.get(n.id)!;
      else if (dropped.has(n.id)) into.set(n.id, [keeper]);
    }
  }
  return tidyRopes(repointRopes(glued, into));
}

/** Copy the selection one `spanTicks` later (⌘B: the pattern length, FL's favourite). */
export function duplicateBySpan(notes: NoteEvent[], ids: Set<string>, spanTicks: number): { notes: NoteEvent[]; newIds: Set<string> } {
  const target = sel(notes, ids);
  const span = Math.round(spanTicks);
  if (!target.length || !(span > 0)) return { notes, newIds: new Set() };
  const copies = copyNotes(target, (n) => ({ startTick: n.startTick + span }));
  return { notes: [...notes, ...copies], newIds: new Set(copies.map((c) => c.id)) };
}

/** Reverse the selection in time within its own span (ends become starts). */
export function reverseNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  const target = sel(notes, ids);
  if (target.length < 2) return notes;
  const set = new Set(target.map((n) => n.id));
  const start = Math.min(...target.map((n) => n.startTick));
  const end = Math.max(...target.map((n) => n.startTick + n.lengthTicks));
  return notes.map((n) => (set.has(n.id) ? { ...n, startTick: start + (end - (n.startTick + n.lengthTicks)) } : n));
}

/** Mirror the selection's pitches around its middle (highest ↔ lowest). */
export function invertNotes(notes: NoteEvent[], ids: Set<string>, range: PitchRange): NoteEvent[] {
  const target = sel(notes, ids);
  if (target.length < 2) return notes;
  const set = new Set(target.map((n) => n.id));
  const lo = Math.min(...target.map((n) => n.pitch));
  const hi = Math.max(...target.map((n) => n.pitch));
  return notes.map((n) => (set.has(n.id) ? { ...n, pitch: Math.min(range.max, Math.max(range.min, lo + hi - n.pitch)) } : n));
}

/** Give every selected note the same length. */
export function setLength(notes: NoteEvent[], ids: Set<string>, lengthTicks: number): NoteEvent[] {
  const length = Math.max(MIN_LENGTH_TICKS, Math.round(lengthTicks));
  const target = new Set(sel(notes, ids).map((n) => n.id));
  return notes.map((n) => (target.has(n.id) ? { ...n, lengthTicks: length } : n));
}

/** Toggle the mute flag on the selection (all unmuted if any is muted → mute all, else unmute). */
export function toggleMute(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  const target = sel(notes, ids);
  if (!target.length) return notes;
  const set = new Set(target.map((n) => n.id));
  const mute = target.some((n) => !n.muted);
  return notes.map((n) => (set.has(n.id) ? { ...n, muted: mute } : n));
}

/**
 * Velocity ramp: a straight line from (`t0`, `v0`) to (`t1`, `v1`) in tick × velocity space;
 * every note (of `ids`, or all) whose start lies between the two ticks takes the line's value.
 */
export function rampVelocity(notes: NoteEvent[], ids: Set<string>, t0: number, v0: number, t1: number, v1: number): NoteEvent[] {
  const [from, to] = t0 <= t1 ? [t0, t1] : [t1, t0];
  const [vf, vt] = t0 <= t1 ? [v0, v1] : [v1, v0];
  const target = new Set(sel(notes, ids).map((n) => n.id));
  const span = Math.max(1, to - from);
  return notes.map((n) => {
    if (!target.has(n.id) || n.startTick < from || n.startTick > to) return n;
    const t = (n.startTick - from) / span;
    return { ...n, velocity: Math.round(Math.min(127, Math.max(1, vf + (vt - vf) * t))) };
  });
}

/** Ids of the next / previous note in time order after the selection (or the first note). */
export function stepSelection(notes: NoteEvent[], ids: Set<string>, dir: 1 | -1): string | null {
  if (!notes.length) return null;
  const ordered = [...notes].sort((a, b) => a.startTick - b.startTick || a.pitch - b.pitch);
  if (!ids.size) return (dir === 1 ? ordered[0] : ordered[ordered.length - 1]).id;
  const idx = ordered.map((n) => n.id);
  const anchor = dir === 1 ? Math.max(...[...ids].map((id) => idx.indexOf(id))) : Math.min(...[...ids].map((id) => idx.indexOf(id)));
  const next = (anchor + dir + ordered.length) % ordered.length;
  return ordered[next].id;
}

// ─── Note properties (FL's piano roll property lanes = its step graph editor) ───────────────

/** Props the model stores as integers: velocity i32, fine i32 cents, repeats u8, shiftTicks i64. */
const INTEGER_PROPS: Partial<Record<NoteProp, true>> = { velocity: true, fine: true, repeats: true, shiftTicks: true };

/**
 * A value landing on `prop`'s own grid: clamped to its range, then rounded for the
 * integer-backed props. Every write below goes through this, and lane gestures call it on the
 * endpoints of a ramp so the interpolated interior matches the values the ends actually take.
 */
export function quantizeNoteProp(prop: NoteProp, value: number): number {
  const spec = NOTE_PROPS[prop];
  const v = Math.min(spec.max, Math.max(spec.min, value));
  return INTEGER_PROPS[prop] ? Math.round(v) : v;
}

function writeProp(n: NoteEvent, prop: NoteProp, value: number): NoteEvent {
  const next: NoteEvent = { ...n };
  next[prop] = value;
  return next;
}

/**
 * Write `value` to `prop` on every note of `ids`. An empty `ids` is a no-op (these run from a
 * lane gesture, which always names its notes — unlike the `sel()` tools, where empty means all).
 * A note already holding the clamped value is left as the same object, so a note that never
 * left the neutral default keeps the property unset and stays byte-identical when saved.
 */
export function setNoteProp(notes: NoteEvent[], ids: Set<string>, prop: NoteProp, value: number): NoteEvent[] {
  if (!ids.size) return notes;
  const v = quantizeNoteProp(prop, value);
  let changed = false;
  const out = notes.map((n) => {
    if (!ids.has(n.id) || noteProp(n, prop) === v) return n;
    changed = true;
    return writeProp(n, prop, v);
  });
  return changed ? out : notes;
}

/** Add `delta` to `prop` on every note of `ids`, each from its own current value. */
export function nudgeNoteProp(notes: NoteEvent[], ids: Set<string>, prop: NoteProp, delta: number): NoteEvent[] {
  if (!ids.size || !delta) return notes;
  let changed = false;
  const out = notes.map((n) => {
    if (!ids.has(n.id)) return n;
    const v = quantizeNoteProp(prop, noteProp(n, prop) + delta);
    if (noteProp(n, prop) === v) return n;
    changed = true;
    return writeProp(n, prop, v);
  });
  return changed ? out : notes;
}

/**
 * A straight line from `from` to `to` across the selection's own span, in tick × property
 * space (the same line `rampVelocity` draws): the earliest selected note takes `from`, the
 * latest takes `to`, and notes between take their time-proportional share. A single note —
 * or a stack that all starts on one tick — takes `to`.
 */
export function rampNoteProp(notes: NoteEvent[], ids: Set<string>, prop: NoteProp, from: number, to: number): NoteEvent[] {
  if (!ids.size) return notes;
  const starts = notes.filter((n) => ids.has(n.id)).map((n) => n.startTick);
  if (!starts.length) return notes;
  const lo = Math.min(...starts);
  const span = Math.max(...starts) - lo;
  let changed = false;
  const out = notes.map((n) => {
    if (!ids.has(n.id)) return n;
    const t = span > 0 ? (n.startTick - lo) / span : 1;
    const v = quantizeNoteProp(prop, from + (to - from) * t);
    if (noteProp(n, prop) === v) return n;
    changed = true;
    return writeProp(n, prop, v);
  });
  return changed ? out : notes;
}

// ─── Pitch envelope (the bend contour drawn inside a note's body) ───────────────────────────

/** Cents are bounded by the same range `fine` is: the envelope is an offset in its units. */
const MIN_CENTS = NOTE_PROPS.fine.min;
const MAX_CENTS = NOTE_PROPS.fine.max;

/**
 * Sort by tick, clamp into range and keep one point per tick — the later one, which is the
 * one the writer just dragged there. Mirrors `dissonant_core::model::tidy_pitch_envelope`
 * apart from the over-the-cap case: the roll refuses the seventeenth point rather than
 * decimating, since a person adding points one at a time should be told no, not have an
 * earlier one quietly rewritten.
 */
export function tidyPitchEnvelope(points: PitchPoint[]): PitchPoint[] {
  const out: PitchPoint[] = [];
  for (const p of [...points].sort((a, b) => a.tick - b.tick)) {
    if (!Number.isFinite(p.cents) || p.tick < 0) continue;
    const point = { tick: Math.round(p.tick), cents: Math.min(MAX_CENTS, Math.max(MIN_CENTS, p.cents)), curve: p.curve };
    if (out.length && out[out.length - 1].tick === point.tick) out[out.length - 1] = point;
    else out.push(point);
  }
  return out;
}

/**
 * The cents in force `tick` after the note's start. Mirrors the voice's reading rule: zero
 * before the first point (an envelope offsets a note, it does not override a control, so a
 * note that has not bent yet is in tune), a linear glide into a `ramp` point, a hold then a
 * jump at a `step` point, and the last value from then on.
 */
export function pitchCentsAt(points: PitchPoint[], tick: number): number {
  let i = 0;
  while (i < points.length && points[i].tick <= tick) i++;
  const prevTick = i === 0 ? 0 : points[i - 1].tick;
  const prev = i === 0 ? 0 : points[i - 1].cents;
  const next = points[i];
  if (next && next.curve === "ramp" && next.tick > prevTick) {
    return prev + (next.cents - prev) * ((tick - prevTick) / (next.tick - prevTick));
  }
  return prev;
}

/**
 * The contour as a polyline in (tick, cents), ready to stroke across the note's body: it
 * opens at the note's own pitch, carries a step point's corner as two vertices so the drawn
 * line matches what is heard, and runs flat to `lengthTicks`.
 */
export function pitchPolyline(points: PitchPoint[], lengthTicks: number): { tick: number; cents: number }[] {
  if (!points.length) return [];
  const out: { tick: number; cents: number }[] = [{ tick: 0, cents: points[0].tick === 0 ? points[0].cents : 0 }];
  let prev = out[0].cents;
  for (const p of points) {
    if (p.tick === 0) continue;
    if (p.curve === "step") out.push({ tick: p.tick, cents: prev });
    out.push({ tick: p.tick, cents: p.cents });
    prev = p.cents;
  }
  const end = points[points.length - 1].tick;
  if (end < lengthTicks) out.push({ tick: lengthTicks, cents: prev });
  return out;
}

/** Replace one note's envelope, tidied. Notes without the id are returned untouched. */
export function setPitchEnvelope(notes: NoteEvent[], id: string, points: PitchPoint[]): NoteEvent[] {
  const tidy = tidyPitchEnvelope(points);
  return notes.map((n) => (n.id === id ? { ...n, pitchEnvelope: tidy } : n));
}

/**
 * Add a point to a note's contour. A point on a tick that already carries one replaces it,
 * and the cap is a refusal rather than a silent eviction: the notes come back unchanged.
 */
export function addPitchPoint(notes: NoteEvent[], id: string, tick: number, cents: number, curve: Curve = "ramp"): NoteEvent[] {
  const note = notes.find((n) => n.id === id);
  if (!note) return notes;
  const points = note.pitchEnvelope ?? [];
  const t = Math.round(Math.max(0, Math.min(note.lengthTicks, tick)));
  if (points.length >= MAX_PITCH_POINTS && !points.some((p) => p.tick === t)) return notes;
  return setPitchEnvelope(notes, id, [...points, { tick: t, cents, curve }]);
}

/**
 * Move the point at `index` to a new tick and cents. The tick stays inside the note (a point
 * past its end would be unreachable), and dragging one point past another reorders rather
 * than refusing — a drag is a gesture, not a constraint solver.
 */
export function movePitchPoint(notes: NoteEvent[], id: string, index: number, tick: number, cents: number): NoteEvent[] {
  const note = notes.find((n) => n.id === id);
  if (!note) return notes;
  const points = note.pitchEnvelope ?? [];
  if (index < 0 || index >= points.length) return notes;
  const t = Math.round(Math.max(0, Math.min(note.lengthTicks, tick)));
  return setPitchEnvelope(
    notes,
    id,
    points.map((p, i) => (i === index ? { ...p, tick: t, cents } : p)),
  );
}

export function removePitchPoint(notes: NoteEvent[], id: string, index: number): NoteEvent[] {
  const note = notes.find((n) => n.id === id);
  if (!note) return notes;
  const points = note.pitchEnvelope ?? [];
  if (index < 0 || index >= points.length) return notes;
  return setPitchEnvelope(
    notes,
    id,
    points.filter((_, i) => i !== index),
  );
}

/** Flip a point between gliding into its value and jumping to it (the lane's ⌥-click). */
export function togglePitchCurve(notes: NoteEvent[], id: string, index: number): NoteEvent[] {
  const note = notes.find((n) => n.id === id);
  if (!note) return notes;
  const points = note.pitchEnvelope ?? [];
  if (index < 0 || index >= points.length) return notes;
  return setPitchEnvelope(
    notes,
    id,
    points.map((p, i) => (i === index ? { ...p, curve: p.curve === "ramp" ? ("step" as const) : ("ramp" as const) } : p)),
  );
}

/**
 * The point under a pointer, as an index into the note's envelope, or null. Distances are in
 * the roll's own pixels, which is why the caller passes both scales: a handle is a fixed size
 * on screen however far the view is zoomed.
 */
export function pitchPointAt(
  note: NoteEvent,
  tick: number,
  cents: number,
  scale: { pxPerTick: number; pxPerCent: number },
  radiusPx: number,
): number | null {
  let best: number | null = null;
  let bestDistance = radiusPx;
  (note.pitchEnvelope ?? []).forEach((p, i) => {
    const d = Math.hypot((p.tick - tick) * scale.pxPerTick, (p.cents - cents) * scale.pxPerCent);
    if (d <= bestDistance) {
      best = i;
      bestDistance = d;
    }
  });
  return best;
}

// ─── Step rows (FL's step sequencer: a step is a note, so these are note edits) ─────────────

/** One drum row inside one turn of the track's loop: `steps` cells of `stepTicks` at `pitch`. */
export interface StepGrid {
  pitch: number;
  stepTicks: number;
  steps: number;
}

/** The step a note sits in, or null when it is another pitch, off-step, or past the turn. */
export function stepIndexOf(note: NoteEvent, grid: StepGrid): number | null {
  if (note.pitch !== grid.pitch) return null;
  const i = Math.round(note.startTick / grid.stepTicks);
  if (i < 0 || i >= grid.steps) return null;
  return note.startTick === i * grid.stepTicks ? i : null;
}

/** This row's current hit per step (a later duplicate wins). */
function rowHits(notes: NoteEvent[], grid: StepGrid): (NoteEvent | undefined)[] {
  const row: (NoteEvent | undefined)[] = new Array(grid.steps);
  for (const n of notes) {
    const i = stepIndexOf(n, grid);
    if (i !== null) row[i] = n;
  }
  return row;
}

function newHit(grid: StepGrid, velocity: number): NoteEvent {
  return {
    id: uuid(),
    startTick: 0,
    lengthTicks: grid.stepTicks,
    pitch: grid.pitch,
    velocity: quantizeNoteProp("velocity", velocity),
  };
}

/**
 * Swap in a whole row: `row[i]` is the note that should sound in step `i`, carrying its own
 * properties with it — the step only decides the timing. Everything else (other pitches, and
 * this pitch off-step or past the turn) keeps its place and order; the row follows in step order.
 */
function replaceRow(notes: NoteEvent[], grid: StepGrid, row: (NoteEvent | undefined)[]): NoteEvent[] {
  const out = notes.filter((n) => stepIndexOf(n, grid) === null);
  for (let i = 0; i < grid.steps; i++) {
    const hit = row[i];
    if (!hit) continue;
    const startTick = i * grid.stepTicks;
    const placed = hit.startTick === startTick && hit.lengthTicks === grid.stepTicks ? hit : { ...hit, startTick, lengthTicks: grid.stepTicks };
    out.push(placed);
  }
  return out;
}

/** FL's "Fill each N steps": one hit on every `every`-th step from `offset`, and nothing else. */
export function fillEverySteps(notes: NoteEvent[], grid: StepGrid, every: number, offset: number, velocity: number): NoteEvent[] {
  const stride = Math.max(1, Math.round(every));
  const existing = rowHits(notes, grid);
  const row: (NoteEvent | undefined)[] = new Array(grid.steps);
  for (let i = Math.max(0, Math.round(offset)); i < grid.steps; i += stride) row[i] = existing[i] ?? newHit(grid, velocity);
  return replaceRow(notes, grid, row);
}

/** Empty the row inside the turn; every other pitch stays. */
export function clearRow(notes: NoteEvent[], grid: StepGrid): NoteEvent[] {
  if (!notes.some((n) => stepIndexOf(n, grid) !== null)) return notes;
  return replaceRow(notes, grid, []);
}

/** FL's shift left/right: rotate the row by `by` steps (positive = later), wrapping in the turn. */
export function rotateRow(notes: NoteEvent[], grid: StepGrid, by: number): NoteEvent[] {
  const shift = ((Math.round(by) % grid.steps) + grid.steps) % grid.steps;
  if (!shift) return notes;
  const existing = rowHits(notes, grid);
  const row: (NoteEvent | undefined)[] = new Array(grid.steps);
  for (let i = 0; i < grid.steps; i++) if (existing[i]) row[(i + shift) % grid.steps] = existing[i];
  return replaceRow(notes, grid, row);
}

/** A hit per step with probability `chance`; `rng` is injected so `seededRng` makes it repeatable. */
export function randomizeRow(notes: NoteEvent[], grid: StepGrid, chance: number, velocity: number, rng: Rng): NoteEvent[] {
  const existing = rowHits(notes, grid);
  const row: (NoteEvent | undefined)[] = new Array(grid.steps);
  for (let i = 0; i < grid.steps; i++) if (rng() < chance) row[i] = existing[i] ?? newHit(grid, velocity);
  return replaceRow(notes, grid, row);
}

/**
 * Euclidean row: `onsets` hits spread as evenly as the step count allows.
 *
 * Lifted from the `Euclidean.pyscript` that ships with FL Studio 2026, whose Bresenham loop is
 *     slope = onsets / steps; previous = -1
 *     for i in range(steps): current = int(i * slope); out[i] = (current != previous); previous = current
 * followed by `rotate(out, -offset)` with `rotate(l, s) = l[s:] + l[:s]` — a left rotation, so
 * `rotation` here shifts the pattern earlier. 5-in-16 comes out as 0, 4, 7, 10, 13.
 */
export function euclideanRow(notes: NoteEvent[], grid: StepGrid, onsets: number, rotation: number, velocity: number): NoteEvent[] {
  const k = Math.min(grid.steps, Math.max(0, Math.round(onsets)));
  const existing = rowHits(notes, grid);
  const row: (NoteEvent | undefined)[] = new Array(grid.steps);
  if (k > 0) {
    const slope = k / grid.steps;
    const pattern: boolean[] = new Array(grid.steps);
    let previous = -1;
    for (let i = 0; i < grid.steps; i++) {
      const current = Math.trunc(i * slope);
      pattern[i] = current !== previous;
      previous = current;
    }
    const turn = ((Math.round(rotation) % grid.steps) + grid.steps) % grid.steps;
    for (let i = 0; i < grid.steps; i++) if (pattern[(i + turn) % grid.steps]) row[i] = existing[i] ?? newHit(grid, velocity);
  }
  return replaceRow(notes, grid, row);
}

// ─── Step selection (the drum grid's marquee, clipboard and nudges) ─────────────────────────

/**
 * Ids of the hits inside a step × row rectangle: those on `pitches` (the kit rows a marquee
 * covered — a kit is a list, not a scale, so its rows are not contiguous in pitch) that start
 * within [`step0`, `step1`]. A hit belongs to the step it starts in, so one leaning early by
 * its shift is still caught by the cell it lives in.
 */
export function hitsInSteps(notes: NoteEvent[], pitches: Iterable<number>, stepTicks: number, step0: number, step1: number): Set<string> {
  const g = gridOf(stepTicks);
  const rows = new Set(pitches);
  const [a, b] = step0 <= step1 ? [step0, step1] : [step1, step0];
  const out = new Set<string>();
  for (const n of notes) {
    if (!rows.has(n.pitch)) continue;
    const step = Math.floor(n.startTick / g);
    if (step >= a && step <= b) out.add(n.id);
  }
  return out;
}

/**
 * Move the selection `delta` rows along `pitches` (the kit's own order). The delta is clamped
 * so no hit falls off either end, which keeps the shape of a selection spanning several pads;
 * hits on pitches the kit has no row for stay where they are.
 */
export function moveHitsByRow(notes: NoteEvent[], ids: Set<string>, pitches: number[], delta: number): NoteEvent[] {
  const d = Math.round(delta);
  if (!d || ids.size === 0 || pitches.length === 0) return notes;
  const rows = new Map(pitches.map((p, i) => [p, i]));
  const rowsUsed: number[] = [];
  for (const n of notes) {
    if (!ids.has(n.id)) continue;
    const i = rows.get(n.pitch);
    if (i !== undefined) rowsUsed.push(i);
  }
  if (!rowsUsed.length) return notes;
  const move = Math.max(-Math.min(...rowsUsed), Math.min(pitches.length - 1 - Math.max(...rowsUsed), d));
  if (!move) return notes;
  return notes.map((n) => {
    const i = rows.get(n.pitch);
    return ids.has(n.id) && i !== undefined ? { ...n, pitch: pitches[i + move] } : n;
  });
}

/**
 * A ghost pattern's notes laid under a pattern `lengthTicks` long: looped every
 * `ghostLengthTicks` the way the song would loop it, cut at the end, muted notes left out
 * (they aren't heard either). Ids get a `~<loop>` suffix so they stay unique per pass.
 */
export function tileGhost(ghost: NoteEvent[], ghostLengthTicks: number, lengthTicks: number): NoteEvent[] {
  const step = ghostLengthTicks > 0 ? ghostLengthTicks : lengthTicks;
  const out: NoteEvent[] = [];
  for (let loop = 0, off = 0; off < lengthTicks; loop++, off += step) {
    for (const n of ghost) {
      const start = n.startTick + off;
      if (n.muted || n.startTick >= step || start >= lengthTicks) continue;
      const tile: NoteEvent = { ...n, id: `${n.id}~${loop}`, startTick: start, lengthTicks: Math.min(n.lengthTicks, lengthTicks - start) };
      // Same suffix, so a ghost rope points at its source's tile in the same loop.
      if (n.slideFrom) tile.slideFrom = `${n.slideFrom}~${loop}`;
      out.push(tile);
    }
  }
  return out;
}
