// Pure clip math for the song view: snapping, group moves, trims, drops, clipboard and marquee
// hits, all kept inside the song's hard end. The backend clamps too; this keeps the preview
// honest.

import type { AudioClip, Clip } from "./types";
import { MAX_FADE_SECONDS, TICKS_PER_BEAT } from "./types";

/** Shortest clip a trim can leave (one beat), unless the song end leaves less room. */
export const MIN_CLIP_TICKS = TICKS_PER_BEAT;

/** What the song view snaps to: a division in ticks, one bar of the project's meter, or nothing. */
export type ArrSnap = number | "bar" | "off";

/**
 * The tick grid a gesture lands on. "off" (and ⇧, which bypasses whatever is set) answers 1:
 * every helper below already rounds to a grid, and a grid of one tick only rounds to a whole
 * tick — so snap-off needs no second code path.
 */
export function snapGrid(snap: ArrSnap, barTicks: number, bypass = false): number {
  if (bypass || snap === "off") return 1;
  if (snap === "bar") return Math.max(1, Math.round(barTicks));
  return Math.max(1, Math.round(snap));
}

/** Round to the nearest multiple of `gridTicks`, staying on whole ticks. */
export function snapTo(tick: number, gridTicks: number): number {
  return Math.round(tick / gridTicks) * gridTicks;
}

/**
 * The shift for a group move: the anchor clip's start snaps to `gridTicks`, every clip moves by
 * the same amount, and the whole group stays inside `[0, songEnd]`. Returns 0 for an empty group.
 */
export function groupMoveDelta(anchor: Clip, group: Clip[], rawDelta: number, gridTicks: number, songEnd: number): number {
  if (group.length === 0) return 0;
  const delta = snapTo(anchor.startTick + rawDelta, gridTicks) - anchor.startTick;
  const lo = -Math.min(...group.map((c) => c.startTick));
  const hi = Math.max(0, songEnd - Math.max(...group.map((c) => c.startTick + c.lengthTicks)));
  return Math.min(Math.max(delta, lo), Math.max(lo, hi));
}

/** The group with every start shifted by `delta` ticks. */
export function moveClips(group: Clip[], delta: number): Clip[] {
  return group.map((c) => ({ ...c, startTick: c.startTick + delta }));
}

/** A right-edge trim or loop-extend: snapped, at least one beat, never past the song end. */
export function trimClip(clip: Clip, rawLength: number, gridTicks: number, songEnd: number): Clip {
  const room = songEnd - clip.startTick;
  const len = Math.max(MIN_CLIP_TICKS, snapTo(rawLength, gridTicks));
  return { ...clip, lengthTicks: Math.max(Math.min(len, room), Math.min(MIN_CLIP_TICKS, room)) };
}

/**
 * Where a pattern dropped at `tick` lands: the start of the `gridTicks` slot it fell in, or null
 * outside the song. It floors rather than rounds, so a drop never jumps past the pointer.
 */
export function dropAt(tick: number, gridTicks: number, songEnd: number): number | null {
  if (!Number.isFinite(tick) || tick < 0 || tick >= songEnd) return null;
  return Math.floor(tick / gridTicks) * gridTicks;
}

/**
 * Clips touched by a marquee spanning lanes `[lane0, lane1]` and ticks `[tick0, tick1]` (either
 * order). `laneOf` gives a clip's lane index, or -1 when it isn't drawn.
 */
export function clipsInBox(clips: Clip[], laneOf: (clip: Clip) => number, lane0: number, lane1: number, tick0: number, tick1: number): string[] {
  const [l0, l1] = lane0 <= lane1 ? [lane0, lane1] : [lane1, lane0];
  const [t0, t1] = tick0 <= tick1 ? [tick0, tick1] : [tick1, tick0];
  return clips
    .filter((c) => {
      const lane = laneOf(c);
      return lane >= l0 && lane <= l1 && c.startTick < t1 && c.startTick + c.lengthTicks > t0;
    })
    .map((c) => c.id);
}

/** `ids` with `id` added, or removed if it was there. */
export function toggleId(ids: string[], id: string): string[] {
  return ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id];
}

/**
 * A group muted or unmuted as one step: any clip still audible means "mute all", so a mixed
 * selection takes one keystroke to silence and a second to bring back.
 */
export function toggleMuted(group: Clip[]): Clip[] {
  const muted = group.some((c) => !c.muted);
  return group.map((c) => ({ ...c, muted }));
}

/**
 * Where pasted clips land: the clipboard shifted so its earliest clip starts at `atTick`, each
 * keeping its own pattern (and so its own track). Copies that would start outside the song are
 * dropped rather than piled up at the end. The result feeds `addClip`, which clamps the lengths.
 */
export function pasteClips(clipboard: Clip[], atTick: number, songEnd: number): { src: Clip; startTick: number }[] {
  if (clipboard.length === 0) return [];
  const first = Math.min(...clipboard.map((c) => c.startTick));
  const at = Math.max(0, Math.round(atTick));
  return clipboard
    .map((src) => ({ src, startTick: src.startTick - first + at }))
    .filter((x) => x.startTick < songEnd);
}

/**
 * Where duplicated clips land: one group-span later, so repeating the gesture marches copies
 * along the timeline instead of stacking them. Copies past the song end are dropped.
 */
export function duplicateClips(group: Clip[], songEnd: number): { src: Clip; startTick: number }[] {
  if (group.length === 0) return [];
  const first = Math.min(...group.map((c) => c.startTick));
  const span = Math.max(...group.map((c) => c.startTick + c.lengthTicks)) - first;
  return group.map((src) => ({ src, startTick: src.startTick + span })).filter((x) => x.startTick < songEnd);
}

// ─── Ripple: opening and cutting time ──────────────────────────────────────────────────────
// These mirror `crates/dissonant-core/src/ripple.rs` exactly; the backend and the mock bridge
// both work from them, and the ruler menu uses them to say what an edit will do before it does.

/** A surviving piece of a span an edit cut: where it lands, how far into the original it
 * begins (what it has to skip to keep playing the same material), and how long it is. */
export interface Piece {
  start: number;
  skip: number;
  len: number;
}

/** Where a point lands when `ticks` of empty time open at `at`. A point *on* `at` moves: the
 * material starting there is what the new time is opened in front of. */
export function insertPoint(tick: number, at: number, ticks: number): number {
  return tick >= at ? tick + ticks : tick;
}

/** Where a point lands when `[at, at + ticks)` is cut out: inside the cut it collapses onto
 * `at`, which is both where the gap closes and where an end reaching into it now stops. */
export function deletePoint(tick: number, at: number, ticks: number): number {
  if (tick < at) return tick;
  return tick < at + ticks ? at : tick - ticks;
}

/** The pieces of material `[start, start + len)` after `ticks` open at `at`: at most two, and
 * together they still play every tick the original did. */
export function insertPieces(start: number, len: number, at: number, ticks: number): Piece[] {
  if (len <= 0) return [];
  const end = start + len;
  if (end <= at) return [{ start, skip: 0, len }];
  if (start >= at) return [{ start: start + ticks, skip: 0, len }];
  return [
    { start, skip: 0, len: at - start },
    { start: at + ticks, skip: at - start, len: end - at },
  ];
}

/** The pieces of `[start, start + len)` after `[at, at + ticks)` is cut out: what was inside
 * the cut is gone and what followed closes up against what came before. */
export function deletePieces(start: number, len: number, at: number, ticks: number): Piece[] {
  if (len <= 0) return [];
  const end = start + len;
  const cutEnd = at + ticks;
  const out: Piece[] = [];
  if (start < at) out.push({ start, skip: 0, len: Math.min(at, end) - start });
  if (end > cutEnd) {
    const s = Math.max(start, cutEnd);
    out.push({ start: s - ticks, skip: s - start, len: end - s });
  }
  return out;
}

/** The part of `[start, start + len)` inside the window `[from, to)`, moved so the window's
 * own start lands on `at`. Null when the material misses the window. */
export function windowPiece(start: number, len: number, from: number, to: number, at: number): Piece | null {
  const s = Math.max(start, from);
  const e = Math.min(start + len, to);
  return e > s ? { start: at + (s - from), skip: s - start, len: e - s } : null;
}

// ─── Audio clips: fades and the punch split ────────────────────────────────────────────────
// Audio never warps, so these work in seconds and only cross into ticks through the song's
// tempo map. They mirror `crates/dissonant-core/src/document.rs` (`punch_split`) and
// `model.rs` (`clamp_fade`); the backend clamps too, this keeps the drag preview honest.

/** `model.rs::clamp_fade`: fades may not overlap, so each is capped at half the clip. */
export function clampFade(seconds: number, lengthSeconds: number): number {
  if (!Number.isFinite(seconds)) return 0;
  return Math.min(Math.max(seconds, 0), MAX_FADE_SECONDS, Math.max(0, lengthSeconds) / 2);
}

/**
 * `audio.rs::equal_power`: the gain `t` of the way through a fade, so the handle draws the
 * curve the engine actually plays rather than a straight line that lies about it.
 */
export function equalPowerGain(t: number): number {
  return Math.sin(Math.min(Math.max(t, 0), 1) * (Math.PI / 2));
}

/**
 * The whole tick nearest `seconds` on a map that only ever increases. Binary search, the same
 * way `bridge.ts::audioLenTicks` crosses the other way: the map is a closure, not a formula.
 */
export function tickAtSeconds(secondsAt: (tick: number) => number, seconds: number): number {
  if (!(seconds > secondsAt(0))) return 0;
  let lo = 0;
  let hi = TICKS_PER_BEAT;
  while (secondsAt(hi) < seconds) hi *= 2;
  for (let i = 0; i < 60; i++) {
    const mid = (lo + hi) / 2;
    if (secondsAt(mid) < seconds) lo = mid;
    else hi = mid;
  }
  return Math.round(hi);
}

/**
 * `document.rs::punch_split`: `clips` with everything the punch covers on the take's track cut
 * back around it, ready for the take itself to be added.
 *
 * A clip the take lands inside becomes at most two — a head running one crossfade *past* the
 * take's start and fading out over it, and a tail starting one crossfade *before* the take's
 * end and fading in over that one. Those overlaps are the crossfades: the engine sums
 * overlapping clips, so both sides sounding at once is the whole mechanism. A clip buried
 * under the take leaves nothing. Each new edge is placed on a whole tick and takes its offset
 * into the file from that tick, so the material under it never slides.
 */
export function punchSplit(
  clips: AudioClip[],
  take: AudioClip,
  crossfadeSeconds: number,
  secondsAt: (tick: number) => number,
  newId: () => string,
): AudioClip[] {
  const from = secondsAt(take.startTick);
  const to = from + take.lengthSeconds;
  const xf = Math.min(Math.max(crossfadeSeconds, 0), MAX_FADE_SECONDS, take.lengthSeconds / 2);
  const out: AudioClip[] = [];
  for (const c of clips) {
    const start = secondsAt(c.startTick);
    const end = start + c.lengthSeconds;
    if (c.trackId !== take.trackId || end <= from || start >= to) {
      out.push(c);
      continue;
    }
    // Only a clip that was already sounding when the punch dropped in has a head.
    const headEnd = Math.min(from + xf, end);
    const head: AudioClip | null =
      start < from
        ? { ...c, lengthSeconds: headEnd - start, fadeOutSeconds: clampFade(xf, headEnd - start) }
        : null;
    let tail: AudioClip | null = null;
    if (end > to) {
      const tick = tickAtSeconds(secondsAt, Math.max(to - xf, start));
      const at = Math.min(Math.max(secondsAt(tick), start), end);
      tail = {
        ...c,
        // The head keeps the clip's identity when there is one; with the take butted against
        // the clip's front the tail is all that is left of it and keeps the id instead.
        id: head ? newId() : c.id,
        startTick: tick,
        lengthSeconds: end - at,
        offsetSeconds: c.offsetSeconds + (at - start),
        fadeInSeconds: clampFade(xf, end - at),
      };
    }
    if (head && head.lengthSeconds > 0) out.push(head);
    if (tail && tail.lengthSeconds > 0) out.push(tail);
  }
  return out;
}
