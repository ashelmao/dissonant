import { describe, expect, it } from "vitest";
import { DEFAULT_SYNTH_PATCH, FACTORY_SOUNDS, factorySound, filledPatch } from "./sound";

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

  it("opens a patch from before voice modes the way Rust does: mono becomes legato", () => {
    const voice = (patch: Record<string, unknown>) => {
      const p = filledPatch({ kind: "synth", ...patch });
      if (p.kind !== "synth") throw new Error("expected synth");
      expect(p).not.toHaveProperty("mono");
      return p.voice;
    };
    expect(voice({})).toBe("poly");
    expect(voice({ mono: true, glide: 0.2 })).toBe("legato");
    expect(voice({ mono: false })).toBe("poly");
    expect(voice({ voice: "mono" })).toBe("mono");
    expect(voice({ mono: true, voice: "mono" })).toBe("mono");
  });

  it("gives the old mono factory sounds legato and the rest poly", () => {
    const voiceOf = (id: string) => {
      const p = factorySound(id)?.patch;
      return p?.kind === "synth" ? p.voice : undefined;
    };
    expect(voiceOf("acid-bass")).toBe("legato");
    expect(voiceOf("saw")).toBe("poly");
  });
});
