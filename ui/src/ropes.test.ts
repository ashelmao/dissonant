// The rope rules mirror crates/dissonant-core/src/model.rs; these cases match the Rust ones in
// crates/dissonant-core/tests/ropes.rs so a drift on either side fails a test.

import { describe, expect, it } from "vitest";
import type { NoteEvent } from "./types";
import { TICKS_PER_BEAT } from "./types";
import { remapRopes, repointRopes, ropeIsLive, tidyRopes } from "./ropes";
import { arpeggiateNotes, chopNotes, copyNotes, duplicateBySpan, duplicateNotes, glueNotes, pasteNotes, splitNotes, tileGhost } from "./noteEditing";

const BEAT = TICKS_PER_BEAT;

function note(id: string, startTick: number, lengthTicks: number, pitch: number, extra: Partial<NoteEvent> = {}): NoteEvent {
  return { id, startTick, lengthTicks, pitch, velocity: 100, ...extra };
}

describe("ropeIsLive", () => {
  const a = note("a", 0, BEAT, 60);

  it("is live while the source still sounds, touching included", () => {
    expect(ropeIsLive(a, note("b", BEAT / 2, BEAT, 62))).toBe(true);
    expect(ropeIsLive(a, note("b", BEAT, BEAT, 62))).toBe(true);
    expect(ropeIsLive(a, note("b", BEAT + 1, BEAT, 62))).toBe(false);
    expect(ropeIsLive(a, note("b", 0, BEAT, 62))).toBe(false);
    expect(ropeIsLive(note("a", BEAT, BEAT, 60), note("b", 0, BEAT, 62))).toBe(false);
  });

  it("measures shifted starts, never before tick 0", () => {
    expect(ropeIsLive(a, note("b", BEAT, BEAT, 62, { shiftTicks: 1 }))).toBe(false);
    expect(ropeIsLive(a, note("b", BEAT, BEAT, 62, { shiftTicks: -BEAT / 2 }))).toBe(true);
    expect(ropeIsLive(a, note("b", 0, BEAT, 62, { shiftTicks: -BEAT }))).toBe(false);
  });

  it("is never live from a muted source or a note on itself", () => {
    expect(ropeIsLive(a, a)).toBe(false);
    expect(ropeIsLive({ ...a, muted: true }, note("b", BEAT / 2, BEAT, 62))).toBe(false);
  });

  it("stays live to the same pitch, as a tie", () => {
    expect(ropeIsLive(a, note("b", BEAT, BEAT, 60))).toBe(true);
  });
});

describe("tidyRopes", () => {
  const a = note("a", 0, BEAT, 60);
  const b = note("b", BEAT, BEAT, 62, { slideFrom: "a" });
  const c = note("c", 2 * BEAT, BEAT, 64, { slideFrom: "b" });

  it("keeps chains, returning the same array", () => {
    const notes = [a, b, c];
    expect(tidyRopes(notes)).toBe(notes);
  });

  it("cuts a loop once, at its first note, and keeps the rest", () => {
    const out = tidyRopes([{ ...a, slideFrom: "c", slideSecs: 0.3 }, b, c]);
    expect(out[0]).toEqual(a);
    expect(out[0]).not.toHaveProperty("slideFrom");
    expect(out[0]).not.toHaveProperty("slideSecs");
    expect(out[1].slideFrom).toBe("a");
    expect(out[2].slideFrom).toBe("b");
  });

  it("cuts self ropes, dangling ropes and times with no rope", () => {
    const out = tidyRopes([
      { ...a, slideFrom: "a", slideSecs: 1 },
      note("x", BEAT, BEAT, 62, { slideFrom: "gone", slideSecs: 1 }),
      note("y", 2 * BEAT, BEAT, 64, { slideSecs: 1 }),
    ]);
    for (const n of out) {
      expect(n).not.toHaveProperty("slideFrom");
      expect(n).not.toHaveProperty("slideSecs");
    }
  });

  it("stops at a loop further up without cutting the note below it", () => {
    const out = tidyRopes([
      note("b", BEAT, BEAT, 62, { slideFrom: "c" }),
      note("c", 2 * BEAT, BEAT, 64, { slideFrom: "b" }),
      note("d", 3 * BEAT, BEAT, 65, { slideFrom: "c" }),
    ]);
    expect(out.map((n) => n.slideFrom)).toEqual([undefined, "b", "c"]);
  });
});

describe("remapRopes and repointRopes", () => {
  it("follows copied sources and drops ropes whose source stayed behind", () => {
    const out = remapRopes(
      [note("b2", BEAT, BEAT, 62, { slideFrom: "a", slideSecs: 0.5 }), note("c2", 2 * BEAT, BEAT, 64, { slideFrom: "z", slideSecs: 0.5 })],
      new Map([["a", "a2"]]),
    );
    expect(out[0]).toMatchObject({ slideFrom: "a2", slideSecs: 0.5 });
    expect(out[1]).not.toHaveProperty("slideFrom");
    expect(out[1]).not.toHaveProperty("slideSecs");
  });

  it("moves a rope to the piece sounding when the slide note starts", () => {
    const left = note("a", 0, BEAT, 60);
    const right = note("a2", BEAT, BEAT, 60);
    const early = note("x", BEAT / 2, BEAT, 62, { slideFrom: "a" });
    const late = note("y", 2 * BEAT, BEAT, 62, { slideFrom: "a" });
    const out = repointRopes([left, right, early, late], new Map([["a", [left, right]]]));
    expect(out[2].slideFrom).toBe("a");
    expect(out[3].slideFrom).toBe("a2");
  });
});

describe("note editing keeps ropes valid", () => {
  const a = note("a", 0, BEAT, 60);
  const b = note("b", BEAT, BEAT, 62, { slideFrom: "a", slideSecs: 0.25 });

  it("duplicates a whole phrase with its rope pointing inside the copy", () => {
    const { notes, newIds } = duplicateNotes([a, b], new Set(["a", "b"]), BEAT);
    const [ca, cb] = notes.filter((n) => newIds.has(n.id));
    expect(cb.slideFrom).toBe(ca.id);
    expect(cb.slideSecs).toBe(0.25);
    expect(notes.find((n) => n.id === "b")!.slideFrom).toBe("a");
  });

  it("drops the rope of a copied slide note whose source was not copied", () => {
    for (const { notes, newIds } of [duplicateNotes([a, b], new Set(["b"]), BEAT), duplicateBySpan([a, b], new Set(["b"]), 4 * BEAT)]) {
      const copy = notes.find((n) => newIds.has(n.id))!;
      expect(copy).not.toHaveProperty("slideFrom");
      expect(copy).not.toHaveProperty("slideSecs");
    }
  });

  it("pastes ropes among the pasted notes, and drops one whose source fell past the end", () => {
    const pasted = pasteNotes([], [a, b], 8 * BEAT);
    const [pa, pb] = pasted.notes;
    expect(pb.slideFrom).toBe(pa.id);
    // Source dropped by the end of a fixed-length pattern: here, `b` slides from a later note.
    const late = note("l", 3 * BEAT, BEAT, 60);
    const early = note("e", 0, BEAT, 62, { slideFrom: "l" });
    const cut = pasteNotes([], [early, late], 0, 2 * BEAT);
    expect(cut.notes).toHaveLength(1);
    expect(cut.notes[0]).not.toHaveProperty("slideFrom");
  });

  it("copyNotes with no placement is a straight copy with fresh ids", () => {
    const [ca, cb] = copyNotes([a, b]);
    expect(ca.id).not.toBe("a");
    expect(cb.slideFrom).toBe(ca.id);
  });

  it("splitting the source moves the rope to the half still sounding", () => {
    const c = note("c", 2 * BEAT, BEAT, 64, { slideFrom: "a" });
    const long = { ...a, lengthTicks: 2 * BEAT };
    const out = splitNotes([long, c], new Set(["a"]), BEAT);
    const right = out.find((n) => n.startTick === BEAT && n.pitch === 60)!;
    expect(out.find((n) => n.id === "c")!.slideFrom).toBe(right.id);
  });

  it("splitting the slide note leaves the rope on the left half only", () => {
    const long = { ...b, lengthTicks: 2 * BEAT };
    const out = splitNotes([a, long], new Set(["b"]), 3 * BEAT / 2);
    const halves = out.filter((n) => n.pitch === 62);
    expect(halves[0]).toMatchObject({ id: "b", slideFrom: "a" });
    expect(halves[1]).not.toHaveProperty("slideFrom");
  });

  it("chopping does the same piece by piece", () => {
    const c = note("c", 2 * BEAT, BEAT, 64, { slideFrom: "a" });
    const out = chopNotes([{ ...a, lengthTicks: 2 * BEAT }, c], new Set(["a"]), BEAT / 2);
    const last = out.filter((n) => n.pitch === 60).at(-1)!;
    expect(out.find((n) => n.id === "c")!.slideFrom).toBe(last.id);
    expect(out.filter((n) => n.pitch === 60 && n.slideFrom)).toHaveLength(0);
  });

  it("gluing re-points ropes to the merged note and cuts one that ran between the glued notes", () => {
    // `a` slides from `a2` (a dead rope, but a rope): once glued it would slide from itself.
    const a2 = note("a2", BEAT, BEAT, 60);
    const c = note("c", 2 * BEAT, BEAT, 64, { slideFrom: "a2" });
    const out = glueNotes([{ ...a, slideFrom: "a2" }, a2, c], new Set(["a", "a2"]));
    expect(out).toHaveLength(2);
    expect(out[0]).toMatchObject({ id: "a", lengthTicks: 2 * BEAT });
    expect(out[0]).not.toHaveProperty("slideFrom");
    expect(out[1].slideFrom).toBe("a");
  });

  it("arpeggiated steps never inherit a rope, and ropes out of the stack go", () => {
    const stack = [note("p", 0, BEAT, 60), note("q", 0, BEAT, 64, { slideFrom: "p" })];
    const after = note("r", BEAT, BEAT, 67, { slideFrom: "q" });
    const out = arpeggiateNotes([...stack, after], new Set(["p", "q"]), BEAT / 4, "up");
    expect(out.every((n) => !n.slideFrom)).toBe(true);
  });

  it("ghost tiles rope each loop to its own source tile", () => {
    const tiles = tileGhost([a, b], 2 * BEAT, 4 * BEAT);
    expect(tiles.map((t) => t.slideFrom)).toEqual([undefined, "a~0", undefined, "a~1"]);
  });
});
