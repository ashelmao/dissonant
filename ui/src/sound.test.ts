import { describe, expect, it } from "vitest";
import { DEFAULT_SYNTH_PATCH, FACTORY_SOUNDS, filledPatch } from "./sound";

describe("filledPatch", () => {
  it("fills like serde: a missing field takes the patch default, a partial one its type's default", () => {
    const missing = filledPatch({ kind: "synth", level: 0.5 });
    if (missing.kind !== "synth") throw new Error("expected synth");
    expect(missing.osc[0].on).toBe(true);
    expect(missing.filterEnv).toEqual(DEFAULT_SYNTH_PATCH.filterEnv);

    const partial = filledPatch({ kind: "synth", osc: [{ wave: "sine" }, {}, {}], filterEnv: { attack: 0.2 } });
    if (partial.kind !== "synth") throw new Error("expected synth");
    expect(partial.osc[0].on).toBe(false);
    expect(partial.filterEnv).toEqual({ attack: 0.2, decay: 0.1, sustain: 0.65, release: 0.14 });
  });

  it("loads every factory sound complete", () => {
    expect(FACTORY_SOUNDS.length).toBeGreaterThan(10);
    for (const s of FACTORY_SOUNDS) expect(Number.isFinite(s.patch.level)).toBe(true);
  });
});
