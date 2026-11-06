# Guitar tracking: latency, tuner, punch, DI → harmony, amp depth

Decided in a Q&A session on 2026-10-29. Covers everything around the native rig that is *not*
the rig itself: how a take lands in time, how you set up to play one, how a recorded DI feeds
the harmonic engine, and where the amp model goes deeper.

What already exists and is not revisited here: the rig chain (input strip → gate → ≤4 pedals →
triode preamp → passive tone stack → power stage → cabinet IR grid → rack), per-track rigs
applied to clips *and* monitored input (`crates/dissonant-engine/src/audio.rs`), clean pre-rig DI
capture, and free re-amping as a consequence.

## Out of scope, deliberately

- **Multi-input recording.** Mono, one channel, one armed track stays. A singer-guitarist
  overdubs in a second pass.
- **Comping and take lanes.** Punch is the fix-a-mistake story; stacking takes is not.
  Loop is blocked while punch is armed specifically so comping does not arrive by the back door.
- **User IR loading.** The cabinet stays synthesized so projects stay self-contained.
- **Neural (NAM-style) captures.** No knobs to learn from, heavy inference; against the point.
- **Warp.** Unchanged from the roadmap.

## 1. Take alignment

`stop_audio_record` (`src-tauri/src/lib.rs`) places a clip at the transport position captured at
record start, with no latency offset. `input_latency_ms` is computed for display only. Every take
lands late.

- Subtract `output block + input backlog + per-device manual trim` from the take's start.
- The manual trim is **per input device, keyed by device name**, persisted in settings.
- When compensation pulls the start before tick 0: **clamp to 0 and trim the audio head**, so the
  waveform still lines up with the grid. The lost head is pickup noise.
- Report it: the REC page shows "aligned −N ms" rather than doing it silently.
- MIDI record is untouched. Monitoring is not routed around the master chain.

## 2. REC page (rescoped from GTR)

The tab row becomes `SETUP | PAT | SONG | REC | SOUND`. The page **follows the armed track**.

- Owns the input device and channel picker, moved out of Settings → audio (which keeps output,
  rate, buffer, test tone, load/xruns).
- Owns arm, monitor, the record-grid chip, the punch range controls, and the alignment readout.
- Input meter, pre-trim, with peak hold and a clip latch — you cannot set gain without one.
- An armed track with no rig (`amp: None`, e.g. vocals) shows the input strip and hides the amp
  sections rather than showing a dead rack.

## 3. Tuner

- A strip on the REC page. Not a transport chip.
- **Mutes output while open** — tune silently, not through a cranked amp.
- **Configurable A4 reference** (415 Hz and friends are on-brand).
- **Named alternate tunings** (standard, drop D, DADGAD, …) with per-string targets, so it says
  which string is flat rather than which note is nearby.
- Detection runs on the armed input while the transport is stopped; monitoring already works
  stopped (`audio.rs`), so no new stream state.

## 4. Punch in / out

Clips already sum when they overlap in the engine and `AudioClip` already carries
`fade_in_seconds` / `fade_out_seconds` / `gain`, so the machinery is mostly placement and fades.

- **Dedicated punch range in the ruler.** Loop is blocked while punch is armed.
- The covered clip is **split**: fade out at punch-in, fade in at punch-out, new take between.
- **15 ms equal-power crossfades by default, draggable after.** Clip fade handles ship in SONG
  with this.
- Pre-roll and count-in reuse the existing MIDI settings.

## 5. Per-note pitch envelope

The first feature that breaks "a note has one pitch". Needed because detection must preserve
bends, and because `Note.fine` is a single value.

- Stored as decimated `(tick, cents)` points reusing the automation `Curve` step/ramp enum.
  Detection output is RDP-decimated at ~5 cents, hard cap 16 points per note.
- The envelope is an **offset on top of `fine`**, which stays the note's base detune. Every
  existing edit path keeps working.
- Edited **inside the note body in the piano roll**, where repeats are already drawn. No new
  property lane.
- Engine: the native voice gains a per-sample pitch modulation source; block-rate retuning will
  zipper on a fast bend.
- **Hosted CLAP instruments get real support via `CLAP_NOTE_EXPRESSION_TUNING`**, which requires
  per-note voice identity (note-id tracking) through the engine. That cost also unlocks per-note
  pan and volume later.
- MIDI export writes envelopes MPE-style, one note per channel.

### Tiers

- A moving note is **tiered by its start pitch** and carries a mark saying it moves. One tier per
  note stays true; the mark is the second channel the Three-Tier Doctrine already demands.

## 6. DI → harmony

The reason guitar tracking belongs in this DAW rather than in Reaper. Detection always reads the
**raw DI source**, ignoring the rig, its gate and its input trim.

- **Triggered by a button on the audio clip**, offline over the whole file. Not automatic, not
  live.
- Monophonic pitch (YIN-class) → notes on a **new MIDI track**: one pattern spanning the clip,
  placed at the clip's song position, named after the take, default synth sound.
- Bends and slides become **one note with a pitch envelope** (§5).
- **Polyphony is detected and refused**: a strummed region offers chords only and writes no
  notes, rather than emitting a garbage top line.
- Chroma → chords land in a **detected lane, accepted per segment**. Nothing touches the song's
  single progression until accepted — a bad detection must not rewrite harmony over sections you
  never played.
- Detected chroma also feeds `theory::key_detector`, so a recorded riff can offer "G minor? lock"
  the way placed notes do.

## 7. Amp depth

- **Power-amp sag and bias excursion**, power stage only, **baked per model** — tweed sags,
  modern does not. No schema field, so retuning sag later changes how old projects sound; that is
  a changelog note, not a version.
- "More models" means **circuit switches, not more enum variants**: bright cap, half-power,
  cathode-follower vs plate drive. Switches are **per-model available** (a tweed offers a bright
  cap and half-power; a modern stack does not), never four toggles bolted onto every amp.
- Rig parameter automation, expression-pedal CC and MIDI footswitch control were raised and are
  not part of this round.

## Files this touches

`crates/dissonant-core`: `model.rs` (note pitch envelope, punch/fade fields, amp switches),
`theory/tier.rs` (moving-note mark), `document.rs` (new commands).
`crates/dissonant-engine`: `rig.rs` (sag, switches), `synth.rs` (per-sample pitch mod),
`engine.rs` (note identity, punch state), `audio.rs`, new pitch/chroma detection.
`crates/dissonant-plugins`: `host.rs` (CLAP note expression).
`src-tauri`: `lib.rs` (take alignment, detection commands, input picker move), `recording.rs`.
`ui`: `GuitarTab.tsx` → REC page, new tuner, punch range in the song ruler, clip fade handles,
note-body pitch contour, detected-chord lane.
