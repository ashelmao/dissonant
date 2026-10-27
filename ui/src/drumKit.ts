// Pure edits on a drum track's kit (`Track.drumPads`, top to bottom). Every edit returns a new
// list; the caller dispatches it whole with `setDrumPads`, so the core always sees a full kit.

import { factorySound, isFactory, lookupSound, MAX_DRUM_PADS, type DrumPad, type Sound } from "./sound";

/** Free pitches tried first when adding a pad: the General MIDI kit, most common first. */
const GM_ORDER = [36, 38, 42, 46, 39, 49, 45, 50, 37, 40, 41, 43, 44, 47, 48, 51, 52, 53, 54, 55, 56, 57, 59, 35];

/** The first unused pitch (GM kit order, then up from 36, then down), or null when 0–127 is full. */
export function nextFreePitch(pads: DrumPad[]): number | null {
  const used = new Set(pads.map((p) => p.pitch));
  for (const p of GM_ORDER) if (!used.has(p)) return p;
  for (let p = 36; p <= 127; p++) if (!used.has(p)) return p;
  for (let p = 35; p >= 0; p--) if (!used.has(p)) return p;
  return null;
}

/** GM pitches for a kind of drum, best first; matched against a sound's id, name and category. */
const GM_ROLES: [RegExp, number[]][] = [
  [/open/, [46]],
  [/pedal/, [44, 42]],
  [/hat|\bhh\b/, [42, 44]],
  [/kick|\bbd\b/, [36, 35]],
  [/rim|stick/, [37]],
  [/snare|\bsd\b/, [38, 40]],
  [/clap|snap/, [39]],
  [/cowbell/, [56]],
  [/bell/, [53]],
  [/ride/, [51, 59]],
  [/china/, [52]],
  [/splash/, [55]],
  [/crash|cym/, [49, 57]],
  [/floor/, [41, 43]],
  [/tom.*(hi|high)/, [50, 48]],
  [/tom.*mid/, [47, 48]],
  [/tom/, [45, 43, 41, 47]],
  [/tambourine/, [54]],
  [/bongo/, [60, 61]],
  [/conga.*(lo|low)/, [64]],
  [/conga/, [62, 63]],
  [/clave/, [75]],
  [/block/, [76, 77]],
  [/shaker|maraca/, [70]],
];

/** The GM pitches a sound would sit on in a standard kit, best first; empty when it's no GM drum. */
export function gmPitchesFor(sound: Pick<Sound, "id" | "name" | "category">): number[] {
  const text = `${sound.id} ${sound.name} ${sound.category}`.toLowerCase();
  return GM_ROLES.find(([re]) => re.test(text))?.[1] ?? [];
}

const isHat = (s: Pick<Sound, "id" | "name" | "category"> | undefined) => !!s && /hat|\bhh\b/.test(`${s.id} ${s.name} ${s.category}`.toLowerCase());

/**
 * `sound` as a new pad on top of the kit (the kick conventionally sits at the bottom), on the
 * GM pitch that fits it when free, else the next free one; null when the kit is full. A hat
 * joins the kit's other hats in one choke group, so the open hat stops when the closed one plays.
 * `lookup` names the sounds already on the kit (to find its hats).
 */
export function addPad(pads: DrumPad[], sound: Sound, lookup: (id: string) => Sound | undefined = factorySound): DrumPad[] | null {
  if (pads.length >= MAX_DRUM_PADS) return null;
  const used = new Set(pads.map((p) => p.pitch));
  const pitch = gmPitchesFor(sound).find((p) => !used.has(p)) ?? nextFreePitch(pads);
  if (pitch === null) return null;
  let choke = 0;
  let rest = pads;
  if (isHat(sound)) {
    const hats = pads.filter((p) => isHat(lookup(p.sound)));
    if (hats.length) {
      const groups = new Set(pads.map((p) => p.choke));
      choke = hats.find((p) => p.choke > 0)?.choke ?? Array.from({ length: 64 }, (_, i) => i + 1).find((g) => !groups.has(g)) ?? 1;
      rest = pads.map((p) => (p.choke === 0 && hats.includes(p) ? { ...p, choke } : p));
    }
  }
  return [{ pitch, sound: sound.id, choke }, ...rest];
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
