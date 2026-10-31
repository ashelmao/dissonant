// Where the piano roll draws ropes and their handles, and what a pointer is over. Pure pixel
// geometry: the roll hands in each note's drawn body (`Box`, already shifted to where the note
// sounds) and gets back points, curves and hit tests, so none of this needs a canvas to test.
//
// A rope leaves its source a little before the slide note starts (or from the source's end, if
// it stopped sounding earlier) and lands on the slide note's left edge, pointing in. Live ropes
// therefore read as a short S between the two rows right where the pitch moves; a dead rope
// that spans a gap, or runs backwards, still connects the two notes so you can find and fix it.

import { MAX_SLIDE_SECS } from "./ropes";

export interface Pt {
  x: number;
  y: number;
}

/** A note's drawn body in canvas pixels. */
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A cubic Bézier: start, two control points, end. */
export interface Curve {
  p0: Pt;
  c1: Pt;
  c2: Pt;
  p3: Pt;
}

/** Gap between a note's right end and its rope knob's rim. */
export const KNOB_GAP = 3;
/** The knob's drawn radius. */
export const KNOB_R = 3;
/** How near the knob's centre a pointer has to be to grab it. */
export const KNOB_HIT = 6;
/** How near the curve a pointer has to be to pick a rope. */
export const ROPE_HIT = 4;
/** How near the midpoint a pointer has to be to grab the slide-time handle. */
export const HANDLE_HIT = 6;
/** How far before the slide note a rope leaves its source, when the source is still sounding. */
export const ROPE_LEAD = 18;
/** Horizontal travel that sweeps the slide-time handle through its whole range. */
export const SLIDE_SWEEP_PX = 240;

const dist = (a: Pt, b: Pt) => Math.hypot(a.x - b.x, a.y - b.y);
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** The rope knob's centre: just past the note's right end, vertically centred. */
export function knobCentre(b: Box): Pt {
  return { x: b.x + b.w + KNOB_GAP + KNOB_R, y: b.y + b.h / 2 };
}

/**
 * Whether (x, y) grabs `b`'s knob. Only points past the note's own right end count, so the
 * knob never eats the resize edge inside the body, however short the note is.
 */
export function overKnob(b: Box, x: number, y: number): boolean {
  return x > b.x + b.w && dist(knobCentre(b), { x, y }) <= KNOB_HIT;
}

/** The curve between two points, leaving and arriving level; a flat run bows upward by `bow`. */
function sCurve(p0: Pt, p3: Pt, bow: number): Curve {
  const k = Math.max(8, Math.abs(p3.x - p0.x) / 2);
  const lift = Math.abs(p3.y - p0.y) < 1 ? bow : 0;
  return { p0, c1: { x: p0.x + k, y: p0.y - lift }, c2: { x: p3.x - k, y: p3.y - lift }, p3 };
}

/**
 * The rope from `source` into `target`'s left edge. It leaves the source `ROPE_LEAD` before the
 * target starts, kept within the source's body: from its start when the target comes first, and
 * from its end when the target comes after it. Ropes between two notes on one row (a tie) bow
 * above them, since a level line would vanish into the bodies.
 */
export function ropeCurve(source: Box, target: Box): Curve {
  const p3 = { x: target.x, y: target.y + target.h / 2 };
  const p0 = { x: clamp(target.x - ROPE_LEAD, source.x, source.x + source.w), y: source.y + source.h / 2 };
  return sCurve(p0, p3, source.h * 1.2);
}

/** The rubber band while a rope is being drawn: from `source`'s knob to the pointer. */
export function draftCurve(source: Box, to: Pt): Curve {
  return sCurve(knobCentre(source), to, 0);
}

/** The point at `t` (0–1) along `c`. */
export function curveAt(c: Curve, t: number): Pt {
  const u = 1 - t;
  const a = u * u * u;
  const b = 3 * u * u * t;
  const d = 3 * u * t * t;
  const e = t * t * t;
  return { x: a * c.p0.x + b * c.c1.x + d * c.c2.x + e * c.p3.x, y: a * c.p0.y + b * c.c1.y + d * c.c2.y + e * c.p3.y };
}

/** Where the slide-time handle sits. */
export function curveMid(c: Curve): Pt {
  return curveAt(c, 0.5);
}

/** Distance from (x, y) to `c`, measured against `steps` straight pieces (plenty for a hit test). */
export function distanceToCurve(c: Curve, x: number, y: number, steps = 16): number {
  let best = Infinity;
  let a = c.p0;
  for (let i = 1; i <= steps; i++) {
    const b = curveAt(c, i / steps);
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const len2 = dx * dx + dy * dy;
    const t = len2 ? clamp(((x - a.x) * dx + (y - a.y) * dy) / len2, 0, 1) : 0;
    best = Math.min(best, Math.hypot(x - (a.x + t * dx), y - (a.y + t * dy)));
    a = b;
  }
  return best;
}

/** A rope the pointer is over, named by its slide note; `handle` when it's on the midpoint. */
export interface RopeHit {
  id: string;
  handle: boolean;
}

/**
 * The rope under (x, y). A midpoint handle wins anywhere within `HANDLE_HIT`, even over a note,
 * since it's small and deliberate; the line itself only counts off the notes (`overNote`), so a
 * rope crossing a note never stops you grabbing the note. The nearest rope wins a tie.
 */
export function ropeAt(ropes: { id: string; curve: Curve }[], x: number, y: number, overNote: boolean): RopeHit | null {
  let best: RopeHit | null = null;
  let bestD = Infinity;
  for (const r of ropes) {
    const d = dist(curveMid(r.curve), { x, y });
    if (d <= HANDLE_HIT && d < bestD) {
      best = { id: r.id, handle: true };
      bestD = d;
    }
  }
  if (best || overNote) return best;
  for (const r of ropes) {
    const d = distanceToCurve(r.curve, x, y);
    if (d <= ROPE_HIT && d < bestD) {
      best = { id: r.id, handle: false };
      bestD = d;
    }
  }
  return best;
}

/** The arrowhead at the curve's end: tip, then the two barbs, pointing along the last tangent. */
export function arrowHead(c: Curve, size = 5): [Pt, Pt, Pt] {
  let dx = c.p3.x - c.c2.x;
  let dy = c.p3.y - c.c2.y;
  const len = Math.hypot(dx, dy);
  if (len < 1e-6) {
    dx = 1;
    dy = 0;
  } else {
    dx /= len;
    dy /= len;
  }
  const bx = c.p3.x - dx * size;
  const by = c.p3.y - dy * size;
  const half = size * 0.6;
  return [c.p3, { x: bx - dy * half, y: by + dx * half }, { x: bx + dy * half, y: by - dx * half }];
}

/**
 * The slide time after dragging the handle `dx` pixels from where it started at `startSecs`.
 * The handle walks a square-root scale, so the short slides you use most get most of the
 * travel: `SLIDE_SWEEP_PX` covers 0 → `MAX_SLIDE_SECS`. Right is longer. Rounded to 10 ms.
 */
export function slideSecsFromDrag(startSecs: number, dx: number): number {
  const u = clamp(Math.sqrt(clamp(startSecs, 0, MAX_SLIDE_SECS) / MAX_SLIDE_SECS) + dx / SLIDE_SWEEP_PX, 0, 1);
  return Math.round(u * u * MAX_SLIDE_SECS * 100) / 100;
}

/** The handle's readout: the rope's own time, or the patch's glide when it has none. */
export function slideLabel(secs: number | undefined, glide: number | null): string {
  if (secs !== undefined) return `${secs.toFixed(2)}s`;
  return glide === null ? "glide" : `glide ${glide.toFixed(2)}s`;
}
