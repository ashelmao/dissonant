import { describe, expect, it } from "vitest";
import { addPad, gmPitchesFor, kitSounds, movePad, nextFreePitch, padName, pitchProblem, removePad, replacePad } from "./drumKit";
import { DEFAULT_DRUM_PATCH, defaultKit, factorySound, MAX_DRUM_PADS, type DrumPad, type Sound } from "./sound";

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

describe("gmPitchesFor", () => {
  const gm = (id: string) => gmPitchesFor(factorySound(id)!)[0];
  it("finds the GM pitch for every kind of factory drum", () => {
    expect(gm("kick-808")).toBe(36);
    expect(gm("snare-brush")).toBe(38);
    expect(gm("rimshot")).toBe(37);
    expect(gm("snap")).toBe(39);
    expect(gm("hat-909")).toBe(42);
    expect(gm("open-hat-808")).toBe(46);
    expect(gm("hat-pedal")).toBe(44);
    expect(gm("crash-big")).toBe(49);
    expect(gm("ride")).toBe(51);
    expect(gm("ride-bell")).toBe(53);
    expect(gm("cowbell")).toBe(56);
    expect(gm("tom-808-hi")).toBe(50);
    expect(gm("tom-floor")).toBe(41);
    expect(gm("conga-lo")).toBe(64);
  });
  it("goes by a user sound's name and category, and has nothing for an odd one", () => {
    expect(gmPitchesFor({ id: "u-1", name: "dusty snare", category: "mine" })[0]).toBe(38);
    expect(gmPitchesFor({ id: "u-2", name: "thud", category: "kick" })[0]).toBe(36);
    expect(gmPitchesFor(factorySound("zap")!)).toEqual([]);
    // Random ids don't read as a drum ("bd" inside a uuid isn't a bass drum).
    expect(gmPitchesFor({ id: "u-3bd4", name: "thing", category: "mine" })).toEqual([]);
  });
});

describe("addPad / removePad / movePad", () => {
  const sound = (id: string) => factorySound(id)!;
  it("adds the sound on top, on its GM pitch when free, up to the pad limit", () => {
    const kit = defaultKit();
    const added = addPad(kit, sound("ride"))!;
    expect(added).toHaveLength(kit.length + 1);
    expect(added[0]).toEqual({ pitch: 51, sound: "ride", choke: 0 });
    expect(added.slice(1)).toEqual(kit);
    // 38 is taken: the second snare goes on the other GM snare pitch, then on any free one.
    expect(addPad(kit, sound("snare-909"))![0].pitch).toBe(40);
    expect(addPad(addPad(kit, sound("snare-909"))!, sound("snare-808"))![0].pitch).toBe(37);
    expect(addPad(kit, sound("zap"))![0].pitch).toBe(37); // no GM role: the next free pitch
    let full: DrumPad[] = [];
    for (let i = 0; i < MAX_DRUM_PADS; i++) full = addPad(full, sound("kick"))!;
    expect(new Set(full.map((p) => p.pitch)).size).toBe(MAX_DRUM_PADS);
    expect(addPad(full, sound("kick"))).toBeNull();
  });
  it("puts a hat in one choke group with the kit's other hats", () => {
    // The default kit's hats already share group 1.
    expect(addPad(defaultKit(), sound("hat-pedal"))![0]).toEqual({ pitch: 44, sound: "hat-pedal", choke: 1 });
    // Ungrouped hats get the first free group, and so does the new one.
    const kit = [pad(42, "hat"), pad(38, "snare", 1), pad(36)];
    const added = addPad(kit, sound("open-hat"))!;
    expect(added.map((p) => [p.pitch, p.choke])).toEqual([[46, 2], [42, 2], [38, 1], [36, 0]]);
    // A lone hat, or a non-hat, chokes nothing.
    expect(addPad([pad(36)], sound("open-hat"))![0].choke).toBe(0);
    expect(addPad(kit, sound("crash"))!.map((p) => p.choke)).toEqual([0, 0, 1, 0]);
    // User hats are found through the lookup.
    const mine = userDrum("u-hat", "my hat");
    const withMine = [{ pitch: 42, sound: "u-hat", choke: 0 }, pad(36)];
    expect(addPad(withMine, sound("open-hat"), (id) => (id === "u-hat" ? mine : factorySound(id)))![1].choke).toBe(1);
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
