//! Produces the per-pitch-class tier map that colors the piano roll, keyed to the chord under
//! the playhead (R10), degrading gracefully when there's no chord track yet (R12 cold start).
//!
//! Three states:
//!  - **Full context** — a chord is sounding: every pitch class is classified against it
//!    (and the key, if set).
//!  - **Cold start (scale level)** — no chord but a key is set: in-key = solid, out = dissonance.
//!  - **Neutral** — no chord and no key: `None` everywhere.

use super::normalize;
use super::tier::{moves, NoteTier, Tier, TierClassifier};
use crate::chord_track::ChordTrack;
use crate::model::{KeyState, NoteEvent};

#[derive(Debug, Clone, Copy, Default)]
pub struct HighlightEngine {
    classifier: TierClassifier,
}

impl HighlightEngine {
    pub fn tier_map(
        &self,
        tick: i64,
        chord_track: &ChordTrack,
        key: &KeyState,
    ) -> [Option<Tier>; 12] {
        let key_pcs = key.scale_pitch_classes();
        if let Some(chord) = chord_track.chord_at(tick) {
            let map = self
                .classifier
                .tier_map(&chord.pitch_classes, key_pcs.as_deref());
            return map.map(Some);
        }
        if let Some(scale) = key_pcs {
            let mut out = [None; 12];
            for (pc, slot) in out.iter_mut().enumerate() {
                *slot = Some(if scale.contains(&(pc as i32)) {
                    Tier::ChordTone
                } else {
                    Tier::Dissonance
                });
            }
            return out;
        }
        [None; 12]
    }

    /// Convenience for a single MIDI pitch.
    pub fn tier(
        &self,
        pitch: i32,
        tick: i64,
        chord_track: &ChordTrack,
        key: &KeyState,
    ) -> Option<Tier> {
        self.tier_map(tick, chord_track, key)[normalize(pitch) as usize]
    }

    /// One note's tier and moving mark, judged at its start tick and start pitch. The pitch
    /// envelope never enters the tier — see [`NoteTier`] for why — so bending a note can add
    /// the mark but can never recolour it.
    pub fn note_tier(
        &self,
        note: &NoteEvent,
        chord_track: &ChordTrack,
        key: &KeyState,
    ) -> NoteTier {
        NoteTier {
            tier: self.tier(note.pitch, note.start_tick, chord_track, key),
            moving: moves(&note.pitch_envelope),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChordEvent, ScaleType};
    use crate::ticks::TICKS_PER_BEAT as BEAT;

    fn track() -> ChordTrack {
        ChordTrack::new(vec![
            ChordEvent::new(0, 4 * BEAT, vec![0, 4, 7], Some("C".into())),
            ChordEvent::new(4 * BEAT, 4 * BEAT, vec![5, 9, 0], Some("F".into())),
        ])
    }

    #[test]
    fn crossing_a_chord_boundary_changes_tiers() {
        let e = HighlightEngine::default();
        let key = KeyState::NONE;
        assert_eq!(e.tier(64, BEAT, &track(), &key), Some(Tier::ChordTone)); // E over C
        assert_eq!(e.tier(64, 5 * BEAT, &track(), &key), Some(Tier::Tension)); // E over F = maj7
        assert_eq!(e.tier(65, BEAT, &track(), &key), Some(Tier::Dissonance)); // F over C: avoid note
        assert_eq!(e.tier(65, 5 * BEAT, &track(), &key), Some(Tier::ChordTone));
        // F over F
    }

    #[test]
    fn three_state_degradation() {
        let e = HighlightEngine::default();
        let empty = ChordTrack::default();
        assert_eq!(e.tier_map(0, &empty, &KeyState::NONE), [None; 12]);
        let key = KeyState::locked(0, ScaleType::Major);
        let m = e.tier_map(0, &empty, &key);
        assert_eq!(m[0], Some(Tier::ChordTone));
        assert_eq!(m[1], Some(Tier::Dissonance));
        assert!(e.tier_map(0, &track(), &key).iter().all(Option::is_some));
    }

    #[test]
    fn key_filter_applies_in_full_context() {
        let e = HighlightEngine::default();
        assert_eq!(
            e.tier(70, BEAT, &track(), &KeyState::NONE),
            Some(Tier::Tension)
        );
        assert_eq!(
            e.tier(70, BEAT, &track(), &KeyState::locked(0, ScaleType::Major)),
            Some(Tier::Dissonance)
        );
    }
}
