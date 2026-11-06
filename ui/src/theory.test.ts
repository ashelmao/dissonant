import { describe, expect, it } from "vitest";
import { TICKS_PER_BEAT, tempoMap } from "./types";

describe("tempo map", () => {
  it("steps, ramps and integrates seconds like the Rust map", () => {
    const flat = tempoMap(120, []);
    expect(flat.isConstant).toBe(true);
    expect(flat.secondsAt(4 * TICKS_PER_BEAT)).toBeCloseTo(2, 9);
    const step = tempoMap(120, [{ id: "a", tick: 4 * TICKS_PER_BEAT, bpm: 60, ramp: false }]);
    expect(step.bpmAt(3.9 * TICKS_PER_BEAT)).toBe(120);
    expect(step.bpmAt(4 * TICKS_PER_BEAT)).toBe(60);
    expect(step.secondsAt(8 * TICKS_PER_BEAT)).toBeCloseTo(6, 9);
    const ramp = tempoMap(120, [{ id: "a", tick: 4 * TICKS_PER_BEAT, bpm: 60, ramp: true }]);
    expect(ramp.bpmAt(2 * TICKS_PER_BEAT)).toBeCloseTo(90, 9);
    expect(ramp.secondsAt(4 * TICKS_PER_BEAT)).toBeCloseTo(4 * Math.LN2, 9);
    expect(ramp.secondsAt(6 * TICKS_PER_BEAT)).toBeCloseTo(4 * Math.LN2 + 2, 9);
  });
});
import { chordAt, chordName, detectKey, diatonicChords, midiName, progression, tier, tierMap } from "./theory";
import { NO_KEY } from "./types";

const C_MAJOR = [0, 2, 4, 5, 7, 9, 11];

describe("tiers (must agree with dissonant-core)", () => {
  it("classifies against a C major triad", () => {
    for (const pc of [0, 4, 7]) expect(tier(pc, [0, 4, 7], null)).toBe("chordTone");
    expect(tier(2, [0, 4, 7], null)).toBe("tension");
    expect(tier(5, [0, 4, 7], null)).toBe("dissonance");
    expect(tier(10, [0, 4, 7], null)).toBe("tension");
    expect(tier(10, [0, 4, 7], C_MAJOR)).toBe("dissonance");
    expect(tier(13, [0, 4, 7], null)).toBe(tier(1, [0, 4, 7], null));
  });

  it("degrades through three states", () => {
    const chords = progression([0, 3], 0, "major", 8 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT);
    expect(tierMap(TICKS_PER_BEAT, chords, NO_KEY).every((t) => t !== null)).toBe(true);
    expect(tierMap(TICKS_PER_BEAT, [], NO_KEY).every((t) => t === null)).toBe(true);
    const scaleOnly = tierMap(TICKS_PER_BEAT, [], { rootPitchClass: 0, scale: "major", isLocked: true });
    expect(scaleOnly[0]).toBe("chordTone");
    expect(scaleOnly[1]).toBe("dissonance");
    expect(chordAt(chords, 4 * TICKS_PER_BEAT)?.name).toBe("F");
    expect(chordAt(chords, 8 * TICKS_PER_BEAT)).toBeNull();
  });
});

describe("harmony", () => {
  it("names diatonic chords and progressions in the key", () => {
    expect(diatonicChords(0, "major").map((c) => c.name)).toEqual(["C", "Dm", "Em", "F", "G", "Am", "Bdim"]);
    expect(diatonicChords(9, "minor").map((c) => c.romanNumeral)).toEqual(["i", "ii°", "III", "iv", "v", "VI", "VII"]);
    expect(progression([0, 3, 4, 5], 7, "major", 16 * TICKS_PER_BEAT).map((c) => c.name)).toEqual(["G", "C", "D", "Em"]);
    expect(progression([0, 3, 4, 5], 0, "major", 32 * TICKS_PER_BEAT)).toHaveLength(8);
  });

  it("names arbitrary chords", () => {
    expect(chordName([4, 7, 0])).toBe("C");
    expect(chordName([9, 0, 4])).toBe("Am");
    expect(chordName([0, 1, 2])).toBe("C·C#·D");
    expect(chordName([])).toBe("—");
    expect(midiName(60)).toBe("C4");
  });
});

describe("key detection", () => {
  it("is confident on a full scale, not on three notes", () => {
    const r = detectKey([0, 2, 4, 5, 7, 9, 11, 0, 4, 7, 0, 7]);
    expect(r.candidates[0]).toMatchObject({ rootPitchClass: 0, scale: "major" });
    expect(r.isConfident).toBe(true);
    expect(detectKey([0, 4, 7]).isConfident).toBe(false);
    expect(detectKey([]).candidates).toHaveLength(0);
  });
});

// ─── Rust ↔ TypeScript parity ──────────────────────────────────────────────────────────────
// These tables are duplicated verbatim from `crates/dissonant-core/tests/theory_behavior.rs`
// (module `parity`). The UI reimplements the theory so the roll can repaint at 60 fps without a
// round trip; if one side is edited without the other, one of the two suites fails — which is
// exactly the drift that makes the colors on screen disagree with the harmony the engine plays.

import { normalize, scalePitchClasses, STARTERS, startersFor } from "./theory";
import type { ScaleType, Tier as TierT } from "./types";

type ParityCase = [label: string, chord: number[], key: [number, ScaleType] | null, pitch: number, expected: TierT];

const TIER_CASES: ParityCase[] = [
  ["C/Cmaj root", [0,4,7], [0, "major"], 0, "chordTone"],
  ["C/Cmaj 9", [0,4,7], [0, "major"], 2, "tension"],
  ["C/Cmaj avoid 11", [0,4,7], [0, "major"], 5, "dissonance"],
  ["C/Cmaj 13", [0,4,7], [0, "major"], 9, "tension"],
  ["C/Cmaj maj7", [0,4,7], [0, "major"], 11, "tension"],
  ["C/Cmaj b9 out", [0,4,7], [0, "major"], 1, "dissonance"],
  ["C/Cmaj #11 out", [0,4,7], [0, "major"], 6, "dissonance"],
  ["C/Cmaj b13 out", [0,4,7], [0, "major"], 8, "dissonance"],
  ["C/nokey b7", [0,4,7], null, 10, "tension"],
  ["C/nokey maj7", [0,4,7], null, 11, "tension"],
  ["C/nokey #9", [0,4,7], null, 3, "tension"],
  ["C/nokey avoid 11", [0,4,7], null, 5, "dissonance"],
  ["C octave-up pitch", [0,4,7], null, 64, "chordTone"],
  ["C negative pitch", [0,4,7], null, -1, "tension"],
  ["C octave chord", [12,16,19], null, 11, "tension"],
  ["Am/Cmaj 9", [9,0,4], [0, "major"], 11, "tension"],
  ["Am/Cmaj 11", [9,0,4], [0, "major"], 2, "tension"],
  ["Am/Cmaj b7", [9,0,4], [0, "major"], 7, "tension"],
  ["Am/Cmaj avoid b13", [9,0,4], [0, "major"], 5, "dissonance"],
  ["Am/Cmaj third", [9,0,4], [0, "major"], 0, "chordTone"],
  ["G7/Cmaj 13", [7,11,2,5], [0, "major"], 4, "tension"],
  ["G7/Cmaj 9", [7,11,2,5], [0, "major"], 9, "tension"],
  ["G7/Cmaj avoid 11", [7,11,2,5], [0, "major"], 0, "dissonance"],
  ["G7/Cmaj seventh", [7,11,2,5], [0, "major"], 5, "chordTone"],
  ["G7/Cmaj b9 out", [7,11,2,5], [0, "major"], 8, "dissonance"],
  ["Cmaj7/Cmaj 7th", [0,4,7,11], [0, "major"], 11, "chordTone"],
  ["Cmaj7/Cmaj 9", [0,4,7,11], [0, "major"], 2, "tension"],
  ["Cmaj7/Cmaj avoid 11", [0,4,7,11], [0, "major"], 5, "dissonance"],
  ["Dm7/Cmaj 13", [2,5,9,0], [0, "major"], 11, "tension"],
  ["Dm7/Cmaj 11", [2,5,9,0], [0, "major"], 7, "tension"],
  ["Dm7/Cmaj 9", [2,5,9,0], [0, "major"], 4, "tension"],
  ["Bdim/Cmaj avoid b9", [11,2,5], [0, "major"], 0, "dissonance"],
  ["Bdim/Cmaj b13", [11,2,5], [0, "major"], 7, "tension"],
  ["Csus4/Cmaj third", [0,5,7], [0, "major"], 4, "tension"],
  ["Csus4/Cmaj maj7", [0,5,7], [0, "major"], 11, "tension"],
  ["Caug/nokey #5", [0,4,8], null, 8, "chordTone"],
  ["Caug/nokey avoid 13", [0,4,8], null, 9, "dissonance"],
  ["Em/Amin avoid b9", [4,7,11], [9, "minor"], 5, "dissonance"],
  ["Em/Amin avoid b13", [4,7,11], [9, "minor"], 0, "dissonance"],
  ["Em/Amin 11", [4,7,11], [9, "minor"], 9, "tension"],
  ["D/Gmaj avoid 11", [2,6,9], [7, "major"], 7, "dissonance"],
  ["D/Gmaj 13", [2,6,9], [7, "major"], 11, "tension"],
];

describe("parity with dissonant-core", () => {
  it("classifies all 42 shared tier cases identically", () => {
    expect(TIER_CASES).toHaveLength(42);
    for (const [label, chord, key, pitch, expected] of TIER_CASES) {
      const scale = key === null ? null : scalePitchClasses(key[0], key[1]);
      expect(`${label}=${tier(pitch, chord, scale)}`).toBe(`${label}=${expected}`);
    }
  });

  it("agrees on normalize, STARTERS and progression", () => {
    expect([-25, -13, -12, -1, 0, 11, 12, 13, 64, 127].map(normalize)).toEqual([11, 11, 0, 11, 0, 11, 0, 1, 4, 7]);

    expect(STARTERS.map((s) => [s.name, s.mood, s.scale, s.degrees])).toEqual([
      ["I–IV–V–vi", "bright, lifting", "major", [0, 3, 4, 5]],
      ["I–V–vi–IV", "big and hopeful", "major", [0, 4, 5, 3]],
      ["vi–IV–I–V", "sad but hopeful", "major", [5, 3, 0, 4]],
      ["I–vi–IV–V", "old-school, sweet", "major", [0, 5, 3, 4]],
      ["ii–V–I", "jazzy, resolved", "major", [1, 4, 0, 0]],
      ["I–IV", "open two-chord vamp", "major", [0, 3]],
      ["i–VI–III–VII", "dark, driving", "minor", [0, 5, 2, 6]],
      ["i–VII–VI–VII", "brooding loop", "minor", [0, 6, 5, 6]],
      ["i–v–VI–iv", "sad, cinematic", "minor", [0, 4, 5, 3]],
      ["i–iv", "moody two-chord vamp", "minor", [0, 3]],
      ["i–VI", "hazy, floating", "minor", [0, 5]],
      ["i", "one-chord drone", "minor", [0]],
    ]);
    expect(startersFor("minor")[0].name).toBe("i–VI–III–VII");
    expect(startersFor("major")).toHaveLength(6);

    const described = (degrees: number[], root: number, scale: ScaleType, totalTicks: number) =>
      progression(degrees, root, scale, totalTicks, 4 * TICKS_PER_BEAT).map((c) => `${c.startTick}:${c.lengthTicks}:[${c.pitchClasses.join(", ")}]:${c.name ?? ""}`);
    expect(described([0, 3, 4, 5], 0, "major", 16 * TICKS_PER_BEAT)).toEqual(["0:3840:[0, 4, 7]:C", "3840:3840:[5, 9, 0]:F", "7680:3840:[7, 11, 2]:G", "11520:3840:[9, 0, 4]:Am"]);
    expect(described([0, 5, 2, 6], 9, "minor", 16 * TICKS_PER_BEAT)).toEqual(["0:3840:[9, 0, 4]:Am", "3840:3840:[5, 9, 0]:F", "7680:3840:[0, 4, 7]:C", "11520:3840:[7, 11, 2]:G"]);
    expect(described([1, 4, 0, 0], 2, "major", 16 * TICKS_PER_BEAT)).toEqual(["0:3840:[4, 7, 11]:Em", "3840:3840:[9, 1, 4]:A", "7680:3840:[2, 6, 9]:D", "11520:3840:[2, 6, 9]:D"]);
    // A pattern too short for one chord per degree divides the length instead of overflowing,
    // matching harmony::progression — a 1-bar pattern gets four 1-beat chords that fit.
    expect(described([0, 3, 4, 5], 0, "major", 4 * TICKS_PER_BEAT)).toEqual(["0:960:[0, 4, 7]:C", "960:960:[5, 9, 0]:F", "1920:960:[7, 11, 2]:G", "2880:960:[9, 0, 4]:Am"]);
    // A ragged length is still covered to the end: the last chord absorbs the remainder.
    expect(described([0, 3], 0, "major", 18 * TICKS_PER_BEAT).at(-1)).toBe("11520:5760:[5, 9, 0]:F");
    expect(progression([0], 0, "major", -8 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT)).toEqual([]);
  });

  it("resolves overlapping chords to the later-starting one whatever the array order", () => {
    // dissonant-core's ChordTrack keeps its chords sorted and documents "the later-starting one
    // wins"; this array arrives in the opposite order, and must still agree.
    const chords = [
      { id: "f", startTick: 4 * TICKS_PER_BEAT, lengthTicks: 4 * TICKS_PER_BEAT, pitchClasses: [5, 9, 0], name: "F" },
      { id: "c", startTick: 0, lengthTicks: 8 * TICKS_PER_BEAT, pitchClasses: [0, 4, 7], name: "C" },
    ];
    expect(chordAt(chords, TICKS_PER_BEAT)?.name).toBe("C");
    expect(chordAt(chords, 5 * TICKS_PER_BEAT)?.name).toBe("F");
    expect(tierMap(5 * TICKS_PER_BEAT, chords, NO_KEY)[5]).toBe("chordTone"); // F over the F chord
    expect(tierMap(TICKS_PER_BEAT, chords, NO_KEY)[5]).toBe("dissonance"); // F over the C chord: avoid note
    expect(chordAt(chords, 8 * TICKS_PER_BEAT)).toBeNull();
    expect(chordAt(chords, -TICKS_PER_BEAT)).toBeNull();
  });

  it("gates key confidence on distinct pitch classes, not note count", () => {
    // Twelve notes, but only two or three distinct pitch classes: harmonically that is a riff,
    // not a key, and it fits half a dozen keys equally well.
    expect(detectKey([0, 4, 0, 4, 0, 4, 0, 4, 0, 4, 0, 4]).isConfident).toBe(false);
    expect(detectKey([0, 4, 7, 0, 4, 7, 0, 4, 7, 0, 4, 7]).isConfident).toBe(false);
    // A Cmaj7 arpeggio correlates best with E minor — a confident wrong answer if it passed.
    const arp = detectKey([0, 4, 7, 11, 0, 4, 7, 11, 0, 4, 7, 11]);
    expect(arp.candidates[0]).toMatchObject({ rootPitchClass: 4, scale: "minor" });
    expect(arp.isConfident).toBe(false);
    // Real melodic material still locks on.
    expect(detectKey([60, 62, 64, 65, 67, 69, 71, 72, 67, 64, 60, 55]).isConfident).toBe(true);
  });
});

import { explainNote, moves, noteTier } from "./theory";
import type { ChordEvent, KeyState, PitchPoint } from "./types";

/** Mirrors `parity::MOVING_CASES` in crates/dissonant-core/tests/theory_behavior.rs verbatim. */
const MOVING_CASES: [label: string, points: [tick: number, cents: number][], expected: boolean][] = [
  ["empty", [], false],
  ["held detune at the start", [[0, 30]], false],
  ["held detune, two points", [[0, 30], [240, 30]], false],
  ["all zero", [[120, 0], [240, 0]], false],
  ["lone late point bends up to it", [[120, 150]], true],
  ["start detune then fall", [[0, 30], [240, -20]], true],
  ["returns to zero", [[0, 0], [120, 50], [240, 0]], true],
];

describe("moving notes (must agree with dissonant-core)", () => {
  const C_KEY: KeyState = { rootPitchClass: 0, scale: "major", isLocked: true };
  const twoBars: ChordEvent[] = progression([0, 3], 0, "major", 8 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT);
  const ramp = (tick: number, cents: number): PitchPoint => ({ tick, cents, curve: "ramp" });

  it("marks exactly the envelopes the Rust table marks", () => {
    expect(moves(undefined)).toBe(false);
    for (const [label, points, expected] of MOVING_CASES) {
      const note = { pitch: 60, startTick: 0, pitchEnvelope: points.map(([t, c]) => ramp(t, c)) };
      expect(`${label}=${noteTier(note, [], NO_KEY).moving}`).toBe(`${label}=${expected}`);
    }
  });

  it("tiers a bent note by its start pitch, exactly as the same note unbent", () => {
    // F over C bending a whole step up into G: still the avoid note where it starts.
    const plain = { pitch: 65, startTick: 0 };
    const bent = { ...plain, pitchEnvelope: [ramp(TICKS_PER_BEAT, 200)] };
    expect(noteTier(plain, twoBars, C_KEY)).toEqual({ tier: "dissonance", moving: false });
    expect(noteTier(bent, twoBars, C_KEY)).toEqual({ tier: "dissonance", moving: true });
    for (let pitch = 48; pitch < 72; pitch++) {
      const contexts: [ChordEvent[], KeyState][] = [[twoBars, C_KEY], [[], C_KEY], [[], NO_KEY]];
      for (const [chords, key] of contexts) {
        for (const startTick of [0, 3 * TICKS_PER_BEAT, 5 * TICKS_PER_BEAT]) {
          const a = noteTier({ pitch, startTick }, chords, key);
          const b = noteTier({ pitch, startTick, pitchEnvelope: [{ tick: 0, cents: -100, curve: "step" }, ramp(TICKS_PER_BEAT / 2, 300)] }, chords, key);
          expect(b.tier).toBe(a.tier);
          expect([a.moving, b.moving]).toEqual([false, true]);
        }
      }
    }
  });

  it("says a bent note bends, and that it is judged where it starts", () => {
    expect(explainNote(65, twoBars, C_KEY, 0, true)).toBe("F over C: a half step above E. It wants to fall to E — or stay, on purpose. It bends — judged where it starts.");
    expect(explainNote(64, [], C_KEY, 0, true)).toBe("E is in C maj. It bends — judged where it starts.");
    expect(explainNote(64, [], NO_KEY, 0, true)).toBe("no chord or key yet — anything goes. It bends.");
    expect(explainNote(64, twoBars, C_KEY, 0)).not.toContain("bends");
  });
});
