import { useEffect, useState } from "react";
import { GRID_OPTIONS, useStore } from "../store";
import { TIER_PRESETS, TIME_SIGNATURES, type Settings as SettingsT } from "../types";
import { keys } from "../platform";
import { UI_SCALE_NATIVE } from "../uiScale";

type Tab = "audio" | "midi" | "editing" | "export" | "appearance";
const TABS: [Tab, string][] = [
  ["audio", "audio"],
  ["midi", "midi"],
  ["editing", "editing"],
  ["export", "export"],
  ["appearance", "appearance"],
];
const BUFFER_SIZES = [64, 128, 256, 512, 1024, 2048];

/** ⌘, — the settings window. Every change is saved and applied immediately. */
export function SettingsWindow() {
  const open = useStore((s) => s.settingsOpen);
  const close = useStore((s) => s.openSettings);
  const [tab, setTab] = useState<Tab>("audio");

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, close]);

  if (!open) return null;
  return (
    <div className="modal-backdrop" onPointerDown={() => close(false)}>
      <div className="modal settings" role="dialog" aria-modal="true" aria-label="Settings" onPointerDown={(e) => e.stopPropagation()}>
        <div className="shead">
          <span className="mtitle">Settings</span>
          <span className="spacer" />
          <button className="ico" onClick={() => close(false)} title="close (Esc)">×</button>
        </div>
        <div className="sbody">
          <div className="stabs">
            {TABS.map(([id, label]) => (
              <div key={id} className={tab === id ? "on" : ""} onClick={() => setTab(id)}>
                {label}
              </div>
            ))}
          </div>
          <div className="spane">
            {tab === "audio" && <AudioTab />}
            {tab === "midi" && <MidiTab />}
            {tab === "editing" && <EditingTab />}
            {tab === "export" && <ExportTab />}
            {tab === "appearance" && <AppearanceTab />}
          </div>
        </div>
      </div>
    </div>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="srow" title={hint}>
      <span className="slabel">{label}</span>
      <span className="sctl">{children}</span>
    </div>
  );
}

function Toggle({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) {
  return (
    <button className={`chip tiny${on ? " on" : ""}`} onClick={() => onChange(!on)} style={{ minWidth: 34 }}>
      {on ? "on" : "off"}
    </button>
  );
}

function Seg<T extends string | number>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div className="seg">
      {options.map(([v, label]) => (
        <div key={String(v)} className={v === value ? "on" : ""} onClick={() => onChange(v)}>
          {label}
        </div>
      ))}
    </div>
  );
}

function Num({ value, min, max, step = 1, onCommit, width = 56 }: { value: number; min: number; max: number; step?: number; onCommit: (v: number) => void; width?: number }) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const v = parseFloat(text);
    if (Number.isFinite(v)) onCommit(Math.min(max, Math.max(min, v)));
    else setText(String(value));
  };
  return (
    <input
      type="text"
      className="mono"
      value={text}
      style={{ width }}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          const d = (e.key === "ArrowUp" ? 1 : -1) * step * (e.shiftKey ? 10 : 1);
          onCommit(Math.min(max, Math.max(min, +(value + d).toFixed(6))));
        }
      }}
    />
  );
}

function AudioTab() {
  const s = useStore();
  const a = s.settings.audio;
  const devices = s.outputDevices;
  const chosen = devices.find((d) => d.name === a.device) ?? devices.find((d) => d.isDefault);
  const rates = chosen?.sampleRates ?? [44100, 48000, 88200, 96000];
  const sr = s.audio?.sampleRate ?? chosen?.defaultSampleRate ?? 44100;
  const latency = (frames: number) => `${((frames / sr) * 1000).toFixed(1)} ms`;
  const live = s.audio;
  return (
    <>
      <Row label="output device" hint="which device the engine plays through">
        <select value={a.device ?? ""} onChange={(e) => s.updateSettings("audio", { device: e.target.value || null })} style={{ maxWidth: 220 }}>
          <option value="">system default{devices.find((d) => d.isDefault) ? ` (${devices.find((d) => d.isDefault)!.name})` : ""}</option>
          {devices.map((d) => (
            <option key={d.name} value={d.name}>{d.name}</option>
          ))}
        </select>
        <button className="ico" title="rescan" onClick={() => s.refreshOutputDevices()}>↻</button>
      </Row>
      <Row label="sample rate" hint="the rates this device reports; the engine follows the device if it refuses">
        <select value={a.sampleRate ?? ""} onChange={(e) => s.updateSettings("audio", { sampleRate: e.target.value ? Number(e.target.value) : null })}>
          <option value="">device default{chosen ? ` (${(chosen.defaultSampleRate / 1000).toFixed(1)} kHz)` : ""}</option>
          {rates.map((r) => (
            <option key={r} value={r}>{(r / 1000).toFixed(1)} kHz</option>
          ))}
        </select>
      </Row>
      <Row label="buffer size" hint="smaller = less latency, more CPU; the device may round it">
        <select value={a.bufferSize ?? ""} onChange={(e) => s.updateSettings("audio", { bufferSize: e.target.value ? Number(e.target.value) : null })}>
          <option value="">device default</option>
          {BUFFER_SIZES.map((n) => (
            <option key={n} value={n}>{n} frames · {latency(n)}</option>
          ))}
        </select>
      </Row>
      <div className="sdiv" />
      <Row label="engine">
        <span className="mono" style={{ color: live?.running ? "var(--ok)" : "var(--rec-text)" }}>
          {live?.running ? `running · ${live.deviceName ?? "output"} · ${((live.sampleRate ?? 0) / 1000).toFixed(1)} kHz` : `off · ${live?.error ?? "no device"}`}
        </span>
      </Row>
      {live?.running && (
        <Row label="latency / load" hint="frames per callback → output latency; load = render time over block time">
          <span className="mono" style={{ color: "var(--text-3)" }}>
            {live.blockFrames ? `${live.blockFrames} frames · ${(live.latencyMs ?? 0).toFixed(1)} ms` : "measuring…"} · {Math.round((live.load ?? 0) * 100)}%
            {live.xruns ? <span style={{ color: "var(--warn)" }}> · {live.xruns} xruns</span> : null}
          </span>
        </Row>
      )}
      <Row label="">
        <button onClick={() => s.testTone()} disabled={!live?.running && !!s.audio}>test tone</button>
        <button onClick={() => s.restartAudio()}>restart engine</button>
      </Row>
      <div className="sdiv" />
      <div className="scap">metronome</div>
      <Row label="click">
        <Toggle on={s.settings.metronome.on} onChange={(v) => s.updateSettings("metronome", { on: v })} />
        <span className="k">M in the transport toggles it too</span>
      </Row>
      <Row label="volume">
        <input type="range" min={0} max={1} step={0.05} value={s.settings.metronome.volume} onChange={(e) => s.updateSettings("metronome", { volume: Number(e.target.value) })} style={{ width: 120 }} />
        <span className="k mono">{Math.round(s.settings.metronome.volume * 100)}%</span>
      </Row>
      <Row label="count-in" hint="bars of click before the transport starts when record is armed">
        <Seg value={s.settings.metronome.countInBars} options={[[0, "off"], [1, "1 bar"], [2, "2 bars"]]} onChange={(v) => s.updateSettings("metronome", { countInBars: v })} />
      </Row>
      <Row label="pre-roll" hint="back up this many bars before the playhead when record is armed, so the first note is not clipped">
        <Seg value={s.settings.metronome.preRollBars} options={[[0, "off"], [1, "1 bar"], [2, "2 bars"]]} onChange={(v) => s.updateSettings("metronome", { preRollBars: v })} />
      </Row>
    </>
  );
}

function MidiTab() {
  const s = useStore();
  const m = s.settings.midi;
  const inputs = s.midi?.inputs ?? [];
  return (
    <>
      <Row label="default input" hint="opened at startup and whenever it reappears">
        <select value={m.defaultInput ?? ""} onChange={(e) => s.updateSettings("midi", { defaultInput: e.target.value || null })} style={{ maxWidth: 220 }}>
          <option value="">none</option>
          {inputs.map((n) => (
            <option key={n} value={n}>{n}</option>
          ))}
          {m.defaultInput && !inputs.includes(m.defaultInput) && <option value={m.defaultInput}>{m.defaultInput} (not connected)</option>}
        </select>
        <button className="ico" title="rescan" onClick={() => s.refreshMidi()}>↻</button>
      </Row>
      <Row label="auto-reconnect">
        <Toggle on={m.autoReconnect} onChange={(v) => s.updateSettings("midi", { autoReconnect: v })} />
      </Row>
      <Row label="velocity curve" hint="soft reaches high velocities easily; hard needs a firm touch; fixed uses the default velocity">
        <Seg value={m.velocityCurve} options={[["linear", "linear"], ["soft", "soft"], ["hard", "hard"], ["fixed", "fixed"]]} onChange={(v) => s.updateSettings("midi", { velocityCurve: v })} />
      </Row>
      <Row label="channel">
        <select value={m.channel ?? ""} onChange={(e) => s.updateSettings("midi", { channel: e.target.value ? Number(e.target.value) : null })}>
          <option value="">all</option>
          {Array.from({ length: 16 }, (_, i) => i + 1).map((c) => (
            <option key={c} value={c}>{c}</option>
          ))}
        </select>
      </Row>
      <Row label="octave offset">
        <Seg value={m.octaveOffset} options={[[-2, "−2"], [-1, "−1"], [0, "0"], [1, "+1"], [2, "+2"]]} onChange={(v) => s.updateSettings("midi", { octaveOffset: v })} />
      </Row>
      <Row label="now open">
        <span className="mono" style={{ color: s.midi?.open ? "var(--rec-text)" : "var(--text-5)" }}>{s.midi?.open ?? "—"}</span>
      </Row>
    </>
  );
}

function EditingTab() {
  const s = useStore();
  const e = s.settings.editing;
  const upd = (patch: Partial<SettingsT["editing"]>) => s.updateSettings("editing", patch);
  const gridOpts: [number, string][] = GRID_OPTIONS.map(([l, v]) => [v, l]);
  return (
    <>
      <Row label="default grid" hint="snap for new projects and at startup">
        <select value={e.defaultGrid} onChange={(ev) => upd({ defaultGrid: Number(ev.target.value) })}>
          {gridOpts.map(([v, l]) => (
            <option key={v} value={v}>{l}</option>
          ))}
        </select>
      </Row>
      <Row label="default velocity" hint="velocity of placed notes">
        <Num value={e.defaultVelocity} min={1} max={127} onCommit={(v) => upd({ defaultVelocity: Math.round(v) })} />
      </Row>
      <Row label="audition on place" hint="sound a note when it is placed or dragged to a new pitch">
        <Toggle on={e.auditionOnPlace} onChange={(v) => upd({ auditionOnPlace: v })} />
      </Row>
      <Row label={keys("⌥ while dragging")} hint={keys("no snap is the usual DAW convention; paint is the original dissonant mapping (⌥-drag on empty paints notes)")}>
        <Seg value={e.altKey} options={[["noSnap", "no snap"], ["paint", "paint"]]} onChange={(v) => upd({ altKey: v })} />
      </Row>
      <Row label="snap to chord changes" hint="a note placed within a grid step of a chord boundary lands on it">
        <Toggle on={e.snapToChords} onChange={(v) => upd({ snapToChords: v })} />
      </Row>
      <Row label="confirm destructive edits" hint="ask before clearing notes, deleting tracks or patterns">
        <Toggle on={e.confirmDestructive} onChange={(v) => upd({ confirmDestructive: v })} />
      </Row>
      <div className="sdiv" />
      <div className="scap">new projects</div>
      <Row label="show on launch" hint="the new-project screen: vibes, starter, recent files">
        <Toggle on={e.showWelcome} onChange={(v) => upd({ showWelcome: v })} />
      </Row>
      <Row label="start from" hint={keys("what ⌘N makes")}>
        <Seg value={e.newProject} options={[["starter", "starter (I–IV–V–vi, melody + drums)"], ["empty", "empty"]]} onChange={(v) => upd({ newProject: v })} />
      </Row>
      <Row label="tempo">
        <Num value={e.defaultTempo} min={20} max={300} onCommit={(v) => upd({ defaultTempo: v })} />
        <span className="k">bpm</span>
      </Row>
      <Row label="time signature">
        <select
          value={`${e.defaultTimeSignature.numerator}/${e.defaultTimeSignature.denominator}`}
          onChange={(ev) => {
            const [n, d] = ev.target.value.split("/").map(Number);
            upd({ defaultTimeSignature: { numerator: n, denominator: d } });
          }}
        >
          {TIME_SIGNATURES.map((t) => (
            <option key={`${t.numerator}/${t.denominator}`} value={`${t.numerator}/${t.denominator}`}>{t.numerator} / {t.denominator}</option>
          ))}
        </select>
      </Row>
      <Row label="pattern length">
        <Seg value={e.defaultPatternBars} options={[[1, "1"], [2, "2"], [4, "4"], [8, "8"]]} onChange={(v) => upd({ defaultPatternBars: v })} />
        <span className="k">bars</span>
      </Row>
      <Row label="song length" hint="where a new song ends; change it per song in SETUP">
        <Seg value={e.defaultSongBars} options={[[8, "8"], [16, "16"], [32, "32"], [64, "64"]]} onChange={(v) => upd({ defaultSongBars: v })} />
        <span className="k">bars</span>
      </Row>
    </>
  );
}

function ExportTab() {
  const s = useStore();
  const x = s.settings.export;
  const upd = (patch: Partial<SettingsT["export"]>) => s.updateSettings("export", patch);
  return (
    <>
      <Row label="sample rate">
        <Seg value={x.sampleRate} options={[[44100, "44.1k"], [48000, "48k"], [88200, "88.2k"], [96000, "96k"]]} onChange={(v) => upd({ sampleRate: v })} />
      </Row>
      <Row label="bit depth">
        <Seg value={x.bitDepth} options={[[16, "16"], [24, "24"], [32, "32 float"]]} onChange={(v) => upd({ bitDepth: v })} />
      </Row>
      <Row label="dither (16-bit)" hint="TPDF noise that hides quantization at low levels">
        <Toggle on={x.dither} onChange={(v) => upd({ dither: v })} />
      </Row>
      <Row label="normalize" hint="scale the render so its peak lands at the target">
        <Toggle on={x.normalize} onChange={(v) => upd({ normalize: v })} />
        <Num value={x.normalizeDb} min={-24} max={0} step={0.5} onCommit={(v) => upd({ normalizeDb: v })} />
        <span className="k">dBFS peak</span>
      </Row>
      <Row label="tail" hint="seconds added after the last beat for releases and reverb">
        <Num value={x.tailSeconds} min={0} max={30} step={0.5} onCommit={(v) => upd({ tailSeconds: v })} />
        <span className="k">s</span>
      </Row>
    </>
  );
}

function AppearanceTab() {
  const s = useStore();
  const a = s.settings.appearance;
  const upd = (patch: Partial<SettingsT["appearance"]>) => s.updateSettings("appearance", patch);
  const preset = TIER_PRESETS.find((p) => JSON.stringify(p.colors) === JSON.stringify(a.tierColors));
  return (
    <>
      <Row label="ui scale">
        <Seg value={a.uiScale} options={[[0.9, "90%"], [1, "100%"], [1.1, "110%"], [1.25, "125%"], [1.5, "150%"]]} onChange={(v) => upd({ uiScale: v })} />
        {!UI_SCALE_NATIVE && <span className="k" style={{ whiteSpace: "nowrap" }}>browser: use its zoom</span>}
      </Row>
      <Row label="roll row height">
        <Seg value={a.rowHeight} options={[[14, "14"], [18, "18"], [22, "22"]]} onChange={(v) => upd({ rowHeight: v })} />
        <span className="k">px</span>
      </Row>
      <Row label="reduced motion" hint="no playhead glow; fewer redraws">
        <Toggle on={a.reducedMotion} onChange={(v) => upd({ reducedMotion: v })} />
      </Row>
      <div className="sdiv" />
      <div className="scap">tier colors · the dot and hatch cues stay whatever you pick</div>
      <Row label="preset">
        <select value={preset?.name ?? "custom"} onChange={(e) => { const p = TIER_PRESETS.find((x) => x.name === e.target.value); if (p) upd({ tierColors: p.colors }); }}>
          {!preset && <option value="custom">custom</option>}
          {TIER_PRESETS.map((p) => (
            <option key={p.name} value={p.name}>{p.name}</option>
          ))}
        </select>
      </Row>
      {(["chordTone", "tension", "dissonance"] as const).map((t) => (
        <Row key={t} label={t === "chordTone" ? "chord tone" : t}>
          <input type="color" value={a.tierColors[t]} onChange={(e) => upd({ tierColors: { ...a.tierColors, [t]: e.target.value } })} />
          <span className="mono" style={{ color: a.tierColors[t] }}>{a.tierColors[t]}</span>
        </Row>
      ))}
      <Row label="accent">
        <input type="color" value={a.accent} onChange={(e) => upd({ accent: e.target.value })} />
        <span className="mono" style={{ color: a.accent }}>{a.accent}</span>
        <button className="chip tiny" onClick={() => upd({ accent: "#b48cff" })}>reset</button>
      </Row>
    </>
  );
}
