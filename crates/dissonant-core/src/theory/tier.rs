//! The three-tier classifier — the product's core mechanic.
//!
//! Rule: chord tones win; otherwise a pitch a half step *above* a chord tone is dissonance (the
//! classic avoid note — F over C, C over G7); an out-of-key pitch is dissonance; everything else
//! is a tension. The rule is deliberately one-directional: a pitch a half step *below* a chord
//! tone is a leading tone into it (B under C, E under F, the 13th of a dominant) and is one of
//! the best-sounding tensions there is, so flagging it red would be musically wrong.

use super::normalize;
use crate::model::PitchPoint;
use serde::{Deserialize, Serialize};

/// How a note relates to the chord sounding right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    /// A note of the chord itself. Rock-solid.
    ChordTone,
    /// In-key and not a half step above any chord tone. Spicy but good.
    Tension,
    /// A half step above a chord tone (an avoid note), or out of key. Flagged — never blocked.
    Dissonance,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TierClassifier;

impl TierClassifier {
    /// Classify one pitch class against a chord, optionally constrained to a key's scale.
    pub fn tier(&self, pitch_class: i32, chord: &[i32], key: Option<&[i32]>) -> Tier {
        let p = normalize(pitch_class);
        if chord.iter().any(|&c| normalize(c) == p) {
            return Tier::ChordTone;
        }
        // The avoid note is the one a half step *above* a chord tone: it smears the tone it sits
        // on (F over C's E, C over G7's B). A half step *below* is a leading tone into the chord
        // tone (B under C = maj7, E under F = the 13th of G7) and stays a tension.
        if chord.iter().any(|&c| normalize(p - c) == 1) {
            return Tier::Dissonance;
        }
        if let Some(key) = key {
            if !key.iter().any(|&k| normalize(k) == p) {
                return Tier::Dissonance;
            }
        }
        Tier::Tension
    }

    /// Precomputed tier for every pitch class 0–11, so notes look up by `pitch % 12`.
    pub fn tier_map(&self, chord: &[i32], key: Option<&[i32]>) -> [Tier; 12] {
        let mut map = [Tier::Tension; 12];
        for (pc, slot) in map.iter_mut().enumerate() {
            *slot = self.tier(pc as i32, chord, key);
        }
        map
    }
}

/// What the roll says about one note: its tier, plus whether its pitch moves.
///
/// A bent note gets exactly one tier, read at its *start pitch*: the row it sits on, judged
/// exactly as the same note unbent. The alternatives are worse: tiering every point of the bend
/// would paint one note in three colours, and tiering where it lands would call a bend from
/// an avoid note up into a chord tone "solid" while it is still rubbing. So the colour stays
/// honest about where the note starts, and `moving` is the second channel that says the rest
/// of it goes somewhere else — the Three-Tier Doctrine's rule that colour never speaks alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteTier {
    /// `None` when there is no chord and no key to judge against.
    pub tier: Option<Tier>,
    pub moving: bool,
}

/// Whether a pitch envelope actually moves the note.
///
/// The envelope reads as zero before its first point, so a note moves as soon as any point
/// leaves the value it started on. An envelope that holds one value throughout — a single
/// point at tick 0, or points that are all zero — is a detune, not a bend, and marking it
/// would be a mark that lies.
pub fn moves(envelope: &[PitchPoint]) -> bool {
    let Some(first) = envelope.first() else {
        return false;
    };
    let start = if first.tick <= 0 { first.cents } else { 0.0 };
    envelope.iter().any(|p| p.cents != start)
}

#[cfg(test)]
mod tests {
    use super::*;

    const C_MAJOR_SCALE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

    #[test]
    fn c_major_triad_no_key() {
        let c = TierClassifier;
        let chord = [0, 4, 7];
        for pc in [0, 4, 7] {
            assert_eq!(c.tier(pc, &chord, None), Tier::ChordTone);
        }
        assert_eq!(c.tier(2, &chord, None), Tier::Tension);
        assert_eq!(c.tier(9, &chord, None), Tier::Tension);
        assert_eq!(c.tier(5, &chord, None), Tier::Dissonance); // half step above E
        assert_eq!(c.tier(1, &chord, None), Tier::Dissonance); // half step above C
                                                               // Half step *below* a chord tone is a leading tone, not an avoid note.
        assert_eq!(c.tier(11, &chord, None), Tier::Tension); // B under C = maj7
        assert_eq!(c.tier(3, &chord, None), Tier::Tension); // D# under E = #9
    }

    #[test]
    fn cmaj7_in_c_major() {
        let c = TierClassifier;
        let chord = [0, 4, 7, 11];
        assert_eq!(c.tier(11, &chord, Some(&C_MAJOR_SCALE)), Tier::ChordTone);
        assert_eq!(c.tier(2, &chord, Some(&C_MAJOR_SCALE)), Tier::Tension);
        assert_eq!(c.tier(5, &chord, Some(&C_MAJOR_SCALE)), Tier::Dissonance);
    }

    #[test]
    fn out_of_key_is_dissonance_even_when_interval_math_says_tension() {
        let c = TierClassifier;
        // A# over C major triad: a whole step from C and three from G → tension by interval,
        // but out of key.
        assert_eq!(c.tier(10, &[0, 4, 7], None), Tier::Tension);
        assert_eq!(
            c.tier(10, &[0, 4, 7], Some(&C_MAJOR_SCALE)),
            Tier::Dissonance
        );
    }

    #[test]
    fn minor_and_dominant_chord_tones() {
        let c = TierClassifier;
        for pc in [9, 0, 4] {
            assert_eq!(c.tier(pc, &[9, 0, 4], None), Tier::ChordTone);
        }
        for pc in [7, 11, 2, 5] {
            assert_eq!(c.tier(pc, &[7, 11, 2, 5], None), Tier::ChordTone);
        }
    }

    #[test]
    fn every_pitch_class_has_exactly_one_tier_and_enharmonics_agree() {
        let c = TierClassifier;
        let map = c.tier_map(&[0, 4, 7], Some(&C_MAJOR_SCALE));
        assert_eq!(map.len(), 12);
        assert_eq!(c.tier(13, &[0, 4, 7], None), c.tier(1, &[0, 4, 7], None));
        assert_eq!(c.tier(-1, &[0, 4, 7], None), c.tier(11, &[0, 4, 7], None));
        assert_eq!(c.tier(4, &[12, 16, 19], None), Tier::ChordTone);
    }

    #[test]
    fn only_an_envelope_that_changes_value_moves() {
        use crate::automation::Curve;
        let p = |tick, cents| PitchPoint::new(tick, cents, Curve::Ramp);
        assert!(!moves(&[]));
        // Held values are a detune, not a bend.
        assert!(!moves(&[p(0, 30.0)]));
        assert!(!moves(&[p(0, 30.0), p(240, 30.0)]));
        assert!(!moves(&[p(120, 0.0), p(240, 0.0)]));
        // Zero before the first point, so a lone late point is a bend up to it.
        assert!(moves(&[p(120, 150.0)]));
        assert!(moves(&[p(0, 30.0), p(240, -20.0)]));
    }
}
