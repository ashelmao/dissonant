import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { barTicks, beatsPerBar, dbText, useStore } from "../store";
import type { DrumPad } from "../sound";
import { automationLane, automationValueAt, autoTargetRange, sectionSpan, songLength, trackColor, trackKind, TICKS_PER_BEAT, type AudioClip, type AutoTarget, type Clip, type NoteEvent, type Section, type SongPattern, type Track, type TrackParam } from "../types";
import { noteName } from "../theory";
import { VMeter } from "./Meter";
import { ChordLane, DetectedChordLane, GUTTER } from "./ChordLane";
import { detectedChords } from "../detection";
import { SECTION_COLORS, SectionEditor } from "./SectionEditor";
import { BreakpointLane, TempoLane } from "./TempoLane";
import { hz, panText, pct } from "./Knob";
import { keys } from "../platform";
import { clampFade, clipsInBox, dropAt, duplicateClips, equalPowerGain, groupMoveDelta, moveClips, pasteClips, snapGrid, snapTo, toggleId, toggleMuted, trimClip, type ArrSnap } from "../clipEditing";
import { usePanelSize } from "../panels";
import { Splitter } from "./Splitter";

/** Lane height; the marquee maps y to a lane with it, so rows take it inline rather than from CSS. */
const ROW_H = 56;
const CLIP_H = ROW_H - 5;
/** An open automation sub-lane's height; the marquee counts it in, so it is inline too. */
const AUTO_H = 44;
/** A press that travels less than this is a click, not a drag. */
const DRAG_PX = 3;
/** The ruler's top strip, where section markers live. */
const MARKER_H = 16;
/** The strip under the markers, where the loop brace is drawn and dragged. */
const LOOP_H = 10;
/** The strip under the brace, where the punch range is drawn and dragged. Its own lane, not a
 *  mode of the brace: the two mean different things and only one of them can be live. */
const PUNCH_H = 10;

/** The song view's snap choices, coarse to fine; the labels match the roll's grid select. */
const SNAP_OPTIONS: [string, ArrSnap][] = [
  ["bar", "bar"],
  ["1/2", TICKS_PER_BEAT * 2],
  ["1/4", TICKS_PER_BEAT],
  ["1/8", TICKS_PER_BEAT / 2],
  ["1/16", TICKS_PER_BEAT / 4],
  ["off", "off"],
];

/** The track parameters an automation sub-lane can draw, in the track head's picker order. */
const AUTO_PARAMS: [TrackParam, string][] = [
  ["volume", "vol"],
  ["pan", "pan"],
  ["tone", "tone"],
  ["reverbSend", "send"],
];

/**
 * Clips cut or copied in the song view. Module-level rather than in the store: unlike notes,
 * which travel between the roll and the drum grid, nothing outside this screen pastes clips.
 */
let clipboard: Clip[] = [];

type Drag =
  | { kind: "move"; anchor: Clip; group: Clip[]; x0: number; dup: boolean; moved: boolean; collapseTo: string | null }
  | { kind: "trim"; clip: Clip; x0: number }
  | { kind: "marquee"; trackId: string; x0: number; y0: number; base: string[]; additive: boolean; moved: boolean };

interface Box {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/**
 * SONG mode center: a bar ruler with section markers, the read-only chord bar, the tempo map,
 * track headers, and one lane per track holding that track's clips (each clip a placed pattern,
 * its notes previewed; an audio track shows its takes read-only). The song has a hard end at
 * `songLengthTicks`; nothing moves, extends or drops past it.
 *
 * Clips: click selects, shift-click toggles, drag across empty lane space to marquee-select over
 * lanes. Drag moves the whole selection along its own lanes (horizontal only), the right edge trims
 * or loop-extends one clip, ⌥-drag copies, ⇧ ignores the snap grid, Delete/Backspace removes,
 * ⌘A/⌘C/⌘X/⌘V/⌘D select / copy / cut / paste at the playhead / duplicate, 0 mutes, right-click
 * opens a menu, double-click opens the pattern at that clip. Patterns arrive by dragging them in
 * from the tree.
 * Sections: double-click the marker strip to add, drag to move, click to edit, right-click to remove.
 * Loop: drag the strip under the markers to set the song's loop brace, right-click it to clear.
 */
export function Arrangement() {
  const model = useStore((s) => s.snapshot!.model);
  // The track column is draggable, and the rows, ruler, chord bar and tempo lane all measure
  // against it, so it is one value rather than a constant.
  const HEAD_W = usePanelSize("songHead");
  const playhead = useStore((s) => s.playhead);
  const pxPerBar = useStore((s) => s.arrZoom);
  const setArrZoom = useStore((s) => s.setArrZoom);
  const selectedTrackId = useStore((s) => s.selectedTrackId);
  const selectedPatternId = useStore((s) => s.selectedPatternId);
  const selectedClipIds = useStore((s) => s.selectedClipIds);
  const patternDrag = useStore((s) => s.patternDrag);
  const hearChords = useStore((s) => s.hearChords);
  const trackPeaks = useStore((s) => s.trackPeaks);
  const arrSnap = useStore((s) => s.arrSnap);
  const follow = useStore((s) => s.follow);
  const playing = useStore((s) => s.playing);
  const looping = useStore((s) => s.looping);
  const bpb = useStore(beatsPerBar);
  const barTick = useStore(barTicks);
  const s = useStore();
  const dispatch = s.dispatch;

  const scrollerRef = useRef<HTMLDivElement>(null);
  const lanesRef = useRef<HTMLDivElement>(null);
  const [viewW, setViewW] = useState(1200);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewW(el.clientWidth));
    ro.observe(el);
    setViewW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const pxPerBeat = pxPerBar / bpb;
  const pxPerTick = pxPerBeat / TICKS_PER_BEAT;
  const songEnd = songLength(model);
  const endX = songEnd * pxPerTick;
  // The timeline runs a few bars past the end so the out-of-bounds stretch reads as such.
  const bars = Math.max(Math.ceil((viewW - HEAD_W - 12) / pxPerBar), Math.ceil(songEnd / barTick) + 8);
  const timelineW = bars * pxPerBar;
  const rowW = HEAD_W + timelineW + 12;
  // The grid every gesture lands on. ⇧ bypasses it for one drag, the way ⌥ does in the roll.
  const grid = snapGrid(arrSnap, barTick);

  const trackOfPattern = new Map(model.patterns.map((p) => [p.id, p.trackId]));
  const laneOfTrack = new Map(model.tracks.map((t, i) => [t.id, i]));
  const laneOf = (clip: Clip) => laneOfTrack.get(trackOfPattern.get(clip.patternId) ?? "") ?? -1;

  // Which track shows an automation sub-lane, and for which parameter. A view choice: it never
  // leaves this screen, so it lives here rather than in the model or the store.
  const [autoShown, setAutoShown] = useState<{ trackId: string; param: TrackParam } | null>(null);
  /** Lane-space y → track index, over the row heights: an open sub-lane pushes the rest down. */
  const trackAtY = (y: number) => {
    if (y < 0) return -1;
    let bottom = 0;
    for (let i = 0; i < model.tracks.length; i++) {
      bottom += ROW_H + (model.tracks[i].id === autoShown?.trackId ? AUTO_H : 0);
      if (y < bottom) return i;
    }
    return model.tracks.length;
  };

  // ── Drop target for patterns dragged out of the tree ───────────────────────────────────
  /** The snapped start tick under a viewport point, or null when it isn't over the song's timeline. */
  const barAt = (clientX: number, clientY: number): number | null => {
    const el = scrollerRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (clientX < r.left + HEAD_W || clientX > r.left + el.clientWidth || clientY < r.top || clientY > r.top + el.clientHeight) return null;
    return dropAt((clientX - r.left - HEAD_W + el.scrollLeft) / pxPerTick, grid, songEnd);
  };
  const barAtRef = useRef(barAt);
  barAtRef.current = barAt;

  const setPatternDropHandler = useStore((st) => st.setPatternDropHandler);
  useEffect(() => {
    // The vertical position doesn't matter: a pattern always lands on its own track's lane.
    setPatternDropHandler((patternId, clientX, clientY) => {
      const tick = barAtRef.current(clientX, clientY);
      if (tick === null) return;
      void (async () => {
        const st = useStore.getState();
        const before = new Set(st.snapshot!.model.clips.map((c) => c.id));
        await st.dispatch({ type: "addClip", patternId, startTick: tick, lengthTicks: null });
        const added = useStore.getState().snapshot!.model.clips.find((c) => !before.has(c.id));
        if (added) {
          st.selectPattern(patternId);
          st.selectClips([added.id]);
        }
      })();
    });
    return () => setPatternDropHandler(null);
  }, [setPatternDropHandler]);

  let dropGhost: Clip | null = null;
  if (patternDrag) {
    const tick = barAt(patternDrag.clientX, patternDrag.clientY);
    const p = model.patterns.find((x) => x.id === patternDrag.patternId);
    if (tick !== null && p) {
      dropGhost = { id: "__drop", patternId: p.id, startTick: tick, lengthTicks: Math.min(p.lengthTicks, songEnd - tick), offsetTicks: 0, muted: false };
    }
  }

  // ── Clip editing ───────────────────────────────────────────────────────────────────────
  const drag = useRef<Drag | null>(null);
  const [preview, setPreview] = useState<Clip[] | null>(null);
  const [box, setBox] = useState<Box | null>(null);
  const [menu, setMenu] = useState<{ clipId: string; x: number; y: number } | null>(null);

  /** Lane-space point: x from the timeline's start, y from the first lane's top. */
  const lanePoint = (e: { clientX: number; clientY: number }) => {
    const r = lanesRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left - HEAD_W, y: e.clientY - r.top };
  };

  /**
   * New clips copying `src` at the given starts (offset and mute carried over). One transient
   * run closed by `commitGesture`, so the whole copy is one undo step. The copies end up selected.
   */
  const addCopies = async (copies: { src: Clip; startTick: number }[], label: string) => {
    const st = useStore.getState();
    const made: string[] = [];
    for (const { src, startTick } of copies) {
      if (startTick < 0 || startTick >= songEnd) continue;
      const before = new Set(useStore.getState().snapshot!.model.clips.map((c) => c.id));
      await st.dispatch({ type: "addClip", patternId: src.patternId, startTick, lengthTicks: src.lengthTicks }, true, label);
      const copy = useStore.getState().snapshot!.model.clips.find((c) => !before.has(c.id));
      if (!copy) continue;
      made.push(copy.id);
      if (src.offsetTicks !== 0 || src.muted) {
        await st.dispatch({ type: "updateClip", clip: { ...copy, offsetTicks: src.offsetTicks, muted: src.muted } }, true, label);
      }
    }
    await st.commitGesture();
    if (made.length) st.selectClips(made);
  };
  // The key handler below is registered once; it reaches the current closure through this.
  const addCopiesRef = useRef(addCopies);
  addCopiesRef.current = addCopies;

  const onClipDown = (e: React.PointerEvent, clip: Clip, edge: boolean) => {
    e.stopPropagation();
    if (e.button !== 0) return;
    setMenu(null);
    const selected = useStore.getState().selectedClipIds;
    const el = e.currentTarget as HTMLElement;
    if (edge) {
      // A trim touches only this clip, whatever else is selected.
      if (!selected.includes(clip.id)) s.selectClips([clip.id]);
      s.selectPattern(clip.patternId);
      el.setPointerCapture(e.pointerId);
      drag.current = { kind: "trim", clip, x0: e.clientX };
      return;
    }
    let ids: string[];
    let collapseTo: string | null = null;
    if (e.shiftKey) {
      ids = toggleId(selected, clip.id);
      s.selectClips(ids);
      if (!ids.includes(clip.id)) return;
    } else if (selected.includes(clip.id)) {
      // Keep the selection so it can move as a group; a plain click narrows it on release.
      ids = selected;
      collapseTo = clip.id;
    } else {
      ids = [clip.id];
      s.selectClips(ids);
    }
    s.selectPattern(clip.patternId);
    el.setPointerCapture(e.pointerId);
    const group = model.clips.filter((c) => ids.includes(c.id));
    drag.current = { kind: "move", anchor: clip, group, x0: e.clientX, dup: e.altKey, moved: false, collapseTo };
  };

  const onLaneDown = (e: React.PointerEvent, track: Track) => {
    if (e.button !== 0) return;
    setMenu(null);
    const p = lanePoint(e);
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    const base = e.shiftKey ? useStore.getState().selectedClipIds : [];
    drag.current = { kind: "marquee", trackId: track.id, x0: p.x, y0: p.y, base, additive: e.shiftKey, moved: false };
  };

  // Captured pointer events bubble up here from whichever clip or lane took the press.
  const onPointerMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    if (d.kind === "marquee") {
      const p = lanePoint(e);
      if (!d.moved && Math.hypot(p.x - d.x0, p.y - d.y0) <= DRAG_PX) return;
      d.moved = true;
      setBox({ x0: d.x0, y0: d.y0, x1: p.x, y1: p.y });
      const hit = clipsInBox(model.clips, laneOf, trackAtY(d.y0), trackAtY(p.y), d.x0 / pxPerTick, p.x / pxPerTick);
      s.selectClips([...d.base, ...hit]);
      return;
    }
    const dx = e.clientX - d.x0;
    // ⇧ drops to whole ticks for this drag, whatever the snap select says.
    const g = snapGrid(arrSnap, barTick, e.shiftKey);
    if (d.kind === "move") {
      if (!d.moved && Math.abs(dx) <= DRAG_PX) return;
      d.moved = true;
      // Horizontal only: a clip never leaves its own track's lane.
      setPreview(moveClips(d.group, groupMoveDelta(d.anchor, d.group, dx / pxPerTick, g, songEnd)));
    } else {
      setPreview([trimClip(d.clip, d.clip.lengthTicks + dx / pxPerTick, g, songEnd)]);
    }
  };

  const onPointerUp = () => {
    const d = drag.current;
    drag.current = null;
    const result = preview;
    setPreview(null);
    setBox(null);
    if (!d) return;
    if (d.kind === "move") {
      if (!d.moved) {
        if (d.collapseTo) s.selectClips([d.collapseTo]);
        return;
      }
      if (!result || result[0].startTick === d.group[0].startTick) return;
      const many = d.group.length > 1;
      if (d.dup) {
        void addCopies(
          d.group.map((src, i) => ({ src, startTick: result[i].startTick })),
          many ? "copy clips" : "copy clip",
        );
      } else {
        // One command for the whole group, so one undo step.
        void dispatch({ type: "updateClips", clips: result }, false, many ? "move clips" : "move clip");
      }
    } else if (d.kind === "trim") {
      if (result && result[0].lengthTicks !== d.clip.lengthTicks) void dispatch({ type: "updateClip", clip: result[0] });
    } else if (!d.moved) {
      // A click on empty lane space selects that track and drops the clip selection.
      s.selectTrack(d.trackId);
      if (!d.additive) s.selectClips([]);
    }
  };

  const onPointerCancel = () => {
    drag.current = null;
    setPreview(null);
    setBox(null);
  };

  // ── Section markers ────────────────────────────────────────────────────────────────────
  const sectionDrag = useRef<{ section: Section; x0: number } | null>(null);
  const [sectionPreview, setSectionPreview] = useState<Section | null>(null);
  const [editingSection, setEditingSection] = useState<string | null>(null);

  const onSectionDown = (e: React.PointerEvent, section: Section) => {
    e.stopPropagation();
    if (e.button === 2) {
      void dispatch({ type: "removeSection", id: section.id });
      return;
    }
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    sectionDrag.current = { section, x0: e.clientX };
  };
  const onSectionMove = (e: React.PointerEvent) => {
    const d = sectionDrag.current;
    if (!d) return;
    const dTicks = (e.clientX - d.x0) / pxPerTick;
    if (Math.abs(e.clientX - d.x0) > DRAG_PX) setSectionPreview({ ...d.section, startTick: Math.max(0, snapTo(d.section.startTick + dTicks, grid)) });
  };
  const onSectionUp = (e: React.PointerEvent) => {
    const d = sectionDrag.current;
    if (!d) return;
    sectionDrag.current = null;
    const moved = sectionPreview;
    setSectionPreview(null);
    if (moved && moved.startTick !== d.section.startTick) void dispatch({ type: "updateSection", section: moved });
    else if (Math.abs(e.clientX - d.x0) <= DRAG_PX) setEditingSection(editingSection === d.section.id ? null : d.section.id);
  };

  // ── Loop brace ─────────────────────────────────────────────────────────────────────────
  // The song's brace is saved with the project, so a drag is an edit: it goes in on release as
  // one undo step, and this holds the rubber band meanwhile.
  const loopDrag = useRef<{ anchor: number } | null>(null);
  const [loopPreview, setLoopPreview] = useState<[number, number] | null>(null);
  const songLoop = loopPreview ?? model.loopRegion ?? null;

  const onLoopDown = (e: React.PointerEvent) => {
    const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
    if (e.button === 2) {
      void s.setSongLoop(null);
      return;
    }
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    loopDrag.current = { anchor: Math.max(0, Math.min(songEnd, snapTo(x / pxPerTick, snapGrid(arrSnap, barTick, e.shiftKey)))) };
  };
  const onLoopMove = (e: React.PointerEvent) => {
    const d = loopDrag.current;
    if (!d) return;
    const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
    const tick = Math.max(0, Math.min(songEnd, snapTo(x / pxPerTick, snapGrid(arrSnap, barTick, e.shiftKey))));
    setLoopPreview(tick === d.anchor ? null : [Math.min(d.anchor, tick), Math.max(d.anchor, tick)]);
  };
  const onLoopUp = () => {
    if (!loopDrag.current) return;
    loopDrag.current = null;
    const region = loopPreview;
    setLoopPreview(null);
    // A press that never moved leaves the brace alone; clearing is the right-click.
    if (region) void s.setSongLoop(region);
  };

  // ── Punch range ────────────────────────────────────────────────────────────────────────
  // Same gesture as the brace, one strip down, and the same deal: saved with the project, so
  // the drag is an edit that lands on release.
  const punchDrag = useRef<{ anchor: number } | null>(null);
  const [punchPreview, setPunchPreview] = useState<[number, number] | null>(null);
  const punch = punchPreview ?? model.punchRegion ?? null;

  const onPunchDown = (e: React.PointerEvent) => {
    const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
    if (e.button === 2) {
      void s.setPunch(null);
      return;
    }
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    punchDrag.current = { anchor: Math.max(0, Math.min(songEnd, snapTo(x / pxPerTick, snapGrid(arrSnap, barTick, e.shiftKey)))) };
  };
  const onPunchMove = (e: React.PointerEvent) => {
    const d = punchDrag.current;
    if (!d) return;
    const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
    const tick = Math.max(0, Math.min(songEnd, snapTo(x / pxPerTick, snapGrid(arrSnap, barTick, e.shiftKey))));
    setPunchPreview(tick === d.anchor ? null : [Math.min(d.anchor, tick), Math.max(d.anchor, tick)]);
  };
  const onPunchUp = () => {
    if (!punchDrag.current) return;
    punchDrag.current = null;
    const region = punchPreview;
    setPunchPreview(null);
    if (region) void s.setPunch(region);
  };

  // ── Audio clip fades ───────────────────────────────────────────────────────────────────
  // The one part of a take the song view edits, because it is where a punch leaves its seams:
  // the crossfades land on these corners and are dragged from here. Transient while dragging
  // and closed by `commitGesture`, so one drag is one undo step, the way a trim is.
  // Audio does not warp, so a clip's width is its seconds at the project tempo — the same
  // conversion the lane already draws it with.
  const ticksToSeconds = (t: number) => ((t / TICKS_PER_BEAT) * 60) / model.tempo;
  const secondsToTicks = (sec: number) => ((sec * model.tempo) / 60) * TICKS_PER_BEAT;
  const fadeDrag = useRef<{ clip: AudioClip; edge: "in" | "out" } | null>(null);
  const [fadePreview, setFadePreview] = useState<AudioClip | null>(null);

  const onFadeDown = (e: React.PointerEvent, clip: AudioClip, edge: "in" | "out") => {
    e.stopPropagation();
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    s.selectAudioClip(clip.id);
    fadeDrag.current = { clip, edge };
    setFadePreview(clip);
  };
  const onFadeMove = (e: React.PointerEvent) => {
    const d = fadeDrag.current;
    if (!d) return;
    e.stopPropagation();
    const tick = lanePoint(e).x / pxPerTick;
    const from =
      d.edge === "in"
        ? tick - d.clip.startTick
        : d.clip.startTick + secondsToTicks(d.clip.lengthSeconds) - tick;
    const fade = clampFade(ticksToSeconds(from), d.clip.lengthSeconds);
    const next: AudioClip =
      d.edge === "in" ? { ...d.clip, fadeInSeconds: fade } : { ...d.clip, fadeOutSeconds: fade };
    setFadePreview(next);
    void dispatch({ type: "updateAudioClip", clip: next }, true, "fade take");
  };
  const onFadeUp = (e: React.PointerEvent) => {
    if (!fadeDrag.current) return;
    e.stopPropagation();
    fadeDrag.current = null;
    setFadePreview(null);
    void s.commitGesture();
  };

  // ── Ripple: insert, delete and duplicate bars ──────────────────────────────────────────
  // The ruler's right-click. Each of these is one command, so each is one undo step however
  // many clips, markers, tempo points, chords and automation points it carries.
  const [rulerMenu, setRulerMenu] = useState<{ tick: number; x: number; y: number } | null>(null);
  /** The bars the menu's range actions work on: the loop brace, which is the song's own "this bit". */
  const rippleRange = model.loopRegion ?? null;

  const onRulerMenu = (e: React.MouseEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    // The marker strip removes a section, the brace strip clears the loop and the punch strip
    // clears the punch; this is the bar ruler below them all.
    if (e.clientY - rect.top < MARKER_H + LOOP_H + PUNCH_H) return;
    e.preventDefault();
    const tick = Math.max(0, Math.floor((e.clientX - rect.left) / pxPerTick / barTick) * barTick);
    setRulerMenu({ tick, x: e.clientX, y: e.clientY });
  };

  // The song view's own editing keys. App binds none of these and the roll isn't mounted in
  // SONG, so a window listener is safe here; it still stands down for text fields, open dialogs
  // and the other screens.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTyping(e.target)) return;
      const st = useStore.getState();
      if (st.screen !== "song" || st.dialog || st.settingsOpen || st.newProjectOpen || st.exportOpen) return;
      if (e.key === "Escape") {
        setMenu(null);
        if (st.selectedClipIds.length) st.selectClips([]);
        return;
      }
      const m = st.snapshot!.model;
      const end = songLength(m);
      const ids = st.selectedClipIds;
      const chosen = m.clips.filter((c) => ids.includes(c.id));
      const mod = e.metaKey || e.ctrlKey;
      const k = e.key.toLowerCase();
      if (mod && !e.altKey) {
        if (k === "a") {
          e.preventDefault();
          setMenu(null);
          st.selectClips(m.clips.map((c) => c.id));
        } else if (k === "c") {
          if (chosen.length) clipboard = chosen;
        } else if (k === "x") {
          if (!chosen.length) return;
          clipboard = chosen;
          setMenu(null);
          void st.dispatch({ type: "removeClips", ids }, false, chosen.length > 1 ? "cut clips" : "cut clip");
        } else if (k === "v") {
          // At the playhead, each copy back on its own pattern's track. The grid is read here
          // rather than captured: this listener is registered once, for the screen's lifetime.
          const g = snapGrid(st.arrSnap, barTicks(st));
          const at = pasteClips(clipboard, Math.floor(st.playhead / g) * g, end);
          if (!at.length) return;
          e.preventDefault();
          setMenu(null);
          void addCopiesRef.current(at, at.length > 1 ? "paste clips" : "paste clip");
        } else if (k === "d") {
          const at = duplicateClips(chosen, end);
          if (!at.length) return;
          e.preventDefault();
          setMenu(null);
          void addCopiesRef.current(at, at.length > 1 ? "duplicate clips" : "duplicate clip");
        }
        return;
      }
      if (e.altKey || mod) return;
      if (e.key === "0") {
        // Same key as the roll's note mute, on the same kind of selection.
        if (!chosen.length) return;
        e.preventDefault();
        setMenu(null);
        const next = toggleMuted(chosen);
        void st.dispatch({ type: "updateClips", clips: next }, false, `${next[0].muted ? "mute" : "unmute"} ${next.length > 1 ? "clips" : "clip"}`);
        return;
      }
      if (e.key !== "Delete" && e.key !== "Backspace") return;
      if (!ids.length) return;
      e.preventDefault();
      setMenu(null);
      void st.dispatch({ type: "removeClips", ids }, false, ids.length > 1 ? "delete clips" : "delete clip");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // ── Follow ─────────────────────────────────────────────────────────────────────────────
  // Page the view when the playhead leaves it, the way the roll does. Two things stand it
  // down: a gesture in progress (the view must not move under the clip being dragged) and a
  // hand scroll, which parks following until the playhead comes back into view by itself.
  const autoScrolledTo = useRef<number | null>(null);
  const lastLeft = useRef(0);
  const parked = useRef(false);
  const onScroll = (e: React.UIEvent<HTMLDivElement>) => {
    if (menu) setMenu(null);
    const left = e.currentTarget.scrollLeft;
    const mine = autoScrolledTo.current !== null && Math.abs(left - autoScrolledTo.current) < 1;
    // Scrolling the lanes vertically is not walking away from the playhead, so only a sideways
    // move the follow effect did not make parks it.
    if (!mine && left !== lastLeft.current) parked.current = true;
    if (mine) autoScrolledTo.current = null;
    lastLeft.current = left;
  };
  useEffect(() => {
    // A fresh press of play always follows again, however far the view had wandered.
    if (playing) parked.current = false;
  }, [playing]);
  useEffect(() => {
    const el = scrollerRef.current;
    if (!el || !follow || !playing || drag.current || loopDrag.current || sectionDrag.current) return;
    const px = HEAD_W + playhead * pxPerTick;
    if (px >= el.scrollLeft + HEAD_W && px <= el.scrollLeft + el.clientWidth - 24) {
      parked.current = false;
      return;
    }
    if (parked.current) return;
    const to = Math.max(0, px - HEAD_W - 16);
    autoScrolledTo.current = to;
    el.scrollLeft = to;
  }, [playhead, follow, playing, pxPerTick, HEAD_W]);

  // ── What's drawn ───────────────────────────────────────────────────────────────────────
  const dup = drag.current?.kind === "move" && drag.current.dup;
  const previewIds = new Set(preview?.map((c) => c.id));
  const selectedIds = new Set(selectedClipIds);
  // A copy drag leaves the originals in place and draws the copies as ghosts.
  const clipsShown = dup || !preview ? model.clips : model.clips.map((c) => preview.find((p) => p.id === c.id) ?? c);
  const extras: { clip: Clip; kind: "copy" | "drop" }[] = [
    ...(dup && preview ? preview.map((c) => ({ clip: { ...c, id: `copy-${c.id}` }, kind: "copy" as const })) : []),
    ...(dropGhost ? [{ clip: dropGhost, kind: "drop" as const }] : []),
  ];
  const audioClips = model.audioClips ?? [];
  const heard = detectedChords(model);
  const sectionsShown = model.sections.map((x) => (sectionPreview && x.id === sectionPreview.id ? sectionPreview : x));
  // A marker drag previews where it would land, so the bands are measured against that list.
  const sectionModel = { ...model, sections: sectionsShown };

  const menuClip = menu ? model.clips.find((c) => c.id === menu.clipId) : undefined;
  // The menu acts on the whole selection when the clicked clip is part of it, else on that clip
  // alone — the same rule the right-click itself follows when it opens.
  const menuTargets = menuClip ? (selectedIds.has(menuClip.id) ? model.clips.filter((c) => selectedIds.has(c.id)) : [menuClip]) : [];
  const playX = HEAD_W + playhead * pxPerTick;

  return (
    <div className="arr song" style={{ "--head-w": `${HEAD_W}px` } as React.CSSProperties}>
      {/* Above the track column's drag handle, which stretches the full height of this box:
          the handle belongs to the lanes below, not to the controls. */}
      <div className="toolbar" style={{ position: "relative", zIndex: 7 }}>
        <div className="opt">
          <span className="k">snap</span>
          <select
            value={String(arrSnap)}
            onChange={(e) => s.setArrSnap(SNAP_OPTIONS.find(([, v]) => String(v) === e.target.value)?.[1] ?? "bar")}
            title={keys("the grid clips move, trim and drop on · ⇧ while dragging ignores it")}
          >
            {SNAP_OPTIONS.map(([label, v]) => (
              <option key={label} value={String(v)}>{label}</option>
            ))}
          </select>
        </div>
        <button className={`chip${follow ? " on" : ""}`} onClick={() => s.toggleFollow()} title="follow: keep the playhead in view while playing">follow</button>
        <div className="vdiv" />
        {/* The transport owns the loop on/off switch; this says what is braced, in bars. */}
        <span
          className="mono"
          style={{ fontSize: 9.5, color: looping && !punch ? "var(--text-4)" : "var(--text-6)" }}
          title={keys(
            punch
              ? "looping is blocked while a punch is armed — clear the punch range to loop again"
              : "drag the strip under the ruler's markers to set this · right-click it to clear · L loops",
          )}
        >
          loop {songLoop ? `${+(songLoop[0] / barTick + 1).toFixed(2)} → ${+(songLoop[1] / barTick + 1).toFixed(2)}` : "whole song"}
          {punch ? " (blocked by punch)" : looping ? "" : " (off)"}
        </span>
        {songLoop && (
          <button className="chip tiny" onClick={() => void s.setSongLoop(null)} title="clear the loop brace">clear</button>
        )}
        <div className="vdiv" />
        <span
          className="mono"
          style={{ fontSize: 9.5, color: punch ? "var(--rec-text)" : "var(--text-6)" }}
          title="drag the strip under the loop brace to set the punch range · right-click it to clear · recording drops in and out on these points"
        >
          punch {punch ? `${+(punch[0] / barTick + 1).toFixed(2)} → ${+(punch[1] / barTick + 1).toFixed(2)}` : "off"}
        </span>
        {punch && (
          <button className="chip tiny" onClick={() => void s.setPunch(null)} title="clear the punch range and let the loop work again">clear</button>
        )}
        <span className="spacer" />
        <span className="mono" style={{ fontSize: 9.5, color: "var(--text-4)" }}>
          {model.clips.length} clips{selectedClipIds.length ? ` · ${selectedClipIds.length} sel` : ""}
        </span>
        <span
          className="chip"
          style={{ cursor: "help" }}
          title={keys(
            "click a clip to select · ⇧-click toggles · drag empty lane space to marquee\ndrag: move · right edge: trim or loop-extend · ⌥-drag: copy · ⇧: ignore the snap grid\n⌘A ⌘C ⌘X ⌘V (at the playhead) ⌘D · 0 mute · ⌫ delete · right-click: more\nruler: click to seek · drag the strip under the markers to set the loop · right-click it to clear",
          )}
        >
          ?
        </span>
      </div>
      <div className="headseam" style={{ left: HEAD_W }}>
        <Splitter panel="songHead" label="the track column" />
      </div>
      <div className="scroller" ref={scrollerRef} onScroll={onScroll}>
        <div className="ruler" style={{ width: rowW }}>
          <div className="corner">
            <div className="r">
              <span className="cap" style={{ flex: 1, whiteSpace: "nowrap" }}>
                {model.tracks.length} tracks
              </span>
              <button className="chip tiny" onClick={() => dispatch({ type: "addTrack", isDrum: false })}>+ inst</button>
              <button className="chip tiny" onClick={() => dispatch({ type: "addTrack", isDrum: true })}>+ drum</button>
            </div>
            <div className="r">
              <span className="mono" style={{ color: "var(--text-5)" }} title="song length: change it in SETUP">
                {+(songEnd / barTick).toFixed(2)} bars
              </span>
              <span className="spacer" />
              <button className="ico" onClick={() => setArrZoom(pxPerBar / 1.25)}>−</button>
              <span className="mono" style={{ color: "var(--text-5)" }}>1:{Math.round(pxPerBar)}</span>
              <button className="ico" onClick={() => setArrZoom(pxPerBar * 1.25)}>+</button>
            </div>
          </div>
          <div
            className="timeline"
            style={{ width: timelineW }}
            onPointerDown={(e) => {
              const rect = e.currentTarget.getBoundingClientRect();
              const y = e.clientY - rect.top;
              if (y < MARKER_H) return; // marker strip
              if (y < MARKER_H + LOOP_H) {
                onLoopDown(e);
                return;
              }
              if (y < MARKER_H + LOOP_H + PUNCH_H) {
                onPunchDown(e);
                return;
              }
              if (e.button !== 0) return;
              s.seek(Math.min(songEnd, Math.max(0, (e.clientX - rect.left) / pxPerTick)));
            }}
            onPointerMove={(e) => {
              onLoopMove(e);
              onPunchMove(e);
            }}
            onPointerUp={() => {
              onLoopUp();
              onPunchUp();
            }}
            onPointerCancel={() => {
              loopDrag.current = null;
              setLoopPreview(null);
              punchDrag.current = null;
              setPunchPreview(null);
            }}
            onDoubleClick={(e) => {
              const rect = e.currentTarget.getBoundingClientRect();
              if (e.clientY - rect.top >= MARKER_H) return;
              void dispatch({ type: "addSection", name: "", startTick: Math.max(0, snapTo((e.clientX - rect.left) / pxPerTick, grid)) });
            }}
            onContextMenu={onRulerMenu}
            title="click to seek · drag the strip under the markers to loop · drag the one under that to set the punch range · right-click either to clear it · double-click the top strip to add a section · right-click the bars to insert, delete or duplicate time"
          >
            <div className="oob" style={{ left: endX }} />
            {sectionsShown.map((sec, i) => {
              // The band is the section's real extent, never a guess: a marker that claimed
              // music it does not own would lie about what moving it moves.
              const end = sectionSpan(sectionModel, sec)[1];
              const color = sec.color ?? SECTION_COLORS[i % SECTION_COLORS.length];
              return (
                <div
                  key={sec.id}
                  className="marker"
                  style={{ left: sec.startTick * pxPerTick, width: Math.max(24, (end - sec.startTick) * pxPerTick - 2), borderLeftColor: color, background: `${color}1a`, color }}
                  onPointerDown={(e) => onSectionDown(e, sec)}
                  onPointerMove={onSectionMove}
                  onPointerUp={onSectionUp}
                  onDoubleClick={(e) => e.stopPropagation()}
                  title={`${sec.name}${sec.key?.rootPitchClass != null ? ` · ${noteName(sec.key.rootPitchClass)} ${sec.key.scale}` : ""} · click: edit · drag: move · right-click: remove`}
                >
                  {sec.name}
                  {sec.key?.rootPitchClass != null && (
                    <span className="mono" style={{ marginLeft: 6, opacity: 0.8, textTransform: "none" }}>
                      {noteName(sec.key.rootPitchClass)} {sec.key.scale === "major" ? "maj" : "min"}
                    </span>
                  )}
                  {editingSection === sec.id && <SectionEditor section={sec} color={color} onClose={() => setEditingSection(null)} />}
                </div>
              );
            })}
            {Array.from({ length: bars }, (_, i) => (
              <div key={i} className={`bar${i % 4 === 0 ? " strong" : ""}${i * barTick >= songEnd ? " past" : ""}`} style={{ left: i * pxPerBar }}>
                {pxPerBar >= 22 || i % 4 === 0 ? i + 1 : ""}
              </div>
            ))}
            {/* The brace strip: a faint bed the whole width so the gesture is discoverable,
                with the region itself drawn over it in the accent colour. */}
            <div style={{ position: "absolute", left: 0, top: MARKER_H + 2, width: endX, height: LOOP_H - 4, background: "var(--line-1)", opacity: 0.5, borderRadius: 3, pointerEvents: "none" }} />
            {songLoop && (
              <div
                style={{
                  position: "absolute",
                  left: songLoop[0] * pxPerTick,
                  top: MARKER_H + 2,
                  width: Math.max(2, (songLoop[1] - songLoop[0]) * pxPerTick),
                  height: LOOP_H - 4,
                  background: "var(--accent)",
                  // Off — or blocked by an armed punch — the brace is still where it was, but
                  // nothing is looping to it.
                  opacity: looping && !punch ? 0.85 : 0.3,
                  borderRadius: 3,
                  pointerEvents: "none",
                }}
              />
            )}
            {/* The punch strip, in the record colour so it never reads as a second loop. */}
            <div style={{ position: "absolute", left: 0, top: MARKER_H + LOOP_H + 2, width: endX, height: PUNCH_H - 4, background: "var(--line-1)", opacity: 0.5, borderRadius: 3, pointerEvents: "none" }} />
            {punch && (
              <div
                style={{
                  position: "absolute",
                  left: punch[0] * pxPerTick,
                  top: MARKER_H + LOOP_H + 2,
                  width: Math.max(2, (punch[1] - punch[0]) * pxPerTick),
                  height: PUNCH_H - 4,
                  background: "var(--rec)",
                  opacity: 0.9,
                  borderRadius: 3,
                  pointerEvents: "none",
                }}
              />
            )}
            <div className="songend" style={{ left: endX }} title="the song ends here: change its length in SETUP">
              end
            </div>
            <div className="phtri" style={{ left: playhead * pxPerTick }} />
          </div>
        </div>

        {/* ChordLane brings its own 64px gutter; the head cell fills the rest of the header column. */}
        <div className="songchords" style={{ width: rowW, "--sc-head": `${HEAD_W - GUTTER}px` } as React.CSSProperties}>
          <div className="schead">
            <button
              className={`chip tiny${hearChords ? " on" : ""}`}
              onClick={() => s.toggleHearChords()}
              title="play the chord progression along while you listen (never exported)"
            >
              hear chords
            </button>
          </div>
          <ChordLane offsetTicks={0} lengthTicks={songEnd} beatW={pxPerBeat} readOnly onEditInSetup={() => s.setScreen("setup")} />
          <div className="oob" style={{ left: HEAD_W + endX }} />
        </div>

        {/* Chords heard in recorded takes, under the progression they may join. Only there
            while something is waiting to be reviewed. */}
        {heard.length > 0 && (
          <div className="songchords detected" style={{ width: rowW, "--sc-head": `${HEAD_W - GUTTER}px` } as React.CSSProperties}>
            <div className="schead">
              <span className="cap" title="chords detected in recorded DI: review each one">
                {heard.length} to review
              </span>
            </div>
            <DetectedChordLane lengthTicks={songEnd} beatW={pxPerBeat} />
            <div className="oob" style={{ left: HEAD_W + endX }} />
          </div>
        )}

        <TempoLane headW={HEAD_W} timelineW={timelineW} pxPerBeat={pxPerBeat} bpb={bpb} />

        <div className="lanes" ref={lanesRef} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerCancel}>
          {model.tracks.map((track, ti) => {
            const color = trackColor(track, ti);
            const own = (c: Clip) => trackOfPattern.get(c.patternId) === track.id;
            return (
              <TrackRow
                key={track.id}
                track={track}
                index={ti}
                color={color}
                selected={track.id === selectedTrackId}
                peak={trackPeaks[ti] ?? 0}
                headW={HEAD_W}
                timelineW={timelineW}
                endX={endX}
                pxPerBar={pxPerBar}
                bpb={bpb}
                autoParam={autoShown?.trackId === track.id ? autoShown.param : null}
                onAutoParam={(param) => setAutoShown(param ? { trackId: track.id, param } : null)}
                onLaneDown={(e) => onLaneDown(e, track)}
              >
                {track.isAudio
                  ? // An audio track's lane shows its own takes. Only the fades are editable
                    // here; trimming, gain and the rig live on the rec page.
                    audioClips
                      .filter((c) => c.trackId === track.id)
                      .map((raw) => {
                        const c = fadePreview?.id === raw.id ? fadePreview : raw;
                        const source = (model.audioSources ?? []).find((src) => src.id === c.sourceId);
                        const widthTicks = secondsToTicks(c.lengthSeconds);
                        const w = Math.max(6, widthTicks * pxPerTick);
                        return (
                          <div
                            key={c.id}
                            className={`clip audio${c.muted ? " muted" : ""}`}
                            style={{ left: c.startTick * pxPerTick, width: w, height: CLIP_H, borderColor: color }}
                            title={`${source?.name ?? "missing audio"} · ${c.lengthSeconds.toFixed(1)}s — drag a top corner to fade · double-click to open it on the rec page`}
                            onDoubleClick={() => (s.selectTrack(track.id), s.selectAudioClip(c.id), s.setScreen("rec"))}
                          >
                            <span className="name">{source?.name ?? "missing audio"}</span>
                            {source && (
                              <button
                                className="chip tiny detect"
                                disabled={s.detectingClipId !== null}
                                title="transcribe this take's raw DI: single notes onto a new synth track (bends kept), strummed parts refused and offered as chords above"
                                onPointerDown={(e) => e.stopPropagation()}
                                onDoubleClick={(e) => e.stopPropagation()}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  void s.detectNotes(c.id);
                                }}
                              >
                                {s.detectingClipId === c.id ? "listening…" : "notes"}
                              </button>
                            )}
                            <FadeHandles
                              clip={c}
                              width={w}
                              height={CLIP_H}
                              pxPerSecond={secondsToTicks(1) * pxPerTick}
                              onDown={onFadeDown}
                              onMove={onFadeMove}
                              onUp={onFadeUp}
                            />
                          </div>
                        );
                      })
                  : clipsShown.filter(own).map((clip) => {
                      const pattern = model.patterns.find((p) => p.id === clip.patternId);
                      if (!pattern) return null;
                      return (
                        <ClipView
                          key={clip.id}
                          clip={clip}
                          pattern={pattern}
                          track={track}
                          color={color}
                          pxPerBeat={pxPerBeat}
                          selected={selectedIds.has(clip.id)}
                          samePattern={pattern.id === selectedPatternId}
                          dragging={!dup && previewIds.has(clip.id)}
                          onDown={onClipDown}
                          onContextMenu={(e) => {
                            e.preventDefault();
                            e.stopPropagation();
                            if (!selectedIds.has(clip.id)) s.selectClips([clip.id]);
                            s.selectPattern(clip.patternId);
                            setMenu({ clipId: clip.id, x: e.clientX, y: e.clientY });
                          }}
                          onDoubleClick={() => s.openPattern(pattern.id, clip.startTick)}
                        />
                      );
                    })}
                {extras
                  .filter((x) => own(x.clip))
                  .map(({ clip, kind }) => (
                    <ClipView
                      key={clip.id}
                      clip={clip}
                      pattern={model.patterns.find((p) => p.id === clip.patternId)!}
                      track={track}
                      color={color}
                      pxPerBeat={pxPerBeat}
                      ghost={kind}
                    />
                  ))}
              </TrackRow>
            );
          })}
          {box && (
            <div
              className="marquee"
              style={{
                left: HEAD_W + Math.min(box.x0, box.x1),
                top: Math.min(box.y0, box.y1),
                width: Math.abs(box.x1 - box.x0),
                height: Math.abs(box.y1 - box.y0),
              }}
            />
          )}
        </div>

        <div className="addrow" style={{ width: rowW }}>
          <div />
          <div className="songhint">
            {model.tracks.length === 0
              ? "add a track in the tree on the left"
              : model.clips.length === 0
                ? "← drag a pattern from the left into its track's lane"
                : keys("shift-click or drag across lanes to select more · ⌥-drag to copy · right-click for more")}
          </div>
        </div>
        <div className="playline" style={{ left: playX }} />
      </div>

      {menu && menuClip && (
        <ClipMenu
          x={menu.x}
          y={menu.y}
          count={menuTargets.length}
          canDuplicate={duplicateClips(menuTargets, songEnd).length > 0}
          muted={menuTargets.every((c) => c.muted)}
          onClose={() => setMenu(null)}
          onMakeUnique={async () => {
            setMenu(null);
            await dispatch({ type: "makeClipUnique", id: menuClip.id });
            // The clip keeps its id and now points at the fork: select that pattern.
            const forked = useStore.getState().snapshot!.model.clips.find((c) => c.id === menuClip.id);
            if (forked) {
              s.selectPattern(forked.patternId);
              s.selectClips([forked.id]);
            }
          }}
          onToggleMute={() => {
            setMenu(null);
            const next = toggleMuted(menuTargets);
            void dispatch({ type: "updateClips", clips: next }, false, `${next[0].muted ? "mute" : "unmute"} ${next.length > 1 ? "clips" : "clip"}`);
          }}
          onCopy={() => {
            setMenu(null);
            clipboard = menuTargets;
          }}
          onDuplicate={() => {
            setMenu(null);
            void addCopies(duplicateClips(menuTargets, songEnd), menuTargets.length > 1 ? "duplicate clips" : "duplicate clip");
          }}
          onDelete={() => {
            setMenu(null);
            const ids = menuTargets.map((c) => c.id);
            void dispatch({ type: "removeClips", ids }, false, ids.length > 1 ? "delete clips" : "delete clip");
          }}
        />
      )}
      {rulerMenu && (
        <RulerMenu
          x={rulerMenu.x}
          y={rulerMenu.y}
          tick={rulerMenu.tick}
          barTick={barTick}
          range={rippleRange}
          onClose={() => setRulerMenu(null)}
          onInsert={(bars) => {
            setRulerMenu(null);
            void dispatch({ type: "insertTime", at: rulerMenu.tick, ticks: bars * barTick }, false, `insert ${bars} bar${bars === 1 ? "" : "s"}`);
          }}
          onDelete={(at, ticks) => {
            setRulerMenu(null);
            const bars = Math.round(ticks / barTick);
            void dispatch({ type: "deleteTime", at, ticks }, false, `delete ${bars} bar${bars === 1 ? "" : "s"}`);
          }}
          onDuplicate={(from, to) => {
            setRulerMenu(null);
            void dispatch({ type: "duplicateRange", from, to, at: rulerMenu.tick }, false, "duplicate bars");
          }}
        />
      )}
    </div>
  );
}

/** A menu pinned at the pointer closes on any press outside it, and when the window blurs. */
function usePinnedMenu(onClose: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) closeRef.current();
    };
    const onBlur = () => closeRef.current();
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("blur", onBlur);
    };
  }, []);
  return ref;
}

/**
 * The right-click menu on the bar ruler: the arranging moves that change *when* the song
 * happens. Insert opens bars at the clicked bar and pushes everything after it along; delete
 * and duplicate work on the loop brace, which is how this screen already says "these bars".
 */
function RulerMenu({
  x,
  y,
  tick,
  barTick,
  range,
  onClose,
  onInsert,
  onDelete,
  onDuplicate,
}: {
  x: number;
  y: number;
  /** The clicked bar's start, in ticks. */
  tick: number;
  barTick: number;
  /** The loop brace, or null when there is none. */
  range: [number, number] | null;
  onClose: () => void;
  onInsert: (bars: number) => void;
  onDelete: (at: number, ticks: number) => void;
  onDuplicate: (from: number, to: number) => void;
}) {
  const ref = usePinnedMenu(onClose);
  const left = Math.min(x, window.innerWidth - 230);
  const top = Math.min(y, window.innerHeight - 200);
  const barOf = (t: number) => Math.floor(t / barTick) + 1;
  const here = barOf(tick);
  const rangeBars = range ? +((range[1] - range[0]) / barTick).toFixed(2) : 0;
  const spans = range ? `bars ${barOf(range[0])}–${barOf(range[1] - 1)}` : "";
  // The loop's own length as a fourth insert size, when it is not one of the three already here.
  const sizes = [1, 2, 4, 8].concat(rangeBars >= 1 && ![1, 2, 4, 8].includes(rangeBars) ? [rangeBars] : []);
  return (
    <div className="dropdown clipmenu" ref={ref} style={{ left, top, minWidth: 210 }}>
      <div className="item disabled">
        <span>bar {here}</span>
      </div>
      {sizes.map((bars) => (
        <div
          key={bars}
          className="item"
          onClick={() => onInsert(bars)}
          title={`open ${bars} empty bar${bars === 1 ? "" : "s"} here: everything from bar ${here} on moves that much later, in one undo step`}
        >
          <span>insert {bars} bar{bars === 1 ? "" : "s"} here</span>
        </div>
      ))}
      <div className="sep" />
      <div
        className={`item${range ? "" : " disabled"}`}
        onClick={range ? () => onDuplicate(range[0], range[1]) : undefined}
        title={range ? `copy ${spans} to bar ${here}, pushing what is there along` : "set the loop brace over the bars to duplicate"}
      >
        <span>{range ? `duplicate ${spans} here` : "duplicate the loop here"}</span>
      </div>
      <div
        className={`item danger${range ? "" : " disabled"}`}
        onClick={range ? () => onDelete(range[0], range[1] - range[0]) : undefined}
        title={range ? `cut ${spans} out of the song and close the gap` : "set the loop brace over the bars to delete"}
      >
        <span>{range ? `delete ${spans}` : "delete the loop's bars"}</span>
      </div>
    </div>
  );
}

/** The right-click menu on a clip, pinned at the pointer; a press anywhere else closes it. */
function ClipMenu({
  x,
  y,
  count,
  canDuplicate,
  muted,
  onClose,
  onMakeUnique,
  onToggleMute,
  onCopy,
  onDuplicate,
  onDelete,
}: {
  x: number;
  y: number;
  count: number;
  canDuplicate: boolean;
  /** True when every clip the menu acts on is already muted. */
  muted: boolean;
  onClose: () => void;
  onMakeUnique: () => void;
  onToggleMute: () => void;
  onCopy: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
}) {
  const ref = usePinnedMenu(onClose);
  // Keep it on screen near the window's right and bottom edges.
  const left = Math.min(x, window.innerWidth - 200);
  const top = Math.min(y, window.innerHeight - 170);
  const what = count > 1 ? `${count} clips` : "clip";
  return (
    <div className="dropdown clipmenu" ref={ref} style={{ left, top }}>
      <div className="item" onClick={onMakeUnique} title="copy this clip's pattern into a new one on the same track, so editing it changes only this clip">
        make unique
      </div>
      <div className="item" onClick={onToggleMute} title="a muted clip stays on the timeline and is drawn faint, but plays nothing">
        <span>{muted ? `unmute ${what}` : `mute ${what}`}</span>
        <span className="k">0</span>
      </div>
      <div className="item" onClick={onCopy}>
        <span>copy</span>
        <span className="k">{keys("⌘C")}</span>
      </div>
      <div className={`item${canDuplicate ? "" : " disabled"}`} onClick={canDuplicate ? onDuplicate : undefined}>
        <span>duplicate after</span>
        <span className="k">{keys("⌘D")}</span>
      </div>
      <div className="item danger" onClick={onDelete}>
        <span>{count > 1 ? `delete ${count} clips` : "delete"}</span>
        <span className="k">{keys("⌫")}</span>
      </div>
    </div>
  );
}

function TrackRow({
  track,
  index,
  color,
  selected,
  peak,
  headW,
  timelineW,
  endX,
  pxPerBar,
  bpb,
  autoParam,
  onAutoParam,
  onLaneDown,
  children,
}: {
  track: Track;
  index: number;
  color: string;
  selected: boolean;
  peak: number;
  headW: number;
  timelineW: number;
  endX: number;
  pxPerBar: number;
  bpb: number;
  /** The parameter whose sub-lane is open under this row, or null when it is closed. */
  autoParam: TrackParam | null;
  onAutoParam: (param: TrackParam | null) => void;
  onLaneDown: (e: React.PointerEvent) => void;
  children: React.ReactNode;
}) {
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  const selectTrack = useStore((s) => s.selectTrack);
  const sliderRef = useRef<HTMLDivElement>(null);
  const volPct = Math.min(100, (track.volume / 1.5) * 100);

  const setVolumeFromEvent = (e: React.PointerEvent) => {
    const rect = sliderRef.current!.getBoundingClientRect();
    const p = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    void dispatch({ type: "setTrackParam", id: track.id, param: "volume", value: p * 1.5 }, true);
  };

  const gridBg = {
    backgroundImage:
      "linear-gradient(90deg,#2a2a31 1px,transparent 1px),linear-gradient(90deg,#1e1e24 1px,transparent 1px),linear-gradient(90deg,#17171b 1px,transparent 1px)",
    backgroundSize: `${pxPerBar * 4}px 100%, ${pxPerBar}px 100%, ${pxPerBar / bpb}px 100%`,
  };

  return (
    <>
      <div className={`trow${selected ? " on" : ""}`} style={{ width: headW + timelineW + 12, height: ROW_H }}>
        <div className={`thead${selected ? " on" : ""}`} style={{ height: ROW_H }} onClick={() => selectTrack(track.id)}>
          <div className="strip" style={{ background: color }} />
          <div className="body">
            <div className="l1">
              <span className="num">{String(index + 1).padStart(2, "0")}</span>
              <span className={`name${track.muted ? " muted" : ""}`}>{track.name}</span>
              <span className="type">{trackKind(track)}</span>
            </div>
            <div className="l2" onClick={(e) => e.stopPropagation()}>
              <button className={`msr m${track.muted ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackMuted", id: track.id, muted: !track.muted })}>M</button>
              <button className={`msr s${track.soloed ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackSoloed", id: track.id, soloed: !track.soloed })}>S</button>
              <select
                className={`autosel${autoParam ? " on" : ""}`}
                value={autoParam ?? ""}
                onChange={(e) => onAutoParam((e.target.value || null) as TrackParam | null)}
                title="automation: the parameter drawn in a lane under this track"
              >
                <option value="">auto</option>
                {AUTO_PARAMS.map(([param, label]) => (
                  <option key={param} value={param}>{label}</option>
                ))}
              </select>
              <div
                className="minislider"
                ref={sliderRef}
                onPointerDown={(e) => {
                  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                  setVolumeFromEvent(e);
                }}
                onPointerMove={(e) => e.buttons === 1 && setVolumeFromEvent(e)}
                onPointerUp={() => void commitGesture()}
                onDoubleClick={() => void dispatch({ type: "setTrackParam", id: track.id, param: "volume", value: 1 })}
                title="volume · double-click resets"
              >
                <div className="fill" style={{ width: `${volPct}%` }} />
                <div className="handle" style={{ left: `${volPct}%` }} />
              </div>
              <span className="db">{dbText(track.volume)}</span>
            </div>
          </div>
          <div className="meters">
            <VMeter peak={peak} height={ROW_H - 12} />
            <VMeter peak={peak} height={ROW_H - 12} />
          </div>
        </div>
        <div className={`lane${selected ? " on" : ""}`} style={{ width: timelineW, height: ROW_H, ...gridBg }} onPointerDown={onLaneDown}>
          <div className="oob" style={{ left: endX }} />
          {children}
        </div>
      </div>
      {autoParam && <AutomationRow track={track} param={autoParam} headW={headW} timelineW={timelineW} pxPerBeat={pxPerBar / bpb} bpb={bpb} />}
    </>
  );
}

/**
 * A track's automation sub-lane, under its clips: one parameter's breakpoints over the same
 * timeline. With no lane yet it sits flat at the track's own value, which is where a
 * double-click starts one.
 */
function AutomationRow({ track, param, headW, timelineW, pxPerBeat, bpb }: { track: Track; param: TrackParam; headW: number; timelineW: number; pxPerBeat: number; bpb: number }) {
  const model = useStore((s) => s.snapshot!.model);
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  const target: AutoTarget = { track: [track.id, param] };
  const lane = automationLane(model, target);
  const points = lane?.points ?? [];
  // A track parameter always has a range; only plugin and insert targets lack one.
  const [lo, hi] = autoTargetRange(target)!;
  const format = param === "volume" ? dbText : param === "pan" ? panText : param === "tone" ? hz : pct;

  return (
    <BreakpointLane
      className="autolane"
      headW={headW}
      timelineW={timelineW}
      pxPerBeat={pxPerBeat}
      bpb={bpb}
      height={AUTO_H}
      points={points}
      range={() => [lo, hi]}
      valueAt={(tick, shown) => automationValueAt(shown, tick) ?? track[param]}
      format={format}
      unit={param === "volume" ? "dB" : param === "tone" ? "Hz" : undefined}
      quantize={(v, fine) => {
        // A hundred steps across the range, ten times that with ⇧.
        const step = (hi - lo) / (fine ? 1000 : 100);
        return Math.min(hi, Math.max(lo, Math.round(v / step) * step));
      }}
      caption={AUTO_PARAMS.find(([p]) => p === param)?.[1] ?? param}
      status={points.length ? `${points.length} point${points.length === 1 ? "" : "s"}` : "double-click to add"}
      flat={points.length === 0}
      headExtra={
        lane && (
          <button className="chip tiny" onClick={() => void dispatch({ type: "clearAutomation", target })} title="remove every point in this lane">
            clear
          </button>
        )
      }
      onAdd={(tick, value) => void dispatch({ type: "addAutomationPoint", target, tick, value, curve: "ramp" })}
      onUpdate={(point, kind) =>
        void dispatch(
          { type: "updateAutomationPoint", lane: lane!.id, point },
          kind === "drag",
          kind === "drag" ? "move automation" : point.curve === "ramp" ? "ramp automation" : "step automation",
        )
      }
      onRemove={(id) => void dispatch({ type: "removeAutomationPoint", lane: lane!.id, id })}
      onCommit={() => void commitGesture()}
    />
  );
}

/**
 * A take's fades, drawn inside the clip with a handle on each top corner.
 *
 * The shaded wedge is the level the fade takes away, sampled off the engine's equal-power
 * curve rather than drawn as a straight line: a fade that looks linear and sounds equal-power
 * lies about where the level is halfway down, and these are the curves a punch seam is made of.
 */
function FadeHandles({
  clip,
  width,
  height,
  pxPerSecond,
  onDown,
  onMove,
  onUp,
}: {
  clip: AudioClip;
  width: number;
  height: number;
  pxPerSecond: number;
  onDown: (e: React.PointerEvent, clip: AudioClip, edge: "in" | "out") => void;
  onMove: (e: React.PointerEvent) => void;
  onUp: (e: React.PointerEvent) => void;
}) {
  const inW = Math.min(width, clip.fadeInSeconds * pxPerSecond);
  const outW = Math.min(width, clip.fadeOutSeconds * pxPerSecond);
  /** The gap between the curve and full level, as a closed path. */
  const wedge = (span: number, rising: boolean) => {
    const steps = 12;
    const points = Array.from({ length: steps + 1 }, (_, i) => {
      const t = i / steps;
      const x = rising ? t * span : width - t * span;
      return `${x.toFixed(2)} ${(height * (1 - equalPowerGain(t))).toFixed(2)}`;
    });
    return `M ${points.join(" L ")} L ${rising ? 0 : width} 0 Z`;
  };
  const handle = (edge: "in" | "out", left: number) => (
    <div
      style={{
        position: "absolute",
        top: 0,
        left,
        width: 8,
        height: 8,
        background: "var(--accent)",
        opacity: 0.9,
        borderRadius: 2,
        cursor: "ew-resize",
        touchAction: "none",
      }}
      title={`${edge === "in" ? "fade in" : "fade out"}: ${((edge === "in" ? clip.fadeInSeconds : clip.fadeOutSeconds) * 1000).toFixed(0)} ms — drag to change`}
      onPointerDown={(e) => onDown(e, clip, edge)}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      onDoubleClick={(e) => e.stopPropagation()}
    />
  );
  return (
    <>
      <svg width={width} height={height} style={{ position: "absolute", inset: 0, pointerEvents: "none" }}>
        {inW > 0 && <path d={wedge(inW, true)} fill="rgba(0, 0, 0, 0.5)" />}
        {outW > 0 && <path d={wedge(outW, false)} fill="rgba(0, 0, 0, 0.5)" />}
      </svg>
      {handle("in", Math.min(width - 8, Math.max(0, inW - 4)))}
      {handle("out", Math.max(0, Math.min(width - 8, width - outW - 4)))}
    </>
  );
}

function ClipView({
  clip,
  pattern,
  track,
  color,
  pxPerBeat,
  selected = false,
  samePattern = false,
  dragging = false,
  ghost,
  onDown,
  onContextMenu,
  onDoubleClick,
}: {
  clip: Clip;
  pattern: SongPattern;
  track: Track;
  color: string;
  pxPerBeat: number;
  selected?: boolean;
  /** Another placement of the selected pattern: editing that pattern changes this clip too. */
  samePattern?: boolean;
  dragging?: boolean;
  /** A preview that takes no input: copies being ⌥-dragged, or a pattern coming in from the tree. */
  ghost?: "copy" | "drop";
  onDown?: (e: React.PointerEvent, clip: Clip, edge: boolean) => void;
  onContextMenu?: (e: React.MouseEvent) => void;
  onDoubleClick?: () => void;
}) {
  const notes = pattern.notes;
  const pxPerTick = pxPerBeat / TICKS_PER_BEAT;
  const w = Math.max(6, clip.lengthTicks * pxPerTick - 1);
  const repeats = clip.lengthTicks / pattern.lengthTicks;
  return (
    <div
      className={`clip${selected ? " sel" : ""}${dragging ? " dragging" : ""}${ghost ? ` ghost ${ghost}` : ""}`}
      style={{
        left: clip.startTick * pxPerTick,
        width: w,
        height: CLIP_H,
        background: `${color}24`,
        boxShadow: `inset 0 0 0 1px ${samePattern ? "#ffffffaa" : `${color}66`}`,
        opacity: clip.muted && !ghost ? 0.4 : undefined,
      }}
      onPointerDown={
        onDown &&
        ((e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          onDown(e, clip, e.clientX > rect.right - 7);
        })
      }
      onContextMenu={onContextMenu}
      onDoubleClick={onDoubleClick}
      title={
        ghost
          ? undefined
          : keys(
              `${pattern.name} · ${notes.length} notes${repeats !== 1 ? ` · ×${+repeats.toFixed(2)}` : ""} · drag: move · right edge: trim/extend · ⌥-drag: copy · double-click: edit · right-click: menu`,
            )
      }
    >
      <div className="ch" style={{ background: color }}>
        {pattern.name}
        {repeats !== 1 && <span style={{ opacity: 0.7, marginLeft: 4 }}>×{+repeats.toFixed(2)}</span>}
      </div>
      <ClipPreview notes={notes} kit={track.isDrum ? track.drumPads : null} pattern={pattern} clip={clip} w={w} h={CLIP_H - 12} color={color} />
      {!ghost && <div className="edge" />}
    </div>
  );
}

/** A clip's notes in miniature: melodic notes by pitch, drum hits by kit row (`kit`, top to bottom). */
function ClipPreview({ notes, kit, pattern, clip, w, h, color }: { notes: NoteEvent[]; kit: DrumPad[] | null; pattern: SongPattern; clip: Clip; w: number; h: number; color: string }) {
  if (notes.length === 0) return <svg width={w} height={h} />;
  const ppt = w / clip.lengthTicks;
  let lo = 36;
  let hi = 84;
  const isDrum = kit !== null;
  if (!isDrum) {
    lo = Math.min(...notes.map((n) => n.pitch));
    hi = Math.max(...notes.map((n) => n.pitch));
    if (hi - lo < 12) {
      const mid = (hi + lo) / 2;
      lo = mid - 6;
      hi = mid + 6;
    }
  }
  const rows = isDrum ? kit.length : hi - lo;
  const rowOf = new Map(kit?.map((k, i) => [k.pitch, i]));
  let d = "";
  // Draw every repetition of the pattern inside the clip, from the offset.
  for (let base = -(clip.offsetTicks % pattern.lengthTicks); base < clip.lengthTicks; base += pattern.lengthTicks) {
    for (const n of notes) {
      const start = base + n.startTick;
      if (start < 0 || start >= clip.lengthTicks) continue;
      const row = rowOf.get(n.pitch);
      if (isDrum && row === undefined) continue;
      const p = isDrum ? 1 - row! / Math.max(1, rows - 1) : (n.pitch - lo) / Math.max(1, rows);
      const x = start * ppt;
      const y = 2 + (1 - p) * (h - 6);
      const ww = Math.max(1, Math.min(n.lengthTicks, clip.lengthTicks - start) * ppt - 1);
      d += `M${x.toFixed(1)} ${y.toFixed(1)}h${ww.toFixed(1)}v3h-${ww.toFixed(1)}z`;
    }
  }
  return (
    <svg width={w} height={h}>
      <path d={d} fill={color} fillOpacity={0.9} />
    </svg>
  );
}
