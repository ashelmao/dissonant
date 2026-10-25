//! Offline rendering: run the same [`Engine`] faster than real time into a WAV file.
//! Deterministic, no device needed — this is what export, stems and tests use.

use crate::engine::{Engine, EngineCommand, Shared};
use dissonant_core::{MasterSettings, Sequence};
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("nothing to render: the sequence is empty")]
    Empty,
    #[error("could not write WAV: {0}")]
    Wav(#[from] hound::Error),
}

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub sample_rate: u32,
    /// Extra seconds after the last beat so releases and reverb tails aren't cut.
    pub tail_seconds: f64,
    pub master: MasterSettings,
    /// 16 or 24 (integer PCM) or 32 (float).
    pub bit_depth: u16,
    /// Scale the whole render so its peak lands at this dBFS (e.g. -1.0); `None` leaves it.
    pub normalize_db: Option<f32>,
    /// TPDF dither when writing 16-bit.
    pub dither: bool,
    /// How many times through the sequence (1 = once). Lets a pattern export as a longer loop.
    pub loops: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            sample_rate: 44_100,
            tail_seconds: 1.5,
            master: MasterSettings::default(),
            bit_depth: 16,
            normalize_db: None,
            dither: true,
            loops: 1,
        }
    }
}

/// Render `sequence` once (no looping) plus a tail, as interleaved stereo f32. The chord bed
/// is a monitor for writing, never part of the song, so a render never includes it.
pub fn render_to_buffer(sequence: Arc<Sequence>, options: &RenderOptions) -> Result<Vec<f32>, RenderError> {
    if sequence.length_beats <= 0.0 {
        return Err(RenderError::Empty);
    }
    let sr = options.sample_rate as f32;
    let loops = options.loops.max(1);
    let mut engine = Engine::new(sr, Arc::new(Shared::default()), &options.master);
    // Looping stays on while there are repetitions left; the engine wraps at the sequence end.
    engine.handle(EngineCommand::SetLooping(loops > 1));
    engine.handle(EngineCommand::SetHearChords(false));
    let loop_secs = sequence.tempo_map.seconds_at(sequence.length_beats);
    let seconds = loop_secs * loops as f64 + options.tail_seconds;
    engine.handle(EngineCommand::SetSequence(sequence));
    engine.handle(EngineCommand::Play);

    let total_frames = (seconds * sr as f64).ceil() as usize;
    let last_pass_at = (loop_secs * (loops as f64 - 1.0) * sr as f64) as usize;
    let block = 1024;
    let mut out = Vec::with_capacity(total_frames * 2);
    let mut buf = vec![0.0f32; block * 2];
    let mut done = 0;
    let mut unlooped = loops <= 1;
    while done < total_frames {
        if !unlooped && done >= last_pass_at {
            // Entering the final repetition: let it run out into the tail instead of wrapping.
            engine.handle(EngineCommand::SetLooping(false));
            unlooped = true;
        }
        let n = (total_frames - done).min(block);
        engine.process(&mut buf[..n * 2]);
        out.extend_from_slice(&buf[..n * 2]);
        done += n;
    }
    Ok(out)
}

/// Peak-normalize in place to `target_db` dBFS (no-op on silence).
pub fn normalize(samples: &mut [f32], target_db: f32) {
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak <= 1e-6 {
        return;
    }
    let gain = 10f32.powf(target_db / 20.0) / peak;
    for s in samples.iter_mut() {
        *s *= gain;
    }
}

/// Render to a stereo WAV file at the options' bit depth.
pub fn render_wav(sequence: Arc<Sequence>, path: &Path, options: &RenderOptions) -> Result<(), RenderError> {
    let mut samples = render_to_buffer(sequence, options)?;
    if let Some(db) = options.normalize_db {
        normalize(&mut samples, db);
    }
    write_wav(path, &samples, options)
}

/// Write interleaved stereo f32 as 16 / 24-bit PCM or 32-bit float.
pub fn write_wav(path: &Path, samples: &[f32], options: &RenderOptions) -> Result<(), RenderError> {
    let depth = match options.bit_depth {
        24 => 24,
        32 => 32,
        _ => 16,
    };
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: options.sample_rate,
        bits_per_sample: depth,
        sample_format: if depth == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    match depth {
        32 => {
            for &s in samples {
                writer.write_sample(s)?;
            }
        }
        24 => {
            let scale = 8_388_607.0f32;
            for &s in samples {
                writer.write_sample((s.clamp(-1.0, 1.0) * scale).round() as i32)?;
            }
        }
        _ => {
            // TPDF dither: the sum of two uniform ±0.5 LSB noises, decorrelating quantization.
            let mut rng = 0x9E37_79B9_7F4A_7C15u64;
            let mut uniform = move || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng >> 11) as f32 / (1u64 << 53) as f32 - 0.5
            };
            let scale = i16::MAX as f32;
            for &s in samples {
                let d = if options.dither { uniform() + uniform() } else { 0.0 };
                let v = (s.clamp(-1.0, 1.0) * scale + d).round().clamp(i16::MIN as f32, i16::MAX as f32);
                writer.write_sample(v as i16)?;
            }
        }
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissonant_core::{NoteEvent, ProjectModel};

    #[test]
    fn renders_a_wav_with_the_expected_length() {
        let mut model = ProjectModel::starter();
        let pid = model.patterns[0].id;
        model.patterns[0].length_beats = 4.0;
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 1.0, 60)];
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap());
        let dir = std::env::temp_dir().join(format!("dissonant-render-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.wav");
        let opts = RenderOptions {
            tail_seconds: 0.5,
            ..Default::default()
        };
        render_wav(seq, &path, &opts).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        // 4 beats @120 = 2 s + 0.5 s tail = 2.5 s → 110250 frames.
        assert_eq!(reader.duration(), 110_250);
        let peak = reader.into_samples::<i16>().map(|s| s.unwrap().abs()).max().unwrap();
        assert!(peak > 1000);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loops_repeat_the_sequence() {
        let mut model = ProjectModel::starter();
        let pid = model.patterns[0].id;
        model.tracks[0].voice = "sine".into();
        model.tracks[0].reverb_send = 0.0;
        model.patterns[0].length_beats = 2.0;
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 0.25, 69)];
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap());
        let opts = RenderOptions { tail_seconds: 0.0, loops: 3, ..Default::default() };
        let buf = render_to_buffer(seq, &opts).unwrap();
        // 2 beats @120 = 1 s per loop → 3 s total; the note sounds at the start of each loop.
        assert_eq!(buf.len() / 2, 44_100 * 3);
        for k in 0..3 {
            let at = k * 44_100;
            assert!(buf[2 * (at + 100)].abs() > 1e-4, "loop {k} has the note");
            assert!(buf[2 * (at + 30_000)].abs() < 1e-4, "loop {k} is quiet before its end");
        }
    }

    #[test]
    fn normalize_hits_the_target_peak_and_24_bit_round_trips() {
        let mut buf = vec![0.1f32, -0.25, 0.05, 0.0];
        normalize(&mut buf, -6.0);
        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 10f32.powf(-6.0 / 20.0)).abs() < 1e-5);
        let mut silent = vec![0.0f32; 4];
        normalize(&mut silent, -1.0);
        assert!(silent.iter().all(|s| *s == 0.0));

        let dir = std::env::temp_dir().join(format!("dissonant-wav-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for depth in [16u16, 24, 32] {
            let path = dir.join(format!("{depth}.wav"));
            let opts = RenderOptions { bit_depth: depth, dither: false, ..Default::default() };
            write_wav(&path, &[0.5, -0.5, 0.25, -0.25], &opts).unwrap();
            let reader = hound::WavReader::open(&path).unwrap();
            assert_eq!(reader.spec().bits_per_sample, depth);
            assert_eq!(reader.duration(), 2);
        }
        std::fs::remove_dir_all(dir).ok();
    }

    /// The chord bed is monitor-only: a project whose only sound would be the bed renders
    /// silent, even though the sequence carries chords.
    #[test]
    fn a_render_never_includes_the_chord_bed() {
        let mut model = ProjectModel::starter();
        model.patterns.iter_mut().for_each(|p| p.notes.clear());
        model.song_length_beats = 4.0;
        let seq = Arc::new(Sequence::from_song(&model));
        assert!(!seq.chords.is_empty());
        let buf = render_to_buffer(seq, &RenderOptions { tail_seconds: 0.0, ..Default::default() }).unwrap();
        assert!(buf.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn empty_sequence_is_an_error() {
        let seq = Arc::new(Sequence::default());
        assert!(matches!(render_to_buffer(seq, &RenderOptions::default()), Err(RenderError::Empty)));
    }
}
