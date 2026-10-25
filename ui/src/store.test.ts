// The store's selection invariant and playback context, driven against the browser mock.
//
// Invariant: a selected pattern always belongs to the selected track. The playback context the
// engine gets carries the pattern and the song beat its chords come from (`chordOffsetBeats`).

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Bridge } from "./bridge";
import type { ProjectModel } from "./types";

beforeAll(() => {
  vi.stubGlobal("requestAnimationFrame", () => 0);
  // The mock sets the window title through `document`.
  vi.stubGlobal("document", { title: "" });
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  });
});

type Store = typeof import("./store");
let S: Store;
let b: Bridge;
let context: ReturnType<typeof vi.spyOn>;

const st = () => S.useStore.getState();
const model = (): ProjectModel => st().snapshot!.model;
/** Let the store's fire-and-forget bridge calls land. */
const settle = () => new Promise((r) => setTimeout(r, 0));
const lastContext = () => context.mock.calls.at(-1);

beforeEach(async () => {
  localStorage.clear();
  vi.resetModules();
  S = await import("./store");
  b = await (await import("./bridge")).getBridge();
  context = vi.spyOn(b, "setPlaybackContext");
  // A fresh starter, the way the new-project screen makes one.
  await st().newFromTemplate("starter");
  await settle();
});

describe("fresh projects", () => {
  it("open on SETUP, with song playback and the first track's first pattern selected", () => {
    expect(st().mode).toBe("setup");
    const m = model();
    expect(st().selectedTrackId).toBe(m.tracks[0].id);
    expect(st().selectedPatternId).toBe(m.patterns[0].id);
    expect(st().chordOffsetBeats).toBe(0);
    expect(lastContext()).toEqual(["song", m.patterns[0].id, 0]);
  });

  it("turn the chord bed on", () => {
    expect(st().hearChords).toBe(true);
  });
});

describe("selection invariant", () => {
  it("selectPattern also selects its track", () => {
    const drums = model().patterns[1];
    st().selectPattern(drums.id);
    expect(st().selectedPatternId).toBe(drums.id);
    expect(st().selectedTrackId).toBe(drums.trackId);
  });

  it("selectTrack selects that track's first pattern, or none", async () => {
    const [melody, drums] = model().tracks;
    st().selectTrack(drums.id);
    expect(st().selectedPatternId).toBe(model().patterns[1].id);
    // A track with no patterns: nothing selected, not another track's pattern.
    await st().dispatch({ type: "deletePattern", id: model().patterns[0].id });
    st().selectTrack(melody.id);
    expect(st().selectedTrackId).toBe(melody.id);
    expect(st().selectedPatternId).toBeNull();
    expect(S.selectedPattern(st())).toBeNull();
  });

  it("selectTrack keeps the selected pattern when it is already on that track", async () => {
    const melody = model().tracks[0];
    await st().dispatch({ type: "addPattern", trackId: melody.id });
    const second = model().patterns.find((p) => p.name === "melody 2")!;
    st().selectPattern(second.id);
    st().selectTrack(melody.id);
    expect(st().selectedPatternId).toBe(second.id);
  });

  it("survives the selected pattern or track being deleted", async () => {
    const [melody, drums] = model().tracks;
    st().selectPattern(model().patterns[1].id);
    await st().dispatch({ type: "deleteTrack", id: drums.id });
    expect(st().selectedTrackId).toBe(melody.id);
    expect(st().selectedPatternId).toBe(model().patterns[0].id);
    // Deleting the selected pattern falls back to the same track's next one (here: none).
    await st().dispatch({ type: "deletePattern", id: model().patterns[0].id });
    expect([st().selectedTrackId, st().selectedPatternId]).toEqual([melody.id, null]);
  });

  it("drops selected clips and the ghost pattern when they disappear", async () => {
    const [a, c] = model().clips;
    st().selectClips([a.id]);
    st().selectClips([c.id], true);
    expect(st().selectedClipIds).toEqual([a.id, c.id]);
    st().setGhostPattern(model().patterns[1].id);
    await st().dispatch({ type: "deleteTrack", id: model().tracks[1].id });
    expect(st().selectedClipIds).toEqual([a.id]);
    expect(st().ghostPatternId).toBeNull();
  });
});

describe("openPattern and the playback context", () => {
  it("opens at a clip's start: pattern mode, chord offset, section key", async () => {
    await st().dispatch({ type: "addSection", name: "chorus", startBeat: 64 });
    const section = model().sections[0];
    await st().dispatch({ type: "updateSection", section: { ...section, key: { rootPitchClass: 9, scale: "minor", isLocked: true } } });
    const drums = model().patterns[1];
    st().openPattern(drums.id, 64);
    await settle();
    expect(st().mode).toBe("pattern");
    expect([st().selectedTrackId, st().selectedPatternId, st().chordOffsetBeats]).toEqual([drums.trackId, drums.id, 64]);
    expect(lastContext()).toEqual(["pattern", drums.id, 64]);
    expect(S.effectiveKey(st()).rootPitchClass).toBe(9);
    // The chords the roll tiers against start at song beat 64, shifted to 0.
    expect(S.patternChords(st()).map((c) => [c.startBeat, c.name])).toEqual([
      [0, "C"],
      [4, "F"],
      [8, "G"],
      [12, "Am"],
    ]);
  });

  it("opens from the tree at song bar 1", async () => {
    st().openPattern(model().patterns[1].id, 32);
    st().openPattern(model().patterns[0].id);
    await settle();
    expect(st().chordOffsetBeats).toBe(0);
    expect(lastContext()).toEqual(["pattern", model().patterns[0].id, 0]);
  });

  it("selecting another pattern resets the chord offset", async () => {
    st().openPattern(model().patterns[0].id, 32);
    st().selectPattern(model().patterns[1].id);
    await settle();
    expect(st().chordOffsetBeats).toBe(0);
  });

  it("SETUP and SONG both play the song", async () => {
    st().setMode("pattern");
    await settle();
    expect(lastContext()?.[0]).toBe("pattern");
    st().setMode("song");
    await settle();
    expect(lastContext()?.[0]).toBe("song");
    const calls = context.mock.calls.length;
    st().setMode("setup");
    await settle();
    // Same engine mode: no new context needed.
    expect(context.mock.calls.length).toBe(calls);
  });
});

describe("pattern drag", () => {
  it("hands the drop to whoever registered a handler", () => {
    const drop = vi.fn();
    st().setPatternDropHandler(drop);
    st().setPatternDrag({ patternId: "p", clientX: 1, clientY: 2 });
    expect(st().patternDrag).toEqual({ patternId: "p", clientX: 1, clientY: 2 });
    st().patternDropHandler?.("p", 3, 4);
    expect(drop).toHaveBeenCalledWith("p", 3, 4);
    st().setPatternDropHandler(null);
    expect(st().patternDropHandler).toBeNull();
  });
});
