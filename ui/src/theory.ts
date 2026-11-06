// A small mirror of dissonant-core's theory, used for 60 fps rendering so the roll never has
// to round-trip to Rust per frame. Rust remains the source of truth; tests pin both to the
// same expectations.

import type { ChordEvent, KeyState, NoteEvent, PitchPoint, ScaleType, Tier } from "./types";
import { TICKS_PER_BEAT, uuid } from "./types";

export const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const MAJOR_STEPS = [0, 2, 4, 5, 7, 9, 11];
const MINOR_STEPS = [0, 2, 3, 5, 7, 8, 10];

export const normalize = (pc: number): number => ((pc % 12) + 12) % 12;

export const noteName = (pc: number): string => NOTE_NAMES[normalize(pc)];
export const midiName = (pitch: number): string => `${noteName(pitch)}${Math.floor(pitch / 12) - 1}`;
export const keyName = (root: number, scale: ScaleType): string =>
  `${noteName(root)} ${scale === "major" ? "maj" : "min"}`;

export function scalePitchClasses(root: number, scale: ScaleType): number[] {
  return (scale === "major" ? MAJOR_STEPS : MINOR_STEPS).map((s) => normalize(root + s));
}

export function keyPitchClasses(key: KeyState): number[] | null {
  return key.rootPitchClass === null ? null : scalePitchClasses(key.rootPitchClass, key.scale);
}

export interface ChordSuggestion {
  pitchClasses: number[];
  name: string;
  romanNumeral: string;
}

type Quality = "major" | "minor" | "dim" | "aug" | "other";

function qualityOf(third: number, fifth: number): Quality {
  if (third === 4 && fifth === 7) return "major";
  if (third === 3 && fifth === 7) return "minor";
  if (third === 3 && fifth === 6) return "dim";
  if (third === 4 && fifth === 8) return "aug";
  return "other";
}

const SUFFIX: Record<Quality, string> = { major: "", minor: "m", dim: "dim", aug: "aug", other: "?" };

function roman(q: Quality, degree: number): string {
  const base = ["I", "II", "III", "IV", "V", "VI", "VII"][degree % 7];
  switch (q) {
    case "major":
      return base;
    case "aug":
      return `${base}+`;
    case "minor":
      return base.toLowerCase();
    case "dim":
      return `${base.toLowerCase()}°`;
    default:
      return `${base}?`;
  }
}

export function diatonicChords(root: number, scale: ScaleType): ChordSuggestion[] {
  const pcs = scalePitchClasses(root, scale);
  return Array.from({ length: 7 }, (_, degree) => {
    const chord = [degree, degree + 2, degree + 4].map((d) => pcs[d % 7]);
    const q = qualityOf(normalize(chord[1] - chord[0]), normalize(chord[2] - chord[0]));
    return { pitchClasses: chord, name: `${noteName(chord[0])}${SUFFIX[q]}`, romanNumeral: roman(q, degree) };
  });
}

export function chordName(pitchClasses: number[]): string {
  const pcs = [...new Set(pitchClasses.map(normalize))].sort((a, b) => a - b);
  if (pcs.length === 0) return "—";
  if (pcs.length === 3) {
    for (const root of pcs) {
      const iv = pcs.map((p) => normalize(p - root)).sort((a, b) => a - b);
      const q = qualityOf(iv[1], iv[2]);
      if (q !== "other") return `${noteName(root)}${SUFFIX[q]}`;
    }
  }
  return pcs.map(noteName).join("·");
}

/** Mirrors `harmony::ProgressionStarter`: numerals, a plain-words mood, and the scale the mood holds in. */
export interface Starter {
  name: string;
  mood: string;
  scale: ScaleType;
  degrees: number[];
}

export const STARTERS: Starter[] = [
  { name: "I–IV–V–vi", mood: "bright, lifting", scale: "major", degrees: [0, 3, 4, 5] },
  { name: "I–V–vi–IV", mood: "big and hopeful", scale: "major", degrees: [0, 4, 5, 3] },
  { name: "vi–IV–I–V", mood: "sad but hopeful", scale: "major", degrees: [5, 3, 0, 4] },
  { name: "I–vi–IV–V", mood: "old-school, sweet", scale: "major", degrees: [0, 5, 3, 4] },
  { name: "ii–V–I", mood: "jazzy, resolved", scale: "major", degrees: [1, 4, 0, 0] },
  { name: "I–IV", mood: "open two-chord vamp", scale: "major", degrees: [0, 3] },
  { name: "i–VI–III–VII", mood: "dark, driving", scale: "minor", degrees: [0, 5, 2, 6] },
  { name: "i–VII–VI–VII", mood: "brooding loop", scale: "minor", degrees: [0, 6, 5, 6] },
  { name: "i–v–VI–iv", mood: "sad, cinematic", scale: "minor", degrees: [0, 4, 5, 3] },
  { name: "i–iv", mood: "moody two-chord vamp", scale: "minor", degrees: [0, 3] },
  { name: "i–VI", mood: "hazy, floating", scale: "minor", degrees: [0, 5] },
  { name: "i", mood: "one-chord drone", scale: "minor", degrees: [0] },
];

/** `harmony::starters_for`: the starters whose mood holds in `scale`. */
export function startersFor(scale: ScaleType): Starter[] {
  return STARTERS.filter((s) => s.scale === scale);
}

/**
 * Mirrors `harmony::progression`. The result always fits inside `totalTicks`: on a pattern too
 * short for one pass of the degrees at `chordTicks` each, the chords are divided to fit rather
 * than written past the end, where they would be invisible and never heard.
 */
export function progression(
  degrees: number[],
  root: number,
  scale: ScaleType,
  totalTicks: number,
  chordTicks = 4 * TICKS_PER_BEAT,
): ChordEvent[] {
  if (degrees.length === 0 || chordTicks <= 0 || totalTicks <= 0) return [];
  const diatonic = diatonicChords(root, scale);
  const wholeSlots = Math.floor(totalTicks / chordTicks);
  const short = wholeSlots < degrees.length;
  const slots = short ? degrees.length : wholeSlots;
  const ticks = short ? totalTicks / degrees.length : chordTicks;
  return Array.from({ length: slots }, (_, i) => {
    const c = diatonic[((degrees[i % degrees.length] % 7) + 7) % 7];
    const startTick = Math.round(i * ticks);
    return {
      id: uuid(),
      startTick,
      // The last chord absorbs any remainder so the lane is covered with no gap.
      lengthTicks: (i + 1 === slots ? totalTicks : Math.round((i + 1) * ticks)) - startTick,
      pitchClasses: [...c.pitchClasses],
      name: c.name,
    };
  });
}

// ─── Tiers ─────────────────────────────────────────────────────────────────────────────────

export function tier(pitchClass: number, chord: number[], key: number[] | null): Tier {
  const p = normalize(pitchClass);
  const chordSet = chord.map(normalize);
  if (chordSet.includes(p)) return "chordTone";
  // Avoid note = a half step *above* a chord tone (F over C, C over G7). A half step below is a
  // leading tone into the chord tone (B under C = maj7, E under F = G7's 13th) and stays a
  // tension. Mirrors dissonant-core's TierClassifier::tier.
  if (chordSet.some((c) => normalize(p - c) === 1)) return "dissonance";
  if (key && !key.some((k) => normalize(k) === p)) return "dissonance";
  return "tension";
}

/** The chord sounding at `tick`. Where chords overlap the later-starting one wins, matching
 *  dissonant-core's ChordTrack — which keeps its chords sorted, while this array may not be. */
export function chordAt(chords: ChordEvent[], tick: number): ChordEvent | null {
  let best: ChordEvent | null = null;
  for (const c of chords) {
    if (tick < c.startTick || tick >= c.startTick + c.lengthTicks) continue;
    if (best === null || c.startTick >= best.startTick) best = c;
  }
  return best;
}

/** Tier for every pitch class at `tick`; `null` entries mean "no guidance". */
export function tierMap(tick: number, chords: ChordEvent[], key: KeyState): (Tier | null)[] {
  const keyPcs = keyPitchClasses(key);
  const chord = chordAt(chords, tick);
  if (chord) return Array.from({ length: 12 }, (_, pc) => tier(pc, chord.pitchClasses, keyPcs));
  if (keyPcs) return Array.from({ length: 12 }, (_, pc) => (keyPcs.includes(pc) ? "chordTone" : "dissonance"));
  return Array(12).fill(null);
}

/**
 * Mirrors `tier::moves`: an envelope moves the note once any point leaves the value the note
 * starts on (zero before the first point). A value held throughout is a detune, not a bend.
 */
export function moves(envelope: PitchPoint[] | undefined): boolean {
  if (!envelope || envelope.length === 0) return false;
  const start = envelope[0].tick <= 0 ? envelope[0].cents : 0;
  return envelope.some((p) => p.cents !== start);
}

/** Mirrors `HighlightEngine::note_tier`. */
export interface NoteTier {
  tier: Tier | null;
  moving: boolean;
}

/**
 * One note's tier and moving mark. A bent note is tiered where it starts — its row, exactly as
 * the same note unbent — and the envelope only adds the mark, so one note keeps one colour and
 * the move is said by a glyph rather than by the colour alone.
 */
export function noteTier(note: Pick<NoteEvent, "pitch" | "startTick" | "pitchEnvelope">, chords: ChordEvent[], key: KeyState): NoteTier {
  return { tier: tierMap(note.startTick, chords, key)[normalize(note.pitch)], moving: moves(note.pitchEnvelope) };
}

// ─── Key detection (Krumhansl–Schmuckler) ─────────────────────────────────────────────────

const MAJOR_PROFILE = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR_PROFILE = [6.33, 2.68, 3.52, 5.38, 2.6, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

export interface KeyCandidate {
  rootPitchClass: number;
  scale: ScaleType;
  score: number;
}

export interface KeyDetectionResult {
  candidates: KeyCandidate[];
  isConfident: boolean;
}

function pearson(x: number[], y: number[]): number {
  const n = x.length;
  const mx = x.reduce((a, b) => a + b, 0) / n;
  const my = y.reduce((a, b) => a + b, 0) / n;
  let num = 0;
  let dx = 0;
  let dy = 0;
  for (let i = 0; i < n; i++) {
    const a = x[i] - mx;
    const b = y[i] - my;
    num += a * b;
    dx += a * a;
    dy += b * b;
  }
  const den = Math.sqrt(dx * dy);
  return den === 0 ? 0 : num / den;
}

/** `minDistinct` mirrors dissonant-core's `min_distinct_pitch_classes`: repetition is not
 *  evidence — fewer than five distinct pitch classes is a chord, not a scale.
 *
 *  `chroma` is `KeyDetector::detect_with_chroma`'s: per pitch class, in note equivalents,
 *  heard in a recorded DI. It adds to the placed notes' histogram and to the evidence count,
 *  and a pitch class counts as distinct from half a note's worth up — every placed note does,
 *  the faint harmonic leakage chroma carries never does. */
export function detectKey(pitchClasses: number[], chroma: readonly number[] = [], minNotes = 10, minGap = 0.04, minDistinct = 5): KeyDetectionResult {
  const hist = Array(12).fill(0);
  for (const pc of pitchClasses) hist[normalize(pc)] += 1;
  for (let i = 0; i < 12; i++) {
    const v = chroma[i] ?? 0;
    if (Number.isFinite(v) && v > 0) hist[i] += v;
  }
  const evidence = hist.reduce((a, b) => a + b, 0);
  if (evidence <= 0) return { candidates: [], isConfident: false };
  const candidates: KeyCandidate[] = [];
  for (let root = 0; root < 12; root++) {
    for (const scale of ["major", "minor"] as ScaleType[]) {
      const base = scale === "major" ? MAJOR_PROFILE : MINOR_PROFILE;
      const profile = Array.from({ length: 12 }, (_, i) => base[normalize(i - root)]);
      candidates.push({ rootPitchClass: root, scale, score: pearson(hist, profile) });
    }
  }
  candidates.sort((a, b) => b.score - a.score);
  const gap = candidates[0].score - candidates[1].score;
  const distinct = hist.filter((h) => h >= 0.5).length;
  const isConfident = evidence >= minNotes && distinct >= minDistinct && gap >= minGap;
  return { candidates, isConfident };
}

// ─── Explain this note ─────────────────────────────────────────────────────────────────────

const INTERVAL_NAMES = ["the root", "a ♭9", "the 9th", "a ♭3 / ♯9", "the 3rd", "the 11th", "a ♯11", "the 5th", "a ♭13", "the 13th", "the ♭7", "the 7th"];

/** Root pitch class of a chord: the named root if the chord is a recognizable triad, else its lowest pitch class. */
export function chordRoot(chord: ChordEvent): number {
  const name = chord.name ?? chordName(chord.pitchClasses);
  const m = /^([A-G]#?)/.exec(name);
  if (m) {
    const idx = NOTE_NAMES.indexOf(m[1]);
    if (idx >= 0) return idx;
  }
  return chord.pitchClasses[0] ?? 0;
}

/**
 * Plain words for why a pitch is tiered the way it is against the chord at `tick`.
 * The voice from PRODUCT.md: names things plainly, never condescends, never forbids.
 *
 * `moving` is the note's moving mark ({@link noteTier}): a bent note is judged where it starts,
 * and the text says so, so a bend up out of an avoid note isn't mistaken for the colour being
 * wrong about where it lands.
 */
export function explainNote(pitch: number, chords: ChordEvent[], key: KeyState, tick: number, moving = false): string {
  const why = explainPitch(pitch, chords, key, tick);
  if (!moving) return why;
  if (!chordAt(chords, tick) && !keyPitchClasses(key)) return `${why}. It bends.`;
  return `${why.endsWith(".") ? why : `${why}.`} It bends — judged where it starts.`;
}

function explainPitch(pitch: number, chords: ChordEvent[], key: KeyState, tick: number): string {
  const chord = chordAt(chords, tick);
  const keyPcs = keyPitchClasses(key);
  const pc = normalize(pitch);
  const name = noteName(pc);
  if (!chord) {
    if (!keyPcs) return "no chord or key yet — anything goes";
    return keyPcs.includes(pc)
      ? `${name} is in ${keyName(key.rootPitchClass!, key.scale)}`
      : `${name} is outside ${keyName(key.rootPitchClass!, key.scale)} — flagged, not forbidden`;
  }
  const label = chord.name ?? chordName(chord.pitchClasses);
  const root = chordRoot(chord);
  const interval = INTERVAL_NAMES[normalize(pc - root)];
  const t = tier(pc, chord.pitchClasses, keyPcs);
  if (t === "chordTone") return `${name} over ${label}: ${interval}. Solid.`;
  if (t === "tension") return `${name} over ${label}: ${interval}. Spicy but good.`;
  // Dissonance: say which chord tone it rubs against and where it wants to go.
  const above = chord.pitchClasses.map(normalize).find((c) => normalize(pc - c) === 1);
  const below = chord.pitchClasses.map(normalize).find((c) => normalize(c - pc) === 1);
  if (above !== undefined) return `${name} over ${label}: a half step above ${noteName(above)}. It wants to fall to ${noteName(above)} — or stay, on purpose.`;
  if (below !== undefined) return `${name} over ${label}: a half step below ${noteName(below)}. It leans up into ${noteName(below)} — or stays, on purpose.`;
  return `${name} over ${label}: outside the key. Flagged, not forbidden.`;
}
