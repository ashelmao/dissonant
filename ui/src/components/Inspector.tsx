import { useState } from "react";
import { beatsPerBar, dbText, effectiveKey, GRID_OPTIONS, gridLabel, selectedPattern, selectedTrack, selectedTrackIndex, usePatternChords, useStore } from "../store";
import { chordAt, explainNote, midiName, NOTE_NAMES, noteName, progression, startersFor, tierMap } from "../theory";
import { arpeggiateNotes, chopNotes, humanizeNotes, invertNotes, legatoNotes, quantizeNotes, resolveTargets, reverseNotes, scaleNotes, setLength, strumNotes, toggleMute } from "../noteEditing";
import { DRUM_KIT, replaceChordWindow, SOUND_PRESETS, soundPresetOf, TRACK_PALETTE, trackColor, VOICES, type ScaleType, type TrackParam } from "../types";
import { hz, Knob, lin, log, panText, pct } from "./Knob";

const VOL = lin(0, 1.5);
const PAN = lin(-1, 1);
const TONE = log(200, 20000);

/** Left column in PAT mode: tracks, pattern, selection, harmony, instrument, overview. */
export function Inspector() {
  const s = useStore();
  const model = s.snapshot!.model;
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const trackIdx = useStore(selectedTrackIndex);
  const bpb = useStore(beatsPerBar);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [renamingPattern, setRenamingPattern] = useState(false);
  // The project chords under this pattern (from where it was opened), pattern-relative.
  const chords = usePatternChords();

  if (!pattern || !track) return null;
  const color = trackColor(track, trackIdx);
  const notes = pattern.notes;
  const selected = notes.filter((n) => s.selectedNoteIds.includes(n.id));
  const stats = selected.length ? selected : notes;

  const range = (xs: number[]) => (xs.length ? [Math.min(...xs), Math.max(...xs)] : null);
  const pitchR = range(stats.map((n) => n.pitch));
  const velR = range(stats.map((n) => n.velocity));
  const lenR = range(stats.map((n) => n.lengthBeats));

  const chord = chordAt(chords, s.playhead);
  const liveKey = effectiveKey(s);
  const map = tierMap(s.playhead, chords, liveKey);
  const root = model.key.rootPitchClass;
  const sectionKey = liveKey !== model.key ? liveKey : null;

  const finishRename = (id: string) => {
    if (draft.trim()) void s.dispatch({ type: "renameTrack", id, name: draft.trim() });
    setRenaming(null);
  };

  const setKey = (rootPc: number | null, scale: ScaleType) =>
    void s.dispatch({ type: "setKey", key: { rootPitchClass: rootPc, scale, isLocked: rootPc !== null } });

  // Writes the window under this pattern back into the song progression.
  const commitChords = (next: typeof chords) =>
    void s.dispatch({ type: "setChords", chords: replaceChordWindow(model.chords, s.chordOffsetBeats, pattern.lengthBeats, next) });
  const param = (p: TrackParam, value: number) => void s.dispatch({ type: "setTrackParam", id: track.id, param: p, value }, true);
  const commit = () => void s.commitGesture();

  // Overview geometry
  const total = model.songLengthBeats;
  const trackPatternIds = new Set(model.patterns.filter((p) => p.trackId === track.id).map((p) => p.id));

  return (
    <aside className="side inspector">
      {/* TRACKS */}
      <div className="section" style={{ gap: 6 }}>
        <div className="head">
          <span className="cap">tracks</span>
          <button className="chip tiny" onClick={() => s.dispatch({ type: "addTrack", isDrum: false })}>+ inst</button>
          <button className="chip tiny" onClick={() => s.dispatch({ type: "addTrack", isDrum: true })}>+ drum</button>
        </div>
        <div className="list">
          {model.tracks.map((t, i) => (
            <div key={t.id} className={`lrow${t.id === track.id ? " on" : ""}`} onClick={() => s.selectTrack(t.id)}>
              <span className="strip" style={{ background: trackColor(t, i) }} />
              <span className="num">{String(i + 1).padStart(2, "0")}</span>
              {renaming === t.id ? (
                <input
                  type="text"
                  autoFocus
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => finishRename(t.id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") finishRename(t.id);
                    if (e.key === "Escape") setRenaming(null);
                  }}
                  onClick={(e) => e.stopPropagation()}
                />
              ) : (
                <span className={`name${t.muted ? " muted" : ""}`} onDoubleClick={() => (setDraft(t.name), setRenaming(t.id))}>
                  {t.name}
                </span>
              )}
              <span className="meta">{t.isDrum ? "drum" : t.voice}</span>
              <button className={`msr m${t.muted ? " on" : ""}`} onClick={(e) => (e.stopPropagation(), s.dispatch({ type: "setTrackMuted", id: t.id, muted: !t.muted }))}>M</button>
              <button className={`msr s${t.soloed ? " on" : ""}`} onClick={(e) => (e.stopPropagation(), s.dispatch({ type: "setTrackSoloed", id: t.id, soloed: !t.soloed }))}>S</button>
              <button className="ico" title="move up" onClick={(e) => (e.stopPropagation(), s.dispatch({ type: "moveTrack", id: t.id, up: true }))}>▲</button>
              <button className="ico" title="move down" onClick={(e) => (e.stopPropagation(), s.dispatch({ type: "moveTrack", id: t.id, up: false }))}>▼</button>
              {model.tracks.length > 1 && (
                <button
                  className="ico"
                  title="delete track"
                  onClick={async (e) => {
                    e.stopPropagation();
                    if (await s.confirm("Delete track?", `“${t.name}”, its patterns and their clips in the song will be removed.`, "Delete", true)) {
                      void s.dispatch({ type: "deleteTrack", id: t.id });
                    }
                  }}
                >
                  ×
                </button>
              )}
            </div>
          ))}
        </div>
      </div>

      {/* PATTERN */}
      <div className="section">
        <div className="head">
          <span className="cap">pattern</span>
          <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>
            midi · {+(pattern.lengthBeats / bpb).toFixed(2)} bars
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
        <div className="swatches" title="track color">
          {TRACK_PALETTE.map((c) => (
            <div key={c} className={c === color ? "on" : ""} style={{ background: c }} onClick={() => s.dispatch({ type: "setTrackColor", id: track.id, color: c })} />
          ))}
        </div>
        <div className="field">
          <span className="flabel">pattern</span>
          <select value={pattern.id} onChange={(e) => s.selectPattern(e.target.value)} style={{ minWidth: 150 }}>
            {model.patterns.filter((p) => p.trackId === track.id).map((p) => (
              <option key={p.id} value={p.id}>{p.name}</option>
            ))}
          </select>
        </div>
        <div className="field">
          <span className="flabel">length</span>
          <div className="row" style={{ gap: 3 }}>
            {[1, 2, 4, 8].map((bars) => (
              <button key={bars} className={`chip tiny${pattern.lengthBeats === bars * bpb ? " on" : ""}`} onClick={() => s.dispatch({ type: "setPatternLength", id: pattern.id, beats: bars * bpb })}>
                {bars}
              </button>
            ))}
          </div>
        </div>
        <div className="field">
          <span className="flabel">notes</span>
          <span className="val">{notes.length}</span>
        </div>
        <div className="row" style={{ gap: 4 }}>
          <button onClick={() => s.dispatch({ type: "addPattern", trackId: track.id })}>+ new</button>
          <button onClick={() => s.dispatch({ type: "duplicatePattern", id: pattern.id })}>dup</button>
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
          <span className="cap">notes</span>
          {selected.length > 0 && <span className="badge">{selected.length} selected</span>}
        </div>
        <RangeRow label="pitch" lo={pitchR ? (pitchR[0] - 24) / 72 : 0} hi={pitchR ? (pitchR[1] - 24) / 72 : 0} color={color} text={pitchR ? (track.isDrum ? `${pitchR[0]} – ${pitchR[1]}` : `${midiName(pitchR[0])} – ${midiName(pitchR[1])}`) : "—"} />
        <RangeRow label="velocity" lo={velR ? velR[0] / 127 : 0} hi={velR ? velR[1] / 127 : 0} color={color} text={velR ? `${velR[0]} – ${velR[1]}` : "—"} />
        <RangeRow label="length" lo={lenR ? Math.min(1, lenR[0] / 4) : 0} hi={lenR ? Math.min(1, lenR[1] / 4) : 0} color="#9a9aa4" text={lenR ? `${gridLabel(lenR[0])} – ${gridLabel(lenR[1])}` : "—"} />
        {selected.length > 0 && !track.isDrum && (
          <div className="chipsrow" title="set the length of every selected note">
            {GRID_OPTIONS.map(([label, v]) => (
              <button key={label} className={`chip tiny${selected.every((n) => n.lengthBeats === v) ? " on" : ""}`} onClick={() => s.transformSelection((ns, ids) => setLength(ns, ids, v), "set note length")}>
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

      {/* HARMONY */}
      <div className="section">
        <div className="head" style={{ gap: 6 }}>
          <span className="cap">harmony</span>
          <select value={root ?? ""} onChange={(e) => setKey(e.target.value === "" ? null : parseInt(e.target.value, 10), model.key.scale)} title="key root">
            <option value="">—</option>
            {NOTE_NAMES.map((n, i) => (
              <option key={n} value={i}>{n}</option>
            ))}
          </select>
          <select value={model.key.scale} onChange={(e) => setKey(root, e.target.value as ScaleType)} title="scale">
            <option value="major">Major</option>
            <option value="minor">Minor</option>
          </select>
        </div>
        {sectionKey && sectionKey.rootPitchClass !== null && (
          <div className="row" style={{ justifyContent: "space-between" }}>
            <span className="flabel">section key</span>
            <span className="mono" style={{ fontSize: 10, color: "var(--accent)" }}>{noteName(sectionKey.rootPitchClass)} {sectionKey.scale}</span>
          </div>
        )}
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="flabel">now</span>
          <span className="mono" style={{ fontSize: 11, color: chord ? "var(--text-1)" : "var(--text-5)" }}>{chord?.name ?? (root !== null ? "scale only" : "no guidance yet")}</span>
        </div>
        {root === null && chords.length === 0 && (
          <div className="mono" style={{ fontSize: 9.5, lineHeight: 1.45, color: "var(--text-4)" }}>
            no key or chords yet, so every note looks the same. pick a progression below, or just play a few notes and lock the key the KEY cell hears.
          </div>
        )}
        <div className="scalestrip" title="how each pitch class fits the chord under the playhead">
          {[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map((pc) => (
            <div key={pc} className={map[pc] ?? ""} style={pc === root ? { boxShadow: "inset 0 0 0 1px var(--accent)" } : undefined}>
              {noteName(pc).replace("#", "♯")}
            </div>
          ))}
        </div>
        <div className="chipsrow">
          <button className={`chip${s.highlightRows ? " on" : ""}`} onClick={() => s.toggleHighlight()}>tiers</button>
          <button className={`chip${s.showLandscape ? " on" : ""}`} onClick={() => s.toggleLandscape()}>map</button>
          <button className={`chip${s.hearChords ? " on" : ""}`} onClick={() => s.toggleHearChords()}>hear</button>
        </div>
        {!track.isDrum && notes.length > 0 && (
          <div className="chipsrow" title="select every note of this pattern by how it fits the chord it sits on">
            <span className="flabel" style={{ flex: "0 0 100%" }}>select by tier</span>
            {(["chordTone", "tension", "dissonance"] as const).map((t) => {
              const count = notes.filter((n) => tierMap(n.startBeat, chords, liveKey)[((n.pitch % 12) + 12) % 12] === t).length;
              return (
                <button
                  key={t}
                  className="chip tiny"
                  disabled={!count}
                  style={{ color: count ? `var(--tier-${t === "chordTone" ? "solid" : t})` : undefined }}
                  onClick={() => s.requestSelection(notes.filter((n) => tierMap(n.startBeat, chords, liveKey)[((n.pitch % 12) + 12) % 12] === t).map((n) => n.id))}
                >
                  {t === "chordTone" ? "chord tones" : t === "tension" ? "tensions" : "dissonant"} · {count}
                </button>
              );
            })}
          </div>
        )}
        <span className="flabel">progressions · in {root !== null ? noteName(root) : "C"} {model.key.scale}</span>
        <div className="chipsrow">
          {startersFor(model.key.scale).map((st) => {
            const full = progression(st.degrees, root ?? 0, model.key.scale, pattern.lengthBeats);
            // One pass of the degrees, named in this key: what the chip will actually write.
            const once = progression(st.degrees, root ?? 0, model.key.scale, st.degrees.length * 4).map((c) => c.name).join(" · ");
            const current =
              full.length === chords.length &&
              full.every((c, i) => c.startBeat === chords[i].startBeat && c.pitchClasses.join() === chords[i].pitchClasses.join());
            return (
              <button
                key={st.name}
                className={`chip starter${current ? " on" : ""}`}
                title={`${st.name} · ${once}`}
                onClick={() => commitChords(full)}
              >
                <span className="mood">{st.mood}</span>
                <span className="chords">{once}</span>
              </button>
            );
          })}
        </div>
        <div className="row" style={{ gap: 4 }}>
          <button
            onClick={() => {
              const end = chords.reduce((m, c) => Math.max(m, c.startBeat + c.lengthBeats), 0);
              if (end >= pattern.lengthBeats) return;
              const tonic = progression([0], root ?? 0, model.key.scale, 4, 4)[0];
              commitChords([...chords, { ...tonic, startBeat: end, lengthBeats: Math.min(4, pattern.lengthBeats - end) }]);
            }}
          >
            + chord
          </button>
          <button className="quiet" onClick={() => commitChords([])}>clear chords</button>
        </div>
      </div>

      {/* INSTRUMENT */}
      <div className="section">
        <div className="head">
          <span className="cap">instrument</span>
          <span className="mono" style={{ fontSize: 9, color: "var(--text-4b)" }}>{track.isDrum ? "procedural kit" : "poly · 12 voices"}</span>
        </div>
        <div className="devchip">
          <span className="led" />
          <span className="n">{track.isDrum ? "drum kit" : track.voice}</span>
          <span className="spacer" />
          <span className="meta">{track.isDrum ? DRUM_KIT.map((d) => d.name).join(" · ") : "waveform synth"}</span>
        </div>
        {!track.isDrum && (
          <div className="chipsrow" style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)" }} title="sounds by character: each sets the waveform, tone and reverb">
            {SOUND_PRESETS.map((p) => (
              <button key={p.name} className={`chip${soundPresetOf(track)?.name === p.name ? " on" : ""}`} style={{ textTransform: "none" }} onClick={() => void s.applySoundPreset(track.id, p)}>
                {p.name}
              </button>
            ))}
          </div>
        )}
        {!track.isDrum && <span className="flabel">waveform</span>}
        {!track.isDrum && (
          <div className="chipsrow" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)" }}>
            {VOICES.map((v) => (
              <button key={v} className={`chip${track.voice === v ? " on" : ""}`} onClick={() => s.dispatch({ type: "setTrackVoice", id: track.id, voice: v })}>
                {v}
              </button>
            ))}
          </div>
        )}
        <div className="knobrow">
          <Knob label="vol" value={VOL.to(track.volume)} display={`${dbText(track.volume)}`} color="#9a9aa4" defaultValue={VOL.to(1)} onChange={(p) => param("volume", VOL.from(p))} onCommit={commit} />
          <Knob label="pan" value={PAN.to(track.pan)} display={panText(track.pan)} color="#9a9aa4" defaultValue={0.5} onChange={(p) => param("pan", PAN.from(p))} onCommit={commit} />
          <Knob label="tone" value={TONE.to(track.tone)} display={hz(track.tone)} color="#3dc8ff" defaultValue={1} onChange={(p) => param("tone", TONE.from(p))} onCommit={commit} />
          <Knob label="reverb" value={track.reverbSend} display={pct(track.reverbSend)} color="#b48cff" defaultValue={0} onChange={(p) => param("reverbSend", p)} onCommit={commit} />
        </div>
      </div>

      <div className="spacer" />

      {/* TRACK OVERVIEW */}
      <div className="section" style={{ borderBottom: 0, borderTop: "1px solid var(--line-1)", gap: 6 }}>
        <span className="cap">song overview · {track.name}</span>
        <div className="overview" title="the song; the pattern you're editing is highlighted">
          {model.clips.filter((c) => trackPatternIds.has(c.patternId)).map((c) => {
            const p = model.patterns.find((x) => x.id === c.patternId);
            const current = c.patternId === pattern.id;
            return (
              <div
                key={c.id}
                style={{ left: `${(c.startBeat / Math.max(1, total)) * 100}%`, width: `calc(${(c.lengthBeats / Math.max(1, total)) * 100}% - 1px)`, background: current ? color : `${color}55`, boxShadow: current ? "0 0 0 1px #fff" : undefined, cursor: "pointer", opacity: c.muted ? 0.4 : 1 }}
                onClick={() => s.openPattern(c.patternId, c.startBeat)}
                title={p?.name}
              />
            );
          })}
        </div>
        <div className="overview-scale">
          <span>1</span>
          <span>{Math.max(1, Math.round(total / bpb / 2) + 1)}</span>
          <span>{Math.max(1, Math.round(total / bpb) + 1)}</span>
        </div>
      </div>
    </aside>
  );
}

function TransformGrid() {
  const s = useStore();
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const key = s.snapshot!.model.key;
  const chords = usePatternChords();
  if (!pattern || !track) return null;
  const notes = pattern.notes;
  const ids = new Set(s.selectedNoteIds);
  const grid = s.noteLength;
  const tierOf = (n: { pitch: number; startBeat: number }) => tierMap(n.startBeat, chords, key)[((n.pitch % 12) + 12) % 12];
  const apply = (next: typeof notes, label: string) => void s.dispatch({ type: "setNotes", patternId: pattern.id, notes: next }, false, label);
  const tools: [string, string, () => void][] = [
    ["quantize", "snap starts and lengths to the grid", () => apply(quantizeNotes(notes, ids, grid), "quantize")],
    ["humanize", "loosen timing and velocity; chord tones stay tighter", () => apply(humanizeNotes(notes, ids, { timing: grid * 0.15, velocity: 12 }, tierOf), "humanize")],
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
    <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 3 }}>
      {tools.map(([label, title, fn]) => (
        <button key={label} title={title} onClick={fn} disabled={notes.length === 0} style={{ background: "var(--bg-2)", color: "var(--text-2)" }}>
          {label}
        </button>
      ))}
    </div>
  );
}

/** For one selected dissonant note: say why, offer the two nearest chord tones, or keep it on purpose. */
function ResolveKeep({ note }: { note: { id: string; pitch: number; startBeat: number; intentional?: boolean } }) {
  const s = useStore();
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const key = s.snapshot!.model.key;
  const chords = usePatternChords();
  if (!pattern || !track) return null;
  const tiers = tierMap(note.startBeat, chords, key);
  const t = tiers[((note.pitch % 12) + 12) % 12];
  const why = explainNote(note.pitch, chords, key, note.startBeat);
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
