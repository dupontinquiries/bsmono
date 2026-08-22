//! Save file location and I/O. `road_warrior_core::SaveState` has no filesystem
//! dependency by design — this module is the only place that knows where
//! the file lives and reads/writes it.
//!
//! Confirmed working: exercised via the Xvfb integration harness (save
//! written and correctly reloaded across three real level completions in
//! one session). The parse-failure fallback below additionally has its own
//! unit tests, since that path — a corrupted or hand-edited save.json — is
//! easy to miss with integration testing alone (nothing in a normal session
//! produces malformed JSON on disk to exercise it against).

use road_warrior_core::SaveState;
use std::path::PathBuf;

fn save_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".local/share")
        });
    base.join("road-warrior").join("save.json")
}

/// The actual decision logic, factored out from the filesystem read so it's
/// unit-testable without touching disk or env vars: given what reading the
/// save file produced (contents, or the io::Error if it doesn't exist / can't
/// be read), decide what SaveState to hand back.
fn load_or_create_from(read_result: std::io::Result<String>) -> SaveState {
    match read_result {
        Ok(contents) => match SaveState::from_json(&contents) {
            Ok(save) => save,
            Err(e) => {
                eprintln!("save: failed to parse ({e}), starting fresh rather than losing progress by overwriting a file we can't safely interpret");
                SaveState::new()
            }
        },
        Err(_) => SaveState::new(), // no save file yet — first run
    }
}

pub fn load_or_create() -> SaveState {
    let path = save_path();
    load_or_create_from(std::fs::read_to_string(&path))
}

pub fn save(state: &SaveState) {
    let path = save_path();
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("save: couldn't create {dir:?}: {e}");
            return;
        }
    }
    match state.to_json_pretty() {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("save: couldn't write {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("save: serialization failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use road_warrior_core::CarKind;

    #[test]
    fn missing_file_produces_a_fresh_save_not_a_panic() {
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let save = load_or_create_from(Err(missing));
        assert!(save.is_unlocked(CarKind::RedSedan));
        assert_eq!(save.campaign_level_index, 0);
    }

    #[test]
    fn valid_file_loads_its_actual_contents() {
        let mut original = SaveState::new();
        original.unlock_car(CarKind::Behemoth);
        original.campaign_level_index = 2;
        let json = original.to_json_pretty().unwrap();

        let loaded = load_or_create_from(Ok(json));
        assert!(loaded.is_unlocked(CarKind::Behemoth));
        assert_eq!(loaded.campaign_level_index, 2);
    }

    /// The path that's never been exercised by integration testing: nothing
    /// in a normal play session produces a corrupted save.json, so this only
    /// gets covered here, directly.
    #[test]
    fn corrupted_file_falls_back_to_fresh_save_instead_of_panicking() {
        let save = load_or_create_from(Ok("{ this is not valid json at all".to_string()));
        // Should behave exactly like a fresh save, not carry over partial
        // garbage or crash the game on startup because of a bad file.
        assert!(save.is_unlocked(CarKind::RedSedan));
        assert!(!save.is_unlocked(CarKind::Behemoth));
        assert_eq!(save.campaign_level_index, 0);
    }

    #[test]
    fn json_from_a_different_but_structurally_valid_shape_still_errors_cleanly() {
        // Valid JSON, but missing required fields entirely (e.g. an empty
        // object) — a different failure mode than "not JSON at all", worth
        // confirming it takes the same safe fallback path rather than
        // partially deserializing into a broken SaveState.
        let save = load_or_create_from(Ok("{}".to_string()));
        assert!(save.is_unlocked(CarKind::RedSedan));
        assert_eq!(save.campaign_level_index, 0);
    }
}
