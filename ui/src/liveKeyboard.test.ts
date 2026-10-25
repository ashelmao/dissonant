// Tier mode's key: the section key at the song position under the playhead. In PATTERN that
// position is `chordOffsetBeats` + the pattern-relative playhead; in SETUP / SONG it is the playhead.

import { beforeAll, describe, expect, it, vi } from "vitest";
import type { KeyState, ProjectModel } from "./types";

beforeAll(() => {
  vi.stubGlobal("requestAnimationFrame", () => 0);
  vi.stubGlobal("document", { title: "" });
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  });
});

const C_MAJOR: KeyState = { rootPitchClass: 0, scale: "major", isLocked: true };
const D_MAJOR: KeyState = { rootPitchClass: 2, scale: "major", isLocked: true };

// No chords, so the home row is the key's scale tones from C3 (48): C major starts on C (48),
// D major on C♯ (49). The second section (D major) starts at song beat 32.
function state(mode: "pattern" | "song", playhead: number, chordOffsetBeats: number) {
  const model = {
    key: C_MAJOR,
    chords: [],
    sections: [
      { id: "a", name: "a", startBeat: 0, key: null, color: null },
      { id: "b", name: "b", startBeat: 32, key: D_MAJOR, color: null },
    ],
    patterns: [{ id: "p", trackId: "t", name: "p", lengthBeats: 16, notes: [] }],
    tracks: [{ id: "t" }],
  } as unknown as ProjectModel;
  return { mode, playhead, chordOffsetBeats, selectedPatternId: "p", selectedTrackId: "t", snapshot: { model } };
}

type LK = typeof import("./liveKeyboard");
let pitchForKey: LK["pitchForKey"];
beforeAll(async () => {
  ({ pitchForKey } = await import("./liveKeyboard"));
});
const home = (s: ReturnType<typeof state>) => pitchForKey("a", "tier", 0, s as unknown as Parameters<LK["pitchForKey"]>[3]);

describe("pitchForKey in tier mode", () => {

  it("song: follows the section at the playhead", () => {
    expect(home(state("song", 4, 0))).toBe(48);
    expect(home(state("song", 33, 0))).toBe(49);
  });

  it("pattern: follows the section at chordOffsetBeats + playhead", () => {
    expect(home(state("pattern", 1, 0))).toBe(48);
    expect(home(state("pattern", 1, 32))).toBe(49);
    // A pattern sitting across the section change: 28 + 6 = song beat 34, in D.
    expect(home(state("pattern", 6, 28))).toBe(49);
  });
});
