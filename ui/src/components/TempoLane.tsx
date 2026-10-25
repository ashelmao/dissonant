import { useRef, useState } from "react";
import { useStore } from "../store";
import { tempoMap, type TempoPoint } from "../types";
import { keys } from "../platform";

/**
 * The tempo map along the song (SETUP, under the chords). Double-click adds a point at the
 * tempo in force there; drag a point up/down for bpm (⇧ fine) and sideways to move it (snapped
 * to beats); ⌥-click toggles ramp (a glide from the previous point); right-click removes.
 */
export function TempoLane({ headW, timelineW, pxPerBeat, bpb, height = 40 }: { headW: number; timelineW: number; pxPerBeat: number; bpb: number; height?: number }) {
  const H = height;
  const model = useStore((s) => s.snapshot!.model);
  const playhead = useStore((s) => s.playhead);
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  const points = model.tempoPoints ?? [];
  const [preview, setPreview] = useState<TempoPoint | null>(null);
  const drag = useRef<{ point: TempoPoint; x0: number; y0: number; moved: boolean } | null>(null);
  const [hover, setHover] = useState<number | null>(null);

  const shown = points.map((p) => (preview && p.id === preview.id ? preview : p));
  const map = tempoMap(model.tempo, shown);
  const all = [model.tempo, ...shown.map((p) => p.bpm)];
  const lo = Math.max(20, Math.min(...all) - 10);
  const hi = Math.min(300, Math.max(...all) + 10);
  const yFor = (bpm: number) => 4 + (H - 8) * (1 - (bpm - lo) / Math.max(1, hi - lo));
  const bpmForY = (y: number) => lo + (1 - (y - 4) / (H - 8)) * (hi - lo);
  const beats = Math.ceil(timelineW / pxPerBeat);

  // Polyline sampled per beat (ramps are straight in bpm, so beat-wise sampling is exact).
  const path = Array.from({ length: beats + 1 }, (_, b) => {
    const prevBpm = b > 0 ? map.bpmAt(b - 1e-6) : map.bpmAt(0);
    const here = map.bpmAt(b);
    // A step shows as a vertical edge: emit the pre-jump value first.
    return (Math.abs(prevBpm - here) > 1e-9 ? `L${b * pxPerBeat},${yFor(prevBpm)} ` : "") + `${b === 0 ? "M" : "L"}${b * pxPerBeat},${yFor(here)}`;
  }).join(" ");

  const onDown = (e: React.PointerEvent, p: TempoPoint) => {
    e.stopPropagation();
    if (e.button === 2) {
      void dispatch({ type: "removeTempoPoint", id: p.id });
      return;
    }
    if (e.altKey) {
      void dispatch({ type: "updateTempoPoint", point: { ...p, ramp: !p.ramp } }, false, p.ramp ? "step tempo change" : "ramp tempo change");
      return;
    }
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    drag.current = { point: p, x0: e.clientX, y0: e.clientY, moved: false };
  };
  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.x0;
    const dy = e.clientY - d.y0;
    if (!d.moved && Math.hypot(dx, dy) < 3) return;
    d.moved = true;
    const step = e.shiftKey ? 0.1 : 1;
    const bpm = Math.min(300, Math.max(20, Math.round((bpmForY(yFor(d.point.bpm) + dy) / step)) * step));
    const beat = Math.max(0, Math.round(d.point.beat + dx / pxPerBeat));
    const next = { ...d.point, bpm, beat };
    setPreview(next);
    void dispatch({ type: "updateTempoPoint", point: next }, true, "move tempo change");
  };
  const onUp = () => {
    if (drag.current?.moved) void commitGesture();
    drag.current = null;
    setPreview(null);
  };

  const now = map.bpmAt(playhead);
  return (
    <div className="tempolane" style={{ width: headW + timelineW + 12, height }}>
      <div className="head" style={{ width: headW }}>
        <span className="cap">tempo</span>
        <span className="mono" style={{ color: map.isConstant ? "var(--text-5)" : "var(--accent)" }}>{now.toFixed(1)}</span>
        <span className="k">{points.length ? `${points.length} change${points.length === 1 ? "" : "s"}` : "double-click to add"}</span>
      </div>
      <svg
        className="lane"
        width={timelineW}
        height={H}
        onDoubleClick={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const beat = Math.max(0, Math.round((e.clientX - rect.left) / pxPerBeat));
          void dispatch({ type: "addTempoPoint", beat, bpm: Math.round(map.bpmAt(beat)), ramp: false });
        }}
        onPointerMove={(e) => {
          onMove(e);
          const rect = e.currentTarget.getBoundingClientRect();
          setHover((e.clientX - rect.left) / pxPerBeat);
        }}
        onPointerLeave={() => setHover(null)}
        onPointerUp={onUp}
        onPointerCancel={onUp}
        onContextMenu={(e) => e.preventDefault()}
      >
        {Array.from({ length: Math.ceil(beats / bpb) + 1 }, (_, i) => (
          <line key={i} x1={i * bpb * pxPerBeat} x2={i * bpb * pxPerBeat} y1={0} y2={H} stroke={i % 4 === 0 ? "#2a2a32" : "#1a1a1f"} />
        ))}
        <path d={path} fill="none" stroke="var(--accent)" strokeWidth={1.5} opacity={map.isConstant ? 0.35 : 0.9} />
        {shown.map((p) => (
          <g key={p.id} onPointerDown={(e) => onDown(e, p)} style={{ cursor: "ns-resize" }}>
            <circle cx={p.beat * pxPerBeat} cy={yFor(p.bpm)} r={8} fill="transparent" />
            <circle cx={p.beat * pxPerBeat} cy={yFor(p.bpm)} r={3.5} fill={p.ramp ? "var(--bg-0)" : "var(--accent)"} stroke="var(--accent)" strokeWidth={1.5} />
            <title>{keys(`${p.bpm.toFixed(1)} bpm at beat ${p.beat}${p.ramp ? " · ramp" : " · step"} · drag: move (⇧ fine) · ⌥-click: ${p.ramp ? "step" : "ramp"} · right-click: remove`)}</title>
          </g>
        ))}
        {hover !== null && !drag.current && (
          <text x={hover * pxPerBeat + 6} y={10} fill="var(--text-4)" fontFamily="JetBrains Mono, monospace" fontSize={9}>
            {map.bpmAt(hover).toFixed(1)}
          </text>
        )}
        <line x1={playhead * pxPerBeat} x2={playhead * pxPerBeat} y1={0} y2={H} stroke="rgba(255,255,255,.5)" />
      </svg>
    </div>
  );
}
