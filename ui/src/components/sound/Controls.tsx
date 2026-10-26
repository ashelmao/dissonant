import type { ReactNode } from "react";
import type { SoundPatch } from "../../sound";
import { fromNorm, getPath, setPath, toNorm, type BoolParam, type ChoiceParam, type NumParam } from "../../soundDesign";
import { Knob } from "../Knob";
import { defaultOf, type Section } from "./soundParams";

/** An edit to the draft patch. `key` coalesces a gesture (a knob drag) into one undo step. */
export type EditPatch = (patch: SoundPatch, key?: string | null) => void;

export interface EditorProps {
  patch: SoundPatch;
  onEdit: EditPatch;
  /** End of a gesture: the next edit starts a new undo step. */
  onCommit: () => void;
}

const KNOB_W = 50;

/** A titled box in the Devices style. `cols` sizes it in knob widths. */
export function Panel({ title, cols, dim, head, children }: { title: string; cols: number; dim?: boolean; head?: ReactNode; children: ReactNode }) {
  return (
    <section className={`sp-panel${dim ? " dim" : ""}`} style={{ width: cols * (KNOB_W + 4) + 14 }}>
      <div className="title">
        <span>{title}</span>
        {head}
      </div>
      <div className="pad">{children}</div>
    </section>
  );
}

export function NumKnob({ d, patch, onEdit, onCommit }: { d: NumParam } & EditorProps) {
  const v = getPath(patch, d.path) as number;
  const def = defaultOf(patch.kind, d) as number;
  return (
    <Knob
      label={d.label}
      value={toNorm(d, v)}
      display={d.format(v)}
      defaultValue={toNorm(d, def)}
      onChange={(p) => {
        const next = fromNorm(d, p);
        if (next !== v) onEdit(setPath(patch, d.path, next), d.id);
      }}
      onCommit={onCommit}
    />
  );
}

export function ChoiceChips({ d, patch, onEdit }: { d: ChoiceParam } & EditorProps) {
  const v = getPath(patch, d.path) as string;
  return (
    <div className="sp-choice" role="radiogroup" aria-label={d.label}>
      <span className="cap">{d.label}</span>
      <div className="chips">
        {d.options.map((o) => (
          <button key={o} className={`chip tiny${o === v ? " on" : ""}`} role="radio" aria-checked={o === v} onClick={() => o !== v && onEdit(setPath(patch, d.path, o))}>
            {d.labels?.[o] ?? o}
          </button>
        ))}
      </div>
    </div>
  );
}

export function BoolChip({ d, patch, onEdit }: { d: BoolParam } & EditorProps) {
  const v = getPath(patch, d.path) as boolean;
  return (
    <button className={`chip tiny${v ? " on" : ""}`} aria-pressed={v} title={d.hint} onClick={() => onEdit(setPath(patch, d.path, !v))}>
      {d.label}
    </button>
  );
}

/**
 * A section rendered from its param list: choices as chip rows, then knobs, then toggles.
 * An `on` toggle (oscillators) moves into the title bar and dims the panel when off.
 */
export function ParamPanel({ section, cols, top, dim, ...ed }: { section: Section; cols: number; top?: ReactNode; dim?: boolean } & EditorProps) {
  const on = section.params.find((d): d is BoolParam => d.type === "bool" && d.path[d.path.length - 1] === "on");
  const isOn = on ? (getPath(ed.patch, on.path) as boolean) : true;
  const choices = section.params.filter((d): d is ChoiceParam => d.type === "choice");
  const knobs = section.params.filter((d): d is NumParam => d.type === "num");
  const toggles = section.params.filter((d): d is BoolParam => d.type === "bool" && d !== on);
  return (
    <Panel title={section.title} cols={cols} dim={!isOn || dim} head={on && <BoolChip d={{ ...on, label: isOn ? "on" : "off" }} {...ed} />}>
      {top}
      {choices.map((d) => (
        <ChoiceChips key={d.id} d={d} {...ed} />
      ))}
      {knobs.length > 0 && (
        <div className="sp-knobs">
          {knobs.map((d) => (
            <NumKnob key={d.id} d={d} {...ed} />
          ))}
        </div>
      )}
      {toggles.length > 0 && (
        <div className="chips">
          {toggles.map((d) => (
            <BoolChip key={d.id} d={d} {...ed} />
          ))}
        </div>
      )}
    </Panel>
  );
}

export const sectionById = (sections: Section[], id: string) => sections.find((s) => s.id === id)!;
