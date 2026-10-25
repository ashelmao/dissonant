import { useRef, useState } from "react";
import { useStore } from "../store";
import { chordAt, chordName, diatonicChords, noteName, tier } from "../theory";
import type { ChordEvent } from "../types";

export const GUTTER = 64;
export const BASE_BEAT_W = 96;

export interface ChordLaneProps {
  /** Song beat drawn at x = 0. */
  offsetBeats: number;
  /** How many beats of the progression the lane shows. */
  lengthBeats: number;
  /** Pixels per beat. */
  beatW: number;
  /** Show only; the progression is edited in SETUP. */
  readOnly?: boolean;
  /** Shown as an "edit in SETUP" affordance in the gutter. */
  onEditInSetup?: () => void;
}

/**
 * The project chord progression (`model.chords`, song beats) through the window
 * `[offsetBeats, offsetBeats + lengthBeats)`, drawn from x = 0. Editable unless `readOnly`:
 * drag to move, drag the right edge to resize, click to free-build, right-click to delete.
 * Edits are written back with `setChords` in song beats.
 */
export function ChordLane({ offsetBeats, lengthBeats, beatW, readOnly = false, onEditInSetup }: ChordLaneProps) {
  const all = useStore((s) => s.snapshot!.model.chords);
  const key = useStore((s) => s.snapshot!.model.key);
  const mode = useStore((s) => s.mode);
  const playhead = useStore((s) => s.playhead);
  const dispatch = useStore((s) => s.dispatch);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [preview, setPreview] = useState<ChordEvent[] | null>(null);
  const drag = useRef<{ id: string; kind: "move" | "resize"; startX: number; orig: ChordEvent } | null>(null);

  const end = offsetBeats + lengthBeats;
  const chords = preview ?? all;
  const shown = chords.filter((c) => c.startBeat < end && c.startBeat + c.lengthBeats > offsetBeats);
  // The pattern editor's playhead runs inside the pattern; SETUP and SONG run on the song.
  const current = chordAt(all, mode === "pattern" ? offsetBeats + playhead : playhead);
  const root = key.rootPitchClass ?? 0;

  const write = (next: ChordEvent[]) => void dispatch({ type: "setChords", chords: next });

  const onPointerDown = (e: React.PointerEvent, chord: ChordEvent, kind: "move" | "resize") => {
    if (readOnly) return;
    if (e.button === 2) {
      write(chords.filter((c) => c.id !== chord.id));
      return;
    }
    if (e.button !== 0) return;
    e.stopPropagation();
    drag.current = { id: chord.id, kind, startX: e.clientX, orig: chord };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dBeats = Math.round((e.clientX - d.startX) / beatW);
    if (dBeats === 0 && preview === null) return;
    setPreview(
      all.map((c) => {
        if (c.id !== d.id) return c;
        if (d.kind === "move") {
          return { ...c, startBeat: Math.min(Math.max(offsetBeats, d.orig.startBeat + dBeats), end - d.orig.lengthBeats) };
        }
        return { ...c, lengthBeats: Math.min(Math.max(1, d.orig.lengthBeats + dBeats), end - d.orig.startBeat) };
      }),
    );
  };

  const onPointerUp = (e: React.PointerEvent, chord: ChordEvent) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (preview) {
      write(preview);
      setPreview(null);
    } else if (d.kind === "move" && Math.abs(e.clientX - d.startX) < 3) {
      setEditingId(editingId === chord.id ? null : chord.id);
    }
  };

  return (
    <div className="chordlane">
      <div
        className="gut"
        onClick={onEditInSetup}
        style={onEditInSetup ? { cursor: "pointer", flexDirection: "column", gap: 1 } : undefined}
        title={onEditInSetup ? "the progression is edited in SETUP: click to go there" : undefined}
      >
        <span className="cap">chords</span>
        {onEditInSetup && <span className="cap" style={{ fontSize: 8, color: "var(--accent)" }}>edit ›</span>}
      </div>
      <div className="inner" style={{ width: lengthBeats * beatW }}>
        {shown.length === 0 && <span className="hint">{readOnly ? "no chords here yet: add them in SETUP" : "no chords yet: pick a progression, or ＋ chord"}</span>}
        {shown.map((c) => {
          // A chord crossing the window's edge is drawn cut at the edge.
          const s0 = Math.max(c.startBeat, offsetBeats);
          const s1 = Math.min(c.startBeat + c.lengthBeats, end);
          return (
            <div
              key={c.id}
              className={`chordblock${current?.id === c.id ? " current" : ""}`}
              style={{ left: (s0 - offsetBeats) * beatW, width: Math.max(8, (s1 - s0) * beatW - 3), cursor: readOnly ? "default" : undefined }}
              onPointerDown={(e) => onPointerDown(e, c, "move")}
              onPointerMove={onPointerMove}
              onPointerUp={(e) => onPointerUp(e, c)}
              title={readOnly ? `${c.name ?? chordName(c.pitchClasses)} · chords are edited in SETUP` : "click: edit · drag: move · right edge: resize · right-click: delete"}
            >
              {c.name ?? chordName(c.pitchClasses)}
              {!readOnly && <div className="handle" onPointerDown={(e) => onPointerDown(e, c, "resize")} />}
              {!readOnly && editingId === c.id && (
                <ChordEditor
                  chord={c}
                  root={root}
                  scale={key.scale}
                  onChange={(pcs) => write(chords.map((x) => (x.id === c.id ? { ...x, pitchClasses: pcs, name: chordName(pcs) } : x)))}
                  onDelete={() => (setEditingId(null), write(chords.filter((x) => x.id !== c.id)))}
                  onClose={() => setEditingId(null)}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function ChordEditor({
  chord,
  root,
  scale,
  onChange,
  onDelete,
  onClose,
}: {
  chord: ChordEvent;
  root: number;
  scale: "major" | "minor";
  onChange: (pcs: number[]) => void;
  onDelete: () => void;
  onClose: () => void;
}) {
  const pcs = chord.pitchClasses;
  const toggle = (pc: number) => onChange(pcs.includes(pc) ? pcs.filter((p) => p !== pc) : [...pcs, pc].sort((a, b) => a - b));
  const nextDiatonic = () => {
    const diatonic = diatonicChords(root, scale);
    const idx = diatonic.findIndex((d) => d.name === (chord.name ?? chordName(pcs)));
    onChange([...diatonic[(idx + 1 + diatonic.length) % diatonic.length].pitchClasses]);
  };

  return (
    <div className="popover" onPointerDown={(e) => e.stopPropagation()} onPointerUp={(e) => e.stopPropagation()}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="big">{chordName(pcs)}</span>
        <span className="cap">free build</span>
      </div>
      <div className="flabel" style={{ marginTop: 4 }}>tap notes · color = how it fits what's stacked</div>
      <div className="pcgrid">
        {Array.from({ length: 12 }, (_, pc) => {
          const inChord = pcs.includes(pc);
          const fit = inChord ? "chordTone" : pcs.length === 0 ? null : tier(pc, pcs, null);
          return (
            <button key={pc} className={inChord ? "in" : fit === "tension" ? "tension" : fit === "dissonance" ? "dissonance" : ""} onClick={() => toggle(pc)}>
              {noteName(pc)}
              {!inChord && fit === "dissonance" && <span className="flag">!</span>}
              {!inChord && fit === "tension" && <span className="flag">·</span>}
            </button>
          );
        })}
      </div>
      <div className="row" style={{ gap: 4 }}>
        <button onClick={nextDiatonic} style={{ textTransform: "none" }}>next ▸ diatonic</button>
        <button className="quiet" onClick={() => onChange([])}>clear</button>
        <button className="quiet danger" onClick={onDelete}>delete</button>
        <span className="spacer" />
        <button className="solid" onClick={onClose}>done</button>
      </div>
    </div>
  );
}
