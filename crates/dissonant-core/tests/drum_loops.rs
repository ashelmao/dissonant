//! The factory loop library (`assets/drum_loops.json`): size, ids, that every loop is whole
//! bars of clean step strings, plays on the kit it names, and tiles and swings as promised.

use dissonant_core::sound::{factory_kit, factory_kits};
use dissonant_core::{beats_to_ticks, factory_loops, DrumLoop, TICKS_PER_BEAT};

const BAR: i64 = 4 * TICKS_PER_BEAT;

fn is_kebab(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[test]
fn library_is_big_and_every_kit_has_a_loop() {
    let loops = factory_loops();
    assert!(loops.len() >= 60, "only {} loops", loops.len());
    for kit in factory_kits() {
        assert!(loops.iter().any(|l| l.kit == kit.id), "no loop written for the {} kit", kit.id);
    }
}

#[test]
fn ids_are_unique_kebab_slugs_and_text_is_set() {
    let mut seen = std::collections::HashSet::new();
    for l in factory_loops() {
        assert!(is_kebab(&l.id), "loop id {:?} is not a lowercase kebab slug", l.id);
        assert!(seen.insert(l.id.as_str()), "duplicate loop id {}", l.id);
        assert!(!l.name.trim().is_empty(), "{} has no name", l.id);
        assert!(!l.blurb.trim().is_empty(), "{} has no blurb", l.id);
        assert!((60.0..=200.0).contains(&l.bpm), "{}: bpm {}", l.id, l.bpm);
        assert!((50.0..=75.0).contains(&l.swing), "{}: swing {}", l.id, l.swing);
    }
}

#[test]
fn lanes_are_clean_step_strings_in_whole_bars() {
    for l in factory_loops() {
        let length = l.length_ticks();
        assert!([BAR, 2 * BAR, 4 * BAR].contains(&length), "{} is {} bars", l.id, length as f64 / BAR as f64);
        for lane in &l.lanes {
            let cells = lane.cells();
            assert!(cells.iter().all(|c| "Xxo.".contains(*c)), "{}: lane {} has a stray character", l.id, lane.pitch);
            assert!(cells.iter().any(|c| *c != '.'), "{}: lane {} is empty", l.id, lane.pitch);
            let step = beats_to_ticks(lane.step);
            assert!(step > 0 && (lane.step * TICKS_PER_BEAT as f64 - step as f64).abs() < 1e-6, "{}: step {} is no whole number of ticks", l.id, lane.step);
            let period = cells.len() as i64 * step;
            assert_eq!(length % period, 0, "{}: lane {} doesn't repeat evenly in the loop", l.id, lane.pitch);
        }
    }
}

/// A loop loaded with its own kit plays every hit, and each one has a kick.
#[test]
fn every_loop_plays_on_the_kit_it_names() {
    for l in factory_loops() {
        let kit = factory_kit(&l.kit).unwrap_or_else(|| panic!("{}: no factory kit {}", l.id, l.kit));
        for lane in &l.lanes {
            assert!(kit.pads.iter().any(|(pitch, ..)| i32::from(*pitch) == lane.pitch), "{}: {} has no pad on {}", l.id, kit.id, lane.pitch);
        }
        assert!(l.lanes.iter().any(|lane| lane.pitch == 36), "{} has no kick", l.id);
    }
}

#[test]
fn hits_tile_to_fill_a_longer_pattern_and_stop_at_a_shorter_one() {
    for l in factory_loops() {
        let once = l.hits(l.length_ticks());
        assert!(!once.is_empty());
        assert!(once.iter().all(|n| n.start_tick < l.length_ticks()));
        assert_eq!(l.hits(4 * l.length_ticks()).len(), 4 * once.len(), "{} doesn't tile", l.id);
        let bar = l.hits(BAR);
        assert!(bar.iter().all(|n| n.start_tick < BAR && n.start_tick + n.length_ticks <= BAR), "{} spills past a one-bar pattern", l.id);
        assert!(once.windows(2).all(|w| (w[0].start_tick, w[0].pitch) <= (w[1].start_tick, w[1].pitch)), "{} isn't sorted", l.id);
    }
}

#[test]
fn swing_shifts_only_the_off_sixteenths() {
    let swung: Vec<&DrumLoop> = factory_loops().iter().filter(|l| l.swing > 50.0).collect();
    assert!(!swung.is_empty());
    for l in swung {
        let late = l.swing_ticks();
        assert!(late > 0);
        for n in l.hits(l.length_ticks()) {
            let off = n.start_tick % 480 == 240;
            let lane_is_16ths = l.lanes.iter().any(|lane| lane.pitch == n.pitch && beats_to_ticks(lane.step) == 240);
            let expected = if off && lane_is_16ths { late } else { 0 };
            assert_eq!(n.shift_ticks, expected, "{}: hit at {} on {}", l.id, n.start_tick, n.pitch);
        }
    }
    let boom_bap = factory_loops().iter().find(|l| l.id == "boom-bap").unwrap();
    assert_eq!(boom_bap.swing_ticks(), 38, "58% swing puts the off-16th 38 ticks late");
    for l in factory_loops().iter().filter(|l| l.swing == 50.0) {
        assert!(l.hits(l.length_ticks()).iter().all(|n| n.shift_ticks == 0), "{} is straight", l.id);
    }
}
