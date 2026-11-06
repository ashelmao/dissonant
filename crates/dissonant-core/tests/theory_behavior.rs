//! Behavioral audit of the harmonic guidance engine: the three-tier classifier, key detection,
//! diatonic harmony/progressions and the highlight engine that feeds the piano roll.
//!
//! Expectations here are written from music theory first (chord tones, available tensions, the
//! avoid note) and only then checked against the implementation — they are ground truth, not a
//! transcript of the code. The `parity` module at the bottom is the table that
//! `ui/src/theory.test.ts` mirrors verbatim, so the colors the UI paints at 60 fps and the
//! harmony the engine plays can never drift apart silently.

use dissonant_core::chord_track::ChordTrack;
use dissonant_core::model::{ChordEvent, KeyState, ScaleType};
use dissonant_core::theory::harmony;
use dissonant_core::{HighlightEngine, KeyDetector, Tier, TierClassifier, TICKS_PER_BEAT as BEAT};

const C_MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
const A_MINOR: [i32; 7] = [9, 11, 0, 2, 4, 5, 7];
const G_MAJOR: [i32; 7] = [7, 9, 11, 0, 2, 4, 6];

/// Compact 12-character tier map, one char per pitch class starting at C: `C`hord tone,
/// `T`ension, `D`issonance. Reads like a keyboard, so the expectations below are legible.
fn row(chord: &[i32], key: Option<&[i32]>) -> String {
    TierClassifier
        .tier_map(chord, key)
        .iter()
        .map(|t| match t {
            Tier::ChordTone => 'C',
            Tier::Tension => 'T',
            Tier::Dissonance => 'D',
        })
        .collect()
}

// ─── Tier classification ───────────────────────────────────────────────────────────────────

/// Ground truth, chord by chord. Each expectation is the set of chord tones, the *available
/// tensions* a player would actually reach for, and the avoid notes — not whatever the code
/// happens to return.
#[test]
fn tier_maps_match_music_theory() {
    //                                                        C  C# D  D# E  F  F# G  G# A  A# B
    // C major triad in C: color tones are the 9th, the 13th and the maj7; F is the avoid note.
    assert_eq!(row(&[0, 4, 7], Some(&C_MAJOR)), "CDTDCDDCDTDT");
    // Same chord with no key set: only the three half steps above chord tones are flagged.
    assert_eq!(row(&[0, 4, 7], None), "CDTTCDTCDTTT");
    // A minor in C: 9th (B), 11th (D) and b7 (G) available; F (b13) is the avoid note.
    assert_eq!(row(&[9, 0, 4], Some(&C_MAJOR)), "CDTDCDDTDCDT");
    // G7 in C: 9th (A) and 13th (E) available; C (the 11th, over the 3rd) is the avoid note.
    assert_eq!(row(&[7, 11, 2, 5], Some(&C_MAJOR)), "DDCDTCDCDTDC");
    // Cmaj7 in C: B joins the chord, 9th and 13th stay available, F stays the avoid note.
    assert_eq!(row(&[0, 4, 7, 11], Some(&C_MAJOR)), "CDTDCDDCDTDC");
    // Dm7 in C (dorian ii): 9th (E), 11th (G) and the dorian 13th (B) are all available.
    assert_eq!(row(&[2, 5, 9, 0], Some(&C_MAJOR)), "CDCDTCDTDCDT");
    // Bdim in C: E, G and A available; C (b9 over the root) is the avoid note.
    assert_eq!(row(&[11, 2, 5], Some(&C_MAJOR)), "DDCDTCDTDTDC");
    // Csus4 in C: the suspension removes the 3rd, so F is a chord tone and E is merely a tension.
    assert_eq!(row(&[0, 5, 7], Some(&C_MAJOR)), "CDTDTCDCDTDT");
    // C augmented, no key: the half steps above C, E and G# are flagged, nothing else.
    assert_eq!(row(&[0, 4, 8], None), "CDTTCDTTCDTT");
    // Em in A minor: F (b9) and C (b13) are the avoid notes, A is the available 11th.
    assert_eq!(row(&[4, 7, 11], Some(&A_MINOR)), "DDTDCDDCDTDC");
    // D major in G: 9th (E), 13th (B) and the b7 (C) available; G is the avoid note over F#.
    assert_eq!(row(&[2, 6, 9], Some(&G_MAJOR)), "TDCDTDCDDCDT");
}

/// The avoid note is directional. A note a half step *above* a chord tone smears it; a note a
/// half step *below* leads into it and is one of the best tensions available (the maj7 over a
/// major triad, the 13th over a dominant 7). Flagging the lower neighbor red would paint the
/// most idiomatic color tone in the language as a mistake.
#[test]
fn avoid_note_is_the_half_step_above_a_chord_tone_not_below() {
    let c = TierClassifier;
    // Above → dissonance.
    assert_eq!(c.tier(5, &[0, 4, 7], None), Tier::Dissonance); // F over E
    assert_eq!(c.tier(1, &[0, 4, 7], None), Tier::Dissonance); // C# over C
    assert_eq!(c.tier(0, &[7, 11, 2, 5], None), Tier::Dissonance); // C over B in G7
                                                                   // Below → tension.
    assert_eq!(c.tier(11, &[0, 4, 7], None), Tier::Tension); // B under C = maj7
    assert_eq!(c.tier(4, &[7, 11, 2, 5], None), Tier::Tension); // E under F = 13th of G7
    assert_eq!(c.tier(4, &[5, 9, 0], None), Tier::Tension); // E under F = maj7 of F
    assert_eq!(c.tier(2, &[3, 7, 10], None), Tier::Tension); // D under D# = maj7 of D#
}

#[test]
fn out_of_key_pitches_are_dissonance_even_when_the_intervals_are_wide() {
    let c = TierClassifier;
    // A# sits a whole step from G and three from C — wide, but outside C major.
    assert_eq!(c.tier(10, &[0, 4, 7], None), Tier::Tension);
    assert_eq!(c.tier(10, &[0, 4, 7], Some(&C_MAJOR)), Tier::Dissonance);
    // A chord tone outranks the key filter: an out-of-key chord keeps its own notes solid.
    for pc in [6, 10, 1] {
        assert_eq!(c.tier(pc, &[6, 10, 1], Some(&C_MAJOR)), Tier::ChordTone);
    }
}

#[test]
fn every_pitch_class_gets_exactly_one_stable_tier() {
    let c = TierClassifier;
    let chords: [&[i32]; 6] = [
        &[0, 4, 7],
        &[7, 11, 2, 5],
        &[9, 0, 4],
        &[3],
        &[0, 1, 2],
        &[],
    ];
    for chord in chords {
        for key in [None, Some(&C_MAJOR[..]), Some(&A_MINOR[..])] {
            let map = c.tier_map(chord, key);
            for pc in 0..12 {
                // tier_map is exactly the per-pitch classifier, so every pitch class has one
                // tier and the roll can look a note up by `pitch % 12`.
                assert_eq!(map[pc as usize], c.tier(pc, chord, key));
                // Octave and enharmonic restatements of the same pitch class agree, in both the
                // pitch argument and the chord spelling.
                for shift in [-24, -12, 12, 24, 120] {
                    assert_eq!(c.tier(pc + shift, chord, key), map[pc as usize]);
                }
                let shifted: Vec<i32> = chord.iter().map(|&x| x + 12).collect();
                assert_eq!(c.tier(pc, &shifted, key), map[pc as usize]);
            }
        }
    }
    // An empty chord has no tones to clash with: in key everything is a tension, out of key
    // everything is flagged — never a panic, never a chord tone.
    assert_eq!(row(&[], Some(&C_MAJOR)), "TDTDTTDTDTDT");
    assert_eq!(row(&[], None), "TTTTTTTTTTTT");
}

// ─── Key detection ─────────────────────────────────────────────────────────────────────────

#[test]
fn a_full_diatonic_scale_ranks_its_own_key_first() {
    let d = KeyDetector::default();
    for root in 0..12 {
        let notes = harmony::scale_pitch_classes(root, ScaleType::Major);
        let r = d.detect(&notes);
        let top = r.top().unwrap();
        assert_eq!(
            (top.root_pitch_class, top.scale),
            (root, ScaleType::Major),
            "flat major scale on {root} should rank its own key first"
        );
        // A bare scale is the same seven notes as its relative minor, so that is the runner-up
        // and nothing else can come between them.
        let second = &r.candidates[1];
        assert_eq!(
            (second.root_pitch_class, second.scale),
            ((root + 9) % 12, ScaleType::Minor),
            "relative minor should be the runner-up on a flat major scale"
        );
        assert_eq!(r.candidates.len(), 24, "all 24 keys are always ranked");
        // Seven notes is below the confidence floor however clean the answer looks.
        assert!(!r.is_confident);
    }
}

#[test]
fn tonic_emphasis_is_what_separates_a_minor_key_from_its_relative_major() {
    let d = KeyDetector::default();
    // Leans on A, E and the raised 7th (G#) — unmistakably A minor, not C major.
    let r = d.detect(&[9, 9, 0, 4, 9, 11, 0, 4, 8, 9, 9, 4]);
    let top = r.top().unwrap();
    assert_eq!((top.root_pitch_class, top.scale), (9, ScaleType::Minor));
    assert!(r.is_confident);
    let a_minor = rank(&r, 9, ScaleType::Minor);
    let c_major = rank(&r, 0, ScaleType::Major);
    assert!(
        a_minor < c_major,
        "A minor must outrank C major on an A-minor fragment"
    );
}

fn rank(r: &dissonant_core::KeyDetectionResult, root: i32, scale: ScaleType) -> usize {
    r.candidates
        .iter()
        .position(|c| c.root_pitch_class == root && c.scale == scale)
        .expect("every key is ranked")
}

#[test]
fn sparse_input_is_never_confident_and_empty_input_has_no_candidate() {
    let d = KeyDetector::default();
    for notes in [vec![0], vec![0, 7], vec![0, 4, 7]] {
        let r = d.detect(&notes);
        assert!(
            !r.is_confident,
            "{notes:?} is far too little to lock a key on"
        );
        assert_eq!(
            r.candidates.len(),
            24,
            "candidates are still offered for display"
        );
    }
    let empty = d.detect(&[]);
    assert!(empty.top().is_none());
    assert!(!empty.is_confident);
    assert!(empty.candidates.is_empty());
}

/// Repetition is not evidence. A riff that hammers two or three pitch classes is harmonically
/// sparse no matter how many notes it contains, and four distinct pitches is still a single
/// chord: a C–E–G–B arpeggio correlates best with *E minor* because E minor's triad is a subset
/// of it. Offering any of these as a confident key suggestion would be confidently wrong.
#[test]
fn repetition_does_not_make_a_thin_input_confident() {
    let d = KeyDetector::default();
    let ostinato = [0, 4, 0, 4, 0, 4, 0, 4, 0, 4, 0, 4]; // two pitch classes, twelve notes
    assert_eq!(ostinato.len(), 12);
    assert!(!d.detect(&ostinato).is_confident);

    let triad: Vec<i32> = [0, 4, 7].repeat(4); // three pitch classes, twelve notes
    assert!(!d.detect(&triad).is_confident);

    let maj7_arp: Vec<i32> = [0, 4, 7, 11].repeat(3); // four pitch classes, twelve notes
    let r = d.detect(&maj7_arp);
    let top = r.top().unwrap();
    assert_eq!(
        (top.root_pitch_class, top.scale),
        (4, ScaleType::Minor),
        "a Cmaj7 arpeggio really does correlate best with E minor — which is why it must not be confident"
    );
    assert!(!r.is_confident);

    // Five distinct pitch classes with a clear tonal center is enough to be confident.
    let melody = [60, 62, 64, 65, 67, 69, 71, 72, 67, 64, 60, 55];
    assert!(d.detect(&melody).is_confident);
}

#[test]
fn detection_is_deterministic_and_order_independent() {
    let d = KeyDetector::default();
    let notes = [0, 2, 4, 5, 7, 9, 11, 0, 4, 7, 0, 7];
    let mut shuffled = notes;
    shuffled.reverse();
    assert_eq!(d.detect(&notes), d.detect(&shuffled));
    assert_eq!(d.detect(&notes), d.detect(&notes));
    // Octave register is irrelevant — only pitch class matters.
    let transposed: Vec<i32> = notes.iter().map(|n| n + 60).collect();
    assert_eq!(
        d.detect(&notes).candidates,
        d.detect(&transposed).candidates
    );
    // A fully chromatic input has no profile to correlate with; it must not pretend otherwise.
    let chromatic: Vec<i32> = (0..12).collect();
    let r = d.detect(&chromatic);
    assert!(r.candidates.iter().all(|c| c.score == 0.0));
    assert!(!r.is_confident);
}

// ─── Progressions and chord suggestions ────────────────────────────────────────────────────

#[test]
fn every_starter_stays_diatonic_in_every_key() {
    for root in 0..12 {
        for scale in [ScaleType::Major, ScaleType::Minor] {
            let in_key = harmony::scale_pitch_classes(root, scale);
            for starter in harmony::starters() {
                let chords =
                    harmony::progression(&starter.degrees, root, scale, 16 * BEAT, 4 * BEAT);
                assert!(!chords.is_empty());
                for c in &chords {
                    for pc in &c.pitch_classes {
                        assert!(
                            in_key.contains(pc),
                            "{} in {} has out-of-key {pc}",
                            starter.name,
                            harmony::key_name(root, scale)
                        );
                    }
                    // Every generated chord is one of the key's own diatonic triads, named.
                    let diatonic = harmony::diatonic_chords(root, scale);
                    assert!(diatonic
                        .iter()
                        .any(|d| d.pitch_classes == c.pitch_classes
                            && Some(&d.name) == c.name.as_ref()));
                }
            }
        }
    }
}

#[test]
fn a_progression_tiles_its_length_with_no_gaps_or_overlaps() {
    let chords = harmony::progression(&[0, 3, 4, 5], 0, ScaleType::Major, 32 * BEAT, 4 * BEAT);
    assert_eq!(
        chords.len(),
        8,
        "the degrees cycle to fill the requested length"
    );
    for (i, c) in chords.iter().enumerate() {
        assert_eq!(c.start_tick, i as i64 * 4 * BEAT);
        assert_eq!(c.length_ticks, 4 * BEAT);
        if i > 0 {
            assert_eq!(chords[i - 1].end_tick(), c.start_tick, "no gap, no overlap");
        }
    }
    assert_eq!(chords.last().unwrap().end_tick(), 32 * BEAT);
    // Chord lookup over the result is total across the covered span.
    let track = ChordTrack::new(chords.clone());
    let mut tick = 0;
    while tick < 32 * BEAT {
        assert!(track.chord_at(tick).is_some(), "tick {tick} uncovered");
        tick += BEAT / 2;
    }
    // The degrees cycle, so the pattern repeats exactly.
    assert_eq!(chords[0].pitch_classes, chords[4].pitch_classes);

    // A pattern too short for one chord per degree divides the length instead of running past
    // the end: on a 1-bar pattern a 4-chord starter becomes four 1-beat chords that all fit.
    let short = harmony::progression(&[0, 3, 4, 5], 0, ScaleType::Major, 4 * BEAT, 4 * BEAT);
    assert_eq!(
        short.len(),
        4,
        "a starter always lands as the progression it names"
    );
    assert_eq!(short[0].start_tick, 0);
    assert_eq!(
        short.last().unwrap().end_tick(),
        4 * BEAT,
        "chords must fit inside the pattern"
    );
    for (i, c) in short.iter().enumerate() {
        assert_eq!(c.length_ticks, BEAT);
        if i > 0 {
            assert_eq!(short[i - 1].end_tick(), c.start_tick, "no gap, no overlap");
        }
    }
    // A length that is not a whole number of chords is still covered to the end.
    let ragged = harmony::progression(&[0, 3], 0, ScaleType::Major, 18 * BEAT, 4 * BEAT);
    assert_eq!(
        ragged.last().unwrap().end_tick(),
        18 * BEAT,
        "the last chord absorbs the remainder"
    );
    // Degenerate inputs yield nothing rather than panicking or looping forever.
    assert!(harmony::progression(&[], 0, ScaleType::Major, 16 * BEAT, 4 * BEAT).is_empty());
    assert!(harmony::progression(&[0], 0, ScaleType::Major, 16 * BEAT, 0).is_empty());
    assert!(harmony::progression(&[0], 0, ScaleType::Major, -8 * BEAT, 4 * BEAT).is_empty());
    // Degrees outside 0–6 wrap onto the scale instead of panicking.
    let wrapped = harmony::progression(&[-1, 9], 0, ScaleType::Major, 8 * BEAT, 4 * BEAT);
    assert_eq!(wrapped[0].name.as_deref(), Some("Bdim"));
    assert_eq!(wrapped[1].name.as_deref(), Some("Em"));
}

#[test]
fn chord_suggestions_are_in_key_and_deterministically_ordered() {
    let c_major = harmony::diatonic_chords(0, ScaleType::Major);
    assert_eq!(
        c_major.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["C", "Dm", "Em", "F", "G", "Am", "Bdim"]
    );
    assert_eq!(
        c_major
            .iter()
            .map(|c| c.roman_numeral.as_str())
            .collect::<Vec<_>>(),
        ["I", "ii", "iii", "IV", "V", "vi", "vii°"]
    );
    let a_minor = harmony::diatonic_chords(9, ScaleType::Minor);
    assert_eq!(
        a_minor
            .iter()
            .map(|c| c.roman_numeral.as_str())
            .collect::<Vec<_>>(),
        ["i", "ii°", "III", "iv", "v", "VI", "VII"]
    );
    // Relative keys share their seven chords, rotated — a good cross-check on the generator.
    let mut sorted_major: Vec<&Vec<i32>> = c_major.iter().map(|c| &c.pitch_classes).collect();
    let mut sorted_minor: Vec<&Vec<i32>> = a_minor.iter().map(|c| &c.pitch_classes).collect();
    sorted_major.sort();
    sorted_minor.sort();
    assert_eq!(sorted_major, sorted_minor);
    for root in 0..12 {
        for scale in [ScaleType::Major, ScaleType::Minor] {
            let in_key = harmony::scale_pitch_classes(root, scale);
            let chords = harmony::diatonic_chords(root, scale);
            assert_eq!(chords.len(), 7);
            assert_eq!(
                chords,
                harmony::diatonic_chords(root, scale),
                "stable across calls"
            );
            for (degree, c) in chords.iter().enumerate() {
                assert_eq!(
                    c.pitch_classes[0], in_key[degree],
                    "degree {degree} is rooted on its scale tone"
                );
                assert!(c.pitch_classes.iter().all(|pc| in_key.contains(pc)));
            }
        }
    }
}

// ─── Highlight engine ──────────────────────────────────────────────────────────────────────

fn two_bar_track() -> ChordTrack {
    ChordTrack::new(vec![
        ChordEvent::new(0, 4 * BEAT, vec![0, 4, 7], Some("C".into())),
        ChordEvent::new(4 * BEAT, 4 * BEAT, vec![5, 9, 0], Some("F".into())),
    ])
}

#[test]
fn the_tier_map_tracks_the_chord_sounding_at_that_tick() {
    let e = HighlightEngine::default();
    let track = two_bar_track();
    let key = KeyState::locked(0, ScaleType::Major);
    // Over C: F is the avoid note, B is the maj7.
    assert_eq!(e.tier(65, 0, &track, &key), Some(Tier::Dissonance));
    assert_eq!(e.tier(71, 4 * BEAT - 1, &track, &key), Some(Tier::Tension));
    // One beat later the same two pitches mean something else: over F (F A C), F is a chord
    // tone and E is the maj7 — the colors must move with the chord.
    assert_eq!(e.tier(65, 4 * BEAT, &track, &key), Some(Tier::ChordTone));
    assert_eq!(e.tier(64, 4 * BEAT, &track, &key), Some(Tier::Tension));
    // The boundary is half-open: beat 4 belongs to the second chord, not the first.
    assert_eq!(
        track.chord_at(4 * BEAT - 1).unwrap().name.as_deref(),
        Some("C")
    );
    assert_eq!(track.chord_at(4 * BEAT).unwrap().name.as_deref(), Some("F"));
    // The whole map is populated whenever a chord is sounding.
    assert!(e.tier_map(BEAT, &track, &key).iter().all(Option::is_some));
}

#[test]
fn ticks_outside_the_chords_fall_back_to_scale_level_instead_of_panicking() {
    let e = HighlightEngine::default();
    let key = KeyState::locked(0, ScaleType::Major);
    let gapped = ChordTrack::new(vec![
        ChordEvent::new(4 * BEAT, 2 * BEAT, vec![0, 4, 7], Some("C".into())),
        ChordEvent::new(10 * BEAT, 2 * BEAT, vec![7, 11, 2], Some("G".into())),
    ]);
    // Before the first chord, inside the gap, and past the last chord: scale-level guidance.
    for tick in [
        -8 * BEAT,
        -BEAT / 2,
        0,
        4 * BEAT - 1,
        6 * BEAT,
        9 * BEAT,
        12 * BEAT,
        1_000 * BEAT,
    ] {
        assert!(
            gapped.chord_at(tick).is_none(),
            "tick {tick} should be in a gap"
        );
        let map = e.tier_map(tick, &gapped, &key);
        assert_eq!(map[0], Some(Tier::ChordTone), "in-key at tick {tick}");
        assert_eq!(map[1], Some(Tier::Dissonance), "out-of-key at tick {tick}");
        assert!(map.iter().all(Option::is_some));
    }
    // No chord and no key is the neutral state: no guidance at all, still no panic.
    for tick in [-BEAT, 0, 7 * BEAT] {
        assert_eq!(
            e.tier_map(tick, &ChordTrack::default(), &KeyState::NONE),
            [None; 12]
        );
        assert_eq!(e.tier_map(tick, &gapped, &KeyState::NONE), [None; 12]);
    }
    // A zero-length chord occupies no tick at all, so it never becomes the context.
    let degenerate = ChordTrack::new(vec![ChordEvent::new(0, 0, vec![0, 4, 7], Some("C".into()))]);
    assert!(degenerate.chord_at(0).is_none());
    assert_eq!(e.tier_map(0, &degenerate, &KeyState::NONE), [None; 12]);
}

#[test]
fn where_chords_overlap_the_later_starting_one_wins() {
    let e = HighlightEngine::default();
    // Built in the "wrong" order on purpose: the track must not depend on insertion order.
    let track = ChordTrack::new(vec![
        ChordEvent::new(4 * BEAT, 4 * BEAT, vec![5, 9, 0], Some("F".into())),
        ChordEvent::new(0, 8 * BEAT, vec![0, 4, 7], Some("C".into())),
    ]);
    assert_eq!(track.chord_at(BEAT).unwrap().name.as_deref(), Some("C"));
    assert_eq!(track.chord_at(5 * BEAT).unwrap().name.as_deref(), Some("F"));
    // F is a chord tone of the freshly dropped F chord, an avoid note under the C it overlaps.
    assert_eq!(
        e.tier(65, 5 * BEAT, &track, &KeyState::NONE),
        Some(Tier::ChordTone)
    );
    assert_eq!(
        e.tier(65, BEAT, &track, &KeyState::NONE),
        Some(Tier::Dissonance)
    );
}

#[test]
fn a_bent_note_keeps_the_tier_of_its_start_pitch_and_gains_the_moving_mark() {
    use dissonant_core::model::{NoteEvent, PitchPoint};
    use dissonant_core::{Curve, NoteTier};
    let e = HighlightEngine::default();
    let track = two_bar_track();
    let key = KeyState::locked(0, ScaleType::Major);
    // F over C is the avoid note. Bend it a whole step up into G — a chord tone — and it is
    // still an avoid note where it starts: the colour reports the start, the mark the move.
    let plain = NoteEvent::new(0, 2 * BEAT, 65);
    let mut bent = plain.clone();
    bent.pitch_envelope = vec![PitchPoint::new(BEAT, 200.0, Curve::Ramp)];
    assert_eq!(
        e.note_tier(&plain, &track, &key),
        NoteTier {
            tier: Some(Tier::Dissonance),
            moving: false
        }
    );
    assert_eq!(
        e.note_tier(&bent, &track, &key),
        NoteTier {
            tier: Some(Tier::Dissonance),
            moving: true
        }
    );
    // Every pitch, every context: bending never changes the tier, only the mark.
    for pitch in 48..72 {
        for (track, key) in [
            (two_bar_track(), key),
            (ChordTrack::default(), key),
            (ChordTrack::default(), KeyState::NONE),
        ] {
            for start in [0, 3 * BEAT, 5 * BEAT] {
                let plain = NoteEvent::new(start, BEAT, pitch);
                let mut bent = plain.clone();
                bent.pitch_envelope = vec![
                    PitchPoint::new(0, -100.0, Curve::Step),
                    PitchPoint::new(BEAT / 2, 300.0, Curve::Ramp),
                ];
                let (a, b) = (
                    e.note_tier(&plain, &track, &key),
                    e.note_tier(&bent, &track, &key),
                );
                assert_eq!(a.tier, b.tier, "pitch {pitch} at {start}");
                assert!(!a.moving && b.moving);
            }
        }
    }
}

// ─── Rust ↔ TypeScript parity ──────────────────────────────────────────────────────────────

/// The UI reimplements this theory in `ui/src/theory.ts` so the roll can repaint at 60 fps
/// without a round trip. These tables are duplicated verbatim in `ui/src/theory.test.ts`; if
/// one side is edited without the other, one of the two suites fails.
mod parity {
    use super::*;

    type Case = (
        &'static str,
        &'static [i32],
        Option<(i32, ScaleType)>,
        i32,
        Tier,
    );

    const CASES: &[Case] = &[
        (
            "C/Cmaj root",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            0,
            Tier::ChordTone,
        ),
        (
            "C/Cmaj 9",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            2,
            Tier::Tension,
        ),
        (
            "C/Cmaj avoid 11",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            5,
            Tier::Dissonance,
        ),
        (
            "C/Cmaj 13",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            9,
            Tier::Tension,
        ),
        (
            "C/Cmaj maj7",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            11,
            Tier::Tension,
        ),
        (
            "C/Cmaj b9 out",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            1,
            Tier::Dissonance,
        ),
        (
            "C/Cmaj #11 out",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            6,
            Tier::Dissonance,
        ),
        (
            "C/Cmaj b13 out",
            &[0, 4, 7],
            Some((0, ScaleType::Major)),
            8,
            Tier::Dissonance,
        ),
        ("C/nokey b7", &[0, 4, 7], None, 10, Tier::Tension),
        ("C/nokey maj7", &[0, 4, 7], None, 11, Tier::Tension),
        ("C/nokey #9", &[0, 4, 7], None, 3, Tier::Tension),
        ("C/nokey avoid 11", &[0, 4, 7], None, 5, Tier::Dissonance),
        ("C octave-up pitch", &[0, 4, 7], None, 64, Tier::ChordTone),
        ("C negative pitch", &[0, 4, 7], None, -1, Tier::Tension),
        ("C octave chord", &[12, 16, 19], None, 11, Tier::Tension),
        (
            "Am/Cmaj 9",
            &[9, 0, 4],
            Some((0, ScaleType::Major)),
            11,
            Tier::Tension,
        ),
        (
            "Am/Cmaj 11",
            &[9, 0, 4],
            Some((0, ScaleType::Major)),
            2,
            Tier::Tension,
        ),
        (
            "Am/Cmaj b7",
            &[9, 0, 4],
            Some((0, ScaleType::Major)),
            7,
            Tier::Tension,
        ),
        (
            "Am/Cmaj avoid b13",
            &[9, 0, 4],
            Some((0, ScaleType::Major)),
            5,
            Tier::Dissonance,
        ),
        (
            "Am/Cmaj third",
            &[9, 0, 4],
            Some((0, ScaleType::Major)),
            0,
            Tier::ChordTone,
        ),
        (
            "G7/Cmaj 13",
            &[7, 11, 2, 5],
            Some((0, ScaleType::Major)),
            4,
            Tier::Tension,
        ),
        (
            "G7/Cmaj 9",
            &[7, 11, 2, 5],
            Some((0, ScaleType::Major)),
            9,
            Tier::Tension,
        ),
        (
            "G7/Cmaj avoid 11",
            &[7, 11, 2, 5],
            Some((0, ScaleType::Major)),
            0,
            Tier::Dissonance,
        ),
        (
            "G7/Cmaj seventh",
            &[7, 11, 2, 5],
            Some((0, ScaleType::Major)),
            5,
            Tier::ChordTone,
        ),
        (
            "G7/Cmaj b9 out",
            &[7, 11, 2, 5],
            Some((0, ScaleType::Major)),
            8,
            Tier::Dissonance,
        ),
        (
            "Cmaj7/Cmaj 7th",
            &[0, 4, 7, 11],
            Some((0, ScaleType::Major)),
            11,
            Tier::ChordTone,
        ),
        (
            "Cmaj7/Cmaj 9",
            &[0, 4, 7, 11],
            Some((0, ScaleType::Major)),
            2,
            Tier::Tension,
        ),
        (
            "Cmaj7/Cmaj avoid 11",
            &[0, 4, 7, 11],
            Some((0, ScaleType::Major)),
            5,
            Tier::Dissonance,
        ),
        (
            "Dm7/Cmaj 13",
            &[2, 5, 9, 0],
            Some((0, ScaleType::Major)),
            11,
            Tier::Tension,
        ),
        (
            "Dm7/Cmaj 11",
            &[2, 5, 9, 0],
            Some((0, ScaleType::Major)),
            7,
            Tier::Tension,
        ),
        (
            "Dm7/Cmaj 9",
            &[2, 5, 9, 0],
            Some((0, ScaleType::Major)),
            4,
            Tier::Tension,
        ),
        (
            "Bdim/Cmaj avoid b9",
            &[11, 2, 5],
            Some((0, ScaleType::Major)),
            0,
            Tier::Dissonance,
        ),
        (
            "Bdim/Cmaj b13",
            &[11, 2, 5],
            Some((0, ScaleType::Major)),
            7,
            Tier::Tension,
        ),
        (
            "Csus4/Cmaj third",
            &[0, 5, 7],
            Some((0, ScaleType::Major)),
            4,
            Tier::Tension,
        ),
        (
            "Csus4/Cmaj maj7",
            &[0, 5, 7],
            Some((0, ScaleType::Major)),
            11,
            Tier::Tension,
        ),
        ("Caug/nokey #5", &[0, 4, 8], None, 8, Tier::ChordTone),
        ("Caug/nokey avoid 13", &[0, 4, 8], None, 9, Tier::Dissonance),
        (
            "Em/Amin avoid b9",
            &[4, 7, 11],
            Some((9, ScaleType::Minor)),
            5,
            Tier::Dissonance,
        ),
        (
            "Em/Amin avoid b13",
            &[4, 7, 11],
            Some((9, ScaleType::Minor)),
            0,
            Tier::Dissonance,
        ),
        (
            "Em/Amin 11",
            &[4, 7, 11],
            Some((9, ScaleType::Minor)),
            9,
            Tier::Tension,
        ),
        (
            "D/Gmaj avoid 11",
            &[2, 6, 9],
            Some((7, ScaleType::Major)),
            7,
            Tier::Dissonance,
        ),
        (
            "D/Gmaj 13",
            &[2, 6, 9],
            Some((7, ScaleType::Major)),
            11,
            Tier::Tension,
        ),
    ];

    #[test]
    fn tier_cases_hold_on_the_rust_side() {
        assert_eq!(CASES.len(), 42);
        for &(label, chord, key, pitch, expected) in CASES {
            let scale = key.map(|(root, scale)| harmony::scale_pitch_classes(root, scale));
            assert_eq!(
                TierClassifier.tier(pitch, chord, scale.as_deref()),
                expected,
                "{label}"
            );
        }
    }

    /// `normalize`, `STARTERS` and `progression` are mirrored in the UI too; these are the exact
    /// values `ui/src/theory.test.ts` asserts.
    #[test]
    fn shared_harmony_values_hold_on_the_rust_side() {
        use dissonant_core::theory::normalize;
        let normalized: Vec<i32> = [-25, -13, -12, -1, 0, 11, 12, 13, 64, 127]
            .iter()
            .map(|&p| normalize(p))
            .collect();
        assert_eq!(normalized, [11, 11, 0, 11, 0, 11, 0, 1, 4, 7]);

        let starters: Vec<(&str, &str, ScaleType, Vec<i32>)> = harmony::starters()
            .into_iter()
            .map(|s| (s.name, s.mood, s.scale, s.degrees))
            .collect();
        use ScaleType::{Major, Minor};
        assert_eq!(
            starters,
            [
                ("I–IV–V–vi", "bright, lifting", Major, vec![0, 3, 4, 5]),
                ("I–V–vi–IV", "big and hopeful", Major, vec![0, 4, 5, 3]),
                ("vi–IV–I–V", "sad but hopeful", Major, vec![5, 3, 0, 4]),
                ("I–vi–IV–V", "old-school, sweet", Major, vec![0, 5, 3, 4]),
                ("ii–V–I", "jazzy, resolved", Major, vec![1, 4, 0, 0]),
                ("I–IV", "open two-chord vamp", Major, vec![0, 3]),
                ("i–VI–III–VII", "dark, driving", Minor, vec![0, 5, 2, 6]),
                ("i–VII–VI–VII", "brooding loop", Minor, vec![0, 6, 5, 6]),
                ("i–v–VI–iv", "sad, cinematic", Minor, vec![0, 4, 5, 3]),
                ("i–iv", "moody two-chord vamp", Minor, vec![0, 3]),
                ("i–VI", "hazy, floating", Minor, vec![0, 5]),
                ("i", "one-chord drone", Minor, vec![0]),
            ]
        );
        assert_eq!(harmony::starters_for(Minor)[0].name, "i–VI–III–VII");
        assert_eq!(harmony::starters_for(Major).len(), 6);

        let described = |degrees: &[i32], root: i32, scale: ScaleType, total: i64| -> Vec<String> {
            harmony::progression(degrees, root, scale, total, 4 * BEAT)
                .iter()
                .map(|c| {
                    format!(
                        "{}:{}:{:?}:{}",
                        c.start_tick,
                        c.length_ticks,
                        c.pitch_classes,
                        c.name.clone().unwrap_or_default()
                    )
                })
                .collect()
        };
        assert_eq!(
            described(&[0, 3, 4, 5], 0, ScaleType::Major, 16 * BEAT),
            [
                "0:3840:[0, 4, 7]:C",
                "3840:3840:[5, 9, 0]:F",
                "7680:3840:[7, 11, 2]:G",
                "11520:3840:[9, 0, 4]:Am"
            ]
        );
        assert_eq!(
            described(&[0, 5, 2, 6], 9, ScaleType::Minor, 16 * BEAT),
            [
                "0:3840:[9, 0, 4]:Am",
                "3840:3840:[5, 9, 0]:F",
                "7680:3840:[0, 4, 7]:C",
                "11520:3840:[7, 11, 2]:G"
            ]
        );
        assert_eq!(
            described(&[1, 4, 0, 0], 2, ScaleType::Major, 16 * BEAT),
            [
                "0:3840:[4, 7, 11]:Em",
                "3840:3840:[9, 1, 4]:A",
                "7680:3840:[2, 6, 9]:D",
                "11520:3840:[2, 6, 9]:D"
            ]
        );
    }

    type MovingCase = (&'static str, &'static [(i64, f32)], bool);

    /// Which pitch envelopes count as moving. `ui/src/theory.test.ts` mirrors this table as
    /// `MOVING_CASES`; the roll draws the mark from the TS side, the engine reads this one.
    const MOVING_CASES: &[MovingCase] = &[
        ("empty", &[], false),
        ("held detune at the start", &[(0, 30.0)], false),
        ("held detune, two points", &[(0, 30.0), (240, 30.0)], false),
        ("all zero", &[(120, 0.0), (240, 0.0)], false),
        ("lone late point bends up to it", &[(120, 150.0)], true),
        ("start detune then fall", &[(0, 30.0), (240, -20.0)], true),
        (
            "returns to zero",
            &[(0, 0.0), (120, 50.0), (240, 0.0)],
            true,
        ),
    ];

    #[test]
    fn moving_cases_hold_on_the_rust_side() {
        use dissonant_core::model::{NoteEvent, PitchPoint};
        use dissonant_core::Curve;
        let e = HighlightEngine::default();
        for &(label, points, expected) in MOVING_CASES {
            let mut note = NoteEvent::new(0, 4 * BEAT, 60);
            note.pitch_envelope = points
                .iter()
                .map(|&(t, c)| PitchPoint::new(t, c, Curve::Ramp))
                .collect();
            let got = e.note_tier(&note, &ChordTrack::default(), &KeyState::NONE);
            assert_eq!(got.moving, expected, "{label}");
        }
    }
}
