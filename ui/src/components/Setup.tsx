import { useLayoutEffect, useRef, useState } from "react";
import { beatsPerBar, useStore } from "../store";
import { ChordLane } from "./ChordLane";

/**
 * SETUP mode center: the song's foundations (chord progression, sections, tempo map, song
 * length). For now a placeholder holding the editable chord lane over the whole song.
 */
export function Setup() {
  const model = useStore((s) => s.snapshot!.model);
  const bpb = useStore(beatsPerBar);
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(1000);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const songBeats = model.songLengthBeats;
  // Fit the whole song, but never squeeze a beat under 12 px (the lane scrolls instead).
  const beatW = Math.max(12, (width - 64 - 16) / Math.max(1, songBeats));

  return (
    <div className="editor" ref={ref}>
      <div className="toolbar">
        <div className="title">
          <b>setup</b>
          <span className="meta">
            {Math.round(songBeats / bpb)} bars · {model.chords.length} chords
          </span>
        </div>
      </div>
      <div style={{ overflowX: "auto", flex: "none" }}>
        <div style={{ width: "max-content" }}>
          <ChordLane offsetBeats={0} lengthBeats={songBeats} beatW={beatW} />
        </div>
      </div>
      <div className="mono" style={{ padding: 16, color: "var(--text-5)", fontSize: 10 }}>
        SETUP page — in progress. The chord progression above runs across the whole song: drag to move, drag the right edge to resize, click to build, right-click to delete.
      </div>
    </div>
  );
}
