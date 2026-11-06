//! The factory kit library (`assets/drum_kits.json`): size, ids, that every kit is one the
//! commands accept, and the GM layout that lets a song's hits survive a kit swap.

use dissonant_core::sound::{default_kit, factory_kit, factory_kits, factory_sound, valid_kit, Sound};
use dissonant_core::{Command, Document, ProjectModel};

const STYLES: &[&str] = &["machines", "hip-hop", "club", "bass music", "acoustic", "world", "lo-fi", "weird"];

const KICK: u8 = 36;
const CLOSED_HAT: u8 = 42;
const OPEN_HAT: u8 = 46;

fn is_kebab(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[test]
fn library_is_big_enough_and_every_style_is_used() {
    let kits = factory_kits();
    assert!(kits.len() >= 36, "only {} kits", kits.len());
    for style in STYLES {
        assert!(kits.iter().any(|k| k.style == *style), "no kit in style {style:?}");
    }
}

#[test]
fn ids_are_unique_kebab_slugs_and_text_is_set() {
    let mut seen = std::collections::HashSet::new();
    for k in factory_kits() {
        assert!(is_kebab(&k.id), "kit id {:?} is not a lowercase kebab slug", k.id);
        assert!(seen.insert(k.id.as_str()), "duplicate kit id {}", k.id);
        assert!(!k.name.trim().is_empty(), "{} has no name", k.id);
        assert!(!k.blurb.trim().is_empty(), "{} has no blurb", k.id);
        assert!(STYLES.contains(&k.style.as_str()), "{} has style {:?}", k.id, k.style);
    }
}

#[test]
fn every_kit_is_valid_and_plays_factory_drums() {
    for k in factory_kits() {
        assert!(valid_kit(&k.drum_pads()), "{} is not a valid kit", k.id);
        for (_, sound, _) in &k.pads {
            assert!(factory_sound(sound).is_some_and(Sound::is_drum), "{}: {sound} is not a factory drum", k.id);
        }
    }
}

/// Swapping kits keeps a song's hits on their pitches, so every kit puts a kick where hits
/// written on any other kit expect one, and an open hat stops when its closed hat plays.
#[test]
fn kits_follow_the_gm_layout() {
    for k in factory_kits() {
        let pads = k.drum_pads();
        let kick = pads.iter().find(|p| p.pitch == KICK).unwrap_or_else(|| panic!("{} has no pad on {KICK}", k.id));
        // The taiko kit's big drum stands in for the kick.
        let category = factory_sound(&kick.sound).map(|s| s.category.as_str());
        assert!(category == Some("kick") || kick.sound == "taiko", "{}: pad {KICK} is not a kick", k.id);
        let closed = pads.iter().find(|p| p.pitch == CLOSED_HAT);
        let open = pads.iter().find(|p| p.pitch == OPEN_HAT);
        if let (Some(c), Some(o)) = (closed, open) {
            assert!(c.choke > 0 && c.choke == o.choke, "{}: the open hat doesn't choke on the closed one", k.id);
        }
    }
}

#[test]
fn the_starter_kit_is_the_default_kit() {
    assert_eq!(factory_kit("starter").map(|k| k.drum_pads()), Some(default_kit()));
}

#[test]
fn every_kit_loads_onto_a_drum_track_as_one_undo_step() {
    // The starter's drum track already plays the starter kit, so loading that one is a no-op.
    for k in factory_kits().iter().filter(|k| k.id != "starter") {
        let mut d = Document::new(ProjectModel::starter());
        let drums = d.model().tracks.iter().find(|t| t.is_drum).expect("the starter has a drum track").id;
        let before = d.model().clone();
        d.apply(Command::SetDrumPads { id: drums, pads: k.drum_pads(), sounds: vec![] }, false)
            .unwrap_or_else(|e| panic!("{} refused: {e:?}", k.id));
        assert_eq!(d.model().tracks.iter().find(|t| t.id == drums).unwrap().drum_pads, k.drum_pads());
        assert!(d.model().sounds.is_empty(), "{}: factory kits embed nothing", k.id);
        assert!(d.undo());
        assert_eq!(d.model(), &before, "{}: one undo puts the old kit back", k.id);
    }
}
