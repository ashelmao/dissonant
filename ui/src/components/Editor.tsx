import { GRID_OPTIONS, selectedPattern, selectedTrack, selectedTrackIndex, useStore } from "../store";
import { trackColor } from "../types";
import { BASE_BEAT_W, GUTTER, ChordLane } from "./ChordLane";
import { DrumGrid } from "./DrumGrid";
import { PianoRoll } from "./PianoRoll";
import { keys } from "../platform";

/** PAT mode editor column: toolbar · chord lane · piano roll (or drum grid) · velocity lane. */
export function Editor() {
  const s = useStore();
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const idx = useStore(selectedTrackIndex);
  if (!track) return null;
  if (!pattern) {
    return (
      <div className="editor" style={{ alignItems: "center", justifyContent: "center", gap: 8, color: "var(--text-5)" }}>
        <span className="mono">{track.name} has no patterns yet</span>
        <button onClick={() => s.dispatch({ type: "addPattern", trackId: track.id })}>+ new pattern</button>
      </div>
    );
  }
  const color = trackColor(track, idx);
  const model = s.snapshot!.model;
  const notes = pattern.notes;

  return (
    <div className="editor">
      <div className="toolbar">
        <div className="title">
          <span className="sq" style={{ background: color }} />
          <b>{track.isDrum ? "step sequencer" : "piano roll"}</b>
          <span className="meta">
            {String(idx + 1).padStart(2, "0")} {track.name} / {pattern.name}
          </span>
        </div>
        <div className="vdiv" />
        <div className="opt">
          <span className="k">snap</span>
          <select value={s.noteLength} onChange={(e) => s.setNoteLength(parseFloat(e.target.value))}>
            {GRID_OPTIONS.map(([l, v]) => (
              <option key={l} value={v}>{l}</option>
            ))}
          </select>
        </div>
        <button className={`chip${s.highlightRows ? " on" : ""}`} onClick={() => s.toggleHighlight()} title="tint rows by tier">tiers</button>
        <button className={`chip${s.showLandscape ? " on" : ""}`} onClick={() => s.toggleLandscape()} title="harmonic map">map</button>
        <button className={`chip${s.hearChords ? " on" : ""}`} onClick={() => s.toggleHearChords()} title="hear the chord bed">chords</button>
        {!track.isDrum && (
          <>
            <div className="vdiv" />
            <button className={`chip${s.brush ? " on" : ""}`} onClick={() => s.toggleBrush()} title={keys("brush: left-drag on empty space paints notes (⌥-drag does this too when ⌥ is set to paint in Settings)")}>brush</button>
            <button className={`chip${s.follow ? " on" : ""}`} onClick={() => s.toggleFollow()} title="follow: keep the playhead in view while playing">follow</button>
            <button className={`chip${s.fold ? " on" : ""}`} onClick={() => s.toggleFold()} title="fold: show only rows that have notes, plus chord tones around them">fold</button>
            <div className="opt" title="ghost: draw another pattern's notes behind this one (dashed, with their tiers, never heard)">
              <span className="k">ghost</span>
              <select value={s.ghostPatternId ?? ""} onChange={(e) => s.setGhostPattern(e.target.value || null)} style={{ maxWidth: 110 }}>
                <option value="">off</option>
                {model.patterns
                  .filter((p) => p.id !== pattern.id && !model.tracks.find((t) => t.id === p.trackId)?.isDrum)
                  .map((p) => (
                    <option key={p.id} value={p.id}>{p.name}</option>
                  ))}
              </select>
            </div>
          </>
        )}
        <span className="spacer" />
        <span className="mono" style={{ fontSize: 9.5, color: "var(--text-4)" }}>
          {notes.length} notes{s.selectedNoteIds.length ? ` · ${s.selectedNoteIds.length} sel` : ""}
        </span>
        <div className="zoom">
          <span className="flabel">zoom</span>
          <input type="range" min={0.25} max={3} step={0.05} value={s.zoom} onChange={(e) => s.setZoom(parseFloat(e.target.value))} />
          <button
            className="chip tiny"
            title="zoom to fit the whole pattern"
            onClick={() => {
              const w = document.querySelector<HTMLElement>(".rollscroll, .drumgrid")?.clientWidth ?? 1000;
              s.setZoom((w - GUTTER - 16) / (pattern.lengthBeats * BASE_BEAT_W));
            }}
          >
            fit
          </button>
        </div>
        <span
          className="chip"
          style={{ cursor: "help" }}
          title={keys(
            "click: place a note (keep dragging to move it) · ⌥: off the grid (or paint, per Settings)\ndrag: move (⇧ one axis) · right edge: resize · ⌥-drag a note: duplicate\n⇧/⌘-drag: marquee · ⇧-click: toggle · double-click a key: select that pitch (⌘: pitch class)\nright-click: erase · Tab/⇧Tab: next/previous note · 0: mute\n⌫ delete · ⌘A ⌘C ⌘X ⌘V ⌘D · ⇧⌘E split at playhead · ⌘J glue · ⌘B duplicate one loop later\narrows nudge (⇧ = octave / bar, ⌥ = fine) · [ ] velocity · ⌥-wheel transpose (⌥⇧ octave)\n⌘-wheel zoom at cursor · ⇧-wheel scroll · ruler: seek · velocity lane: drag stems, ⌥-drag ramp",
          )}
        >
          ?
        </span>
      </div>
      <ChordLane offsetBeats={s.chordOffsetBeats} lengthBeats={pattern.lengthBeats} beatW={BASE_BEAT_W * s.zoom} readOnly onEditInSetup={() => s.setMode("setup")} />
      {track.isDrum ? <DrumGrid /> : <PianoRoll />}
    </div>
  );
}
