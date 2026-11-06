//! `dissonant-core` — the pure heart of Dissonant.
//!
//! Everything in here is deterministic, dependency-light and headless: the project model and
//! its JSON schema, the music theory (three-tier classifier, key detection, diatonic harmony),
//! the chord track, pattern arrangement, the playback [`Sequence`] the engine consumes, and
//! the [`Document`] that applies [`Command`]s with undo/redo.
//!
//! The audio engine (`dissonant-engine`) and the Tauri shell depend on this crate; this crate
//! depends on nothing platform-specific, so it also compiles to WASM if the UI ever wants it.

pub mod arrangement;
pub mod automation;
pub mod chord_track;
pub mod document;
pub mod effects;
pub mod ids;
pub mod midi_file;
pub mod model;
pub mod patch;
pub mod ripple;
pub mod sequence;
pub mod sound;
pub mod tempo;
pub mod theory;
pub mod ticks;
pub mod vibes;

pub use arrangement::Arrangement;
pub use automation::{
    track_param_range, AutoTarget, AutomationCurve, AutomationLane, Breakpoint, Curve, MasterParam,
};
pub use chord_track::ChordTrack;
pub use document::{Command, Document, EditError, InsertTarget, StepOutcome, TimeEdit, TrackParam};
pub use effects::{NativeKind, NativeParam, ParamMap, NATIVE_PREFIX};
pub use patch::{Conflict, Patch, PatchError, Personal};
pub use midi_file::{ImportedMidi, ImportedTrack, MidiError, MidiScope};
pub use model::{
    decimate_pitch_envelope, remap_ropes, rope_is_live, tidy_chords, tidy_pitch_envelope,
    tidy_ropes, tile_chords, AmpHead, AmpModel, AmpRig, AudioClip, AudioSource, CabModel, Cabinet,
    ChordEvent, Clip, ClipDetection, DetectedChord, EqModel, Gate, Insert, KeyState,
    MasterSettings, MicModel, NoteEvent, Pedal, PedalKind, PitchPoint, ProjectModel, Rack,
    ScaleType, Section, SongPattern, TimeSignature, Track, TrackKind, TrackPlugin,
    DEFAULT_PUNCH_CROSSFADE_SECONDS, DEFAULT_SONG_BARS, MAX_CLIP_GAIN, MAX_DETECTED_CHORDS,
    MAX_FADE_SECONDS, MAX_FINE_CENTS, MAX_INSERTS, MAX_PEDALS, MAX_PITCH_POINTS, MAX_SHIFT_TICKS,
    MAX_SLIDE_SECS, MIN_NOTE_TICKS, PITCH_DECIMATE_CENTS, ROPE_OVERLAP_SECS, SCHEMA_VERSION,
};
pub use sequence::{
    MasterAutomation, NoteRope, Sequence, SequenceAudioClip, SequenceTrack, TrackAutomation,
};
pub use sound::{
    default_kit, factory_kit, factory_kits, factory_sound, factory_sounds, DrumKit, DrumPad, DrumPatch, ResolvedPad, Sound, SoundPatch, SynthPatch, VoiceMode, MAX_DRUM_PADS,
};
pub use tempo::{swing_warp_ticks, Tempo, TempoMap, TempoPoint};
pub use theory::harmony::{self, ChordSuggestion, ProgressionStarter};
pub use theory::highlight::HighlightEngine;
pub use theory::key_detector::{KeyCandidate, KeyDetectionResult, KeyDetector};
pub use theory::tier::{NoteTier, Tier, TierClassifier};
pub use ticks::{bar_ticks, beats_to_ticks, ticks_to_beats, TICKS_PER_BEAT};
pub use vibes::{factory_loops, vibe_specs, vibes, DrumLane, DrumLoop, Groove, VibeInfo, VibeRole, VibeSpec};
