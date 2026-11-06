// The store's selection invariant and playback context, driven against the browser mock.
//
// Invariant: a selected pattern always belongs to the selected track. The playback context the
// engine gets carries the pattern and the song tick its chords come from (`chordOffsetTicks`).

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Bridge } from "./bridge";
import type { ProjectModel } from "./types";
import { TICKS_PER_BEAT as B } from "./types";

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
    expect(st().screen).toBe("setup");
    const m = model();
    expect(st().selectedTrackId).toBe(m.tracks[0].id);
    expect(st().selectedPatternId).toBe(m.patterns[0].id);
    expect(st().chordOffsetTicks).toBe(0);
    expect(lastContext()).toEqual(["song", m.patterns[0].id, 0, null, 1]);
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
    await st().dispatch({ type: "addSection", name: "chorus", startTick: 64 * B });
    const section = model().sections[0];
    await st().dispatch({ type: "updateSection", section: { ...section, key: { rootPitchClass: 9, scale: "minor", isLocked: true } } });
    const drums = model().patterns[1];
    st().openPattern(drums.id, 64 * B);
    await settle();
    expect(st().screen).toBe("pattern");
    expect([st().selectedTrackId, st().selectedPatternId, st().chordOffsetTicks]).toEqual([drums.trackId, drums.id, 64 * B]);
    expect(lastContext()).toEqual(["pattern", drums.id, 64 * B, null, 1]);
    expect(S.effectiveKey(st()).rootPitchClass).toBe(9);
    // The chords the roll tiers against start at song beat 64, shifted to 0.
    expect(S.patternChords(st()).map((c) => [c.startTick / B, c.name])).toEqual([
      [0, "C"],
      [4, "F"],
      [8, "G"],
      [12, "Am"],
    ]);
  });

  it("opens from the tree at song bar 1", async () => {
    st().openPattern(model().patterns[1].id, 32 * B);
    st().openPattern(model().patterns[0].id);
    await settle();
    expect(st().chordOffsetTicks).toBe(0);
    expect(lastContext()).toEqual(["pattern", model().patterns[0].id, 0, null, 1]);
  });

  it("selecting another pattern resets the chord offset", async () => {
    st().openPattern(model().patterns[0].id, 32 * B);
    st().selectPattern(model().patterns[1].id);
    await settle();
    expect(st().chordOffsetTicks).toBe(0);
  });

  it("plays a picked ghost along unless hearing it is off, and never the edited pattern itself", async () => {
    const [melody, drums] = model().patterns;
    st().openPattern(melody.id);
    st().setGhostPattern(drums.id);
    await settle();
    expect(lastContext()).toEqual(["pattern", melody.id, 0, drums.id, 1]);
    st().toggleHearGhost();
    await settle();
    expect(st().hearGhost).toBe(false);
    expect(lastContext()).toEqual(["pattern", melody.id, 0, null, 1]);
    st().toggleHearGhost();
    st().selectPattern(drums.id);
    await settle();
    expect(lastContext()).toEqual(["pattern", drums.id, 0, null, 1]);
    // Deleting the ghost pattern drops it from playback too.
    st().selectPattern(melody.id);
    await st().dispatch({ type: "deletePattern", id: drums.id });
    await settle();
    expect(st().ghostPatternId).toBe(null);
    expect(lastContext()).toEqual(["pattern", melody.id, 0, null, 1]);
  });

  it("sends the ghost volume, clamped to 0…150%", async () => {
    const [melody, drums] = model().patterns;
    st().openPattern(melody.id);
    st().setGhostPattern(drums.id);
    st().setGhostLevel(0.4);
    await settle();
    expect(lastContext()).toEqual(["pattern", melody.id, 0, drums.id, 0.4]);
    st().setGhostLevel(9);
    expect(st().ghostLevel).toBe(1.5);
    st().setGhostLevel(-1);
    expect(st().ghostLevel).toBe(0);
  });

  it("SETUP, SONG and the rec page all play the song", async () => {
    st().setScreen("pattern");
    await settle();
    expect(lastContext()?.[0]).toBe("pattern");
    st().setScreen("song");
    await settle();
    expect(lastContext()?.[0]).toBe("song");
    const calls = context.mock.calls.length;
    st().setScreen("setup");
    st().setScreen("rec");
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

describe("grid options", () => {
  it("are whole ticks, in order, with exact triplets and dotted values", () => {
    const byLabel = new Map(S.GRID_OPTIONS);
    for (const [, ticks] of S.GRID_OPTIONS) expect(Number.isInteger(ticks)).toBe(true);
    const values = S.GRID_OPTIONS.map(([, v]) => v);
    expect([...values].sort((a, b) => a - b)).toEqual(values);
    expect(new Set(values).size).toBe(values.length);
    // A triplet is two thirds of the straight value, a dot adds half of it — and at 960 PPQ
    // every one of those lands on a whole tick, which is the point of counting in ticks.
    for (const straight of ["1/16", "1/8", "1/4", "1/2"]) {
      const base = byLabel.get(straight)!;
      expect(byLabel.get(`${straight}T`)).toBe((base * 2) / 3);
      expect(byLabel.get(`${straight}.`)).toBe((base * 3) / 2);
    }
    expect(byLabel.get("1/8T")).toBe(320);
    expect(byLabel.get("1/16T")).toBe(160);
  });

  it("labels a tick value, and says so plainly when it is off the list", () => {
    expect(S.gridLabel(320)).toBe("1/8T");
    expect(S.gridLabel(720)).toBe("1/8.");
    expect(S.gridLabel(7)).toBe("7");
  });
});

describe("zoom memory", () => {
  // The reload is the point of these: `import` after `resetModules` is a relaunch, which is
  // where a plain piece of store state used to lose the zoom.
  it("keeps every zoom across a reload", async () => {
    st().setZoom(2.5);
    st().setArrZoom(80);
    S.setStoredZoom("setup", 12);
    vi.resetModules();
    const reloaded = await import("./store");
    expect(reloaded.useStore.getState().zoom).toBe(2.5);
    expect(reloaded.useStore.getState().arrZoom).toBe(80);
    expect(reloaded.storedZoom("setup")).toBe(12);
  });

  it("holds a zoom inside its range and remembers SETUP's fit", async () => {
    st().setZoom(99);
    expect(st().zoom).toBe(3);
    st().setArrZoom(1);
    expect(st().arrZoom).toBe(12);
    S.setStoredZoom("setup", null);
    vi.resetModules();
    const reloaded = await import("./store");
    expect(reloaded.storedZoom("setup")).toBeNull();
    expect(reloaded.useStore.getState().zoom).toBe(3);
  });
});


describe("the REC page's track", () => {
  it("follows the arm over the selection", async () => {
    // The starter project may have no audio tracks at all, so make the pair the rule needs.
    while (model().tracks.filter((t) => t.isAudio).length < 2) await st().addAudioTrack();
    const tracks = model().tracks.filter((t) => t.isAudio);
    // A new audio track arms itself, which is the behaviour under test: clear it first.
    await st().setArm(null);
    st().selectTrack(tracks[0].id);
    expect(S.recTrack(st())?.id).toBe(tracks[0].id);
    await st().setArm(tracks[1].id);
    // Armed wins: a take lands on the armed track, so that is the one the page shows.
    expect(S.recTrack(st())?.id).toBe(tracks[1].id);
    await st().setArm(null);
    expect(S.recTrack(st())?.id).toBe(tracks[0].id);
  });
});

describe("the tuner", () => {
  it("closes itself when the screen changes, so nothing is left muted", async () => {
    S.useStore.setState({ screen: "rec" });
    await st().toggleTuner();
    expect(st().tunerOpen).toBe(true);
    st().setScreen("song");
    await settle();
    expect(st().tunerOpen).toBe(false);
  });

  it("closes itself when the transport starts", async () => {
    S.useStore.setState({ screen: "rec" });
    await st().toggleTuner();
    expect(st().tunerOpen).toBe(true);
    st().play();
    await settle();
    expect(st().tunerOpen).toBe(false);
  });

  it("forgets its reading on both edges", async () => {
    S.useStore.setState({ screen: "rec", tunerHz: 110, tunerClarity: 0.9 });
    await st().toggleTuner();
    expect(st().tunerHz).toBeNull();
    S.useStore.setState({ tunerHz: 110, tunerClarity: 0.9 });
    await st().toggleTuner();
    expect(st().tunerHz).toBeNull();
    expect(st().tunerClarity).toBe(0);
  });
});