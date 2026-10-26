# Plan: SOUND page — design synth and drum sounds

Tracked in beads (epic `dis-*`, run `bd list`). Branch: `feat/sound-design`.

## Goal

A fourth mode, **sound** (next to setup / pat / song), for designing synth and drum sounds with
deep controls. Sounds you save show up in the Devices panel's *waveform synth* and *drum kit*
boxes on every project, next to a much larger factory library.

## Decisions

- **Storage: both.** A global user library (`sound-library.json` in app data) shared by all
  projects, *and* a copy embedded in each project that uses a sound (`ProjectModel.sounds`), so a
  song opens identically on another machine. The engine only ever reads the project copy.
- **Drum kits: bigger, assignable.** A drum track owns `drumPads` (1–16 pads, unique pitches).
  Each pad points at any drum sound and has a choke group. New drum tracks get an 8-pad kit
  that keeps the old four pitches (36/38/39/42), so existing grooves still play.
- **Synth: full subtractive + FM.** 3 oscillators with unison, noise, FM/ring/sync, multimode
  filter with its own envelope, pitch envelope, 2 LFOs, mono/glide, drive, crush, per-sound
  chorus and delay. About 70 parameters.
- **Drums: synthesized one-shots.** A drum patch is body (pitched osc + sweep + overtone), noise
  (filtered, multi-burst for claps), metal (808-style 6-square cluster), click, drive/crush,
  cuts. Rendered to a buffer when the patch changes, never per sample.
- **Page extras:** audition keyboard / drum pads, live scope + drawn envelopes,
  randomize / mutate, factory browser (duplicate to edit).
- **Factory sounds live in one JSON file**, `crates/dissonant-core/assets/factory_sounds.json`.
  Rust `include_str!`s it; the UI imports the same file. Patches in it may be partial: every
  field has a default (Rust `Default`, TS `DEFAULT_SYNTH_PATCH` / `DEFAULT_DRUM_PATCH`, which
  must match).
- The six old voice names (`saw square triangle sine pad pluck`) and four drum names
  (`hat clap snare kick`) stay valid factory ids and sound close to before. No schema bump: the
  new fields all default.

## Contract (Rust ⇄ TS)

Rust: `crates/dissonant-core/src/sound.rs`. TS: `ui/src/sound.ts`. serde camelCase.

```ts
type OscWave = "sine" | "triangle" | "saw" | "square" | "pulse";
type FilterMode = "off" | "lowpass12" | "lowpass24" | "highpass12" | "bandpass" | "notch";
type LfoWave = "sine" | "triangle" | "saw" | "square" | "sampleHold";
type LfoTarget = "none" | "pitch" | "cutoff" | "amp" | "pulseWidth" | "fm";
interface Envelope { attack: number; decay: number; sustain: number; release: number } // s, s, 0–1, s
interface OscParams { on; wave; octave /*-3..3*/; semi /*-12..12*/; fine /*cents ±100*/; level /*0–1*/;
  pulseWidth /*0.05–0.95*/; unison /*1–8*/; detune /*cents 0–100*/; randomPhase: boolean }
interface FilterParams { mode; cutoff /*Hz 20–20000*/; resonance /*0–1*/; drive /*0–1*/;
  keyTrack /*0–1*/; envAmount /*octaves ±8*/; velocity /*0–1, vel → cutoff*/ }
interface LfoParams { wave; rate /*Hz 0.01–30*/; depth /*0–1*/; target; retrigger: boolean }
//   depth scale: pitch ±12 st · cutoff ±4 oct · amp tremolo · pulseWidth ±0.45 · fm ±1
interface SynthPatch {
  osc: [OscParams, OscParams, OscParams];
  noiseLevel; noiseColor /*0 white – 1 dark*/; fmAmount /*osc2 → osc1 phase, 0–1*/;
  ringMod /*0–1*/; sync: boolean /*osc2 hard-synced to osc1*/;
  filter: FilterParams; ampEnv: Envelope; filterEnv: Envelope;
  pitchEnvAmount /*st ±48*/; pitchEnvDecay /*s 0.001–2*/;
  lfo: [LfoParams, LfoParams]; mono: boolean; glide /*s 0–2*/; velocity /*0–1*/;
  drive /*0–1*/; crush /*0–1*/;
  chorus: { mix; rate /*Hz 0.05–5*/; depth };
  delay: { mix; time /*s 0.01–1.5*/; feedback /*0–0.95*/ };
  level /*0–1*/;
}
interface DrumPatch {
  length /*s 0.02–3*/; tune /*st ±24*/;
  body: { level; wave: OscWave; startHz /*20–4000*/; endHz /*20–4000*/; pitchDecay /*s*/; decay /*s*/;
          overtoneLevel; overtoneRatio /*0.5–8*/ };
  noise: { level; mode: FilterMode; cutoff; resonance; attack /*s 0–0.1*/; decay; bursts /*1–8*/;
           burstSpacing /*s 0.002–0.05*/ };
  metal: { level; baseHz /*100–1000*/; decay; cutoff /*Hz, band-pass centre*/ };
  click: { level; decay /*s 0.0005–0.02*/; toneHz /*500–12000*/ };
  drive; crush; lowCut /*Hz 20–2000*/; highCut /*Hz 200–20000*/; velocity; level;
}
type SoundPatch = ({ kind: "synth" } & SynthPatch) | ({ kind: "drum" } & DrumPatch);
interface Sound { id: string; name: string; category: string; patch: SoundPatch }
interface DrumPad { pitch: number /*0–127, unique per track*/; sound: string /*sound id*/; choke: number /*0 = none*/ }

Track { …unchanged; voice: string /*synth sound id*/; drumPads: DrumPad[] }
ProjectModel { …unchanged; sounds: Sound[] /*embedded non-factory sounds*/ }
```

Ids: factory sounds use short slugs (`saw`, `acid-bass`, `kick-808`); user sounds use
`u-<uuid>`. A sound id resolves through `ProjectModel.sounds` first, then the factory, then a
fallback (factory `saw` / `DrumPatch::default()`).

Commands (new):

```ts
| { type: "setTrackSound"; id: string; sound: Sound }        // synth tracks; embeds unless factory
| { type: "setDrumPads"; id: string; pads: DrumPad[]; sounds: Sound[] } // whole kit, 1–16 pads
| { type: "upsertSound"; sound: Sound }                       // replace an embedded copy (NoSuchSound if absent)
```

`setTrackSound` / `setDrumPads` / `deleteTrack` prune embedded sounds no track uses. Errors:
wrong kind or bad pads → `InvalidValue`; `NoSuchSound` = `"no such sound"`.

Engine: `SequenceTrack` gains `synth: SynthPatch` and `drumPads: Vec<ResolvedPad { pitch, choke,
patch }>`, resolved in core. New `EngineCommand`s: `SetPreview(PreviewPatch)` (synth or drum,
updates live voices without cutting them), `PreviewNoteOn { pitch, velocity }`,
`PreviewNoteOff { pitch }`, plus a scope tap of the preview channel's output.

Bridge (new):

```ts
librarySounds(): Promise<Sound[]>;
saveLibrarySound(sound: Sound): Promise<Sound[]>;   // insert or replace by id
deleteLibrarySound(id: string): Promise<Sound[]>;
previewSound(sound: Sound | null): Promise<void>;   // null stops the preview channel
previewNote(pitch: number, velocity: number, on: boolean): Promise<void>;
previewScope(): Promise<number[]>;                  // ~512 samples, zero-cross aligned
```

## Tasks

0. **Contract foundation** (lead, before fan-out): `sound.rs` types, defaults, sanitize,
   factory loader, resolve; model fields; the three commands; `SequenceTrack` resolution;
   `ui/src/sound.ts` types + defaults; bridge signatures with mock stubs; seed factory JSON.
1. **Synth engine**: rewrite `synth.rs` on `SynthPatch` (all params), live `set_patch`; DSP
   additions in `dsp.rs`. Old six presets render close to before.
2. **Drum engine + wiring**: `drums.rs` on `DrumPatch` with variable pads and choke groups;
   `channel.rs` builds instruments from resolved patches; preview channel, preview commands and
   scope tap in `engine.rs`; Tauri commands for preview + scope.
3. **Factory library**: 40+ synth sounds (bass, lead, pad, keys, pluck, fx…) and 30+ drum
   sounds (kicks, snares, claps, hats, toms, cymbals, perc) in the JSON, with categories;
   a test that renders each and checks it sounds (non-silent, no NaN, no clipping blow-up).
4. **Library + commands**: Tauri library persistence and commands; real bridge; mock bridge
   mirrors library + commands (localStorage).
5. **SOUND page UI**: the mode, browser, parameter panels, scope + envelope views, audition
   keys/pads, randomize/mutate, save / save as / delete / assign to track.
6. **Devices panel + kit UI**: synth box picks any sound by category; drum box edits pads
   (sound, pitch, choke, add/remove); DrumGrid, Arrangement minimap and Inspector read
   `track.drumPads` instead of `DRUM_KIT`.
7. **Integration + verification** (lead): merge, full test suite, drive the app.

1–6 run in parallel after 0. Ownership: 1 → `synth.rs`, `dsp.rs`; 2 → `drums.rs`,
`channel.rs`, `engine.rs`, `render.rs`, preview/scope commands in `src-tauri`; 3 → factory JSON
and its test; 4 → library code in `src-tauri`, `bridge.ts`; 5 → new `ui/src/components/sound/*`,
`App.tsx`, `Transport.tsx`, `store.ts` mode; 6 → `BottomPanel.tsx`, `DrumGrid.tsx`,
`Arrangement.tsx`, `Inspector.tsx`.
