import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useStore } from "../store";
import { matchingKit, strandedHits } from "../drumKit";
import { FACTORY_LOOPS, loopHits, loopLengthTicks, type LoopPreset } from "../drumLoops";
import { TICKS_PER_BEAT, type SongPattern, type Track } from "../types";
import { keys } from "../platform";
import { Popover } from "./SoundPicker";
import { PresetBrowser } from "./PresetBrowser";
import { KIT_STYLES } from "./KitPicker";

const BAR = 4 * TICKS_PER_BEAT;
const WITH_KIT = "dissonant.loopWithKit";

function readWithKit(): boolean {
  try {
    return localStorage.getItem(WITH_KIT) !== "0";
  } catch {
    return true;
  }
}

/**
 * Hear a loop before loading it: one pass at the tempo it was written at, auditioned through
 * the selected track's own kit (swing included). A new preview, the same ▶ again, or closing
 * the picker stops it.
 */
function useLoopPreview() {
  const audition = useStore((s) => s.audition);
  const timers = useRef<number[]>([]);
  const [playing, setPlaying] = useState<string | null>(null);
  const stop = useCallback(() => {
    timers.current.forEach((t) => window.clearTimeout(t));
    timers.current = [];
    setPlaying(null);
  }, []);
  useEffect(() => () => timers.current.forEach((t) => window.clearTimeout(t)), []);
  const play = (loop: LoopPreset) => {
    const again = playing === loop.id;
    stop();
    if (again) return;
    setPlaying(loop.id);
    const msPerTick = 60_000 / (loop.bpm * TICKS_PER_BEAT);
    const length = loopLengthTicks(loop);
    for (const hit of loopHits(loop, length)) {
      const at = (hit.startTick + (hit.shiftTicks ?? 0)) * msPerTick;
      timers.current.push(window.setTimeout(() => audition(hit.pitch, hit.velocity, hit.lengthTicks), at));
    }
    timers.current.push(window.setTimeout(() => setPlaying(null), length * msPerTick));
  };
  return { play, playing };
}

/**
 * Write a factory loop into a drum pattern: a popover of loops grouped like the kits (▶ or ↑ ↓
 * to hear one on this track's kit), and the pick replaces the pattern's hits, repeated to fill
 * its length. With "load its kit" on, the kit the loop was written for comes too; either way
 * it's one undo step.
 */
export function LoopPicker({ track, pattern, className, children }: { track: Track; pattern: SongPattern; className: string; children: ReactNode }) {
  const dispatch = useStore((s) => s.dispatch);
  const commitGesture = useStore((s) => s.commitGesture);
  const showToast = useStore((s) => s.showToast);
  const [open, setOpen] = useState(false);
  const [withKit, setWithKit] = useState(readWithKit);
  const trigger = useRef<HTMLButtonElement>(null);

  const toggleWithKit = (on: boolean) => {
    setWithKit(on);
    try {
      localStorage.setItem(WITH_KIT, on ? "1" : "0");
    } catch {
      // A per-viewer convenience: without storage it just resets next time.
    }
  };

  const load = async (loop: LoopPreset) => {
    setOpen(false);
    const notes = loopHits(loop, pattern.lengthTicks);
    const label = `load loop: ${loop.name}`;
    const swapKit = withKit && matchingKit(track.drumPads)?.id !== loop.kit.id;
    if (swapKit) {
      // Kit and hits as one undo step, the way a knob drag folds its moves together.
      await dispatch({ type: "setDrumPads", id: track.id, pads: loop.kit.pads, sounds: [] }, true, label);
      await dispatch({ type: "setNotes", patternId: pattern.id, notes }, true, label);
      await commitGesture();
    } else {
      await dispatch({ type: "setNotes", patternId: pattern.id, notes }, false, label);
    }
    const lost = strandedHits(swapKit ? loop.kit.pads : track.drumPads, notes);
    if (lost) showToast(`${lost} hit${lost === 1 ? "" : "s"} land on pitches this kit has no pad for · load ${loop.kit.name} to hear them, or ${keys("⌘Z")}`);
    else if (pattern.notes.length) showToast(`${loop.name} replaced ${pattern.notes.length} hit${pattern.notes.length === 1 ? "" : "s"} · ${keys("⌘Z")} brings them back`);
  };

  return (
    <>
      <button ref={trigger} className={`${className}${open ? " open" : ""}`} title="fill this pattern with a ready-made loop · replaces its hits" onClick={() => setOpen((o) => !o)}>
        {children}
      </button>
      {open && trigger.current && (
        <Popover anchor={trigger.current} onClose={() => setOpen(false)} width={320}>
          <LoopBrowser
            onPick={(l) => void load(l)}
            onClose={() => setOpen(false)}
            footer={
              <label className="kp-opt" title="also swap this track's kit for the one the loop was written for">
                <input type="checkbox" checked={withKit} onChange={(e) => toggleWithKit(e.target.checked)} />
                load its kit too {matchingKit(track.drumPads) ? `· now on ${matchingKit(track.drumPads)!.name}` : "· your kit is custom"}
              </label>
            }
          />
        </Popover>
      )}
    </>
  );
}

function LoopBrowser({ onPick, onClose, footer }: { onPick: (loop: LoopPreset) => void; onClose: () => void; footer: ReactNode }) {
  const { play, playing } = useLoopPreview();
  return (
    <PresetBrowser
      items={FACTORY_LOOPS}
      groups={KIT_STYLES}
      groupOf={(l) => l.kit.style}
      value=""
      playing={playing}
      onPick={onPick}
      onPlay={play}
      onClose={onClose}
      meta={(l) => `${l.kit.name} · ${loopLengthTicks(l) / BAR} bar · ${l.bpm}`}
      matches={(l, q) => l.blurb.toLowerCase().includes(q) || l.kit.name.toLowerCase().includes(q)}
      noun="loops"
      hint="▶ plays a loop on this track's kit · picking one replaces the pattern's hits"
      footer={footer}
    />
  );
}
