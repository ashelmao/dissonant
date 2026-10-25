# Track-owned patterns, SETUP page, drag-to-song

Decided in a Q&A session on 2026-10-25. Replaces the FL-style "a pattern holds every track"
model with patterns that belong to one track, so a melody pattern can sit over a different
drum pattern.

## Model (schema v6)

- `SongPattern { id, trackId, name, lengthBeats, notes: NoteEvent[] }`. No more
  `notesByTrack`, no more per-pattern `chords`.
- `Clip` is unchanged; its track is its pattern's track. Moving, trimming, or extending a
  clip touches only that track.
- `ProjectModel` gains a song-level chord timeline (`chords: ChordEvent[]`, song beats) and
  an explicit `songLengthBeats`.
- No migration: there are no users with v5 files to keep.

## Chord bed

- One project-wide chord progression on the song timeline. Chords can change between
  verse and chorus.
- It is for writing and hearing, not part of the song: **monitor only, never exported**.
  Remove the "hear chords" option from export.
- Toggleable. It replaces today's "chords" / hear button. The toggle lives in the pattern
  editor and in the song view.
- The song view shows it as a bar that looks the same as the piano roll's chord lane.

## Pattern editor

- Plays **only the pattern being edited + the chord bed** (if on).
- The chords it shows and tiers against:
  - opened from the tree / not placed → the progression **from song bar 1**;
  - opened by double-clicking a clip → the progression **from that clip's start**.

## SETUP page

A third mode: `[ SETUP | PATTERN | SONG ]` in the top-left switcher. It holds every
foundational setting of the song:

- the chord progression (add, resize, reorder chords; starters and suggestions move here),
- sections and their per-section keys,
- tempo, the tempo map, swing, time signature, global key,
- song length, editable in **bars and in min:sec**.

Values edited in the moment (BPM especially) stay in the top bar too.

## Song length

- Explicit, set in SETUP. Hard end: clips can't be dropped or extended past it.
- New projects use a new **Settings › editing › default song length**.

## Left panel: track › pattern tree

- Replaces the patterns panel, in both PATTERN and SONG modes.
- Track nodes *are* the tracks: add inst/drum, rename, reorder, and delete from here.
- Collapsible branches.
- A **+ on each track node** makes a new child pattern for that track.
- No "append to song" ＋. Patterns get into the song **only by dragging**.

## Song view

- Drag a pattern from the tree onto the timeline. It always lands on **its own track's
  lane** at the dropped bar, wherever it's dropped vertically.
- ⌥-drag a clip to copy it (kept).
- Right-click a clip opens a menu (incl. **make unique**: fork into a new pattern on the same
  track). Right-click no longer deletes right away.
- Clip selection: click, shift-click, and marquee across lanes. Selected clips move
  together. **Delete/Backspace** removes them.

## New projects (starter and vibes)

- Each track gets its own pattern (melody 1, bass 1, drums 1), the chords go into the
  project progression, and the patterns are already placed so ▶ plays music right away.
- New projects open on **SETUP**.

## Still open

- Is the piano-roll chord lane still editable, or read-only with SETUP as the only chord
  editor? Same question for the song-view chord bar.
- With sections moving to SETUP, do the section markers still show (read-only) in the song
  ruler?
- The ghost-track overlay in the roll needs a pattern from the other track. Drop it, or let
  you pick which of the other track's patterns to ghost?
- Old v5 files: refuse with a clear message, or best-effort load?
- MIDI import: one new track per MIDI track, each with one pattern (assumed).

## Code this touches

Rust: `dissonant-core` `model.rs`, `arrangement.rs`, `sequence.rs`, `document.rs`
(commands), `vibes.rs`, `midi_file.rs`; `dissonant-engine` `engine.rs`, `render.rs`
(chord bed excluded from export). UI: `types.ts`, `bridge.ts`, `store.ts`, `Arrangement.tsx`,
`PatternsPanel.tsx` → tree, `PianoRoll.tsx`, `DrumGrid.tsx`, `ChordLane.tsx`,
`Inspector.tsx`, `Editor.tsx`, `Transport.tsx`, `ExportDialog.tsx`, `MenuBar.tsx`,
`StatusBar.tsx`, `Settings.tsx`, new `Setup.tsx`.
