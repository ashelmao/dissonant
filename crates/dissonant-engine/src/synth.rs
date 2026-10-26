//! Polyphonic waveform synth: a pool of oscillator + ADSR voices with velocity, round-robin
//! allocation and oldest-note stealing. Voice presets mirror the original app's `VoiceKind`.

use crate::dsp::{midi_to_hz, Adsr, AdsrParams, Oscillator, Waveform};
use dissonant_core::sound::OscWave;
use dissonant_core::SynthPatch;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoicePreset {
    pub waveform: Waveform,
    pub env: AdsrParams,
    pub level: f32,
}

impl VoicePreset {
    /// Look a preset up by its persisted name; unknown names fall back to `saw`.
    pub fn named(name: &str) -> VoicePreset {
        match name {
            "square" => VoicePreset {
                waveform: Waveform::Square,
                env: AdsrParams { attack: 0.004, decay: 0.12, sustain: 0.45, release: 0.10 },
                level: 0.22,
            },
            "triangle" => VoicePreset {
                waveform: Waveform::Triangle,
                env: AdsrParams { attack: 0.008, decay: 0.16, sustain: 0.55, release: 0.16 },
                level: 0.30,
            },
            "sine" => VoicePreset {
                waveform: Waveform::Sine,
                env: AdsrParams { attack: 0.015, decay: 0.18, sustain: 0.65, release: 0.20 },
                level: 0.32,
            },
            "pad" => VoicePreset {
                waveform: Waveform::Saw,
                env: AdsrParams { attack: 0.45, decay: 0.40, sustain: 0.80, release: 0.90 },
                level: 0.18,
            },
            "pluck" => VoicePreset {
                waveform: Waveform::Triangle,
                env: AdsrParams { attack: 0.001, decay: 0.16, sustain: 0.0, release: 0.14 },
                level: 0.32,
            },
            _ => VoicePreset {
                waveform: Waveform::Saw,
                env: AdsrParams { attack: 0.008, decay: 0.10, sustain: 0.65, release: 0.14 },
                level: 0.22,
            },
        }
    }

    pub const ALL: [&'static str; 6] = ["saw", "square", "triangle", "sine", "pad", "pluck"];
}

// ─── TEMPORARY ADAPTER (drum-engine worktree) ───────────────────────────────────────────────
// Lets channel/engine code call the new `PolySynth::new(&SynthPatch, ..)` / `set_patch` API while
// `synth.rs` is rewritten elsewhere. Maps only osc 1's wave, the amp envelope and the level onto
// the old preset. Replaced wholesale when the real `SynthPatch` synth merges.

impl From<&SynthPatch> for VoicePreset {
    fn from(p: &SynthPatch) -> Self {
        VoicePreset {
            waveform: match p.osc[0].wave {
                OscWave::Sine => Waveform::Sine,
                OscWave::Triangle => Waveform::Triangle,
                OscWave::Saw => Waveform::Saw,
                OscWave::Square | OscWave::Pulse => Waveform::Square,
            },
            env: AdsrParams {
                attack: p.amp_env.attack,
                decay: p.amp_env.decay,
                sustain: p.amp_env.sustain,
                release: p.amp_env.release,
            },
            level: p.level,
        }
    }
}

impl PolySynth {
    /// Live patch change without cutting voices: the wave switches now, the envelope applies
    /// to voices that start after this (old `Adsr` params can't be changed in flight).
    pub fn set_patch(&mut self, patch: &SynthPatch) {
        let preset = VoicePreset::from(patch);
        for v in &mut self.voices {
            v.osc.waveform = preset.waveform;
            if !v.env.is_active() {
                v.env = Adsr::new(preset.env, self.sample_rate);
            }
        }
        self.preset = preset;
    }
}
// ─── END TEMPORARY ADAPTER ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Voice {
    osc: Oscillator,
    env: Adsr,
    pitch: Option<u8>,
    amp: f32,
    started_at: u64,
}

#[derive(Debug, Clone)]
pub struct PolySynth {
    preset: VoicePreset,
    sample_rate: f32,
    voices: Vec<Voice>,
    counter: u64,
}

impl PolySynth {
    pub fn new(preset: impl Into<VoicePreset>, sample_rate: f32, polyphony: usize) -> Self {
        let preset = preset.into(); // TEMPORARY ADAPTER: accepts `&SynthPatch` too.
        let voices = (0..polyphony.max(1))
            .map(|_| Voice {
                osc: Oscillator::new(preset.waveform),
                env: Adsr::new(preset.env, sample_rate),
                pitch: None,
                amp: 0.0,
                started_at: 0,
            })
            .collect();
        PolySynth {
            preset,
            sample_rate,
            voices,
            counter: 0,
        }
    }

    pub fn preset(&self) -> &VoicePreset {
        &self.preset
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8) {
        self.counter += 1;
        let idx = match self.voices.iter().position(|v| v.pitch.is_none() && !v.env.is_active()) {
            Some(i) => i,
            None => match self.voices.iter().position(|v| v.pitch.is_none()) {
                // Releasing voice: reuse the one that started longest ago.
                Some(_) => self
                    .voices
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.pitch.is_none())
                    .min_by_key(|(_, v)| v.started_at)
                    .map(|(i, _)| i)
                    .unwrap(),
                None => self
                    .voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, v)| v.started_at)
                    .map(|(i, _)| i)
                    .unwrap(),
            },
        };
        let v = &mut self.voices[idx];
        if !v.env.is_active() {
            // A free voice is already at zero; resetting keeps renders deterministic.
            v.env.reset();
            v.osc.reset();
        }
        // A *stolen* voice keeps its phase and envelope level: slamming either to zero while
        // it is still sounding is a step discontinuity, i.e. an audible click. The attack
        // simply ramps on from wherever the old note had got to.
        v.osc.set_frequency(midi_to_hz(pitch as f32), self.sample_rate);
        v.amp = (velocity as f32 / 127.0) * self.preset.level;
        v.pitch = Some(pitch);
        v.started_at = self.counter;
        v.env.gate_on();
    }

    /// Release *one* voice: the oldest one still holding `pitch`. Note-ons and note-offs are
    /// 1:1, so releasing every matching voice would make two overlapping notes of the same
    /// pitch cut each other short — the first note's end would silence the second.
    pub fn note_off(&mut self, pitch: u8) {
        let oldest = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.pitch == Some(pitch))
            .min_by_key(|(_, v)| v.started_at)
            .map(|(i, _)| i);
        if let Some(i) = oldest {
            self.voices[i].env.gate_off();
            self.voices[i].pitch = None;
        }
    }

    pub fn all_notes_off(&mut self) {
        for v in &mut self.voices {
            if v.pitch.is_some() {
                v.env.gate_off();
                v.pitch = None;
            }
        }
    }

    pub fn is_silent(&self) -> bool {
        self.voices.iter().all(|v| !v.env.is_active())
    }

    /// Render mono into `out` (additive).
    pub fn render_add(&mut self, out: &mut [f32]) {
        for v in self.voices.iter_mut().filter(|v| v.env.is_active()) {
            for s in out.iter_mut() {
                *s += v.osc.tick() * v.env.tick() * v.amp;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_and_releases() {
        let mut s = PolySynth::new(VoicePreset::named("saw"), 48_000.0, 4);
        assert!(s.is_silent());
        s.note_on(60, 100);
        let mut buf = vec![0.0; 4800];
        s.render_add(&mut buf);
        assert!(buf.iter().any(|x| x.abs() > 0.01));
        s.note_off(60);
        let mut tail = vec![0.0; 48_000];
        s.render_add(&mut tail);
        assert!(s.is_silent());
    }

    #[test]
    fn steals_oldest_voice_when_full() {
        let mut s = PolySynth::new(VoicePreset::named("pad"), 48_000.0, 2);
        s.note_on(60, 100);
        s.note_on(64, 100);
        s.note_on(67, 100);
        let sounding: Vec<u8> = s.voices.iter().filter_map(|v| v.pitch).collect();
        assert_eq!(sounding.len(), 2);
        assert!(!sounding.contains(&60));
    }

    #[test]
    fn velocity_scales_amplitude() {
        let render = |vel: u8| {
            let mut s = PolySynth::new(VoicePreset::named("sine"), 48_000.0, 1);
            s.note_on(69, vel);
            let mut buf = vec![0.0; 9600];
            s.render_add(&mut buf);
            buf.iter().fold(0.0f32, |m, x| m.max(x.abs()))
        };
        assert!(render(127) > render(40) * 2.0);
    }
}
