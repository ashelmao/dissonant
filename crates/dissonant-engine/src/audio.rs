//! Audio clip playback and the per-track audio lane.
//!
//! An audio track plays clips off a shared [`AudioPool`], optionally mixes in the live input
//! when it is armed for monitoring, and runs the result through its [`Rig`]. Decoding and file
//! I/O happen in the app layer; by the time the engine sees a source it is already f32 samples
//! in memory, shared by `Arc` so swapping the pool never copies audio.

use crate::rig::Rig;
use dissonant_core::{SequenceAudioClip, SequenceTrack};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// One decoded source, interleaved, at the file's own sample rate — never resampled on import,
/// so changing the output device never degrades the original.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: u8,
    pub frames: usize,
    /// `frames * channels` samples, interleaved.
    pub data: Vec<f32>,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, channels: u8, data: Vec<f32>) -> Self {
        let ch = channels.max(1) as usize;
        let frames = data.len() / ch;
        AudioBuffer {
            sample_rate,
            channels: channels.max(1),
            frames,
            data,
        }
    }

    /// One frame's left and right samples. Mono sources feed both sides; anything past stereo
    /// is folded down to its first two channels.
    #[inline]
    fn frame(&self, i: isize) -> (f32, f32) {
        if i < 0 || i as usize >= self.frames {
            return (0.0, 0.0);
        }
        let ch = self.channels as usize;
        let base = i as usize * ch;
        let l = self.data[base];
        let r = if ch > 1 { self.data[base + 1] } else { l };
        (l, r)
    }
}

/// Every decoded source the engine can reach, keyed by `AudioSource::id`.
///
/// Built whole on the app thread and handed over by `Arc`, so the audio thread only ever swaps
/// a pointer; the displaced pool is dropped on the app thread when the engine releases it.
#[derive(Debug, Default)]
pub struct AudioPool {
    sources: HashMap<Uuid, Arc<AudioBuffer>>,
}

impl AudioPool {
    pub fn new(sources: HashMap<Uuid, Arc<AudioBuffer>>) -> Self {
        AudioPool { sources }
    }

    pub fn get(&self, id: &Uuid) -> Option<&Arc<AudioBuffer>> {
        self.sources.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// What a sub-block needs to know beyond its samples.
pub struct BlockContext<'a> {
    /// Transport time in seconds at frame 0 of this sub-block.
    pub time_seconds: f64,
    /// How long one beat lasts at the song's tempo where the playhead is, so a tempo-synced
    /// insert is in time with the song rather than with a number someone typed. Read per
    /// sub-block: a tempo ramp moves it.
    pub beat_seconds: f32,
    /// False while stopped or counting in: clips stay silent, but monitoring keeps working —
    /// which is how you practise before hitting record.
    pub playing: bool,
    pub pool: &'a AudioPool,
    /// Every audio clip in the sequence; the lane picks out its own.
    pub clips: &'a [SequenceAudioClip],
    /// This sub-block of live input, mono, one sample per output frame.
    pub input: &'a [f32],
}

/// Scratch buffers start here and grow to the block size on first use, like `Channel`'s.
const SCRATCH_FRAMES: usize = 8192;

/// What an audio track does with a block: play its clips, mix in the live input when it is
/// monitored, and run the result through its rig.
#[derive(Debug)]
pub struct AudioLane {
    track_id: Uuid,
    rig: Rig,
    rig_enabled: bool,
    /// Set when this track is armed and monitoring is on.
    pub monitor: bool,
    clip_l: Vec<f32>,
    clip_r: Vec<f32>,
    mono: Vec<f32>,
    /// Whether the last block had any clip or input material at all.
    had_signal: bool,
}

impl AudioLane {
    pub fn new(track_id: Uuid, sample_rate: f32) -> Self {
        AudioLane {
            track_id,
            rig: Rig::new(sample_rate),
            rig_enabled: true,
            monitor: false,
            clip_l: vec![0.0; SCRATCH_FRAMES],
            clip_r: vec![0.0; SCRATCH_FRAMES],
            mono: vec![0.0; SCRATCH_FRAMES],
            had_signal: false,
        }
    }

    pub fn apply(&mut self, track: &SequenceTrack) {
        if let Some(settings) = &track.amp {
            self.rig_enabled = settings.enabled;
            self.rig.apply(settings);
        } else {
            self.rig_enabled = false;
        }
    }

    /// Peak at the rig's input over the last block, for the UI's input meter.
    pub fn input_peak(&self) -> f32 {
        self.rig.input_peak()
    }

    pub fn reset(&mut self) {
        self.rig.reset();
        self.had_signal = false;
    }

    /// Render this lane's clips and monitored input into `l`/`r`, additively.
    ///
    /// Returns false when it wrote nothing — no clip, nothing monitored, and the rig's tails
    /// already run out — so the caller can skip its own per-sample work. The decision is made
    /// here and not by the caller: whether there is anything to play depends on the block's
    /// clips, which only the lane looks at.
    pub fn render_add(
        &mut self,
        l: &mut [f32],
        r: &mut [f32],
        ctx: &BlockContext<'_>,
        sample_rate: f32,
    ) -> bool {
        let frames = l.len().min(r.len());
        if self.clip_l.len() < frames {
            self.clip_l.resize(frames, 0.0);
            self.clip_r.resize(frames, 0.0);
            self.mono.resize(frames, 0.0);
        }
        let (cl, cr) = (&mut self.clip_l[..frames], &mut self.clip_r[..frames]);
        cl.fill(0.0);
        cr.fill(0.0);

        let mut had_signal = false;
        if ctx.playing {
            let block_end = ctx.time_seconds + frames as f64 / sample_rate as f64;
            for clip in ctx.clips.iter().filter(|c| c.track_id == self.track_id) {
                let clip_end = clip.start_seconds + clip.length_seconds;
                if clip_end <= ctx.time_seconds || clip.start_seconds >= block_end {
                    continue;
                }
                let Some(source) = ctx.pool.get(&clip.source_id) else {
                    // A source whose file is missing: the clip is drawn in the UI and marked,
                    // but there is nothing to play. Silence is the honest answer.
                    continue;
                };
                had_signal = true;
                mix_clip(clip, source, cl, cr, ctx.time_seconds, sample_rate);
            }
        }

        if self.monitor && !ctx.input.is_empty() {
            let n = frames.min(ctx.input.len());
            for i in 0..n {
                cl[i] += ctx.input[i];
                cr[i] += ctx.input[i];
            }
            had_signal = true;
        }
        self.had_signal = had_signal;

        if !self.rig_enabled {
            if !had_signal {
                return false;
            }
            for i in 0..frames {
                l[i] += cl[i];
                r[i] += cr[i];
            }
            return true;
        }

        // With the rig on, a settled tail is the only thing that makes a silent block skippable.
        if !had_signal && self.rig.is_silent() {
            return false;
        }
        let mono = &mut self.mono[..frames];
        for i in 0..frames {
            mono[i] = (cl[i] + cr[i]) * 0.5;
        }
        self.rig.process_add(mono, l, r);
        true
    }
}

/// Read one clip's overlapping region into `cl`/`cr`.
///
/// The cursor advances by `source rate / engine rate` per output frame and is sampled with
/// 4-point Catmull-Rom, so a 44.1 kHz file keeps its pitch on a 48 kHz device.
fn mix_clip(
    clip: &SequenceAudioClip,
    source: &AudioBuffer,
    cl: &mut [f32],
    cr: &mut [f32],
    block_start: f64,
    sample_rate: f32,
) {
    let frames = cl.len();
    let gain = clip.gain as f32;

    // Which output frames this clip covers in this block.
    let first = (((clip.start_seconds - block_start) * sample_rate as f64)
        .ceil()
        .max(0.0) as usize)
        .min(frames);
    let last = (((clip.start_seconds + clip.length_seconds - block_start) * sample_rate as f64)
        .ceil() as i64)
        .clamp(0, frames as i64) as usize;

    for i in first..last {
        // Seconds into the clip at this output frame.
        let into = block_start + i as f64 / sample_rate as f64 - clip.start_seconds;
        let pos = (into + clip.offset_seconds) * source.sample_rate as f64;
        let whole = pos.floor();
        let frac = (pos - whole) as f32;
        let n = whole as isize;

        let (l0, r0) = source.frame(n - 1);
        let (l1, r1) = source.frame(n);
        let (l2, r2) = source.frame(n + 1);
        let (l3, r3) = source.frame(n + 2);

        let env = fade_gain(clip, into) * gain;
        cl[i] += catmull_rom(l0, l1, l2, l3, frac) * env;
        cr[i] += catmull_rom(r0, r1, r2, r3, frac) * env;
    }
}

/// The clip's fade envelope `into` seconds from its start.
///
/// Equal power rather than linear: a punch seam is two clips overlapping, one fading out while
/// the other fades in, and a pair of linear ramps dips ~3 dB in the middle of the join. With
/// `sin`/`cos` the two squared gains sum to one, so a crossfade between takes of the same
/// material holds its level. A lone fade sounds the same either way to within the curve.
#[inline]
fn fade_gain(clip: &SequenceAudioClip, into: f64) -> f32 {
    let mut g = 1.0f64;
    if clip.fade_in_seconds > 0.0 && into < clip.fade_in_seconds {
        g *= equal_power(into / clip.fade_in_seconds);
    }
    let from_end = clip.length_seconds - into;
    if clip.fade_out_seconds > 0.0 && from_end < clip.fade_out_seconds {
        g *= equal_power(from_end / clip.fade_out_seconds);
    }
    g as f32
}

/// `sin(t · π/2)`: the rising half of an equal-power pair, so that `f(t)² + f(1 - t)² == 1`.
#[inline]
fn equal_power(t: f64) -> f64 {
    (t.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2).sin()
}

/// Catmull-Rom through `p1`/`p2`, using `p0`/`p3` for the slopes.
#[inline]
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}
