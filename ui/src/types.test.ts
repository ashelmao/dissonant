import { describe, expect, it } from "vitest";
import type { ChordEvent } from "./types";
import { chordsInWindow, replaceChordWindow } from "./types";

const ch = (id: string, startBeat: number, lengthBeats: number): ChordEvent => ({ id, startBeat, lengthBeats, pitchClasses: [0, 4, 7], name: id });
const spans = (cs: ChordEvent[]) => cs.map((c) => [c.name, c.startBeat, c.lengthBeats]);

describe("chordsInWindow", () => {
  const song = [ch("C", 0, 4), ch("F", 4, 4), ch("G", 8, 4), ch("Am", 12, 4)];

  it("shifts the window to 0 and clips chords at its edges", () => {
    expect(spans(chordsInWindow(song, 6, 8))).toEqual([
      ["F", 0, 2],
      ["G", 2, 4],
      ["Am", 6, 2],
    ]);
  });

  it("is the whole progression from 0", () => {
    expect(spans(chordsInWindow(song, 0, 16))).toEqual(spans(song));
    expect(chordsInWindow(song, 16, 8)).toEqual([]);
  });
});

describe("replaceChordWindow", () => {
  it("keeps what lies outside, trims chords crossing the edges, and places the new ones", () => {
    const song = [ch("C", 0, 8), ch("F", 8, 8)];
    const next = replaceChordWindow(song, 4, 8, [ch("G", 0, 8)]);
    expect(spans(next)).toEqual([
      ["C", 0, 4],
      ["G", 4, 8],
      ["F", 12, 4],
    ]);
  });

  it("splits a chord spanning the whole window into two with distinct ids", () => {
    const next = replaceChordWindow([ch("C", 0, 16)], 4, 4, []);
    expect(spans(next)).toEqual([
      ["C", 0, 4],
      ["C", 8, 8],
    ]);
    expect(next[0].id).not.toBe(next[1].id);
  });

  it("round-trips with chordsInWindow", () => {
    const song = [ch("C", 0, 4), ch("F", 4, 4), ch("G", 8, 4)];
    expect(spans(replaceChordWindow(song, 4, 4, chordsInWindow(song, 4, 4)))).toEqual(spans(song));
  });
});
