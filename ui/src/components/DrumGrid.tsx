import { reportCursor } from "../collab";
import { PeerCursors } from "./PeerCursors";
import { useEffect, useMemo, useRef, useState } from "react";
import { barTicks, gridLabel, selectedPattern, selectedTrack, selectedTrackIndex, useStore } from "../store";
import { matchingKit, padName } from "../drumKit";
import { NOTE_PROPS, noteProp, TICKS_PER_BEAT, trackColor, type NoteEvent, type NoteProp, uuid } from "../types";
import {
  clearRow,
  euclideanRow,
  fillEverySteps,
  hitsInSteps,
  moveHitsByRow,
  moveNotes,
  pasteNotes,
  quantizeNoteProp,
  rampNoteProp,
  randomizeRow,
  removeNotes,
  rotateRow,
  seededRng,
  selectionBounds,
  setNoteProp,
  stepIndexOf,
  tileGhost,
  type StepGrid,
} from "../noteEditing";
import { BASE_BEAT_W } from "./ChordLane";
import { AddPadPicker, SwapPadPicker } from "./AddPadPicker";
import { KitPicker } from "./KitPicker";
import { LoopPicker } from "./LoopPicker";
import { keys } from "../platform";

/** Geometry shared with the CSS: `.drumrow .dname` width, `.drumcells` gap and left padding. */
const GUTTER = 64;
const CELL_GAP = 2;
const CELL_PAD = 1;
/** `.drumgraph` height, and the breathing room the lane keeps above and below its range. */
const GRAPH_H = 96;
const GRAPH_PAD = 10;

const NO_NOTES: NoteEvent[] = [];
const PROPS = Object.keys(NOTE_PROPS) as NoteProp[];

const numStyle: React.CSSProperties = {
  width: 36,
  fontFamily: "var(--font-mono)",
  fontSize: 10,
  padding: "2px 4px",
  background: "var(--bg-control)",
  color: "var(--text-1)",
  border: "1px solid var(--line-1)",
  borderRadius: 3,
};

/**
 * Step sequencer for a drum track, aligned to the chord lane: one row per kit pad, top to
 * bottom. Left-click/drag adds hits, right-click/drag removes; click a row name to hear the
 * pad, ▾ beside it to swap its sound (the hits stay); "+ add sound" under the rows grows the
 * kit. Hits are ordinary notes (pitch = pad pitch), which is why the graph editor below the
 * grid edits exactly the properties the piano roll's lane does — in FL a step *is* a note, and
 * here it already was one. The grid cell stays the step's identity; `shift` only leans how it
 * plays, so a dragged-early hat still lives in its own cell. Hits on pitches the kit has no pad
 * for aren't drawn. A ghost pattern shows as outlined steps (another drum pattern) or an onset
 * row (a melodic one).
 */
export function DrumGrid() {
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const idx = useStore(selectedTrackIndex);
  const playhead = useStore((s) => s.playhead);
  const noteLength = useStore((s) => s.noteLength);
  const zoom = useStore((s) => s.zoom);
  const dispatch = useStore((s) => s.dispatch);
  const audition = useStore((s) => s.audition);
  const laneProp = useStore((s) => s.laneProp);
  const setLaneProp = useStore((s) => s.setLaneProp);
  const defaultVelocity = useStore((s) => s.settings.editing.defaultVelocity);
  const ghostPatternId = useStore((s) => s.ghostPatternId);
  const ticksPerBar = useStore(barTicks);
  const clipboard = useStore((s) => s.noteClipboard);
  const setClipboard = useStore((s) => s.setNoteClipboard);
  const setSelectedNoteIds = useStore((s) => s.setSelectedNoteIds);
  const ghostPattern = useStore((s) => s.snapshot?.model.patterns.find((p) => p.id === ghostPatternId) ?? null);
  const ghostTrack = useStore((s) => (ghostPattern ? (s.snapshot?.model.tracks.find((t) => t.id === ghostPattern.trackId) ?? null) : null));
  const ghostIdx = useStore((s) => (ghostTrack ? (s.snapshot?.model.tracks.indexOf(ghostTrack) ?? 0) : 0));
  const projectSounds = useStore((s) => s.snapshot?.model.sounds);
  const dragMode = useRef<"add" | "remove" | null>(null);
  const [working, setWorking] = useState<NoteEvent[] | null>(null);
  /** Which kit row the graph editor and the step tools act on; null until one is picked. */
  const [rowPitch, setRowPitch] = useState<number | null>(null);
  const [euclid, setEuclid] = useState({ onsets: 5, rotation: 0 });
  const graphRef = useRef<HTMLCanvasElement>(null);
  /** Hits picked with a marquee (⇧/⌘-drag): what copy, cut, nudge and delete act on. */
  const [selection, setSelection] = useState<Set<string>>(() => new Set());
  /** Where the next paste lands: the last cell the pointer touched. */
  const [anchorStep, setAnchorStep] = useState(0);
  const gridRef = useRef<HTMLDivElement>(null);
  const rowEls = useRef(new Map<number, HTMLDivElement>());
  const marquee = useRef<{ x0: number; y0: number; add: boolean; base: Set<string> } | null>(null);
  const [marqueeBox, setMarqueeBox] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  const graphDrag = useRef<{ base: NoteEvent[]; ramp: boolean; anchor: { step: number; value: number } } | null>(null);
  /** Names the pending gesture in the Edit menu; one gesture, one undo step. */
  const commitLabel = useRef("edit drum steps");

  const color = track ? trackColor(track, idx) : "#8a8a93";
  const committed = (pattern ? pattern.notes : undefined) ?? NO_NOTES;
  const notes = working ?? committed;
  const pxPerTick = (BASE_BEAT_W * zoom) / TICKS_PER_BEAT;
  // The step is the snap, whatever the snap is: a 1/8T step grid is what writing a triplet
  // shuffle needs, and a step never falls outside the pattern.
  const totalTicks = pattern?.lengthTicks ?? 4 * TICKS_PER_BEAT;
  const stepTicks = Math.max(1, Math.min(totalTicks, Math.round(noteLength)));
  /** The grid spans the whole pattern: one step per `stepTicks`, the last one short if it must be. */
  const steps = Math.max(1, Math.ceil(totalTicks / stepTicks));
  const cellW = stepTicks * pxPerTick - 2;
  const pitchW = cellW + CELL_GAP;
  const currentStep = Math.floor((playhead % totalTicks) / stepTicks);
  const graphW = GUTTER + CELL_PAD + steps * pitchW;
  const spec = NOTE_PROPS[laneProp];

  // A kit the pick is no longer in (a pad was swapped out, or nothing is picked yet) falls back
  // to the top row, so the graph editor always has a row.
  const pads = track?.drumPads ?? [];
  const padPitch = pads.some((p) => p.pitch === rowPitch) ? rowPitch! : (pads[0]?.pitch ?? 0);
  const grid = useMemo<StepGrid>(() => ({ pitch: padPitch, stepTicks, steps }), [padPitch, stepTicks, steps]);

  /** The selected row's note in each step slot, or null where the slot is empty. */
  const cellsOf = (list: NoteEvent[]): (NoteEvent | null)[] => {
    const out: (NoteEvent | null)[] = Array.from({ length: steps }, () => null);
    for (const n of list) {
      const i = stepIndexOf(n, grid);
      if (i !== null && i >= 0 && i < steps) out[i] = n;
    }
    return out;
  };
  const rowCells = useMemo(() => cellsOf(notes), [notes, grid, steps]);

  // ─── Graph editor canvas ────────────────────────────────────────────────────────────────
  useEffect(() => {
    const canvas = graphRef.current;
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(graphW * dpr) || canvas.height !== Math.round(GRAPH_H * dpr)) {
      canvas.width = Math.round(graphW * dpr);
      canvas.height = Math.round(GRAPH_H * dpr);
      canvas.style.width = `${graphW}px`;
      canvas.style.height = `${GRAPH_H}px`;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.textBaseline = "middle";
    ctx.font = '400 8.5px "JetBrains Mono", ui-monospace, monospace';
    ctx.fillStyle = "#0c0c0f";
    ctx.fillRect(0, 0, graphW, GRAPH_H);

    const yFor = (v: number) =>
      GRAPH_H - GRAPH_PAD - ((v - spec.min) / (spec.max - spec.min)) * (GRAPH_H - 2 * GRAPH_PAD);
    // Bipolar properties grow from the lane centre, unipolar ones from the floor.
    const floor = Math.round(yFor(spec.bipolar ? spec.neutral : spec.min));
    ctx.fillStyle = "#1c1c22";
    ctx.fillRect(GUTTER, floor, graphW - GUTTER, 1);

    for (let s = 0; s < steps; s++) {
      const x = GUTTER + CELL_PAD + s * pitchW;
      if (s === currentStep) {
        ctx.fillStyle = "rgba(255,255,255,.05)";
        ctx.fillRect(x, 0, cellW, GRAPH_H);
      }
      const n = rowCells[s];
      if (!n) {
        // Nothing to edit here: a dead tick, so the empty slot reads as empty rather than zero.
        ctx.fillStyle = "#17171c";
        ctx.fillRect(x, floor - 1, cellW, 3);
        continue;
      }
      const y = Math.round(yFor(noteProp(n, laneProp)));
      ctx.fillStyle = color;
      ctx.globalAlpha = 0.7;
      ctx.fillRect(x, Math.min(y, floor), cellW, Math.max(1, Math.abs(y - floor)));
      ctx.globalAlpha = 1;
      ctx.fillRect(x, y - 1, cellW, 2);
    }

    // Scale column: the property's own range, read against the bars beside it.
    ctx.fillStyle = "#101013";
    ctx.fillRect(0, 0, GUTTER, GRAPH_H);
    ctx.fillStyle = "#23232a";
    ctx.fillRect(GUTTER - 1, 0, 1, GRAPH_H);
    ctx.fillStyle = "#4a4a52";
    const right = (text: string, y: number) => ctx.fillText(text, GUTTER - 8 - ctx.measureText(text).width, y);
    right(spec.format(spec.max), GRAPH_PAD);
    if (spec.bipolar) right(spec.format(spec.neutral), floor);
    right(spec.format(spec.min), GRAPH_H - GRAPH_PAD);
  }, [rowCells, laneProp, spec, steps, cellW, pitchW, graphW, currentStep, color]);

  const ghostNotes = useMemo(
    () => (ghostPattern && ghostPattern.id !== pattern?.id ? tileGhost(ghostPattern.notes, ghostPattern.lengthTicks, totalTicks) : []),
    [ghostPattern, pattern?.id, totalTicks],
  );

  // The marquee's pick lives here, but the status bar and the inspector read the selection from
  // the store — the same arrangement the piano roll has. A different pattern starts empty.
  useEffect(() => {
    setSelectedNoteIds([...selection]);
  }, [selection, setSelectedNoteIds]);
  useEffect(() => {
    setSelection(new Set());
  }, [pattern?.id]);

  if (!pattern || !track) return null;
  const ghostColor = ghostTrack ? trackColor(ghostTrack, ghostIdx) : "var(--text-4)";
  const selectedPad = track.drumPads.find((p) => p.pitch === padPitch) ?? track.drumPads[0];
  const selectedPadName = padName(projectSounds ?? [], selectedPad);
  // Ghost lookups per step: kit hits by pitch (a drum ghost), or onsets (a melodic ghost).
  const ghostIsDrum = !!ghostTrack?.isDrum;
  const stepOf = (tick: number) => Math.floor(tick / stepTicks);
  const ghostHits = new Set(ghostIsDrum ? ghostNotes.map((n) => `${n.pitch}:${stepOf(n.startTick)}`) : []);
  const ghostOnsets = new Map<number, number>();
  if (!ghostIsDrum) for (const n of ghostNotes) ghostOnsets.set(stepOf(n.startTick), (ghostOnsets.get(stepOf(n.startTick)) ?? 0) + 1);

  const find = (list: NoteEvent[], pitch: number, step: number) =>
    list.find((n) => n.pitch === pitch && n.startTick === step * stepTicks);

  const commit = () => {
    if (working) {
      void dispatch({ type: "setNotes", patternId: pattern.id, notes: working }, false, commitLabel.current);
      setWorking(null);
    }
    dragMode.current = null;
    graphDrag.current = null;
    marquee.current = null;
    setMarqueeBox(null);
  };

  /** One-shot step tool: a whole new row in a single command, so it is a single undo step. */
  const run = (next: NoteEvent[], label: string) => {
    if (next === committed) return;
    void dispatch({ type: "setNotes", patternId: pattern.id, notes: next }, false, label);
  };

  const paint = (pitch: number, step: number) => {
    const mode = dragMode.current;
    if (!mode) return;
    setWorking((prev) => {
      const cur = prev ?? committed;
      const existing = find(cur, pitch, step);
      if (mode === "add" && !existing) {
        audition(pitch);
        // The last step of an odd grid (a dotted or triplet snap under a 4-bar pattern) is
        // short: its hit ends with the pattern rather than hanging past the loop.
        return [...cur, { id: uuid(), startTick: step * stepTicks, lengthTicks: Math.min(stepTicks, totalTicks - step * stepTicks), pitch, velocity: defaultVelocity }];
      }
      if (mode === "remove" && existing) return cur.filter((n) => n !== existing);
      return cur;
    });
  };

  // ─── Selection, clipboard, nudges ───────────────────────────────────────────────────────
  // Writing drums is repetition: bar 1's hits belong on bar 3 too. The marquee picks them and
  // every command below is a pure helper from `noteEditing`, so a copy or a nudge means the
  // same thing here as in the piano roll — and the clipboard is the roll's, so hits carry over.

  const padPitches = track.drumPads.map((p) => p.pitch);

  /** Tell everyone where the pointer is: which pad's row, and how far along the pattern. */
  const reportDrumCursor = (e: React.PointerEvent) => {
    for (const pitch of padPitches) {
      const r = rowEls.current.get(pitch)?.getBoundingClientRect();
      if (!r || e.clientY < r.top || e.clientY >= r.bottom) continue;
      const x = e.clientX - r.left - GUTTER - CELL_PAD;
      reportCursor(x < 0 ? null : { tick: (x / pitchW) * stepTicks, pitch, frac: (e.clientY - r.top) / r.height });
      return;
    }
    reportCursor(null);
  };

  /** Pointer rectangle (client px) → the hits it covers, row by row. */
  const hitsUnder = (x0: number, y0: number, x1: number, y1: number): Set<string> => {
    const top = Math.min(y0, y1);
    const bottom = Math.max(y0, y1);
    const covered: number[] = [];
    let cellsLeft: number | null = null;
    for (const pitch of padPitches) {
      const el = rowEls.current.get(pitch);
      if (!el) continue;
      const r = el.getBoundingClientRect();
      if (cellsLeft === null) cellsLeft = r.left + GUTTER + CELL_PAD;
      if (r.bottom >= top && r.top <= bottom) covered.push(pitch);
    }
    if (cellsLeft === null || !covered.length) return new Set();
    const first = Math.floor((Math.min(x0, x1) - cellsLeft) / pitchW);
    const last = Math.floor((Math.max(x0, x1) - cellsLeft) / pitchW);
    return hitsInSteps(notes, covered, stepTicks, Math.max(0, first), Math.min(steps - 1, last));
  };

  /** The marquee rectangle in the grid's own (scrolled) coordinates, for drawing it. */
  const boxOf = (x0: number, y0: number, x1: number, y1: number) => {
    const el = gridRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return {
      left: Math.min(x0, x1) - r.left + el.scrollLeft,
      top: Math.min(y0, y1) - r.top + el.scrollTop,
      width: Math.abs(x1 - x0),
      height: Math.abs(y1 - y0),
    };
  };

  const startMarquee = (e: React.PointerEvent, add: boolean) => {
    gridRef.current?.setPointerCapture(e.pointerId);
    const base = add ? new Set(selection) : new Set<string>();
    marquee.current = { x0: e.clientX, y0: e.clientY, add, base };
    setSelection(base);
    setMarqueeBox(boxOf(e.clientX, e.clientY, e.clientX, e.clientY));
  };

  const dragMarquee = (e: React.PointerEvent) => {
    const m = marquee.current;
    if (!m) return;
    setMarqueeBox(boxOf(m.x0, m.y0, e.clientX, e.clientY));
    const picked = hitsUnder(m.x0, m.y0, e.clientX, e.clientY);
    setSelection(m.add ? new Set([...m.base, ...picked]) : picked);
  };

  const copySelection = (cut: boolean) => {
    const picked = notes.filter((n) => selection.has(n.id));
    if (!picked.length) return;
    setClipboard(picked);
    if (cut) {
      run(removeNotes(notes, selection), "cut hits");
      setSelection(new Set());
    }
  };

  /** Paste the clipboard so its first hit lands on `step`; copies past the pattern are dropped. */
  const pasteAt = (step: number) => {
    if (!clipboard.length) return;
    const { notes: next, newIds } = pasteNotes(notes, clipboard, Math.max(0, Math.min(steps - 1, step)) * stepTicks, totalTicks);
    if (next === notes) return;
    run(next, "paste hits");
    setSelection(newIds);
  };

  /** The selection again, one selection-span later: the bar-1-to-bar-2 move, in one key. */
  const duplicateSelection = () => {
    const bounds = selectionBounds(notes, selection);
    if (!bounds) return;
    const span = Math.max(stepTicks, Math.ceil((bounds.end - bounds.start) / stepTicks) * stepTicks);
    const { notes: next, newIds } = pasteNotes(notes, notes.filter((n) => selection.has(n.id)), bounds.start + span, totalTicks);
    if (next === notes) return;
    run(next, "duplicate hits");
    setSelection(newIds);
  };

  const deleteSelection = () => {
    if (!selection.size) return;
    run(removeNotes(notes, selection), "delete hits");
    setSelection(new Set());
  };

  const nudge = (deltaTicks: number) => {
    if (!selection.size) return;
    run(moveNotes(notes, selection, deltaTicks, 0, { min: 0, max: 127 }, totalTicks), "nudge hits");
  };

  /** Up and down move hits between kit rows, which are a list of pads, not a run of pitches. */
  const moveRows = (delta: number) => {
    if (!selection.size) return;
    run(moveHitsByRow(notes, selection, padPitches, delta), "move hits to another pad");
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key;
    // ⌥ belongs to the window manager here: the grid has no alt chord of its own.
    if (e.altKey) return;
    if (k === "Backspace" || k === "Delete") {
      deleteSelection();
    } else if (mod && k.toLowerCase() === "a") {
      const rows = new Set(padPitches);
      setSelection(new Set(notes.filter((n) => rows.has(n.pitch)).map((n) => n.id)));
    } else if (mod && k.toLowerCase() === "c") {
      copySelection(false);
    } else if (mod && k.toLowerCase() === "x") {
      copySelection(true);
    } else if (mod && k.toLowerCase() === "v") {
      pasteAt(anchorStep);
    } else if (mod && k.toLowerCase() === "d") {
      duplicateSelection();
    } else if (k === "Escape") {
      setSelection(new Set());
    } else if (k === "ArrowLeft" || k === "ArrowRight") {
      nudge((k === "ArrowLeft" ? -1 : 1) * (e.shiftKey ? ticksPerBar : stepTicks));
    } else if (k === "ArrowUp" || k === "ArrowDown") {
      moveRows(k === "ArrowUp" ? -1 : 1);
    } else {
      return;
    }
    e.preventDefault();
  };

  // ─── Graph interaction ──────────────────────────────────────────────────────────────────
  /** Which step the pointer is over, and the property value its height stands for. */
  const graphAt = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const step = Math.floor((e.clientX - r.left - GUTTER - CELL_PAD) / pitchW);
    const t = Math.min(1, Math.max(0, (GRAPH_H - GRAPH_PAD - (e.clientY - r.top)) / (GRAPH_H - 2 * GRAPH_PAD)));
    return { step, value: spec.min + t * (spec.max - spec.min) };
  };

  const graphApply = (at: { step: number; value: number }) => {
    const d = graphDrag.current;
    if (!d || at.step < 0 || at.step >= steps) return;
    if (d.ramp) {
      // ⌥: a straight line from where the drag started to where it is now, across the steps between.
      const cells = cellsOf(d.base);
      const lo = Math.min(d.anchor.step, at.step);
      const hi = Math.max(d.anchor.step, at.step);
      const ids = new Set<string>();
      for (let s = lo; s <= hi; s++) {
        const n = cells[s];
        if (n) ids.add(n.id);
      }
      if (!ids.size) return;
      // Quantise the ends first, so the interior is the straight line between the values the
      // end steps really take (the module owns the clamp-and-round rule, not this component).
      const first = quantizeNoteProp(laneProp, d.anchor.step <= at.step ? d.anchor.value : at.value);
      const last = quantizeNoteProp(laneProp, d.anchor.step <= at.step ? at.value : d.anchor.value);
      setWorking(rampNoteProp(d.base, ids, laneProp, first, last));
      return;
    }
    setWorking((prev) => {
      const cur = prev ?? d.base;
      const n = cellsOf(cur)[at.step];
      // Only steps that exist are editable: the lane never conjures a hit.
      return n ? setNoteProp(cur, new Set([n.id]), laneProp, at.value) : cur;
    });
  };

  const onGraphDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const at = graphAt(e);
    if (at.step < 0 || at.step >= steps) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    commitLabel.current = `set step ${spec.label}`;
    graphDrag.current = { base: notes, ramp: e.altKey, anchor: at };
    graphApply(at);
  };

  return (
    // Past eight pads the rows tighten so a full 16-pad kit mostly fits; the grid scrolls past that.
    <div
      className={`drumgrid${track.drumPads.length > 8 ? " dense" : ""}`}
      ref={gridRef}
      tabIndex={0}
      style={{ position: "relative", outline: "none" }}
      onPointerUp={commit}
      onPointerLeave={() => {
        reportCursor(null);
        commit();
      }}
      onPointerMove={(e) => {
        reportDrumCursor(e);
        dragMarquee(e);
      }}
      onKeyDown={onKeyDown}
    >
      <PeerCursors
        pick={(l) => (l.screen === "pattern" && l.patternId === pattern.id ? l.cursor : undefined)}
        place={(c) => {
          const row = rowEls.current.get(c.pitch);
          return row ? { x: GUTTER + CELL_PAD + (c.tick / stepTicks) * pitchW, y: row.offsetTop + c.frac * row.offsetHeight } : null;
        }}
        scroller={gridRef}
        inset={{ left: GUTTER }}
      />
      {marqueeBox && (
        <div
          style={{
            position: "absolute",
            left: marqueeBox.left,
            top: marqueeBox.top,
            width: marqueeBox.width,
            height: marqueeBox.height,
            border: "1px solid var(--text-2)",
            background: "rgba(255,255,255,.06)",
            pointerEvents: "none",
            zIndex: 4,
          }}
        />
      )}
      {ghostPattern && !ghostIsDrum && ghostNotes.length > 0 && (
        <div className="drumrow ghostrow" title={`${ghostPattern.name}: where its notes start · the ghost`}>
          <span className="dname">ghost</span>
          <div className="drumcells">
            {Array.from({ length: steps }, (_, step) => {
              const n = ghostOnsets.get(step) ?? 0;
              return <div key={step} className={`gstep${n ? " on" : ""}`} style={{ width: cellW, background: n ? ghostColor : undefined, opacity: n ? Math.min(0.75, 0.3 + n * 0.15) : undefined }} />;
            })}
          </div>
        </div>
      )}
      {track.drumPads.map((drum) => {
        const picked = drum.pitch === padPitch;
        return (
          <div
            className="drumrow"
            key={drum.pitch}
            ref={(el) => {
              if (el) rowEls.current.set(drum.pitch, el);
              else rowEls.current.delete(drum.pitch);
            }}
            style={picked ? { background: "rgba(255,255,255,.045)" } : undefined}
          >
            <span
              className="dname"
              title="the row the graph editor and the step tools act on"
              style={{
                cursor: "pointer",
                color: picked ? "var(--text-1)" : undefined,
                background: picked ? "var(--bg-control)" : undefined,
                boxShadow: picked ? `inset 3px 0 0 ${color}` : undefined,
              }}
              onPointerDown={(e) => {
                e.preventDefault();
                setRowPitch(drum.pitch);
              }}
            >
              <span className="dlabel" title={`pitch ${drum.pitch} · click to hear`} onClick={() => audition(drum.pitch)}>
                {padName(projectSounds ?? [], drum)}
              </span>
              <SwapPadPicker track={track} pitch={drum.pitch} className="dswap">
                ▾
              </SwapPadPicker>
            </span>
            <div className="drumcells">
              {Array.from({ length: steps }, (_, step) => {
                const hit = find(notes, drum.pitch, step);
                const ghost = !hit && ghostHits.has(`${drum.pitch}:${step}`);
                const onBeat = (step * stepTicks) % TICKS_PER_BEAT === 0;
                // A hit shows how hard it plays, and leans by its shift — at most one cell, so the
                // slot it sits in still reads as the step it is.
                const lean = hit ? Math.max(-1, Math.min(1, noteProp(hit, "shiftTicks") / stepTicks)) : 0;
                const inSelection = !!hit && selection.has(hit.id);
                return (
                  <div
                    key={step}
                    className={`step${hit ? " on" : ""}${onBeat ? " beat" : ""}${step === currentStep ? " now" : ""}${ghost ? " ghost" : ""}`}
                    style={{
                      width: cellW,
                      ...(ghost ? { borderColor: ghostColor } : {}),
                      ...(inSelection ? { boxShadow: "inset 0 0 0 1.5px var(--accent)" } : {}),
                    }}
                    onPointerDown={(e) => {
                      e.preventDefault();
                      gridRef.current?.focus();
                      setAnchorStep(step);
                      // ⇧/⌘-drag picks hits instead of painting them, as the roll's marquee does;
                      // ⇧⌘ adds to what is already picked.
                      if (e.button === 0 && (e.shiftKey || e.metaKey || e.ctrlKey)) {
                        startMarquee(e, e.shiftKey && (e.metaKey || e.ctrlKey));
                        return;
                      }
                      if (selection.size) setSelection(new Set());
                      commitLabel.current = "edit drum steps";
                      dragMode.current = e.button === 2 ? "remove" : "add";
                      paint(drum.pitch, step);
                    }}
                    onPointerEnter={() => paint(drum.pitch, step)}
                  >
                    {hit && (
                      <span
                        className="lean"
                        style={{
                          background: color,
                          opacity: 0.35 + 0.65 * Math.min(1, Math.max(0, (noteProp(hit, "velocity") - 1) / 126)),
                          transform: lean ? `translateX(${(lean * 100).toFixed(1)}%)` : undefined,
                        }}
                      />
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        );
      })}
      <div className="drumrow addrow">
        <AddPadPicker track={track} className="dname dadd">
          + add sound
        </AddPadPicker>
        <span className="dpresets">
          <KitPicker track={track} className="dkits">
            {matchingKit(track.drumPads)?.name ?? "kits"} ▾
          </KitPicker>
          {pattern && (
            <LoopPicker track={track} pattern={pattern} className="dkits">
              loops ▾
            </LoopPicker>
          )}
        </span>
      </div>

      <div className="steptools" style={{ position: "sticky", left: 0 }}>
        <span className="lbl" style={{ color }}>{selectedPadName}</span>
        <select
          className="lanepick"
          value={laneProp}
          title="which per-note property the graph editor below draws (shared with the piano roll's lane)"
          onChange={(e) => setLaneProp(e.target.value as NoteProp)}
        >
          {PROPS.map((p) => (
            <option key={p} value={p}>
              {NOTE_PROPS[p].label}
            </option>
          ))}
        </select>
        <button className="chip tiny" title="a hit every 2 steps" onClick={() => run(fillEverySteps(committed, grid, 2, 0, defaultVelocity), "fill every 2 steps")}>
          fill /2
        </button>
        <button className="chip tiny" title="a hit every 4 steps" onClick={() => run(fillEverySteps(committed, grid, 4, 0, defaultVelocity), "fill every 4 steps")}>
          fill /4
        </button>
        <button className="chip tiny" title="a hit every 8 steps" onClick={() => run(fillEverySteps(committed, grid, 8, 0, defaultVelocity), "fill every 8 steps")}>
          fill /8
        </button>
        <button className="chip tiny" title="empty this row" onClick={() => run(clearRow(committed, grid), `clear ${selectedPadName} steps`)}>
          clear
        </button>
        <button className="chip tiny" title="rotate the row one step earlier" onClick={() => run(rotateRow(committed, grid, -1), "rotate steps left")}>
          ◀
        </button>
        <button className="chip tiny" title="rotate the row one step later" onClick={() => run(rotateRow(committed, grid, 1), "rotate steps right")}>
          ▶
        </button>
        <button
          className="chip tiny"
          title="coin-flip each step"
          onClick={() => run(randomizeRow(committed, grid, 0.5, defaultVelocity, seededRng(Date.now())), "randomize steps")}
        >
          rand
        </button>
        <button
          className="chip tiny"
          title="spread the onsets as evenly as the steps allow"
          onClick={() => run(euclideanRow(committed, grid, euclid.onsets, euclid.rotation, defaultVelocity), "euclidean steps")}
        >
          euclid
        </button>
        <input
          type="number"
          style={numStyle}
          min={0}
          max={steps}
          value={euclid.onsets}
          title="onsets"
          onChange={(e) => setEuclid((p) => ({ ...p, onsets: Math.max(0, Math.min(steps, Math.round(+e.target.value || 0))) }))}
        />
        <input
          type="number"
          style={numStyle}
          min={0}
          max={Math.max(0, steps - 1)}
          value={euclid.rotation}
          title="rotation"
          onChange={(e) => setEuclid((p) => ({ ...p, rotation: Math.max(0, Math.min(steps - 1, Math.round(+e.target.value || 0))) }))}
        />
      </div>

      <div className="steptools" style={{ position: "sticky", left: 0 }}>
        <span className="lbl" style={{ color }}>picked</span>
        <span className="mono" style={{ fontSize: 9.5, color: "var(--text-4)" }}>
          {selection.size ? `${selection.size} hit${selection.size === 1 ? "" : "s"}` : keys("⇧-drag to pick")}
        </span>
        <button className="chip tiny" disabled={!selection.size} title={keys("copy the picked hits (⌘C)")} onClick={() => copySelection(false)}>
          copy
        </button>
        <button className="chip tiny" disabled={!selection.size} title={keys("cut the picked hits (⌘X)")} onClick={() => copySelection(true)}>
          cut
        </button>
        <button className="chip tiny" disabled={!clipboard.length} title={keys("paste at the last cell you touched (⌘V)")} onClick={() => pasteAt(anchorStep)}>
          paste
        </button>
        <button className="chip tiny" disabled={!selection.size} title={keys("the picked hits again, one selection later (⌘D)")} onClick={duplicateSelection}>
          dup
        </button>
        <button className="chip tiny" disabled={!selection.size} title="delete the picked hits (⌫)" onClick={deleteSelection}>
          del
        </button>
        <button className="chip tiny" disabled={!selection.size} title="nudge the picked hits one step earlier (←)" onClick={() => nudge(-stepTicks)}>
          ◀ step
        </button>
        <button className="chip tiny" disabled={!selection.size} title="nudge the picked hits one step later (→)" onClick={() => nudge(stepTicks)}>
          step ▶
        </button>
      </div>

      <div className="drumgraph" style={{ width: graphW }}>
        <canvas
          ref={graphRef}
          onPointerDown={onGraphDown}
          onPointerMove={(e) => {
            if (graphDrag.current) graphApply(graphAt(e));
          }}
          onContextMenu={(e) => e.preventDefault()}
        />
      </div>
      {notes.length === 0 && <div className="help emptyline">no hits yet: click a step to add one. kick on 1 and 3, snare on 2 and 4 is a fine place to start.</div>}
      <div className="help">
        left-click / drag: add hits · right-click / drag: remove · {keys("⇧-drag: pick hits (⇧⌘ adds)")} · step {gridLabel(stepTicks)} follows snap
        {keys(` · ⌘C/⌘X/⌘V/⌘D · ⌫ delete · ← → nudge (⇧ = bar) · ↑ ↓ move pad · graph: drag to set ${spec.label} on ${selectedPadName}, ⌥-drag to ramp`)}
      </div>
    </div>
  );
}
