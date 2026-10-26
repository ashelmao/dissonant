//! `dissonant-engine` — everything that makes sound.
//!
//! Architecture (see `docs/brainstorms/2026-10-02-full-daw-brainstorm.md`, §2.1):
//!
//! * [`Engine`] is a pure block processor: `process(buffer)` advances the transport by the
//!   block length, finds every note-on/off that falls inside the block (to the sample), and
//!   renders sub-blocks between those events. It owns no threads and never allocates on the
//!   hot path after warm-up, so the same code runs under a real-time device callback
//!   ([`device`]) and in faster-than-real-time offline rendering ([`render`]).
//! * The non-real-time side talks to it through a lock-free ring buffer of
//!   [`EngineCommand`]s and reads position/metering back from atomics in [`Shared`].
//! * The engine never sees the project model: it plays a [`dissonant_core::Sequence`], which
//!   the application layer rebuilds whenever the model changes.

pub mod channel;
pub mod device;
pub mod drums;
pub mod dsp;
pub mod engine;
pub mod render;
pub mod synth;

pub use device::{default_output_name, list_output_devices, AudioConfig, AudioDevice, DeviceError, OutputDeviceInfo};
pub use drums::{render_drum, DrumBank, DrumKit, RenderedDrum};
pub use engine::{Engine, EngineCommand, PreviewPatch, Shared, MAX_TRACKS, SCOPE_SAMPLES};
pub use render::{normalize, render_to_buffer, render_wav, write_wav, RenderError, RenderOptions};
