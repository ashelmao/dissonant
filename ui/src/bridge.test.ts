// Parity tests for the browser mock bridge.
//
// `pnpm --dir ui dev` runs the mock, so every place the mock is more permissive than
// `dissonant-core` is a feature that looks fine in the browser and breaks in the desktop app.
// These tests assert the semantics of `Document::apply` / `apply_to`
// (crates/dissonant-core/src/document.rs) and `ProjectModel` (…/src/model.rs), not the mock's
// current behavior. The mock is expected to match Rust, including rejecting invalid commands:
// over Tauri IPC an `EditError` comes back as a rejected promise carrying the plain error
// string (`lib.rs::apply` does `.map_err(|e| e.to_string())`).

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Bridge } from "./bridge";
import type { AudioClip, AudioSource, AutoTarget, ChordEvent, Clip, Command, EqModel, InsertTarget, MasterSettings, MicModel, NoteEvent, ProjectModel } from "./types";
import { automationLane, defaultAmpRig, insertChain, layOutLoop, MAX_CLIP_GAIN, MAX_INSERTS, MAX_PEDALS, MIN_NOTE_TICKS, newPedal, PEDAL_KINDS, SOUND_PRESETS, soundPresetOf, TICKS_PER_BEAT } from "./types";
import { DEFAULT_DRUM_PATCH, DEFAULT_SYNTH_PATCH, defaultKit, FACTORY_SOUNDS, type DrumPad, type Sound } from "./sound";

const FAKE_ID = "00000000-0000-4000-8000-000000000000";

beforeAll(() => {
  // The mock drives a playhead off rAF and persists through localStorage; neither exists in
  // the node test environment.
  vi.stubGlobal("requestAnimationFrame", () => 0);
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
  });
});

/**
 * A bridge with its own state. `getBridge` memoizes its bridge in module scope, so a static
 * import would hand every test the same mock; re-importing after `vi.resetModules()` is the
 * only way to get a fresh one.
 */
async function freshBridge(): Promise<Bridge> {
  vi.resetModules();
  const { getBridge } = await import("./bridge");
  return getBridge();
}

let b: Bridge;
const model = async (): Promise<ProjectModel> => (await b.getState()).model;

/** The starter project's ids, in the order the UI sees them. `pattern` is the melody's. */
async function ids() {
  const m = await model();
  return { melody: m.tracks[0].id, drums: m.tracks[1].id, pattern: m.patterns[0].id, drumPattern: m.patterns[1].id };
}

/** One clip, looked up by id. */
async function clip(id: string): Promise<Clip> {
  return (await model()).clips.find((c) => c.id === id)!;
}

function note(startTick: number, pitch: number, extra: Partial<NoteEvent> = {}): NoteEvent {
  return { id: FAKE_ID, startTick, lengthTicks: TICKS_PER_BEAT, pitch, velocity: 100, ...extra };
}

/** Assert a command is rejected (as Rust's `EditError` would be) and leaves the model alone. */
async function rejectsAndKeepsModel(command: Command, reason?: string) {
  const before = await model();
  const p = b.apply(command);
  if (reason === undefined) await expect(p).rejects.toBeTruthy();
  else await expect(p).rejects.toBe(reason);
  expect(await model()).toEqual(before);
}

beforeEach(async () => {
  // Settings and saved files live in the stubbed storage; each test starts from the defaults.
  localStorage.clear();
  b = await freshBridge();
});

// ─── new project ───────────────────────────────────────────────────────────────────────────

describe("newProject", () => {
  it("matches ProjectModel::starter()", async () => {
    const s = await b.newProject(true);
    const m = s.model;
    expect(m.schemaVersion).toBe(10);
    expect(m.tempo).toBe(120);
    expect(m.key).toEqual({ rootPitchClass: 0, scale: "major", isLocked: true });
    expect(m.tracks.map((t) => [t.name, t.isDrum])).toEqual([
      ["melody", false],
      ["drums", true],
    ]);
    // One pattern per track.
    const [melody, drums] = m.tracks;
    expect(m.patterns.map((p) => [p.name, p.trackId, p.lengthTicks])).toEqual([
      ["melody 1", melody.id, 16 * TICKS_PER_BEAT],
      ["drums 1", drums.id, 16 * TICKS_PER_BEAT],
    ]);
    // The song is editing.defaultSongBars (32) of 4/4, and I–IV–V–vi is tiled across all of it.
    expect(m.songLengthTicks).toBe(128 * TICKS_PER_BEAT);
    expect(m.chords).toHaveLength(32);
    expect(m.chords.slice(0, 5).map((c) => [c.startTick, c.name])).toEqual([
      [0, "C"],
      [4 * TICKS_PER_BEAT, "F"],
      [8 * TICKS_PER_BEAT, "G"],
      [12 * TICKS_PER_BEAT, "Am"],
      [16 * TICKS_PER_BEAT, "C"],
    ]);
    // A melody (STARTER_MELODY) and a groove (starter_groove), so space plays music at once.
    expect(melody.voice).toBe("pluck");
    expect(m.patterns[0].notes.map((n) => n.pitch)).toEqual([76, 74, 72, 69, 72, 77, 74, 71, 67, 72, 71, 69]);
    const count = (pitch: number) => m.patterns[1].notes.filter((n) => n.pitch === pitch).length;
    expect([count(36), count(38), count(42)]).toEqual([8, 8, 32]);
    // One clip per track, loop-extended from 0 to the song end.
    expect(m.clips.map((c) => [c.patternId, c.startTick, c.lengthTicks, c.offsetTicks])).toEqual([
      [m.patterns[0].id, 0, 128 * TICKS_PER_BEAT, 0],
      [m.patterns[1].id, 0, 128 * TICKS_PER_BEAT, 0],
    ]);
    expect(m.master).toEqual({ gain: 1, reverbWet: 0, lowCutHz: 20, highCutHz: 18000, lowEq: 1, midEq: 1, highEq: 1, ceilingDb: -0.3, limiter: true });
    expect([s.canUndo, s.canRedo, s.dirty, s.path]).toEqual([false, false, false, null]);
  });

  it("matches ProjectModel::empty() when starter is false", async () => {
    const s = await b.newProject(false);
    expect(s.model.tracks.map((t) => t.name)).toEqual(["melody"]);
    expect(s.model.tracks[0].isDrum).toBe(false);
    expect(s.model.patterns.map((p) => [p.name, p.trackId])).toEqual([["melody 1", s.model.tracks[0].id]]);
    expect(s.model.chords).toEqual([]);
    expect(s.model.clips).toEqual([]);
    expect(s.model.songLengthTicks).toBe(128 * TICKS_PER_BEAT);
  });

  it("sizes the song from editing.defaultSongBars", async () => {
    const settings = await b.getSettings();
    expect(settings.editing.defaultSongBars).toBe(32);
    await b.setSettings({ ...settings, editing: { ...settings.editing, defaultSongBars: 8 } });
    const m = (await b.newProject(true)).model;
    expect(m.songLengthTicks).toBe(32 * TICKS_PER_BEAT);
    expect(m.chords.at(-1)!.startTick + m.chords.at(-1)!.lengthTicks).toBe(32 * TICKS_PER_BEAT);
    expect(m.clips.every((c) => c.lengthTicks === 32 * TICKS_PER_BEAT)).toBe(true);
  });

  it("follows the new-project setting when no choice is passed (new_project's fallback)", async () => {
    const settings = await b.getSettings();
    await b.setSettings({ ...settings, editing: { ...settings.editing, newProject: "empty" } });
    expect((await b.newProject()).model.tracks).toHaveLength(1);
    await b.setSettings({ ...settings, editing: { ...settings.editing, newProject: "starter" } });
    expect((await b.newProject()).model.tracks).toHaveLength(2);
  });

  it("drops a pending transient gesture (Document::replace clears `pending`)", async () => {
    const { melody } = await ids();
    await b.apply({ type: "setTrackParam", id: melody, param: "volume", value: 0.4 }, true);
    expect((await b.getState()).canUndo).toBe(true);
    const s = await b.newProject(true);
    expect(s.canUndo).toBe(false);
    expect(s.dirty).toBe(false);
  });
});

// ─── scalar edits and clamping ─────────────────────────────────────────────────────────────

describe("setTempo", () => {
  it("clamps to Tempo::MIN_BPM..MAX_BPM", async () => {
    await b.apply({ type: "setTempo", bpm: 1000 });
    expect((await model()).tempo).toBe(300);
    await b.apply({ type: "setTempo", bpm: 1 });
    expect((await model()).tempo).toBe(20);
  });

  it("rejects a non-finite bpm instead of storing NaN", async () => {
    await rejectsAndKeepsModel({ type: "setTempo", bpm: Number.NaN });
    await rejectsAndKeepsModel({ type: "setTempo", bpm: Number.POSITIVE_INFINITY });
  });
});

describe("setKey", () => {
  it("stores a copy, not a reference to the caller's command", async () => {
    const key = { rootPitchClass: 0, scale: "major" as const, isLocked: true };
    await b.apply({ type: "setKey", key });
    key.rootPitchClass = 7;
    expect((await model()).key.rootPitchClass).toBe(0);
  });
});

describe("setTrackParam", () => {
  it("clamps each parameter to the engine's range", async () => {
    const { melody } = await ids();
    const set = async (param: "volume" | "reverbSend" | "tone" | "pan", value: number) => {
      await b.apply({ type: "setTrackParam", id: melody, param, value });
      return (await model()).tracks[0][param];
    };
    expect(await set("volume", 9)).toBe(1.5);
    expect(await set("volume", -2)).toBe(0);
    expect(await set("reverbSend", 3)).toBe(1);
    expect(await set("reverbSend", -1)).toBe(0);
    expect(await set("tone", 1)).toBe(200);
    expect(await set("tone", 99_999)).toBe(20_000);
    expect(await set("pan", -5)).toBe(-1);
    expect(await set("pan", 5)).toBe(1);
  });

  it("rejects a non-finite value and an unknown track", async () => {
    const { melody } = await ids();
    await rejectsAndKeepsModel({ type: "setTrackParam", id: melody, param: "volume", value: Number.NaN });
    await rejectsAndKeepsModel({ type: "setTrackParam", id: FAKE_ID, param: "volume", value: 0.5 }, "no such track");
  });
});

describe("setTrackColor", () => {
  it("accepts #rrggbb, lowercasing it, and accepts null", async () => {
    const { melody } = await ids();
    await b.apply({ type: "setTrackColor", id: melody, color: "#ABCDEF" });
    expect((await model()).tracks[0].color).toBe("#abcdef");
    await b.apply({ type: "setTrackColor", id: melody, color: null });
    expect((await model()).tracks[0].color).toBeNull();
  });

  it("rejects anything that is not #rrggbb", async () => {
    const { melody } = await ids();
    for (const color of ["red", "#abc", "#abcdeg", "abcdef", "#abcdef0"]) {
      await rejectsAndKeepsModel({ type: "setTrackColor", id: melody, color }, "invalid value");
    }
  });

  it("validates the color before looking up the track", async () => {
    await rejectsAndKeepsModel({ type: "setTrackColor", id: FAKE_ID, color: "nope" }, "invalid value");
    await rejectsAndKeepsModel({ type: "setTrackColor", id: FAKE_ID, color: "#abcdef" }, "no such track");
  });
});

// ─── tracks ────────────────────────────────────────────────────────────────────────────────

describe("tracks", () => {
  it("names a new track after its position and gives it one empty pattern", async () => {
    await b.apply({ type: "addTrack", isDrum: false });
    const m = await model();
    expect(m.tracks[2]).toMatchObject({ name: "track 3", voice: "saw", isDrum: false, volume: 1, reverbSend: 0, tone: 18000, pan: 0, color: null });
    expect(m.patterns.filter((p) => p.trackId === m.tracks[2].id).map((p) => [p.name, p.notes])).toEqual([["track 3 1", []]]);
    await b.apply({ type: "addTrack", isDrum: true });
    const m2 = await model();
    expect(m2.tracks[3]).toMatchObject({ name: "drums", isDrum: true });
    expect(m2.patterns.filter((p) => p.trackId === m2.tracks[3].id).map((p) => p.name)).toEqual(["drums 1"]);
  });

  it("adds tracks and their pattern as one undo step", async () => {
    await b.apply({ type: "addTrack", isDrum: false });
    const back = (await b.undo()).model;
    expect(back.tracks).toHaveLength(2);
    expect(back.patterns).toHaveLength(2);
  });

  it("rejects edits to an unknown track", async () => {
    await rejectsAndKeepsModel({ type: "renameTrack", id: FAKE_ID, name: "x" }, "no such track");
    await rejectsAndKeepsModel({ type: "setTrackMuted", id: FAKE_ID, muted: true }, "no such track");
    await rejectsAndKeepsModel({ type: "setTrackSoloed", id: FAKE_ID, soloed: true }, "no such track");
    await rejectsAndKeepsModel({ type: "setTrackVoice", id: FAKE_ID, voice: "pad" }, "no such track");
    await rejectsAndKeepsModel({ type: "moveTrack", id: FAKE_ID, up: true }, "no such track");
    await rejectsAndKeepsModel({ type: "deleteTrack", id: FAKE_ID }, "no such track");
  });

  it("moves within bounds and no-ops (without erroring) at the edges", async () => {
    const { melody, drums } = await ids();
    await b.apply({ type: "moveTrack", id: melody, up: true });
    expect((await model()).tracks.map((t) => t.id)).toEqual([melody, drums]);
    await b.apply({ type: "moveTrack", id: drums, up: false });
    expect((await model()).tracks.map((t) => t.id)).toEqual([melody, drums]);
    await b.apply({ type: "moveTrack", id: drums, up: true });
    expect((await model()).tracks.map((t) => t.id)).toEqual([drums, melody]);
  });

  it("deletes the track's patterns and their clips with it", async () => {
    const { melody, drums, pattern, drumPattern } = await ids();
    await b.apply({ type: "addPattern", trackId: melody });
    const extra = (await model()).patterns.find((p) => p.name === "melody 2")!.id;
    await b.apply({ type: "addClip", patternId: extra, startTick: 64 * TICKS_PER_BEAT, lengthTicks: null });
    await b.apply({ type: "deleteTrack", id: melody });
    const m = await model();
    expect(m.tracks.map((t) => t.id)).toEqual([drums]);
    expect(m.patterns.map((p) => p.id)).toEqual([drumPattern]);
    expect(m.clips.map((c) => c.patternId)).toEqual([drumPattern]);
    expect(m.patterns.some((p) => p.id === pattern)).toBe(false);
    // One undo step brings all of it back.
    const back = (await b.undo()).model;
    expect(back.patterns).toHaveLength(3);
    expect(back.clips).toHaveLength(3);
  });

  it("refuses to delete the last track", async () => {
    const { melody, drums } = await ids();
    await b.apply({ type: "deleteTrack", id: melody });
    await rejectsAndKeepsModel({ type: "deleteTrack", id: drums }, "a project needs at least one track");
    expect((await model()).tracks).toHaveLength(1);
  });
});

// ─── patterns ──────────────────────────────────────────────────────────────────────────────

describe("patterns", () => {
  it("duplicates in place on the same track with fresh note ids", async () => {
    const { melody, pattern } = await ids();
    await b.apply({ type: "addPattern", trackId: melody });
    await b.apply({ type: "duplicatePattern", id: pattern });
    const m = await model();
    expect(m.patterns.map((p) => p.name)).toEqual(["melody 1", "melody 1 copy", "drums 1", "melody 2"]);
    const [orig, copy] = m.patterns;
    expect(copy.id).not.toBe(orig.id);
    expect(copy.trackId).toBe(melody);
    expect(copy.notes.map((n) => n.pitch)).toEqual(orig.notes.map((n) => n.pitch));
    expect(copy.notes.some((n, i) => n.id === orig.notes[i].id)).toBe(false);
  });

  it("adds an empty pattern to a track, named after it", async () => {
    const { drums } = await ids();
    await b.apply({ type: "addPattern", trackId: drums });
    await b.apply({ type: "addPattern", trackId: drums });
    const p = (await model()).patterns.filter((x) => x.trackId === drums);
    expect(p.map((x) => x.name)).toEqual(["drums 1", "drums 2", "drums 3"]);
    expect(p[1]).toMatchObject({ lengthTicks: 16 * TICKS_PER_BEAT, notes: [] });
    await rejectsAndKeepsModel({ type: "addPattern", trackId: FAKE_ID }, "no such track");
  });

  it("skips a pattern number already taken on that track", async () => {
    const { melody, pattern } = await ids();
    await b.apply({ type: "renamePattern", id: pattern, name: "melody 2" });
    await b.apply({ type: "addPattern", trackId: melody });
    expect((await model()).patterns.filter((p) => p.trackId === melody).map((p) => p.name)).toEqual(["melody 2", "melody 3"]);
  });

  it("refuses a blank rename and an unknown pattern", async () => {
    const { pattern } = await ids();
    await rejectsAndKeepsModel({ type: "renamePattern", id: pattern, name: "   " }, "invalid value");
    await rejectsAndKeepsModel({ type: "renamePattern", id: FAKE_ID, name: "verse" }, "no such pattern");
    await rejectsAndKeepsModel({ type: "duplicatePattern", id: FAKE_ID }, "no such pattern");
    await rejectsAndKeepsModel({ type: "deletePattern", id: FAKE_ID }, "no such pattern");
  });

  it("rejects a pattern shorter than one beat rather than clamping it", async () => {
    const { pattern } = await ids();
    await rejectsAndKeepsModel({ type: "setPatternLength", id: pattern, ticks: 480 }, "invalid value");
    await rejectsAndKeepsModel({ type: "setPatternLength", id: pattern, ticks: 0 }, "invalid value");
    await rejectsAndKeepsModel({ type: "setPatternLength", id: pattern, ticks: Number.NaN });
    await rejectsAndKeepsModel({ type: "setPatternLength", id: FAKE_ID, ticks: 8 * TICKS_PER_BEAT }, "no such pattern");
    await b.apply({ type: "setPatternLength", id: pattern, ticks: 8 * TICKS_PER_BEAT });
    expect((await model()).patterns[0].lengthTicks).toBe(7680);
  });

  it("removes a deleted pattern's clips, and lets a track end up with no patterns", async () => {
    const { melody, pattern, drumPattern } = await ids();
    await b.apply({ type: "deletePattern", id: pattern });
    const m = await model();
    expect(m.tracks.map((t) => t.id)).toContain(melody);
    expect(m.patterns.map((p) => p.id)).toEqual([drumPattern]);
    expect(m.clips.map((c) => c.patternId)).toEqual([drumPattern]);
    // Even the last pattern of the project can go.
    await b.apply({ type: "deletePattern", id: drumPattern });
    expect((await model()).patterns).toEqual([]);
    expect((await model()).clips).toEqual([]);
  });
  /// Mirrors how a clip longer than its pattern loops it: two turns of a 2-bar loop over 4 bars.
  it("lays a short loop out across the pattern, trimming each turn at its own end", async () => {
    const events = [
      { id: "a", startTick: 0, lengthTicks: TICKS_PER_BEAT },
      { id: "b", startTick: 6 * TICKS_PER_BEAT, lengthTicks: 4 * TICKS_PER_BEAT },
    ];
    const out = layOutLoop(events, 8 * TICKS_PER_BEAT, 16 * TICKS_PER_BEAT);
    expect(out.map((e) => [e.startTick, e.lengthTicks])).toEqual([
      [0, 960],
      [5760, 1920],
      [7680, 960],
      [13440, 1920],
    ]);
    expect(new Set(out.map((e) => e.id)).size).toBe(4);
    expect(out[0].id).toBe("a");
    expect(layOutLoop(events, 16 * TICKS_PER_BEAT, 16 * TICKS_PER_BEAT)).toBe(events);
  });
});

// ─── notes ─────────────────────────────────────────────────────────────────────────────────

describe("setNotes", () => {
  it("rejects an unknown pattern", async () => {
    await rejectsAndKeepsModel({ type: "setNotes", patternId: FAKE_ID, notes: [note(0, 60)] }, "no such pattern");
  });

  it("clamps every note into NoteEvent::sanitized's ranges", async () => {
    const { pattern } = await ids();
    await b.apply({
      type: "setNotes",
      patternId: pattern,
      notes: [
        note(0, 300, { velocity: 999, lengthTicks: 0 }),
        note(-4 * TICKS_PER_BEAT, -9, { velocity: 0, lengthTicks: -1 }),
      ],
    });
    const notes = (await model()).patterns[0].notes;
    expect(notes.map((n) => n.pitch)).toEqual([0, 127]);
    expect(notes.map((n) => n.velocity)).toEqual([1, 127]);
    expect(notes.map((n) => n.startTick)).toEqual([0, 0]);
    expect(notes.every((n) => n.lengthTicks === MIN_NOTE_TICKS)).toBe(true);
  });

  it("sorts by start tick then pitch", async () => {
    const { pattern } = await ids();
    await b.apply({
      type: "setNotes",
      patternId: pattern,
      notes: [note(4 * TICKS_PER_BEAT, 60), note(0, 72), note(0, 48), note(2 * TICKS_PER_BEAT, 55)],
    });
    const notes = (await model()).patterns[0].notes;
    expect(notes.map((n) => [n.startTick, n.pitch])).toEqual([
      [0, 48],
      [0, 72],
      [1920, 55],
      [3840, 60],
    ]);
  });

  it("touches only that pattern", async () => {
    const { pattern } = await ids();
    const drumsBefore = (await model()).patterns[1].notes;
    await b.apply({ type: "setNotes", patternId: pattern, notes: [] });
    expect((await model()).patterns[0].notes).toEqual([]);
    expect((await model()).patterns[1].notes).toEqual(drumsBefore);
  });

  it("rejects non-finite note fields instead of writing NaN into the model", async () => {
    const { pattern } = await ids();
    const bad = (n: NoteEvent) => ({ type: "setNotes" as const, patternId: pattern, notes: [n] });
    await rejectsAndKeepsModel(bad(note(0, Number.NaN)));
    await rejectsAndKeepsModel(bad(note(0, 60, { velocity: Number.NaN })));
    await rejectsAndKeepsModel(bad(note(Number.NaN, 60)));
    await rejectsAndKeepsModel(bad(note(0, 60, { lengthTicks: Number.NaN })));
    await rejectsAndKeepsModel(bad(note(0, Number.POSITIVE_INFINITY)));
  });
});

// ─── ropes ─────────────────────────────────────────────────────────────────────────────────

describe("ropes", () => {
  const A = "aaaaaaaa-0000-4000-8000-000000000000";
  const B = "bbbbbbbb-0000-4000-8000-000000000000";
  const C = "cccccccc-0000-4000-8000-000000000000";
  const roped = (id: string, startTick: number, pitch: number, extra: Partial<NoteEvent> = {}) => note(startTick, pitch, { id, ...extra });

  async function seed(notes: NoteEvent[]) {
    const { pattern } = await ids();
    await b.apply({ type: "setNotes", patternId: pattern, notes });
    return pattern;
  }
  const notes = async () => (await model()).patterns[0].notes;
  const find = async (id: string) => (await notes()).find((n) => n.id === id)!;

  it("setRope ropes a note, labelled for undo like document.rs", async () => {
    const pattern = await seed([roped(A, 0, 60), roped(B, TICKS_PER_BEAT, 67)]);
    const s = await b.apply({ type: "setRope", patternId: pattern, noteId: B, sourceId: A });
    expect(s.undoLabel).toBe("add rope");
    expect((await find(B)).slideFrom).toBe(A);
    expect((await b.undo()).model.patterns[0].notes.find((n) => n.id === B)).not.toHaveProperty("slideFrom");
  });

  it("setRope refuses missing notes, self ropes and loops", async () => {
    const pattern = await seed([roped(A, 0, 60), roped(B, TICKS_PER_BEAT, 62, { slideFrom: A }), roped(C, 2 * TICKS_PER_BEAT, 64, { slideFrom: B })]);
    const rope = (noteId: string, sourceId: string): Command => ({ type: "setRope", patternId: pattern, noteId, sourceId });
    await rejectsAndKeepsModel(rope(A, FAKE_ID), "no such note");
    await rejectsAndKeepsModel(rope(FAKE_ID, A), "no such note");
    const loop = "a note can't slide from itself, or from a note that slides from it";
    await rejectsAndKeepsModel(rope(A, A), loop);
    await rejectsAndKeepsModel(rope(A, B), loop);
    await rejectsAndKeepsModel(rope(A, C), loop);
    await rejectsAndKeepsModel({ type: "setRope", patternId: FAKE_ID, noteId: B, sourceId: A }, "no such pattern");
  });

  it("clearRope cuts the rope and its time, and leaves no keys behind", async () => {
    const pattern = await seed([roped(A, 0, 60), roped(B, TICKS_PER_BEAT, 62, { slideFrom: A, slideSecs: 0.4 })]);
    const s = await b.apply({ type: "clearRope", patternId: pattern, noteId: B });
    expect(s.undoLabel).toBe("remove rope");
    const got = await find(B);
    expect(got).not.toHaveProperty("slideFrom");
    expect(got).not.toHaveProperty("slideSecs");
    await rejectsAndKeepsModel({ type: "clearRope", patternId: pattern, noteId: FAKE_ID }, "no such note");
  });

  it("setSlideTime clamps, resets and needs a rope", async () => {
    const pattern = await seed([roped(A, 0, 60), roped(B, TICKS_PER_BEAT, 62, { slideFrom: A })]);
    const time = (secs: number | null): Command => ({ type: "setSlideTime", patternId: pattern, noteId: B, secs });
    expect((await b.apply(time(10))).undoLabel).toBe("change slide time");
    expect((await find(B)).slideSecs).toBe(2);
    await b.apply(time(0.5));
    expect((await find(B)).slideSecs).toBe(0.5);
    await b.apply(time(null));
    expect(await find(B)).not.toHaveProperty("slideSecs");
    await rejectsAndKeepsModel(time(Number.NaN), "invalid value");
    await rejectsAndKeepsModel({ type: "setSlideTime", patternId: pattern, noteId: A, secs: 0.5 }, "that note has no rope");
  });

  it("setNotes cuts ropes to deleted notes and clamps slide times", async () => {
    const pattern = await seed([roped(A, 0, 60), roped(B, TICKS_PER_BEAT, 62, { slideFrom: A, slideSecs: 9 })]);
    expect((await find(B)).slideSecs).toBe(2);
    await b.apply({ type: "setNotes", patternId: pattern, notes: (await notes()).filter((n) => n.id !== A) });
    const got = await find(B);
    expect(got).not.toHaveProperty("slideFrom");
    expect(got).not.toHaveProperty("slideSecs");
  });

  it("duplicatePattern points the copy's ropes inside the copy", async () => {
    const pattern = await seed([roped(A, 0, 60), roped(B, TICKS_PER_BEAT, 62, { slideFrom: A })]);
    await b.apply({ type: "duplicatePattern", id: pattern });
    const copy = (await model()).patterns[1].notes;
    expect(copy[0].id).not.toBe(A);
    expect(copy[1].slideFrom).toBe(copy[0].id);
  });
});

// ─── chords and arrangement ────────────────────────────────────────────────────────────────

describe("setChords", () => {
  const chord = (startTick: number, lengthTicks: number, pitchClasses: number[]): ChordEvent => ({
    id: FAKE_ID,
    startTick,
    lengthTicks,
    pitchClasses,
    name: null,
  });

  it("replaces the project progression (song ticks), dropping empty or negatively placed chords", async () => {
    await b.apply({
      type: "setChords",
      chords: [
        chord(40 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT, [7, 23, -1, 11, 7]),
        chord(2 * TICKS_PER_BEAT, 0, [0]),
        chord(-1, 4 * TICKS_PER_BEAT, [0]),
        chord(0, 4 * TICKS_PER_BEAT, [4, 0, 7]),
      ],
    });
    const chords = (await model()).chords;
    expect(chords.map((c) => c.startTick)).toEqual([0, 40 * TICKS_PER_BEAT]);
    // Order is kept so the root stays first, as `tidy_chords` does.
    expect(chords[0].pitchClasses).toEqual([4, 0, 7]);
    expect(chords[1].pitchClasses).toEqual([7, 11]);
  });

  it("cuts each chord at the next one's start", async () => {
    await b.apply({
      type: "setChords",
      chords: [chord(0, 8 * TICKS_PER_BEAT, [0, 4, 7]), chord(4 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT, [5, 9, 0])],
    });
    expect((await model()).chords.map((c) => [c.startTick, c.lengthTicks])).toEqual([
      [0, 4 * TICKS_PER_BEAT],
      [4 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT],
    ]);
  });
});

describe("clips", () => {
  it("places clips in start order and validates like AddClip", async () => {
    const { pattern } = await ids();
    await b.apply({ type: "addClip", patternId: pattern, startTick: 16 * TICKS_PER_BEAT, lengthTicks: null });
    expect((await model()).clips.map((c) => c.startTick)).toEqual([0, 0, 16 * TICKS_PER_BEAT]);
    // Clip length defaults to the pattern's own length.
    expect((await model()).clips[2].lengthTicks).toBe(16 * TICKS_PER_BEAT);
    await rejectsAndKeepsModel({ type: "addClip", patternId: FAKE_ID, startTick: 32 * TICKS_PER_BEAT, lengthTicks: null }, "no such pattern");
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startTick: -1, lengthTicks: null }, "invalid value");
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startTick: 32 * TICKS_PER_BEAT, lengthTicks: 0 }, "invalid value");
    await rejectsAndKeepsModel({ type: "removeClip", id: FAKE_ID }, "no such clip");
  });

  it("refuses a clip at or past the song end and clamps one running over it", async () => {
    const { pattern } = await ids();
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startTick: 128 * TICKS_PER_BEAT, lengthTicks: null }, "that is past the end of the song");
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startTick: 200 * TICKS_PER_BEAT, lengthTicks: 4 * TICKS_PER_BEAT }, "that is past the end of the song");
    await b.apply({ type: "addClip", patternId: pattern, startTick: 120 * TICKS_PER_BEAT, lengthTicks: null });
    const placed = (await model()).clips.find((c) => c.startTick === 120 * TICKS_PER_BEAT)!;
    expect(placed.lengthTicks).toBe(8 * TICKS_PER_BEAT);
  });

  it("clamps updateClip the same way and never moves a clip to another pattern", async () => {
    const { drumPattern } = await ids();
    const c = (await model()).clips[0];
    await b.apply({ type: "updateClip", clip: { ...c, startTick: 100 * TICKS_PER_BEAT, lengthTicks: 64 * TICKS_PER_BEAT, patternId: drumPattern } });
    const moved = await clip(c.id);
    expect([moved.startTick, moved.lengthTicks, moved.patternId]).toEqual([100 * TICKS_PER_BEAT, 28 * TICKS_PER_BEAT, c.patternId]);
    await rejectsAndKeepsModel({ type: "updateClip", clip: { ...moved, startTick: 128 * TICKS_PER_BEAT } }, "that is past the end of the song");
    await rejectsAndKeepsModel({ type: "updateClip", clip: { ...moved, id: FAKE_ID } }, "no such clip");
    await rejectsAndKeepsModel({ type: "updateClip", clip: { ...moved, lengthTicks: Number.NaN } }, "invalid value");
  });

  it("moves a group of clips with updateClips as one undo step", async () => {
    const [a, c] = (await model()).clips;
    await b.apply({ type: "setSongLength", ticks: 256 * TICKS_PER_BEAT });
    await b.apply({ type: "updateClips", clips: [{ ...a, startTick: 32 * TICKS_PER_BEAT }, { ...c, startTick: 32 * TICKS_PER_BEAT }] });
    expect((await model()).clips.map((x) => x.startTick)).toEqual([32 * TICKS_PER_BEAT, 32 * TICKS_PER_BEAT]);
    const back = (await b.undo()).model;
    expect(back.clips.map((x) => x.startTick)).toEqual([0, 0]);
    // One bad clip rejects the whole batch.
    await rejectsAndKeepsModel({ type: "updateClips", clips: [{ ...a, startTick: 8 * TICKS_PER_BEAT }, { ...c, id: FAKE_ID }] }, "no such clip");
  });

  it("removes a group of clips with removeClips as one undo step", async () => {
    const [a, c] = (await model()).clips;
    // Unknown ids are skipped, as in the Rust document.
    await b.apply({ type: "removeClips", ids: [a.id, c.id, FAKE_ID] });
    expect((await model()).clips).toEqual([]);
    expect((await b.undo()).model.clips).toHaveLength(2);
  });

  it("makeClipUnique forks the pattern on the same track and repoints just that clip", async () => {
    const { melody, pattern } = await ids();
    await b.apply({ type: "addClip", patternId: pattern, startTick: 64 * TICKS_PER_BEAT, lengthTicks: null });
    const target = (await model()).clips.find((c) => c.startTick === 64 * TICKS_PER_BEAT)!;
    await b.apply({ type: "makeClipUnique", id: target.id });
    const m = await model();
    const fork = m.patterns.find((p) => p.name === "melody 1 copy")!;
    expect(fork.trackId).toBe(melody);
    expect(fork.notes.map((n) => n.pitch)).toEqual(m.patterns[0].notes.map((n) => n.pitch));
    expect((await clip(target.id)).patternId).toBe(fork.id);
    // The other clip of the original still plays the original.
    expect(m.clips.filter((c) => c.patternId === pattern)).toHaveLength(1);
    // One undo step.
    const back = (await b.undo()).model;
    expect(back.patterns).toHaveLength(2);
    expect(back.clips.find((c) => c.id === target.id)!.patternId).toBe(pattern);
    await rejectsAndKeepsModel({ type: "makeClipUnique", id: FAKE_ID }, "no such clip");
  });
});

describe("setSongLength", () => {
  it("removes clips starting at or past the end and trims clips crossing it", async () => {
    const { pattern } = await ids();
    await b.apply({ type: "addClip", patternId: pattern, startTick: 64 * TICKS_PER_BEAT, lengthTicks: 16 * TICKS_PER_BEAT });
    await b.apply({ type: "addClip", patternId: pattern, startTick: 96 * TICKS_PER_BEAT, lengthTicks: 16 * TICKS_PER_BEAT });
    await b.apply({ type: "setSongLength", ticks: 72 * TICKS_PER_BEAT });
    const m = await model();
    expect(m.songLengthTicks).toBe(72 * TICKS_PER_BEAT);
    expect(m.clips.map((c) => [c.startTick, c.lengthTicks])).toEqual([
      [0, 72 * TICKS_PER_BEAT],
      [0, 72 * TICKS_PER_BEAT],
      [64 * TICKS_PER_BEAT, 8 * TICKS_PER_BEAT],
    ]);
  });

  it("keeps at least one bar and rejects a non-finite length", async () => {
    await b.apply({ type: "setSongLength", ticks: TICKS_PER_BEAT });
    expect((await model()).songLengthTicks).toBe(4 * TICKS_PER_BEAT);
    await b.apply({ type: "setTimeSignature", numerator: 3, denominator: 4 });
    await b.apply({ type: "setSongLength", ticks: 0 });
    expect((await model()).songLengthTicks).toBe(3 * TICKS_PER_BEAT);
    await rejectsAndKeepsModel({ type: "setSongLength", ticks: Number.NaN }, "invalid value");
  });

  it("grows without touching clips, and undoes in one step", async () => {
    const before = await model();
    await b.apply({ type: "setSongLength", ticks: 256 * TICKS_PER_BEAT });
    expect((await model()).clips).toEqual(before.clips);
    await b.apply({ type: "setSongLength", ticks: 8 * TICKS_PER_BEAT });
    expect((await b.undo()).model.clips).toEqual(before.clips);
  });
});

describe("setLoopRegion", () => {
  it("sets, clears and undoes the song's loop brace", async () => {
    await b.apply({ type: "setLoopRegion", region: [16 * TICKS_PER_BEAT, 32 * TICKS_PER_BEAT] });
    expect((await model()).loopRegion).toEqual([16 * TICKS_PER_BEAT, 32 * TICKS_PER_BEAT]);
    await b.apply({ type: "setLoopRegion", region: null });
    expect((await model()).loopRegion).toBeNull();
    expect((await b.undo()).model.loopRegion).toEqual([16 * TICKS_PER_BEAT, 32 * TICKS_PER_BEAT]);
  });

  it("holds the brace inside the song and refuses an empty or inverted one", async () => {
    const end = (await model()).songLengthTicks;
    await b.apply({ type: "setLoopRegion", region: [8 * TICKS_PER_BEAT, end + 400] });
    expect((await model()).loopRegion).toEqual([8 * TICKS_PER_BEAT, end]);
    await rejectsAndKeepsModel({ type: "setLoopRegion", region: [4 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT] }, "invalid value");
    await rejectsAndKeepsModel({ type: "setLoopRegion", region: [32 * TICKS_PER_BEAT, 16 * TICKS_PER_BEAT] }, "invalid value");
    await rejectsAndKeepsModel({ type: "setLoopRegion", region: [-TICKS_PER_BEAT, 4 * TICKS_PER_BEAT] }, "invalid value");
    await rejectsAndKeepsModel({ type: "setLoopRegion", region: [end, end + 960] }, "invalid value");
  });

  it("is pulled back inside a shortened song", async () => {
    await b.apply({ type: "setLoopRegion", region: [16 * TICKS_PER_BEAT, 48 * TICKS_PER_BEAT] });
    await b.apply({ type: "setSongLength", ticks: 32 * TICKS_PER_BEAT });
    expect((await model()).loopRegion).toEqual([16 * TICKS_PER_BEAT, 32 * TICKS_PER_BEAT]);
    await b.apply({ type: "setSongLength", ticks: 8 * TICKS_PER_BEAT });
    expect((await model()).loopRegion).toBeNull();
  });
});

describe("setMaster", () => {
  const master = (over: Partial<MasterSettings> = {}): MasterSettings => ({
    gain: 1,
    reverbWet: 0,
    lowCutHz: 20,
    highCutHz: 18000,
    lowEq: 1,
    midEq: 1,
    highEq: 1,
    ceilingDb: -0.3,
    limiter: true,
    ...over,
  });

  it("clamps every field", async () => {
    await b.apply({
      type: "setMaster",
      master: master({ gain: 9, reverbWet: 3, lowCutHz: 1, highCutHz: 99_999, lowEq: 5, midEq: -1, highEq: 5, ceilingDb: 6 }),
    });
    expect((await model()).master).toEqual({ gain: 1.5, reverbWet: 1, lowCutHz: 10, highCutHz: 20_000, lowEq: 2, midEq: 0, highEq: 2, ceilingDb: 0, limiter: true });
    await b.apply({ type: "setMaster", master: master({ lowCutHz: 9_999, highCutHz: 1, ceilingDb: -99 }) });
    expect((await model()).master).toMatchObject({ lowCutHz: 2_000, highCutHz: 500, ceilingDb: -12 });
  });

  it("turns the limiter off and leaves everything else alone", async () => {
    await b.apply({ type: "setMaster", master: master({ limiter: false }) });
    expect((await model()).master).toEqual(master({ limiter: false }));
  });

  it("rejects a non-finite field", async () => {
    await rejectsAndKeepsModel({ type: "setMaster", master: master({ gain: Number.NaN }) });
    await rejectsAndKeepsModel({ type: "setMaster", master: master({ ceilingDb: Number.NaN }) });
  });
});

// ─── undo / redo / gestures ────────────────────────────────────────────────────────────────

describe("undo, redo and transient gestures", () => {
  it("does not record an undo step for a command that changes nothing", async () => {
    const { melody } = await ids();
    const s = await b.apply({ type: "setTrackMuted", id: melody, muted: false });
    expect(s.canUndo).toBe(false);
    expect(s.dirty).toBe(false);
  });

  it("round-trips a single edit and clears redo on the next edit", async () => {
    const { melody } = await ids();
    await b.apply({ type: "renameTrack", id: melody, name: "lead" });
    expect((await b.getState()).canUndo).toBe(true);
    const undone = await b.undo();
    expect(undone.model.tracks[0].name).toBe("melody");
    expect(undone.canRedo).toBe(true);
    expect((await b.redo()).model.tracks[0].name).toBe("lead");
    await b.undo();
    await b.apply({ type: "renameTrack", id: melody, name: "bass" });
    expect((await b.getState()).canRedo).toBe(false);
  });

  it("coalesces a run of transient edits into one undo step", async () => {
    const { melody } = await ids();
    for (const value of [0.9, 0.8, 0.7]) {
      await b.apply({ type: "setTrackParam", id: melody, param: "volume", value }, true);
    }
    expect((await b.getState()).canUndo).toBe(true);
    await b.commitGesture();
    expect((await b.undo()).model.tracks[0].volume).toBe(1);
    expect((await b.getState()).canUndo).toBe(false);
  });

  it("closes the open gesture when a non-transient command arrives", async () => {
    const { melody } = await ids();
    await b.apply({ type: "setTrackParam", id: melody, param: "volume", value: 0.5 }, true);
    await b.apply({ type: "renameTrack", id: melody, name: "lead" });
    expect((await b.undo()).model.tracks[0]).toMatchObject({ name: "melody", volume: 0.5 });
    expect((await b.undo()).model.tracks[0]).toMatchObject({ name: "melody", volume: 1 });
  });

  it("closes the open gesture on redo, like Document::redo", async () => {
    const { melody } = await ids();
    await b.apply({ type: "setTrackParam", id: melody, param: "volume", value: 0.5 }, true);
    await b.redo(); // nothing to redo, but it still ends the gesture
    await b.apply({ type: "setTrackParam", id: melody, param: "volume", value: 0.7 }, true);
    await b.commitGesture();
    expect((await b.undo()).model.tracks[0].volume).toBe(0.5);
  });

  it("keeps at most MAX_UNDO (200) steps", async () => {
    const { pattern } = await ids();
    for (let i = 0; i < 201; i++) {
      await b.apply({ type: "setPatternLength", id: pattern, ticks: (i + 2) * TICKS_PER_BEAT });
    }
    for (let i = 0; i < 200; i++) await b.undo();
    const s = await b.getState();
    expect(s.model.patterns[0].lengthTicks).toBe(2 * TICKS_PER_BEAT);
    expect(s.canUndo).toBe(false);
  });
});

// ─── files ─────────────────────────────────────────────────────────────────────────────────

describe("save and open", () => {
  it("refuses to save with no path, like save_project", async () => {
    const { melody } = await ids();
    await b.apply({ type: "renameTrack", id: melody, name: "lead" });
    await expect(b.saveProject()).rejects.toBe("no file path yet — use Save As");
    expect((await b.getState()).dirty).toBe(true);
  });

  it("stores the path and clears the dirty flag", async () => {
    const { melody } = await ids();
    await b.apply({ type: "renameTrack", id: melody, name: "lead" });
    const s = await b.saveProject("/tmp/song.dissonant");
    expect(s.path).toBe("/tmp/song.dissonant");
    expect(s.dirty).toBe(false);
    expect((await b.saveProject()).path).toBe("/tmp/song.dissonant");
  });

  it("replaces the document on open: history cleared, not dirty", async () => {
    const { melody } = await ids();
    await b.apply({ type: "renameTrack", id: melody, name: "lead" });
    await b.saveProject("/tmp/song.dissonant");
    await b.apply({ type: "renameTrack", id: melody, name: "scratch" });
    const s = await b.openProject("/tmp/song.dissonant");
    expect(s.path).toBe("/tmp/song.dissonant");
    expect(s.dirty).toBe(false);
    expect(s.canUndo).toBe(false);
    expect(s.canRedo).toBe(false);
    expect(s.model.tracks[0].name).toBe("lead");
  });

  it("migrates a schema 9 project: one pattern per track, its chords lifted onto the song", async () => {
    const current = await model();
    const melody = current.tracks[0].id;
    // A pre-v10 pattern: every track's notes in one record, with its own looping chord track.
    const old = {
      ...current,
      schemaVersion: 9,
      // A schema 9 file has no song-level progression: the patterns carried the chords.
      chords: [],
      patterns: [
        {
          id: "11111111-1111-4111-8111-111111111111",
          name: "verse",
          lengthTicks: 4 * TICKS_PER_BEAT,
          notesByTrack: { [melody]: [note(0, 60)] },
          chords: { chords: [{ id: FAKE_ID, startTick: 0, lengthTicks: 4 * TICKS_PER_BEAT, pitchClasses: [0, 4, 7], name: "C" }] },
        },
      ],
      clips: [
        {
          id: "22222222-2222-4222-8222-222222222222",
          patternId: "11111111-1111-4111-8111-111111111111",
          startTick: 0,
          lengthTicks: 8 * TICKS_PER_BEAT,
          offsetTicks: 0,
          muted: false,
        },
      ],
      songLengthTicks: undefined,
    };
    localStorage.setItem("dissonant.mock.project", JSON.stringify(old));
    const m = (await b.openProject("/tmp/old.dissonant")).model;
    expect(m.schemaVersion).toBe(10);
    // The melody's notes became its own pattern; the clip follows it.
    expect(m.patterns.map((p) => [p.name, p.trackId])).toEqual([["verse melody", melody]]);
    expect(m.patterns[0].notes.map((n) => n.pitch)).toEqual([60]);
    expect(m.clips.map((c) => [c.patternId, c.startTick, c.lengthTicks])).toEqual([[m.patterns[0].id, 0, 8 * TICKS_PER_BEAT]]);
    // The pattern's progression looped under the clip, so it is two chords on the song now.
    expect(m.chords.map((c) => [c.startTick, c.lengthTicks])).toEqual([
      [0, 4 * TICKS_PER_BEAT],
      [4 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT],
    ]);
    // A pre-v10 file never said how long the song was: it ends with its last clip.
    expect(m.songLengthTicks).toBe(8 * TICKS_PER_BEAT);
  });

  it("migrates an old template the same way", async () => {
    const current = await model();
    localStorage.setItem("dissonant.mock.templates", JSON.stringify({ old: { ...current, schemaVersion: 9 } }));
    const m = (await b.newFromTemplate("old")).model;
    expect(m.schemaVersion).toBe(10);
    expect(m.patterns.map((p) => p.trackId)).toEqual(current.patterns.map((p) => p.trackId));
  });
});

// ─── sound presets ─────────────────────────────────────────────────────────────────────────

describe("sound presets (store.applySoundPreset)", () => {
  it("voice, tone and reverb as transient edits undo as one labelled step", async () => {
    const { melody } = await ids();
    const before = (await model()).tracks[0];
    const preset = SOUND_PRESETS.find((p) => p.name === "warm pad")!;
    const label = `sound: ${preset.name}`;
    await b.apply({ type: "setTrackVoice", id: melody, voice: preset.voice }, true, label);
    await b.apply({ type: "setTrackParam", id: melody, param: "tone", value: preset.tone }, true, label);
    await b.apply({ type: "setTrackParam", id: melody, param: "reverbSend", value: preset.reverbSend }, true, label);
    const s = await b.commitGesture();
    expect(soundPresetOf(s.model.tracks[0])?.name).toBe("warm pad");
    expect(s.undoLabel).toBe(label);
    const back = await b.undo();
    expect(back.model.tracks[0]).toEqual(before);
    expect(back.canUndo).toBe(false);
  });
});

// ─── sounds and drum kits ──────────────────────────────────────────────────────────────────

const synthSound = (id: string, level = 0.5): Sound => ({
  id,
  name: id,
  category: "lead",
  patch: { kind: "synth", ...structuredClone(DEFAULT_SYNTH_PATCH), level },
});
const drumSound = (id: string, tune = 0): Sound => ({
  id,
  name: id,
  category: "kick",
  patch: { kind: "drum", ...structuredClone(DEFAULT_DRUM_PATCH), tune },
});
const pad = (pitch: number, sound: string, choke = 0): DrumPad => ({ pitch, sound, choke });
const embedded = async () => ((await model()).sounds ?? []).map((s) => s.id);

/** Mirrors the "sounds and drum kits" section of crates/dissonant-core/tests/document_behavior.rs. */
describe("sounds (setTrackSound / setDrumPads / upsertSound)", () => {
  it("setTrackSound embeds a user sound, replaces it by id, and a factory sound prunes it", async () => {
    const { melody } = await ids();
    await b.apply({ type: "setTrackSound", id: melody, sound: synthSound("u-a") });
    expect((await model()).tracks[0].voice).toBe("u-a");
    expect(await embedded()).toEqual(["u-a"]);
    // Another user sound: the first, now unused, is pruned.
    await b.apply({ type: "setTrackSound", id: melody, sound: synthSound("u-b") });
    expect(await embedded()).toEqual(["u-b"]);
    await b.apply({ type: "setTrackSound", id: melody, sound: synthSound("u-b", 0.25) });
    expect((await model()).sounds![0].patch.level).toBe(0.25);
    // Back to a factory sound: nothing is embedded and, like Rust's skip_serializing_if, the
    // key is gone rather than an empty array.
    await b.apply({ type: "setTrackSound", id: melody, sound: FACTORY_SOUNDS.find((s) => s.id === "saw")! });
    const m = await model();
    expect(m.tracks[0].voice).toBe("saw");
    expect("sounds" in m).toBe(false);
  });

  it("refuses the wrong kind of sound and an unknown track", async () => {
    const { melody, drums } = await ids();
    await rejectsAndKeepsModel({ type: "setTrackSound", id: drums, sound: synthSound("u-a") }, "invalid value");
    await rejectsAndKeepsModel({ type: "setTrackSound", id: melody, sound: drumSound("u-boom") }, "invalid value");
    await rejectsAndKeepsModel({ type: "setTrackSound", id: FAKE_ID, sound: synthSound("u-a") }, "no such track");
  });

  it("sanitizes what it embeds like Sound::sanitized, filling missing fields like serde", async () => {
    const { melody, drums } = await ids();
    const wild = synthSound("u-wild", 9);
    wild.name = "  ";
    if (wild.patch.kind === "synth") {
      wild.patch.osc[1].unison = 99;
      wild.patch.osc[0].octave = -7;
      wild.patch.filter.cutoff = 5;
      delete (wild.patch as Partial<typeof wild.patch>).delay;
    }
    await b.apply({ type: "setTrackSound", id: melody, sound: wild });
    const s = (await model()).sounds![0];
    expect(s.name).toBe("untitled");
    if (s.patch.kind !== "synth") throw new Error("expected a synth");
    expect([s.patch.level, s.patch.osc[1].unison, s.patch.osc[0].octave, s.patch.filter.cutoff]).toEqual([1, 8, -3, 20]);
    expect(s.patch.delay).toEqual(DEFAULT_SYNTH_PATCH.delay);

    const boom = drumSound("u-boom", 99);
    if (boom.patch.kind === "drum") boom.patch.noise.bursts = 0;
    await b.apply({ type: "setDrumPads", id: drums, pads: [pad(36, "u-boom")], sounds: [boom] });
    const d = (await model()).sounds!.find((x) => x.id === "u-boom")!;
    if (d.patch.kind !== "drum") throw new Error("expected a drum");
    expect([d.patch.tune, d.patch.noise.bursts]).toEqual([24, 1]);

    await b.apply({ type: "upsertSound", sound: synthSound("u-wild", -1) });
    expect((await model()).sounds![0].patch.level).toBe(0);

    // A fraction in an integer field would not even decode in Rust.
    const half = synthSound("u-half");
    if (half.patch.kind === "synth") half.patch.osc[0].unison = 1.5;
    await rejectsAndKeepsModel({ type: "setTrackSound", id: melody, sound: half }, "invalid value");
  });

  it("setDrumPads refuses a bad kit and changes nothing", async () => {
    const { melody, drums } = await ids();
    await b.apply({ type: "setTrackSound", id: melody, sound: synthSound("u-test") });
    const seventeen = Array.from({ length: 17 }, (_, i) => pad(30 + i, "kick"));
    const kit = (pads: DrumPad[], sounds: Sound[] = []): Command => ({ type: "setDrumPads", id: drums, pads, sounds });
    await rejectsAndKeepsModel(kit([]), "invalid value");
    await rejectsAndKeepsModel(kit(seventeen), "invalid value");
    await rejectsAndKeepsModel(kit([pad(36, "kick"), pad(36, "snare")]), "invalid value");
    await rejectsAndKeepsModel(kit([pad(128, "kick")]), "invalid value");
    await rejectsAndKeepsModel(kit([pad(36, "u-test")], [synthSound("u-test")]), "invalid value");
    await rejectsAndKeepsModel(kit([pad(36, "saw")]), "no such sound");
    await rejectsAndKeepsModel(kit([pad(36, "u-test")]), "no such sound");
    // The carried sound is embedded on the way, but the refusal throws the whole edit away.
    await rejectsAndKeepsModel(kit([pad(36, "u-boom"), pad(38, "u-missing")], [drumSound("u-boom")]), "no such sound");
    await rejectsAndKeepsModel({ type: "setDrumPads", id: melody, pads: defaultKit(), sounds: [] }, "invalid value");
    await rejectsAndKeepsModel({ type: "setDrumPads", id: FAKE_ID, pads: defaultKit(), sounds: [] }, "no such track");
    expect((await model()).tracks[1].drumPads).toEqual(defaultKit());
    expect(await embedded()).toEqual(["u-test"]);
    // Sixteen is the limit, not one past it.
    await b.apply(kit(seventeen.slice(0, 16)));
    expect((await model()).tracks[1].drumPads).toHaveLength(16);
  });

  it("setDrumPads embeds only the user sounds its pads use; deleting a track prunes its sounds", async () => {
    const { melody, drums } = await ids();
    await b.apply({ type: "setTrackSound", id: melody, sound: synthSound("u-test") });
    const pads = [pad(36, "u-boom"), pad(42, "hat", 1), pad(46, "open-hat", 1)];
    const hat = FACTORY_SOUNDS.find((s) => s.id === "hat")!;
    await b.apply({ type: "setDrumPads", id: drums, pads, sounds: [drumSound("u-boom"), drumSound("u-unused"), hat] });
    expect((await model()).tracks[1].drumPads).toEqual(pads);
    expect(await embedded()).toEqual(["u-test", "u-boom"]);

    await b.apply({ type: "deleteTrack", id: drums });
    expect(await embedded()).toEqual(["u-test"]);
    await b.undo();
    await b.apply({ type: "deleteTrack", id: melody });
    expect(await embedded()).toEqual(["u-boom"]);
    await b.undo();
    // A sound another track still plays survives.
    const added = (await b.apply({ type: "addTrack", isDrum: false })).model.tracks[2].id;
    await b.apply({ type: "setTrackVoice", id: added, voice: "u-test" });
    await b.apply({ type: "deleteTrack", id: melody });
    expect(await embedded()).toEqual(["u-test", "u-boom"]);
  });

  it("upsertSound only replaces a copy the project embeds", async () => {
    const { melody } = await ids();
    await b.apply({ type: "setTrackSound", id: melody, sound: synthSound("u-test") });
    await rejectsAndKeepsModel({ type: "upsertSound", sound: synthSound("u-nope") }, "no such sound");
    await rejectsAndKeepsModel({ type: "upsertSound", sound: FACTORY_SOUNDS.find((s) => s.id === "saw")! }, "no such sound");
    await rejectsAndKeepsModel({ type: "upsertSound", sound: drumSound("u-test") }, "invalid value");
    await b.apply({ type: "upsertSound", sound: synthSound("u-test", 0.75) });
    expect((await model()).sounds!.map((s) => [s.id, s.patch.level])).toEqual([["u-test", 0.75]]);
  });

  it("deleteTrack checks the id before the last-track rule, like Rust", async () => {
    const { melody } = await ids();
    await b.apply({ type: "deleteTrack", id: melody });
    await rejectsAndKeepsModel({ type: "deleteTrack", id: FAKE_ID }, "no such track");
  });
});

/** Mirrors `library::tests` in src-tauri/src/library.rs. */
describe("sound library (sound-library.json)", () => {
  const LIBRARY_KEY = "dissonant.mock.soundLibrary";
  const libIds = (lib: Sound[]) => lib.map((s) => s.id);

  it("starts empty, inserts, replaces in place, deletes, and persists", async () => {
    expect(await b.librarySounds()).toEqual([]);
    await b.saveLibrarySound(synthSound("u-a", 0.1));
    await b.saveLibrarySound(drumSound("u-b"));
    const lib = await b.saveLibrarySound(synthSound("u-a", 0.9));
    expect(libIds(lib)).toEqual(["u-a", "u-b"]);
    expect(lib[0].patch.level).toBe(0.9);
    // What came back is a copy, and it is what a fresh session reads.
    lib.pop();
    const next = await freshBridge();
    expect(libIds(await next.librarySounds())).toEqual(["u-a", "u-b"]);
    expect(libIds(await next.deleteLibrarySound("u-a"))).toEqual(["u-b"]);
    expect(libIds(await next.deleteLibrarySound("u-nope"))).toEqual(["u-b"]);
    expect(await next.deleteLibrarySound("u-b")).toEqual([]);
    expect(await b.librarySounds()).toEqual([]);
  });

  it("refuses factory and unprefixed ids with library.rs's messages", async () => {
    await expect(b.saveLibrarySound(synthSound("saw"))).rejects.toBe('"saw" is a factory sound; save a copy under a new id');
    await expect(b.saveLibrarySound(drumSound("kick"))).rejects.toBe('"kick" is a factory sound; save a copy under a new id');
    await expect(b.saveLibrarySound(synthSound("my-sound"))).rejects.toBe('a library sound id must start with "u-" (got "my-sound")');
    expect(await b.librarySounds()).toEqual([]);
  });

  it("sanitizes what it saves", async () => {
    const loud = synthSound("u-loud", 7);
    loud.name = "";
    const [s] = await b.saveLibrarySound(loud);
    expect([s.name, s.patch.level]).toEqual(["untitled", 1]);
  });

  it("reads unreadable storage as an empty library and drops bad entries", async () => {
    localStorage.setItem(LIBRARY_KEY, "{ not json");
    expect(await b.librarySounds()).toEqual([]);
    localStorage.setItem(LIBRARY_KEY, JSON.stringify({ not: "an array" }));
    expect(await b.librarySounds()).toEqual([]);
    localStorage.setItem(LIBRARY_KEY, JSON.stringify([synthSound("u-good"), { id: "u-broken" }, synthSound("saw"), synthSound("u-good", 0.9)]));
    const lib = await b.librarySounds();
    expect(libIds(lib)).toEqual(["u-good"]);
    expect(lib[0].patch.level).toBe(0.5);
    // The next save starts from what could be read.
    expect(libIds(await b.saveLibrarySound(synthSound("u-new")))).toEqual(["u-good", "u-new"]);
  });
});

// ─── vibes ─────────────────────────────────────────────────────────────────────────────────

describe("vibes (ProjectModel::from_vibe)", () => {
  it("match vibe_values_shared_with_the_ui in crates/dissonant-core/tests/persistence.rs", async () => {
    const infos = await b.listVibes();
    const described = [];
    for (const info of infos) {
      const s = await b.newFromVibe(info.id);
      const m = s.model;
      expect(m.key).toEqual(info.key);
      expect([s.dirty, s.canUndo, s.path]).toEqual([false, false, null]);
      // One pattern per track, each placed once from 0 to the song end.
      expect(m.patterns.map((p) => p.trackId)).toEqual(m.tracks.map((t) => t.id));
      expect(m.patterns.map((p) => p.name)).toEqual(m.tracks.map((t) => `${t.name} 1`));
      expect(m.clips.map((c) => [c.patternId, c.startTick, c.lengthTicks])).toEqual(m.patterns.map((p) => [p.id, 0, m.songLengthTicks]));
      // The progression is tiled across the whole song; the first 16 beats name one pass.
      expect(m.chords.at(-1)!.startTick + m.chords.at(-1)!.lengthTicks).toBe(m.songLengthTicks);
      const pitchSum = m.patterns.flatMap((p) => p.notes).reduce((sum, n) => sum + n.pitch, 0);
      described.push(
        `${info.id}|${info.tempo}|${m.swing}|${m.tracks.map((t) => t.name).join(",")}|${m.chords.filter((c) => c.startTick < 16 * TICKS_PER_BEAT).map((c) => c.name).join(" ")}|${m.tracks[0].voice}|${m.patterns.map((p) => p.notes.length).join(",")}|${pitchSum}`,
      );
    }
    expect(described).toEqual([
      "jungle|172|50|bells,reese,pad,amen|Fm C# A#m Cm|fm-bell|0,12,12,67|4194",
      "dubstep|140|50|lead,wobble,pad,drums|Dm A# C Am|screamer-lead|0,24,12,62|4195",
      "blownout|138|54|voice,bass,stabs,machine|Em F#dim Em D|robot|0,24,36,90|7149",
      "rage|160|50|synth,808,chords,drums|C#m A B A|rage-lead|0,16,12,72|4273",
      "jersey|140|50|melody,sub,keys,drums|Am F C G|bright-pluck|0,20,36,81|6657",
      "hyperpop|160|50|lead,808,synths,drums|A B C#m E|chip-lead|0,16,60,108|9080",
      "pluggnb|150|50|flute,808,keys,drums|F# Fm A#m G#|whistle-lead|0,8,24,59|4319",
    ]);
  });

  it("rejects an unknown vibe and keeps the project", async () => {
    const before = await model();
    await expect(b.newFromVibe("polka")).rejects.toBeTruthy();
    expect(await model()).toEqual(before);
  });
});

// ─── plugins ───────────────────────────────────────────────────────────────────────────────

describe("track plugins", () => {
  const anyPlugin = async () => (await b.scanPlugins())[0];

  it("refuses a plugin on a track that does not exist", async () => {
    await rejectsAndKeepsModel({ type: "setTrackPlugin", id: FAKE_ID, plugin: { uid: "clap:/x.clap:x", name: "x", state: null } }, "no such track");
  });

  it("refuses a blank uid", async () => {
    const { melody } = await ids();
    await rejectsAndKeepsModel({ type: "setTrackPlugin", id: melody, plugin: { uid: "   ", name: "x", state: null } }, "invalid value");
  });

  it("keeps the saved state while the plugin stays the same and drops it when it changes", async () => {
    const { melody } = await ids();
    const [a, b2] = await b.scanPlugins();
    await b.apply({ type: "setTrackPlugin", id: melody, plugin: { uid: a.uid, name: a.name, state: null } });
    await b.apply({ type: "setPluginState", id: melody, state: "c2F2ZWQ=" });

    await b.apply({ type: "setTrackPlugin", id: melody, plugin: { uid: a.uid, name: a.name, state: null } });
    expect((await model()).tracks[0].plugin?.state).toBe("c2F2ZWQ=");

    await b.apply({ type: "setTrackPlugin", id: melody, plugin: { uid: b2.uid, name: b2.name, state: null } });
    expect((await model()).tracks[0].plugin).toEqual({ uid: b2.uid, name: b2.name, state: null });
  });

  it("refuses plugin state on a track that has no plugin", async () => {
    const { melody } = await ids();
    await rejectsAndKeepsModel({ type: "setPluginState", id: melody, state: "eA==" }, "track has no plugin");
  });

  it("goes back to the built-in voice when the plugin is cleared", async () => {
    const { melody } = await ids();
    const voice = (await model()).tracks[0].voice;
    const p = await anyPlugin();
    await b.setTrackPlugin(melody, p.uid);
    expect((await model()).tracks[0].plugin?.name).toBe(p.name);
    await b.setTrackPlugin(melody, null);
    expect((await model()).tracks[0].plugin).toBeNull();
    // The track's own voice was never touched, so it sounds again.
    expect((await model()).tracks[0].voice).toBe(voice);
  });

  it("reports no parameters for a track with no plugin", async () => {
    const { melody } = await ids();
    expect(await b.pluginParams(melody)).toEqual([]);
  });

  it("clamps a parameter to the range the plugin published", async () => {
    const { melody } = await ids();
    await b.setTrackPlugin(melody, (await anyPlugin()).uid);
    const cutoff = (await b.pluginParams(melody))[0];
    await b.setPluginParam(melody, cutoff.id, 1e9);
    expect((await b.pluginParams(melody))[0].value).toBe(cutoff.max);
  });

  it("rounds a stepped parameter", async () => {
    const { melody } = await ids();
    await b.setTrackPlugin(melody, (await anyPlugin()).uid);
    const stepped = (await b.pluginParams(melody)).find((p) => p.isStepped)!;
    await b.setPluginParam(melody, stepped.id, 7.4);
    expect((await b.pluginParams(melody)).find((p) => p.id === stepped.id)!.value).toBe(7);
  });

  it("drags without touching the document and commits one undo step", async () => {
    const { melody } = await ids();
    await b.setTrackPlugin(melody, (await anyPlugin()).uid);
    const res = (await b.pluginParams(melody)).find((p) => p.name === "resonance")!;
    const before = await b.getState();

    await b.setPluginParam(melody, res.id, 0.9, false);
    const dragging = await b.getState();
    // The plugin heard it; the document did not, so there is nothing to undo yet.
    expect((await b.pluginParams(melody)).find((p) => p.id === res.id)!.value).toBe(0.9);
    expect(dragging.model).toEqual(before.model);
    expect(dragging.undoLabel).toBe("change plugin");

    await b.setPluginParam(melody, res.id, 0.9, true);
    const committed = await b.getState();
    expect(committed.undoLabel).toBe("edit plugin");
    expect(committed.model.tracks[0].plugin?.state).toEqual(expect.any(String));

    const undone = await b.undo();
    expect(undone.model).toEqual(before.model);
    // Undo has to reach the plugin too, not just the document.
    expect((await b.pluginParams(melody)).find((p) => p.id === res.id)!.value).toBe(res.default);
  });

  it("restores the saved values when a project is reopened", async () => {
    const { melody } = await ids();
    await b.setTrackPlugin(melody, (await anyPlugin()).uid);
    const res = (await b.pluginParams(melody)).find((p) => p.name === "resonance")!;
    await b.setPluginParam(melody, res.id, 0.77, true);
    await b.saveProject("/tmp/plugged.dissonant");

    await b.newProject(true);
    await b.openProject("/tmp/plugged.dissonant");
    const reopened = (await model()).tracks[0].id;
    expect((await b.pluginParams(reopened)).find((p) => p.id === res.id)!.value).toBe(0.77);
  });
});

// ─── insert chains ─────────────────────────────────────────────────────────────────────────

describe("insert chains", () => {
  /** The two effects the mock's plugin folder holds, in scan order. */
  const effects = async () => (await b.scanPlugins()).filter((p) => p.kind === "effect");
  const chain = async (target: InsertTarget) => insertChain(await model(), target);
  const names = async (target: InsertTarget) => (await chain(target)).map((i) => i.name);

  /** A chain of `n` effects, alternating the two so order is readable. */
  async function fill(target: InsertTarget, n: number) {
    const es = await effects();
    for (let i = 0; i < n; i++) await b.addInsert(target, es[i % es.length].uid);
    return chain(target);
  }

  it("appends by default and inserts at an index when given one", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const [tape, fuzz] = [...(await effects())].reverse();
    await b.addInsert(target, tape.uid);
    await b.addInsert(target, fuzz.uid);
    await b.addInsert(target, fuzz.uid, 0);
    expect(await names(target)).toEqual([fuzz.name, tape.name, fuzz.name]);
    // Past the end is the end, not a hole.
    await b.addInsert(target, tape.uid, 99);
    expect((await names(target)).at(-1)).toBe(tape.name);
  });

  it("starts every effect enabled with no state of its own", async () => {
    const { melody } = await ids();
    const [e] = await effects();
    await b.addInsert({ track: melody }, e.uid);
    const [ins] = await chain({ track: melody });
    expect([ins.uid, ins.name, ins.enabled]).toEqual([e.uid, e.name, true]);
    expect(ins.state ?? null).toBeNull();
  });

  it("refuses a ninth effect and leaves the chain as it was", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const full = await fill(target, MAX_INSERTS);
    expect(full).toHaveLength(MAX_INSERTS);
    const [e] = await effects();
    await expect(b.addInsert(target, e.uid)).rejects.toBe(
      `an insert chain holds at most ${MAX_INSERTS} effects`,
    );
    expect(await chain(target)).toEqual(full);
  });

  it("refuses a blank uid and an unknown track", async () => {
    const { melody } = await ids();
    await rejectsAndKeepsModel(
      { type: "addInsert", target: { track: melody }, uid: "  ", name: "x", index: null },
      "invalid value",
    );
    await rejectsAndKeepsModel(
      { type: "addInsert", target: { track: FAKE_ID }, uid: "clap:/x.clap:x", name: "x", index: null },
      "no such track",
    );
  });

  it("refuses an effect this computer does not have", async () => {
    const { melody } = await ids();
    await expect(b.addInsert({ track: melody }, "clap:/nowhere.clap:nope")).rejects.toBe("no such plugin");
  });

  it("moves one effect and shuffles the rest up or down around it", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const built = await fill(target, 3);
    await b.moveInsert(target, built[0].id, 2);
    expect((await chain(target)).map((i) => i.id)).toEqual([built[1].id, built[2].id, built[0].id]);
    await b.moveInsert(target, built[0].id, 1);
    expect((await chain(target)).map((i) => i.id)).toEqual([built[1].id, built[0].id, built[2].id]);
  });

  it("clamps a move past either end instead of failing", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const built = await fill(target, 3);
    const order = built.map((i) => i.id);

    // The arrows on the first and last rows are pressed anyway; they must do nothing rather
    // than throw, because the UI's drop targets and the chain's bounds need not agree.
    await b.moveInsert(target, built[0].id, -4);
    expect((await chain(target)).map((i) => i.id)).toEqual(order);
    await b.moveInsert(target, built[2].id, 99);
    expect((await chain(target)).map((i) => i.id)).toEqual(order);
  });

  it("bypasses in place and switches back on", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const built = await fill(target, 3);

    await b.setInsertEnabled(target, built[1].id, false);
    expect((await chain(target)).map((i) => i.enabled)).toEqual([true, false, true]);
    expect((await chain(target)).map((i) => i.id)).toEqual(built.map((i) => i.id));

    await b.setInsertEnabled(target, built[1].id, true);
    expect((await chain(target)).every((i) => i.enabled)).toBe(true);
  });

  it("removes the slot it names and keeps the order of the rest", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const built = await fill(target, 3);
    await b.removeInsert(target, built[1].id);
    expect((await chain(target)).map((i) => i.id)).toEqual([built[0].id, built[2].id]);
  });

  it("refuses to remove, move or bypass an insert that is not there", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    await fill(target, 1);
    await expect(b.removeInsert(target, FAKE_ID)).rejects.toBe("no such insert");
    await expect(b.moveInsert(target, FAKE_ID, 0)).rejects.toBe("no such insert");
    await expect(b.setInsertEnabled(target, FAKE_ID, false)).rejects.toBe("no such insert");
  });

  it("keeps the master chain and a track's chain apart", async () => {
    const { melody } = await ids();
    const [fuzz, tape] = await effects();
    await b.addInsert({ track: melody }, fuzz.uid);
    await b.addInsert("master", tape.uid);
    expect(await names({ track: melody })).toEqual([fuzz.name]);
    expect(await names("master")).toEqual([tape.name]);
    expect((await model()).masterInserts).toHaveLength(1);
  });

  it("takes the chain away with the track it belonged to", async () => {
    const { melody, drums } = await ids();
    const [e] = await effects();
    await b.addInsert({ track: drums }, e.uid);
    await b.apply({ type: "deleteTrack", id: drums });
    expect(await names({ track: drums })).toEqual([]);
    expect(await names({ track: melody })).toEqual([]);
  });

  it("puts a chain back on undo and takes it away again on redo", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const built = await fill(target, 2);
    await b.removeInsert(target, built[0].id);
    expect(await chain(target)).toHaveLength(1);

    await b.undo();
    expect((await chain(target)).map((i) => i.id)).toEqual(built.map((i) => i.id));
    await b.redo();
    expect((await chain(target)).map((i) => i.id)).toEqual([built[1].id]);
  });

  it("drops inserts that name nothing, repeat an id, or run past the cap when a file is read", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    await fill(target, 2);
    await b.saveProject("/tmp/chained.dissonant");

    // What a hand-edited project can carry. `openProject` runs the mock's `normalizedModel`,
    // which stands in for `ProjectModel::normalized`.
    const raw = JSON.parse(localStorage.getItem("dissonant.mock.project")!) as ProjectModel;
    const slot = (id: string, uid: string) => ({ id, uid, name: "x", enabled: true });
    raw.tracks[0].inserts = [
      slot(FAKE_ID, "   "),
      slot("11111111-0000-4000-8000-000000000000", "clap:/a.clap:a"),
      slot("11111111-0000-4000-8000-000000000000", "clap:/a.clap:a"),
      ...Array.from({ length: MAX_INSERTS + 2 }, (_, i) =>
        slot(`2222222${i}-0000-4000-8000-000000000000`, "clap:/b.clap:b"),
      ),
    ];
    localStorage.setItem("dissonant.mock.project", JSON.stringify(raw));

    await b.openProject("/tmp/chained.dissonant");
    const loaded = await chain(target);
    expect(loaded).toHaveLength(MAX_INSERTS);
    expect(loaded.every((i) => i.uid.trim().length > 0)).toBe(true);
    expect(new Set(loaded.map((i) => i.id)).size).toBe(MAX_INSERTS);
  });

  it("reads an effect's parameters and drags without touching the document", async () => {
    const { melody } = await ids();
    const target: InsertTarget = { track: melody };
    const [built] = await fill(target, 1);
    const res = (await b.insertParams(built.id)).find((p) => p.name === "resonance")!;
    const before = await b.getState();

    await b.setInsertParam(built.id, res.id, 0.9, false);
    expect((await b.insertParams(built.id)).find((p) => p.id === res.id)!.value).toBe(0.9);
    expect((await b.getState()).model).toEqual(before.model);

    const committed = await b.setInsertParam(built.id, res.id, 0.9, true);
    expect(committed?.model.tracks[0].inserts?.[0].state).toEqual(expect.any(String));

    // Undo has to reach the effect too, not just the document.
    await b.undo();
    expect((await b.insertParams(built.id)).find((p) => p.id === res.id)!.value).toBe(res.default);
  });

  it("clamps an effect's parameter to the range it published", async () => {
    const { melody } = await ids();
    const [built] = await fill({ track: melody }, 1);
    const cutoff = (await b.insertParams(built.id))[0];
    await b.setInsertParam(built.id, cutoff.id, 1e9);
    expect((await b.insertParams(built.id))[0].value).toBe(cutoff.max);
  });

  it("reports no parameters for an insert that is gone", async () => {
    const { melody } = await ids();
    const [built] = await fill({ track: melody }, 1);
    await b.removeInsert({ track: melody }, built.id);
    expect(await b.insertParams(built.id)).toEqual([]);
  });

  it("keeps a restored project's dialled-in values", async () => {
    const { melody } = await ids();
    const [built] = await fill({ track: melody }, 1);
    const res = (await b.insertParams(built.id)).find((p) => p.name === "resonance")!;
    await b.setInsertParam(built.id, res.id, 0.77, true);
    await b.saveProject("/tmp/dialled.dissonant");

    await b.newProject(true);
    await b.openProject("/tmp/dialled.dissonant");
    const reopened = (await model()).tracks[0].inserts![0];
    expect((await b.insertParams(reopened.id)).find((p) => p.id === res.id)!.value).toBe(0.77);
  });
});

describe("plugin catalog", () => {
  it("leads every entry with plain words and tags, not format jargon", async () => {
    const catalog = await b.pluginCatalog();
    expect(catalog.length).toBeGreaterThan(0);
    for (const e of catalog) {
      expect(e.summary.trim().length).toBeGreaterThan(0);
      expect(e.tags.length).toBeGreaterThan(0);
      expect(e.license.trim().length).toBeGreaterThan(0);
    }
  });

  it("installs an entry and the plugin then shows up in a scan", async () => {
    const seen: { phase: string; done: number; total: number }[] = [];
    b.onPluginInstall((p) => seen.push({ phase: p.phase, done: p.done, total: p.total }));
    expect((await b.scanPlugins()).map((p) => p.name)).not.toContain("Dexed");

    await b.installPlugin("dexed");

    expect(seen[0].phase).toBe("downloading");
    expect(seen.at(-1)?.phase).toBe("installed");
    // A progress bar drawn from these must never run past its end.
    for (const s of seen) expect(s.done).toBeLessThanOrEqual(s.total);
    expect((await b.scanPlugins()).map((p) => p.name)).toContain("Dexed");
  });

  it("refuses an entry that is not in the catalog", async () => {
    await expect(b.installPlugin("not-a-plugin")).rejects.toBe("no such catalog entry");
  });
});

// ─── audio tracks ──────────────────────────────────────────────────────────────────────────

describe("audio tracks", () => {
  const SOURCE_ID = "11111111-2222-3333-4444-555555555555";
  /** An id nothing in these tests ever uses. */
  const MISSING_ID = "99999999-9999-4999-8999-999999999999";

  /** Add an audio track and return its id. */
  async function addAudioTrack(name = "guitar"): Promise<string> {
    await b.apply({ type: "addAudioTrack", name });
    const m = await model();
    return m.tracks[m.tracks.length - 1].id;
  }

  const source = (over: Partial<AudioSource> = {}): AudioSource => ({
    id: SOURCE_ID,
    name: "take 1",
    frames: 48_000,
    sampleRate: 48_000,
    channels: 1,
    ...over,
  });

  const clip = (trackId: string, over: Partial<AudioClip> = {}): AudioClip => ({
    id: FAKE_ID,
    trackId,
    sourceId: SOURCE_ID,
    startTick: 0,
    lengthSeconds: 1,
    offsetSeconds: 0,
    gain: 1,
    fadeInSeconds: 0,
    fadeOutSeconds: 0,
    muted: false,
    ...over,
  });

  it("creates an audio track with a rig and no drum kit", async () => {
    const id = await addAudioTrack();
    const t = (await model()).tracks.find((x) => x.id === id)!;
    expect(t.isAudio).toBe(true);
    expect(t.isDrum).toBe(false);
    expect(t.amp).not.toBeNull();
    expect(t.amp!.amp.model).toBe("crunch");
    // A note track never carries one.
    expect((await model()).tracks[0].amp).toBeNull();
  });

  it("names a blank audio track rather than leaving it unidentifiable", async () => {
    await b.apply({ type: "addAudioTrack", name: "   " });
    const m = await model();
    expect(m.tracks[m.tracks.length - 1].name).toBe("audio");
  });

  it("lands a take as one source and one clip", async () => {
    const id = await addAudioTrack();
    await b.apply({ type: "addAudioTake", trackId: id, source: source(), clip: clip(id) });
    const m = await model();
    expect(m.audioSources).toHaveLength(1);
    expect(m.audioClips).toHaveLength(1);
    expect(m.audioClips![0].trackId).toBe(id);
  });

  it("refuses a take whose clip points somewhere else", async () => {
    const id = await addAudioTrack();
    // Both of these would be silently dropped by `normalized`, so they are refused outright.
    await rejectsAndKeepsModel(
      { type: "addAudioTake", trackId: id, source: source(), clip: clip(FAKE_ID) },
      "invalid value",
    );
    await rejectsAndKeepsModel(
      { type: "addAudioTake", trackId: id, source: source(), clip: clip(id, { sourceId: FAKE_ID }) },
      "invalid value",
    );
    await rejectsAndKeepsModel(
      { type: "addAudioTake", trackId: id, source: source({ frames: 0 }), clip: clip(id) },
      "invalid value",
    );
  });

  it("refuses audio on a note track", async () => {
    const { melody } = await ids();
    await rejectsAndKeepsModel({ type: "setTrackAmp", id: melody, amp: defaultAmpRig() }, "not an audio track");
    await rejectsAndKeepsModel(
      { type: "addAudioTake", trackId: melody, source: source(), clip: clip(melody) },
      "not an audio track",
    );
  });

  it("clamps a rig instead of storing nonsense, and rejects NaN outright", async () => {
    const id = await addAudioTrack();
    const base = defaultAmpRig();
    await b.apply({
      type: "setTrackAmp",
      id,
      amp: {
        ...base,
        inputDb: 99,
        amp: { ...base.amp, gain: -5, master: 50 },
        cab: { ...base.cab, posX: 99, posZ: -4 },
      },
    });
    const rig = (await model()).tracks.find((t) => t.id === id)!.amp!;
    expect(rig.inputDb).toBe(24);
    expect(rig.amp.gain).toBe(0);
    expect(rig.amp.master).toBe(10);
    // Mic placement is a 0…10 knob like the rest of the rig.
    expect(rig.cab.posX).toBe(10);
    expect(rig.cab.posZ).toBe(0);

    await rejectsAndKeepsModel(
      { type: "setTrackAmp", id, amp: { ...base, inputDb: Number.NaN } },
      "invalid value",
    );
    // A placement that cannot be made sane sinks the whole command, like any other knob.
    await rejectsAndKeepsModel(
      { type: "setTrackAmp", id, amp: { ...base, cab: { ...base.cab, posZ: Number.POSITIVE_INFINITY } } },
      "invalid value",
    );
  });

  it("defaults an unknown eq or mic instead of refusing the rig", async () => {
    const id = await addAudioTrack();
    const base = defaultAmpRig();
    // What a file from another build looks like: a component this build has never heard of, and
    // one the old shape never wrote at all.
    await b.apply({
      type: "setTrackAmp",
      id,
      amp: {
        ...base,
        eq: "tweedBright" as unknown as EqModel,
        cab: { ...base.cab, micModel: undefined as unknown as MicModel },
      },
    });
    const rig = (await model()).tracks.find((t) => t.id === id)!.amp!;
    expect(rig.eq).toBe("usClassic");
    expect(rig.cab.micModel).toBe("dynamic");
  });

  it("keeps at most MAX_PEDALS pedals", async () => {
    const id = await addAudioTrack();
    const pedals = PEDAL_KINDS.map(newPedal);
    expect(pedals.length).toBeGreaterThan(MAX_PEDALS);
    await b.apply({ type: "setTrackAmp", id, amp: { ...defaultAmpRig(), pedals } });
    expect((await model()).tracks.find((t) => t.id === id)!.amp!.pedals).toHaveLength(MAX_PEDALS);
  });

  it("clamps clip gain and fades, and refuses a clip with no length", async () => {
    const id = await addAudioTrack();
    await b.apply({ type: "addAudioTake", trackId: id, source: source(), clip: clip(id, { lengthSeconds: 2 }) });
    const placed = (await model()).audioClips![0];

    await b.apply({
      type: "updateAudioClip",
      clip: { ...placed, gain: 99, fadeInSeconds: 99, fadeOutSeconds: -1, offsetSeconds: -3 },
    });
    const edited = (await model()).audioClips![0];
    expect(edited.gain).toBe(MAX_CLIP_GAIN);
    // Fades may not overlap, so each is capped at half the clip.
    expect(edited.fadeInSeconds).toBe(1);
    expect(edited.fadeOutSeconds).toBe(0);
    expect(edited.offsetSeconds).toBe(0);

    await rejectsAndKeepsModel({ type: "updateAudioClip", clip: { ...placed, lengthSeconds: 0 } }, "invalid value");
    await rejectsAndKeepsModel({ type: "updateAudioClip", clip: { ...placed, startTick: -1 } }, "invalid value");
    // Not `FAKE_ID`: the helper gives the placed clip that id, so it names a real clip here.
    await rejectsAndKeepsModel({ type: "removeAudioClip", id: MISSING_ID }, "no such audio clip");
  });

  it("keeps audio clips in start order", async () => {
    const id = await addAudioTrack();
    await b.apply({ type: "addAudioTake", trackId: id, source: source(), clip: clip(id, { startTick: 8 * TICKS_PER_BEAT }) });
    await b.apply({
      type: "addAudioTake",
      trackId: id,
      source: source(),
      clip: clip(id, { id: "22222222-2222-2222-2222-222222222222", startTick: 2 * TICKS_PER_BEAT }),
    });
    expect((await model()).audioClips!.map((c) => c.startTick)).toEqual([1920, 7680]);
    // The same source twice is stored once.
    expect((await model()).audioSources).toHaveLength(1);
  });

  it("purges a deleted audio track's clips", async () => {
    const id = await addAudioTrack();
    await b.apply({ type: "addAudioTake", trackId: id, source: source(), clip: clip(id) });
    await b.apply({ type: "deleteTrack", id });
    const m = await model();
    expect(m.audioClips).toHaveLength(0);
    // The source survives: undoing the delete must not force a re-decode.
    expect(m.audioSources).toHaveLength(1);
  });

  it("records one undo step for a rig gesture", async () => {
    const id = await addAudioTrack();
    const before = (await model()).tracks.find((t) => t.id === id)!.amp!;
    for (const gain of [6, 7, 8, 9]) {
      await b.apply({ type: "setTrackAmp", id, amp: { ...before, amp: { ...before.amp, gain } } }, true);
    }
    await b.commitGesture();
    const s = await b.undo();
    expect(s.model.tracks.find((t) => t.id === id)!.amp!.amp.gain).toBe(before.amp.gain);
  });

  it("survives a save and reopen", async () => {
    const id = await addAudioTrack();
    await b.apply({
      type: "setTrackAmp",
      id,
      amp: {
        ...defaultAmpRig(),
        pedals: [newPedal("delay"), newPedal("wah")],
        eq: "britishBright",
        cab: { model: "twoByTwelve", micModel: "ribbon", posX: 7.5, posZ: 1 },
      },
    });
    await b.apply({ type: "addAudioTake", trackId: id, source: source(), clip: clip(id) });
    await b.saveProject("/tmp/guitar.dissonant");
    await b.newProject(false);
    await b.openProject("/tmp/guitar.dissonant");

    const m = await model();
    const t = m.tracks.find((x) => x.isAudio)!;
    expect(t.amp!.pedals.map((p) => p.kind)).toEqual(["delay", "wah"]);
    expect(t.amp!.eq).toBe("britishBright");
    expect(t.amp!.cab).toEqual({ model: "twoByTwelve", micModel: "ribbon", posX: 7.5, posZ: 1 });
    expect(m.audioClips).toHaveLength(1);
  });

  it("drops a clip whose source is gone when the project is read back", async () => {
    const id = await addAudioTrack();
    await b.apply({ type: "addAudioTake", trackId: id, source: source(), clip: clip(id) });
    await b.saveProject("/tmp/orphan.dissonant");
    // Simulate a file hand-edited to lose its source list.
    const m = await model();
    m.audioSources = [];
    await b.newProject(false);
    await b.openProject("/tmp/orphan.dissonant");
    expect((await model()).audioClips).toHaveLength(1);
  });

  it("says so plainly when recording needs the desktop app", async () => {
    await expect(b.startAudioRecord()).rejects.toThrow("desktop app");
    await expect(b.stopAudioRecord()).rejects.toThrow("desktop app");
    await expect(b.importAudio("/tmp/riff.wav", null)).rejects.toThrow("desktop app");
    expect(await b.inputDevices()).toEqual([]);
  });

  it("draws a deterministic waveform so the screen can be designed without audio", async () => {
    const a = await b.audioPeaks(SOURCE_ID, 64);
    const again = await b.audioPeaks(SOURCE_ID, 64);
    expect(a).toHaveLength(64);
    expect(a).toEqual(again);
    expect(await b.audioPeaks("a-different-id", 64)).not.toEqual(a);
    for (const [lo, hi] of a) {
      expect(lo).toBeLessThanOrEqual(hi);
      expect(lo).toBeGreaterThanOrEqual(-1);
      expect(hi).toBeLessThanOrEqual(1);
    }
  });
});

/** Mirrors the automation section of crates/dissonant-core/src/document.rs. */
describe("automation", () => {
  const MASTER_GAIN: AutoTarget = { master: "gain" };
  const lane = async (target: AutoTarget) => automationLane(await model(), target);
  /** One lane's points as `[tick, value, curve]`, which is what the rules are about. */
  const shape = async (target: AutoTarget) =>
    (await lane(target))!.points.map((p) => [p.tick, p.value, p.curve]);

  it("creates one lane per target and replaces the point already on a tick", async () => {
    const { melody } = await ids();
    const volume: AutoTarget = { track: [melody, "volume"] };
    await b.apply({ type: "addAutomationPoint", target: volume, tick: 4 * TICKS_PER_BEAT, value: 0.5, curve: "ramp" });
    await b.apply({ type: "addAutomationPoint", target: volume, tick: 0, value: 1, curve: "step" });
    expect((await model()).automation).toHaveLength(1);
    expect(await shape(volume)).toEqual([
      [0, 1, "step"],
      [4 * TICKS_PER_BEAT, 0.5, "ramp"],
    ]);
    // Drawing again on a tick replaces what was there rather than stacking a second point.
    await b.apply({ type: "addAutomationPoint", target: volume, tick: 0, value: 0.25, curve: "ramp" });
    expect(await shape(volume)).toEqual([
      [0, 0.25, "ramp"],
      [4 * TICKS_PER_BEAT, 0.5, "ramp"],
    ]);
    await b.apply({ type: "addAutomationPoint", target: MASTER_GAIN, tick: 0, value: 1, curve: "step" });
    expect((await model()).automation).toHaveLength(2);
  });

  it("keeps a point's id when it moves and swallows the neighbour it lands on", async () => {
    await b.apply({ type: "addAutomationPoint", target: MASTER_GAIN, tick: 0, value: 1, curve: "step" });
    await b.apply({ type: "addAutomationPoint", target: MASTER_GAIN, tick: 8 * TICKS_PER_BEAT, value: 0, curve: "ramp" });
    const l = (await lane(MASTER_GAIN))!;
    const moved = { ...l.points[1], tick: 4 * TICKS_PER_BEAT, value: 0.5 };
    await b.apply({ type: "updateAutomationPoint", lane: l.id, point: moved });
    expect(await shape(MASTER_GAIN)).toEqual([
      [0, 1, "step"],
      [4 * TICKS_PER_BEAT, 0.5, "ramp"],
    ]);
    expect((await lane(MASTER_GAIN))!.points[1].id).toBe(l.points[1].id);
    // Dragged onto its neighbour, the edited point wins and the other one goes.
    await b.apply({ type: "updateAutomationPoint", lane: l.id, point: { ...moved, tick: 0 } });
    expect(await shape(MASTER_GAIN)).toEqual([[0, 0.5, "ramp"]]);
    expect((await lane(MASTER_GAIN))!.points[0].id).toBe(l.points[1].id);
  });

  it("drops the lane when its last point is removed, and clears a whole lane", async () => {
    await b.apply({ type: "addAutomationPoint", target: MASTER_GAIN, tick: 0, value: 1, curve: "step" });
    await b.apply({ type: "addAutomationPoint", target: MASTER_GAIN, tick: 960, value: 0.5, curve: "ramp" });
    const l = (await lane(MASTER_GAIN))!;
    await b.apply({ type: "removeAutomationPoint", lane: l.id, id: l.points[0].id });
    expect(await shape(MASTER_GAIN)).toEqual([[960, 0.5, "ramp"]]);
    await b.apply({ type: "removeAutomationPoint", lane: l.id, id: l.points[1].id });
    expect(await lane(MASTER_GAIN)).toBeUndefined();
    // An automation-free project carries no lanes at all, not an empty list.
    expect((await model()).automation).toBeUndefined();

    await b.apply({ type: "addAutomationPoint", target: MASTER_GAIN, tick: 0, value: 1, curve: "step" });
    await b.apply({ type: "clearAutomation", target: MASTER_GAIN });
    expect((await model()).automation).toBeUndefined();
  });

  it("rejects an edit that names nothing and leaves the model alone", async () => {
    const { melody } = await ids();
    const volume: AutoTarget = { track: [melody, "volume"] };
    await rejectsAndKeepsModel({ type: "addAutomationPoint", target: volume, tick: -1, value: 1, curve: "step" }, "invalid value");
    await rejectsAndKeepsModel({ type: "addAutomationPoint", target: volume, tick: 0, value: Number.NaN, curve: "step" }, "invalid value");
    await rejectsAndKeepsModel({ type: "addAutomationPoint", target: { track: [FAKE_ID, "volume"] }, tick: 0, value: 1, curve: "step" }, "no such track");
    await rejectsAndKeepsModel({ type: "addAutomationPoint", target: { trackPlugin: [FAKE_ID, 7] }, tick: 0, value: 1, curve: "step" }, "no such track");
    await rejectsAndKeepsModel({ type: "addAutomationPoint", target: { insert: [{ track: FAKE_ID }, FAKE_ID, 3] }, tick: 0, value: 1, curve: "step" }, "no such track");
    await rejectsAndKeepsModel({ type: "clearAutomation", target: volume }, "no such automation lane");
    await rejectsAndKeepsModel({ type: "removeAutomationPoint", lane: FAKE_ID, id: FAKE_ID }, "no such automation lane");

    await b.apply({ type: "addAutomationPoint", target: volume, tick: 0, value: 1, curve: "step" });
    const l = (await lane(volume))!;
    await rejectsAndKeepsModel({ type: "removeAutomationPoint", lane: l.id, id: FAKE_ID }, "no such automation point");
    await rejectsAndKeepsModel({ type: "updateAutomationPoint", lane: l.id, point: { ...l.points[0], id: FAKE_ID } }, "no such automation point");
    await rejectsAndKeepsModel({ type: "updateAutomationPoint", lane: FAKE_ID, point: l.points[0] }, "no such automation lane");
    await rejectsAndKeepsModel({ type: "updateAutomationPoint", lane: l.id, point: { ...l.points[0], tick: -1 } }, "invalid value");
    await rejectsAndKeepsModel({ type: "updateAutomationPoint", lane: l.id, point: { ...l.points[0], value: Number.NaN } }, "invalid value");
  });

  it("holds every value inside its target's range, like the fader it writes to", async () => {
    const { melody } = await ids();
    const drawn = async (target: AutoTarget, value: number) => {
      await b.apply({ type: "addAutomationPoint", target, tick: 0, value, curve: "step" });
      return (await lane(target))!.points[0].value;
    };
    expect(await drawn({ track: [melody, "volume"] }, 9)).toBe(1.5);
    expect(await drawn({ track: [melody, "pan"] }, -9)).toBe(-1);
    expect(await drawn({ track: [melody, "tone"] }, 0)).toBe(200);
    expect(await drawn({ track: [melody, "reverbSend"] }, 2)).toBe(1);
    expect(await drawn(MASTER_GAIN, 2)).toBe(1.5);
    // A plugin parameter's range belongs to the plugin, so the host leaves it alone.
    expect(await drawn({ trackPlugin: [melody, 7] }, 42)).toBe(42);
    expect(await drawn({ insert: ["master", FAKE_ID, 3] }, 42)).toBe(42);
  });

  it("drops the lanes of a deleted track and keeps everyone else's", async () => {
    const { melody, drums } = await ids();
    for (const target of [
      { track: [melody, "volume"] } as AutoTarget,
      { trackPlugin: [melody, 7] } as AutoTarget,
      { insert: [{ track: melody }, FAKE_ID, 3] } as AutoTarget,
      { track: [drums, "pan"] } as AutoTarget,
      MASTER_GAIN,
    ]) {
      await b.apply({ type: "addAutomationPoint", target, tick: 0, value: 0.5, curve: "step" });
    }
    expect((await model()).automation).toHaveLength(5);
    await b.apply({ type: "deleteTrack", id: melody });
    const left = (await model()).automation!;
    expect(left).toHaveLength(2);
    expect(await lane({ track: [drums, "pan"] })).toBeDefined();
    expect(await lane(MASTER_GAIN)).toBeDefined();
  });
});

// ─── ripple edits ──────────────────────────────────────────────────────────────────────────

describe("insertTime / deleteTime / duplicateRange (ripple.rs)", () => {
  const BAR = 4 * TICKS_PER_BEAT;

  /** Everything inside `[from, to)` measured from `from`: two stretches of song, compared. */
  const windowOf = (m: ProjectModel, from: number, to: number) => ({
    clips: m.clips.filter((c) => c.startTick >= from && c.startTick < to).map((c) => [c.patternId, c.startTick - from, c.lengthTicks, c.offsetTicks]),
    sections: m.sections.filter((s) => s.startTick >= from && s.startTick < to).map((s) => [s.name, s.startTick - from]),
    chords: m.chords.filter((c) => c.startTick >= from && c.startTick < to).map((c) => [c.startTick - from, c.lengthTicks, c.pitchClasses]),
    tempo: (m.tempoPoints ?? []).filter((p) => p.tick >= from && p.tick < to).map((p) => [p.tick - from, p.bpm]),
  });

  it("moves every kind of thing past the cut, and splits the clip it falls inside", async () => {
    const { melody } = await ids();
    await b.apply({ type: "addSection", name: "chorus", startTick: 16 * BAR });
    await b.apply({ type: "addTempoPoint", tick: 16 * BAR, bpm: 140, ramp: false });
    await b.apply({ type: "addAutomationPoint", target: { track: [melody, "volume"] } as AutoTarget, tick: 20 * BAR, value: 0.5, curve: "step" });
    await b.apply({ type: "setLoopRegion", region: [16 * BAR, 20 * BAR] });
    const before = await model();

    await b.apply({ type: "insertTime", at: 16 * BAR, ticks: 4 * BAR });
    const m = await model();

    expect(m.songLengthTicks).toBe(before.songLengthTicks + 4 * BAR);
    // The starter's clips run the whole song, so each is cut in two at bar 17 and the tail
    // picks its pattern up exactly where the head left it.
    expect(m.clips.map((c) => [c.startTick, c.lengthTicks, c.offsetTicks])).toEqual([
      [0, 16 * BAR, 0],
      [0, 16 * BAR, 0],
      [20 * BAR, 16 * BAR, 16 * BAR],
      [20 * BAR, 16 * BAR, 16 * BAR],
    ]);
    expect(m.sections[0].startTick).toBe(20 * BAR);
    expect(m.tempoPoints![0].tick).toBe(20 * BAR);
    expect(m.automation![0].points[0].tick).toBe(24 * BAR);
    expect(m.loopRegion).toEqual([20 * BAR, 24 * BAR]);
    // The progression moved with the music (the clips are asserted above; they start before
    // the cut, so they are not part of this window either way).
    expect(windowOf(m, 20 * BAR, m.songLengthTicks).chords).toEqual(windowOf(before, 16 * BAR, before.songLengthTicks).chords);

    // One command, one undo step.
    expect((await b.undo()).model).toEqual(before);
  });

  it("cuts a range out and closes the gap, taking what lived inside it", async () => {
    const { melody } = await ids();
    await b.apply({ type: "addSection", name: "break", startTick: 16 * BAR });
    await b.apply({ type: "addTempoPoint", tick: 17 * BAR, bpm: 140, ramp: false });
    await b.apply({ type: "addAutomationPoint", target: { track: [melody, "volume"] } as AutoTarget, tick: 18 * BAR, value: 0.5, curve: "step" });
    const before = await model();

    await b.apply({ type: "deleteTime", at: 16 * BAR, ticks: 4 * BAR });
    const m = await model();

    expect(m.songLengthTicks).toBe(before.songLengthTicks - 4 * BAR);
    expect(m.clips.map((c) => [c.startTick, c.lengthTicks, c.offsetTicks])).toEqual([
      [0, 16 * BAR, 0],
      [0, 16 * BAR, 0],
      [16 * BAR, 12 * BAR, 20 * BAR],
      [16 * BAR, 12 * BAR, 20 * BAR],
    ]);
    expect(m.sections.map((s) => s.startTick)).toEqual([16 * BAR]);
    expect(m.tempoPoints ?? []).toEqual([]);
    expect(m.automation ?? []).toEqual([]);
    expect(m.chords).toHaveLength(before.chords.length - 4);
    expect(windowOf(m, 16 * BAR, m.songLengthTicks).chords).toEqual(windowOf(before, 20 * BAR, before.songLengthTicks).chords);
  });

  it("duplicates a range at the target, marker and all, pushing what was there along", async () => {
    const { pattern, drumPattern } = await ids();
    const start = await model();
    await b.apply({ type: "removeClips", ids: start.clips.map((c) => c.id) });
    await b.apply({ type: "addClip", patternId: pattern, startTick: 0, lengthTicks: 4 * BAR });
    await b.apply({ type: "addClip", patternId: drumPattern, startTick: 4 * BAR, lengthTicks: 4 * BAR });
    await b.apply({ type: "addClip", patternId: pattern, startTick: 8 * BAR, lengthTicks: 4 * BAR });
    await b.apply({ type: "addSection", name: "verse", startTick: 0 });
    await b.apply({ type: "addTempoPoint", tick: 2 * BAR, bpm: 100, ramp: false });
    const before = await model();

    await b.apply({ type: "duplicateRange", from: 0, to: 8 * BAR, at: 8 * BAR });
    const m = await model();

    expect(m.songLengthTicks).toBe(before.songLengthTicks + 8 * BAR);
    expect(windowOf(m, 0, 8 * BAR)).toEqual(windowOf(before, 0, 8 * BAR));
    expect(windowOf(m, 8 * BAR, 16 * BAR)).toEqual(windowOf(before, 0, 8 * BAR));
    expect(windowOf(m, 16 * BAR, 24 * BAR)).toEqual(windowOf(before, 8 * BAR, 16 * BAR));
    // Copies are copies: every id in the song is still its own.
    expect(new Set(m.clips.map((c) => c.id)).size).toBe(m.clips.length);
    expect(new Set(m.sections.map((s) => s.id)).size).toBe(m.sections.length);
  });

  it("grows a section the time opens inside instead of splitting it", async () => {
    await b.apply({ type: "addSection", name: "outro", startTick: 8 * BAR });
    const outro = (await model()).sections[0];
    await b.apply({ type: "updateSection", section: { ...outro, lengthTicks: 4 * BAR } });
    await b.apply({ type: "insertTime", at: 10 * BAR, ticks: 2 * BAR });
    expect((await model()).sections[0]).toMatchObject({ startTick: 8 * BAR, lengthTicks: 6 * BAR });

    // And one cut away to nothing goes with its music.
    await b.apply({ type: "deleteTime", at: 8 * BAR, ticks: 6 * BAR });
    expect((await model()).sections).toEqual([]);
  });

  it("refuses a ripple that cannot mean anything", async () => {
    await rejectsAndKeepsModel({ type: "insertTime", at: -1, ticks: BAR }, "invalid value");
    await rejectsAndKeepsModel({ type: "insertTime", at: 0, ticks: 0 }, "invalid value");
    await rejectsAndKeepsModel({ type: "deleteTime", at: 0, ticks: -1 }, "invalid value");
    await rejectsAndKeepsModel({ type: "duplicateRange", from: 4 * BAR, to: 4 * BAR, at: 0 }, "invalid value");
    await rejectsAndKeepsModel({ type: "duplicateRange", from: -1, to: BAR, at: 0 }, "invalid value");
  });
});
