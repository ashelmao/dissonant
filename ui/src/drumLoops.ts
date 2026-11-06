// Drum loops: the factory library of ready-made patterns. Mirrors `dissonant_core::vibes::
// DrumLoop`; the JSON is shared. Loops are lanes on GM pitches, so they play on any kit that
// follows the GM layout (every factory kit does), and each names the kit it was written for.

import loopsJson from "../../crates/dissonant-core/assets/drum_loops.json";
import { FACTORY_KITS, type KitPreset } from "./drumKit";
import { beatsToTicks, uuid, type NoteEvent } from "./types";

/** One drum row: a step string, one character per `step` beats (`X` accent, `x` hit, `o` ghost,
 *  anything else a rest; spaces are only for reading), repeated to fill the pattern. */
export interface DrumLane {
  pitch: number;
  /** Beats per character; a 16th when absent. */
  step?: number;
  steps: string;
}

export interface LoopPreset {
  id: string;
  name: string;
  blurb: string;
  /** The factory kit it was written for. */
  kit: KitPreset;
  /** The tempo it was written at; loading a loop leaves the song's tempo alone. */
  bpm: number;
  /** 50 straight … 75 hard shuffle, on the 16th lanes. */
  swing: number;
  lanes: DrumLane[];
}

type RawLoop = Omit<LoopPreset, "kit" | "swing"> & { kit: string; swing?: number };

export const FACTORY_LOOPS: LoopPreset[] = (loopsJson as RawLoop[]).map((l) => {
  const kit = FACTORY_KITS.find((k) => k.id === l.kit);
  if (!kit) throw new Error(`drum loop ${l.id}: no factory kit ${l.kit}`);
  return { ...l, kit, swing: l.swing ?? 50 };
});

const VELOCITY: Record<string, number> = { X: 120, x: 96, o: 64 };
const SIXTEENTH = beatsToTicks(0.25);

const cells = (lane: DrumLane) => [...lane.steps].filter((c) => !/\s/.test(c));
const stepTicks = (lane: DrumLane) => beatsToTicks(lane.step ?? 0.25);

/** Ticks in one pass of the loop: its longest lane. */
export function loopLengthTicks(loop: LoopPreset): number {
  return Math.max(0, ...loop.lanes.map((l) => cells(l).length * stepTicks(l)));
}

/** How far a swung off-16th lands late, in ticks. */
export function swingTicks(loop: LoopPreset): number {
  return beatsToTicks((loop.swing / 100) * 0.5) - SIXTEENTH;
}

/**
 * The loop's hits for `length` ticks: every lane repeated to fill it, sorted by time then pitch.
 * Swing goes into `shiftTicks`, so each hit keeps its grid cell. Matches `DrumLoop::hits`.
 */
export function loopHits(loop: LoopPreset, length: number): NoteEvent[] {
  const late = swingTicks(loop);
  const hits: NoteEvent[] = [];
  for (const lane of loop.lanes) {
    const cs = cells(lane);
    const step = stepTicks(lane);
    const period = cs.length * step;
    if (period <= 0) continue;
    for (let at = 0; at < length; at += period) {
      cs.forEach((c, i) => {
        const startTick = at + i * step;
        if (!VELOCITY[c] || startTick >= length) return;
        const hit: NoteEvent = { id: uuid(), startTick, lengthTicks: Math.min(step, length - startTick), pitch: lane.pitch, velocity: VELOCITY[c] };
        if (late && step === SIXTEENTH && Math.floor(startTick / SIXTEENTH) % 2 === 1) hit.shiftTicks = late;
        hits.push(hit);
      });
    }
  }
  return hits.sort((a, b) => a.startTick - b.startTick || a.pitch - b.pitch);
}
