import { ParamPanel, sectionById, type EditorProps } from "./Controls";
import { AdsrView } from "./EnvelopeView";
import { SYNTH_SECTIONS } from "./soundParams";

/** [section id, width in knob columns]: the panels in signal-flow order, wrapping as they fit. */
const LAYOUT: [string, number][] = [
  ["osc1", 4],
  ["osc2", 4],
  ["osc3", 4],
  ["noise", 2],
  ["mod", 2],
  ["filter", 4],
  ["filterEnv", 4],
  ["ampEnv", 4],
  ["pitchEnv", 2],
  ["lfo1", 4],
  ["lfo2", 4],
  ["voice", 2],
  ["character", 2],
  ["chorus", 3],
  ["delay", 3],
  ["output", 1],
];

/** Every synth parameter, one panel per stage. The envelopes draw their curve above the knobs. */
export function SynthEditor(props: EditorProps) {
  const { patch } = props;
  if (patch.kind !== "synth") return null;
  return (
    <div className="sp-panels">
      {LAYOUT.map(([id, cols]) => (
        <ParamPanel
          key={id}
          section={sectionById(SYNTH_SECTIONS, id)}
          cols={cols}
          top={id === "ampEnv" ? <AdsrView env={patch.ampEnv} /> : id === "filterEnv" ? <AdsrView env={patch.filterEnv} /> : undefined}
          {...props}
        />
      ))}
    </div>
  );
}
