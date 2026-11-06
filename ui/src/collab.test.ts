import { describe, expect, it } from "vitest";
import { nearestPitch, sameView, whereLabel, type PeerLocation } from "./collab";
import type { ProjectModel } from "./types";

describe("sameView", () => {
  const pattern = (patternId: string): PeerLocation => ({ screen: "pattern", patternId });

  it("puts two people in the song in the same place", () => {
    expect(sameView({ screen: "song" }, { screen: "song" })).toBe(true);
  });

  it("only counts a pattern when it's the same pattern", () => {
    expect(sameView(pattern("a"), pattern("a"))).toBe(true);
    expect(sameView(pattern("a"), pattern("b"))).toBe(false);
  });

  it("doesn't mix up screens, and never shows cursors on the ones with no timeline", () => {
    expect(sameView({ screen: "song" }, pattern("a"))).toBe(false);
    expect(sameView({ screen: "setup" }, { screen: "setup" })).toBe(false);
    expect(sameView({ screen: "rec" }, { screen: "rec" })).toBe(false);
  });

  it("ignores where the pointer is", () => {
    const moving: PeerLocation = { screen: "pattern", patternId: "a", cursor: { tick: 5, pitch: 60, frac: 0.5 } };
    expect(sameView(pattern("a"), moving)).toBe(true);
  });
});

describe("whereLabel", () => {
  const model = { patterns: [{ id: "p", name: "bass 2" }] } as unknown as ProjectModel;

  it("names the pattern", () => {
    expect(whereLabel({ screen: "pattern", patternId: "p" }, model)).toBe("in bass 2");
  });

  it("copes with a pattern that has gone", () => {
    expect(whereLabel({ screen: "pattern", patternId: "gone" }, model)).toBe("in a pattern");
  });

  it("says something before anything has been heard", () => {
    expect(whereLabel(undefined, model)).toBe("here");
  });
});

describe("nearestPitch", () => {
  it("draws a folded-away pitch on the closest row shown", () => {
    expect(nearestPitch(62, [48, 60, 67])).toBe(60);
    expect(nearestPitch(66, [48, 60, 67])).toBe(67);
  });

  it("has nothing to offer an empty roll", () => {
    expect(nearestPitch(60, [])).toBeNull();
  });
});
