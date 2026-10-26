// The live keyboard: play the selected track from QWERTY.
//  - "chromatic": FL/Ableton-style — Z row is one octave from C4, Q row the octave above.
//  - "tier": the product's own mode — home row = the chord tones of the chord under the
//    playhead, top row = its tensions, remapped live as the chord changes. A beginner can jam
//    in-harmony with no theory. Held keys keep their pitch until released.
import { useEffect } from "react";
import { patternChords, selectedPattern, useStore } from "./store";
import { tierMap } from "./theory";
import { keyAt } from "./types";

const CHROMATIC_LOW = ["z", "s", "x", "d", "c", "v", "g", "b", "h", "n", "j", "m", ",", "l", ".", ";", "/"];
const CHROMATIC_HIGH = ["q", "2", "w", "3", "e", "r", "5", "t", "6", "y", "7", "u", "i", "9", "o", "0", "p"];
const HOME_ROW = ["a", "s", "d", "f", "g", "h", "j", "k", "l", ";", "'"];
const TOP_ROW = ["q", "w", "e", "r", "t", "y", "u", "i", "o", "p", "["];

function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

/** Pitch for a key in the current mode, or null. */
export function pitchForKey(key: string, mode: "tier" | "chromatic", octave: number, state = useStore.getState()): number | null {
  const k = key.toLowerCase();
  if (mode === "chromatic") {
    let i = CHROMATIC_LOW.indexOf(k);
    if (i >= 0) return 48 + octave * 12 + i;
    i = CHROMATIC_HIGH.indexOf(k);
    if (i >= 0) return 60 + octave * 12 + i;
    return null;
  }
  const pattern = selectedPattern(state);
  const model = state.snapshot?.model;
  if (!pattern || !model) return null;
  // The pattern editor's playhead runs inside the pattern, which sits at `chordOffsetBeats` in
  // the song; SETUP and SONG run on the song itself. The key is the section key at that song
  // position, the chords are the ones the editor shows (pattern-relative) or the song's.
  const inPattern = state.mode === "pattern";
  const songBeat = inPattern ? state.chordOffsetBeats + state.playhead : state.playhead;
  const chords = inPattern ? patternChords(state) : model.chords;
  const map = tierMap(state.playhead, chords, keyAt(model, songBeat));
  const base = 48 + octave * 12;
  const pitches = (tier: "chordTone" | "tension") => {
    const out: number[] = [];
    for (let p = base; p < base + 24 && out.length < 11; p++) if (map[p % 12] === tier) out.push(p);
    return out;
  };
  let i = HOME_ROW.indexOf(k);
  if (i >= 0) return pitches("chordTone")[i] ?? null;
  i = TOP_ROW.indexOf(k);
  if (i >= 0) return pitches("tension")[i] ?? null;
  return null;
}

export function useLiveKeyboard() {
  useEffect(() => {
    const down = new Map<string, number>();
    let octave = 0;
    const onDown = (e: KeyboardEvent) => {
      const s = useStore.getState();
      // SOUND routes the keyboard to its own preview channel.
      if (s.liveKeyboard === "off" || s.mode === "sound" || e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target) || e.repeat) return;
      if (e.key === "z" && s.liveKeyboard === "tier") return; // unused in tier mode
      if (e.key === "-" || e.key === "+" || e.key === "=") {
        octave = Math.max(-2, Math.min(2, octave + (e.key === "-" ? -1 : 1)));
        s.showToast(`keyboard octave ${octave >= 0 ? "+" : ""}${octave}`);
        return;
      }
      const pitch = pitchForKey(e.key, s.liveKeyboard, octave, s);
      if (pitch === null || down.has(e.key)) return;
      e.preventDefault();
      down.set(e.key, pitch);
      s.noteOn(pitch, 100);
    };
    const onUp = (e: KeyboardEvent) => {
      const pitch = down.get(e.key);
      if (pitch === undefined) return;
      down.delete(e.key);
      useStore.getState().noteOff(pitch);
    };
    const onBlur = () => {
      for (const pitch of down.values()) useStore.getState().noteOff(pitch);
      down.clear();
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
      onBlur();
    };
  }, []);
}
