/**
 * Named tunings and the readout the tuner strip shows.
 *
 * The engine answers one question — what frequency is on the input — and everything musical
 * happens here: which string that is in the tuning you picked, how far off it is in cents, and
 * what A4 is worth while you do it. Keeping it in TypeScript means the whole mapping is pure
 * and testable without an audio device, and the detector stays a detector.
 *
 * Naming the *string* rather than the nearest note is the point. "F2, 42 cents sharp" tells a
 * player nothing useful; "low E, 42 sharp" tells them which peg to turn. The nearest string is
 * chosen by cents distance, so a badly flat low E still reads as the low E it is meant to be
 * rather than as the D♯ it has become.
 */

/** One string of a tuning: what it is called and the note it is tuned to. */
export interface TuningString {
  /** What the strip calls it — "low E", "A", "D"… */
  name: string;
  /** MIDI note number of the target, against A4 = 69. */
  midi: number;
}

export interface Tuning {
  id: string;
  name: string;
  /** Lowest string first, the way a guitar is drawn in a chord box read upside down. */
  strings: TuningString[];
}

/** MIDI numbers for the notes tunings are built from, so the tables below read as music. */
const E2 = 40;
const A2 = 45;
const D3 = 50;
const G3 = 55;
const B3 = 59;
const E4 = 64;
const D2 = 38;
const A3 = 57;
const G2 = 43;

/**
 * The tunings the strip offers. Standard, drop D and DADGAD are the ones a guitarist reaches
 * for without thinking; open G is the other one that is a different *instrument* rather than a
 * variation. Bass is here because an armed track with a bass on it is the same problem.
 */
export const TUNINGS: Tuning[] = [
  {
    id: "standard",
    name: "standard",
    strings: [
      { name: "low E", midi: E2 },
      { name: "A", midi: A2 },
      { name: "D", midi: D3 },
      { name: "G", midi: G3 },
      { name: "B", midi: B3 },
      { name: "high E", midi: E4 },
    ],
  },
  {
    id: "dropD",
    name: "drop D",
    strings: [
      { name: "low D", midi: D2 },
      { name: "A", midi: A2 },
      { name: "D", midi: D3 },
      { name: "G", midi: G3 },
      { name: "B", midi: B3 },
      { name: "high E", midi: E4 },
    ],
  },
  {
    id: "dadgad",
    name: "DADGAD",
    strings: [
      { name: "low D", midi: D2 },
      { name: "A", midi: A2 },
      { name: "D", midi: D3 },
      { name: "G", midi: G3 },
      { name: "A", midi: A3 },
      { name: "high D", midi: D3 + 12 },
    ],
  },
  {
    id: "openG",
    name: "open G",
    strings: [
      { name: "low D", midi: D2 },
      { name: "G", midi: G2 },
      { name: "D", midi: D3 },
      { name: "G", midi: G3 },
      { name: "B", midi: B3 },
      { name: "high D", midi: D3 + 12 },
    ],
  },
  {
    id: "halfStepDown",
    name: "half step down",
    strings: [
      { name: "low E♭", midi: E2 - 1 },
      { name: "A♭", midi: A2 - 1 },
      { name: "D♭", midi: D3 - 1 },
      { name: "G♭", midi: G3 - 1 },
      { name: "B♭", midi: B3 - 1 },
      { name: "high E♭", midi: E4 - 1 },
    ],
  },
  {
    id: "bass",
    name: "bass (4-string)",
    strings: [
      { name: "E", midi: E2 - 12 },
      { name: "A", midi: A2 - 12 },
      { name: "D", midi: D3 - 12 },
      { name: "G", midi: G3 - 12 },
    ],
  },
];

export const DEFAULT_TUNING = TUNINGS[0];

/** The tuning with this id, or standard when a settings file names one that has since gone. */
export function tuningById(id: string): Tuning {
  return TUNINGS.find((t) => t.id === id) ?? DEFAULT_TUNING;
}

const NOTE_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];

/** The frequency of a MIDI note, with A4 (69) at `a4Hz`. */
export function noteHz(midi: number, a4Hz: number): number {
  return a4Hz * Math.pow(2, (midi - 69) / 12);
}

/** Cents from `target` to `hz`: positive is sharp. */
export function centsBetween(hz: number, target: number): number {
  return 1200 * Math.log2(hz / target);
}

/** "E2", "A♯3" — the nearest chromatic note, for when nothing in the tuning is close. */
export function noteName(midi: number): string {
  const pc = ((midi % 12) + 12) % 12;
  return `${NOTE_NAMES[pc]}${Math.floor(midi / 12) - 1}`;
}

/** What the strip shows for one reading. */
export interface TunerDisplay {
  /** Which string of the tuning it is nearest, by cents. */
  stringIndex: number;
  /** That string's name, or the nearest chromatic note when nothing is within `maxCents`. */
  label: string;
  /** The note name of whatever is being shown, always — "low E (E2)". */
  note: string;
  /** How far off, in cents; positive is sharp. */
  cents: number;
  /** The frequency being aimed at, in Hz. */
  targetHz: number;
  /** True when the string is close enough that a player would stop turning. */
  inTune: boolean;
  /** No string of the tuning is anywhere near: the note is named instead. */
  offTuning: boolean;
}

/**
 * Within this many cents the needle is green. ±3 is tighter than a guitar holds after a bend
 * and loose enough that the last cent is not a fight with the detector.
 */
export const IN_TUNE_CENTS = 3;

/**
 * Past this the reading is not that string any more, it is a different note, so it is named
 * chromatically rather than pinned to the nearest peg. Half a semitone would mean a string
 * tuned a quarter-tone flat is claimed by its neighbour; 150 cents keeps a string that is a
 * whole semitone out still readable as itself, which is what retuning a guitar looks like.
 */
const MAX_STRING_CENTS = 150;

/**
 * Turn a detected frequency into what the strip shows.
 *
 * Returns `null` for a frequency that is not a frequency — the detector reports nothing as
 * nothing, and the strip has a resting state.
 */
export function tunerDisplay(hz: number | null | undefined, tuning: Tuning, a4Hz: number): TunerDisplay | null {
  if (!hz || !Number.isFinite(hz) || hz <= 0) return null;
  let best = 0;
  let bestCents = Infinity;
  tuning.strings.forEach((s, i) => {
    const c = centsBetween(hz, noteHz(s.midi, a4Hz));
    if (Math.abs(c) < Math.abs(bestCents)) {
      best = i;
      bestCents = c;
    }
  });
  if (Math.abs(bestCents) <= MAX_STRING_CENTS) {
    const string = tuning.strings[best];
    return {
      stringIndex: best,
      label: string.name,
      note: noteName(string.midi),
      cents: bestCents,
      targetHz: noteHz(string.midi, a4Hz),
      inTune: Math.abs(bestCents) <= IN_TUNE_CENTS,
      offTuning: false,
    };
  }
  // Nothing in the tuning is close: name the note instead of lying about a string.
  const midi = Math.round(69 + 12 * Math.log2(hz / a4Hz));
  const target = noteHz(midi, a4Hz);
  const cents = centsBetween(hz, target);
  return {
    stringIndex: -1,
    label: noteName(midi),
    note: noteName(midi),
    cents,
    targetHz: target,
    inTune: Math.abs(cents) <= IN_TUNE_CENTS,
    offTuning: true,
  };
}

/** "12 cents flat" / "in tune" — the sentence under the needle. */
export function tunerAdvice(d: TunerDisplay | null): string {
  if (!d) return "play a string";
  if (d.inTune) return "in tune";
  const n = Math.round(Math.abs(d.cents));
  return `${n} cent${n === 1 ? "" : "s"} ${d.cents > 0 ? "sharp" : "flat"}`;
}

/**
 * "aligned −12 ms" — what the REC page says about the last take.
 *
 * A take that was also head-trimmed says so: the trim threw audio away and the player is owed
 * that detail, even though both halves add up to the same correction.
 */
export function alignmentText(align: { shiftedMs: number; trimmedMs: number } | null | undefined): string | null {
  if (!align) return null;
  const total = align.shiftedMs + align.trimmedMs;
  if (total < 0.05) return "aligned · no correction needed";
  const text = `aligned −${total.toFixed(1)} ms`;
  return align.trimmedMs >= 0.05 ? `${text} · ${align.trimmedMs.toFixed(1)} ms trimmed off the head` : text;
}
