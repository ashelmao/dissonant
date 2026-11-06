//! DI → notes and chords, against takes we wrote ourselves.
//!
//! Every signal here is a synthesized plucked string: harmonics at 1/n with the upper ones
//! dying faster, a burst of pick noise on the attack, and a re-pick on the same string cutting
//! the old vibration off — the three things that make a guitar DI a guitar DI as far as a
//! detector can tell. Because we wrote the score, every assertion is "the right answer", and
//! each test prints what it measured (`cargo test -- --nocapture`) so the accuracy can be read
//! off rather than taken on trust.

use dissonant_core::{KeyDetector, ScaleType, TempoMap, TICKS_PER_BEAT};
use dissonant_engine::{hear, place_on_song};
use uuid::Uuid;

const SR: f32 = 48_000.0;

/// One plucked string.
struct Pluck {
    start: f64,
    end: f64,
    midi: f32,
    amp: f32,
    /// `(at, semitones, step seconds)`: fret by fret, one semitone per step, no new attack.
    slide: Option<(f64, i32, f64)>,
    /// `(from, to, cents)`: a ramp from 0 to `cents` between the two times, then held.
    bend: Option<(f64, f64, f32)>,
    /// `(at, semitones)`: the pitch steps there with no new attack (a hammer-on).
    hammer: Option<(f64, f32)>,
}

fn pluck(start: f64, end: f64, midi: f32, amp: f32) -> Pluck {
    Pluck {
        start,
        end,
        midi,
        amp,
        slide: None,
        bend: None,
        hammer: None,
    }
}

/// Deterministic noise in -1…1.
struct Lcg(u32);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

fn render(plucks: &[Pluck], seconds: f64) -> Vec<f32> {
    let n = (seconds * SR as f64) as usize;
    let mut out = vec![0.0f32; n];
    let mut noise = Lcg(0x2468_ace1);
    for p in plucks {
        let first = (p.start * SR as f64) as usize;
        let last = ((p.end * SR as f64) as usize).min(n);
        let mut phase = 0.0f64;
        for (i, slot) in out.iter_mut().enumerate().take(last).skip(first) {
            let t = i as f64 / SR as f64;
            let age = (t - p.start) as f32;
            let mut midi = p.midi;
            if let Some((a, b, cents)) = p.bend {
                let x = ((t - a) / (b - a)).clamp(0.0, 1.0) as f32;
                midi += cents / 100.0 * x;
            }
            if let Some((at, semis, step)) = p.slide {
                if t >= at {
                    midi += (((t - at) / step).floor() as i32 + 1).min(semis) as f32;
                }
            }
            if let Some((at, semis)) = p.hammer {
                if t >= at {
                    midi += semis;
                }
            }
            let hz = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0);
            phase += 2.0 * std::f64::consts::PI * hz / SR as f64;
            let mut s = 0.0f32;
            let mut h = 1;
            while (h as f64) * hz < 5_000.0 && h <= 24 {
                let decay = (-(age) * (1.2 + 0.6 * h as f32)).exp();
                s += (1.0 / h as f32) * decay * ((h as f64 * phase).sin() as f32);
                h += 1;
            }
            // The pick: a few milliseconds of broadband noise.
            if age < 0.004 {
                s += 0.4 * noise.next() * (1.0 - age / 0.004);
            }
            // The next pick (or the player's hand) stops the string in 5 ms.
            let fade = (((p.end - t) / 0.005).clamp(0.0, 1.0)) as f32;
            *slot += p.amp * s * fade;
        }
    }
    out
}

fn note_name(m: i32) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[m.rem_euclid(12) as usize], m.div_euclid(12) - 1)
}

/// The test riff: E3, G3 picked twice (the second softer), A3 bent up a whole tone, D4 — then
/// a strummed C major triad that must be refused as notes and offered as a chord.
fn riff() -> Vec<Pluck> {
    let mut a3 = pluck(1.00, 1.60, 57.0, 0.4);
    a3.bend = Some((1.15, 1.30, 200.0));
    vec![
        pluck(0.10, 0.40, 52.0, 0.4),
        pluck(0.40, 0.70, 55.0, 0.4),
        pluck(0.70, 1.00, 55.0, 0.2),
        a3,
        pluck(1.80, 2.20, 62.0, 0.4),
        pluck(2.500, 3.5, 48.0, 0.3),
        pluck(2.515, 3.5, 52.0, 0.3),
        pluck(2.530, 3.5, 55.0, 0.3),
    ]
}

#[test]
fn a_plucked_line_becomes_its_notes_with_the_repeat_split_and_the_bend_kept() {
    let take = render(&riff(), 3.8);
    let heard = hear(&take, SR);
    let want = [(0.10, 52), (0.40, 55), (0.70, 55), (1.00, 57), (1.80, 62)];
    for n in &heard.notes {
        eprintln!(
            "heard {:>4} {:.3}–{:.3}s vel {:>3} envelope peak {:+.0}¢",
            note_name(n.pitch),
            n.start,
            n.end,
            n.velocity,
            n.contour.iter().map(|c| c.1).fold(0.0f32, |m, c| if c.abs() > m.abs() { c } else { m })
        );
    }
    let got: Vec<i32> = heard.notes.iter().map(|n| n.pitch).collect();
    assert_eq!(got, want.iter().map(|w| w.1).collect::<Vec<_>>(), "pitches");
    let mut worst_onset = 0.0f64;
    for (n, &(t, _)) in heard.notes.iter().zip(&want) {
        worst_onset = worst_onset.max((n.start - t).abs());
    }
    eprintln!("pitch accuracy 5/5 notes, worst onset error {:.1} ms", worst_onset * 1000.0);
    assert!(worst_onset < 0.015, "an onset landed {:.1} ms off", worst_onset * 1000.0);

    // The repeated G3 is two notes, the second one softer because it was picked softer.
    let (g1, g2) = (&heard.notes[1], &heard.notes[2]);
    assert!(g1.end <= g2.start + 1e-9, "the repeated note overlaps itself");
    assert!(g2.velocity < g1.velocity, "velocity {} after {}", g2.velocity, g1.velocity);

    // The bend: the note is tiered by where it starts, and its contour climbs to +200.
    let bent = &heard.notes[3];
    let peak = bent.contour.iter().map(|c| c.1).fold(f32::MIN, f32::max);
    let held = bent.contour.last().unwrap().1;
    eprintln!("bend: peak {peak:+.1}¢, held {held:+.1}¢ (wanted +200)");
    assert!((peak - 200.0).abs() < 20.0, "bend peaked at {peak} cents");
    assert!((held - 200.0).abs() < 20.0, "bend held at {held} cents");
    for (i, n) in heard.notes.iter().enumerate() {
        if i != 3 {
            let wobble = n.contour.iter().map(|c| c.1.abs()).fold(0.0f32, f32::max);
            assert!(wobble < 20.0, "{} wobbled {wobble} cents", note_name(n.pitch));
        }
    }

    // Laid on the song, only the bent note carries an envelope, and it ends up a whole tone.
    let map = TempoMap::constant(120.0);
    let t = place_on_song(&heard, Uuid::new_v4(), 4 * TICKS_PER_BEAT, 3.8, &map);
    assert_eq!(t.notes.len(), 5);
    for (i, n) in t.notes.iter().enumerate() {
        assert_eq!(!n.pitch_envelope.is_empty(), i == 3, "envelope on note {i}");
    }
    let env = &t.notes[3].pitch_envelope;
    eprintln!("envelope: {} points {:?}", env.len(), env.iter().map(|p| (p.tick, p.cents.round())).collect::<Vec<_>>());
    assert!(env.len() >= 3 && env.len() <= dissonant_core::MAX_PITCH_POINTS);
    assert!((env.last().unwrap().cents - 200.0).abs() < 20.0);
    // 120 bpm: 0.1 s is 192 ticks after the clip's start.
    assert!((t.notes[0].start_tick - 192).abs() <= 30, "first note at {}", t.notes[0].start_tick);
}

#[test]
fn a_strummed_triad_is_refused_as_notes_and_offered_as_its_chord() {
    let take = render(&riff(), 3.8);
    let heard = hear(&take, SR);
    eprintln!("refused {:?}", heard.polyphonic);
    eprintln!(
        "chords {:?}",
        heard.chords.iter().map(|c| (c.name.clone(), c.start, c.end, c.confidence)).collect::<Vec<_>>()
    );
    // Exactly one refused region, over the strum, and nothing written inside it.
    assert_eq!(heard.polyphonic.len(), 1, "refused regions: {:?}", heard.polyphonic);
    let (a, b) = heard.polyphonic[0];
    assert!((a - 2.5).abs() < 0.03 && b > 3.2, "refused {a:.3}–{b:.3}");
    assert!(heard.notes.iter().all(|n| n.end <= a + 1e-9 || n.start >= b), "a note was written over the strum");
    assert_eq!(heard.chords.len(), 1);
    let c = &heard.chords[0];
    assert_eq!(c.name, "C");
    assert_eq!(c.pitch_classes, vec![0, 4, 7]);
    assert!(c.confidence >= 0.85, "confidence {}", c.confidence);

    // The detected chord lands in song ticks, waiting in its lane.
    let map = TempoMap::constant(120.0);
    let clip = Uuid::new_v4();
    let t = place_on_song(&heard, clip, 0, 3.8, &map);
    assert_eq!(t.detection.clip_id, clip);
    assert_eq!(t.detection.chords.len(), 1);
    assert!((t.detection.chords[0].start_tick - 4800).abs() < 60);
    assert_eq!(t.refused.len(), 1);
}

#[test]
fn a_slide_is_one_note_that_moves_rather_than_a_run_of_short_ones() {
    // A3 slid up three frets, 30 ms a fret, and held on C4.
    let mut a = pluck(0.1, 1.0, 57.0, 0.4);
    a.slide = Some((0.5, 3, 0.03));
    let heard = hear(&render(&[a], 1.2), SR);
    let got: Vec<i32> = heard.notes.iter().map(|n| n.pitch).collect();
    let held = heard.notes.first().and_then(|n| n.contour.last()).map(|c| c.1);
    eprintln!("slide: {got:?}, held at {held:?}¢");
    assert_eq!(got, vec![57]);
    assert!((held.unwrap() - 300.0).abs() < 20.0);
}

#[test]
fn a_hammer_on_is_a_second_note_without_a_second_pick() {
    let mut a = pluck(0.1, 0.9, 57.0, 0.4);
    a.hammer = Some((0.45, 2.0));
    let heard = hear(&render(&[a], 1.1), SR);
    let got: Vec<(i32, f64)> = heard.notes.iter().map(|n| (n.pitch, n.start)).collect();
    eprintln!("hammer-on: {got:?}");
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!((got[0].0, got[1].0), (57, 59));
    assert!((got[1].1 - 0.45).abs() < 0.03, "the hammer landed at {}", got[1].1);
    assert!(heard.polyphonic.is_empty());
}

#[test]
fn strummed_chroma_names_the_key_through_the_key_detector() {
    // i iv V i VI VII V i in G minor, one strum each.
    let progression: [(&str, [f32; 3]); 8] = [
        ("Gm", [55.0, 58.0, 62.0]),
        ("Cm", [48.0, 51.0, 55.0]),
        ("D", [50.0, 54.0, 57.0]),
        ("Gm", [55.0, 58.0, 62.0]),
        ("D#", [51.0, 55.0, 58.0]),
        ("F", [53.0, 57.0, 60.0]),
        ("D", [50.0, 54.0, 57.0]),
        ("Gm", [55.0, 58.0, 62.0]),
    ];
    let mut plucks = Vec::new();
    for (i, (_, notes)) in progression.iter().enumerate() {
        let at = 0.1 + i as f64 * 0.6;
        for (s, &m) in notes.iter().enumerate() {
            plucks.push(pluck(at + s as f64 * 0.012, at + 0.6, m, 0.3));
        }
    }
    let heard = hear(&render(&plucks, 5.0), SR);
    assert!(heard.notes.is_empty(), "notes written over strums: {:?}", heard.notes);
    let names: Vec<&str> = heard.chords.iter().map(|c| c.name.as_str()).collect();
    // Merging only joins neighbours, and no two neighbours here are the same chord.
    let want: Vec<&str> = progression.iter().map(|p| p.0).collect();
    let right = names.iter().zip(&want).filter(|(a, b)| a == b).count();
    eprintln!("chords {right}/{} right: {names:?}", want.len());
    assert_eq!(names, want);

    eprintln!("chroma {:?}", heard.chroma.map(|c| (c * 10.0).round() / 10.0));
    let key = KeyDetector::default().detect_with_chroma(&[], &heard.chroma);
    let top = key.top().unwrap();
    eprintln!("key: {} {:?} (score {:.3}, confident {})", top.root_pitch_class, top.scale, top.score, key.is_confident);
    assert_eq!((top.root_pitch_class, top.scale), (7, ScaleType::Minor));
    assert!(key.is_confident);
}

#[test]
fn silence_hears_nothing() {
    let heard = hear(&vec![0.0; 48_000], SR);
    assert!(heard.notes.is_empty() && heard.chords.is_empty() && heard.polyphonic.is_empty());
    assert!(hear(&[], SR).notes.is_empty());
}
