//! Loudness of every factory sound through the real engines, so the library sits at a
//! consistent level: a sound much quieter or louder than its neighbours is a tuning bug.

use dissonant_core::{factory_sounds, SoundPatch};
use dissonant_engine::render_drum;
use dissonant_engine::synth::PolySynth;

const SR: f32 = 48_000.0;

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// A two-second C-major triad at velocity 100 (long enough for slow attacks to arrive).
fn synth_level(patch: &dissonant_core::SynthPatch) -> (f32, f32) {
    let mut s = PolySynth::new(patch, SR, 12);
    for p in [60, 64, 67] {
        s.note_on(p, 100);
    }
    let mut buf = vec![0.0; 2 * SR as usize];
    s.render_add(&mut buf);
    (rms(&buf), peak(&buf))
}

#[test]
fn factory_levels_are_consistent() {
    let mut report = vec![];
    for sound in factory_sounds() {
        let (r, p) = match &sound.patch {
            SoundPatch::Synth(patch) => synth_level(patch),
            SoundPatch::Drum(patch) => {
                let hit = render_drum(patch, SR);
                // A drum's loudness is its peak; RMS over the tail says little.
                (rms(&hit[..hit.len().min((0.05 * SR) as usize)]), peak(&hit))
            }
        };
        report.push((sound.id.clone(), sound.category.clone(), r, p));
    }
    for (id, cat, r, p) in &report {
        println!("{cat:>8} {id:<22} rms {r:.3} peak {p:.3}");
    }
    for (id, _, r, p) in &report {
        assert!(r.is_finite() && *p < 2.0, "{id} is too loud (peak {p:.2})");
        assert!(*p > 0.05, "{id} is nearly silent (peak {p:.3})");
    }
}
