//! The block processor: transport + sample-accurate event scheduling + mixing.

use crate::audio::{AudioPool, BlockContext};
use crate::channel::{
    chain_is_silent, run_inserts, Channel, ChannelAuto, InsertSlot, Instrument, Master,
    SYNTH_POLYPHONY,
};
use crate::drums::{DrumBank, DrumKit, RenderedDrum};
use crate::dsp::pan_gains;
use crate::synth::{Bend, PolySynth};
use crate::tuner::PitchTracker;
use dissonant_core::{
    factory_sound, swing_warp_ticks, AutomationCurve, InsertTarget, MasterSettings, NoteEvent,
    NoteRope, Sequence, SequenceTrack, SoundPatch, SynthPatch, Tempo, TempoMap, TrackAutomation,
    VoiceMode, MAX_INSERTS, ROPE_OVERLAP_SECS, TICKS_PER_BEAT,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

pub const MAX_TRACKS: usize = 64;

/// Audition hold bounds. The floor keeps very fine grids audible; the ceiling stops a stray
/// value from leaving a voice ringing.
pub const MIN_AUDITION_SECS: f32 = 0.12;
pub const MAX_AUDITION_SECS: f32 = 4.0;

/// Clean blocks before the input slack is eased back toward its floor. At 512 frames and
/// 48 kHz this is about five seconds: long enough that one stray glitch costs the player no
/// lasting latency, short enough that swapping to a better interface is felt right away.
const CLEAN_BLOCKS_BEFORE_TIGHTENING: u32 = 512;

/// Samples [`Shared::preview_scope`] returns.
pub const SCOPE_SAMPLES: usize = 512;
/// The preview scope's history. Readers look at the newest half, so a block written while they
/// read can't tear the window they return.
const SCOPE_RING: usize = 4096;

/// What the real-time side publishes for the UI to read without locks.
#[derive(Debug)]
pub struct Shared {
    /// Playhead in ticks (f64: sub-tick between blocks), as f64 bits.
    position_bits: AtomicU64,
    playing: AtomicBool,
    sample_rate: AtomicU32,
    /// Per-track peak (index = position in the current sequence), as f32 bits.
    track_peaks: [AtomicU32; MAX_TRACKS],
    master_peak: [AtomicU32; 2],
    /// Latched: the master went over the ceiling before limiting. Stays set until `ClearClip`,
    /// because an over that lasted one block is still an over the user has to be told about.
    master_over: AtomicBool,
    /// Frames per device callback, as last observed.
    block_frames: AtomicU32,
    /// Render time / block time, smoothed, as f32 bits (1.0 = no headroom left).
    load: AtomicU32,
    /// Callback gaps detected since the stream started.
    xruns: AtomicU32,
    /// Beats of count-in still to go (f32 bits); 0 when not counting in.
    count_in: AtomicU32,
    /// Peak of the live input over the last block, as f32 bits.
    input_peak: AtomicU32,
    /// The highest the input has reached since the last `ClearInputPeak`, as f32 bits. A peak
    /// that only lasted a block is still the peak the gain has to leave room for.
    input_peak_hold: AtomicU32,
    /// Latched the same way: the input hit full scale, so it clipped before the rig ever saw
    /// it. Cleared only by `ClearInputPeak`.
    input_over: AtomicBool,
    /// Input samples still in hand after the last block — latency the player feels.
    input_backlog: AtomicU32,
    /// Blocks in which the input ring ran dry since the stream started.
    input_underruns: AtomicU32,
    /// True once the transport started with recording armed.
    recording: AtomicBool,
    /// The tick the current take started on, as f64 bits.
    record_start_tick: AtomicU64,
    /// Frames captured into the current take.
    record_frames: AtomicU64,
    /// The tuner's last reading in Hz, as f32 bits; 0 when it has heard nothing periodic.
    tuner_hz: AtomicU32,
    /// How periodic that reading was, 0…1, as f32 bits.
    tuner_clarity: AtomicU32,
    /// The preview channel's mono output as f32 bits, a ring indexed by `scope_written`.
    scope: [AtomicU32; SCOPE_RING],
    /// Samples ever written to `scope`.
    scope_written: AtomicUsize,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            input_backlog: AtomicU32::new(0),
            position_bits: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            sample_rate: AtomicU32::new(44_100),
            track_peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            master_peak: [AtomicU32::new(0), AtomicU32::new(0)],
            master_over: AtomicBool::new(false),
            block_frames: AtomicU32::new(0),
            load: AtomicU32::new(0),
            xruns: AtomicU32::new(0),
            count_in: AtomicU32::new(0),
            input_peak: AtomicU32::new(0),
            input_peak_hold: AtomicU32::new(0),
            input_over: AtomicBool::new(false),
            input_underruns: AtomicU32::new(0),
            recording: AtomicBool::new(false),
            record_start_tick: AtomicU64::new(0),
            record_frames: AtomicU64::new(0),
            tuner_hz: AtomicU32::new(0),
            tuner_clarity: AtomicU32::new(0),
            scope: std::array::from_fn(|_| AtomicU32::new(0)),
            scope_written: AtomicUsize::new(0),
        }
    }
}

impl Shared {
    /// Playhead in ticks; fractional between blocks.
    pub fn position_ticks(&self) -> f64 {
        f64::from_bits(self.position_bits.load(Ordering::Relaxed))
    }
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }
    pub fn track_peak(&self, index: usize) -> f32 {
        self.track_peaks
            .get(index)
            .map(|a| f32::from_bits(a.load(Ordering::Relaxed)))
            .unwrap_or(0.0)
    }
    pub fn master_peak(&self) -> (f32, f32) {
        (
            f32::from_bits(self.master_peak[0].load(Ordering::Relaxed)),
            f32::from_bits(self.master_peak[1].load(Ordering::Relaxed)),
        )
    }
    /// Latched since the last clear: the mix asked for more than the ceiling.
    pub fn master_over(&self) -> bool {
        self.master_over.load(Ordering::Relaxed)
    }
    pub fn block_frames(&self) -> u32 {
        self.block_frames.load(Ordering::Relaxed)
    }
    /// 0.0 … 1.0+ : fraction of each block's time budget spent rendering.
    pub fn load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }
    pub fn xruns(&self) -> u32 {
        self.xruns.load(Ordering::Relaxed)
    }
    pub fn set_stats(&self, block_frames: u32, load: f32) {
        self.block_frames.store(block_frames, Ordering::Relaxed);
        self.load.store(load.to_bits(), Ordering::Relaxed);
    }
    pub fn note_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }
    /// Beats of count-in left before the transport starts (0 = not counting in).
    pub fn count_in_beats(&self) -> f32 {
        f32::from_bits(self.count_in.load(Ordering::Relaxed))
    }
    /// Peak of the live input over the last block.
    pub fn input_peak(&self) -> f32 {
        f32::from_bits(self.input_peak.load(Ordering::Relaxed))
    }
    pub fn input_underruns(&self) -> u32 {
        self.input_underruns.load(Ordering::Relaxed)
    }
    /// The highest the input has reached since the last clear — the meter's hold marker.
    pub fn input_peak_hold(&self) -> f32 {
        f32::from_bits(self.input_peak_hold.load(Ordering::Relaxed))
    }
    /// Latched since the last clear: the input hit full scale before the rig, so the only fix
    /// is the interface's own gain.
    pub fn input_over(&self) -> bool {
        self.input_over.load(Ordering::Relaxed)
    }
    /// Input frames still queued after the last block. With the device block size this is the
    /// whole monitoring delay.
    pub fn input_backlog(&self) -> u32 {
        self.input_backlog.load(Ordering::Relaxed)
    }
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }
    /// Where the current take started (ticks), and how many frames it has captured. Only
    /// meaningful once [`Shared::is_recording`] has been true.
    pub fn take_span(&self) -> (f64, u64) {
        (
            f64::from_bits(self.record_start_tick.load(Ordering::Relaxed)),
            self.record_frames.load(Ordering::Relaxed),
        )
    }

    /// The tuner's last reading: frequency in Hz and how periodic the window was. `None` until
    /// the tuner is open and hearing something with a pitch.
    pub fn tuner_reading(&self) -> Option<(f32, f32)> {
        let hz = f32::from_bits(self.tuner_hz.load(Ordering::Relaxed));
        (hz > 0.0).then(|| (hz, f32::from_bits(self.tuner_clarity.load(Ordering::Relaxed))))
    }

    /// Audio thread: append a block of the preview channel's output to the scope ring.
    fn write_scope(&self, samples: &[f32]) {
        let mut w = self.scope_written.load(Ordering::Relaxed);
        for s in samples {
            self.scope[w % SCOPE_RING].store(s.to_bits(), Ordering::Relaxed);
            w = w.wrapping_add(1);
        }
        self.scope_written.store(w, Ordering::Release);
    }

    /// The latest [`SCOPE_SAMPLES`] of the preview channel, starting on a rising zero crossing
    /// so a steady tone stands still; just the newest samples when there is none.
    pub fn preview_scope(&self) -> Vec<f32> {
        let w = self.scope_written.load(Ordering::Acquire);
        let span = SCOPE_RING / 2;
        let mut recent = vec![0.0f32; span];
        let have = w.min(span);
        for (i, s) in recent[span - have..].iter_mut().enumerate() {
            let at = w.wrapping_sub(have).wrapping_add(i);
            *s = f32::from_bits(self.scope[at % SCOPE_RING].load(Ordering::Relaxed));
        }
        // The newest window that opens on a crossing.
        let last_start = span - SCOPE_SAMPLES;
        let start = (1..=last_start)
            .rev()
            .find(|&i| recent[i - 1] < 0.0 && recent[i] >= 0.0)
            .unwrap_or(last_start);
        recent[start..start + SCOPE_SAMPLES].to_vec()
    }
}

/// What the preview channel (the SOUND page) plays.
#[derive(Debug, Clone)]
pub enum PreviewPatch {
    Synth(Box<SynthPatch>),
    /// Rendered by the sender; the engine re-renders only if its sample rate differs.
    Drum(Arc<RenderedDrum>),
}

impl PreviewPatch {
    /// Build from a sound's patch, rendering a drum at `sample_rate` (call off the audio
    /// thread).
    pub fn from_sound(patch: &SoundPatch, sample_rate: f32) -> Self {
        match patch {
            SoundPatch::Synth(p) => PreviewPatch::Synth(Box::new(*p)),
            SoundPatch::Drum(p) => PreviewPatch::Drum(Arc::new(RenderedDrum::new(p, sample_rate))),
        }
    }
}

/// Messages from the application to the audio thread.
///
/// Not `Clone`: `SetInstrument` carries a live plugin, which cannot be duplicated. Commands are
/// sent, not copied.
#[derive(Debug)]
pub enum EngineCommand {
    Play,
    Stop,
    Seek {
        tick: f64,
    },
    SetLooping(bool),
    /// Loop a sub-range `[start, end)` of the sequence instead of the whole thing, in ticks.
    SetLoop {
        start: f64,
        end: f64,
    },
    ClearLoop,
    /// Arm a punch over `[start, end)` in ticks: capture is held until the transport reaches
    /// `start` and dropped again at `end`. Looping is suppressed for as long as one is armed —
    /// a loop would record the same bars twice, which is comping, which punch is not.
    SetPunch {
        start: f64,
        end: f64,
    },
    ClearPunch,
    /// A held note from a controller / typing keyboard. Released by `NoteOff`.
    NoteOn {
        track_id: Uuid,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        track_id: Uuid,
        pitch: u8,
    },
    /// Replace what's playing. The old `Arc` is dropped on the audio thread; sequences are
    /// small so this is acceptable for now (a return channel is the real-time-pure fix).
    ///
    /// Drum pads are rendered on the spot, which suits offline rendering; `AudioDevice::send`
    /// turns this into `SetSequenceWithDrums` so a live engine never does that.
    SetSequence(Arc<Sequence>),
    /// `SetSequence` with the kits' drums already rendered (see [`DrumBank`]).
    SetSequenceWithDrums {
        sequence: Arc<Sequence>,
        drums: Arc<DrumBank>,
    },
    SetHearChords(bool),
    SetMaster(MasterSettings),
    /// Sound a note on a track right now (placement audition / typing keyboard). `seconds` is
    /// how long to hold it before the automatic release — the roll passes the length of the note
    /// being previewed, so auditions don't outlast the note the user is drawing.
    Audition {
        track_id: Uuid,
        pitch: u8,
        velocity: u8,
        seconds: f32,
    },
    /// Release an audition note early (typing keyboard key-up).
    AuditionOff {
        track_id: Uuid,
        pitch: u8,
    },
    /// A short sine on the master bus, for checking the output device from Settings.
    TestTone,
    /// Reset the latched over-the-ceiling flag in `Shared` — the user acknowledging the clip
    /// indicator, not a mix change.
    ClearClip,
    /// Reset the input meter's peak hold and its clip latch.
    ClearInputPeak,
    /// Open or close the tuner. While it is open the engine's output is silenced — you tune
    /// against the strings, not through whatever the rig was doing — and the live input is run
    /// through pitch detection on every block the transport is stopped for.
    SetTuner(bool),
    /// Click on every beat (accented on the bar) while playing; `volume` 0…1.
    SetMetronome {
        on: bool,
        volume: f32,
    },
    /// Click through `bars` bars with the transport held, then play.
    PlayWithCountIn {
        bars: u32,
    },
    /// Install an instrument built on the app thread. Loading a plugin takes hundreds of
    /// milliseconds, so it is never done here; the app hands over a ready one and the audio
    /// thread only swaps a pointer. The displaced instrument is dropped here, the same
    /// tradeoff `SetSequence` already makes.
    SetInstrument {
        track_id: Uuid,
        instrument: Box<Instrument>,
    },
    /// Restore a track to its built-in voice, releasing whatever plugin it hosted.
    ClearInstrument {
        track_id: Uuid,
    },
    /// A parameter change on a hosted plugin, applied at the next block.
    SetPluginParam {
        track_id: Uuid,
        param_id: u32,
        value: f64,
    },
    /// Decoded audio, built on the app thread. The displaced pool is dropped here — the same
    /// tradeoff `SetSequence` makes, and it only happens when the set of sources changes.
    SetAudioPool(Arc<AudioPool>),
    /// Which audio track hears the live input, and whether it is monitored through the rig.
    SetAudioArm {
        track_id: Option<Uuid>,
        monitor: bool,
    },
    /// Capture the input into the take ring from the next playing sample.
    StartRecord,
    StopRecord,
    /// Install an effect in a chain, at `index` (clamped to the chain's length). The slot is
    /// built on the app thread for the same reason `SetInstrument`'s instrument is, and its
    /// unit is `None` when the project names a plugin this machine cannot load — the slot still
    /// takes its place in the chain so everything after it keeps its index.
    AddInsert {
        target: InsertTarget,
        index: usize,
        slot: InsertSlot,
    },
    /// Drop an effect. Its unit is released here, the same tradeoff `ClearInstrument` makes.
    RemoveInsert {
        target: InsertTarget,
        id: Uuid,
    },
    /// Move an effect within its chain; `to` is clamped.
    MoveInsert {
        target: InsertTarget,
        id: Uuid,
        to: usize,
    },
    /// Bypass (`on: false`) or re-enable an effect, leaving it loaded and in place.
    SetInsertEnabled {
        target: InsertTarget,
        id: Uuid,
        on: bool,
    },
    /// A parameter change on a hosted effect, applied at the next block. Inserts are keyed by
    /// their own id, so the chain they live in does not have to be named.
    SetInsertParam {
        id: Uuid,
        param_id: u32,
        value: f64,
    },
    /// Load (or with `None`, release and close) the preview channel: an instrument outside the
    /// track list that sounds whether or not the transport runs, through the master chain,
    /// untouched by mute / solo / stop. A new synth patch applies to held notes without
    /// cutting.
    SetPreview(Option<PreviewPatch>),
    /// Play the preview. A drum preview plays its sound on any pitch.
    PreviewNoteOn {
        pitch: u8,
        velocity: u8,
    },
    PreviewNoteOff {
        pitch: u8,
    },
}

/// One sounding metronome click: a short decaying sine.
#[derive(Debug, Clone, Copy)]
struct Click {
    /// Sample offset within the current block at which it starts (0 once running).
    start: usize,
    phase: f32,
    samples_left: usize,
    total: usize,
    freq: f32,
    gain: f32,
}

/// Everything a note says about *how* it should sound, as opposed to when and at what pitch.
///
/// FL Studio's piano roll and step sequencer expose the same nine property lanes because in FL
/// a step simply *is* a note; `dissonant` follows that, so one struct travels from either
/// editor to whichever voice plays it. Pitch, velocity and timing are the event's own fields;
/// these five are the ones a voice has to be told about.
///
/// `Copy` and allocation-free: it rides inside the scheduler's event enum, which is pushed by
/// value on the audio thread. `bend` is what makes it wide — a note's whole pitch contour,
/// inlined at its hard cap rather than borrowed, because the audio thread must not follow a
/// pointer into a [`Sequence`] that an edit may swap out from under a sounding note.
///
/// [`Sequence`]: dissonant_core::sequence::Sequence
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteShape {
    /// 0 hard left … 0.5 centre … 1 hard right.
    pub pan: f32,
    /// Scales the voice's release; 0.5 leaves the instrument's own tail.
    pub release: f32,
    /// Detune in cents.
    pub fine: i32,
    /// Filter cutoff against the track's tone; 0.5 is the track's own.
    pub mod_x: f32,
    /// Filter resonance; 0.5 is flat.
    pub mod_y: f32,
    /// The note's pitch contour on top of `fine`, already resolved onto the sample clock.
    pub bend: Bend,
}

impl NoteShape {
    /// Every axis at rest: what a note that nobody has shaped asks for, and what the live
    /// keyboard, the chord bed and auditions play with.
    pub const fn neutral() -> Self {
        NoteShape {
            pan: 0.5,
            release: 0.5,
            fine: 0,
            mod_x: 0.5,
            mod_y: 0.5,
            bend: Bend::NONE,
        }
    }

    /// The note's expression with no contour. The two schedulers below add the bend, since
    /// resolving it needs the tempo map and the sample rate that only they hold.
    pub fn from_note(note: &NoteEvent) -> Self {
        NoteShape {
            pan: note.pan,
            release: note.release,
            fine: note.fine,
            mod_x: note.mod_x,
            mod_y: note.mod_y,
            bend: Bend::NONE,
        }
    }

    /// The same expression carrying a resolved pitch contour.
    pub fn bent(mut self, bend: Bend) -> Self {
        self.bend = bend;
        self
    }
}

impl Default for NoteShape {
    fn default() -> Self {
        NoteShape::neutral()
    }
}

/// Which sequenced note a voice is playing, so that note's own end — or a rope sliding out of
/// it — finds exactly that voice, rather than whichever voice happens to hold the same pitch.
///
/// A note's index on its track and which of its hits it is: unique among everything that can
/// sound at once on a track, and the same for a hit's on and its off however the blocks fall.
/// A loop pass reuses the tags of the pass before, which is safe because the seam releases
/// everything. The live keyboard, the chord bed and auditions carry no tag and are released by
/// pitch. `Copy` and one word, for the same reason as [`NoteShape`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceTag(pub u32);

impl VoiceTag {
    /// Hit `hit` (at most 14, see [`sub_hits`]) of note `note` on its track.
    pub fn for_hit(note: usize, hit: usize) -> Self {
        VoiceTag(((note as u32) << 4) | (hit as u32 & 0xF))
    }
}

/// What turns a note-on into a slide: a live rope from another note on the same track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slide {
    /// The voice to take over: the tag of the note the rope comes from.
    pub from: VoiceTag,
    /// How long the glide (and the easing in of the new note's expression) takes. `None` uses
    /// the patch's glide, which the synth knows and the scheduler doesn't need to.
    pub secs: Option<f32>,
}

/// How many times a note strikes. FL's `repeats` is a count of *extra* hits spread evenly
/// across the note's own length, so `r` sounds it `r + 1` times and 0 plays it once.
#[inline]
fn sub_hits(note: &NoteEvent) -> usize {
    note.repeats.min(14) as usize + 1
}

/// Which of `note`'s hits is sounding at `tick`, a hit counting as sounding up to and including
/// its end: a rope that starts exactly where its source ends still catches it (touching is
/// live). Only asked about ticks after the note's start.
fn hit_at(note: &NoteEvent, tick: f64) -> usize {
    let hits = sub_hits(note);
    let len = note.length_ticks as f64 / hits as f64;
    let start = note.shifted_start() as f64;
    (0..hits)
        .find(|&h| tick <= start + len * (h + 1) as f64)
        .unwrap_or(hits - 1)
}

/// The slide a live rope gives the first hit of the note it ends on: from the hit of its source
/// that is sounding as the note starts.
fn rope_slide(notes: &[NoteEvent], rope: NoteRope, note: &NoteEvent) -> Option<Slide> {
    let from = rope.from?;
    let source = &notes[from];
    Some(Slide {
        from: VoiceTag::for_hit(from, hit_at(source, note.shifted_start() as f64)),
        secs: note.slide_secs,
    })
}

/// The hit of `note` whose voice a live rope out of it takes over, if any. That hit's own end
/// must not release the voice: it belongs to the slide note by then, and the slide note's end
/// releases it.
fn handed_over_hit(notes: &[NoteEvent], rope: NoteRope, note: &NoteEvent) -> Option<usize> {
    rope.to
        .map(|to| hit_at(note, notes[to].shifted_start() as f64))
}

/// How a track's instrument plays the ropes on it; see [`Engine::ropes_on`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RopePlay {
    /// The built-in synth: the slide note takes its source's voice over by tag and glides it,
    /// and the source's own end is never sent.
    Glide,
    /// A hosted plugin, which is addressed by pitch and has no voice to hand over. The source
    /// is held just past its slide note's start instead (see [`overlap_end`]), so a plugin in
    /// legato or mono mode sees two keys overlap and glides with its own portamento. The slide
    /// note itself plays and ends as written.
    Overlap,
}

/// Where a plugin track ends the hit of `notes[source]` that a live rope hands to
/// `notes[target]`, if not at its written end `end`. Everything is on the swung ticks the
/// scheduler works in, and the result is clamped to the timeline like every other end.
///
/// Only a source that touches its slide note (ends at or before its start) is stretched, to
/// [`ROPE_OVERLAP_SECS`] past that start: one that already overlaps it already glides on a
/// legato plugin. The overlap is a fixed time rather than a number of ticks so it means the
/// same thing at every tempo. `None` too when the two notes share a pitch (the rope is a tie,
/// and a plugin would take the late note-off as ending the slide note). Whether the stretched
/// end would cut another note of the same key is a separate question
/// ([`key_starts_between`]), asked only when the end is about to be scheduled.
///
/// A chord's ropes are each stretched, so a polyphonic plugin hears every pair overlap for a
/// moment: a pitch-addressed plugin can't glide several voices separately. MPE could, and is
/// not built.
fn overlap_end(
    notes: &[NoteEvent],
    source: usize,
    target: usize,
    end: f64,
    swing: impl Fn(f64) -> f64,
    map: &TempoMap,
    length: f64,
) -> Option<f64> {
    let (src, tgt) = (&notes[source], &notes[target]);
    let on = swing(tgt.shifted_start() as f64);
    if end > on || src.pitch == tgt.pitch {
        return None;
    }
    Some(
        map.tick_at_seconds(map.seconds_at_tick(on) + ROPE_OVERLAP_SECS)
            .min(length),
    )
}

/// Whether any hit of another note of `notes[source]`'s pitch — including the source's own
/// later repeats — starts within `[from, to]` (swung ticks). A plugin knows a note only by its
/// key, so a source held on across such a start would end that note instead of its own.
fn key_starts_between(
    notes: &[NoteEvent],
    source: usize,
    from: f64,
    to: f64,
    swing: impl Fn(f64) -> f64,
) -> bool {
    let pitch = notes[source].pitch;
    notes
        .iter()
        .filter(|n| !n.muted && n.pitch == pitch)
        .any(|n| {
            let hits = sub_hits(n);
            let len = n.length_ticks as f64 / hits as f64;
            let start = n.shifted_start() as f64;
            // The stretched hit itself, and the source's hits before it, start before `from`.
            (0..hits).any(|h| (from..=to).contains(&swing(start + len * h as f64)))
        })
}

/// A note's pitch envelope resolved onto the sample clock for the strike that sounds at song
/// tick `at`, with the playhead standing at `now` (`now > at` only when a seek lands inside
/// the note, and the bend is then picked up mid-flight rather than restarted).
///
/// Envelope ticks are relative to the strike and go through the tempo map, so a bend drawn
/// across a tempo ramp still lands where it was drawn.
fn note_bend(note: &NoteEvent, map: &TempoMap, at: f64, now: f64, sr: f64) -> Bend {
    if note.pitch_envelope.is_empty() {
        return Bend::NONE;
    }
    let t0 = map.seconds_at_tick(at);
    Bend::new(
        &note.pitch_envelope,
        |tick| (map.seconds_at_tick(at + tick as f64) - t0) * sr,
        ((map.seconds_at_tick(now) - t0) * sr).max(0.0) as u32,
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Event {
    offset: usize,
    kind: EventKind,
}

/// `NoteOn` carries a whole [`NoteShape`] (its bend included) and dwarfs the other variants.
/// Boxing it would put an allocation on the audio thread for every note, so the size stays.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum EventKind {
    NoteOn {
        track: usize,
        pitch: u8,
        velocity: u8,
        shape: NoteShape,
        tag: VoiceTag,
        slide: Option<Slide>,
    },
    NoteOff {
        track: usize,
        pitch: u8,
        tag: VoiceTag,
    },
    ChordOn {
        pitch: u8,
    },
    ChordOff {
        pitch: u8,
    },
    ReleaseAll,
    Click {
        accent: bool,
    },
}

/// Whether the beat click at `tick` is the first of its bar, and so the accented one.
///
/// A bar is not always a whole number of beats: 7/8 is 3360 ticks against a 960-tick beat, so
/// after the first bar no beat ever lands exactly on a bar line, and testing `tick % bar == 0`
/// drops the accent for every bar that does not happen to coincide. The accent belongs to the
/// first click at or after each bar line instead — the one a player counts as "1". Only one
/// beat can fall inside the first beat's worth of a bar, so exactly one click per bar accents.
#[inline]
fn accents_bar(tick: i64, bar_ticks: i64) -> bool {
    tick.rem_euclid(bar_ticks.max(1)) < TICKS_PER_BEAT
}

struct Audition {
    track_id: Uuid,
    pitch: u8,
    samples_left: usize,
}

/// One track's automation sampled at the two ends of a sub-block.
///
/// Hot: called once per channel per sub-block on the audio thread, so it allocates nothing and
/// leaves early for the overwhelmingly common track with no lanes at all.
fn channel_auto(auto: &TrackAutomation, from: f64, to: f64) -> ChannelAuto {
    if auto.is_empty() {
        return ChannelAuto::default();
    }
    let ramp = |c: &Option<AutomationCurve>| {
        c.as_ref()
            .map(|c| (c.value_at(from) as f32, c.value_at(to) as f32))
    };
    ChannelAuto {
        volume: ramp(&auto.volume),
        pan: ramp(&auto.pan),
        reverb_send: ramp(&auto.reverb_send),
        tone_hz: auto.tone.as_ref().map(|c| c.value_at(from) as f32),
    }
}

pub struct Engine {
    sample_rate: f32,
    shared: Arc<Shared>,
    sequence: Arc<Sequence>,
    channels: Vec<Channel>,
    channel_ids: Vec<Uuid>,
    chord_bed: PolySynth,
    hear_chords: bool,
    master: Master,
    /// The master bus's own effect chain, run over the mixed bus ahead of `Master::process`.
    master_inserts: Vec<InsertSlot>,
    playing: bool,
    looping: bool,
    /// Loop sub-range in ticks.
    loop_region: Option<(f64, f64)>,
    /// Armed punch range in ticks. Blocks looping and gates capture while it is set.
    punch: Option<(f64, f64)>,
    /// Playhead in ticks; fractional between blocks.
    position: f64,
    tempo: Tempo,
    mix_l: Vec<f32>,
    mix_r: Vec<f32>,
    events: Vec<Event>,
    sounding_chord: Option<Uuid>,
    auditions: Vec<Audition>,
    /// Set by Play/Seek: the next block starts any note the playhead is sitting inside.
    resume_notes: bool,
    /// Samples of test tone left to play, and its phase.
    test_tone: (usize, f32),
    metronome: bool,
    metronome_volume: f32,
    clicks: Vec<Click>,
    /// Count-in: total samples and samples elapsed; `None` when not counting in.
    count_in: Option<(usize, usize)>,
    /// Samples into the current block at which the transport (re)starts; 0 normally.
    late_start: usize,

    /// Decoded audio, shared with the app thread. Empty until the first `SetAudioPool`.
    audio_pool: Arc<AudioPool>,
    /// The audio track that hears the live input, and whether it is monitored.
    armed: Option<Uuid>,
    monitor: bool,
    recording: bool,
    /// True once the transport actually started under `recording`, so a count-in is excluded.
    record_started: bool,
    /// This block's live input, mono, one sample per output frame.
    input_block: Vec<f32>,
    /// Input samples kept in hand after each block. See [`Engine::pull_input`].
    input_slack: usize,
    /// Consecutive blocks with no input underrun, for easing `input_slack` back down.
    clean_blocks: u32,
    /// Live input from the capture device. `None` offline or when no input device is open.
    input: Option<rtrb::Consumer<f32>>,
    /// Where recorded samples go to be written to disk. `None` offline.
    capture: Option<rtrb::Producer<f32>>,
    /// The tuner strip is open: silence the output and keep detection running.
    tuner_open: bool,
    /// Pitch detection over the live input. Allocated with the engine rather than when the
    /// tuner opens, because opening it happens on the audio thread.
    tuner: PitchTracker,
    /// The SOUND page's instrument: outside the track list, audible with the transport
    /// stopped, and untouched by mute / solo.
    preview: Option<Instrument>,
    /// `SetPreview(None)` arrived: let the preview ring out, then drop it.
    preview_closing: bool,
    /// Stereo scratch for the preview, and the scope's mono fold.
    preview_buf: (Vec<f32>, Vec<f32>),
    /// Tracks the scheduler plays ropes on as if they hosted a plugin. A real plugin can't be
    /// opened without one installed, and only the scheduling is under test.
    #[cfg(test)]
    pretend_plugin: Vec<usize>,
}

/// The chord bed's sound: the factory pad.
fn chord_bed_patch() -> SynthPatch {
    match factory_sound("pad").map(|s| &s.patch) {
        Some(SoundPatch::Synth(p)) => *p,
        _ => SynthPatch::default(),
    }
}

impl Engine {
    pub fn new(sample_rate: f32, shared: Arc<Shared>, master: &MasterSettings) -> Self {
        shared
            .sample_rate
            .store(sample_rate as u32, Ordering::Relaxed);
        Engine {
            sample_rate,
            shared,
            sequence: Arc::new(Sequence::default()),
            channels: Vec::new(),
            channel_ids: Vec::new(),
            chord_bed: PolySynth::new(&chord_bed_patch(), sample_rate, 8),
            hear_chords: false,
            master: Master::new(sample_rate, master),
            master_inserts: Vec::new(),
            playing: false,
            looping: true,
            loop_region: None,
            punch: None,
            position: 0.0,
            tempo: Tempo::default(),
            mix_l: vec![0.0; 8192],
            mix_r: vec![0.0; 8192],
            events: Vec::with_capacity(512),
            sounding_chord: None,
            auditions: Vec::with_capacity(32),
            resume_notes: false,
            test_tone: (0, 0.0),
            metronome: false,
            metronome_volume: 0.6,
            clicks: Vec::with_capacity(8),
            count_in: None,
            late_start: 0,
            audio_pool: Arc::new(AudioPool::default()),
            armed: None,
            monitor: false,
            recording: false,
            record_started: false,
            input_block: vec![0.0; 8192],
            input_slack: 0,
            clean_blocks: 0,
            input: None,
            capture: None,
            tuner_open: false,
            tuner: PitchTracker::new(sample_rate),
            preview: None,
            preview_closing: false,
            preview_buf: (vec![0.0; 8192], vec![0.0; 8192]),
            #[cfg(test)]
            pretend_plugin: Vec::new(),
        }
    }

    /// Hand the engine its live-input and take-capture rings. Called once by the device after
    /// the engine is built, so `Engine::new` stays usable offline and in tests.
    pub fn attach_io(&mut self, input: rtrb::Consumer<f32>, capture: rtrb::Producer<f32>) {
        self.input = Some(input);
        self.capture = Some(capture);
    }

    /// Queue a click at `offset` samples into the current block.
    fn click(&mut self, offset: usize, accent: bool) {
        if self.clicks.len() >= 8 {
            self.clicks.remove(0);
        }
        let total = (self.sample_rate * if accent { 0.045 } else { 0.03 }) as usize;
        self.clicks.push(Click {
            start: offset,
            phase: 0.0,
            samples_left: total,
            total,
            freq: if accent { 1760.0 } else { 1320.0 },
            gain: self.metronome_volume * if accent { 0.5 } else { 0.32 },
        });
    }

    /// Mix the pending clicks into an interleaved stereo block (after the master chain).
    fn render_clicks(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        for c in &mut self.clicks {
            let from = c.start.min(frames);
            for i in from..frames {
                if c.samples_left == 0 {
                    break;
                }
                let t = 1.0 - c.samples_left as f32 / c.total as f32;
                let env = (1.0 - t).powi(3);
                let s = c.phase.sin() * c.gain * env;
                out[2 * i] += s;
                out[2 * i + 1] += s;
                c.phase += 2.0 * std::f32::consts::PI * c.freq / self.sample_rate;
                c.samples_left -= 1;
            }
            c.start = 0;
        }
        self.clicks.retain(|c| c.samples_left > 0);
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Samples by which the engine's music output trails the transport, from the master
    /// limiter's lookahead. The metronome and test tone are summed in after it and are not
    /// delayed, which is what makes a click a reliable reference while playing.
    pub fn output_latency_samples(&self) -> usize {
        self.master.latency_samples()
    }

    pub fn handle(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => {
                self.count_in = None;
                self.playing = true;
                // Starting mid-note (after a stop or a seek) must sound that note, not wait for
                // the next one.
                self.resume_notes = true;
            }
            EngineCommand::Stop => {
                self.playing = false;
                self.count_in = None;
                self.release_all();
            }
            EngineCommand::SetMetronome { on, volume } => {
                self.metronome = on;
                self.metronome_volume = volume.clamp(0.0, 1.0);
            }
            EngineCommand::PlayWithCountIn { bars } => {
                let ticks = bars as f64 * self.sequence.bar_ticks.max(1) as f64;
                let total = (ticks * self.tempo.samples_per_tick(self.sample_rate as f64)) as usize;
                if total == 0 {
                    self.playing = true;
                } else {
                    self.playing = false;
                    self.release_all();
                    self.count_in = Some((total, 0));
                }
            }
            EngineCommand::Seek { tick } => {
                self.release_all();
                self.position = tick.max(0.0);
                let (start, end) = self.loop_bounds();
                if self.loop_active() && self.position >= end {
                    self.position = start;
                }
                self.resume_notes = true;
            }
            EngineCommand::SetLooping(on) => self.looping = on,
            EngineCommand::SetLoop { start, end } => {
                if end > start && start >= 0.0 {
                    self.loop_region = Some((start, end));
                } else {
                    self.loop_region = None;
                }
            }
            EngineCommand::ClearLoop => self.loop_region = None,
            EngineCommand::SetPunch { start, end } => {
                self.punch = (end > start && start >= 0.0).then_some((start, end));
            }
            EngineCommand::ClearPunch => self.punch = None,
            EngineCommand::NoteOn {
                track_id,
                pitch,
                velocity,
            } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_on(pitch, velocity, NoteShape::neutral());
                }
                self.auditions
                    .retain(|a| !(a.track_id == track_id && a.pitch == pitch));
            }
            EngineCommand::NoteOff { track_id, pitch } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_off(pitch);
                }
                self.auditions
                    .retain(|a| !(a.track_id == track_id && a.pitch == pitch));
            }
            EngineCommand::SetSequence(seq) => self.set_sequence(seq, None),
            EngineCommand::SetSequenceWithDrums { sequence, drums } => {
                self.set_sequence(sequence, Some(&drums))
            }
            EngineCommand::SetPreview(patch) => self.set_preview(patch),
            EngineCommand::PreviewNoteOn { pitch, velocity } => {
                match (&mut self.preview, self.preview_closing) {
                    // A drum preview holds exactly one pad, which every key plays.
                    (Some(Instrument::Drums(kit)), false) => {
                        kit.trigger(0, velocity, NoteShape::neutral())
                    }
                    (Some(inst), false) => inst.note_on(pitch, velocity, NoteShape::neutral()),
                    _ => {}
                }
            }
            EngineCommand::PreviewNoteOff { pitch } => {
                if let Some(inst) = &mut self.preview {
                    inst.note_off(pitch);
                }
            }
            EngineCommand::SetHearChords(on) => {
                self.hear_chords = on;
                if !on {
                    self.chord_bed.all_notes_off();
                    self.sounding_chord = None;
                }
            }
            EngineCommand::SetMaster(settings) => self.master.apply(&settings),
            EngineCommand::Audition {
                track_id,
                pitch,
                velocity,
                seconds,
            } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_on(pitch, velocity, NoteShape::neutral());
                    let hold = (self.sample_rate
                        * seconds.clamp(MIN_AUDITION_SECS, MAX_AUDITION_SECS))
                        as usize;
                    if let Some(a) = self
                        .auditions
                        .iter_mut()
                        .find(|a| a.track_id == track_id && a.pitch == pitch)
                    {
                        a.samples_left = hold;
                    } else {
                        self.auditions.push(Audition {
                            track_id,
                            pitch,
                            samples_left: hold,
                        });
                    }
                }
            }
            EngineCommand::AuditionOff { track_id, pitch } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_off(pitch);
                }
                self.auditions
                    .retain(|a| !(a.track_id == track_id && a.pitch == pitch));
            }
            EngineCommand::TestTone => self.test_tone = ((self.sample_rate * 0.6) as usize, 0.0),
            EngineCommand::ClearClip => self.shared.master_over.store(false, Ordering::Relaxed),
            EngineCommand::ClearInputPeak => {
                self.shared
                    .input_peak_hold
                    .store(0f32.to_bits(), Ordering::Relaxed);
                self.shared.input_over.store(false, Ordering::Relaxed);
            }
            EngineCommand::SetTuner(on) => {
                self.tuner_open = on;
                // Start from nothing either way: a reading left over from the last time the
                // tuner was open would show up under the next string for a hop or two.
                self.tuner.reset();
                self.shared.tuner_hz.store(0f32.to_bits(), Ordering::Relaxed);
                self.shared
                    .tuner_clarity
                    .store(0f32.to_bits(), Ordering::Relaxed);
            }
            EngineCommand::SetInstrument {
                track_id,
                instrument,
            } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    // Release what was sounding first: the outgoing instrument is about to be
                    // dropped and the incoming one has no idea which keys are held.
                    ch.instrument.all_notes_off();
                    ch.instrument = *instrument;
                }
            }
            EngineCommand::ClearInstrument { track_id } => {
                let fallback = self
                    .sequence
                    .tracks
                    .iter()
                    .find(|t| t.id == track_id)
                    // No bank here: a plugin being cleared is rare, and this renders the kit
                    // the track already describes.
                    .map(|t| Instrument::for_track(t, self.sample_rate, None));
                if let (Some(ch), Some(instrument)) = (self.channel_mut(&track_id), fallback) {
                    ch.instrument.all_notes_off();
                    ch.instrument = instrument;
                }
            }
            EngineCommand::SetPluginParam {
                track_id,
                param_id,
                value,
            } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    if let Instrument::Plugin(p) = &mut ch.instrument {
                        p.set_param(param_id, value);
                    }
                }
            }
            EngineCommand::SetAudioPool(pool) => {
                self.audio_pool = pool;
            }
            EngineCommand::SetAudioArm { track_id, monitor } => {
                self.armed = track_id;
                self.monitor = monitor;
                self.apply_arm();
            }
            EngineCommand::StartRecord => {
                self.recording = true;
                self.record_started = false;
                self.shared.recording.store(false, Ordering::Relaxed);
                self.shared.record_frames.store(0, Ordering::Relaxed);
            }
            EngineCommand::StopRecord => {
                self.recording = false;
                self.record_started = false;
                self.shared.recording.store(false, Ordering::Relaxed);
            }
            EngineCommand::AddInsert {
                target,
                index,
                slot,
            } => {
                if let Some(chain) = self.chain_mut(target) {
                    // The document enforces the same ceiling; the audio thread refuses to grow
                    // past it whatever reaches it, so the per-block cost stays bounded.
                    if chain.len() < MAX_INSERTS {
                        let at = index.min(chain.len());
                        chain.insert(at, slot);
                    }
                }
            }
            EngineCommand::RemoveInsert { target, id } => {
                if let Some(chain) = self.chain_mut(target) {
                    chain.retain(|s| s.id != id);
                }
            }
            EngineCommand::MoveInsert { target, id, to } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(from) = chain.iter().position(|s| s.id == id) {
                        let to = to.min(chain.len() - 1);
                        let slot = chain.remove(from);
                        chain.insert(to, slot);
                    }
                }
            }
            EngineCommand::SetInsertEnabled { target, id, on } => {
                if let Some(chain) = self.chain_mut(target) {
                    if let Some(slot) = chain.iter_mut().find(|s| s.id == id) {
                        slot.enabled = on;
                    }
                }
            }
            EngineCommand::SetInsertParam {
                id,
                param_id,
                value,
            } => {
                if let Some(slot) = self.insert_mut(id) {
                    slot.set_param(param_id, value);
                }
            }
        }
        self.publish();
    }

    /// The active loop as `(start, end)`: the region if one is set (clamped to the
    /// sequence), else the whole sequence. In ticks.
    fn loop_bounds(&self) -> (f64, f64) {
        let len = self.sequence.length_ticks.max(0) as f64;
        match self.loop_region {
            Some((s, e)) if e > s => (s.max(0.0).min(len), e.min(len).max(s.max(0.0))),
            _ => (0.0, len),
        }
    }

    /// Whether the transport actually wraps. An armed punch suppresses looping outright: the
    /// loop would carry the playhead back over the punch range and record it a second time,
    /// which is take stacking by accident. The UI says so too; this is the backstop.
    fn loop_active(&self) -> bool {
        self.looping && self.punch.is_none()
    }

    fn channel_mut(&mut self, id: &Uuid) -> Option<&mut Channel> {
        let idx = self.channel_ids.iter().position(|c| c == id)?;
        self.channels.get_mut(idx)
    }

    /// The chain an [`InsertTarget`] names. The master's always exists; a track's only exists
    /// while a channel does — a command for a track that has since gone is dropped.
    fn chain_mut(&mut self, target: InsertTarget) -> Option<&mut Vec<InsertSlot>> {
        match target {
            InsertTarget::Track(id) => self.channel_mut(&id).map(|ch| &mut ch.inserts),
            InsertTarget::Master => Some(&mut self.master_inserts),
        }
    }

    /// One slot anywhere in the engine, by its own id. Parameter moves are addressed this way,
    /// so the app does not have to say which chain a knob belongs to.
    fn insert_mut(&mut self, id: Uuid) -> Option<&mut InsertSlot> {
        self.channels
            .iter_mut()
            .flat_map(|ch| ch.inserts.iter_mut())
            .chain(self.master_inserts.iter_mut())
            .find(|slot| slot.id == id)
    }

    fn set_preview(&mut self, patch: Option<PreviewPatch>) {
        let sr = self.sample_rate;
        match patch {
            None => {
                if let Some(inst) = &mut self.preview {
                    inst.all_notes_off();
                    self.preview_closing = true;
                }
            }
            Some(PreviewPatch::Synth(p)) => {
                match &mut self.preview {
                    Some(Instrument::Synth(s)) => s.set_patch(&p),
                    _ => {
                        let synth = Box::new(PolySynth::new(&p, sr, SYNTH_POLYPHONY));
                        self.preview = Some(Instrument::Synth(synth));
                    }
                }
                self.preview_closing = false;
            }
            Some(PreviewPatch::Drum(drum)) => {
                // The rates differ only around a device restart; rendering here then beats
                // playing the drum at the wrong speed.
                let drum = if drum.sample_rate == sr {
                    drum
                } else {
                    Arc::new(RenderedDrum::new(&drum.patch, sr))
                };
                match &mut self.preview {
                    Some(Instrument::Drums(kit)) => kit.set_drum(0, drum),
                    _ => self.preview = Some(Instrument::Drums(DrumKit::single(drum))),
                }
                self.preview_closing = false;
            }
        }
    }

    /// Mix the preview channel into the first `frames` of the mix (centred, like a track at
    /// pan 0) and feed the scope its mono sum.
    fn render_preview(&mut self, frames: usize) {
        if self.preview_buf.0.len() < frames {
            self.preview_buf.0.resize(frames, 0.0);
            self.preview_buf.1.resize(frames, 0.0);
        }
        let (pl, pr) = (
            &mut self.preview_buf.0[..frames],
            &mut self.preview_buf.1[..frames],
        );
        pl.fill(0.0);
        pr.fill(0.0);
        if let Some(inst) = &mut self.preview {
            inst.render_add(pl, pr);
            if self.preview_closing && inst.is_silent() {
                self.preview = None;
                self.preview_closing = false;
            }
            let (gl, gr) = pan_gains(0.0);
            for i in 0..frames {
                self.mix_l[i] += pl[i] * gl;
                self.mix_r[i] += pr[i] * gr;
            }
        }
        // The scope draws one trace, so it sees the mono sum. `pl` is free to be overwritten:
        // the mix already has what it needs.
        for i in 0..frames {
            pl[i] = 0.5 * (pl[i] + pr[i]);
        }
        self.shared.write_scope(pl);
    }

    fn set_sequence(&mut self, seq: Arc<Sequence>, drums: Option<&DrumBank>) {
        // Rebuild the channel list in sequence order, keeping existing channels (and their
        // sounding voices) for tracks that are still present.
        let mut new_channels = Vec::with_capacity(seq.tracks.len());
        let mut new_ids = Vec::with_capacity(seq.tracks.len());
        let old_ids = std::mem::take(&mut self.channel_ids);
        let mut old = std::mem::take(&mut self.channels)
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        for track in &seq.tracks {
            let existing = old_ids
                .iter()
                .position(|id| id == &track.id)
                .and_then(|i| old[i].take());
            let mut ch = match existing {
                Some(ch) => ch,
                None => Channel::new(track, self.sample_rate, drums),
            };
            ch.apply(track, drums);
            new_channels.push(ch);
            new_ids.push(track.id);
        }
        // Notes that vanished or moved under the playhead: release everything that is
        // sounding on a track whose note list changed, so nothing hangs.
        let old_seq = std::mem::replace(&mut self.sequence, seq);
        for (i, track) in self.sequence.tracks.iter().enumerate() {
            let changed = old_seq
                .tracks
                .iter()
                .find(|t| t.id == track.id)
                .map(|t| {
                    t.notes != track.notes
                        || t.audible(old_seq.any_solo()) != track.audible(self.sequence.any_solo())
                })
                .unwrap_or(true);
            if changed {
                new_channels[i].instrument.all_notes_off();
            }
        }
        if old_seq.chords != self.sequence.chords {
            self.chord_bed.all_notes_off();
            self.sounding_chord = None;
        }
        self.channels = new_channels;
        self.channel_ids = new_ids;
        self.apply_arm();
        self.tempo = Tempo::new(self.sequence.tempo_bpm);
        let (start, end) = self.loop_bounds();
        if self.loop_active() && self.position >= end {
            self.position = start;
        }
    }

    /// Push the current arm onto the channels. A rebuilt or replaced lane starts unmonitored,
    /// so this runs after every sequence change as well as on the arm command itself.
    fn apply_arm(&mut self) {
        for (ch, id) in self.channels.iter_mut().zip(self.channel_ids.iter()) {
            if let Some(lane) = &mut ch.audio {
                lane.monitor = self.monitor && self.armed == Some(*id);
            }
        }
    }

    fn release_all(&mut self) {
        for ch in &mut self.channels {
            ch.instrument.all_notes_off();
        }
        self.chord_bed.all_notes_off();
        self.sounding_chord = None;
        self.auditions.clear();
    }

    fn publish(&self) {
        self.shared
            .position_bits
            .store(self.position.to_bits(), Ordering::Relaxed);
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        for (i, ch) in self.channels.iter().enumerate().take(MAX_TRACKS) {
            self.shared.track_peaks[i].store(ch.peak.to_bits(), Ordering::Relaxed);
        }
        self.shared.master_peak[0].store(self.master.peak_l.to_bits(), Ordering::Relaxed);
        self.shared.master_peak[1].store(self.master.peak_r.to_bits(), Ordering::Relaxed);
        if self.master.over {
            // Latch, never clear: only `ClearClip` lowers it.
            self.shared.master_over.store(true, Ordering::Relaxed);
        }
        // Still published in beats: it is a UI countdown ("3, 2, 1"), not a time position.
        let left = match self.count_in {
            Some((total, done)) => {
                (self
                    .tempo
                    .ticks_for_samples((total - done) as f64, self.sample_rate as f64)
                    / TICKS_PER_BEAT as f64) as f32
            }
            None => 0.0,
        };
        self.shared
            .count_in
            .store(left.to_bits(), Ordering::Relaxed);
    }

    /// Render one block of interleaved stereo. `out.len()` must be even.
    pub fn process(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        if self.mix_l.len() < frames {
            self.mix_l.resize(frames, 0.0);
            self.mix_r.resize(frames, 0.0);
        }
        self.mix_l[..frames].fill(0.0);
        self.mix_r[..frames].fill(0.0);
        self.pull_input(frames);
        self.run_tuner(frames);

        self.tick_count_in(frames);
        self.capture_take(frames);
        self.collect_events(frames);
        self.tick_auditions(frames);

        // Render sub-blocks between events so every note starts on its exact sample.
        let mut cursor = 0usize;
        let mut ev_idx = 0usize;
        while cursor < frames {
            let next = self
                .events
                .get(ev_idx)
                .map(|e| e.offset.min(frames))
                .unwrap_or(frames);
            if next > cursor {
                self.render_range(cursor, next);
                cursor = next;
            }
            while let Some(e) = self
                .events
                .get(ev_idx)
                .filter(|e| e.offset.min(frames) <= cursor)
            {
                match e.kind {
                    EventKind::Click { accent } => self.click(cursor, accent),
                    kind => self.fire(kind),
                }
                ev_idx += 1;
            }
        }

        self.render_preview(frames);
        // The master lane spans the whole block: nothing between the tracks and the master is
        // cut on an event boundary, so there are no sub-blocks to follow here.
        let (master_from, master_to) = self.auto_span(0, frames);
        let master_gain = self
            .sequence
            .master_automation
            .gain
            .as_ref()
            .map(|c| (c.value_at(master_from) as f32, c.value_at(master_to) as f32));
        self.master.set_auto_gain(master_gain);
        let beat_seconds = self.beat_seconds();
        let (l, r) = (&mut self.mix_l[..frames], &mut self.mix_r[..frames]);
        // The master's own inserts hear the mixed bus exactly as it leaves the tracks, ahead of
        // the low cut / EQ / reverb / gain the master settings describe.
        run_inserts(&mut self.master_inserts, l, r, beat_seconds);
        self.master.process(l, r);
        if self.test_tone.0 > 0 {
            // 440 Hz at -12 dBFS with a 10 ms fade at both ends, after the master chain so it
            // reaches the device no matter how the mix is set.
            let total = (self.sample_rate * 0.6) as usize;
            let fade = (self.sample_rate * 0.01).max(1.0) as usize;
            let step = 2.0 * std::f32::consts::PI * 440.0 / self.sample_rate;
            for i in 0..frames {
                if self.test_tone.0 == 0 {
                    break;
                }
                let done = total - self.test_tone.0;
                let env = (done.min(fade) as f32 / fade as f32)
                    .min(self.test_tone.0.min(fade) as f32 / fade as f32);
                let s = self.test_tone.1.sin() * 0.25 * env;
                l[i] += s;
                r[i] += s;
                self.test_tone.1 = (self.test_tone.1 + step) % (2.0 * std::f32::consts::PI);
                self.test_tone.0 -= 1;
            }
        }
        for i in 0..frames {
            out[2 * i] = l[i];
            out[2 * i + 1] = r[i];
        }
        if !self.clicks.is_empty() {
            self.render_clicks(out);
        }
        // The safety net sits here, after the metronome and the test tone, so the device never
        // receives an over — while those monitor sounds stay out of the limiter's detector and
        // out of the meters. With the limiter on this is a no-op on the music itself.
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        if self.tuner_open {
            // After the limiter and the clicks, so nothing — mix, metronome, monitored rig —
            // survives the tuner being open. The meters above still read the real signal, so
            // the input strip keeps working while the room stays quiet.
            out.fill(0.0);
        }

        if self.playing {
            self.position = self.advance(self.position, frames - self.late_start.min(frames));
        }
        self.late_start = 0;
        self.publish();
    }

    /// Feed the tuner this block's input while it is open and the transport is stopped.
    ///
    /// Stopped only: playing back and tuning at once is not a thing anyone does, and the
    /// search is the one piece of per-block work here that is not proportional to the mix.
    fn run_tuner(&mut self, frames: usize) {
        if !self.tuner_open || self.playing {
            return;
        }
        if !self.tuner.push(&self.input_block[..frames]) {
            return;
        }
        // A search that heard nothing publishes zero: a muted string has to stop reading as
        // whatever was last plucked, or the needle lies while you are reaching for the peg.
        let (hz, clarity) = self.tuner.last().map(|p| (p.hz, p.clarity)).unwrap_or((0.0, 0.0));
        self.shared.tuner_hz.store(hz.to_bits(), Ordering::Relaxed);
        self.shared
            .tuner_clarity
            .store(clarity.to_bits(), Ordering::Relaxed);
    }

    /// While counting in: click on each beat boundary inside this block and, when the count
    /// runs out, start the transport at the exact sample (the remainder of the block plays).
    fn tick_count_in(&mut self, frames: usize) {
        let Some((total, done)) = self.count_in else {
            return;
        };
        let spb = self.tempo.samples_per_tick(self.sample_rate as f64) * TICKS_PER_BEAT as f64;
        let bar = self.sequence.bar_ticks.max(1);
        let end = (done + frames).min(total);
        // Beat boundaries at k·spb for integer k in [done, end); the accent falls on a bar line.
        let mut k = (done as f64 / spb).ceil() as usize;
        while (k as f64 * spb) < end as f64 {
            let at = (k as f64 * spb).round() as usize;
            if at >= done {
                self.click(at - done, accents_bar(k as i64 * TICKS_PER_BEAT, bar));
            }
            k += 1;
        }
        if end >= total {
            self.count_in = None;
            self.playing = true;
            // The transport starts `total - done` samples into this block.
            self.late_start = total - done;
        } else {
            self.count_in = Some((total, end));
        }
    }

    /// Fill `input_block` with this block's live input.
    ///
    /// Input and output are separate device streams with independent clocks, so the ring both
    /// drifts and arrives out of phase. Whatever is left in the ring after a block is latency
    /// the player feels, so this keeps it as small as it can get away with: trim to
    /// `frames + slack` before reading, where `slack` starts at half a block — the floor that
    /// absorbs the phase offset between two callbacks — and grows only when the ring actually
    /// runs dry, easing back down once it stops.
    ///
    /// A fixed, generous target would be simpler and is what this used to do; it also put 3
    /// blocks of delay between the strings and the speaker on every rig, glitch-free or not.
    fn pull_input(&mut self, frames: usize) {
        if self.input_block.len() < frames {
            self.input_block.resize(frames, 0.0);
        }
        let block = &mut self.input_block[..frames];
        let floor = frames / 2;
        let ceiling = frames * 3;
        let slack = self.input_slack.clamp(floor, ceiling);
        let Some(input) = &mut self.input else {
            block.fill(0.0);
            self.shared
                .input_peak
                .store(0f32.to_bits(), Ordering::Relaxed);
            self.shared.input_backlog.store(0, Ordering::Relaxed);
            return;
        };

        // Anything past the target is the input stream having run ahead: drop it so the
        // newest audio is what gets monitored, not the oldest.
        let backlog = input.slots();
        for _ in slack + frames..backlog {
            let _ = input.pop();
        }
        let mut got = 0;
        while got < frames {
            match input.pop() {
                Ok(s) => {
                    block[got] = s;
                    got += 1;
                }
                Err(_) => break,
            }
        }

        if got < frames {
            block[got..].fill(0.0);
            self.shared.input_underruns.fetch_add(1, Ordering::Relaxed);
            // Buy headroom rather than keep clicking, and start the clock on giving it back.
            self.input_slack = (slack + frames / 2).min(ceiling);
            self.clean_blocks = 0;
        } else {
            self.clean_blocks = self.clean_blocks.saturating_add(1);
            // Long enough that a rare glitch does not cost the player any latency, short
            // enough that plugging in a better interface is felt within seconds.
            if self.clean_blocks >= CLEAN_BLOCKS_BEFORE_TIGHTENING && slack > floor {
                self.input_slack = slack.saturating_sub(frames / 8).max(floor);
                self.clean_blocks = 0;
            } else {
                self.input_slack = slack;
            }
        }

        let peak = block.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        self.shared
            .input_peak
            .store(peak.to_bits(), Ordering::Relaxed);
        // Hold and latch here, where every captured sample passes, rather than in the UI: the
        // meter is read a few times a second and the peak that matters when you are setting
        // gain is the one that happened between two of those reads.
        let held = f32::from_bits(self.shared.input_peak_hold.load(Ordering::Relaxed));
        if peak > held {
            self.shared
                .input_peak_hold
                .store(peak.to_bits(), Ordering::Relaxed);
        }
        if peak >= 1.0 {
            self.shared.input_over.store(true, Ordering::Relaxed);
        }
        // What is still in hand is latency the player hears, so report it rather than the
        // block size alone.
        self.shared.input_backlog.store(
            self.input.as_ref().map(|i| i.slots()).unwrap_or(0) as u32,
            Ordering::Relaxed,
        );
    }

    /// Push this block's input into the take ring while recording and playing.
    ///
    /// Capture starts at `late_start`, so a count-in is never recorded, and the take's start
    /// tick is published once, at the first captured sample.
    fn capture_take(&mut self, frames: usize) {
        if !self.recording || !self.playing {
            return;
        }
        let (first, last, start_tick) = self.capture_window(frames);
        // Before the drop-in and after the drop-out there is nothing to write and nothing to
        // announce: as far as the take is concerned those blocks did not happen.
        if self.punch.is_some() && first >= last {
            return;
        }
        if !self.record_started {
            self.record_started = true;
            self.shared
                .record_start_tick
                .store(start_tick.to_bits(), Ordering::Relaxed);
            self.shared.record_frames.store(0, Ordering::Relaxed);
            self.shared.recording.store(true, Ordering::Relaxed);
        }
        let Some(capture) = &mut self.capture else {
            return;
        };
        let mut written = 0u64;
        for &s in &self.input_block[first..last] {
            // A full ring drops samples rather than blocking: missing the audio deadline would
            // glitch everything the user is playing along to.
            if capture.push(s).is_err() {
                break;
            }
            written += 1;
        }
        self.shared
            .record_frames
            .fetch_add(written, Ordering::Relaxed);
    }

    /// Which frames of this block reach the take, and the tick the take starts on.
    ///
    /// Without a punch that is everything from `late_start`, stamped with the playhead. With
    /// one it narrows to the frames whose playhead falls inside the punch range, and the take
    /// is stamped with the punch-in tick itself: the clip has to land exactly on the punch,
    /// not within a block of it, or the crossfade it was recorded for lands somewhere else.
    fn capture_window(&self, frames: usize) -> (usize, usize, f64) {
        let base = self.late_start.min(frames);
        let Some((punch_in, punch_out)) = self.punch else {
            return (base, frames, self.position);
        };
        let map = &self.sequence.tempo_map;
        let sr = self.sample_rate as f64;
        let t0 = map.seconds_at_tick(self.position);
        // `position` is the tick at frame `base`, so that is where the block's clock starts.
        let frame_of = |tick: f64| base as f64 + (map.seconds_at_tick(tick) - t0) * sr;
        let first = (frame_of(punch_in).round().max(base as f64) as usize).min(frames);
        let last = (frame_of(punch_out).round().max(0.0) as usize).min(frames);
        (first, last, punch_in)
    }

    /// The song ticks at the first and last frame of the sub-block `[from, to)`, for reading
    /// automation. A stopped transport holds its position, so every lane holds with it.
    fn auto_span(&self, from: usize, to: usize) -> (f64, f64) {
        if !self.playing {
            return (self.position, self.position);
        }
        let map = &self.sequence.tempo_map;
        let base = map.seconds_at_tick(self.position);
        let sr = self.sample_rate as f64;
        (
            map.tick_at_seconds(base + from as f64 / sr),
            map.tick_at_seconds(base + to as f64 / sr),
        )
    }

    /// How long one beat lasts where the playhead is. A tempo-synced insert asks for this
    /// every block, so a tempo map — and a ramp inside one — carries its repeats along, and an
    /// offline bounce lands them exactly where playback did.
    fn beat_seconds(&self) -> f32 {
        (60.0 / self.sequence.tempo_map.bpm_at_tick(self.position).max(1.0)) as f32
    }

    fn render_range(&mut self, from: usize, to: usize) {
        // `seconds_at_tick` is exact and `advance` moves the transport by exactly `frames/sr`
        // seconds, so the block's start time needs no extra state of its own.
        let ctx = BlockContext {
            time_seconds: self.sequence.tempo_map.seconds_at_tick(self.position)
                + from as f64 / self.sample_rate as f64,
            playing: self.playing && from >= self.late_start,
            pool: &self.audio_pool,
            clips: &self.sequence.audio,
            input: &self.input_block[from..to],
            beat_seconds: self.beat_seconds(),
        };
        // Automation is read at the two ends of this sub-block, so a moving parameter is
        // continuous from one sub-block (and one block) to the next instead of stepping.
        let (auto_from, auto_to) = self.auto_span(from, to);
        let (l, r) = (&mut self.mix_l[from..to], &mut self.mix_r[from..to]);
        let any_solo = self.sequence.any_solo();
        for (ch, track) in self.channels.iter_mut().zip(self.sequence.tracks.iter()) {
            ch.set_auto(channel_auto(&track.automation, auto_from, auto_to));
            // A muted track whose insert is still ringing keeps rendering: the chain's own tail
            // is as real as an instrument's release.
            if track.audible(any_solo)
                || !ch.instrument.is_silent()
                || !chain_is_silent(&ch.inserts)
            {
                ch.render_add(l, r, &ctx);
            } else {
                // Muted and finished releasing: nothing to mix, but the meter still has to fall
                // instead of staying lit at the level it had when the track was muted.
                ch.decay_meter(to - from);
            }
        }
        if !self.chord_bed.is_silent() {
            let frames = to - from;
            // The bed plays neutral notes, so its two sides are identical, but the synth now
            // renders stereo — give it a pair of stack scratch buffers rather than allocating.
            let mut buf = ([0.0f32; 256], [0.0f32; 256]);
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(256);
                let (bl, br) = (&mut buf.0[..n], &mut buf.1[..n]);
                bl.fill(0.0);
                br.fill(0.0);
                self.chord_bed.render_add(bl, br);
                for i in 0..n {
                    l[done + i] += bl[i] * 0.6;
                    r[done + i] += br[i] * 0.6;
                }
                done += n;
            }
        }
    }

    fn fire(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn {
                track,
                pitch,
                velocity,
                shape,
                tag,
                slide,
            } => {
                if let Some(ch) = self.channels.get_mut(track) {
                    // Through the channel, not the instrument: a hosted instrument's bent note
                    // needs following there for its contour to reach the plugin.
                    ch.note_on_tagged(pitch, velocity, shape, tag, slide);
                }
            }
            EventKind::NoteOff { track, pitch, tag } => {
                if let Some(ch) = self.channels.get_mut(track) {
                    ch.instrument.note_off_tagged(pitch, tag);
                }
            }
            EventKind::ChordOn { pitch } => self.chord_bed.note_on(pitch, 80, NoteShape::neutral()),
            EventKind::ChordOff { pitch } => self.chord_bed.note_off(pitch),
            EventKind::ReleaseAll => {
                for ch in &mut self.channels {
                    ch.instrument.all_notes_off();
                }
                self.chord_bed.all_notes_off();
                self.sounding_chord = None;
            }
            EventKind::Click { .. } => {}
        }
    }

    /// Move the tick playhead on by `frames` samples, wrapping at the loop end.
    fn advance(&self, position: f64, frames: usize) -> f64 {
        let map = &self.sequence.tempo_map;
        let secs = frames as f64 / self.sample_rate as f64;
        let mut p = if map.is_constant() {
            position
                + self
                    .tempo
                    .ticks_for_samples(frames as f64, self.sample_rate as f64)
        } else {
            map.tick_at_seconds(map.seconds_at_tick(position) + secs)
        };
        let (start, end) = self.loop_bounds();
        let len = end - start;
        if self.loop_active() && len > 0.0 && p >= end {
            // Carry the time past the loop end into the loop start at the tempo there.
            let excess = map.seconds_at_tick(p) - map.seconds_at_tick(end);
            p = map.tick_at_seconds(map.seconds_at_tick(start) + excess);
            if p >= end {
                p = start + (p - end) % len;
            }
        }
        p
    }

    /// Fill `self.events` with everything that happens during the next `frames` samples.
    fn collect_events(&mut self, frames: usize) {
        self.events.clear();
        if !self.playing {
            return;
        }
        let sr = self.sample_rate as f64;
        let seq = Arc::clone(&self.sequence);
        let map = &seq.tempo_map;
        let base = self.late_start.min(frames);
        let avail = (frames - base) as f64 / sr;
        let (loop_start, loop_end) = self.loop_bounds();
        let start = self.position;
        let t0 = map.seconds_at_tick(start);
        let end = map.tick_at_seconds(t0 + avail);

        if std::mem::take(&mut self.resume_notes) {
            self.resume_notes_under(start);
        }

        if self.loop_active() && loop_end > loop_start && end >= loop_end && start < loop_end {
            // Two segments: [start, loop_end) then [loop_start, …), with a release at the seam.
            let seam_secs = map.seconds_at_tick(loop_end) - t0;
            let seam = base + (seam_secs * sr).round() as usize;
            self.collect_segment(start, loop_end, base);
            self.events.push(Event {
                offset: seam.min(frames),
                kind: EventKind::ReleaseAll,
            });
            let to =
                map.tick_at_seconds(map.seconds_at_tick(loop_start) + (avail - seam_secs).max(0.0));
            self.collect_segment(loop_start, to, seam);
        } else {
            self.collect_segment(start, end, base);
        }
        self.events.sort_by_key(|e| e.offset);
        self.connect_touching_notes();
    }

    /// Let Legato hear a note that starts exactly where the previous one ends. Each note pushes
    /// its own on and off, so at a shared offset the earlier note's off would land first and
    /// the next note would find no key held and replay its attack. On a Legato track, each
    /// note-off in a run of note events at one offset is moved after the note-ons there —
    /// but never past a note-on of its own pitch: a repeated note has to end before it starts
    /// again, or the old note's off would cut the new one short.
    ///
    /// Only built-in synths in Legato are touched, so Poly, Mono, drums and plugins keep the
    /// order they were scheduled in. In place and allocation-free: a run is a handful of
    /// events, so shuffling it with rotations costs nothing.
    fn connect_touching_notes(&mut self) {
        let tracks = &self.sequence.tracks;
        let legato = |track: usize| {
            tracks.get(track).is_some_and(|t| {
                !t.is_drum && t.plugin.is_none() && t.synth.voice == VoiceMode::Legato
            })
        };
        if !(0..tracks.len()).any(legato) {
            return;
        }
        let is_note =
            |e: &Event| matches!(e.kind, EventKind::NoteOn { .. } | EventKind::NoteOff { .. });
        let events = &mut self.events;
        let mut start = 0;
        while start < events.len() {
            let offset = events[start].offset;
            let len = events[start..]
                .iter()
                .take_while(|e| e.offset == offset && is_note(e))
                .count();
            let run = &mut events[start..start + len];
            start += len.max(1);
            // Right to left, so every off to the right is already in place when one moves past it.
            for i in (0..run.len()).rev() {
                let EventKind::NoteOff { track, pitch, .. } = run[i].kind else {
                    continue;
                };
                if !legato(track) {
                    continue;
                }
                let stop = run[i + 1..]
                    .iter()
                    .position(|e| matches!(e.kind, EventKind::NoteOn { track: t, pitch: p, .. } if t == track && p == pitch))
                    .map_or(run.len(), |k| i + 1 + k);
                run[i..stop].rotate_left(1);
            }
        }
    }

    /// The live ropes on track `ti`, indexed like its notes, and how its instrument plays them;
    /// no ropes when they don't apply.
    ///
    /// The built-in synth glides a voice from note to note. A plugin is addressed by pitch, so
    /// it gets the overlap trick instead ([`RopePlay::Overlap`]). A drum hit has no voice to
    /// hand on, so drums (and an audio track's silent instrument) ignore ropes. That is checked
    /// against the channel's actual instrument rather than the track's settings, so a plugin
    /// still being swapped in is never sent a slide it can't play, and the synth that stands in
    /// for it never has a note-off held back that nothing will release. A table that no longer
    /// matches the notes is ignored too (see [`SequenceTrack::ropes`]).
    ///
    /// Liveness was decided on raw ticks when the sequence was built and is not re-checked on
    /// swung times. Swing is a strictly increasing warp of the tick line, so a source that
    /// starts before its target and ends at or after it still does once both are swung, and a
    /// rope that touches still touches.
    fn ropes_on<'s>(&self, ti: usize, track: &'s SequenceTrack) -> (RopePlay, &'s [NoteRope]) {
        let play = match self.channels.get(ti).map(|c| &c.instrument) {
            Some(Instrument::Synth(_)) => Some(RopePlay::Glide),
            Some(Instrument::Plugin(_)) => Some(RopePlay::Overlap),
            _ => None,
        };
        #[cfg(test)]
        let play = play.map(|p| {
            if self.pretend_plugin.contains(&ti) {
                RopePlay::Overlap
            } else {
                p
            }
        });
        match play {
            Some(play) if track.ropes.len() == track.notes.len() => (play, &track.ropes),
            _ => (RopePlay::Glide, &[]),
        }
    }

    /// Notes already sounding at `from` — the ones the playhead landed inside. Scheduling only
    /// fires a note at its start tick, so without this, seeking into (or starting playback in)
    /// the middle of a long note gives silence until the next note begins. The chord bed has
    /// always resumed this way; notes now match it.
    ///
    /// Resumed notes start plainly, at their own pitch: landing inside a slide note plays it
    /// as an ordinary note. A source whose slide note is still ahead resumes with its tag, so
    /// the slide still finds it. A source whose slide note has already started is not resumed
    /// at all: its voice would have been the slide note's by now, and its own end, which the
    /// scheduler holds back, would never release it.
    ///
    /// A plugin track resumes every note by its written length. A source that only touches its
    /// slide note has ended by the time the slide note starts, so it stays out of the slide
    /// note just as on a synth; its stretched note-off may still be ahead (the playhead landed
    /// in the overlap's few milliseconds) and then reaches the plugin for a key it isn't
    /// holding, which it ignores. A source that already overlaps is resumed, because a plugin
    /// really does hear it until its own end.
    ///
    /// A resumed note's bend is positioned where it would be, not restarted: seeking into the
    /// middle of a bend should land you in the middle of the bend.
    fn resume_notes_under(&mut self, from: f64) {
        let any_solo = self.sequence.any_solo();
        let seq = Arc::clone(&self.sequence);
        let swing = |tick: f64| swing_warp_ticks(tick, seq.swing, seq.swing_grid_ticks);
        for (ti, track) in seq.tracks.iter().enumerate() {
            // A drum hit is a one-shot: re-firing it mid-note would retrigger the sample.
            if !track.audible(any_solo) || track.is_drum {
                continue;
            }
            let ropes = match self.ropes_on(ti, track) {
                (RopePlay::Glide, ropes) => ropes,
                (RopePlay::Overlap, _) => &[],
            };
            for (ni, note) in track.notes.iter().enumerate() {
                if note.muted {
                    continue;
                }
                // A repeated note is several strikes, so resume the one the playhead is
                // actually standing inside rather than the note as a whole.
                let hits = sub_hits(note);
                let len = note.length_ticks as f64 / hits as f64;
                let start = note.shifted_start() as f64;
                for h in 0..hits {
                    let s = start + len * h as f64;
                    if s < from && (s + len).min(seq.length_ticks as f64) > from {
                        let rope = ropes.get(ni).copied().unwrap_or_default();
                        let handed = rope.to.is_some_and(|to| {
                            handed_over_hit(&track.notes, rope, note) == Some(h)
                                && swing(track.notes[to].shifted_start() as f64) < from
                        });
                        if !handed {
                            self.events.push(Event {
                                offset: 0,
                                kind: EventKind::NoteOn {
                                    track: ti,
                                    pitch: note.pitch.clamp(0, 127) as u8,
                                    velocity: note.velocity.clamp(1, 127) as u8,
                                    shape: NoteShape::from_note(note).bent(note_bend(
                                        note,
                                        &seq.tempo_map,
                                        s,
                                        from,
                                        self.sample_rate as f64,
                                    )),
                                    tag: VoiceTag::for_hit(ni, h),
                                    slide: None,
                                },
                            });
                        }
                        break;
                    }
                }
            }
        }
    }

    fn collect_segment(&mut self, from: f64, to: f64, base_offset: usize) {
        let any_solo = self.sequence.any_solo();
        let seq = Arc::clone(&self.sequence);
        let sr = self.sample_rate as f64;
        let t_from = seq.tempo_map.seconds_at_tick(from);
        let offset_of = |tick: f64| {
            base_offset
                + ((seq.tempo_map.seconds_at_tick(tick) - t_from) * sr)
                    .round()
                    .max(0.0) as usize
        };
        let swing = |tick: f64| swing_warp_ticks(tick, seq.swing, seq.swing_grid_ticks);

        if self.metronome {
            // One click per beat, accented on the first click of each bar.
            let bar = seq.bar_ticks.max(1);
            let mut k = (from / TICKS_PER_BEAT as f64).ceil() as i64;
            while ((k * TICKS_PER_BEAT) as f64) < to {
                let tick = k * TICKS_PER_BEAT;
                self.events.push(Event {
                    offset: offset_of(tick as f64),
                    kind: EventKind::Click {
                        accent: accents_bar(tick, bar),
                    },
                });
                k += 1;
            }
        }

        for (ti, track) in seq.tracks.iter().enumerate() {
            if !track.audible(any_solo) {
                continue;
            }
            let (play, ropes) = self.ropes_on(ti, track);
            for (ni, note) in track.notes.iter().enumerate() {
                if note.muted {
                    continue;
                }
                let rope = ropes.get(ni).copied().unwrap_or_default();
                let handed = handed_over_hit(&track.notes, rope, note);
                let pitch = note.pitch.clamp(0, 127) as u8;
                let velocity = note.velocity.clamp(1, 127) as u8;
                let shape = NoteShape::from_note(note);
                let bends = !note.pitch_envelope.is_empty();
                // `shift_ticks` moves only when the note *sounds*: its grid position stays its
                // identity, so a step nudged early still belongs to the cell it is drawn in.
                let start = note.shifted_start() as f64;
                let hits = sub_hits(note);
                let len = note.length_ticks as f64 / hits as f64;
                for h in 0..hits {
                    let s = swing(start + len * h as f64);
                    let e = swing(start + len * (h + 1) as f64).min(seq.length_ticks as f64);
                    let tag = VoiceTag::for_hit(ni, h);
                    if s >= from && s < to {
                        self.events.push(Event {
                            offset: offset_of(s),
                            kind: EventKind::NoteOn {
                                track: ti,
                                pitch,
                                velocity,
                                // Each repeat is its own strike, so each one bends from its
                                // own start; the envelope is written against one note's
                                // length and the repeats share it.
                                shape: if bends {
                                    shape.bent(note_bend(note, &seq.tempo_map, s, s, sr))
                                } else {
                                    shape
                                },
                                tag,
                                // Only the first hit slides; the rest retrigger as written.
                                slide: if h == 0 && play == RopePlay::Glide {
                                    rope_slide(&track.notes, rope, note)
                                } else {
                                    None
                                },
                            },
                        });
                    }
                    // A hit whose voice a rope hands on is released by the slide note instead
                    // on a synth, and held into the slide note on a plugin.
                    let e = match (rope.to, play) {
                        (Some(_), RopePlay::Glide) if handed == Some(h) => continue,
                        (Some(target), RopePlay::Overlap) if handed == Some(h) => {
                            match overlap_end(
                                &track.notes,
                                ni,
                                target,
                                e,
                                swing,
                                &seq.tempo_map,
                                seq.length_ticks as f64,
                            ) {
                                // The note-off lands at `e` or `late`, never between, so it
                                // belongs to this window only if `[e, late]` meets it. Only
                                // then is it worth scanning the track for a note it would cut,
                                // and the answer is the same in whichever window asks: the off
                                // is scheduled exactly once, wherever the block boundaries or
                                // the loop seam fall. One past the seam is never scheduled,
                                // and the seam's release ends the note there instead.
                                Some(late)
                                    if late >= from
                                        && e < to
                                        && key_starts_between(&track.notes, ni, e, late, swing) =>
                                {
                                    e
                                }
                                Some(late) => late,
                                None => e,
                            }
                        }
                        _ => e,
                    };
                    // A drum hit is a one-shot: it rings out on its own and has nothing to
                    // release.
                    if e > s && e >= from && e < to && !track.is_drum {
                        self.events.push(Event {
                            offset: offset_of(e),
                            kind: EventKind::NoteOff {
                                track: ti,
                                pitch,
                                tag,
                            },
                        });
                    }
                }
            }
        }

        if self.hear_chords {
            for chord in seq.chords.chords() {
                let s = chord.start_tick as f64;
                let e = (chord.end_tick() as f64).min(seq.length_ticks as f64);
                if s >= from && s < to {
                    for &pc in &chord.pitch_classes {
                        self.events.push(Event {
                            offset: offset_of(s),
                            kind: EventKind::ChordOn {
                                pitch: (60 + pc.rem_euclid(12)) as u8,
                            },
                        });
                    }
                }
                if e > s && e >= from && e < to {
                    for &pc in &chord.pitch_classes {
                        self.events.push(Event {
                            offset: offset_of(e),
                            kind: EventKind::ChordOff {
                                pitch: (60 + pc.rem_euclid(12)) as u8,
                            },
                        });
                    }
                }
            }
            // Seeking into the middle of a chord: start it now.
            if let Some(chord) = seq.chords.chord_at(from.floor() as i64) {
                if self.sounding_chord != Some(chord.id) && (chord.start_tick as f64) < from {
                    for &pc in &chord.pitch_classes {
                        self.events.push(Event {
                            offset: base_offset,
                            kind: EventKind::ChordOn {
                                pitch: (60 + pc.rem_euclid(12)) as u8,
                            },
                        });
                    }
                }
                self.sounding_chord = Some(chord.id);
            } else {
                self.sounding_chord = None;
            }
        }
    }

    fn tick_auditions(&mut self, frames: usize) {
        let mut expired: Vec<(Uuid, u8)> = Vec::new();
        for a in &mut self.auditions {
            if a.samples_left <= frames {
                expired.push((a.track_id, a.pitch));
                a.samples_left = 0;
            } else {
                a.samples_left -= frames;
            }
        }
        if !expired.is_empty() {
            for (track_id, pitch) in expired {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_off(pitch);
                }
            }
            self.auditions.retain(|a| a.samples_left > 0);
        }
    }

    /// Convenience for tests and offline rendering: build a lookup from track id to index.
    pub fn track_index(&self) -> HashMap<Uuid, usize> {
        self.channel_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissonant_core::{NoteEvent, ProjectModel, TimeSignature};

    const SR: f32 = 44_100.0;
    /// Ticks per beat, short enough to keep the musical intent of each test readable.
    const B: i64 = TICKS_PER_BEAT;

    /// Ticks as beats, for assertions whose tolerance is musical rather than sample-sized.
    fn in_beats(ticks: f64) -> f64 {
        ticks / B as f64
    }

    fn sequence_with_note(start: i64, len_ticks: i64, loop_len: i64) -> Arc<Sequence> {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = loop_len;
        model.patterns[0].notes = vec![NoteEvent::new(start, len_ticks, 69)];
        Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap())
    }

    fn render(engine: &mut Engine, total_frames: usize, block: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(total_frames);
        let mut buf = vec![0.0f32; block * 2];
        let mut done = 0;
        while done < total_frames {
            let n = (total_frames - done).min(block);
            engine.process(&mut buf[..n * 2]);
            out.extend(buf[..n * 2].iter().step_by(2));
            done += n;
        }
        out
    }

    fn render_stereo(
        engine: &mut Engine,
        total_frames: usize,
        block: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (
            Vec::with_capacity(total_frames),
            Vec::with_capacity(total_frames),
        );
        let mut buf = vec![0.0f32; block * 2];
        let mut done = 0;
        while done < total_frames {
            let n = (total_frames - done).min(block);
            engine.process(&mut buf[..n * 2]);
            left.extend(buf[..n * 2].iter().step_by(2));
            right.extend(buf[1..n * 2].iter().step_by(2));
            done += n;
        }
        (left, right)
    }

    fn rms(xs: &[f32]) -> f32 {
        (xs.iter().map(|x| x * x).sum::<f32>() / xs.len() as f32).sqrt()
    }

    fn first_onset(samples: &[f32]) -> Option<usize> {
        samples.iter().position(|s| s.abs() > 1e-6)
    }

    /// `first_onset` against the transport rather than against the buffer: the master limiter's
    /// lookahead delays the music by a fixed, reported amount, so scheduling stays sample-exact
    /// once that latency is taken off.
    fn onset_tick_sample(e: &Engine, samples: &[f32]) -> i64 {
        first_onset(samples).expect("nothing sounded") as i64 - e.output_latency_samples() as i64
    }

    #[test]
    fn note_starts_on_the_exact_sample_regardless_of_block_size() {
        for block in [64usize, 480, 1024, 4096] {
            let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
            e.handle(EngineCommand::SetSequence(sequence_with_note(B, B, 16 * B)));
            e.handle(EngineCommand::Play);
            let left = render(&mut e, 30_000, block);
            let onset = onset_tick_sample(&e, &left);
            // 120 bpm → beat 1 = 22050 samples; the first non-zero sample is one after onset.
            assert!((onset - 22_050).abs() <= 2, "block {block}: onset {onset}");
        }
    }

    #[test]
    fn note_releases_and_loop_retriggers() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(
            0,
            B / 2,
            2 * B,
        )));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 44_100 * 2, 512);
        // One loop = 2 beats = 44100 samples. The second onset must land at the loop start.
        let second_half = &left[44_100 - 10..];
        let onset = onset_tick_sample(&e, second_half) + 44_100 - 10;
        assert!((onset - 44_100).abs() <= 2, "second onset {onset}");
        // Between the release tail (0.5 beat = 11025 samples + 0.2 s release) and the loop, silence.
        let quiet = &left[11_025 + 10_000..44_000];
        assert!(quiet.iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn stopped_engine_is_silent_and_position_holds() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(0, B, 4 * B)));
        let left = render(&mut e, 10_000, 256);
        assert!(left.iter().all(|s| *s == 0.0));
        assert_eq!(e.position(), 0.0);
    }

    #[test]
    fn mute_and_solo_are_honored() {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        model.tracks[0].muted = true;
        let pid = model.patterns[0].id;
        model.patterns[0].notes = vec![NoteEvent::new(0, B, 69)];
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::Play);
        assert!(render(&mut e, 8_000, 256).iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn drums_and_chord_bed_make_sound() {
        let mut model = ProjectModel::starter();
        let pid = model.patterns[1].id; // the drum pattern
        model.patterns[1].notes = vec![NoteEvent::new(0, B / 2, 36)];
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::SetHearChords(true));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 20_000, 512);
        assert!(left.iter().any(|s| s.abs() > 0.05));
        assert!(shared.position_ticks() > 0.0);
        assert!(shared.is_playing());
        assert!(shared.master_peak().0 > 0.0);
    }

    /// The starter has to make music on the first press of space, chord bed or not: in the
    /// song (where a new project opens) and in each of its patterns on its own.
    #[test]
    fn starter_makes_sound_without_the_chord_bed() {
        let model = ProjectModel::starter();
        let mut sequences = vec![Sequence::from_song(&model)];
        sequences.extend(
            model
                .patterns
                .iter()
                .map(|p| Sequence::from_pattern(&model, &p.id, 0).unwrap()),
        );
        for seq in sequences {
            let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
            e.handle(EngineCommand::SetSequence(Arc::new(seq)));
            e.handle(EngineCommand::Play);
            assert!(render(&mut e, 20_000, 512).iter().any(|s| s.abs() > 0.05));
        }
    }

    /// The bed is a live monitor in song mode too: an empty song with chords is silent until
    /// it is switched on.
    #[test]
    fn the_chord_bed_monitors_the_song_when_switched_on() {
        let mut model = ProjectModel::starter();
        model.clips.clear();
        for on in [false, true] {
            let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
            e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(
                &model,
            ))));
            e.handle(EngineCommand::SetHearChords(on));
            e.handle(EngineCommand::Play);
            let loud = render(&mut e, 20_000, 512).iter().any(|s| s.abs() > 0.01);
            assert_eq!(loud, on, "hear chords {on}");
        }
    }

    #[test]
    fn audition_plays_then_stops_by_itself() {
        let model = ProjectModel::empty();
        let (tid, pid) = (model.tracks[0].id, model.patterns[0].id);
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::Audition {
            track_id: tid,
            pitch: 60,
            velocity: 100,
            seconds: 0.8,
        });
        let left = render(&mut e, 44_100 * 2, 512);
        assert!(left[..4000].iter().any(|s| s.abs() > 0.01));
        assert!(left[70_000..].iter().all(|s| s.abs() < 1e-4));
    }

    /// A preview must not outlast the note being drawn: at a 1/16 grid (0.125 s at 120 bpm) the
    /// voice has to be releasing long before the fixed 0.8 s hold this used to apply, otherwise
    /// painting a fast run stacks overlapping voices into a wash that sounds like reverb.
    #[test]
    fn audition_honors_the_requested_length() {
        let model = ProjectModel::empty();
        let (tid, pid) = (model.tracks[0].id, model.patterns[0].id);
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::Audition {
            track_id: tid,
            pitch: 60,
            velocity: 100,
            seconds: 0.125,
        });
        let left = render(&mut e, SR as usize, 512);
        let sr = SR as usize;
        let loud =
            |from: usize, to: usize| left[from..to].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(loud(0, sr / 20) > 0.01, "the preview must speak");
        // Held 0.125 s, then the voice's release (~0.14 s) runs out: silent well before 0.5 s.
        assert!(
            loud(sr / 2, sr) < 1e-4,
            "preview outlived the note it previewed"
        );
    }

    /// A grid finer than the floor still has to be audible rather than clicking off instantly.
    #[test]
    fn audition_length_is_clamped_to_a_floor() {
        let model = ProjectModel::empty();
        let (tid, pid) = (model.tracks[0].id, model.patterns[0].id);
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::Audition {
            track_id: tid,
            pitch: 60,
            velocity: 100,
            seconds: 0.0,
        });
        let left = render(&mut e, SR as usize, 512);
        let at_floor = (SR * MIN_AUDITION_SECS * 0.8) as usize;
        assert!(
            left[..at_floor].iter().any(|s| s.abs() > 0.01),
            "a sub-floor preview must still speak"
        );
    }

    #[test]
    fn tempo_map_moves_notes_and_the_playhead() {
        use dissonant_core::{Clip, TempoPoint};
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = 16 * B;
        model.patterns[0].notes = vec![NoteEvent::new(8 * B, B, 69)];
        model.clips = vec![Clip::new(pid, 0, 16 * B)];
        // 120 for 4 beats (2 s), then 60: beat 8 is reached at 2 + 4 = 6 s.
        model.tempo_points = vec![TempoPoint::new(4 * B, 60.0, false)];
        let seq = Arc::new(Sequence::from_song(&model));
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(seq));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 44_100 * 6 + 2_000, 1000);
        let onset = onset_tick_sample(&e, &left);
        assert!((onset - 44_100 * 6).abs() <= 2, "onset {onset}");
        // The playhead: 6 s + 2000 samples at 60 bpm ≈ beat 8.045.
        let expected = 8.0 + (2_000.0 / 44_100.0) * (60.0 / 60.0);
        assert!(
            (in_beats(e.position()) - expected).abs() < 1e-6,
            "{}",
            e.position()
        );
    }

    #[test]
    fn muted_notes_are_skipped() {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        let mut n = NoteEvent::new(0, B, 69);
        n.muted = true;
        model.patterns[0].notes = vec![n];
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap());
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(seq));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 20_000, 512);
        assert!(left.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn swing_delays_the_offbeat_only() {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = 4 * B;
        model.patterns[0].notes = vec![NoteEvent::new(B / 2, B / 4, 69)];
        model.swing = 200.0 / 3.0; // triplet feel: the "and" of 1 lands at 2/3 beat
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap());
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(seq));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 30_000, 512);
        let onset = onset_tick_sample(&e, &left);
        let expected = (22_050.0 * 2.0 / 3.0) as i64;
        assert!((onset - expected).abs() <= 2, "onset {onset} vs {expected}");
    }

    #[test]
    fn metronome_clicks_on_beats_and_count_in_holds_the_transport() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(B, B, 16 * B)));
        e.handle(EngineCommand::SetMetronome {
            on: true,
            volume: 1.0,
        });
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 22_050 + 100, 256);
        // A click at beat 0 (sample 0) and another at beat 1 (sample 22050).
        assert!(left[..50].iter().any(|s| s.abs() > 1e-4), "click on beat 0");
        assert!(
            left[2_500..21_000].iter().all(|s| s.abs() < 1e-4),
            "silence between clicks (note starts at beat 1)"
        );
        assert!(
            left[22_050..22_100].iter().any(|s| s.abs() > 1e-4),
            "click on beat 1"
        );

        // Count-in: one bar (4 beats = 88200 samples) of clicks, then the transport starts.
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(0, B, 16 * B)));
        e.handle(EngineCommand::SetMetronome {
            on: false,
            volume: 1.0,
        });
        e.handle(EngineCommand::PlayWithCountIn { bars: 1 });
        assert!(!e.is_playing());
        let half = render(&mut e, 44_100, 1000);
        assert!(
            shared.count_in_beats() > 1.9 && shared.count_in_beats() < 2.1,
            "{}",
            shared.count_in_beats()
        );
        assert_eq!(e.position(), 0.0, "transport held during count-in");
        assert!(
            half[..50].iter().any(|s| s.abs() > 1e-4),
            "count-in clicks even with the metronome off"
        );
        let rest = render(&mut e, 44_100 + 2_000, 1000);
        assert!(e.is_playing());
        assert_eq!(shared.count_in_beats(), 0.0);
        // The note at beat 0 starts exactly when the count-in ends (sample 88200 overall).
        let onset = onset_tick_sample(&e, &rest[44_100 - 10..]) + 44_100 - 10;
        assert!((onset - 44_100).abs() <= 2, "onset {onset}");
        // Position advanced only for the part of the block after the start.
        assert!(
            (in_beats(e.position()) - 2_000.0 / 22_050.0).abs() < 1e-6,
            "{}",
            e.position()
        );
    }

    /// The peak the metronome reaches in the first 2000 samples of each beat. An accented click
    /// is sent at 0.5 and a plain one at 0.32, both before any envelope, so the two are told
    /// apart by level rather than by pitch.
    fn beat_click_peaks(numerator: u32, denominator: u32, beats: usize) -> Vec<f32> {
        let mut model = ProjectModel::empty();
        model.time_signature = TimeSignature {
            numerator,
            denominator,
        };
        let pid = model.patterns[0].id;
        // Long enough that nothing loops back inside the window being measured.
        model.patterns[0].length_ticks = (beats as i64 + 2) * B;
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::SetMetronome {
            on: true,
            volume: 1.0,
        });
        e.handle(EngineCommand::Play);
        // 120 bpm at 44.1 kHz: one beat is 22050 samples.
        let spb = 22_050usize;
        let left = render(&mut e, beats * spb + 2_000, 256);
        (0..beats)
            .map(|k| {
                left[k * spb..k * spb + 2_000]
                    .iter()
                    .fold(0.0f32, |p, s| p.max(s.abs()))
            })
            .collect()
    }

    #[test]
    fn the_metronome_accents_the_first_beat_of_every_bar() {
        // 7/8 is 3.5 beats — 3360 ticks against a 960-tick beat — so only every other bar line
        // falls on a beat. The accent has to land on the first click of each bar regardless:
        // bars start at beats 0, 3.5, 7, 10.5, 14, so the clicks that open them are 0, 4, 7, 11.
        let peaks = beat_click_peaks(7, 8, 14);
        let accented: Vec<usize> = peaks
            .iter()
            .enumerate()
            .filter(|(_, p)| **p > 0.4)
            .map(|(k, _)| k)
            .collect();
        assert_eq!(accented, vec![0, 4, 7, 11], "7/8 accents: {peaks:?}");
        assert!(
            peaks.iter().all(|p| *p > 0.25),
            "every beat still clicks: {peaks:?}"
        );

        // 4/4 is the case that always worked, and must keep working.
        let peaks = beat_click_peaks(4, 4, 12);
        let accented: Vec<usize> = peaks
            .iter()
            .enumerate()
            .filter(|(_, p)| **p > 0.4)
            .map(|(k, _)| k)
            .collect();
        assert_eq!(accented, vec![0, 4, 8], "4/4 accents: {peaks:?}");
    }

    #[test]
    fn loop_region_repeats_only_its_range() {
        // Notes at beat 0 and beat 4 in a 16-beat pattern; loop [4, 8) → only the beat-4 note
        // sounds, once per 4 beats.
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].notes = vec![
            NoteEvent::new(0, B / 2, 69),
            NoteEvent::new(4 * B, B / 2, 69),
        ];
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::SetLoop {
            start: (4 * B) as f64,
            end: (8 * B) as f64,
        });
        e.handle(EngineCommand::Seek { tick: 0.0 });
        assert_eq!(e.position(), 0.0, "seeking before the loop is allowed");
        e.handle(EngineCommand::Seek {
            tick: (4 * B) as f64,
        });
        e.handle(EngineCommand::Play);
        // 4 beats = 88200 samples per loop pass; render two passes.
        let left = render(&mut e, 88_200 * 2 + 100, 512);
        let first = onset_tick_sample(&e, &left);
        assert!(first <= 2, "first onset {first}");
        let second = onset_tick_sample(&e, &left[88_200 - 10..]) + 88_200 - 10;
        assert!((second - 88_200).abs() <= 2, "second onset {second}");
        assert!(e.position() >= (4 * B) as f64 && e.position() < (8 * B) as f64);
    }

    #[test]
    fn held_note_sounds_until_released() {
        let model = ProjectModel::empty();
        let (tid, pid) = (model.tracks[0].id, model.patterns[0].id);
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::NoteOn {
            track_id: tid,
            pitch: 60,
            velocity: 100,
        });
        let held = render(&mut e, 44_100 * 2, 512);
        assert!(
            held[80_000..].iter().any(|s| s.abs() > 0.01),
            "a held note must not auto-release"
        );
        e.handle(EngineCommand::NoteOff {
            track_id: tid,
            pitch: 60,
        });
        let tail = render(&mut e, 44_100, 512);
        assert!(tail[30_000..].iter().all(|s| s.abs() < 1e-4));
    }

    #[test]
    fn seek_wraps_into_loop_and_sequence_swap_keeps_channels() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        let seq = sequence_with_note(0, B, 4 * B);
        e.handle(EngineCommand::SetSequence(seq.clone()));
        e.handle(EngineCommand::Seek {
            tick: (9 * B) as f64,
        });
        assert_eq!(e.position(), 0.0);
        e.handle(EngineCommand::Seek {
            tick: 2.5 * B as f64,
        });
        assert_eq!(e.position(), 2.5 * B as f64);
        e.handle(EngineCommand::SetSequence(seq.clone()));
        assert_eq!(e.track_index().len(), 1);
    }

    /// The offsets of every note-on the scheduler lines up for the next `frames` samples.
    fn note_on_offsets(engine: &mut Engine, frames: usize) -> Vec<usize> {
        engine.collect_events(frames);
        engine
            .events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::NoteOn { .. }))
            .map(|e| e.offset)
            .collect()
    }

    fn sequence_with(note: NoteEvent, loop_len: i64) -> Arc<Sequence> {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = loop_len;
        model.patterns[0].notes = vec![note];
        Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap())
    }

    fn playing_engine(note: NoteEvent, loop_len: i64) -> Engine {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with(note, loop_len)));
        e.handle(EngineCommand::Play);
        e
    }

    /// `shift_ticks` moves when a note sounds without moving where it lives, so a step nudged
    /// still belongs to the cell it is drawn in — and a note at the very start cannot be
    /// dragged before the transport.
    #[test]
    fn shift_moves_a_note_off_its_own_grid_position() {
        // 120 bpm: one beat is 22050 samples, so a quarter-beat shift is 5512.5, rounded.
        let mut early = NoteEvent::new(B, B / 2, 69);
        early.shift_ticks = -B / 4;
        assert_eq!(
            note_on_offsets(&mut playing_engine(early, 8 * B), 44_100),
            vec![16_538]
        );

        let mut late = NoteEvent::new(B, B / 2, 69);
        late.shift_ticks = B / 4;
        assert_eq!(
            note_on_offsets(&mut playing_engine(late, 8 * B), 44_100),
            vec![27_563]
        );

        // The note itself never moved: shifting it did not renumber its grid position.
        let mut plain = NoteEvent::new(B, B / 2, 69);
        assert_eq!(
            note_on_offsets(&mut playing_engine(plain.clone(), 8 * B), 44_100),
            vec![22_050]
        );
        plain.shift_ticks = -B / 4;
        assert_eq!(plain.start_tick, B);

        // Negative shift at the very start clamps to tick 0 rather than going missing.
        let mut clamped = NoteEvent::new(0, B / 2, 69);
        clamped.shift_ticks = -2 * B;
        assert_eq!(
            note_on_offsets(&mut playing_engine(clamped, 8 * B), 44_100),
            vec![0]
        );

        // And it is audible there: an onset at the top of the block, not silence.
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        let mut n = NoteEvent::new(B, B / 2, 69);
        n.shift_ticks = -B / 4;
        e.handle(EngineCommand::SetSequence(sequence_with(n, 8 * B)));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 30_000, 512);
        let onset = onset_tick_sample(&e, &left);
        assert!((onset - 16_538).abs() <= 2, "onset {onset}");
    }

    /// Every note on and off the scheduler lines up for the next `frames` samples, in the
    /// order they will fire: `(offset, on, pitch)`.
    fn note_events(engine: &mut Engine, frames: usize) -> Vec<(usize, bool, u8)> {
        engine.collect_events(frames);
        engine
            .events
            .iter()
            .filter_map(|e| match e.kind {
                EventKind::NoteOn { pitch, .. } => Some((e.offset, true, pitch)),
                EventKind::NoteOff { pitch, .. } => Some((e.offset, false, pitch)),
                _ => None,
            })
            .collect()
    }

    /// An engine playing `notes` on one track whose sound is `voice`.
    fn playing_notes(voice: &str, notes: Vec<NoteEvent>) -> Engine {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = voice.into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = 8 * B;
        model.patterns[0].notes = notes;
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap());
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(seq));
        e.handle(EngineCommand::Play);
        e
    }

    /// In Legato a note that starts where the last one ends connects to it, so its on has to
    /// reach the synth while the last one is still held.
    #[test]
    fn touching_notes_on_a_legato_track_start_before_the_last_one_ends() {
        let touching = || vec![NoteEvent::new(0, B, 57), NoteEvent::new(B, B, 60)];
        // `sub-bass` is a factory Legato sound.
        let legato = note_events(&mut playing_notes("sub-bass", touching()), 2 * SR as usize);
        assert_eq!(
            legato,
            vec![
                (0, true, 57),
                (22_050, true, 60),
                (22_050, false, 57),
                (44_100, false, 60)
            ]
        );
        // Poly keeps the order notes were scheduled in: the end of one, then the next.
        let poly = note_events(&mut playing_notes("sine", touching()), 2 * SR as usize);
        assert_eq!(
            poly,
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 60),
                (44_100, false, 60)
            ]
        );
    }

    /// A repeated pitch must end before it starts again, or the old note's off would release
    /// the voice the new note just took over.
    #[test]
    fn touching_notes_of_one_pitch_still_end_before_they_restart() {
        let notes = vec![
            NoteEvent::new(0, B, 57),
            NoteEvent::new(B, B, 57),
            NoteEvent::new(B, B, 64),
        ];
        let events = note_events(&mut playing_notes("sub-bass", notes), 2 * SR as usize);
        assert_eq!(
            events[..4],
            [
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 57),
                (22_050, true, 64)
            ]
        );
        // A note's own repeats are the same pitch touching itself: untouched.
        let mut rep = NoteEvent::new(0, 2 * B, 57);
        rep.repeats = 1;
        let events = note_events(&mut playing_notes("sub-bass", vec![rep]), 2 * SR as usize);
        assert_eq!(
            events,
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 57),
                (44_100, false, 57)
            ]
        );
    }

    /// End to end: a Legato bass playing two touching notes attacks once, where Mono attacks
    /// twice. Measured as the loudest moment just after the join against the level the first
    /// note settled at.
    #[test]
    fn legato_connects_touching_notes_in_playback() {
        let join = |voice: VoiceMode| {
            let mut model = ProjectModel::empty();
            let mut patch = match factory_sound("sine").map(|s| &s.patch) {
                Some(SoundPatch::Synth(p)) => *p,
                _ => unreachable!(),
            };
            patch.voice = voice;
            patch.amp_env.attack = 0.02;
            patch.amp_env.decay = 0.02;
            patch.amp_env.sustain = 0.3;
            model.sounds.push(dissonant_core::Sound {
                id: "u-test".into(),
                name: "test".into(),
                category: "bass".into(),
                patch: SoundPatch::Synth(patch),
            });
            model.tracks[0].voice = "u-test".into();
            let pid = model.patterns[0].id;
            model.patterns[0].length_ticks = 8 * B;
            model.patterns[0].notes = vec![NoteEvent::new(0, B, 57), NoteEvent::new(B, B, 60)];
            let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap());
            let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
            e.handle(EngineCommand::SetSequence(seq));
            e.handle(EngineCommand::Play);
            let left = render(&mut e, 30_000, 512);
            let peak = |xs: &[f32]| xs.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            peak(&left[22_050..24_000]) / peak(&left[18_000..22_000])
        };
        let legato = join(VoiceMode::Legato);
        let mono = join(VoiceMode::Mono);
        assert!(legato < 1.3, "legato re-attacked at the join: {legato}");
        assert!(mono > 2.0, "mono should re-attack at the join: {mono}");
    }

    /// `repeats = r` sounds the note `r + 1` times, evenly spaced across its *own* length.
    #[test]
    fn repeats_retrigger_evenly_inside_the_note() {
        let mut note = NoteEvent::new(0, 2 * B, 69);
        note.repeats = 3;
        // 2 beats split four ways = half a beat = 11025 samples apart.
        assert_eq!(
            note_on_offsets(&mut playing_engine(note, 8 * B), 80_000),
            vec![0, 11_025, 22_050, 33_075]
        );

        // Zero repeats is one strike, and the note's length is untouched by the split.
        let plain = NoteEvent::new(0, 2 * B, 69);
        assert_eq!(
            note_on_offsets(&mut playing_engine(plain, 8 * B), 80_000),
            vec![0]
        );

        // Repeats ride the shift with the note.
        let mut shifted = NoteEvent::new(0, 2 * B, 69);
        shifted.repeats = 1;
        shifted.shift_ticks = B / 2;
        assert_eq!(
            note_on_offsets(&mut playing_engine(shifted, 8 * B), 80_000),
            vec![11_025, 33_075]
        );
    }

    /// Seeking into a repeated note resumes the sub-hit the playhead is standing inside, not
    /// the first one and not all four.
    #[test]
    fn seeking_inside_a_repeated_note_resumes_one_sub_hit() {
        let mut note = NoteEvent::new(0, 2 * B, 69);
        note.repeats = 3;
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with(note, 8 * B)));
        e.handle(EngineCommand::Seek {
            tick: (B + B / 4) as f64,
        });
        e.handle(EngineCommand::Play);
        // The third strike runs [1.0, 1.5): exactly one resumed note-on, at the block start,
        // and the next scheduled strike at beat 1.5 lands a quarter-beat later.
        assert_eq!(note_on_offsets(&mut e, 20_000), vec![0, 5_513]);
    }

    /// The whole chain: the scheduler reads the note's properties, the channel renders the
    /// voice in stereo and the master passes the two sides through independently.
    #[test]
    fn a_notes_pan_and_filter_survive_the_whole_chain() {
        // A saw on a track voiced at 2 kHz: something for Mod X to actually close down.
        let shaped = |pan: f32, mod_x: f32| {
            let mut model = ProjectModel::empty();
            model.tracks[0].voice = "saw".into();
            model.tracks[0].tone = 2_000.0;
            let pid = model.patterns[0].id;
            model.patterns[0].length_ticks = 8 * B;
            let mut note = NoteEvent::new(0, 2 * B, 69);
            note.pan = pan;
            note.mod_x = mod_x;
            model.patterns[0].notes = vec![note];
            let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
            e.handle(EngineCommand::SetSequence(Arc::new(
                Sequence::from_pattern(&model, &pid, 0).unwrap(),
            )));
            e.handle(EngineCommand::Play);
            render_stereo(&mut e, 20_000, 512)
        };

        let (l, r) = shaped(0.0, 0.5);
        assert!(rms(&l) > 0.001, "a hard-left note must reach the mix");
        assert!(
            rms(&r) < rms(&l) * 1e-5,
            "hard left bled into the right bus: {}",
            rms(&r)
        );

        let (l, r) = shaped(0.5, 0.5);
        assert_eq!(l, r, "a centred note must land identically on both buses");

        // Mod X reaches the voice through the channel, which is also what tells it the track's
        // own tone to sweep around.
        let bright = rms(&shaped(0.5, 0.5).0);
        let dark = rms(&shaped(0.5, 0.0).0);
        assert!(
            dark < bright * 0.5,
            "mod_x 0 was not darker through the mix: {dark} vs {bright}"
        );
    }

    /// Swing is applied to an integer tick, so the same musical position has to resolve to
    /// the same sample every pass — no drift between the first turn of the loop and the next.
    #[test]
    fn a_swung_sixteenth_lands_on_the_same_sample_every_pass() {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = 4 * B;
        // The second sixteenth of beat 1: the note swing actually moves.
        model.patterns[0].notes = vec![NoteEvent::new(B / 4, B / 4, 69)];
        model.swing = 75.0; // hard shuffle
        model.swing_grid_ticks = B / 4;
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0).unwrap());

        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::clone(&seq)));
        e.handle(EngineCommand::Play);
        let first = note_on_offsets(&mut e, 44_100);
        // Tick 240 warps to 360 = 0.375 beat = 8268.75 samples at 120 bpm.
        assert_eq!(first, vec![8_269]);
        e.handle(EngineCommand::Seek { tick: 0.0 });
        assert_eq!(
            note_on_offsets(&mut e, 44_100),
            first,
            "the same pass must schedule identically"
        );

        let mut other = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        other.handle(EngineCommand::SetSequence(seq));
        other.handle(EngineCommand::Play);
        assert_eq!(
            note_on_offsets(&mut other, 44_100),
            first,
            "a fresh engine must agree"
        );
    }

    /// 960 PPQ divides by three, so eighth-note triplets are exact ticks (320) and land on
    /// exact samples — the thing a beat-float time base could never quite do.
    #[test]
    fn a_triplet_grid_position_is_sample_exact() {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = 4 * B;
        assert_eq!(B % 3, 0, "a triplet has to be an integer number of ticks");
        let triplets: Vec<NoteEvent> = (0..3)
            .map(|i| NoteEvent::new(i * (B / 3), B / 3, 69))
            .collect();
        model.patterns[0].notes = triplets;
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::Play);
        // 22050 samples per beat: thirds of it are 7350 samples apart, with no remainder.
        assert_eq!(note_on_offsets(&mut e, 44_100), vec![0, 7_350, 14_700]);
    }

    fn factory_patch(id: &str) -> SoundPatch {
        factory_sound(id).unwrap().patch.clone()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn a_preview_synth_note_sounds_with_the_transport_stopped() {
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(0, B, 4 * B)));
        assert!(
            shared.preview_scope().iter().all(|s| *s == 0.0),
            "nothing previewed yet"
        );
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(
            &factory_patch("saw"),
            SR,
        ))));
        e.handle(EngineCommand::PreviewNoteOn {
            pitch: 57,
            velocity: 100,
        });
        let held = render(&mut e, 8_000, 512);
        assert!(
            !e.is_playing() && e.position() == 0.0,
            "the transport stayed put"
        );
        assert!(peak(&held[4_000..]) > 0.05, "the preview note must sound");

        // The scope shows it, opening on a rising zero crossing.
        let scope = shared.preview_scope();
        assert_eq!(scope.len(), SCOPE_SAMPLES);
        assert!(peak(&scope) > 0.05);
        assert!(scope[0] >= 0.0, "{}", scope[0]);

        // A new patch reaches the held note without cutting it, and Stop leaves it alone.
        let SoundPatch::Synth(mut p) = factory_patch("saw") else {
            unreachable!()
        };
        p.osc[0].wave = dissonant_core::sound::OscWave::Square;
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::Synth(
            Box::new(p),
        ))));
        e.handle(EngineCommand::Stop);
        let after = render(&mut e, 2_000, 512);
        assert!(
            peak(&after[..200]) > 0.05,
            "updating the patch cut the held note"
        );

        e.handle(EngineCommand::PreviewNoteOff { pitch: 57 });
        let tail = render(&mut e, SR as usize, 512);
        assert!(peak(&tail[30_000..]) < 1e-4, "released");
    }

    #[test]
    fn the_preview_ignores_mute_and_closing_it_rings_out() {
        let mut model = ProjectModel::empty();
        model.tracks[0].muted = true;
        let pid = model.patterns[0].id;
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(
            Sequence::from_pattern(&model, &pid, 0).unwrap(),
        )));
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(
            &factory_patch("sine"),
            SR,
        ))));
        e.handle(EngineCommand::PreviewNoteOn {
            pitch: 69,
            velocity: 100,
        });
        assert!(
            peak(&render(&mut e, 4_000, 512)) > 0.05,
            "muting a track must not mute the preview"
        );
        e.handle(EngineCommand::SetPreview(None));
        let tail = render(&mut e, 2_000, 512);
        assert!(
            peak(&tail[..100]) > 0.01,
            "closing releases rather than cuts"
        );
        render(&mut e, SR as usize, 512);
        assert!(e.preview.is_none(), "dropped once silent");
        e.handle(EngineCommand::PreviewNoteOn {
            pitch: 69,
            velocity: 100,
        });
        assert!(e.preview.is_none(), "a closed preview plays nothing");
    }

    #[test]
    fn a_drum_preview_plays_on_any_pitch_and_swaps_its_sound() {
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(
            &factory_patch("kick"),
            SR,
        ))));
        e.handle(EngineCommand::PreviewNoteOn {
            pitch: 60,
            velocity: 127,
        });
        assert!(peak(&render(&mut e, 4_000, 512)) > 0.1);
        assert!(peak(&shared.preview_scope()) > 0.05);
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(
            &factory_patch("hat"),
            SR,
        ))));
        let Some(Instrument::Drums(kit)) = &e.preview else {
            panic!("still a drum preview")
        };
        assert_eq!(
            kit.pad_drum(0).unwrap().samples.len(),
            (0.06 * SR).round() as usize,
            "now the hat"
        );
        // A drum rendered at another rate is re-rendered at the engine's.
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(
            &factory_patch("hat"),
            22_050.0,
        ))));
        let Some(Instrument::Drums(kit)) = &e.preview else {
            panic!()
        };
        assert_eq!(kit.pad_drum(0).unwrap().sample_rate, SR);
    }

    #[test]
    fn a_prerendered_bank_feeds_the_kits_and_unchanged_pads_survive_an_edit() {
        let mut model = ProjectModel::starter();
        let drum = model.tracks.iter().position(|t| t.is_drum).unwrap();
        let seq = Arc::new(Sequence::silent(&model, 4 * B));
        let bank = Arc::new(DrumBank::for_sequence(&seq, SR));
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequenceWithDrums {
            sequence: seq.clone(),
            drums: bank.clone(),
        });
        let kit = |e: &Engine| match &e.channels[drum].instrument {
            Instrument::Drums(k) => k.clone(),
            _ => panic!("drum track"),
        };
        let pads = seq.tracks[drum].drum_pads.clone();
        for p in &pads {
            assert!(
                Arc::ptr_eq(
                    kit(&e).pad_drum(p.pitch).unwrap(),
                    bank.get(&p.patch).unwrap()
                ),
                "pad {} not from the bank",
                p.pitch
            );
        }
        // Swap one pad's sound; every other pad keeps its buffer.
        model.tracks[drum].drum_pads[0].sound = "clap".into();
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::silent(
            &model,
            4 * B,
        ))));
        let k = kit(&e);
        assert_eq!(
            k.pad_drum(pads[0].pitch).unwrap().samples.len(),
            (0.2 * SR).round() as usize,
            "the clap now"
        );
        for p in &pads[1..] {
            assert!(
                Arc::ptr_eq(k.pad_drum(p.pitch).unwrap(), bank.get(&p.patch).unwrap()),
                "pad {} re-rendered",
                p.pitch
            );
        }
    }

    // ── Ropes ──

    /// An engine playing `notes` on one synth track in `mode`, with `glide` as the patch's
    /// slide time. The envelope settles well below its peak, so a replayed attack shows up as
    /// the level climbing back towards 1.
    fn roped(mode: VoiceMode, glide: f32, notes: Vec<NoteEvent>) -> Engine {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_ticks = 8 * B;
        model.patterns[0].notes = notes;
        let mut seq = Sequence::from_pattern(&model, &pid, 0).unwrap();
        let p = &mut seq.tracks[0].synth;
        p.voice = mode;
        p.glide = glide;
        p.amp_env.attack = 0.02;
        p.amp_env.decay = 0.02;
        p.amp_env.sustain = 0.3;
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(seq)));
        e.handle(EngineCommand::Play);
        e
    }

    /// `note`, roped to slide from `source`.
    fn rope(source: &NoteEvent, mut note: NoteEvent) -> NoteEvent {
        note.slide_from = Some(source.id);
        note
    }

    /// `rope`, with its own slide time.
    fn rope_secs(source: &NoteEvent, note: NoteEvent, secs: f32) -> NoteEvent {
        NoteEvent {
            slide_secs: Some(secs),
            ..rope(source, note)
        }
    }

    /// The voices on the first track that are holding a note.
    fn held(e: &Engine) -> Vec<crate::synth::VoiceProbe> {
        match &e.channels[0].instrument {
            Instrument::Synth(s) => s.held_voices(),
            other => panic!("not a synth: {other:?}"),
        }
    }

    /// Play `frames` more samples in small blocks; the loudest any held voice's envelope got.
    fn run(e: &mut Engine, frames: usize) -> f32 {
        let mut top = 0.0f32;
        let mut buf = [0.0f32; 128];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(64);
            e.process(&mut buf[..n * 2]);
            top = held(e).iter().map(|v| v.level).fold(top, f32::max);
            done += n;
        }
        top
    }

    /// Every note-on in the next `frames` samples, and whether it slides.
    fn note_ons(e: &mut Engine, frames: usize) -> Vec<(usize, u8, bool)> {
        e.collect_events(frames);
        e.events
            .iter()
            .filter_map(|ev| match ev.kind {
                EventKind::NoteOn { pitch, slide, .. } => Some((ev.offset, pitch, slide.is_some())),
                _ => None,
            })
            .collect()
    }

    const MODES: [VoiceMode; 3] = [VoiceMode::Poly, VoiceMode::Mono, VoiceMode::Legato];
    /// A little over three and a half beats: everything these tests schedule, short of the loop.
    const SPAN: usize = 7 * SR as usize / 2;

    #[test]
    fn a_rope_glides_into_its_note_without_an_attack_in_every_mode() {
        for mode in MODES {
            // Overlapping, and touching (the source ends just as the slide note starts).
            for len in [2 * B, B] {
                let a = NoteEvent::new(0, len, 57);
                let notes = vec![a.clone(), rope_secs(&a, NoteEvent::new(B, 2 * B, 64), 0.2)];
                assert_eq!(
                    note_events(&mut roped(mode, 0.0, notes.clone()), SPAN),
                    vec![(0, true, 57), (22_050, true, 64), (66_150, false, 64)],
                    "{mode:?}: the source's own end is held back"
                );
                let mut e = roped(mode, 0.0, notes);
                run(&mut e, 22_050 + 640);
                let v = held(&e);
                assert_eq!(v.len(), 1, "{mode:?}: one voice");
                assert_eq!(v[0].tag, Some(VoiceTag::for_hit(1, 0)), "{mode:?}");
                assert_eq!(v[0].target, 64.0);
                assert!(
                    v[0].glide > 57.0 && v[0].glide < 59.0,
                    "{mode:?}: {}",
                    v[0].glide
                );
                let top = run(&mut e, 22_050);
                assert!(
                    top < 0.35,
                    "{mode:?}: the slide replayed the attack ({top})"
                );
                assert!((held(&e)[0].glide - 64.0).abs() < 0.01, "{mode:?}: arrives");
                run(&mut e, 22_050);
                assert!(
                    held(&e).is_empty(),
                    "{mode:?}: the slide note's end releases it"
                );
            }
        }
    }

    #[test]
    fn a_dead_rope_plays_its_note_with_an_attack() {
        let a = NoteEvent::new(0, B, 57);
        let notes = vec![a.clone(), rope(&a, NoteEvent::new(B + B / 2, B, 64))];
        assert_eq!(
            note_events(&mut roped(VoiceMode::Poly, 0.2, notes.clone()), SPAN),
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (33_075, true, 64),
                (55_125, false, 64)
            ]
        );
        for mode in MODES {
            let mut e = roped(mode, 0.2, notes.clone());
            run(&mut e, 33_075 + 64);
            let v = held(&e);
            assert_eq!(v.len(), 1, "{mode:?}");
            assert_eq!(
                v[0].glide, 64.0,
                "{mode:?}: at its own pitch from the start"
            );
            assert!(run(&mut e, 1_200) > 0.9, "{mode:?}: attacks");
        }
    }

    #[test]
    fn a_chord_slides_only_the_notes_with_ropes() {
        let chord: Vec<NoteEvent> = [48, 52, 55].map(|p| NoteEvent::new(0, 2 * B, p)).into();
        let mut notes = chord.clone();
        notes.push(rope_secs(&chord[0], NoteEvent::new(B, B, 50), 0.05));
        notes.push(rope_secs(&chord[1], NoteEvent::new(B, B, 53), 0.05));
        let events = note_events(&mut roped(VoiceMode::Poly, 0.0, notes.clone()), SPAN);
        let offs: Vec<u8> = events.iter().filter(|e| !e.1).map(|e| e.2).collect();
        assert_eq!(
            offs,
            vec![55, 50, 53],
            "only the unroped chord note ends on its own"
        );

        let mut e = roped(VoiceMode::Poly, 0.0, notes);
        run(&mut e, 22_050 + 11_025);
        let v = held(&e);
        assert_eq!(v.len(), 3, "{v:?}");
        let at = |tag: VoiceTag| *v.iter().find(|v| v.tag == Some(tag)).unwrap();
        assert_eq!(
            at(VoiceTag::for_hit(2, 0)).glide,
            55.0,
            "the third keeps ringing"
        );
        assert!((at(VoiceTag::for_hit(3, 0)).glide - 50.0).abs() < 0.01);
        assert!((at(VoiceTag::for_hit(4, 0)).glide - 53.0).abs() < 0.01);
        assert!(
            v.iter().all(|v| v.level < 0.35),
            "nothing re-attacked: {v:?}"
        );
    }

    #[test]
    fn a_rope_takes_its_own_time_or_the_patch_glide() {
        let a = NoteEvent::new(0, 2 * B, 57);
        // How far from 57 towards 64 the slide has got 0.1 s in.
        let progress = |b: NoteEvent| {
            let mut e = roped(VoiceMode::Poly, 0.3, vec![a.clone(), b]);
            run(&mut e, 22_050 + 4_410);
            (held(&e)[0].glide - 57.0) / 7.0
        };
        let patch = progress(rope(&a, NoteEvent::new(B, B, 64)));
        assert!(
            (patch - 0.63).abs() < 0.05,
            "patch glide, one τ in: {patch}"
        );
        let own = progress(rope_secs(&a, NoteEvent::new(B, B, 64), 0.9));
        assert!(
            (own - 0.28).abs() < 0.05,
            "its own, a third of a τ in: {own}"
        );
        let mut e = roped(
            VoiceMode::Poly,
            0.3,
            vec![a.clone(), rope_secs(&a, NoteEvent::new(B, B, 64), 0.0)],
        );
        run(&mut e, 22_050 + 64);
        assert_eq!(held(&e)[0].glide, 64.0, "0 s jumps");
        assert_eq!(held(&e).len(), 1, "on the same voice");
    }

    #[test]
    fn only_the_first_hit_of_a_repeated_slide_note_slides() {
        let a = NoteEvent::new(0, 2 * B, 57);
        let mut b = rope_secs(&a, NoteEvent::new(B, B, 64), 0.05);
        b.repeats = 1;
        let notes = vec![a, b];
        assert_eq!(
            note_ons(&mut roped(VoiceMode::Poly, 0.0, notes.clone()), SPAN),
            vec![(0, 57, false), (22_050, 64, true), (33_075, 64, false)]
        );
        let mut e = roped(VoiceMode::Poly, 0.0, notes);
        run(&mut e, 11_025);
        assert!(
            run(&mut e, 33_000 - 11_025) < 0.35,
            "the first hit took the voice over"
        );
        assert!(run(&mut e, 1_200) > 0.9, "the second hit attacks");
        let v = held(&e);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].tag, Some(VoiceTag::for_hit(1, 1)));
        run(&mut e, 22_050);
        assert!(held(&e).is_empty());
    }

    /// Two ropes from one note: the first takes its voice, the second finds nothing to take
    /// and plays as an ordinary note. Nothing hangs.
    #[test]
    fn two_ropes_from_one_note_hand_its_voice_to_the_first() {
        let a = NoteEvent::new(0, 2 * B, 57);
        let notes = vec![
            a.clone(),
            rope_secs(&a, NoteEvent::new(B, 2 * B, 60), 0.05),
            rope_secs(&a, NoteEvent::new(B + B / 2, 2 * B, 64), 0.05),
        ];
        assert_eq!(
            note_events(&mut roped(VoiceMode::Poly, 0.0, notes.clone()), SPAN),
            vec![
                (0, true, 57),
                (22_050, true, 60),
                (33_075, true, 64),
                (66_150, false, 60),
                (77_175, false, 64)
            ]
        );
        for mode in MODES {
            let mut e = roped(mode, 0.0, notes.clone());
            run(&mut e, 33_075 + 64);
            let v = held(&e);
            let last = v.last().unwrap();
            assert_eq!(last.tag, Some(VoiceTag::for_hit(2, 0)), "{mode:?}");
            assert_eq!(
                last.glide, 64.0,
                "{mode:?}: the second plays from its own pitch"
            );
            if mode == VoiceMode::Poly {
                assert_eq!(v.len(), 2);
                assert_eq!(
                    v[0].tag,
                    Some(VoiceTag::for_hit(1, 0)),
                    "the first has a's voice"
                );
                assert!(run(&mut e, 1_200) > 0.9, "the second attacks");
            }
            run(&mut e, 2 * 22_050);
            assert!(held(&e).is_empty(), "{mode:?}: nothing hangs");
        }
    }

    #[test]
    fn a_chain_of_ropes_passes_one_voice_along() {
        let a = NoteEvent::new(0, B, 57);
        let b = rope_secs(&a, NoteEvent::new(B / 2, B, 60), 0.05);
        let c = rope_secs(&b, NoteEvent::new(B + B / 2, B, 64), 0.05);
        let notes = vec![a, b, c];
        assert_eq!(
            note_events(&mut roped(VoiceMode::Poly, 0.0, notes.clone()), SPAN),
            vec![
                (0, true, 57),
                (11_025, true, 60),
                (33_075, true, 64),
                (55_125, false, 64)
            ]
        );
        for mode in MODES {
            let mut e = roped(mode, 0.0, notes.clone());
            run(&mut e, 11_025 + 4_410);
            assert_eq!(held(&e)[0].tag, Some(VoiceTag::for_hit(1, 0)), "{mode:?}");
            let top = run(&mut e, 33_075 - 15_435 + 1_470);
            let v = held(&e);
            assert_eq!(v.len(), 1, "{mode:?}");
            assert_eq!(v[0].tag, Some(VoiceTag::for_hit(2, 0)), "{mode:?}");
            assert!(
                v[0].glide > 60.5 && v[0].glide < 64.0,
                "{mode:?}: {}",
                v[0].glide
            );
            assert!(top < 0.35, "{mode:?}: one attack for the whole chain");
            run(&mut e, 22_050);
            assert!(held(&e).is_empty(), "{mode:?}");
        }
    }

    #[test]
    fn seeking_respects_ropes() {
        let a = NoteEvent::new(0, 2 * B, 57);
        let notes = vec![a.clone(), rope_secs(&a, NoteEvent::new(B, B, 64), 0.2)];
        for mode in MODES {
            // Into the source, with its slide still ahead: the slide still happens.
            let mut e = roped(mode, 0.0, notes.clone());
            e.handle(EngineCommand::Seek {
                tick: (B / 2) as f64,
            });
            run(&mut e, 11_025 + 640);
            let v = held(&e);
            assert_eq!(v.len(), 1, "{mode:?}");
            assert_eq!(v[0].tag, Some(VoiceTag::for_hit(1, 0)), "{mode:?}");
            assert!(
                v[0].glide > 57.0 && v[0].glide < 59.0,
                "{mode:?}: {}",
                v[0].glide
            );

            // Into the slide note: it starts plainly at its own pitch, and the source, whose
            // end is held back, stays out of it.
            e.handle(EngineCommand::Seek {
                tick: (B + B / 2) as f64,
            });
            run(&mut e, 64);
            let v = held(&e);
            assert_eq!(v.len(), 1, "{mode:?}: {v:?}");
            assert_eq!(v[0].tag, Some(VoiceTag::for_hit(1, 0)), "{mode:?}");
            assert_eq!(v[0].glide, 64.0, "{mode:?}");
            run(&mut e, 11_025);
            assert!(held(&e).is_empty(), "{mode:?}: nothing hangs");
        }
    }

    /// A rope near the loop end: the slide note runs past the seam, where its end is never
    /// scheduled. The seam's release still frees the voice, and the next pass slides again.
    #[test]
    fn a_rope_at_the_loop_end_frees_its_voice_at_the_seam() {
        let a = NoteEvent::new(6 * B, 2 * B, 57);
        let notes = vec![
            a.clone(),
            rope_secs(&a, NoteEvent::new(7 * B, 2 * B, 64), 0.2),
        ];
        let mut e = roped(VoiceMode::Poly, 0.0, notes);
        run(&mut e, 7 * 22_050 + 640);
        assert_eq!(
            held(&e)[0].tag,
            Some(VoiceTag::for_hit(1, 0)),
            "slides on the first pass"
        );
        run(&mut e, 22_050);
        assert!(held(&e).is_empty(), "released at the seam");
        run(&mut e, 7 * 22_050);
        let v = held(&e);
        assert_eq!(v.len(), 1);
        assert_eq!(
            v[0].tag,
            Some(VoiceTag::for_hit(1, 0)),
            "and again on the next"
        );
        assert!(v[0].glide > 57.0 && v[0].glide < 59.0, "{}", v[0].glide);
    }

    // ── Ropes on a plugin ──

    /// `roped`, with the first track's ropes scheduled as for a hosted plugin.
    fn on_plugin(notes: Vec<NoteEvent>) -> Engine {
        let mut e = roped(VoiceMode::Poly, 0.0, notes);
        e.pretend_plugin.push(0);
        e
    }

    /// Whether `offset` is [`ROPE_OVERLAP_SECS`] after `at` (220.5 samples, which rounds
    /// either way).
    fn overlaps(at: usize, offset: usize) -> bool {
        let late = at as f64 + ROPE_OVERLAP_SECS * SR as f64;
        (offset as f64 - late).abs() <= 0.5
    }

    /// Like [`note_events`], over `frames` samples actually played in blocks of `block`, with
    /// every offset counted from the first block's start.
    fn note_events_in_blocks(
        e: &mut Engine,
        frames: usize,
        block: usize,
    ) -> Vec<(usize, bool, u8)> {
        let mut out = vec![];
        let mut buf = vec![0.0f32; block * 2];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(block);
            e.process(&mut buf[..n * 2]);
            out.extend(e.events.iter().filter_map(|ev| match ev.kind {
                EventKind::NoteOn { pitch, .. } => Some((done + ev.offset, true, pitch)),
                EventKind::NoteOff { pitch, .. } => Some((done + ev.offset, false, pitch)),
                _ => None,
            }));
            done += n;
        }
        out
    }

    #[test]
    fn a_plugin_hears_a_touching_rope_as_two_keys_overlapping() {
        let a = NoteEvent::new(0, B, 57);
        let notes = vec![a.clone(), rope_secs(&a, NoteEvent::new(B, 2 * B, 64), 0.2)];
        let events = note_events(&mut on_plugin(notes.clone()), SPAN);
        assert_eq!(events.len(), 4, "{events:?}");
        assert_eq!(&events[..2], &[(0, true, 57), (22_050, true, 64)]);
        let (off, on, pitch) = events[2];
        assert!(
            !on && pitch == 57 && overlaps(22_050, off),
            "the source ends just after the slide note starts: {events:?}"
        );
        assert_eq!(
            events[3],
            (66_150, false, 64),
            "the slide note ends as written"
        );
        assert_eq!(
            note_ons(&mut on_plugin(notes.clone()), SPAN),
            vec![(0, 57, false), (22_050, 64, false)],
            "a plugin is never asked to slide"
        );
        assert_eq!(
            note_events(&mut roped(VoiceMode::Poly, 0.0, notes), SPAN),
            vec![(0, true, 57), (22_050, true, 64), (66_150, false, 64)],
            "a synth track still hands the voice over and never ends the source"
        );
    }

    #[test]
    fn a_plugin_keeps_written_ends_where_overlapping_would_be_wrong() {
        let plain = |notes: Vec<NoteEvent>, expected: Vec<(usize, bool, u8)>, why: &str| {
            let mut events = note_events(&mut on_plugin(notes), SPAN);
            events.sort();
            assert_eq!(events, expected, "{why}");
        };
        // Already overlapping: it glides on a legato plugin as it is.
        let a = NoteEvent::new(0, 2 * B, 57);
        plain(
            vec![a.clone(), rope(&a, NoteEvent::new(B, 2 * B, 64))],
            vec![
                (0, true, 57),
                (22_050, true, 64),
                (44_100, false, 57),
                (66_150, false, 64),
            ],
            "an overlapping source keeps its own end",
        );
        // No rope.
        let a = NoteEvent::new(0, B, 57);
        plain(
            vec![a.clone(), NoteEvent::new(B, B, 64)],
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 64),
                (44_100, false, 64),
            ],
            "touching notes without a rope end where they are written",
        );
        // A tie: a late note-off would end the slide note's own key.
        plain(
            vec![a.clone(), rope(&a, NoteEvent::new(B, B, 57))],
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 57),
                (44_100, false, 57),
            ],
            "a rope between equal pitches",
        );
        // Another note of the source's key starting inside the overlap would be cut by it.
        plain(
            vec![
                a.clone(),
                rope(&a, NoteEvent::new(B, B, 64)),
                NoteEvent::new(B, B / 2, 57),
            ],
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 57),
                (22_050, true, 64),
                (33_075, false, 57),
                (44_100, false, 64),
            ],
            "a note of the same key starting under the overlap",
        );
    }

    #[test]
    fn a_repeated_plugin_source_holds_only_the_hit_that_touches_the_slide() {
        // Two hits, 0–1 and 1–2 beats; the slide note starts as the second ends.
        let mut a = NoteEvent::new(0, 2 * B, 57);
        a.repeats = 1;
        let events = note_events(
            &mut on_plugin(vec![a.clone(), rope(&a, NoteEvent::new(2 * B, B, 64))]),
            SPAN,
        );
        assert_eq!(events.len(), 6, "{events:?}");
        assert_eq!(
            &events[..4],
            &[
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 57),
                (44_100, true, 64)
            ],
            "the first hit ends where it is written"
        );
        let (off, on, pitch) = events[4];
        assert!(!on && pitch == 57 && overlaps(44_100, off), "{events:?}");
        assert_eq!(events[5], (66_150, false, 64));

        // The slide note starts as the *first* hit ends, which is where the second begins:
        // holding the first on would cut the second, so nothing is stretched.
        let mut events = note_events(
            &mut on_plugin(vec![a.clone(), rope(&a, NoteEvent::new(B, B, 64))]),
            SPAN,
        );
        events.sort();
        assert_eq!(
            events,
            vec![
                (0, true, 57),
                (22_050, false, 57),
                (22_050, true, 57),
                (22_050, true, 64),
                (44_100, false, 57),
                (44_100, false, 64),
            ]
        );
    }

    /// The stretched end lands in whichever block holds its time, exactly once, however the
    /// blocks fall around the slide note's start and the end itself.
    #[test]
    fn the_overlap_note_off_fires_once_across_block_boundaries() {
        let a = NoteEvent::new(0, B, 57);
        let notes = vec![a.clone(), rope(&a, NoteEvent::new(B, B, 64))];
        let whole = note_events_in_blocks(&mut on_plugin(notes.clone()), SPAN, SPAN);
        assert_eq!(
            whole.iter().filter(|&&(_, on, p)| !on && p == 57).count(),
            1
        );
        for block in [64, 441, 512, 22_051, 22_100, 22_270, 22_271, 22_272] {
            let events = note_events_in_blocks(&mut on_plugin(notes.clone()), SPAN, block);
            // The overlap is 220.5 samples, so where it rounds can depend on the block's start.
            let same = events.len() == whole.len()
                && events
                    .iter()
                    .zip(&whole)
                    .all(|(a, b)| a.1 == b.1 && a.2 == b.2 && a.0.abs_diff(b.0) <= 1);
            assert!(same, "blocks of {block}: {events:?} against {whole:?}");
        }
    }

    /// A slide note starting a hair before the loop end: its source's stretched end falls past
    /// the seam, so it is never scheduled, the seam's release ends the note instead, and the
    /// next pass neither hangs on to it nor gets a stray note-off.
    #[test]
    fn an_overlap_past_the_loop_seam_is_left_to_the_seams_release() {
        let a = NoteEvent::new(7 * B, B - 5, 57);
        let notes = vec![a.clone(), rope(&a, NoteEvent::new(8 * B - 5, B, 64))];
        let pass = 8 * 22_050;
        let mut e = on_plugin(notes);
        let events = note_events_in_blocks(&mut e, 2 * pass, 512);
        let on_at = |tick: i64| (tick as f64 * 22_050.0 / B as f64).round() as usize;
        assert_eq!(
            events,
            vec![
                (on_at(7 * B), true, 57),
                (on_at(8 * B - 5), true, 64),
                (pass + on_at(7 * B), true, 57),
                (pass + on_at(8 * B - 5), true, 64),
            ],
            "no note-off at all: both notes run into the seam"
        );
        run(&mut e, 512);
        assert!(held(&e).is_empty(), "released at the seam: {:?}", held(&e));
    }

    #[test]
    fn seeking_into_a_plugin_source_still_overlaps_its_slide_note() {
        let a = NoteEvent::new(0, B, 57);
        let notes = vec![a.clone(), rope(&a, NoteEvent::new(B, B, 64))];
        let mut e = on_plugin(notes.clone());
        e.handle(EngineCommand::Seek {
            tick: (B / 2) as f64,
        });
        let events = note_events(&mut e, 22_050);
        assert_eq!(
            &events[..2],
            &[(0, true, 57), (11_025, true, 64)],
            "{events:?}"
        );
        assert!(events[2].0 > 11_025 && !events[2].1 && events[2].2 == 57);

        // Into the slide note: the touching source has ended, so it is not resumed.
        let mut e = on_plugin(notes);
        e.handle(EngineCommand::Seek {
            tick: (B + B / 2) as f64,
        });
        assert_eq!(note_ons(&mut e, 22_050), vec![(0, 64, false)]);
    }

    #[test]
    fn drum_tracks_ignore_ropes() {
        let mut model = ProjectModel::starter();
        model.tempo = 120.0;
        model.patterns[1].length_ticks = 8 * B;
        let drums = model.patterns[1].id;
        let kick = NoteEvent::new(0, 2 * B, 36);
        model.patterns[1].notes = vec![kick.clone(), rope(&kick, NoteEvent::new(B, B, 38))];
        let seq = Sequence::from_pattern(&model, &drums, 0).unwrap();
        assert!(seq.tracks[1].ropes.is_empty());
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(seq)));
        e.handle(EngineCommand::Play);
        assert_eq!(
            note_ons(&mut e, SPAN),
            vec![(0, 36, false), (22_050, 38, false)]
        );
    }
}
