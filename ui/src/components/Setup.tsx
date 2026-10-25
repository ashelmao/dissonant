import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { beatsPerBar, useStore } from "../store";
import { chordName, diatonicChords, keyName, noteName, NOTE_NAMES, startersFor, progression } from "../theory";
import { keyAt, replaceChordWindow, tempoMap, TIME_SIGNATURES, uuid, type ChordEvent, type ScaleType, type Section } from "../types";
import {
  barsForSeconds,
  barsText,
  degreeOf,
  formatMinSec,
  keySpans,
  lengthText,
  matchesWindow,
  nextChords,
  parseMinSec,
  placeChord,
  tileStarter,
} from "../songSetup";
import { ChordBuilder, ChordLane } from "./ChordLane";
import { SECTION_COLORS, SectionEditor } from "./SectionEditor";
import { TempoLane } from "./TempoLane";
import { keys } from "../platform";

/** Width of the lane heads; every lane's x = 0 sits this far in, so they all line up. */
const HEAD = 104;
const CHORD_H = 64;
const TEMPO_H = 56;

/**
 * SETUP: the song's foundations on one page. A band of fundamentals (key, meter, tempo, swing,
 * length, hear chords), then the whole song as aligned lanes sharing one scroll and zoom
 * (bars, sections, chords, tempo), then the tools for the chords and sections. The only place
 * chords and sections are edited; a fresh project opens here.
 */
export function Setup() {
  const model = useStore((s) => s.snapshot!.model);
  const dispatch = useStore((s) => s.dispatch);
  const bpb = useStore(beatsPerBar);
  const songBeats = model.songLengthBeats;

  const scrollerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // Zoom: null fits the whole song in view; otherwise px per beat.
  const [zoom, setZoom] = useState<number | null>(null);
  const fitW = Math.max(4, (width - HEAD - 24) / Math.max(1, songBeats));
  const beatW = zoom ?? fitW;
  const timelineW = songBeats * beatW;
  const zoomBy = (f: number) => setZoom(Math.min(160, Math.max(4, beatW * f)));
  // ⌘/ctrl-wheel zooms the lanes (a non-passive listener, so the page doesn't zoom too).
  useEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.metaKey && !e.ctrlKey) return;
      e.preventDefault();
      setZoom((z) => Math.min(160, Math.max(4, (z ?? fitW) * (e.deltaY < 0 ? 1.15 : 1 / 1.15))));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [fitW]);

  const [snap, setSnap] = useState<"bar" | "beat" | "half">("bar");
  const snapBeats = snap === "bar" ? bpb : snap === "beat" ? 1 : 0.5;
  const [chordId, setChordId] = useState<string | null>(null);
  const [sectionId, setSectionId] = useState<string | null>(null);
  // A bar range picked on the ruler: starters fill it instead of the whole song.
  const [range, setRange] = useState<[number, number] | null>(null);
  const sel = range ? ([Math.max(0, range[0]), Math.min(songBeats, range[1])] as [number, number]) : null;
  const picked = sel && sel[1] > sel[0] ? sel : null;
  const target: [number, number] = picked ?? [0, songBeats];

  const chord = model.chords.find((c) => c.id === chordId) ?? null;
  const section = model.sections.find((x) => x.id === sectionId) ?? null;
  const sectionColor = (x: Section) => x.color ?? SECTION_COLORS[Math.max(0, model.sections.findIndex((y) => y.id === x.id)) % SECTION_COLORS.length];

  return (
    <div className="setup" style={{ ["--setup-head" as string]: `${HEAD}px` }}>
      <Fundamentals />

      <div className="st-bar">
        <span className="cap">song</span>
        <span className="mono st-meta">
          {barsText(songBeats, bpb)} bars · {model.sections.length} section{model.sections.length === 1 ? "" : "s"} · {model.chords.length} chord{model.chords.length === 1 ? "" : "s"}
        </span>
        <span className="vdiv" />
        <span className="flabel">snap</span>
        <div className="seg sm">
          {(["bar", "beat", "half"] as const).map((v) => (
            <div key={v} className={snap === v ? "on" : ""} onClick={() => setSnap(v)}>
              {v === "half" ? "½" : v}
            </div>
          ))}
        </div>
        <span className="vdiv" />
        <button className="ico" onClick={() => zoomBy(1 / 1.25)} title="zoom out">−</button>
        <button className={`chip tiny${zoom === null ? " on" : ""}`} onClick={() => setZoom(null)} title={keys("fit the whole song · ⌘-scroll zooms")}>fit</button>
        <button className="ico" onClick={() => zoomBy(1.25)} title="zoom in">+</button>
        {picked && (
          <>
            <span className="vdiv" />
            <span className="mono st-range">
              bars {Math.floor(picked[0] / bpb) + 1}–{Math.ceil(picked[1] / bpb)} picked
            </span>
            <button className="chip tiny" onClick={() => setRange(null)}>clear</button>
          </>
        )}
        <span className="spacer" />
        <span className="st-hint">{keys("drag the bar ruler to pick bars · ⌥-drag a chord to copy · ⌘-scroll to zoom")}</span>
      </div>

      <div className="st-scroller" ref={scrollerRef}>
        <div className="st-lanes" style={{ width: HEAD + timelineW + 40 }}>
          <Ruler beatW={beatW} songBeats={songBeats} bpb={bpb} sel={picked} setRange={setRange} />
          <SectionStrip beatW={beatW} songBeats={songBeats} bpb={bpb} selectedId={sectionId} onSelect={setSectionId} colorOf={sectionColor} />
          <ChordLane
            offsetBeats={0}
            lengthBeats={songBeats}
            beatW={beatW}
            height={CHORD_H}
            snap={snapBeats}
            selectedId={chordId}
            onSelect={setChordId}
            underlay={<Underlay beatW={beatW} songBeats={songBeats} bpb={bpb} sel={picked} colorOf={sectionColor} />}
          />
          <TempoLane headW={HEAD} timelineW={timelineW} pxPerBeat={beatW} bpb={bpb} height={TEMPO_H} />
          <Playline beatW={beatW} />
        </div>
      </div>

      <div className="st-tools">
        <ChordPanel chord={chord} onSelect={setChordId} />
        <NextPanel chord={chord} target={target} onSelect={setChordId} />
        <StarterPanel target={target} picked={!!picked} onClearPick={() => setRange(null)} />
        <SectionPanel
          section={section}
          onSelect={setSectionId}
          colorOf={sectionColor}
          addAt={picked ? picked[0] : null}
          onAdd={(beat) => void dispatch({ type: "addSection", name: "", startBeat: beat })}
        />
      </div>
    </div>
  );
}

// ─── Fundamentals band ─────────────────────────────────────────────────────────────────────

function swingWords(swing: number): string {
  if (swing <= 50.5) return "straight";
  if (swing < 58) return "a little lilt";
  if (swing < 64) return "loose swing";
  if (swing <= 69) return "triplet feel";
  return "hard shuffle";
}

/** Key, meter, tempo, swing, length and the chord bed: the values a song is built on. */
function Fundamentals() {
  const model = useStore((s) => s.snapshot!.model);
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  const confirm = useStore((s) => s.confirm);
  const hearChords = useStore((s) => s.hearChords);
  const toggleHearChords = useStore((s) => s.toggleHearChords);
  const playing = useStore((s) => s.playing);
  const togglePlay = useStore((s) => s.togglePlay);
  const rewind = useStore((s) => s.rewind);
  const defaultTempo = useStore((s) => s.settings.editing.defaultTempo);
  const bpb = useStore(beatsPerBar);

  const key = model.key;
  const setKey = (root: number | null, scale: ScaleType, isLocked = root !== null) => void dispatch({ type: "setKey", key: { rootPitchClass: root, scale, isLocked } });

  const setTempo = (bpm: number, transient = false) => void dispatch({ type: "setTempo", bpm: Math.min(300, Math.max(20, bpm)) }, transient, "set tempo");
  const [bpmText, setBpmText] = useState<string | null>(null);
  const commitBpm = () => {
    const v = parseFloat(bpmText ?? "");
    if (bpmText !== null && Number.isFinite(v) && v !== model.tempo) setTempo(v);
    setBpmText(null);
  };
  const map = tempoMap(model.tempo, model.tempoPoints);
  const points = model.tempoPoints ?? [];
  const bpms = [model.tempo, ...points.map((p) => p.bpm)];

  const swing = model.swing ?? 50;
  const grid = model.swingGrid ?? 0.5;

  // Song length, in bars and in min:sec along the tempo map.
  const songBeats = model.songLengthBeats;
  const [barsDraft, setBarsDraft] = useState<string | null>(null);
  const [timeDraft, setTimeDraft] = useState<string | null>(null);
  const seconds = map.secondsAt(songBeats);
  const setLength = async (bars: number) => {
    const beats = Math.min(999, Math.max(1, Math.round(bars))) * bpb;
    if (Math.abs(beats - songBeats) < 1e-9) return;
    if (beats < songBeats) {
      // Shortening cuts into whatever runs past the new end: ask first (unless that's switched off).
      const past = model.clips.filter((c) => c.startBeat + c.lengthBeats > beats + 1e-9);
      if (past.length) {
        const gone = past.filter((c) => c.startBeat >= beats - 1e-9).length;
        const cut = past.length - gone;
        const parts = [cut && `${cut} trimmed`, gone && `${gone} removed`].filter(Boolean).join(", ");
        const ok = await confirm(
          "Shorten the song?",
          `${past.length} clip${past.length === 1 ? " runs" : "s run"} past bar ${Math.round(beats / bpb)}: ${parts}. Undo brings ${past.length === 1 ? "it" : "them"} back.`,
          "Shorten",
          true,
        );
        if (!ok) return;
      }
    }
    void dispatch({ type: "setSongLength", beats }, false, "set song length");
  };
  const commitBars = () => {
    const v = parseFloat(barsDraft ?? "");
    setBarsDraft(null);
    if (barsDraft !== null && Number.isFinite(v)) void setLength(v);
  };
  const commitTime = () => {
    const t = parseMinSec(timeDraft ?? "");
    setTimeDraft(null);
    if (timeDraft !== null && t !== null) void setLength(barsForSeconds(map.secondsAt, t, bpb));
  };
  const bars = songBeats / bpb;

  return (
    <div className="st-fund">
      <div className="cell">
        <span className="cap">key</span>
        <div className="row" style={{ gap: 4 }}>
          <select value={key.rootPitchClass ?? ""} onChange={(e) => setKey(e.target.value === "" ? null : parseInt(e.target.value, 10), key.scale)} aria-label="key root" className="big">
            <option value="">—</option>
            {NOTE_NAMES.map((n, i) => (
              <option key={n} value={i}>{n}</option>
            ))}
          </select>
          <div className="seg sm">
            {(["major", "minor"] as const).map((sc) => (
              <div key={sc} className={key.scale === sc ? "on" : ""} onClick={() => setKey(key.rootPitchClass, sc)}>
                {sc === "major" ? "maj" : "min"}
              </div>
            ))}
          </div>
          <button
            className={`chip tiny${key.isLocked ? " on" : ""}`}
            disabled={key.rootPitchClass === null}
            onClick={() => setKey(key.rootPitchClass, key.scale, !key.isLocked)}
            title="a locked key is what every note is tiered against; unlocked, the top bar offers a guess from your notes"
          >
            {key.isLocked ? "locked" : "lock"}
          </button>
        </div>
        <span className="sub">
          {key.rootPitchClass === null ? "no key yet: every note looks the same" : key.isLocked ? `notes tier against ${keyName(key.rootPitchClass, key.scale)}` : "unlocked: a hint, not a rule"}
        </span>
      </div>

      <div className="cell">
        <span className="cap">meter</span>
        <select
          className="big"
          value={`${model.timeSignature?.numerator ?? 4}/${model.timeSignature?.denominator ?? 4}`}
          onChange={(e) => {
            const [n, d] = e.target.value.split("/").map(Number);
            void dispatch({ type: "setTimeSignature", numerator: n, denominator: d });
          }}
          aria-label="time signature"
        >
          {TIME_SIGNATURES.map((t) => (
            <option key={`${t.numerator}/${t.denominator}`} value={`${t.numerator}/${t.denominator}`}>
              {t.numerator} / {t.denominator}
            </option>
          ))}
        </select>
        <span className="sub">{+bpb.toFixed(2)} beats a bar</span>
      </div>

      <div className="cell">
        <span className="cap">tempo</span>
        <div className="row" style={{ gap: 4 }}>
          <input
            type="text"
            className="mono big"
            style={{ width: 64 }}
            value={bpmText ?? String(+model.tempo.toFixed(2))}
            onFocus={(e) => e.target.select()}
            onChange={(e) => setBpmText(e.target.value)}
            onBlur={commitBpm}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              if (e.key === "Escape") setBpmText(null);
              if (e.key === "ArrowUp" || e.key === "ArrowDown") {
                e.preventDefault();
                setBpmText(null);
                setTempo(Math.round(model.tempo) + (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 10 : 1));
              }
            }}
            onWheel={(e) => setTempo(Math.round(model.tempo) + (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 10 : 1))}
            onDoubleClick={() => setTempo(defaultTempo)}
            aria-label="base tempo in bpm"
            title={keys("type a tempo · ↑↓ ±1 (⇧ ±10) · scroll to nudge · double-click resets")}
          />
          <span className="unit">bpm</span>
        </div>
        <span className="sub">
          {map.isConstant
            ? "steady · double-click the tempo lane to change it mid-song"
            : `${points.length} change${points.length === 1 ? "" : "s"} · ${Math.round(Math.min(...bpms))}–${Math.round(Math.max(...bpms))} bpm`}
        </span>
      </div>

      <div className="cell">
        <span className="cap">swing</span>
        <div className="row" style={{ gap: 6 }}>
          <span className="mono big" style={{ color: swing > 50 ? "var(--accent)" : "var(--text-3)", minWidth: 22, textAlign: "right" }}>{Math.round(swing)}</span>
          <input
            type="range"
            min={50}
            max={75}
            step={1}
            value={Math.round(swing)}
            onChange={(e) => void dispatch({ type: "setSwing", swing: Number(e.target.value), grid }, true, "set swing")}
            onPointerUp={() => void commitGesture()}
            onKeyUp={() => void commitGesture()}
            onDoubleClick={() => void dispatch({ type: "setSwing", swing: 50, grid })}
            aria-label="swing amount"
            title="50 = straight · 67 = triplet feel · 75 = hard shuffle · double-click: straight"
          />
          <div className="seg sm" title="which notes swing: every second eighth, or every second sixteenth">
            {(
              [
                [0.5, "8th"],
                [0.25, "16th"],
              ] as const
            ).map(([g, l]) => (
              <div key={g} className={grid === g ? "on" : ""} onClick={() => void dispatch({ type: "setSwing", swing, grid: g })}>
                {l}
              </div>
            ))}
          </div>
        </div>
        <span className="sub">{swingWords(swing)}</span>
      </div>

      <div className="cell">
        <span className="cap">length</span>
        <div className="row" style={{ gap: 4 }}>
          <input
            type="text"
            className="mono big"
            style={{ width: 44 }}
            value={barsDraft ?? barsText(songBeats, bpb)}
            onFocus={(e) => e.target.select()}
            onChange={(e) => setBarsDraft(e.target.value)}
            onBlur={commitBars}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              if (e.key === "Escape") setBarsDraft(null);
              if (e.key === "ArrowUp" || e.key === "ArrowDown") {
                e.preventDefault();
                setBarsDraft(null);
                void setLength(Math.round(bars) + (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 4 : 1));
              }
            }}
            aria-label="song length in bars"
            title="song length in bars · ↑↓ ±1 (⇧ ±4)"
          />
          <span className="unit">bars</span>
          <span className="unit">=</span>
          <input
            type="text"
            className="mono big"
            style={{ width: 52 }}
            value={timeDraft ?? formatMinSec(seconds)}
            onFocus={(e) => e.target.select()}
            onChange={(e) => setTimeDraft(e.target.value)}
            onBlur={commitTime}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              if (e.key === "Escape") setTimeDraft(null);
            }}
            aria-label="song length in minutes and seconds"
            title="type a length like 2:30: it rounds to the nearest whole bar at this tempo"
          />
          <span className="unit">min</span>
        </div>
        <span className="sub">the song's hard end · clips stop here</span>
      </div>

      <div className="cell grow">
        <span className="cap">listen</span>
        <div className="row" style={{ gap: 4 }}>
          <button className={`st-play${playing ? " on" : ""}`} onClick={() => togglePlay()} title="play the song from the playhead (space)">
            {playing ? "■ stop" : "▶ play"}
          </button>
          <button className="quiet" onClick={() => rewind()} title="back to bar 1 (Enter)" aria-label="back to bar 1">
            ⏮
          </button>
          <button className={`chip${hearChords ? " on" : ""}`} onClick={() => toggleHearChords()} aria-pressed={hearChords} title="plays the progression under the song while you write">
            hear chords
          </button>
        </div>
        <span className="sub">{hearChords ? "you'll hear the chords. they're never exported" : "chords are silent: switch them on to hear the progression"}</span>
      </div>
    </div>
  );
}

// ─── Timeline lanes ────────────────────────────────────────────────────────────────────────

/** Bar numbers. Click to move the playhead; drag to pick a bar range for the starters. */
function Ruler({ beatW, songBeats, bpb, sel, setRange }: { beatW: number; songBeats: number; bpb: number; sel: [number, number] | null; setRange: (r: [number, number] | null) => void }) {
  const seek = useStore((s) => s.seek);
  const barW = beatW * bpb;
  const bars = Math.ceil(songBeats / bpb - 1e-9);
  const every = barW >= 22 ? 1 : barW >= 10 ? 2 : 4;
  const drag = useRef<{ anchor: number; x0: number; moved: boolean } | null>(null);
  const barAt = (e: React.PointerEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    return Math.min(bars - 1, Math.max(0, Math.floor((e.clientX - r.left) / barW)));
  };
  return (
    <div className="st-row st-ruler">
      <div className="st-head">
        <span className="cap">bar</span>
      </div>
      <div
        className="st-track"
        style={{ width: songBeats * beatW }}
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          drag.current = { anchor: barAt(e), x0: e.clientX, moved: false };
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d) return;
          if (!d.moved && Math.abs(e.clientX - d.x0) < 4) return;
          d.moved = true;
          const b = barAt(e);
          setRange([Math.min(d.anchor, b) * bpb, (Math.max(d.anchor, b) + 1) * bpb]);
        }}
        onPointerUp={(e) => {
          const d = drag.current;
          drag.current = null;
          if (!d || d.moved) return;
          const r = e.currentTarget.getBoundingClientRect();
          seek(Math.max(0, Math.min(songBeats, (e.clientX - r.left) / beatW)));
          setRange(null);
        }}
        title="click: move the playhead · drag: pick bars for a starter"
      >
        {sel && <div className="st-sel" style={{ left: sel[0] * beatW, width: (sel[1] - sel[0]) * beatW }} />}
        {Array.from({ length: bars }, (_, i) => (
          <div key={i} className={`st-bar-tick${i % 4 === 0 ? " strong" : ""}`} style={{ left: i * barW }}>
            {i % every === 0 ? i + 1 : ""}
          </div>
        ))}
        <div className="st-end" style={{ left: songBeats * beatW }}>end</div>
        <PlayTri beatW={beatW} />
      </div>
    </div>
  );
}

/** Sections as colored spans. Double-click to add, drag to move (by bar), click to edit, right-click removes. */
function SectionStrip({
  beatW,
  songBeats,
  bpb,
  selectedId,
  onSelect,
  colorOf,
}: {
  beatW: number;
  songBeats: number;
  bpb: number;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  colorOf: (s: Section) => string;
}) {
  const sections = useStore((s) => s.snapshot!.model.sections);
  const dispatch = useStore((s) => s.dispatch);
  const [preview, setPreview] = useState<Section | null>(null);
  const drag = useRef<{ section: Section; x0: number; moved: boolean } | null>(null);
  const shown = sections.map((x) => (preview && x.id === preview.id ? preview : x)).sort((a, b) => a.startBeat - b.startBeat);

  return (
    <div className="st-row st-sections">
      <div className="st-head">
        <span className="cap">sections</span>
      </div>
      <div
        className="st-track"
        style={{ width: songBeats * beatW }}
        onDoubleClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const bar = Math.floor((e.clientX - r.left) / (beatW * bpb));
          void dispatch({ type: "addSection", name: "", startBeat: Math.max(0, Math.min(songBeats - bpb, bar * bpb)) });
        }}
        title="double-click to mark a section here"
      >
        {shown.length === 0 && <span className="st-empty">no sections yet. double-click to mark a verse, a chorus, a break</span>}
        {shown.map((sec, i) => {
          const next = shown[i + 1];
          const end = Math.min(songBeats, next ? next.startBeat : songBeats);
          const color = colorOf(sec);
          const k = sec.key?.rootPitchClass != null ? `${noteName(sec.key.rootPitchClass)} ${sec.key.scale === "major" ? "maj" : "min"}` : null;
          return (
            <div
              key={sec.id}
              className={`st-section${sec.id === selectedId ? " sel" : ""}`}
              style={{ left: sec.startBeat * beatW, width: Math.max(14, (end - sec.startBeat) * beatW - 1), borderLeftColor: color, background: `${color}22`, color }}
              onPointerDown={(e) => {
                e.stopPropagation();
                if (e.button === 2) {
                  if (sec.id === selectedId) onSelect(null);
                  void dispatch({ type: "removeSection", id: sec.id });
                  return;
                }
                if (e.button !== 0) return;
                e.currentTarget.setPointerCapture(e.pointerId);
                drag.current = { section: sec, x0: e.clientX, moved: false };
              }}
              onPointerMove={(e) => {
                const d = drag.current;
                if (!d) return;
                if (!d.moved && Math.abs(e.clientX - d.x0) < 4) return;
                d.moved = true;
                const beat = Math.round((d.section.startBeat + (e.clientX - d.x0) / beatW) / bpb) * bpb;
                setPreview({ ...d.section, startBeat: Math.max(0, Math.min(songBeats - bpb, beat)) });
              }}
              onPointerUp={() => {
                const d = drag.current;
                drag.current = null;
                const moved = preview;
                setPreview(null);
                if (!d) return;
                if (d.moved && moved && moved.startBeat !== d.section.startBeat) void dispatch({ type: "updateSection", section: moved });
                else if (!d.moved) onSelect(sec.id === selectedId ? null : sec.id);
              }}
              onDoubleClick={(e) => e.stopPropagation()}
              title={`${sec.name}${k ? ` · ${k}` : ""} · click: edit · drag: move · right-click: remove`}
            >
              <span className="nm">{sec.name}</span>
              {k && <span className="k">{k}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Behind the chords: section colors, the bar grid, and the picked range. */
function Underlay({ beatW, songBeats, bpb, sel, colorOf }: { beatW: number; songBeats: number; bpb: number; sel: [number, number] | null; colorOf: (s: Section) => string }) {
  const sections = useStore((s) => s.snapshot!.model.sections);
  const barW = beatW * bpb;
  const bars = Math.ceil(songBeats / bpb - 1e-9);
  return (
    <div className="st-underlay" aria-hidden>
      {sections.map((sec, i) => {
        const end = Math.min(songBeats, sections[i + 1]?.startBeat ?? songBeats);
        return <div key={sec.id} className="band" style={{ left: sec.startBeat * beatW, width: Math.max(0, (end - sec.startBeat) * beatW), background: `${colorOf(sec)}12` }} />;
      })}
      {beatW >= 10 && Array.from({ length: Math.floor(songBeats) }, (_, b) => (b % bpb === 0 ? null : <div key={`b${b}`} className="beat" style={{ left: b * beatW }} />))}
      {Array.from({ length: bars }, (_, i) => (
        <div key={i} className={`barline${i % 4 === 0 ? " strong" : ""}`} style={{ left: i * barW }} />
      ))}
      {sel && <div className="sel" style={{ left: sel[0] * beatW, width: (sel[1] - sel[0]) * beatW }} />}
    </div>
  );
}

function Playline({ beatW }: { beatW: number }) {
  const playhead = useStore((s) => s.playhead);
  return <div className="st-playline" style={{ left: HEAD + playhead * beatW }} />;
}

function PlayTri({ beatW }: { beatW: number }) {
  const playhead = useStore((s) => s.playhead);
  return <div className="st-phtri" style={{ left: playhead * beatW }} />;
}

// ─── Tools under the lanes ─────────────────────────────────────────────────────────────────

/** The selected chord, editable: in-key picks, root + quality, free build, name. */
function ChordPanel({ chord, onSelect }: { chord: ChordEvent | null; onSelect: (id: string | null) => void }) {
  const chords = useStore((s) => s.snapshot!.model.chords);
  const dispatch = useStore((s) => s.dispatch);
  const seek = useStore((s) => s.seek);
  const bpb = useStore(beatsPerBar);
  const i = chord ? chords.findIndex((c) => c.id === chord.id) : -1;

  if (!chord) {
    return (
      <div className="st-panel">
        <div className="st-ph">
          <span className="cap">chord</span>
        </div>
        <p className="st-guide">
          {chords.length === 0
            ? "no chords yet. pick a starter, or click the chord lane to build one."
            : "click a chord to edit it. click or drag on empty lane to add one; drag a chord's edges to make it longer or shorter."}
        </p>
        <p className="st-guide dim">{keys("⌥-drag copies · right-click or Delete removes · ← → step through chords")}</p>
      </div>
    );
  }
  const bar = Math.floor(chord.startBeat / bpb) + 1;
  const beat = +(chord.startBeat - (bar - 1) * bpb + 1).toFixed(2);
  return (
    <div className="st-panel">
      <div className="st-ph">
        <span className="cap">
          chord {i + 1} of {chords.length}
        </span>
        <span className="mono st-meta">
          bar {bar}
          {beat !== 1 ? ` beat ${beat}` : ""} · {lengthText(chord.lengthBeats, bpb)}
        </span>
        <span className="spacer" />
        <button className="ico" disabled={i <= 0} onClick={() => onSelect(chords[i - 1].id)} title="previous chord" aria-label="previous chord">‹</button>
        <button className="ico" disabled={i >= chords.length - 1} onClick={() => onSelect(chords[i + 1].id)} title="next chord" aria-label="next chord">›</button>
      </div>
      <ChordBuilder key={chord.id} chord={chord} onChange={(patch) => void dispatch({ type: "setChords", chords: chords.map((c) => (c.id === chord.id ? { ...c, ...patch } : c)) })} />
      <div className="row" style={{ gap: 4 }}>
        <button className="quiet" onClick={() => seek(chord.startBeat)} title="move the playhead here">play from here</button>
        <span className="spacer" />
        <button
          className="quiet danger"
          onClick={() => {
            onSelect(null);
            void dispatch({ type: "setChords", chords: chords.filter((c) => c.id !== chord.id) });
          }}
        >
          delete
        </button>
      </div>
    </div>
  );
}

/** What could follow the selected chord (or the last one): the usual moves first, in plain words. */
function NextPanel({ chord, target, onSelect }: { chord: ChordEvent | null; target: [number, number]; onSelect: (id: string | null) => void }) {
  const model = useStore((s) => s.snapshot!.model);
  const dispatch = useStore((s) => s.dispatch);
  const bpb = useStore(beatsPerBar);
  const chords = model.chords;
  const songBeats = model.songLengthBeats;
  // After the selected chord; with none selected, after the last one (or at the start of the picked bars).
  const after = chord ?? chords[chords.length - 1] ?? null;
  const at = after ? after.startBeat + after.lengthBeats : target[0];
  const len = Math.min(after?.lengthBeats ?? bpb, songBeats - at);
  const room = len > 1e-9;
  const k = keyAt(model, Math.min(at, songBeats - 1e-6));
  const root = k.rootPitchClass ?? 0;
  const ak = after ? keyAt(model, after.startBeat) : null;
  const from = after && ak ? degreeOf(after.pitchClasses, ak.rootPitchClass ?? 0, ak.scale) : null;
  const fromRoman = from !== null && ak ? diatonicChords(ak.rootPitchClass ?? 0, ak.scale)[from].romanNumeral : null;
  const list = nextChords(from, root, k.scale);

  const add = (pcs: number[], name: string) => {
    if (!room) return;
    const c: ChordEvent = { id: uuid(), startBeat: at, lengthBeats: len, pitchClasses: [...pcs].sort((a, b) => a - b), name };
    void dispatch({ type: "setChords", chords: placeChord(chords, c) });
    // The new chord becomes the one to follow, so clicking down the list builds a progression.
    onSelect(c.id);
  };
  const swap = (pcs: number[], name: string) => {
    if (!chord) return;
    void dispatch({ type: "setChords", chords: chords.map((c) => (c.id === chord.id ? { ...c, pitchClasses: [...pcs].sort((a, b) => a - b), name } : c)) });
  };

  return (
    <div className="st-panel">
      <div className="st-ph">
        <span className="cap">{after ? "what could come next" : "start with"}</span>
        <span className="mono st-meta">
          {after ? `after ${after.name ?? chordName(after.pitchClasses)}${fromRoman ? ` (${fromRoman})` : ""}` : `at bar ${Math.floor(at / bpb) + 1}`}
          {k.rootPitchClass === null ? " · in C (no key set)" : ` · in ${keyName(root, k.scale)}`}
        </span>
      </div>
      {!room && (
        <p className="st-guide">
          {chord ? "no room after it: that chord runs to the song's end. shorten it, or make the song longer." : "the chords fill the song. pick one to see what could follow it, or swap it."}
        </p>
      )}
      {(room || chord) && (
        <div className="st-next">
          {list.map((n, i) => (
            <div key={n.degree} className={`nx${n.common ? " common" : ""}${i > 0 && n.common !== list[i - 1].common ? " split" : ""}`}>
              <button className="nx-add" disabled={!room} onClick={() => add(n.pitchClasses, n.name)} title={`add ${n.name} at bar ${Math.floor(at / bpb) + 1}, ${lengthText(len, bpb)} long`}>
                <span className="rn">{n.roman}</span>
                <span className="nm">{n.name}</span>
                <span className="why">{n.why}</span>
              </button>
              {chord && (
                <button className="quiet nx-swap" onClick={() => swap(n.pitchClasses, n.name)} title={`turn the selected chord into ${n.name}`}>
                  swap
                </button>
              )}
            </div>
          ))}
        </div>
      )}
      <p className="st-guide dim">
        {after && from === null ? "that chord is outside the key, so here's the whole key." : "the bright rows are the usual moves. the rest work too: try them."}
      </p>
    </div>
  );
}

/** Progression starters by mood, tiled over the whole song or the picked bars, each section in its own key. */
function StarterPanel({ target, picked, onClearPick }: { target: [number, number]; picked: boolean; onClearPick: () => void }) {
  const model = useStore((s) => s.snapshot!.model);
  const dispatch = useStore((s) => s.dispatch);
  const bpb = useStore(beatsPerBar);
  const [every, setEvery] = useState(1);
  const [start, end] = target;
  const spans = keySpans(model.key, model.sections, start, end);
  const first = spans[0] ?? { root: model.key.rootPitchClass ?? 0, scale: model.key.scale };
  const chordBeats = every * bpb;

  // One setChords, so one undo puts the old progression back.
  const apply = (tiled: ChordEvent[]) =>
    void dispatch(
      { type: "setChords", chords: replaceChordWindow(model.chords, start, end - start, tiled.map((c) => ({ ...c, startBeat: c.startBeat - start }))) },
      false,
      tiled.length ? "starter progression" : "clear chords",
    );
  const where = picked ? `bars ${Math.floor(start / bpb) + 1}–${Math.ceil(end / bpb)}` : `the whole song (${barsText(end - start, bpb)} bars)`;

  return (
    <div className="st-panel">
      <div className="st-ph">
        <span className="cap">starters</span>
        <span className="mono st-meta">
          in {keyName(first.root, first.scale)}
          {spans.length > 1 ? ` + ${spans.length - 1} section key${spans.length > 2 ? "s" : ""}` : ""}
        </span>
        <span className="spacer" />
        <span className="flabel">a chord every</span>
        <div className="seg sm">
          {(
            [
              [0.5, "½"],
              [1, "1"],
              [2, "2"],
            ] as const
          ).map(([v, l]) => (
            <div key={v} className={every === v ? "on" : ""} onClick={() => setEvery(v)}>
              {l}
            </div>
          ))}
        </div>
        <span className="flabel">bar{every === 1 ? "" : "s"}</span>
      </div>
      <p className="st-guide">
        fills {where}
        {picked ? (
          <>
            {" · "}
            <button className="linkish" onClick={onClearPick}>use the whole song</button>
          </>
        ) : (
          " · drag the bar ruler to fill just part of it"
        )}
      </p>
      <div className="chipsrow">
        {startersFor(first.scale).map((st) => {
          const tiled = tileStarter(st.degrees, spans, chordBeats);
          // One pass of the degrees, named in this key: what the chip writes.
          const once = progression(st.degrees, first.root, first.scale, st.degrees.length * 4)
            .map((c) => c.name)
            .join(" · ");
          return (
            <button key={st.name} className={`chip starter${matchesWindow(model.chords, tiled, start, end) ? " on" : ""}`} title={`${st.name} · ${once}`} onClick={() => apply(tiled)}>
              <span className="mood">{st.mood}</span>
              <span className="chords">{once}</span>
            </button>
          );
        })}
      </div>
      <div className="row" style={{ gap: 4 }}>
        <button className="quiet" disabled={!model.chords.some((c) => c.startBeat < end && c.startBeat + c.lengthBeats > start)} onClick={() => apply([])}>
          clear chords{picked ? " in these bars" : ""}
        </button>
      </div>
    </div>
  );
}

/** Every section in order, with the selected one's editor under the list. */
function SectionPanel({
  section,
  onSelect,
  colorOf,
  addAt,
  onAdd,
}: {
  section: Section | null;
  onSelect: (id: string | null) => void;
  colorOf: (s: Section) => string;
  addAt: number | null;
  onAdd: (beat: number) => void;
}) {
  const model = useStore((s) => s.snapshot!.model);
  const playhead = useStore((s) => s.playhead);
  const bpb = useStore(beatsPerBar);
  const songBeats = model.songLengthBeats;
  const sections = model.sections;
  // New sections go at the picked bars, else at the playhead's bar.
  const at = Math.min(songBeats - bpb, Math.max(0, addAt ?? Math.floor(playhead / bpb) * bpb));
  const taken = sections.some((x) => Math.abs(x.startBeat - at) < 1e-9);

  return (
    <div className="st-panel">
      <div className="st-ph">
        <span className="cap">sections</span>
        <span className="mono st-meta">song key {model.key.rootPitchClass !== null ? keyName(model.key.rootPitchClass, model.key.scale) : "not set"}</span>
        <span className="spacer" />
        <button className="chip tiny" disabled={taken} onClick={() => onAdd(at)} title={taken ? "there's already a section on that bar" : undefined}>
          + at bar {Math.floor(at / bpb) + 1}
        </button>
      </div>
      {sections.length === 0 ? (
        <p className="st-guide">no sections yet. they're markers (verse, chorus, a break), and each one can switch key.</p>
      ) : (
        <div className="st-seclist">
          {sections.map((x, i) => {
            const end = Math.min(songBeats, sections[i + 1]?.startBeat ?? songBeats);
            return (
              <div key={x.id} className={`sr${section?.id === x.id ? " sel" : ""}`} onClick={() => onSelect(section?.id === x.id ? null : x.id)}>
                <span className="sw" style={{ background: colorOf(x) }} />
                <span className="nm">{x.name}</span>
                <span className="mono">bar {Math.floor(x.startBeat / bpb) + 1}</span>
                <span className="mono dim">{lengthText(Math.max(0, end - x.startBeat), bpb)}</span>
                <span className="mono key">{x.key?.rootPitchClass != null ? keyName(x.key.rootPitchClass, x.key.scale) : "song key"}</span>
              </div>
            );
          })}
        </div>
      )}
      {section && <SectionEditor key={section.id} section={section} color={colorOf(section)} onClose={() => onSelect(null)} docked />}
    </div>
  );
}
