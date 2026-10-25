import { beatsPerBar, gridLabel, selectedPattern, selectedTrack, useStore } from "../store";

export function StatusBar() {
  const snapshot = useStore((s) => s.snapshot!);
  const mode = useStore((s) => s.mode);
  const audio = useStore((s) => s.audio);
  const noteLength = useStore((s) => s.noteLength);
  const selectedNoteIds = useStore((s) => s.selectedNoteIds);
  const pattern = useStore(selectedPattern);
  const track = useStore(selectedTrack);
  const bpb = useStore(beatsPerBar);

  const noteCount = pattern && track ? (pattern.notes.length) : 0;
  const totalBeats = snapshot.model.songLengthBeats;

  const context =
    mode === "pattern"
      ? `${track?.name ?? "—"} › ${pattern?.name ?? "—"} · ${noteCount} notes${selectedNoteIds.length ? ` · ${selectedNoteIds.length} selected` : ""} · snap ${gridLabel(noteLength)}`
      : `${mode} · ${snapshot.model.clips.length} clips · ${snapshot.model.sections.length} sections · ${+(totalBeats / bpb).toFixed(2)} bars · ${snapshot.model.tracks.length} tracks`;

  return (
    <div className="statusbar">
      <span style={{ color: audio?.running ? "var(--ok)" : "var(--rec-text)" }}>● {audio?.running ? "READY" : "NO AUDIO"}</span>
      <span>{context}</span>
      {useStore.getState().armed && <span style={{ color: "var(--rec-text)" }}>● REC</span>}
      {useStore.getState().liveKeyboard !== "off" && <span style={{ color: "var(--accent)" }}>keys: {useStore.getState().liveKeyboard}</span>}
      <span className="spacer" />
      <span title={snapshot.undoLabel ? `undo ${snapshot.undoLabel}` : "nothing to undo"}>
        UNDO {snapshot.canUndo ? "●" : "○"}
        {snapshot.undoLabel && <span style={{ color: "var(--text-5)", marginLeft: 4 }}>{snapshot.undoLabel}</span>}
      </span>
      <span title={snapshot.redoLabel ? `redo ${snapshot.redoLabel}` : "nothing to redo"}>REDO {snapshot.canRedo ? "●" : "○"}</span>
      {audio?.running && audio.blockFrames ? (
        <span title={`engine load · ${audio.blockFrames} frames per block · ${(audio.latencyMs ?? 0).toFixed(1)} ms`} style={{ color: (audio.load ?? 0) > 0.8 ? "var(--warn)" : undefined }}>
          cpu {Math.round((audio.load ?? 0) * 100)}%{audio.xruns ? ` · ${audio.xruns} xrun${audio.xruns === 1 ? "" : "s"}` : ""}
        </span>
      ) : null}
      <span>{audio?.sampleRate ? `${audio.sampleRate / 1000} kHz` : "—"}</span>
      <span style={{ color: "var(--text-3)" }}>v0.2.0</span>
    </div>
  );
}
