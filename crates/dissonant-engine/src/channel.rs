//! A track channel (instrument → tone → inserts → reverb/gain/pan) and the master chain
//! (master inserts → low-cut → 3-band EQ → high-cut → reverb → gain → limiter).

use crate::audio::{AudioLane, BlockContext};
use crate::drums::{DrumBank, DrumKit};
use crate::dsp::{db_to_linear, linear_to_db, pan_gains, Limiter, Reverb, StereoBiquad};
use crate::effects::NativeEffect;
use crate::engine::{NoteShape, Slide, VoiceTag};
use crate::synth::PolySynth;
use dissonant_core::{MasterSettings, SequenceTrack, SynthPatch};
use dissonant_plugins::PluginAudioUnit;
use uuid::Uuid;

/// Voices per synth track (and the preview synth).
pub const SYNTH_POLYPHONY: usize = 12;

/// What makes a track's sound. The built-in voices place themselves in stereo from each note's
/// own pan; a plugin renders its own stereo, so the channel feeds it the track pan rather than
/// applying one.
pub enum Instrument {
    /// Boxed for the same reason as `Plugin`: a patch-driven synth carries its whole voice
    /// pool inline, and a drum or audio channel would otherwise pay for it.
    Synth(Box<PolySynth>),
    Drums(DrumKit),
    /// A hosted plugin. Boxed because it is an order of magnitude larger than the other two and
    /// every `Channel` would otherwise carry that size whether or not it hosts anything.
    Plugin(Box<PluginAudioUnit>),
    /// An audio track has no note source: its sound comes from its [`AudioLane`]. Every note
    /// call is a no-op and it renders nothing of its own.
    Silent,
}

impl std::fmt::Debug for Instrument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instrument::Synth(s) => f.debug_tuple("Synth").field(s).finish(),
            Instrument::Drums(d) => f.debug_tuple("Drums").field(d).finish(),
            // A plugin's innards belong to the plugin; naming it is all a log line needs.
            Instrument::Plugin(_) => f.write_str("Plugin(..)"),
            Instrument::Silent => f.write_str("Silent"),
        }
    }
}

impl Instrument {
    /// The instrument a track plays, with no plugin involved: an audio track has none, a drum
    /// track takes its pads' drums from `drums` when they are already rendered there (off the
    /// audio thread) and renders them here otherwise, and a synth track plays its patch.
    pub fn for_track(track: &SequenceTrack, sample_rate: f32, drums: Option<&DrumBank>) -> Self {
        if track.is_audio {
            Instrument::Silent
        } else if track.is_drum {
            Instrument::Drums(DrumKit::with_bank(&track.drum_pads, sample_rate, drums))
        } else {
            Instrument::Synth(Box::new(PolySynth::new(
                &track.synth,
                sample_rate,
                SYNTH_POLYPHONY,
            )))
        }
    }

    pub fn note_on(&mut self, pitch: u8, velocity: u8, shape: NoteShape) {
        match self {
            Instrument::Synth(s) => s.note_on(pitch, velocity, shape),
            Instrument::Drums(d) => d.note_on(pitch, velocity, shape),
            // A plugin owns its own expression: MIDI is all the contract there is.
            Instrument::Plugin(p) => p.note_on(pitch, velocity),
            Instrument::Silent => {}
        }
    }

    /// A sequenced note, released by [`Instrument::note_off_tagged`] with the same tag. Only
    /// the built-in synth knows about tags and slides: a drum hit is a one-shot, and a plugin
    /// is addressed by pitch, so a slide note on a plugin track plays as an ordinary note.
    pub fn note_on_tagged(
        &mut self,
        pitch: u8,
        velocity: u8,
        shape: NoteShape,
        tag: VoiceTag,
        slide: Option<Slide>,
    ) {
        match self {
            Instrument::Synth(s) => s.note_on_tagged(pitch, velocity, shape, tag, slide),
            other => other.note_on(pitch, velocity, shape),
        }
    }

    /// End the sequenced note `tag`, which a plugin knows only as `pitch`.
    pub fn note_off_tagged(&mut self, pitch: u8, tag: VoiceTag) {
        match self {
            Instrument::Synth(s) => s.note_off_tagged(tag),
            other => other.note_off(pitch),
        }
    }

    /// Tell the built-in voices the track's tone cutoff, which per-note Mod X sweeps around.
    fn set_tone_hz(&mut self, hz: f32) {
        match self {
            Instrument::Synth(s) => s.set_tone_hz(hz),
            Instrument::Drums(d) => d.set_tone_hz(hz),
            Instrument::Plugin(_) | Instrument::Silent => {}
        }
    }
    pub fn note_off(&mut self, pitch: u8) {
        match self {
            Instrument::Synth(s) => s.note_off(pitch),
            Instrument::Plugin(p) => p.note_off(pitch),
            // A drum hit is a one-shot: it has no sustain to release.
            Instrument::Drums(_) | Instrument::Silent => {}
        }
    }
    pub fn all_notes_off(&mut self) {
        match self {
            Instrument::Synth(s) => s.all_notes_off(),
            Instrument::Plugin(p) => p.all_notes_off(),
            Instrument::Drums(_) | Instrument::Silent => {}
        }
    }
    pub fn is_silent(&self) -> bool {
        match self {
            Instrument::Synth(s) => s.is_silent(),
            Instrument::Drums(d) => d.is_silent(),
            Instrument::Plugin(p) => p.is_silent(),
            Instrument::Silent => true,
        }
    }
    pub(crate) fn render_add(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        match self {
            Instrument::Synth(s) => s.render_add(out_l, out_r),
            Instrument::Drums(d) => d.render_add(out_l, out_r),
            // A plugin goes through `Channel::render_add`'s own path.
            Instrument::Plugin(_) | Instrument::Silent => {}
        }
    }
}

/// What a slot actually runs: a hosted plugin, or one of the stock effects.
///
/// Both are built on the app thread and handed over whole, which is why they sit behind one
/// enum rather than a trait object — the audio thread only ever calls `process` on them, and a
/// two-armed match is cheaper and clearer than a vtable.
pub enum InsertUnit {
    Plugin(Box<PluginAudioUnit>),
    /// Boxed so the enum stays the size of a pointer: a reverb's tank and a delay's line are
    /// an order of magnitude larger than a plugin handle, and a chain is an array of these.
    Native(Box<NativeEffect>),
}

/// One effect slot in a chain, on a track or on the master.
///
/// `unit` is `None` when the plugin the project names is not installed: the slot keeps its place
/// so the chain's order (and the name the UI shows) survives a round trip through a machine that
/// cannot load it. A native effect is never missing.
pub struct InsertSlot {
    pub id: Uuid,
    pub enabled: bool,
    pub unit: Option<InsertUnit>,
}

impl std::fmt::Debug for InsertSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loaded = match &self.unit {
            Some(InsertUnit::Plugin(_)) => "plugin",
            Some(InsertUnit::Native(n)) => n.kind().slug(),
            None => "none",
        };
        f.debug_struct("InsertSlot")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .field("loaded", &loaded)
            .finish()
    }
}

impl InsertSlot {
    /// A slot starts enabled; a bypassed one is set so by its own command right after.
    pub fn new(id: Uuid, unit: Option<InsertUnit>) -> Self {
        InsertSlot {
            id,
            enabled: true,
            unit,
        }
    }

    /// The unit this slot would run this block, or `None` when there is nothing to run.
    #[inline]
    fn active(&mut self) -> Option<&mut InsertUnit> {
        if self.enabled {
            self.unit.as_mut()
        } else {
            None
        }
    }

    /// Hand one parameter to whatever is loaded. A plugin takes its own id space; a native
    /// effect takes [`dissonant_core::effects::NativeParam::id`].
    pub fn set_param(&mut self, param_id: u32, value: f64) {
        match &mut self.unit {
            Some(InsertUnit::Plugin(p)) => p.set_param(param_id, value),
            Some(InsertUnit::Native(n)) => n.set_param(param_id, value),
            None => {}
        }
    }

    /// Whether the slot has stopped making sound of its own. A bypassed or empty slot is silent
    /// by definition; a loaded effect is silent once its tail has run out.
    fn is_silent(&self) -> bool {
        match &self.unit {
            Some(InsertUnit::Plugin(p)) if self.enabled => p.is_silent(),
            Some(InsertUnit::Native(n)) if self.enabled => n.is_silent(),
            _ => true,
        }
    }
}

/// Run a chain in place over one block. A bypassed slot, or one whose plugin is missing, is
/// skipped outright: the signal passes through it untouched and unconverted.
pub fn run_inserts(chain: &mut [InsertSlot], l: &mut [f32], r: &mut [f32], beat_seconds: f32) {
    for slot in chain {
        match slot.active() {
            Some(InsertUnit::Plugin(unit)) => unit.process_replace(l, r),
            Some(InsertUnit::Native(unit)) => unit.process(l, r, beat_seconds),
            None => {}
        }
    }
}

/// Whether every slot in a chain has finished ringing, so the chain can be skipped for a block
/// whose input is already silent.
pub fn chain_is_silent(chain: &[InsertSlot]) -> bool {
    chain.iter().all(InsertSlot::is_silent)
}

/// One sub-block's worth of automation for a channel strip: the parameter's value at the first
/// and last frame, so the strip can ramp between them instead of stepping once per block.
/// `None` means the track has no lane for that parameter and its static value stands.
///
/// `tone_hz` carries one value, not a pair: it is a filter cutoff, and re-deriving biquad
/// coefficients per sample would cost more than the whole strip. Stepping the coefficients
/// does not click — the filter's state carries across the change, only its response moves —
/// so a sub-block is fine enough granularity for it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChannelAuto {
    pub volume: Option<(f32, f32)>,
    pub pan: Option<(f32, f32)>,
    pub reverb_send: Option<(f32, f32)>,
    pub tone_hz: Option<f32>,
}

/// Not `Clone`: a hosted plugin instance cannot be duplicated, and nothing copies a live
/// channel — the engine moves them between sequences instead.
#[derive(Debug)]
pub struct Channel {
    pub instrument: Instrument,
    /// The patch the synth was last given, to skip `set_patch` when nothing changed.
    synth: SynthPatch,
    is_drum: bool,
    is_audio: bool,
    /// The track's tone filter. Stereo, because the built-in voices now place themselves from
    /// each note's pan and the two sides are no longer the same signal.
    tone: StereoBiquad,
    tone_hz: f32,
    reverb: Reverb,
    gain: f32,
    pan: (f32, f32),
    /// This sub-block's automation ramps, already in strip units: a gain pair, a pair of
    /// pan gain pairs and a reverb-send pair. Set before every render, so a lane that goes
    /// away simply stops being written here.
    auto_gain: Option<(f32, f32)>,
    auto_pan: Option<((f32, f32), (f32, f32))>,
    auto_wet: Option<(f32, f32)>,
    sample_rate: f32,
    /// Stereo scratch: the instrument renders here, then the channel strip runs over it.
    stereo: (Vec<f32>, Vec<f32>),
    /// This track's effect chain, pre-fader. Filled by the engine's insert commands, never by
    /// [`Channel::apply`]: loading a plugin is far too slow for the audio deadline, so a
    /// sequence rebuild must leave a live chain exactly where it is.
    pub inserts: Vec<InsertSlot>,
    /// Set on an audio track: where its clips and monitored input are rendered. Boxed for the
    /// same reason as `Instrument::Plugin` — a rig is far larger than the rest of a channel.
    pub audio: Option<Box<AudioLane>>,
    pub peak: f32,
}

impl Channel {
    pub fn new(track: &SequenceTrack, sample_rate: f32, drums: Option<&DrumBank>) -> Self {
        let mut c = Channel {
            instrument: Instrument::for_track(track, sample_rate, drums),
            synth: track.synth,
            is_drum: track.is_drum,
            is_audio: track.is_audio,
            tone: StereoBiquad::identity(),
            tone_hz: 0.0,
            reverb: Reverb::new(sample_rate),
            gain: 1.0,
            pan: pan_gains(0.0),
            auto_gain: None,
            auto_pan: None,
            auto_wet: None,
            sample_rate,
            stereo: (vec![0.0; 8192], vec![0.0; 8192]),
            inserts: Vec::new(),
            audio: track
                .is_audio
                .then(|| Box::new(AudioLane::new(track.id, sample_rate))),
            peak: 0.0,
        };
        c.apply(track, drums);
        c
    }

    /// Update params from the track. A changed synth patch is applied live (held notes keep
    /// sounding); a changed kit re-renders only the pads whose patch changed; switching
    /// between kinds builds a new instrument.
    ///
    /// A plugin is never swapped away here: it is installed by `EngineCommand::SetInstrument`
    /// from the app thread, because loading one is far too slow for the audio deadline, and a
    /// sequence rebuild must not silently replace it with the track's own sound.
    pub fn apply(&mut self, track: &SequenceTrack, drums: Option<&DrumBank>) {
        match &mut self.instrument {
            Instrument::Plugin(_) => {}
            Instrument::Synth(s) if !track.is_drum && !track.is_audio => {
                if track.synth != self.synth {
                    s.set_patch(&track.synth);
                    self.synth = track.synth;
                }
            }
            Instrument::Drums(kit) if track.is_drum => kit.update(&track.drum_pads, drums),
            Instrument::Silent if track.is_audio => {}
            _ => {
                self.instrument = Instrument::for_track(track, self.sample_rate, drums);
                self.synth = track.synth;
            }
        }
        self.is_drum = track.is_drum;
        self.is_audio = track.is_audio;
        match (track.is_audio, &mut self.audio) {
            (true, Some(lane)) => lane.apply(track),
            (true, slot @ None) => {
                let mut lane = Box::new(AudioLane::new(track.id, self.sample_rate));
                lane.apply(track);
                *slot = Some(lane);
            }
            (false, slot) => *slot = None,
        }
        let tone_hz = track.tone.clamp(200.0, 20_000.0) as f32;
        if (tone_hz - self.tone_hz).abs() > 0.5 {
            self.tone
                .configure(|b| b.lowpass(self.sample_rate, tone_hz, 0.707));
            self.tone_hz = tone_hz;
        }
        // A note's Mod X is an offset from the track's own voicing, so the voices need to know
        // what that voicing currently is. Pushed on every apply, including the one that just
        // built a fresh instrument above.
        self.instrument.set_tone_hz(self.tone_hz);
        self.reverb.wet = track.reverb_send.clamp(0.0, 1.0) as f32;
        self.gain = track.volume.clamp(0.0, 1.5) as f32;
        self.pan = pan_gains(track.pan as f32);
    }

    /// Hand the strip this sub-block's automation. Called once per sub-block per channel, so
    /// it stays free of allocation and of anything that could block.
    pub fn set_auto(&mut self, auto: ChannelAuto) {
        self.auto_gain = auto.volume;
        self.auto_pan = auto
            .pan
            .map(|(a, b)| (pan_gains(a.clamp(-1.0, 1.0)), pan_gains(b.clamp(-1.0, 1.0))));
        self.auto_wet = auto.reverb_send.map(|(a, b)| (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0)));
        if let Some(hz) = auto.tone_hz {
            let hz = hz.clamp(200.0, 20_000.0);
            // The same 0.5 Hz deadband `apply` uses: re-deriving coefficients is the expensive
            // part, and a cutoff that has not really moved does not need new ones.
            if (hz - self.tone_hz).abs() > 0.5 {
                self.tone
                    .configure(|b| b.lowpass(self.sample_rate, hz, 0.707));
                self.tone_hz = hz;
                self.instrument.set_tone_hz(hz);
            }
        }
    }

    /// Render `frames` samples and add them into the stereo mix buffers.
    pub fn render_add(&mut self, mix_l: &mut [f32], mix_r: &mut [f32], ctx: &BlockContext<'_>) {
        let frames = mix_l.len();

        // An audio track's sound comes from its lane, not from an instrument. It takes the
        // plugin path's treatment — inserts, gain, pan and reverb on both sides, then peak
        // tracking — so it meters, pans and mutes exactly like any other track.
        if self.audio.is_some() {
            self.clear_scratch(frames);
            let wrote = {
                let (al, ar) = (&mut self.stereo.0[..frames], &mut self.stereo.1[..frames]);
                // `is_some` above, so the lane is there; taking it out of the `if let` keeps the
                // scratch borrow and the lane borrow from overlapping.
                match &mut self.audio {
                    Some(lane) => lane.render_add(al, ar, ctx, self.sample_rate),
                    None => false,
                }
            };
            // Nothing played, no send tail to flush and no insert still ringing: let the meter
            // fall and stop.
            if !wrote && self.reverb.wet <= 0.0001 && chain_is_silent(&self.inserts) {
                self.hold_peak(0.0, frames);
                return;
            }
            self.strip(frames, mix_l, mix_r, ctx.beat_seconds);
            return;
        }

        if self.instrument.is_silent()
            && self.reverb.wet <= 0.0001
            && chain_is_silent(&self.inserts)
        {
            // Nothing to render, but the meter still has to fall back to zero — otherwise it
            // freezes at whatever the last audible block measured and the UI stays lit.
            self.hold_peak(0.0, frames);
            return;
        }
        self.clear_scratch(frames);
        if let Instrument::Plugin(plugin) = &mut self.instrument {
            // A plugin produces its own stereo image, so panning scales the two sides it gives
            // us rather than placing a mono source. The channel's tone filter is left for the
            // built-in voices.
            plugin.render_add(&mut self.stereo.0[..frames], &mut self.stereo.1[..frames]);
            self.strip(frames, mix_l, mix_r, ctx.beat_seconds);
            return;
        }
        {
            let (il, ir) = (&mut self.stereo.0[..frames], &mut self.stereo.1[..frames]);
            self.instrument.render_add(il, ir);
            // Per-note pan has already placed the voice between the two sides; the tone filter
            // is stereo so the two sides are filtered independently.
            for i in 0..frames {
                let (tl, tr) = self.tone.process(il[i], ir[i]);
                il[i] = tl;
                ir[i] = tr;
            }
        }
        self.strip(frames, mix_l, mix_r, ctx.beat_seconds);
    }

    /// Grow the stereo scratch to `frames` and clear it.
    fn clear_scratch(&mut self, frames: usize) {
        if self.stereo.0.len() < frames {
            self.stereo.0.resize(frames, 0.0);
            self.stereo.1.resize(frames, 0.0);
        }
        self.stereo.0[..frames].fill(0.0);
        self.stereo.1[..frames].fill(0.0);
    }

    /// The rest of the channel strip over whatever is in the scratch: inserts → gain → pan →
    /// reverb send → meter, added into the mix. Inserts sit here, pre-fader, which is what an
    /// insert means in every other DAW: the chain hears the instrument at its own level.
    ///
    /// Gain, pan and send walk from their value at the first frame to their value at the last
    /// one rather than stepping. With no automation the two ends are equal and the increments
    /// are zero, so there is one loop and no branch per sample; with automation the parameter
    /// is continuous across sub-block and block boundaries alike, which is the difference
    /// between a fade and a staircase of clicks.
    fn strip(
        &mut self,
        frames: usize,
        mix_l: &mut [f32],
        mix_r: &mut [f32],
        beat_seconds: f32,
    ) {
        let peak = {
            let (g0, g1) = self.auto_gain.unwrap_or((self.gain, self.gain));
            let ((l0, r0), (l1, r1)) = self.auto_pan.unwrap_or((self.pan, self.pan));
            let (w0, w1) = self.auto_wet.unwrap_or((self.reverb.wet, self.reverb.wet));
            let step = 1.0 / frames.max(1) as f32;
            let (dg, dl, dr, dw) = (
                (g1 - g0) * step,
                (l1 - l0) * step,
                (r1 - r0) * step,
                (w1 - w0) * step,
            );
            let (mut gain, mut gl, mut gr, mut wet) = (g0, l0, r0, w0);
            let (sl, sr) = (&mut self.stereo.0[..frames], &mut self.stereo.1[..frames]);
            run_inserts(&mut self.inserts, sl, sr, beat_seconds);
            let reverb = &mut self.reverb;
            let mut peak = 0.0f32;
            for i in 0..frames {
                reverb.wet = wet;
                let (l, r) = reverb.process(sl[i] * gain * gl, sr[i] * gain * gr);
                mix_l[i] += l;
                mix_r[i] += r;
                peak = peak.max(l.abs().max(r.abs()));
                gain += dg;
                gl += dl;
                gr += dr;
                wet += dw;
            }
            // The send level the silence check reads has to be where the ramp ended, not where
            // it started, or a lane that just faded a send in would be ignored next block.
            reverb.wet = w1;
            peak
        };
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
    limiter: Limiter,
    /// `settings.ceiling_db` as linear amplitude.
    ceiling: f32,
    gain: f32,
    /// This block's gain ramp from the master gain lane, overriding `gain` while it is set.
    auto_gain: Option<(f32, f32)>,
    sample_rate: f32,
    settings: MasterSettings,
    pub peak_l: f32,
    pub peak_r: f32,
    /// Whether the last [`Master::process`] saw a sample over the ceiling *before* limiting.
    /// The latch that survives until the user clears it lives in the engine's `Shared`.
    pub over: bool,
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
            limiter: Limiter::new(sample_rate),
            ceiling: 1.0,
            gain: 1.0,
            auto_gain: None,
            sample_rate,
            settings: MasterSettings {
                gain: f64::NAN,
                ..settings.clone()
            },
            peak_l: 0.0,
            peak_r: 0.0,
            over: false,
        };
        m.apply(settings);
        m
    }

    pub fn apply(&mut self, s: &MasterSettings) {
        if *s == self.settings {
            return;
        }
        let sr = self.sample_rate;
        self.low_cut
            .configure(|b| b.highpass(sr, s.low_cut_hz as f32, 0.707));
        self.eq_low
            .configure(|b| b.low_shelf(sr, 120.0, linear_to_db(s.low_eq as f32), 0.7));
        self.eq_mid
            .configure(|b| b.peaking(sr, 1_000.0, linear_to_db(s.mid_eq as f32), 0.7));
        self.eq_high
            .configure(|b| b.high_shelf(sr, 6_000.0, linear_to_db(s.high_eq as f32), 0.7));
        self.high_cut
            .configure(|b| b.lowpass(sr, s.high_cut_hz as f32, 0.707));
        self.reverb.wet = s.reverb_wet.clamp(0.0, 1.0) as f32;
        self.gain = s.gain.clamp(0.0, 1.5) as f32;
        self.ceiling = db_to_linear(s.ceiling_db.clamp(-12.0, 0.0) as f32);
        self.settings = s.clone();
    }

    /// How far behind its input the master's output runs. The limiter's lookahead is the only
    /// delay in the chain, so this is zero whenever the limiter is off.
    pub fn latency_samples(&self) -> usize {
        if self.settings.limiter {
            self.limiter.latency()
        } else {
            0
        }
    }

    /// Hand the master this block's gain ramp (first frame, last frame), or `None` to go back
    /// to the gain the master settings carry. Set once per block by the engine.
    pub fn set_auto_gain(&mut self, ramp: Option<(f32, f32)>) {
        self.auto_gain = ramp;
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        let (ceiling, limiting) = (self.ceiling, self.settings.limiter);
        // Ramped across the block for the same reason the track strip ramps: a lane stepping
        // the whole mix's gain once per buffer is audible as a click on every boundary.
        let (g0, g1) = self.auto_gain.unwrap_or((self.gain, self.gain));
        let mut gain = g0;
        let dg = (g1 - g0) / l.len().max(1) as f32;
        self.over = false;
        for i in 0..l.len() {
            let (mut a, mut b) = self.low_cut.process(l[i], r[i]);
            (a, b) = self.eq_low.process(a, b);
            (a, b) = self.eq_mid.process(a, b);
            (a, b) = self.eq_high.process(a, b);
            (a, b) = self.high_cut.process(a, b);
            (a, b) = self.reverb.process(a, b);
            a *= gain;
            b *= gain;
            // Metered before the limiter: a mix driven 6 dB over has to read 6 dB over, not sit
            // pinned at the ceiling looking fine.
            let peak = a.abs().max(b.abs());
            pl = pl.max(a.abs());
            pr = pr.max(b.abs());
            self.over |= peak > ceiling;
            if limiting {
                (a, b) = self.limiter.process(a, b, ceiling);
            } else {
                // The opt-out is the clamp this stage used to be, unchanged.
                a = a.clamp(-1.0, 1.0);
                b = b.clamp(-1.0, 1.0);
            }
            gain += dg;
            l[i] = a;
            r[i] = b;
        }
        let frames = l.len();
        self.peak_l = hold(pl, self.peak_l, frames, self.sample_rate);
        self.peak_r = hold(pr, self.peak_r, frames, self.sample_rate);
    }
}
