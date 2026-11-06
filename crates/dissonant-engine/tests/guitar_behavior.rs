//! Behavioural audit of the guitar path: audio clip playback, the rig, and how an audio track
//! behaves in the mixer.
//!
//! Offline throughout — a hand-driven `Engine::process` loop with a synthetic `AudioPool`. No
//! device, no files, no randomness. Spectral claims come from a Goertzel bin.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use dissonant_core::{
    AmpModel, AmpRig, AudioClip, AudioSource, CabModel, Cabinet, EqModel, MasterSettings, MicModel,
    Pedal, ProjectModel, Sequence, SequenceAudioClip, SequenceTrack, Track, TICKS_PER_BEAT,
};
use dissonant_engine::audio::{AudioBuffer, AudioLane, AudioPool, BlockContext};
use dissonant_engine::engine::{Engine, EngineCommand, Shared};
use dissonant_engine::rig::Rig;
use uuid::Uuid;

const SR: f32 = 44_100.0;
const SRD: f64 = 44_100.0;
/// Ticks per beat, short enough to keep each test's musical intent readable.
const B: i64 = TICKS_PER_BEAT;

// ─── helpers ─────────────────────────────────────────────────────────────────────────────────

/// A project at 120 bpm with one guitar track and nothing else making noise.
fn guitar_project() -> (ProjectModel, Uuid) {
    let mut m = ProjectModel::empty();
    m.tempo = 120.0;
    // The default melody track must not colour the measurements.
    m.tracks[0].muted = true;
    let t = Track::audio("guitar");
    let id = t.id;
    m.tracks.push(t);
    (m, id)
}

/// A mono sine source at `hz` lasting `seconds`, at `rate`.
fn sine_source(hz: f64, seconds: f64, rate: u32, amplitude: f32) -> (AudioSource, AudioBuffer) {
    let frames = (seconds * rate as f64) as usize;
    let data: Vec<f32> = (0..frames)
        .map(|n| amplitude * (2.0 * PI * hz * n as f64 / rate as f64).sin() as f32)
        .collect();
    let source = AudioSource {
        id: Uuid::new_v4(),
        name: "test".into(),
        frames: frames as u64,
        sample_rate: rate,
        channels: 1,
    };
    (source, AudioBuffer::new(rate, 1, data))
}

/// Full-scale white-ish noise at `amplitude`, deterministic.
fn noise_source(seconds: f64, rate: u32, amplitude: f32) -> (AudioSource, AudioBuffer) {
    let frames = (seconds * rate as f64) as usize;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let data: Vec<f32> = (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            amplitude * ((state >> 40) as f32 / 8_388_608.0 - 1.0)
        })
        .collect();
    let source = AudioSource {
        id: Uuid::new_v4(),
        name: "noise".into(),
        frames: frames as u64,
        sample_rate: rate,
        channels: 1,
    };
    (source, AudioBuffer::new(rate, 1, data))
}

/// A single-sample impulse source.
fn impulse_source(seconds: f64, rate: u32) -> (AudioSource, AudioBuffer) {
    let frames = (seconds * rate as f64) as usize;
    let mut data = vec![0.0f32; frames];
    data[0] = 1.0;
    let source = AudioSource {
        id: Uuid::new_v4(),
        name: "impulse".into(),
        frames: frames as u64,
        sample_rate: rate,
        channels: 1,
    };
    (source, AudioBuffer::new(rate, 1, data))
}

fn pool(entries: Vec<(&AudioSource, AudioBuffer)>) -> Arc<AudioPool> {
    let map: HashMap<Uuid, Arc<AudioBuffer>> = entries
        .into_iter()
        .map(|(s, b)| (s.id, Arc::new(b)))
        .collect();
    Arc::new(AudioPool::new(map))
}

/// Build an engine on the song sequence, with `pool` installed and the transport ready to play.
fn engine_for(m: &ProjectModel, pool: Arc<AudioPool>) -> (Engine, Arc<Shared>) {
    let shared = Arc::new(Shared::default());
    let mut e = Engine::new(SR, Arc::clone(&shared), &MasterSettings::default());
    e.handle(EngineCommand::SetAudioPool(pool));
    e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(m))));
    e.handle(EngineCommand::SetLooping(false));
    (e, shared)
}

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
    (
        out.iter().step_by(2).copied().collect(),
        out.iter().skip(1).step_by(2).copied().collect(),
    )
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

fn assert_finite(x: &[f32], what: &str) {
    for (i, s) in x.iter().enumerate() {
        assert!(s.is_finite(), "{what}: sample {i} is {s}");
        assert!(s.abs() <= 1.0, "{what}: sample {i} = {s} escapes [-1, 1]");
    }
}

/// A rig that passes audio through untouched, so a test can measure the clip alone.
fn bypassed() -> AmpRig {
    AmpRig {
        enabled: false,
        ..AmpRig::default()
    }
}

/// A rig with no pedals, no gate and no reverb: just the amp and cab under test.
fn bare(amp_model: AmpModel, gain: f64) -> AmpRig {
    let mut rig = AmpRig {
        pedals: vec![],
        rack: dissonant_core::Rack {
            reverb: 0.0,
            decay: 5.0,
            output_db: 0.0,
        },
        ..AmpRig::default()
    };
    rig.gate.on = false;
    rig.amp.model = amp_model;
    rig.amp.gain = gain;
    rig
}

/// A flattened audio track carrying `rig`, for driving an [`AudioLane`] on its own.
fn audio_track(id: Uuid, rig: AmpRig) -> SequenceTrack {
    let mut m = ProjectModel::empty();
    let mut t = Track::audio("guitar");
    t.id = id;
    t.amp = Some(rig);
    m.tracks.push(t);
    Sequence::from_song(&m)
        .tracks
        .into_iter()
        .find(|t| t.id == id)
        .expect("the track is in the sequence")
}

// ─── clip playback ───────────────────────────────────────────────────────────────────────────

#[test]
fn a_clip_sounds_from_its_start_tick_for_its_length() {
    let (mut m, gid) = guitar_project();
    let (source, buffer) = sine_source(440.0, 1.0, 44_100, 0.5);
    // Beat 4 at 120 bpm is 2.0 s in.
    m.audio_clips = vec![AudioClip::new(gid, source.id, 4 * B, 1.0)];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(bypassed());

    let p = pool(vec![(&source, buffer)]);
    let (mut e, _shared) = engine_for(&m, p);
    e.handle(EngineCommand::Play);
    let (l, _r) = run(&mut e, secs(4.0), 512);

    // Leave a block of slack either side of each boundary: the clip starts and ends on an
    // exact sample, but asserting *at* the boundary would be testing rounding, not behaviour.
    assert!(
        peak(&l[..secs(1.9)]) < 1e-5,
        "sound before the clip started"
    );
    assert!(
        peak(&l[secs(2.1)..secs(2.9)]) > 0.1,
        "the clip did not sound"
    );
    assert!(peak(&l[secs(3.1)..]) < 1e-5, "sound after the clip ended");
    assert_finite(&l, "clip playback");
}

#[test]
fn a_44_1k_source_keeps_its_pitch_in_a_48k_engine() {
    // Resampling is the only thing standing between a 44.1 kHz file and a semitone of detune
    // on a 48 kHz interface.
    let (mut m, gid) = guitar_project();
    let (source, buffer) = sine_source(440.0, 2.0, 44_100, 0.5);
    m.audio_clips = vec![AudioClip::new(gid, source.id, 0, 2.0)];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(bypassed());

    let shared = Arc::new(Shared::default());
    let mut e = Engine::new(48_000.0, Arc::clone(&shared), &MasterSettings::default());
    e.handle(EngineCommand::SetAudioPool(pool(vec![(&source, buffer)])));
    e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(
        &m,
    ))));
    e.handle(EngineCommand::SetLooping(false));
    e.handle(EngineCommand::Play);

    let mut buf = vec![0.0f32; 1024];
    let mut left = Vec::new();
    for _ in 0..80 {
        e.process(&mut buf);
        left.extend(buf.iter().step_by(2).copied());
    }
    // Goertzel at 48 kHz: k = f/fs · n.
    let n = left.len() as f64;
    let amp_at = |hz: f64| {
        let k = (hz / 48_000.0 * n).round().max(1.0);
        let w = 2.0 * PI * k / n;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &v in &left {
            let s = v as f64 + coeff * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let re = s1 - s2 * w.cos();
        let im = s2 * w.sin();
        2.0 * (re * re + im * im).sqrt() / n
    };
    let at_440 = amp_at(440.0);
    // 440 · 48/44.1 = 478.9 Hz — what you would hear if the file were played back raw.
    let at_479 = amp_at(478.9);
    // Goertzel rounds to an integer bin, so an off-bin tone loses up to a third of its
    // amplitude to scalloping; the claim here is about *which* frequency is present.
    assert!(at_440 > 0.1, "the resampled tone is missing: {at_440}");
    assert!(
        at_479 < at_440 * 0.05,
        "the file played at the wrong rate: {at_479} against {at_440}"
    );
}

#[test]
fn clip_gain_and_fades_shape_the_block() {
    let (mut m, gid) = guitar_project();
    let (source, buffer) = sine_source(440.0, 2.0, 44_100, 1.0);
    let mut clip = AudioClip::new(gid, source.id, 0, 2.0);
    clip.gain = 0.5;
    clip.fade_in_seconds = 0.25;
    clip.fade_out_seconds = 0.25;
    m.audio_clips = vec![clip];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(bypassed());

    let (mut e, _shared) = engine_for(&m, pool(vec![(&source, buffer)]));
    e.handle(EngineCommand::Play);
    let (l, _r) = run(&mut e, secs(2.0), 512);

    // The fade rises: each 50 ms window of the fade is louder than the one before it.
    let window = secs(0.05);
    let mut previous = 0.0;
    for w in 0..4 {
        let here = peak(&l[w * window..(w + 1) * window]);
        assert!(
            here > previous,
            "fade-in is not monotonic at window {w}: {here} after {previous}"
        );
        previous = here;
    }
    // A centred mono clip is written to both sides and then panned at constant power, so
    // each side carries gain × cos(π/4).
    let expected = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
    let middle = peak(&l[secs(0.9)..secs(1.1)]);
    assert!(
        (middle - expected).abs() < 0.02,
        "gain 0.5 produced a peak of {middle}, wanted {expected}"
    );
    assert!(peak(&l[secs(1.99)..]) < 0.05, "the clip did not fade out");
}

#[test]
fn a_bypassed_rig_passes_the_clip_through_unchanged() {
    let (mut m, gid) = guitar_project();
    let (source, buffer) = sine_source(220.0, 1.0, 44_100, 0.4);
    m.audio_clips = vec![AudioClip::new(gid, source.id, 0, 1.0)];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(bypassed());

    let (mut e, _shared) = engine_for(
        &m,
        pool(vec![(&source, Arc::try_unwrap(Arc::new(buffer)).unwrap())]),
    );
    e.handle(EngineCommand::Play);
    let (l, r) = run(&mut e, secs(0.5), 512);
    // Bypassed means bypassed. Measured past the start: the master's 20 Hz low-cut rings for
    // a few ms when a sine begins at full amplitude, which is the filter, not the rig.
    let body = &l[secs(0.1)..];
    let expected = 0.4 * std::f32::consts::FRAC_1_SQRT_2;
    assert!(
        (peak(body) - expected).abs() < 0.01,
        "a bypassed rig changed the level: {} wanted {expected}",
        peak(body)
    );
    for (i, (a, b)) in l.iter().zip(r.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-6,
            "a mono clip is not centred at sample {i}"
        );
    }
    let fundamental = tone_amplitude(&l[secs(0.1)..], 220.0);
    let second = tone_amplitude(&l[secs(0.1)..], 440.0);
    assert!(
        second < fundamental * 0.01,
        "a bypassed rig added harmonics: {second} against {fundamental}"
    );
}

// ─── the rig ─────────────────────────────────────────────────────────────────────────────────

/// Render `seconds` of `source` through `rig` on the guitar track.
fn through_rig(rig: AmpRig, source: AudioSource, buffer: AudioBuffer, seconds: f64) -> Vec<f32> {
    let (mut m, gid) = guitar_project();
    m.audio_clips = vec![AudioClip::new(gid, source.id, 0, seconds)];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(rig);
    let (mut e, _shared) = engine_for(&m, pool(vec![(&source, buffer)]));
    e.handle(EngineCommand::Play);
    run(&mut e, secs(seconds), 512).0
}

#[test]
fn amp_drive_adds_harmonics_without_fold_back() {
    // 7 kHz hard-driven: the 5th harmonic at 35 kHz would fold back to 44100 − 35000 = 9.1 kHz,
    // which is inharmonic and plainly audible. The rig oversamples, so it must not.
    let (source, buffer) = sine_source(7_000.0, 1.0, 44_100, 0.5);
    let out = through_rig(bare(AmpModel::Lead, 10.0), source, buffer, 1.0);
    let tail = &out[secs(0.3)..];

    let fundamental = tone_amplitude(tail, 7_000.0);
    assert!(
        fundamental > 1e-3,
        "the amp swallowed the signal: {fundamental}"
    );
    let alias = tone_amplitude(tail, 9_100.0);
    assert!(
        alias < fundamental * 0.01,
        "fold-back survived oversampling: {alias} against {fundamental}"
    );

    // And a mid-band tone really does grow harmonics — distortion, not just a filter.
    let (source, buffer) = sine_source(400.0, 1.0, 44_100, 0.5);
    let driven = through_rig(bare(AmpModel::Lead, 10.0), source.clone(), buffer, 1.0);
    let tail = &driven[secs(0.3)..];
    let f0 = tone_amplitude(tail, 400.0);
    let f3 = tone_amplitude(tail, 1_200.0);
    assert!(
        f3 > f0 * 0.05,
        "a cranked amp produced no third harmonic: {f3} against {f0}"
    );
}

#[test]
fn the_amp_gain_knob_changes_how_much_it_distorts() {
    let harmonic_ratio = |gain: f64| {
        let (source, buffer) = sine_source(400.0, 1.0, 44_100, 0.3);
        let out = through_rig(bare(AmpModel::Crunch, gain), source, buffer, 1.0);
        let tail = &out[secs(0.3)..];
        let f0 = tone_amplitude(tail, 400.0).max(1e-9);
        tone_amplitude(tail, 1_200.0) / f0
    };
    let clean = harmonic_ratio(0.0);
    let cranked = harmonic_ratio(10.0);
    assert!(
        cranked > clean * 3.0,
        "gain barely changed the distortion: {clean} → {cranked}"
    );
}

#[test]
fn the_gate_silences_hiss_and_passes_a_note() {
    let mut gated = bare(AmpModel::Clean, 2.0);
    gated.gate.on = true;
    gated.gate.threshold_db = -40.0;
    let mut open = gated.clone();
    open.gate.on = false;

    // -70 dBFS of hiss is well under the threshold.
    let (source, buffer) = noise_source(1.0, 44_100, 10f32.powf(-70.0 / 20.0));
    let closed_rms = rms(&through_rig(gated.clone(), source.clone(), buffer, 1.0));
    let (source2, buffer2) = noise_source(1.0, 44_100, 10f32.powf(-70.0 / 20.0));
    let ungated_rms = rms(&through_rig(open, source2, buffer2, 1.0));
    assert!(
        closed_rms < ungated_rms * 0.2,
        "the gate leaked hiss: {closed_rms} against {ungated_rms}"
    );

    // A -12 dBFS note goes straight through.
    let (source3, buffer3) = sine_source(220.0, 1.0, 44_100, 10f32.powf(-12.0 / 20.0));
    let note = through_rig(gated, source3, buffer3, 1.0);
    assert!(
        peak(&note[secs(0.2)..]) > 0.05,
        "the gate swallowed a note: {}",
        peak(&note)
    );
}

#[test]
fn the_delay_pedal_repeats_at_the_time_it_is_set_to() {
    let mut rig = bare(AmpModel::Clean, 0.0);
    rig.pedals = vec![Pedal::Delay {
        on: true,
        time_ms: 300.0,
        feedback: 0.0,
        mix: 10.0,
    }];
    let (source, buffer) = impulse_source(1.0, 44_100);
    let out = through_rig(rig, source, buffer, 1.0);

    // Find the loudest sample in a window around 300 ms.
    let centre = secs(0.3);
    let window = secs(0.02);
    let (at, level) = out[centre - window..centre + window]
        .iter()
        .enumerate()
        .fold((0usize, 0.0f32), |(bi, bv), (i, v)| {
            if v.abs() > bv {
                (i, v.abs())
            } else {
                (bi, bv)
            }
        });
    assert!(level > 1e-4, "no repeat around 300 ms: peak was {level}");
    let offset_ms = ((centre - window + at) as f64 / SRD - 0.3) * 1000.0;
    // The cab and amp filters smear the impulse, so a couple of ms of slop is the filters,
    // not the delay line.
    assert!(
        offset_ms.abs() < 3.0,
        "the repeat landed {offset_ms:.2} ms off"
    );
}

#[test]
fn the_clip_reader_is_sample_exact_at_every_block_size() {
    // The device picks the buffer size. Driven directly — with the transport time handed in
    // exactly — reading a clip must not depend on it at all.
    let (source, buffer) = sine_source(330.0, 1.0, 44_100, 0.4);
    let tid = Uuid::new_v4();
    let map: HashMap<Uuid, Arc<AudioBuffer>> =
        [(source.id, Arc::new(buffer))].into_iter().collect();
    let pool = AudioPool::new(map);
    let clips = vec![SequenceAudioClip {
        track_id: tid,
        source_id: source.id,
        start_seconds: 0.0,
        length_seconds: 1.0,
        offset_seconds: 0.0,
        gain: 1.0,
        fade_in_seconds: 0.0,
        fade_out_seconds: 0.0,
    }];

    let total = secs(0.5);
    let mut reference: Option<Vec<f32>> = None;
    for block in [128usize, 512, 4096] {
        let mut lane = AudioLane::new(tid, SR);
        lane.apply(&audio_track(tid, bypassed()));
        let mut out = Vec::with_capacity(total);
        let mut done = 0;
        while done < total {
            let frames = block.min(total - done);
            let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
            let ctx = BlockContext {
                time_seconds: done as f64 / SRD,
                beat_seconds: 0.5,
                playing: true,
                pool: &pool,
                clips: &clips,
                input: &[],
            };
            lane.render_add(&mut l, &mut r, &ctx, SR);
            out.extend_from_slice(&l);
            done += frames;
        }
        match &reference {
            None => {
                assert!(
                    (peak(&out) - 0.4).abs() < 1e-4,
                    "the clip reader changed the level: {}",
                    peak(&out)
                );
                reference = Some(out);
            }
            Some(first) => {
                let worst = first
                    .iter()
                    .zip(out.iter())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max);
                // One f32 ULP at 0.4 is 3e-8; 1e-6 leaves room for the interpolator without
                // admitting a dropped sample, an off-by-one or unflushed state.
                assert!(worst < 1e-6, "block size {block} diverged by {worst}");
            }
        }
    }
}

#[test]
fn the_rig_sounds_the_same_whatever_the_buffer_size_is() {
    // Through the whole engine it is no longer bit-exact: transport position is carried in
    // beats, so `seconds_at(position)` leaves a few ULPs of f64 residue that differ with the
    // block count, the interpolator turns that into a fraction-of-a-sample offset, and a
    // cranked amp amplifies it. The bound below is about -50 dBFS — inaudible under a
    // distorted guitar, and still tight enough to catch a dropped sample or lost filter state.
    let (mut m, gid) = guitar_project();
    let (source, buffer) = sine_source(330.0, 1.0, 44_100, 0.4);
    m.audio_clips = vec![AudioClip::new(gid, source.id, 0, 1.0)];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(bare(AmpModel::Crunch, 7.0));
    let shared_buffer = Arc::new(buffer);

    let mut reference: Option<Vec<f32>> = None;
    for block in [128usize, 512, 4096] {
        let map: HashMap<Uuid, Arc<AudioBuffer>> = [(source.id, Arc::clone(&shared_buffer))]
            .into_iter()
            .collect();
        let (mut e, _shared) = engine_for(&m, Arc::new(AudioPool::new(map)));
        e.handle(EngineCommand::Play);
        let (l, _r) = run(&mut e, secs(0.5), block);
        match &reference {
            None => reference = Some(l),
            Some(first) => {
                let worst = first
                    .iter()
                    .zip(l.iter())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max);
                assert!(worst < 3e-3, "block size {block} diverged by {worst}");
            }
        }
    }
}

#[test]
fn an_extreme_rig_never_produces_a_non_finite_sample() {
    let mut rig = AmpRig {
        input_db: 24.0,
        pedals: vec![
            Pedal::Comp {
                on: true,
                sustain: 10.0,
                level: 10.0,
            },
            Pedal::Drive {
                on: true,
                drive: 10.0,
                tone: 10.0,
                level: 10.0,
            },
            Pedal::Delay {
                on: true,
                time_ms: 20.0,
                feedback: 10.0,
                mix: 10.0,
            },
            Pedal::Phaser {
                on: true,
                rate_hz: 8.0,
                depth: 10.0,
                feedback: 10.0,
            },
        ],
        rack: dissonant_core::Rack {
            reverb: 10.0,
            decay: 10.0,
            output_db: 12.0,
        },
        ..AmpRig::default()
    };
    rig.gate.on = false;
    rig.amp = dissonant_core::AmpHead {
        model: AmpModel::Fuzz,
        gain: 10.0,
        bass: 10.0,
        mid: 10.0,
        treble: 10.0,
        presence: 10.0,
        master: 10.0,
        bright_cap: true,
        half_power: true,
        cathode_follower: true,
    };

    let (source, buffer) = noise_source(2.0, 44_100, 1.0);
    let out = through_rig(rig.clone(), source, buffer, 2.0);
    for (i, s) in out.iter().enumerate() {
        assert!(s.is_finite(), "sample {i} is {s}");
    }

    // The fuzz voicing has none of those switches, so the run above never touched the circuits
    // they throw. The tweed has all three, and all three in at once is the extreme that matters:
    // a follower conducting into a half-powered supply that is already on the floor.
    rig.amp.model = AmpModel::Clean;
    rig.amp.cathode_follower = true;
    let (source, buffer) = noise_source(2.0, 44_100, 1.0);
    let out = through_rig(rig, source, buffer, 2.0);
    for (i, s) in out.iter().enumerate() {
        assert!(s.is_finite(), "sample {i} with every switch thrown is {s}");
    }
}

// ─── an audio track is a track ───────────────────────────────────────────────────────────────

#[test]
fn an_audio_track_meters_pans_and_mutes_like_any_other() {
    let (base, gid) = guitar_project();
    let (source, buffer) = sine_source(440.0, 1.0, 44_100, 0.5);
    let shared_buffer = Arc::new(buffer);
    let render = |mutate: &dyn Fn(&mut ProjectModel)| {
        let mut m = base.clone();
        m.audio_clips = vec![AudioClip::new(gid, source.id, 0, 1.0)];
        m.audio_sources = vec![source.clone()];
        m.tracks[1].amp = Some(bypassed());
        mutate(&mut m);
        let map: HashMap<Uuid, Arc<AudioBuffer>> = [(source.id, Arc::clone(&shared_buffer))]
            .into_iter()
            .collect();
        let (mut e, shared) = engine_for(&m, Arc::new(AudioPool::new(map)));
        e.handle(EngineCommand::Play);
        let (l, r) = run(&mut e, secs(0.5), 512);
        (l, r, shared)
    };

    let (l, r, shared) = render(&|_| {});
    assert!(peak(&l) > 0.1, "the audio track made no sound");
    // Track 1 is the guitar: its meter follows the clip.
    assert!(
        shared.track_peak(1) > 0.05,
        "the audio track did not meter: {}",
        shared.track_peak(1)
    );
    let centred = (peak(&l), peak(&r));

    let (l, r, _) = render(&|m| m.tracks[1].pan = -1.0);
    assert!(
        peak(&l) > peak(&r) * 4.0,
        "pan left did not move the energy: {} vs {}",
        peak(&l),
        peak(&r)
    );
    assert!(peak(&l) >= centred.0 * 0.9, "panning hard left lost level");

    let (l, r, _) = render(&|m| m.tracks[1].muted = true);
    assert!(
        peak(&l) < 1e-5 && peak(&r) < 1e-5,
        "a muted audio track still sounded"
    );

    let (l, _r, _) = render(&|m| m.tracks[1].volume = 0.25);
    assert!(
        (peak(&l) - centred.0 * 0.25).abs() < 0.02,
        "volume did not scale the audio track"
    );
}

#[test]
fn a_clip_whose_source_is_missing_plays_silence_rather_than_panicking() {
    // Opening a project on a machine where the audio store is gone must not take the app down.
    let (mut m, gid) = guitar_project();
    let (source, _buffer) = sine_source(440.0, 1.0, 44_100, 0.5);
    m.audio_clips = vec![AudioClip::new(gid, source.id, 0, 1.0)];
    m.audio_sources = vec![source];
    m.tracks[1].amp = Some(bypassed());

    let (mut e, _shared) = engine_for(&m, Arc::new(AudioPool::default()));
    e.handle(EngineCommand::Play);
    let (l, _r) = run(&mut e, secs(0.5), 512);
    assert!(peak(&l) < 1e-6, "a missing source produced sound");
    assert_finite(&l, "missing source");
}

#[test]
fn a_muted_clip_is_silent_but_the_track_still_plays_its_others() {
    let (mut m, gid) = guitar_project();
    let (source, buffer) = sine_source(440.0, 0.5, 44_100, 0.5);
    let mut quiet = AudioClip::new(gid, source.id, 0, 0.5);
    quiet.muted = true;
    // Beat 2 at 120 bpm is 1.0 s.
    m.audio_clips = vec![quiet, AudioClip::new(gid, source.id, 2 * B, 0.5)];
    m.audio_sources = vec![source.clone()];
    m.tracks[1].amp = Some(bypassed());

    let (mut e, _shared) = engine_for(&m, pool(vec![(&source, buffer)]));
    e.handle(EngineCommand::Play);
    let (l, _r) = run(&mut e, secs(2.0), 512);
    assert!(peak(&l[..secs(0.4)]) < 1e-5, "a muted clip sounded");
    assert!(
        peak(&l[secs(1.1)..secs(1.4)]) > 0.1,
        "the unmuted clip did not sound"
    );
}

// ─── monitoring latency ──────────────────────────────────────────────────────────────────────

/// Drive the engine with a matched input stream: `frames` samples pushed per block, exactly
/// as a capture device running at the same rate would. Returns the backlog after each block.
fn monitor_backlogs(block: usize, blocks: usize, feed: impl Fn(usize) -> usize) -> Vec<u32> {
    let (mut m, gid) = guitar_project();
    m.tracks[1].amp = Some(bypassed());
    let shared = Arc::new(Shared::default());
    let mut e = Engine::new(SR, Arc::clone(&shared), &MasterSettings::default());
    let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(SR as usize);
    let (capture, _sink) = rtrb::RingBuffer::<f32>::new(1024);
    e.attach_io(consumer, capture);
    e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(
        &m,
    ))));
    e.handle(EngineCommand::SetAudioArm {
        track_id: Some(gid),
        monitor: true,
    });

    let mut out = vec![0.0f32; block * 2];
    let mut seen = Vec::with_capacity(blocks);
    for i in 0..blocks {
        for _ in 0..feed(i) {
            let _ = producer.push(0.25);
        }
        e.process(&mut out);
        seen.push(shared.input_backlog());
    }
    seen
}

#[test]
fn monitoring_holds_well_under_one_block_of_input() {
    // Everything left in the ring after a block is delay the player hears between the string
    // and the speaker, on top of the two device blocks. Holding whole blocks "to be safe" is
    // what made this unplayable: 512 frames with three blocks of slack is 50 ms round trip.
    for block in [128usize, 256, 512] {
        // A device that has run far ahead — the worst case, and what happens at startup while
        // the capture stream warms up before the first output callback.
        let backlogs = monitor_backlogs(block, 200, |i| if i == 0 { block * 20 } else { block });
        let settled = &backlogs[8..];
        let worst = *settled.iter().max().unwrap();
        assert!(
            (worst as usize) <= block,
            "block {block}: {worst} frames still queued after settling — that is over a block of delay"
        );
        // And the startup flood is cleared rather than carried forever.
        assert!(
            backlogs[0] as usize <= block * 2,
            "block {block}: a run-ahead input left {} frames queued on the first block",
            backlogs[0]
        );
    }
}

#[test]
fn a_matched_input_stream_never_runs_dry() {
    // The flip side: trimming hard must not cost a click every block.
    let shared_underruns = {
        let (mut m, gid) = guitar_project();
        m.tracks[1].amp = Some(bypassed());
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(SR, Arc::clone(&shared), &MasterSettings::default());
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(SR as usize);
        let (capture, _sink) = rtrb::RingBuffer::<f32>::new(1024);
        e.attach_io(consumer, capture);
        e.handle(EngineCommand::SetSequence(Arc::new(Sequence::from_song(
            &m,
        ))));
        e.handle(EngineCommand::SetAudioArm {
            track_id: Some(gid),
            monitor: true,
        });

        let block = 256usize;
        let mut out = vec![0.0f32; block * 2];
        // Prime the ring the way a capture stream does, then stay matched.
        for _ in 0..block * 2 {
            let _ = producer.push(0.25);
        }
        for _ in 0..400 {
            e.process(&mut out);
            for _ in 0..block {
                let _ = producer.push(0.25);
            }
        }
        shared.input_underruns()
    };
    assert_eq!(
        shared_underruns, 0,
        "a matched input stream underran {shared_underruns} times"
    );
}

#[test]
fn a_starved_input_buys_slack_and_gives_it_back() {
    // A device that cannot keep up should stop clicking rather than stay broken — and once it
    // settles, the latency it bought has to come back.
    let block = 256usize;
    // Feed nothing every third block: the ring runs dry and the slack has to grow.
    let starved = monitor_backlogs(block, 60, |i| if i % 3 == 0 { 0 } else { block * 3 / 2 });
    let after_starving = *starved.last().unwrap() as usize;
    assert!(
        after_starving > block / 2,
        "the engine never bought slack for a starving device"
    );

    // Then a long matched run: it must ease back toward the floor rather than keep the delay.
    let recovered = monitor_backlogs(
        block,
        4_000,
        |i| if i % 3 == 0 && i < 60 { 0 } else { block },
    );
    let settled = *recovered.last().unwrap() as usize;
    assert!(
        settled <= block,
        "the engine kept {settled} frames of slack long after the device settled"
    );
}

// ─── the amp's components ────────────────────────────────────────────────────────────────────
//
// The head, the tone stack, the cabinet and the microphone are chosen independently, so each one
// is audited on its own: drive the rig directly, hold everything else still, move one thing.

/// Half a second of settled steady tone through `rig` alone. 22 050 samples at 44.1 kHz puts
/// every frequency these tests measure on an exact Goertzel bin.
fn rig_tone(rig: &AmpRig, hz: f64, amplitude: f32) -> Vec<f32> {
    let mut r = Rig::new(SR);
    r.apply(rig);
    let n = 44_100;
    let input: Vec<f32> = (0..n)
        .map(|i| amplitude * (2.0 * PI * hz * i as f64 / SRD).sin() as f32)
        .collect();
    let (mut l, mut right) = (vec![0.0; n], vec![0.0; n]);
    r.process_add(&input, &mut l, &mut right);
    l[n / 2..].to_vec()
}

/// A rig with nothing but the amp in it: no pedals, no gate, no reverb.
fn head_only(model: AmpModel, gain: f64) -> AmpRig {
    let mut rig = bare(model, gain);
    rig.cab.model = CabModel::Direct;
    rig
}

fn level_db(a: f64, b: f64) -> f64 {
    20.0 * (a / b.max(1e-12)).log10()
}

#[test]
fn the_preamp_clips_asymmetrically_like_a_real_tube() {
    // A triode cuts off abruptly and saturates gently, so a driven stage is lopsided and throws
    // *even* harmonics. A symmetric soft-clipper cannot: it only ever produces odd ones. This is
    // the whole reason the preamp reads a plate curve instead of calling `tanh`.
    let driven = rig_tone(&head_only(AmpModel::Crunch, 8.0), 220.0, 0.3);
    let h1 = tone_amplitude(&driven, 220.0);
    let h2 = tone_amplitude(&driven, 440.0);
    let h3 = tone_amplitude(&driven, 660.0);
    assert!(
        h2 > h1 * 0.15,
        "a driven tube stage made almost no second harmonic: {h2} against {h1}"
    );
    assert!(
        h2 > h3 * 1.3,
        "the second harmonic did not lead the third: {h2} against {h3}"
    );

    // And the same stage left alone is clean, so the asymmetry is the drive and not a permanent
    // lopsidedness baked into the curve.
    let clean = rig_tone(&head_only(AmpModel::Clean, 1.0), 220.0, 0.3);
    let c1 = tone_amplitude(&clean, 220.0);
    assert!(
        tone_amplitude(&clean, 440.0) < c1 * 0.05,
        "an undriven stage is not supposed to distort"
    );
}

#[test]
fn the_tone_stack_controls_load_each_other() {
    // A passive tone stack is one network, not three bands: moving the mid pot drags the treble
    // response with it. If these ever became independent filters this is the test that notices.
    for eq in EqModel::ALL {
        let at = |mid: f64| {
            let mut rig = head_only(AmpModel::Clean, 2.0);
            rig.eq = eq;
            rig.amp.mid = mid;
            tone_amplitude(&rig_tone(&rig, 2_500.0, 0.1), 2_500.0)
        };
        let moved = level_db(at(10.0), at(0.0));
        assert!(
            moved > 1.5,
            "{eq:?}: the mid knob moved 2.5 kHz by only {moved:.2} dB, so the stack is not interacting"
        );
    }
}

#[test]
fn switching_tone_stack_changes_the_voice_not_the_volume() {
    // Each topology makes up its own insertion loss, so the stack list is a tonal choice rather
    // than a volume control — otherwise nobody can hear what the choice actually does.
    let mut levels = Vec::new();
    let mut shapes = Vec::new();
    for eq in EqModel::ALL {
        let mut rig = head_only(AmpModel::Clean, 2.0);
        rig.eq = eq;
        let band: Vec<f64> = [200.0, 800.0, 3_200.0]
            .into_iter()
            .map(|f| tone_amplitude(&rig_tone(&rig, f, 0.1), f))
            .collect();
        levels.push(band.iter().sum::<f64>() / band.len() as f64);
        shapes.push(level_db(band[2], band[0]));
    }
    let spread = level_db(
        levels.iter().cloned().fold(0.0, f64::max),
        levels.iter().cloned().fold(f64::MAX, f64::min),
    );
    assert!(
        spread < 4.0,
        "the tone stacks are {spread:.1} dB apart in level, which is a volume control"
    );

    let tilt = shapes.iter().cloned().fold(0.0, f64::max)
        - shapes.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        tilt > 3.0,
        "every tone stack has the same tilt ({tilt:.1} dB apart), so the choice does nothing"
    );
}

#[test]
fn moving_the_mic_across_the_cone_trades_top_end_for_nothing_else() {
    // The cone beams its highs forward: on the dust cap the mic hears them, out at the surround it
    // does not. The midrange must stay put, or the control reads as a volume rather than a tone.
    let at = |pos_x: f64, hz: f64| {
        let mut rig = bare(AmpModel::Clean, 2.0);
        rig.cab = Cabinet {
            model: CabModel::FourByTwelve,
            mic_model: MicModel::Dynamic,
            pos_x,
            pos_z: 2.0,
        };
        tone_amplitude(&rig_tone(&rig, hz, 0.1), hz)
    };
    let top = level_db(at(10.0, 4_000.0), at(0.0, 4_000.0));
    assert!(
        top > 8.0,
        "the dust cap was only {top:.1} dB brighter than the cone's edge at 4 kHz"
    );
    let mids = level_db(at(10.0, 1_000.0), at(0.0, 1_000.0)).abs();
    assert!(
        mids < 2.0,
        "moving the mic across the cone shifted 1 kHz by {mids:.1} dB"
    );
}

#[test]
fn backing_the_mic_off_the_grille_loses_the_proximity_bass() {
    let at = |pos_z: f64, hz: f64| {
        let mut rig = bare(AmpModel::Clean, 2.0);
        rig.cab = Cabinet {
            model: CabModel::FourByTwelve,
            mic_model: MicModel::Dynamic,
            pos_x: 5.0,
            pos_z,
        };
        tone_amplitude(&rig_tone(&rig, hz, 0.1), hz)
    };
    let lows = level_db(at(10.0, 100.0), at(0.0, 100.0));
    assert!(
        lows < -3.0,
        "pulling the mic off the grille only changed 100 Hz by {lows:.1} dB"
    );
}

#[test]
fn each_microphone_hears_a_different_top_end() {
    let at = |mic: MicModel, hz: f64| {
        let mut rig = bare(AmpModel::Clean, 2.0);
        rig.cab = Cabinet {
            model: CabModel::FourByTwelve,
            mic_model: mic,
            pos_x: 5.0,
            pos_z: 2.0,
        };
        tone_amplitude(&rig_tone(&rig, hz, 0.1), hz)
    };
    let (ribbon, dynamic, condenser) = (
        at(MicModel::Ribbon, 10_000.0),
        at(MicModel::Dynamic, 10_000.0),
        at(MicModel::Condenser, 10_000.0),
    );
    assert!(
        ribbon < dynamic && dynamic < condenser,
        "the mics are not ordered dark to bright at 10 kHz: ribbon {ribbon}, dynamic {dynamic}, condenser {condenser}"
    );
    assert!(
        level_db(condenser, ribbon) > 6.0,
        "a ribbon and a condenser sound the same up top, which cannot be right"
    );
}

#[test]
fn grid_conduction_droops_the_bias_and_then_lets_go() {
    // Drive the grid positive and the coupling capacitor charges, leaving the stage idling colder
    // until it bleeds back. A quiet note right after a loud one is quieter than the same note
    // before it — and tens of milliseconds later the amp is itself again.
    let mut rig = bare(AmpModel::Crunch, 5.0);
    rig.cab.model = CabModel::Direct;
    let mut r = Rig::new(SR);
    r.apply(&rig);
    let n = 44_100 * 3 / 5;
    let input: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / SRD;
            let env = if (0.15..0.25).contains(&t) { 0.9 } else { 0.05 };
            (env * (2.0 * PI * 150.0 * t).sin()) as f32
        })
        .collect();
    let (mut l, mut right) = (vec![0.0; n], vec![0.0; n]);
    r.process_add(&input, &mut l, &mut right);

    // 1176 samples is exactly four cycles of 150 Hz at 44.1 kHz, so every window lands on the bin.
    let at = |t: f64| {
        let start = (t * SRD) as usize;
        tone_amplitude(&l[start..start + 1_176], 150.0)
    };
    let before = at(0.10);
    let just_after = level_db(at(0.28), before);
    let later = level_db(at(0.45), before);
    assert!(
        just_after < -0.15,
        "a loud burst left no bias droop behind it ({just_after:.2} dB)"
    );
    assert!(
        later.abs() < 0.1,
        "the stage never recovered its bias: still {later:.2} dB off"
    );
}

#[test]
fn the_cabinet_costs_no_latency() {
    // The speaker is a convolution, and a convolution is where latency usually sneaks into an amp.
    // Against the grille the response arrives at once, so monitoring a live DI stays immediate.
    let mut rig = bare(AmpModel::Clean, 5.0);
    rig.cab = Cabinet {
        model: CabModel::FourByTwelve,
        mic_model: MicModel::Dynamic,
        pos_x: 5.0,
        pos_z: 0.0,
    };
    let mut r = Rig::new(SR);
    r.apply(&rig);
    let mut input = vec![0.0f32; 64];
    input[0] = 1.0;
    let (mut l, mut right) = (vec![0.0; 64], vec![0.0; 64]);
    r.process_add(&input, &mut l, &mut right);
    let peak_at = |x: &[f32]| {
        x.iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0
    };
    let close = peak_at(&l);
    assert!(
        close < 12,
        "the close microphone's response peaked {close} samples late"
    );

    // Backed off, the sound has to travel, and that delay is the point rather than an accident.
    rig.cab.pos_z = 10.0;
    let mut far = Rig::new(SR);
    far.apply(&rig);
    let (mut fl, mut fr) = (vec![0.0; 64], vec![0.0; 64]);
    far.process_add(&input, &mut fl, &mut fr);
    assert!(
        peak_at(&fl) > close,
        "the far microphone heard the speaker no later than the close one"
    );
}

// ─── power supply and circuit switches ───────────────────────────────────────────────────────

/// Peak over `len` seconds starting at `t`. Sag has to be measured this way: the envelope *is*
/// the effect, so a Goertzel window long enough to be accurate would average it away.
fn window_peak(x: &[f32], t: f64, len: f64) -> f64 {
    let start = (t * SRD) as usize;
    let end = (start + (len * SRD) as usize).min(x.len());
    peak(&x[start..end]) as f64
}

/// Two loud 220 Hz bursts with a long rest between them, which is how a player finds out what
/// an amp's supply does. Returns `(droop over the first burst, how far the second burst's
/// attack came back)`, both in dB against the first attack.
///
/// The attack window is deliberately the first 9 ms — under two cycles, and shorter than any
/// rail's time constant. Measure later and the sag has already happened, which is what the amp
/// wants and the measurement does not.
fn sag_profile(rig: &AmpRig) -> (f64, f64) {
    let mut r = Rig::new(SR);
    r.apply(rig);
    let n = secs(1.8);
    let input: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / SRD;
            // Nearly a second of rest between the bursts: the reservoir refills in a couple of
            // hundred ms, but the cathode-bias capacitor behind it takes its time.
            let loud = (0.05..0.45).contains(&t) || t >= 1.35;
            ((if loud { 0.9 } else { 0.0 }) * (2.0 * PI * 220.0 * t).sin()) as f32
        })
        .collect();
    let (mut l, mut right) = (vec![0.0; n], vec![0.0; n]);
    r.process_add(&input, &mut l, &mut right);
    let attack = window_peak(&l, 0.051, 0.009);
    let settled = window_peak(&l, 0.41, 0.02);
    let again = window_peak(&l, 1.351, 0.009);
    (level_db(settled, attack), level_db(again, attack))
}

/// A head with its master wide open and plenty of signal arriving, so the measurement is of the
/// power stage rather than of whatever the preamp did first. `gain` chooses how much of a front
/// end is in the way.
fn cranked(model: AmpModel, gain: f64) -> AmpRig {
    let mut rig = head_only(model, gain);
    rig.amp.master = 10.0;
    rig.input_db = 12.0;
    rig
}

#[test]
fn the_supply_sags_under_a_sustained_note_and_climbs_back() {
    // Hold a loud note into a tweed and the rail falls away under it: the attack gets out at
    // full level and what follows is squashed. Stop, and the rectifier refills the reservoir, so
    // the next attack is back where the first one was. That is the whole feel of a small amp.
    let (droop, recovery) = sag_profile(&cranked(AmpModel::Clean, 7.0));
    assert!(
        droop < -2.0,
        "a held note never pulled the rail down ({droop:.2} dB)"
    );
    assert!(
        recovery.abs() < 0.5,
        "the supply never climbed back: the second attack is {recovery:.2} dB off the first"
    );
}

#[test]
fn a_tweed_sags_and_a_modern_head_does_not() {
    // Baked per model, not on a knob. A small cathode-biased combo on a rectifier tube folds;
    // a modern head with solid-state rectification and a bank of capacitors barely moves.
    let (tweed, _) = sag_profile(&cranked(AmpModel::Clean, 7.0));
    let (modern, _) = sag_profile(&cranked(AmpModel::Lead, 7.0));
    assert!(
        tweed < modern - 1.5,
        "the tweed sagged no further than the modern head ({tweed:.2} against {modern:.2} dB)"
    );
    assert!(
        modern > -1.0,
        "the modern head sagged like a tweed ({modern:.2} dB)"
    );
}

#[test]
fn the_bright_cap_returns_the_top_and_gives_it_up_as_the_gain_opens() {
    // A treble bypass across the gain pot can only hand back what the pot is throwing away. Down
    // low it is most of the amp's voice; wide open the wiper is at the top and the cap is out of
    // the circuit, which is why nobody notices the switch on a cranked amp.
    let lift = |gain: f64| {
        let mut off = head_only(AmpModel::Clean, gain);
        off.amp.master = 3.0;
        let mut on = off.clone();
        on.amp.bright_cap = true;
        let at = |rig: &AmpRig, hz: f64| tone_amplitude(&rig_tone(rig, hz, 0.1), hz);
        (
            level_db(at(&on, 3_000.0), at(&off, 3_000.0)),
            level_db(at(&on, 150.0), at(&off, 150.0)),
        )
    };
    let (top_low, bottom_low) = lift(2.0);
    let (top_high, _) = lift(10.0);
    assert!(
        top_low > 4.0,
        "the bright cap returned no top end at a low gain setting ({top_low:.2} dB)"
    );
    assert!(
        bottom_low.abs() < 0.5,
        "the bright cap moved the bottom end too ({bottom_low:.2} dB), so it is a tone control"
    );
    assert!(
        top_high < top_low * 0.25,
        "the bright cap still lifted {top_high:.2} dB with the gain wide open, against \
         {top_low:.2} dB at 2"
    );
}

#[test]
fn half_power_lowers_the_headroom_and_sags_further() {
    // Pulling half the output tubes trades volume for break-up: the stage arrives at its ceiling
    // sooner, so the gap between playing quietly and playing hard closes, and the supply behind
    // it gives way further. If it only turned everything down it would be a master volume.
    let full = cranked(AmpModel::Clean, 2.0);
    let mut half = full.clone();
    half.amp.half_power = true;

    let level = |rig: &AmpRig, amp: f32| peak(&rig_tone(rig, 150.0, amp)) as f64;
    let span = |rig: &AmpRig| level_db(level(rig, 0.6), level(rig, 0.06));
    let (full_span, half_span) = (span(&full), span(&half));
    let (full_droop, _) = sag_profile(&full);
    let (half_droop, _) = sag_profile(&half);
    assert!(
        half_span < full_span - 1.0,
        "half power left the headroom alone: {half_span:.2} dB of range against {full_span:.2} dB"
    );
    assert!(
        half_droop < full_droop - 1.0,
        "half power sagged no deeper ({half_droop:.2} against {full_droop:.2} dB)"
    );
}

#[test]
fn the_cathode_follower_squashes_before_the_tone_stack() {
    // A follower has no voltage gain, so if it did nothing the switch would be a lie. What it
    // does is source far more current than it sinks: hard playing bends the top of the wave
    // against grid conduction while the bottom is still clean, which compresses and throws even
    // harmonics — and it happens ahead of the stack, where the stack can still colour it.
    let mut off = head_only(AmpModel::Crunch, 4.0);
    off.amp.master = 4.0;
    let mut on = off.clone();
    on.amp.cathode_follower = true;

    let fundamental = |rig: &AmpRig, amp: f32| tone_amplitude(&rig_tone(rig, 220.0, amp), 220.0);
    let span = |rig: &AmpRig| level_db(fundamental(rig, 0.6), fundamental(rig, 0.15));
    let (off_span, on_span) = (span(&off), span(&on));

    let loud = rig_tone(&on, 220.0, 0.6);
    let quiet_loud = rig_tone(&off, 220.0, 0.6);
    let even = |x: &[f32]| tone_amplitude(x, 440.0) / tone_amplitude(x, 220.0);
    let (off_even, on_even) = (even(&quiet_loud), even(&loud));
    assert!(
        on_span < off_span - 0.75,
        "the follower compressed nothing: {on_span:.2} dB of range against {off_span:.2} dB"
    );
    assert!(
        on_even > off_even * 1.15,
        "the follower clipped symmetrically ({on_even:.4} against {off_even:.4}), which is not \
         what grid conduction does"
    );
}

#[test]
fn a_switch_the_circuit_never_had_does_nothing() {
    // The switches are per model, and the engine is the last line of that promise: a rig that
    // arrives claiming a bright cap on a fuzz voicing has to sound exactly as it did without it.
    let plain = head_only(AmpModel::Fuzz, 7.0);
    let mut claimed = plain.clone();
    claimed.amp.bright_cap = true;
    claimed.amp.half_power = true;
    claimed.amp.cathode_follower = true;
    let a = rig_tone(&plain, 220.0, 0.4);
    let b = rig_tone(&claimed, 220.0, 0.4);
    assert_eq!(a, b, "a switch the fuzz voicing does not have changed its sound");
}
