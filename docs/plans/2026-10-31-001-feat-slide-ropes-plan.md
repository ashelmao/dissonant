# Plan: slide ropes and voice modes

Tracked in beads (epic `dis-*`, run `bd list`).

## Goal

Make sliding something you choose and can see, instead of a hidden side effect of the patch.
You draw a **rope** from one note to another in the piano roll, and the first note's voice
glides into the second. The sound tab's mono toggle becomes **Poly / Mono / Legato**, and glide
only ever happens through ropes.

## Decisions

Settled with the user on 2026-10-31.

- **Voice modes** replace `SynthPatch.mono: bool`:
  - **Poly**: many voices, and every note replays its attack.
  - **Mono**: one voice, and every note replays its attack.
  - **Legato**: one voice. A note that overlaps *or touches* the previous one connects with no
    new attack. The pitch **jumps**: it doesn't glide.
- **Nothing glides on its own.** Poly portamento (each note glides in from the last one
  played, [synth.rs:769](../../crates/dissonant-engine/src/synth.rs)) and mono's automatic glide
  on overlap both go away. The **glide** knob shows in every mode and becomes the default
  slide time for ropes.
- **Migration:** a patch with `mono: true` loads as Legato, so it keeps the connected feel and
  loses the automatic glide. The 16 mono factory sounds are rewritten to
  `"voice": "legato"`. No schema bump: this follows the existing `legacy_*` serde pattern.
- **Ropes sit on the destination.** A slide note stores which note it slides from. Each rope is
  separate, so each note of a chord can slide to its own target.
- **A rope is live** when its source is still sounding at the moment the slide note starts:
  `source.start < target.start <= source.end`. Touching counts as live. A rope that isn't live
  is drawn dashed and the slide note plays normally. This is one pure function in core, shared
  by the engine and the roll.
- **Slide time** comes from the patch's glide, with an optional per-note override in seconds.
  You change it by dragging the rope's midpoint; double-clicking resets it to the patch's glide.
- **Expression eases in.** The slide note's velocity, pan, filter and release take over
  gradually across the slide, not at its first sample.
- **Editing:** you drag from a rope knob onto another note on the same track. The knob is a
  small dot just past the note's right end, visible when you hover or select the note. The
  right edge itself stays the resize zone
  ([PianoRoll.tsx:756](../../ui/src/components/PianoRoll.tsx)). To delete a rope, click it and
  press Delete. When you move a note, its ropes stay attached and may become dashed. When you
  delete a note, any ropes from it go too.
- **Repeats:** if a slide note has `repeats`, only its first hit slides.
- **Plugins and MIDI export** get the overlap trick: the source note's note-off is pushed a few
  milliseconds past the slide note's start, so a synth in legato mode glides on its own. Chord
  ropes can't be honoured this way. MPE is out of scope for now.

## Approach

- **Core.** `NoteEvent` gains `slide_from: Option<Uuid>` and `slide_secs: Option<f32>`, both
  skipped when unset. Every path that clones notes keeps its ropes pointing at the right notes:
  - paste and `DuplicateRange` / `DuplicatePattern` remap ropes inside the copied set and drop
    any whose source wasn't copied;
  - the ghost repeats in [sequence.rs](../../crates/dissonant-core/src/sequence.rs) give each
    copy a deterministic id (`new_v5(id, salt)`), so a rope is remapped with the same salt.
- **Scheduler.** Sequenced notes are given a per-instance voice **tag** that rides on the event.
  A live rope makes the slide note's NoteOn carry `slide: { from_tag, secs }`. The source note's
  own note-off is suppressed: ownership of that voice passes to the slide note, whose note-off
  then releases it. The live keyboard and auditions keep releasing by pitch.
- **Touching notes in Legato.** At the same offset on the same track, NoteOn is processed before
  NoteOff, except when both are the same pitch. Today the stable sort puts the off first
  ([engine.rs:1530](../../crates/dissonant-engine/src/engine.rs)), which is why touching notes
  re-attack.
- **Synth.** Voices store the tag of the note they're playing. A slide NoteOn finds the voice
  with `from_tag` (in any voice mode), retargets its pitch, glides there over `secs`, and
  smooths in the new expression. If no voice has that tag, it plays as a normal note.

## Tasks

1. **Voice modes.** `VoiceMode` replaces `mono` in core, TS and the factory JSON, with migration.
   The engine implements Poly, Mono and Legato with automatic glide removed. Touching notes
   connect in Legato. The sound tab gets a three-way switch, and glide is always shown with the
   hint "default slide time". Remove the code that zeroes glide when mono is off.
2. **Rope model.** Add `slide_from` / `slide_secs` to `NoteEvent` in Rust and TS, the live-rope
   function, document commands (set rope, clear rope, set slide time) with undo labels, and
   rope integrity across delete, paste, duplicate and ghost repeats. Persistence and IPC
   contract tests.
3. **Slide playback.** Tagged voices; the scheduler resolves ropes and suppresses the source's
   note-off; the synth glides with eased expression; repeats slide on the first hit only; ropes
   behave correctly at loop seams and when the playhead seeks into the middle of a slide. Depends
   on 1 and 2.
4. **Ropes in the piano roll.** Rope knob and drag-to-create; ropes drawn solid when live and
   dashed when not; select a rope and press Delete; drag the midpoint to set slide time and
   double-click to reset. Depends on 2.
5. **Plugins and MIDI export.** The overlap trick for plugin tracks and in
   [midi_file.rs](../../crates/dissonant-core/src/midi_file.rs) export. Depends on 3.
