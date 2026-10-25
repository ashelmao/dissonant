//! Behavioural audit of the realtime engine and mixer.
//!
//! Everything here is offline: either `render::render_to_buffer` or a hand-driven
//! `Engine::process` loop. No audio device, no sleeps, no randomness.
//!
//! Spectral claims are measured with a Goertzel bin rather than eyeballed from samples.

use std::f64::consts::PI;
use std::sync::Arc;

use dissonant_core::{MasterSettings, NoteEvent, ProjectModel, Sequence, SongPattern};
use dissonant_engine::engine::{Engine, EngineCommand, Shared};
use dissonant_engine::render::{render_to_buffer, RenderOptions};

const SR: f32 = 44_100.0;
const SRD: f64 = 44_100.0;

// ─── helpers ─────────────────────────────────────────────────────────────────────────────────

fn model(voice: &str, length_beats: f64) -> ProjectModel {
    let mut m = ProjectModel::empty();
    m.tracks[0].voice = voice.to_string();
    m.patterns[0].length_beats = length_beats;
    m
}

/// Put `notes` in the track's own pattern, making one (as long as the first) on first use.
fn set_notes(m: &mut ProjectModel, track: usize, notes: Vec<NoteEvent>) {
    let id = m.tracks[track].id;
    let length = m.patterns[0].length_beats;
    match m.patterns.iter_mut().find(|p| p.track_id == id) {
        Some(p) => p.notes = notes,
        None => {
            let mut p = SongPattern::new(id, "p");
            p.length_beats = length;
            p.notes = notes;
            m.patterns.push(p);
        }
    }
}

/// The first pattern's loop with every track's own pattern playing along at pattern-local
/// beats, as if each were placed at beat 0 (so multi-track mixes can be measured).
fn sequence(m: &ProjectModel) -> Arc<Sequence> {
    let mut seq = Sequence::from_pattern(m, &m.patterns[0].id, 0.0).expect("pattern exists");
    for t in &mut seq.tracks {
        if let Some(p) = m.patterns.iter().find(|p| p.track_id == t.id) {
            t.notes = p.notes.clone();
        }
    }
    Arc::new(seq)
}

/// Offline render of the model's first pattern. Returns de-interleaved (left, right).
fn render(m: &ProjectModel, tail_seconds: f64) -> (Vec<f32>, Vec<f32>) {
    let opts = RenderOptions {
        sample_rate: SR as u32,
        tail_seconds,
        master: m.master.clone(),
        // Measurements below compare raw sample values, so no normalising, dither or extra loops.
        ..RenderOptions::default()
    };
    let inter = render_to_buffer(sequence(m), &opts).expect("render");
    split(&inter)
}

fn split(interleaved: &[f32]) -> (Vec<f32>, Vec<f32>) {
    (
        interleaved.iter().step_by(2).copied().collect(),
        interleaved.iter().skip(1).step_by(2).copied().collect(),
    )
}

/// Drive the engine by hand (looping stays on) and return de-interleaved stereo.
fn run(engine: &mut Engine, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut out = Vec::with_capacity(frames * 2);
    let mut buf = vec![0.0f32; block * 2];
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(block);
        engine.process(&mut buf[..n * 2]);
        out.extend_from_slice(&buf[..n * 2]);
        done += n;
    }
    split(&out)
}

fn engine_with(m: &ProjectModel) -> (Engine, Arc<Shared>) {
    let shared = Arc::new(Shared::default());
    let mut e = Engine::new(SR, Arc::clone(&shared), &m.master);
    e.handle(EngineCommand::SetSequence(sequence(m)));
    (e, shared)
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |a, b| a.max(b.abs()))
}

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Amplitude of the sinusoidal component nearest `freq`, via Goertzel.
fn tone_amplitude(x: &[f32], freq: f64) -> f64 {
    let n = x.len() as f64;
    if n < 8.0 {
        return 0.0;
    }
    let k = (freq / SRD * n).round().max(1.0);
    let w = 2.0 * PI * k / n;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s = v as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let re = s1 - s2 * w.cos();
    let im = s2 * w.sin();
    2.0 * (re * re + im * im).sqrt() / n
}

fn secs(s: f64) -> usize {
    (s * SRD) as usize
}

/// Largest sample-to-sample step inside a window — a proxy for an audible click.
fn max_slew(x: &[f32]) -> f32 {
    x.windows(2).fold(0.0f32, |a, w| a.max((w[1] - w[0]).abs()))
}

fn midi_hz(p: i32) -> f64 {
    440.0 * 2f64.powf((p as f64 - 69.0) / 12.0)
}

fn assert_finite(x: &[f32], what: &str) {
    for (i, s) in x.iter().enumerate() {
        assert!(s.is_finite(), "{what}: sample {i} is {s}");
        assert!(s.abs() <= 1.0, "{what}: sample {i} = {s} escapes [-1, 1]");
    }
}

// ─── 1. pan ──────────────────────────────────────────────────────────────────────────────────

fn panned_peaks(pan: f64) -> (f32, f32) {
    let mut m = model("sine", 2.0);
    m.tracks[0].pan = pan;
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 1.0, 69)]);
    let (l, r) = render(&m, 0.5);
    (peak(&l), peak(&r))
}

#[test]
fn pan_moves_energy_between_channels() {
    let (hard_l_left, hard_l_right) = panned_peaks(-1.0);
    let (center_l, center_r) = panned_peaks(0.0);
    let (hard_r_left, hard_r_right) = panned_peaks(1.0);

    assert!(hard_l_left > 0.01, "pan -1 produced no left signal: {hard_l_left}");
    assert!(
        hard_l_right < hard_l_left * 1e-3,
        "pan -1 leaked into the right channel: l={hard_l_left} r={hard_l_right}"
    );
    assert!(hard_r_right > 0.01, "pan +1 produced no right signal: {hard_r_right}");
    assert!(
        hard_r_left < hard_r_right * 1e-3,
        "pan +1 leaked into the left channel: l={hard_r_left} r={hard_r_right}"
    );

    // Centre must be balanced...
    assert!(
        (center_l - center_r).abs() < center_l * 1e-3,
        "centre pan is not balanced: l={center_l} r={center_r}"
    );
    // ...and constant power: -3 dB per side relative to a hard-panned side.
    let ratio = center_l / hard_l_left;
    assert!(
        (ratio - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02,
        "centre/hard ratio {ratio} is not the constant-power -3 dB ({})",
        std::f32::consts::FRAC_1_SQRT_2
    );
}

#[test]
fn pan_is_monotonic_across_the_sweep() {
    let mut last_bias = f32::NEG_INFINITY;
    for pan in [-1.0, -0.5, 0.0, 0.5, 1.0] {
        let (l, r) = panned_peaks(pan);
        let bias = r - l;
        assert!(
            bias > last_bias,
            "pan {pan} did not move energy further right (bias {bias} <= {last_bias})"
        );
        last_bias = bias;
    }
}

// ─── 2. per-track volume / tone / reverb send ────────────────────────────────────────────────

#[test]
fn track_volume_scales_the_output() {
    let build = |vol: f64| {
        let mut m = model("sine", 2.0);
        m.tracks[0].volume = vol;
        set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 1.0, 69)]);
        let (l, _) = render(&m, 0.5);
        peak(&l)
    };
    let full = build(1.0);
    let half = build(0.5);
    let zero = build(0.0);
    assert!(full > 0.05, "unity volume is inaudible: {full}");
    assert!(
        ((half / full) - 0.5).abs() < 0.02,
        "volume 0.5 gave {half} against {full} (ratio {})",
        half / full
    );
    assert!(zero < 1e-6, "volume 0 still produced {zero}");
}

#[test]
fn track_tone_lowpasses_a_bright_voice() {
    // A 220 Hz saw: fundamental well under the cutoff, 8th harmonic well over it.
    let build = |tone: f64| {
        let mut m = model("saw", 2.0);
        m.tracks[0].tone = tone;
        set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 1.0, 57)]);
        let (l, _) = render(&m, 0.5);
        // Measure inside the sustain, away from the attack transient.
        let w = l[secs(0.10)..secs(0.40)].to_vec();
        (tone_amplitude(&w, 220.0), tone_amplitude(&w, 1760.0))
    };
    let (open_f0, open_h8) = build(18_000.0);
    let (dark_f0, dark_h8) = build(300.0);

    assert!(open_h8 > 0.0, "bright render has no 8th harmonic");
    assert!(
        dark_f0 / open_f0 > 0.6,
        "tone 300 Hz crushed the 220 Hz fundamental too ({dark_f0} vs {open_f0}) — that is a volume knob, not a tone knob"
    );
    assert!(
        dark_h8 / open_h8 < 0.1,
        "tone 300 Hz left {:.1}% of the 1760 Hz energy — the lowpass is not doing anything",
        100.0 * dark_h8 / open_h8
    );
}

#[test]
fn reverb_send_adds_a_tail_and_zero_adds_none() {
    let build = |send: f64| {
        let mut m = model("sine", 2.0);
        m.tracks[0].reverb_send = send;
        set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.5, 69)]);
        let (l, _) = render(&m, 1.5);
        l
    };
    // note off at 0.25 s, "sine" release is 0.20 s → dry signal is gone well before 0.7 s.
    let window = |l: &[f32]| rms(&l[secs(0.80)..secs(1.10)]);

    let dry = build(0.0);
    let wet = build(0.6);
    assert!(peak(&dry) > 0.05, "dry render is silent");
    assert!(
        window(&dry) < 1e-6,
        "reverb_send = 0 still produced a tail (rms {})",
        window(&dry)
    );
    assert!(
        window(&wet) > 1e-3,
        "reverb_send = 0.6 produced no tail after note-off (rms {})",
        window(&wet)
    );
}

/// Deterministic broadband noise — the honest worst case for a reverb's gain staging.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }
}

#[test]
fn the_reverb_mix_knob_changes_ambience_not_level() {
    // Both "reverb" knobs in the UI are a 0–100% dry/wet mix. Sweeping one must not change how
    // loud the bus is: with a steady input the output rms has to hold across the whole sweep.
    let level = |wet: f32| {
        let mut rv = dissonant_engine::dsp::Reverb::new(SR);
        rv.wet = wet;
        let mut rng = Lcg(1);
        let mut out = Vec::with_capacity(secs(4.0));
        for _ in 0..secs(4.0) {
            let x = rng.next() * 0.3;
            out.push(rv.process(x, x).0);
        }
        // Skip the build-up; measure the steady state.
        rms(&out[secs(1.5)..])
    };
    let dry = level(0.0);
    assert!(dry > 0.1, "the dry reference is wrong: {dry}");
    for wet in [0.25f32, 0.5, 0.75, 1.0] {
        let got = level(wet);
        assert!(
            (got / dry - 1.0).abs() < 0.15,
            "reverb at {wet:.2} renders at {:.2}x the dry level ({got:.4} vs {dry:.4}) — the knob is a volume control",
            got / dry
        );
    }
}

#[test]
fn a_fully_wet_track_does_not_slam_the_master_clamp() {
    // A sustained note with the track reverb wide open must stay in the same ballpark as the
    // dry take, rather than being pushed into the limiter at `Master::process`.
    let bus = |send: f64| {
        let mut m = model("sine", 8.0);
        m.tracks[0].reverb_send = send;
        set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 6.0, 69)]);
        let (l, _) = render(&m, 1.0);
        // 6 beats = 3 s; measure once the reverb has fully built up.
        (peak(&l[secs(1.8)..secs(2.8)]), rms(&l[secs(1.8)..secs(2.8)]))
    };
    let (dry_peak, dry_rms) = bus(0.0);
    for send in [0.25f64, 0.5, 1.0] {
        let (p, r) = bus(send);
        assert!(
            p < dry_peak * 1.25,
            "reverb_send {send} pushed the bus peak to {p} against a dry {dry_peak}"
        );
        assert!(
            r > dry_rms * 0.6 && r < dry_rms * 1.4,
            "reverb_send {send} moved the bus level to {r} against a dry {dry_rms}"
        );
    }
}

// ─── 3. mute / solo ──────────────────────────────────────────────────────────────────────────

/// Two sine tracks on unrelated pitches so each one's presence is a single Goertzel bin.
const A_PITCH: i32 = 69; // 440.0 Hz
const B_PITCH: i32 = 74; // 587.3 Hz

fn two_track_model() -> ProjectModel {
    let mut m = model("sine", 2.0);
    let mut b = dissonant_core::Track::new("b");
    b.voice = "sine".into();
    m.tracks.push(b);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 1.0, A_PITCH)]);
    set_notes(&mut m, 1, vec![NoteEvent::new(0.0, 1.0, B_PITCH)]);
    m
}

fn track_energies(m: &ProjectModel) -> (f64, f64) {
    let (l, _) = render(m, 0.5);
    let w = l[secs(0.10)..secs(0.40)].to_vec();
    (tone_amplitude(&w, midi_hz(A_PITCH)), tone_amplitude(&w, midi_hz(B_PITCH)))
}

#[test]
fn mute_silences_only_that_track() {
    let m = two_track_model();
    let (a0, b0) = track_energies(&m);
    assert!(a0 > 0.01 && b0 > 0.01, "baseline should have both tracks: {a0} / {b0}");

    let mut muted = two_track_model();
    muted.tracks[0].muted = true;
    let (a1, b1) = track_energies(&muted);
    assert!(a1 < a0 * 0.01, "muted track A still sounds: {a1} (was {a0})");
    assert!(
        (b1 - b0).abs() < b0 * 0.01,
        "muting A changed B: {b1} (was {b0})"
    );
}

#[test]
fn solo_silences_every_non_soloed_track() {
    let base = track_energies(&two_track_model());
    let mut soloed = two_track_model();
    soloed.tracks[1].soloed = true;
    let (a, b) = track_energies(&soloed);
    assert!(a < base.0 * 0.01, "track A still sounds while B is soloed: {a}");
    assert!((b - base.1).abs() < base.1 * 0.01, "soloed track B changed level: {b}");
}

#[test]
fn solo_beats_mute_exactly_as_sequence_track_audible_says() {
    let mut m = two_track_model();
    m.tracks[0].muted = true;
    m.tracks[0].soloed = true;
    let seq = sequence(&m);
    assert!(seq.any_solo());
    // The contract: with any solo active, `audible` ignores `muted`.
    assert!(seq.tracks[0].audible(true), "contract changed: solo should beat mute");
    assert!(!seq.tracks[1].audible(true));

    let base = track_energies(&two_track_model());
    let (a, b) = track_energies(&m);
    assert!(
        a > base.0 * 0.5,
        "track is soloed *and* muted — SequenceTrack::audible says it sounds, engine gave {a}"
    );
    assert!(b < base.1 * 0.01, "non-soloed track B still sounds: {b}");
}

// ─── 4. note lifecycle ───────────────────────────────────────────────────────────────────────

/// Index of the last sample above `thresh`, i.e. where the track actually stops making sound.
fn last_sounding(x: &[f32], thresh: f32) -> usize {
    x.iter().rposition(|s| s.abs() > thresh).unwrap_or(0)
}

#[test]
fn notes_stop_at_their_end_beat() {
    let mut m = model("sine", 4.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 1.0, 69)]);
    let (l, _) = render(&m, 0.5);
    // 1 beat @120 bpm = 0.5 s, "sine" release = 0.2 s → silence from ~0.70 s onwards.
    assert!(rms(&l[secs(0.10)..secs(0.40)]) > 1e-3, "note never sounded");
    assert!(
        rms(&l[secs(0.85)..secs(2.0)]) < 1e-6,
        "voice is stuck past the note's end beat (rms {})",
        rms(&l[secs(0.85)..secs(2.0)])
    );
}

#[test]
fn length_beats_is_honored_not_a_fixed_duration() {
    // `sine`'s release is a linear ramp that takes `release` seconds from full scale, so the
    // tail after note-off is at most 0.2 s and shrinks with the level the note was at. What
    // must hold is that the note sustains right up to its end beat and stops within one
    // release afterwards — and that doubling the length doubles the sound.
    let probe = |len: f64| {
        let mut m = model("sine", 8.0);
        set_notes(&mut m, 0, vec![NoteEvent::new(0.0, len, 69)]);
        let (l, _) = render(&m, 0.5);
        let end = len * 0.5; // seconds, @120 bpm
        let before_end = rms(&l[secs(end - 0.05)..secs(end)]);
        (last_sounding(&l, 1e-4) as f64 / SRD, before_end)
    };
    let (short, short_sustain) = probe(0.25); // ends at 0.125 s
    let (long, long_sustain) = probe(3.0); // ends at 1.500 s

    assert!(short_sustain > 1e-3 && long_sustain > 1e-3, "note died before its end beat");
    assert!(
        short > 0.125 && short < 0.125 + 0.21,
        "a 0.25-beat note sounded for {short:.3} s; expected its 0.125 s plus at most one 0.2 s release"
    );
    assert!(
        long > 1.5 && long < 1.5 + 0.21,
        "a 3-beat note sounded for {long:.3} s; expected its 1.5 s plus at most one 0.2 s release"
    );
    assert!(
        ((long - short) - 1.375).abs() < 0.05,
        "the extra 2.75 beats added {:.3} s of sound, not 1.375 s — length_beats is not driving the duration",
        long - short
    );
}

#[test]
fn a_note_shorter_than_one_block_still_sounds() {
    // 0.04 beats @120 bpm = 882 samples; both the note-on and the note-off land inside one
    // 4096-frame block, so the engine must still render the sub-block between them.
    let mut m = model("sine", 4.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.04, 69)]);
    let (mut e, _) = engine_with(&m);
    e.handle(EngineCommand::SetLooping(false));
    e.handle(EngineCommand::Play);
    let (l, _) = run(&mut e, 4096 * 4, 4096);
    assert!(
        peak(&l) > 0.02,
        "a sub-block note was swallowed entirely (peak {})",
        peak(&l)
    );
    assert!(
        last_sounding(&l, 1e-4) as f64 / SRD > 0.02,
        "the sub-block note barely existed"
    );
}

#[test]
fn repeated_same_pitch_notes_retrigger() {
    let mut m = model("sine", 4.0);
    set_notes(
        &mut m,
        0,
        vec![NoteEvent::new(0.0, 1.0, 69), NoteEvent::new(1.0, 1.0, 69)],
    );
    let (l, _) = render(&m, 0.5);
    // Second note runs 0.5 s → 1.0 s. If it never retriggered, the first note's 0.2 s release
    // would have ended by 0.7 s.
    assert!(rms(&l[secs(0.80)..secs(0.95)]) > 1e-3, "the second note never fired");
    assert!(
        rms(&l[secs(1.30)..secs(2.0)]) < 1e-6,
        "the retriggered note is stuck past its end beat"
    );
}

#[test]
fn overlapping_same_pitch_notes_do_not_cut_each_other_off() {
    // note A: beats 0–2 (0.0–1.0 s), note B: beats 1–3 (0.5–1.5 s), same pitch.
    // A's note-off at 1.0 s must not take B with it; B has to keep sounding to 1.5 s.
    let mut m = model("sine", 6.0);
    set_notes(
        &mut m,
        0,
        vec![NoteEvent::new(0.0, 2.0, 69), NoteEvent::new(1.0, 2.0, 69)],
    );
    let (l, _) = render(&m, 0.5);
    let during_b = rms(&l[secs(1.15)..secs(1.45)]);
    assert!(
        during_b > 1e-3,
        "the overlapping note died with the first note-off (rms {during_b} between 1.15 s and 1.45 s)"
    );
    assert!(
        rms(&l[secs(1.85)..secs(2.6)]) < 1e-6,
        "the overlapping pair left a stuck voice"
    );
}

// ─── 5. loop boundaries ──────────────────────────────────────────────────────────────────────

fn onsets(x: &[f32], thresh: f32) -> Vec<usize> {
    let mut out = Vec::new();
    let mut armed = true;
    let mut quiet = 0usize;
    for (i, s) in x.iter().enumerate() {
        if s.abs() > thresh {
            if armed {
                out.push(i);
                armed = false;
            }
            quiet = 0;
        } else {
            quiet += 1;
            if quiet > 2_000 {
                armed = true;
            }
        }
    }
    out
}

#[test]
fn looping_retriggers_at_the_loop_point() {
    // 2-beat loop = 1.0 s @120 bpm, one note at the top of each cycle.
    let mut m = model("sine", 2.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.5, 69)]);
    let (mut e, _) = engine_with(&m);
    e.handle(EngineCommand::Play);
    let (l, _) = run(&mut e, secs(3.05), 512);
    let o = onsets(&l, 1e-3);
    assert!(o.len() >= 3, "expected 3 loop retriggers, found {:?}", o);
    for (cycle, idx) in o.iter().take(3).enumerate() {
        let expected = cycle * 44_100;
        assert!(
            (*idx as i64 - expected as i64).abs() < 64,
            "cycle {cycle} onset at {idx}, expected ~{expected}"
        );
    }
}

#[test]
fn a_note_crossing_the_loop_end_does_not_leak_into_the_next_cycle() {
    // 2-beat loop; the note runs beats 1.5–2.5, i.e. 0.5 beats past the loop end.
    let mut m = model("sine", 2.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(1.5, 1.0, 69)]);
    let (mut e, _) = engine_with(&m);
    e.handle(EngineCommand::Play);
    let (l, _) = run(&mut e, secs(3.0), 512);

    // Cycle 2 spans 1.0–2.0 s; the note restarts at 1.75 s. Between the loop seam's release
    // tail (<= 1.25 s) and 1.75 s the track must be silent — otherwise the voice leaked.
    let gap = rms(&l[secs(1.30)..secs(1.72)]);
    assert!(gap < 1e-6, "a voice leaked across the loop seam (rms {gap})");
    assert!(
        rms(&l[secs(1.80)..secs(1.95)]) > 1e-3,
        "the crossing note did not restart in the next cycle"
    );
}

#[test]
fn disabling_looping_plays_the_sequence_once() {
    let mut m = model("sine", 2.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.5, 69)]);
    let (mut e, _) = engine_with(&m);
    e.handle(EngineCommand::SetLooping(false));
    e.handle(EngineCommand::Play);
    let (l, _) = run(&mut e, secs(3.0), 512);
    assert_eq!(onsets(&l, 1e-3).len(), 1, "non-looping playback repeated itself");
}

// ─── 6. voice stealing ───────────────────────────────────────────────────────────────────────

const POLYPHONY: usize = 12;

#[test]
fn exceeding_polyphony_steals_the_oldest_voice() {
    // 16 simultaneous notes on a 12-voice synth. Notes fire in list order, so the four
    // lowest pitches must be the ones stolen.
    let mut m = model("sine", 4.0);
    m.tracks[0].volume = 0.15; // keep the sum clear of the master clamp so the spectrum is clean
    let notes: Vec<NoteEvent> = (60..76).map(|p| NoteEvent::new(0.0, 2.0, p)).collect();
    set_notes(&mut m, 0, notes);
    let (l, r) = render(&m, 0.5);
    assert_finite(&l, "left");
    assert_finite(&r, "right");

    let w = l[secs(0.10)..secs(0.60)].to_vec();
    let amp = |p: i32| tone_amplitude(&w, midi_hz(p));
    for stolen in 60..64 {
        assert!(
            amp(stolen) < 1e-3,
            "pitch {stolen} should have been stolen but still rings at {}",
            amp(stolen)
        );
    }
    for kept in 64..76 {
        assert!(
            amp(kept) > 1e-3,
            "pitch {kept} is inside the last {POLYPHONY} notes but is silent ({})",
            amp(kept)
        );
    }
}

#[test]
fn voice_stealing_stays_in_range_and_free_of_nan() {
    let mut m = model("saw", 4.0);
    m.tracks[0].volume = 1.5;
    m.tracks[0].reverb_send = 0.8;
    m.master.gain = 1.5;
    // 24 notes, staggered so steals happen mid-sustain rather than all at sample 0.
    let notes: Vec<NoteEvent> = (0..24)
        .map(|i| NoteEvent::new(i as f64 * 0.05, 2.0, 48 + i))
        .collect();
    set_notes(&mut m, 0, notes);
    let (l, r) = render(&m, 1.0);
    assert_finite(&l, "left");
    assert_finite(&r, "right");
    assert!(peak(&l) > 0.5, "the overloaded render went quiet: {}", peak(&l));
}

#[test]
fn stealing_a_sounding_voice_does_not_click() {
    // 12 sustained low notes (65–130 Hz, so the waveform's own slew per sample is tiny), then
    // a 13th a quarter-second in forces the oldest to be reassigned mid-sustain. Slamming the
    // stolen voice's oscillator and envelope back to zero shows up as a step discontinuity
    // many times the natural slew — that step is the click.
    let mut m = model("sine", 8.0);
    let mut notes: Vec<NoteEvent> = (0..POLYPHONY)
        .map(|i| NoteEvent::new(0.0, 4.0, 36 + i as i32).with_velocity(127))
        .collect();
    notes.push(NoteEvent::new(0.5, 2.0, 80).with_velocity(127)); // 0.25 s in
    set_notes(&mut m, 0, notes);
    let (l, _) = render(&m, 0.5);
    assert_finite(&l, "left");

    let steady = max_slew(&l[secs(0.60)..secs(0.90)]);
    let at_steal = max_slew(&l[secs(0.245)..secs(0.258)]);
    assert!(steady > 0.0);
    assert!(
        at_steal < steady * 1.6,
        "voice stealing produced a {at_steal} step against a {steady} steady-state slew ({:.1}x) — that is a click",
        at_steal / steady
    );
}

// ─── 7. master chain ─────────────────────────────────────────────────────────────────────────

fn master_render(pitch: i32, voice: &str, tweak: impl Fn(&mut MasterSettings)) -> Vec<f32> {
    let mut m = model(voice, 2.0);
    tweak(&mut m.master);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 1.0, pitch)]);
    let (l, _) = render(&m, 0.5);
    l[secs(0.10)..secs(0.40)].to_vec()
}

#[test]
fn master_gain_scales_the_output() {
    let full = peak(&master_render(69, "sine", |_| {}));
    let half = peak(&master_render(69, "sine", |s| s.gain = 0.5));
    assert!(full > 0.05, "unity master gain is inaudible: {full}");
    assert!(
        ((half / full) - 0.5).abs() < 0.02,
        "master gain 0.5 gave {half} against {full}"
    );
}

#[test]
fn master_low_cut_removes_low_frequencies() {
    let f = midi_hz(36); // 65.4 Hz
    let open = tone_amplitude(&master_render(36, "sine", |_| {}), f);
    let cut = tone_amplitude(&master_render(36, "sine", |s| s.low_cut_hz = 300.0), f);
    assert!(open > 1e-3, "the 65 Hz test tone never rendered: {open}");
    assert!(
        cut / open < 0.2,
        "low cut at 300 Hz left {:.0}% of a 65 Hz tone",
        100.0 * cut / open
    );
}

#[test]
fn master_high_cut_removes_high_frequencies() {
    let open = master_render(57, "saw", |_| {});
    let cut = master_render(57, "saw", |s| s.high_cut_hz = 600.0);
    let (f0, h8) = (220.0, 1760.0);
    assert!(tone_amplitude(&open, h8) > 1e-4, "no high harmonics to cut");
    assert!(
        tone_amplitude(&cut, f0) / tone_amplitude(&open, f0) > 0.7,
        "high cut at 600 Hz also ate the 220 Hz fundamental"
    );
    assert!(
        tone_amplitude(&cut, h8) / tone_amplitude(&open, h8) < 0.25,
        "high cut at 600 Hz left {:.0}% of the 1760 Hz energy",
        100.0 * tone_amplitude(&cut, h8) / tone_amplitude(&open, h8)
    );
}

#[test]
fn master_eq_bands_each_move_their_own_band() {
    // low shelf @120 Hz, peaking @1 kHz, high shelf @6 kHz. Probe each with a tone that sits
    // squarely inside that band, and check the other bands leave it alone.
    let cases: [(i32, fn(&mut MasterSettings), &str); 3] = [
        (31, |s| s.low_eq = 2.0, "low"),   //  49 Hz
        (83, |s| s.mid_eq = 2.0, "mid"),   // 988 Hz
        (127, |s| s.high_eq = 2.0, "high"), // 12.5 kHz
    ];
    for (pitch, boost, name) in cases {
        let f = midi_hz(pitch);
        let flat = tone_amplitude(&master_render(pitch, "sine", |_| {}), f);
        let boosted = tone_amplitude(&master_render(pitch, "sine", boost), f);
        assert!(flat > 1e-4, "{name} probe tone ({f:.0} Hz) did not render: {flat}");
        assert!(
            boosted / flat > 1.5,
            "{name} EQ at 2.0 only moved {f:.0} Hz by {:.2}x",
            boosted / flat
        );
    }
}

#[test]
fn master_clamp_never_emits_nan() {
    let mut m = model("saw", 2.0);
    m.tracks[0].volume = 1.5;
    m.tracks[0].reverb_send = 1.0;
    m.master = MasterSettings {
        gain: 1.5,
        reverb_wet: 1.0,
        low_cut_hz: 5.0,
        high_cut_hz: 21_000.0,
        low_eq: 0.0,
        mid_eq: 4.0,
        high_eq: 4.0,
    };
    let notes: Vec<NoteEvent> = (0..16).map(|i| NoteEvent::new(0.0, 2.0, 40 + i * 4)).collect();
    set_notes(&mut m, 0, notes);
    let (l, r) = render(&m, 1.0);
    assert_finite(&l, "left");
    assert_finite(&r, "right");
}

// ─── 8. meters ───────────────────────────────────────────────────────────────────────────────

/// Step the engine one block at a time, checking the published meter against the audio that
/// block actually produced. `Shared`'s meters are decaying peak-holds, so after every block
/// the reading must be at least that block's peak and never above the running maximum.
#[test]
fn master_peak_tracks_each_block_then_falls_back_to_zero() {
    let mut m = model("sine", 8.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.5, 69)]);
    let (mut e, shared) = engine_with(&m);
    e.handle(EngineCommand::SetLooping(false));
    e.handle(EngineCommand::Play);

    let mut buf = vec![0.0f32; 256 * 2];
    let mut running = 0.0f32;
    let mut ever_lit = false;
    for _ in 0..(secs(0.2) / 256) {
        e.process(&mut buf);
        let (left, _) = split(&buf);
        let block = peak(&left);
        running = running.max(block);
        let (pl, _) = shared.master_peak();
        assert!(
            pl >= block - 1e-6,
            "master meter reads {pl} but the block peaked at {block}"
        );
        assert!(pl <= running + 1e-6, "master meter reads {pl}, above the running max {running}");
        ever_lit |= pl > 0.0;
    }
    assert!(ever_lit && running > 0.05, "nothing was rendered to meter: {running}");

    run(&mut e, secs(2.0), 256);
    assert_eq!(
        shared.master_peak(),
        (0.0, 0.0),
        "master meter never fell back after silence"
    );
}

#[test]
fn track_peak_falls_back_to_zero_after_the_track_goes_silent() {
    for block in [256usize, 4096] {
        for voice in ["sine", "pluck"] {
            let mut m = model(voice, 16.0);
            set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.5, 69)]);
            let (mut e, shared) = engine_with(&m);
            e.handle(EngineCommand::SetLooping(false));
            e.handle(EngineCommand::Play);

            run(&mut e, secs(0.2), block);
            assert!(
                shared.track_peak(0) > 0.0,
                "{voice}/{block}: track meter is dead while the track is sounding"
            );

            // 0.25 s note + release: the track is silent long before this run ends. The
            // peak-hold decays per *block*, so a big buffer needs more wall-clock time.
            run(&mut e, secs(8.0), block);
            assert_eq!(
                shared.track_peak(0),
                0.0,
                "{voice}/{block}: track meter froze instead of returning to zero"
            );
        }
    }
}

#[test]
fn a_drum_track_meter_also_returns_to_zero() {
    let mut m = model("sine", 16.0);
    m.tracks[0].is_drum = true;
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.25, 36)]);
    let (mut e, shared) = engine_with(&m);
    e.handle(EngineCommand::SetLooping(false));
    e.handle(EngineCommand::Play);
    run(&mut e, secs(0.2), 4096);
    assert!(shared.track_peak(0) > 0.0, "drum meter is dead while the kick sounds");
    run(&mut e, secs(8.0), 4096);
    assert_eq!(shared.track_peak(0), 0.0, "drum meter froze after the one-shot ended");
}

#[test]
fn a_silent_project_leaves_every_meter_at_zero() {
    let m = model("sine", 4.0);
    let (mut e, shared) = engine_with(&m);
    e.handle(EngineCommand::Play);
    let (l, r) = run(&mut e, secs(1.0), 256);
    assert_eq!(peak(&l), 0.0);
    assert_eq!(peak(&r), 0.0);
    assert_eq!(shared.master_peak(), (0.0, 0.0));
    assert_eq!(shared.track_peak(0), 0.0);
}

// ─── resuming a note the playhead landed inside ──────────────────────────────────────────────

/// Clicking the ruler in the middle of a long note must play that note, not silence until the
/// next one starts. Scheduling alone only fires a note at its start beat.
#[test]
fn seeking_into_a_sustained_note_sounds_it() {
    let mut m = model("sine", 8.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 6.0, 69)]);
    let (mut e, _shared) = engine_with(&m);
    e.handle(EngineCommand::SetLooping(false));
    e.handle(EngineCommand::Seek { beat: 3.0 });
    e.handle(EngineCommand::Play);

    let (left, _) = run(&mut e, secs(0.5), 512);
    assert!(peak(&left) > 0.01, "seeking into the note gave silence: peak {}", peak(&left));

    // It is the note under the playhead, at its own pitch (A4 = 440 Hz), not a retrigger of the
    // pattern from the top.
    assert!(tone_amplitude(&left, 440.0) > 0.01, "the resumed note is not sounding its pitch");

    // And it still ends where it should rather than hanging: 3 beats left at 120 bpm = 1.5 s,
    // plus the voice's release.
    let (tail, _) = run(&mut e, secs(2.5), 512);
    let end = tail.len() - secs(0.3);
    assert!(peak(&tail[end..]) < 1e-3, "the resumed note never stopped");
}


/// Starting playback (not just seeking) from inside a note must sound it too — stop, scrub,
/// play is the ordinary way to audition the middle of a pattern.
#[test]
fn playing_from_inside_a_note_sounds_it() {
    let mut m = model("sine", 8.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(2.0, 4.0, 69)]);
    let (mut e, _shared) = engine_with(&m);
    e.handle(EngineCommand::Seek { beat: 4.0 });
    let (before, _) = run(&mut e, secs(0.2), 512);
    assert_eq!(peak(&before), 0.0, "a stopped engine must stay silent");

    e.handle(EngineCommand::Play);
    let (left, _) = run(&mut e, secs(0.2), 512);
    assert!(peak(&left) > 0.01, "playback from inside a note gave silence");
}

/// A drum hit is a one-shot; resuming must not retrigger it from the middle.
#[test]
fn seeking_does_not_retrigger_a_drum_hit() {
    let mut m = ProjectModel::empty();
    m.patterns[0].length_beats = 8.0;
    m.tracks[0].is_drum = true;
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 4.0, 36)]);
    let (mut e, _shared) = engine_with(&m);
    e.handle(EngineCommand::Seek { beat: 2.0 });
    e.handle(EngineCommand::Play);
    let (left, _) = run(&mut e, secs(0.4), 512);
    assert!(peak(&left) < 1e-3, "seeking past a drum hit replayed it: peak {}", peak(&left));
}

// ─── muted tracks still release their meter ──────────────────────────────────────────────────

/// Muting a sounding track silences it, but its meter must fall instead of freezing lit at the
/// level it had when the mute landed.
#[test]
fn muting_a_sounding_track_lets_its_meter_fall() {
    let mut m = model("sine", 16.0);
    set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 12.0, 69)]);
    let (mut e, shared) = engine_with(&m);
    e.handle(EngineCommand::Play);
    run(&mut e, secs(0.3), 512);
    assert!(shared.track_peak(0) > 0.01, "the track never lit its meter");

    m.tracks[0].muted = true;
    e.handle(EngineCommand::SetSequence(sequence(&m)));
    let (left, _) = run(&mut e, secs(2.0), 512);
    // The voice releases rather than cutting dead, so judge the steady state, not the tail.
    let settled = secs(0.5);
    assert!(peak(&left[settled..]) < 1e-3, "a muted track was still audible");
    assert_eq!(shared.track_peak(0), 0.0, "the muted track's meter froze lit");
}

// ─── meter ballistics are a time constant, not a per-call factor ─────────────────────────────

/// Identical audio must produce the same meter fall whatever buffer size the device hands us:
/// the decay is per unit time, so it cannot depend on how the block happens to be chopped up.
#[test]
fn meter_release_does_not_depend_on_the_buffer_size() {
    let mut lit = Vec::new();
    for block in [128usize, 512, 4096] {
        let mut m = model("sine", 16.0);
        set_notes(&mut m, 0, vec![NoteEvent::new(0.0, 0.5, 69)]);
        let (mut e, shared) = engine_with(&m);
        e.handle(EngineCommand::SetLooping(false));
        e.handle(EngineCommand::Play);

        let mut elapsed = 0usize;
        while elapsed < secs(6.0) && (elapsed < secs(0.5) || shared.track_peak(0) > 0.0) {
            run(&mut e, block, block);
            elapsed += block;
        }
        lit.push(elapsed as f64 / SRD);
    }
    let (min, max) = (lit.iter().cloned().fold(f64::MAX, f64::min), lit.iter().cloned().fold(0.0, f64::max));
    assert!(
        max - min < 0.15,
        "meter release swings with the buffer size: {lit:?} seconds for 128/512/4096 frames"
    );
}
