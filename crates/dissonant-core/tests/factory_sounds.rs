//! The factory sound library (`assets/factory_sounds.json`): size, ids, categories, and that
//! every patch is already in range and reads the same in Rust and in the UI.

use dissonant_core::sound::{default_kit, factory_sound, factory_sounds, DrumPatch, Sound, SoundPatch, SynthPatch};
use serde_json::{Map, Value};

const RAW: &str = include_str!("../assets/factory_sounds.json");

const SYNTH_CATEGORIES: &[&str] = &["basic", "bass", "lead", "pad", "keys", "pluck", "bell", "brass", "arp", "fx"];
const DRUM_CATEGORIES: &[&str] = &["kick", "snare", "clap", "hat", "cymbal", "tom", "perc"];

fn raw_values() -> Vec<Value> {
    match serde_json::from_str(RAW).expect("factory_sounds.json is valid JSON") {
        Value::Array(v) => v,
        _ => panic!("factory_sounds.json must be an array"),
    }
}

fn raw_sounds() -> Vec<Sound> {
    serde_json::from_str(RAW).expect("factory_sounds.json parses as sounds")
}

fn is_kebab(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[test]
fn library_is_big_enough() {
    let sounds = raw_sounds();
    let drums = sounds.iter().filter(|s| s.is_drum()).count();
    let synths = sounds.len() - drums;
    assert!(synths >= 46, "only {synths} synth sounds");
    assert!(drums >= 44, "only {drums} drum sounds");
    assert_eq!(factory_sounds().len(), sounds.len());
}

#[test]
fn ids_are_unique_kebab_slugs_and_names_are_set() {
    let sounds = raw_sounds();
    let mut seen = std::collections::HashSet::new();
    for s in &sounds {
        assert!(is_kebab(&s.id), "id {:?} is not a lowercase kebab slug", s.id);
        assert!(seen.insert(s.id.as_str()), "duplicate id {}", s.id);
        assert!(!s.name.trim().is_empty(), "{} has no name", s.id);
    }
    for id in [
        "saw", "square", "triangle", "sine", "pad", "pluck", "kick", "snare", "clap", "hat", "open-hat", "crash", "tom-hi",
        "tom-lo",
    ] {
        assert!(seen.contains(id), "legacy id {id} missing");
    }
}

#[test]
fn every_sound_has_an_allowed_category() {
    for s in raw_sounds() {
        let allowed = if s.is_drum() { DRUM_CATEGORIES } else { SYNTH_CATEGORIES };
        assert!(allowed.contains(&s.category.as_str()), "{} has category {:?}", s.id, s.category);
    }
}

#[test]
fn every_patch_is_already_in_range() {
    for s in raw_sounds() {
        assert_eq!(s.clone().sanitized(), s, "{} changes when sanitized (a value is out of range)", s.id);
    }
}

#[test]
fn default_kit_sounds_exist_and_are_drums() {
    for pad in default_kit() {
        assert!(factory_sound(&pad.sound).is_some_and(Sound::is_drum), "kit sound {} missing or not a drum", pad.sound);
    }
}

/// Every key in the JSON must be a real field: serde ignores unknown keys, so a typo would
/// silently fall back to the default.
#[test]
fn no_unknown_keys() {
    fn check(id: &str, path: &str, raw: &Value, known: &Value) {
        match (raw, known) {
            (Value::Object(r), Value::Object(k)) => {
                for (key, v) in r {
                    let p = format!("{path}.{key}");
                    let kv = k.get(key).unwrap_or_else(|| panic!("{id}: unknown key {p}"));
                    check(id, &p, v, kv);
                }
            }
            (Value::Array(r), Value::Array(k)) => {
                for (i, v) in r.iter().enumerate() {
                    check(id, &format!("{path}[{i}]"), v, &k[i]);
                }
            }
            _ => {}
        }
    }
    for (raw, sound) in raw_values().iter().zip(raw_sounds()) {
        let known = serde_json::to_value(&sound).unwrap();
        check(&sound.id, "", raw, &known);
    }
}

/// The UI fills a partial patch by deep-merging it over the whole default patch, while serde
/// fills each missing nested field from that struct's own default (e.g. a partial `filterEnv`
/// gets `Envelope::default()`, a partial `osc[0]` is off). The factory JSON must be written so
/// both read it the same way.
#[test]
fn rust_and_ui_fill_partial_patches_the_same() {
    fn merge(base: &Value, partial: &Value) -> Value {
        match (base, partial) {
            (Value::Object(b), Value::Object(p)) => {
                let mut out = Map::new();
                for (k, bv) in b {
                    out.insert(k.clone(), p.get(k).map_or_else(|| bv.clone(), |pv| merge(bv, pv)));
                }
                Value::Object(out)
            }
            (Value::Array(b), Value::Array(p)) => {
                Value::Array(b.iter().enumerate().map(|(i, bv)| p.get(i).map_or_else(|| bv.clone(), |pv| merge(bv, pv))).collect())
            }
            (b, Value::Null) => b.clone(),
            (_, p) => p.clone(),
        }
    }
    let synth = serde_json::to_value(SoundPatch::Synth(SynthPatch::default())).unwrap();
    let drum = serde_json::to_value(SoundPatch::Drum(DrumPatch::default())).unwrap();
    for (raw, sound) in raw_values().iter().zip(raw_sounds()) {
        let base = if sound.is_drum() { &drum } else { &synth };
        let ui: SoundPatch = serde_json::from_value(merge(base, &raw["patch"])).unwrap();
        assert_eq!(ui, sound.patch, "{}: UI-style fill differs from serde's (write nested objects in full)", sound.id);
    }
}
