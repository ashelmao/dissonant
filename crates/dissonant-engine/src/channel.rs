//! A track channel (instrument → tone → reverb → gain/pan) and the master chain
//! (low-cut → 3-band EQ → high-cut → reverb → gain).

use crate::drums::{DrumBank, DrumKit};
use crate::dsp::{linear_to_db, pan_gains, Biquad, Reverb, StereoBiquad};
use crate::synth::PolySynth;
use dissonant_core::{MasterSettings, SequenceTrack, SynthPatch};

/// Voices per synth track (and the preview synth).
pub const SYNTH_POLYPHONY: usize = 12;

#[derive(Debug, Clone)]
pub enum Instrument {
    Synth(PolySynth),
    Drums(DrumKit),
}

impl Instrument {
    /// The instrument a track plays. Drum pads come from `drums` when it has them (rendered off
    /// the audio thread) and are rendered here otherwise.
    pub fn for_track(track: &SequenceTrack, sample_rate: f32, drums: Option<&DrumBank>) -> Self {
        if track.is_drum {
            Instrument::Drums(DrumKit::with_bank(&track.drum_pads, sample_rate, drums))
        } else {
            Instrument::Synth(PolySynth::new(&track.synth, sample_rate, SYNTH_POLYPHONY))
        }
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8) {
        match self {
            Instrument::Synth(s) => s.note_on(pitch, velocity),
            Instrument::Drums(d) => d.note_on(pitch, velocity),
        }
    }
    pub fn note_off(&mut self, pitch: u8) {
        if let Instrument::Synth(s) = self {
            s.note_off(pitch)
        }
    }
    pub fn all_notes_off(&mut self) {
        if let Instrument::Synth(s) = self {
            s.all_notes_off()
        }
    }
    pub fn is_silent(&self) -> bool {
        match self {
            Instrument::Synth(s) => s.is_silent(),
            Instrument::Drums(d) => d.is_silent(),
        }
    }
    pub(crate) fn render_add(&mut self, out: &mut [f32]) {
        match self {
            Instrument::Synth(s) => s.render_add(out),
            Instrument::Drums(d) => d.render_add(out),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub instrument: Instrument,
    /// The patch the synth was last given, to skip `set_patch` when nothing changed.
    synth: SynthPatch,
    tone: Biquad,
    tone_hz: f32,
    reverb: Reverb,
    gain: f32,
    pan: (f32, f32),
    sample_rate: f32,
    mono: Vec<f32>,
    pub peak: f32,
}

impl Channel {
    pub fn new(track: &SequenceTrack, sample_rate: f32, drums: Option<&DrumBank>) -> Self {
        let mut c = Channel {
            instrument: Instrument::for_track(track, sample_rate, drums),
            synth: track.synth,
            tone: Biquad::identity(),
            tone_hz: 0.0,
            reverb: Reverb::new(sample_rate),
            gain: 1.0,
            pan: pan_gains(0.0),
            sample_rate,
            mono: vec![0.0; 8192],
            peak: 0.0,
        };
        c.apply(track, drums);
        c
    }

    /// Update params from the track. A changed synth patch is applied live (held notes keep
    /// sounding); a changed kit re-renders only the pads whose patch changed; switching between
    /// synth and drums builds a new instrument.
    pub fn apply(&mut self, track: &SequenceTrack, drums: Option<&DrumBank>) {
        match (&mut self.instrument, track.is_drum) {
            (Instrument::Synth(s), false) => {
                if track.synth != self.synth {
                    s.set_patch(&track.synth);
                    self.synth = track.synth;
                }
            }
            (Instrument::Drums(kit), true) => kit.update(&track.drum_pads, drums),
            _ => {
                self.instrument = Instrument::for_track(track, self.sample_rate, drums);
                self.synth = track.synth;
            }
        }
        let tone_hz = track.tone.clamp(200.0, 20_000.0) as f32;
        if (tone_hz - self.tone_hz).abs() > 0.5 {
            self.tone.lowpass(self.sample_rate, tone_hz, 0.707);
            self.tone_hz = tone_hz;
        }
        self.reverb.wet = track.reverb_send.clamp(0.0, 1.0) as f32;
        self.gain = track.volume.clamp(0.0, 1.5) as f32;
        self.pan = pan_gains(track.pan as f32);
    }

    /// Render `frames` samples and add them (panned) into the stereo mix buffers.
    pub fn render_add(&mut self, mix_l: &mut [f32], mix_r: &mut [f32]) {
        let frames = mix_l.len();
        if self.mono.len() < frames {
            self.mono.resize(frames, 0.0);
        }
        if self.instrument.is_silent() && self.reverb.wet <= 0.0001 {
            // Nothing to render, but the meter still has to fall back to zero — otherwise it
            // freezes at whatever the last audible block measured and the UI stays lit.
            self.hold_peak(0.0, frames);
            return;
        }
        let mono = &mut self.mono[..frames];
        mono.fill(0.0);
        self.instrument.render_add(mono);
        let (gl, gr) = self.pan;
        let mut peak = 0.0f32;
        for i in 0..frames {
            let s = self.tone.process(mono[i]) * self.gain;
            let (l, r) = self.reverb.process(s * gl, s * gr);
            mix_l[i] += l;
            mix_r[i] += r;
            peak = peak.max(l.abs().max(r.abs()));
        }
        self.hold_peak(peak, frames);
    }

    fn hold_peak(&mut self, block_peak: f32, frames: usize) {
        self.peak = hold(block_peak, self.peak, frames, self.sample_rate);
    }

    /// Let the meter fall for a block this channel didn't render at all (muted and finished
    /// releasing); without it the meter freezes lit at whatever the last audible block measured.
    pub fn decay_meter(&mut self, frames: usize) {
        self.hold_peak(0.0, frames);
    }
}

/// How much of a peak survives one second of silence (≈ -26 dB/s). The decay is per unit time,
/// not per call: the engine renders a sub-block per note event and the device picks the buffer
/// size, so a per-call factor makes the meters fall between 7× faster and slower for identical
/// audio.
const METER_DECAY_PER_SECOND: f32 = 0.05;

/// Below this the UI already draws an empty meter (`peakDb` floors at -60 dB), so snap to zero
/// rather than asymptote — the meter lands on 0 and the audio thread never grinds on denormals.
const METER_FLOOR: f32 = 1e-3;

#[inline]
fn hold(block_peak: f32, previous: f32, frames: usize, sample_rate: f32) -> f32 {
    let decay = METER_DECAY_PER_SECOND.powf(frames as f32 / sample_rate);
    let held = block_peak.max(previous * decay);
    if held < METER_FLOOR {
        0.0
    } else {
        held
    }
}

#[derive(Debug, Clone)]
pub struct Master {
    low_cut: StereoBiquad,
    eq_low: StereoBiquad,
    eq_mid: StereoBiquad,
    eq_high: StereoBiquad,
    high_cut: StereoBiquad,
    reverb: Reverb,
    gain: f32,
    sample_rate: f32,
    settings: MasterSettings,
    pub peak_l: f32,
    pub peak_r: f32,
}

impl Master {
    pub fn new(sample_rate: f32, settings: &MasterSettings) -> Self {
        let mut m = Master {
            low_cut: StereoBiquad::identity(),
            eq_low: StereoBiquad::identity(),
            eq_mid: StereoBiquad::identity(),
            eq_high: StereoBiquad::identity(),
            high_cut: StereoBiquad::identity(),
            reverb: Reverb::new(sample_rate),
            gain: 1.0,
            sample_rate,
            settings: MasterSettings {
                gain: f64::NAN,
                ..settings.clone()
            },
            peak_l: 0.0,
            peak_r: 0.0,
        };
        m.apply(settings);
        m
    }

    pub fn apply(&mut self, s: &MasterSettings) {
        if *s == self.settings {
            return;
        }
        let sr = self.sample_rate;
        self.low_cut.configure(|b| b.highpass(sr, s.low_cut_hz as f32, 0.707));
        self.eq_low.configure(|b| b.low_shelf(sr, 120.0, linear_to_db(s.low_eq as f32), 0.7));
        self.eq_mid.configure(|b| b.peaking(sr, 1_000.0, linear_to_db(s.mid_eq as f32), 0.7));
        self.eq_high.configure(|b| b.high_shelf(sr, 6_000.0, linear_to_db(s.high_eq as f32), 0.7));
        self.high_cut.configure(|b| b.lowpass(sr, s.high_cut_hz as f32, 0.707));
        self.reverb.wet = s.reverb_wet.clamp(0.0, 1.0) as f32;
        self.gain = s.gain.clamp(0.0, 1.5) as f32;
        self.settings = s.clone();
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for i in 0..l.len() {
            let (mut a, mut b) = self.low_cut.process(l[i], r[i]);
            (a, b) = self.eq_low.process(a, b);
            (a, b) = self.eq_mid.process(a, b);
            (a, b) = self.eq_high.process(a, b);
            (a, b) = self.high_cut.process(a, b);
            (a, b) = self.reverb.process(a, b);
            a = (a * self.gain).clamp(-1.0, 1.0);
            b = (b * self.gain).clamp(-1.0, 1.0);
            l[i] = a;
            r[i] = b;
            pl = pl.max(a.abs());
            pr = pr.max(b.abs());
        }
        let frames = l.len();
        self.peak_l = hold(pl, self.peak_l, frames, self.sample_rate);
        self.peak_r = hold(pr, self.peak_r, frames, self.sample_rate);
    }
}
