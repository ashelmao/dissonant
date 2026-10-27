import type { ReactNode } from "react";
import { useStore } from "../store";
import { addPad, kitSounds, replacePad } from "../drumKit";
import { factorySound, MAX_DRUM_PADS, type DrumPad, type Sound } from "../sound";
import type { Track } from "../types";
import { SoundPicker } from "./SoundPicker";

/** Send `track` a whole new kit; `picked` is a just-picked library sound the project may not hold yet. */
function useSetKit(track: Track) {
  const dispatch = useStore((s) => s.dispatch);
  const library = useStore((s) => s.library);
  const projectSounds = useStore((s) => s.snapshot?.model.sounds) ?? [];
  const lookup = (id: string) => projectSounds.find((x) => x.id === id) ?? library.find((x) => x.id === id) ?? factorySound(id);
  const setKit = (next: DrumPad[], picked: Sound, label: string) =>
    void dispatch({ type: "setDrumPads", id: track.id, pads: next, sounds: kitSounds(next, [picked, ...library], projectSounds) }, false, label);
  return { setKit, lookup };
}

/**
 * Add a drum sound to a kit in one go: the picker opens (▶ or ↑ ↓ to hear a sound first), and
 * the picked sound lands on a new pad on the GM pitch that suits it (a snare on 38, an open hat
 * on 46 sharing the closed hat's choke group). Used by the drum grid and the kit device.
 */
export function AddPadPicker({ track, className, children }: { track: Track; className: string; children: ReactNode }) {
  const showToast = useStore((s) => s.showToast);
  const { setKit, lookup } = useSetKit(track);
  const full = track.drumPads.length >= MAX_DRUM_PADS;

  const pick = (s: Sound) => {
    const next = addPad(track.drumPads, s, lookup);
    if (!next) return showToast(`a kit holds up to ${MAX_DRUM_PADS} pads`, true);
    setKit(next, s, `add pad: ${s.name}`);
  };

  return (
    <SoundPicker
      kind="drum"
      value=""
      preview
      onPick={pick}
      triggerClass={className}
      disabled={full}
      title={full ? `a kit holds up to ${MAX_DRUM_PADS} pads` : "add a drum sound to the kit, on a new pad"}
    >
      {children}
    </SoundPicker>
  );
}

/**
 * Swap the sound on one pad. The pad keeps its pitch, and hits are notes on that pitch, so every
 * hit in every pattern stays where it is and plays the new sound.
 */
export function SwapPadPicker({ track, pitch, className, children }: { track: Track; pitch: number; className?: string; children: ReactNode }) {
  const { setKit } = useSetKit(track);
  const index = track.drumPads.findIndex((p) => p.pitch === pitch);
  const pad = track.drumPads[index];
  if (!pad) return null;

  const pick = (s: Sound) => {
    if (s.id === pad.sound) return;
    const next = replacePad(track.drumPads, index, { sound: s.id });
    if (next) setKit(next, s, `pad sound: ${s.name}`);
  };

  return (
    <SoundPicker kind="drum" value={pad.sound} preview onPick={pick} triggerClass={className} title="swap this pad's sound · its hits stay where they are">
      {children}
    </SoundPicker>
  );
}
