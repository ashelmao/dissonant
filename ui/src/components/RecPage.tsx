import { useEffect } from "react";
import { AmpRack } from "./AmpRack";
import { WaveformLane } from "./WaveformLane";
import { RECORD_GRIDS } from "./Transport";
import { dbText, meterPos, recTrack, useStore } from "../store";
import { usePanelSize } from "../panels";
import { Splitter } from "./Splitter";
import { ticksToBeats } from "../types";
import { alignmentText, IN_TUNE_CENTS, noteHz, TUNINGS, tunerAdvice, tunerDisplay, tuningById } from "../tuner";

/** `m:ss` for a clip length. */
function clock(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** How often the tuner strip asks the engine what it hears. The detector itself runs every
 *  ~43 ms, so anything faster just re-reads the same number. */
const TUNER_POLL_MS = 60;

/**
 * The input meter: pre-rig, with a peak-hold marker and a clip latch you have to acknowledge.
 *
 * Both come from the engine rather than from these samples, because the meter is read a few
 * times a second and the peak that decides your gain is the one that happened in between.
 */
function InputMeter() {
  const peak = useStore((s) => s.inputPeak);
  const hold = useStore((s) => s.audio?.inputPeakHold ?? 0);
  const over = useStore((s) => s.audio?.inputOver ?? false);
  const clear = useStore((s) => s.clearInputPeak);
  // `meterPos` is the curve every other meter uses, so "half way" means the same thing here.
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 6, width: "100%" }}>
      <div className="hbar" style={{ position: "relative", flex: 1 }} title="input level, before the rig">
        <div style={{ width: `${(1 - meterPos(peak)) * 100}%` }} />
        {hold > 0 && (
          <div
            style={{
              position: "absolute",
              left: `${meterPos(hold) * 100}%`,
              top: -1,
              bottom: -1,
              width: 1,
              background: "var(--text-3)",
            }}
          />
        )}
      </div>
      <span
        className="mono tiny"
        onClick={() => void clear()}
        style={{ cursor: "pointer", minWidth: 58, textAlign: "right", color: over ? "var(--rec-text)" : "var(--text-6)" }}
        title="peak hold, pre-rig · click to reset it and the clip indicator"
      >
        {over ? "CLIP" : hold > 0 ? `${dbText(hold)} dB` : "—"}
      </span>
    </div>
  );
}

/**
 * The tuner strip.
 *
 * It lives here rather than in the transport on purpose: tuning is something you do while
 * setting up to record, and the engine is silent the whole time it is open — which would be an
 * alarming thing for a chip in the transport to do.
 */
function TunerStrip() {
  const hz = useStore((s) => s.tunerHz);
  const clarity = useStore((s) => s.tunerClarity);
  const poll = useStore((s) => s.pollTuner);
  const settings = useStore((s) => s.settings);
  const updateSettings = useStore((s) => s.updateSettings);

  useEffect(() => {
    const id = window.setInterval(() => void poll(), TUNER_POLL_MS);
    return () => window.clearInterval(id);
  }, [poll]);

  const a4 = settings.tuner.a4Hz;
  const tuning = tuningById(settings.tuner.tuning);
  // A chord or a palm mute reads as a pitch the detector is not sure about; showing it anyway
  // would have the needle swinging at notes nobody played.
  const reading = clarity >= 0.6 ? tunerDisplay(hz, tuning, a4) : null;
  const cents = reading?.cents ?? 0;
  // ±50 cents across the bar: past that you are on another note and the label says so.
  const needle = 50 + Math.max(-50, Math.min(50, cents));
  const color = !reading ? "var(--text-6)" : reading.inTune ? "var(--ok)" : "var(--warn)";

  return (
    <div className="section" style={{ borderBottom: "1px solid var(--line-0)" }}>
      <div className="shead">
        <span>tuner</span>
        <span className="meta mono">output muted</span>
      </div>
      <div className="field">
        <span className="cap">tuning</span>
        <select value={tuning.id} onChange={(e) => void updateSettings("tuner", { tuning: e.target.value })}>
          {TUNINGS.map((t) => (
            <option key={t.id} value={t.id}>
              {t.name}
            </option>
          ))}
        </select>
        <span className="cap">A4</span>
        <input
          type="number"
          className="mono"
          min={390}
          max={460}
          step={1}
          value={Math.round(a4)}
          style={{ width: 56 }}
          onChange={(e) => {
            const v = Number(e.target.value);
            if (Number.isFinite(v)) void updateSettings("tuner", { a4Hz: Math.min(460, Math.max(390, v)) });
          }}
          title="what A4 is worth, in Hz"
        />
      </div>
      <div style={{ display: "flex", alignItems: "baseline", gap: 10, padding: "4px 8px" }}>
        <span style={{ fontSize: 22, fontWeight: 600, color, minWidth: 92 }}>{reading?.label ?? "—"}</span>
        <span className="mono tiny" style={{ color: "var(--text-3)" }}>
          {reading ? `${reading.note} · ${reading.targetHz.toFixed(2)} Hz` : "listening"}
        </span>
        <span className="spacer" style={{ flex: 1 }} />
        <span className="mono" style={{ color }}>
          {reading ? `${cents > 0 ? "+" : ""}${cents.toFixed(1)}¢` : ""}
        </span>
        <span className="mono tiny" style={{ color: "var(--text-6)", minWidth: 88, textAlign: "right" }}>
          {hz ? `${hz.toFixed(2)} Hz` : ""}
        </span>
      </div>
      <div style={{ position: "relative", height: 10, margin: "0 8px 6px", background: "var(--meter-h)" }}>
        {/* The in-tune window, drawn so "close enough" is a place rather than a number. */}
        <div
          style={{
            position: "absolute",
            left: `${50 - IN_TUNE_CENTS}%`,
            width: `${IN_TUNE_CENTS * 2}%`,
            top: 0,
            bottom: 0,
            background: "var(--ok-bg, rgba(61,255,176,0.18))",
          }}
        />
        {reading && (
          <div style={{ position: "absolute", left: `${needle}%`, top: -2, bottom: -2, width: 2, background: color }} />
        )}
      </div>
      <div className="field" style={{ gap: 6, flexWrap: "wrap" }}>
        {tuning.strings.map((s, i) => (
          <span
            key={`${s.name}-${i}`}
            className={`chip tiny${reading?.stringIndex === i ? " on" : ""}`}
            title={`${s.name} · ${noteHz(s.midi, a4).toFixed(2)} Hz`}
          >
            {s.name}
          </span>
        ))}
      </div>
      <div className="mono tiny" style={{ padding: "0 8px 6px", color: "var(--text-6)" }}>
        {tunerAdvice(reading)}
      </div>
    </div>
  );
}

/**
 * The REC page: everything you touch between picking up the guitar and having a take on the
 * grid — input device and channel, arm, monitor, the record grid, the tuner, the level, and
 * what the last take's latency correction came to.
 *
 * It follows the **armed** track, not the selected one, because the arm is what decides where
 * a take lands. A track with no rig (`amp: null` — a vocal take) shows the input strip and
 * nothing else; a dead rack would only invite someone to turn knobs that do nothing.
 */
export function RecPage() {
  const model = useStore((s) => s.snapshot!.model);

  const sideW = usePanelSize("rec");
  const selectTrack = useStore((s) => s.selectTrack);
  const addAudioTrack = useStore((s) => s.addAudioTrack);
  const audio = useStore((s) => s.audio);
  const inputDevices = useStore((s) => s.inputDevices);
  const armedId = useStore((s) => s.armedAudioTrackId);
  const setArm = useStore((s) => s.setArm);
  const monitor = useStore((s) => s.monitor);
  const toggleMonitor = useStore((s) => s.toggleMonitor);
  const settings = useStore((s) => s.settings);
  const updateSettings = useStore((s) => s.updateSettings);
  const setInputTrim = useStore((s) => s.setInputTrim);
  const importAudioFile = useStore((s) => s.importAudioFile);
  const importing = useStore((s) => s.importing);
  const selectedAudioClipId = useStore((s) => s.selectedAudioClipId);
  const selectAudioClip = useStore((s) => s.selectAudioClip);
  const dispatch = useStore((s) => s.dispatch);
  const refreshInputDevices = useStore((s) => s.refreshInputDevices);
  const recordQuantize = useStore((s) => s.recordQuantize);
  const setRecordQuantize = useStore((s) => s.setRecordQuantize);
  const tunerOpen = useStore((s) => s.tunerOpen);
  const toggleTuner = useStore((s) => s.toggleTuner);

  useEffect(() => {
    void refreshInputDevices();
  }, [refreshInputDevices]);

  const audioTracks = model.tracks.filter((t) => t.isAudio);
  // The page follows the arm; `recTrack` falls back to the selection and then to the first
  // audio track, so an unarmed session still has something to look at.
  const track = useStore(recTrack);
  const clips = (model.audioClips ?? []).filter((c) => c.trackId === track?.id);
  const sourceName = (id: string) => (model.audioSources ?? []).find((s) => s.id === id)?.name ?? "missing";
  const device = inputDevices.find((d) => d.name === (settings.audio.inputDevice ?? audio?.inputDevice));
  const channels = device?.channels ?? 1;
  // The trim is keyed by the device that actually opened, not by the setting: "system default"
  // is a wish, and the latency belongs to whichever box answered it.
  const openDevice = audio?.inputDevice ?? null;
  const trim = openDevice ? (settings.audio.inputTrims[openDevice] ?? 0) : 0;
  const gridIndex = (() => {
    const found = RECORD_GRIDS.findIndex((g) => g.ticks === recordQuantize);
    return found < 0 ? RECORD_GRIDS.length - 1 : found;
  })();
  const lastAlign = alignmentText(audio?.lastTakeAlign);

  return (
    <>
      <div className="side guitar" style={{ width: sideW }}>
        <div className="section">
          <div className="shead">
            <span>audio tracks</span>
            <span className="act" onClick={() => void addAudioTrack()} title="add an audio track">
              ＋
            </span>
          </div>
          {audioTracks.length === 0 ? (
            <div className="empty mono">no audio tracks yet — press ＋</div>
          ) : (
            audioTracks.map((t) => (
              <div
                key={t.id}
                className={`row${t.id === track?.id ? " sel" : ""}`}
                onClick={() => selectTrack(t.id)}
                title={t.amp?.enabled ? `rig: ${t.amp.amp.model}` : t.amp ? "rig bypassed" : "no rig on this track"}
              >
                <span className={`led${t.id === armedId ? " rec" : ""}`} />
                <span className="name">{t.name}</span>
                <span className="meta mono">{(model.audioClips ?? []).filter((c) => c.trackId === t.id).length}</span>
              </div>
            ))
          )}
        </div>

        <div className="section">
          <div className="shead">
            <span>input</span>
            <span className="act" onClick={() => void refreshInputDevices()} title="look for devices again">
              ↻
            </span>
          </div>
          <div className="field">
            <select
              value={settings.audio.inputDevice ?? ""}
              onChange={(e) => void updateSettings("audio", { inputDevice: e.target.value || null })}
              title="which device the guitar is plugged into"
            >
              <option value="">system default</option>
              {inputDevices.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.name}
                  {d.isDefault ? " (default)" : ""}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <span className="cap">channel</span>
            <select
              value={settings.audio.inputChannel}
              onChange={(e) => void updateSettings("audio", { inputChannel: Number(e.target.value) })}
            >
              {Array.from({ length: Math.max(1, channels) }, (_, i) => (
                <option key={i} value={i}>
                  {i + 1}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <InputMeter />
          </div>
          <div className="field">
            <button
              className={`armbtn${armedId && armedId === track?.id ? " on" : ""}`}
              disabled={!track}
              onClick={() => void setArm(armedId === track?.id ? null : (track?.id ?? null))}
              title="send the input to this track — the REC page follows whatever is armed"
            >
              {armedId === track?.id ? "armed" : "arm"}
            </button>
            <span className={`chip${monitor ? " on" : ""}`} onClick={() => void toggleMonitor()} title="hear the input through the rig">
              monitor
            </span>
            <span
              className="chip tiny"
              onClick={() => setRecordQuantize(RECORD_GRIDS[(gridIndex + 1) % RECORD_GRIDS.length].ticks)}
              title={
                RECORD_GRIDS[gridIndex].ticks === null
                  ? "recording grid: none — the take is kept exactly as played · click to change"
                  : `recording grid: takes snap to ${RECORD_GRIDS[gridIndex].label} · click to change`
              }
            >
              {RECORD_GRIDS[gridIndex].label}
            </span>
          </div>
          <div className="field">
            <span className="cap">trim</span>
            <input
              type="number"
              className="mono"
              min={-50}
              max={200}
              step={1}
              value={Math.round(trim)}
              disabled={!openDevice}
              style={{ width: 56 }}
              onChange={(e) => {
                const v = Number(e.target.value);
                if (openDevice && Number.isFinite(v)) void setInputTrim(openDevice, Math.min(200, Math.max(-50, v)));
              }}
              title="extra latency this interface adds that nothing can measure — takes move back by it, so raise it until a recorded click lands on the beat"
            />
            <span className="k mono">ms</span>
          </div>
          <div className="mono tiny">
            {audio?.inputError ? (
              <span style={{ color: "var(--rec-text)" }}>{audio.inputError}</span>
            ) : (
              <>
                round-trip {(audio?.roundTripMs ?? 0).toFixed(1)} ms · takes move −{(audio?.alignmentMs ?? 0).toFixed(1)} ms
                {audio?.inputUnderruns ? ` · ${audio.inputUnderruns} drops` : ""}
              </>
            )}
          </div>
          {lastAlign && (
            <div className="mono tiny" style={{ color: "var(--text-3)" }} title="the last take was moved back by this much so it landed where it was played">
              last take: {lastAlign}
            </div>
          )}
        </div>

        <div className="section">
          <div className="shead">
            <span>clips</span>
            <span className="meta mono">{clips.length}</span>
          </div>
          {clips.length === 0 ? (
            <div className="empty mono">record, drop a file, or import audio</div>
          ) : (
            clips.map((c) => (
              <div
                key={c.id}
                className={`takerow${c.id === selectedAudioClipId ? " sel" : ""}`}
                onClick={() => selectAudioClip(c.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  void dispatch({ type: "removeAudioClip", id: c.id });
                }}
                title="click: select · right-click: remove"
              >
                <span className="name">{sourceName(c.sourceId)}</span>
                <span className="meta mono">
                  {clock(c.lengthSeconds)} · bar {Math.floor(ticksToBeats(c.startTick) / 4) + 1}
                </span>
              </div>
            ))
          )}
        </div>
      </div>
      <Splitter panel="rec" label="the audio track list" />

      <div className="editor">
        <div className="toolbar">
          <span className="cap">{track?.name ?? "no audio track"}</span>
          {armedId === track?.id && track && <span className="chip on">armed</span>}
          <span
            className={`chip${tunerOpen ? " on" : ""}`}
            onClick={() => void toggleTuner()}
            title="tuner: the engine goes silent while it is open, so you tune the strings and not the amp"
          >
            tuner
          </span>
          {track && (
            <>
              {track.amp && (
                <span
                  className={`chip${track.amp.enabled ? " on" : ""}`}
                  onClick={() => void useStore.getState().setAmp(track.id, { enabled: !track.amp?.enabled })}
                  title="bypass the whole rig"
                >
                  rig
                </span>
              )}
              <span className="chip" onClick={() => void importAudioFile(track.id)}>
                import audio…
              </span>
            </>
          )}
          <span className="spacer" />
          {importing && (
            <span className="mono tiny">
              importing {importing.name} — {Math.round(importing.fraction * 100)}%
            </span>
          )}
          <span
            className="chip"
            title={"drag a clip to move it (⇧ for beats) · drag an edge to trim · drag a top corner for a fade · right-click to remove"}
          >
            ?
          </span>
        </div>
        {tunerOpen && <TunerStrip />}
        {track ? (
          <>
            <WaveformLane track={track} />
            {track.amp ? (
              <AmpRack track={track} />
            ) : (
              <div className="empty mono" style={{ padding: 16 }}>
                no rig on this track — the input is recorded and played back as it comes in
              </div>
            )}
          </>
        ) : (
          <div className="empty mono" style={{ padding: 24 }}>
            add an audio track to record or import a guitar part
          </div>
        )}
      </div>
    </>
  );
}
