// DI detection in the UI: the detected lane's reading of the model, the key suggestion's use of
// heard chroma, the mock's parity with `document.rs` for the three detection commands, and the
// store's "notes" flow landing as one undo step.

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Bridge } from "./bridge";
import type * as StoreModule from "./store";
import type { AudioClip, AudioSource, ClipDetection, DetectedChord, DetectionReport, NoteEvent, ProjectModel } from "./types";
import { TICKS_PER_BEAT as B } from "./types";
import { confidenceText, detectedChords, detectedChroma, detectionSummary } from "./detection";
import { detectKey } from "./theory";

const SOURCE: AudioSource = { id: "5a5a5a5a-0000-4000-8000-000000000001", name: "take 3", frames: 96_000, sampleRate: 48_000, channels: 1 };

function chord(startTick: number, lengthTicks: number, pitchClasses: number[], name: string, id = crypto.randomUUID()): DetectedChord {
  return { id, startTick, lengthTicks, pitchClasses, name, confidence: 0.9 };
}

function detection(clipId: string, chords: DetectedChord[], chroma = Array(12).fill(0)): ClipDetection {
  return { clipId, chords, chroma };
}

describe("reading detections off the model", () => {
  const live = { id: "live", trackId: "t", sourceId: "s", startTick: 0, lengthSeconds: 1, offsetSeconds: 0, gain: 1, fadeInSeconds: 0, fadeOutSeconds: 0, muted: false } as AudioClip;
  const base = { audioClips: [live] } as unknown as ProjectModel;

  it("lists live candidates in song order and hides a deleted take's", () => {
    const m = {
      ...base,
      detections: [
        detection("live", [chord(4 * B, B, [7, 11, 2], "G"), chord(0, B, [0, 4, 7], "C")]),
        detection("gone", [chord(2 * B, B, [9, 0, 4], "Am")]),
      ],
    } as ProjectModel;
    expect(detectedChords(m).map((c) => [c.name, c.clipId])).toEqual([
      ["C", "live"],
      ["G", "live"],
    ]);
    expect(detectedChords({ ...base } as ProjectModel)).toEqual([]);
  });

  it("sums the chroma of live detections only", () => {
    const one = Array(12).fill(0);
    one[7] = 2;
    const m = { ...base, detections: [detection("live", [], one), detection("gone", [], Array(12).fill(5))] } as ProjectModel;
    expect(detectedChroma(m)[7]).toBe(2);
    expect(detectedChroma(m).reduce((a, b) => a + b, 0)).toBe(2);
  });

  it("says what an analysis wrote and what it refused", () => {
    const report = (notes: number, chords: number, refused: [number, number][]) => ({ notes, chords, refused, trackId: null }) as unknown as DetectionReport;
    expect(detectionSummary(report(5, 1, [[0, 4 * B]]), "take 3 notes")).toBe(
      "5 notes on take 3 notes · 1 strummed region (4.0 beats) refused as notes · 1 chord to review above the progression",
    );
    expect(detectionSummary(report(0, 0, [[0, B]]), null)).toBe("no notes · 1 strummed region (1.0 beats) refused as notes · no chord recognized in them");
    expect(confidenceText(0.876)).toBe("88%");
  });
});

describe("the key suggestion hears strummed chroma", () => {
  it("weighs chroma like placed notes, mirroring KeyDetector::detect_with_chroma", () => {
    // Three strums of G minor and two of D major, in note equivalents.
    const chroma = Array(12).fill(0);
    for (const [pc, w] of [[7, 3], [10, 3], [2, 5], [6, 2], [9, 2], [0, 1], [3, 1]]) chroma[pc] = w;
    const r = detectKey([], chroma);
    expect(r.candidates[0]).toMatchObject({ rootPitchClass: 7, scale: "minor" });
    expect(r.isConfident).toBe(true);
    expect(detectKey([], Array(12).fill(0)).candidates).toHaveLength(0);
    // Faint leakage is not a pitch class.
    const thin = Array(12).fill(0.3);
    thin[0] = 6;
    thin[7] = 6;
    expect(detectKey([], thin).isConfident).toBe(false);
    // And placed notes alone behave exactly as before.
    expect(detectKey([0, 2, 4, 5, 7, 9, 11, 0, 4, 7, 0, 7]).isConfident).toBe(true);
  });
});

// ─── Against the browser mock ───────────────────────────────────────────────────────────────

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

let S: typeof StoreModule;
let b: Bridge;
const st = () => S.useStore.getState();
const model = (): ProjectModel => st().snapshot!.model;

/** A starter song with one guitar take at bar 2; answers the take's clip id. */
async function withTake(): Promise<string> {
  await st().newFromTemplate("starter");
  await st().dispatch({ type: "addAudioTrack", name: "guitar" });
  const track = model().tracks[model().tracks.length - 1].id;
  const clip: AudioClip = { id: crypto.randomUUID(), trackId: track, sourceId: SOURCE.id, startTick: 4 * B, lengthSeconds: 2, offsetSeconds: 0, gain: 1, fadeInSeconds: 0, fadeOutSeconds: 0, muted: false };
  await st().dispatch({ type: "addAudioTake", trackId: track, source: SOURCE, clip });
  return clip.id;
}

function bent(): NoteEvent {
  return {
    id: crypto.randomUUID(),
    startTick: B,
    lengthTicks: 2 * B,
    pitch: 57,
    velocity: 96,
    pitchEnvelope: [
      { tick: 0, cents: 0, curve: "ramp" },
      { tick: B / 2, cents: 200, curve: "ramp" },
    ],
  };
}

beforeEach(async () => {
  localStorage.clear();
  // Dynamic on purpose: `getBridge` memoizes its mock in module scope, so only a re-import
  // after `vi.resetModules()` gives each test a project of its own.
  vi.resetModules();
  S = await import("./store");
  b = await (await import("./bridge")).getBridge();
});

describe("clip → notes", () => {
  it("lands a new synth track, its pattern and its clip as one undo step", async () => {
    const clipId = await withTake();
    // The desktop app does the listening; here the analysis' answer is fed through the mock's
    // own `addTranscription`, exactly as `detect_audio_clip` lands it in Rust.
    vi.spyOn(b, "detectAudioClip").mockImplementation(async (id) => {
      const snapshot = await b.apply(
        {
          type: "addTranscription",
          name: "take 3 notes",
          startTick: 4 * B,
          lengthTicks: 8 * B,
          notes: [{ id: crypto.randomUUID(), startTick: 0, lengthTicks: B, pitch: 52, velocity: 100 }, bent()],
          detection: detection(id, [chord(12 * B, 4 * B, [0, 4, 7], "C")]),
        },
        false,
        "detect notes",
      );
      return { snapshot, trackId: snapshot.model.tracks.at(-1)!.id, notes: 2, chords: 1, refused: [[12 * B, 16 * B]] };
    });
    const before = structuredClone(model());
    await st().detectNotes(clipId);
    expect(st().detectingClipId).toBeNull();
    const m = model();
    const track = m.tracks.at(-1)!;
    expect(track).toMatchObject({ name: "take 3 notes", isAudio: false, isDrum: false, voice: "saw" });
    const pattern = m.patterns.find((p) => p.trackId === track.id)!;
    expect(pattern).toMatchObject({ name: "take 3 notes 1", lengthTicks: 8 * B });
    expect(pattern.notes[1].pitchEnvelope).toHaveLength(2);
    expect(m.clips.find((c) => c.patternId === pattern.id)).toMatchObject({ startTick: 4 * B, lengthTicks: 8 * B });
    expect(detectedChords(m).map((c) => c.name)).toEqual(["C"]);
    expect(m.chords).toEqual(before.chords);
    expect(st().toast?.text).toContain("2 notes on take 3 notes");

    await st().undo();
    expect(model().tracks).toEqual(before.tracks);
    expect(model().patterns).toEqual(before.patterns);
    expect(model().clips).toEqual(before.clips);
    expect(model().detections).toBeUndefined();
  });

  it("says so when the desktop app is not there to listen", async () => {
    const clipId = await withTake();
    await st().detectNotes(clipId);
    expect(st().toast).toMatchObject({ error: true });
    expect(st().toast!.text).toContain("desktop app");
    expect(st().detectingClipId).toBeNull();
  });
});

describe("the detected lane", () => {
  async function waiting(): Promise<{ am: DetectedChord; f: DetectedChord }> {
    const clipId = await withTake();
    const am = chord(2 * B, 4 * B, [9, 0, 4], "Am");
    const f = chord(8 * B, 2 * B, [5, 9, 0], "F");
    await st().dispatch({ type: "addTranscription", name: "take 3 notes", startTick: 4 * B, lengthTicks: 8 * B, notes: [], detection: detection(clipId, [am, f]) });
    return { am, f };
  }

  it("accepts one chord onto the progression in one undo step, trimming what it lands on", async () => {
    const { am, f } = await waiting();
    const tracks = model().tracks.length;
    const before = structuredClone(model());
    await st().dispatch({ type: "acceptDetectedChord", id: am.id });
    const m = model();
    expect(m.tracks).toHaveLength(tracks);
    const landed = m.chords.find((c) => c.id === am.id)!;
    expect(landed).toMatchObject({ startTick: 2 * B, lengthTicks: 4 * B, pitchClasses: [9, 0, 4], name: "Am" });
    expect(m.chords.slice(0, 3).map((c) => [c.startTick, c.startTick + c.lengthTicks])).toEqual([
      [0, 2 * B],
      [2 * B, 6 * B],
      [6 * B, 8 * B],
    ]);
    expect(detectedChords(m).map((c) => c.id)).toEqual([f.id]);
    await st().undo();
    expect(model().chords).toEqual(before.chords);
    expect(model().detections).toEqual(before.detections);
  });

  it("dismisses without touching the progression, and refuses an unknown chord", async () => {
    const { am, f } = await waiting();
    const before = structuredClone(model());
    await st().dispatch({ type: "dismissDetectedChord", id: f.id });
    expect(model().chords).toEqual(before.chords);
    expect(detectedChords(model()).map((c) => c.id)).toEqual([am.id]);
    await st().dispatch({ type: "dismissDetectedChord", id: am.id });
    expect(model().detections).toBeUndefined();
    await st().undo();
    await st().undo();
    expect(model().detections).toEqual(before.detections);
    await expect(b.apply({ type: "acceptDetectedChord", id: crypto.randomUUID() })).rejects.toBe("no such detected chord");
  });

  it("moves candidates with the music when time is opened", async () => {
    const { am } = await waiting();
    await st().dispatch({ type: "insertTime", at: 0, ticks: 4 * B });
    expect(detectedChords(model()).find((c) => c.id === am.id)).toMatchObject({ startTick: 6 * B, lengthTicks: 4 * B });
  });

  it("refuses a transcription for a take that is not on the song", async () => {
    await withTake();
    await expect(
      b.apply({ type: "addTranscription", name: "x", startTick: 0, lengthTicks: B, notes: [], detection: detection(crypto.randomUUID(), []) }),
    ).rejects.toBe("no such audio clip");
  });
});
