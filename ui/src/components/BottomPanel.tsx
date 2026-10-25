import { dbText, selectedPattern, selectedTrack, selectedTrackIndex, useStore } from "../store";
import { DRUM_KIT, trackColor, VOICES, type MasterSettings, type Track, type TrackParam } from "../types";
import { eqDb, hz, Knob, lin, log, panText, pct } from "./Knob";
import { VMeter } from "./Meter";

const VOL = lin(0, 1.5);
const PAN = lin(-1, 1);
const TONE = log(200, 20000);
const LOWCUT = log(10, 2000);
const HICUT = log(500, 20000);
const EQ = lin(0, 2);

/** The 300px bottom panel in SONG mode: DEVICES (track instrument + bus + master) or MIXER. */
export function BottomPanel() {
  const panel = useStore((s) => s.bottomPanel);
  const setBottomPanel = useStore((s) => s.setBottomPanel);
  const track = useStore(selectedTrack);
  const index = useStore(selectedTrackIndex);
  const pattern = useStore(selectedPattern);

  const noteCount = track && pattern ? (pattern.notes.length) : 0;

  return (
    <div className="bottom">
      <div className="ptabs">
        <div className={`ptab${panel === "devices" ? " on" : ""}`} onClick={() => setBottomPanel("devices")}>devices</div>
        <div className={`ptab${panel === "mixer" ? " on" : ""}`} onClick={() => setBottomPanel("mixer")}>mixer</div>
        <div style={{ width: 1, height: 14, background: "var(--line-1)", margin: "0 8px" }} />
        {track && (
          <>
            <span style={{ width: 8, height: 8, background: trackColor(track, index), borderRadius: 1 }} />
            <span style={{ fontWeight: 600, fontSize: 11, color: "var(--text-1)", marginLeft: 5 }}>
              {String(index + 1).padStart(2, "0")} {track.name}
            </span>
            <span className="mono" style={{ fontSize: 9.5, color: "var(--text-5)", marginLeft: 10 }}>
              {track.isDrum ? "drum kit" : `synth · ${track.voice}`} · {noteCount} notes in {pattern?.name ?? "—"}
            </span>
          </>
        )}
      </div>
      {panel === "devices" ? <Devices /> : <Mixer />}
    </div>
  );
}

function Devices() {
  const track = useStore(selectedTrack);
  const master = useStore((s) => s.snapshot!.model.master);
  const masterPeak = useStore((s) => s.masterPeak);
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  if (!track) return null;

  const commit = () => void commitGesture();
  const param = (p: TrackParam, value: number) => void dispatch({ type: "setTrackParam", id: track.id, param: p, value }, true);
  const setMaster = (patch: Partial<MasterSettings>) => void dispatch({ type: "setMaster", master: { ...master, ...patch } }, true);

  return (
    <div className="devices">
      <div className="device" style={{ width: 250 }}>
        <div className="title">
          <span className="led" />
          <span>{track.isDrum ? "drum kit" : "waveform synth"}</span>
          <span className="meta">{track.isDrum ? `${DRUM_KIT.length} voices` : "6 voices"}</span>
        </div>
        <div className="pad">
          {track.isDrum ? (
            <div className="list">
              {DRUM_KIT.map((d) => (
                <div key={d.pitch} className="lrow" style={{ cursor: "default" }}>
                  <span className="num">{d.pitch}</span>
                  <span className="name">{d.name}</span>
                  <span className="meta">procedural</span>
                </div>
              ))}
              <div className="help" style={{ padding: "6px 0 0" }}>per-pad tuning and samples are on the roadmap</div>
            </div>
          ) : (
            <>
              <span className="flabel">waveform · envelope</span>
              <div className="chipsrow" style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)" }}>
                {VOICES.map((v) => (
                  <button key={v} className={`chip${track.voice === v ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackVoice", id: track.id, voice: v })}>
                    {v}
                  </button>
                ))}
              </div>
              <div className="help" style={{ padding: "4px 0 0" }}>polyblep oscillator · 12 voices · velocity → level</div>
            </>
          )}
        </div>
      </div>

      <div className="device" style={{ width: 240 }}>
        <div className="title">
          <span className="led" />
          <span>track bus</span>
          <span className="meta">tone → reverb → gain → pan</span>
        </div>
        <div className="pad">
          <div className="knobrow">
            <Knob label="vol" value={VOL.to(track.volume)} display={`${dbText(track.volume)} dB`} color="#9a9aa4" defaultValue={VOL.to(1)} onChange={(p) => param("volume", VOL.from(p))} onCommit={commit} />
            <Knob label="pan" value={PAN.to(track.pan)} display={panText(track.pan)} color="#9a9aa4" defaultValue={0.5} onChange={(p) => param("pan", PAN.from(p))} onCommit={commit} />
            <Knob label="tone" value={TONE.to(track.tone)} display={hz(track.tone)} color="#3dc8ff" defaultValue={1} onChange={(p) => param("tone", TONE.from(p))} onCommit={commit} />
            <Knob label="reverb" value={track.reverbSend} display={pct(track.reverbSend)} color="#b48cff" defaultValue={0} onChange={(p) => param("reverbSend", p)} onCommit={commit} />
          </div>
        </div>
      </div>

      <div className="device" style={{ width: 440 }}>
        <div className="title">
          <span className="led" />
          <span>master</span>
          <span className="meta">low cut → eq → high cut → reverb → gain</span>
        </div>
        <div className="pad">
          <div className="knobrow">
            <Knob label="gain" value={VOL.to(master.gain)} display={`${dbText(master.gain)} dB`} color="#9a9aa4" defaultValue={VOL.to(1)} onChange={(p) => setMaster({ gain: VOL.from(p) })} onCommit={commit} />
            <Knob label="low cut" value={LOWCUT.to(master.lowCutHz)} display={hz(master.lowCutHz)} color="#3dc8ff" defaultValue={LOWCUT.to(20)} onChange={(p) => setMaster({ lowCutHz: LOWCUT.from(p) })} onCommit={commit} />
            <Knob label="low" value={EQ.to(master.lowEq)} display={eqDb(master.lowEq)} color="#3dc8ff" defaultValue={0.5} onChange={(p) => setMaster({ lowEq: EQ.from(p) })} onCommit={commit} />
            <Knob label="mid" value={EQ.to(master.midEq)} display={eqDb(master.midEq)} color="#3dc8ff" defaultValue={0.5} onChange={(p) => setMaster({ midEq: EQ.from(p) })} onCommit={commit} />
            <Knob label="high" value={EQ.to(master.highEq)} display={eqDb(master.highEq)} color="#3dc8ff" defaultValue={0.5} onChange={(p) => setMaster({ highEq: EQ.from(p) })} onCommit={commit} />
            <Knob label="hi cut" value={HICUT.to(master.highCutHz)} display={hz(master.highCutHz)} color="#3dc8ff" defaultValue={HICUT.to(18000)} onChange={(p) => setMaster({ highCutHz: HICUT.from(p) })} onCommit={commit} />
            <Knob label="reverb" value={master.reverbWet} display={pct(master.reverbWet)} color="#b48cff" defaultValue={0} onChange={(p) => setMaster({ reverbWet: p })} onCommit={commit} />
          </div>
          <div className="row" style={{ gap: 6, fontFamily: "var(--font-mono)", fontSize: 9, color: "var(--text-4b)" }}>
            <span>OUT</span>
            <div className="hbar" style={{ flex: 1, height: 4 }}>
              <div style={{ width: `${(1 - Math.min(1, Math.max(0, (20 * Math.log10(Math.max(1e-4, Math.max(...masterPeak))) + 48) / 48))) * 100}%` }} />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function Mixer() {
  const model = useStore((s) => s.snapshot!.model);
  const selectedTrackId = useStore((s) => s.selectedTrackId);
  const trackPeaks = useStore((s) => s.trackPeaks);
  const masterPeak = useStore((s) => s.masterPeak);
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  const selectTrack = useStore((s) => s.selectTrack);
  const commit = () => void commitGesture();

  return (
    <div className="mixer">
      {model.tracks.map((t, i) => (
        <Strip
          key={t.id}
          num={String(i + 1).padStart(2, "0")}
          name={t.name}
          color={trackColor(t, i)}
          selected={t.id === selectedTrackId}
          gain={t.volume}
          pan={t.pan}
          peakL={trackPeaks[i] ?? 0}
          peakR={trackPeaks[i] ?? 0}
          track={t}
          onSelect={() => selectTrack(t.id)}
          onGain={(v) => void dispatch({ type: "setTrackParam", id: t.id, param: "volume", value: v }, true)}
          onPan={(v) => void dispatch({ type: "setTrackParam", id: t.id, param: "pan", value: v }, true)}
          onCommit={commit}
        />
      ))}
      <Strip
        num="M"
        name="master"
        color="#e6e6ea"
        selected={false}
        master
        gain={model.master.gain}
        peakL={masterPeak[0]}
        peakR={masterPeak[1]}
        onGain={(v) => void dispatch({ type: "setMaster", master: { ...model.master, gain: v } }, true)}
        onCommit={commit}
      />
    </div>
  );
}

function Strip({
  num,
  name,
  color,
  selected,
  master = false,
  gain,
  pan,
  peakL,
  peakR,
  track,
  onSelect,
  onGain,
  onPan,
  onCommit,
}: {
  num: string;
  name: string;
  color: string;
  selected: boolean;
  master?: boolean;
  gain: number;
  pan?: number;
  peakL: number;
  peakR: number;
  track?: Track;
  onSelect?: () => void;
  onGain: (v: number) => void;
  onPan?: (v: number) => void;
  onCommit: () => void;
}) {
  const dispatch = useStore((s) => s.dispatch);
  const p = VOL.to(gain);
  const setFromEvent = (e: React.PointerEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const y = (e.clientY - rect.top - 10) / (rect.height - 20);
    onGain(VOL.from(Math.min(1, Math.max(0, 1 - y))));
  };
  return (
    <div className={`strip${selected ? " on" : ""}${master ? " master" : ""}`} onClick={onSelect}>
      <div className="cap3" style={{ background: color }} />
      <div className="sname">
        <span className="num">{num}</span>
        <span>{name}</span>
      </div>
      <div style={{ display: "flex", justifyContent: "center", padding: "3px 0" }} onClick={(e) => e.stopPropagation()}>
        {pan !== undefined && onPan ? (
          <Knob label="pan" value={PAN.to(pan)} display={panText(pan)} color="#9a9aa4" size={30} defaultValue={0.5} onChange={(v) => onPan(PAN.from(v))} onCommit={onCommit} />
        ) : (
          <div style={{ height: 50 }} />
        )}
      </div>
      <div
        className="fader"
        onClick={(e) => e.stopPropagation()}
        onPointerDown={(e) => {
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          setFromEvent(e);
        }}
        onPointerMove={(e) => e.buttons === 1 && setFromEvent(e)}
        onPointerUp={onCommit}
        onDoubleClick={() => (onGain(1), onCommit())}
        title="gain · double-click resets to 0 dB"
      >
        <div className="track" />
        <div className="fcap" style={{ top: `calc(${(1 - p) * 100}% - ${(1 - p) * 20}px + 4px)` }} />
        <div className="lr">
          <VMeter peak={peakL} />
          <VMeter peak={peakR} />
        </div>
      </div>
      <div className="db">{dbText(gain)}</div>
      <div className="msrow" onClick={(e) => e.stopPropagation()}>
        {track ? (
          <>
            <button className={`msr m${track.muted ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackMuted", id: track.id, muted: !track.muted })}>M</button>
            <button className={`msr s${track.soloed ? " on" : ""}`} onClick={() => dispatch({ type: "setTrackSoloed", id: track.id, soloed: !track.soloed })}>S</button>
          </>
        ) : (
          <span className="mono" style={{ fontSize: 8.5, color: "var(--text-5)", alignSelf: "center", margin: "auto" }}>stereo out</span>
        )}
      </div>
    </div>
  );
}
