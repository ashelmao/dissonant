//! The block processor: transport + sample-accurate event scheduling + mixing.

use crate::channel::{Channel, Instrument, Master, SYNTH_POLYPHONY};
use crate::drums::{DrumBank, DrumKit, RenderedDrum};
use crate::dsp::pan_gains;
use crate::synth::PolySynth;
use dissonant_core::{factory_sound, swing_warp, MasterSettings, Sequence, SoundPatch, SynthPatch, Tempo};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use uuid::Uuid;

pub const MAX_TRACKS: usize = 64;

/// Audition hold bounds. The floor keeps very fine grids audible; the ceiling stops a stray
/// value from leaving a voice ringing.
pub const MIN_AUDITION_SECS: f32 = 0.12;
pub const MAX_AUDITION_SECS: f32 = 4.0;

/// Samples [`Shared::preview_scope`] returns.
pub const SCOPE_SAMPLES: usize = 512;
/// The preview scope's history. Readers look at the newest half, so a block written while they
/// read can't tear the window they return.
const SCOPE_RING: usize = 4096;

/// What the real-time side publishes for the UI to read without locks.
#[derive(Debug)]
pub struct Shared {
    position_bits: AtomicU64,
    playing: AtomicBool,
    sample_rate: AtomicU32,
    /// Per-track peak (index = position in the current sequence), as f32 bits.
    track_peaks: [AtomicU32; MAX_TRACKS],
    master_peak: [AtomicU32; 2],
    /// Frames per device callback, as last observed.
    block_frames: AtomicU32,
    /// Render time / block time, smoothed, as f32 bits (1.0 = no headroom left).
    load: AtomicU32,
    /// Callback gaps detected since the stream started.
    xruns: AtomicU32,
    /// Beats of count-in still to go (f32 bits); 0 when not counting in.
    count_in: AtomicU32,
    /// The preview channel's mono output as f32 bits, a ring indexed by `scope_written`.
    scope: [AtomicU32; SCOPE_RING],
    /// Samples ever written to `scope`.
    scope_written: AtomicUsize,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            position_bits: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            sample_rate: AtomicU32::new(44_100),
            track_peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            master_peak: [AtomicU32::new(0), AtomicU32::new(0)],
            block_frames: AtomicU32::new(0),
            load: AtomicU32::new(0),
            xruns: AtomicU32::new(0),
            count_in: AtomicU32::new(0),
            scope: std::array::from_fn(|_| AtomicU32::new(0)),
            scope_written: AtomicUsize::new(0),
        }
    }
}

impl Shared {
    pub fn position_beats(&self) -> f64 {
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
        let start = (1..=last_start).rev().find(|&i| recent[i - 1] < 0.0 && recent[i] >= 0.0).unwrap_or(last_start);
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
    /// Build from a sound's patch, rendering a drum at `sample_rate` (call off the audio thread).
    pub fn from_sound(patch: &SoundPatch, sample_rate: f32) -> Self {
        match patch {
            SoundPatch::Synth(p) => PreviewPatch::Synth(Box::new(*p)),
            SoundPatch::Drum(p) => PreviewPatch::Drum(Arc::new(RenderedDrum::new(p, sample_rate))),
        }
    }
}

/// Messages from the application to the audio thread.
#[derive(Debug, Clone)]
pub enum EngineCommand {
    Play,
    Stop,
    Seek { beat: f64 },
    SetLooping(bool),
    /// Loop a sub-range `[start, end)` of the sequence instead of the whole thing.
    SetLoop { start: f64, end: f64 },
    ClearLoop,
    /// A held note from a controller / typing keyboard. Released by `NoteOff`.
    NoteOn { track_id: Uuid, pitch: u8, velocity: u8 },
    NoteOff { track_id: Uuid, pitch: u8 },
    /// Replace what's playing. The old `Arc` is dropped on the audio thread; sequences are
    /// small so this is acceptable for now (a return channel is the real-time-pure fix).
    /// Drum pads are rendered on the spot, which suits offline rendering; `AudioDevice::send`
    /// turns this into `SetSequenceWithDrums` so a live engine never does that.
    SetSequence(Arc<Sequence>),
    /// `SetSequence` with the kits' drums already rendered (see [`DrumBank`]).
    SetSequenceWithDrums { sequence: Arc<Sequence>, drums: Arc<DrumBank> },
    SetHearChords(bool),
    SetMaster(MasterSettings),
    /// Sound a note on a track right now (placement audition / typing keyboard). `seconds` is
    /// how long to hold it before the automatic release — the roll passes the length of the note
    /// being previewed, so auditions don't outlast the note the user is drawing.
    Audition { track_id: Uuid, pitch: u8, velocity: u8, seconds: f32 },
    /// Release an audition note early (typing keyboard key-up).
    AuditionOff { track_id: Uuid, pitch: u8 },
    /// A short sine on the master bus, for checking the output device from Settings.
    TestTone,
    /// Click on every beat (accented on the bar) while playing; `volume` 0…1.
    SetMetronome { on: bool, volume: f32 },
    /// Click through `bars` bars with the transport held, then play.
    PlayWithCountIn { bars: u32 },
    /// Load (or with `None`, release and close) the preview channel: an instrument outside the
    /// track list that sounds whether or not the transport runs, through the master chain,
    /// untouched by mute / solo / stop. A new synth patch applies to held notes without cutting.
    SetPreview(Option<PreviewPatch>),
    /// Play the preview. A drum preview plays its sound on any pitch.
    PreviewNoteOn { pitch: u8, velocity: u8 },
    PreviewNoteOff { pitch: u8 },
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct Event {
    offset: usize,
    kind: EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EventKind {
    NoteOn { track: usize, pitch: u8, velocity: u8 },
    NoteOff { track: usize, pitch: u8 },
    ChordOn { pitch: u8 },
    ChordOff { pitch: u8 },
    ReleaseAll,
    Click { accent: bool },
}

struct Audition {
    track_id: Uuid,
    pitch: u8,
    samples_left: usize,
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
    playing: bool,
    looping: bool,
    loop_region: Option<(f64, f64)>,
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
    preview: Option<Instrument>,
    /// `SetPreview(None)` arrived: let the preview ring out, then drop it.
    preview_closing: bool,
    preview_buf: Vec<f32>,
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
        shared.sample_rate.store(sample_rate as u32, Ordering::Relaxed);
        Engine {
            sample_rate,
            shared,
            sequence: Arc::new(Sequence::default()),
            channels: Vec::new(),
            channel_ids: Vec::new(),
            chord_bed: PolySynth::new(&chord_bed_patch(), sample_rate, 8),
            hear_chords: false,
            master: Master::new(sample_rate, master),
            playing: false,
            looping: true,
            loop_region: None,
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
            preview: None,
            preview_closing: false,
            preview_buf: vec![0.0; 8192],
        }
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
                let beats = bars as f64 * self.sequence.beats_per_bar.max(1.0);
                let total = (beats * self.tempo.samples_per_beat(self.sample_rate as f64)) as usize;
                if total == 0 {
                    self.playing = true;
                } else {
                    self.playing = false;
                    self.release_all();
                    self.count_in = Some((total, 0));
                }
            }
            EngineCommand::Seek { beat } => {
                self.release_all();
                self.position = beat.max(0.0);
                let (start, end) = self.loop_bounds();
                if self.looping && self.position >= end {
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
            EngineCommand::NoteOn { track_id, pitch, velocity } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_on(pitch, velocity);
                }
                self.auditions.retain(|a| !(a.track_id == track_id && a.pitch == pitch));
            }
            EngineCommand::NoteOff { track_id, pitch } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_off(pitch);
                }
                self.auditions.retain(|a| !(a.track_id == track_id && a.pitch == pitch));
            }
            EngineCommand::SetSequence(seq) => self.set_sequence(seq, None),
            EngineCommand::SetSequenceWithDrums { sequence, drums } => self.set_sequence(sequence, Some(&drums)),
            EngineCommand::SetPreview(patch) => self.set_preview(patch),
            EngineCommand::PreviewNoteOn { pitch, velocity } => match (&mut self.preview, self.preview_closing) {
                (Some(Instrument::Drums(kit)), false) => kit.trigger(0, velocity),
                (Some(inst), false) => inst.note_on(pitch, velocity),
                _ => {}
            },
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
            EngineCommand::Audition { track_id, pitch, velocity, seconds } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_on(pitch, velocity);
                    let hold = (self.sample_rate * seconds.clamp(MIN_AUDITION_SECS, MAX_AUDITION_SECS)) as usize;
                    if let Some(a) = self.auditions.iter_mut().find(|a| a.track_id == track_id && a.pitch == pitch) {
                        a.samples_left = hold;
                    } else {
                        self.auditions.push(Audition { track_id, pitch, samples_left: hold });
                    }
                }
            }
            EngineCommand::AuditionOff { track_id, pitch } => {
                if let Some(ch) = self.channel_mut(&track_id) {
                    ch.instrument.note_off(pitch);
                }
                self.auditions.retain(|a| !(a.track_id == track_id && a.pitch == pitch));
            }
            EngineCommand::TestTone => self.test_tone = ((self.sample_rate * 0.6) as usize, 0.0),
        }
        self.publish();
    }

    /// The active loop as `(start, end)`: the region if one is set (clamped to the
    /// sequence), else the whole sequence.
    fn loop_bounds(&self) -> (f64, f64) {
        let len = self.sequence.length_beats.max(0.0);
        match self.loop_region {
            Some((s, e)) if e > s => (s.max(0.0).min(len), e.min(len).max(s.max(0.0))),
            _ => (0.0, len),
        }
    }

    fn channel_mut(&mut self, id: &Uuid) -> Option<&mut Channel> {
        let idx = self.channel_ids.iter().position(|c| c == id)?;
        self.channels.get_mut(idx)
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
                    _ => self.preview = Some(Instrument::Synth(PolySynth::new(&*p, sr, SYNTH_POLYPHONY))),
                }
                self.preview_closing = false;
            }
            Some(PreviewPatch::Drum(drum)) => {
                // The rates differ only around a device restart; rendering here then beats
                // playing the drum at the wrong speed.
                let drum = if drum.sample_rate == sr { drum } else { Arc::new(RenderedDrum::new(&drum.patch, sr)) };
                match &mut self.preview {
                    Some(Instrument::Drums(kit)) => kit.set_drum(0, drum),
                    _ => self.preview = Some(Instrument::Drums(DrumKit::single(drum))),
                }
                self.preview_closing = false;
            }
        }
    }

    /// Mix the preview channel into the first `frames` of the mix (centred, like a track at pan
    /// 0) and feed the scope.
    fn render_preview(&mut self, frames: usize) {
        if self.preview_buf.len() < frames {
            self.preview_buf.resize(frames, 0.0);
        }
        let buf = &mut self.preview_buf[..frames];
        buf.fill(0.0);
        if let Some(inst) = &mut self.preview {
            inst.render_add(buf);
            if self.preview_closing && inst.is_silent() {
                self.preview = None;
                self.preview_closing = false;
            }
            let (gl, gr) = pan_gains(0.0);
            for (i, s) in buf.iter().enumerate() {
                self.mix_l[i] += s * gl;
                self.mix_r[i] += s * gr;
            }
        }
        self.shared.write_scope(buf);
    }

    fn set_sequence(&mut self, seq: Arc<Sequence>, drums: Option<&DrumBank>) {
        // Rebuild the channel list in sequence order, keeping existing channels (and their
        // sounding voices) for tracks that are still present.
        let mut new_channels = Vec::with_capacity(seq.tracks.len());
        let mut new_ids = Vec::with_capacity(seq.tracks.len());
        let old_ids = std::mem::take(&mut self.channel_ids);
        let mut old = std::mem::take(&mut self.channels).into_iter().map(Some).collect::<Vec<_>>();
        for track in &seq.tracks {
            let existing = old_ids.iter().position(|id| id == &track.id).and_then(|i| old[i].take());
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
                .map(|t| t.notes != track.notes || t.audible(old_seq.any_solo()) != track.audible(self.sequence.any_solo()))
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
        self.tempo = Tempo::new(self.sequence.tempo_bpm);
        let (start, end) = self.loop_bounds();
        if self.looping && self.position >= end {
            self.position = start;
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
        self.shared.position_bits.store(self.position.to_bits(), Ordering::Relaxed);
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        for (i, ch) in self.channels.iter().enumerate().take(MAX_TRACKS) {
            self.shared.track_peaks[i].store(ch.peak.to_bits(), Ordering::Relaxed);
        }
        self.shared.master_peak[0].store(self.master.peak_l.to_bits(), Ordering::Relaxed);
        self.shared.master_peak[1].store(self.master.peak_r.to_bits(), Ordering::Relaxed);
        let left = match self.count_in {
            Some((total, done)) => ((total - done) as f64 / self.tempo.samples_per_beat(self.sample_rate as f64)) as f32,
            None => 0.0,
        };
        self.shared.count_in.store(left.to_bits(), Ordering::Relaxed);
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

        self.tick_count_in(frames);
        self.collect_events(frames);
        self.tick_auditions(frames);

        // Render sub-blocks between events so every note starts on its exact sample.
        let mut cursor = 0usize;
        let mut ev_idx = 0usize;
        while cursor < frames {
            let next = self.events.get(ev_idx).map(|e| e.offset.min(frames)).unwrap_or(frames);
            if next > cursor {
                self.render_range(cursor, next);
                cursor = next;
            }
            while let Some(e) = self.events.get(ev_idx).filter(|e| e.offset.min(frames) <= cursor) {
                match e.kind {
                    EventKind::Click { accent } => self.click(cursor, accent),
                    kind => self.fire(kind),
                }
                ev_idx += 1;
            }
        }

        self.render_preview(frames);
        let (l, r) = (&mut self.mix_l[..frames], &mut self.mix_r[..frames]);
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
                let env = (done.min(fade) as f32 / fade as f32).min(self.test_tone.0.min(fade) as f32 / fade as f32);
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

        if self.playing {
            self.position = self.advance(self.position, frames - self.late_start.min(frames));
        }
        self.late_start = 0;
        self.publish();
    }

    /// While counting in: click on each beat boundary inside this block and, when the count
    /// runs out, start the transport at the exact sample (the remainder of the block plays).
    fn tick_count_in(&mut self, frames: usize) {
        let Some((total, done)) = self.count_in else { return };
        let spb = self.tempo.samples_per_beat(self.sample_rate as f64);
        let bpb = self.sequence.beats_per_bar.max(1.0).round() as usize;
        let end = (done + frames).min(total);
        // Beat boundaries at k·spb for integer k in [done, end).
        let mut k = (done as f64 / spb).ceil() as usize;
        while (k as f64 * spb) < end as f64 {
            let at = (k as f64 * spb).round() as usize;
            if at >= done {
                self.click(at - done, k.is_multiple_of(bpb));
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

    fn render_range(&mut self, from: usize, to: usize) {
        let (l, r) = (&mut self.mix_l[from..to], &mut self.mix_r[from..to]);
        let any_solo = self.sequence.any_solo();
        for (ch, track) in self.channels.iter_mut().zip(self.sequence.tracks.iter()) {
            if track.audible(any_solo) || !ch.instrument.is_silent() {
                ch.render_add(l, r);
            } else {
                // Muted and finished releasing: nothing to mix, but the meter still has to fall
                // instead of staying lit at the level it had when the track was muted.
                ch.decay_meter(to - from);
            }
        }
        if !self.chord_bed.is_silent() {
            let frames = to - from;
            // Render the bed mono into a scratch slice of mix_l's tail? No: reuse a small
            // stack buffer in chunks to avoid allocation.
            let mut buf = [0.0f32; 256];
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(256);
                let chunk = &mut buf[..n];
                chunk.fill(0.0);
                self.chord_bed.render_add(chunk);
                for i in 0..n {
                    let s = chunk[i] * 0.6;
                    l[done + i] += s;
                    r[done + i] += s;
                }
                done += n;
            }
        }
    }

    fn fire(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn { track, pitch, velocity } => {
                if let Some(ch) = self.channels.get_mut(track) {
                    ch.instrument.note_on(pitch, velocity);
                }
            }
            EventKind::NoteOff { track, pitch } => {
                if let Some(ch) = self.channels.get_mut(track) {
                    ch.instrument.note_off(pitch);
                }
            }
            EventKind::ChordOn { pitch } => self.chord_bed.note_on(pitch, 80),
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

    fn advance(&self, position: f64, frames: usize) -> f64 {
        let map = &self.sequence.tempo_map;
        let secs = frames as f64 / self.sample_rate as f64;
        let mut p = if map.is_constant() {
            position + self.tempo.beats_for_samples(frames as f64, self.sample_rate as f64)
        } else {
            map.beat_at(map.seconds_at(position) + secs)
        };
        let (start, end) = self.loop_bounds();
        let len = end - start;
        if self.looping && len > 0.0 && p >= end {
            // Carry the time past the loop end into the loop start at the tempo there.
            let excess = map.seconds_at(p) - map.seconds_at(end);
            p = map.beat_at(map.seconds_at(start) + excess);
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
        let t0 = map.seconds_at(start);
        let end = map.beat_at(t0 + avail);

        if std::mem::take(&mut self.resume_notes) {
            self.resume_notes_under(start);
        }

        if self.looping && loop_end > loop_start && end >= loop_end && start < loop_end {
            // Two segments: [start, loop_end) then [loop_start, …), with a release at the seam.
            let seam_secs = map.seconds_at(loop_end) - t0;
            let seam = base + (seam_secs * sr).round() as usize;
            self.collect_segment(start, loop_end, base);
            self.events.push(Event { offset: seam.min(frames), kind: EventKind::ReleaseAll });
            let to = map.beat_at(map.seconds_at(loop_start) + (avail - seam_secs).max(0.0));
            self.collect_segment(loop_start, to, seam);
        } else {
            self.collect_segment(start, end, base);
        }
        self.events.sort_by_key(|e| e.offset);
    }

    /// Notes already sounding at `from` — the ones the playhead landed inside. Scheduling only
    /// fires a note at its start beat, so without this, seeking into (or starting playback in)
    /// the middle of a long note gives silence until the next note begins. The chord bed has
    /// always resumed this way; notes now match it.
    fn resume_notes_under(&mut self, from: f64) {
        let any_solo = self.sequence.any_solo();
        let seq = Arc::clone(&self.sequence);
        for (ti, track) in seq.tracks.iter().enumerate() {
            // A drum hit is a one-shot: re-firing it mid-note would retrigger the sample.
            if !track.audible(any_solo) || track.is_drum {
                continue;
            }
            for note in &track.notes {
                if note.muted {
                    continue;
                }
                if note.start_beat < from && note.end_beat().min(seq.length_beats) > from {
                    self.events.push(Event {
                        offset: 0,
                        kind: EventKind::NoteOn {
                            track: ti,
                            pitch: note.pitch.clamp(0, 127) as u8,
                            velocity: note.velocity.clamp(1, 127) as u8,
                        },
                    });
                }
            }
        }
    }

    fn collect_segment(&mut self, from: f64, to: f64, base_offset: usize) {
        let any_solo = self.sequence.any_solo();
        let seq = Arc::clone(&self.sequence);
        let sr = self.sample_rate as f64;
        let t_from = seq.tempo_map.seconds_at(from);
        let offset_of = |beat: f64| base_offset + ((seq.tempo_map.seconds_at(beat) - t_from) * sr).round().max(0.0) as usize;
        let swing = |beat: f64| swing_warp(beat, seq.swing, seq.swing_grid);

        if self.metronome {
            let bpb = seq.beats_per_bar.max(1.0);
            let mut b = from.ceil();
            while b < to {
                let accent = (b % bpb).abs() < 1e-9;
                self.events.push(Event { offset: offset_of(b), kind: EventKind::Click { accent } });
                b += 1.0;
            }
        }

        for (ti, track) in seq.tracks.iter().enumerate() {
            if !track.audible(any_solo) {
                continue;
            }
            for note in &track.notes {
                if note.muted {
                    continue;
                }
                let s = swing(note.start_beat);
                let e = swing(note.end_beat()).min(seq.length_beats);
                if s >= from && s < to {
                    self.events.push(Event {
                        offset: offset_of(s),
                        kind: EventKind::NoteOn {
                            track: ti,
                            pitch: note.pitch.clamp(0, 127) as u8,
                            velocity: note.velocity.clamp(1, 127) as u8,
                        },
                    });
                }
                if e > s && e >= from && e < to && !track.is_drum {
                    self.events.push(Event {
                        offset: offset_of(e),
                        kind: EventKind::NoteOff {
                            track: ti,
                            pitch: note.pitch.clamp(0, 127) as u8,
                        },
                    });
                }
            }
        }

        if self.hear_chords {
            for chord in seq.chords.chords() {
                let s = chord.start_beat;
                let e = chord.end_beat().min(seq.length_beats);
                if s >= from && s < to {
                    for &pc in &chord.pitch_classes {
                        self.events.push(Event {
                            offset: offset_of(s),
                            kind: EventKind::ChordOn { pitch: (60 + pc.rem_euclid(12)) as u8 },
                        });
                    }
                }
                if e > s && e >= from && e < to {
                    for &pc in &chord.pitch_classes {
                        self.events.push(Event {
                            offset: offset_of(e),
                            kind: EventKind::ChordOff { pitch: (60 + pc.rem_euclid(12)) as u8 },
                        });
                    }
                }
            }
            // Seeking into the middle of a chord: start it now.
            if let Some(chord) = seq.chords.chord_at(from) {
                if self.sounding_chord != Some(chord.id) && chord.start_beat < from {
                    for &pc in &chord.pitch_classes {
                        self.events.push(Event {
                            offset: base_offset,
                            kind: EventKind::ChordOn { pitch: (60 + pc.rem_euclid(12)) as u8 },
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
        self.channel_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissonant_core::{NoteEvent, ProjectModel};

    const SR: f32 = 44_100.0;

    fn sequence_with_note(start: f64, len_beats: f64, loop_len: f64) -> Arc<Sequence> {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_beats = loop_len;
        model.patterns[0].notes = vec![NoteEvent::new(start, len_beats, 69)];
        Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())
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

    fn first_onset(samples: &[f32]) -> Option<usize> {
        samples.iter().position(|s| s.abs() > 1e-6)
    }

    #[test]
    fn note_starts_on_the_exact_sample_regardless_of_block_size() {
        for block in [64usize, 480, 1024, 4096] {
            let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
            e.handle(EngineCommand::SetSequence(sequence_with_note(1.0, 1.0, 16.0)));
            e.handle(EngineCommand::Play);
            let left = render(&mut e, 30_000, block);
            let onset = first_onset(&left).expect("note should sound");
            // 120 bpm → beat 1 = 22050 samples; the first non-zero sample is one after onset.
            assert!((onset as i64 - 22_050).abs() <= 2, "block {block}: onset {onset}");
        }
    }

    #[test]
    fn note_releases_and_loop_retriggers() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(0.0, 0.5, 2.0)));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 44_100 * 2, 512);
        // One loop = 2 beats = 44100 samples. The second onset must land at the loop start.
        let second_half = &left[44_100 - 10..];
        let onset = first_onset(second_half).unwrap() + 44_100 - 10;
        assert!((onset as i64 - 44_100).abs() <= 2, "second onset {onset}");
        // Between the release tail (0.5 beat = 11025 samples + 0.2 s release) and the loop, silence.
        let quiet = &left[11_025 + 10_000..44_000];
        assert!(quiet.iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn stopped_engine_is_silent_and_position_holds() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(0.0, 1.0, 4.0)));
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
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 1.0, 69)];
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::Play);
        assert!(render(&mut e, 8_000, 256).iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn drums_and_chord_bed_make_sound() {
        let mut model = ProjectModel::starter();
        let pid = model.patterns[1].id; // the drum pattern
        model.patterns[1].notes = vec![NoteEvent::new(0.0, 0.5, 36)];
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::SetHearChords(true));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 20_000, 512);
        assert!(left.iter().any(|s| s.abs() > 0.05));
        assert!(shared.position_beats() > 0.0);
        assert!(shared.is_playing());
        assert!(shared.master_peak().0 > 0.0);
    }

    /// The starter has to make music on the first press of space, chord bed or not: in the
    /// song (where a new project opens) and in each of its patterns on its own.
    #[test]
    fn starter_makes_sound_without_the_chord_bed() {
        let model = ProjectModel::starter();
        let mut sequences = vec![Sequence::from_song(&model)];
        sequences.extend(model.patterns.iter().map(|p| Sequence::from_pattern(&model, &p.id, 0.0).unwrap()));
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
            e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(&model))));
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
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::Audition { track_id: tid, pitch: 60, velocity: 100, seconds: 0.8 });
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
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::Audition { track_id: tid, pitch: 60, velocity: 100, seconds: 0.125 });
        let left = render(&mut e, SR as usize, 512);
        let sr = SR as usize;
        let loud = |from: usize, to: usize| left[from..to].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(loud(0, sr / 20) > 0.01, "the preview must speak");
        // Held 0.125 s, then the voice's release (~0.14 s) runs out: silent well before 0.5 s.
        assert!(loud(sr / 2, sr) < 1e-4, "preview outlived the note it previewed");
    }

    /// A grid finer than the floor still has to be audible rather than clicking off instantly.
    #[test]
    fn audition_length_is_clamped_to_a_floor() {
        let model = ProjectModel::empty();
        let (tid, pid) = (model.tracks[0].id, model.patterns[0].id);
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::Audition { track_id: tid, pitch: 60, velocity: 100, seconds: 0.0 });
        let left = render(&mut e, SR as usize, 512);
        let at_floor = (SR * MIN_AUDITION_SECS * 0.8) as usize;
        assert!(left[..at_floor].iter().any(|s| s.abs() > 0.01), "a sub-floor preview must still speak");
    }

    #[test]
    fn tempo_map_moves_notes_and_the_playhead() {
        use dissonant_core::{Clip, TempoPoint};
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].length_beats = 16.0;
        model.patterns[0].notes = vec![NoteEvent::new(8.0, 1.0, 69)];
        model.clips = vec![Clip::new(pid, 0.0, 16.0)];
        // 120 for 4 beats (2 s), then 60: beat 8 is reached at 2 + 4 = 6 s.
        model.tempo_points = vec![TempoPoint::new(4.0, 60.0, false)];
        let seq = Arc::new(Sequence::from_song(&model));
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(seq));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 44_100 * 6 + 2_000, 1000);
        let onset = first_onset(&left).unwrap() as i64;
        assert!((onset - 44_100 * 6).abs() <= 2, "onset {onset}");
        // The playhead: 6 s + 2000 samples at 60 bpm ≈ beat 8.045.
        let expected = 8.0 + (2_000.0 / 44_100.0) * (60.0 / 60.0);
        assert!((e.position() - expected).abs() < 1e-6, "{}", e.position());
    }

    #[test]
    fn muted_notes_are_skipped() {
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        let mut n = NoteEvent::new(0.0, 1.0, 69);
        n.muted = true;
        model.patterns[0].notes = vec![n];
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap());
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
        model.patterns[0].length_beats = 4.0;
        model.patterns[0].notes = vec![NoteEvent::new(0.5, 0.25, 69)];
        model.swing = 200.0 / 3.0; // triplet feel: the "and" of 1 lands at 2/3 beat
        let seq = Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap());
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(seq));
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 30_000, 512);
        let onset = first_onset(&left).unwrap() as i64;
        let expected = (22_050.0 * 2.0 / 3.0) as i64;
        assert!((onset - expected).abs() <= 2, "onset {onset} vs {expected}");
    }

    #[test]
    fn metronome_clicks_on_beats_and_count_in_holds_the_transport() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(1.0, 1.0, 16.0)));
        e.handle(EngineCommand::SetMetronome { on: true, volume: 1.0 });
        e.handle(EngineCommand::Play);
        let left = render(&mut e, 22_050 + 100, 256);
        // A click at beat 0 (sample 0) and another at beat 1 (sample 22050).
        assert!(left[..50].iter().any(|s| s.abs() > 1e-4), "click on beat 0");
        assert!(left[2_500..21_000].iter().all(|s| s.abs() < 1e-4), "silence between clicks (note starts at beat 1)");
        assert!(left[22_050..22_100].iter().any(|s| s.abs() > 1e-4), "click on beat 1");

        // Count-in: one bar (4 beats = 88200 samples) of clicks, then the transport starts.
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(sequence_with_note(0.0, 1.0, 16.0)));
        e.handle(EngineCommand::SetMetronome { on: false, volume: 1.0 });
        e.handle(EngineCommand::PlayWithCountIn { bars: 1 });
        assert!(!e.is_playing());
        let half = render(&mut e, 44_100, 1000);
        assert!(shared.count_in_beats() > 1.9 && shared.count_in_beats() < 2.1, "{}", shared.count_in_beats());
        assert_eq!(e.position(), 0.0, "transport held during count-in");
        assert!(half[..50].iter().any(|s| s.abs() > 1e-4), "count-in clicks even with the metronome off");
        let rest = render(&mut e, 44_100 + 2_000, 1000);
        assert!(e.is_playing());
        assert_eq!(shared.count_in_beats(), 0.0);
        // The note at beat 0 starts exactly when the count-in ends (sample 88200 overall).
        let onset = first_onset(&rest[44_100 - 10..]).unwrap() + 44_100 - 10;
        assert!((onset as i64 - 44_100).abs() <= 2, "onset {onset}");
        // Position advanced only for the part of the block after the start.
        assert!((e.position() - 2_000.0 / 22_050.0).abs() < 1e-6, "{}", e.position());
    }

    #[test]
    fn loop_region_repeats_only_its_range() {
        // Notes at beat 0 and beat 4 in a 16-beat pattern; loop [4, 8) → only the beat-4 note
        // sounds, once per 4 beats.
        let mut model = ProjectModel::empty();
        model.tracks[0].voice = "sine".into();
        let pid = model.patterns[0].id;
        model.patterns[0].notes = vec![NoteEvent::new(0.0, 0.5, 69), NoteEvent::new(4.0, 0.5, 69)];
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::SetLoop { start: 4.0, end: 8.0 });
        e.handle(EngineCommand::Seek { beat: 0.0 });
        assert_eq!(e.position(), 0.0, "seeking before the loop is allowed");
        e.handle(EngineCommand::Seek { beat: 4.0 });
        e.handle(EngineCommand::Play);
        // 4 beats = 88200 samples per loop pass; render two passes.
        let left = render(&mut e, 88_200 * 2 + 100, 512);
        let first = first_onset(&left).unwrap();
        assert!(first <= 2, "first onset {first}");
        let second = first_onset(&left[88_200 - 10..]).unwrap() + 88_200 - 10;
        assert!((second as i64 - 88_200).abs() <= 2, "second onset {second}");
        assert!(e.position() >= 4.0 && e.position() < 8.0);
    }

    #[test]
    fn held_note_sounds_until_released() {
        let model = ProjectModel::empty();
        let (tid, pid) = (model.tracks[0].id, model.patterns[0].id);
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::NoteOn { track_id: tid, pitch: 60, velocity: 100 });
        let held = render(&mut e, 44_100 * 2, 512);
        assert!(held[80_000..].iter().any(|s| s.abs() > 0.01), "a held note must not auto-release");
        e.handle(EngineCommand::NoteOff { track_id: tid, pitch: 60 });
        let tail = render(&mut e, 44_100, 512);
        assert!(tail[30_000..].iter().all(|s| s.abs() < 1e-4));
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
        e.handle(EngineCommand::SetSequence(sequence_with_note(0.0, 1.0, 4.0)));
        assert!(shared.preview_scope().iter().all(|s| *s == 0.0), "nothing previewed yet");
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(&factory_patch("saw"), SR))));
        e.handle(EngineCommand::PreviewNoteOn { pitch: 57, velocity: 100 });
        let held = render(&mut e, 8_000, 512);
        assert!(!e.is_playing() && e.position() == 0.0, "the transport stayed put");
        assert!(peak(&held[4_000..]) > 0.05, "the preview note must sound");

        // The scope shows it, opening on a rising zero crossing.
        let scope = shared.preview_scope();
        assert_eq!(scope.len(), SCOPE_SAMPLES);
        assert!(peak(&scope) > 0.05);
        assert!(scope[0] >= 0.0, "{}", scope[0]);

        // A new patch reaches the held note without cutting it, and Stop leaves it alone.
        let SoundPatch::Synth(mut p) = factory_patch("saw") else { unreachable!() };
        p.osc[0].wave = dissonant_core::sound::OscWave::Square;
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::Synth(Box::new(p)))));
        e.handle(EngineCommand::Stop);
        let after = render(&mut e, 2_000, 512);
        assert!(peak(&after[..200]) > 0.05, "updating the patch cut the held note");

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
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_pattern(&model, &pid, 0.0).unwrap())));
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(&factory_patch("sine"), SR))));
        e.handle(EngineCommand::PreviewNoteOn { pitch: 69, velocity: 100 });
        assert!(peak(&render(&mut e, 4_000, 512)) > 0.05, "muting a track must not mute the preview");
        e.handle(EngineCommand::SetPreview(None));
        let tail = render(&mut e, 2_000, 512);
        assert!(peak(&tail[..100]) > 0.01, "closing releases rather than cuts");
        render(&mut e, SR as usize, 512);
        assert!(e.preview.is_none(), "dropped once silent");
        e.handle(EngineCommand::PreviewNoteOn { pitch: 69, velocity: 100 });
        assert!(e.preview.is_none(), "a closed preview plays nothing");
    }

    #[test]
    fn a_drum_preview_plays_on_any_pitch_and_swaps_its_sound() {
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, shared.clone(), &MasterSettings::default());
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(&factory_patch("kick"), SR))));
        e.handle(EngineCommand::PreviewNoteOn { pitch: 60, velocity: 127 });
        assert!(peak(&render(&mut e, 4_000, 512)) > 0.1);
        assert!(peak(&shared.preview_scope()) > 0.05);
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(&factory_patch("hat"), SR))));
        let Some(Instrument::Drums(kit)) = &e.preview else { panic!("still a drum preview") };
        assert_eq!(kit.pad_drum(0).unwrap().samples.len(), (0.06 * SR).round() as usize, "now the hat");
        // A drum rendered at another rate is re-rendered at the engine's.
        e.handle(EngineCommand::SetPreview(Some(PreviewPatch::from_sound(&factory_patch("hat"), 22_050.0))));
        let Some(Instrument::Drums(kit)) = &e.preview else { panic!() };
        assert_eq!(kit.pad_drum(0).unwrap().sample_rate, SR);
    }

    #[test]
    fn a_prerendered_bank_feeds_the_kits_and_unchanged_pads_survive_an_edit() {
        let mut model = ProjectModel::starter();
        let drum = model.tracks.iter().position(|t| t.is_drum).unwrap();
        let seq = Arc::new(Sequence::silent(&model, 4.0));
        let bank = Arc::new(DrumBank::for_sequence(&seq, SR));
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        e.handle(EngineCommand::SetSequenceWithDrums { sequence: seq.clone(), drums: bank.clone() });
        let kit = |e: &Engine| match &e.channels[drum].instrument {
            Instrument::Drums(k) => k.clone(),
            _ => panic!("drum track"),
        };
        let pads = seq.tracks[drum].drum_pads.clone();
        for p in &pads {
            assert!(Arc::ptr_eq(kit(&e).pad_drum(p.pitch).unwrap(), bank.get(&p.patch).unwrap()), "pad {} not from the bank", p.pitch);
        }
        // Swap one pad's sound; every other pad keeps its buffer.
        model.tracks[drum].drum_pads[0].sound = "clap".into();
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::silent(&model, 4.0))));
        let k = kit(&e);
        assert_eq!(k.pad_drum(pads[0].pitch).unwrap().samples.len(), (0.2 * SR).round() as usize, "the clap now");
        for p in &pads[1..] {
            assert!(Arc::ptr_eq(k.pad_drum(p.pitch).unwrap(), bank.get(&p.patch).unwrap()), "pad {} re-rendered", p.pitch);
        }
    }

    #[test]
    fn seek_wraps_into_loop_and_sequence_swap_keeps_channels() {
        let mut e = Engine::new(SR, Arc::new(Shared::default()), &MasterSettings::default());
        let seq = sequence_with_note(0.0, 1.0, 4.0);
        e.handle(EngineCommand::SetSequence(seq.clone()));
        e.handle(EngineCommand::Seek { beat: 9.0 });
        assert_eq!(e.position(), 0.0);
        e.handle(EngineCommand::Seek { beat: 2.5 });
        assert_eq!(e.position(), 2.5);
        e.handle(EngineCommand::SetSequence(seq.clone()));
        assert_eq!(e.track_index().len(), 1);
    }
}
