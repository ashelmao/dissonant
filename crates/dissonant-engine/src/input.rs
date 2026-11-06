//! Real-time capture through cpal: a guitar, a mic, anything the interface offers.
//!
//! Mirrors [`crate::device`]: the stream lives on its own thread (cpal streams aren't `Send`
//! everywhere), readiness comes back over an `mpsc` channel, and a `stop`/`failed` pair plus a
//! `Drop` that unparks and joins tears it down.
//!
//! The engine consumes input 1:1 with its output frames, so neither the rate nor the buffer
//! size is negotiable here: both are already fixed by the output stream. A device that cannot
//! match them is an error the user has to see, not something to silently resample — and unlike
//! the output device there is nothing to fall back to, since the ring's other end is already
//! inside the running engine.

use crate::device::DeviceError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample};
use dissonant_core::TempoMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

/// One input device as the guitar screen lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct InputDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub channels: u16,
    pub default_sample_rate: u32,
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "input".into())
}

/// Enumerate capture devices.
pub fn list_input_devices() -> Vec<InputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().map(|d| device_name(&d));
    let Ok(devices) = host.input_devices() else {
        return vec![];
    };
    devices
        .map(|d| {
            let name = device_name(&d);
            let default = d.default_input_config().ok();
            InputDeviceInfo {
                is_default: Some(&name) == default_name.as_ref(),
                name,
                channels: default.as_ref().map(|c| c.channels()).unwrap_or(1),
                default_sample_rate: default.as_ref().map(|c| c.sample_rate()).unwrap_or(44_100),
            }
        })
        .collect()
}

/// The name of the host's current default input device, if any.
pub fn default_input_name() -> Option<String> {
    Some(device_name(&cpal::default_host().default_input_device()?))
}

/// Where a take has to land, once the capture path's latency is taken off its start.
///
/// Everything on the way in is delay: the output block the engine renders before the input is
/// read, whatever the ring is still holding, and whatever the interface and its driver add on
/// top (which only the player can measure, so it is a manual per-device trim). The engine
/// stamps a take's start tick at the moment the first captured sample reaches it, so the
/// *sound* happened that whole latency earlier and the clip has to move back by it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TakeAlignment {
    /// Where the clip goes.
    pub start_tick: i64,
    /// How far into the recording the clip starts. Non-zero only when the correction ran off
    /// the front of the song.
    pub offset_seconds: f64,
    /// How far back the clip actually moved, in ms — what the REC page reports.
    pub shifted_ms: f64,
    /// How much of the take's head was cut instead of moved, in ms.
    pub trimmed_ms: f64,
}

/// Pull `recorded_start_tick` back by `latency_ms`, in musical time.
///
/// The conversion goes through the tempo map rather than a single bpm: a take that starts in
/// the middle of a ritardando is played at one tempo and written at another, and 20 ms is a
/// different number of ticks at each. Seconds are the invariant, so the correction is done
/// there and converted back once.
///
/// A correction that would push the start before tick 0 clamps there and cuts the difference
/// off the head of the audio instead. Moving the clip and keeping all of it would slide the
/// waveform off the grid, which is the one thing this exists to prevent; the lost head is the
/// pickup noise before the first note.
pub fn align_take(map: &TempoMap, recorded_start_tick: f64, latency_ms: f64) -> TakeAlignment {
    let start = recorded_start_tick.max(0.0);
    let latency = if latency_ms.is_finite() { latency_ms.max(0.0) / 1000.0 } else { 0.0 };
    let recorded_seconds = map.seconds_at_tick(start);
    let wanted = recorded_seconds - latency;
    if wanted < 0.0 {
        return TakeAlignment {
            start_tick: 0,
            offset_seconds: -wanted,
            shifted_ms: recorded_seconds * 1000.0,
            trimmed_ms: -wanted * 1000.0,
        };
    }
    let tick = map.tick_at_seconds(wanted).round().max(0.0);
    TakeAlignment {
        start_tick: tick as i64,
        offset_seconds: 0.0,
        // Report the tick the clip really landed on, not the ideal: rounding to a whole tick
        // is part of the correction the user is being told about.
        shifted_ms: (recorded_seconds - map.seconds_at_tick(tick)) * 1000.0,
        trimmed_ms: 0.0,
    }
}

/// The capture path's latency in ms: the engine's block, whatever the input ring still holds,
/// and the device's manual trim. The first two come from the engine, the third from settings.
///
/// With no sample rate there is no stream, so there is nothing to compensate and the trim does
/// not apply on its own — a trim describes a device that is open.
pub fn input_latency_ms(block_frames: u32, backlog_frames: u32, sample_rate: u32, trim_ms: f64) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    let measured = (block_frames as f64 + backlog_frames as f64) * 1000.0 / sample_rate as f64;
    let trim = if trim_ms.is_finite() { trim_ms } else { 0.0 };
    (measured + trim).max(0.0)
}

/// Handle to a running capture stream. Dropping it stops recording and monitoring.
pub struct AudioInput {
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub device_name: String,
    pub sample_rate: u32,
    /// Channels the device actually opened with.
    pub channels: u16,
    /// Which of them is being captured.
    pub channel: u16,
}

/// What the capture thread reports once the stream is up.
struct Opened {
    name: String,
    channels: u16,
    channel: u16,
}

impl AudioInput {
    /// Open `device` at exactly `sample_rate`, take `channel`, and push one mono sample per
    /// frame into `sink`.
    ///
    /// `buffer_size` should be whatever the output stream accepted; `None` lets the host
    /// choose, which is what the output device does when it has no fixed size of its own.
    pub fn start_with(
        device: Option<&str>,
        sample_rate: u32,
        buffer_size: Option<u32>,
        channel: u16,
        sink: rtrb::Producer<f32>,
    ) -> Result<AudioInput, DeviceError> {
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Opened, DeviceError>>();

        let thread_stop = stop.clone();
        let thread_failed = failed.clone();
        let wanted = device.map(str::to_string);
        let thread = std::thread::Builder::new()
            .name("dissonant-input".into())
            .spawn(move || {
                match build_input(
                    wanted.as_deref(),
                    sample_rate,
                    buffer_size,
                    channel,
                    sink,
                    thread_failed,
                ) {
                    Ok((stream, opened)) => {
                        if let Err(e) = stream.play() {
                            let _ = ready_tx.send(Err(DeviceError::Cpal(e.to_string())));
                            return;
                        }
                        let _ = ready_tx.send(Ok(opened));
                        while !thread_stop.load(Ordering::Relaxed) {
                            std::thread::park_timeout(std::time::Duration::from_millis(250));
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| DeviceError::Cpal(e.to_string()))?;

        let opened = ready_rx
            .recv()
            .map_err(|_| DeviceError::Cpal("input thread died".into()))??;

        Ok(AudioInput {
            stop,
            failed,
            thread: Some(thread),
            device_name: opened.name,
            sample_rate,
            channels: opened.channels,
            channel: opened.channel,
        })
    }

    /// True once the stream reported an error (e.g. the interface was unplugged).
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

impl Drop for AudioInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn build_input(
    wanted: Option<&str>,
    sample_rate: u32,
    buffer_size: Option<u32>,
    channel: u16,
    sink: rtrb::Producer<f32>,
    failed: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Opened), DeviceError> {
    let host = cpal::default_host();
    let device = match wanted {
        Some(name) => host
            .input_devices()
            .ok()
            .and_then(|mut devs| devs.find(|d| device_name(d) == name))
            .or_else(|| host.default_input_device()),
        None => host.default_input_device(),
    }
    .ok_or(DeviceError::NoDevice)?;
    let name = device_name(&device);

    // The rate is fixed by the output device, so only configs covering it are usable. Prefer
    // the widest channel count so a interface's later inputs stay selectable.
    let config = device
        .supported_input_configs()
        .map_err(|e| DeviceError::Cpal(e.to_string()))?
        .filter(|c| c.min_sample_rate() <= sample_rate && sample_rate <= c.max_sample_rate())
        .max_by_key(|c| c.channels())
        .map(|c| c.with_sample_rate(sample_rate))
        .ok_or_else(|| DeviceError::Cpal(format!("'{name}' cannot record at {sample_rate} Hz")))?;

    let channels = config.channels();
    let format = config.sample_format();
    let mut stream_config: cpal::StreamConfig = config.into();
    if let Some(n) = buffer_size {
        stream_config.buffer_size = cpal::BufferSize::Fixed(n);
    }
    let take = channel.min(channels.saturating_sub(1));

    let stream = match format {
        SampleFormat::F32 => capture::<f32>(&device, &stream_config, channels, take, sink, failed),
        SampleFormat::I16 => capture::<i16>(&device, &stream_config, channels, take, sink, failed),
        SampleFormat::U16 => capture::<u16>(&device, &stream_config, channels, take, sink, failed),
        SampleFormat::I32 => capture::<i32>(&device, &stream_config, channels, take, sink, failed),
        other => {
            return Err(DeviceError::Cpal(format!(
                "unsupported input sample format {other:?}"
            )))
        }
    }?;
    Ok((
        stream,
        Opened {
            name,
            channels,
            channel: take,
        },
    ))
}

fn capture<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: u16,
    take: u16,
    mut sink: rtrb::Producer<f32>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let stride = channels.max(1) as usize;
    let take = take as usize;
    device
        .build_input_stream(
            *config,
            move |data: &[T], _| {
                for frame in data.chunks(stride) {
                    let Some(&s) = frame.get(take) else { continue };
                    // A full ring means the engine is not draining it: drop rather than block,
                    // the same bargain every real-time path here makes.
                    if sink.push(f32::from_sample(s)).is_err() {
                        break;
                    }
                }
            },
            move |err| {
                log::error!("audio input error: {err}");
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| DeviceError::Cpal(e.to_string()))
}
