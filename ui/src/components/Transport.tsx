import { useEffect, useRef, useState } from "react";
import { peakDb, selectedPattern, useStore } from "../store";
import { DEFAULT_SWING_GRID_TICKS, tempoMap, TICKS_PER_BEAT, ticksToBeats, TIME_SIGNATURES } from "../types";
import { detectKey, keyName } from "../theory";
import { detectedChroma } from "../detection";
import { HMeter } from "./Meter";
import { keys } from "../platform";

/** What the chip beside REC cycles through: the grid a take snaps to, and "free" — as played.
 *  Exported because the REC page owns the same chip for audio takes. */
export const RECORD_GRIDS: { ticks: number | null; label: string }[] = [
  { ticks: TICKS_PER_BEAT, label: "1/4" },
  { ticks: TICKS_PER_BEAT / 2, label: "1/8" },
  { ticks: TICKS_PER_BEAT / 4, label: "1/16" },
  { ticks: TICKS_PER_BEAT / 8, label: "1/32" },
  { ticks: null, label: "free" },
];

function pad(n: number, w: number) {
  return String(n).padStart(w, "0");
}

export function Transport() {
  const s = useStore();
  const model = s.snapshot!.model;
  const [editingBpm, setEditingBpm] = useState(false);
  const [bpmText, setBpmText] = useState(model.tempo.toFixed(3));
  useEffect(() => setBpmText(model.tempo.toFixed(3)), [model.tempo]);
  const bpmDrag = useRef<{ y0: number; bpm0: number; moved: boolean } | null>(null);
  const setTempo = (bpm: number, transient = false) => void s.dispatch({ type: "setTempo", bpm: Math.min(300, Math.max(20, bpm)) }, transient, "set tempo");
  const swing = model.swing ?? 50;
  const swingGridTicks = model.swingGridTicks ?? DEFAULT_SWING_GRID_TICKS;
  // A punch range on the song blocks looping; the loop switch shows it rather than lying.
  const punchArmed = !!model.punchRegion;

  // Position: BBB.B.SS (bar . beat . sixteenth) and MM:SS.mmm
  const tick = s.playhead;
  const bticks = 4 * TICKS_PER_BEAT;
  const bar = Math.floor(tick / bticks) + 1;
  const bib = Math.floor((tick % bticks) / TICKS_PER_BEAT) + 1;
  const six = Math.floor((tick % TICKS_PER_BEAT) / (TICKS_PER_BEAT / 4)) + 1;
  // Only PATTERN runs inside a pattern; SETUP, SONG and the rec page run along the song.
  const map = s.screen !== "pattern" ? tempoMap(model.tempo, model.tempoPoints) : null;
  const secs = map && !map.isConstant ? map.secondsAt(tick) : (ticksToBeats(tick) * 60) / model.tempo;
  const liveBpm = map && !map.isConstant ? map.bpmAt(tick) : null;
  const mm = Math.floor(secs / 60);
  const ss = secs - mm * 60;

  // Key: locked value, or a suggestion from the notes of every melodic pattern (each pattern
  // belongs to one track, so drum patterns are left out by their track; muted notes too) plus
  // the chroma DI detection heard in strummed takes, which wrote no notes of its own.
  const drumIds = new Set(model.tracks.filter((t) => t.isDrum).map((t) => t.id));
  const pcs = model.patterns.filter((p) => !drumIds.has(p.trackId)).flatMap((p) => p.notes.filter((n) => !n.muted).map((n) => n.pitch));
  // Record target: the selected pattern, on its track.
  const recPattern = selectedPattern(s);
  const recInto = recPattern ? `${model.tracks.find((t) => t.id === recPattern.trackId)?.name ?? "?"} › ${recPattern.name}` : null;
  const detected = detectKey(pcs, detectedChroma(model));
  const suggestion = !model.key.isLocked && detected.isConfident ? detected.candidates[0] : null;

  const commitBpm = () => {
    const v = parseFloat(bpmText);
    if (Number.isFinite(v)) setTempo(v);
    setEditingBpm(false);
  };

  const [pl, pr] = s.masterPeak;
  const peak = Math.max(pl, pr);
  // Latched by the engine the moment the mix went over the ceiling, so a single over during a
  // take is still there when the take ends. Only the click below lowers it.
  const clipped = s.audio?.masterOver ?? false;
  // On the rec page the record button captures audio instead of arming MIDI.
  const audioRecord = s.screen === "rec" && s.armedAudioTrackId !== null;
  // Which grid the chip shows; a value off the list (an older session's) reads as "free".
  const foundGrid = RECORD_GRIDS.findIndex((g) => g.ticks === s.recordQuantize);
  const recGrid = foundGrid < 0 ? RECORD_GRIDS.length - 1 : foundGrid;

  return (
    <div className="transport">
      <div className="tgroup">
        <span className="cap">view</span>
        <div className="seg">
          <div className={s.screen === "setup" ? "on" : ""} onClick={() => s.setScreen("setup")} title="setup: chords, sections, tempo, song length">setup</div>
          <div className={s.screen === "pattern" ? "on" : ""} onClick={() => s.setScreen("pattern")} title="pattern: the piano roll (Tab cycles)">pat</div>
          <div className={s.screen === "song" ? "on" : ""} onClick={() => s.setScreen("song")} title="song: the arrangement (Tab cycles)">song</div>
          <div className={s.screen === "rec" ? "on" : ""} onClick={() => s.setScreen("rec")} title="rec: input, tuner, takes and the rig">rec</div>
          <div className={s.screen === "sound" ? "on" : ""} onClick={() => s.setScreen("sound")} title="sound: design synth and drum sounds, saved for every project">sound</div>
        </div>
      </div>
      <div className="tdiv" />
      <div className="tbtns">
        <button className="tbtn" onClick={() => s.rewind()} title="return to start (Enter)">
          <svg width="14" height="12" viewBox="0 0 14 12">
            <rect x="0" y="0" width="2" height="12" fill="#9a9aa4" />
            <polygon points="13,0 13,12 3,6" fill="#9a9aa4" />
          </svg>
        </button>
        <button className="tbtn" onClick={() => s.stop()} title="stop (space)">
          <div style={{ width: 11, height: 11, background: "#9a9aa4" }} />
        </button>
        <button className={`tbtn play${s.playing ? "" : " off"}`} onClick={() => s.play()} title="play (space)">
          <svg width="12" height="14" viewBox="0 0 12 14">
            <polygon points="0,0 12,7 0,14" fill={s.playing ? "#0b0b0d" : "#9a9aa4"} />
          </svg>
        </button>
        {audioRecord ? (
          <button
            className="tbtn"
            style={s.recordingAudio ? { background: "var(--rec-bg)", borderColor: "var(--rec-border)" } : undefined}
            onClick={() => void (s.recordingAudio ? s.stopRecording() : s.startRecording())}
            title="record (R): capture the input onto the armed audio track"
          >
            <div
              style={{
                width: 11,
                height: 11,
                borderRadius: "50%",
                background: s.recordingAudio ? "#ff3b30" : "#5f5f68",
                boxShadow: s.recordingAudio ? "0 0 8px #ff3b30" : "none",
              }}
            />
          </button>
        ) : (
          <>
            <button
              className="tbtn"
              style={s.armed ? { background: "var(--rec-bg)", borderColor: "var(--rec-border)" } : undefined}
              onClick={() => s.toggleRecord()}
              disabled={!s.armed && !recInto}
              title={recInto ? `record arm (R): notes you play land in ${recInto} while playing` : "record arm (R): pick a pattern to record into first"}
            >
              <div style={{ width: 11, height: 11, borderRadius: "50%", background: s.armed ? "#ff3b30" : "#5f5f68", boxShadow: s.armed ? "0 0 8px #ff3b30" : "none" }} />
            </button>
            <button
              className="chip tiny"
              onClick={() => s.setRecordQuantize(RECORD_GRIDS[(recGrid + 1) % RECORD_GRIDS.length].ticks)}
              title={
                RECORD_GRIDS[recGrid].ticks === null
                  ? "recording grid: none — the take is kept exactly as played · click to change"
                  : `recording grid: notes snap to ${RECORD_GRIDS[recGrid].label} · click to change`
              }
            >
              {RECORD_GRIDS[recGrid].label}
            </button>
          </>
        )}
        <button
          className="tbtn"
          // An armed punch blocks looping outright, so the switch says so instead of pretending
          // to work: the engine would ignore it anyway.
          disabled={punchArmed}
          style={s.looping && !punchArmed ? { background: "var(--accent-bg)", borderColor: "var(--accent-border)" } : undefined}
          onClick={() => s.toggleLooping()}
          title={
            punchArmed
              ? "loop is blocked while punch is armed — clear the punch range in SONG"
              : "loop (L) · drag the ruler's top strip to set a region · right-click it to clear"
          }
        >
          <svg width="16" height="12" viewBox="0 0 16 12">
            <path d="M3 4 H12 L10 2 M13 8 H4 L6 10" stroke={s.looping && !punchArmed ? "#b48cff" : "#9a9aa4"} strokeWidth="1.6" fill="none" />
          </svg>
        </button>
        <button
          className="tbtn"
          style={s.settings.metronome.on ? { background: "var(--accent-bg)", borderColor: "var(--accent-border)" } : undefined}
          onClick={() => s.toggleMetronome()}
          title={`metronome (M) · count-in ${s.settings.metronome.countInBars} bar${s.settings.metronome.countInBars === 1 ? "" : "s"} when recording · volume and pre-roll in Settings → audio`}
        >
          <svg width="12" height="14" viewBox="0 0 12 14">
            <path d="M3 13 L5 1 H7 L9 13 Z" fill="none" stroke={s.settings.metronome.on ? "#b48cff" : "#9a9aa4"} strokeWidth="1.4" />
            <path d="M6 10 L9.5 3" stroke={s.settings.metronome.on ? "#b48cff" : "#9a9aa4"} strokeWidth="1.4" />
          </svg>
        </button>
      </div>
      <div className="lcd">
        <div className="tgroup">
          {s.countIn > 0 ? (
            <span className="big" style={{ color: "var(--rec-text)" }} title="count-in">
              {Math.ceil(s.countIn)}
            </span>
          ) : (
            <span className="big">
              {pad(bar, 3)}.{bib}.{pad(six, 2)}
            </span>
          )}
          <span className="micro">{s.countIn > 0 ? "count-in" : "bar · beat · 16th"}</span>
        </div>
        <div style={{ width: 1, height: 28, background: "var(--line-0)" }} />
        <div className="tgroup">
          <span className="time">
            {pad(mm, 2)}:{ss.toFixed(3).padStart(6, "0")}
          </span>
          <span className="micro">min:sec.ms</span>
        </div>
      </div>
      <div className="lcd" style={{ padding: "0 8px", gap: 8 }}>
        <div className="tgroup">
          {editingBpm ? (
            <input
              type="text"
              className="mono"
              autoFocus
              value={bpmText}
              onChange={(e) => setBpmText(e.target.value)}
              onBlur={commitBpm}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitBpm();
                if (e.key === "Escape") setEditingBpm(false);
                if (e.key === "ArrowUp" || e.key === "ArrowDown") {
                  e.preventDefault();
                  const base = parseFloat(bpmText);
                  const d = (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 10 : 1);
                  const next = Math.min(300, Math.max(20, (Number.isFinite(base) ? Math.round(base) : model.tempo) + d));
                  setBpmText(next.toFixed(3));
                  setTempo(next);
                }
              }}
            />
          ) : (
            <span
              className="bpm"
              onPointerDown={(e) => {
                if (e.button !== 0) return;
                bpmDrag.current = { y0: e.clientY, bpm0: model.tempo, moved: false };
                (e.target as HTMLElement).setPointerCapture(e.pointerId);
              }}
              onPointerMove={(e) => {
                const d = bpmDrag.current;
                if (!d) return;
                const dy = d.y0 - e.clientY;
                if (!d.moved && Math.abs(dy) < 3) return;
                d.moved = true;
                // 1 bpm per 2 px; ⇧ for fine 0.1 bpm steps.
                const step = e.shiftKey ? 0.1 : 1;
                setTempo(Math.round((d.bpm0 + (dy / 2) * step) / step) * step, true);
              }}
              onPointerUp={() => {
                const d = bpmDrag.current;
                bpmDrag.current = null;
                if (!d) return;
                if (d.moved) void s.commitGesture();
                else (setBpmText(model.tempo.toFixed(3)), setEditingBpm(true));
              }}
              onDoubleClick={() => setTempo(s.settings.editing.defaultTempo)}
              onWheel={(e) => {
                e.preventDefault();
                setTempo(Math.round(model.tempo) + (e.deltaY < 0 ? 1 : -1) * (e.shiftKey ? 10 : 1));
              }}
              title={keys("click to type (↑↓ ±1, ⇧ ±10) · drag up/down · scroll to nudge · double-click resets")}
            >
              {model.tempo.toFixed(3)}
            </span>
          )}
          <span className="micro">{liveBpm !== null && Math.abs(liveBpm - model.tempo) > 0.05 ? `bpm · now ${liveBpm.toFixed(1)}` : "bpm"}</span>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
          <button className="chip tiny" onClick={() => s.tapTempo()} title="tap tempo (averages the last 8 taps)">tap</button>
          <div className="row" style={{ gap: 2 }}>
            <button className="chip tiny" onClick={() => setTempo(model.tempo / 2)} title="half time">÷2</button>
            <button className="chip tiny" onClick={() => setTempo(model.tempo * 2)} title="double time">×2</button>
          </div>
        </div>
        <div className="tgroup" title="swing: delays every second eighth (or sixteenth). 50 = straight · 67 = triplet feel · 75 = hard shuffle">
          <span className="row" style={{ gap: 3 }}>
            <span
              className="mono"
              style={{ color: swing > 50 ? "var(--accent)" : "var(--text-4)", cursor: "ns-resize", minWidth: 22, textAlign: "right" }}
              onWheel={(e) => {
                e.preventDefault();
                void s.dispatch({ type: "setSwing", swing: Math.round(swing) + (e.deltaY < 0 ? 1 : -1), gridTicks: swingGridTicks }, false, "set swing");
              }}
              onDoubleClick={() => s.dispatch({ type: "setSwing", swing: 50, gridTicks: swingGridTicks })}
            >
              {Math.round(swing)}
            </span>
            <input
              type="range"
              min={50}
              max={75}
              step={1}
              value={Math.round(swing)}
              style={{ width: 44 }}
              onChange={(e) => void s.dispatch({ type: "setSwing", swing: Number(e.target.value), gridTicks: swingGridTicks }, true, "set swing")}
              onPointerUp={() => void s.commitGesture()}
            />
            <select value={swingGridTicks} onChange={(e) => s.dispatch({ type: "setSwing", swing, gridTicks: Number(e.target.value) })} style={{ height: 16, padding: "0 12px 0 3px", fontSize: 9 }}>
              <option value={480}>8th</option>
              <option value={240}>16th</option>
            </select>
          </span>
          <span className="micro">swing</span>
        </div>
      </div>
      <div className="grid22">
        <span className="k">sig</span>
        <span className="v">
          <select
            value={`${model.timeSignature?.numerator ?? 4}/${model.timeSignature?.denominator ?? 4}`}
            onChange={(e) => {
              const [n, d] = e.target.value.split("/").map(Number);
              void s.dispatch({ type: "setTimeSignature", numerator: n, denominator: d });
            }}
            style={{ height: 18, padding: "0 14px 0 4px" }}
          >
            {TIME_SIGNATURES.map((t) => (
              <option key={`${t.numerator}/${t.denominator}`} value={`${t.numerator}/${t.denominator}`}>
                {t.numerator} / {t.denominator}
              </option>
            ))}
          </select>
        </span>
        <span className="k">key</span>
        <span className="v">
          {model.key.isLocked && model.key.rootPitchClass !== null ? (
            <>
              <span>{keyName(model.key.rootPitchClass, model.key.scale)}</span>
              <button className="ico" title="unlock key" onClick={() => s.dispatch({ type: "setKey", key: { rootPitchClass: null, scale: "major", isLocked: false } })}>×</button>
            </>
          ) : suggestion ? (
            <button
              className="chip tiny on"
              title="the notes you placed look like this key — lock it?"
              onClick={() =>
                s.dispatch({ type: "setKey", key: { rootPitchClass: suggestion.rootPitchClass, scale: suggestion.scale, isLocked: true } })
              }
            >
              {keyName(suggestion.rootPitchClass, suggestion.scale)}? lock
            </button>
          ) : (
            <span style={{ color: "var(--text-5)" }}>—</span>
          )}
        </span>
      </div>
      <span className="spacer" />
      <div className="row" style={{ gap: 3 }}>
        <div className="seg" title="play the selected track from your computer keyboard · tier: home row = chord tones, top row = tensions, following the playhead · chrom: Z/Q rows chromatic · −/+ octave">
          <div className={s.liveKeyboard === "off" ? "on" : ""} onClick={() => s.setLiveKeyboard("off")}>keys</div>
          <div className={s.liveKeyboard === "tier" ? "on" : ""} onClick={() => s.setLiveKeyboard("tier")}>tier</div>
          <div className={s.liveKeyboard === "chromatic" ? "on" : ""} onClick={() => s.setLiveKeyboard("chromatic")}>chrom</div>
        </div>
        <MidiChip />
      </div>
      <div className="mastermeter">
        <div className="hdr">
          <span>master</span>
          <span className="row" style={{ gap: 4 }}>
            {clipped && (
              <button
                className="chip tiny on"
                title="the mix asked for more than the master ceiling — the limiter held it, but the gain staging is hot. click to reset"
                onClick={() => s.clearClip()}
              >
                CLIP
              </button>
            )}
            <span className="mono" style={{ color: clipped ? "var(--rec)" : "var(--text-6)" }}>
              {peakDb(peak).toFixed(1)}
            </span>
          </span>
        </div>
        <HMeter peak={pl} />
        <HMeter peak={pr} />
        <div className="scale">
          <span>-48</span>
          <span>-24</span>
          <span>-12</span>
          <span>-6</span>
          <span>0</span>
        </div>
      </div>
    </div>
  );
}

function MidiChip() {
  const s = useStore();
  const midi = s.midi;
  const active = performance.now() - s.midiActivityAt < 150;
  if (!midi) return null;
  const isMock = !s.audio?.running && midi.inputs.length === 0 && !midi.open;
  return (
    <div
      className="opt"
      style={{ borderColor: midi.open ? "var(--rec-border)" : "var(--line-3)", gap: 6 }}
      title={isMock ? "MIDI input needs the desktop app" : "MIDI input device"}
    >
      <span className="dot" style={{ width: 6, height: 6, borderRadius: "50%", background: midi.open ? (active ? "#ff6a5e" : "#ff3b30") : "#4a4a52", display: "inline-block", boxShadow: active ? "0 0 6px #ff3b30" : "none" }} />
      <span className="k" style={{ color: midi.open ? "var(--rec-text)" : undefined }}>midi in</span>
      <select
        value={midi.open ?? ""}
        onChange={(e) => (e.target.value ? void s.openMidi(e.target.value) : void s.closeMidi())}
        onFocus={() => void s.refreshMidi()}
        style={{ height: 16, padding: "0 14px 0 4px", border: 0, background: "transparent", maxWidth: 140 }}
      >
        <option value="">off</option>
        {midi.inputs.map((name) => (
          <option key={name} value={name}>{name}</option>
        ))}
        {midi.open && !midi.inputs.includes(midi.open) && <option value={midi.open}>{midi.open}</option>}
      </select>
    </div>
  );
}
