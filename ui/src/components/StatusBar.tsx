import {
  barTicks,
  gridLabel,
  recTrack,
  selectedPattern,
  selectedTrack,
  useStore,
} from "../store";
import { barLabel } from "./Editor";

export function StatusBar() {
  const snapshot = useStore((s) => s.snapshot!);
  const screen = useStore((s) => s.screen);
  const audio = useStore((s) => s.audio);
  const noteLength = useStore((s) => s.noteLength);
  const selectedNoteIds = useStore((s) => s.selectedNoteIds);
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const barTick = useStore(barTicks);
  const armed = useStore((s) => s.armed);
  const recordingAudio = useStore((s) => s.recordingAudio);
  const liveKeyboard = useStore((s) => s.liveKeyboard);
  const hearChords = useStore((s) => s.hearChords);
  const chordOffsetTicks = useStore((s) => s.chordOffsetTicks);
  const ghost = useStore((s) => s.snapshot?.model.patterns.find((p) => p.id === s.ghostPatternId && p.id !== s.selectedPatternId) ?? null);

  const m = snapshot.model;
  const noteCount = pattern ? pattern.notes.length : 0;
  const totalTicks = m.songLengthTicks;
  // "<track> › <pattern>": what PATTERN plays, what recording and the live keys write into.
  const target = pattern && track ? `${track.name} › ${pattern.name}` : null;
  const ghostTrack = ghost ? m.tracks.find((t) => t.id === ghost.trackId) : null;

  const audioTrack = useStore(recTrack);
  const takes = (snapshot.model.audioClips ?? []).filter(
    (c) => c.trackId === audioTrack?.id,
  ).length;
  const context =
    screen === "rec"
      ? `rec · ${audioTrack?.name ?? "no audio track"} · ${takes} clip${takes === 1 ? "" : "s"}${
          audioTrack?.amp
            ? ` · ${audioTrack.amp.enabled ? audioTrack.amp.amp.model : "rig off"}`
            : ""
        }`
      : screen === "pattern" && !target
        ? `${track?.name ?? "—"} › no pattern`
        : screen === "pattern"
          ? [
              target,
              `${noteCount} ${track?.isDrum ? "hits" : "notes"}${selectedNoteIds.length ? ` · ${selectedNoteIds.length} selected` : ""}`,
              `snap ${gridLabel(noteLength)}`,
              `chords from ${barLabel(chordOffsetTicks, barTick)}`,
              hearChords ? "chord bed on" : null,
              ghost ? `ghost ${ghostTrack?.name ?? "?"} › ${ghost.name}` : null,
            ]
              .filter(Boolean)
              .join(" · ")
          : `${screen} · ${m.tracks.length} tracks · ${m.patterns.length} patterns · ${m.clips.length} clips · ${m.sections.length} sections · ${+(totalTicks / barTick).toFixed(2)} bars`;

  return (
    <div className="statusbar">
      <span style={{ color: audio?.running ? "var(--ok)" : "var(--rec-text)" }}>
        ● {audio?.running ? "READY" : "NO AUDIO"}
      </span>
      <span>{context}</span>
      {(armed || recordingAudio) && <span style={{ color: "var(--rec-text)" }}>● REC → {target ?? "no pattern"}</span>}
      {liveKeyboard !== "off" && (
        <span style={{ color: "var(--accent)" }}>
          keys: {liveKeyboard} → {track?.name ?? "—"}
        </span>
      )}
      <span className="spacer" />
      <span
        title={
          snapshot.undoLabel ? `undo ${snapshot.undoLabel}` : "nothing to undo"
        }
      >
        UNDO {snapshot.canUndo ? "●" : "○"}
        {snapshot.undoLabel && (
          <span style={{ color: "var(--text-5)", marginLeft: 4 }}>
            {snapshot.undoLabel}
          </span>
        )}
      </span>
      <span
        title={
          snapshot.redoLabel ? `redo ${snapshot.redoLabel}` : "nothing to redo"
        }
      >
        REDO {snapshot.canRedo ? "●" : "○"}
      </span>
      {audio?.running && audio.blockFrames ? (
        <span
          title={`engine load · ${audio.blockFrames} frames per block · ${(audio.latencyMs ?? 0).toFixed(1)} ms`}
          style={{ color: (audio.load ?? 0) > 0.8 ? "var(--warn)" : undefined }}
        >
          cpu {Math.round((audio.load ?? 0) * 100)}%
          {audio.xruns
            ? ` · ${audio.xruns} xrun${audio.xruns === 1 ? "" : "s"}`
            : ""}
        </span>
      ) : null}
      <span>{audio?.sampleRate ? `${audio.sampleRate / 1000} kHz` : "—"}</span>
      <span style={{ color: "var(--text-3)" }}>v0.2.0</span>
    </div>
  );
}
