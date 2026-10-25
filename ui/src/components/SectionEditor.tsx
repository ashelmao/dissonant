import { useState } from "react";
import { beatsPerBar, useStore } from "../store";
import { TRACK_PALETTE, type ScaleType, type Section } from "../types";
import { NOTE_NAMES } from "../theory";

/** Section marker colors, by position (a section's own `color` wins). */
export const SECTION_COLORS = ["#9a9aa4", "#ff9d2a", "#ff3b30", "#3dc8ff", "#b48cff", "#3dffb0"];

/**
 * One section: name, start bar, key (or inherit the song key), color, delete. A popover by
 * default; `docked` draws it as a plain panel (SETUP's sections column).
 */
export function SectionEditor({ section, color, onClose, docked = false }: { section: Section; color: string; onClose: () => void; docked?: boolean }) {
  const dispatch = useStore((s) => s.dispatch);
  const bpb = useStore(beatsPerBar);
  const [name, setName] = useState(section.name);
  const [barText, setBarText] = useState<string | null>(null);
  const bar = Math.floor(section.startBeat / bpb) + 1;
  const update = (patch: Partial<Section>) => void dispatch({ type: "updateSection", section: { ...section, ...patch } });
  const root = section.key?.rootPitchClass ?? null;
  const scale: ScaleType = section.key?.scale ?? "major";
  const commitBar = () => {
    const v = parseInt(barText ?? "", 10);
    if (Number.isFinite(v) && v >= 1 && v !== bar) update({ startBeat: (v - 1) * bpb });
    setBarText(null);
  };
  return (
    <div
      className={docked ? "sectioneditor docked" : "popover sectioneditor"}
      style={docked ? undefined : { top: 16, left: 0, width: 260 }}
      onPointerDown={(e) => e.stopPropagation()}
      onPointerUp={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      {!docked && (
        <div className="row" style={{ justifyContent: "space-between" }}>
          <span className="cap">section</span>
          <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>from bar {bar}</span>
        </div>
      )}
      <input
        type="text"
        autoFocus={!docked}
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={() => name.trim() && name !== section.name && update({ name: name.trim() })}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") onClose();
        }}
        style={{ width: "100%", margin: "6px 0", fontFamily: "var(--font-ui)", fontWeight: 600, fontSize: 11 }}
        aria-label="section name"
      />
      <div className="field">
        <span className="flabel">starts at bar</span>
        <input
          type="text"
          className="mono"
          value={barText ?? String(bar)}
          onChange={(e) => setBarText(e.target.value)}
          onBlur={commitBar}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            if (e.key === "Escape") setBarText(null);
            if (e.key === "ArrowUp" || e.key === "ArrowDown") {
              e.preventDefault();
              const v = Math.max(1, bar + (e.key === "ArrowUp" ? 1 : -1));
              if (v !== bar) update({ startBeat: (v - 1) * bpb });
            }
          }}
          style={{ width: 48, textAlign: "right" }}
          aria-label="start bar"
        />
      </div>
      <div className="field">
        <span className="flabel">key</span>
        <div className="row" style={{ gap: 4 }}>
          <select
            value={root ?? ""}
            onChange={(e) => update({ key: e.target.value === "" ? null : { rootPitchClass: parseInt(e.target.value, 10), scale, isLocked: true } })}
            title="a section key tiers the patterns placed inside it against this key"
          >
            <option value="">inherit</option>
            {NOTE_NAMES.map((n, i) => (
              <option key={n} value={i}>{n}</option>
            ))}
          </select>
          <select value={scale} disabled={root === null} onChange={(e) => update({ key: { rootPitchClass: root, scale: e.target.value as ScaleType, isLocked: true } })}>
            <option value="major">Major</option>
            <option value="minor">Minor</option>
          </select>
        </div>
      </div>
      <div className="field">
        <span className="flabel">color</span>
        <div className="swatches" style={{ width: 150 }}>
          {[...new Set([...SECTION_COLORS, ...TRACK_PALETTE.slice(0, 6)])].map((c) => (
            <div key={c} className={c === color ? "on" : ""} style={{ background: c, height: 10 }} onClick={() => update({ color: c })} />
          ))}
        </div>
      </div>
      <div className="row" style={{ gap: 4, marginTop: 6 }}>
        <button className="quiet danger" onClick={() => (onClose(), dispatch({ type: "removeSection", id: section.id }))}>delete</button>
        <span className="spacer" />
        <button className={docked ? "quiet" : "solid"} onClick={onClose}>done</button>
      </div>
    </div>
  );
}
