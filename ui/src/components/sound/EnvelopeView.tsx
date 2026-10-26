import type { DrumPatch, Envelope } from "../../sound";
import { adsrPoints, drumSketch, fmt } from "../../soundDesign";

const W = 200;
const H = 54;

/**
 * An ADSR drawn as it plays: the time axis is square-root warped so a 5 ms attack is still
 * visible next to a 4 s release. The held stretch is drawn at a fixed share and dashed.
 */
export function AdsrView({ env, width = W, height = H }: { env: Envelope; width?: number; height?: number }) {
  const hold = 0.15 * (env.attack + env.decay + env.release) + 0.05;
  const pts = adsrPoints(env, hold);
  const total = pts[pts.length - 1][0] || 1;
  const x = (t: number) => 2 + Math.sqrt(t / total) * (width - 4);
  const y = (v: number) => height - 3 - v * (height - 8);
  const line = pts.map(([t, v], i) => `${i ? "L" : "M"}${x(t).toFixed(1)} ${y(v).toFixed(1)}`).join(" ");
  const holdA = env.attack + env.decay;
  const marks = [
    { t: env.attack, l: "a" },
    { t: holdA, l: "d" },
    { t: holdA + hold, l: "r" },
  ];
  return (
    <svg className="sp-env" width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`envelope: attack ${fmt.secs(env.attack)}, decay ${fmt.secs(env.decay)}, sustain ${fmt.pct(env.sustain)}, release ${fmt.secs(env.release)}`}>
      <line className="grid" x1={0} x2={width} y1={y(env.sustain)} y2={y(env.sustain)} strokeDasharray="2 3" />
      {marks.map((m) => (
        <g key={m.l}>
          <line className="grid" x1={x(m.t)} x2={x(m.t)} y1={4} y2={height - 3} />
          <text className="lbl" x={x(m.t) + 2} y={10}>
            {m.l}
          </text>
        </g>
      ))}
      <path className="fill" d={`${line} L${x(total).toFixed(1)} ${y(0)} Z`} />
      <path className="curve" d={line} />
      <text className="lbl end" x={width - 3} y={height - 6}>
        {fmt.secs(env.attack + env.decay + env.release)}
      </text>
    </svg>
  );
}

const LAYERS = [
  ["body", "var(--accent)"],
  ["noise", "var(--text-3)"],
  ["metal", "var(--mute)"],
  ["click", "var(--ok)"],
] as const;

/**
 * The hit sketched from its patch: the summed amplitude filled, each audible layer as a thin
 * labelled line, and the body pitch sweep dashed on a log axis (20 Hz – 4 kHz).
 */
export function DrumSketch({ patch, width = 360, height = 120 }: { patch: DrumPatch; width?: number; height?: number }) {
  const s = drumSketch(patch, 180);
  const len = s.t[s.t.length - 1] || 1;
  const x = (t: number) => 2 + (t / len) * (width - 4);
  const ay = (v: number) => height - 14 - Math.min(1.2, v) * ((height - 22) / 1.2);
  const py = (hz: number) => height - 14 - (Math.log(Math.max(20, Math.min(4000, hz)) / 20) / Math.log(200)) * (height - 22);
  const path = (vals: number[], yf: (v: number) => number) => vals.map((v, i) => `${i ? "L" : "M"}${x(s.t[i]).toFixed(1)} ${yf(v).toFixed(1)}`).join(" ");
  const amp = path(s.amp, ay);
  const audible = LAYERS.filter(([k]) => patch[k].level > 0.01);
  return (
    <div className="sp-sketch">
      <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`drum hit sketch, ${fmt.secs(len)} long`}>
        {[0.25, 0.5, 0.75].map((f) => (
          <line key={f} className="grid" x1={x(len * f)} x2={x(len * f)} y1={4} y2={height - 14} />
        ))}
        <path className="fill" d={`${amp} L${x(len)} ${ay(0)} L${x(0)} ${ay(0)} Z`} />
        {audible.map(([k, c]) => (
          <path key={k} d={path(s.layers[k], ay)} fill="none" stroke={c} strokeWidth={1} opacity={0.75} />
        ))}
        {patch.body.level > 0.01 && <path className="pitch" d={path(s.pitch, py)} />}
        <path className="curve" d={amp} />
        <text className="lbl" x={3} y={height - 3}>
          0
        </text>
        <text className="lbl end" x={width - 3} y={height - 3}>
          {fmt.secs(len)}
        </text>
      </svg>
      <div className="legend">
        <span className="amp">amp</span>
        {audible.map(([k, c]) => (
          <span key={k} style={{ color: c }}>
            — {k}
          </span>
        ))}
        {patch.body.level > 0.01 && (
          <span className="pitch">
            ┄ pitch {fmt.hz(patch.body.startHz * Math.pow(2, patch.tune / 12))} → {fmt.hz(patch.body.endHz * Math.pow(2, patch.tune / 12))}
          </span>
        )}
      </div>
    </div>
  );
}
