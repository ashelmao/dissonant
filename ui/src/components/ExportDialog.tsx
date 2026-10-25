import { useState } from "react";
import { selectedPattern, useStore } from "../store";
import type { ExportRequest } from "../types";

/**
 * ⌘E — export audio. Starts from the export defaults in Settings; every field can be changed
 * for this one render, and "save as defaults" writes them back.
 */
export function ExportDialog() {
  const open = useStore((s) => s.exportOpen);
  const close = useStore((s) => s.openExport);
  const settings = useStore((s) => s.settings.export);
  const update = useStore((s) => s.updateSettings);
  const run = useStore((s) => s.runExport);
  const mode = useStore((s) => s.mode);
  const model = useStore((s) => s.snapshot?.model);
  const pattern = useStore(selectedPattern);
  const [scope, setScope] = useState<"song" | "pattern">(mode !== "pattern" && (model?.clips.length ?? 0) > 0 ? "song" : "pattern");
  const [rate, setRate] = useState(settings.sampleRate);
  const [depth, setDepth] = useState<16 | 24 | 32>(settings.bitDepth);
  const [dither, setDither] = useState(settings.dither);
  const [normalize, setNormalize] = useState(settings.normalize);
  const [normalizeDb, setNormalizeDb] = useState(settings.normalizeDb);
  const [tail, setTail] = useState(settings.tailSeconds);
  const [loops, setLoops] = useState(1);
  const [stems, setStems] = useState(false);
  const [saveDefaults, setSaveDefaults] = useState(false);

  if (!open || !model) return null;
  const hasSong = model.clips.length > 0;
  const beats = scope === "song" ? model.songLengthBeats : (pattern?.lengthBeats ?? 0);
  const seconds = (beats * loops * 60) / model.tempo + tail;
  const mm = Math.floor(seconds / 60);
  const ss = Math.round(seconds - mm * 60);

  const go = () => {
    if (saveDefaults) void update("export", { sampleRate: rate, bitDepth: depth, dither, normalize, normalizeDb, tailSeconds: tail });
    const req: Omit<ExportRequest, "path"> = {
      scope,
      sampleRate: rate,
      bitDepth: depth,
      dither,
      normalizeDb: normalize ? normalizeDb : null,
      tailSeconds: tail,
      loops,
      stems,
    };
    void run(req);
  };

  const Seg = <T extends string | number>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (v: T) => void }) => (
    <div className="seg">
      {options.map(([v, label]) => (
        <div key={String(v)} className={v === value ? "on" : ""} onClick={() => onChange(v)}>
          {label}
        </div>
      ))}
    </div>
  );
  const Row = ({ label, children, hint }: { label: string; children: React.ReactNode; hint?: string }) => (
    <div className="srow" title={hint}>
      <span className="slabel">{label}</span>
      <span className="sctl">{children}</span>
    </div>
  );
  const Toggle = ({ on, onChange }: { on: boolean; onChange: (v: boolean) => void }) => (
    <button className={`chip tiny${on ? " on" : ""}`} onClick={() => onChange(!on)} style={{ minWidth: 34 }}>
      {on ? "on" : "off"}
    </button>
  );

  return (
    <div className="modal-backdrop" onPointerDown={() => close(false)}>
      <div className="modal settings exportdlg" role="dialog" aria-modal="true" aria-label="Export audio" onPointerDown={(e) => e.stopPropagation()}>
        <div className="shead">
          <span className="mtitle">Export audio</span>
          <span className="spacer" />
          <button className="ico" onClick={() => close(false)} title="close (Esc)">×</button>
        </div>
        <div className="spane" style={{ padding: "12px 14px" }}>
          <Row label="what">
            <Seg value={scope} options={hasSong ? [["song", "song"], ["pattern", `pattern · ${pattern?.name ?? ""}`]] : [["pattern", `pattern · ${pattern?.name ?? ""}`]]} onChange={setScope} />
          </Row>
          <Row label="loops" hint="render the whole thing this many times through (a pattern as a longer loop)">
            <Seg value={loops} options={[[1, "1"], [2, "2"], [4, "4"], [8, "8"]]} onChange={setLoops} />
            <span className="k mono">{beats ? `${mm}:${String(ss).padStart(2, "0")} with tail` : "nothing to render"}</span>
          </Row>
          <Row label="stems" hint="also write one WAV per track (everything else muted) next to the mix">
            <Toggle on={stems} onChange={setStems} />
            {stems && <span className="k">{model.tracks.length} files next to the mix</span>}
          </Row>
          <div className="sdiv" />
          <Row label="sample rate">
            <Seg value={rate} options={[[44100, "44.1k"], [48000, "48k"], [88200, "88.2k"], [96000, "96k"]]} onChange={setRate} />
          </Row>
          <Row label="bit depth">
            <Seg value={depth} options={[[16, "16"], [24, "24"], [32, "32 float"]]} onChange={setDepth} />
            {depth === 16 && (
              <>
                <span className="k">dither</span>
                <Toggle on={dither} onChange={setDither} />
              </>
            )}
          </Row>
          <Row label="normalize" hint="scale so the peak lands at the target">
            <Toggle on={normalize} onChange={setNormalize} />
            {normalize && (
              <>
                <input type="range" min={-12} max={0} step={0.5} value={normalizeDb} onChange={(e) => setNormalizeDb(Number(e.target.value))} style={{ width: 90 }} />
                <span className="k mono">{normalizeDb.toFixed(1)} dBFS</span>
              </>
            )}
          </Row>
          <Row label="tail" hint="seconds after the last beat for releases and reverb">
            <input type="range" min={0} max={10} step={0.5} value={tail} onChange={(e) => setTail(Number(e.target.value))} style={{ width: 90 }} />
            <span className="k mono">{tail.toFixed(1)} s</span>
          </Row>
          <div className="sdiv" />
          <Row label="">
            <label className="row" style={{ gap: 6, fontSize: 10.5, color: "var(--text-3)" }}>
              <input type="checkbox" checked={saveDefaults} onChange={(e) => setSaveDefaults(e.target.checked)} /> save these as the defaults
            </label>
            <span className="spacer" />
            <button className="quiet" onClick={() => close(false)}>Cancel</button>
            <button className="solid" onClick={go} disabled={!beats}>Export…</button>
          </Row>
        </div>
      </div>
    </div>
  );
}

/** Progress strip shown while an export runs. */
export function ExportProgressBar() {
  const p = useStore((s) => s.exporting);
  if (!p) return null;
  const pct = p.total ? Math.round((p.done / p.total) * 100) : 0;
  return (
    <div className="toast" style={{ display: "flex", flexDirection: "column", gap: 4, minWidth: 220 }}>
      <span>
        exporting {p.total > 1 ? `${p.done + 1} / ${p.total} · ` : ""}
        {p.current}
      </span>
      <div style={{ height: 3, background: "var(--line-2)", borderRadius: 2 }}>
        <div style={{ height: 3, width: `${pct}%`, background: "var(--accent)", borderRadius: 2, transition: "width .2s" }} />
      </div>
    </div>
  );
}
