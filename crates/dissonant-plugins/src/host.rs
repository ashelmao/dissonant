//! Running a CLAP plugin: one handle for the app thread, one for the audio thread.
//!
//! The split into [`HostedPlugin`] and [`PluginAudioUnit`] is not a stylistic choice, it is forced
//! by CLAP's thread model and by clack's encoding of it. A `clack_host::PluginInstance` is
//! deliberately **not** `Send`: the plugin's `[main-thread]` callbacks (parameter queries, state
//! save/load, GUI) may only ever run on the thread that created the instance. Its audio processor
//! *is* `Send`, and that is the only half the realtime thread is allowed to touch.
//!
//! So [`HostedPlugin`] stays with the app: it owns the instance, the parameter list and the state
//! blob. [`PluginAudioUnit`] goes to the mixer: it owns the processor and every buffer it will ever
//! need. Loading, activation and all allocation happen in [`HostedPlugin::open`], on the app
//! thread, because dlopen-ing a plugin takes hundreds of milliseconds and would blow the audio
//! deadline many times over.
//!
//! Dropping order matters: drop the [`PluginAudioUnit`] before the [`HostedPlugin`] it came from.
//! clack refuses to destroy an instance whose audio processor is still alive, and the instance
//! would leak instead. [`HostedPlugin`]'s destructor logs loudly if it is dropped the wrong way
//! round.

use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::{PluginState, StateError};
use clack_host::entry::PluginEntryError;
use clack_host::events::event_types::{
    NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent, ParamValueEvent,
};
use clack_host::events::io::EventBuffer;
use clack_host::events::{Match, Pckn};
use clack_host::prelude::*;
use clack_host::process::audio_buffers::{
    AudioPortBuffer, AudioPortBufferType, AudioPorts, InputAudioBuffers, InputChannel,
};
use serde::{Deserialize, Serialize};

use crate::scan::load_entry;
use crate::{PluginDescriptor, PluginKind};

/// Stereo is what the mixer speaks, and what a plugin with no audio-ports extension is assumed to
/// produce.
const DEFAULT_OUTPUT_CHANNELS: usize = 2;

/// Headroom for the events queued between two render calls, and the hard ceiling on tuning
/// expressions within one.
///
/// Notes and parameters were the whole budget once, and 256 covered a dense bar of chords plus
/// automation. Per-note tuning is what sized it up: the engine follows at most 16 bent notes
/// per plugin and sends each one a value at most every 64 samples while it moves, plus one at
/// each envelope point it lands on, so a 2048-frame block of sixteen notes all bending peaks at
/// roughly 800. 1024 holds that with the old 256 still spare for notes and parameters. Beyond
/// it, [`PluginAudioUnit::note_tuning`] refuses rather than letting the queue reach its growth
/// path, which would allocate on the audio thread; a refused value is simply sent a little later.
const EVENT_CAPACITY: usize = 1024;

/// CLAP's tuning expression spans ±120 semitones; anything wider is not a value a plugin has
/// agreed to accept.
const MAX_TUNING_SEMITONES: f64 = 120.0;

/// Where every note id comes from, shared by every unit in the process.
///
/// Per-unit counters would hand the same id to two different notes the moment a track swaps one
/// plugin for another, and an expression still addressed to the old note would land on the new
/// one. A process-wide counter makes ids unique for as long as anyone could be holding one; the
/// top bit is masked off because CLAP reserves negative ids for "unspecified".
static NEXT_NOTE_ID: AtomicU32 = AtomicU32::new(0);

fn next_note_id() -> u32 {
    NEXT_NOTE_ID.fetch_add(1, Ordering::Relaxed) & i32::MAX as u32
}

// ─── parameters ──────────────────────────────────────────────────────────────────────────────

/// One automatable parameter of a hosted plugin, as the UI needs to draw it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamInfo {
    /// The plugin's own stable id, which is what automation and project files store.
    pub id: u32,
    pub name: String,
    /// The plugin's grouping path for this parameter, e.g. `"Oscillators/Wavetable 1"`. Often
    /// empty.
    pub module: String,
    pub min: f64,
    pub max: f64,
    /// What the parameter resets to, i.e. where double-click-to-reset snaps a knob.
    pub default: f64,
    /// What the parameter is set to right now. Re-read on every [`HostedPlugin::params`] call.
    pub value: f64,
    /// The parameter only takes integral values; a knob should snap and a readout should not show
    /// decimals.
    pub is_stepped: bool,
}

// ─── errors ──────────────────────────────────────────────────────────────────────────────────

/// Everything that can go wrong between picking a plugin and hearing it.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("could not load {path}: {source}")]
    Load {
        path: PathBuf,
        #[source]
        source: PluginEntryError,
    },

    #[error("plugin id {0:?} is not usable as a C string")]
    InvalidPluginId(String),

    #[error("could not instantiate {plugin_id} from {path}: {source}")]
    Instantiate {
        plugin_id: String,
        path: PathBuf,
        #[source]
        source: PluginInstanceError,
    },

    #[error("{plugin_id} refused to activate at {sample_rate} Hz with {max_frames} frames: {source}")]
    Activate {
        plugin_id: String,
        sample_rate: f64,
        max_frames: u32,
        #[source]
        source: PluginInstanceError,
    },

    #[error("{0} does not implement the CLAP state extension, so its settings cannot be saved")]
    NoStateExtension(String),

    #[error("{plugin_id} failed to transfer its state: {source}")]
    State {
        plugin_id: String,
        #[source]
        source: StateError,
    },
}

// ─── host callbacks ──────────────────────────────────────────────────────────────────────────

/// The callback surface this host exposes to plugins.
///
/// No host-side extensions are declared. Every CLAP host extension is optional, and declaring one
/// obliges the host to honour it; Dissonant honours none of `clap_host_params`,
/// `clap_host_state` or the port-rescan calls today, so claiming them would be a lie a plugin
/// could act on.
struct DissonantHost;

#[derive(Default)]
struct HostShared {
    /// Set when the plugin asks for `on_main_thread()`. Drained by the main-thread entry points on
    /// [`HostedPlugin`], which are the only places this host is known to be on that thread.
    callback_requested: AtomicBool,
}

impl SharedHandler<'_> for HostShared {
    fn request_restart(&self) {
        // Honouring this means deactivating, which needs the audio unit back from the mixer.
        // Dissonant keeps a plugin at one fixed sample rate and block size for its whole life, so
        // the request is recorded for diagnosis and otherwise declined.
        log::debug!("plugin requested a restart; Dissonant does not re-activate plugins in place");
    }

    fn request_process(&self) {
        // The mixer decides when a channel renders. A plugin that wants to be woken while idle is
        // covered by `PluginAudioUnit::is_silent` returning false as soon as anything is queued.
        log::debug!("plugin requested processing");
    }

    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::Release);
    }
}

impl HostHandlers for DissonantHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

fn host_info() -> HostInfo {
    // PANIC: all four strings are compile-time constants with no interior NUL.
    HostInfo::new(
        "Dissonant",
        "Dissonant",
        "https://github.com/lilylilylily123/dissonant",
        env!("CARGO_PKG_VERSION"),
    )
    .expect("host info is constant and NUL-free")
}

// ─── app thread ──────────────────────────────────────────────────────────────────────────────

/// A loaded plugin, living on the thread that loaded it.
///
/// See the [module docs](self) for why this is separate from [`PluginAudioUnit`], and for the
/// drop-order rule.
pub struct HostedPlugin {
    descriptor: PluginDescriptor,
    instance: PluginInstance<DissonantHost>,
    params_ext: Option<PluginParams>,
    state_ext: Option<PluginState>,
    params: Vec<ParamInfo>,
}

impl HostedPlugin {
    /// Loads, instantiates and activates `desc`, returning the app-thread handle and the audio
    /// unit to hand to the mixer.
    ///
    /// Every allocation the audio thread will need is made here: event buffers, port tables and
    /// the scratch audio buffer are all sized for `max_frames` and never grow afterwards.
    ///
    /// `max_frames` is the largest block the mixer will ever ask for. Asking for more is still
    /// correct — [`PluginAudioUnit::render_add`] splits the request — but costs an extra `process`
    /// call per block.
    pub fn open(
        desc: &PluginDescriptor,
        sample_rate: f64,
        max_frames: u32,
    ) -> Result<(HostedPlugin, PluginAudioUnit), PluginError> {
        // A zero-frame configuration is rejected by CLAP, and a zero-size buffer would make every
        // render a no-op rather than an audible failure.
        let max_frames = max_frames.max(1);

        let entry = load_entry(&desc.path).map_err(|source| PluginError::Load {
            path: desc.path.clone(),
            source,
        })?;

        let plugin_id = CString::new(desc.plugin_id.as_str())
            .map_err(|_| PluginError::InvalidPluginId(desc.plugin_id.clone()))?;

        let mut instance = PluginInstance::<DissonantHost>::new(
            |_| HostShared::default(),
            |_| (),
            &entry,
            &plugin_id,
            &host_info(),
        )
        .map_err(|source| PluginError::Instantiate {
            plugin_id: desc.plugin_id.clone(),
            path: desc.path.clone(),
            source,
        })?;

        let shared = instance.plugin_shared_handle();
        let params_ext = shared.get_extension::<PluginParams>();
        let state_ext = shared.get_extension::<PluginState>();
        let ports_ext = shared.get_extension::<PluginAudioPorts>();

        let (in_channels, out_channels) = port_channel_counts(&mut instance, ports_ext, desc.kind);
        let params = read_params(&mut instance, params_ext);

        let processor = instance
            .activate(
                |_, _| (),
                PluginAudioConfiguration {
                    sample_rate,
                    min_frames_count: 1,
                    max_frames_count: max_frames,
                },
            )
            .map_err(|source| PluginError::Activate {
                plugin_id: desc.plugin_id.clone(),
                sample_rate,
                max_frames,
                source,
            })?;

        let max_frames = max_frames as usize;
        let unit = PluginAudioUnit {
            processor: processor.into(),
            kind: desc.kind,
            queue: EventQueue::new(),
            input_events: EventBuffer::with_capacity(EVENT_CAPACITY),
            output_events: EventBuffer::with_capacity(EVENT_CAPACITY),
            in_ports: AudioPorts::with_capacity(in_channels.max(1), 1),
            out_ports: AudioPorts::with_capacity(out_channels, 1),
            input: vec![0.0; in_channels * max_frames],
            scratch: vec![0.0; out_channels * max_frames],
            in_channels,
            out_channels,
            max_frames,
            status: ProcessStatus::Sleep,
        };

        Ok((
            HostedPlugin {
                descriptor: desc.clone(),
                instance,
                params_ext,
                state_ext,
                params,
            },
            unit,
        ))
    }

    /// What this plugin was opened from, including the `uid` a project file stores.
    pub fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    /// The plugin's parameters, with [`ParamInfo::value`] refreshed from the plugin.
    ///
    /// Only `value` is re-read on each call; everything else is cached from
    /// [`open`](Self::open), because a plugin may not change a parameter's identity, range or
    /// default without being deactivated first. Refreshing matters after
    /// [`load_state`](Self::load_state) or after the user has moved a knob in the plugin's own
    /// window: the cached defaults would otherwise be displayed as if they were the current sound.
    ///
    /// A plugin with no parameters extension returns an empty slice. That is an ordinary plugin,
    /// not a failure.
    pub fn params(&mut self) -> &[ParamInfo] {
        self.service_callbacks();

        if let Some(ext) = self.params_ext {
            let handle = self.instance.plugin_handle();
            for param in &mut self.params {
                if let Some(id) = ClapId::from_raw(param.id) {
                    if let Some(value) = ext.get_value(&handle, id) {
                        param.value = value;
                    }
                }
            }
        }

        &self.params
    }

    /// The plugin's own opaque settings blob, to be stored in the project file.
    pub fn save_state(&mut self) -> Result<Vec<u8>, PluginError> {
        self.service_callbacks();

        let ext = self
            .state_ext
            .ok_or_else(|| PluginError::NoStateExtension(self.descriptor.name.clone()))?;

        let mut bytes = Vec::new();
        let handle = self.instance.plugin_handle();
        ext.save(&handle, &mut bytes)
            .map_err(|source| PluginError::State {
                plugin_id: self.descriptor.plugin_id.clone(),
                source,
            })?;
        Ok(bytes)
    }

    /// Restores a blob previously produced by [`save_state`](Self::save_state).
    ///
    /// Parameter values move with the state, so a following [`params`](Self::params) call reports
    /// the restored values.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), PluginError> {
        self.service_callbacks();

        let ext = self
            .state_ext
            .ok_or_else(|| PluginError::NoStateExtension(self.descriptor.name.clone()))?;

        let mut reader = std::io::Cursor::new(bytes);
        let handle = self.instance.plugin_handle();
        ext.load(&handle, &mut reader)
            .map_err(|source| PluginError::State {
                plugin_id: self.descriptor.plugin_id.clone(),
                source,
            })
    }

    /// Runs the plugin's `on_main_thread` callback if it asked for one.
    ///
    /// Plugins use that callback to finish work they started on another thread, so it is drained
    /// at the top of every main-thread entry point rather than on a timer.
    fn service_callbacks(&mut self) {
        let requested = self
            .instance
            .access_shared_handler(|shared| shared.callback_requested.swap(false, Ordering::AcqRel));
        if requested {
            self.instance.call_on_main_thread_callback();
        }
    }
}

impl Drop for HostedPlugin {
    fn drop(&mut self) {
        // Deactivating returns the plugin's processing resources before the instance itself is
        // destroyed. It only succeeds once the audio unit is gone; see the module docs.
        if self.instance.try_deactivate().is_err() {
            log::warn!(
                "{} was dropped while its audio unit was still alive; the plugin instance will leak",
                self.descriptor.name
            );
        }
    }
}

/// Channel counts of the plugin's main input and output ports.
///
/// A plugin without the audio-ports extension has no ports at all by the CLAP spec, but hosts that
/// take that literally break a good number of real plugins, so ports are assumed from what the
/// plugin is for: an instrument gets an output only, an effect gets a stereo pair on both sides,
/// since an effect with no input to process would be pointless.
///
/// The two sides are read independently, so a plugin whose layouts differ — mono in, stereo out,
/// or the reverse — is described as it really is; [`PluginAudioUnit`] converts between that and
/// the mixer's stereo.
fn port_channel_counts(
    instance: &mut PluginInstance<DissonantHost>,
    ports: Option<PluginAudioPorts>,
    kind: PluginKind,
) -> (usize, usize) {
    let Some(ports) = ports else {
        return match kind {
            PluginKind::Instrument => (0, DEFAULT_OUTPUT_CHANNELS),
            PluginKind::Effect => (DEFAULT_OUTPUT_CHANNELS, DEFAULT_OUTPUT_CHANNELS),
        };
    };

    let handle = instance.plugin_handle();
    let mut buffer = AudioPortInfoBuffer::new();

    let channels = |is_input: bool, buffer: &mut AudioPortInfoBuffer| -> usize {
        if ports.count(&handle, is_input) == 0 {
            return 0;
        }
        ports
            .get(&handle, 0, is_input, buffer)
            .map(|info| info.channel_count as usize)
            .unwrap_or(0)
    };

    let in_channels = channels(true, &mut buffer);
    let out_channels = channels(false, &mut buffer).max(1);
    (in_channels, out_channels)
}

/// Reads the full parameter list once, at open time.
fn read_params(
    instance: &mut PluginInstance<DissonantHost>,
    params: Option<PluginParams>,
) -> Vec<ParamInfo> {
    let Some(params) = params else {
        return Vec::new();
    };

    let handle = instance.plugin_handle();
    let count = params.count(&handle);
    let mut buffer = ParamInfoBuffer::new();
    let mut out = Vec::with_capacity(count as usize);

    for index in 0..count {
        let Some(info) = params.get_info(&handle, index, &mut buffer) else {
            // A plugin that reports N parameters but cannot describe one of them is misbehaving;
            // the rest of its list is still usable.
            log::warn!("plugin parameter {index} could not be read");
            continue;
        };

        out.push(ParamInfo {
            id: info.id.get(),
            name: String::from_utf8_lossy(info.name).into_owned(),
            module: String::from_utf8_lossy(info.module).into_owned(),
            min: info.min_value,
            max: info.max_value,
            default: info.default_value,
            value: params.get_value(&handle, info.id).unwrap_or(info.default_value),
            is_stepped: info.flags.contains(ParamInfoFlags::IS_STEPPED),
        });
    }

    out
}

// ─── audio thread ────────────────────────────────────────────────────────────────────────────

/// The realtime half of a hosted plugin: the processor plus every buffer it needs.
///
/// `Send`, so it can be handed to the audio thread, and allocation-free once
/// [`HostedPlugin::open`] has returned.
pub struct PluginAudioUnit {
    processor: PluginAudioProcessor<DissonantHost>,
    /// What the plugin is for, which decides how it is fed and when it may be skipped.
    kind: PluginKind,
    /// Notes, parameter changes and expressions queued since the last render, with the held
    /// keys and the note id each one is sounding under.
    queue: EventQueue,
    /// The slice of `queue` one `process` call sees, re-stamped relative to its own first frame.
    input_events: EventBuffer,
    /// Where the plugin writes events back. Nothing upstream consumes them, so it is emptied every
    /// `process` call to keep it from growing.
    output_events: EventBuffer,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    /// De-interleaved plugin input, `in_channels` blocks of `max_frames`. It stays permanently
    /// zero for an instrument — which is fed silence on any input port it insists on declaring —
    /// and is refilled from the caller's buffers by every
    /// [`process_replace`](PluginAudioUnit::process_replace) chunk.
    input: Vec<f32>,
    /// De-interleaved plugin output, `out_channels` blocks of `max_frames`.
    scratch: Vec<f32>,
    in_channels: usize,
    out_channels: usize,
    max_frames: usize,
    status: ProcessStatus,
}

/// What the plugin's input port carries for one `process` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortInput {
    /// Silence, flagged constant so a plugin is free to skip reading it at all.
    Silence,
    /// The caller's audio, already copied into `input` by `fill_input`.
    Audio,
}

impl PluginAudioUnit {
    /// Queues a note-on at the start of the next rendered block and returns the note id it
    /// sounds under.
    ///
    /// Every note gets a real id rather than CLAP's "unspecified" -1: per-note expressions are
    /// addressed by it, and a plugin that tracks voices by id needs the note-off to name the
    /// same one.
    pub fn note_on(&mut self, pitch: u8, velocity: u8) -> u32 {
        self.queue.note_on(pitch, f64::from(velocity) / 127.0)
    }

    /// Queues a note-off at the start of the next rendered block, addressed to the id the key
    /// was struck under.
    pub fn note_off(&mut self, pitch: u8) {
        self.queue.note_off(pitch);
    }

    /// Releases every held note with a single wildcard note-off, as CLAP defines it.
    pub fn all_notes_off(&mut self) {
        self.queue.all_notes_off();
    }

    /// Queues a parameter change. Ignores `id`s CLAP cannot represent.
    pub fn set_param(&mut self, id: u32, value: f64) {
        let Some(id) = ClapId::from_raw(id) else {
            return;
        };
        self.queue.push(0, Queued::Param { id, value });
    }

    /// Whether `pitch` is still held under `note_id`: struck by [`note_on`](Self::note_on) and
    /// neither released nor struck again since.
    pub fn holds(&self, note_id: u32, pitch: u8) -> bool {
        self.queue.holds(note_id, pitch)
    }

    /// Queues a `CLAP_NOTE_EXPRESSION_TUNING` for one held note, `at` samples into the next
    /// render call, retuning it by `semitones` from its key — an absolute statement, not a step
    /// on top of the last one, and clamped to CLAP's ±120.
    ///
    /// Returns false, queueing nothing, when the note is no longer held under that id or the
    /// queue is at [`EVENT_CAPACITY`]; the caller keeps the value and offers it again later.
    /// A plugin without note-expression support ignores the event, which leaves it exactly
    /// where it was before expressions were sent at all.
    pub fn note_tuning(&mut self, at: u32, note_id: u32, pitch: u8, semitones: f64) -> bool {
        self.queue.tuning(at, note_id, pitch, semitones)
    }

    /// Whether the mixer can skip this unit entirely.
    ///
    /// For an instrument: true only when the plugin itself asked to be put to sleep, no note is
    /// held, and nothing is waiting to be delivered to it. Any other `ProcessStatus` — including
    /// a release tail — keeps the channel awake.
    ///
    /// For an effect: true only when the plugin asked to be put to sleep, whatever is queued for
    /// it. An effect is driven by its input rather than by notes, so the answer is "has the tail
    /// finished", and a reverb reporting `Tail` or `Continue` must keep being processed after its
    /// input has gone quiet. It is **not** a licence to bypass an effect whose input is sounding:
    /// the chain only consults this once the signal reaching the slot is already silent.
    pub fn is_silent(&self) -> bool {
        match self.kind {
            PluginKind::Instrument => {
                self.queue.is_empty()
                    && !self.queue.any_held()
                    && self.status == ProcessStatus::Sleep
            }
            PluginKind::Effect => self.status == ProcessStatus::Sleep,
        }
    }

    /// Renders the plugin and **adds** its stereo output into `l` and `r`.
    ///
    /// This is the instrument path: the plugin is fed silence on any input port it declares.
    ///
    /// Requests longer than the `max_frames` this unit was opened with are split into several
    /// `process` calls, so nothing here allocates or resizes; each call is handed the queued
    /// events that fall inside it, re-stamped against its own first frame. A plugin whose main
    /// output is mono is added to both sides; extra channels beyond the first two are ignored,
    /// which is what the stereo mixer can carry.
    pub fn render_add(&mut self, l: &mut [f32], r: &mut [f32]) {
        let frames = l.len().min(r.len());
        let mut done = 0;
        let mut next_event = 0;
        self.queue.sort();

        while done < frames {
            let chunk = (frames - done).min(self.max_frames);
            self.queue
                .deliver(&mut next_event, done, chunk, done + chunk == frames, &mut self.input_events);
            self.process_chunk(chunk, PortInput::Silence);

            let left = &self.scratch[..chunk];
            let right = if self.out_channels >= 2 {
                &self.scratch[self.max_frames..self.max_frames + chunk]
            } else {
                left
            };
            for i in 0..chunk {
                l[done + i] += left[i];
                r[done + i] += right[i];
            }

            done += chunk;
        }

        // A render drains the queue whether or not every event found a frame to land on.
        self.queue.clear();
        self.input_events.clear();
    }

    /// Runs the plugin over `l` and `r` and writes its output back over them.
    ///
    /// This is the effect path, and it **replaces**: an insert hears the signal reaching it and
    /// decides entirely what leaves the slot, wet/dry included. The caller's buffers are copied
    /// into this unit's own input before the plugin runs and overwritten from its output
    /// afterwards, so the plugin never sees the mixer's memory and `l` and `r` may be any two
    /// buffers the caller likes, including ones it reuses for the rest of the chain.
    ///
    /// Blocks longer than `max_frames` are split into several `process` calls, exactly as in
    /// [`render_add`](Self::render_add), and nothing here allocates. A plugin with a mono input
    /// hears the mean of the two sides; one with a mono output is written to both; channels
    /// beyond the first two are fed silence and ignored respectively, which is what the stereo
    /// mixer can carry. A plugin that declares no input at all — rare, but legal for an effect —
    /// still has its output written over the caller's buffers.
    pub fn process_replace(&mut self, l: &mut [f32], r: &mut [f32]) {
        let frames = l.len().min(r.len());
        let mut done = 0;
        let mut next_event = 0;
        self.queue.sort();

        while done < frames {
            let chunk = (frames - done).min(self.max_frames);
            self.fill_input(&l[done..done + chunk], &r[done..done + chunk]);
            self.queue
                .deliver(&mut next_event, done, chunk, done + chunk == frames, &mut self.input_events);
            self.process_chunk(chunk, PortInput::Audio);

            let left = &self.scratch[..chunk];
            let right = if self.out_channels >= 2 {
                &self.scratch[self.max_frames..self.max_frames + chunk]
            } else {
                left
            };
            l[done..done + chunk].copy_from_slice(left);
            r[done..done + chunk].copy_from_slice(right);

            done += chunk;
        }

        self.queue.clear();
        self.input_events.clear();
    }

    /// Copies one chunk of the caller's stereo pair into the plugin's input buffer.
    ///
    /// Channels past the second keep the zeros they were opened with: the mixer has nothing to
    /// put there, and silence is the only honest filler.
    fn fill_input(&mut self, l: &[f32], r: &[f32]) {
        let frames = l.len();
        match self.in_channels {
            0 => {}
            // Mono in: the centre of the pair, not an arbitrary side, so nothing is lost when the
            // two differ.
            1 => {
                let mono = &mut self.input[..frames];
                for (sample, (left, right)) in mono.iter_mut().zip(l.iter().zip(r)) {
                    *sample = 0.5 * (left + right);
                }
            }
            _ => {
                self.input[..frames].copy_from_slice(l);
                self.input[self.max_frames..self.max_frames + frames].copy_from_slice(r);
            }
        }
    }

    /// One `process` call of at most `max_frames`, leaving the result in `scratch`.
    fn process_chunk(&mut self, frames: usize, port_input: PortInput) {
        let Self {
            processor,
            input_events,
            output_events,
            in_ports,
            out_ports,
            input,
            scratch,
            in_channels,
            max_frames,
            ..
        } = self;

        // A plugin is free to return without writing anything — one that reports `Sleep` usually
        // does — so the samples this chunk is about to read have to start at zero rather than hold
        // whatever the previous chunk left there. Only the frames in play are touched.
        for channel in scratch.chunks_mut(*max_frames) {
            channel[..frames].fill(0.0);
        }
        output_events.clear();

        let mut inputs = if *in_channels == 0 {
            InputAudioBuffers::empty()
        } else {
            in_ports.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(input.chunks_mut(*max_frames).map(
                    |channel| match port_input {
                        PortInput::Silence => InputChannel::constant(channel),
                        PortInput::Audio => InputChannel::variable(channel),
                    },
                )),
            }])
        };
        let mut outputs = out_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(scratch.chunks_mut(*max_frames)),
        }]);

        // The buffers are sized for max_frames; a shorter chunk is expressed by narrowing the
        // frame count rather than by re-slicing, which would need a fresh port table.
        let frames = frames as u32;
        inputs.truncate(frames);
        outputs.truncate(frames);

        let events = input_events.as_input();
        let mut out_events = output_events.as_output();

        let status = match processor.ensure_processing_started() {
            Ok(started) => started.process(&inputs, &mut outputs, &events, &mut out_events, None, None),
            Err(err) => Err(err),
        };

        self.status = match status {
            Ok(status) => status,
            Err(err) => {
                // `scratch` is already zeroed, so a failed block is silence rather than noise.
                log::warn!("plugin process failed: {err}");
                ProcessStatus::Sleep
            }
        };
    }
}

// ─── event queue ─────────────────────────────────────────────────────────────────────────────

/// One event waiting for the next render, in this host's own terms. Kept as plain data rather
/// than as CLAP events so a render split into several `process` calls can re-stamp each one
/// against the call it lands in; clack's buffer stores events opaquely and cannot be edited.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Queued {
    NoteOn {
        key: u8,
        id: u32,
        velocity: f64,
    },
    /// `id` is `None` when the key was struck again while still held: two notes then share
    /// it, and only a wildcard id releases both, which is what the note-off has always done.
    NoteOff {
        key: u8,
        id: Option<u32>,
    },
    AllNotesOff,
    Param {
        id: ClapId,
        value: f64,
    },
    Tuning {
        key: u8,
        id: u32,
        semitones: f64,
    },
}

/// A queued event and where it lands: `time` in samples from the start of the next render call,
/// `seq` its insertion order, so events sharing a sample stay in the order they were queued — a
/// note's first tuning has to reach the plugin after the note-on it addresses.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Timed {
    time: u32,
    seq: u32,
    event: Queued,
}

/// Everything a [`PluginAudioUnit`] owes its plugin before the next render, and which note id
/// each held key is sounding under.
struct EventQueue {
    /// Pre-sized to [`EVENT_CAPACITY`] and drained by every render.
    events: Vec<Timed>,
    /// One bit per MIDI key currently held down.
    held: u128,
    /// Keys struck again while already held; see [`Queued::NoteOff`].
    restruck: u128,
    /// The id of the most recent note on each key. Meaningful only while the key's `held` bit
    /// is set.
    ids: [u32; 128],
}

impl EventQueue {
    fn new() -> Self {
        EventQueue {
            events: Vec::with_capacity(EVENT_CAPACITY),
            held: 0,
            restruck: 0,
            ids: [0; 128],
        }
    }

    fn note_on(&mut self, key: u8, velocity: f64) -> u32 {
        let id = next_note_id();
        if key < 128 {
            let bit = 1u128 << key;
            if self.held & bit != 0 {
                self.restruck |= bit;
            }
            self.held |= bit;
            self.ids[key as usize] = id;
        }
        self.push(0, Queued::NoteOn { key, id, velocity });
        id
    }

    fn note_off(&mut self, key: u8) {
        let mut id = None;
        if key < 128 {
            let bit = 1u128 << key;
            if self.held & bit != 0 && self.restruck & bit == 0 {
                id = Some(self.ids[key as usize]);
            }
            self.held &= !bit;
            self.restruck &= !bit;
        }
        self.push(0, Queued::NoteOff { key, id });
    }

    fn all_notes_off(&mut self) {
        self.push(0, Queued::AllNotesOff);
        self.held = 0;
        self.restruck = 0;
    }

    fn holds(&self, id: u32, key: u8) -> bool {
        key < 128 && self.held & (1u128 << key) != 0 && self.ids[key as usize] == id
    }

    fn any_held(&self) -> bool {
        self.held != 0
    }

    fn tuning(&mut self, at: u32, id: u32, key: u8, semitones: f64) -> bool {
        if !self.holds(id, key) || self.events.len() >= EVENT_CAPACITY {
            return false;
        }
        let semitones = semitones.clamp(-MAX_TUNING_SEMITONES, MAX_TUNING_SEMITONES);
        self.push(at, Queued::Tuning { key, id, semitones });
        true
    }

    fn push(&mut self, time: u32, event: Queued) {
        let seq = self.events.len() as u32;
        self.events.push(Timed { time, seq, event });
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    fn clear(&mut self) {
        self.events.clear();
    }

    /// CLAP requires a `process` call's events in time order. Unstable on purpose — the std
    /// stable sort may allocate — with `seq` breaking ties so it behaves as a stable one.
    fn sort(&mut self) {
        self.events.sort_unstable_by_key(|e| (e.time, e.seq));
    }

    /// Writes into `out` the sorted events landing in the `len` frames that start `from` frames
    /// into the render, stamped relative to `from`, and advances `next` past them. The `last`
    /// call of a render also takes anything stamped past its end, on its final frame, rather
    /// than dropping a note-off on the floor.
    fn deliver(&self, next: &mut usize, from: usize, len: usize, last: bool, out: &mut EventBuffer) {
        out.clear();
        while let Some(e) = self.events.get(*next) {
            let t = e.time as usize;
            if !last && t >= from + len {
                break;
            }
            let time = (t.saturating_sub(from)).min(len.saturating_sub(1)) as u32;
            match e.event {
                Queued::NoteOn { key, id, velocity } => {
                    out.push(&NoteOnEvent::new(time, pckn(key, Some(id)), velocity))
                }
                Queued::NoteOff { key, id } => out.push(&NoteOffEvent::new(time, pckn(key, id), 0.0)),
                Queued::AllNotesOff => out.push(&NoteOffEvent::new(time, Pckn::match_all(), 0.0)),
                Queued::Param { id, value } => {
                    out.push(&ParamValueEvent::new(time, id, Pckn::match_all(), value))
                }
                Queued::Tuning { key, id, semitones } => out.push(&NoteExpressionEvent::new(
                    time,
                    pckn(key, Some(id)),
                    NoteExpressionType::Tuning,
                    semitones,
                )),
            }
            *next += 1;
        }
    }
}

/// Targets one key on the first note port and first channel, and one note on it when `note_id`
/// is known. The engine speaks a single MIDI-shaped port; the id is what lets a per-note
/// expression reach the voice it belongs to rather than every voice on the key.
fn pckn(key: u8, note_id: Option<u32>) -> Pckn {
    Pckn {
        port_index: Match::Specific(0),
        channel: Match::Specific(0),
        key: Match::Specific(u16::from(key)),
        note_id: note_id.map_or(Match::All, Match::Specific),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivered(queue: &mut EventQueue, frames: usize, max_frames: usize) -> Vec<Vec<(u32, String)>> {
        queue.sort();
        let mut out = EventBuffer::with_capacity(EVENT_CAPACITY);
        let (mut next, mut done, mut calls) = (0, 0, Vec::new());
        while done < frames {
            let chunk = (frames - done).min(max_frames);
            queue.deliver(&mut next, done, chunk, done + chunk == frames, &mut out);
            calls.push(
                out.iter()
                    .map(|e| {
                        let what = if let Some(on) = e.as_event::<NoteOnEvent>() {
                            format!("on {:?} {:?}", on.key(), on.note_id())
                        } else if let Some(off) = e.as_event::<NoteOffEvent>() {
                            format!("off {:?} {:?}", off.key(), off.note_id())
                        } else if let Some(x) = e.as_event::<NoteExpressionEvent>() {
                            format!("tune {:?} {:?} {}", x.key(), x.note_id(), x.value())
                        } else {
                            "other".to_string()
                        };
                        (e.header().time(), what)
                    })
                    .collect(),
            );
            done += chunk;
        }
        calls
    }

    #[test]
    fn a_note_sounds_and_releases_under_one_real_id() {
        let mut q = EventQueue::new();
        let id = q.note_on(60, 1.0);
        assert!(id <= i32::MAX as u32);
        assert!(q.holds(id, 60));
        q.note_off(60);
        assert!(!q.holds(id, 60));
        let calls = delivered(&mut q, 64, 64);
        assert_eq!(
            calls[0],
            vec![
                (0, format!("on Specific(60) Specific({id})")),
                (0, format!("off Specific(60) Specific({id})")),
            ]
        );
    }

    #[test]
    fn ids_are_never_reused_between_notes() {
        let mut q = EventQueue::new();
        let a = q.note_on(60, 1.0);
        q.note_off(60);
        let b = q.note_on(60, 1.0);
        assert_ne!(a, b);
        assert!(!q.holds(a, 60), "a released note's id must not match its successor");
        assert!(q.holds(b, 60));
    }

    #[test]
    fn a_restruck_key_releases_with_a_wildcard_id() {
        let mut q = EventQueue::new();
        let first = q.note_on(64, 1.0);
        let second = q.note_on(64, 1.0);
        assert!(!q.holds(first, 64), "the key now sounds under the newer id");
        assert!(q.holds(second, 64));
        q.note_off(64);
        let calls = delivered(&mut q, 16, 16);
        assert_eq!(calls[0][2], (0, "off Specific(64) All".to_string()));
    }

    #[test]
    fn tuning_is_refused_for_a_note_that_is_not_held() {
        let mut q = EventQueue::new();
        let id = q.note_on(60, 1.0);
        assert!(!q.tuning(0, id, 61, 0.5), "wrong key");
        assert!(!q.tuning(0, id.wrapping_add(1), 60, 0.5), "wrong id");
        q.all_notes_off();
        assert!(!q.tuning(0, id, 60, 0.5), "released");
    }

    #[test]
    fn tuning_stops_at_capacity_instead_of_growing_the_queue() {
        let mut q = EventQueue::new();
        let id = q.note_on(60, 1.0);
        let accepted = (0..2 * EVENT_CAPACITY as u32)
            .filter(|&i| q.tuning(i, id, 60, 0.01 * f64::from(i)))
            .count();
        assert_eq!(accepted, EVENT_CAPACITY - 1, "the note-on already holds one slot");
        assert_eq!(q.events.capacity(), EVENT_CAPACITY);
    }

    #[test]
    fn timed_events_land_in_the_process_call_that_contains_them() {
        let mut q = EventQueue::new();
        let id = q.note_on(60, 1.0);
        // Queued out of order, as several notes' expressions are: delivery must sort them.
        q.tuning(100, id, 60, 1.0);
        q.tuning(0, id, 60, 0.25);
        q.tuning(64, id, 60, 0.5);
        q.tuning(63, id, 60, 0.75);
        let calls = delivered(&mut q, 160, 64);
        assert_eq!(calls.len(), 3);
        let tune = |t: u32, v: f64| (t, format!("tune Specific(60) Specific({id}) {v}"));
        assert_eq!(
            calls[0],
            vec![(0, format!("on Specific(60) Specific({id})")), tune(0, 0.25), tune(63, 0.75)],
            "the note-on comes first even though a tuning shares its sample"
        );
        assert_eq!(
            calls[1],
            vec![tune(0, 0.5), tune(36, 1.0)],
            "re-stamped against the second call"
        );
        assert!(calls[2].is_empty());
    }

    #[test]
    fn tuning_is_clamped_to_the_clap_range() {
        let mut q = EventQueue::new();
        let id = q.note_on(60, 1.0);
        q.tuning(0, id, 60, 500.0);
        let calls = delivered(&mut q, 8, 8);
        assert_eq!(calls[0][1].1, format!("tune Specific(60) Specific({id}) 120"));
    }
}
