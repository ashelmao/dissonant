import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { getBridge } from "../bridge";
import { useStore } from "../store";
import { FACTORY_KITS, kitGroove, matchingKit, strandedHits, type KitPreset } from "../drumKit";
import { factorySound } from "../sound";
import type { Track } from "../types";
import { keys } from "../platform";
import { DRUM_PITCH } from "./sound/AuditionKeys";
import { Popover } from "./SoundPicker";
import { PresetBrowser } from "./PresetBrowser";

/** Styles in file order, each once. */
export const KIT_STYLES = FACTORY_KITS.reduce<string[]>((out, k) => (out.includes(k.style) ? out : [...out, k.style]), []);

/** 8th notes at about 110 bpm. */
const STEP_MS = 270;

/**
 * Hear a kit before loading it: one bar of kick / hat / snare on the engine's preview channel,
 * a sound per step. A new preview, the same ▶ again, or closing the picker stops the old one.
 */
function useKitPreview() {
  const timers = useRef<number[]>([]);
  const [playing, setPlaying] = useState<string | null>(null);
  const stop = useCallback(() => {
    timers.current.forEach((t) => window.clearTimeout(t));
    timers.current = [];
    setPlaying(null);
  }, []);
  useEffect(
    () => () => {
      timers.current.forEach((t) => window.clearTimeout(t));
      void getBridge().then((b) => b.previewSound(null));
    },
    [],
  );
  const play = (kit: KitPreset) => {
    const again = playing === kit.id;
    stop();
    if (again) return;
    setPlaying(kit.id);
    const groove = kitGroove(kit.pads);
    groove.forEach((pad, i) => {
      const sound = pad && factorySound(pad.sound);
      if (!sound) return;
      const hit = () =>
        void getBridge().then(async (b) => {
          await b.previewSound(sound);
          await b.previewNote(DRUM_PITCH, i % 2 ? 80 : 110, true);
          await b.previewNote(DRUM_PITCH, 0, false);
        });
      timers.current.push(window.setTimeout(hit, i * STEP_MS));
    });
    timers.current.push(window.setTimeout(() => setPlaying(null), groove.length * STEP_MS));
  };
  return { play, playing };
}

/**
 * Load a whole factory kit onto a drum track: a popover of kits by style (▶ or ↑ ↓ to hear one
 * first), and the pick replaces the kit in one undoable `setDrumPads`. Kits share the GM layout,
 * so the patterns' hits keep playing the same kind of drum; any hit the new kit has no pad for
 * stays in the pattern, unheard, and a toast says how many.
 */
export function KitPicker({ track, className, children }: { track: Track; className: string; children: ReactNode }) {
  const dispatch = useStore((s) => s.dispatch);
  const showToast = useStore((s) => s.showToast);
  const patterns = useStore((s) => s.snapshot?.model.patterns);
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const current = matchingKit(track.drumPads);

  const load = (kit: KitPreset) => {
    setOpen(false);
    if (kit.id === current?.id) return;
    const notes = (patterns ?? []).filter((p) => p.trackId === track.id).flatMap((p) => p.notes);
    const lost = strandedHits(kit.pads, notes);
    void dispatch({ type: "setDrumPads", id: track.id, pads: kit.pads, sounds: [] }, false, `load kit: ${kit.name}`).then(() => {
      if (lost) showToast(`${lost} hit${lost === 1 ? "" : "s"} sit on pitches ${kit.name} has no pad for · ${keys("⌘Z")} brings the old kit back`);
    });
  };

  return (
    <>
      <button
        ref={trigger}
        className={`${className}${open ? " open" : ""}`}
        title={current ? `kit: ${current.name} · load a different kit` : "load a whole kit · replaces every pad, hits stay where they are"}
        onClick={() => setOpen((o) => !o)}
      >
        {children}
      </button>
      {open && trigger.current && (
        <Popover anchor={trigger.current} onClose={() => setOpen(false)} width={290}>
          <KitBrowser value={current?.id ?? ""} onPick={load} onClose={() => setOpen(false)} />
        </Popover>
      )}
    </>
  );
}

function KitBrowser({ value, onPick, onClose }: { value: string; onPick: (kit: KitPreset) => void; onClose: () => void }) {
  const { play, playing } = useKitPreview();
  return (
    <PresetBrowser
      items={FACTORY_KITS}
      groups={KIT_STYLES}
      groupOf={(k) => k.style}
      value={value}
      playing={playing}
      onPick={onPick}
      onPlay={play}
      onClose={onClose}
      meta={(k) => `${k.pads.length} pads`}
      matches={(k, q) => k.blurb.toLowerCase().includes(q) || k.pads.some((p) => p.sound.includes(q))}
      noun="kits"
      hint="loading a kit replaces every pad · hits stay where they are"
    />
  );
}
