//! DI → notes and chords: offline analysis of one recorded take.
//!
//! Runs on a worker thread over a whole clip, never on the audio thread, and reads the **raw
//! source** — the samples as they were captured, before the clip's gain and fades and long
//! before the rig. A gate or a drive pedal is a sound choice; the pitch is in the strings.
//!
//! It lives in the engine rather than `dissonant-core` for one reason: it runs the tuner's own
//! detector ([`crate::tuner::Conditioner`] and [`crate::tuner::Yin`]). A second YIN would drift
//! from the first, and a take the tuner calls in tune would transcribe sharp.
//!
//! The pipeline, every stage chosen for being explainable rather than clever:
//!
//! 1. **Onsets** from the high-frequency envelope: the RMS of the first difference over 10 ms
//!    windows every 5 ms. A pick attack is broadband, so the derivative jumps even when the
//!    string it lands on is still ringing at the same pitch — which is what makes a repeated
//!    note two notes instead of one long one. An onset is that envelope doubling across 10 ms
//!    (or sound rising out of silence, for a swell with no attack), with a 50 ms refractory so
//!    the three strings of one strum are one event.
//! 2. **Frames** every 10 ms: YIN (the tuner's) for the pitch, and a 1024-point spectrum at the
//!    tuner's decimated rate (~85 ms) for the two questions YIN cannot answer — "is this one
//!    note?" and "which pitch classes are sounding?". A frame is *mono* when one harmonic comb
//!    explains at least [`HARMONIC_FIT`] of its spectral peak power and that comb's fundamental
//!    is actually present; it is *polyphonic* when no comb does and two or more pitch classes
//!    stand out. Requiring the fundamental is what stops a triad passing as one note: C–E–G is
//!    perfectly explained by a comb on the C an octave below, which nobody played.
//! 3. **Segments** between onsets are voted on by their clean frames (those whose windows do
//!    not reach across an onset). A polyphonic segment is **refused**: it writes no notes, is
//!    reported back, and becomes a chord candidate. A mono segment becomes notes: its pitch
//!    track is split at legato jumps (hammer-ons, pull-offs), except that a run of short steps
//!    is a slide and stays one note. Each note is pitched by where it *starts*, and its contour
//!    relative to that start is what [`dissonant_core::decimate_pitch_envelope`] turns into
//!    the note's pitch envelope, so a bend survives as one bent note.
//! 4. **Chords**: each refused segment's chroma (spectral peaks folded onto pitch classes) is
//!    cosine-matched against the triads `dissonant-core` names on the chord track — major,
//!    minor, diminished, augmented on every root. Adjacent strums of the same chord merge.
//!    The winning similarity is the confidence the lane shows; nothing below
//!    [`MIN_CHORD_CONFIDENCE`] is offered at all.
//! 5. **Velocity** from the RMS just after each onset, in dB relative to the loudest note: input
//!    gain says nothing about how hard a string was picked, the differences between notes do.

use crate::audio::AudioBuffer;
use crate::tuner::{Conditioner, Pitch, Yin, HALF_WINDOW};
use dissonant_core::harmony::{note_name, TriadQuality};
use dissonant_core::{
    decimate_pitch_envelope, tidy_pitch_envelope, ClipDetection, DetectedChord, NoteEvent,
    TempoMap, MAX_FINE_CENTS, PITCH_DECIMATE_CENTS,
};
use uuid::Uuid;

/// Pitch and spectrum frames, in seconds. Fine enough for a fast bend; RDP throws away
/// whatever detail the contour does not need anyway.
const FRAME_SECONDS: f64 = 0.01;
/// Onset envelope hop; each window is two hops long.
const ONSET_HOP_SECONDS: f64 = 0.005;
/// The lowest fundamental taken as a note: a seven-string's low B, with a little room. A
/// lower YIN answer on a guitar DI is the virtual root of a power chord, not a string.
const MIN_HZ: f32 = 58.0;
/// The highest: a 24th-fret high E is 1319 Hz.
const MAX_HZ: f32 = 1_400.0;
/// YIN readings less periodic than this are not trusted as a note's pitch.
const MIN_CLARITY: f32 = 0.75;
/// Frames this far under the loudest one are silence (the tail of a decaying string).
const SILENCE_DB: f32 = -40.0;
/// Absolute floor under which nothing is a note, whatever the take's level.
const ABS_FLOOR: f32 = 1e-4;
/// HF envelope growth across 10 ms that counts as a pick attack.
const ONSET_RISE: f32 = 2.0;
/// One onset per this long: a strum's strings land within it.
const REFRACTORY_SECONDS: f64 = 0.05;
/// Spectrum size at the decimated (~12 kHz) rate: ~85 ms, 11.7 Hz bins, interpolated.
const FFT_LEN: usize = 1024;
/// Share of spectral peak power one harmonic comb must explain for a frame to be one note.
pub const HARMONIC_FIT: f32 = 0.7;
/// How far off a harmonic a partial may sit and still belong to it (string stiffness pushes
/// upper partials sharp, the interpolated peak is good to a few cents).
const HARMONIC_CENTS: f32 = 45.0;
/// Peaks quieter than this under the frame's loudest are ignored altogether.
const PEAK_FLOOR_DB: f32 = -35.0;
/// A comb's fundamental must be at least this loud, relative to the loudest peak.
const FUNDAMENTAL_DB: f32 = -25.0;
/// Peaks this loud count as "a pitch class standing out" for the polyphony test.
const STRONG_DB: f32 = -20.0;
/// Chroma is folded from peaks in this band: guitar fundamentals and the first few partials.
const CHROMA_LO_HZ: f32 = 58.0;
const CHROMA_HI_HZ: f32 = 1_100.0;
/// YIN's window reaches half its length either side of a frame; frames closer than this to an
/// onset hear the previous note too.
const SETTLE_SECONDS: f64 = 0.02;
/// The spectrum's half window: frames at least this far inside a segment are clean.
const CLEAN_SECONDS: f64 = 0.045;
/// A pitch step this large between two frames, with no onset, is a legato note change.
const LEGATO_JUMP: f32 = 0.8;
/// A legato step shorter than this is passing through, not landing: a slide.
const MIN_LEGATO_SECONDS: f64 = 0.06;
/// Voiced frames further apart than this are separate notes, onset or not.
const MAX_VOICED_GAP: f64 = 0.05;
/// Shorter than this is a glitch, not a note.
const MIN_NOTE_SECONDS: f64 = 0.04;
/// Below this similarity a refused region offers no chord at all.
pub const MIN_CHORD_CONFIDENCE: f32 = 0.7;
/// Within one strum, chroma bins under this share of the loudest are harmonic leakage (the
/// fifth every string's third partial adds), not strings.
const STRUM_FLOOR: f32 = 0.3;

/// One note heard in the take, in seconds from the start of the analysed stretch.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardNote {
    pub start: f64,
    pub end: f64,
    /// MIDI note number of where the note *starts* (a bend is tiered by its unbent pitch).
    pub pitch: i32,
    pub velocity: i32,
    /// `(seconds after start, cents from the starting pitch)`, one per voiced frame.
    pub contour: Vec<(f64, f32)>,
}

/// A chord heard in a refused region.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardChord {
    pub start: f64,
    pub end: f64,
    /// Root first: `[root, third, fifth]`.
    pub pitch_classes: Vec<i32>,
    pub name: String,
    /// Cosine similarity of the chroma to the chord's template, 0…1.
    pub confidence: f32,
}

/// Everything one pass over a take heard.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hearing {
    pub notes: Vec<HeardNote>,
    /// Regions judged polyphonic, `(start, end)` in seconds: no notes were written there.
    pub polyphonic: Vec<(f64, f64)>,
    pub chords: Vec<HeardChord>,
    /// Pitch-class weight of the refused regions in note equivalents (see
    /// [`dissonant_core::ClipDetection::chroma`]).
    pub chroma: [f32; 12],
}

/// The stretch of `buffer` a clip plays, mixed to mono. Raw: no gain, no fades — those shape
/// playback, not the performance.
pub fn clip_mono(buffer: &AudioBuffer, offset_seconds: f64, length_seconds: f64) -> Vec<f32> {
    let rate = buffer.sample_rate.max(1) as f64;
    let ch = buffer.channels.max(1) as usize;
    let frames = buffer.frames.min(buffer.data.len() / ch);
    let first = if offset_seconds.is_finite() {
        ((offset_seconds.max(0.0) * rate).round() as usize).min(frames)
    } else {
        0
    };
    let len = if length_seconds.is_finite() {
        (length_seconds.max(0.0) * rate).round() as usize
    } else {
        0
    };
    let last = first.saturating_add(len).min(frames);
    let scale = 1.0 / ch as f32;
    (first..last)
        .map(|f| buffer.data[f * ch..(f + 1) * ch].iter().sum::<f32>() * scale)
        .collect()
}

/// What a frame sounded like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Silent,
    /// One harmonic comb with YIN agreeing on its pitch.
    Mono,
    /// One harmonic comb, but YIN gave no usable pitch (an attack, a weak window).
    MonoUnpitched,
    Poly,
    /// Loud, but neither: noise, a scrape, a mute.
    Unclear,
}

#[derive(Debug, Clone)]
struct Frame {
    t: f64,
    kind: Kind,
    pitch: Option<Pitch>,
    chroma: [f32; 12],
}

/// One spectral peak: interpolated frequency and the bin's power.
#[derive(Debug, Clone, Copy)]
struct Peak {
    hz: f32,
    power: f32,
}

/// Analyse one mono take. `sample_rate` is the take's own; anything not a plausible rate is
/// read as 48 kHz rather than refused.
pub fn hear(mono: &[f32], sample_rate: f32) -> Hearing {
    let sr = if sample_rate.is_finite() && sample_rate > 1_000.0 {
        sample_rate
    } else {
        48_000.0
    };
    if mono.is_empty() {
        return Hearing::default();
    }
    let duration = mono.len() as f64 / sr as f64;

    // The tuner's front end over the whole take, padded with silence so the first and last
    // notes get whole windows.
    let mut cond = Conditioner::new(sr);
    let d = cond.decimation();
    let low_rate = sr / d as f32;
    let pad_low = HALF_WINDOW.max(FFT_LEN / 2) + 1;
    let pad_full = pad_low * d;
    let mut full = vec![0.0f32; pad_full];
    full.reserve(mono.len() + pad_full);
    let mut low = vec![0.0f32; pad_low];
    low.reserve(mono.len() / d + pad_low + 2);
    for &s in mono {
        let (x, y) = cond.push(s);
        full.push(x);
        if let Some(y) = y {
            low.push(y);
        }
    }
    full.resize(full.len() + pad_full, 0.0);
    low.resize(low.len() + pad_low + 1, 0.0);

    // ── Onsets ───────────────────────────────────────────────────────────────────────────
    let hop_o = ((sr as f64 * ONSET_HOP_SECONDS).round() as usize).max(1);
    let n_o = mono.len() / hop_o + 1;
    let mut rms = vec![0.0f32; n_o];
    let mut hf = vec![0.0f32; n_o];
    for k in 0..n_o {
        let a = pad_full + k * hop_o;
        let b = (a + 2 * hop_o).min(pad_full + mono.len()).max(a + 1);
        let (mut e, mut h) = (0.0f32, 0.0f32);
        for i in a..b {
            e += full[i] * full[i];
            let dd = full[i] - full[i - 1];
            h += dd * dd;
        }
        let n = (b - a) as f32;
        rms[k] = (e / n).sqrt();
        hf[k] = (h / n).sqrt();
    }
    let loudest = rms.iter().fold(0.0f32, |m, &v| m.max(v));
    let floor = (loudest * db(SILENCE_DB)).max(ABS_FLOOR);
    if loudest < ABS_FLOOR {
        return Hearing::default();
    }
    let mut onsets: Vec<f64> = Vec::new();
    for k in 0..n_o {
        if rms[k] < floor {
            continue;
        }
        let before = if k >= 2 { hf[k - 2] } else { 0.0 };
        // A swell has no attack to find, but sound out of silence is still a new note.
        let from_silence = k > 0 && rms[k - 1] < floor;
        if hf[k] <= ONSET_RISE * before.max(1e-9) && !from_silence {
            continue;
        }
        // The attack is somewhere in this 10 ms window; its middle is the honest guess.
        let t = (k * hop_o) as f64 / sr as f64 + ONSET_HOP_SECONDS * 0.5;
        if onsets.last().is_some_and(|&last| t - last < REFRACTORY_SECONDS) {
            continue;
        }
        onsets.push(t);
    }
    // A take that starts mid-note has sound before its first attack.
    if rms[0] >= floor && onsets.first().is_none_or(|&t| t > 0.03) {
        onsets.insert(0, 0.0);
    }
    let rms_at = |t: f64| rms[((t * sr as f64) as usize / hop_o).min(n_o - 1)];

    // ── Frames ───────────────────────────────────────────────────────────────────────────
    let hop_p = ((sr as f64 * FRAME_SECONDS).round() as usize).max(1);
    let n_p = mono.len() / hop_p + 1;
    let mut yin = Yin::new();
    let mut fft = Fft::new(FFT_LEN);
    let mut frames = Vec::with_capacity(n_p);
    for j in 0..n_p {
        let c = j * hop_p;
        let t = c as f64 / sr as f64;
        let loud = rms_at(t) >= floor;
        if !loud {
            frames.push(Frame {
                t,
                kind: Kind::Silent,
                pitch: None,
                chroma: [0.0; 12],
            });
            continue;
        }
        let lc = pad_low + c / d;
        let yin_low = &low[lc - HALF_WINDOW..lc + HALF_WINDOW];
        let yin_full = &full[(lc - HALF_WINDOW) * d..(lc + HALF_WINDOW) * d];
        let pitch = yin
            .search(yin_low, yin_full, d, sr)
            .filter(|p| (MIN_HZ..=MAX_HZ).contains(&p.hz) && p.clarity >= MIN_CLARITY);
        let peaks = fft.peaks(&low[lc - FFT_LEN / 2..lc + FFT_LEN / 2], low_rate);
        let kind = classify(&peaks, pitch);
        frames.push(Frame {
            t,
            kind,
            pitch: if kind == Kind::Mono { pitch } else { None },
            chroma: chroma_of(&peaks),
        });
    }

    // ── Segments ─────────────────────────────────────────────────────────────────────────
    let mut notes: Vec<(HeardNote, f32)> = Vec::new();
    let mut refused: Vec<(f64, f64, [f32; 12])> = Vec::new();
    for (i, &on) in onsets.iter().enumerate() {
        let next = onsets.get(i + 1).copied();
        let until = next.unwrap_or(duration);
        let in_seg: Vec<&Frame> = frames
            .iter()
            .filter(|f| f.t >= on && f.t < until && f.kind != Kind::Silent)
            .collect();
        let Some(last_loud) = in_seg.last() else {
            continue;
        };
        let end = (last_loud.t + FRAME_SECONDS * 0.5).min(until);
        // Frames whose windows reach across the next onset hear the next note.
        let tail_guard = if next.is_some() { CLEAN_SECONDS } else { 0.0 };
        let clean: Vec<&Frame> = in_seg
            .iter()
            .copied()
            .filter(|f| f.t >= on + CLEAN_SECONDS && f.t <= until - tail_guard)
            .collect();
        let (mono_votes, poly_votes) = if clean.is_empty() {
            // Too short for a clean spectrum: YIN alone decides, and it can only say "mono".
            let voiced = in_seg
                .iter()
                .filter(|f| f.t >= on + SETTLE_SECONDS && f.pitch.is_some())
                .count();
            (voiced, 0)
        } else {
            let m = clean
                .iter()
                .filter(|f| matches!(f.kind, Kind::Mono | Kind::MonoUnpitched))
                .count();
            let p = clean.iter().filter(|f| f.kind == Kind::Poly).count();
            (m, p)
        };
        if poly_votes >= 2 && poly_votes > mono_votes {
            let from = if clean.is_empty() { &in_seg } else { &clean };
            let mut chroma = [0.0f32; 12];
            for f in from.iter() {
                for (acc, v) in chroma.iter_mut().zip(f.chroma) {
                    *acc += v;
                }
            }
            refused.push((on, end, chroma));
            continue;
        }
        if mono_votes == 0 {
            continue;
        }
        let voiced: Vec<(f64, f32)> = in_seg
            .iter()
            .filter(|f| f.t >= on + SETTLE_SECONDS && f.t <= until - SETTLE_SECONDS.min(tail_guard))
            .filter_map(|f| f.pitch.map(|p| (f.t, midi_of(p.hz))))
            .collect();
        for note in segment_notes(on, end, &voiced) {
            let energy = (0..10)
                .map(|k| rms_at(note.start + k as f64 * ONSET_HOP_SECONDS))
                .fold(0.0f32, f32::max);
            notes.push((note, energy));
        }
    }

    // ── Velocity ─────────────────────────────────────────────────────────────────────────
    let top = notes.iter().map(|(_, e)| *e).fold(0.0f32, f32::max).max(1e-9);
    let notes: Vec<HeardNote> = notes
        .into_iter()
        .map(|(mut n, e)| {
            let rel = 20.0 * (e.max(1e-9) / top).log10();
            n.velocity = (112.0 + 2.5 * rel).round().clamp(20.0, 127.0) as i32;
            n
        })
        .collect();

    // ── Refused regions, chords, key chroma ──────────────────────────────────────────────
    let mut polyphonic: Vec<(f64, f64)> = Vec::new();
    let mut chords: Vec<HeardChord> = Vec::new();
    let mut key_chroma = [0.0f32; 12];
    for &(start, end, chroma) in &refused {
        match polyphonic.last_mut() {
            Some(last) if start - last.1 < 0.1 => last.1 = end,
            _ => polyphonic.push((start, end)),
        }
        let top = chroma.iter().fold(0.0f32, |m, &v| m.max(v));
        if top > 0.0 {
            for (acc, &v) in key_chroma.iter_mut().zip(&chroma) {
                let share = v / top;
                if share >= STRUM_FLOOR {
                    *acc += share;
                }
            }
        }
        let Some((pcs, name, confidence)) = match_chord(&chroma) else {
            continue;
        };
        match chords.last_mut() {
            Some(last) if last.pitch_classes == pcs && start - last.end < 0.1 => {
                let (a, b) = ((last.end - last.start) as f32, (end - start) as f32);
                last.confidence = (last.confidence * a + confidence * b) / (a + b).max(1e-9);
                last.end = end;
            }
            _ => chords.push(HeardChord {
                start,
                end,
                pitch_classes: pcs,
                name,
                confidence,
            }),
        }
    }
    chords.retain(|c| c.confidence >= MIN_CHORD_CONFIDENCE);

    Hearing {
        notes,
        polyphonic,
        chords,
        chroma: key_chroma,
    }
}

/// A mono segment's voiced frames, `(seconds, fractional MIDI pitch)`, into notes. `on` is the
/// pick attack that opened the segment and `end` where it stopped sounding.
fn segment_notes(on: f64, end: f64, voiced: &[(f64, f32)]) -> Vec<HeardNote> {
    if voiced.is_empty() {
        return vec![];
    }
    // Three-point median: one YIN glitch (an octave slip on a weak window) is not a note.
    let smooth: Vec<(f64, f32)> = (0..voiced.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(voiced.len() - 1);
            let mut w: Vec<f32> = voiced[lo..=hi].iter().map(|v| v.1).collect();
            w.sort_by(f32::total_cmp);
            (voiced[i].0, w[w.len() / 2])
        })
        .collect();

    // Runs of continuous pitch: a gap or a jump starts the next one.
    struct Run {
        from: usize,
        to: usize,
        after_gap: bool,
    }
    let mut runs: Vec<Run> = vec![Run {
        from: 0,
        to: 0,
        after_gap: false,
    }];
    for i in 1..smooth.len() {
        let gap = smooth[i].0 - smooth[i - 1].0 > MAX_VOICED_GAP;
        let jump = (smooth[i].1 - smooth[i - 1].1).abs() >= LEGATO_JUMP;
        if gap || jump {
            runs.push(Run {
                from: i,
                to: i,
                after_gap: gap,
            });
        } else {
            runs.last_mut().unwrap().to = i;
        }
    }

    // Runs into notes. A legato jump onto a run that holds is a new note; a run too short to
    // hold is a fret passing under a sliding finger, and it — and the run the slide lands
    // on — stay in the note the slide started from.
    let half = FRAME_SECONDS * 0.5;
    let mut spans: Vec<(f64, usize, usize, f64)> = Vec::new(); // (start, from, to, end)
    let mut sliding = false;
    for (r, run) in runs.iter().enumerate() {
        let dur = smooth[run.to].0 - smooth[run.from].0 + FRAME_SECONDS;
        if r == 0 {
            spans.push((on, run.from, run.to, smooth[run.to].0 + half));
            continue;
        }
        let cur = spans.last_mut().unwrap();
        if run.after_gap {
            sliding = false;
            spans.push((smooth[run.from].0 - half, run.from, run.to, smooth[run.to].0 + half));
        } else if dur < MIN_LEGATO_SECONDS || sliding {
            sliding = dur < MIN_LEGATO_SECONDS;
            cur.2 = run.to;
            cur.3 = smooth[run.to].0 + half;
        } else {
            let at = smooth[run.from].0 - half;
            cur.3 = at;
            spans.push((at, run.from, run.to, smooth[run.to].0 + half));
        }
    }
    // A note that rang right up to where the segment stopped sounding ends there, not at its
    // last trusted frame.
    if let Some(last) = spans.last_mut() {
        if end - last.3 < MAX_VOICED_GAP {
            last.3 = end;
        }
    }

    spans
        .into_iter()
        .filter(|&(start, _, _, stop)| stop - start >= MIN_NOTE_SECONDS)
        .map(|(start, from, to, stop)| {
            let frames = &smooth[from..=to];
            // Where the note starts: the median of its first few frames, so the sharp pitch
            // of a hard attack does not get to name it.
            let mut head: Vec<f32> = frames.iter().take(5).map(|f| f.1).collect();
            head.sort_by(f32::total_cmp);
            let reference = head[head.len() / 2];
            let limit = MAX_FINE_CENTS as f32;
            let contour: Vec<(f64, f32)> = frames
                .iter()
                .map(|&(t, m)| ((t - start).max(0.0), ((m - reference) * 100.0).clamp(-limit, limit)))
                .collect();
            HeardNote {
                start,
                end: stop,
                pitch: (reference.round() as i32).clamp(0, 127),
                velocity: 100,
                contour,
            }
        })
        .collect()
}

/// What one frame's spectrum (and YIN's pitch for it) says it is.
fn classify(peaks: &[Peak], pitch: Option<Pitch>) -> Kind {
    if peaks.is_empty() {
        return Kind::Unclear;
    }
    if let Some(p) = pitch {
        if harmonic_fit(peaks, p.hz) >= HARMONIC_FIT {
            return Kind::Mono;
        }
    }
    let top = peaks.iter().fold(0.0f32, |m, p| m.max(p.power));
    // YIN said nothing usable: try the lowest strong peaks as the fundamental themselves.
    let strong = top * power_db(FUNDAMENTAL_DB);
    let best = peaks
        .iter()
        .filter(|p| p.power >= strong && p.hz >= MIN_HZ)
        .take(4)
        .map(|p| harmonic_fit(peaks, p.hz))
        .fold(0.0f32, f32::max);
    if best >= HARMONIC_FIT {
        return Kind::MonoUnpitched;
    }
    let loud = top * power_db(STRONG_DB);
    let mut classes = [false; 12];
    for p in peaks
        .iter()
        .filter(|p| p.power >= loud && (CHROMA_LO_HZ..=CHROMA_HI_HZ).contains(&p.hz))
    {
        classes[pitch_class_of(p.hz)] = true;
    }
    if classes.iter().filter(|&&c| c).count() >= 2 {
        Kind::Poly
    } else {
        Kind::Unclear
    }
}

/// Share of peak power explained by the harmonic comb on `f0`, or 0 when the comb's own
/// fundamental is missing.
fn harmonic_fit(peaks: &[Peak], f0: f32) -> f32 {
    let top = peaks.iter().fold(0.0f32, |m, p| m.max(p.power));
    let total: f32 = peaks.iter().map(|p| p.power).sum();
    if total <= 0.0 || f0 <= 0.0 {
        return 0.0;
    }
    let near = |hz: f32, k: f32| (1200.0 * (hz / (k * f0)).log2()).abs() <= HARMONIC_CENTS;
    let has_fundamental = peaks
        .iter()
        .any(|p| p.power >= top * power_db(FUNDAMENTAL_DB) && near(p.hz, 1.0));
    if !has_fundamental {
        return 0.0;
    }
    let explained: f32 = peaks
        .iter()
        .filter(|p| {
            let k = (p.hz / f0).round();
            k >= 1.0 && near(p.hz, k)
        })
        .map(|p| p.power)
        .sum();
    explained / total
}

/// Peak magnitudes folded onto pitch classes, over the band where guitar notes live.
fn chroma_of(peaks: &[Peak]) -> [f32; 12] {
    let mut out = [0.0f32; 12];
    for p in peaks.iter().filter(|p| (CHROMA_LO_HZ..=CHROMA_HI_HZ).contains(&p.hz)) {
        out[pitch_class_of(p.hz)] += p.power.sqrt();
    }
    out
}

/// The triad whose template a chroma vector is closest to: `(pitch classes root first, name,
/// cosine similarity)`. `None` for an empty vector.
fn match_chord(chroma: &[f32; 12]) -> Option<(Vec<i32>, String, f32)> {
    let norm = chroma.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= 0.0 {
        return None;
    }
    // The qualities the chord track names (`harmony::chord_name`), commonest first so a tie
    // goes to the chord a guitarist more likely played. Augmented is symmetric: four roots
    // cover all twelve.
    const QUALITIES: [(TriadQuality, i32, i32, i32); 4] = [
        (TriadQuality::Major, 4, 7, 12),
        (TriadQuality::Minor, 3, 7, 12),
        (TriadQuality::Diminished, 3, 6, 12),
        (TriadQuality::Augmented, 4, 8, 4),
    ];
    let mut best: Option<(Vec<i32>, String, f32)> = None;
    for (quality, third, fifth, roots) in QUALITIES {
        for root in 0..roots {
            let pcs = [root, (root + third) % 12, (root + fifth) % 12];
            let dot: f32 = pcs.iter().map(|&pc| chroma[pc as usize]).sum();
            let cos = dot / (norm * 3.0f32.sqrt());
            if best.as_ref().is_none_or(|b| cos > b.2) {
                best = Some((
                    pcs.to_vec(),
                    format!("{}{}", note_name(root), quality.suffix()),
                    cos.clamp(0.0, 1.0),
                ));
            }
        }
    }
    best
}

fn midi_of(hz: f32) -> f32 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

fn pitch_class_of(hz: f32) -> usize {
    (midi_of(hz).round() as i32).rem_euclid(12) as usize
}

/// An amplitude ratio from decibels.
fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

/// A power ratio from decibels: peak powers are compared with this, levels with [`db`].
fn power_db(v: f32) -> f32 {
    10f32.powf(v / 10.0)
}

/// A radix-2 FFT with its own Hann window and scratch, for peak picking only.
struct Fft {
    n: usize,
    window: Vec<f32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        debug_assert!(n.is_power_of_two());
        let bits = n.trailing_zeros();
        let tau = 2.0 * std::f32::consts::PI;
        Fft {
            n,
            window: (0..n)
                .map(|i| 0.5 - 0.5 * (tau * i as f32 / n as f32).cos())
                .collect(),
            cos: (0..n / 2).map(|i| (tau * i as f32 / n as f32).cos()).collect(),
            sin: (0..n / 2).map(|i| (tau * i as f32 / n as f32).sin()).collect(),
            rev: (0..n)
                .map(|i| i.reverse_bits() >> (usize::BITS - bits))
                .collect(),
            re: vec![0.0; n],
            im: vec![0.0; n],
        }
    }

    /// Local maxima of the power spectrum of `input` (exactly `n` samples at `rate`), no more
    /// than [`PEAK_FLOOR_DB`] under the loudest, between 50 Hz and 4 kHz, sorted by frequency.
    /// Frequencies are interpolated on the log spectrum, which is what lets 11.7 Hz bins
    /// still name the low E.
    fn peaks(&mut self, input: &[f32], rate: f32) -> Vec<Peak> {
        let n = self.n;
        for (i, &x) in input.iter().enumerate().take(n) {
            self.re[self.rev[i]] = x * self.window[i];
            self.im[self.rev[i]] = 0.0;
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            let half = len / 2;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], -self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = self.re[b] * wr - self.im[b] * wi;
                    let ti = self.re[b] * wi + self.im[b] * wr;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            len <<= 1;
        }
        let bin_hz = rate / n as f32;
        let lo = ((50.0 / bin_hz).floor() as usize).max(1);
        let hi = ((4_000.0f32.min(rate * 0.45) / bin_hz).ceil() as usize).min(n / 2 - 1);
        let power = |k: usize| self.re[k] * self.re[k] + self.im[k] * self.im[k];
        let top = (lo..=hi).map(power).fold(0.0f32, f32::max);
        if top <= 0.0 {
            return vec![];
        }
        let floor = top * power_db(PEAK_FLOOR_DB);
        let mut out = Vec::new();
        for k in lo..=hi {
            let p = power(k);
            if p < floor || p <= power(k - 1) || p < power(k + 1) {
                continue;
            }
            let (a, b, c) = (
                power(k - 1).max(1e-30).ln(),
                p.max(1e-30).ln(),
                power(k + 1).max(1e-30).ln(),
            );
            let denom = a - 2.0 * b + c;
            let delta = if denom.abs() > 1e-12 {
                (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            out.push(Peak {
                hz: (k as f32 + delta) * bin_hz,
                power: p,
            });
        }
        out
    }
}

/// A [`Hearing`] laid onto the song, ready for `Command::AddTranscription`.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    /// Pattern ticks: 0 is the clip's start.
    pub notes: Vec<NoteEvent>,
    /// The clip's span on the song, in ticks.
    pub length_ticks: i64,
    pub detection: ClipDetection,
    /// The refused regions, in song ticks.
    pub refused: Vec<(i64, i64)>,
}

/// Convert a hearing of the clip `clip_id` — which starts at song tick `start_tick` and plays
/// `length_seconds` — from seconds into ticks through the song's tempo map. Each note's
/// contour is decimated into its pitch envelope here, in ticks, because that is the unit the
/// envelope is stored and tolerance-checked in. A contour that never strays more than the
/// decimation tolerance from the note's starting pitch is no envelope at all.
pub fn place_on_song(
    hearing: &Hearing,
    clip_id: Uuid,
    start_tick: i64,
    length_seconds: f64,
    map: &TempoMap,
) -> Transcription {
    let start_tick = start_tick.max(0);
    let s0 = map.seconds_at_tick(start_tick as f64);
    let tick = |t: f64| map.tick_at_seconds(s0 + t.max(0.0)).round() as i64;
    let length_ticks = (tick(length_seconds.max(0.0)) - start_tick).max(1);
    let notes = hearing
        .notes
        .iter()
        .filter_map(|h| {
            let at = tick(h.start);
            let len = tick(h.end) - at;
            if len <= 0 {
                return None;
            }
            let mut n = NoteEvent::new(at - start_tick, len, h.pitch);
            n.velocity = h.velocity.clamp(1, 127);
            let bends = h.contour.iter().any(|&(_, c)| c.abs() > PITCH_DECIMATE_CENTS);
            if bends {
                let dense: Vec<(i64, f32)> = h
                    .contour
                    .iter()
                    .map(|&(dt, c)| ((tick(h.start + dt) - at).clamp(0, len), c))
                    .collect();
                n.pitch_envelope =
                    tidy_pitch_envelope(decimate_pitch_envelope(&dense, PITCH_DECIMATE_CENTS));
            }
            Some(n)
        })
        .collect();
    let chords = hearing
        .chords
        .iter()
        .map(|c| DetectedChord {
            id: Uuid::new_v4(),
            start_tick: tick(c.start),
            length_ticks: (tick(c.end) - tick(c.start)).max(1),
            pitch_classes: c.pitch_classes.clone(),
            name: Some(c.name.clone()),
            confidence: c.confidence,
        })
        .collect();
    Transcription {
        notes,
        length_ticks,
        detection: ClipDetection {
            clip_id,
            chords,
            chroma: hearing.chroma,
        },
        refused: hearing
            .polyphonic
            .iter()
            .map(|&(a, b)| (tick(a), tick(b)))
            .collect(),
    }
}
