import { describe, expect, it } from "vitest";
import {
  arrowHead,
  curveAt,
  curveMid,
  distanceToCurve,
  draftCurve,
  HANDLE_HIT,
  KNOB_HIT,
  knobCentre,
  overKnob,
  ROPE_HIT,
  ROPE_LEAD,
  ropeAt,
  ropeCurve,
  SLIDE_SWEEP_PX,
  slideLabel,
  slideSecsFromDrag,
  type Box,
} from "./ropeGeometry";
import { MAX_SLIDE_SECS } from "./ropes";

// Two 18px rows, 96px per beat: a one-beat source on row 0 and a slide note on row 3.
const box = (x: number, row: number, w: number): Box => ({ x, y: 28 + row * 18, w, h: 17 });
const source = box(100, 0, 95);

describe("knob", () => {
  it("sits just past the right end, vertically centred", () => {
    const k = knobCentre(source);
    expect(k.x).toBeGreaterThan(source.x + source.w);
    expect(k.y).toBe(source.y + source.h / 2);
  });

  it("is grabbed within its radius, but never inside the body's resize edge", () => {
    const k = knobCentre(source);
    expect(overKnob(source, k.x, k.y)).toBe(true);
    expect(overKnob(source, k.x + KNOB_HIT - 1, k.y)).toBe(true);
    expect(overKnob(source, k.x + KNOB_HIT + 1, k.y)).toBe(false);
    expect(overKnob(source, k.x, k.y + KNOB_HIT + 1)).toBe(false);
    expect(overKnob(source, source.x + source.w - 1, k.y)).toBe(false);
    expect(overKnob(source, source.x + source.w, k.y)).toBe(false);
  });

  it("stays grabbable on a note only a few pixels wide", () => {
    const tiny = box(100, 0, 3);
    const k = knobCentre(tiny);
    expect(overKnob(tiny, k.x, k.y)).toBe(true);
    expect(overKnob(tiny, tiny.x + 1, k.y)).toBe(false);
  });
});

describe("ropeCurve", () => {
  it("leaves the source just before the slide note and lands on its left edge", () => {
    const target = box(196, 3, 95);
    const c = ropeCurve(source, target);
    expect(c.p0).toEqual({ x: 196 - ROPE_LEAD, y: source.y + source.h / 2 });
    expect(c.p3).toEqual({ x: 196, y: target.y + target.h / 2 });
    // Leaves and arrives level, so the arrow points straight into the note.
    expect(c.c1.y).toBe(c.p0.y);
    expect(c.c2.y).toBe(c.p3.y);
  });

  it("keeps its start on the source's body", () => {
    expect(ropeCurve(source, box(400, 3, 20)).p0.x).toBe(source.x + source.w);
    expect(ropeCurve(source, box(40, 3, 20)).p0.x).toBe(source.x);
    expect(ropeCurve(box(100, 0, 6), box(106, 3, 20)).p0.x).toBe(100);
  });

  it("bows a tie above the row instead of running through the notes", () => {
    const c = ropeCurve(source, box(196, 0, 95));
    expect(curveMid(c).y).toBeLessThan(source.y);
  });

  it("draws the rubber band from the knob to the pointer", () => {
    const c = draftCurve(source, { x: 300, y: 200 });
    expect(c.p0).toEqual(knobCentre(source));
    expect(c.p3).toEqual({ x: 300, y: 200 });
  });
});

describe("curve sampling", () => {
  const c = ropeCurve(source, box(196, 3, 95));

  it("runs from end to end", () => {
    expect(curveAt(c, 0)).toEqual(c.p0);
    expect(curveAt(c, 1)).toEqual(c.p3);
  });

  it("measures distance to the line, zero on it", () => {
    const m = curveMid(c);
    expect(distanceToCurve(c, m.x, m.y)).toBeLessThan(0.5);
    expect(distanceToCurve(c, m.x + 30, m.y)).toBeGreaterThan(20);
  });

  it("points the arrow along the last tangent, tip on the note's edge", () => {
    const [tip, a, b] = arrowHead(c, 5);
    expect(tip).toEqual(c.p3);
    expect(a.x).toBeLessThan(tip.x);
    expect(b.x).toBeLessThan(tip.x);
    expect(a.y).not.toBe(b.y);
  });

  it("still makes an arrow when the curve ends on its own control point", () => {
    const p = { x: 10, y: 10 };
    const [tip, a] = arrowHead({ p0: p, c1: p, c2: p, p3: p });
    expect(tip).toEqual(p);
    expect(Number.isFinite(a.x)).toBe(true);
  });
});

describe("ropeAt", () => {
  const ropes = [
    { id: "b", curve: ropeCurve(source, box(196, 3, 95)) },
    { id: "c", curve: ropeCurve(source, box(196, 6, 95)) },
  ];

  it("grabs a handle anywhere within its radius, even over a note", () => {
    const m = curveMid(ropes[1].curve);
    expect(ropeAt(ropes, m.x + HANDLE_HIT - 1, m.y, true)).toEqual({ id: "c", handle: true });
  });

  it("picks the line only off the notes", () => {
    const p = curveAt(ropes[0].curve, 0.8);
    expect(ropeAt(ropes, p.x + ROPE_HIT - 1, p.y, false)).toEqual({ id: "b", handle: false });
    expect(ropeAt(ropes, p.x, p.y, true)).toBeNull();
  });

  it("misses when nothing is near", () => {
    expect(ropeAt(ropes, 600, 20, false)).toBeNull();
    expect(ropeAt([], 0, 0, false)).toBeNull();
  });
});

describe("slide time", () => {
  it("is unchanged until the handle moves", () => {
    expect(slideSecsFromDrag(0.25, 0)).toBe(0.25);
  });

  it("gets longer to the right and shorter to the left, clamped to the range", () => {
    expect(slideSecsFromDrag(0.25, 20)).toBeGreaterThan(0.25);
    expect(slideSecsFromDrag(0.25, -20)).toBeLessThan(0.25);
    expect(slideSecsFromDrag(0.25, -10_000)).toBe(0);
    expect(slideSecsFromDrag(0.25, 10_000)).toBe(MAX_SLIDE_SECS);
    expect(slideSecsFromDrag(0, SLIDE_SWEEP_PX)).toBe(MAX_SLIDE_SECS);
  });

  it("spends more travel on short slides than long ones", () => {
    const short = slideSecsFromDrag(0, SLIDE_SWEEP_PX / 4);
    expect(short).toBeLessThan(MAX_SLIDE_SECS / 4);
    expect(short).toBeGreaterThan(0);
  });

  it("rounds to 10 ms", () => {
    const s = slideSecsFromDrag(0.1, 7);
    expect(Math.round(s * 100) / 100).toBe(s);
  });

  it("reads the rope's own time, or the glide it falls back to", () => {
    expect(slideLabel(0.123, 0.05)).toBe("0.12s");
    expect(slideLabel(undefined, 0.05)).toBe("glide 0.05s");
    expect(slideLabel(undefined, null)).toBe("glide");
  });
});
