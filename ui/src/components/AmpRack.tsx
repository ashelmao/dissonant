import { useState } from "react";
import { Knob, lin, log } from "./Knob";
import { useStore } from "../store";
import type {
  AmpHead,
  AmpModel,
  AmpRig,
  AmpSwitchKey,
  Cabinet,
  EqModel,
  Pedal,
  PedalKind,
  Track,
} from "../types";
import {
  AMP_MODELS,
  AMP_SWITCH_MODELS,
  CAB_LABELS,
  CAB_MODELS,
  EQ_LABELS,
  EQ_MODELS,
  MAX_PEDALS,
  MIC_LABELS,
  MIC_MODELS,
  newPedal,
  PEDAL_KINDS,
} from "../types";

/** One knob on a pedal: where it sits, what it reads, and the pedal it produces when moved. */
export interface PedalKnob {
  label: string;
  /** 0…1, for the knob's arc. */
  norm: number;
  display: string;
  set(norm: number): Pedal;
}

const knob10 = lin(0, 10);
const oneDecimal = (v: number) => v.toFixed(1);

/** A 0…10 knob reading one field of a pedal. */
function tenths<P extends Pedal>(p: P, label: string, field: keyof P): PedalKnob {
  const value = p[field] as number;
  return {
    label,
    norm: knob10.to(value),
    display: oneDecimal(value),
    set: (n) => ({ ...p, [field]: knob10.from(n) }),
  };
}

/**
 * The knobs one pedal shows, in order.
 *
 * Pure so the mapping is testable without a DOM — the rack itself is only markup around this.
 */
export function pedalKnobs(p: Pedal): PedalKnob[] {
  switch (p.kind) {
    case "comp":
      return [tenths(p, "sustain", "sustain"), tenths(p, "level", "level")];
    case "drive":
      return [tenths(p, "drive", "drive"), tenths(p, "tone", "tone"), tenths(p, "level", "level")];
    case "wah":
      return [tenths(p, "sens", "sensitivity"), tenths(p, "range", "range")];
    case "chorus": {
      const rate = log(0.1, 8);
      return [
        { label: "rate", norm: rate.to(p.rateHz), display: `${p.rateHz.toFixed(2)}Hz`, set: (n) => ({ ...p, rateHz: rate.from(n) }) },
        tenths(p, "depth", "depth"),
        tenths(p, "mix", "mix"),
      ];
    }
    case "phaser": {
      const rate = log(0.1, 8);
      return [
        { label: "rate", norm: rate.to(p.rateHz), display: `${p.rateHz.toFixed(2)}Hz`, set: (n) => ({ ...p, rateHz: rate.from(n) }) },
        tenths(p, "depth", "depth"),
        tenths(p, "fdbk", "feedback"),
      ];
    }
    case "tremolo": {
      const rate = log(0.1, 14);
      return [
        { label: "rate", norm: rate.to(p.rateHz), display: `${p.rateHz.toFixed(2)}Hz`, set: (n) => ({ ...p, rateHz: rate.from(n) }) },
        tenths(p, "depth", "depth"),
      ];
    }
    case "delay": {
      const time = log(20, 1200);
      return [
        { label: "time", norm: time.to(p.timeMs), display: `${Math.round(p.timeMs)}ms`, set: (n) => ({ ...p, timeMs: time.from(n) }) },
        tenths(p, "fdbk", "feedback"),
        tenths(p, "mix", "mix"),
      ];
    }
  }
}

/** What each pedal is called on its card. */
export const PEDAL_LABELS: Record<PedalKind, string> = {
  comp: "comp",
  drive: "drive",
  wah: "wah",
  chorus: "chorus",
  phaser: "phaser",
  tremolo: "trem",
  delay: "delay",
};

/** What one circuit switch is called on the panel, and what throwing it does. Which amps offer
 *  it is `AMP_SWITCH_MODELS`, next to the rest of the model mirror; this is the wording. */
export interface AmpSwitch {
  key: AmpSwitchKey;
  label: string;
  hint: string;
}

export const AMP_SWITCHES: AmpSwitch[] = [
  {
    key: "brightCap",
    label: "bright",
    hint: "treble bypass across the gain pot: most of the voice down low, nothing wide open",
  },
  {
    key: "halfPower",
    label: "half power",
    hint: "half the output tubes: break-up and sag at a volume the room can stand",
  },
  {
    key: "cathodeFollower",
    label: "follower",
    hint: "the tone stack driven from a cathode follower, which squashes and conducts first",
  },
];

/** The switches this model has, in panel order. Empty for the amps that have none — those show
 *  no switch row at all rather than three dead toggles. */
export function ampSwitches(model: AmpModel): AmpSwitch[] {
  return AMP_SWITCHES.filter((s) => AMP_SWITCH_MODELS[s.key].includes(model));
}

/** One entry of the model menu. A rig is chosen whole — amp, tone stack, cabinet and mic are
 *  separate controls, but nobody starts from four independent guesses. Owned here rather than in
 *  the core: these are presentation, not state the engine needs to know about. */
export interface AmpPreset {
  name: string;
  /** The knob row only. Wheeling in another amp leaves its switches where a shipped amp has
   *  them, which is off — throwing one is the player's decision, not the preset's. */
  amp: Omit<AmpHead, AmpSwitchKey>;
  eq: EqModel;
  cab: Cabinet;
}

/** The head a preset builds: its knobs, and every switch in the off position. */
export function presetHead(p: AmpPreset): AmpHead {
  return { ...p.amp, brightCap: false, halfPower: false, cathodeFollower: false };
}

export const AMP_PRESETS: AmpPreset[] = [
  {
    name: "tweed combo",
    amp: { model: "clean", gain: 4, bass: 5, mid: 6, treble: 6, presence: 4, master: 6 },
    eq: "usClassic",
    cab: { model: "oneByTwelve", micModel: "dynamic", posX: 4, posZ: 3 },
  },
  {
    name: "blackface",
    amp: { model: "clean", gain: 3, bass: 6, mid: 4, treble: 7, presence: 6, master: 5 },
    eq: "usClassic",
    cab: { model: "twoByTwelve", micModel: "condenser", posX: 5, posZ: 4 },
  },
  {
    name: "british blues",
    amp: { model: "crunch", gain: 6, bass: 5, mid: 6, treble: 6, presence: 5, master: 6 },
    eq: "british",
    cab: { model: "twoByTwelve", micModel: "dynamic", posX: 5, posZ: 2 },
  },
  {
    name: "vintage stack",
    amp: { model: "crunch", gain: 7, bass: 6, mid: 5, treble: 7, presence: 6, master: 5 },
    eq: "british",
    cab: { model: "fourByTwelve", micModel: "dynamic", posX: 6, posZ: 2 },
  },
  {
    name: "modern stack",
    amp: { model: "lead", gain: 8, bass: 6, mid: 4, treble: 7, presence: 7, master: 5 },
    eq: "britishBright",
    cab: { model: "fourByTwelve", micModel: "dynamic", posX: 7, posZ: 1 },
  },
  {
    name: "boutique",
    amp: { model: "crunch", gain: 5, bass: 5, mid: 6, treble: 5, presence: 4, master: 6 },
    eq: "boutique",
    cab: { model: "twoByTwelve", micModel: "ribbon", posX: 3, posZ: 4 },
  },
  {
    name: "high octane",
    amp: { model: "lead", gain: 9, bass: 7, mid: 3, treble: 7, presence: 8, master: 4 },
    eq: "britishBright",
    cab: { model: "fourByTwelve", micModel: "condenser", posX: 8, posZ: 1 },
  },
  {
    name: "pawnshop fuzz",
    amp: { model: "fuzz", gain: 9, bass: 4, mid: 7, treble: 5, presence: 3, master: 7 },
    eq: "boutique",
    cab: { model: "oneByTwelve", micModel: "ribbon", posX: 3, posZ: 2 },
  },
];

/** The preset a rig is currently sitting on, or null once its components have been mixed by hand.
 *  Only the four component choices count — the knobs are the player's business, as on a real amp. */
export function matchingPreset(rig: AmpRig): AmpPreset | null {
  return (
    AMP_PRESETS.find(
      (p) =>
        p.amp.model === rig.amp.model &&
        p.eq === rig.eq &&
        p.cab.model === rig.cab.model &&
        p.cab.micModel === rig.cab.micModel,
    ) ?? null
  );
}

/** The distortion hue from the handoff, for the amp's own knobs. */
const DRIVE_COLOR = "var(--rec-text)";

export function AmpRack({ track }: { track: Track }) {
  const setAmp = useStore((s) => s.setAmp);
  const commitGesture = useStore((s) => s.commitGesture);
  const [adding, setAdding] = useState(false);
  const rig = track.amp;
  if (!rig) return null;

  const patch = (p: Partial<AmpRig>, transient = false) => void setAmp(track.id, p, transient);
  const commit = () => void commitGesture();
  const setPedal = (i: number, p: Pedal, transient = false) =>
    patch({ pedals: rig.pedals.map((x, j) => (j === i ? p : x)) }, transient);

  const preset = matchingPreset(rig);
  /** The macro: picking a model swaps the whole front end at once, in one undo step, the way
   *  wheeling in another amp would. */
  const pickPreset = (p: AmpPreset) => patch({ amp: presetHead(p), eq: p.eq, cab: p.cab });
  /** A direct feed never reaches a speaker, so the mic in front of one has nothing to hear. */
  const direct = rig.cab.model === "direct";
  /** Only the switches this amp's circuit has. A modern stack shows none. */
  const switches = ampSwitches(rig.amp.model);

  const ampKnob = (label: string, field: Exclude<keyof AmpHead, "model" | AmpSwitchKey>) => (
    <Knob
      key={label}
      label={label}
      value={knob10.to(rig.amp[field])}
      display={oneDecimal(rig.amp[field])}
      color={DRIVE_COLOR}
      defaultValue={0.5}
      onChange={(n) => patch({ amp: { ...rig.amp, [field]: knob10.from(n) } }, true)}
      onCommit={commit}
    />
  );

  const inDb = lin(-24, 24);
  const gateDb = lin(-80, 0);
  const release = log(5, 500);
  const outDb = lin(-24, 12);

  return (
    <>
      {/* Model menu — the macro over everything the rack below splits apart. */}
      <div className={`ampmodels${rig.enabled ? "" : " bypassed"}`}>
        <span className="cap">model</span>
        <div className="seg">
          {AMP_PRESETS.map((p) => (
            <div key={p.name} className={preset?.name === p.name ? "on" : ""} onClick={() => pickPreset(p)}>
              {p.name}
            </div>
          ))}
        </div>
      </div>
      <div className={`devices amprack${rig.enabled ? "" : " bypassed"}`}>
        {/* Input strip */}
        <div className="device">
          <div className="title">
            <span className={`led${rig.gate.on ? " on" : ""}`} />
            <span>input</span>
            <span className="meta mono" onClick={() => patch({ gate: { ...rig.gate, on: !rig.gate.on } })} title="noise gate">
              gate {rig.gate.on ? "on" : "off"}
            </span>
          </div>
          <div className="pad">
            <div className="knobrow">
              <Knob
                label="in"
                value={inDb.to(rig.inputDb)}
                display={`${rig.inputDb >= 0 ? "+" : ""}${rig.inputDb.toFixed(1)}`}
                defaultValue={inDb.to(0)}
                onChange={(n) => patch({ inputDb: inDb.from(n) }, true)}
                onCommit={commit}
              />
              <Knob
                label="thresh"
                value={gateDb.to(rig.gate.thresholdDb)}
                display={rig.gate.thresholdDb.toFixed(0)}
                defaultValue={gateDb.to(-55)}
                onChange={(n) => patch({ gate: { ...rig.gate, thresholdDb: gateDb.from(n) } }, true)}
                onCommit={commit}
              />
              <Knob
                label="rel"
                value={release.to(rig.gate.releaseMs)}
                display={`${Math.round(rig.gate.releaseMs)}ms`}
                defaultValue={release.to(120)}
                onChange={(n) => patch({ gate: { ...rig.gate, releaseMs: release.from(n) } }, true)}
                onCommit={commit}
              />
            </div>
          </div>
        </div>

        {/* Pedalboard */}
        {rig.pedals.map((p, i) => (
          <div key={`${p.kind}-${i}`} className={`device pedal${p.on ? "" : " off"}`}>
            <div className="title">
              <span
                className={`led${p.on ? " on" : ""}`}
                onClick={() => setPedal(i, { ...p, on: !p.on })}
                title={p.on ? "switch off" : "switch on"}
                style={{ cursor: "pointer" }}
              />
              <span>{PEDAL_LABELS[p.kind]}</span>
              <span
                className="meta"
                style={{ cursor: "pointer" }}
                title="remove this pedal"
                onClick={() => patch({ pedals: rig.pedals.filter((_, j) => j !== i) })}
              >
                ×
              </span>
            </div>
            <div className="pad">
              <div className="knobrow">
                {pedalKnobs(p).map((k) => (
                  <Knob
                    key={k.label}
                    label={k.label}
                    value={k.norm}
                    display={k.display}
                    onChange={(n) => setPedal(i, k.set(n), true)}
                    onCommit={commit}
                  />
                ))}
              </div>
            </div>
          </div>
        ))}

        {rig.pedals.length < MAX_PEDALS && (
          <div className="ampslot empty" onClick={() => setAdding((v) => !v)} title="add a pedal">
            {adding ? (
              <div className="pedalmenu">
                {PEDAL_KINDS.map((kind) => (
                  <div
                    key={kind}
                    onClick={(e) => {
                      e.stopPropagation();
                      setAdding(false);
                      patch({ pedals: [...rig.pedals, newPedal(kind)] });
                    }}
                  >
                    {PEDAL_LABELS[kind]}
                  </div>
                ))}
              </div>
            ) : (
              <span>＋</span>
            )}
          </div>
        )}

        {/* Amp head */}
        <div className="device amp">
          <div className="title">
            <span className={`led${rig.enabled ? " on" : ""}`} />
            <span>amp</span>
            <span className="meta mono">{rig.amp.model} · {EQ_LABELS[rig.eq]}</span>
          </div>
          <div className="pad">
            <div className="seg">
              {AMP_MODELS.map((m) => (
                <div key={m} className={rig.amp.model === m ? "on" : ""} onClick={() => patch({ amp: { ...rig.amp, model: m } })}>
                  {m}
                </div>
              ))}
            </div>
            <div className="segline">
              <span className="cap">eq</span>
              <div className="seg" title="the tone stack, chosen apart from the amp">
                {EQ_MODELS.map((m) => (
                  <div key={m} className={rig.eq === m ? "on" : ""} onClick={() => patch({ eq: m })}>
                    {EQ_LABELS[m]}
                  </div>
                ))}
              </div>
            </div>
            {switches.length > 0 && (
              <div className="segline">
                <span className="cap">circuit</span>
                <div className="seg">
                  {switches.map((s) => (
                    <div
                      key={s.key}
                      className={rig.amp[s.key] ? "on" : ""}
                      title={s.hint}
                      onClick={() => patch({ amp: { ...rig.amp, [s.key]: !rig.amp[s.key] } })}
                    >
                      {s.label}
                    </div>
                  ))}
                </div>
              </div>
            )}
            <div className="knobrow">
              {ampKnob("gain", "gain")}
              {ampKnob("bass", "bass")}
              {ampKnob("mid", "mid")}
              {ampKnob("treble", "treble")}
              {ampKnob("presence", "presence")}
              {ampKnob("master", "master")}
            </div>
          </div>
        </div>

        {/* Cabinet */}
        <div className="device">
          <div className="title">
            <span className="led on" />
            <span>cab</span>
            <span className="meta mono">
              {CAB_LABELS[rig.cab.model]}
              {direct ? "" : ` · ${MIC_LABELS[rig.cab.micModel]}`}
            </span>
          </div>
          <div className="pad">
            <div className="seg">
              {CAB_MODELS.map((m) => (
                <div key={m} className={rig.cab.model === m ? "on" : ""} onClick={() => patch({ cab: { ...rig.cab, model: m } })}>
                  {CAB_LABELS[m]}
                </div>
              ))}
            </div>
            <div className={`segline${direct ? " off" : ""}`}>
              <span className="cap">mic</span>
              <div className="seg">
                {MIC_MODELS.map((m) => (
                  <div key={m} className={rig.cab.micModel === m ? "on" : ""} onClick={() => patch({ cab: { ...rig.cab, micModel: m } })}>
                    {MIC_LABELS[m]}
                  </div>
                ))}
              </div>
            </div>
            <div className={`knobrow${direct ? " off" : ""}`}>
              <Knob
                label="pos x"
                value={knob10.to(rig.cab.posX)}
                display={oneDecimal(rig.cab.posX)}
                defaultValue={0.5}
                hint="across the cone: 0 = cone edge, mellow · 10 = dust cap, bright"
                onChange={(n) => patch({ cab: { ...rig.cab, posX: knob10.from(n) } }, true)}
                onCommit={commit}
              />
              <Knob
                label="pos z"
                value={knob10.to(rig.cab.posZ)}
                display={oneDecimal(rig.cab.posZ)}
                defaultValue={0.2}
                hint="distance: 0 = on the grille, tight and bassy · 10 = backed off, roomier"
                onChange={(n) => patch({ cab: { ...rig.cab, posZ: knob10.from(n) } }, true)}
                onCommit={commit}
              />
            </div>
          </div>
        </div>

        {/* Rack */}
        <div className="device">
          <div className="title">
            <span className={`led${rig.rack.reverb > 0 ? " on" : ""}`} />
            <span>rack</span>
            <span className="meta mono">{`${rig.rack.outputDb >= 0 ? "+" : ""}${rig.rack.outputDb.toFixed(1)} dB`}</span>
          </div>
          <div className="pad">
            <div className="knobrow">
              <Knob
                label="verb"
                value={knob10.to(rig.rack.reverb)}
                display={oneDecimal(rig.rack.reverb)}
                defaultValue={0.2}
                onChange={(n) => patch({ rack: { ...rig.rack, reverb: knob10.from(n) } }, true)}
                onCommit={commit}
              />
              <Knob
                label="decay"
                value={knob10.to(rig.rack.decay)}
                display={oneDecimal(rig.rack.decay)}
                defaultValue={0.5}
                onChange={(n) => patch({ rack: { ...rig.rack, decay: knob10.from(n) } }, true)}
                onCommit={commit}
              />
              <Knob
                label="out"
                value={outDb.to(rig.rack.outputDb)}
                display={`${rig.rack.outputDb >= 0 ? "+" : ""}${rig.rack.outputDb.toFixed(1)}`}
                defaultValue={outDb.to(0)}
                onChange={(n) => patch({ rack: { ...rig.rack, outputDb: outDb.from(n) } }, true)}
                onCommit={commit}
              />
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
