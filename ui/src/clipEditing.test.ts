import { describe, expect, it } from "vitest";
import { clampFade, clipsInBox, deletePieces, deletePoint, dropAt, duplicateClips, equalPowerGain, groupMoveDelta, insertPieces, insertPoint, moveClips, pasteClips, punchSplit, snapGrid, snapTo, tickAtSeconds, toggleId, toggleMuted, trimClip, windowPiece } from "./clipEditing";
import type { AudioClip, Clip } from "./types";
import { DEFAULT_PUNCH_CROSSFADE_SECONDS as XF, TICKS_PER_BEAT as B } from "./types";

// Positions read as beats here; the model counts ticks, so everything is scaled by B.
const c = (id: string, startBeats: number, lenBeats: number, patternId = "p"): Clip => ({
  id,
  patternId,
  startTick: startBeats * B,
  lengthTicks: lenBeats * B,
  offsetTicks: 0,
  muted: false,
});

describe("group move", () => {
  it("snaps the anchor to the grid and shifts the group together", () => {
    const a = c("a", 4, 4);
    const b = c("b", 12, 4);
    const d = groupMoveDelta(a, [a, b], 5.2 * B, 4 * B, 64 * B);
    expect(d).toBe(4 * B);
    expect(moveClips([a, b], d).map((x) => x.startTick)).toEqual([8 * B, 16 * B]);
  });

  it("stops at bar 1 and at the song end", () => {
    const a = c("a", 4, 4);
    const b = c("b", 12, 4);
    expect(groupMoveDelta(a, [a, b], -40 * B, 4 * B, 64 * B)).toBe(-4 * B);
    expect(groupMoveDelta(a, [a, b], 100 * B, 4 * B, 64 * B)).toBe(48 * B); // b ends exactly at 64
  });

  it("is zero for an empty group", () => {
    expect(groupMoveDelta(c("a", 0, 4), [], 8 * B, 4 * B, 64 * B)).toBe(0);
  });
});

describe("trim", () => {
  it("snaps, keeps at least one beat and never crosses the end", () => {
    expect(trimClip(c("a", 0, 4), 6.4 * B, 4 * B, 64 * B).lengthTicks).toBe(8 * B);
    expect(trimClip(c("a", 0, 4), -3 * B, 4 * B, 64 * B).lengthTicks).toBe(B);
    expect(trimClip(c("a", 56, 4), 30 * B, 4 * B, 64 * B).lengthTicks).toBe(8 * B);
    expect(trimClip(c("a", 63.5, 0.5), 4 * B, B, 64 * B).lengthTicks).toBe(0.5 * B);
  });
});

describe("drop", () => {
  it("lands on the grid slot under the pointer, inside the song only", () => {
    expect(dropAt(9.9 * B, 4 * B, 64 * B)).toBe(8 * B);
    expect(dropAt(0, 4 * B, 64 * B)).toBe(0);
    expect(dropAt(63.9 * B, 4 * B, 64 * B)).toBe(60 * B);
    expect(dropAt(64 * B, 4 * B, 64 * B)).toBeNull();
    expect(dropAt(-B, 4 * B, 64 * B)).toBeNull();
  });

  it("drops on the exact tick when snap is off", () => {
    expect(dropAt(9.9 * B, snapGrid("off", 4 * B), 64 * B)).toBe(Math.floor(9.9 * B));
  });
});

describe("marquee", () => {
  it("hits clips on the spanned lanes that overlap the tick range", () => {
    const clips = [c("a", 0, 4, "x"), c("b", 8, 4, "y"), c("c", 20, 4, "x"), c("d", 4, 4, "z")];
    const lane = (cl: Clip) => ({ x: 0, y: 1, z: 2 })[cl.patternId as "x" | "y" | "z"] ?? -1;
    expect(clipsInBox(clips, lane, 1, 0, 10 * B, 2 * B)).toEqual(["a", "b"]);
    expect(clipsInBox(clips, lane, 0, 2, 4 * B, 4.5 * B)).toEqual(["d"]);
  });

  it("toggles ids", () => {
    expect(toggleId(["a", "b"], "a")).toEqual(["b"]);
    expect(toggleId(["a"], "b")).toEqual(["a", "b"]);
  });
});

describe("snap", () => {
  it("rounds to the grid", () => {
    expect(snapTo(5.9 * B, 4 * B)).toBe(4 * B);
    expect(snapTo(6.1 * B, 4 * B)).toBe(8 * B);
  });

  it("resolves the setting against the project's bar", () => {
    expect(snapGrid("bar", 3 * B)).toBe(3 * B);
    expect(snapGrid(B / 2, 4 * B)).toBe(B / 2);
    expect(snapGrid("off", 4 * B)).toBe(1);
    expect(snapGrid("bar", 4 * B, true)).toBe(1); // ⇧ bypasses whatever is set
  });

  it("snapping to one tick leaves a position alone", () => {
    const grid = snapGrid("off", 4 * B);
    expect(snapTo(1234.4, grid)).toBe(1234);
    const a = c("a", 4, 4);
    expect(groupMoveDelta(a, [a], 101.2, grid, 64 * B)).toBe(101);
  });
});

describe("mute", () => {
  it("silences a mixed selection, then brings it all back", () => {
    const group = [c("a", 0, 4), { ...c("b", 8, 4), muted: true }];
    const muted = toggleMuted(group);
    expect(muted.map((x) => x.muted)).toEqual([true, true]);
    expect(toggleMuted(muted).map((x) => x.muted)).toEqual([false, false]);
  });

  it("is empty for an empty group", () => {
    expect(toggleMuted([])).toEqual([]);
  });
});

describe("clipboard", () => {
  it("pastes the group with its shape, earliest clip at the playhead", () => {
    const group = [c("a", 8, 4, "x"), c("b", 12, 4, "y")];
    expect(pasteClips(group, 32 * B, 64 * B)).toEqual([
      { src: group[0], startTick: 32 * B },
      { src: group[1], startTick: 36 * B },
    ]);
  });

  it("keeps each clip on its own pattern, so copies stay on their tracks", () => {
    const group = [c("a", 0, 4, "x"), c("b", 0, 4, "y")];
    expect(pasteClips(group, 16 * B, 64 * B).map((x) => x.src.patternId)).toEqual(["x", "y"]);
  });

  it("drops copies that would land outside the song, and pastes nothing from nothing", () => {
    const group = [c("a", 0, 4), c("b", 40, 4)];
    expect(pasteClips(group, 32 * B, 64 * B).map((x) => x.startTick)).toEqual([32 * B]);
    expect(pasteClips(group, -99 * B, 64 * B).map((x) => x.startTick)).toEqual([0, 40 * B]);
    expect(pasteClips([], 0, 64 * B)).toEqual([]);
  });

  it("duplicates one group-span later so repeats march along", () => {
    const group = [c("a", 0, 4), c("b", 4, 4)];
    const once = duplicateClips(group, 64 * B);
    expect(once.map((x) => x.startTick)).toEqual([8 * B, 12 * B]);
    const twice = duplicateClips(once.map((x, i) => ({ ...x.src, id: `n${i}`, startTick: x.startTick })), 64 * B);
    expect(twice.map((x) => x.startTick)).toEqual([16 * B, 20 * B]);
  });

  it("duplicates nothing past the song end", () => {
    expect(duplicateClips([c("a", 60, 4)], 64 * B)).toEqual([]);
    expect(duplicateClips([], 64 * B)).toEqual([]);
  });
});

describe("ripple", () => {
  const piece = (start: number, skip: number, len: number) => ({ start: start * B, skip: skip * B, len: len * B });

  it("moves a point only when the time opens at or before it", () => {
    expect(insertPoint(4 * B, 8 * B, 4 * B)).toBe(4 * B);
    expect(insertPoint(8 * B, 8 * B, 4 * B)).toBe(12 * B);
    expect(insertPoint(20 * B, 8 * B, 4 * B)).toBe(24 * B);
  });

  it("collapses a point inside a cut onto the cut's start", () => {
    expect(deletePoint(4 * B, 8 * B, 4 * B)).toBe(4 * B);
    expect(deletePoint(8 * B, 8 * B, 4 * B)).toBe(8 * B);
    expect(deletePoint(10 * B, 8 * B, 4 * B)).toBe(8 * B);
    expect(deletePoint(12 * B, 8 * B, 4 * B)).toBe(8 * B);
    expect(deletePoint(20 * B, 8 * B, 4 * B)).toBe(16 * B);
  });

  it("leaves material before the cut alone and moves material after it whole", () => {
    expect(insertPieces(0, 8 * B, 8 * B, 4 * B)).toEqual([piece(0, 0, 8)]);
    expect(insertPieces(8 * B, 8 * B, 8 * B, 4 * B)).toEqual([piece(12, 0, 8)]);
    expect(deletePieces(0, 8 * B, 8 * B, 4 * B)).toEqual([piece(0, 0, 8)]);
    expect(deletePieces(16 * B, 8 * B, 8 * B, 4 * B)).toEqual([piece(12, 0, 8)]);
  });

  it("splits what the cut falls inside, so both halves play what they played", () => {
    expect(insertPieces(0, 16 * B, 4 * B, 4 * B)).toEqual([piece(0, 0, 4), piece(8, 4, 12)]);
    expect(deletePieces(0, 16 * B, 4 * B, 4 * B)).toEqual([piece(0, 0, 4), piece(4, 8, 8)]);
  });

  it("drops material a delete swallows whole", () => {
    expect(deletePieces(4 * B, 4 * B, 4 * B, 8 * B)).toEqual([]);
    expect(deletePieces(4 * B, 0, 0, 8 * B)).toEqual([]);
    expect(insertPieces(4 * B, 0, 0, 8 * B)).toEqual([]);
  });

  it("takes only the part of a span that overlaps the window it copies", () => {
    expect(windowPiece(0, 16 * B, 4 * B, 8 * B, 32 * B)).toEqual(piece(32, 4, 4));
    expect(windowPiece(4 * B, 4 * B, 4 * B, 8 * B, 32 * B)).toEqual(piece(32, 0, 4));
    expect(windowPiece(0, 4 * B, 4 * B, 8 * B, 32 * B)).toBeNull();
  });
});

// ─── audio: fades and punch ────────────────────────────────────────────────────────────────
// 120 bpm throughout: a beat is half a second, so a tick is 1/1920 s.
const secondsAt = (tick: number) => tick / (2 * B);
const audio = (id: string, startBeats: number, lengthSeconds: number, over: Partial<AudioClip> = {}): AudioClip => ({
  id,
  trackId: "gtr",
  sourceId: "src",
  startTick: startBeats * B,
  lengthSeconds,
  offsetSeconds: 0,
  gain: 1,
  fadeInSeconds: 0,
  fadeOutSeconds: 0,
  muted: false,
  ...over,
});

describe("clip fades", () => {
  it("caps a fade at five seconds and at half the clip", () => {
    expect(clampFade(0.4, 2)).toBe(0.4);
    expect(clampFade(3, 2)).toBe(1);
    expect(clampFade(30, 40)).toBe(5);
    expect(clampFade(-1, 2)).toBe(0);
    expect(clampFade(Number.NaN, 2)).toBe(0);
  });

  it("draws the curve the engine plays: equal power, not a straight line", () => {
    expect(equalPowerGain(0)).toBe(0);
    expect(equalPowerGain(1)).toBeCloseTo(1, 12);
    expect(equalPowerGain(0.5)).toBeCloseTo(Math.SQRT1_2, 12);
    // The point of it: the two halves of a seam sum to one in power.
    for (const t of [0.1, 0.25, 0.5, 0.75, 0.9]) {
      expect(equalPowerGain(t) ** 2 + equalPowerGain(1 - t) ** 2).toBeCloseTo(1, 12);
    }
    expect(equalPowerGain(-3)).toBe(0);
    expect(equalPowerGain(9)).toBeCloseTo(1, 12);
  });
});

describe("punch split", () => {
  let n = 0;
  const newId = () => `new-${++n}`;

  it("cuts the clip underneath into a head and a tail that crossfade with the take", () => {
    // A two-second take from beat 0, punched from 1 s (beat 2) to 1.5 s.
    const original = audio("a", 0, 2);
    const take = audio("take", 2, 0.5, { id: "take", sourceId: "punched" });
    const out = punchSplit([original], take, XF, secondsAt, newId);
    expect(out).toHaveLength(2);
    const [head, tail] = out;

    expect(head.id).toBe("a");
    expect(head.startTick).toBe(0);
    expect(head.lengthSeconds).toBeCloseTo(1 + XF, 9);
    expect(head.fadeOutSeconds).toBeCloseTo(XF, 9);
    expect(head.fadeInSeconds).toBe(0);

    // The tail lands on the nearest whole tick to 1.485 s and plays the material that lives
    // there, so nothing slides against the file.
    expect(tail.id).not.toBe("a");
    expect(secondsAt(tail.startTick)).toBeCloseTo(1.5 - XF, 3);
    expect(tail.offsetSeconds).toBeCloseTo(secondsAt(tail.startTick), 9);
    expect(tail.offsetSeconds + tail.lengthSeconds).toBeCloseTo(2, 9);
    expect(tail.fadeInSeconds).toBeCloseTo(XF, 9);
    expect(tail.fadeOutSeconds).toBe(0);
  });

  it("leaves clips the punch misses, and clips on other tracks, alone", () => {
    const before = audio("before", 0, 0.5);
    const after = audio("after", 8, 1);
    const elsewhere = audio("vox", 2, 2, { trackId: "vox" });
    const take = audio("take", 2, 0.5);
    expect(punchSplit([before, after, elsewhere], take, XF, secondsAt, newId)).toEqual([
      before,
      after,
      elsewhere,
    ]);
  });

  it("drops a clip buried under the take and keeps only the tail of one it starts on", () => {
    const buried = audio("buried", 2, 0.2);
    const overlapping = audio("late", 2, 2);
    const take = audio("take", 2, 0.5);
    const out = punchSplit([buried, overlapping], take, XF, secondsAt, newId);
    expect(out).toHaveLength(1);
    // Nothing in front of it to fade out, so it keeps its identity rather than forking.
    expect(out[0].id).toBe("late");
    expect(secondsAt(out[0].startTick)).toBeCloseTo(1.5 - XF, 3);
    expect(out[0].fadeInSeconds).toBeCloseTo(XF, 9);
  });

  it("never lets the seam overlap itself on a very short take", () => {
    const original = audio("a", 0, 4);
    const take = audio("take", 2, 0.01);
    const [head, tail] = punchSplit([original], take, XF, secondsAt, newId);
    expect(head.fadeOutSeconds).toBeCloseTo(0.005, 9);
    expect(tail.fadeInSeconds).toBeCloseTo(0.005, 9);
  });

  it("finds the tick nearest a time on a map that only increases", () => {
    expect(tickAtSeconds(secondsAt, 0)).toBe(0);
    expect(tickAtSeconds(secondsAt, -5)).toBe(0);
    expect(tickAtSeconds(secondsAt, 0.5)).toBe(B);
    expect(tickAtSeconds(secondsAt, 1.485)).toBe(2851);
  });
});
