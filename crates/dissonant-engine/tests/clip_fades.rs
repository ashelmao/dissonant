//! Behavioural audit of clip fades and the punch crossfade they build.
//!
//! Lane-level and offline: an [`AudioLane`] is driven directly with a hand-built pool, so no
//! device, no files and no pan law stand between the clip and the assertion. A DC source means
//! every rendered sample *is* the envelope, so the curve is read off the output rather than
//! inferred from a peak.

use std::collections::HashMap;
use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

use dissonant_core::{
    AmpRig, AudioSource, ProjectModel, Sequence, SequenceAudioClip, SequenceTrack, Track,
    DEFAULT_PUNCH_CROSSFADE_SECONDS,
};
use dissonant_engine::audio::{AudioBuffer, AudioLane, AudioPool, BlockContext};
use uuid::Uuid;

const SR: f32 = 48_000.0;
const SRD: f64 = 48_000.0;
const XF: f64 = DEFAULT_PUNCH_CROSSFADE_SECONDS;

// ─── helpers ─────────────────────────────────────────────────────────────────────────────────

/// A constant `level` source: whatever comes out of the lane is the envelope, times `level`.
fn dc_source(seconds: f64, level: f32) -> (AudioSource, AudioBuffer) {
    let frames = (seconds * SRD) as usize;
    let source = AudioSource {
        id: Uuid::new_v4(),
        name: "dc".into(),
        frames: frames as u64,
        sample_rate: SR as u32,
        channels: 1,
    };
    (source, AudioBuffer::new(SR as u32, 1, vec![level; frames]))
}

/// Two takes of the same part are not the same waveform, so a crossfade between them is a
/// crossfade between uncorrelated signals. Deterministic, seeded per clip.
fn noise_source(seconds: f64, level: f32, seed: u64) -> (AudioSource, AudioBuffer) {
    let frames = (seconds * SRD) as usize;
    let mut state = seed | 1;
    let data: Vec<f32> = (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            level * ((state >> 40) as f32 / 8_388_608.0 - 1.0)
        })
        .collect();
    let source = AudioSource {
        id: Uuid::new_v4(),
        name: "noise".into(),
        frames: frames as u64,
        sample_rate: SR as u32,
        channels: 1,
    };
    (source, AudioBuffer::new(SR as u32, 1, data))
}

/// A flattened audio track whose rig is off, so the lane measures the clip alone.
fn bypassed_track(id: Uuid) -> SequenceTrack {
    let mut m = ProjectModel::empty();
    let mut t = Track::audio("guitar");
    t.id = id;
    t.amp = Some(AmpRig {
        enabled: false,
        ..AmpRig::default()
    });
    m.tracks.push(t);
    Sequence::from_song(&m)
        .tracks
        .into_iter()
        .find(|t| t.id == id)
        .expect("the track is in the sequence")
}

/// `seconds` of the left channel with `clips` playing from time 0.
fn render(clips: &[SequenceAudioClip], sources: Vec<(&AudioSource, AudioBuffer)>, seconds: f64) -> Vec<f32> {
    let track_id = clips[0].track_id;
    let map: HashMap<Uuid, Arc<AudioBuffer>> = sources
        .into_iter()
        .map(|(s, b)| (s.id, Arc::new(b)))
        .collect();
    let pool = AudioPool::new(map);
    let mut lane = AudioLane::new(track_id, SR);
    lane.apply(&bypassed_track(track_id));
    let total = (seconds * SRD) as usize;
    let block = 256usize;
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
            clips,
            input: &[],
        };
        lane.render_add(&mut l, &mut r, &ctx, SR);
        out.extend_from_slice(&l);
        done += frames;
    }
    out
}

fn clip(track_id: Uuid, source: &AudioSource, start: f64, length: f64) -> SequenceAudioClip {
    SequenceAudioClip {
        track_id,
        source_id: source.id,
        start_seconds: start,
        length_seconds: length,
        offset_seconds: 0.0,
        gain: 1.0,
        fade_in_seconds: 0.0,
        fade_out_seconds: 0.0,
    }
}

fn rms(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt()
}

fn at(x: &[f32], seconds: f64) -> f64 {
    x[(seconds * SRD) as usize] as f64
}

// ─── the curve ───────────────────────────────────────────────────────────────────────────────

#[test]
fn a_fade_is_equal_power_not_linear() {
    let (source, buffer) = dc_source(1.0, 1.0);
    let track_id = Uuid::new_v4();
    let mut c = clip(track_id, &source, 0.0, 1.0);
    c.fade_in_seconds = 0.2;
    c.fade_out_seconds = 0.2;
    let out = render(&[c], vec![(&source, buffer)], 1.0);

    // Halfway through the fade a linear ramp would be at 0.5; equal power is at sin(π/4).
    let half = at(&out, 0.1);
    let wanted = (0.5 * FRAC_PI_2).sin();
    assert!(
        (half - wanted).abs() < 0.01,
        "halfway up the fade the gain was {half}, wanted {wanted}"
    );
    assert!(half > 0.6, "a linear fade would be at 0.5 here, got {half}");
    // Ends: silent at the very start, full by the top of the fade, silent again at the end.
    assert!(at(&out, 0.0).abs() < 0.01, "{}", at(&out, 0.0));
    assert!((at(&out, 0.2) - 1.0).abs() < 0.01, "{}", at(&out, 0.2));
    assert!((at(&out, 0.5) - 1.0).abs() < 1e-4, "no fade in the middle");
    assert!(at(&out, 0.999).abs() < 0.02, "{}", at(&out, 0.999));

    // Monotonic the whole way up and the whole way down, with no steps.
    let rising = (0..(0.2 * SRD) as usize).all(|i| out[i + 1] >= out[i]);
    assert!(rising, "the fade-in is not monotonic");
    let from = (0.8 * SRD) as usize;
    let falling = (from..out.len() - 1).all(|i| out[i + 1] <= out[i]);
    assert!(falling, "the fade-out is not monotonic");
}

// ─── the punch seam ──────────────────────────────────────────────────────────────────────────

#[test]
fn a_punch_seam_sums_to_one_in_power() {
    // The geometry `document.rs::punch_split` lays down: the head runs one crossfade past the
    // take's start and fades out over it, the take fades in over the same window.
    let track_id = Uuid::new_v4();
    let (head_src, head_buf) = dc_source(1.0, 1.0);
    let (take_src, take_buf) = dc_source(1.0, 1.0);
    let mut head = clip(track_id, &head_src, 0.0, 0.5 + XF);
    head.fade_out_seconds = XF;
    let mut take = clip(track_id, &take_src, 0.5, 0.5);
    take.fade_in_seconds = XF;

    // Rendered apart, so each sample is that clip's own gain rather than their sum.
    let a = render(&[head], vec![(&head_src, head_buf)], 1.0);
    let b = render(&[take], vec![(&take_src, take_buf)], 1.0);

    let from = (0.5 * SRD) as usize;
    let to = ((0.5 + XF) * SRD) as usize;
    let mut worst = 0.0f64;
    for i in from..to {
        let (ga, gb) = (a[i] as f64, b[i] as f64);
        worst = worst.max((ga * ga + gb * gb - 1.0).abs());
    }
    assert!(
        worst < 0.01,
        "the seam is not equal power: squared gains were off by {worst}"
    );
    // And it really is a crossfade: both sides are sounding in the middle of it.
    let mid = from + (to - from) / 2;
    assert!(a[mid] > 0.5 && b[mid] > 0.5, "{} / {}", a[mid], b[mid]);
}

#[test]
fn a_punch_between_two_takes_holds_its_level() {
    // What the player hears: two different performances of the same part, crossfaded. Equal
    // power is the curve that keeps uncorrelated material at one level across the join; a
    // linear pair dips about 3 dB in the middle.
    let track_id = Uuid::new_v4();
    let (head_src, head_buf) = noise_source(1.0, 0.5, 0x2545_F491_4F6C_DD1D);
    let (take_src, take_buf) = noise_source(1.0, 0.5, 0x9E37_79B9_7F4A_7C15);
    let mut head = clip(track_id, &head_src, 0.0, 0.5 + XF);
    head.fade_out_seconds = XF;
    let mut take = clip(track_id, &take_src, 0.5, 0.5);
    take.fade_in_seconds = XF;
    let out = render(
        &[head, take],
        vec![(&head_src, head_buf), (&take_src, take_buf)],
        1.0,
    );

    let before = rms(&out[((0.5 - XF) * SRD) as usize..(0.5 * SRD) as usize]);
    let seam = rms(&out[(0.5 * SRD) as usize..((0.5 + XF) * SRD) as usize]);
    let after = rms(&out[((0.5 + XF) * SRD) as usize..((0.5 + 2.0 * XF) * SRD) as usize]);
    let steady = (before + after) * 0.5;
    assert!(
        (seam / steady - 1.0).abs() < 0.1,
        "the join dips or bumps: {seam} against {steady}"
    );
}
