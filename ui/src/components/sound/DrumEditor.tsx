import { Panel, ParamPanel, sectionById, type EditorProps } from "./Controls";
import { DrumSketch } from "./EnvelopeView";
import { DRUM_SECTIONS } from "./soundParams";

const LAYOUT: [string, number][] = [
  ["body", 4],
  ["noise", 4],
  ["metal", 4],
  ["click", 3],
  ["global", 4],
];

/** A drum one-shot: its four layers, the global stage, and a sketch of the hit. */
export function DrumEditor(props: EditorProps) {
  const { patch } = props;
  if (patch.kind !== "drum") return null;
  return (
    <div className="sp-panels">
      <Panel title="the hit" cols={7}>
        <DrumSketch patch={patch} width={7 * 54 - 2} height={124} />
      </Panel>
      {LAYOUT.map(([id, cols]) => (
        // A layer at zero level is silent: dimmed, still editable.
        <ParamPanel key={id} section={sectionById(DRUM_SECTIONS, id)} cols={cols} dim={id !== "global" && patch[id as "body"].level === 0} {...props} />
      ))}
    </div>
  );
}
