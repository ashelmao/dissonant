import { describe, expect, it } from "vitest";
import {
  alignmentText,
  centsBetween,
  DEFAULT_TUNING,
  noteHz,
  noteName,
  TUNINGS,
  tunerAdvice,
  tunerDisplay,
  tuningById,
} from "./tuner";

describe("note arithmetic", () => {
  it("puts A4 where the reference says it is", () => {
    expect(noteHz(69, 440)).toBeCloseTo(440, 6);
    expect(noteHz(69, 415)).toBeCloseTo(415, 6);
    // An octave is a doubling, whatever the reference.
    expect(noteHz(81, 415)).toBeCloseTo(830, 6);
    // Standard low E.
    expect(noteHz(40, 440)).toBeCloseTo(82.407, 2);
  });

  it("measures cents with sharp positive", () => {
    expect(centsBetween(440, 440)).toBeCloseTo(0, 9);
    expect(centsBetween(880, 440)).toBeCloseTo(1200, 6);
    expect(centsBetween(440, 880)).toBeCloseTo(-1200, 6);
  });

  it("names notes with the octave a player would call them", () => {
    expect(noteName(69)).toBe("A4");
    expect(noteName(40)).toBe("E2");
    expect(noteName(60)).toBe("C4");
  });
});

describe("tunings", () => {
  it("offers the four the spec asks for, each with per-string targets", () => {
    for (const id of ["standard", "dropD", "dadgad", "openG"]) {
      const t = tuningById(id);
      expect(t.id).toBe(id);
      expect(t.strings.length).toBe(6);
      for (const s of t.strings) expect(s.name.length).toBeGreaterThan(0);
    }
  });

  it("falls back to standard for a tuning that is no longer there", () => {
    expect(tuningById("klingon").id).toBe("standard");
  });

  it("drops only the sixth string in drop D", () => {
    const standard = tuningById("standard").strings.map((s) => s.midi);
    const drop = tuningById("dropD").strings.map((s) => s.midi);
    expect(drop[0]).toBe(standard[0] - 2);
    expect(drop.slice(1)).toEqual(standard.slice(1));
  });

  it("spells DADGAD as its name does", () => {
    expect(tuningById("dadgad").strings.map((s) => noteName(s.midi))).toEqual(["D2", "A2", "D3", "G3", "A3", "D4"]);
  });

  it("keeps every tuning's strings in ascending order", () => {
    for (const t of TUNINGS) {
      const midis = t.strings.map((s) => s.midi);
      expect([...midis].sort((a, b) => a - b)).toEqual(midis);
    }
  });
});

describe("the readout", () => {
  it("names the string that is flat, not the note that is near", () => {
    // Low E, 20 cents flat.
    const hz = noteHz(40, 440) * Math.pow(2, -20 / 1200);
    const d = tunerDisplay(hz, DEFAULT_TUNING, 440)!;
    expect(d.label).toBe("low E");
    expect(d.note).toBe("E2");
    expect(d.cents).toBeCloseTo(-20, 1);
    expect(d.inTune).toBe(false);
    expect(tunerAdvice(d)).toBe("20 cents flat");
  });

  it("tells the two E strings apart by octave", () => {
    const low = tunerDisplay(noteHz(40, 440), DEFAULT_TUNING, 440)!;
    const high = tunerDisplay(noteHz(64, 440), DEFAULT_TUNING, 440)!;
    expect(low.stringIndex).toBe(0);
    expect(high.stringIndex).toBe(5);
    expect(low.label).toBe("low E");
    expect(high.label).toBe("high E");
  });

  it("measures against the reference, so a 440 Hz guitar is sharp at a 415 Hz A", () => {
    // Concert low E, read by a tuner set to the old pitch: a whole semitone sharp of where
    // that tuner wants the string, and still obviously the low E.
    const d = tunerDisplay(noteHz(40, 440), DEFAULT_TUNING, 415)!;
    expect(d.label).toBe("low E");
    expect(d.offTuning).toBe(false);
    expect(d.cents).toBeCloseTo(1200 * Math.log2(440 / 415), 3);
    expect(d.targetHz).toBeCloseTo(noteHz(40, 415), 6);
    // The same string at the same reference is, of course, in tune.
    expect(tunerDisplay(noteHz(40, 415), DEFAULT_TUNING, 415)!.inTune).toBe(true);
  });

  it("calls a string in tune only inside the window", () => {
    const target = noteHz(45, 440);
    expect(tunerDisplay(target * Math.pow(2, 2 / 1200), DEFAULT_TUNING, 440)!.inTune).toBe(true);
    expect(tunerDisplay(target * Math.pow(2, 5 / 1200), DEFAULT_TUNING, 440)!.inTune).toBe(false);
    expect(tunerAdvice(tunerDisplay(target, DEFAULT_TUNING, 440))).toBe("in tune");
  });

  it("reads the low string against the tuning that is chosen, not standard", () => {
    const d2 = noteHz(38, 440);
    const drop = tunerDisplay(d2, tuningById("dropD"), 440)!;
    expect(drop.label).toBe("low D");
    expect(drop.cents).toBeCloseTo(0, 6);
    expect(drop.inTune).toBe(true);
    // In standard tuning there is no string within a tone of it, so it is named rather than
    // blamed on the low E: at that distance the answer is "you are in drop D", not "flat".
    const standard = tunerDisplay(d2, tuningById("standard"), 440)!;
    expect(standard.offTuning).toBe(true);
    expect(standard.label).toBe("D2");
  });

  it("names the note when nothing in the tuning is anywhere near", () => {
    // C4 against standard tuning: 100 cents from B3, so still claimed by the B string…
    expect(tunerDisplay(noteHz(60, 440), DEFAULT_TUNING, 440)!.label).toBe("B");
    // …but F♯2 sits in the gap between the low E and the A, 200 cents from either.
    const f = tunerDisplay(noteHz(42, 440), DEFAULT_TUNING, 440)!;
    expect(f.offTuning).toBe(true);
    expect(f.label).toBe("F♯2");
    expect(f.stringIndex).toBe(-1);
  });

  it("has a resting state", () => {
    expect(tunerDisplay(null, DEFAULT_TUNING, 440)).toBeNull();
    expect(tunerDisplay(0, DEFAULT_TUNING, 440)).toBeNull();
    expect(tunerDisplay(Number.NaN, DEFAULT_TUNING, 440)).toBeNull();
    expect(tunerAdvice(null)).toBe("play a string");
  });
});

describe("the alignment readout", () => {
  it("says nothing before a take has been recorded", () => {
    expect(alignmentText(null)).toBeNull();
    expect(alignmentText(undefined)).toBeNull();
  });

  it("reports the whole correction as one number", () => {
    expect(alignmentText({ shiftedMs: 12.34, trimmedMs: 0 })).toBe("aligned −12.3 ms");
  });

  it("owns up to audio it threw away", () => {
    expect(alignmentText({ shiftedMs: 5, trimmedMs: 7.5 })).toBe("aligned −12.5 ms · 7.5 ms trimmed off the head");
  });

  it("does not claim a correction it did not make", () => {
    expect(alignmentText({ shiftedMs: 0, trimmedMs: 0 })).toBe("aligned · no correction needed");
  });
});
