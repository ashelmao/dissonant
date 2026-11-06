//! User settings: one `settings.json` in the app-data dir, loaded at startup and edited through
//! the Settings window. Every field has a default, so an older or hand-edited file still loads.

use dissonant_core::{beats_to_ticks, TimeSignature, TICKS_PER_BEAT};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub audio: AudioSettings,
    pub midi: MidiSettings,
    pub editing: EditingSettings,
    pub export: ExportSettings,
    pub appearance: AppearanceSettings,
    pub metronome: MetronomeSettings,
    pub tuner: TunerSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MetronomeSettings {
    pub on: bool,
    /// 0…1
    pub volume: f32,
    /// Bars of click before recording starts (0 = none).
    pub count_in_bars: u32,
    /// Bars to back up before the playhead when recording starts (0 = none).
    pub pre_roll_bars: u32,
    /// Click during ordinary playback too, not only while record-armed.
    pub during_playback: bool,
}

impl Default for MetronomeSettings {
    fn default() -> Self {
        MetronomeSettings {
            on: false,
            volume: 0.6,
            count_in_bars: 1,
            pre_roll_bars: 0,
            during_playback: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AudioSettings {
    /// Output device name; `None` follows the system default.
    pub device: Option<String>,
    pub sample_rate: Option<u32>,
    /// Frames per callback; `None` lets the host choose.
    pub buffer_size: Option<u32>,
    /// Capture device name; `None` follows the system default.
    pub input_device: Option<String>,
    /// Which of the input device's channels the guitar is plugged into.
    pub input_channel: u16,
    /// Manual latency trim per input device, in ms, keyed by device name.
    ///
    /// Keyed by name rather than kept as one number because the right value is a property of
    /// the interface and its driver, not of the project: plugging the laptop's own mic in
    /// after an audio interface must not inherit the interface's trim. Devices the user has
    /// never trimmed are simply absent and read as 0.
    pub input_trims: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TunerSettings {
    /// What A4 is worth, in Hz. 440 unless you play with people who do not.
    pub a4_hz: f64,
    /// Which named tuning the readout names its strings from; see `ui/src/tuner.ts`.
    pub tuning: String,
}

impl Default for TunerSettings {
    fn default() -> Self {
        TunerSettings {
            a4_hz: 440.0,
            tuning: "standard".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VelocityCurve {
    #[default]
    Linear,
    /// Easier to reach high velocities.
    Soft,
    /// Needs a firm touch for high velocities.
    Hard,
    /// Every note at the default velocity.
    Fixed,
}

impl VelocityCurve {
    /// Map an incoming velocity (1–127) through the curve.
    pub fn apply(self, velocity: u8, fixed: u8) -> u8 {
        let v = velocity.clamp(1, 127) as f64 / 127.0;
        let out = match self {
            VelocityCurve::Linear => v,
            VelocityCurve::Soft => v.sqrt(),
            VelocityCurve::Hard => v * v,
            VelocityCurve::Fixed => return fixed.clamp(1, 127),
        };
        ((out * 127.0).round() as u8).clamp(1, 127)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MidiSettings {
    /// Input to open at startup and whenever it reappears.
    pub default_input: Option<String>,
    pub auto_reconnect: bool,
    pub velocity_curve: VelocityCurve,
    /// 1–16, or `None` for every channel.
    pub channel: Option<u8>,
    /// Octaves added to incoming notes.
    pub octave_offset: i8,
}

impl Default for MidiSettings {
    fn default() -> Self {
        MidiSettings {
            default_input: None,
            auto_reconnect: true,
            velocity_curve: VelocityCurve::Linear,
            channel: None,
            octave_offset: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum NewProjectKind {
    /// A I–IV–V–vi progression and a drum track.
    #[default]
    Starter,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AltKey {
    /// ⌥ while dragging ignores the grid (industry convention).
    #[default]
    NoSnap,
    /// ⌥-drag on empty space paints notes (the original mapping).
    Paint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EditingSettings {
    /// Grid / snap in ticks (240 = a sixteenth).
    #[serde(deserialize_with = "de_grid_ticks")]
    pub default_grid: i64,
    #[serde(deserialize_with = "de_grid_ticks")]
    pub default_note_length: i64,
    pub default_velocity: u8,
    pub default_pattern_bars: u32,
    /// Song length of a new project (new, starter, vibe), in bars.
    pub default_song_bars: u32,
    pub default_tempo: f64,
    pub default_time_signature: TimeSignature,
    pub new_project: NewProjectKind,
    pub audition_on_place: bool,
    /// Ask before clearing notes, deleting tracks or patterns.
    pub confirm_destructive: bool,
    pub alt_key: AltKey,
    /// Snap note starts to chord changes when within a grid step of one.
    pub snap_to_chords: bool,
    /// Open the new-project screen (vibes, recent files) when the app starts.
    pub show_welcome: bool,
}

/// The smallest grid that can be ticks rather than beats. `MIN_NOTE_TICKS` is 60, so a real
/// tick grid is never below that; the old beat grids never reached 32 beats.
const SMALLEST_TICK_GRID: f64 = 32.0;

/// Grids were written in beats before the tick migration, so a settings file from an older
/// build carries values like 0.25. Read as a number and fold the beat ones up to ticks; a
/// non-positive or non-finite value is left at 0 for [`Settings::sanitized`] to replace.
fn de_grid_ticks<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let value = f64::deserialize(d)?;
    if !value.is_finite() || value <= 0.0 {
        return Ok(0);
    }
    Ok(if value < SMALLEST_TICK_GRID { beats_to_ticks(value) } else { value.round() as i64 })
}

impl Default for EditingSettings {
    fn default() -> Self {
        EditingSettings {
            default_grid: TICKS_PER_BEAT / 4,
            default_note_length: TICKS_PER_BEAT / 4,
            default_velocity: 100,
            default_pattern_bars: 4,
            default_song_bars: dissonant_core::DEFAULT_SONG_BARS,
            default_tempo: 120.0,
            default_time_signature: TimeSignature::default(),
            new_project: NewProjectKind::Starter,
            audition_on_place: true,
            confirm_destructive: true,
            alt_key: AltKey::NoSnap,
            snap_to_chords: false,
            show_welcome: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportSettings {
    pub sample_rate: u32,
    /// 16, 24 or 32 (float).
    pub bit_depth: u16,
    pub dither: bool,
    pub normalize: bool,
    pub normalize_db: f32,
    pub tail_seconds: f64,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            sample_rate: 44_100,
            bit_depth: 16,
            dither: true,
            normalize: false,
            normalize_db: -1.0,
            tail_seconds: 1.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TierColors {
    pub chord_tone: String,
    pub tension: String,
    pub dissonance: String,
}

impl Default for TierColors {
    fn default() -> Self {
        TierColors {
            chord_tone: "#3dffb0".into(),
            tension: "#ff9d2a".into(),
            dissonance: "#ff3b30".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppearanceSettings {
    /// 0.9 … 1.5
    pub ui_scale: f64,
    /// Piano-roll row height in px (14 / 18 / 22).
    pub row_height: u32,
    pub reduced_motion: bool,
    pub tier_colors: TierColors,
    pub accent: String,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        AppearanceSettings {
            ui_scale: 1.0,
            row_height: 18,
            reduced_motion: false,
            tier_colors: TierColors::default(),
            accent: "#b48cff".into(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|json| serde_json::from_str::<Settings>(&json).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Clamp everything into the ranges the rest of the app assumes.
    pub fn sanitized(mut self) -> Settings {
        let e = &mut self.editing;
        if e.default_grid <= 0 {
            e.default_grid = TICKS_PER_BEAT / 4;
        }
        if e.default_note_length <= 0 {
            e.default_note_length = e.default_grid;
        }
        e.default_velocity = e.default_velocity.clamp(1, 127);
        e.default_pattern_bars = e.default_pattern_bars.clamp(1, 64);
        e.default_song_bars = e.default_song_bars.clamp(1, 999);
        if !e.default_tempo.is_finite() {
            e.default_tempo = 120.0;
        }
        e.default_tempo = e.default_tempo.clamp(20.0, 300.0);
        if !(1..=16).contains(&e.default_time_signature.numerator) || ![2, 4, 8, 16].contains(&e.default_time_signature.denominator) {
            e.default_time_signature = TimeSignature::default();
        }
        let x = &mut self.export;
        if ![44_100, 48_000, 88_200, 96_000].contains(&x.sample_rate) {
            x.sample_rate = 44_100;
        }
        if ![16, 24, 32].contains(&x.bit_depth) {
            x.bit_depth = 16;
        }
        if !x.normalize_db.is_finite() {
            x.normalize_db = -1.0;
        }
        x.normalize_db = x.normalize_db.clamp(-24.0, 0.0);
        if !x.tail_seconds.is_finite() {
            x.tail_seconds = 1.5;
        }
        x.tail_seconds = x.tail_seconds.clamp(0.0, 30.0);
        let a = &mut self.audio;
        if let Some(n) = a.buffer_size {
            if !(16..=8192).contains(&n) {
                a.buffer_size = None;
            }
        }
        if let Some(r) = a.sample_rate {
            if !(8_000..=384_000).contains(&r) {
                a.sample_rate = None;
            }
        }
        // No interface exposes more than this, and a wild value would silently fall back to
        // the last channel rather than telling the user anything.
        a.input_channel = a.input_channel.min(63);
        // A trim is the player's own measurement, so it is kept whatever it says — but a
        // nonsense one would move every take by half a bar, and the window either side of the
        // measured latency is the only range a real interface can hide in.
        a.input_trims
            .retain(|name, ms| !name.is_empty() && ms.is_finite() && (-50.0..=200.0).contains(&*ms));
        let t = &mut self.tuner;
        if !t.a4_hz.is_finite() {
            t.a4_hz = 440.0;
        }
        // Wide enough for baroque pitch at one end and the sharp orchestras at the other.
        t.a4_hz = t.a4_hz.clamp(390.0, 460.0);
        if t.tuning.trim().is_empty() {
            t.tuning = "standard".into();
        }
        let m = &mut self.midi;
        if let Some(c) = m.channel {
            if !(1..=16).contains(&c) {
                m.channel = None;
            }
        }
        m.octave_offset = m.octave_offset.clamp(-4, 4);
        let p = &mut self.appearance;
        if !p.ui_scale.is_finite() {
            p.ui_scale = 1.0;
        }
        p.ui_scale = p.ui_scale.clamp(0.8, 1.6);
        p.row_height = p.row_height.clamp(12, 28);
        let ok = |c: &String| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
        let d = TierColors::default();
        if !ok(&p.tier_colors.chord_tone) {
            p.tier_colors.chord_tone = d.chord_tone;
        }
        if !ok(&p.tier_colors.tension) {
            p.tier_colors.tension = d.tension;
        }
        if !ok(&p.tier_colors.dissonance) {
            p.tier_colors.dissonance = d.dissonance;
        }
        if !ok(&p.accent) {
            p.accent = "#b48cff".into();
        }
        let mt = &mut self.metronome;
        if !mt.volume.is_finite() {
            mt.volume = 0.6;
        }
        mt.volume = mt.volume.clamp(0.0, 1.0);
        mt.count_in_bars = mt.count_in_bars.min(4);
        mt.pre_roll_bars = mt.pre_roll_bars.min(4);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_partial_files_fill_in() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        let partial: Settings = serde_json::from_str(r#"{"editing":{"defaultVelocity":90},"audio":{"bufferSize":256}}"#).unwrap();
        assert_eq!(partial.editing.default_velocity, 90);
        assert_eq!(partial.editing.default_grid, TICKS_PER_BEAT / 4);
        assert_eq!(partial.audio.buffer_size, Some(256));
        assert!(partial.midi.auto_reconnect);
        assert_eq!(partial.editing.default_song_bars, 32, "a settings file from before song length still loads");
        let set: Settings = serde_json::from_str(r#"{"editing":{"defaultSongBars":48}}"#).unwrap();
        assert_eq!(set.editing.default_song_bars, 48);
        assert!(serde_json::to_string(&s).unwrap().contains("\"defaultSongBars\":32"));
    }

    #[test]
    fn sanitize_clamps_bad_values() {
        let mut s = Settings::default();
        s.editing.default_velocity = 0;
        s.editing.default_tempo = f64::NAN;
        s.export.bit_depth = 12;
        s.audio.buffer_size = Some(3);
        s.appearance.tier_colors.tension = "orange".into();
        s.editing.default_song_bars = 0;
        let s = s.sanitized();
        assert_eq!(s.editing.default_song_bars, 1);
        assert_eq!(s.editing.default_velocity, 1);
        assert_eq!(s.editing.default_tempo, 120.0);
        assert_eq!(s.export.bit_depth, 16);
        assert_eq!(s.audio.buffer_size, None);
        assert_eq!(s.appearance.tier_colors.tension, "#ff9d2a");
    }

    /// The per-device trim and the tuner are the newest sections, so an older settings file
    /// has neither — and a hand-edited one can have nonsense in both.
    #[test]
    fn trims_and_the_tuner_default_and_clamp() {
        let old: Settings = serde_json::from_str(r#"{"audio":{"inputChannel":1}}"#).unwrap();
        assert!(old.audio.input_trims.is_empty());
        assert_eq!(old.tuner.a4_hz, 440.0);
        assert_eq!(old.tuner.tuning, "standard");

        let mut s = Settings::default();
        s.audio.input_trims.insert("good interface".into(), 7.5);
        s.audio.input_trims.insert("absurd".into(), 5_000.0);
        s.audio.input_trims.insert("not a number".into(), f64::NAN);
        s.audio.input_trims.insert(String::new(), 3.0);
        s.tuner.a4_hz = 9_000.0;
        s.tuner.tuning = "  ".into();
        let s = s.sanitized();
        assert_eq!(s.audio.input_trims.get("good interface"), Some(&7.5));
        assert_eq!(s.audio.input_trims.len(), 1, "only the measurable trim survives");
        assert_eq!(s.tuner.a4_hz, 460.0);
        assert_eq!(s.tuner.tuning, "standard");
    }

    /// Settings written before the tick migration stored grids in beats. They have to come
    /// back as the same musical grid, not as 0 or as a tick count of 0.25.
    #[test]
    fn grids_written_in_beats_migrate_to_ticks() {
        let old: Settings =
            serde_json::from_str(r#"{"editing":{"defaultGrid":0.25,"defaultNoteLength":0.5}}"#).unwrap();
        assert_eq!(old.editing.default_grid, 240);
        assert_eq!(old.editing.default_note_length, 480);

        // A file already written in ticks is left exactly as it is.
        let new: Settings =
            serde_json::from_str(r#"{"editing":{"defaultGrid":120,"defaultNoteLength":3840}}"#).unwrap();
        assert_eq!(new.editing.default_grid, 120);
        assert_eq!(new.editing.default_note_length, 3840);

        // Junk falls back to the default rather than to a zero grid.
        let bad: Settings = serde_json::from_str(r#"{"editing":{"defaultGrid":-1}}"#).unwrap();
        assert_eq!(bad.sanitized().editing.default_grid, TICKS_PER_BEAT / 4);
    }

    #[test]
    fn velocity_curves() {
        assert_eq!(VelocityCurve::Linear.apply(64, 100), 64);
        assert!(VelocityCurve::Soft.apply(64, 100) > 64);
        assert!(VelocityCurve::Hard.apply(64, 100) < 64);
        assert_eq!(VelocityCurve::Fixed.apply(5, 100), 100);
        assert_eq!(VelocityCurve::Hard.apply(127, 100), 127);
    }
}
