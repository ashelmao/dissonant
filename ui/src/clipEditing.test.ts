import { describe, expect, it } from "vitest";
import { clipsInBox, dropBar, groupMoveDelta, moveClips, snapTo, toggleId, trimClip } from "./clipEditing";
import type { Clip } from "./types";

const c = (id: string, start: number, len: number, patternId = "p"): Clip => ({
  id,
  patternId,
  startBeat: start,
  lengthBeats: len,
  offsetBeats: 0,
  muted: false,
});

describe("group move", () => {
  it("snaps the anchor to the grid and shifts the group together", () => {
    const a = c("a", 4, 4);
    const b = c("b", 12, 4);
    const d = groupMoveDelta(a, [a, b], 5.2, 4, 64);
    expect(d).toBe(4);
    expect(moveClips([a, b], d).map((x) => x.startBeat)).toEqual([8, 16]);
  });

  it("stops at bar 1 and at the song end", () => {
    const a = c("a", 4, 4);
    const b = c("b", 12, 4);
    expect(groupMoveDelta(a, [a, b], -40, 4, 64)).toBe(-4);
    expect(groupMoveDelta(a, [a, b], 100, 4, 64)).toBe(48); // b ends exactly at 64
  });

  it("is zero for an empty group", () => {
    expect(groupMoveDelta(c("a", 0, 4), [], 8, 4, 64)).toBe(0);
  });
});

describe("trim", () => {
  it("snaps, keeps at least one beat and never crosses the end", () => {
    expect(trimClip(c("a", 0, 4), 6.4, 4, 64).lengthBeats).toBe(8);
    expect(trimClip(c("a", 0, 4), -3, 4, 64).lengthBeats).toBe(1);
    expect(trimClip(c("a", 56, 4), 30, 4, 64).lengthBeats).toBe(8);
    expect(trimClip(c("a", 63.5, 0.5), 4, 1, 64).lengthBeats).toBe(0.5);
  });
});

describe("drop", () => {
  it("lands on the bar under the pointer, inside the song only", () => {
    expect(dropBar(9.9, 4, 64)).toBe(8);
    expect(dropBar(0, 4, 64)).toBe(0);
    expect(dropBar(63.9, 4, 64)).toBe(60);
    expect(dropBar(64, 4, 64)).toBeNull();
    expect(dropBar(-1, 4, 64)).toBeNull();
  });
});

describe("marquee", () => {
  it("hits clips on the spanned lanes that overlap the beat range", () => {
    const clips = [c("a", 0, 4, "x"), c("b", 8, 4, "y"), c("c", 20, 4, "x"), c("d", 4, 4, "z")];
    const lane = (cl: Clip) => ({ x: 0, y: 1, z: 2 })[cl.patternId as "x" | "y" | "z"] ?? -1;
    expect(clipsInBox(clips, lane, 1, 0, 10, 2)).toEqual(["a", "b"]);
    expect(clipsInBox(clips, lane, 0, 2, 4, 4.5)).toEqual(["d"]);
  });

  it("toggles ids", () => {
    expect(toggleId(["a", "b"], "a")).toEqual(["b"]);
    expect(toggleId(["a"], "b")).toEqual(["a", "b"]);
  });
});

describe("snap", () => {
  it("rounds to the grid", () => {
    expect(snapTo(5.9, 4)).toBe(4);
    expect(snapTo(6.1, 4)).toBe(8);
  });
});
