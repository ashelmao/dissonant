import { useState } from "react";
import { barTicks, dbText, effectiveKey, GRID_OPTIONS, gridLabel, selectedPattern, selectedTrack, selectedTrackIndex, usePatternChords, useStore } from "../store";
import { chordAt, explainNote, keyName, midiName, noteName, noteTier, tierMap } from "../theory";
import { arpeggiateNotes, chopNotes, humanizeNotes, invertNotes, legatoNotes, quantizeNotes, resolveTargets, reverseNotes, scaleNotes, setLength, strumNotes, toggleMute } from "../noteEditing";
import { EQ_LABELS, keyAt, songLength, SOUND_PRESETS, soundPresetOf, TICKS_PER_BEAT, TRACK_PALETTE, trackColor, type PitchPoint, type TrackParam } from "../types";
import { padName } from "../drumKit";
import { useMediaQuery } from "../uiScale";
import { hz, Knob, lin, log, panText, pct } from "./Knob";
import { InstrumentPicker, PluginParams } from "./PluginBrowser";
import { barLabel, PatternBars } from "./Editor";
import { SoundPicker } from "./SoundPicker";
import { Splitter } from "./Splitter";
import { usePanelSize } from "../panels";

const VOL = lin(0, 1.5);
const PAN = lin(-1, 1);
const TONE = log(200, 20000);

/** Remembered per machine; with no choice yet, a narrow window starts with it folded away. */
const COLLAPSE_KEY = "dissonant.inspector.collapsed";
const NARROW = "(max-width: 1279px)";
function readCollapsed(): boolean | null {
  try {
    const v = localStorage.getItem(COLLAPSE_KEY);
    return v === null ? null : v === "1";
  } catch {
    return null;
  }
}

/**
 * Right column in PAT mode: the selected pattern, its notes, transforms, the harmony it's
 * tiered against (read-only: chords and keys are edited in SETUP), the track's instrument
 * (synth, CLAP plugin or amp rig), and where the track sits in the song. Tracks themselves
 * live in the tree. Folds to a rail.
 */
export function Inspector() {
  const s = useStore();
  const model = s.snapshot!.model;
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const trackIdx = useStore(selectedTrackIndex);
  const bticks = useStore(barTicks);
  const narrow = useMediaQuery(NARROW);
  const insW = usePanelSize("inspector");
  const [pref, setPref] = useState<boolean | null>(readCollapsed);
  const [draft, setDraft] = useState("");
  const [renamingPattern, setRenamingPattern] = useState(false);
  // The project chords under this pattern (from where it was opened), pattern-relative.
  const chords = usePatternChords();
  const collapsed = pref ?? narrow;
  const setCollapsed = (v: boolean) => {
    setPref(v);
    try {
      localStorage.setItem(COLLAPSE_KEY, v ? "1" : "0");
    } catch {
      /* storage off: it just won't be remembered */
    }
  };

  if (collapsed) {
    return (
      <aside className="side inspector collapsed">
        <button className="rail" onClick={() => setCollapsed(false)} title="show the inspector: pattern, notes, transforms, harmony, instrument">
          <span aria-hidden="true">‹</span>
          <span className="vlabel">inspector</span>
        </button>
      </aside>
    );
  }
  if (!pattern || !track) {
    return (
      <>
        <Splitter panel="inspector" grows="before" label="the inspector" />
        <aside className="side inspector" style={{ width: insW }}>
          <InspectorHead onCollapse={() => setCollapsed(true)} />
          <div className="section">
            <span className="mono" style={{ fontSize: 9.5, color: "var(--text-4)", lineHeight: 1.45 }}>
              pick a pattern in the tree, or make one with the + on its track.
            </span>
          </div>
        </aside>
      </>
    );
  }
  const color = trackColor(track, trackIdx);
  const notes = pattern.notes;
  const selected = notes.filter((n) => s.selectedNoteIds.includes(n.id));
  const stats = selected.length ? selected : notes;

  const range = (xs: number[]) => (xs.length ? [Math.min(...xs), Math.max(...xs)] : null);
  const pitchR = range(stats.map((n) => n.pitch));
  const velR = range(stats.map((n) => n.velocity));
  const lenR = range(stats.map((n) => n.lengthTicks));

  // Harmony under the playhead: the playhead runs inside the pattern, the song position is
  // `chordOffsetTicks` + playhead, and the key is that song position's section key.
  const chord = chordAt(chords, s.playhead);
  const liveKey = effectiveKey(s);
  const nowKey = keyAt(model, s.chordOffsetTicks + s.playhead);
  const map = tierMap(s.playhead, chords, nowKey);
  const root = nowKey.rootPitchClass;
  const isSectionKey = nowKey !== model.key;

  const param = (p: TrackParam, value: number) => void s.dispatch({ type: "setTrackParam", id: track.id, param: p, value }, true);
  const commit = () => void s.commitGesture();

  // Overview geometry
  const total = songLength(model);
  const trackPatternIds = new Set(model.patterns.filter((p) => p.trackId === track.id).map((p) => p.id));

  return (
    <>
      <Splitter panel="inspector" grows="before" label="the inspector" />
      <aside className="side inspector" style={{ width: insW }}>
        <InspectorHead onCollapse={() => setCollapsed(true)} />
      <div className="scrollbody">
        {/* PATTERN */}
        <div className="section">
          <div className="head">
            <span className="cap">pattern</span>
            <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>
              {track.name} · {+(pattern.lengthTicks / bticks).toFixed(2)} bars
            </span>
          </div>
          <div className="namebar" style={{ background: color }} onClick={() => !renamingPattern && (setDraft(pattern.name), setRenamingPattern(true))} title="click to rename">
            {renamingPattern ? (
              <input
                type="text"
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onBlur={() => {
                  if (draft.trim()) void s.dispatch({ type: "renamePattern", id: pattern.id, name: draft.trim() });
                  setRenamingPattern(false);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  if (e.key === "Escape") setRenamingPattern(false);
                }}
              />
            ) : (
              pattern.name
            )}
          </div>
          <div className="swatches" title={`${track.name}'s color (every pattern on the track shares it)`}>
            {TRACK_PALETTE.map((c) => (
              <div key={c} className={c === color ? "on" : ""} style={{ background: c }} onClick={() => s.dispatch({ type: "setTrackColor", id: track.id, color: c })} />
            ))}
          </div>
          <div className="field" title={`the patterns on ${track.name}`}>
            <span className="flabel">pattern</span>
            <select value={pattern.id} onChange={(e) => s.selectPattern(e.target.value)} style={{ minWidth: 0, maxWidth: 150 }}>
              {model.patterns.filter((p) => p.trackId === track.id).map((p) => (
                <option key={p.id} value={p.id}>{p.name}</option>
              ))}
            </select>
          </div>
          <div className="field" title="length in bars: the chips are the usual four, the field is every other number">
            <span className="flabel">length</span>
            <div className="row" style={{ gap: 3 }}>
              {[1, 2, 4, 8].map((bars) => (
                <button key={bars} className={`chip tiny${pattern.lengthTicks === bars * bticks ? " on" : ""}`} onClick={() => s.dispatch({ type: "setPatternLength", id: pattern.id, ticks: bars * bticks })}>
                  {bars}
                </button>
              ))}
              <PatternBars pattern={pattern} barTick={bticks} />
            </div>
          </div>
          <div className="field">
            <span className="flabel">{track.isDrum ? "hits" : "notes"}</span>
            <span className="val">{notes.length}</span>
          </div>
          <div className="row" style={{ gap: 4 }}>
            <button onClick={() => s.dispatch({ type: "addPattern", trackId: track.id })} title={`a new empty pattern on ${track.name}`}>+ new</button>
            <button onClick={() => s.dispatch({ type: "duplicatePattern", id: pattern.id })} title={`a copy of ${pattern.name} on ${track.name}`}>dup</button>
            <button
              onClick={async () => {
                if (await s.confirm("Delete pattern?", `“${pattern.name}” and its clips in the song will be removed.`, "Delete", true)) {
                  void s.dispatch({ type: "deletePattern", id: pattern.id });
                }
              }}
            >
              del
            </button>
          </div>
        </div>

        {/* NOTES */}
        <div className="section" style={{ gap: 6 }}>
          <div className="head">
            <span className="cap">{track.isDrum ? "hits" : "notes"} · {notes.length}</span>
            {selected.length > 0 && <span className="badge">{selected.length} selected</span>}
          </div>
          <RangeRow label="pitch" lo={pitchR ? (pitchR[0] - 24) / 72 : 0} hi={pitchR ? (pitchR[1] - 24) / 72 : 0} color={color} text={pitchR ? (track.isDrum ? `${pitchR[0]} – ${pitchR[1]}` : `${midiName(pitchR[0])} – ${midiName(pitchR[1])}`) : "—"} />
          <RangeRow label="velocity" lo={velR ? velR[0] / 127 : 0} hi={velR ? velR[1] / 127 : 0} color={color} text={velR ? `${velR[0]} – ${velR[1]}` : "—"} />
          <RangeRow label="length" lo={lenR ? Math.min(1, lenR[0] / (4 * TICKS_PER_BEAT)) : 0} hi={lenR ? Math.min(1, lenR[1] / (4 * TICKS_PER_BEAT)) : 0} color="#9a9aa4" text={lenR ? `${gridLabel(lenR[0])} – ${gridLabel(lenR[1])}` : "—"} />
          {selected.length > 0 && !track.isDrum && (
            <div className="chipsrow" title="set the length of every selected note">
              {GRID_OPTIONS.map(([label, v]) => (
                <button key={label} className={`chip tiny${selected.every((n) => n.lengthTicks === v) ? " on" : ""}`} onClick={() => s.transformSelection((ns, ids) => setLength(ns, ids, v), "set note length")}>
                  {label}
                </button>
              ))}
              <button className={`chip tiny${selected.every((n) => n.muted) ? " on" : ""}`} onClick={() => s.transformSelection(toggleMute, "mute notes")} title="mute the selected notes (0): kept, drawn hollow, not played">
                {selected.every((n) => n.muted) ? "muted" : "mute"}
              </button>
            </div>
          )}
          {selected.length === 1 && !track.isDrum && <ResolveKeep note={selected[0]} />}
        </div>

        {/* TRANSFORM */}
        {!track.isDrum && (
          <div className="section">
            <div className="head">
              <span className="cap">transform</span>
              <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>{selected.length ? `${selected.length} selected` : "all notes"}</span>
            </div>
            <TransformGrid />
            <div className="chipsrow">
              <button className={`chip${s.magnet ? " on" : ""}`} onClick={() => s.toggleMagnet()} title="pull placed and dragged notes to the nearest chord tone (then tension) — a magnet, not a wall">magnet</button>
              <button className={`chip${s.stamp ? " on" : ""}`} onClick={() => s.toggleStamp()} title="click places the whole chord under the cursor, voiced upward">stamp chord</button>
            </div>
          </div>
        )}

        {/* HARMONY (read-only: the progression, sections and keys live in SETUP) */}
        {!track.isDrum && (
          <div className="section">
            <div className="head">
              <span className="cap">harmony</span>
              <button className="chip tiny" onClick={() => s.setScreen("setup")} title="chords, sections and keys are edited in SETUP">
                edit in setup ›
              </button>
            </div>
            <div className="row" style={{ justifyContent: "space-between" }}>
              <span className="flabel">{isSectionKey ? "section key" : "key"}</span>
              <span className="mono" style={{ fontSize: 10, color: root !== null ? (model.key.isLocked || isSectionKey ? "var(--accent)" : "var(--text-2)") : "var(--text-5)" }}>
                {root !== null ? keyName(root, nowKey.scale) : "not set"}
              </span>
            </div>
            <div className="row" style={{ justifyContent: "space-between" }}>
              <span className="flabel">now</span>
              <span className="mono" style={{ fontSize: 11, color: chord ? "var(--text-1)" : "var(--text-5)" }}>{chord?.name ?? (root !== null ? "scale only" : "no guidance yet")}</span>
            </div>
            <div className="row" style={{ justifyContent: "space-between" }}>
              <span className="flabel">chords from</span>
              <span className="mono" style={{ fontSize: 10, color: "var(--text-2)" }}>song {barLabel(s.chordOffsetTicks, bticks)}</span>
            </div>
            {root === null && chords.length === 0 && (
              <div className="mono" style={{ fontSize: 9.5, lineHeight: 1.45, color: "var(--text-4)" }}>
                no key or chords yet, so every note looks the same. pick a progression in SETUP, or just play a few notes and lock the key the KEY cell hears.
              </div>
            )}
            <div className="scalestrip" title="how each pitch class fits the chord under the playhead">
              {[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map((pc) => (
                <div key={pc} className={map[pc] ?? ""} style={pc === root ? { boxShadow: "inset 0 0 0 1px var(--accent)" } : undefined}>
                  {noteName(pc).replace("#", "♯")}
                </div>
              ))}
            </div>
            <div className="tierlegend" aria-label="tier legend">
              <span><i className="sw chordTone" />chord tone</span>
              <span><i className="sw tension" />tension</span>
              <span><i className="sw dissonance">!</i>dissonant</span>
            </div>
            {notes.length > 0 && (
              <div className="chipsrow" title="select every note of this pattern by how it fits the chord it sits on">
                <span className="flabel" style={{ flex: "0 0 100%" }}>select by tier</span>
                {(["chordTone", "tension", "dissonance"] as const).map((t) => {
                  const ofTier = notes.filter((n) => tierMap(n.startTick, chords, liveKey)[((n.pitch % 12) + 12) % 12] === t);
                  return (
                    <button
                      key={t}
                      className="chip tiny"
                      disabled={!ofTier.length}
                      style={{ color: ofTier.length ? `var(--tier-${t === "chordTone" ? "solid" : t})` : undefined }}
                      onClick={() => s.requestSelection(ofTier.map((n) => n.id))}
                    >
                      {t === "chordTone" ? "chord tones" : t === "tension" ? "tensions" : "dissonant"} · {ofTier.length}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        )}

        {/* INSTRUMENT — an audio track has a rig instead, which lives on the rec page. */}
        <div className="section">
          <div className="head">
            <span className="cap">{track.isAudio ? "rig" : "instrument"} · {track.name}</span>
            <span className="mono" style={{ fontSize: 9, color: "var(--text-4b)" }}>
              {track.isAudio
                ? track.amp?.enabled
                  ? `${track.amp.amp.model} · ${EQ_LABELS[track.amp.eq]} · ${track.amp.pedals.filter((p) => p.on).length} pedals`
                  : "bypassed"
                : track.plugin
                  ? "clap plugin"
                  : track.isDrum
                    ? `${track.drumPads.length} pads`
                    : "waveform synth"}
            </span>
          </div>
          {track.isAudio ? (
            <div className="devchip" style={{ cursor: "pointer" }} onClick={() => s.setScreen("rec")} title="open the rec page">
              <span className={`led${track.amp?.enabled ? " on" : ""}`} />
              <span className="n">amp + cab</span>
              <span className="spacer" />
              <span className="meta">edit on the rec page →</span>
            </div>
          ) : (
            <>
              <div
                className="devchip"
                title={track.isDrum && !track.plugin ? `${track.drumPads.map((p) => `${p.pitch} ${padName(model.sounds ?? [], p)}`).join(" · ")}\nedit the kit in SONG → devices` : undefined}
              >
                <span className="led" />
                <span className="n">{track.plugin ? track.plugin.name : track.isDrum ? "drum kit" : "synth"}</span>
                <span className="spacer" />
                <span className="meta kitnames">
                  {track.plugin ? "hosted" : track.isDrum ? track.drumPads.map((p) => padName(model.sounds ?? [], p)).join(" · ") : "waveform synth"}
                </span>
              </div>
              <InstrumentPicker columns={3} />
              {track.plugin && <PluginParams trackId={track.id} uid={track.plugin.uid} state={track.plugin.state} />}
              {!track.plugin && !track.isDrum && (
                <div className="row">
                  <SoundPicker kind="synth" value={track.voice} onPick={(sound) => void s.dispatch({ type: "setTrackSound", id: track.id, sound }, false, `sound: ${sound.name}`)} title="pick this track's sound" />
                  <button className="chip tiny" onClick={() => s.setScreen("sound")} title="open the SOUND page to shape this sound">edit ↗</button>
                </div>
              )}
              {!track.plugin && !track.isDrum && (
                <div className="chipsrow" style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)" }} title="sounds by character: each sets the waveform, tone and reverb">
                  {SOUND_PRESETS.map((p) => (
                    <button key={p.name} className={`chip${soundPresetOf(track)?.name === p.name ? " on" : ""}`} style={{ textTransform: "none" }} onClick={() => void s.applySoundPreset(track.id, p)}>
                      {p.name}
                    </button>
                  ))}
                </div>
              )}
            </>
          )}
          <div className="knobrow">
            <Knob label="vol" value={VOL.to(track.volume)} display={`${dbText(track.volume)}`} color="#9a9aa4" defaultValue={VOL.to(1)} onChange={(p) => param("volume", VOL.from(p))} onCommit={commit} />
            <Knob label="pan" value={PAN.to(track.pan)} display={panText(track.pan)} color="#9a9aa4" defaultValue={0.5} onChange={(p) => param("pan", PAN.from(p))} onCommit={commit} />
            <Knob label="tone" value={TONE.to(track.tone)} display={hz(track.tone)} color="#3dc8ff" defaultValue={1} onChange={(p) => param("tone", TONE.from(p))} onCommit={commit} />
            <Knob label="reverb" value={track.reverbSend} display={pct(track.reverbSend)} color="#b48cff" defaultValue={0} onChange={(p) => param("reverbSend", p)} onCommit={commit} />
          </div>
        </div>

        {/* TRACK OVERVIEW */}
        <div className="section" style={{ borderBottom: 0, gap: 6 }}>
          <span className="cap">in the song · {track.name}</span>
          <div className="overview" title="this track's clips in the song; the pattern you're editing is highlighted. click a clip to edit it against the chords where it sits">
            {model.clips.filter((c) => trackPatternIds.has(c.patternId)).map((c) => {
              const p = model.patterns.find((x) => x.id === c.patternId);
              const current = c.patternId === pattern.id;
              return (
                <div
                  key={c.id}
                  style={{ left: `${(c.startTick / Math.max(1, total)) * 100}%`, width: `calc(${(c.lengthTicks / Math.max(1, total)) * 100}% - 1px)`, background: current ? color : `${color}55`, boxShadow: current ? "0 0 0 1px #fff" : undefined, cursor: "pointer", opacity: c.muted ? 0.4 : 1 }}
                  onClick={() => s.openPattern(c.patternId, c.startTick)}
                  title={`${p?.name ?? "?"} · ${barLabel(c.startTick, bticks)}`}
                />
              );
            })}
          </div>
          <div className="overview-scale">
            <span>1</span>
            <span>{Math.max(1, Math.round(total / bticks / 2) + 1)}</span>
            <span>{Math.max(1, Math.round(total / bticks) + 1)}</span>
          </div>
        </div>
      </div>
      </aside>
    </>
  );
}

function InspectorHead({ onCollapse }: { onCollapse: () => void }) {
  return (
    <div className="inshead">
      <span className="cap">inspector</span>
      <button className="ico" onClick={onCollapse} title="fold the inspector away (more room for the roll)" aria-label="fold the inspector away">
        ›
      </button>
    </div>
  );
}

function TransformGrid() {
  const s = useStore();
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const key = useStore(effectiveKey);
  const chords = usePatternChords();
  // Quantize at 100% is a machine; the dial is how much of the grid's opinion to take, and the
  // humanize amounts are the same question asked the other way round. They are view state (a
  // feel you dial in while writing), so they live with the panel, not in the project.
  const [strength, setStrength] = useState(1);
  const [drift, setDrift] = useState(0.15);
  const [velDrift, setVelDrift] = useState(12);
  if (!pattern || !track) return null;
  const notes = pattern.notes;
  const ids = new Set(s.selectedNoteIds);
  const grid = s.noteLength;
  const tierOf = (n: { pitch: number; startTick: number }) => tierMap(n.startTick, chords, key)[((n.pitch % 12) + 12) % 12];
  const apply = (next: typeof notes, label: string) => void s.dispatch({ type: "setNotes", patternId: pattern.id, notes: next }, false, label);
  const tools: [string, string, () => void][] = [
    ["quantize", `pull starts and lengths toward the grid, ${Math.round(strength * 100)}% of the way`, () => apply(quantizeNotes(notes, ids, grid, strength), "quantize")],
    ["humanize", `loosen timing by up to ${Math.round(drift * 100)}% of the grid and velocity by ±${velDrift}; chord tones stay tighter`, () => apply(humanizeNotes(notes, ids, { timing: grid * drift, velocity: velDrift }, tierOf), "humanize")],
    ["legato", "extend each note to the next one", () => apply(legatoNotes(notes, ids), "legato")],
    ["arp ↑", "spread stacked notes into rising grid steps", () => apply(arpeggiateNotes(notes, ids, grid, "up"), "arpeggiate")],
    ["arp ↓", "spread stacked notes into falling grid steps", () => apply(arpeggiateNotes(notes, ids, grid, "down"), "arpeggiate")],
    ["arp ⇅", "up then down", () => apply(arpeggiateNotes(notes, ids, grid, "updown"), "arpeggiate")],
    ["strum", "offset the notes of each stack, low to high", () => apply(strumNotes(notes, ids, grid / 4), "strum")],
    ["chop", "split notes into grid-length pieces", () => apply(chopNotes(notes, ids, grid), "chop")],
    ["arp ?", "random order", () => apply(arpeggiateNotes(notes, ids, grid, "random"), "arpeggiate")],
    ["×2 slower", "stretch the selection to twice its length (half speed)", () => apply(scaleNotes(notes, ids, 2, grid / 2), "stretch notes")],
    ["½ faster", "squeeze the selection to half its length (double speed)", () => apply(scaleNotes(notes, ids, 0.5, grid / 2), "squeeze notes")],
    ["reverse", "play the selection backwards in time", () => apply(reverseNotes(notes, ids), "reverse notes")],
    ["invert", "mirror pitches around the selection's middle", () => apply(invertNotes(notes, ids, { min: 24, max: 96 }), "invert notes")],
  ];
  return (
    <>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 3 }}>
        {tools.map(([label, title, fn]) => (
          <button key={label} title={title} onClick={fn} disabled={notes.length === 0} style={{ background: "var(--bg-2)", color: "var(--text-2)" }}>
            {label}
          </button>
        ))}
      </div>
      <DialRow label="strength" title="how far quantize pulls a note to the grid: 100% is dead on it, less keeps the hand that played it" value={strength} min={0} max={1} step={0.05} text={`${Math.round(strength * 100)}%`} onChange={setStrength} />
      <DialRow label="drift" title={`how far humanize moves a start, as a share of the ${gridLabel(grid)} grid`} value={drift} min={0} max={0.5} step={0.01} text={`${Math.round(drift * 100)}%`} onChange={setDrift} />
      <DialRow label="vel ±" title="how far humanize moves a velocity" value={velDrift} min={0} max={48} step={1} text={`${velDrift}`} onChange={setVelDrift} />
    </>
  );
}

/** One slider of the transform panel: a label, the slider and the value it reads as. */
function DialRow({ label, title, value, min, max, step, text, onChange }: { label: string; title: string; value: number; min: number; max: number; step: number; text: string; onChange: (v: number) => void }) {
  return (
    <div className="field" title={title}>
      <span className="flabel">{label}</span>
      <input type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(parseFloat(e.target.value))} style={{ flex: 1, minWidth: 0 }} />
      <span className="val mono" style={{ fontSize: 9.5, minWidth: 28, textAlign: "right" }}>{text}</span>
    </div>
  );
}

/** For one selected dissonant note: say why, offer the two nearest chord tones, or keep it on purpose. */
function ResolveKeep({ note }: { note: { id: string; pitch: number; startTick: number; intentional?: boolean; pitchEnvelope?: PitchPoint[] } }) {
  const s = useStore();
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const key = useStore(effectiveKey);
  const chords = usePatternChords();
  if (!pattern || !track) return null;
  const tiers = tierMap(note.startTick, chords, key);
  const { tier: t, moving } = noteTier(note, chords, key);
  const why = explainNote(note.pitch, chords, key, note.startTick, moving);
  const notes = pattern.notes;
  const update = (patch: Partial<{ pitch: number; intentional: boolean }>) =>
    void s.dispatch(
      { type: "setNotes", patternId: pattern.id, notes: notes.map((n) => (n.id === note.id ? { ...n, ...patch } : n)) },
      false,
      "pitch" in patch ? "resolve note" : "keep note",
    );
  const { down, up } = resolveTargets(note.pitch, tiers);
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 5, marginTop: 2 }}>
      <div className="mono" style={{ fontSize: 9.5, color: t === "dissonance" ? "var(--tier-dissonance)" : t === "tension" ? "var(--tier-tension)" : "var(--tier-solid)" }}>
        {why}
      </div>
      {t === "dissonance" && (
        <div className="chipsrow">
          {down !== null && <button className="chip" onClick={() => (update({ pitch: down }), s.audition(down))}>↓ {midiName(down)}</button>}
          {up !== null && <button className="chip" onClick={() => (update({ pitch: up }), s.audition(up))}>↑ {midiName(up)}</button>}
          <button className={`chip${note.intentional ? " on" : ""}`} onClick={() => update({ intentional: !note.intentional })} title="it's dissonant on purpose: the hatch stays, the flag calms down">
            {note.intentional ? "kept ✓" : "keep"}
          </button>
        </div>
      )}
    </div>
  );
}

function RangeRow({ label, lo, hi, color, text }: { label: string; lo: number; hi: number; color: string; text: string }) {
  const a = Math.min(1, Math.max(0, lo));
  const b = Math.min(1, Math.max(a, hi));
  return (
    <div className="rangerow">
      <span className="flabel">{label}</span>
      <div className="rangebar">
        <div style={{ left: `${a * 100}%`, width: `${Math.max(1.5, (b - a) * 100)}%`, background: color }} />
      </div>
      <span className="v">{text}</span>
    </div>
  );
}
