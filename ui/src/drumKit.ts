// Pure edits on a drum track's kit (`Track.drumPads`, top to bottom). Every edit returns a new
// list; the caller dispatches it whole with `setDrumPads`, so the core always sees a full kit.

import { FACTORY_SOUNDS, isFactory, lookupSound, MAX_DRUM_PADS, type DrumPad, type Sound } from "./sound";

/** Free pitches tried first when adding a pad: the General MIDI kit, most common first. */
const GM_ORDER = [36, 38, 42, 46, 39, 49, 45, 50, 37, 40, 41, 43, 44, 47, 48, 51, 52, 53, 54, 55, 56, 57, 59, 35];

/** Factory sound ids that fit a GM pitch, best first (only ones that exist are used). */
const GM_SOUNDS: Record<number, string[]> = {
  35: ["kick"],
  36: ["kick"],
  37: ["rim", "clap"],
  38: ["snare"],
  39: ["clap"],
  40: ["snare"],
  41: ["tom-lo"],
  42: ["hat"],
  43: ["tom-lo"],
  44: ["hat"],
  45: ["tom-lo"],
  46: ["open-hat"],
  47: ["tom-mid", "tom-lo"],
  48: ["tom-mid", "tom-hi"],
  49: ["crash"],
  50: ["tom-hi"],
  51: ["ride", "crash"],
  52: ["china", "crash"],
  53: ["ride-bell", "ride", "crash"],
  54: ["tambourine", "hat"],
  55: ["splash", "crash"],
  56: ["cowbell"],
  57: ["crash"],
  59: ["ride", "crash"],
};

/** The first unused pitch (GM kit order, then up from 36, then down), or null when 0–127 is full. */
export function nextFreePitch(pads: DrumPad[]): number | null {
  const used = new Set(pads.map((p) => p.pitch));
  for (const p of GM_ORDER) if (!used.has(p)) return p;
  for (let p = 36; p <= 127; p++) if (!used.has(p)) return p;
  for (let p = 35; p >= 0; p--) if (!used.has(p)) return p;
  return null;
}

/** A sensible factory drum sound for a pitch: its GM instrument when the factory has one. */
export function defaultSoundFor(pitch: number, factory: Sound[] = FACTORY_SOUNDS): string {
  const drums = factory.filter((s) => s.patch.kind === "drum");
  const hit = (GM_SOUNDS[pitch] ?? []).find((id) => drums.some((s) => s.id === id));
  return hit ?? drums.find((s) => s.id === "kick")?.id ?? drums[0]?.id ?? "kick";
}

/** A new pad on top of the kit (the kick conventionally sits at the bottom), or null when full. */
export function addPad(pads: DrumPad[], factory: Sound[] = FACTORY_SOUNDS): DrumPad[] | null {
  if (pads.length >= MAX_DRUM_PADS) return null;
  const pitch = nextFreePitch(pads);
  if (pitch === null) return null;
  return [{ pitch, sound: defaultSoundFor(pitch, factory), choke: 0 }, ...pads];
}

/** Without pad `index`; a kit never drops below one pad (returns null then). */
export function removePad(pads: DrumPad[], index: number): DrumPad[] | null {
  if (pads.length <= 1 || index < 0 || index >= pads.length) return null;
  return pads.filter((_, i) => i !== index);
}

/** Pad `index` swapped with its neighbour `delta` rows away (-1 up, +1 down), or null at an edge. */
export function movePad(pads: DrumPad[], index: number, delta: number): DrumPad[] | null {
  const j = index + delta;
  if (index < 0 || index >= pads.length || j < 0 || j >= pads.length || delta === 0) return null;
  const out = pads.slice();
  [out[index], out[j]] = [out[j], out[index]];
  return out;
}

/** Why `pitch` can't go on pad `index`, or null when it can (whole number 0–127, unused elsewhere). */
export function pitchProblem(pads: DrumPad[], index: number, pitch: number): string | null {
  if (!Number.isInteger(pitch) || pitch < 0 || pitch > 127) return "pitch must be a whole number 0–127";
  const other = pads.findIndex((p, i) => i !== index && p.pitch === pitch);
  return other >= 0 ? `pitch ${pitch} is already pad ${other + 1}` : null;
}

/** Pad `index` with `patch` merged in, or null when the result would break the kit. */
export function replacePad(pads: DrumPad[], index: number, patch: Partial<DrumPad>): DrumPad[] | null {
  if (index < 0 || index >= pads.length) return null;
  const next = { ...pads[index], ...patch };
  if (patch.pitch !== undefined && pitchProblem(pads, index, next.pitch)) return null;
  if (!Number.isInteger(next.choke) || next.choke < 0) return null;
  return pads.map((p, i) => (i === index ? next : p));
}

/**
 * The non-factory sounds a kit's pads use, once each, for `setDrumPads`. The project's embedded
 * copy wins over the library's, so editing a pad never silently swaps a sound the song already
 * plays; a sound new to the project comes from the library. Unknown ids are skipped.
 */
export function kitSounds(pads: DrumPad[], library: Sound[], projectSounds: Sound[]): Sound[] {
  const out: Sound[] = [];
  for (const { sound: id } of pads) {
    if (isFactory(id) || out.some((s) => s.id === id)) continue;
    const s = projectSounds.find((x) => x.id === id) ?? library.find((x) => x.id === id);
    if (s) out.push(s);
  }
  return out;
}

/** A pad's row label: its sound's name, falling back to the id. */
export function padName(projectSounds: Sound[], pad: DrumPad): string {
  return lookupSound(projectSounds, pad.sound)?.name ?? pad.sound;
}
