import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getBridge } from "../../bridge";
import { pitchForKey } from "../../liveKeyboard";
import { midiName } from "../../theory";

/** Every key the chromatic live-keyboard layout uses, low row then top row. */
const LAYOUT_KEYS = "zsxdcvgbhnjm,l.;/q2w3er5t6y7ui9o0p";
const BLACK = new Set([1, 3, 6, 8, 10]);
export const DRUM_PITCH = 60;

/** Preview notes held by the mouse and the computer keyboard, counted so two holders don't cut each other. */
export function useAudition(velocity: number) {
  const [held, setHeld] = useState<ReadonlySet<number>>(() => new Set());
  const counts = useRef(new Map<number, number>());
  const vel = useRef(velocity);
  vel.current = velocity;
  const sync = () => setHeld(new Set(counts.current.keys()));
  const on = useCallback((pitch: number) => {
    const m = counts.current;
    m.set(pitch, (m.get(pitch) ?? 0) + 1);
    void getBridge().then((b) => b.previewNote(pitch, vel.current, true));
    sync();
  }, []);
  const off = useCallback((pitch: number) => {
    const m = counts.current;
    const c = m.get(pitch);
    if (c === undefined) return;
    if (c > 1) m.set(pitch, c - 1);
    else {
      m.delete(pitch);
      void getBridge().then((b) => b.previewNote(pitch, 0, false));
    }
    sync();
  }, []);
  const allOff = useCallback(() => {
    const pitches = [...counts.current.keys()];
    counts.current.clear();
    if (pitches.length) void getBridge().then((b) => pitches.forEach((p) => b.previewNote(p, 0, false)));
    sync();
  }, []);
  useEffect(() => () => allOff(), [allOff]);
  return { held, on, off, allOff };
}

export type Audition = ReturnType<typeof useAudition>;

/** Two octaves from C(3 + octave); drag across keys for a gliss. Keys show their QWERTY letter. */
export function Keyboard({ octave, audition }: { octave: number; audition: Audition }) {
  const low = 48 + octave * 12;
  const pitches = Array.from({ length: 25 }, (_, i) => low + i);
  const whites = pitches.filter((p) => !BLACK.has(p % 12));
  const letters = useMemo(() => {
    const m = new Map<number, string>();
    for (const ch of LAYOUT_KEYS) {
      const p = pitchForKey(ch, "chromatic", octave);
      if (p !== null && !m.has(p)) m.set(p, ch);
    }
    return m;
  }, [octave]);
  const mouse = useRef<number | null>(null);
  const pitchAt = (x: number, y: number) => {
    const el = document.elementFromPoint(x, y) as HTMLElement | null;
    const v = el?.closest<HTMLElement>("[data-pitch]")?.dataset.pitch;
    return v === undefined ? null : Number(v);
  };
  const move = (p: number | null) => {
    if (p === mouse.current) return;
    if (mouse.current !== null) audition.off(mouse.current);
    mouse.current = p;
    if (p !== null) audition.on(p);
  };
  const ww = 100 / whites.length;
  return (
    <div
      className="sp-keys"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        move(pitchAt(e.clientX, e.clientY));
      }}
      onPointerMove={(e) => {
        if (mouse.current !== null) move(pitchAt(e.clientX, e.clientY));
      }}
      onPointerUp={() => move(null)}
      onPointerCancel={() => move(null)}
      role="group"
      aria-label="audition keyboard"
    >
      {whites.map((p, i) => (
        <div key={p} data-pitch={p} className={`wk${audition.held.has(p) ? " on" : ""}`} style={{ left: `${i * ww}%`, width: `${ww}%` }}>
          <span className="ltr">{letters.get(p) ?? ""}</span>
          {p % 12 === 0 && <span className="nm">{midiName(p)}</span>}
        </div>
      ))}
      {pitches
        .filter((p) => BLACK.has(p % 12))
        .map((p) => {
          const before = whites.filter((w) => w < p).length;
          return (
            <div key={p} data-pitch={p} className={`bk${audition.held.has(p) ? " on" : ""}`} style={{ left: `${before * ww - ww * 0.32}%`, width: `${ww * 0.64}%` }}>
              <span className="ltr">{letters.get(p) ?? ""}</span>
            </div>
          );
        })}
    </div>
  );
}

/** One big pad for a drum draft. Any note key hits it too. */
export function DrumPadButton({ audition }: { audition: Audition }) {
  const down = useRef(false);
  const lit = [...audition.held].length > 0;
  return (
    <button
      className={`sp-drumpad${lit ? " on" : ""}`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        down.current = true;
        audition.on(DRUM_PITCH);
      }}
      onPointerUp={() => {
        if (down.current) audition.off(DRUM_PITCH);
        down.current = false;
      }}
      title="hit it · or press any note key (Z, X, Q…)"
    >
      <span className="big">hit</span>
      <span className="sub">click · or any note key</span>
    </button>
  );
}
