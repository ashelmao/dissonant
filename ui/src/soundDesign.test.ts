import { describe, expect, it } from "vitest";
import { DEFAULT_DRUM_PATCH, DEFAULT_SYNTH_PATCH, FACTORY_SOUNDS, type SoundPatch } from "./sound";
import { adsrPoints, drumSketch, fmt, fromNorm, getPath, historyOf, mutate, record, redo, seal, seededRng, setPath, toNorm, undo, type NumParam, type ParamDef } from "./soundDesign";
import { DRUM_KINDS, DRUM_PARAMS, archetypeFor, defaultOf, isRelevant, mutatePatch, paramsFor, rollPatch, SYNTH_PARAMS } from "./components/sound/soundParams";

const synth: SoundPatch = { kind: "synth", ...DEFAULT_SYNTH_PATCH };
const drum: SoundPatch = { kind: "drum", ...DEFAULT_DRUM_PATCH };

function inRange(patch: SoundPatch, defs: ParamDef[]) {
  for (const d of defs) {
    const v = getPath(patch, d.path);
    if (d.type === "num") {
      expect(typeof v, d.id).toBe("number");
      expect(Number.isFinite(v as number), d.id).toBe(true);
      expect(v as number, d.id).toBeGreaterThanOrEqual(d.min);
      expect(v as number, d.id).toBeLessThanOrEqual(d.max);
      if (d.taper === "int") expect(Number.isInteger(v), d.id).toBe(true);
    } else if (d.type === "choice") {
      expect(d.options, d.id).toContain(v);
    } else {
      expect(typeof v, d.id).toBe("boolean");
    }
  }
}

const numericDiffs = (a: SoundPatch, b: SoundPatch, defs: ParamDef[]) => defs.filter((d) => d.type === "num" && getPath(a, d.path) !== getPath(b, d.path)).length;

describe("tapers", () => {
  const cases: Pick<NumParam, "min" | "max" | "taper">[] = [
    { min: 20, max: 20000, taper: "log" },
    { min: 0, max: 10, taper: "pow" },
    { min: -8, max: 8, taper: "lin" },
    { min: 0.001, max: 2, taper: "log" },
  ];
  it("fromNorm inverts toNorm", () => {
    for (const d of cases) {
      for (let i = 0; i <= 20; i++) {
        const p = i / 20;
        expect(toNorm(d, fromNorm(d, p))).toBeCloseTo(p, 9);
      }
    }
  });
  it("int tapers land on whole steps and round-trip", () => {
    const d = { min: -3, max: 3, taper: "int" as const };
    for (let v = -3; v <= 3; v++) expect(fromNorm(d, toNorm(d, v))).toBe(v);
    expect(Number.isInteger(fromNorm(d, 0.37))).toBe(true);
  });
  it("clamps out-of-range input", () => {
    const d = cases[0];
    expect(fromNorm(d, 2)).toBe(20000);
    expect(fromNorm(d, -1)).toBe(20);
    expect(toNorm(d, 5)).toBe(0);
  });
  it("log puts 632 Hz near the middle of 20 Hz–20 kHz", () => {
    expect(fromNorm(cases[0], 0.5)).toBeCloseTo(632.5, 0);
  });
  it("every param's default sits inside its range", () => {
    inRange(synth, SYNTH_PARAMS);
    inRange(drum, DRUM_PARAMS);
    for (const s of FACTORY_SOUNDS) inRange(s.patch, paramsFor(s.patch.kind));
  });
});

describe("readouts", () => {
  it("formats times, frequencies and signs readably", () => {
    expect(fmt.secs(0.0035)).toBe("3.5 ms");
    expect(fmt.secs(0.25)).toBe("250 ms");
    expect(fmt.secs(1.5)).toBe("1.50 s");
    expect(fmt.hz(440)).toBe("440 Hz");
    expect(fmt.hz(12000)).toBe("12.0kHz");
    expect(fmt.semis(7)).toBe("+7 st");
    expect(fmt.semis(-12)).toBe("−12 st");
    expect(fmt.db(1)).toBe("0.0 dB");
  });
});

describe("paths", () => {
  it("setPath copies along the path only", () => {
    const next = setPath(synth, ["osc", 1, "level"], 0.3);
    expect(getPath(next, ["osc", 1, "level"])).toBe(0.3);
    expect(getPath(synth, ["osc", 1, "level"])).toBe(0.8);
    expect((next as typeof synth & { osc: unknown[] }).osc[0]).toBe((synth as typeof synth & { osc: unknown[] }).osc[0]);
    expect(Array.isArray(getPath(next, ["osc"]))).toBe(true);
  });
  it("defaults come from the default patches", () => {
    expect(defaultOf("synth", SYNTH_PARAMS.find((d) => d.id === "filter.cutoff")!)).toBe(20000);
    expect(defaultOf("drum", DRUM_PARAMS.find((d) => d.id === "body.startHz")!)).toBe(160);
  });
});

describe("randomize", () => {
  it("synth rolls are always in range and keep osc 1 on", () => {
    for (let seed = 1; seed <= 300; seed++) {
      const p = rollPatch(synth, seededRng(seed));
      expect(p.kind).toBe("synth");
      inRange(p, SYNTH_PARAMS);
      if (p.kind === "synth") {
        expect(p.osc[0].on).toBe(true);
        expect(p.level).toBe(synth.level);
        for (const l of p.lfo) if (l.target === "pitch") expect(l.depth).toBeLessThanOrEqual(0.05);
      }
    }
  });
  it("drum rolls are always in range and audible, for every kind", () => {
    for (const kind of [...DRUM_KINDS, null]) {
      for (let seed = 1; seed <= 80; seed++) {
        const p = rollPatch(drum, seededRng(seed), kind);
        inRange(p, DRUM_PARAMS);
        if (p.kind === "drum") expect(Math.max(p.body.level, p.noise.level, p.metal.level, p.click.level)).toBeGreaterThanOrEqual(0.1);
      }
    }
  });
  it("is not uniform noise: rolls differ but stay constrained", () => {
    const a = rollPatch(synth, seededRng(1));
    const b = rollPatch(synth, seededRng(2));
    expect(JSON.stringify(a)).not.toBe(JSON.stringify(b));
    // The constrained cutoff range never rolls the extremes of the knob.
    for (let seed = 1; seed <= 100; seed++) {
      const p = rollPatch(synth, seededRng(seed));
      if (p.kind === "synth") expect(p.filter.cutoff).toBeLessThanOrEqual(9000.001);
    }
  });
  it("kick rolls sweep down and use no metal", () => {
    for (let seed = 1; seed <= 50; seed++) {
      const p = rollPatch(drum, seededRng(seed), "kick");
      if (p.kind !== "drum") throw new Error("kind");
      expect(p.body.startHz).toBeGreaterThan(p.body.endHz);
      expect(p.metal.level).toBe(0);
    }
  });
  it("is repeatable with the same seed", () => {
    expect(rollPatch(synth, seededRng(7))).toEqual(rollPatch(synth, seededRng(7)));
  });
  it("maps categories to drum kinds", () => {
    expect(archetypeFor("kick")).toBe("kick");
    expect(archetypeFor("Open Hat")).toBe("hat");
    expect(archetypeFor("crash")).toBe("cymbal");
    expect(archetypeFor("texture")).toBeNull();
  });
});

describe("mutate", () => {
  it("changes some but not all params, and stays in range", () => {
    for (const base of [synth, drum, ...FACTORY_SOUNDS.map((s) => s.patch)]) {
      const defs = paramsFor(base.kind);
      const audible = defs.filter((d) => d.type === "num" && !d.noMutate && isRelevant(base, d)).length;
      for (let seed = 1; seed <= 40; seed++) {
        const m = mutatePatch(base, seededRng(seed));
        inRange(m, defs);
        const changed = numericDiffs(base, m, defs);
        expect(changed).toBeGreaterThan(0);
        if (audible > 1) expect(changed).toBeLessThan(audible);
      }
    }
  });
  it("nudges a little, not a re-roll", () => {
    const m = mutatePatch(synth, seededRng(3));
    for (const d of SYNTH_PARAMS) {
      if (d.type !== "num" || d.taper === "int") continue;
      const before = toNorm(d, getPath(synth, d.path) as number);
      const after = toNorm(d, getPath(m, d.path) as number);
      expect(Math.abs(after - before)).toBeLessThanOrEqual(0.081);
    }
  });
  it("never touches output level or velocity", () => {
    for (let seed = 1; seed <= 40; seed++) {
      const m = mutate(synth, SYNTH_PARAMS, seededRng(seed), undefined, 1);
      if (m.kind === "synth") {
        expect(m.level).toBe(synth.level);
        expect(m.velocity).toBe(synth.velocity);
      }
    }
  });
});

describe("draft history", () => {
  it("records, undoes and redoes", () => {
    let h = historyOf(1);
    h = record(h, 2);
    h = record(h, 3);
    expect(h.present).toBe(3);
    h = undo(h);
    expect(h.present).toBe(2);
    h = undo(h);
    h = undo(h); // nothing left: no-op
    expect(h.present).toBe(1);
    h = redo(h);
    expect(h.present).toBe(2);
    h = record(h, 9);
    expect(h.future).toEqual([]);
    expect(redo(h).present).toBe(9);
  });
  it("coalesces a gesture under one key until sealed", () => {
    let h = historyOf(0);
    for (let v = 1; v <= 50; v++) h = record(h, v, "cutoff");
    expect(h.past).toEqual([0]);
    h = seal(h);
    h = record(h, 51, "cutoff");
    expect(h.past).toEqual([0, 50]);
    expect(undo(undo(h)).present).toBe(0);
  });
});

describe("drawn shapes", () => {
  it("ADSR points rise to 1, settle at sustain and end at 0", () => {
    const pts = adsrPoints({ attack: 0.1, decay: 0.2, sustain: 0.5, release: 0.3 }, 0.25);
    expect(pts[0]).toEqual([0, 0]);
    expect(Math.max(...pts.map((p) => p[1]))).toBeCloseTo(1, 5);
    const end = pts[pts.length - 1];
    expect(end[0]).toBeCloseTo(0.85, 9);
    expect(end[1]).toBe(0);
    expect(pts.some(([t, v]) => t > 0.3 && t < 0.55 && Math.abs(v - 0.5) < 0.02)).toBe(true);
  });
  it("the drum sketch decays and its pitch sweeps from start to end", () => {
    const s = drumSketch(DEFAULT_DRUM_PATCH, 100);
    expect(s.amp[0]).toBeGreaterThan(0.9);
    expect(s.amp[99]).toBeLessThan(0.05);
    expect(s.pitch[0]).toBeCloseTo(160, 0);
    expect(s.pitch[99]).toBeLessThan(60);
  });
});
