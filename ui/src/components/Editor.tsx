import { useState } from "react";
import { barTicks, GRID_OPTIONS, selectedPattern, selectedTrack, selectedTrackIndex, useStore } from "../store";
import { songLength, TICKS_PER_BEAT, ticksToBeats, trackColor, type SongPattern } from "../types";
import { BASE_BEAT_W, GUTTER, ChordLane } from "./ChordLane";
import { DrumGrid } from "./DrumGrid";
import { PianoRoll } from "./PianoRoll";
import { keys } from "../platform";

/** "bar 9", or "bar 9.3" when the tick sits inside a bar (1-based, like the rulers). */
export function barLabel(tick: number, ticksPerBar: number): string {
  const bar = Math.floor(tick / ticksPerBar);
  const inBar = tick - bar * ticksPerBar;
  return inBar === 0 ? `bar ${bar + 1}` : `bar ${bar + 1}.${Math.floor(ticksToBeats(inBar)) + 1}`;
}

/** The bars field's own look: the inspector and the tree both place it beside the length chips. */
const barsFieldStyle: React.CSSProperties = {
  width: 46,
  fontFamily: "var(--font-mono)",
  fontSize: 10,
  padding: "2px 4px",
  background: "var(--bg-control)",
  color: "var(--text-1)",
  border: "1px solid var(--line-1)",
  borderRadius: 3,
};

/** Longest pattern the field will set: past this a "pattern" is a song, and the song view is that. */
const MAX_PATTERN_BARS = 64;

/**
 * A pattern's length in bars, typed. The chips cover 1, 2, 4 and 8; a 3-bar loop, a 6-bar
 * turnaround and a 16-bar build are just as ordinary, and `setPatternLength` has always taken
 * any tick count — only the UI was rounding the music down to powers of two. The engine's floor
 * is one beat, so that is the floor here.
 */
export function PatternBars({ pattern, barTick }: { pattern: SongPattern; barTick: number }) {
  const dispatch = useStore((s) => s.dispatch);
  const [draft, setDraft] = useState<string | null>(null);
  const bars = +(pattern.lengthTicks / barTick).toFixed(3);
  const commit = (text: string) => {
    setDraft(null);
    const typed = parseFloat(text);
    if (!Number.isFinite(typed)) return;
    const ticks = Math.max(TICKS_PER_BEAT, Math.round(Math.min(MAX_PATTERN_BARS, typed) * barTick));
    if (ticks !== pattern.lengthTicks) void dispatch({ type: "setPatternLength", id: pattern.id, ticks });
  };
  return (
    <input
      type="number"
      style={barsFieldStyle}
      min={TICKS_PER_BEAT / barTick}
      max={MAX_PATTERN_BARS}
      step={1}
      value={draft ?? bars}
      title="length in bars: any number, so 3- and 16-bar patterns are as easy as 4"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={(e) => commit(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") {
          setDraft(null);
          (e.target as HTMLInputElement).blur();
        }
      }}
    />
  );
}

/**
 * PAT mode editor column. Two bars: what you're editing and what it's heard with
 * (track › pattern, where the chords come from, the chord bed, the ghost), then the
 * editing tools; under them the read-only chord lane and the roll (or drum grid).
 */
export function Editor() {
  const s = useStore();
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const idx = useStore(selectedTrackIndex);
  const ticksPerBar = useStore(barTicks);
  if (!track) {
    return (
      <div className="editor empty">
        <span className="mono">no tracks yet. add one and start writing.</span>
        <div className="row" style={{ gap: 6 }}>
          <button onClick={() => s.dispatch({ type: "addTrack", isDrum: false })}>+ instrument</button>
          <button onClick={() => s.dispatch({ type: "addTrack", isDrum: true })}>+ drums</button>
        </div>
      </div>
    );
  }
  if (!pattern) {
    return (
      <div className="editor empty">
        <span className="mono">{track.name} has no patterns yet. make one and start writing.</span>
        <button className="solid" onClick={() => s.dispatch({ type: "addPattern", trackId: track.id })}>+ new pattern</button>
        <span className="mono hint">patterns get into the song by dragging them from the tree onto SONG.</span>
      </div>
    );
  }
  const color = trackColor(track, idx);
  const notes = pattern.notes;

  return (
    <div className="editor">
      <div className="toolbar ctxbar">
        <div className="crumbs" title={`editing ${track.name} › ${pattern.name}: this is all you hear here, plus the chord bed when it's on`}>
          <span className="sq" style={{ background: color }} />
          <span className="trk">{track.name}</span>
          <span className="sep">›</span>
          <b className="pat">{pattern.name}</b>
          <span className="meta">
            {track.isDrum ? "steps" : "roll"} · {+(pattern.lengthTicks / ticksPerBar).toFixed(2)} {pattern.lengthTicks === ticksPerBar ? "bar" : "bars"}
          </span>
        </div>
        <div className="vdiv" />
        <ChordsFrom pattern={pattern} />
        <button
          className={`chip bedtoggle${s.hearChords ? " on" : ""}`}
          onClick={() => s.toggleHearChords()}
          aria-pressed={s.hearChords}
          aria-label="hear chord bed"
          title="hear the chord bed under this pattern while you write (never exported)"
        >
          <span className="box" aria-hidden="true">{s.hearChords ? "■" : "□"}</span> hear chord bed
        </button>
        <div className="vdiv" />
        <GhostPicker pattern={pattern} />
        <span className="spacer" />
        {s.armed && (
          <span className="rectarget" title="record is armed: notes you play land in this pattern while it plays">
            ● rec into {pattern.name}
          </span>
        )}
      </div>
      <div className="toolbar">
        <div className="opt">
          <span className="k">snap</span>
          {/* Grouped so the triplet and dotted snaps read as what they are rather than as odd
              numbers among the binary ones. */}
          <select value={s.noteLength} onChange={(e) => s.setNoteLength(parseInt(e.target.value, 10))} title="the grid notes snap to: straight, triplet (T) or dotted (.)">
            {([["straight", ""], ["triplet", "T"], ["dotted", "."]] as const).map(([group, suffix]) => (
              <optgroup key={group} label={group}>
                {GRID_OPTIONS.filter(([l]) => (l.endsWith("T") ? "T" : l.endsWith(".") ? "." : "") === suffix).map(([l, v]) => (
                  <option key={l} value={v}>{l}</option>
                ))}
              </optgroup>
            ))}
          </select>
        </div>
        {!track.isDrum && (
          <>
            <button className={`chip${s.highlightRows ? " on" : ""}`} onClick={() => s.toggleHighlight()} title="tint rows by tier">tiers</button>
            <button className={`chip${s.showLandscape ? " on" : ""}`} onClick={() => s.toggleLandscape()} title="harmonic map">map</button>
            <div className="vdiv" />
            <button className={`chip${s.brush ? " on" : ""}`} onClick={() => s.toggleBrush()} title={keys("brush: left-drag on empty space paints notes (⌥-drag does this too when ⌥ is set to paint in Settings)")}>brush</button>
            <button className={`chip${s.follow ? " on" : ""}`} onClick={() => s.toggleFollow()} title="follow: keep the playhead in view while playing">follow</button>
            <button className={`chip${s.fold ? " on" : ""}`} onClick={() => s.toggleFold()} title="fold: show only rows that have notes, plus chord tones around them">fold</button>
          </>
        )}
        <span className="spacer" />
        <span className="mono" style={{ fontSize: 9.5, color: "var(--text-4)" }}>
          {notes.length} {track.isDrum ? "hits" : "notes"}
          {s.selectedNoteIds.length ? ` · ${s.selectedNoteIds.length} sel` : ""}
        </span>
        <div className="zoom">
          <span className="flabel">zoom</span>
          <input type="range" min={0.25} max={3} step={0.05} value={s.zoom} onChange={(e) => s.setZoom(parseFloat(e.target.value))} />
          <button
            className="chip tiny"
            title="zoom to fit the whole pattern"
            onClick={() => {
              const w = document.querySelector<HTMLElement>(".rollscroll, .drumgrid")?.clientWidth ?? 1000;
              s.setZoom((w - GUTTER - 16) / (ticksToBeats(pattern.lengthTicks) * BASE_BEAT_W));
            }}
          >
            fit
          </button>
        </div>
        {!track.isDrum && (
          <span
            className="chip"
            style={{ cursor: "help" }}
            title={keys(
              "click: place a note (keep dragging to move it) · ⌥: off the grid (or paint, per Settings)\ndrag: move (⇧ one axis) · right edge: resize · ⌥-drag a note: duplicate\nrope: drag the dot past a note onto another to slide into it · click a rope, ⌫ removes it · drag its midpoint: slide time · double-click it: back to glide\n⇧/⌘-drag: marquee · ⇧-click: toggle · double-click a key: select that pitch (⌘: pitch class)\nright-click: erase · Tab/⇧Tab: next/previous note · 0: mute\n⌫ delete · ⌘A ⌘C ⌘X ⌘V ⌘D · ⇧⌘E split at playhead · ⌘J glue · ⌘B duplicate one loop later\narrows nudge (⇧ = octave / bar, ⌥ = fine) · [ ] velocity · ⌥-wheel transpose (⌥⇧ octave)\n⌘-wheel zoom at cursor · ⇧-wheel scroll · ruler: seek · velocity lane: drag stems, ⌥-drag ramp",
            )}
          >
            ?
          </span>
        )}
      </div>
      <ChordLane offsetTicks={s.chordOffsetTicks} lengthTicks={pattern.lengthTicks} beatW={BASE_BEAT_W * s.zoom} readOnly onEditInSetup={() => s.setScreen("setup")} />
      {track.isDrum ? <DrumGrid /> : <PianoRoll />}
    </div>
  );
}

/**
 * Which song bar the chords under this pattern start at (`chordOffsetTicks`): bar 1 when it
 * was opened from the tree, the clip's start when opened from a clip. ‹ › move it by a bar;
 * the list jumps to bar 1 or to wherever this pattern sits in the song.
 */
function ChordsFrom({ pattern }: { pattern: SongPattern }) {
  const offset = useStore((s) => s.chordOffsetTicks);
  const model = useStore((s) => s.snapshot!.model);
  const openPattern = useStore((s) => s.openPattern);
  const ticksPerBar = useStore(barTicks);
  const lastStart = Math.max(0, songLength(model) - ticksPerBar);
  const go = (tick: number) => openPattern(pattern.id, Math.min(lastStart, Math.max(0, tick)));
  const barStart = Math.floor(offset / ticksPerBar) * ticksPerBar;
  const placed = [...new Set(model.clips.filter((c) => c.patternId === pattern.id).map((c) => c.startTick))].sort((a, b) => a - b);
  const choices = [...new Set([0, ...placed, offset])].sort((a, b) => a - b);
  return (
    <div
      className={`opt chordsfrom${offset > 0 ? " moved" : ""}`}
      title="the chords you see, hear and tier against are the song's, starting at this bar. ‹ › moves by a bar; the list jumps to where this pattern sits in the song."
    >
      <span className="k">chords from</span>
      <button className="ico" disabled={offset <= 0} onClick={() => go(offset > barStart ? barStart : barStart - ticksPerBar)} aria-label="chords from one bar earlier">‹</button>
      <select value={String(offset)} onChange={(e) => go(parseInt(e.target.value, 10))} aria-label="chords from which song bar">
        {choices.map((t) => (
          <option key={t} value={String(t)}>
            {barLabel(t, ticksPerBar)}
            {placed.includes(t) ? " · placed here" : ""}
          </option>
        ))}
      </select>
      <button className="ico" disabled={barStart + ticksPerBar > lastStart} onClick={() => go(barStart + ticksPerBar)} aria-label="chords from one bar later">›</button>
    </div>
  );
}

/** Pick another pattern (any track) to draw faint behind this one, and hear it along if you like. */
function GhostPicker({ pattern }: { pattern: SongPattern }) {
  const model = useStore((s) => s.snapshot!.model);
  const ghostPatternId = useStore((s) => s.ghostPatternId);
  const setGhostPattern = useStore((s) => s.setGhostPattern);
  const hearGhost = useStore((s) => s.hearGhost);
  const toggleHearGhost = useStore((s) => s.toggleHearGhost);
  const ghostLevel = useStore((s) => s.ghostLevel);
  const setGhostLevel = useStore((s) => s.setGhostLevel);
  const value = ghostPatternId && ghostPatternId !== pattern.id ? ghostPatternId : "";
  const ghost = model.patterns.find((p) => p.id === value);
  const gi = ghost ? model.tracks.findIndex((t) => t.id === ghost.trackId) : -1;
  const groups = model.tracks
    .map((t, i) => ({ track: t, i, patterns: model.patterns.filter((p) => p.trackId === t.id && p.id !== pattern.id) }))
    .filter((g) => g.patterns.length);
  return (
    <div
      className="opt ghostpick"
      title="ghost: draw another pattern behind this one, faint, to write against it, and loop it along while you play. a drum pattern shows as its rhythm."
    >
      <span className="k">ghost</span>
      {gi >= 0 && <span className="sq" style={{ background: trackColor(model.tracks[gi], gi) }} />}
      <select value={value} onChange={(e) => setGhostPattern(e.target.value || null)} disabled={!groups.length} aria-label="ghost pattern">
        <option value="">{groups.length ? "off" : "nothing to ghost yet"}</option>
        {groups.map((g) => (
          <optgroup key={g.track.id} label={g.track.name}>
            {g.patterns.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </optgroup>
        ))}
      </select>
      {ghost && (
        <button
          className={`chip bedtoggle${hearGhost ? " on" : ""}`}
          onClick={toggleHearGhost}
          aria-pressed={hearGhost}
          aria-label="hear ghost"
          title={`play ${ghost.name} along on its own track while you write (pattern mode only, never exported)`}
        >
          <span className="box" aria-hidden="true">{hearGhost ? "■" : "□"}</span> hear
        </button>
      )}
      {ghost && (
        <span className={`ghostlevel${hearGhost ? "" : " off"}`} title={`ghost volume ${Math.round(ghostLevel * 100)}% · 100% is its track's own level · double-click resets`}>
          <input
            type="range"
            min={0}
            max={150}
            step={5}
            value={Math.round(ghostLevel * 100)}
            disabled={!hearGhost}
            onChange={(e) => setGhostLevel(Number(e.target.value) / 100)}
            onDoubleClick={() => setGhostLevel(1)}
            aria-label="ghost volume"
            aria-valuetext={`${Math.round(ghostLevel * 100)}%`}
          />
          <span className="pct">{Math.round(ghostLevel * 100)}%</span>
        </span>
      )}
      {ghost && (
        <button className="ico" onClick={() => setGhostPattern(null)} aria-label="ghost off" title="ghost off">
          ×
        </button>
      )}
    </div>
  );
}
