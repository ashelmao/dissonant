import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { beatsPerBar, dbText, useStore } from "../store";
import type { DrumPad } from "../sound";
import { trackColor, type Clip, type NoteEvent, type SongPattern, type Track } from "../types";
import { VMeter } from "./Meter";
import { ChordLane, GUTTER } from "./ChordLane";
import { keys } from "../platform";
import { clipsInBox, dropBar, groupMoveDelta, moveClips, toggleId, trimClip } from "../clipEditing";

const HEAD_W = 220;
/** Lane height; the marquee maps y to a lane with it, so rows take it inline rather than from CSS. */
const ROW_H = 56;
const CLIP_H = ROW_H - 5;
/** A press that travels less than this is a click, not a drag. */
const DRAG_PX = 3;

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
 * SONG mode center: a bar ruler, the read-only chord bar, track headers, and one lane per track
 * holding that track's clips (each clip a placed pattern, its notes previewed). The song has a
 * hard end at `songLengthBeats`; nothing moves, extends or drops past it.
 *
 * Clips: click selects, shift-click toggles, drag across empty lane space to marquee-select over
 * lanes. Drag moves the whole selection along its own lanes (horizontal only), the right edge trims
 * or loop-extends one clip, ⌥-drag copies, Delete/Backspace removes, right-click opens a menu,
 * double-click opens the pattern at that clip. Patterns arrive by dragging them in from the tree.
 */
export function Arrangement() {
  const model = useStore((s) => s.snapshot!.model);
  const playhead = useStore((s) => s.playhead);
  const pxPerBar = useStore((s) => s.arrZoom);
  const setArrZoom = useStore((s) => s.setArrZoom);
  const selectedTrackId = useStore((s) => s.selectedTrackId);
  const selectedPatternId = useStore((s) => s.selectedPatternId);
  const selectedClipIds = useStore((s) => s.selectedClipIds);
  const patternDrag = useStore((s) => s.patternDrag);
  const hearChords = useStore((s) => s.hearChords);
  const trackPeaks = useStore((s) => s.trackPeaks);
  const bpb = useStore(beatsPerBar);
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
  const songEnd = model.songLengthBeats;
  const endX = songEnd * pxPerBeat;
  // The timeline runs a few bars past the end so the out-of-bounds stretch reads as such.
  const bars = Math.max(Math.ceil((viewW - HEAD_W - 12) / pxPerBar), Math.ceil(songEnd / bpb) + 8);
  const timelineW = bars * pxPerBar;
  const rowW = HEAD_W + timelineW + 12;

  const trackOfPattern = new Map(model.patterns.map((p) => [p.id, p.trackId]));
  const laneOfTrack = new Map(model.tracks.map((t, i) => [t.id, i]));
  const laneOf = (clip: Clip) => laneOfTrack.get(trackOfPattern.get(clip.patternId) ?? "") ?? -1;

  // ── Drop target for patterns dragged out of the tree ───────────────────────────────────
  /** The bar under a viewport point, or null when it isn't over the song's timeline. */
  const barAt = (clientX: number, clientY: number): number | null => {
    const el = scrollerRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (clientX < r.left + HEAD_W || clientX > r.left + el.clientWidth || clientY < r.top || clientY > r.top + el.clientHeight) return null;
    return dropBar((clientX - r.left - HEAD_W + el.scrollLeft) / pxPerBeat, bpb, songEnd);
  };
  const barAtRef = useRef(barAt);
  barAtRef.current = barAt;

  const setPatternDropHandler = useStore((st) => st.setPatternDropHandler);
  useEffect(() => {
    // The vertical position doesn't matter: a pattern always lands on its own track's lane.
    setPatternDropHandler((patternId, clientX, clientY) => {
      const bar = barAtRef.current(clientX, clientY);
      if (bar === null) return;
      void (async () => {
        const st = useStore.getState();
        const before = new Set(st.snapshot!.model.clips.map((c) => c.id));
        await st.dispatch({ type: "addClip", patternId, startBeat: bar, lengthBeats: null });
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
    const bar = barAt(patternDrag.clientX, patternDrag.clientY);
    const p = model.patterns.find((x) => x.id === patternDrag.patternId);
    if (bar !== null && p) {
      dropGhost = { id: "__drop", patternId: p.id, startBeat: bar, lengthBeats: Math.min(p.lengthBeats, songEnd - bar), offsetBeats: 0, muted: false };
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
  const addCopies = async (copies: { src: Clip; startBeat: number }[], label: string) => {
    const st = useStore.getState();
    const made: string[] = [];
    for (const { src, startBeat } of copies) {
      if (startBeat < 0 || startBeat >= songEnd) continue;
      const before = new Set(useStore.getState().snapshot!.model.clips.map((c) => c.id));
      await st.dispatch({ type: "addClip", patternId: src.patternId, startBeat, lengthBeats: src.lengthBeats }, true, label);
      const copy = useStore.getState().snapshot!.model.clips.find((c) => !before.has(c.id));
      if (!copy) continue;
      made.push(copy.id);
      if (src.offsetBeats !== 0 || src.muted) {
        await st.dispatch({ type: "updateClip", clip: { ...copy, offsetBeats: src.offsetBeats, muted: src.muted } }, true, label);
      }
    }
    await st.commitGesture();
    if (made.length) st.selectClips(made);
  };

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
      const hit = clipsInBox(model.clips, laneOf, Math.floor(d.y0 / ROW_H), Math.floor(p.y / ROW_H), d.x0 / pxPerBeat, p.x / pxPerBeat);
      s.selectClips([...d.base, ...hit]);
      return;
    }
    const dx = e.clientX - d.x0;
    if (d.kind === "move") {
      if (!d.moved && Math.abs(dx) <= DRAG_PX) return;
      d.moved = true;
      // Horizontal only: a clip never leaves its own track's lane.
      setPreview(moveClips(d.group, groupMoveDelta(d.anchor, d.group, dx / pxPerBeat, bpb, songEnd)));
    } else {
      setPreview([trimClip(d.clip, d.clip.lengthBeats + dx / pxPerBeat, e.shiftKey ? 1 : bpb, songEnd)]);
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
      if (!result || result[0].startBeat === d.group[0].startBeat) return;
      const many = d.group.length > 1;
      if (d.dup) {
        void addCopies(
          d.group.map((src, i) => ({ src, startBeat: result[i].startBeat })),
          many ? "copy clips" : "copy clip",
        );
      } else {
        // One command for the whole group, so one undo step.
        void dispatch({ type: "updateClips", clips: result }, false, many ? "move clips" : "move clip");
      }
    } else if (d.kind === "trim") {
      if (result && result[0].lengthBeats !== d.clip.lengthBeats) void dispatch({ type: "updateClip", clip: result[0] });
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

  // Delete/Backspace removes the selected clips; Escape closes the menu or clears the selection.
  // App binds neither and the roll isn't mounted in SONG, so a window listener is safe here; it
  // still stands down for text fields, open dialogs and the other modes.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTyping(e.target)) return;
      const st = useStore.getState();
      if (st.mode !== "song" || st.dialog || st.settingsOpen || st.newProjectOpen || st.exportOpen) return;
      if (e.key === "Escape") {
        setMenu(null);
        if (st.selectedClipIds.length) st.selectClips([]);
        return;
      }
      if ((e.key !== "Delete" && e.key !== "Backspace") || e.metaKey || e.ctrlKey || e.altKey) return;
      const ids = st.selectedClipIds;
      if (!ids.length) return;
      e.preventDefault();
      setMenu(null);
      void st.dispatch({ type: "removeClips", ids }, false, ids.length > 1 ? "delete clips" : "delete clip");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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

  const menuClip = menu ? model.clips.find((c) => c.id === menu.clipId) : undefined;
  const playX = HEAD_W + playhead * pxPerBeat;

  return (
    <div className="arr song">
      <div className="scroller" ref={scrollerRef} onScroll={() => menu && setMenu(null)}>
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
                {+(songEnd / bpb).toFixed(2)} bars
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
              s.seek(Math.min(songEnd, Math.max(0, (e.clientX - rect.left) / pxPerBeat)));
            }}
            title="click to seek"
          >
            <div className="oob" style={{ left: endX }} />
            {Array.from({ length: bars }, (_, i) => (
              <div key={i} className={`bar${i % 4 === 0 ? " strong" : ""}${i * bpb >= songEnd ? " past" : ""}`} style={{ left: i * pxPerBar }}>
                {pxPerBar >= 22 || i % 4 === 0 ? i + 1 : ""}
              </div>
            ))}
            <div className="songend" style={{ left: endX }} title="the song ends here: change its length in SETUP">
              end
            </div>
            <div className="phtri" style={{ left: playhead * pxPerBeat }} />
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
          <ChordLane offsetBeats={0} lengthBeats={songEnd} beatW={pxPerBeat} readOnly onEditInSetup={() => s.setMode("setup")} />
          <div className="oob" style={{ left: HEAD_W + endX }} />
        </div>

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
                timelineW={timelineW}
                endX={endX}
                pxPerBar={pxPerBar}
                bpb={bpb}
                onLaneDown={(e) => onLaneDown(e, track)}
              >
                {clipsShown.filter(own).map((clip) => {
                  const pattern = model.patterns.find((p) => p.id === clip.patternId)!;
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
                      onDoubleClick={() => s.openPattern(pattern.id, clip.startBeat)}
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
          count={selectedIds.has(menuClip.id) ? selectedIds.size : 1}
          canDuplicate={menuClip.startBeat + menuClip.lengthBeats < songEnd}
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
          onDuplicate={() => {
            setMenu(null);
            void addCopies([{ src: menuClip, startBeat: menuClip.startBeat + menuClip.lengthBeats }], "duplicate clip");
          }}
          onDelete={() => {
            setMenu(null);
            const ids = selectedIds.has(menuClip.id) ? [...selectedIds] : [menuClip.id];
            void dispatch({ type: "removeClips", ids }, false, ids.length > 1 ? "delete clips" : "delete clip");
          }}
        />
      )}
    </div>
  );
}

/** The right-click menu on a clip, pinned at the pointer; a press anywhere else closes it. */
function ClipMenu({
  x,
  y,
  count,
  canDuplicate,
  onClose,
  onMakeUnique,
  onDuplicate,
  onDelete,
}: {
  x: number;
  y: number;
  count: number;
  canDuplicate: boolean;
  onClose: () => void;
  onMakeUnique: () => void;
  onDuplicate: () => void;
  onDelete: () => void;
}) {
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
  // Keep it on screen near the window's right and bottom edges.
  const left = Math.min(x, window.innerWidth - 200);
  const top = Math.min(y, window.innerHeight - 110);
  return (
    <div className="dropdown clipmenu" ref={ref} style={{ left, top }}>
      <div className="item" onClick={onMakeUnique} title="copy this clip's pattern into a new one on the same track, so editing it changes only this clip">
        make unique
      </div>
      <div className={`item${canDuplicate ? "" : " disabled"}`} onClick={canDuplicate ? onDuplicate : undefined}>
        duplicate after
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
  timelineW,
  endX,
  pxPerBar,
  bpb,
  onLaneDown,
  children,
}: {
  track: Track;
  index: number;
  color: string;
  selected: boolean;
  peak: number;
  timelineW: number;
  endX: number;
  pxPerBar: number;
  bpb: number;
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
    <div className={`trow${selected ? " on" : ""}`} style={{ width: HEAD_W + timelineW + 12, height: ROW_H }}>
      <div className={`thead${selected ? " on" : ""}`} style={{ height: ROW_H }} onClick={() => selectTrack(track.id)}>
        <div className="strip" style={{ background: color }} />
        <div className="body">
          <div className="l1">
            <span className="num">{String(index + 1).padStart(2, "0")}</span>
            <span className={`name${track.muted ? " muted" : ""}`}>{track.name}</span>
            <span className="type">{track.isDrum ? "drum" : "midi"}</span>
          </div>
          <div className="l2" onClick={(e) => e.stopPropagation()}>
            <button className={`msr m${track.muted ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackMuted", id: track.id, muted: !track.muted })}>M</button>
            <button className={`msr s${track.soloed ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackSoloed", id: track.id, soloed: !track.soloed })}>S</button>
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
  const w = Math.max(6, clip.lengthBeats * pxPerBeat - 1);
  const repeats = clip.lengthBeats / pattern.lengthBeats;
  return (
    <div
      className={`clip${selected ? " sel" : ""}${dragging ? " dragging" : ""}${ghost ? ` ghost ${ghost}` : ""}`}
      style={{
        left: clip.startBeat * pxPerBeat,
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
  const ppb = w / clip.lengthBeats;
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
  for (let base = -(clip.offsetBeats % pattern.lengthBeats); base < clip.lengthBeats; base += pattern.lengthBeats) {
    for (const n of notes) {
      const start = base + n.startBeat;
      if (start < 0 || start >= clip.lengthBeats) continue;
      const row = rowOf.get(n.pitch);
      if (isDrum && row === undefined) continue;
      const p = isDrum ? 1 - row! / Math.max(1, rows - 1) : (n.pitch - lo) / Math.max(1, rows);
      const x = start * ppb;
      const y = 2 + (1 - p) * (h - 6);
      const ww = Math.max(1, Math.min(n.lengthBeats, clip.lengthBeats - start) * ppb - 1);
      d += `M${x.toFixed(1)} ${y.toFixed(1)}h${ww.toFixed(1)}v3h-${ww.toFixed(1)}z`;
    }
  }
  return (
    <svg width={w} height={h}>
      <path d={d} fill={color} fillOpacity={0.9} />
    </svg>
  );
}
