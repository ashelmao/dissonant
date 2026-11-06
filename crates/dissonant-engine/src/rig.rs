//! The guitar rig: mono in, stereo out.
//!
//! Input strip → pedalboard → amp head → cabinet → rack, built from the primitives in
//! [`crate::dsp`]. [`dissonant_core::AmpRig`] describes it in plain numbers; this module is the
//! only place those numbers acquire a meaning.
//!
//! Every stage is a fixed set of filters and delay lines sized at construction, so
//! [`Rig::process_add`] never allocates. Rebuilding only happens in [`Rig::apply`], off the
//! audio thread.

use crate::dsp::{
    db_to_linear, Allpass1, Biquad, CabIr, CathodeFollower, Compressor, EnvFollower, MicVoice,
    NoiseGate, Over4, PowerAmp, Reverb, RigDelay, Sag, SineLfo, SpeakerVoice, ToneStack,
    ToneStackParts, TubeStage,
};
use dissonant_core::{AmpModel, AmpRig, CabModel, EqModel, MicModel, Pedal, PedalKind};

/// Longest delay the delay pedal can be set to, in seconds. Matches `Pedal::Delay`'s range.
const MAX_DELAY_SECONDS: f32 = 1.2;
/// Longest chorus delay, in seconds. Comfortably past 12 ms + 6 ms of sweep.
const MAX_CHORUS_SECONDS: f32 = 0.04;
/// Below this the reverb and delay tails are inaudible and the lane can stop rendering.
const SILENCE: f32 = 1e-4;
/// Hard ceiling on the rig's output. A runaway feedback path must not poison the mix bus.
const CEILING: f32 = 8.0;

/// One stompbox's state. Rebuilt only when its kind changes, so turning a knob never clicks.
#[derive(Debug, Clone)]
enum PedalState {
    Comp {
        comp: Compressor,
    },
    /// `os` is boxed so one big variant does not set the size of every pedal slot. The extra
    /// indirection is one pointer chase inside a stage that already runs 16 biquads.
    Drive {
        dc: Biquad,
        hump: Biquad,
        tone: Biquad,
        os: Box<Over4>,
        drive: f32,
        level: f32,
    },
    Wah {
        env: EnvFollower,
        filter: Biquad,
        sensitivity: f32,
        base_hz: f32,
        span: f32,
        sample_rate: f32,
    },
    Chorus {
        line: RigDelay,
        lfo: SineLfo,
        depth_samples: f32,
        centre_samples: f32,
        mix: f32,
    },
    Phaser {
        stages: [Allpass1; 4],
        lfo: SineLfo,
        feedback: f32,
        depth: f32,
        last: f32,
        sample_rate: f32,
    },
    Tremolo {
        lfo: SineLfo,
        depth: f32,
    },
    Delay {
        line: RigDelay,
        damp: Biquad,
        delay_samples: f32,
        feedback: f32,
        mix: f32,
    },
}

impl PedalState {
    fn kind(&self) -> PedalKind {
        match self {
            PedalState::Comp { .. } => PedalKind::Comp,
            PedalState::Drive { .. } => PedalKind::Drive,
            PedalState::Wah { .. } => PedalKind::Wah,
            PedalState::Chorus { .. } => PedalKind::Chorus,
            PedalState::Phaser { .. } => PedalKind::Phaser,
            PedalState::Tremolo { .. } => PedalKind::Tremolo,
            PedalState::Delay { .. } => PedalKind::Delay,
        }
    }

    fn new(kind: PedalKind, sample_rate: f32) -> Self {
        match kind {
            PedalKind::Comp => PedalState::Comp {
                comp: Compressor::new(sample_rate),
            },
            PedalKind::Drive => PedalState::Drive {
                dc: Biquad::default(),
                hump: Biquad::default(),
                tone: Biquad::default(),
                os: Box::new(Over4::new(sample_rate)),
                drive: 1.0,
                level: 1.0,
            },
            PedalKind::Wah => PedalState::Wah {
                env: EnvFollower::new(sample_rate, 10.0, 160.0),
                filter: Biquad::default(),
                sensitivity: 1.0,
                base_hz: 380.0,
                span: 3.0,
                sample_rate,
            },
            PedalKind::Chorus => PedalState::Chorus {
                line: RigDelay::new((MAX_CHORUS_SECONDS * sample_rate) as usize),
                lfo: SineLfo::default(),
                depth_samples: 0.0,
                centre_samples: 0.012 * sample_rate,
                mix: 0.0,
            },
            PedalKind::Phaser => PedalState::Phaser {
                stages: [Allpass1::default(); 4],
                lfo: SineLfo::default(),
                feedback: 0.0,
                depth: 0.0,
                last: 0.0,
                sample_rate,
            },
            PedalKind::Tremolo => PedalState::Tremolo {
                lfo: SineLfo::default(),
                depth: 0.0,
            },
            PedalKind::Delay => PedalState::Delay {
                line: RigDelay::new((MAX_DELAY_SECONDS * sample_rate) as usize + 2),
                damp: Biquad::default(),
                delay_samples: 0.0,
                feedback: 0.0,
                mix: 0.0,
            },
        }
    }

    /// Re-tune from the settings. The caller guarantees `settings.kind() == self.kind()`.
    fn apply(&mut self, settings: &Pedal, sample_rate: f32) {
        match (self, settings) {
            (PedalState::Comp { comp }, Pedal::Comp { sustain, level, .. }) => {
                // More sustain = a lower threshold, so more of the note is held up.
                let threshold = -40.0 + 2.4 * (10.0 - *sustain as f32);
                comp.set(threshold, 4.0, -6.0 + *level as f32 * 1.8);
            }
            (
                PedalState::Drive {
                    dc,
                    hump,
                    tone,
                    os,
                    drive,
                    level,
                },
                Pedal::Drive {
                    drive: d,
                    tone: t,
                    level: l,
                    ..
                },
            ) => {
                dc.highpass(sample_rate, 30.0, 0.707);
                hump.peaking(sample_rate, 720.0, 4.0, 0.9);
                tone.lowpass(sample_rate, log_map(*t as f32, 1_500.0, 9_000.0), 0.707);
                os.reset();
                *drive = 1.0 + *d as f32 * 3.5;
                *level = db_to_linear(-12.0 + *l as f32 * 1.8);
            }
            (
                PedalState::Wah {
                    env,
                    sensitivity,
                    base_hz,
                    span,
                    ..
                },
                Pedal::Wah {
                    sensitivity: s,
                    range: r,
                    ..
                },
            ) => {
                env.set(sample_rate, 10.0, 160.0);
                *sensitivity = (*s as f32 / 5.0).max(0.05);
                *base_hz = 380.0;
                // Range 0…10 sweeps up to three octaves above the base.
                *span = *r as f32 / 10.0 * 3.0;
            }
            (
                PedalState::Chorus {
                    lfo,
                    depth_samples,
                    centre_samples,
                    mix,
                    ..
                },
                Pedal::Chorus {
                    rate_hz,
                    depth,
                    mix: m,
                    ..
                },
            ) => {
                lfo.set_rate(sample_rate, *rate_hz as f32);
                *centre_samples = 0.012 * sample_rate;
                *depth_samples = *depth as f32 / 10.0 * 0.006 * sample_rate;
                *mix = *m as f32 / 10.0;
            }
            (
                PedalState::Phaser {
                    lfo,
                    feedback,
                    depth,
                    ..
                },
                Pedal::Phaser {
                    rate_hz,
                    depth: d,
                    feedback: fb,
                    ..
                },
            ) => {
                lfo.set_rate(sample_rate, *rate_hz as f32);
                *feedback = *fb as f32 / 10.0 * 0.85;
                *depth = *d as f32 / 10.0;
            }
            (
                PedalState::Tremolo { lfo, depth },
                Pedal::Tremolo {
                    rate_hz, depth: d, ..
                },
            ) => {
                lfo.set_rate(sample_rate, *rate_hz as f32);
                *depth = *d as f32 / 10.0;
            }
            (
                PedalState::Delay {
                    damp,
                    delay_samples,
                    feedback,
                    mix,
                    ..
                },
                Pedal::Delay {
                    time_ms,
                    feedback: fb,
                    mix: m,
                    ..
                },
            ) => {
                damp.lowpass(sample_rate, 3_500.0, 0.707);
                *delay_samples = (*time_ms as f32 * 0.001 * sample_rate)
                    .min(MAX_DELAY_SECONDS * sample_rate - 2.0);
                *feedback = *fb as f32 / 10.0 * 0.92;
                *mix = *m as f32 / 10.0;
            }
            // Unreachable: `Rig::apply` rebuilds the state whenever the kind changes.
            _ => debug_assert!(false, "pedal state and settings disagree on kind"),
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        match self {
            PedalState::Comp { comp } => comp.process(x),
            PedalState::Drive {
                dc,
                hump,
                tone,
                os,
                drive,
                level,
            } => {
                let v = dc.process(x);
                let d = *drive;
                // Asymmetric: the +0.15 bias favours even harmonics, which is what makes a
                // real overdrive sound thick rather than merely loud.
                let v = os.process(v, &mut |s| (s * d + 0.15).tanh() - 0.148_885);
                tone.process(hump.process(v)) * *level
            }
            PedalState::Wah {
                env,
                filter,
                sensitivity,
                base_hz,
                span,
                sample_rate,
            } => {
                let e = (env.process(x) * *sensitivity).clamp(0.0, 1.0);
                filter.bandpass(*sample_rate, *base_hz * 2f32.powf(e * *span), 3.5);
                filter.process(x)
            }
            PedalState::Chorus {
                line,
                lfo,
                depth_samples,
                centre_samples,
                mix,
            } => {
                line.write(x);
                let d = *centre_samples + lfo.tick() * *depth_samples;
                let wet = line.read(d);
                x * (1.0 - *mix * 0.5) + wet * *mix
            }
            PedalState::Phaser {
                stages,
                lfo,
                feedback,
                depth,
                last,
                sample_rate,
            } => {
                let sweep = log_map((lfo.tick() * 0.5 + 0.5) * 10.0, 200.0, 1_600.0);
                let mut v = x + *last * *feedback;
                for s in stages.iter_mut() {
                    s.set_freq(*sample_rate, sweep);
                    v = s.process(v);
                }
                *last = v;
                x + v * *depth
            }
            PedalState::Tremolo { lfo, depth } => x * (1.0 - *depth * (0.5 - 0.5 * lfo.tick())),
            PedalState::Delay {
                line,
                damp,
                delay_samples,
                feedback,
                mix,
            } => {
                let wet = line.read(*delay_samples);
                line.write(x + damp.process(wet) * *feedback);
                x + wet * *mix
            }
        }
    }

    fn reset(&mut self) {
        match self {
            PedalState::Comp { comp } => comp.reset(),
            PedalState::Drive {
                dc, hump, tone, os, ..
            } => {
                dc.reset();
                hump.reset();
                tone.reset();
                os.reset();
            }
            PedalState::Wah { env, filter, .. } => {
                env.reset();
                filter.reset();
            }
            PedalState::Chorus { line, lfo, .. } => {
                line.reset();
                lfo.reset();
            }
            PedalState::Phaser {
                stages, lfo, last, ..
            } => {
                for s in stages.iter_mut() {
                    s.reset();
                }
                lfo.reset();
                *last = 0.0;
            }
            PedalState::Tremolo { lfo, .. } => lfo.reset(),
            PedalState::Delay { line, damp, .. } => {
                line.reset();
                damp.reset();
            }
        }
    }
}

/// Map a 0…10 knob onto `lo`…`hi` logarithmically, the way a frequency control should feel.
#[inline]
fn log_map(knob: f32, lo: f32, hi: f32) -> f32 {
    lo * (hi / lo).powf(knob.clamp(0.0, 10.0) / 10.0)
}

/// How hard one amp model bites: how many triode stages the signal passes through, where they
/// idle on the tube's curve, how far `gain` pushes them, the pre-emphasis that decides which
/// part of the spectrum distorts first, and how willingly its supply gives up under load.
struct Voicing {
    stages: usize,
    /// Grid volts away from the operating point. Colder — more negative — rides the signal nearer
    /// cutoff, so the negative half clips first and the stage throws even harmonics.
    bias: f32,
    drive_scale: f32,
    pre_emphasis_hz: f32,
    /// The power supply's manners. Baked here and nowhere else: there is no schema field and no
    /// knob, because nobody adjusts their amp's filter capacitors, and because the difference
    /// between a tweed and a modern head is mostly this rather than anything in the preamp.
    sag: Sag,
}

fn voicing(model: AmpModel) -> Voicing {
    match model {
        // A small cathode-biased combo on a tube rectifier: it collapses under a hard chord and
        // takes its time coming back, which is the whole reason people still record them.
        AmpModel::Clean => Voicing {
            stages: 1,
            bias: 0.0,
            drive_scale: 6.0,
            pre_emphasis_hz: 0.0,
            sag: Sag {
                droop: 0.5,
                fall_ms: 13.0,
                rise_ms: 190.0,
                bias_depth: 0.1,
                bias_ms: 240.0,
            },
        },
        // A bigger fixed-bias head, still on a rectifier tube: it breathes, but it does not fold.
        AmpModel::Crunch => Voicing {
            stages: 2,
            bias: -0.3,
            drive_scale: 24.0,
            pre_emphasis_hz: 720.0,
            sag: Sag {
                droop: 0.26,
                fall_ms: 17.0,
                rise_ms: 140.0,
                bias_depth: 0.055,
                bias_ms: 200.0,
            },
        },
        // Solid-state rectification and a bank of filter capacitors: the rail hardly notices,
        // which is what "tight" means when a modern amp is described that way.
        AmpModel::Lead => Voicing {
            stages: 3,
            bias: -0.6,
            drive_scale: 70.0,
            pre_emphasis_hz: 900.0,
            sag: Sag {
                droop: 0.1,
                fall_ms: 22.0,
                rise_ms: 110.0,
                bias_depth: 0.02,
                bias_ms: 180.0,
            },
        },
        // The preamp has already flattened everything; a supply that moved too would only make
        // the wall of fuzz quieter.
        AmpModel::Fuzz => Voicing {
            stages: 3,
            bias: -1.2,
            drive_scale: 220.0,
            pre_emphasis_hz: 1_100.0,
            sag: Sag {
                droop: 0.05,
                fall_ms: 25.0,
                rise_ms: 90.0,
                bias_depth: 0.01,
                bias_ms: 150.0,
            },
        },
    }
}

/// The bright cap's corner, and the most it can lift with the gain pot right down. A treble
/// bypass is a capacitor across that pot: it can only return what the pot is throwing away, so
/// its effect shrinks to nothing as the knob opens.
const BRIGHT_HZ: f32 = 1_200.0;
const BRIGHT_DB: f32 = 11.0;

/// What the half-power switch leaves of the rail, and how much further it lets the supply fall.
/// Pulling half the output tubes does not halve the volume — it halves the headroom, so the
/// stage arrives at its limit sooner and stays there longer.
const HALF_POWER_HEADROOM: f32 = 0.55;
const HALF_POWER_SAG: f32 = 1.4;

/// Most preamp stages any voicing uses.
const MAX_STAGES: usize = 3;

/// The parts in each tone stack. Same third-order network every time — these are the values that
/// make one amp family sound like itself rather than like the one next to it.
fn tone_stack_parts(model: EqModel) -> ToneStackParts {
    match model {
        // The '59 Bassman stack: a small treble cap and a 25 k mid pot, so the mids scoop deeply.
        EqModel::UsClassic => ToneStackParts {
            c1: 0.25e-9,
            c2: 20e-9,
            c3: 20e-9,
            r1: 250e3,
            r2: 1e6,
            r3: 25e3,
            r4: 56e3,
        },
        // A bigger treble cap and a lower slope resistor: browner, and the mid control bites.
        EqModel::British => ToneStackParts {
            c1: 0.47e-9,
            c2: 22e-9,
            c3: 22e-9,
            r1: 220e3,
            r2: 1e6,
            r3: 22e3,
            r4: 33e3,
        },
        // The same again with the treble cap doubled — the glassy, cutting voice.
        EqModel::BritishBright => ToneStackParts {
            c1: 1.0e-9,
            c2: 22e-9,
            c3: 22e-9,
            r1: 220e3,
            r2: 1e6,
            r3: 22e3,
            r4: 56e3,
        },
        // A 100 k mid pot fills the scoop back in, which is most of what "boutique" means.
        EqModel::Boutique => ToneStackParts {
            c1: 0.5e-9,
            c2: 47e-9,
            c3: 47e-9,
            r1: 250e3,
            r2: 1e6,
            r3: 100e3,
            r4: 56e3,
        },
    }
}

/// What the cabinet does to everything that reaches it. `Direct` has no speaker and never asks.
fn speaker_voice(model: CabModel) -> SpeakerVoice {
    match model {
        CabModel::FourByTwelve => SpeakerVoice {
            resonance_hz: 85.0,
            resonance_db: 3.0,
            body_hz: 180.0,
            body_db: 2.0,
            breakup_hz: 2_300.0,
            top_hz: 5_200.0,
        },
        CabModel::TwoByTwelve => SpeakerVoice {
            resonance_hz: 95.0,
            resonance_db: 2.5,
            body_hz: 220.0,
            body_db: 1.5,
            breakup_hz: 2_700.0,
            top_hz: 5_800.0,
        },
        CabModel::OneByTwelve | CabModel::Direct => SpeakerVoice {
            resonance_hz: 110.0,
            resonance_db: 2.0,
            body_hz: 260.0,
            body_db: 1.0,
            breakup_hz: 3_200.0,
            top_hz: 6_400.0,
        },
    }
}

fn mic_voice(model: MicModel) -> MicVoice {
    match model {
        // A moving coil: the famous presence lift, and nothing much above 7 kHz.
        MicModel::Dynamic => MicVoice {
            presence_hz: 4_500.0,
            presence_db: 5.0,
            presence_q: 1.1,
            top_hz: 7_500.0,
        },
        // Flat and extended, which means it hears the fizz as well as the note.
        MicModel::Condenser => MicVoice {
            presence_hz: 9_000.0,
            presence_db: 3.0,
            presence_q: 0.7,
            top_hz: 16_000.0,
        },
        // A ribbon is dark and thick, and forgives a cranked amp more than either of the others.
        MicModel::Ribbon => MicVoice {
            presence_hz: 2_200.0,
            presence_db: 2.0,
            presence_q: 0.8,
            top_hz: 5_500.0,
        },
    }
}

/// A guitar rig: mono in, stereo out.
#[derive(Debug, Clone)]
pub struct Rig {
    sample_rate: f32,

    // Input strip.
    input_gain: f32,
    gate: NoiseGate,
    gate_on: bool,
    input_peak: f32,

    pedals: Vec<PedalState>,
    /// Which pedals are switched on, parallel to `pedals`.
    pedal_on: Vec<bool>,

    // Amp head.
    /// The treble bypass across the gain pot, when the model has one. Identity when it does not,
    /// or when the pot is wide open and there is nothing left for the cap to return.
    bright: Biquad,
    stages: [TubeStage; MAX_STAGES],
    active_stages: usize,
    /// Between the last preamp stage and the tone stack, on the models whose circuit has one.
    follower: CathodeFollower,
    follower_on: bool,
    stack: ToneStack,
    power: PowerAmp,
    presence: Biquad,

    // Cabinet and microphone.
    cab: CabIr,
    cab_on: bool,
    /// What the grid was last measured for. Re-measuring is only needed when the cabinet or the
    /// microphone changes — dragging the mic across the grille just re-blends what is here.
    baked: Option<(CabModel, MicModel)>,

    // Rack.
    reverb: Reverb,
    output_gain: f32,

    /// Blocks since the last non-trivial input, for [`Rig::is_silent`].
    quiet_blocks: u32,
}

/// How many consecutive silent blocks before the lane may stop rendering. At 512 frames and
/// 48 kHz this is about 1.4 s — longer than any tail the rack can produce at full decay.
const QUIET_BLOCKS_UNTIL_SILENT: u32 = 128;

impl Rig {
    pub fn new(sample_rate: f32) -> Self {
        let mut rig = Rig {
            sample_rate,
            input_gain: 1.0,
            gate: NoiseGate::new(sample_rate),
            gate_on: true,
            input_peak: 0.0,
            pedals: Vec::new(),
            pedal_on: Vec::new(),
            stages: [
                TubeStage::new(sample_rate),
                TubeStage::new(sample_rate),
                TubeStage::new(sample_rate),
            ],
            bright: Biquad::identity(),
            active_stages: 1,
            follower: CathodeFollower::new(sample_rate),
            follower_on: false,
            stack: ToneStack::new(),
            power: PowerAmp::new(sample_rate),
            presence: Biquad::identity(),
            cab: CabIr::new(),
            cab_on: true,
            baked: None,
            reverb: Reverb::new(sample_rate),
            output_gain: 1.0,
            quiet_blocks: QUIET_BLOCKS_UNTIL_SILENT,
        };
        rig.apply(&AmpRig::default());
        rig
    }

    /// Re-tune every stage. Cheap and idempotent; a pedal is only rebuilt when its kind
    /// changed, so dragging a knob never restarts a delay line or clicks.
    pub fn apply(&mut self, settings: &AmpRig) {
        let sr = self.sample_rate;

        self.input_gain = db_to_linear(settings.input_db as f32);
        self.gate_on = settings.gate.on;
        self.gate.set(
            sr,
            settings.gate.threshold_db as f32,
            settings.gate.release_ms as f32,
        );

        self.pedals.truncate(settings.pedals.len());
        self.pedal_on.resize(settings.pedals.len(), false);
        for (i, p) in settings.pedals.iter().enumerate() {
            match self.pedals.get(i) {
                Some(existing) if existing.kind() == p.kind() => {}
                _ => {
                    let fresh = PedalState::new(p.kind(), sr);
                    if i < self.pedals.len() {
                        self.pedals[i] = fresh;
                    } else {
                        self.pedals.push(fresh);
                    }
                }
            }
            self.pedals[i].apply(p, sr);
            self.pedal_on[i] = p.on();
        }

        let v = voicing(settings.amp.model);
        let head = &settings.amp;
        // A switch is only real on the models whose circuit has it. `AmpRig::sanitized` already
        // clears the others, but `apply` is handed rigs that never went through it — defaults,
        // and whatever a plugin host passes — so the decision is made here too.
        let model = head.model;
        let bright_cap = head.bright_cap && model.has_bright_cap();
        let half_power = head.half_power && model.has_half_power();
        self.follower_on = head.cathode_follower && model.has_cathode_follower();

        self.active_stages = v.stages;
        // A gain pot is an audio taper, and the useful range runs from "barely tickling the grid"
        // to "slammed", which is multiplicative, not additive.
        let gain_knob = head.gain as f32;
        let stage_gain = log_map(gain_knob, 0.2, v.drive_scale);
        for stage in &mut self.stages {
            stage.set(sr, stage_gain, v.bias, v.pre_emphasis_hz);
        }
        // A 0 dB shelf is exactly unity, so switching the cap out keeps the filter's state
        // rather than zeroing it — throwing the switch mid-note must not click.
        let bright_db = if bright_cap {
            BRIGHT_DB * (1.0 - gain_knob / 10.0)
        } else {
            0.0
        };
        self.bright.high_shelf(sr, BRIGHT_HZ, bright_db, 0.707);
        // How hard the stage in front swings the follower's grid, which is the only thing that
        // decides whether it sits there politely or starts conducting.
        self.follower.set(sr, log_map(gain_knob, 1.5, 5.0));

        self.stack.set(
            sr,
            tone_stack_parts(settings.eq),
            head.treble as f32 / 10.0,
            head.mid as f32 / 10.0,
            head.bass as f32 / 10.0,
        );
        let mut sag = v.sag;
        let headroom = if half_power {
            // Half the bottles, half the reservoir behind them: the stage runs out sooner and
            // what is left gives way further.
            sag.droop *= HALF_POWER_SAG;
            sag.bias_depth *= HALF_POWER_SAG;
            HALF_POWER_HEADROOM
        } else {
            1.0
        };
        self.power
            .set(sr, 0.6 + head.master as f32 / 10.0 * 2.4, headroom, sag);
        self.presence
            .high_shelf(sr, 6_000.0, head.presence as f32, 0.707);

        self.cab_on = settings.cab.model != CabModel::Direct;
        if self.cab_on {
            let want = (settings.cab.model, settings.cab.mic_model);
            if self.baked != Some(want) {
                self.cab.bake(sr, speaker_voice(want.0), mic_voice(want.1));
                self.baked = Some(want);
            }
            self.cab.morph(
                settings.cab.pos_x as f32 / 10.0,
                settings.cab.pos_z as f32 / 10.0,
            );
        }

        self.reverb.wet = settings.rack.reverb as f32 / 10.0 * 0.6;
        self.reverb.set_decay(settings.rack.decay as f32 / 10.0);
        self.output_gain = db_to_linear(settings.rack.output_db as f32);
    }

    /// Add the processed stereo result into `l`/`r`. `input` is mono and the same length.
    pub fn process_add(&mut self, input: &[f32], l: &mut [f32], r: &mut [f32]) {
        let n = input.len().min(l.len()).min(r.len());
        let mut peak = 0.0f32;
        for i in 0..n {
            let mut s = input[i] * self.input_gain;
            peak = peak.max(s.abs());
            if self.gate_on {
                s = self.gate.process(s);
            }

            for (p, on) in self.pedals.iter_mut().zip(self.pedal_on.iter()) {
                if *on {
                    s = p.process(s);
                }
            }

            s = self.bright.process(s);
            for stage in self.stages.iter_mut().take(self.active_stages) {
                s = stage.process(s);
            }
            if self.follower_on {
                s = self.follower.process(s);
            }

            s = self.stack.process(s);
            s = self.power.process(s);
            s = self.presence.process(s);

            if self.cab_on {
                s = self.cab.process(s);
            }

            let (wl, wr) = self.reverb.process(s, s);
            l[i] += clean(wl * self.output_gain);
            r[i] += clean(wr * self.output_gain);
        }

        self.input_peak = peak;
        if peak > SILENCE {
            self.quiet_blocks = 0;
        } else {
            self.quiet_blocks = self.quiet_blocks.saturating_add(1);
        }
    }

    /// Peak at the pedalboard input over the last block, for the UI's input meter.
    pub fn input_peak(&self) -> f32 {
        self.input_peak
    }

    /// True once the reverb and delay tails have run out, so the lane can stop rendering.
    pub fn is_silent(&self) -> bool {
        self.quiet_blocks >= QUIET_BLOCKS_UNTIL_SILENT
    }

    pub fn reset(&mut self) {
        self.gate.reset();
        for p in &mut self.pedals {
            p.reset();
        }
        self.bright.reset();
        for stage in &mut self.stages {
            stage.reset();
        }
        self.follower.reset();
        self.stack.reset();
        self.power.reset();
        self.presence.reset();
        self.cab.reset();
        self.reverb.reset();
        self.input_peak = 0.0;
        self.quiet_blocks = QUIET_BLOCKS_UNTIL_SILENT;
    }
}

/// Clamp and de-NaN one sample on its way to the mix bus.
#[inline]
fn clean(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(-CEILING, CEILING)
    } else {
        0.0
    }
}
