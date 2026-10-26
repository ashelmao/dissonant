//! The global user sound library: one `sound-library.json` (an array of [`Sound`]) in the
//! app-data dir, shared by every project. Projects embed their own copies of the sounds they
//! use, so this file is only ever the browser's source, never the engine's.
//!
//! Loading is tolerant: a missing file is an empty library, and a file that does not parse is
//! moved aside to `sound-library.corrupt-<ms>.json` (never silently overwritten by the next
//! save). Entries that fail on their own are dropped, after the original is copied aside the
//! same way.

use dissonant_core::sound::is_factory;
use dissonant_core::Sound;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE_NAME: &str = "sound-library.json";

/// Every read-modify-write goes through this, so two saves can't lose each other's sound.
static LOCK: Mutex<()> = Mutex::new(());

/// The library at `path`, in file order. Never fails; see the module docs.
pub fn load(path: &Path) -> Vec<Sound> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_unlocked(path)
}

/// Insert `sound` (sanitized), replacing any sound with the same id in place. Factory ids and
/// ids without the `u-` prefix are refused. Returns the whole library.
pub fn save(path: &Path, sound: Sound) -> Result<Vec<Sound>, String> {
    check_id(&sound.id)?;
    let sound = sound.sanitized();
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut sounds = load_unlocked(path);
    match sounds.iter_mut().find(|s| s.id == sound.id) {
        Some(slot) => *slot = sound,
        None => sounds.push(sound),
    }
    write(path, &sounds).map_err(|e| format!("could not save the sound library: {e}"))?;
    Ok(sounds)
}

/// Remove a sound by id. An id that is not in the library changes nothing. Returns the whole
/// library.
pub fn delete(path: &Path, id: &str) -> Result<Vec<Sound>, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut sounds = load_unlocked(path);
    let before = sounds.len();
    sounds.retain(|s| s.id != id);
    if sounds.len() != before {
        write(path, &sounds).map_err(|e| format!("could not save the sound library: {e}"))?;
    }
    Ok(sounds)
}

fn check_id(id: &str) -> Result<(), String> {
    if is_factory(id) {
        return Err(format!("\"{id}\" is a factory sound; save a copy under a new id"));
    }
    if !id.starts_with("u-") {
        return Err(format!("a library sound id must start with \"u-\" (got \"{id}\")"));
    }
    Ok(())
}

fn load_unlocked(path: &Path) -> Vec<Sound> {
    let Ok(json) = std::fs::read_to_string(path) else { return vec![] };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(&json) else {
        // Not even a JSON array: move it out of the way so the next save starts clean.
        let backup = backup_path(path);
        log::warn!("sound library unreadable, moved to {}", backup.display());
        let _ = std::fs::rename(path, &backup);
        return vec![];
    };
    let total = items.len();
    let mut sounds: Vec<Sound> = Vec::with_capacity(total);
    for item in items {
        match serde_json::from_value::<Sound>(item) {
            Ok(s) if check_id(&s.id).is_ok() && !sounds.iter().any(|k| k.id == s.id) => sounds.push(s.sanitized()),
            _ => {}
        }
    }
    if sounds.len() != total {
        // Keep the original before rewriting, so a sound this build can't read isn't lost.
        let backup = backup_path(path);
        log::warn!("sound library had {} bad entries; original copied to {}", total - sounds.len(), backup.display());
        if std::fs::copy(path, &backup).is_ok() {
            let _ = write(path, &sounds);
        }
    }
    sounds
}

/// Write via a temp file and a rename, so a crash mid-write leaves the old library intact.
fn write(path: &Path, sounds: &[Sound]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(sounds).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `sound-library.corrupt-<ms>.json` next to `path`, with a counter if that is taken.
fn backup_path(path: &Path) -> PathBuf {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sound-library");
    let mut n = 0;
    loop {
        let suffix = if n == 0 { String::new() } else { format!("-{n}") };
        let candidate = path.with_file_name(format!("{stem}.corrupt-{ms}{suffix}.json"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dissonant_core::{DrumPatch, SoundPatch, SynthPatch};

    /// A fresh, empty directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("dissonant-library-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn library(&self) -> PathBuf {
            self.0.join(FILE_NAME)
        }

        fn backups(&self) -> Vec<PathBuf> {
            std::fs::read_dir(&self.0)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains(".corrupt-")))
                .collect()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn synth(id: &str, level: f32) -> Sound {
        Sound { id: id.into(), name: id.into(), category: "lead".into(), patch: SoundPatch::Synth(SynthPatch { level, ..SynthPatch::default() }) }
    }

    fn ids(sounds: &[Sound]) -> Vec<&str> {
        sounds.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn a_missing_file_is_an_empty_library() {
        let dir = TempDir::new();
        assert!(load(&dir.library()).is_empty());
        assert!(!dir.library().exists(), "loading created the file");
    }

    #[test]
    fn save_inserts_then_replaces_in_place_and_persists() {
        let dir = TempDir::new();
        let path = dir.library();
        save(&path, synth("u-a", 0.1)).unwrap();
        save(&path, synth("u-b", 0.2)).unwrap();
        let lib = save(&path, synth("u-a", 0.9)).unwrap();
        assert_eq!(ids(&lib), ["u-a", "u-b"], "a re-save kept its place");
        assert_eq!(lib[0].patch, SoundPatch::Synth(SynthPatch { level: 0.9, ..SynthPatch::default() }));
        assert_eq!(load(&path), lib, "what save returned is what is on disk");
        assert!(!path.with_extension("json.tmp").exists(), "the temp file was left behind");
    }

    #[test]
    fn saved_sounds_are_sanitized() {
        let dir = TempDir::new();
        let mut s = synth("u-loud", 7.0);
        s.name = "  ".into();
        let lib = save(&dir.library(), s).unwrap();
        assert_eq!(lib[0].name, "untitled");
        assert_eq!(lib[0].patch, SoundPatch::Synth(SynthPatch { level: 1.0, ..SynthPatch::default() }));
    }

    #[test]
    fn factory_and_unprefixed_ids_are_refused() {
        let dir = TempDir::new();
        let path = dir.library();
        assert!(save(&path, synth("saw", 0.5)).unwrap_err().contains("factory"));
        let kick = Sound { id: "kick".into(), name: "kick".into(), category: "kick".into(), patch: SoundPatch::Drum(DrumPatch::default()) };
        assert!(save(&path, kick).unwrap_err().contains("factory"));
        assert!(save(&path, synth("my-sound", 0.5)).unwrap_err().contains("u-"));
        assert!(!path.exists(), "a refused save wrote the file");
    }

    #[test]
    fn delete_removes_by_id_and_ignores_unknown_ids() {
        let dir = TempDir::new();
        let path = dir.library();
        save(&path, synth("u-a", 0.1)).unwrap();
        save(&path, synth("u-b", 0.2)).unwrap();
        assert_eq!(ids(&delete(&path, "u-a").unwrap()), ["u-b"]);
        assert_eq!(ids(&delete(&path, "u-nope").unwrap()), ["u-b"]);
        assert_eq!(ids(&load(&path)), ["u-b"]);
        assert!(delete(&path, "u-b").unwrap().is_empty());
        assert!(load(&path).is_empty());
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_not_overwritten() {
        let dir = TempDir::new();
        let path = dir.library();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).is_empty());
        let backups = dir.backups();
        assert_eq!(backups.len(), 1, "the corrupt file was not backed up");
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "{ not json");
        // The next save starts a fresh library and leaves the backup alone.
        assert_eq!(ids(&save(&path, synth("u-a", 0.5)).unwrap()), ["u-a"]);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "{ not json");
        // A second corruption gets its own backup.
        std::fs::write(&path, "[").unwrap();
        assert!(load(&path).is_empty());
        assert_eq!(dir.backups().len(), 2);
    }

    #[test]
    fn bad_entries_are_dropped_after_backing_up_the_original() {
        let dir = TempDir::new();
        let path = dir.library();
        let good = serde_json::to_value(synth("u-good", 0.5)).unwrap();
        let dup = serde_json::to_value(synth("u-good", 0.9)).unwrap();
        let factory = serde_json::to_value(synth("saw", 0.5)).unwrap();
        let original = serde_json::to_string(&serde_json::json!([good, {"id": "u-broken"}, factory, dup])).unwrap();
        std::fs::write(&path, &original).unwrap();

        let lib = load(&path);
        assert_eq!(ids(&lib), ["u-good"]);
        assert_eq!(lib[0].patch, SoundPatch::Synth(SynthPatch { level: 0.5, ..SynthPatch::default() }), "the first copy wins");
        let backups = dir.backups();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
        // The file was rewritten clean, so the next load does not back up again.
        assert_eq!(load(&path), lib);
        assert_eq!(dir.backups().len(), 1);
    }
}
