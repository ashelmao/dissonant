//! Real-time output through cpal. The stream lives on its own thread (cpal streams aren't
//! `Send` on every platform), and the application talks to the engine through a lock-free
//! ring buffer plus the [`Shared`] atomics.

use crate::drums::DrumBank;
use crate::engine::{Engine, EngineCommand, Shared};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};
use dissonant_core::MasterSettings;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;
use thiserror::Error;

/// What the application asks for. Every field is a wish: the device decides, and
/// [`AudioDevice`] reports what it actually got.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioConfig {
    /// Output device name, or `None` for the system default.
    pub device: Option<String>,
    pub sample_rate: Option<u32>,
    /// Frames per callback. `None` lets the host choose.
    pub buffer_size: Option<u32>,
}

/// One output device as the settings window lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputDeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// Common rates the device supports (intersection with 44.1/48/88.2/96/176.4/192 kHz).
    pub sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
}

const COMMON_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

/// Enumerate output devices with the sample rates they accept.
pub fn list_output_devices() -> Vec<OutputDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().map(|d| device_name(&d));
    let Ok(devices) = host.output_devices() else { return vec![] };
    devices
        .map(|d| {
            let name = device_name(&d);
            let default_rate = d.default_output_config().map(|c| c.sample_rate()).unwrap_or(44_100);
            let mut rates: Vec<u32> = d
                .supported_output_configs()
                .map(|cfgs| {
                    let cfgs: Vec<_> = cfgs.collect();
                    COMMON_RATES
                        .iter()
                        .copied()
                        .filter(|r| cfgs.iter().any(|c| c.min_sample_rate() <= *r && *r <= c.max_sample_rate()))
                        .collect()
                })
                .unwrap_or_default();
            if !rates.contains(&default_rate) {
                rates.push(default_rate);
                rates.sort_unstable();
            }
            OutputDeviceInfo {
                is_default: Some(&name) == default_name.as_ref(),
                name,
                sample_rates: rates,
                default_sample_rate: default_rate,
            }
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("no default output device")]
    NoDevice,
    #[error("audio device error: {0}")]
    Cpal(String),
    #[error("engine command queue is full")]
    QueueFull,
}

/// Handle to a running output stream. Dropping it stops audio.
pub struct AudioDevice {
    producer: Mutex<rtrb::Producer<EngineCommand>>,
    /// Drums rendered for the last sequence sent, so the next one renders only what changed.
    drums: Mutex<DrumBank>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    /// Set from the stream's error callback (device unplugged, format lost). The application
    /// polls it and restarts on the current default device.
    failed: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
    /// Name of the device the stream was opened on.
    pub device_name: String,
    /// The buffer size that was requested and accepted, if a fixed one was asked for.
    pub buffer_size: Option<u32>,
}

/// The name of the host's current default output device, if any.
pub fn default_output_name() -> Option<String> {
    let device = cpal::default_host().default_output_device()?;
    Some(device_name(&device))
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "output".into())
}

/// What `build_stream` reports back once the stream is up.
struct Opened {
    sample_rate: u32,
    name: String,
    buffer_size: Option<u32>,
}

impl AudioDevice {
    /// Open the default output device and start rendering. `master` seeds the master chain.
    pub fn start(master: MasterSettings) -> Result<AudioDevice, DeviceError> {
        Self::start_with(master, AudioConfig::default())
    }

    /// Open the requested device / rate / buffer size, falling back to the device defaults
    /// for whatever it refuses.
    pub fn start_with(master: MasterSettings, config: AudioConfig) -> Result<AudioDevice, DeviceError> {
        let shared = Arc::new(Shared::default());
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(Opened, rtrb::Producer<EngineCommand>), DeviceError>>();

        let thread_shared = shared.clone();
        let thread_stop = stop.clone();
        let thread_failed = failed.clone();
        let thread = std::thread::Builder::new()
            .name("dissonant-audio".into())
            .spawn(move || {
                // Attempt the wish first; if a fixed buffer size is refused, the host default.
                let mut attempts = vec![config.clone()];
                if config.buffer_size.is_some() {
                    attempts.push(AudioConfig { buffer_size: None, ..config.clone() });
                }
                let mut result = Err(DeviceError::NoDevice);
                for (i, attempt) in attempts.iter().enumerate() {
                    let (producer, consumer) = rtrb::RingBuffer::<EngineCommand>::new(1024);
                    result = build_stream(consumer, thread_shared.clone(), master.clone(), thread_failed.clone(), attempt)
                        .map(|(stream, opened)| (stream, opened, producer));
                    match &result {
                        Ok(_) => break,
                        Err(e) if i + 1 < attempts.len() => {
                            log::warn!("buffer size {:?} refused ({e}); trying the device default", attempt.buffer_size);
                        }
                        Err(_) => {}
                    }
                }
                match result {
                    Ok((stream, opened, producer)) => {
                        if let Err(e) = stream.play() {
                            let _ = ready_tx.send(Err(DeviceError::Cpal(e.to_string())));
                            return;
                        }
                        let _ = ready_tx.send(Ok((opened, producer)));
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

        let (opened, producer) = ready_rx
            .recv()
            .map_err(|_| DeviceError::Cpal("audio thread died".into()))??;

        Ok(AudioDevice {
            producer: Mutex::new(producer),
            drums: Mutex::new(DrumBank::new(opened.sample_rate as f32)),
            shared,
            stop,
            failed,
            thread: Some(thread),
            sample_rate: opened.sample_rate,
            device_name: opened.name,
            buffer_size: opened.buffer_size,
        })
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// True once the stream reported an error (e.g. the device went away).
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Queue a command for the audio thread. A `SetSequence` has its drum kits rendered here,
    /// on the caller's thread (only pads whose patch changed since the last one), and travels on
    /// as `SetSequenceWithDrums`, so the audio thread never synthesizes a drum.
    pub fn send(&self, cmd: EngineCommand) -> Result<(), DeviceError> {
        let cmd = match cmd {
            EngineCommand::SetSequence(sequence) => {
                let mut bank = self.drums.lock().unwrap_or_else(|e| e.into_inner());
                bank.prepare(&sequence, self.sample_rate as f32);
                EngineCommand::SetSequenceWithDrums { sequence, drums: Arc::new(bank.clone()) }
            }
            other => other,
        };
        let mut p = self.producer.lock().map_err(|_| DeviceError::QueueFull)?;
        p.push(cmd).map_err(|_| DeviceError::QueueFull)
    }
}

impl Drop for AudioDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}

fn build_stream(
    consumer: rtrb::Consumer<EngineCommand>,
    shared: Arc<Shared>,
    master: MasterSettings,
    failed: Arc<AtomicBool>,
    wanted: &AudioConfig,
) -> Result<(cpal::Stream, Opened), DeviceError> {
    let host = cpal::default_host();
    let device = match &wanted.device {
        Some(name) => host
            .output_devices()
            .ok()
            .and_then(|mut devs| devs.find(|d| &device_name(d) == name))
            .or_else(|| host.default_output_device()),
        None => host.default_output_device(),
    }
    .ok_or(DeviceError::NoDevice)?;
    let name = device_name(&device);
    let default = device
        .default_output_config()
        .map_err(|e| DeviceError::Cpal(e.to_string()))?;
    // A requested sample rate is honoured only when a supported config range contains it.
    let config = match wanted.sample_rate {
        Some(rate) if rate != default.sample_rate() => device
            .supported_output_configs()
            .ok()
            .and_then(|cfgs| {
                let mut cfgs: Vec<_> = cfgs
                    .filter(|c| c.min_sample_rate() <= rate && rate <= c.max_sample_rate())
                    .collect();
                // Prefer the default's channel count and format so nothing else changes.
                cfgs.sort_by_key(|c| (c.channels() != default.channels(), c.sample_format() != default.sample_format()));
                cfgs.into_iter().next().map(|c| c.with_sample_rate(rate))
            })
            .unwrap_or(default),
        _ => default,
    };
    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;
    let format = config.sample_format();
    let mut stream_config: cpal::StreamConfig = config.into();
    let buffer_size = wanted.buffer_size;
    if let Some(n) = buffer_size {
        stream_config.buffer_size = cpal::BufferSize::Fixed(n);
    }
    let engine = Engine::new(sample_rate as f32, shared, &master);
    let stream = match format {
        SampleFormat::F32 => make_stream::<f32>(&device, &stream_config, channels, engine, consumer, failed),
        SampleFormat::I16 => make_stream::<i16>(&device, &stream_config, channels, engine, consumer, failed),
        SampleFormat::U16 => make_stream::<u16>(&device, &stream_config, channels, engine, consumer, failed),
        SampleFormat::I32 => make_stream::<i32>(&device, &stream_config, channels, engine, consumer, failed),
        other => return Err(DeviceError::Cpal(format!("unsupported sample format {other:?}"))),
    }?;
    Ok((stream, Opened { sample_rate, name, buffer_size }))
}

fn make_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut engine: Engine,
    mut consumer: rtrb::Consumer<EngineCommand>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample + cpal::FromSample<f32>,
{
    let mut stereo: Vec<f32> = vec![0.0; 8192];
    let sample_rate = config.sample_rate as f64;
    let mut last_callback: Option<Instant> = None;
    let mut load_ema = 0.0f64;
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _| {
                let started = Instant::now();
                let frames = data.len() / channels.max(1);
                let block_secs = frames as f64 / sample_rate;
                // A gap of more than two blocks between callbacks means the host starved.
                if let Some(prev) = last_callback {
                    if started.duration_since(prev).as_secs_f64() > block_secs * 2.5 {
                        engine.shared().note_xrun();
                    }
                }
                last_callback = Some(started);
                while let Ok(cmd) = consumer.pop() {
                    engine.handle(cmd);
                }
                if stereo.len() < frames * 2 {
                    stereo.resize(frames * 2, 0.0);
                }
                engine.process(&mut stereo[..frames * 2]);
                let load = started.elapsed().as_secs_f64() / block_secs.max(1e-9);
                load_ema += (load - load_ema) * 0.1;
                engine.shared().set_stats(frames as u32, load_ema as f32);
                for (i, frame) in data.chunks_mut(channels.max(1)).enumerate() {
                    let (l, r) = (stereo[2 * i], stereo[2 * i + 1]);
                    match frame.len() {
                        1 => frame[0] = T::from_sample((l + r) * 0.5),
                        _ => {
                            frame[0] = T::from_sample(l);
                            frame[1] = T::from_sample(r);
                            for s in &mut frame[2..] {
                                *s = T::from_sample(0.0);
                            }
                        }
                    }
                }
            },
            move |err| {
                log::error!("audio stream error: {err}");
                failed.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| DeviceError::Cpal(e.to_string()))
}
