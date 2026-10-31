import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { barTicks, beatsPerBar, effectiveKey, gridLabel, selectedPattern, selectedTrack, selectedTrackIndex, usePatternChords, useStore } from "../store";
import { chordAt, explainNote, midiName, tierMap } from "../theory";
import {
  copyNotes,
  duplicateBySpan,
  duplicateNotes,
  glueNotes,
  magnetPitch,
  moveNotes,
  stampChord,
  noteAt,
  notesInRect,
  nudgeNoteProp,
  pasteNotes,
  placeNote,
  quantizeNoteProp,
  rampNoteProp,
  removeNotes,
  resizeNotes,
  selectionBounds,
  setNoteProp,
  snapFloor,
  snapRound,
  splitNotes,
  stepSelection,
  tileGhost,
  toggleMute,
} from "../noteEditing";
import {
  DEFAULT_SWING_GRID_TICKS,
  mixHex,
  noteProp,
  NOTE_PROPS,
  swingWarp,
  TICKS_PER_BEAT,
  ticksToBeats,
  TIER_COLORS,
  trackColor,
  type NoteEvent,
  type NoteProp,
  type Tier,
} from "../types";
import { BASE_BEAT_W, GUTTER } from "./ChordLane";
import { useMediaQuery, usePixelRatio } from "../uiScale";

// Geometry (from the handoff: 18px rows, 96px beats at 1×, 64px keyboard, 28px ruler)
const LOW = 24; // C1
const HIGH = 96; // C7
const RULER_H = 28;
const EDGE_PX = 7;
/** Velocity lane height; shorter when the page is short (a small window or a large ui scale). */
const VEL_H_TALL = 110;
const VEL_H_SHORT = 64;
const SHORT_PAGE = "(max-height: 600px)";
/** Grid used while ⌥ bypasses snap. */
const FINE_TICKS = TICKS_PER_BEAT / 64;
const BLACK = new Set([1, 3, 6, 8, 10]);
/** Note lengths in ticks, doubling: 1/32 … whole. ⇧-dragging a note walks this ladder. */
const NOTE_VALUE_TICKS = [TICKS_PER_BEAT / 8, TICKS_PER_BEAT / 4, TICKS_PER_BEAT / 2, TICKS_PER_BEAT, 2 * TICKS_PER_BEAT, 4 * TICKS_PER_BEAT];
/** Horizontal travel that moves one rung of `NOTE_VALUE_TICKS`. */
const VALUE_STEP_PX = 26;

/**
 * The property lane's drawn scale: its floor sits 4px above the lane's bottom edge (the lane
 * is `VEL_H` tall, which depends on how short the page is), `LANE_TOP` is its ceiling, and a
 * value maps linearly onto the span between them. Unipolar lanes (velocity, repeats) read
 * from zero — velocity's `min` of 1 is a clamp, not a scale mark — while bipolar lanes span
 * their own range, whose midpoint is the property's neutral value.
 */
const LANE_TOP = 10;
const laneBot = (h: number) => h - 4;
const laneFloor = (p: NoteProp) => (NOTE_PROPS[p].bipolar ? NOTE_PROPS[p].min : 0);
/** Lane y for a value, rounded to the pixel the stem is drawn on. */
const laneY = (p: NoteProp, v: number, h: number) =>
  laneBot(h) - Math.round(((v - laneFloor(p)) / (NOTE_PROPS[p].max - laneFloor(p))) * (laneBot(h) - LANE_TOP));
/** The value a lane y stands for; left unclamped, since the pure setters clamp. */
const laneValue = (p: NoteProp, y: number, h: number) =>
  laneFloor(p) + ((laneBot(h) - y) / (laneBot(h) - LANE_TOP)) * (NOTE_PROPS[p].max - laneFloor(p));

/** How far `[` / `]` moves each lane property: one usable increment, not one LSB. */
const NUDGE_STEP: Record<NoteProp, number> = {
  velocity: 10,
  pan: 0.05,
  release: 0.05,
  modX: 0.05,
  modY: 0.05,
  fine: 25,
  shiftTicks: TICKS_PER_BEAT / 64,
  repeats: 1,
};

const C = {
  editor: "#101013",
  ruler: "#121215",
  line1: "#26262d",
  rowWhite: "#17171b",
  rowBlack: "#121215",
  rowSep: "#0b0b0d",
  octave: "#2e2e36",
  gridBar: "#3a3a44",
  gridBeat: "#26262e",
  gridSub: "rgba(255,255,255,.035)",
  keyWhite: "#d4d4d9",
  keyBlack: "#0e0e10",
  keyBlackBase: "#c9c9ce",
  keySep: "#8a8a92",
  text1: "#e6e6ea",
  text4: "#6a6a74",
  text5: "#5f5f68",
  accent: "#b48cff",
  rootRow: "#261d3a",
  rootRowBlack: "#1f1830",
};

// Borders and selection fills derive from the live tier palette (settings → appearance).
const noteBorder = (t: Tier) => mixHex(TIER_COLORS[t], "#000000", 0.38);
const noteSel = (t: Tier) => mixHex(TIER_COLORS[t], "#ffffff", 0.72);
const tierColor = (t: Tier | null) => (t ? TIER_COLORS[t] : "#9a9aa4");

type Drag =
  | { kind: "move"; ids: Set<string>; anchor: NoteEvent; startX: number; startY: number; startPitch: number; base: NoteEvent[]; lastPitch: number; origin?: "place" | "duplicate"; axis?: "x" | "y" | null }
  | { kind: "resize"; ids: Set<string>; anchor: NoteEvent; startX: number; base: NoteEvent[] }
  | { kind: "noteValue"; ids: Set<string>; anchor: NoteEvent; startX: number; base: NoteEvent[]; steps: number }
  | { kind: "ramp"; ids: Set<string>; prop: NoteProp; t0: number; v0: number; base: NoteEvent[] }
  | { kind: "paint"; base: NoteEvent[] }
  | { kind: "erase"; base: NoteEvent[] }
  | { kind: "marquee"; x0: number; y0: number; x1: number; y1: number; baseSelection: Set<string> }
  | { kind: "lane"; ids: Set<string>; prop: NoteProp; anchorId: string; startY: number; base: NoteEvent[] };

/**
 * The piano roll. Modeless direct manipulation, FL-style:
 *  - click empty = place a note (and keep dragging to move it) · brush on (or ⌥ in "paint"
 *    mode) = paint · ⌥ in "no snap" mode = place / move / resize off the grid
 *  - drag a note = move (auditions pitch; ⇧ constrains to one axis) · right edge = resize ·
 *    ⌥-drag = duplicate
 *  - shift/⌘-drag empty = marquee select · shift-click = toggle · double-click a key = select
 *    that pitch (⌘: the pitch class) · Tab/⇧Tab = next/previous note
 *  - right-click / right-drag = erase · ruler click = seek · property lane: pick the property
 *    it edits, drag stems, ⌥-drag draws a ramp · ⌥-wheel transposes (⌥⇧ octaves) · ⌘-wheel
 *    zooms at the cursor
 *  - keys: ⌫ delete · ⌘A/⌘C/⌘X/⌘V/⌘D · ⇧⌘E split at playhead · ⌘J glue · ⌘B duplicate one
 *    loop later · 0 mute · arrows nudge (⇧ = octave / bar, ⌥ = fine) · [ ] the lane's
 *    property · esc
 * Tiering is visual only — any note can be placed in any tier.
 */
export function PianoRoll() {
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const trackIdx = useStore(selectedTrackIndex);
  const key = useStore(effectiveKey);
  const playhead = useStore((s) => s.playhead);
  const showLandscape = useStore((s) => s.showLandscape);
  const highlightRows = useStore((s) => s.highlightRows);
  const noteLength = useStore((s) => s.noteLength);
  const zoom = useStore((s) => s.zoom);
  const dispatch = useStore((s) => s.dispatch);
  const audition = useStore((s) => s.audition);
  const seek = useStore((s) => s.seek);
  const setZoom = useStore((s) => s.setZoom);
  const setSelectedNoteIds = useStore((s) => s.setSelectedNoteIds);
  const clipboard = useStore((s) => s.noteClipboard);
  const setClipboard = useStore((s) => s.setNoteClipboard);
  const bpb = useStore(beatsPerBar);
  const ticksPerBar = useStore(barTicks);
  const loopRegion = useStore((s) => s.loopRegion);
  const looping = useStore((s) => s.looping);
  const setLoopRegion = useStore((s) => s.setLoopRegion);
  const loopDrag = useRef<{ anchor: number } | null>(null);
  const magnet = useStore((s) => s.magnet);
  const stamp = useStore((s) => s.stamp);
  const brush = useStore((s) => s.brush);
  const follow = useStore((s) => s.follow);
  const playing = useStore((s) => s.playing);
  const selectionRequest = useStore((s) => s.selectionRequest);
  const fold = useStore((s) => s.fold);
  const ghostPatternId = useStore((s) => s.ghostPatternId);
  const ghostPattern = useStore((s) => s.snapshot?.model.patterns.find((p) => p.id === ghostPatternId) ?? null);
  // A drum ghost has no pitches worth tiering: it's drawn as the rhythm (hit lines), in its track's color.
  const ghostDrum = useStore((s) => !!ghostPattern && !!s.snapshot?.model.tracks.find((t) => t.id === ghostPattern.trackId)?.isDrum);
  const ghostColor = useStore((s) => {
    const ts = s.snapshot?.model.tracks ?? [];
    const i = ghostPattern ? ts.findIndex((t) => t.id === ghostPattern.trackId) : -1;
    return i >= 0 ? trackColor(ts[i], i) : C.text4;
  });
  const laneProp = useStore((s) => s.laneProp);
  const setLaneProp = useStore((s) => s.setLaneProp);
  const commitGesture = useStore((s) => s.commitGesture);
  const editing = useStore((s) => s.settings.editing);
  const swing = useStore((s) => s.snapshot?.model.swing ?? 50);
  const swingGridTicks = useStore((s) => s.snapshot?.model.swingGridTicks ?? DEFAULT_SWING_GRID_TICKS);
  const appearance = useStore((s) => s.settings.appearance);
  const pixelRatio = usePixelRatio();
  const VEL_H = useMediaQuery(SHORT_PAGE) ? VEL_H_SHORT : VEL_H_TALL;
  const ROW_H = appearance.rowHeight;
  const defaultVelocity = editing.defaultVelocity;
  // Audition only when the setting says so; drags still audition pitch changes. `lengthTicks` is
  // the length of the note being previewed, so a preview never outlasts the note it stands for.
  const auditionPlace = (pitch: number, velocity: number, lengthTicks?: number) => {
    if (editing.auditionOnPlace) audition(pitch, velocity, lengthTicks);
  };

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rulerRef = useRef<HTMLCanvasElement>(null);
  const velRef = useRef<HTMLCanvasElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState<Set<string>>(new Set());
  const dragRef = useRef<Drag | null>(null);
  const [preview, setPreview] = useState<NoteEvent[] | null>(null);
  const [hover, setHover] = useState<{ tick: number; pitch: number } | null>(null);
  const [scrollLeft, setScrollLeft] = useState(0);
  const [viewW, setViewW] = useState(800);

  const notes = useMemo(() => (pattern && track ? pattern.notes : []), [pattern, track]);
  const totalTicks = pattern?.lengthTicks ?? 16 * TICKS_PER_BEAT;
  // Another pattern's notes, drawn faint behind this one (and heard while hearGhost is on); looped under this
  // pattern the way the song would loop it. A drum ghost is kept apart (rhythm lines, no rows).
  const ghostAll = useMemo(
    () => (ghostPattern && ghostPattern.id !== pattern?.id ? tileGhost(ghostPattern.notes, ghostPattern.lengthTicks, totalTicks) : []),
    [ghostPattern, pattern?.id, totalTicks],
  );
  const ghostNotes = useMemo(() => (ghostDrum ? [] : ghostAll), [ghostDrum, ghostAll]);
  const ghostHits = useMemo(() => (ghostDrum ? ghostAll : []), [ghostDrum, ghostAll]);
  // The project chords under this pattern, from where it was opened (`chordOffsetTicks`), shifted to 0.
  const chords = usePatternChords();
  const beatW = BASE_BEAT_W * zoom;
  const pxPerTick = beatW / TICKS_PER_BEAT;
  const gridW = totalTicks * pxPerTick;
  const shown = preview ?? notes;
  const color = track ? trackColor(track, trackIdx) : C.accent;
  const rootPc = key.rootPitchClass;

  // Row layout: every pitch, or (folded) the pitches that carry notes plus the chord tones of
  // the pattern's chords within an octave of them. Top row first.
  const visiblePitches = useMemo(() => {
    const all: number[] = [];
    for (let p = HIGH; p >= LOW; p--) all.push(p);
    if (!fold) return all;
    const used = new Set<number>([...shown, ...ghostNotes].map((n) => n.pitch));
    const pcs = new Set<number>(chords.flatMap((c) => c.pitchClasses.map((pc) => ((pc % 12) + 12) % 12)));
    const lo = used.size ? Math.min(...used) - 12 : 48;
    const hi = used.size ? Math.max(...used) + 12 : 72;
    return all.filter((p) => used.has(p) || (p >= lo && p <= hi && pcs.has(p % 12)));
  }, [fold, shown, ghostNotes, chords]);
  const rowOf = useMemo(() => new Map(visiblePitches.map((p, i) => [p, i])), [visiblePitches]);
  const rows = visiblePitches.length;
  const gridH = rows * ROW_H;

  /** Top of a pitch's row; hidden pitches land above the ruler (never drawn). */
  const yForPitch = (p: number) => RULER_H + (rowOf.get(p) ?? -2) * ROW_H;
  const pitchForY = (y: number) => visiblePitches[Math.floor((y - RULER_H) / ROW_H)] ?? NaN;
  const tickForX = (x: number) => (x - GUTTER) / pxPerTick;

  const commit = useCallback(
    (next: NoteEvent[], label = "edit notes") => {
      if (pattern && track) void dispatch({ type: "setNotes", patternId: pattern.id, notes: next }, false, label);
    },
    [dispatch, pattern, track],
  );

  // Keep the selection valid and mirror it to the store (inspector shows ranges).
  useEffect(() => {
    setSelection((sel) => {
      const ids = new Set(notes.map((n) => n.id));
      const next = new Set([...sel].filter((id) => ids.has(id)));
      return next.size === sel.size ? sel : next;
    });
  }, [notes]);
  useEffect(() => {
    setSelectedNoteIds([...selection]);
  }, [selection, setSelectedNoteIds]);
  useEffect(() => {
    if (selectionRequest) setSelection(new Set(selectionRequest.ids));
  }, [selectionRequest]);

  // Follow: page the view when the playhead leaves it.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !follow || !playing) return;
    const px = GUTTER + playhead * pxPerTick;
    const left = el.scrollLeft;
    const w = el.clientWidth;
    if (px < left + GUTTER || px > left + w - 24) el.scrollLeft = Math.max(0, px - GUTTER - 16);
  }, [playhead, follow, playing, pxPerTick]);

  // Track the viewport width for the velocity lane. (Centering on C4 happens in the canvas
  // effect below, once the canvas has its height — a layout effect here would clamp to 0.)
  const centered = useRef(false);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewW(el.clientWidth));
    ro.observe(el);
    setViewW(el.clientWidth);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ─── Main canvas ──────────────────────────────────────────────────────────────────────────
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const dpr = pixelRatio;
    const W = GUTTER + gridW;
    const H = RULER_H + gridH;
    if (canvas.width !== Math.round(W * dpr) || canvas.height !== Math.round(H * dpr)) {
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);
      canvas.style.width = `${W}px`;
      canvas.style.height = `${H}px`;
    }
    if (!centered.current && scrollRef.current) {
      centered.current = true;
      const el = scrollRef.current;
      const middle = rowOf.has(60) ? 60 : (visiblePitches[Math.floor(visiblePitches.length / 2)] ?? 60);
      el.scrollTop = Math.max(0, yForPitch(middle) - el.clientHeight / 2);
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.textBaseline = "middle";
    const mono = (w: number, px: number) => `${w} ${px}px "JetBrains Mono", ui-monospace, monospace`;

    ctx.fillStyle = C.editor;
    ctx.fillRect(0, 0, W, H);

    // Rows: tinted per chord, each chord's tiers across its own span (the stretches between
    // chords fall back to the key), so the rows read ahead of the playhead.
    const live = tierMap(playhead, chords, key);
    const edges = [...new Set([0, totalTicks, ...chords.flatMap((c) => [c.startTick, c.startTick + c.lengthTicks])])]
      .filter((t) => t >= 0 && t <= totalTicks)
      .sort((a, b) => a - b);
    const spans = edges.slice(1).map((end, i) => ({ start: edges[i], end, tiers: tierMap((edges[i] + end) / 2, chords, key) }));
    for (const [i, p] of visiblePitches.entries()) {
      const y = yForPitch(p);
      const blk = BLACK.has(p % 12);
      const octaveBreak = fold ? visiblePitches[i + 1] !== undefined && Math.floor(visiblePitches[i + 1] / 12) !== Math.floor(p / 12) : p % 12 === 0;
      const plain = blk ? C.rowBlack : C.rowWhite;
      ctx.fillStyle = plain;
      ctx.fillRect(GUTTER, y, gridW, ROW_H);
      if (highlightRows && !showLandscape) {
        for (const s of spans) {
          const t = s.tiers[p % 12];
          let bg = plain;
          if (rootPc !== null && p % 12 === rootPc && t !== "dissonance") bg = blk ? C.rootRowBlack : C.rootRow;
          else if (t === "chordTone") bg = mixHex(TIER_COLORS.chordTone, plain, blk ? 0.93 : 0.9);
          else if (t === "tension") bg = mixHex(TIER_COLORS.tension, plain, blk ? 0.95 : 0.93);
          else if (t === "dissonance") bg = blk ? "#0e0e10" : "#121214";
          if (bg === plain) continue;
          const x0 = Math.round(GUTTER + s.start * pxPerTick);
          ctx.fillStyle = bg;
          ctx.fillRect(x0, y, Math.round(GUTTER + s.end * pxPerTick) - x0, ROW_H);
        }
      }
      ctx.fillStyle = octaveBreak ? C.octave : C.rowSep;
      ctx.fillRect(GUTTER, y + ROW_H - 1, gridW, 1);
    }

    // Harmonic landscape: every cell tinted by its fit against the chord at that beat
    if (showLandscape) {
      for (let tk = 0; tk < totalTicks; tk += TICKS_PER_BEAT) {
        const map = tierMap(tk + TICKS_PER_BEAT / 2, chords, key);
        for (const p of visiblePitches) {
          const t = map[p % 12];
          if (!t) continue;
          const x = GUTTER + tk * pxPerTick;
          const y = yForPitch(p);
          ctx.globalAlpha = t === "dissonance" ? 0.28 : t === "tension" ? 0.2 : 0.14;
          ctx.fillStyle = tierColor(t);
          ctx.fillRect(x, y, beatW, ROW_H - 1);
          ctx.globalAlpha = 1;
          if (t === "dissonance") {
            ctx.strokeStyle = "rgba(255,59,48,.45)";
            ctx.lineWidth = 0.8;
            ctx.beginPath();
            ctx.moveTo(x, y + ROW_H);
            ctx.lineTo(x + beatW, y);
            ctx.stroke();
          }
        }
      }
    }

    // Vertical grid: subdivision, beat, bar. With swing on, the swung sub-lines are drawn
    // where the notes will actually sound (accent-tinted), next to the straight grid.
    const sub = Math.min(noteLength, TICKS_PER_BEAT);
    for (let t = 0; t <= totalTicks + 1e-9; t += sub) {
      const x = GUTTER + t * pxPerTick;
      const isBar = t % ticksPerBar === 0;
      const isBeat = t % TICKS_PER_BEAT === 0;
      ctx.fillStyle = isBar ? C.gridBar : isBeat ? C.gridBeat : C.gridSub;
      ctx.fillRect(Math.round(x), RULER_H, 1, gridH);
      if (swing > 50 && !isBeat) {
        const w = swingWarp(t, swing, swingGridTicks);
        if (Math.abs(w - t) > 1e-6) {
          ctx.fillStyle = "rgba(180,140,255,.22)";
          ctx.fillRect(Math.round(GUTTER + w * pxPerTick), RULER_H, 1, gridH);
        }
      }
    }

    // Ghost notes: another pattern, dashed, tiered against these chords, behind everything else.
    if (ghostNotes.length) {
      ctx.setLineDash([3, 2]);
      ctx.lineWidth = 1;
      for (const g of ghostNotes) {
        if (!rowOf.has(g.pitch)) continue;
        const t = tierMap(g.startTick, chords, key)[g.pitch % 12];
        const x = GUTTER + g.startTick * pxPerTick;
        const y = yForPitch(g.pitch);
        const w = Math.max(3, g.lengthTicks * pxPerTick - 1);
        ctx.globalAlpha = 0.16;
        ctx.fillStyle = tierColor(t);
        roundRect(ctx, x, y, w, ROW_H - 1, 2);
        ctx.fill();
        ctx.globalAlpha = 0.55;
        ctx.strokeStyle = tierColor(t);
        roundRect(ctx, x + 0.5, y + 0.5, w - 1, ROW_H - 2, 2);
        ctx.stroke();
      }
      ctx.globalAlpha = 1;
      ctx.setLineDash([]);
    }
    // A drum ghost: one faint line per hit (kick and snare/clap stronger than hats); the ruler
    // carries a tick per hit along its foot, so you can lock a bassline to the kick.
    if (ghostHits.length) {
      ctx.fillStyle = ghostColor;
      for (const h of ghostHits) {
        const x = Math.round(GUTTER + h.startTick * pxPerTick);
        const strong = h.pitch === 36 || h.pitch === 38 || h.pitch === 39;
        ctx.globalAlpha = (strong ? 0.26 : 0.1) * (0.5 + 0.5 * (h.velocity / 127));
        ctx.fillRect(x, RULER_H, h.pitch === 36 ? 2 : 1, gridH);
      }
      ctx.globalAlpha = 1;
    }

    // Notes
    const velocityAlpha = (v: number) => 0.45 + 0.55 * (v / 127);
    for (const n of shown) {
      if (!rowOf.has(n.pitch)) continue;
      const t = tierMap(n.startTick, chords, key)[n.pitch % 12];
      const sel = selection.has(n.id);
      const y = yForPitch(n.pitch);
      const w = Math.max(3, n.lengthTicks * pxPerTick - 1);
      const h = ROW_H - 1;
      // A shifted note is drawn where it sounds, but keeps a tick on the grid slot it belongs
      // to: the slot stays its identity, and is what every edit and hit test works from.
      const shiftTicks = n.shiftTicks ?? 0;
      const gridX = GUTTER + n.startTick * pxPerTick;
      const x = shiftTicks ? GUTTER + Math.max(0, n.startTick + shiftTicks) * pxPerTick : gridX;
      if (shiftTicks) {
        ctx.fillStyle = "rgba(255,255,255,.35)";
        ctx.fillRect(Math.round(gridX), y, 1, h);
      }
      ctx.fillStyle = sel ? (t ? noteSel(t) : "#f0f0f4") : tierColor(t);
      ctx.globalAlpha = n.muted ? 0.14 : sel ? 1 : velocityAlpha(n.velocity);
      roundRect(ctx, x, y, w, h, 2);
      ctx.fill();
      ctx.globalAlpha = 1;
      if (n.muted) {
        ctx.setLineDash([3, 2]);
        ctx.strokeStyle = sel ? "#ffffff" : tierColor(t);
        ctx.lineWidth = 1;
        roundRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, 2);
        ctx.stroke();
        ctx.setLineDash([]);
        continue;
      }
      // Non-color tier cues
      if (t === "tension") {
        ctx.fillStyle = "rgba(4,20,13,.85)";
        ctx.beginPath();
        ctx.arc(x + w - 6, y + h / 2, 2, 0, Math.PI * 2);
        ctx.fill();
      } else if (t === "dissonance") {
        ctx.save();
        ctx.beginPath();
        ctx.rect(x, y, w, h);
        ctx.clip();
        ctx.strokeStyle = "rgba(0,0,0,.5)";
        ctx.lineWidth = 1.2;
        for (let sx = x - h; sx < x + w; sx += 5) {
          ctx.beginPath();
          ctx.moveTo(sx, y);
          ctx.lineTo(sx + h, y + h);
          ctx.stroke();
        }
        ctx.restore();
      }
      // Repeats: the note retriggers `repeats + 1` times across its own length, so the body
      // carries one divider per sub-hit — dropped when they would crowd into a solid block.
      const repeats = n.repeats ?? 0;
      if (repeats > 0 && w / (repeats + 1) >= 3) {
        ctx.fillStyle = "rgba(0,0,0,.45)";
        for (let i = 1; i <= repeats; i++) ctx.fillRect(Math.round(x + (i * w) / (repeats + 1)), y, 1, h);
      }
      // Accent marker (vel ≥ 118): 3px bar on the left edge
      if (n.velocity >= 118) {
        ctx.fillStyle = "#ff9d2a";
        ctx.fillRect(x, y, 3, h);
      }
      // Label (a deliberate dissonance keeps its hatch but loses the "!")
      if (w > 22) {
        ctx.fillStyle = "#04140d";
        ctx.font = mono(600, 8);
        ctx.fillText(t === "dissonance" && !n.intentional ? `${midiName(n.pitch)} !` : midiName(n.pitch), x + (n.velocity >= 118 ? 6 : 4), y + h / 2);
      }
      if (n.intentional && t === "dissonance") {
        ctx.strokeStyle = C.accent;
        ctx.lineWidth = 1;
        roundRect(ctx, x - 1.5, y - 1.5, w + 3, h + 3, 3);
        ctx.stroke();
      }
      // Border
      ctx.strokeStyle = sel ? "#ffffff" : t ? noteBorder(t) : "#5f5f68";
      ctx.lineWidth = 1;
      roundRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, 2);
      ctx.stroke();
      if (sel) {
        ctx.strokeStyle = "rgba(255,255,255,.4)";
        roundRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, 3);
        ctx.stroke();
      }
    }

    // Marquee
    const d = dragRef.current;
    if (d && d.kind === "marquee") {
      ctx.setLineDash([3, 3]);
      ctx.strokeStyle = "rgba(255,255,255,.4)";
      ctx.lineWidth = 1;
      ctx.strokeRect(Math.min(d.x0, d.x1), Math.min(d.y0, d.y1), Math.abs(d.x1 - d.x0), Math.abs(d.y1 - d.y0));
      ctx.setLineDash([]);
    }

    // Keyboard (folded: every key is labelled, since neighbours are no longer adjacent)
    for (const p of visiblePitches) {
      const y = yForPitch(p);
      const blk = BLACK.has(p % 12);
      ctx.fillStyle = blk ? C.keyBlackBase : C.keyWhite;
      ctx.fillRect(0, y, GUTTER, ROW_H);
      if (blk) {
        ctx.fillStyle = C.keyBlack;
        ctx.fillRect(0, y, 40, ROW_H);
      }
      ctx.fillStyle = C.keySep;
      ctx.fillRect(0, y + ROW_H - 1, GUTTER, 1);
      const isC = p % 12 === 0;
      const isRoot = rootPc !== null && p % 12 === rootPc;
      if (isC || isRoot || fold) {
        ctx.font = mono(600, 8.5);
        ctx.fillStyle = isRoot ? C.accent : "#2a2a31";
        const label = isC || fold ? midiName(p) : midiName(p).replace(/\d+$/, "");
        ctx.fillText(label, GUTTER - 4 - ctx.measureText(label).width - 6, y + ROW_H / 2);
      }
      // Tier dot at the key edge (guidance cue, mirrors the rows)
      const t = live[p % 12];
      if (t) {
        ctx.fillStyle = tierColor(t);
        ctx.beginPath();
        ctx.arc(GUTTER - 5, y + ROW_H / 2, 2, 0, Math.PI * 2);
        ctx.fill();
      }
    }
    ctx.fillStyle = C.line1;
    ctx.fillRect(GUTTER - 1, RULER_H, 1, gridH);

    // Playhead (its head is in the ruler)
    const px = Math.round(GUTTER + playhead * pxPerTick);
    if (!appearance.reducedMotion) {
      ctx.shadowColor = "rgba(255,255,255,.5)";
      ctx.shadowBlur = 6;
    }
    ctx.fillStyle = "#fff";
    ctx.fillRect(px, RULER_H, 1, gridH);
    ctx.shadowBlur = 0;

    // Hover readout (kept below the ruler, wherever the rows have scrolled to)
    if (hover && !dragRef.current) {
      const t = tierMap(hover.tick, chords, key)[hover.pitch % 12];
      const label = `${midiName(hover.pitch)}${t ? ` · ${t === "chordTone" ? "chord tone" : t}` : ""}`;
      const why = explainNote(hover.pitch, chords, key, hover.tick);
      ctx.font = mono(500, 9);
      const tw = Math.max(ctx.measureText(label).width, ctx.measureText(why).width) + 12;
      const hx = Math.min(GUTTER + hover.tick * pxPerTick + 8, W - tw - 4);
      const hy = Math.max((scrollRef.current?.scrollTop ?? 0) + RULER_H + 2, yForPitch(hover.pitch) - 30);
      ctx.fillStyle = "rgba(22,22,26,.94)";
      ctx.fillRect(hx, hy, tw, 28);
      ctx.fillStyle = t ? tierColor(t) : C.text1;
      ctx.fillText(label, hx + 6, hy + 8);
      ctx.fillStyle = "#b8b8c0";
      ctx.fillText(why, hx + 6, hy + 20);
    }
  }, [shown, ghostNotes, ghostHits, ghostColor, chords, key, playhead, showLandscape, highlightRows, noteLength, beatW, pxPerTick, totalTicks, gridW, gridH, selection, hover, preview, rootPc, ticksPerBar, ROW_H, appearance, swing, swingGridTicks, visiblePitches, rowOf, fold, pixelRatio]);

  // ─── Ruler canvas ─────────────────────────────────────────────────────────────────────────
  // Its own canvas, sticky over the grid's top band, so the bars stay in view as the rows scroll.
  useEffect(() => {
    const canvas = rulerRef.current;
    if (!canvas) return;
    const dpr = pixelRatio;
    const W = GUTTER + gridW;
    const H = RULER_H;
    if (canvas.width !== Math.round(W * dpr) || canvas.height !== Math.round(H * dpr)) {
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);
      canvas.style.width = `${W}px`;
      canvas.style.height = `${H}px`;
    }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.textBaseline = "middle";
    const mono = (w: number, px: number) => `${w} ${px}px "JetBrains Mono", ui-monospace, monospace`;

    // Loop bar (the pattern loops), bar + beat labels
    ctx.fillStyle = C.ruler;
    ctx.fillRect(0, 0, W, RULER_H);
    ctx.fillStyle = C.line1;
    ctx.fillRect(0, RULER_H - 1, W, 1);
    // Loop brace: the whole pattern in the track color, a sub-region in accent.
    ctx.globalAlpha = looping ? 0.8 : 0.25;
    ctx.fillStyle = color;
    ctx.fillRect(GUTTER, 2, gridW, 6);
    if (loopRegion) {
      ctx.fillStyle = C.accent;
      ctx.fillRect(GUTTER + loopRegion[0] * pxPerTick, 2, (loopRegion[1] - loopRegion[0]) * pxPerTick, 6);
    }
    ctx.globalAlpha = 1;
    for (let tk = 0; tk < totalTicks; tk += TICKS_PER_BEAT) {
      const x = GUTTER + tk * pxPerTick;
      const beat = ticksToBeats(tk);
      const barIdx = Math.floor(beat / bpb);
      const isBar = tk % ticksPerBar === 0;
      if (!isBar && beatW < 40) continue;
      ctx.fillStyle = isBar ? "#5a5a62" : "#2e2e36";
      ctx.fillRect(Math.round(x), 10, 1, 18);
      ctx.fillStyle = isBar ? C.text1 : C.text4;
      ctx.font = isBar ? mono(600, 11) : mono(400, 9);
      ctx.fillText(isBar ? `${barIdx + 1}` : `${barIdx + 1}.${Math.floor(beat % bpb) + 1}`, x + 4, 19);
    }
    // Chord names
    ctx.font = mono(500, 9);
    ctx.fillStyle = C.accent;
    for (const c of chords) {
      const x = GUTTER + c.startTick * pxPerTick;
      const label = c.name ?? "";
      const tw = ctx.measureText(label).width;
      ctx.fillText(label, x + c.lengthTicks * pxPerTick - tw - 4, 19);
    }
    // A drum ghost's hits, ticked along the foot
    if (ghostHits.length) {
      ctx.fillStyle = ghostColor;
      for (const h of ghostHits) {
        const x = Math.round(GUTTER + h.startTick * pxPerTick);
        const strong = h.pitch === 36 || h.pitch === 38 || h.pitch === 39;
        ctx.globalAlpha = strong ? 0.9 : 0.45;
        ctx.fillRect(x, RULER_H - (h.pitch === 36 ? 7 : 4), h.pitch === 36 ? 2 : 1, h.pitch === 36 ? 6 : 3);
      }
      ctx.globalAlpha = 1;
    }
    // Corner over the keyboard: the grid value
    ctx.fillStyle = C.ruler;
    ctx.fillRect(0, 0, GUTTER, RULER_H);
    ctx.fillStyle = C.line1;
    ctx.fillRect(GUTTER - 1, 0, 1, RULER_H);
    ctx.fillRect(0, RULER_H - 1, GUTTER, 1);
    ctx.fillStyle = C.text5;
    ctx.font = mono(400, 9);
    const gl = gridLabel(noteLength);
    ctx.fillText(gl, (GUTTER - ctx.measureText(gl).width) / 2, RULER_H / 2);
    // Playhead head
    const px = Math.round(GUTTER + playhead * pxPerTick);
    ctx.fillStyle = "#fff";
    ctx.beginPath();
    ctx.moveTo(px - 5, 8);
    ctx.lineTo(px + 5, 8);
    ctx.lineTo(px, 15);
    ctx.closePath();
    ctx.fill();
  }, [ghostHits, ghostColor, chords, playhead, noteLength, beatW, pxPerTick, totalTicks, gridW, color, bpb, ticksPerBar, loopRegion, looping, pixelRatio]);

  // ─── Property lane canvas ─────────────────────────────────────────────────────────────────
  // One stem per note, drawn for whichever property the lane picker selects. Unipolar
  // properties grow from the floor; bipolar ones grow out of a visible centre line, so a
  // note left at its neutral value reads as a cap sitting on that line.
  useEffect(() => {
    const canvas = velRef.current;
    if (!canvas) return;
    const dpr = pixelRatio;
    const W = viewW;
    const H = VEL_H;
    if (canvas.width !== Math.round(W * dpr) || canvas.height !== Math.round(H * dpr)) {
      canvas.width = Math.round(W * dpr);
      canvas.height = Math.round(H * dpr);
      canvas.style.width = `${W}px`;
      canvas.style.height = `${H}px`;
    }
    const spec = NOTE_PROPS[laneProp];
    const floor = laneFloor(laneProp);
    const middle = (floor + spec.max) / 2;
    // Bipolar lanes measure against their centre, so the rule has to sit on the value it
    // stands for; a unipolar lane's rule is a half-way mark, left where it has always been.
    const ruleY = spec.bipolar ? laneY(laneProp, middle, H) : H / 2;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.textBaseline = "middle";
    ctx.fillStyle = "#0c0c0f";
    ctx.fillRect(0, 0, W, H);
    ctx.save();
    ctx.translate(-scrollLeft, 0);
    // grid
    for (let tk = 0; tk <= totalTicks; tk += TICKS_PER_BEAT) {
      ctx.fillStyle = tk % ticksPerBar === 0 ? "#2a2a32" : "#18181d";
      ctx.fillRect(Math.round(GUTTER + tk * pxPerTick), 0, 1, H);
    }
    ctx.fillStyle = spec.bipolar ? "#32323c" : "#1c1c22";
    ctx.fillRect(GUTTER, ruleY, gridW, 1);
    // stems
    for (const n of shown) {
      const t = tierMap(n.startTick, chords, key)[n.pitch % 12];
      const sel = selection.has(n.id);
      const x = GUTTER + n.startTick * pxPerTick + 2;
      const yv = laneY(laneProp, noteProp(n, laneProp), H);
      const root = spec.bipolar ? ruleY : laneBot(H);
      ctx.fillStyle = sel ? "#e6fff4" : tierColor(t);
      ctx.fillRect(x, Math.min(yv, root), 3, Math.max(1, Math.abs(root - yv)));
      ctx.fillRect(x - 2, yv - 2, 7, 4);
    }
    // playhead
    ctx.fillStyle = "rgba(255,255,255,.6)";
    ctx.fillRect(Math.round(GUTTER + playhead * pxPerTick), 0, 1, H);
    ctx.restore();
    // scale column (fixed): ceiling, middle, floor of whatever the lane is showing
    ctx.fillStyle = "#101013";
    ctx.fillRect(0, 0, GUTTER, H);
    ctx.fillStyle = C.line1;
    ctx.fillRect(GUTTER - 1, 0, 1, H);
    ctx.fillStyle = "#4a4a52";
    ctx.font = '400 8.5px "JetBrains Mono", ui-monospace, monospace';
    for (const [text, ty] of [
      [spec.format(spec.max), 8],
      [spec.format(middle), ruleY],
      [spec.format(floor), H - 8],
    ] as const) {
      ctx.fillText(text, GUTTER - 8 - ctx.measureText(text).width, ty);
    }
  }, [shown, chords, key, playhead, pxPerTick, totalTicks, gridW, selection, scrollLeft, viewW, ticksPerBar, laneProp, pixelRatio, VEL_H]);

  // ─── Interaction ──────────────────────────────────────────────────────────────────────────
  const local = (e: React.PointerEvent, el: HTMLElement) => {
    const rect = el.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };
  /** A grid-canvas y measured from the sticky ruler's top, which sits at the scroll offset. */
  const rulerY = (y: number) => y - (scrollRef.current?.scrollTop ?? 0);

  const hitTest = (x: number, y: number): { note: NoteEvent; edge: boolean } | null => {
    const n = noteAt(shown, tickForX(x), pitchForY(y));
    if (!n) return null;
    const rightX = GUTTER + (n.startTick + n.lengthTicks) * pxPerTick;
    const edgeZone = Math.min(EDGE_PX, (n.lengthTicks * pxPerTick) / 3);
    return { note: n, edge: x >= rightX - edgeZone };
  };

  const onPointerDown = (e: React.PointerEvent) => {
    const canvas = canvasRef.current!;
    scrollRef.current?.focus();
    const { x, y } = local(e, canvas);
    if (rulerY(y) < RULER_H) {
      if (x < GUTTER) return;
      if (e.button === 2) {
        setLoopRegion(null);
        return;
      }
      if (rulerY(y) < 10) {
        // Drag in the brace strip to set a loop region (snapped to beats).
        canvas.setPointerCapture(e.pointerId);
        loopDrag.current = { anchor: Math.max(0, Math.min(totalTicks, Math.round(tickForX(x) / TICKS_PER_BEAT) * TICKS_PER_BEAT)) };
        return;
      }
      seek(Math.max(0, Math.min(totalTicks, tickForX(x))));
      return;
    }
    if (x < GUTTER) {
      const pitch = pitchForY(y);
      if (pitch >= LOW && pitch <= HIGH) audition(pitch);
      return;
    }
    const tick = tickForX(x);
    const pitch = pitchForY(y);
    if (tick < 0 || tick >= totalTicks || pitch < LOW || pitch > HIGH) return;
    canvas.setPointerCapture(e.pointerId);

    if (e.button === 2) {
      const n = noteAt(notes, tick, pitch);
      const base = n ? removeNotes(notes, new Set([n.id])) : notes;
      dragRef.current = { kind: "erase", base };
      setPreview(base);
      return;
    }
    if (e.button !== 0) return;

    const hit = hitTest(x, y);
    if (hit) {
      if (e.shiftKey) {
        // Click toggles the selection; drag steps the note's length through the note values.
        const ids = selection.has(hit.note.id) ? new Set(selection) : new Set([hit.note.id]);
        dragRef.current = { kind: "noteValue", ids, anchor: hit.note, startX: x, base: notes, steps: 0 };
        setPreview(notes);
        return;
      }
      let ids = selection.has(hit.note.id) ? new Set(selection) : new Set([hit.note.id]);
      let base = notes;
      let anchor = hit.note;
      let origin: "duplicate" | undefined;
      if (e.altKey && !hit.edge) {
        origin = "duplicate";
        const originals = notes.filter((n) => ids.has(n.id));
        const copies = copyNotes(originals);
        base = [...notes, ...copies];
        ids = new Set(copies.map((c) => c.id));
        anchor = copies[originals.indexOf(hit.note)] ?? copies[0];
      }
      setSelection(ids);
      dragRef.current = hit.edge
        ? { kind: "resize", ids, anchor, startX: x, base }
        : { kind: "move", ids, anchor, startX: x, startY: y, startPitch: pitchForY(y), base, lastPitch: anchor.pitch, origin };
      setPreview(base);
      return;
    }

    if (e.shiftKey || e.metaKey || e.ctrlKey) {
      dragRef.current = { kind: "marquee", x0: x, y0: y, x1: x, y1: y, baseSelection: new Set(e.shiftKey ? selection : []) };
      setPreview(notes);
      return;
    }
    if (brush || (e.altKey && editing.altKey === "paint")) {
      const placed = placeNote(notes, tick, pitch, noteLength, noteLength, defaultVelocity);
      const base = placed ? [...notes, placed] : notes;
      if (placed) auditionPlace(pitch, placed.velocity, placed.lengthTicks);
      dragRef.current = { kind: "paint", base };
      setPreview(base);
      return;
    }
    // ⌥ (no-snap mode): place exactly where the cursor is instead of on the grid.
    const snapGrid = e.altKey && editing.altKey === "noSnap" ? FINE_TICKS : noteLength;
    if (stamp) {
      // Chord stamp: place the chord under the cursor as stacked notes, voiced upward.
      const chord = chordAt(chords, tick);
      if (!chord) return;
      const added = stampChord(notes, tick, chord.pitchClasses, pitch, noteLength, noteLength).map((n) => ({ ...n, velocity: defaultVelocity }));
      if (!added.length) return;
      for (const a of added) auditionPlace(a.pitch, a.velocity, a.lengthTicks);
      commit([...notes, ...added], "stamp chord");
      setSelection(new Set(added.map((a) => a.id)));
      return;
    }
    const target = magnet ? magnetPitch(pitch, tierMap(tick, chords, key)) : pitch;
    const placed = placeNote(notes, tick, target, noteLength, snapGrid, defaultVelocity);
    if (!placed) {
      setSelection(new Set());
      return;
    }
    auditionPlace(target, placed.velocity, placed.lengthTicks);
    const base = [...notes, placed];
    const ids = new Set([placed.id]);
    setSelection(ids);
    dragRef.current = { kind: "move", ids, anchor: placed, startX: x, startY: y, startPitch: pitch, base, lastPitch: target, origin: "place" };
    setPreview(base);
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const canvas = canvasRef.current!;
    const { x, y } = local(e, canvas);
    if (loopDrag.current) {
      const tk = Math.max(0, Math.min(totalTicks, Math.round(tickForX(x) / TICKS_PER_BEAT) * TICKS_PER_BEAT));
      const a = loopDrag.current.anchor;
      if (tk !== a) setLoopRegion([Math.min(a, tk), Math.max(a, tk)]);
      return;
    }
    const d = dragRef.current;
    if (!d) {
      const tick = tickForX(x);
      const pitch = pitchForY(y);
      setHover(x >= GUTTER && rulerY(y) >= RULER_H && tick < totalTicks && pitch >= LOW && pitch <= HIGH ? { tick, pitch } : null);
      return;
    }
    // Auto-scroll when dragging past the viewport's edges.
    const el = scrollRef.current;
    if (el && d.kind !== "lane" && d.kind !== "ramp") {
      const rect = el.getBoundingClientRect();
      if (e.clientX > rect.right - 24) el.scrollLeft += 12;
      else if (e.clientX < rect.left + GUTTER + 12) el.scrollLeft = Math.max(0, el.scrollLeft - 12);
      if (e.clientY > rect.bottom - 24) el.scrollTop += 10;
      else if (e.clientY < rect.top + RULER_H + 12) el.scrollTop = Math.max(0, el.scrollTop - 10);
    }
    const snapGrid = e.altKey && editing.altKey === "noSnap" ? FINE_TICKS : noteLength;
    switch (d.kind) {
      case "move": {
        const rawDelta = (x - d.startX) / pxPerTick;
        const newStart = snapRound(d.anchor.startTick + rawDelta, snapGrid);
        let dTicks = newStart - d.anchor.startTick;
        const rowPitch = pitchForY(y);
        let dPitch = Number.isNaN(rowPitch) ? 0 : rowPitch - d.startPitch;
        // ⇧ constrains to the axis that moved first (time or pitch).
        if (e.shiftKey) {
          d.axis ??= Math.abs(x - d.startX) >= Math.abs(y - d.startY) ? "x" : "y";
          if (d.axis === "x") dPitch = 0;
          else dTicks = 0;
        } else {
          d.axis = null;
        }
        if (magnet && d.ids.size === 1) {
          const want = magnetPitch(d.anchor.pitch + dPitch, tierMap(d.anchor.startTick + dTicks, chords, key));
          dPitch = want - d.anchor.pitch;
        }
        const next = moveNotes(d.base, d.ids, dTicks, dPitch, { min: LOW, max: HIGH }, totalTicks);
        const moved = next.find((n) => n.id === d.anchor.id);
        if (moved && moved.pitch !== d.lastPitch) {
          d.lastPitch = moved.pitch;
          audition(moved.pitch, moved.velocity, moved.lengthTicks);
        }
        setPreview(next);
        break;
      }
      case "resize": {
        const rawDelta = (x - d.startX) / pxPerTick;
        const newLen = Math.max(snapGrid, snapRound(d.anchor.lengthTicks + rawDelta, snapGrid));
        setPreview(resizeNotes(d.base, d.ids, newLen - d.anchor.lengthTicks, snapGrid));
        break;
      }
      case "noteValue": {
        // One rung per VALUE_STEP_PX of travel: right doubles the note value, left halves it.
        const steps = Math.round((x - d.startX) / VALUE_STEP_PX);
        if (steps === d.steps) break;
        d.steps = steps;
        setPreview(d.base.map((n) => (d.ids.has(n.id) ? { ...n, lengthTicks: steppedNoteValue(n.lengthTicks, steps) } : n)));
        break;
      }
      case "paint": {
        const tick = tickForX(x);
        const pitch = pitchForY(y);
        if (tick < 0 || tick >= totalTicks || pitch < LOW || pitch > HIGH) return;
        const placed = placeNote(d.base, tick, pitch, noteLength, noteLength, defaultVelocity);
        if (placed) {
          d.base = [...d.base, placed];
          auditionPlace(pitch, placed.velocity, placed.lengthTicks);
          setPreview(d.base);
        }
        break;
      }
      case "erase": {
        const n = noteAt(d.base, tickForX(x), pitchForY(y));
        if (n) {
          d.base = removeNotes(d.base, new Set([n.id]));
          setPreview(d.base);
        }
        break;
      }
      case "marquee": {
        d.x1 = x;
        d.y1 = y;
        const ids = notesInRect(notes, tickForX(d.x0), tickForX(d.x1), pitchForY(d.y0), pitchForY(d.y1));
        setSelection(new Set([...d.baseSelection, ...ids]));
        setPreview([...notes]);
        break;
      }
      case "lane":
      case "ramp":
        break;
    }
  };

  // Double-click a key: select every note at that pitch (⌘: the pitch class in every octave).
  const onDoubleClick = (e: React.MouseEvent) => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    if (x >= GUTTER || rulerY(y) < RULER_H) return;
    const pitch = pitchForY(y);
    const pc = ((pitch % 12) + 12) % 12;
    const byClass = e.metaKey || e.ctrlKey;
    setSelection(new Set(notes.filter((n) => (byClass ? ((n.pitch % 12) + 12) % 12 === pc : n.pitch === pitch)).map((n) => n.id)));
  };

  // ⌥-wheel transposes the selection (⌥⇧: octaves); ⌘-wheel zooms around the cursor; ⇧-wheel scrolls sideways.
  // Attached natively (non-passive) because React's onWheel cannot preventDefault.
  const wheelCommit = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onWheel = (e: WheelEvent) => {
    const el = scrollRef.current;
    if (!el) return;
    if (e.altKey && selection.size) {
      e.preventDefault();
      const step = (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 12 : 1);
      const next = moveNotes(notes, selection, 0, step, { min: LOW, max: HIGH }, totalTicks);
      if (next !== notes && pattern && track) {
        void dispatch({ type: "setNotes", patternId: pattern.id, notes: next }, true, "transpose notes");
        if (wheelCommit.current) clearTimeout(wheelCommit.current);
        wheelCommit.current = setTimeout(() => void commitGesture(), 400);
      }
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const cursorX = e.clientX - rect.left;
      const tickUnderCursor = (el.scrollLeft + cursorX - GUTTER) / pxPerTick;
      const nextZoom = Math.min(3, Math.max(0.25, zoom * (e.deltaY < 0 ? 1.1 : 0.9)));
      setZoom(nextZoom);
      const nextPxPerTick = (BASE_BEAT_W * nextZoom) / TICKS_PER_BEAT;
      requestAnimationFrame(() => {
        el.scrollLeft = Math.max(0, tickUnderCursor * nextPxPerTick + GUTTER - cursorX);
      });
      return;
    }
    if (e.shiftKey && Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
      e.preventDefault();
      el.scrollLeft += e.deltaY;
    }
  };
  const onWheelRef = useRef(onWheel);
  onWheelRef.current = onWheel;
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const handler = (e: WheelEvent) => onWheelRef.current(e);
    el.addEventListener("wheel", handler, { passive: false });
    return () => el.removeEventListener("wheel", handler);
  }, []);

  const endDrag = () => {
    if (loopDrag.current) {
      loopDrag.current = null;
      return;
    }
    const d = dragRef.current;
    dragRef.current = null;
    if (!d) return;
    if (d.kind === "marquee") {
      setPreview(null);
      return;
    }
    // ⇧ on a note that never moved far enough to change its value is still a selection toggle.
    if (d.kind === "noteValue" && d.steps === 0) {
      setPreview(null);
      setSelection((sel) => {
        const next = new Set(sel);
        if (next.has(d.anchor.id)) next.delete(d.anchor.id);
        else next.add(d.anchor.id);
        return next;
      });
      return;
    }
    const result = preview;
    setPreview(null);
    if (result && result !== notes) commit(result, dragLabel(d, notes, result));
  };

  // Property lane: drag a stem (and every selected stem with it) vertically. The anchor lands
  // on the value under the pointer and the rest travel by the same amount, so a drag keeps the
  // shape of a selection instead of flattening it.
  const onLaneDown = (e: React.PointerEvent) => {
    const canvas = velRef.current!;
    const { x, y } = local(e, canvas);
    if (x < GUTTER || e.button !== 0) return;
    const tick = (x + scrollLeft - GUTTER) / pxPerTick;
    if (e.altKey) {
      // Ramp: draw a line through the stems; selected notes only when there is a selection.
      canvas.setPointerCapture(e.pointerId);
      dragRef.current = { kind: "ramp", ids: new Set(selection), prop: laneProp, t0: tick, v0: quantizeNoteProp(laneProp, laneValue(laneProp, y, VEL_H)), base: notes };
      setPreview(notes);
      return;
    }
    let best: NoteEvent | null = null;
    let bestDx = Infinity;
    for (const n of notes) {
      const dx = Math.abs(n.startTick - tick) * pxPerTick;
      if (dx < bestDx && dx <= 8) {
        best = n;
        bestDx = dx;
      }
    }
    if (!best) return;
    canvas.setPointerCapture(e.pointerId);
    const ids = selection.has(best.id) ? new Set(selection) : new Set([best.id]);
    if (!selection.has(best.id)) setSelection(ids);
    dragRef.current = { kind: "lane", ids, prop: laneProp, anchorId: best.id, startY: y, base: notes };
    applyLaneAt(y);
  };
  const applyLaneAt = (y: number) => {
    const d = dragRef.current;
    if (!d || d.kind !== "lane") return;
    const anchor = d.base.find((n) => n.id === d.anchorId);
    if (!anchor) return;
    // Pin the anchor to the value under the pointer, then carry the others by the distance it
    // actually travelled — the setter owns clamping, so that distance is the real one.
    const pinned = setNoteProp(d.base, new Set([d.anchorId]), d.prop, laneValue(d.prop, y, VEL_H));
    const delta = noteProp(pinned.find((n) => n.id === d.anchorId)!, d.prop) - noteProp(anchor, d.prop);
    const others = new Set([...d.ids].filter((id) => id !== d.anchorId));
    setPreview(others.size ? nudgeNoteProp(pinned, others, d.prop, delta) : pinned);
  };
  const onLaneMove = (e: React.PointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    if (d.kind === "ramp") {
      const { x, y } = local(e, velRef.current!);
      const t1 = (x + scrollLeft - GUTTER) / pxPerTick;
      setPreview(rampAcross(d, t1, quantizeNoteProp(d.prop, laneValue(d.prop, y, VEL_H))));
      return;
    }
    if (d.kind !== "lane") return;
    applyLaneAt(local(e, velRef.current!).y);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const mod = e.metaKey || e.ctrlKey;
    const k = e.key;
    // ⌥ + arrows is the roll's one alt chord (fine nudge). Every other alt combination belongs
    // to the window manager, which binds alt-<key> for workspaces and focus; claiming them here
    // would swallow those keys whenever the roll has focus.
    if (e.altKey && !k.startsWith("Arrow")) return;
    const bounds = selectionBounds(notes, selection);
    const grid = noteLength;

    if (k === "Backspace" || k === "Delete") {
      if (selection.size) commit(removeNotes(notes, selection), "delete notes");
    } else if (k === "Tab") {
      const id = stepSelection(notes, selection, e.shiftKey ? -1 : 1);
      if (id) {
        setSelection(new Set([id]));
        const n = notes.find((x) => x.id === id);
        if (n) auditionPlace(n.pitch, n.velocity, n.lengthTicks);
      }
    } else if (k === "0" && !mod) {
      if (selection.size) commit(toggleMute(notes, selection), "mute notes");
    } else if (mod && e.shiftKey && k.toLowerCase() === "e") {
      // ⇧⌘E splits at the playhead. ⌘E alone is always export (App's global shortcuts), so the
      // same keys never do two different things depending on the selection.
      const next = splitNotes(notes, selection, playhead);
      if (next !== notes && next.length !== notes.length) commit(next, "split notes");
    } else if (mod && k.toLowerCase() === "j") {
      if (selection.size) commit(glueNotes(notes, selection), "glue notes");
    } else if (mod && k.toLowerCase() === "b") {
      if (!selection.size) return;
      const { notes: next, newIds } = duplicateBySpan(notes, selection, totalTicks);
      commit(next, "duplicate to next loop");
      setSelection(newIds);
    } else if (mod && k.toLowerCase() === "a") {
      setSelection(new Set(notes.map((n) => n.id)));
    } else if (mod && k.toLowerCase() === "c") {
      setClipboard(notes.filter((n) => selection.has(n.id)));
    } else if (mod && k.toLowerCase() === "x") {
      setClipboard(notes.filter((n) => selection.has(n.id)));
      if (selection.size) commit(removeNotes(notes, selection), "cut notes");
    } else if (mod && k.toLowerCase() === "v") {
      if (!clipboard.length) return;
      const at = bounds ? snapFloor(bounds.end, grid) : snapFloor(playhead, grid);
      const { notes: next, newIds } = pasteNotes(notes, clipboard, at);
      commit(next, "paste notes");
      setSelection(newIds);
    } else if (mod && k.toLowerCase() === "d") {
      if (!selection.size) return;
      const { notes: next, newIds } = duplicateNotes(notes, selection, grid);
      commit(next, "duplicate notes");
      setSelection(newIds);
    } else if (mod && (k === "=" || k === "+")) {
      setZoom(zoom * 1.25);
    } else if (mod && k === "-") {
      setZoom(zoom / 1.25);
    } else if (k === "Escape") {
      setSelection(new Set());
    } else if (k.startsWith("Arrow") && selection.size) {
      const step = e.altKey ? FINE_TICKS : e.shiftKey ? ticksPerBar : grid;
      const dTicks = k === "ArrowLeft" ? -step : k === "ArrowRight" ? step : 0;
      const dPitch = k === "ArrowUp" ? (e.shiftKey ? 12 : 1) : k === "ArrowDown" ? -(e.shiftKey ? 12 : 1) : 0;
      commit(moveNotes(notes, selection, dTicks, dPitch, { min: LOW, max: HIGH }, totalTicks), dPitch ? "transpose notes" : "nudge notes");
    } else if ((k === "[" || k === "]") && selection.size) {
      commit(nudgeNoteProp(notes, selection, laneProp, k === "]" ? NUDGE_STEP[laneProp] : -NUDGE_STEP[laneProp]), `change ${NOTE_PROPS[laneProp].label}`);
    } else {
      return; // let global shortcuts through
    }
    e.preventDefault();
    e.stopPropagation();
  };

  if (!pattern || !track) return null;

  // The lane header reports the property the lane is showing, over the selection when there is
  // one and over the whole track otherwise.
  const laneSpec = NOTE_PROPS[laneProp];
  const laneVals = (selection.size ? notes.filter((n) => selection.has(n.id)) : notes).map((n) => noteProp(n, laneProp));
  const laneAvg = laneVals.length ? laneSpec.format(laneVals.reduce((a, b) => a + b, 0) / laneVals.length) : "";
  const laneSummary = selection.size
    ? laneVals.length
      ? `sel ${laneSpec.format(Math.min(...laneVals))}–${laneSpec.format(Math.max(...laneVals))} · avg ${laneAvg}`
      : "—"
    : notes.length
      ? `${notes.length} notes · avg ${laneAvg}`
      : "—";

  return (
    <div className="rollwrap">
      <div
        className="rollscroll"
        ref={scrollRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onScroll={(e) => setScrollLeft((e.target as HTMLDivElement).scrollLeft)}
      >
        <canvas className="rollruler" ref={rulerRef} style={{ marginBottom: -RULER_H }} />
        <canvas
          ref={canvasRef}
          onPointerDown={onPointerDown}
          onDoubleClick={onDoubleClick}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onPointerLeave={() => setHover(null)}
          onContextMenu={(e) => e.preventDefault()}
        />
      </div>
      {notes.length === 0 && (
        <div className="emptyhint" aria-hidden="true">
          click anywhere to place a note
          <br />
          or set KEYS to TIER and play the home row: A S D F… are the chord tones
        </div>
      )}
      <div className="lanehdr">
        <select className="lanepick" value={laneProp} onChange={(e) => setLaneProp(e.target.value as NoteProp)} aria-label="lane property">
          {(Object.keys(NOTE_PROPS) as NoteProp[]).map((p) => (
            <option key={p} value={p}>
              {NOTE_PROPS[p].label}
            </option>
          ))}
        </select>
        <span className="sum">{laneSummary}</span>
      </div>
      <div className="vellane" style={{ height: VEL_H }}>
        <canvas ref={velRef} onPointerDown={onLaneDown} onPointerMove={onLaneMove} onPointerUp={endDrag} onPointerCancel={endDrag} onContextMenu={(e) => e.preventDefault()} />
      </div>
    </div>
  );
}

/**
 * The ⌥-ramp: a straight line from (`t0`, `v0`) to (`t1`, `v1`) in tick × value space, taken
 * by every stem whose note starts between the two ticks — the selection's stems when there is
 * a selection, otherwise all of them. Handing `rampNoteProp` the line's own value at the first
 * and last note it touches makes its interpolation reproduce that line exactly.
 */
function rampAcross(d: Extract<Drag, { kind: "ramp" }>, t1: number, v1: number): NoteEvent[] {
  const [from, to] = d.t0 <= t1 ? [d.t0, t1] : [t1, d.t0];
  const [vf, vt] = d.t0 <= t1 ? [d.v0, v1] : [v1, d.v0];
  const pool = d.ids.size ? d.base.filter((n) => d.ids.has(n.id)) : d.base;
  const touched = pool.filter((n) => n.startTick >= from - 1e-9 && n.startTick <= to + 1e-9);
  if (!touched.length) return d.base;
  const slope = (vt - vf) / Math.max(1e-9, to - from);
  const starts = touched.map((n) => n.startTick);
  const lo = Math.min(...starts);
  const hi = Math.max(...starts);
  return rampNoteProp(d.base, new Set(touched.map((n) => n.id)), d.prop, vf + slope * (lo - from), vf + slope * (hi - from));
}

/** Name a finished drag for the Edit menu. */
function dragLabel(d: Drag, before: NoteEvent[], after: NoteEvent[]): string {
  switch (d.kind) {
    case "move": {
      if (d.origin === "place") return "place note";
      if (d.origin === "duplicate") return "duplicate notes";
      const anchorBefore = before.find((n) => n.id === d.anchor.id);
      const anchorAfter = after.find((n) => n.id === d.anchor.id);
      if (anchorBefore && anchorAfter && anchorBefore.pitch !== anchorAfter.pitch && anchorBefore.startTick === anchorAfter.startTick) return "transpose notes";
      return "move notes";
    }
    case "resize":
      return "resize notes";
    case "noteValue":
      return "change note value";
    case "paint":
      return "paint notes";
    case "erase":
      return "erase notes";
    case "lane":
      return `set note ${NOTE_PROPS[d.prop].label}`;
    case "ramp":
      return `ramp note ${NOTE_PROPS[d.prop].label}`;
    case "marquee":
      return "edit notes";
  }
}

/**
 * Walk a length `steps` rungs along `NOTE_VALUE_TICKS`, starting from the rung nearest the length
 * it has now — so a 1/16 note dragged one rung right becomes a 1/8, and an off-grid length
 * lands on the ladder instead of staying off it.
 */
function steppedNoteValue(lengthTicks: number, steps: number): number {
  let nearest = 0;
  for (let i = 1; i < NOTE_VALUE_TICKS.length; i++) {
    if (Math.abs(NOTE_VALUE_TICKS[i] - lengthTicks) < Math.abs(NOTE_VALUE_TICKS[nearest] - lengthTicks)) nearest = i;
  }
  return NOTE_VALUE_TICKS[Math.min(NOTE_VALUE_TICKS.length - 1, Math.max(0, nearest + steps))];
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.lineTo(x + w - rr, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + rr);
  ctx.lineTo(x + w, y + h - rr);
  ctx.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
  ctx.lineTo(x + rr, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - rr);
  ctx.lineTo(x, y + rr);
  ctx.quadraticCurveTo(x, y, x + rr, y);
  ctx.closePath();
}
