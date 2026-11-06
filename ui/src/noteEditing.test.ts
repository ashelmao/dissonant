import { describe, expect, it } from "vitest";
import {
  addPitchPoint,
  adjustVelocity,
  duplicateNotes,
  hitsInSteps,
  moveHitsByRow,
  movePitchPoint,
  moveNotes,
  noteAt,
  notesInRect,
  pasteNotes,
  pitchCentsAt,
  pitchPointAt,
  pitchPolyline,
  placeNote,
  quantizeNotes,
  noteValueLabel,
  resizeEdge,
  removePitchPoint,
  resizeNotes,
  snapFloor,
  snapRound,
  tidyPitchEnvelope,
  tileGhost,
  togglePitchCurve,
} from "./noteEditing";
import { TICKS_PER_BEAT, type NoteEvent, type PitchPoint } from "./types";

const n = (id: string, start: number, len: number, pitch: number, velocity = 100): NoteEvent => ({
  id,
  startTick: start,
  lengthTicks: len,
  pitch,
  velocity,
});

describe("placement", () => {
  it("snaps to the grid and refuses overlaps on the same pitch", () => {
    const notes = [n("a", 0, 960, 60)];
    expect(placeNote(notes, 672, 60, 960, 480)).toBeNull();
    const placed = placeNote(notes, 1248, 60, 480, 480);
    expect(placed?.startTick).toBe(960);
    expect(placed?.lengthTicks).toBe(480);
    expect(placeNote(notes, 192, 62, 960, 960)?.pitch).toBe(62);
  });

  it("snapFloor is stable at grid boundaries", () => {
    expect(snapFloor(959.9999999999, 240)).toBe(960);
    expect(snapFloor(1190, 240)).toBe(960);
  });

  it("snaps exactly on a triplet grid, where beat-fraction floats drifted", () => {
    const g = TICKS_PER_BEAT / 3; // 320: eighth-note triplets, the grid 1/3 of a beat never divided cleanly
    for (let i = 0; i < 24; i++) {
      const onGrid = i * g;
      expect(snapFloor(onGrid, g)).toBe(onGrid); // never falls back into the previous cell
      expect(snapRound(onGrid, g)).toBe(onGrid);
      expect(snapFloor(onGrid, g) % g).toBe(0); // exact multiples, no half-tick residue
      expect(snapFloor(onGrid + 1, g)).toBe(onGrid); // just inside the cell stays in it
      expect(snapFloor(onGrid + g - 1, g)).toBe(onGrid);
      expect(snapRound(onGrid + 159, g)).toBe(onGrid); // under half a cell rounds back
      expect(snapRound(onGrid + 161, g)).toBe(onGrid + g); // over half a cell rounds on
    }
    expect(snapFloor(4 * TICKS_PER_BEAT, g)).toBe(3840); // 12 triplets to the 4/4 bar, no drift
    expect(snapRound(4 * TICKS_PER_BEAT - 1, g)).toBe(3840);
  });

  it("finds the note under a point, preferring the latest", () => {
    const notes = [n("a", 0, 1920, 60), n("b", 960, 960, 60)];
    expect(noteAt(notes, 1440, 60)?.id).toBe("b");
    expect(noteAt(notes, 480, 60)?.id).toBe("a");
    expect(noteAt(notes, 480, 61)).toBeNull();
  });
});

describe("move", () => {
  const notes = [n("a", 960, 960, 60), n("b", 2880, 960, 64), n("c", 7680, 960, 50)];

  it("moves the whole selection together", () => {
    const out = moveNotes(notes, new Set(["a", "b"]), 1920, -1, { min: 0, max: 127 });
    expect(out.find((x) => x.id === "a")).toMatchObject({ startTick: 2880, pitch: 59 });
    expect(out.find((x) => x.id === "b")).toMatchObject({ startTick: 4800, pitch: 63 });
    expect(out.find((x) => x.id === "c")).toMatchObject({ startTick: 7680, pitch: 50 });
  });

  it("clamps so nothing goes before tick 0 or outside the pitch range", () => {
    const out = moveNotes(notes, new Set(["a", "b"]), -4800, 70, { min: 24, max: 84 });
    expect(out.find((x) => x.id === "a")!.startTick).toBe(0);
    expect(out.find((x) => x.id === "b")!.startTick).toBe(1920);
    expect(out.find((x) => x.id === "b")!.pitch).toBe(84);
    expect(out.find((x) => x.id === "a")!.pitch).toBe(80);
  });

  it("respects a maximum end", () => {
    const out = moveNotes(notes, new Set(["c"]), 9600, 0, { min: 0, max: 127 }, 15360);
    expect(out.find((x) => x.id === "c")!.startTick).toBe(14400);
  });

  it("returns the same array when nothing changes", () => {
    expect(moveNotes(notes, new Set(["zzz"]), 960, 1, { min: 0, max: 127 })).toBe(notes);
  });
});

describe("resize / select / velocity", () => {
  const notes = [n("a", 0, 960, 60), n("b", 1920, 1920, 62)];

  it("resizes with a minimum length", () => {
    const out = resizeNotes(notes, new Set(["a", "b"]), -4800, 240);
    expect(out.every((x) => x.lengthTicks === 240)).toBe(true);
  });

  it("drags the end edge by one delta for every note, never below the minimum", () => {
    const out = resizeEdge(notes, new Set(["a", "b"]), "end", 480, 240);
    expect(out.map((x) => [x.startTick, x.lengthTicks])).toEqual([[0, 1440], [1920, 2400]]);
    const shrunk = resizeEdge(notes, new Set(["a", "b"]), "end", -4800, 240);
    expect(shrunk.every((x) => x.lengthTicks === 240)).toBe(true);
  });

  it("trims the start edge and keeps the end where it was", () => {
    const out = resizeEdge(notes, new Set(["b"]), "start", 480, 240);
    expect(out[1]).toMatchObject({ startTick: 2400, lengthTicks: 1440 });
    // Pulled left past 0, the start stops there; pushed right, it stops a minimum short of the end.
    expect(resizeEdge(notes, new Set(["b"]), "start", -9999, 240)[1]).toMatchObject({ startTick: 0, lengthTicks: 3840 });
    expect(resizeEdge(notes, new Set(["b"]), "start", 9999, 240)[1]).toMatchObject({ startTick: 3600, lengthTicks: 240 });
  });

  it("never lengthens a note that was already shorter than the minimum", () => {
    const short = [n("s", 960, 120, 60)];
    expect(resizeEdge(short, new Set(["s"]), "end", -480, 240)[0].lengthTicks).toBe(120);
    expect(resizeEdge(short, new Set(["s"]), "start", 480, 240)[0]).toMatchObject({ startTick: 960, lengthTicks: 120 });
  });

  it("names lengths in note values", () => {
    expect(noteValueLabel(480)).toBe("1/8");
    expect(noteValueLabel(1440)).toBe("1/4.");
    expect(noteValueLabel(320)).toBe("1/8T");
    expect(noteValueLabel(1200)).toBe("1/4 + 1/16");
    expect(noteValueLabel(3600)).toBe("1/2. + 1/8.");
    expect(noteValueLabel(3840 + 1440)).toBe("1/1 + 1/4.");
    expect(noteValueLabel(2 * 3840)).toBe("2×1/1");
    expect(noteValueLabel(960 + 15)).toBe("1/4 + 15t");
  });

  it("marquee selects overlapping notes in any drag direction", () => {
    expect([...notesInRect(notes, 2880, 480, 63, 60)].sort()).toEqual(["a", "b"]);
    expect([...notesInRect(notes, 960, 1824, 60, 62)]).toEqual([]);
  });

  it("clamps velocity", () => {
    const out = adjustVelocity(notes, new Set(["a"]), 200);
    expect(out[0].velocity).toBe(127);
    expect(adjustVelocity(out, new Set(["a"]), -500)[0].velocity).toBe(1);
  });
});

describe("duplicate / paste", () => {
  it("duplicates right after the selection on the grid with fresh ids", () => {
    const notes = [n("a", 0, 960, 60), n("b", 960, 480, 62)];
    const { notes: out, newIds } = duplicateNotes(notes, new Set(["a", "b"]), 480);
    expect(out).toHaveLength(4);
    expect(newIds.size).toBe(2);
    const copies = out.filter((x) => newIds.has(x.id)).sort((p, q) => p.startTick - q.startTick);
    expect(copies[0]).toMatchObject({ startTick: 1440, pitch: 60 });
    expect(copies[1]).toMatchObject({ startTick: 2400, pitch: 62 });
  });

  it("pastes relative to the earliest clipboard note", () => {
    const clip = [n("x", 3840, 960, 60), n("y", 4800, 960, 64)];
    const { notes: out } = pasteNotes([], clip, 7680);
    expect(out.map((x) => x.startTick).sort()).toEqual([7680, 8640]);
    expect(out.every((x) => x.id !== "x" && x.id !== "y")).toBe(true);
  });
});

describe("tileGhost", () => {
  it("loops a shorter ghost across the pattern and cuts at the end", () => {
    const B = TICKS_PER_BEAT;
    const ghost = [n("a", 0, B, 60), n("b", 3 * B, 2 * B, 62)];
    const out = tileGhost(ghost, 4 * B, 10 * B);
    expect(out.map((x) => [x.startTick / B, x.lengthTicks / B])).toEqual([
      [0, 1],
      [3, 2],
      [4, 1],
      [7, 2],
      [8, 1],
    ]);
    expect(new Set(out.map((x) => x.id)).size).toBe(out.length);
  });
  it("leaves muted notes and notes past the ghost's own end out", () => {
    const B = TICKS_PER_BEAT;
    const ghost = [n("a", 0, B, 60), { ...n("m", B, B, 61), muted: true }, n("late", 5 * B, B, 64)];
    expect(tileGhost(ghost, 4 * B, 4 * B).map((x) => x.pitch)).toEqual([60]);
  });
});

describe("triplet and dotted snapping", () => {
  // 960 PPQ makes the odd grids whole numbers: a 1/8 triplet is 320 ticks, a dotted 1/8 is 720.
  const T8 = 320;

  it("snaps to a triplet grid without drifting off it", () => {
    expect(snapFloor(500, T8)).toBe(320);
    expect(snapRound(500, T8)).toBe(640);
    expect(snapRound(3 * T8 - 1, T8)).toBe(960);
    // Three of them make the beat, exactly, which is the whole reason to count in ticks.
    expect(snapRound(TICKS_PER_BEAT, T8)).toBe(TICKS_PER_BEAT);
  });

  it("quantizes onto a triplet grid, and part of the way there at less than full strength", () => {
    const notes = [n("a", 300, 480, 60), n("b", 700, 480, 62)];
    const hard = quantizeNotes(notes, new Set(["a", "b"]), T8);
    expect(hard.map((x) => x.startTick)).toEqual([320, 640]);
    // Lengths land on the triplet ladder too: a straight 1/8 (480) is nearest two triplet steps.
    expect(hard.map((x) => x.lengthTicks)).toEqual([640, 640]);

    // Half strength closes half the distance: 300 → 310, 700 → 670. Feel survives.
    const soft = quantizeNotes(notes, new Set(["a", "b"]), T8, 0.5);
    expect(soft.map((x) => x.startTick)).toEqual([310, 670]);

    // Zero strength moves no start at all.
    expect(quantizeNotes(notes, new Set(["a"]), T8, 0).map((x) => x.startTick)).toEqual([300, 700]);
  });
});

describe("step selection", () => {
  const S = 240; // a 1/16 step
  // Two bars of a three-pad kit: kick, snare, hat.
  const hits = [
    n("k1", 0, S, 36),
    n("h1", S, S, 42),
    n("s1", 4 * S, S, 38),
    n("k2", 16 * S, S, 36),
    n("h2", 17 * S, S, 42),
  ];
  const pads = [36, 38, 42];

  it("picks the hits a step × row rectangle covers", () => {
    expect([...hitsInSteps(hits, pads, S, 0, 15)].sort()).toEqual(["h1", "k1", "s1"]);
    expect([...hitsInSteps(hits, [36], S, 0, 15)]).toEqual(["k1"]);
    expect([...hitsInSteps(hits, pads, S, 16, 31)].sort()).toEqual(["h2", "k2"]);
    expect(hitsInSteps(hits, [60], S, 0, 31).size).toBe(0);
  });

  it("catches a hit that leans off its step by the step it starts in", () => {
    const dragged = [{ ...n("k1", 0, S, 36), shiftTicks: -40 }];
    expect([...hitsInSteps(dragged, [36], S, 0, 0)]).toEqual(["k1"]);
  });

  it("copies a bar to another bar through the clipboard, cut to the pattern", () => {
    const picked = hits.filter((x) => hitsInSteps(hits, pads, S, 0, 15).has(x.id));
    const { notes: out, newIds } = pasteNotes(hits, picked, 32 * S, 48 * S);
    expect(newIds.size).toBe(3);
    expect(out.filter((x) => newIds.has(x.id)).map((x) => x.startTick).sort((a, b) => a - b)).toEqual([32 * S, 33 * S, 36 * S]);

    // A paste that would run past the pattern loses the copies that land outside it and cuts
    // the one straddling the end, so the grid never grows a step it cannot show.
    const tight = pasteNotes(hits, picked, 47 * S + 120, 48 * S);
    expect(tight.newIds.size).toBe(1);
    expect(tight.notes.at(-1)).toMatchObject({ startTick: 47 * S + 120, lengthTicks: 120 });
  });

  it("moves hits between kit rows and stops at the ends", () => {
    const down = moveHitsByRow(hits, new Set(["k1"]), pads, 1);
    expect(down.find((x) => x.id === "k1")!.pitch).toBe(38);
    // The whole selection shifts together, so a move that would push one hit off the kit is
    // refused for all of them: the shape of what was picked survives.
    expect(moveHitsByRow(hits, new Set(["k1", "h1"]), pads, 1)).toBe(hits);
    expect(moveHitsByRow(hits, new Set(["k1"]), pads, -1)).toBe(hits);
    const up = moveHitsByRow(hits, new Set(["h1", "s1"]), pads, -1);
    expect(up.find((x) => x.id === "h1")!.pitch).toBe(38);
    expect(up.find((x) => x.id === "s1")!.pitch).toBe(36);
  });
});

describe("pitch envelope", () => {
  const bent = (points: PitchPoint[]): NoteEvent[] => [{ ...n("a", 0, 960, 60), pitchEnvelope: points }, n("b", 960, 480, 62)];
  const ramp = (tick: number, cents: number): PitchPoint => ({ tick, cents, curve: "ramp" });

  it("reads as zero before the first point and holds after the last", () => {
    // An envelope offsets a note; a note that has not bent yet is in tune, which is where
    // this parts company with an automation lane holding its first value backwards.
    const points = [ramp(240, 200), { tick: 480, cents: 0, curve: "step" as const }];
    expect(pitchCentsAt(points, 0)).toBe(0);
    expect(pitchCentsAt(points, 120)).toBe(100);
    expect(pitchCentsAt(points, 240)).toBe(200);
    // A step holds the previous value right up to its own tick, then jumps.
    expect(pitchCentsAt(points, 479)).toBe(200);
    expect(pitchCentsAt(points, 480)).toBe(0);
    expect(pitchCentsAt(points, 5_000)).toBe(0);
    expect(pitchCentsAt([], 100)).toBe(0);
  });

  it("tidies into one clamped, sorted point per tick, keeping the later one", () => {
    expect(
      tidyPitchEnvelope([ramp(480, 9_000), ramp(-5, 50), ramp(240, 10), ramp(240, 20), ramp(120, Number.NaN)]),
    ).toEqual([ramp(240, 20), ramp(480, 1_200)]);
  });

  it("adds points, refuses the seventeenth and replaces one that lands on an occupied tick", () => {
    let notes = bent([]);
    for (let i = 0; i < 16; i++) notes = addPitchPoint(notes, "a", i * 50, i * 10);
    expect(notes[0].pitchEnvelope).toHaveLength(16);
    // Full: a new tick is refused outright rather than evicting somebody's earlier point.
    expect(addPitchPoint(notes, "a", 900, 100)).toBe(notes);
    // But an occupied tick is a replacement, which does not grow the envelope.
    const replaced = addPitchPoint(notes, "a", 100, -75);
    expect(replaced[0].pitchEnvelope).toHaveLength(16);
    expect(replaced[0].pitchEnvelope!.find((p) => p.tick === 100)!.cents).toBe(-75);
    // A point is held inside the note it belongs to, both ends.
    expect(addPitchPoint(bent([]), "a", -400, 50)[0].pitchEnvelope![0].tick).toBe(0);
    expect(addPitchPoint(bent([]), "a", 5_000, 50)[0].pitchEnvelope![0].tick).toBe(960);
  });

  it("leaves other notes and the note's own fine alone", () => {
    const before = [{ ...n("a", 0, 960, 60), fine: -40 }, n("b", 960, 480, 62)];
    const notes = addPitchPoint(before, "a", 120, 150);
    expect(notes[0].fine).toBe(-40);
    expect(notes[0].pitchEnvelope).toEqual([ramp(120, 150)]);
    // The untouched note comes back as the same object, so React skips redrawing it.
    expect(notes[1]).toBe(before[1]);
    expect(addPitchPoint(notes, "nope", 0, 0)).toBe(notes);
  });

  it("moves a point, reordering when a drag carries it past its neighbour", () => {
    const notes = bent([ramp(100, 0), ramp(200, 100), ramp(300, 50)]);
    const dragged = movePitchPoint(notes, "a", 1, 400, 25);
    expect(dragged[0].pitchEnvelope).toEqual([ramp(100, 0), ramp(300, 50), ramp(400, 25)]);
    // The same drag run again from the same base lands in the same place: a drag previews
    // from its own starting array, so reordering mid-gesture cannot lose the point.
    expect(movePitchPoint(notes, "a", 1, 400, 25)).toEqual(dragged);
    expect(movePitchPoint(notes, "a", 9, 0, 0)).toBe(notes);
  });

  it("removes a point and flips its curve", () => {
    const notes = bent([ramp(100, 0), ramp(200, 100)]);
    expect(removePitchPoint(notes, "a", 0)[0].pitchEnvelope).toEqual([ramp(200, 100)]);
    expect(togglePitchCurve(notes, "a", 1)[0].pitchEnvelope![1].curve).toBe("step");
    expect(togglePitchCurve(togglePitchCurve(notes, "a", 1), "a", 1)).toEqual(notes);
    expect(removePitchPoint(notes, "a", 7)).toBe(notes);
  });

  it("finds the handle under the pointer, nearest first, and nothing outside the radius", () => {
    // One pixel per tick horizontally, one pixel per cent vertically: distances are ticks.
    const scale = { pxPerTick: 1, pxPerCent: 1 };
    const note = bent([ramp(100, 0), ramp(104, 20)])[0];
    expect(pitchPointAt(note, 101, 1, scale, 7)).toBe(0);
    expect(pitchPointAt(note, 103, 18, scale, 7)).toBe(1);
    expect(pitchPointAt(note, 200, 0, scale, 7)).toBeNull();
    expect(pitchPointAt(n("b", 0, 960, 62), 0, 0, scale, 7)).toBeNull();
  });

  it("draws a polyline that opens in tune and carries a step's corner", () => {
    expect(pitchPolyline([], 960)).toEqual([]);
    expect(pitchPolyline([ramp(240, 200)], 960)).toEqual([
      { tick: 0, cents: 0 },
      { tick: 240, cents: 200 },
      { tick: 960, cents: 200 },
    ]);
    // A step is two vertices at one tick, so the drawn line matches what is heard.
    expect(pitchPolyline([ramp(0, 50), { tick: 480, cents: -50, curve: "step" }], 480)).toEqual([
      { tick: 0, cents: 50 },
      { tick: 480, cents: 50 },
      { tick: 480, cents: -50 },
    ]);
  });
});