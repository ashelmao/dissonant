//! Sounds: synth and drum patches, the factory library, and how a track's sound id resolves to
//! a patch.
//!
//! A [`Sound`] is a named patch. Factory sounds ship in `assets/factory_sounds.json` (the UI
//! imports the same file); user sounds live in the app's global library and are *embedded* in
//! every project that uses them ([`ProjectModel::sounds`]), so the engine only ever reads the
//! project. Every field has a default, so a patch in JSON may be partial.

use crate::model::ProjectModel;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ─── Enums ───────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OscWave {
    Sine,
    Triangle,
    #[default]
    Saw,
    Square,
    /// A square with a variable duty cycle (`pulse_width`).
    Pulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterMode {
    Off,
    #[default]
    Lowpass12,
    Lowpass24,
    Highpass12,
    Bandpass,
    Notch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LfoWave {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    SampleHold,
}

/// What an LFO moves. Depth 1 means: pitch ±12 st, cutoff ±4 octaves, amp full tremolo,
/// pulse width ±0.45, FM amount ±1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LfoTarget {
    #[default]
    None,
    Pitch,
    Cutoff,
    Amp,
    PulseWidth,
    Fm,
}

// ─── Synth patch ─────────────────────────────────────────────────────────────────────────────

/// Attack / decay / release in seconds, sustain 0–1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Envelope {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Envelope { attack: 0.008, decay: 0.10, sustain: 0.65, release: 0.14 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OscParams {
    pub on: bool,
    pub wave: OscWave,
    /// -3…3
    pub octave: i32,
    /// -12…12
    pub semi: i32,
    /// Cents, -100…100.
    pub fine: f32,
    /// 0–1
    pub level: f32,
    /// 0.05–0.95, for `Pulse`.
    pub pulse_width: f32,
    /// Stacked copies, 1–8.
    pub unison: u32,
    /// Total unison spread in cents, 0–100.
    pub detune: f32,
    /// Start each note at a random phase instead of 0.
    pub random_phase: bool,
}

impl Default for OscParams {
    fn default() -> Self {
        OscParams {
            on: false,
            wave: OscWave::Saw,
            octave: 0,
            semi: 0,
            fine: 0.0,
            level: 0.8,
            pulse_width: 0.5,
            unison: 1,
            detune: 15.0,
            random_phase: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FilterParams {
    pub mode: FilterMode,
    /// Hz, 20–20000.
    pub cutoff: f32,
    /// 0–1
    pub resonance: f32,
    /// Pre-filter saturation, 0–1.
    pub drive: f32,
    /// How far cutoff follows the note, 0–1 (1 = one octave per octave).
    pub key_track: f32,
    /// Filter envelope depth in octaves, -8…8.
    pub env_amount: f32,
    /// Velocity → cutoff, 0–1.
    pub velocity: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        FilterParams {
            mode: FilterMode::Lowpass12,
            cutoff: 20_000.0,
            resonance: 0.0,
            drive: 0.0,
            key_track: 0.0,
            env_amount: 0.0,
            velocity: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LfoParams {
    pub wave: LfoWave,
    /// Hz, 0.01–30.
    pub rate: f32,
    /// 0–1, scaled per target (see [`LfoTarget`]).
    pub depth: f32,
    pub target: LfoTarget,
    /// Restart the LFO on every note (otherwise it free-runs).
    pub retrigger: bool,
}

impl Default for LfoParams {
    fn default() -> Self {
        LfoParams { wave: LfoWave::Sine, rate: 5.0, depth: 0.0, target: LfoTarget::None, retrigger: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChorusParams {
    /// 0–1
    pub mix: f32,
    /// Hz, 0.05–5.
    pub rate: f32,
    /// 0–1
    pub depth: f32,
}

impl Default for ChorusParams {
    fn default() -> Self {
        ChorusParams { mix: 0.0, rate: 0.8, depth: 0.5 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DelayParams {
    /// 0–1
    pub mix: f32,
    /// Seconds, 0.01–1.5.
    pub time: f32,
    /// 0–0.95
    pub feedback: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        DelayParams { mix: 0.0, time: 0.3, feedback: 0.35 }
    }
}

/// A subtractive + FM synth voice. Signal: 3 oscillators + noise (with FM / ring / sync between
/// osc 1 and 2) → drive → filter → amp envelope → drive / crush → chorus → delay → level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SynthPatch {
    pub osc: [OscParams; 3],
    /// 0–1
    pub noise_level: f32,
    /// 0 = white … 1 = dark.
    pub noise_color: f32,
    /// Osc 2 → osc 1 phase modulation depth, 0–1.
    pub fm_amount: f32,
    /// Crossfade osc 1 toward osc 1 × osc 2, 0–1.
    pub ring_mod: f32,
    /// Osc 2 hard-synced to osc 1.
    pub sync: bool,
    pub filter: FilterParams,
    pub amp_env: Envelope,
    pub filter_env: Envelope,
    /// Semitones, -48…48: the pitch starts this far off and falls back over `pitch_env_decay`.
    pub pitch_env_amount: f32,
    /// Seconds, 0.001–2.
    pub pitch_env_decay: f32,
    pub lfo: [LfoParams; 2],
    /// One voice, legato.
    pub mono: bool,
    /// Portamento seconds, 0–2.
    pub glide: f32,
    /// How much velocity scales level, 0–1.
    pub velocity: f32,
    /// Post-filter saturation, 0–1.
    pub drive: f32,
    /// Bit-depth / sample-rate reduction, 0–1.
    pub crush: f32,
    pub chorus: ChorusParams,
    pub delay: DelayParams,
    /// Output level, 0–1.
    pub level: f32,
}

impl Default for SynthPatch {
    fn default() -> Self {
        let mut osc = [OscParams::default(); 3];
        osc[0].on = true;
        SynthPatch {
            osc,
            noise_level: 0.0,
            noise_color: 0.0,
            fm_amount: 0.0,
            ring_mod: 0.0,
            sync: false,
            filter: FilterParams::default(),
            amp_env: Envelope::default(),
            filter_env: Envelope { attack: 0.005, decay: 0.3, sustain: 0.0, release: 0.2 },
            pitch_env_amount: 0.0,
            pitch_env_decay: 0.05,
            lfo: [LfoParams::default(); 2],
            mono: false,
            glide: 0.0,
            velocity: 1.0,
            drive: 0.0,
            crush: 0.0,
            chorus: ChorusParams::default(),
            delay: DelayParams::default(),
            level: 0.22,
        }
    }
}

// ─── Drum patch ──────────────────────────────────────────────────────────────────────────────

/// The pitched part: an oscillator sweeping `start_hz` → `end_hz`, plus one overtone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DrumBody {
    pub level: f32,
    pub wave: OscWave,
    /// Hz, 20–4000.
    pub start_hz: f32,
    /// Hz, 20–4000.
    pub end_hz: f32,
    /// Seconds for the sweep (exponential time constant).
    pub pitch_decay: f32,
    /// Seconds (amplitude time constant).
    pub decay: f32,
    pub overtone_level: f32,
    /// Overtone frequency as a multiple of the body, 0.5–8.
    pub overtone_ratio: f32,
}

impl Default for DrumBody {
    fn default() -> Self {
        DrumBody {
            level: 1.0,
            wave: OscWave::Sine,
            start_hz: 160.0,
            end_hz: 50.0,
            pitch_decay: 0.035,
            decay: 0.125,
            overtone_level: 0.0,
            overtone_ratio: 1.6,
        }
    }
}

/// Filtered noise, optionally as several quick bursts (a clap).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DrumNoise {
    pub level: f32,
    pub mode: FilterMode,
    /// Hz, 20–20000.
    pub cutoff: f32,
    /// 0–1
    pub resonance: f32,
    /// Seconds, 0–0.1.
    pub attack: f32,
    /// Seconds (amplitude time constant).
    pub decay: f32,
    /// 1–8
    pub bursts: u32,
    /// Seconds between bursts, 0.002–0.05.
    pub burst_spacing: f32,
}

impl Default for DrumNoise {
    fn default() -> Self {
        DrumNoise {
            level: 0.0,
            mode: FilterMode::Off,
            cutoff: 8_000.0,
            resonance: 0.0,
            attack: 0.0,
            decay: 0.06,
            bursts: 1,
            burst_spacing: 0.01,
        }
    }
}

/// An 808-style cluster of six detuned squares through a band-pass (hats, cymbals, cowbells).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DrumMetal {
    pub level: f32,
    /// Hz, 100–1000.
    pub base_hz: f32,
    /// Seconds (amplitude time constant).
    pub decay: f32,
    /// Band-pass centre in Hz.
    pub cutoff: f32,
}

impl Default for DrumMetal {
    fn default() -> Self {
        DrumMetal { level: 0.0, base_hz: 205.0, decay: 0.05, cutoff: 9_000.0 }
    }
}

/// A short transient at the very start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DrumClick {
    pub level: f32,
    /// Seconds, 0.0005–0.02.
    pub decay: f32,
    /// Hz, 500–12000.
    pub tone_hz: f32,
}

impl Default for DrumClick {
    fn default() -> Self {
        DrumClick { level: 0.0, decay: 0.003, tone_hz: 3_000.0 }
    }
}

/// A synthesized one-shot. Rendered to a buffer whenever the patch changes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DrumPatch {
    /// Buffer length in seconds, 0.02–3.
    pub length: f32,
    /// Semitones, -24…24, applied to body, metal and click.
    pub tune: f32,
    pub body: DrumBody,
    pub noise: DrumNoise,
    pub metal: DrumMetal,
    pub click: DrumClick,
    /// 0–1
    pub drive: f32,
    /// 0–1
    pub crush: f32,
    /// Hz, 20–2000.
    pub low_cut: f32,
    /// Hz, 200–20000.
    pub high_cut: f32,
    /// How much velocity scales level, 0–1.
    pub velocity: f32,
    /// 0–1
    pub level: f32,
}

impl Default for DrumPatch {
    fn default() -> Self {
        DrumPatch {
            length: 0.34,
            tune: 0.0,
            body: DrumBody::default(),
            noise: DrumNoise::default(),
            metal: DrumMetal::default(),
            click: DrumClick::default(),
            drive: 0.0,
            crush: 0.0,
            low_cut: 20.0,
            high_cut: 20_000.0,
            velocity: 0.65,
            level: 0.7,
        }
    }
}

// ─── Sounds ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SoundPatch {
    Synth(SynthPatch),
    Drum(DrumPatch),
}

/// A named patch. Factory ids are short slugs (`saw`, `kick-808`); user ids are `u-<uuid>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sound {
    pub id: String,
    pub name: String,
    /// Browser grouping: "bass", "lead", "pad", "kick", "hat", …
    #[serde(default)]
    pub category: String,
    pub patch: SoundPatch,
}

impl Sound {
    pub fn is_drum(&self) -> bool {
        matches!(self.patch, SoundPatch::Drum(_))
    }

    /// Clamp every parameter into its documented range and give a blank name a placeholder.
    pub fn sanitized(mut self) -> Self {
        self.patch = match self.patch {
            SoundPatch::Synth(p) => SoundPatch::Synth(p.sanitized()),
            SoundPatch::Drum(p) => SoundPatch::Drum(p.sanitized()),
        };
        if self.name.trim().is_empty() {
            self.name = "untitled".into();
        }
        self
    }
}

/// One row of a drum track's kit: which note plays which sound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrumPad {
    /// MIDI pitch, unique within the kit.
    pub pitch: u8,
    /// A drum sound id.
    pub sound: String,
    /// Pads sharing a non-zero group cut each other off (closed hat chokes open hat).
    #[serde(default)]
    pub choke: u8,
}

pub const MAX_DRUM_PADS: usize = 16;

/// The kit a new drum track gets, top to bottom. Keeps the original four pitches.
pub fn default_kit() -> Vec<DrumPad> {
    [
        (49, "crash", 0),
        (46, "open-hat", 1),
        (42, "hat", 1),
        (39, "clap", 0),
        (38, "snare", 0),
        (50, "tom-hi", 0),
        (45, "tom-lo", 0),
        (36, "kick", 0),
    ]
    .into_iter()
    .map(|(pitch, sound, choke)| DrumPad { pitch, sound: sound.into(), choke })
    .collect()
}

/// A kit pad with its sound looked up, for the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPad {
    pub pitch: u8,
    pub choke: u8,
    pub patch: DrumPatch,
}

// ─── Factory ─────────────────────────────────────────────────────────────────────────────────

const FACTORY_JSON: &str = include_str!("../assets/factory_sounds.json");

/// Every factory sound, in file order. Panics at first use if the bundled JSON is malformed
/// (a test covers it).
pub fn factory_sounds() -> &'static [Sound] {
    static FACTORY: OnceLock<Vec<Sound>> = OnceLock::new();
    FACTORY.get_or_init(|| {
        let sounds: Vec<Sound> = serde_json::from_str(FACTORY_JSON).expect("factory_sounds.json is valid");
        sounds.into_iter().map(Sound::sanitized).collect()
    })
}

pub fn factory_sound(id: &str) -> Option<&'static Sound> {
    factory_sounds().iter().find(|s| s.id == id)
}

pub fn is_factory(id: &str) -> bool {
    factory_sound(id).is_some()
}

impl ProjectModel {
    /// A sound by id: the project's embedded copy first, then the factory.
    pub fn sound(&self, id: &str) -> Option<&Sound> {
        self.sounds.iter().find(|s| s.id == id).or_else(|| factory_sound(id))
    }

    /// The synth patch a sound id plays; unknown or drum ids fall back to factory `saw`.
    pub fn resolve_synth(&self, id: &str) -> SynthPatch {
        let of = |s: Option<&Sound>| match s.map(|s| &s.patch) {
            Some(SoundPatch::Synth(p)) => Some(*p),
            _ => None,
        };
        of(self.sound(id)).or_else(|| of(factory_sound("saw"))).unwrap_or_default()
    }

    /// The drum patch a sound id plays; unknown or synth ids fall back to the default patch.
    pub fn resolve_drum(&self, id: &str) -> DrumPatch {
        match self.sound(id).map(|s| &s.patch) {
            Some(SoundPatch::Drum(p)) => *p,
            _ => DrumPatch::default(),
        }
    }

    /// Drop embedded sounds that no track uses any more.
    pub fn prune_sounds(&mut self) {
        let tracks = &self.tracks;
        self.sounds.retain(|s| {
            tracks.iter().any(|t| if t.is_drum { t.drum_pads.iter().any(|p| p.sound == s.id) } else { t.voice == s.id })
        });
    }
}

// ─── Sanitizing ──────────────────────────────────────────────────────────────────────────────

fn c(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        lo
    }
}

impl Envelope {
    fn sanitized(self) -> Self {
        Envelope {
            attack: c(self.attack, 0.0, 10.0),
            decay: c(self.decay, 0.001, 10.0),
            sustain: c(self.sustain, 0.0, 1.0),
            release: c(self.release, 0.001, 10.0),
        }
    }
}

impl SynthPatch {
    pub fn sanitized(mut self) -> Self {
        for o in &mut self.osc {
            o.octave = o.octave.clamp(-3, 3);
            o.semi = o.semi.clamp(-12, 12);
            o.fine = c(o.fine, -100.0, 100.0);
            o.level = c(o.level, 0.0, 1.0);
            o.pulse_width = c(o.pulse_width, 0.05, 0.95);
            o.unison = o.unison.clamp(1, 8);
            o.detune = c(o.detune, 0.0, 100.0);
        }
        self.noise_level = c(self.noise_level, 0.0, 1.0);
        self.noise_color = c(self.noise_color, 0.0, 1.0);
        self.fm_amount = c(self.fm_amount, 0.0, 1.0);
        self.ring_mod = c(self.ring_mod, 0.0, 1.0);
        let f = &mut self.filter;
        f.cutoff = c(f.cutoff, 20.0, 20_000.0);
        f.resonance = c(f.resonance, 0.0, 1.0);
        f.drive = c(f.drive, 0.0, 1.0);
        f.key_track = c(f.key_track, 0.0, 1.0);
        f.env_amount = c(f.env_amount, -8.0, 8.0);
        f.velocity = c(f.velocity, 0.0, 1.0);
        self.amp_env = self.amp_env.sanitized();
        self.filter_env = self.filter_env.sanitized();
        self.pitch_env_amount = c(self.pitch_env_amount, -48.0, 48.0);
        self.pitch_env_decay = c(self.pitch_env_decay, 0.001, 2.0);
        for l in &mut self.lfo {
            l.rate = c(l.rate, 0.01, 30.0);
            l.depth = c(l.depth, 0.0, 1.0);
        }
        self.glide = c(self.glide, 0.0, 2.0);
        self.velocity = c(self.velocity, 0.0, 1.0);
        self.drive = c(self.drive, 0.0, 1.0);
        self.crush = c(self.crush, 0.0, 1.0);
        self.chorus.mix = c(self.chorus.mix, 0.0, 1.0);
        self.chorus.rate = c(self.chorus.rate, 0.05, 5.0);
        self.chorus.depth = c(self.chorus.depth, 0.0, 1.0);
        self.delay.mix = c(self.delay.mix, 0.0, 1.0);
        self.delay.time = c(self.delay.time, 0.01, 1.5);
        self.delay.feedback = c(self.delay.feedback, 0.0, 0.95);
        self.level = c(self.level, 0.0, 1.0);
        self
    }
}

impl DrumPatch {
    pub fn sanitized(mut self) -> Self {
        self.length = c(self.length, 0.02, 3.0);
        self.tune = c(self.tune, -24.0, 24.0);
        let b = &mut self.body;
        b.level = c(b.level, 0.0, 1.0);
        b.start_hz = c(b.start_hz, 20.0, 4_000.0);
        b.end_hz = c(b.end_hz, 20.0, 4_000.0);
        b.pitch_decay = c(b.pitch_decay, 0.001, 2.0);
        b.decay = c(b.decay, 0.001, 3.0);
        b.overtone_level = c(b.overtone_level, 0.0, 1.0);
        b.overtone_ratio = c(b.overtone_ratio, 0.5, 8.0);
        let n = &mut self.noise;
        n.level = c(n.level, 0.0, 1.0);
        n.cutoff = c(n.cutoff, 20.0, 20_000.0);
        n.resonance = c(n.resonance, 0.0, 1.0);
        n.attack = c(n.attack, 0.0, 0.1);
        n.decay = c(n.decay, 0.001, 3.0);
        n.bursts = n.bursts.clamp(1, 8);
        n.burst_spacing = c(n.burst_spacing, 0.002, 0.05);
        let m = &mut self.metal;
        m.level = c(m.level, 0.0, 1.0);
        m.base_hz = c(m.base_hz, 100.0, 1_000.0);
        m.decay = c(m.decay, 0.001, 3.0);
        m.cutoff = c(m.cutoff, 200.0, 20_000.0);
        let k = &mut self.click;
        k.level = c(k.level, 0.0, 1.0);
        k.decay = c(k.decay, 0.0005, 0.02);
        k.tone_hz = c(k.tone_hz, 500.0, 12_000.0);
        self.drive = c(self.drive, 0.0, 1.0);
        self.crush = c(self.crush, 0.0, 1.0);
        self.low_cut = c(self.low_cut, 20.0, 2_000.0);
        self.high_cut = c(self.high_cut, 200.0, 20_000.0);
        self.velocity = c(self.velocity, 0.0, 1.0);
        self.level = c(self.level, 0.0, 1.0);
        self
    }
}

/// Validate a kit: 1–16 pads, unique pitches 0–127.
pub fn valid_kit(pads: &[DrumPad]) -> bool {
    if pads.is_empty() || pads.len() > MAX_DRUM_PADS {
        return false;
    }
    let mut seen = [false; 128];
    pads.iter().all(|p| p.pitch < 128 && !std::mem::replace(&mut seen[p.pitch as usize], true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_parses_and_has_legacy_ids() {
        for id in ["saw", "square", "triangle", "sine", "pad", "pluck", "hat", "clap", "snare", "kick"] {
            assert!(factory_sound(id).is_some(), "missing factory sound {id}");
        }
        for pad in default_kit() {
            assert!(factory_sound(&pad.sound).is_some_and(Sound::is_drum), "kit sound {} missing", pad.sound);
        }
        let mut ids: Vec<_> = factory_sounds().iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate factory ids");
    }

    #[test]
    fn partial_patch_json_fills_defaults() {
        let s: Sound = serde_json::from_str(r#"{"id":"x","name":"x","patch":{"kind":"synth","level":0.5}}"#).unwrap();
        match s.patch {
            SoundPatch::Synth(p) => {
                assert_eq!(p.level, 0.5);
                assert!(p.osc[0].on && !p.osc[1].on);
            }
            _ => panic!("expected synth"),
        }
    }

    #[test]
    fn sanitize_clamps_and_kit_validates() {
        let mut p = SynthPatch::default();
        p.filter.cutoff = f32::NAN;
        p.osc[1].unison = 99;
        let p = p.sanitized();
        assert_eq!(p.filter.cutoff, 20.0);
        assert_eq!(p.osc[1].unison, 8);
        assert!(valid_kit(&default_kit()));
        let mut dup = default_kit();
        dup[1].pitch = dup[0].pitch;
        assert!(!valid_kit(&dup));
        assert!(!valid_kit(&[]));
    }
}
