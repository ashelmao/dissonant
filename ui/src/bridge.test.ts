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
import { OLD_SCHEMA } from "./bridge";
import type { ChordEvent, Clip, Command, MasterSettings, NoteEvent, ProjectModel } from "./types";
import { SOUND_PRESETS, soundPresetOf } from "./types";

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

function note(startBeat: number, pitch: number, extra: Partial<NoteEvent> = {}): NoteEvent {
  return { id: FAKE_ID, startBeat, lengthBeats: 1, pitch, velocity: 100, ...extra };
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
    expect(m.schemaVersion).toBe(6);
    expect(m.tempo).toBe(120);
    expect(m.key).toEqual({ rootPitchClass: 0, scale: "major", isLocked: true });
    expect(m.tracks.map((t) => [t.name, t.isDrum])).toEqual([
      ["melody", false],
      ["drums", true],
    ]);
    // One pattern per track.
    const [melody, drums] = m.tracks;
    expect(m.patterns.map((p) => [p.name, p.trackId, p.lengthBeats])).toEqual([
      ["melody 1", melody.id, 16],
      ["drums 1", drums.id, 16],
    ]);
    // The song is editing.defaultSongBars (32) of 4/4, and I–IV–V–vi is tiled across all of it.
    expect(m.songLengthBeats).toBe(128);
    expect(m.chords).toHaveLength(32);
    expect(m.chords.slice(0, 5).map((c) => [c.startBeat, c.name])).toEqual([
      [0, "C"],
      [4, "F"],
      [8, "G"],
      [12, "Am"],
      [16, "C"],
    ]);
    // A melody (STARTER_MELODY) and a groove (starter_groove), so space plays music at once.
    expect(melody.voice).toBe("pluck");
    expect(m.patterns[0].notes.map((n) => n.pitch)).toEqual([76, 74, 72, 69, 72, 77, 74, 71, 67, 72, 71, 69]);
    const count = (pitch: number) => m.patterns[1].notes.filter((n) => n.pitch === pitch).length;
    expect([count(36), count(38), count(42)]).toEqual([8, 8, 32]);
    // One clip per track, loop-extended from 0 to the song end.
    expect(m.clips.map((c) => [c.patternId, c.startBeat, c.lengthBeats, c.offsetBeats])).toEqual([
      [m.patterns[0].id, 0, 128, 0],
      [m.patterns[1].id, 0, 128, 0],
    ]);
    expect(m.master).toEqual({ gain: 1, reverbWet: 0, lowCutHz: 20, highCutHz: 18000, lowEq: 1, midEq: 1, highEq: 1 });
    expect([s.canUndo, s.canRedo, s.dirty, s.path]).toEqual([false, false, false, null]);
  });

  it("matches ProjectModel::empty() when starter is false", async () => {
    const s = await b.newProject(false);
    expect(s.model.tracks.map((t) => t.name)).toEqual(["melody"]);
    expect(s.model.tracks[0].isDrum).toBe(false);
    expect(s.model.patterns.map((p) => [p.name, p.trackId])).toEqual([["melody 1", s.model.tracks[0].id]]);
    expect(s.model.chords).toEqual([]);
    expect(s.model.clips).toEqual([]);
    expect(s.model.songLengthBeats).toBe(128);
  });

  it("sizes the song from editing.defaultSongBars", async () => {
    const settings = await b.getSettings();
    expect(settings.editing.defaultSongBars).toBe(32);
    await b.setSettings({ ...settings, editing: { ...settings.editing, defaultSongBars: 8 } });
    const m = (await b.newProject(true)).model;
    expect(m.songLengthBeats).toBe(32);
    expect(m.chords.at(-1)!.startBeat + m.chords.at(-1)!.lengthBeats).toBe(32);
    expect(m.clips.every((c) => c.lengthBeats === 32)).toBe(true);
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
    await b.apply({ type: "addClip", patternId: extra, startBeat: 64, lengthBeats: null });
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
    expect(p[1]).toMatchObject({ lengthBeats: 16, notes: [] });
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
    await rejectsAndKeepsModel({ type: "setPatternLength", id: pattern, beats: 0.5 }, "invalid value");
    await rejectsAndKeepsModel({ type: "setPatternLength", id: pattern, beats: 0 }, "invalid value");
    await rejectsAndKeepsModel({ type: "setPatternLength", id: pattern, beats: Number.NaN });
    await rejectsAndKeepsModel({ type: "setPatternLength", id: FAKE_ID, beats: 8 }, "no such pattern");
    await b.apply({ type: "setPatternLength", id: pattern, beats: 8 });
    expect((await model()).patterns[0].lengthBeats).toBe(8);
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
        note(0, 300, { velocity: 999, lengthBeats: 0 }),
        note(-4, -9, { velocity: 0, lengthBeats: -1 }),
      ],
    });
    const notes = (await model()).patterns[0].notes;
    expect(notes.map((n) => n.pitch)).toEqual([0, 127]);
    expect(notes.map((n) => n.velocity)).toEqual([1, 127]);
    expect(notes.map((n) => n.startBeat)).toEqual([0, 0]);
    expect(notes.every((n) => n.lengthBeats === 1 / 64)).toBe(true);
  });

  it("sorts by start beat then pitch", async () => {
    const { pattern } = await ids();
    await b.apply({
      type: "setNotes",
      patternId: pattern,
      notes: [note(4, 60), note(0, 72), note(0, 48), note(2, 55)],
    });
    const notes = (await model()).patterns[0].notes;
    expect(notes.map((n) => [n.startBeat, n.pitch])).toEqual([
      [0, 48],
      [0, 72],
      [2, 55],
      [4, 60],
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
    await rejectsAndKeepsModel(bad(note(0, 60, { lengthBeats: Number.NaN })));
    await rejectsAndKeepsModel(bad(note(0, Number.POSITIVE_INFINITY)));
  });
});

// ─── chords and arrangement ────────────────────────────────────────────────────────────────

describe("setChords", () => {
  const chord = (startBeat: number, lengthBeats: number, pitchClasses: number[]): ChordEvent => ({
    id: FAKE_ID,
    startBeat,
    lengthBeats,
    pitchClasses,
    name: null,
  });

  it("replaces the project progression (song beats), dropping empty or negatively placed chords", async () => {
    await b.apply({
      type: "setChords",
      chords: [chord(40, 4, [7, 23, -1, 11, 7]), chord(2, 0, [0]), chord(-1, 4, [0]), chord(0, 4, [4, 0, 7])],
    });
    const chords = (await model()).chords;
    expect(chords.map((c) => c.startBeat)).toEqual([0, 40]);
    expect(chords[0].pitchClasses).toEqual([0, 4, 7]);
    expect(chords[1].pitchClasses).toEqual([7, 11]);
  });
});

describe("clips", () => {
  it("places clips in start order and validates like AddClip", async () => {
    const { pattern } = await ids();
    await b.apply({ type: "addClip", patternId: pattern, startBeat: 16, lengthBeats: null });
    expect((await model()).clips.map((c) => c.startBeat)).toEqual([0, 0, 16]);
    // Clip length defaults to the pattern's own length.
    expect((await model()).clips[2].lengthBeats).toBe(16);
    await rejectsAndKeepsModel({ type: "addClip", patternId: FAKE_ID, startBeat: 32, lengthBeats: null }, "no such pattern");
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startBeat: -1, lengthBeats: null }, "invalid value");
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startBeat: 32, lengthBeats: 0 }, "invalid value");
    await rejectsAndKeepsModel({ type: "removeClip", id: FAKE_ID }, "no such clip");
  });

  it("refuses a clip at or past the song end and clamps one running over it", async () => {
    const { pattern } = await ids();
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startBeat: 128, lengthBeats: null }, "that is past the end of the song");
    await rejectsAndKeepsModel({ type: "addClip", patternId: pattern, startBeat: 200, lengthBeats: 4 }, "that is past the end of the song");
    await b.apply({ type: "addClip", patternId: pattern, startBeat: 120, lengthBeats: null });
    const placed = (await model()).clips.find((c) => c.startBeat === 120)!;
    expect(placed.lengthBeats).toBe(8);
  });

  it("clamps updateClip the same way and never moves a clip to another pattern", async () => {
    const { drumPattern } = await ids();
    const c = (await model()).clips[0];
    await b.apply({ type: "updateClip", clip: { ...c, startBeat: 100, lengthBeats: 64, patternId: drumPattern } });
    const moved = await clip(c.id);
    expect([moved.startBeat, moved.lengthBeats, moved.patternId]).toEqual([100, 28, c.patternId]);
    await rejectsAndKeepsModel({ type: "updateClip", clip: { ...moved, startBeat: 128 } }, "that is past the end of the song");
    await rejectsAndKeepsModel({ type: "updateClip", clip: { ...moved, id: FAKE_ID } }, "no such clip");
    await rejectsAndKeepsModel({ type: "updateClip", clip: { ...moved, lengthBeats: Number.NaN } }, "invalid value");
  });

  it("moves a group of clips with updateClips as one undo step", async () => {
    const [a, c] = (await model()).clips;
    await b.apply({ type: "setSongLength", beats: 256 });
    await b.apply({ type: "updateClips", clips: [{ ...a, startBeat: 32 }, { ...c, startBeat: 32 }] });
    expect((await model()).clips.map((x) => x.startBeat)).toEqual([32, 32]);
    const back = (await b.undo()).model;
    expect(back.clips.map((x) => x.startBeat)).toEqual([0, 0]);
    // One bad clip rejects the whole batch.
    await rejectsAndKeepsModel({ type: "updateClips", clips: [{ ...a, startBeat: 8 }, { ...c, id: FAKE_ID }] }, "no such clip");
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
    await b.apply({ type: "addClip", patternId: pattern, startBeat: 64, lengthBeats: null });
    const target = (await model()).clips.find((c) => c.startBeat === 64)!;
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
    await b.apply({ type: "addClip", patternId: pattern, startBeat: 64, lengthBeats: 16 });
    await b.apply({ type: "addClip", patternId: pattern, startBeat: 96, lengthBeats: 16 });
    await b.apply({ type: "setSongLength", beats: 72 });
    const m = await model();
    expect(m.songLengthBeats).toBe(72);
    expect(m.clips.map((c) => [c.startBeat, c.lengthBeats])).toEqual([
      [0, 72],
      [0, 72],
      [64, 8],
    ]);
  });

  it("keeps at least one bar and rejects a non-finite length", async () => {
    await b.apply({ type: "setSongLength", beats: 1 });
    expect((await model()).songLengthBeats).toBe(4);
    await b.apply({ type: "setTimeSignature", numerator: 3, denominator: 4 });
    await b.apply({ type: "setSongLength", beats: 0 });
    expect((await model()).songLengthBeats).toBe(3);
    await rejectsAndKeepsModel({ type: "setSongLength", beats: Number.NaN }, "invalid value");
  });

  it("grows without touching clips, and undoes in one step", async () => {
    const before = await model();
    await b.apply({ type: "setSongLength", beats: 256 });
    expect((await model()).clips).toEqual(before.clips);
    await b.apply({ type: "setSongLength", beats: 8 });
    expect((await b.undo()).model.clips).toEqual(before.clips);
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
    ...over,
  });

  it("clamps every field", async () => {
    await b.apply({
      type: "setMaster",
      master: master({ gain: 9, reverbWet: 3, lowCutHz: 1, highCutHz: 99_999, lowEq: 5, midEq: -1, highEq: 5 }),
    });
    expect((await model()).master).toEqual({ gain: 1.5, reverbWet: 1, lowCutHz: 10, highCutHz: 20_000, lowEq: 2, midEq: 0, highEq: 2 });
    await b.apply({ type: "setMaster", master: master({ lowCutHz: 9_999, highCutHz: 1 }) });
    expect((await model()).master).toMatchObject({ lowCutHz: 2_000, highCutHz: 500 });
  });

  it("rejects a non-finite field", async () => {
    await rejectsAndKeepsModel({ type: "setMaster", master: master({ gain: Number.NaN }) });
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
      await b.apply({ type: "setPatternLength", id: pattern, beats: i + 2 });
    }
    for (let i = 0; i < 200; i++) await b.undo();
    const s = await b.getState();
    expect(s.model.patterns[0].lengthBeats).toBe(2);
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

  it("refuses a project older than schema 6, keeping the open one", async () => {
    const before = await model();
    const old = { ...before, schemaVersion: 5 };
    localStorage.setItem("dissonant.mock.project", JSON.stringify(old));
    await expect(b.openProject("/tmp/old.dissonant")).rejects.toBe(OLD_SCHEMA);
    expect(await model()).toEqual(before);
  });

  it("refuses an old template the same way", async () => {
    localStorage.setItem("dissonant.mock.templates", JSON.stringify({ old: { ...(await model()), schemaVersion: 5 } }));
    await expect(b.newFromTemplate("old")).rejects.toBe(OLD_SCHEMA);
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
      expect(m.clips.map((c) => [c.patternId, c.startBeat, c.lengthBeats])).toEqual(m.patterns.map((p) => [p.id, 0, m.songLengthBeats]));
      // The progression is tiled across the whole song; the first 16 beats name one pass.
      expect(m.chords.at(-1)!.startBeat + m.chords.at(-1)!.lengthBeats).toBe(m.songLengthBeats);
      const count = (track: number, pitch?: number) => m.patterns[track].notes.filter((n) => pitch === undefined || n.pitch === pitch).length;
      described.push(
        `${info.id}|${info.tempo}|${m.swing}|${m.tracks.map((t) => t.name).join(",")}|${m.chords.filter((c) => c.startBeat < 16).map((c) => c.name).join(" ")}|${m.tracks[0].voice}|kick ${count(2, 36)}|bass ${count(1)}|${count(2)}`,
      );
    }
    expect(described).toEqual([
      "lofi|80|60|keys,bass,drums|Gm C F Dm|triangle|kick 12|bass 12|52",
      "postpunk|148|50|lead,bass,drums|Em C G D|square|kick 12|bass 32|52",
      "ambient|70|50|pad,bass,drums|D G|pad|kick 4|bass 4|8",
      "songwriter|96|50|melody,bass,drums|G D Em C|pluck|kick 8|bass 8|48",
      "club|124|50|lead,bass,drums|Am G F G|square|kick 16|bass 16|40",
    ]);
  });

  it("rejects an unknown vibe and keeps the project", async () => {
    const before = await model();
    await expect(b.newFromVibe("polka")).rejects.toBeTruthy();
    expect(await model()).toEqual(before);
  });
});
