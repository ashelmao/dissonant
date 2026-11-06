//! Krumhansl–Schmuckler key finding: a 12-bin pitch-class histogram Pearson-correlated against
//! all 24 major/minor key profiles. Accurate on lots of notes, unreliable on a handful — so
//! confidence gates on note count and the top-two margin, and callers must never auto-lock an
//! unconfident result.
//!
//! The histogram can also carry chroma heard in a recorded DI ([`KeyDetector::detect_with_chroma`]),
//! already scaled to note equivalents, so a strummed riff is evidence on the same footing as
//! the notes placed in the roll.

use super::normalize;
use crate::model::ScaleType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyCandidate {
    pub root_pitch_class: i32,
    pub scale: ScaleType,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyDetectionResult {
    /// Ranked best-first.
    pub candidates: Vec<KeyCandidate>,
    /// Confident enough to *suggest* a lock.
    pub is_confident: bool,
}

impl KeyDetectionResult {
    pub fn top(&self) -> Option<&KeyCandidate> {
        self.candidates.first()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KeyDetector {
    /// Minimum number of placed notes before a result can be confident (~10–12).
    pub min_notes_for_confidence: usize,
    /// Minimum correlation margin between the top two candidates.
    pub min_score_gap: f64,
    /// Minimum number of *distinct* pitch classes. Repetition is not evidence: a riff that
    /// hammers two or three pitches sits inside half a dozen keys no matter how many times it
    /// repeats, and four distinct pitches is still just one chord (a C–E–G–B arpeggio correlates
    /// best with E minor). Five is the first count that describes a scale rather than a chord.
    pub min_distinct_pitch_classes: usize,
}

impl Default for KeyDetector {
    fn default() -> Self {
        KeyDetector {
            min_notes_for_confidence: 10,
            min_score_gap: 0.04,
            min_distinct_pitch_classes: 5,
        }
    }
}

// Krumhansl–Kessler profiles.
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

impl KeyDetector {
    pub fn detect(&self, pitch_classes: &[i32]) -> KeyDetectionResult {
        self.detect_with_chroma(pitch_classes, &[0.0; 12])
    }

    /// Placed notes plus detected chroma. `chroma` is per pitch class (0 = C) in note
    /// equivalents — the scale [`crate::model::ClipDetection::chroma`] is stored on — so a bin
    /// worth three strums weighs what three placed notes do, and every gate below reads the
    /// combined evidence. A pitch class counts as distinct from half a note's worth up: a whole
    /// placed note always does, and the faint harmonic leakage chroma carries never does.
    pub fn detect_with_chroma(&self, pitch_classes: &[i32], chroma: &[f32; 12]) -> KeyDetectionResult {
        let mut histogram = [0.0f64; 12];
        for &pc in pitch_classes {
            histogram[normalize(pc) as usize] += 1.0;
        }
        for (bin, &c) in histogram.iter_mut().zip(chroma) {
            if c.is_finite() && c > 0.0 {
                *bin += c as f64;
            }
        }
        let evidence: f64 = histogram.iter().sum();
        if evidence <= 0.0 {
            return KeyDetectionResult {
                candidates: vec![],
                is_confident: false,
            };
        }

        let mut candidates = Vec::with_capacity(24);
        for root in 0..12 {
            for scale in [ScaleType::Major, ScaleType::Minor] {
                let profile = rotated_profile(scale, root);
                candidates.push(KeyCandidate {
                    root_pitch_class: root,
                    scale,
                    score: pearson(&histogram, &profile),
                });
            }
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));

        let gap = if candidates.len() >= 2 {
            candidates[0].score - candidates[1].score
        } else {
            1.0
        };
        let distinct = histogram.iter().filter(|&&h| h >= 0.5).count();
        let is_confident = evidence >= self.min_notes_for_confidence as f64
            && distinct >= self.min_distinct_pitch_classes
            && gap >= self.min_score_gap;
        KeyDetectionResult {
            candidates,
            is_confident,
        }
    }
}

fn rotated_profile(scale: ScaleType, root: i32) -> [f64; 12] {
    let base = match scale {
        ScaleType::Major => &MAJOR_PROFILE,
        ScaleType::Minor => &MINOR_PROFILE,
    };
    let mut out = [0.0; 12];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = base[normalize(i as i32 - root) as usize];
    }
    out
}

fn pearson(x: &[f64; 12], y: &[f64; 12]) -> f64 {
    let n = 12.0;
    let mx = x.iter().sum::<f64>() / n;
    let my = y.iter().sum::<f64>() / n;
    let (mut num, mut dx2, mut dy2) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let a = x[i] - mx;
        let b = y[i] - my;
        num += a * b;
        dx2 += a * a;
        dy2 += b * b;
    }
    let den = (dx2 * dy2).sqrt();
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_c_major_scale_ranks_c_major_first() {
        let notes: Vec<i32> = [0, 2, 4, 5, 7, 9, 11, 0, 4, 7, 0, 7].to_vec();
        let r = KeyDetector::default().detect(&notes);
        let top = r.top().unwrap();
        assert_eq!((top.root_pitch_class, top.scale), (0, ScaleType::Major));
        assert!(r.is_confident);
    }

    #[test]
    fn a_minor_fragment_ranks_a_minor_above_c_major() {
        let notes: Vec<i32> = [9, 9, 0, 4, 9, 11, 0, 4, 8, 9, 9, 4].to_vec(); // leans on A, E, G#
        let r = KeyDetector::default().detect(&notes);
        let a_minor = r
            .candidates
            .iter()
            .position(|c| c.root_pitch_class == 9 && c.scale == ScaleType::Minor)
            .unwrap();
        let c_major = r
            .candidates
            .iter()
            .position(|c| c.root_pitch_class == 0 && c.scale == ScaleType::Major)
            .unwrap();
        assert!(a_minor < c_major);
    }

    #[test]
    fn sparse_input_is_not_confident() {
        let r = KeyDetector::default().detect(&[0, 4, 7]);
        assert!(!r.is_confident);
        assert_eq!(r.candidates.len(), 24);
    }

    #[test]
    fn empty_input_has_no_candidate() {
        let r = KeyDetector::default().detect(&[]);
        assert!(r.top().is_none());
        assert!(!r.is_confident);
    }

    #[test]
    fn adding_one_note_can_flip_top_but_stays_unconfident_when_few() {
        let d = KeyDetector::default();
        let a = d.detect(&[0, 4, 7, 9]);
        let b = d.detect(&[0, 4, 7, 9, 5]);
        assert!(!a.is_confident && !b.is_confident);
    }
}
