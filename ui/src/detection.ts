// What DI detection left on the model, as the screens read it: the detected chord lane, the
// chroma the KEY cell weighs, and the words a finished analysis is reported in.

import type { DetectedChord, DetectionReport, ProjectModel } from "./types";
import { TICKS_PER_BEAT } from "./types";

/** A detected chord with the clip it was heard in. */
export interface LaneChord extends DetectedChord {
  clipId: string;
}

/**
 * Every chord waiting in the detected lane, in song order. Only detections whose take is
 * still on the song count: deleting a take hides its candidates at once, and the next load
 * drops them for good (`ProjectModel::normalized`).
 */
export function detectedChords(model: ProjectModel): LaneChord[] {
  const live = new Set((model.audioClips ?? []).map((c) => c.id));
  return (model.detections ?? [])
    .filter((d) => live.has(d.clipId))
    .flatMap((d) => d.chords.map((c) => ({ ...c, clipId: d.clipId })))
    .sort((a, b) => a.startTick - b.startTick);
}

/** The chroma of every live detection, summed: what the KEY cell adds to the placed notes. */
export function detectedChroma(model: ProjectModel): number[] {
  const live = new Set((model.audioClips ?? []).map((c) => c.id));
  const sum = Array<number>(12).fill(0);
  for (const d of model.detections ?? []) {
    if (!live.has(d.clipId)) continue;
    for (let i = 0; i < 12; i++) {
      const v = d.chroma[i] ?? 0;
      if (Number.isFinite(v) && v > 0) sum[i] += v;
    }
  }
  return sum;
}

/** "87%" — how closely the heard chroma matched the chord. */
export function confidenceText(confidence: number): string {
  return `${Math.round(Math.min(1, Math.max(0, confidence)) * 100)}%`;
}

/** The toast after an analysis lands: what was written, and what was refused and why. */
export function detectionSummary(report: DetectionReport, trackName: string | null): string {
  const parts: string[] = [];
  if (report.notes > 0) parts.push(`${report.notes} note${report.notes === 1 ? "" : "s"} on ${trackName ?? "a new track"}`);
  else parts.push("no notes");
  const refused = report.refused.length;
  if (refused > 0) {
    const beats = report.refused.reduce((sum, [a, b]) => sum + (b - a), 0) / TICKS_PER_BEAT;
    parts.push(`${refused} strummed region${refused === 1 ? "" : "s"} (${beats.toFixed(1)} beats) refused as notes`);
  }
  if (report.chords > 0) parts.push(`${report.chords} chord${report.chords === 1 ? "" : "s"} to review above the progression`);
  else if (refused > 0) parts.push("no chord recognized in them");
  return parts.join(" · ");
}
