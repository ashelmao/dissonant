import { useState } from "react";
import { useStore } from "../store";
import { TRACK_PALETTE, type ScaleType, type Section } from "../types";
import { NOTE_NAMES } from "../theory";

/** Section marker colors, by position (a section's own `color` wins). */
export const SECTION_COLORS = ["#9a9aa4", "#ff9d2a", "#ff3b30", "#3dc8ff", "#b48cff", "#3dffb0"];

/** Popover for one section: name, key (or inherit), color, delete. */
export function SectionEditor({ section, color, onClose }: { section: Section; color: string; onClose: () => void }) {
  const dispatch = useStore((s) => s.dispatch);
  const [name, setName] = useState(section.name);
  const update = (patch: Partial<Section>) => void dispatch({ type: "updateSection", section: { ...section, ...patch } });
  const root = section.key?.rootPitchClass ?? null;
  const scale: ScaleType = section.key?.scale ?? "major";
  return (
    <div
      className="popover"
      style={{ top: 16, left: 0, width: 260 }}
      onPointerDown={(e) => e.stopPropagation()}
      onPointerUp={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
    >
      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className="cap">section</span>
        <span className="mono" style={{ fontSize: 9, color: "var(--text-5)" }}>from beat {section.startBeat}</span>
      </div>
      <input
        type="text"
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={() => name.trim() && name !== section.name && update({ name: name.trim() })}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") onClose();
        }}
        style={{ width: "100%", margin: "6px 0", fontFamily: "var(--font-ui)", fontWeight: 600, fontSize: 11 }}
      />
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
          {[...SECTION_COLORS, ...TRACK_PALETTE.slice(0, 6)].map((c) => (
            <div key={c} className={c === color ? "on" : ""} style={{ background: c, height: 10 }} onClick={() => update({ color: c })} />
          ))}
        </div>
      </div>
      <div className="row" style={{ gap: 4, marginTop: 6 }}>
        <button className="quiet danger" onClick={() => (onClose(), dispatch({ type: "removeSection", id: section.id }))}>delete</button>
        <span className="spacer" />
        <button className="solid" onClick={onClose}>done</button>
      </div>
    </div>
  );
}
