// Pure clip math for the song view: group moves, trims, drops and marquee hits, all kept
// inside the song's hard end. The backend clamps too; this keeps the preview honest.

import type { Clip } from "./types";

/** Shortest clip a trim can leave (one beat), unless the song end leaves less room. */
export const MIN_CLIP_BEATS = 1;

/** Round to the nearest multiple of `grid`. */
export function snapTo(beat: number, grid: number): number {
  return Math.round(beat / grid) * grid;
}

/**
 * The shift for a group move: the anchor clip's start snaps to `grid`, every clip moves by the
 * same amount, and the whole group stays inside `[0, songEnd]`. Returns 0 for an empty group.
 */
export function groupMoveDelta(anchor: Clip, group: Clip[], rawDelta: number, grid: number, songEnd: number): number {
  if (group.length === 0) return 0;
  const delta = snapTo(anchor.startBeat + rawDelta, grid) - anchor.startBeat;
  const lo = -Math.min(...group.map((c) => c.startBeat));
  const hi = Math.max(0, songEnd - Math.max(...group.map((c) => c.startBeat + c.lengthBeats)));
  return Math.min(Math.max(delta, lo), Math.max(lo, hi));
}

/** The group with every start shifted by `delta`. */
export function moveClips(group: Clip[], delta: number): Clip[] {
  return group.map((c) => ({ ...c, startBeat: c.startBeat + delta }));
}

/** A right-edge trim or loop-extend: snapped, at least one beat, never past the song end. */
export function trimClip(clip: Clip, rawLength: number, grid: number, songEnd: number): Clip {
  const room = songEnd - clip.startBeat;
  const len = Math.max(MIN_CLIP_BEATS, snapTo(rawLength, grid));
  return { ...clip, lengthBeats: Math.max(Math.min(len, room), Math.min(MIN_CLIP_BEATS, room)) };
}

/** Where a pattern dropped at `beat` lands: the bar it's in, or null outside the song. */
export function dropBar(beat: number, beatsPerBar: number, songEnd: number): number | null {
  if (!Number.isFinite(beat) || beat < 0 || beat >= songEnd) return null;
  return Math.floor(beat / beatsPerBar) * beatsPerBar;
}

/**
 * Clips touched by a marquee spanning lanes `[lane0, lane1]` and beats `[beat0, beat1]` (either
 * order). `laneOf` gives a clip's lane index, or -1 when it isn't drawn.
 */
export function clipsInBox(clips: Clip[], laneOf: (clip: Clip) => number, lane0: number, lane1: number, beat0: number, beat1: number): string[] {
  const [l0, l1] = lane0 <= lane1 ? [lane0, lane1] : [lane1, lane0];
  const [b0, b1] = beat0 <= beat1 ? [beat0, beat1] : [beat1, beat0];
  return clips
    .filter((c) => {
      const lane = laneOf(c);
      return lane >= l0 && lane <= l1 && c.startBeat < b1 && c.startBeat + c.lengthBeats > b0;
    })
    .map((c) => c.id);
}

/** `ids` with `id` added, or removed if it was there. */
export function toggleId(ids: string[], id: string): string[] {
  return ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id];
}
