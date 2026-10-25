import { useEffect, useMemo, useRef, useState } from "react";
import { beatsPerBar, selectedPattern, useStore } from "../store";
import { trackColor } from "../types";
import type { SongPattern, Track } from "../types";
import { keys } from "../platform";
import { addedId, dropGap, loadCollapsed, patternKey, reorderMoves, saveCollapsed, stepRow, trackKey, treeRows } from "../trackTree";
import type { TreeRow } from "../trackTree";

/** Pixels the pointer must travel before a press on a row becomes a drag. */
const DRAG_THRESHOLD = 4;

/** A press on a row that may turn into a drag: a pattern out to the song, or a track to reorder. */
interface Press {
  kind: "pattern" | "track";
  id: string;
  x0: number;
  y0: number;
  dragging: boolean;
  el: HTMLElement;
  pointerId: number;
  /** Removes the window listeners the press installed. */
  stop: () => void;
}

/**
 * Left column in PATTERN and SONG modes: every track with its patterns as children ("versions
 * of the track"). Click selects, double-click opens a pattern in the editor, ＋ on a track makes
 * a new pattern for it. Drag a pattern onto the song to place it (pointer events; the song view
 * registers `patternDropHandler`); drag a track to reorder. With focus: ↑↓ move, ←→ fold,
 * Enter opens, F2 renames, Delete deletes, ⌥↑↓ moves the track.
 */
export function TrackTree() {
  const model = useStore((s) => s.snapshot!.model);
  const mode = useStore((s) => s.mode);
  const selectedTrackId = useStore((s) => s.selectedTrackId);
  const pattern = useStore(selectedPattern);
  const selectedClipIds = useStore((s) => s.selectedClipIds);
  const selectTrack = useStore((s) => s.selectTrack);
  const selectPattern = useStore((s) => s.selectPattern);
  const openPattern = useStore((s) => s.openPattern);
  const dispatch = useStore((s) => s.dispatch);
  const confirmDialog = useStore((s) => s.confirm);
  const patternDrag = useStore((s) => s.patternDrag);
  const hasDropTarget = useStore((s) => s.patternDropHandler !== null);
  const setPatternDrag = useStore((s) => s.setPatternDrag);
  const bpb = useStore(beatsPerBar);
  const [collapsed, setCollapsed] = useState<Set<string>>(loadCollapsed);
  const [renaming, setRenaming] = useState<{ kind: "track" | "pattern"; id: string } | null>(null);
  const [draft, setDraft] = useState("");
  /** The keyboard row; follows the selection, but can rest on a track row too. */
  const [cursor, setCursor] = useState<string | null>(null);
  /** A track being dragged and the gap between branches it would land in. */
  const [trackDrag, setTrackDrag] = useState<{ id: string; gap: number } | null>(null);
  const press = useRef<Press | null>(null);
  const justDragged = useRef(false);
  const renameRef = useRef(renaming);
  const treeRef = useRef<HTMLDivElement>(null);

  const rows = useMemo(() => treeRows(model, collapsed), [model, collapsed]);
  /** How many clips in the song use each pattern. */
  const uses = useMemo(() => {
    const n = new Map<string, number>();
    for (const c of model.clips) n.set(c.patternId, (n.get(c.patternId) ?? 0) + 1);
    return n;
  }, [model.clips]);
  /** Patterns behind the clips selected in the song view. */
  const inSelection = useMemo(() => {
    const ids = new Set(selectedClipIds);
    return new Set(model.clips.filter((c) => ids.has(c.id)).map((c) => c.patternId));
  }, [model.clips, selectedClipIds]);

  // ── Open state ─────────────────────────────────────────────────────────────────────────
  useEffect(() => saveCollapsed(collapsed, model.tracks.map((t) => t.id)), [collapsed, model.tracks]);
  const setOpen = (trackId: string, open: boolean) =>
    setCollapsed((c) => {
      if (c.has(trackId) !== open) return c;
      const next = new Set(c);
      if (open) next.delete(trackId);
      else next.add(trackId);
      return next;
    });
  // The selected track's branch opens whenever the selection moves to it.
  useEffect(() => {
    if (selectedTrackId) setOpen(selectedTrackId, true);
  }, [selectedTrackId]);

  // The cursor follows the selection unless it's already on it (a track row or its pattern).
  useEffect(() => {
    setCursor((c) => {
      if (c === trackKey(selectedTrackId ?? "") || (pattern && c === patternKey(pattern.id))) return c;
      return pattern ? patternKey(pattern.id) : selectedTrackId ? trackKey(selectedTrackId) : null;
    });
  }, [selectedTrackId, pattern?.id]);
  useEffect(() => {
    if (cursor) treeRef.current?.querySelector(`[data-row="${CSS.escape(cursor)}"]`)?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  // ── Commands ───────────────────────────────────────────────────────────────────────────
  const latest = () => useStore.getState().snapshot!.model;
  const refocus = () => treeRef.current?.focus({ preventScroll: true });

  const addTrack = async (isDrum: boolean) => {
    const before = latest().tracks;
    await dispatch({ type: "addTrack", isDrum });
    const id = addedId(before, latest().tracks);
    if (id) selectTrack(id);
  };
  /** Make a pattern (new or a copy) and select it, so the next double-click or drag is on it. */
  const makePattern = async (trackId: string, copyOf?: string) => {
    const before = latest().patterns;
    await dispatch(copyOf ? { type: "duplicatePattern", id: copyOf } : { type: "addPattern", trackId });
    const id = addedId(before, latest().patterns);
    if (!id) return;
    setOpen(trackId, true);
    selectPattern(id);
    setCursor(patternKey(id));
  };
  const moveTrack = async (id: string, up: boolean, count = 1) => {
    // Several steps from one drag fold into a single undo step.
    for (let i = 0; i < count; i++) await dispatch({ type: "moveTrack", id, up }, count > 1, "move track");
    if (count > 1) await useStore.getState().commitGesture();
  };
  const deleteTrack = async (t: Track) => {
    if (latest().tracks.length <= 1) return;
    if (await confirmDialog("Delete track?", `“${t.name}”, its patterns and their clips in the song will be removed.`, "Delete", true)) {
      await dispatch({ type: "deleteTrack", id: t.id });
    }
    refocus();
  };
  const deletePattern = async (p: SongPattern) => {
    const n = uses.get(p.id) ?? 0;
    const what = n ? `“${p.name}” and its ${n === 1 ? "clip" : `${n} clips`} in the song will be removed.` : `“${p.name}” will be removed. It isn't in the song.`;
    if (await confirmDialog("Delete pattern?", what, "Delete", true)) await dispatch({ type: "deletePattern", id: p.id });
    refocus();
  };

  // ── Rename ─────────────────────────────────────────────────────────────────────────────
  const startRename = (kind: "track" | "pattern", id: string, name: string) => {
    setDraft(name);
    setRenaming((renameRef.current = { kind, id }));
  };
  const finishRename = (commit: boolean) => {
    const r = renameRef.current;
    renameRef.current = null;
    setRenaming(null);
    const name = draft.trim();
    if (!r || !commit || !name) return;
    void dispatch(r.kind === "track" ? { type: "renameTrack", id: r.id, name } : { type: "renamePattern", id: r.id, name });
  };
  const inputProps = {
    type: "text",
    autoFocus: true,
    value: draft,
    spellCheck: false,
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => setDraft(e.target.value),
    onFocus: (e: React.FocusEvent<HTMLInputElement>) => e.target.select(),
    onBlur: () => finishRename(true),
    onKeyDown: (e: React.KeyboardEvent) => {
      e.stopPropagation();
      if (e.key === "Enter" || e.key === "Escape") {
        finishRename(e.key === "Enter");
        refocus();
      }
    },
    onClick: (e: React.MouseEvent) => e.stopPropagation(),
    onDoubleClick: (e: React.MouseEvent) => e.stopPropagation(),
    onPointerDown: (e: React.PointerEvent) => e.stopPropagation(),
  };

  // ── Drag: a pattern out to the song, a track up or down the list ───────────────────────
  // The press listens on the window, so a fast flick that leaves the row before the
  // threshold still turns into a drag; past the threshold the row captures the pointer.
  const onRowDown = (e: React.PointerEvent, kind: Press["kind"], id: string) => {
    justDragged.current = false;
    if (e.button !== 0 || renaming || (e.target as HTMLElement).closest("button, input")) return;
    press.current?.stop(); // a press whose release happened outside the window
    const onMove = (ev: PointerEvent) => ev.pointerId === e.pointerId && onPressMove(ev);
    const onUp = (ev: PointerEvent) => ev.pointerId === e.pointerId && onPressUp(ev);
    const onCancel = (ev: PointerEvent) => ev.pointerId === e.pointerId && cancelDrag();
    const stop = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    press.current = { kind, id, x0: e.clientX, y0: e.clientY, dragging: false, el: e.currentTarget as HTMLElement, pointerId: e.pointerId, stop };
  };
  const gapAt = (y: number) => {
    const branches = [...(treeRef.current?.querySelectorAll<HTMLElement>("[data-branch]") ?? [])];
    return dropGap(branches.map((b) => b.getBoundingClientRect()).map((r) => r.top + r.height / 2), y);
  };
  const onPressMove = (e: PointerEvent) => {
    const p = press.current;
    if (!p) return;
    if (!p.dragging) {
      if (Math.hypot(e.clientX - p.x0, e.clientY - p.y0) < DRAG_THRESHOLD) return;
      p.dragging = true;
      try {
        p.el.setPointerCapture(p.pointerId);
      } catch {
        /* the pointer is already gone; the window listeners still see it out */
      }
    }
    if (p.kind === "pattern") setPatternDrag({ patternId: p.id, clientX: e.clientX, clientY: e.clientY });
    else setTrackDrag({ id: p.id, gap: gapAt(e.clientY) });
  };
  const onPressUp = (e: PointerEvent) => {
    const p = press.current;
    press.current = null;
    p?.stop();
    if (!p?.dragging) return;
    justDragged.current = true;
    if (p.kind === "pattern") {
      useStore.getState().patternDropHandler?.(p.id, e.clientX, e.clientY);
      setPatternDrag(null);
    } else {
      setTrackDrag(null);
      const from = latest().tracks.findIndex((t) => t.id === p.id);
      const { up, count } = reorderMoves(from, gapAt(e.clientY));
      if (from >= 0 && count > 0) void moveTrack(p.id, up, count);
    }
  };
  const cancelDrag = () => {
    const p = press.current;
    press.current = null;
    p?.stop();
    if (p?.dragging) {
      justDragged.current = true;
      try {
        p.el.releasePointerCapture(p.pointerId);
      } catch {
        /* already released */
      }
    }
    setPatternDrag(null);
    setTrackDrag(null);
  };
  // A drag still in flight when the tree unmounts (Tab to another mode) ends with it.
  useEffect(
    () => () => {
      const p = press.current;
      press.current = null;
      p?.stop();
      if (p?.dragging && p.kind === "pattern") useStore.getState().setPatternDrag(null);
    },
    [],
  );
  const dragging = patternDrag !== null || trackDrag !== null;
  // Escape drops a drag on the floor, wherever focus is.
  useEffect(() => {
    if (!dragging) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopPropagation();
      cancelDrag();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [dragging]);
  /** Swallow the click that ends a drag, so dropping doesn't also re-select. */
  const unlessDragged = (fn: () => void) => () => {
    if (justDragged.current) justDragged.current = false;
    else fn();
  };
  const rowHandlers = (kind: Press["kind"], id: string) => ({ onPointerDown: (e: React.PointerEvent) => onRowDown(e, kind, id) });

  // ── Keyboard (only while the tree has focus) ───────────────────────────────────────────
  const goTo = (r: TreeRow | null) => {
    if (!r) return;
    setCursor(r.key);
    if (r.kind === "pattern") selectPattern(r.id);
    else selectTrack(r.id);
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    const target = e.target as HTMLElement;
    if (renaming || target.tagName === "INPUT" || e.metaKey || e.ctrlKey) return;
    if (target.tagName === "BUTTON" && (e.key === "Enter" || e.key === " ")) return;
    const row = rows.find((r) => r.key === cursor) ?? null;
    const t = row && model.tracks.find((x) => x.id === row.trackId);
    const p = row?.kind === "pattern" ? model.patterns.find((x) => x.id === row.id) : undefined;
    const open = !!t && !collapsed.has(t.id);
    switch (e.key) {
      case "ArrowUp":
      case "ArrowDown":
        if (e.altKey) {
          if (t) void moveTrack(t.id, e.key === "ArrowUp");
        } else goTo(stepRow(rows, cursor, e.key === "ArrowUp" ? -1 : 1));
        break;
      case "Home":
      case "End":
        goTo(stepRow(rows, cursor, e.key === "Home" ? -Infinity : Infinity));
        break;
      case "ArrowLeft":
        if (row?.kind === "track" && open) setOpen(row.id, false);
        else if (row?.kind === "pattern") goTo(rows.find((r) => r.key === trackKey(row.trackId)) ?? null);
        break;
      case "ArrowRight":
        if (row?.kind === "track" && !open) setOpen(row.id, true);
        else if (row?.kind === "track") {
          const next = stepRow(rows, row.key, 1);
          if (next?.kind === "pattern" && next.trackId === row.id) goTo(next);
        }
        break;
      case "Enter": {
        // A track row opens the pattern of it that's selected, or its first.
        const toOpen = p ?? (t && (pattern?.trackId === t.id ? pattern : model.patterns.find((x) => x.trackId === t.id)));
        if (toOpen) openPattern(toOpen.id);
        break;
      }
      case "F2":
        if (p) startRename("pattern", p.id, p.name);
        else if (t) startRename("track", t.id, t.name);
        break;
      case "Delete":
      case "Backspace":
        if (p) void deletePattern(p);
        else if (t) void deleteTrack(t);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };

  // ── Render ─────────────────────────────────────────────────────────────────────────────
  const dragged = patternDrag && model.patterns.find((p) => p.id === patternDrag.patternId);
  const draggedIndex = dragged ? model.tracks.findIndex((t) => t.id === dragged.trackId) : -1;
  const trackFrom = trackDrag ? model.tracks.findIndex((t) => t.id === trackDrag.id) : -1;
  const dropGapShown = trackDrag && reorderMoves(trackFrom, trackDrag.gap).count > 0 ? trackDrag.gap : -1;

  return (
    <aside className="side browser tt">
      <div className="tabs">
        <div className="on">tracks</div>
      </div>
      <div className="section tt-head">
        <div className="row" style={{ gap: 4 }}>
          <button onClick={() => void addTrack(false)} title="new instrument track, with one empty pattern">+ inst</button>
          <button onClick={() => void addTrack(true)} title="new drum track, with one empty pattern">+ drum</button>
          <span className="spacer" />
          <span className="tt-count">
            {model.tracks.length}t · {model.patterns.length}p
          </span>
        </div>
      </div>
      <div
        ref={treeRef}
        className={`tt-tree${trackDrag ? " reordering" : ""}`}
        tabIndex={0}
        role="tree"
        aria-label="tracks and their patterns"
        onKeyDown={onKeyDown}
        // Row buttons don't take focus from the tree, so the arrows keep working after a click.
        onMouseDown={(e) => {
          if ((e.target as HTMLElement).closest("button")) {
            e.preventDefault();
            treeRef.current?.focus({ preventScroll: true });
          }
        }}
      >
        {model.tracks.map((t, ti) => {
          const patterns = model.patterns.filter((p) => p.trackId === t.id);
          const open = !collapsed.has(t.id);
          const sel = t.id === selectedTrackId;
          const color = trackColor(t, ti);
          const drop = dropGapShown === ti ? " drop-before" : dropGapShown === model.tracks.length && ti === model.tracks.length - 1 ? " drop-after" : "";
          return (
            <div
              key={t.id}
              data-branch={t.id}
              className={`tt-branch${sel ? " sel" : ""}${trackDrag?.id === t.id ? " lifted" : ""}${drop}`}
              style={{ "--tc": color } as React.CSSProperties}
              role="treeitem"
              aria-expanded={open}
              aria-selected={sel}
            >
              <div
                data-row={trackKey(t.id)}
                className={`tt-row tt-track${cursor === trackKey(t.id) ? " cursor" : ""}`}
                {...rowHandlers("track", t.id)}
                onClick={unlessDragged(() => (setCursor(trackKey(t.id)), selectTrack(t.id)))}
                onDoubleClick={() => startRename("track", t.id, t.name)}
                title="click: select · double-click: rename · drag: reorder"
              >
                <button className="ico tt-caret" title={open ? "collapse" : "expand"} tabIndex={-1} onClick={(e) => (e.stopPropagation(), setOpen(t.id, !open))}>
                  {open ? "▾" : "▸"}
                </button>
                <span className="tt-strip" />
                {renaming?.kind === "track" && renaming.id === t.id ? (
                  <input {...inputProps} />
                ) : (
                  <span className={`tt-name${t.muted ? " muted" : ""}`}>{t.name}</span>
                )}
                <span className="tt-meta">
                  {t.isDrum ? "drum" : t.voice} · {patterns.length}
                </span>
                <span className="tt-acts">
                  <button className="ico" title="move up" tabIndex={-1} disabled={ti === 0} onClick={(e) => (e.stopPropagation(), void moveTrack(t.id, true))}>
                    ▲
                  </button>
                  <button className="ico" title="move down" tabIndex={-1} disabled={ti === model.tracks.length - 1} onClick={(e) => (e.stopPropagation(), void moveTrack(t.id, false))}>
                    ▼
                  </button>
                  {model.tracks.length > 1 && (
                    <button className="ico danger" title="delete track" tabIndex={-1} onClick={(e) => (e.stopPropagation(), void deleteTrack(t))}>
                      ×
                    </button>
                  )}
                </span>
                <button
                  className={`msr m${t.muted ? " on" : ""}`}
                  title={t.muted ? "unmute" : "mute"}
                  tabIndex={-1}
                  onClick={(e) => (e.stopPropagation(), void dispatch({ type: "setTrackMuted", id: t.id, muted: !t.muted }))}
                >
                  M
                </button>
                <button
                  className={`msr s${t.soloed ? " on" : ""}`}
                  title={t.soloed ? "unsolo" : "solo"}
                  tabIndex={-1}
                  onClick={(e) => (e.stopPropagation(), void dispatch({ type: "setTrackSoloed", id: t.id, soloed: !t.soloed }))}
                >
                  S
                </button>
                <button className="ico tt-add" title={`new pattern on ${t.name}`} tabIndex={-1} onClick={(e) => (e.stopPropagation(), void makePattern(t.id))}>
                  ＋
                </button>
              </div>
              {open && (
                <div className="tt-kids" role="group">
                  {patterns.map((p) => {
                    const n = uses.get(p.id) ?? 0;
                    const on = p.id === pattern?.id;
                    const inSel = mode === "song" && inSelection.has(p.id);
                    return (
                      <div
                        key={p.id}
                        data-row={patternKey(p.id)}
                        role="treeitem"
                        aria-selected={on}
                        className={`tt-row tt-pat${on ? " on" : ""}${inSel ? " insel" : ""}${patternDrag?.patternId === p.id ? " lifted" : ""}${cursor === patternKey(p.id) ? " cursor" : ""}`}
                        {...rowHandlers("pattern", p.id)}
                        onClick={unlessDragged(() => (setCursor(patternKey(p.id)), selectPattern(p.id)))}
                        onDoubleClick={() => openPattern(p.id)}
                        title="click: select · double-click: open in the editor · drag onto the song to place it"
                      >
                        <span className={`tt-dot${n ? " placed" : ""}`} title={n ? `in the song ×${n}` : "not in the song yet"} />
                        {renaming?.kind === "pattern" && renaming.id === p.id ? (
                          <input {...inputProps} />
                        ) : (
                          <span className="tt-name">{p.name}</span>
                        )}
                        <span className="tt-meta">
                          {+(p.lengthBeats / bpb).toFixed(2)}b · {p.notes.length}n{n ? <span className="tt-uses"> · ×{n}</span> : null}
                        </span>
                        <span className="tt-acts">
                          <button className="ico" title="rename (F2)" tabIndex={-1} onClick={(e) => (e.stopPropagation(), startRename("pattern", p.id, p.name))}>
                            ✎
                          </button>
                          <button className="ico" title="duplicate" tabIndex={-1} onClick={(e) => (e.stopPropagation(), void makePattern(t.id, p.id))}>
                            ⧉
                          </button>
                          <button className="ico danger" title="delete pattern" tabIndex={-1} onClick={(e) => (e.stopPropagation(), void deletePattern(p))}>
                            ×
                          </button>
                        </span>
                      </div>
                    );
                  })}
                  {patterns.length === 0 && (
                    <div className="tt-row tt-pat tt-empty" onClick={() => void makePattern(t.id)}>
                      <span className="tt-name">no patterns · ＋ make one</span>
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
      <div className="section tt-foot">
        {pattern && (
          <>
            <div className="head">
              <span className="cap">length · {pattern.name}</span>
              <span className="tt-count">{+(pattern.lengthBeats / bpb).toFixed(2)} bars</span>
            </div>
            <div className="chipsrow">
              {[1, 2, 4, 8].map((bars) => (
                <button
                  key={bars}
                  className={`chip${pattern.lengthBeats === bars * bpb ? " on" : ""}`}
                  onClick={() => dispatch({ type: "setPatternLength", id: pattern.id, beats: bars * bpb })}
                >
                  {bars}
                </button>
              ))}
            </div>
          </>
        )}
        <div className="tt-help">
          {mode === "song" ? "drag a pattern onto the song to place it. double-click one to edit it." : "double-click a pattern to edit it. to place it, switch to SONG and drag it in."}
          <br />
          {keys("↑↓ move · ←→ fold · enter open · F2 rename · ⌥↑↓ reorder")}
        </div>
      </div>
      {dragged && patternDrag && (
        <div
          className="tt-draglabel"
          style={{ left: patternDrag.clientX + 12, top: patternDrag.clientY + 10, "--tc": trackColor(model.tracks[draggedIndex] ?? model.tracks[0], Math.max(0, draggedIndex)) } as React.CSSProperties}
        >
          <span className="tt-strip" />
          <span className="tt-dl-name">{dragged.name}</span>
          <span className="tt-dl-sub">{hasDropTarget ? `${+(dragged.lengthBeats / bpb).toFixed(2)}b · drop on the song` : "switch to SONG to place it"} · esc cancels</span>
        </div>
      )}
    </aside>
  );
}
