// Ropes: a note that slides from another note on the same pattern. The rope is stored on the
// destination (`NoteEvent.slideFrom`), so each note of a chord can slide to its own target.
//
// These mirror the rope helpers in crates/dissonant-core/src/model.rs (`rope_is_live`,
// `tidy_ropes`, `remap_ropes`). The roll draws live and dead ropes from `ropeIsLive` without an
// IPC round trip, and the engine plays from the Rust one, so the two definitions must stay
// identical: change one, change both, and their tests.

import type { NoteEvent } from "./types";

/** Longest slide a rope may ask for, in seconds; the patch's glide knob tops out here too. */
export const MAX_SLIDE_SECS = 2;

/** Where a note really sounds once its shift is applied; never negative (`shifted_start`). */
export function shiftedStart(n: NoteEvent): number {
  return Math.max(0, n.startTick + (n.shiftTicks ?? 0));
}

/**
 * Whether `target`'s rope from `source` actually slides: the source must still be sounding when
 * the target starts. Touching counts, so a legato line drawn on the grid connects. A muted
 * source has no voice to slide out of, and a note can't slide from itself. Equal pitches are
 * fine: the rope then acts as a tie. A rope that isn't live is still kept; the roll draws it
 * dashed and the target plays as an ordinary note.
 */
export function ropeIsLive(source: NoteEvent, target: NoteEvent): boolean {
  if (source.id === target.id || source.muted) return false;
  const from = shiftedStart(source);
  const at = shiftedStart(target);
  return from < at && at <= from + source.lengthTicks;
}

/** `n` with no rope and no slide time. The keys go entirely, so a saved note stays bare. */
export function unroped(n: NoteEvent): NoteEvent {
  const out = { ...n };
  delete out.slideFrom;
  delete out.slideSecs;
  return out;
}

/**
 * One pattern's ropes made valid: a rope whose source isn't in `notes`, that points at its own
 * note, or that closes a loop (A from B from A) is cut, and a slide time left without a rope
 * goes with it. A loop is cut at its first note in array order. Chains (A → B → C) stay.
 * Returns `notes` itself when nothing needed cutting.
 */
export function tidyRopes(notes: NoteEvent[]): NoteEvent[] {
  const index = new Map(notes.map((n, i) => [n.id, i]));
  const out = [...notes];
  let changed = false;
  for (let i = 0; i < out.length; i++) {
    const n = out[i];
    const first = n.slideFrom;
    if (!first) {
      if (n.slideSecs !== undefined) {
        out[i] = unroped(n);
        changed = true;
      }
      continue;
    }
    let cut = !index.has(first);
    if (!cut) {
      // Walk upstream. Reaching this note again is a loop through it; a note seen before is a
      // loop further up, cut when its own first note comes round.
      const seen = new Set<string>();
      let from: string | undefined = first;
      while (from !== undefined) {
        if (from === n.id) {
          cut = true;
          break;
        }
        if (seen.has(from)) break;
        seen.add(from);
        const j = index.get(from);
        from = j === undefined ? undefined : (out[j].slideFrom ?? undefined);
      }
    }
    if (cut) {
      out[i] = unroped(n);
      changed = true;
    }
  }
  return changed ? out : notes;
}

/**
 * Point copied ropes at the copies. `copyOf` maps an original note id to its copy's id; a rope
 * whose source was copied follows it, and one whose source was left behind is dropped with its
 * slide time, since a copy sliding out of the original would tie two unrelated phrases.
 */
export function remapRopes(copies: NoteEvent[], copyOf: Map<string, string>): NoteEvent[] {
  return copies.map((n) => {
    if (!n.slideFrom) return n.slideSecs === undefined ? n : unroped(n);
    const to = copyOf.get(n.slideFrom);
    return to ? { ...n, slideFrom: to } : unroped(n);
  });
}

/**
 * Re-point ropes after notes were cut into pieces or merged. `pieces` maps an old note id to the
 * notes now standing in for it; a rope that slid from the old note moves to whichever piece is
 * sounding when the slide note starts (the last piece that starts before it), so a split never
 * kills a live rope. Run `tidyRopes` afterwards to cut anything a merge turned into a self-rope.
 */
export function repointRopes(notes: NoteEvent[], pieces: Map<string, NoteEvent[]>): NoteEvent[] {
  if (pieces.size === 0) return notes;
  let changed = false;
  const out = notes.map((n) => {
    const parts = n.slideFrom ? pieces.get(n.slideFrom) : undefined;
    if (!parts?.length) return n;
    const at = shiftedStart(n);
    const sounding = [...parts]
      .sort((a, b) => shiftedStart(a) - shiftedStart(b))
      .filter((p) => shiftedStart(p) < at)
      .pop() ?? parts[0];
    if (sounding.id === n.slideFrom) return n;
    changed = true;
    return { ...n, slideFrom: sounding.id };
  });
  return changed ? out : notes;
}
