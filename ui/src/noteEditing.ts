// Pure note-editing math for the piano roll. Geometry-free: works in beats and MIDI pitches.
// The roll previews with these during a drag and commits one `setNotes` on release.

import type { NoteEvent } from "./types";
import { uuid } from "./types";

export const MIN_LENGTH = 1 / 16;

export const snapFloor = (beat: number, grid: number): number => Math.floor(beat / grid + 1e-9) * grid;
export const snapRound = (beat: number, grid: number): number => Math.round(beat / grid) * grid;

export interface PitchRange {
  min: number;
  max: number;
}

export function noteAt(notes: NoteEvent[], beat: number, pitch: number): NoteEvent | null {
  for (let i = notes.length - 1; i >= 0; i--) {
    const n = notes[i];
    if (n.pitch === pitch && beat >= n.startBeat && beat < n.startBeat + n.lengthBeats) return n;
  }
  return null;
}

/** New note at a snapped slot, or null if a same-pitch note already covers it. */
export function placeNote(
  notes: NoteEvent[],
  beat: number,
  pitch: number,
  length: number,
  grid: number,
  velocity = 100,
): NoteEvent | null {
  const start = snapFloor(beat, grid);
  const overlaps = notes.some(
    (n) => n.pitch === pitch && n.startBeat < start + length && start < n.startBeat + n.lengthBeats,
  );
  if (overlaps) return null;
  return { id: uuid(), startBeat: start, lengthBeats: length, pitch, velocity };
}

/**
 * Move `ids` by a delta, clamping the delta so no moved note leaves the pitch range or starts
 * before 0 (the whole selection shifts together, so relative positions survive).
 */
export function moveNotes(
  notes: NoteEvent[],
  ids: Set<string>,
  deltaBeats: number,
  deltaPitch: number,
  range: PitchRange,
  maxEnd?: number,
): NoteEvent[] {
  const sel = notes.filter((n) => ids.has(n.id));
  if (sel.length === 0) return notes;
  const minStart = Math.min(...sel.map((n) => n.startBeat));
  const maxEndSel = Math.max(...sel.map((n) => n.startBeat + n.lengthBeats));
  const minPitch = Math.min(...sel.map((n) => n.pitch));
  const maxPitch = Math.max(...sel.map((n) => n.pitch));
  let db = Math.max(deltaBeats, -minStart);
  if (maxEnd !== undefined) db = Math.min(db, Math.max(0, maxEnd - maxEndSel));
  const dp = Math.min(Math.max(deltaPitch, range.min - minPitch), range.max - maxPitch);
  if (db === 0 && dp === 0) return notes;
  return notes.map((n) => (ids.has(n.id) ? { ...n, startBeat: n.startBeat + db, pitch: n.pitch + dp } : n));
}

export function resizeNotes(notes: NoteEvent[], ids: Set<string>, deltaBeats: number, minLength = MIN_LENGTH): NoteEvent[] {
  return notes.map((n) => (ids.has(n.id) ? { ...n, lengthBeats: Math.max(minLength, n.lengthBeats + deltaBeats) } : n));
}

/** Ids of notes overlapping a beat × pitch rectangle (inclusive pitch bounds). */
export function notesInRect(notes: NoteEvent[], beat0: number, beat1: number, pitch0: number, pitch1: number): Set<string> {
  const [b0, b1] = beat0 <= beat1 ? [beat0, beat1] : [beat1, beat0];
  const [p0, p1] = pitch0 <= pitch1 ? [pitch0, pitch1] : [pitch1, pitch0];
  const out = new Set<string>();
  for (const n of notes) {
    if (n.pitch >= p0 && n.pitch <= p1 && n.startBeat < b1 && n.startBeat + n.lengthBeats > b0) out.add(n.id);
  }
  return out;
}

export function removeNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  return notes.filter((n) => !ids.has(n.id));
}

export function adjustVelocity(notes: NoteEvent[], ids: Set<string>, delta: number): NoteEvent[] {
  return notes.map((n) => (ids.has(n.id) ? { ...n, velocity: Math.min(127, Math.max(1, n.velocity + delta)) } : n));
}

/** Copies of `ids` placed right after the selection, on the grid. Returns new notes + their ids. */
export function duplicateNotes(notes: NoteEvent[], ids: Set<string>, grid: number): { notes: NoteEvent[]; newIds: Set<string> } {
  const sel = notes.filter((n) => ids.has(n.id));
  if (sel.length === 0) return { notes, newIds: new Set() };
  const start = Math.min(...sel.map((n) => n.startBeat));
  const end = Math.max(...sel.map((n) => n.startBeat + n.lengthBeats));
  const offset = Math.max(grid, Math.ceil((end - start) / grid - 1e-9) * grid);
  const copies = sel.map((n) => ({ ...n, id: uuid(), startBeat: n.startBeat + offset }));
  return { notes: [...notes, ...copies], newIds: new Set(copies.map((c) => c.id)) };
}

/** Paste `clipboard` so its earliest note lands at `atBeat`. */
export function pasteNotes(notes: NoteEvent[], clipboard: NoteEvent[], atBeat: number): { notes: NoteEvent[]; newIds: Set<string> } {
  if (clipboard.length === 0) return { notes, newIds: new Set() };
  const start = Math.min(...clipboard.map((n) => n.startBeat));
  const copies = clipboard.map((n) => ({ ...n, id: uuid(), startBeat: n.startBeat - start + atBeat }));
  return { notes: [...notes, ...copies], newIds: new Set(copies.map((c) => c.id)) };
}

export function selectionBounds(notes: NoteEvent[], ids: Set<string>): { start: number; end: number } | null {
  const sel = notes.filter((n) => ids.has(n.id));
  if (sel.length === 0) return null;
  return {
    start: Math.min(...sel.map((n) => n.startBeat)),
    end: Math.max(...sel.map((n) => n.startBeat + n.lengthBeats)),
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
export function quantizeNotes(notes: NoteEvent[], ids: Set<string>, grid: number, strength = 1): NoteEvent[] {
  const target = new Set(sel(notes, ids).map((n) => n.id));
  return notes.map((n) => {
    if (!target.has(n.id)) return n;
    const snapped = snapRound(n.startBeat, grid);
    const startBeat = n.startBeat + (snapped - n.startBeat) * strength;
    const lengthBeats = Math.max(grid, snapRound(n.lengthBeats, grid));
    return { ...n, startBeat: Math.max(0, startBeat), lengthBeats };
  });
}

/**
 * Humanize timing and velocity. Chord tones stay tighter (a third of the timing jitter) so
 * the strong notes keep the groove; tensions and dissonances loosen more.
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
    const dt = (rng() * 2 - 1) * amount.timing * tight;
    const dv = Math.round((rng() * 2 - 1) * amount.velocity);
    return { ...n, startBeat: Math.max(0, n.startBeat + dt), velocity: Math.min(127, Math.max(1, n.velocity + dv)) };
  });
}

/** Spread stacked notes into a sequence of `grid`-length steps across the selection's span. */
export function arpeggiateNotes(
  notes: NoteEvent[],
  ids: Set<string>,
  grid: number,
  mode: "up" | "down" | "updown" | "random",
  rng: Rng = Math.random,
): NoteEvent[] {
  const s = sel(notes, ids);
  if (s.length < 2) return notes;
  const start = Math.min(...s.map((n) => n.startBeat));
  const end = Math.max(...s.map((n) => n.startBeat + n.lengthBeats));
  let order = [...s].sort((a, b) => a.pitch - b.pitch || a.startBeat - b.startBeat);
  if (mode === "down") order = order.reverse();
  if (mode === "updown" && order.length > 2) order = [...order, ...order.slice(1, -1).reverse()];
  const removed = new Set(s.map((n) => n.id));
  const out = notes.filter((n) => !removed.has(n.id));
  let i = 0;
  for (let t = start; t < end - 1e-9; t += grid) {
    const src = mode === "random" ? order[Math.floor(rng() * order.length)] : order[i % order.length];
    out.push({ ...src, id: uuid(), startBeat: t, lengthBeats: Math.min(grid, end - t) });
    i++;
  }
  return out;
}

/** Offset the notes of each stack (same start) by `offset` per voice, low to high. */
export function strumNotes(notes: NoteEvent[], ids: Set<string>, offset: number): NoteEvent[] {
  const s = sel(notes, ids);
  const stacks = new Map<number, NoteEvent[]>();
  for (const n of s) {
    const k = Math.round(n.startBeat * 1000);
    stacks.set(k, [...(stacks.get(k) ?? []), n]);
  }
  const moved = new Map<string, number>();
  for (const stack of stacks.values()) {
    stack.sort((a, b) => a.pitch - b.pitch).forEach((n, i) => moved.set(n.id, i * offset));
  }
  return notes.map((n) => {
    const d = moved.get(n.id);
    if (!d) return n;
    return { ...n, startBeat: n.startBeat + d, lengthBeats: Math.max(MIN_LENGTH, n.lengthBeats - d) };
  });
}

/**
 * Time-stretch the selection by `factor` around its first note (2 = half speed, 0.5 = double
 * speed). Starts and lengths scale together; lengths never drop below the grid floor.
 */
export function scaleNotes(notes: NoteEvent[], ids: Set<string>, factor: number, minLength = MIN_LENGTH): NoteEvent[] {
  const target = sel(notes, ids);
  if (!target.length || !(factor > 0) || factor === 1) return notes;
  const origin = Math.min(...target.map((n) => n.startBeat));
  const set = new Set(target.map((n) => n.id));
  return notes.map((n) =>
    set.has(n.id) ? { ...n, startBeat: origin + (n.startBeat - origin) * factor, lengthBeats: Math.max(minLength, n.lengthBeats * factor) } : n,
  );
}

/** Split each note into `grid`-length pieces. */
export function chopNotes(notes: NoteEvent[], ids: Set<string>, grid: number): NoteEvent[] {
  const target = new Set(sel(notes, ids).map((n) => n.id));
  const out: NoteEvent[] = [];
  for (const n of notes) {
    if (!target.has(n.id) || n.lengthBeats <= grid + 1e-9) {
      out.push(n);
      continue;
    }
    const end = n.startBeat + n.lengthBeats;
    for (let t = n.startBeat; t < end - 1e-9; t += grid) {
      out.push({ ...n, id: t === n.startBeat ? n.id : uuid(), startBeat: t, lengthBeats: Math.min(grid, end - t) });
    }
  }
  return out;
}

/** Extend each note up to the next note's start (any pitch), FL legato. */
export function legatoNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  const s = sel(notes, ids).sort((a, b) => a.startBeat - b.startBeat);
  const starts = [...new Set(s.map((n) => n.startBeat))].sort((a, b) => a - b);
  const target = new Map(s.map((n) => [n.id, n]));
  return notes.map((n) => {
    if (!target.has(n.id)) return n;
    const next = starts.find((t) => t > n.startBeat + 1e-9);
    return next === undefined ? n : { ...n, lengthBeats: Math.max(MIN_LENGTH, next - n.startBeat) };
  });
}

/** Voice a chord's pitch classes upward from `basePitch` and return the notes to add. */
export function stampChord(notes: NoteEvent[], beat: number, pitchClasses: number[], basePitch: number, length: number, grid: number): NoteEvent[] {
  const start = snapFloor(beat, grid);
  const base = basePitch - (((basePitch % 12) + 12) % 12);
  const added: NoteEvent[] = [];
  for (const pc of pitchClasses) {
    let pitch = base + (((pc % 12) + 12) % 12);
    if (pitch < basePitch) pitch += 12;
    const clash = notes.some((n) => n.pitch === pitch && n.startBeat < start + length && start < n.startBeat + n.lengthBeats);
    if (!clash) added.push({ id: uuid(), startBeat: start, lengthBeats: length, pitch, velocity: 100 });
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

/** Cut every selected note that straddles `beat` into two at `beat`. */
export function splitNotes(notes: NoteEvent[], ids: Set<string>, beat: number, minLength = MIN_LENGTH): NoteEvent[] {
  const target = new Set(sel(notes, ids).map((n) => n.id));
  const out: NoteEvent[] = [];
  for (const n of notes) {
    const end = n.startBeat + n.lengthBeats;
    if (target.has(n.id) && beat > n.startBeat + minLength - 1e-9 && beat < end - minLength + 1e-9) {
      out.push({ ...n, lengthBeats: beat - n.startBeat });
      out.push({ ...n, id: uuid(), startBeat: beat, lengthBeats: end - beat });
    } else {
      out.push(n);
    }
  }
  return out;
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
    group.sort((a, b) => a.startBeat - b.startBeat);
    let cur = { ...group[0] };
    for (const n of group.slice(1)) {
      if (n.startBeat <= cur.startBeat + cur.lengthBeats + 1e-9) {
        cur.lengthBeats = Math.max(cur.lengthBeats, n.startBeat + n.lengthBeats - cur.startBeat);
        dropped.add(n.id);
      } else {
        merged.set(cur.id, cur);
        cur = { ...n };
      }
    }
    merged.set(cur.id, cur);
  }
  if (dropped.size === 0) return notes;
  return notes.filter((n) => !dropped.has(n.id)).map((n) => (targetIds.has(n.id) ? (merged.get(n.id) ?? n) : n));
}

/** Copy the selection one `span` later (⌘B: the pattern length, FL's favourite). */
export function duplicateBySpan(notes: NoteEvent[], ids: Set<string>, span: number): { notes: NoteEvent[]; newIds: Set<string> } {
  const target = sel(notes, ids);
  if (!target.length || !(span > 0)) return { notes, newIds: new Set() };
  const copies = target.map((n) => ({ ...n, id: uuid(), startBeat: n.startBeat + span }));
  return { notes: [...notes, ...copies], newIds: new Set(copies.map((c) => c.id)) };
}

/** Reverse the selection in time within its own span (ends become starts). */
export function reverseNotes(notes: NoteEvent[], ids: Set<string>): NoteEvent[] {
  const target = sel(notes, ids);
  if (target.length < 2) return notes;
  const set = new Set(target.map((n) => n.id));
  const start = Math.min(...target.map((n) => n.startBeat));
  const end = Math.max(...target.map((n) => n.startBeat + n.lengthBeats));
  return notes.map((n) => (set.has(n.id) ? { ...n, startBeat: start + (end - (n.startBeat + n.lengthBeats)) } : n));
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
export function setLength(notes: NoteEvent[], ids: Set<string>, length: number): NoteEvent[] {
  const target = new Set(sel(notes, ids).map((n) => n.id));
  return notes.map((n) => (target.has(n.id) ? { ...n, lengthBeats: Math.max(MIN_LENGTH, length) } : n));
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
 * Velocity ramp: a straight line from (`b0`, `v0`) to (`b1`, `v1`) in beat × velocity space;
 * every note (of `ids`, or all) whose start lies between the two beats takes the line's value.
 */
export function rampVelocity(notes: NoteEvent[], ids: Set<string>, b0: number, v0: number, b1: number, v1: number): NoteEvent[] {
  const [from, to] = b0 <= b1 ? [b0, b1] : [b1, b0];
  const [vf, vt] = b0 <= b1 ? [v0, v1] : [v1, v0];
  const target = new Set(sel(notes, ids).map((n) => n.id));
  const span = Math.max(1e-9, to - from);
  return notes.map((n) => {
    if (!target.has(n.id) || n.startBeat < from - 1e-9 || n.startBeat > to + 1e-9) return n;
    const t = (n.startBeat - from) / span;
    return { ...n, velocity: Math.round(Math.min(127, Math.max(1, vf + (vt - vf) * t))) };
  });
}

/** Ids of the next / previous note in time order after the selection (or the first note). */
export function stepSelection(notes: NoteEvent[], ids: Set<string>, dir: 1 | -1): string | null {
  if (!notes.length) return null;
  const ordered = [...notes].sort((a, b) => a.startBeat - b.startBeat || a.pitch - b.pitch);
  if (!ids.size) return (dir === 1 ? ordered[0] : ordered[ordered.length - 1]).id;
  const idx = ordered.map((n) => n.id);
  const anchor = dir === 1 ? Math.max(...[...ids].map((id) => idx.indexOf(id))) : Math.min(...[...ids].map((id) => idx.indexOf(id)));
  const next = (anchor + dir + ordered.length) % ordered.length;
  return ordered[next].id;
}

/**
 * A ghost pattern's notes laid under a pattern `lengthBeats` long: looped every
 * `ghostLengthBeats` the way the song would loop it, cut at the end, muted notes left out
 * (they aren't heard either). Ids get a `~<loop>` suffix so they stay unique per pass.
 */
export function tileGhost(ghost: NoteEvent[], ghostLengthBeats: number, lengthBeats: number): NoteEvent[] {
  const step = ghostLengthBeats > 0 ? ghostLengthBeats : lengthBeats;
  const out: NoteEvent[] = [];
  for (let loop = 0, off = 0; off < lengthBeats - 1e-9; loop++, off += step) {
    for (const n of ghost) {
      const start = n.startBeat + off;
      if (n.muted || n.startBeat >= step - 1e-9 || start >= lengthBeats - 1e-9) continue;
      out.push({ ...n, id: `${n.id}~${loop}`, startBeat: start, lengthBeats: Math.min(n.lengthBeats, lengthBeats - start) });
    }
  }
  return out;
}
