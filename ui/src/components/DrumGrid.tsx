import { useRef, useState } from "react";
import { selectedPattern, selectedTrack, selectedTrackIndex, useStore } from "../store";
import { DRUM_KIT, trackColor, type NoteEvent, uuid } from "../types";
import { BASE_BEAT_W } from "./ChordLane";

/** Step sequencer for a drum track, aligned to the chord lane. Left-click/drag adds hits,
 *  right-click/drag removes. Hits are ordinary notes (pitch = kit note). */
export function DrumGrid() {
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const idx = useStore(selectedTrackIndex);
  const playhead = useStore((s) => s.playhead);
  const noteLength = useStore((s) => s.noteLength);
  const zoom = useStore((s) => s.zoom);
  const dispatch = useStore((s) => s.dispatch);
  const audition = useStore((s) => s.audition);
  const dragMode = useRef<"add" | "remove" | null>(null);
  const [working, setWorking] = useState<NoteEvent[] | null>(null);

  if (!pattern || !track) return null;
  const color = trackColor(track, idx);
  const committed = pattern.notes;
  const notes = working ?? committed;
  const stepBeats = Math.min(0.5, Math.max(0.125, noteLength));
  const steps = Math.max(1, Math.round(pattern.lengthBeats / stepBeats));
  const cellW = stepBeats * BASE_BEAT_W * zoom - 2;
  const currentStep = Math.floor(playhead / stepBeats);

  const find = (list: NoteEvent[], pitch: number, step: number) =>
    list.find((n) => n.pitch === pitch && Math.abs(n.startBeat - step * stepBeats) < 1e-6);

  const commit = () => {
    if (working) {
      void dispatch({ type: "setNotes", patternId: pattern.id, notes: working }, false, "edit drum steps");
      setWorking(null);
    }
    dragMode.current = null;
  };

  const paint = (pitch: number, step: number) => {
    const mode = dragMode.current;
    if (!mode) return;
    setWorking((prev) => {
      const cur = prev ?? committed;
      const existing = find(cur, pitch, step);
      if (mode === "add" && !existing) {
        audition(pitch);
        return [...cur, { id: uuid(), startBeat: step * stepBeats, lengthBeats: stepBeats, pitch, velocity: 100 }];
      }
      if (mode === "remove" && existing) return cur.filter((n) => n !== existing);
      return cur;
    });
  };

  return (
    <div className="drumgrid" onPointerUp={commit} onPointerLeave={commit}>
      {DRUM_KIT.map((drum) => (
        <div className="drumrow" key={drum.pitch}>
          <span className="dname">{drum.name}</span>
          <div className="drumcells">
            {Array.from({ length: steps }, (_, step) => {
              const on = !!find(notes, drum.pitch, step);
              const onBeat = Math.abs((step * stepBeats) % 1) < 1e-6;
              return (
                <div
                  key={step}
                  className={`step${on ? " on" : ""}${onBeat ? " beat" : ""}${step === currentStep ? " now" : ""}`}
                  style={{ width: cellW, background: on ? color : undefined }}
                  onPointerDown={(e) => {
                    e.preventDefault();
                    dragMode.current = e.button === 2 ? "remove" : "add";
                    paint(drum.pitch, step);
                  }}
                  onPointerEnter={() => paint(drum.pitch, step)}
                />
              );
            })}
          </div>
        </div>
      ))}
      {notes.length === 0 && <div className="help emptyline">no hits yet: click a step to add one. kick on 1 and 3, snare on 2 and 4 is a fine place to start.</div>}
      <div className="help">left-click / drag: add hits · right-click / drag: remove · step {stepBeats === 0.5 ? "1/8" : stepBeats === 0.25 ? "1/16" : "1/32"} follows snap</div>
    </div>
  );
}
