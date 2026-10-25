import { describe, expect, it } from "vitest";
import {
  adjustVelocity,
  duplicateNotes,
  moveNotes,
  noteAt,
  notesInRect,
  pasteNotes,
  placeNote,
  resizeNotes,
  snapFloor,
  tileGhost,
} from "./noteEditing";
import type { NoteEvent } from "./types";

const n = (id: string, start: number, len: number, pitch: number, velocity = 100): NoteEvent => ({
  id,
  startBeat: start,
  lengthBeats: len,
  pitch,
  velocity,
});

describe("placement", () => {
  it("snaps to the grid and refuses overlaps on the same pitch", () => {
    const notes = [n("a", 0, 1, 60)];
    expect(placeNote(notes, 0.7, 60, 1, 0.5)).toBeNull();
    const placed = placeNote(notes, 1.3, 60, 0.5, 0.5);
    expect(placed?.startBeat).toBe(1);
    expect(placed?.lengthBeats).toBe(0.5);
    expect(placeNote(notes, 0.2, 62, 1, 1)?.pitch).toBe(62);
  });

  it("snapFloor is stable at grid boundaries", () => {
    expect(snapFloor(0.9999999999, 0.25)).toBe(1);
    expect(snapFloor(1.24, 0.25)).toBe(1);
  });

  it("finds the note under a point, preferring the latest", () => {
    const notes = [n("a", 0, 2, 60), n("b", 1, 1, 60)];
    expect(noteAt(notes, 1.5, 60)?.id).toBe("b");
    expect(noteAt(notes, 0.5, 60)?.id).toBe("a");
    expect(noteAt(notes, 0.5, 61)).toBeNull();
  });
});

describe("move", () => {
  const notes = [n("a", 1, 1, 60), n("b", 3, 1, 64), n("c", 8, 1, 50)];

  it("moves the whole selection together", () => {
    const out = moveNotes(notes, new Set(["a", "b"]), 2, -1, { min: 0, max: 127 });
    expect(out.find((x) => x.id === "a")).toMatchObject({ startBeat: 3, pitch: 59 });
    expect(out.find((x) => x.id === "b")).toMatchObject({ startBeat: 5, pitch: 63 });
    expect(out.find((x) => x.id === "c")).toMatchObject({ startBeat: 8, pitch: 50 });
  });

  it("clamps so nothing goes before beat 0 or outside the pitch range", () => {
    const out = moveNotes(notes, new Set(["a", "b"]), -5, 70, { min: 24, max: 84 });
    expect(out.find((x) => x.id === "a")!.startBeat).toBe(0);
    expect(out.find((x) => x.id === "b")!.startBeat).toBe(2);
    expect(out.find((x) => x.id === "b")!.pitch).toBe(84);
    expect(out.find((x) => x.id === "a")!.pitch).toBe(80);
  });

  it("respects a maximum end", () => {
    const out = moveNotes(notes, new Set(["c"]), 10, 0, { min: 0, max: 127 }, 16);
    expect(out.find((x) => x.id === "c")!.startBeat).toBe(15);
  });

  it("returns the same array when nothing changes", () => {
    expect(moveNotes(notes, new Set(["zzz"]), 1, 1, { min: 0, max: 127 })).toBe(notes);
  });
});

describe("resize / select / velocity", () => {
  const notes = [n("a", 0, 1, 60), n("b", 2, 2, 62)];

  it("resizes with a minimum length", () => {
    const out = resizeNotes(notes, new Set(["a", "b"]), -5, 0.25);
    expect(out.every((x) => x.lengthBeats === 0.25)).toBe(true);
  });

  it("marquee selects overlapping notes in any drag direction", () => {
    expect([...notesInRect(notes, 3, 0.5, 63, 60)].sort()).toEqual(["a", "b"]);
    expect([...notesInRect(notes, 1, 1.9, 60, 62)]).toEqual([]);
  });

  it("clamps velocity", () => {
    const out = adjustVelocity(notes, new Set(["a"]), 200);
    expect(out[0].velocity).toBe(127);
    expect(adjustVelocity(out, new Set(["a"]), -500)[0].velocity).toBe(1);
  });
});

describe("duplicate / paste", () => {
  it("duplicates right after the selection on the grid with fresh ids", () => {
    const notes = [n("a", 0, 1, 60), n("b", 1, 0.5, 62)];
    const { notes: out, newIds } = duplicateNotes(notes, new Set(["a", "b"]), 0.5);
    expect(out).toHaveLength(4);
    expect(newIds.size).toBe(2);
    const copies = out.filter((x) => newIds.has(x.id)).sort((p, q) => p.startBeat - q.startBeat);
    expect(copies[0]).toMatchObject({ startBeat: 1.5, pitch: 60 });
    expect(copies[1]).toMatchObject({ startBeat: 2.5, pitch: 62 });
  });

  it("pastes relative to the earliest clipboard note", () => {
    const clip = [n("x", 4, 1, 60), n("y", 5, 1, 64)];
    const { notes: out } = pasteNotes([], clip, 8);
    expect(out.map((x) => x.startBeat).sort()).toEqual([8, 9]);
    expect(out.every((x) => x.id !== "x" && x.id !== "y")).toBe(true);
  });
});

describe("tileGhost", () => {
  it("loops a shorter ghost across the pattern and cuts at the end", () => {
    const ghost = [n("a", 0, 1, 60), n("b", 3, 2, 62)];
    const out = tileGhost(ghost, 4, 10);
    expect(out.map((x) => [x.startBeat, x.lengthBeats])).toEqual([
      [0, 1],
      [3, 2],
      [4, 1],
      [7, 2],
      [8, 1],
    ]);
    expect(new Set(out.map((x) => x.id)).size).toBe(out.length);
  });
  it("leaves muted notes and notes past the ghost's own end out", () => {
    const ghost = [n("a", 0, 1, 60), { ...n("m", 1, 1, 61), muted: true }, n("late", 5, 1, 64)];
    expect(tileGhost(ghost, 4, 4).map((x) => x.pitch)).toEqual([60]);
  });
});
