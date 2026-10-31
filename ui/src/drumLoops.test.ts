import { describe, expect, it } from "vitest";
import { FACTORY_KITS, strandedHits } from "./drumKit";
import { FACTORY_LOOPS, loopHits, loopLengthTicks, swingTicks } from "./drumLoops";
import { TICKS_PER_BEAT } from "./types";

const BAR = 4 * TICKS_PER_BEAT;
const loop = (id: string) => FACTORY_LOOPS.find((l) => l.id === id)!;

describe("factory loops", () => {
  it("load from the shared JSON, each on a factory kit", () => {
    expect(FACTORY_LOOPS.length).toBeGreaterThanOrEqual(60);
    for (const k of FACTORY_KITS) expect(FACTORY_LOOPS.some((l) => l.kit.id === k.id), k.id).toBe(true);
  });
  it("play every hit on the kit they name, in whole bars", () => {
    for (const l of FACTORY_LOOPS) {
      expect([BAR, 2 * BAR, 4 * BAR], l.id).toContain(loopLengthTicks(l));
      expect(strandedHits(l.kit.pads, loopHits(l, loopLengthTicks(l))), l.id).toBe(0);
    }
  });
});

describe("loopHits", () => {
  it("reads the step strings: X accent, x hit, o ghost, a 16th each", () => {
    const hits = loopHits(loop("backbeat"), BAR);
    expect(hits).toHaveLength(8 + 2 + 3);
    const kick = hits.filter((h) => h.pitch === 36);
    expect(kick.map((h) => [h.startTick, h.velocity])).toEqual([
      [0, 120],
      [2 * TICKS_PER_BEAT, 120],
      [2.5 * TICKS_PER_BEAT, 96],
    ]);
    expect(new Set(hits.map((h) => h.id)).size).toBe(hits.length);
  });
  it("repeats to fill a longer pattern and stops at a shorter one", () => {
    const amen = loop("amen");
    const once = loopHits(amen, loopLengthTicks(amen));
    expect(loopHits(amen, 2 * loopLengthTicks(amen))).toHaveLength(2 * once.length);
    const bar = loopHits(amen, BAR);
    expect(bar.every((h) => h.startTick + h.lengthTicks <= BAR)).toBe(true);
    expect(bar.length).toBeLessThan(once.length);
  });
  it("reads triplet and 32nd lanes at their own step", () => {
    const trips = loopHits(loop("triplet-trap"), BAR).filter((h) => h.pitch === 42);
    expect(trips).toHaveLength(12);
    expect(trips[1].startTick).toBe(TICKS_PER_BEAT / 3);
    const rolls = loopHits(loop("trap"), BAR).filter((h) => h.pitch === 42);
    expect(rolls[1].startTick).toBe(TICKS_PER_BEAT / 2);
    expect(rolls[0].lengthTicks).toBe(TICKS_PER_BEAT / 8);
  });
  it("swings the off-16ths late without moving them off their cell, like the core", () => {
    const l = loop("boom-bap");
    expect(swingTicks(l)).toBe(38);
    const hits = loopHits(l, loopLengthTicks(l));
    for (const h of hits) expect(h.shiftTicks ?? 0).toBe(h.startTick % 480 === 240 ? 38 : 0);
    expect(hits.some((h) => h.shiftTicks)).toBe(true);
    expect(loopHits(loop("backbeat"), BAR).some((h) => h.shiftTicks)).toBe(false);
  });
});
