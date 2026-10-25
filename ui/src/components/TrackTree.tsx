import { useRef, useState } from "react";
import { beatsPerBar, selectedPattern, useStore } from "../store";
import { trackColor } from "../types";

/** Pixels the pointer must travel before a press on a pattern becomes a drag. */
const DRAG_THRESHOLD = 4;

/**
 * Left column in PATTERN and SONG modes: every track with its patterns as children. Click
 * selects, double-click opens a pattern in the editor, ＋ on a track makes a new pattern for
 * it. Drag a pattern onto the song to place it (pointer events; the song view registers
 * `patternDropHandler`).
 */
export function TrackTree() {
  const model = useStore((s) => s.snapshot!.model);
  const selectedTrackId = useStore((s) => s.selectedTrackId);
  const pattern = useStore(selectedPattern);
  const selectTrack = useStore((s) => s.selectTrack);
  const selectPattern = useStore((s) => s.selectPattern);
  const openPattern = useStore((s) => s.openPattern);
  const dispatch = useStore((s) => s.dispatch);
  const confirmDialog = useStore((s) => s.confirm);
  const patternDrag = useStore((s) => s.patternDrag);
  const setPatternDrag = useStore((s) => s.setPatternDrag);
  const bpb = useStore(beatsPerBar);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [renaming, setRenaming] = useState<{ kind: "track" | "pattern"; id: string } | null>(null);
  const [draft, setDraft] = useState("");
  const press = useRef<{ patternId: string; x0: number; y0: number; dragging: boolean } | null>(null);
  const justDragged = useRef(false);

  const startRename = (kind: "track" | "pattern", id: string, name: string) => {
    setDraft(name);
    setRenaming({ kind, id });
  };
  const finishRename = () => {
    const r = renaming;
    setRenaming(null);
    if (!r || !draft.trim()) return;
    void dispatch(r.kind === "track" ? { type: "renameTrack", id: r.id, name: draft.trim() } : { type: "renamePattern", id: r.id, name: draft.trim() });
  };
  const toggle = (id: string) =>
    setCollapsed((c) => {
      const next = new Set(c);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // ── Drag a pattern out to the song ─────────────────────────────────────────────────────
  const onPatternDown = (e: React.PointerEvent, patternId: string) => {
    if (e.button !== 0 || renaming) return;
    press.current = { patternId, x0: e.clientX, y0: e.clientY, dragging: false };
  };
  const onPatternMove = (e: React.PointerEvent) => {
    const p = press.current;
    if (!p) return;
    if (!p.dragging) {
      if (Math.hypot(e.clientX - p.x0, e.clientY - p.y0) < DRAG_THRESHOLD) return;
      p.dragging = true;
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    }
    setPatternDrag({ patternId: p.patternId, clientX: e.clientX, clientY: e.clientY });
  };
  const onPatternUp = (e: React.PointerEvent) => {
    const p = press.current;
    press.current = null;
    if (!p?.dragging) return;
    justDragged.current = true;
    useStore.getState().patternDropHandler?.(p.patternId, e.clientX, e.clientY);
    setPatternDrag(null);
  };

  const inputProps = {
    type: "text",
    autoFocus: true,
    value: draft,
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => setDraft(e.target.value),
    onBlur: finishRename,
    onKeyDown: (e: React.KeyboardEvent) => {
      if (e.key === "Enter") finishRename();
      if (e.key === "Escape") setRenaming(null);
    },
    onClick: (e: React.MouseEvent) => e.stopPropagation(),
    onPointerDown: (e: React.PointerEvent) => e.stopPropagation(),
  };

  return (
    <aside className="side browser">
      <div className="tabs">
        <div className="on">tracks</div>
      </div>
      <div className="section" style={{ gap: 6 }}>
        <div className="row" style={{ gap: 4 }}>
          <button onClick={() => dispatch({ type: "addTrack", isDrum: false })}>+ inst</button>
          <button onClick={() => dispatch({ type: "addTrack", isDrum: true })}>+ drum</button>
          <span className="spacer" />
          <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>
            {model.tracks.length} · {model.patterns.length}p
          </span>
        </div>
      </div>
      <div className="section grow" style={{ padding: "6px 6px", gap: 0, overflowY: "auto" }}>
        <div className="list">
          {model.tracks.map((t, ti) => {
            const patterns = model.patterns.filter((p) => p.trackId === t.id);
            const open = !collapsed.has(t.id);
            const color = trackColor(t, ti);
            return (
              <div key={t.id}>
                <div
                  className={`lrow${t.id === selectedTrackId && !pattern ? " on" : ""}`}
                  onClick={() => selectTrack(t.id)}
                  onDoubleClick={() => startRename("track", t.id, t.name)}
                  title="click: select · double-click: rename · ＋: new pattern on this track"
                >
                  <span className="strip" style={{ background: color }} />
                  <button className="ico" title={open ? "collapse" : "expand"} onClick={(e) => (e.stopPropagation(), toggle(t.id))}>
                    {open ? "▾" : "▸"}
                  </button>
                  {renaming?.kind === "track" && renaming.id === t.id ? (
                    <input {...inputProps} />
                  ) : (
                    <span className={`name${t.muted ? " muted" : ""}`} style={{ fontWeight: t.id === selectedTrackId ? 700 : undefined }}>
                      {t.name}
                    </span>
                  )}
                  <span className="meta">{t.isDrum ? "drum" : t.voice}</span>
                  <button className="ico" title="new pattern on this track" style={{ color: "var(--accent)" }} onClick={(e) => (e.stopPropagation(), dispatch({ type: "addPattern", trackId: t.id }))}>
                    ＋
                  </button>
                  <button className="ico" title="move up" onClick={(e) => (e.stopPropagation(), dispatch({ type: "moveTrack", id: t.id, up: true }))}>▲</button>
                  <button className="ico" title="move down" onClick={(e) => (e.stopPropagation(), dispatch({ type: "moveTrack", id: t.id, up: false }))}>▼</button>
                  {model.tracks.length > 1 && (
                    <button
                      className="ico"
                      title="delete track"
                      onClick={async (e) => {
                        e.stopPropagation();
                        if (await confirmDialog("Delete track?", `“${t.name}”, its patterns and their clips in the song will be removed.`, "Delete", true)) {
                          void dispatch({ type: "deleteTrack", id: t.id });
                        }
                      }}
                    >
                      ×
                    </button>
                  )}
                </div>
                {open &&
                  patterns.map((p) => (
                    <div
                      key={p.id}
                      className={`lrow${p.id === pattern?.id ? " on" : ""}`}
                      style={{ paddingLeft: 26, cursor: "grab" }}
                      onPointerDown={(e) => onPatternDown(e, p.id)}
                      onPointerMove={onPatternMove}
                      onPointerUp={onPatternUp}
                      onPointerCancel={() => ((press.current = null), setPatternDrag(null))}
                      onClick={() => {
                        if (justDragged.current) {
                          justDragged.current = false;
                          return;
                        }
                        selectPattern(p.id);
                      }}
                      onDoubleClick={() => openPattern(p.id)}
                      title="click: select · double-click: open in the editor · drag onto the song to place it"
                    >
                      {renaming?.kind === "pattern" && renaming.id === p.id ? (
                        <input {...inputProps} />
                      ) : (
                        <span className="name">{p.name}</span>
                      )}
                      <span className="meta">
                        {+(p.lengthBeats / bpb).toFixed(2)}b · {p.notes.length}n
                      </span>
                      <button className="ico" title="rename" onClick={(e) => (e.stopPropagation(), startRename("pattern", p.id, p.name))}>✎</button>
                      <button className="ico" title="duplicate" onClick={(e) => (e.stopPropagation(), dispatch({ type: "duplicatePattern", id: p.id }))}>⧉</button>
                      <button
                        className="ico"
                        title="delete pattern"
                        onClick={async (e) => {
                          e.stopPropagation();
                          if (await confirmDialog("Delete pattern?", `“${p.name}” and its clips in the song will be removed.`, "Delete", true)) {
                            void dispatch({ type: "deletePattern", id: p.id });
                          }
                        }}
                      >
                        ×
                      </button>
                    </div>
                  ))}
                {open && patterns.length === 0 && (
                  <div className="lrow" style={{ paddingLeft: 26, color: "var(--text-5)" }} onClick={() => dispatch({ type: "addPattern", trackId: t.id })}>
                    <span className="name">＋ new pattern</span>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>
      {pattern && (
        <div className="section" style={{ borderBottom: 0, borderTop: "1px solid var(--line-1)" }}>
          <div className="head">
            <span className="cap">length · {pattern.name}</span>
            <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>{+(pattern.lengthBeats / bpb).toFixed(2)} bars</span>
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
        </div>
      )}
      {patternDrag && (
        <div
          className="mono"
          style={{ position: "fixed", left: patternDrag.clientX + 10, top: patternDrag.clientY + 10, pointerEvents: "none", zIndex: 50, fontSize: 10, padding: "2px 6px", background: "var(--accent-bg)", border: "1px solid var(--accent-border)", color: "var(--accent)" }}
        >
          {model.patterns.find((p) => p.id === patternDrag.patternId)?.name}
        </div>
      )}
    </aside>
  );
}
