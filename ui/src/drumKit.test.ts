import { describe, expect, it } from "vitest";
import { addPad, defaultSoundFor, kitSounds, movePad, nextFreePitch, padName, pitchProblem, removePad, replacePad } from "./drumKit";
import { DEFAULT_DRUM_PATCH, defaultKit, MAX_DRUM_PADS, type DrumPad, type Sound } from "./sound";

const userDrum = (id: string, name = id): Sound => ({ id, name, category: "mine", patch: { kind: "drum", ...DEFAULT_DRUM_PATCH } });
const pad = (pitch: number, sound = "kick", choke = 0): DrumPad => ({ pitch, sound, choke });

describe("nextFreePitch", () => {
  it("walks the GM kit first", () => {
    expect(nextFreePitch([])).toBe(36);
    expect(nextFreePitch([pad(36)])).toBe(38);
    // The default kit uses 49 46 42 39 38 50 45 36: the next GM pitch is 37 (side stick).
    expect(nextFreePitch(defaultKit())).toBe(37);
  });
  it("falls back to any unused pitch, and null when all 128 are taken", () => {
    const all = Array.from({ length: 128 }, (_, p) => pad(p));
    expect(nextFreePitch(all)).toBeNull();
    expect(nextFreePitch(all.filter((p) => p.pitch !== 3))).toBe(3);
  });
});

describe("defaultSoundFor", () => {
  it("picks the GM instrument when the factory has it", () => {
    expect(defaultSoundFor(36)).toBe("kick");
    expect(defaultSoundFor(38)).toBe("snare");
    expect(defaultSoundFor(46)).toBe("open-hat");
    expect(defaultSoundFor(49)).toBe("crash");
  });
  it("falls through missing candidates and unknown pitches", () => {
    const factory = [userDrum("kick"), userDrum("crash")];
    expect(defaultSoundFor(51, factory)).toBe("crash"); // no ride → crash
    expect(defaultSoundFor(90, factory)).toBe("kick");
  });
});

describe("addPad / removePad / movePad", () => {
  it("adds on top with a free pitch, up to the pad limit", () => {
    const kit = defaultKit();
    const added = addPad(kit)!;
    expect(added).toHaveLength(kit.length + 1);
    expect(added[0]).toMatchObject({ pitch: 37, choke: 0 });
    expect(added.slice(1)).toEqual(kit);
    let full: DrumPad[] = [];
    for (let i = 0; i < MAX_DRUM_PADS; i++) full = addPad(full)!;
    expect(new Set(full.map((p) => p.pitch)).size).toBe(MAX_DRUM_PADS);
    expect(addPad(full)).toBeNull();
  });
  it("never removes the last pad", () => {
    expect(removePad([pad(36)], 0)).toBeNull();
    expect(removePad([pad(38), pad(36)], 0)).toEqual([pad(36)]);
    expect(removePad([pad(38), pad(36)], 5)).toBeNull();
  });
  it("swaps neighbours and refuses the edges", () => {
    const kit = [pad(42), pad(38), pad(36)];
    expect(movePad(kit, 1, -1)!.map((p) => p.pitch)).toEqual([38, 42, 36]);
    expect(movePad(kit, 1, 1)!.map((p) => p.pitch)).toEqual([42, 36, 38]);
    expect(movePad(kit, 0, -1)).toBeNull();
    expect(movePad(kit, 2, 1)).toBeNull();
    expect(kit.map((p) => p.pitch)).toEqual([42, 38, 36]); // not mutated
  });
});

describe("replacePad / pitchProblem", () => {
  const kit = [pad(42, "hat", 1), pad(38, "snare"), pad(36)];
  it("keeps pitches unique and in range", () => {
    expect(pitchProblem(kit, 0, 42)).toBeNull(); // its own pitch
    expect(pitchProblem(kit, 0, 38)).toMatch(/pad 2/);
    expect(pitchProblem(kit, 0, 128)).not.toBeNull();
    expect(pitchProblem(kit, 0, 40.5)).not.toBeNull();
    expect(replacePad(kit, 0, { pitch: 38 })).toBeNull();
    expect(replacePad(kit, 0, { pitch: 44 })![0]).toEqual(pad(44, "hat", 1));
  });
  it("replaces sound and choke", () => {
    const next = replacePad(kit, 1, { sound: "clap", choke: 2 })!;
    expect(next[1]).toEqual(pad(38, "clap", 2));
    expect(next[0]).toBe(kit[0]);
    expect(replacePad(kit, 1, { choke: -1 })).toBeNull();
    expect(replacePad(kit, 9, { choke: 1 })).toBeNull();
  });
});

describe("kitSounds", () => {
  it("embeds each non-factory sound once, preferring the project's copy", () => {
    const libA = userDrum("u-a", "a (library)");
    const projA = userDrum("u-a", "a (project)");
    const libB = userDrum("u-b");
    const pads = [pad(36, "kick"), pad(38, "u-a"), pad(39, "u-b"), pad(40, "u-a"), pad(41, "u-gone")];
    expect(kitSounds(pads, [libA, libB], [projA])).toEqual([projA, libB]);
    expect(kitSounds(defaultKit(), [libA], [])).toEqual([]);
  });
});

describe("padName", () => {
  it("names a pad by its sound, falling back to the id", () => {
    expect(padName([], pad(36, "kick"))).toBe("kick");
    expect(padName([userDrum("u-x", "big boom")], pad(36, "u-x"))).toBe("big boom");
    expect(padName([], pad(36, "u-missing"))).toBe("u-missing");
  });
});
