# Plan: track-owned patterns, SETUP page, drag-to-song

Requirements: `docs/brainstorms/2026-10-25-track-owned-patterns-requirements.md`.
Tracked in beads (epic `dis-*`, run `bd list`). Branch: `feat/track-patterns`.

Decisions made after the requirements doc:

- Chords are edited **only in SETUP**. The piano-roll chord lane and the song-view chord bar are
  read-only, each with an "edit in SETUP" affordance.
- The song view shows the bare minimum: bars and clips. Section markers and the tempo lane
  leave the song view; both are edited in SETUP. BPM stays in the top bar.
- The roll's ghost overlay stays, as a **ghost pattern** picker: any other track's pattern, drawn
  faint, never heard.
- Old (schema < 6) files and templates are refused with a clear message. No migration.

## 1. Contract (Rust ⇄ TS). Both sides implement exactly this.

### Model, schema 6 (serde camelCase, mirrored in `ui/src/types.ts`)

```ts
interface SongPattern { id: string; trackId: string; name: string; lengthBeats: number; notes: NoteEvent[] }
interface Clip { id; patternId; startBeat; lengthBeats; offsetBeats; muted }   // unchanged; track = pattern's track
interface ProjectModel {
  schemaVersion: 6;
  ...unchanged fields (tempo, key, tracks, patterns, clips, sections, master, timeSignature,
      swing, swingGrid, tempoPoints);
  chords: ChordEvent[];        // project-wide progression in SONG beats, sorted, non-overlapping
  songLengthBeats: number;     // explicit hard end of the song, >= one bar
}
```

`SongPattern.notesByTrack` and `SongPattern.chords` are gone. `ChordTrack` stays as a Rust
helper type but the model field is a plain `Vec<ChordEvent>` named `chords`.

Decoding a file with `schemaVersion < 6` fails with the error text
`"This project was made with an older version of dissonant and can't be opened."`

### Commands (serde tag `type`, camelCase; TS `Command` union)

Changed or new (everything not listed is unchanged):

| command | behavior |
|---|---|
| `addTrack { isDrum }` | also creates one empty pattern `"<track name> 1"` for the new track |
| `deleteTrack { id }` | removes the track, its patterns, and their clips |
| `addPattern { trackId }` | new empty pattern on that track, named `"<track name> <n>"`, default length as today |
| `duplicatePattern { id }` | copy on the same track (fresh note ids) |
| `deletePattern { id }` | removes it and its clips; a track may end up with zero patterns |
| `setNotes { patternId, notes }` | no `trackId` any more |
| `setChords { chords }` | replaces the project progression (song beats) |
| `addClip { patternId, startBeat, lengthBeats \| null }` | `null` = pattern length; rejected if `startBeat >= songLengthBeats`; end clamped to `songLengthBeats` |
| `updateClip { clip }` | same clamping; `patternId` changes are ignored |
| `updateClips { clips }` | batch of `updateClip`, one undo step (group move) |
| `removeClips { ids }` | batch remove, one undo step (`removeClip` stays) |
| `makeClipUnique { id }` | duplicates the clip's pattern (same naming as `duplicatePattern`) and repoints that one clip at the copy |
| `setSongLength { beats }` | min one bar; clips starting at/after the end are removed, clips crossing it are trimmed |

### Playback / bridge

- `setPlaybackContext(mode: "pattern" | "song", patternId: string | null, chordOffsetBeats: number)`
  (Tauri command `set_playback_context` gains `chord_offset_beats: f64`, default 0).
- **Pattern mode** sequence: only the pattern's own track has notes (other tracks silent). Loop
  length = pattern length. Chord bed / tiering chords = project chords in
  `[chordOffsetBeats, chordOffsetBeats + lengthBeats)`, shifted to start at 0 and clipped to that window.
- **Song mode** sequence: per track, flattened clips of patterns whose `trackId` is that track.
  Length = `songLengthBeats` (hard end). Chords = project chords.
- `hearChords` is monitor-only, in both modes. **Export never renders the chord bed**: remove
  `hearChords` from `ExportRequest`/`ExportOptions`, no chord stem.
- Recording (MIDI / typing keyboard) writes into the playback-context pattern only when the live
  track is that pattern's track; otherwise it doesn't record.
- `importMidi(path)` (no `patternId`): each MIDI track becomes a new track with one pattern and
  one clip at beat 0; the song grows to fit if needed. Chords are not imported.
- `exportMidi(path, "pattern")` exports the context pattern (its track only); `"song"` exports
  every track over `songLengthBeats`.

### New projects, starter, vibes, templates

- Settings gain `editing.defaultSongBars` (default **32**; Rust `settings.rs` + TS `Settings`).
- `songLengthBeats = defaultSongBars × beatsPerBar`.
- Starter and vibes: one pattern per track (e.g. "melody 1", "bass 1", "drums 1"); the
  progression goes into `model.chords`, tiled across the whole song; one clip per track starting at
  beat 0 and loop-extended to the song end, so ▶ makes music immediately.
- The UI opens a fresh project (new / starter / vibe / template) in **SETUP** mode.

### Store (`ui/src/store.ts`) API that every UI piece builds on

```ts
type ViewMode = "setup" | "pattern" | "song";
mode: ViewMode; setMode(m)               // setup and song both use song playback
selectedTrackId; selectedPatternId       // invariant: a selected pattern's track is the selected track
selectTrack(id)                          // also selects that track's first pattern (or null)
selectPattern(id)                        // also selects its track
chordOffsetBeats: number                 // which song beat the edited pattern's bar 1 sits on
openPattern(patternId, atBeat?)          // select + chordOffsetBeats = atBeat ?? 0 + mode "pattern"
selectedClipIds: string[]; selectClips(ids: string[], additive?: boolean)
ghostPatternId: string | null; setGhostPattern(id | null)   // replaces ghostTrackId
patternDrag: { patternId: string; clientX: number; clientY: number } | null
setPatternDrag(d | null)
patternDropHandler: ((patternId: string, clientX: number, clientY: number) => void) | null
setPatternDropHandler(fn | null)
hearChords / toggleHearChords()          // unchanged
```

Drag protocol (pointer events, not HTML5 DnD; the Tauri webview's file-drop handling interferes
with HTML5 DnD): the tree owns the gesture. After a 4 px threshold it captures the pointer and keeps
`patternDrag` updated. On release it calls `patternDropHandler?.(id, x, y)` and then
`setPatternDrag(null)`. The song view registers the handler while it's mounted, draws a preview clip
on the pattern's own lane from `patternDrag`, and on drop dispatches `addClip` at the bar under the
pointer. The vertical position doesn't matter; the clip always goes on its own track's lane.

### Shared components

- `ChordLane` props: `{ offsetBeats: number; lengthBeats: number; beatW: number; readOnly?: boolean; onEditInSetup?: () => void }`.
  It reads `model.chords`, shows the window `[offsetBeats, offsetBeats + lengthBeats)` at x = 0, and
  when editable writes `setChords` with song-beat positions.
- `SectionEditor` lives in its own file (moved out of `Arrangement.tsx`).

## 2. Work breakdown and file ownership

Wave 1 (parallel):

- **R: Rust backend.** Owns `crates/**` and `src-tauri/**`. Covers the model and schema 6,
  arrangement and sequence, document commands, vibes and starter, MIDI, engine context, export
  without the bed, settings, and tests.
- **T: TS foundation.** Owns `types.ts`, `bridge.ts` (the browser mock mirrors the Rust commands),
  `store.ts`, `App.tsx`, the `Transport.tsx` mode switch (SETUP | PAT | SONG), the `ChordLane`
  prop refactor, the moved `SectionEditor.tsx`, `Setup.tsx` and `TrackTree.tsx` stubs, and the
  minimal compile fixes everywhere else, plus UI tests. When it's done, `pnpm typecheck` and
  `pnpm --dir ui test` pass.

Wave 2 (parallel, after T is merged; R may still be running):

- **A: track › pattern tree.** `TrackTree.tsx`, replacing `PatternsPanel.tsx`.
- **B: song view.** `Arrangement.tsx`.
- **C: SETUP page.** `Setup.tsx`, `ChordLane.tsx` (editing features), `SectionEditor.tsx`,
  `TempoLane.tsx`, and `Settings.tsx` (default song length).
- **D: pattern editor.** `PianoRoll.tsx`, `DrumGrid.tsx`, `Editor.tsx`, `Inspector.tsx`,
  `StatusBar.tsx`, `MenuBar.tsx`.

Each agent appends its CSS at the end of `ui/src/styles.css` under a `/* ── <area> ── */` heading.

Wave 3: integrate. Run `pnpm test`, `pnpm typecheck`, and `cargo test`, then a browser preview pass
(`pnpm ui`, which uses the bridge mock).
