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
pub mod chord_track;
pub mod document;
pub mod midi_file;
pub mod model;
pub mod sequence;
pub mod sound;
pub mod tempo;
pub mod theory;
pub mod vibes;

pub use arrangement::Arrangement;
pub use chord_track::ChordTrack;
pub use document::{Command, Document, EditError, TrackParam};
pub use midi_file::{ImportedMidi, ImportedTrack, MidiError, MidiScope};
pub use model::{
    tidy_chords, tile_chords, ChordEvent, Clip, KeyState, MasterSettings, NoteEvent, ProjectModel, ScaleType, Section,
    SongPattern, TimeSignature, Track, DEFAULT_SONG_BARS, OLD_SCHEMA_ERROR, SCHEMA_VERSION,
};
pub use sequence::{Sequence, SequenceTrack};
pub use sound::{
    default_kit, factory_sound, factory_sounds, DrumPad, DrumPatch, ResolvedPad, Sound, SoundPatch, SynthPatch, MAX_DRUM_PADS,
};
pub use tempo::{swing_warp, Tempo, TempoMap, TempoPoint};
pub use theory::harmony::{self, ChordSuggestion, ProgressionStarter};
pub use theory::highlight::HighlightEngine;
pub use theory::key_detector::{KeyCandidate, KeyDetectionResult, KeyDetector};
pub use theory::tier::{Tier, TierClassifier};
pub use vibes::{vibes, Groove, VibeInfo};
