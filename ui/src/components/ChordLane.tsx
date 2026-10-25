import { useRef, useState } from "react";
import { beatsPerBar, useStore } from "../store";
import { chordAt, chordName, diatonicChords, noteName, NOTE_NAMES, tier } from "../theory";
import { keyAt, uuid, type ChordEvent, type ScaleType } from "../types";
import { buildChord, degreeOf, gapAt, identifyChord, lengthText, nameOf, nextChords, placeChord, QUALITIES, rootOfName } from "../songSetup";

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
  /** Lane height in px (30 by default; SETUP draws it tall, with numerals and lengths). */
  height?: number;
  /** Snap for moving, resizing and drawing chords, in beats (default 1). */
  snap?: number;
  /** Controlled selection: when `onSelect` is given the lane opens no popover of its own. */
  selectedId?: string | null;
  onSelect?: (id: string | null) => void;
  /** Drawn behind the chords (bar grid, section colors), in lane coordinates. */
  underlay?: React.ReactNode;
}

type Drag = { kind: "move" | "start" | "end" | "create"; id: string; x0: number; orig: ChordEvent; copy: boolean; moved: boolean };

/**
 * The project chord progression (`model.chords`, song beats) through the window
 * `[offsetBeats, offsetBeats + lengthBeats)`, drawn from x = 0. Editable unless `readOnly`:
 * click a chord to edit it, drag to move (⌥ copies), drag either edge to resize, drag on empty
 * lane to draw a new one (a plain click adds a bar's worth), right-click or Delete removes.
 * Whatever a moved or drawn chord lands on is trimmed back, so the progression never overlaps.
 * Each edit is one `setChords` in song beats.
 */
export function ChordLane({
  offsetBeats,
  lengthBeats,
  beatW,
  readOnly = false,
  onEditInSetup,
  height = 30,
  snap = 1,
  selectedId,
  onSelect,
  underlay,
}: ChordLaneProps) {
  const model = useStore((s) => s.snapshot!.model);
  const all = model.chords;
  const mode = useStore((s) => s.mode);
  const playhead = useStore((s) => s.playhead);
  const dispatch = useStore((s) => s.dispatch);
  const bpb = useStore(beatsPerBar);
  const [ownSel, setOwnSel] = useState<string | null>(null);
  const [preview, setPreview] = useState<ChordEvent[] | null>(null);
  const drag = useRef<Drag | null>(null);
  const innerRef = useRef<HTMLDivElement>(null);

  const controlled = onSelect !== undefined;
  const sel = controlled ? (selectedId ?? null) : ownSel;
  const select = (id: string | null) => (controlled ? onSelect(id) : setOwnSel(id));

  const end = offsetBeats + lengthBeats;
  const chords = preview ?? all;
  const shown = chords.filter((c) => c.startBeat < end && c.startBeat + c.lengthBeats > offsetBeats);
  // The pattern editor's playhead runs inside the pattern; SETUP and SONG run on the song.
  const current = chordAt(all, mode === "pattern" ? offsetBeats + playhead : playhead);
  const tall = height >= 44;

  const write = (next: ChordEvent[]) => void dispatch({ type: "setChords", chords: next });
  const remove = (id: string) => {
    if (sel === id) select(null);
    write(all.filter((c) => c.id !== id));
  };
  const snapTo = (beat: number) => Math.round(beat / snap) * snap;
  const beatAt = (clientX: number) => offsetBeats + (clientX - innerRef.current!.getBoundingClientRect().left) / beatW;

  /** What a freshly drawn chord at `beat` holds: the usual next step from the chord before it, else the key's I. */
  const freshChord = (beat: number): Pick<ChordEvent, "pitchClasses" | "name"> => {
    const k = keyAt(model, beat);
    const root = k.rootPitchClass ?? 0;
    const before = [...all].reverse().find((c) => c.startBeat + c.lengthBeats <= beat + 1e-9);
    const from = before ? degreeOf(before.pitchClasses, root, k.scale) : null;
    const pick = from === null ? diatonicChords(root, k.scale)[0] : nextChords(from, root, k.scale)[0];
    return { pitchClasses: [...pick.pitchClasses].sort((a, b) => a - b), name: pick.name };
  };

  const onBlockDown = (e: React.PointerEvent, chord: ChordEvent, kind: "move" | "start" | "end") => {
    if (readOnly) return;
    e.stopPropagation();
    if (e.button === 2) {
      remove(chord.id);
      return;
    }
    if (e.button !== 0) return;
    innerRef.current!.focus({ preventScroll: true });
    innerRef.current!.setPointerCapture(e.pointerId);
    drag.current = { kind, id: chord.id, x0: e.clientX, orig: chord, copy: kind === "move" && e.altKey, moved: false };
  };

  const onLaneDown = (e: React.PointerEvent) => {
    if (readOnly || e.button !== 0) return;
    innerRef.current!.focus({ preventScroll: true });
    const beat = beatAt(e.clientX);
    const gap = gapAt(all, beat, end);
    const start = Math.max(gap.start, Math.floor(beat / snap) * snap);
    if (start >= gap.end - 1e-9) return;
    innerRef.current!.setPointerCapture(e.pointerId);
    // A plain click fills up to a bar of the gap; dragging sets the length.
    const len = Math.min(Math.max(snap, bpb), gap.end - start);
    const orig: ChordEvent = { id: uuid(), startBeat: start, lengthBeats: len, ...freshChord(start) };
    drag.current = { kind: "create", id: orig.id, x0: e.clientX, orig, copy: false, moved: false };
    setPreview(placeChord(all, orig));
  };

  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    if (!d.moved && Math.abs(e.clientX - d.x0) < 3) return;
    d.moved = true;
    const o = d.orig;
    const oEnd = o.startBeat + o.lengthBeats;
    const delta = (e.clientX - d.x0) / beatW;
    let next: ChordEvent;
    if (d.kind === "move") {
      const start = Math.min(Math.max(offsetBeats, snapTo(o.startBeat + delta)), end - o.lengthBeats);
      next = { ...o, id: d.copy ? d.id + ":copy" : o.id, startBeat: start };
    } else if (d.kind === "end") {
      next = { ...o, lengthBeats: Math.min(Math.max(snap, snapTo(oEnd + delta) - o.startBeat), end - o.startBeat) };
    } else if (d.kind === "start") {
      const start = Math.min(Math.max(offsetBeats, snapTo(o.startBeat + delta)), oEnd - snap);
      next = { ...o, startBeat: start, lengthBeats: oEnd - start };
    } else {
      // Drawing: from the anchor to the pointer, either direction, at least one snap step.
      const at = beatAt(e.clientX);
      const a = Math.max(offsetBeats, Math.min(o.startBeat, Math.floor(at / snap) * snap));
      const b = Math.min(end, Math.max(o.startBeat + snap, Math.ceil(at / snap) * snap));
      next = { ...o, startBeat: a, lengthBeats: b - a };
    }
    setPreview(placeChord(d.copy ? all : all.filter((c) => c.id !== o.id), next));
  };

  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    const result = preview;
    setPreview(null);
    if (d.kind === "create") {
      if (!result) return;
      write(result);
      select(d.id);
    } else if (d.moved && result) {
      // A copy gets a real id once it lands.
      const fresh = d.copy ? uuid() : null;
      write(fresh ? result.map((c) => (c.id === d.id + ":copy" ? { ...c, id: fresh } : c)) : result);
      select(fresh ?? d.id);
    } else if (!d.moved) {
      select(controlled ? d.id : sel === d.id ? null : d.id);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (readOnly) return;
    const i = all.findIndex((c) => c.id === sel);
    if ((e.key === "Delete" || e.key === "Backspace") && i >= 0) {
      e.preventDefault();
      remove(all[i].id);
    } else if (e.key === "Escape") {
      select(null);
    } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      const j = i < 0 ? 0 : Math.min(all.length - 1, Math.max(0, i + (e.key === "ArrowRight" ? 1 : -1)));
      if (all[j]) select(all[j].id);
    }
  };

  // Without a controlled selection the lane edits in its own popover.
  const editing = !controlled && !readOnly ? shown.find((c) => c.id === sel) : undefined;

  return (
    <div className={`chordlane${tall ? " tall" : ""}${editing ? " editing" : ""}`} style={height !== 30 ? { height } : undefined}>
      <div
        className="gut"
        onClick={onEditInSetup}
        style={onEditInSetup ? { cursor: "pointer", flexDirection: "column", gap: 1 } : undefined}
        title={onEditInSetup ? "the progression is edited in SETUP: click to go there" : undefined}
      >
        <span className="cap">chords</span>
        {onEditInSetup && <span className="cap" style={{ fontSize: 8, color: "var(--accent)" }}>edit ›</span>}
      </div>
      <div
        className="inner"
        ref={innerRef}
        tabIndex={readOnly ? undefined : 0}
        style={{ width: lengthBeats * beatW, height, cursor: readOnly ? undefined : "copy" }}
        onPointerDown={onLaneDown}
        onPointerMove={onMove}
        onPointerUp={onUp}
        onPointerCancel={onUp}
        onKeyDown={onKeyDown}
        aria-label={readOnly ? "chord progression" : "chord progression: click or drag on empty space to add a chord, arrows pick one, Delete removes it"}
      >
        {underlay}
        {shown.length === 0 && (
          <span className="hint">{readOnly ? "no chords here yet: add them in SETUP" : "no chords yet. pick a starter, or click the lane to build one"}</span>
        )}
        {shown.map((c) => {
          // A chord crossing the window's edge is drawn cut at the edge.
          const s0 = Math.max(c.startBeat, offsetBeats);
          const s1 = Math.min(c.startBeat + c.lengthBeats, end);
          const name = c.name ?? chordName(c.pitchClasses);
          const k = tall ? keyAt(model, c.startBeat) : null;
          const deg = k && k.rootPitchClass !== null ? degreeOf(c.pitchClasses, k.rootPitchClass, k.scale) : null;
          const roman = deg !== null ? diatonicChords(k!.rootPitchClass!, k!.scale)[deg].romanNumeral : null;
          const w = Math.max(8, (s1 - s0) * beatW - 3);
          return (
            <div
              key={c.id}
              className={`chordblock${current?.id === c.id ? " current" : ""}${sel === c.id ? " sel" : ""}${roman === null && k?.rootPitchClass != null ? " outside" : ""}`}
              style={{ left: (s0 - offsetBeats) * beatW, width: w, ...(height !== 30 ? { height: height - 6 } : null), cursor: readOnly ? "default" : undefined }}
              onPointerDown={(e) => onBlockDown(e, c, "move")}
              role={readOnly ? undefined : "button"}
              aria-label={`${name}${roman ? `, ${roman}` : ""}, ${lengthText(c.lengthBeats, bpb)}`}
              title={readOnly ? `${name} · chords are edited in SETUP` : `${name} · click: edit · drag: move (⌥ copies) · edges: resize · right-click: delete`}
            >
              <span className="nm">{name}</span>
              {tall && w > 34 && (
                <span className="sub">
                  {roman ?? (k?.rootPitchClass != null ? "outside" : "")}
                  {w > 70 && <span className="len">{lengthText(c.lengthBeats, bpb)}</span>}
                </span>
              )}
              {!readOnly && <div className="handle l" onPointerDown={(e) => onBlockDown(e, c, "start")} />}
              {!readOnly && <div className="handle" onPointerDown={(e) => onBlockDown(e, c, "end")} />}
            </div>
          );
        })}
        {editing && (
          <div
            className="popover"
            style={{ left: (Math.max(editing.startBeat, offsetBeats) - offsetBeats) * beatW, top: height - 2 }}
            onPointerDown={(e) => e.stopPropagation()}
            onPointerUp={(e) => e.stopPropagation()}
          >
            <ChordBuilder key={editing.id} chord={editing} onChange={(patch) => write(all.map((x) => (x.id === editing.id ? { ...x, ...patch } : x)))} />
            <div className="row" style={{ gap: 4, marginTop: 6 }}>
              <button className="quiet danger" onClick={() => remove(editing.id)}>delete</button>
              <span className="spacer" />
              <button className="solid" onClick={() => select(null)}>done</button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Edits one chord: take one of the key's own chords, pick a root and a quality, or free-build
 * note by note (each note colored by how it fits what's already stacked). A typed name sticks
 * until the notes change.
 */
export function ChordBuilder({ chord, onChange }: { chord: ChordEvent; onChange: (patch: Pick<ChordEvent, "pitchClasses" | "name">) => void }) {
  const model = useStore((s) => s.snapshot!.model);
  const k = keyAt(model, chord.startBeat);
  const keyRoot = k.rootPitchClass ?? 0;
  const scale: ScaleType = k.scale;
  const pcs = chord.pitchClasses;
  const named = rootOfName(chord.name);
  const id = identifyChord(pcs, named);
  const root = id?.root ?? named ?? pcs[0] ?? keyRoot;
  const [nameDraft, setNameDraft] = useState<string | null>(null);

  const setPcs = (next: number[], name?: string) => onChange({ pitchClasses: next, name: name ?? (next.length ? nameOf(next, root) : null) });
  const toggle = (pc: number) => setPcs(pcs.includes(pc) ? pcs.filter((p) => p !== pc) : [...pcs, pc].sort((a, b) => a - b));
  const diatonic = diatonicChords(keyRoot, scale);
  const deg = degreeOf(pcs, keyRoot, scale);
  const commitName = () => {
    if (nameDraft === null) return;
    const t = nameDraft.trim();
    onChange({ pitchClasses: pcs, name: t || (pcs.length ? nameOf(pcs, root) : null) });
    setNameDraft(null);
  };

  return (
    <div className="chordbuilder">
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="big">{chord.name ?? chordName(pcs)}</span>
        <span className="cap">
          {deg !== null ? `${diatonic[deg].romanNumeral} in ${noteName(keyRoot)} ${scale}` : k.rootPitchClass !== null ? `outside ${noteName(keyRoot)} ${scale}` : "no key set"}
        </span>
      </div>

      <span className="flabel">in key</span>
      <div className="cb-diatonic">
        {diatonic.map((d, i) => (
          <button key={d.name} className={deg === i ? "on" : ""} onClick={() => setPcs([...d.pitchClasses].sort((a, b) => a - b), d.name)} title={`${d.romanNumeral}: ${d.name}`}>
            <span className="rn">{d.romanNumeral}</span>
            <span>{d.name}</span>
          </button>
        ))}
      </div>

      <span className="flabel">root · quality</span>
      <div className="cb-roots">
        {NOTE_NAMES.map((n, pc) => (
          <button
            key={n}
            className={id?.root === pc ? "on" : ""}
            onClick={() => {
              const b = buildChord(pc, id?.quality ?? QUALITIES[scale === "minor" ? 1 : 0]);
              setPcs(b.pitchClasses, b.name);
            }}
          >
            {n}
          </button>
        ))}
      </div>
      <div className="cb-quals">
        {QUALITIES.map((q) => (
          <button
            key={q.id}
            className={id?.quality.id === q.id ? "on" : ""}
            onClick={() => {
              const b = buildChord(root, q);
              setPcs(b.pitchClasses, b.name);
            }}
          >
            {q.label}
          </button>
        ))}
      </div>

      <span className="flabel">free build · color = how it fits what's stacked</span>
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

      <div className="row" style={{ gap: 6 }}>
        <span className="flabel">name</span>
        <input
          type="text"
          value={nameDraft ?? chord.name ?? ""}
          placeholder={chordName(pcs)}
          onChange={(e) => setNameDraft(e.target.value)}
          onBlur={commitName}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") setNameDraft(null);
          }}
          style={{ flex: 1, minWidth: 0 }}
          title="what the lane calls this chord; clear it to name it from its notes"
        />
        <button className="quiet" onClick={() => setPcs([])}>clear notes</button>
      </div>
    </div>
  );
}
