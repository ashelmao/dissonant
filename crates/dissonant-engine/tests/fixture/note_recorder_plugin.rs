// A real CLAP instrument that makes no sound and writes down every note event it is sent.
//
// Per-note expression is a conversation the host has with a plugin, and nothing in the audio
// proves it took place: a plugin that ignores expressions sounds the same as one never sent any.
// So this fixture *listens* instead — note-ons, note-offs and note expressions, each with the
// sample it landed on counted from the plugin's first processed frame — and hands the log back
// through the state extension, the one channel a CLAP plugin has for giving the host bytes. The
// test reads it with `HostedPlugin::save_state` on its own thread while the engine owns the
// audio half, exactly the split the app uses.
//
// It reports `Continue` from every block so the mixer never skips it once it has woken: a skipped
// block would put the fixture's frame count out of step with the engine's.
//
// `include!`d into a `mod` by its entry point and by the test, so it carries neither a `mod`
// nor inner attributes of its own.

use std::sync::Mutex;

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts, PluginAudioPortsImpl,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::event_types::{NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent};
use clack_plugin::events::Match;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};

pub const PLUGIN_ID: &str = "org.dissonant.test.note-recorder";
pub const PLUGIN_NAME: &str = "Dissonant Test Note Recorder";
pub const PLUGIN_VENDOR: &str = "Dissonant";
pub const PLUGIN_VERSION: &str = "0.1.0";

/// What a received event was. Expressions other than tuning are recorded too, so a host that
/// sent the wrong expression id fails a test rather than vanishing from the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NoteOn,
    NoteOff,
    Tuning,
    OtherExpression,
}

/// One received event. `key` and `note_id` are CLAP's raw values, -1 for a wildcard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Received {
    pub at: u64,
    pub kind: Kind,
    pub key: i16,
    pub note_id: i32,
    pub value: f64,
}

const RECORD_BYTES: usize = 8 + 1 + 2 + 4 + 8;

impl Received {
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.at.to_le_bytes());
        out.push(match self.kind {
            Kind::NoteOn => 0,
            Kind::NoteOff => 1,
            Kind::Tuning => 2,
            Kind::OtherExpression => 3,
        });
        out.extend_from_slice(&self.key.to_le_bytes());
        out.extend_from_slice(&self.note_id.to_le_bytes());
        out.extend_from_slice(&self.value.to_le_bytes());
    }
}

/// The log back out of a saved state blob.
pub fn decode(blob: &[u8]) -> Vec<Received> {
    blob.chunks_exact(RECORD_BYTES)
        .map(|r| Received {
            at: u64::from_le_bytes(r[0..8].try_into().unwrap()),
            kind: match r[8] {
                0 => Kind::NoteOn,
                1 => Kind::NoteOff,
                2 => Kind::Tuning,
                _ => Kind::OtherExpression,
            },
            key: i16::from_le_bytes(r[9..11].try_into().unwrap()),
            note_id: i32::from_le_bytes(r[11..15].try_into().unwrap()),
            value: f64::from_le_bytes(r[15..23].try_into().unwrap()),
        })
        .collect()
}

fn raw<T: Copy + Into<i64>>(m: Match<T>) -> i64 {
    match m {
        Match::Specific(v) => v.into(),
        Match::All => -1,
    }
}

// ─── shared ──────────────────────────────────────────────────────────────────────────────────

/// The log, written by the audio thread and read by the main thread's state save. A lock is
/// fine in a fixture: nothing here has a deadline to miss.
pub struct RecorderShared {
    log: Mutex<Vec<Received>>,
}

impl PluginShared<'_> for RecorderShared {}

// ─── main thread ─────────────────────────────────────────────────────────────────────────────

pub struct RecorderMainThread<'a> {
    shared: &'a RecorderShared,
}

impl<'a> PluginMainThread<'a, RecorderShared> for RecorderMainThread<'a> {}

impl PluginAudioPortsImpl for RecorderMainThread<'_> {
    fn count(&self, is_input: bool) -> u32 {
        u32::from(!is_input)
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if is_input || index != 0 {
            return;
        }
        writer.set(&AudioPortInfo {
            id: ClapId::new(0),
            name: b"Stereo Out",
            channel_count: 2,
            flags: AudioPortFlags::IS_MAIN,
            port_type: Some(AudioPortType::STEREO),
            in_place_pair: None,
        });
    }
}

impl PluginStateImpl for RecorderMainThread<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        use std::io::Write;
        let log = self.shared.log.lock().map_err(|_| PluginError::Message("log poisoned"))?;
        let mut blob = Vec::with_capacity(log.len() * RECORD_BYTES);
        for r in log.iter() {
            r.write(&mut blob);
        }
        output.write_all(&blob).map_err(|_| PluginError::Message("state write failed"))
    }

    fn load(&self, _input: &mut InputStream) -> Result<(), PluginError> {
        // A recorder has nothing to restore; the log belongs to the run that produced it.
        Ok(())
    }
}

// ─── audio thread ────────────────────────────────────────────────────────────────────────────

pub struct RecorderAudio<'a> {
    shared: &'a RecorderShared,
    /// Frames processed so far: the clock every recorded event is placed on.
    frames: u64,
}

impl<'a> PluginAudioProcessor<'a, RecorderShared, RecorderMainThread<'a>> for RecorderAudio<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &RecorderMainThread<'a>,
        shared: &'a RecorderShared,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self { shared, frames: 0 })
    }

    fn process(&mut self, _process: Process, mut audio: Audio, events: Events) -> Result<ProcessStatus, PluginError> {
        if let Ok(mut log) = self.shared.log.lock() {
            for event in events.input {
                let at = self.frames + u64::from(event.header().time());
                let received = if let Some(e) = event.as_event::<NoteOnEvent>() {
                    Some((Kind::NoteOn, raw(e.key()), raw(e.note_id()), e.velocity()))
                } else if let Some(e) = event.as_event::<NoteOffEvent>() {
                    Some((Kind::NoteOff, raw(e.key()), raw(e.note_id()), e.velocity()))
                } else if let Some(e) = event.as_event::<NoteExpressionEvent>() {
                    let kind = if e.expression_type() == Some(NoteExpressionType::Tuning) {
                        Kind::Tuning
                    } else {
                        Kind::OtherExpression
                    };
                    Some((kind, raw(e.key()), raw(e.note_id()), e.value()))
                } else {
                    None
                };
                if let Some((kind, key, note_id, value)) = received {
                    log.push(Received {
                        at,
                        kind,
                        key: key as i16,
                        note_id: note_id as i32,
                        value,
                    });
                }
            }
        }

        let frames = audio.frames_count();
        if let Some(mut port) = audio.output_port(0) {
            if let Some(mut channels) = port.channels()?.into_f32() {
                for channel in channels.iter_mut() {
                    channel.fill(0.0);
                }
            }
        }
        self.frames += u64::from(frames);
        Ok(ProcessStatus::Continue)
    }
}

// ─── plugin ──────────────────────────────────────────────────────────────────────────────────

pub struct NoteRecorderPlugin;

impl Plugin for NoteRecorderPlugin {
    type AudioProcessor<'a> = RecorderAudio<'a>;
    type Shared<'a> = RecorderShared;
    type MainThread<'a> = RecorderMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&RecorderShared>) {
        builder.register::<PluginAudioPorts>().register::<PluginState>();
    }
}

impl DefaultPluginFactory for NoteRecorderPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;

        PluginDescriptor::new(PLUGIN_ID, PLUGIN_NAME)
            .with_vendor(PLUGIN_VENDOR)
            .with_version(PLUGIN_VERSION)
            .with_description("Records every note event it receives, for testing Dissonant's CLAP host.")
            .with_features([INSTRUMENT, STEREO])
    }

    fn new_shared(_host: HostSharedHandle<'_>) -> Result<RecorderShared, PluginError> {
        Ok(RecorderShared {
            log: Mutex::new(Vec::with_capacity(4096)),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a RecorderShared,
    ) -> Result<RecorderMainThread<'a>, PluginError> {
        Ok(RecorderMainThread { shared })
    }
}

pub type NoteRecorderEntry = SinglePluginEntry<NoteRecorderPlugin>;
