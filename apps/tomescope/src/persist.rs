//! Persistence layer: everything is plain `serde_json` written under the
//! OS config directory via the `dirs` crate, namespaced under
//! "tomescope". No database — a reading app's state (a library index, a
//! handful of prefs blobs, and one small JSON per open book) is well
//! within what flat JSON files handle comfortably, and it keeps the app
//! trivially inspectable/backupable by the user.

use crate::library::Library;
use crate::stats::StatsLog;
use crate::study::{Deck, Highlight};
use crate::theme::ReaderPrefs;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("tomescope")
}

pub fn covers_dir() -> PathBuf {
    config_dir().join("covers")
}

pub fn book_state_dir() -> PathBuf {
    config_dir().join("book_state")
}

fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(covers_dir())?;
    std::fs::create_dir_all(book_state_dir())?;
    Ok(())
}

/// Atomically write JSON to `path`: write to a sibling temp file first,
/// then rename over the target, so a crash mid-write can't corrupt
/// existing state.
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    ensure_dirs()?;
    let json = serde_json::to_string_pretty(value)?;
    let tmp_path = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(json.as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

// ---------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------

pub fn library_path() -> PathBuf {
    config_dir().join("library.json")
}

pub fn save_library(lib: &Library) -> anyhow::Result<()> {
    write_json_atomic(&library_path(), lib)
}

pub fn load_library() -> Library {
    read_json(&library_path())
}

// ---------------------------------------------------------------------
// Reader preferences + typography (app-wide, not per book)
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AppPrefsBlob {
    pub reader: ReaderPrefs,
    pub typography: crate::layout::Typography,
    pub app_palette: String,
    pub keymap: Keymap,
}

pub fn prefs_path() -> PathBuf {
    config_dir().join("prefs.json")
}

pub fn save_prefs(blob: &AppPrefsBlob) -> anyhow::Result<()> {
    write_json_atomic(&prefs_path(), blob)
}

pub fn load_prefs() -> AppPrefsBlob {
    read_json(&prefs_path())
}

// ---------------------------------------------------------------------
// Per-book state: reading position, highlights, bookmarks
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct BookState {
    pub position: crate::content::Locus,
    pub highlights: Vec<Highlight>,
    pub bookmarks: Vec<crate::panels::Bookmark>,
    pub raster_page: usize,
}

fn book_state_path(book_id: u64) -> PathBuf {
    book_state_dir().join(format!("{book_id:016x}.json"))
}

pub fn save_book_state(book_id: u64, state: &BookState) -> anyhow::Result<()> {
    write_json_atomic(&book_state_path(book_id), state)
}

pub fn load_book_state(book_id: u64) -> BookState {
    read_json(&book_state_path(book_id))
}

// ---------------------------------------------------------------------
// Global stats log + flashcard deck (single files, not per-book, so
// cross-book stats/decks are trivial to query)
// ---------------------------------------------------------------------

pub fn stats_path() -> PathBuf {
    config_dir().join("stats.json")
}

pub fn save_stats(log: &StatsLog) -> anyhow::Result<()> {
    write_json_atomic(&stats_path(), log)
}

pub fn load_stats() -> StatsLog {
    read_json(&stats_path())
}

pub fn deck_path() -> PathBuf {
    config_dir().join("flashcards.json")
}

pub fn save_deck(deck: &Deck) -> anyhow::Result<()> {
    write_json_atomic(&deck_path(), deck)
}

pub fn load_deck() -> Deck {
    read_json(&deck_path())
}

// ---------------------------------------------------------------------
// Keymap (desktop: customizable shortcuts for page turn, search, etc.)
// ---------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Keymap {
    pub bindings: HashMap<String, String>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        bindings.insert("next_page".to_string(), "ArrowRight".to_string());
        bindings.insert("prev_page".to_string(), "ArrowLeft".to_string());
        bindings.insert("toggle_toc".to_string(), "T".to_string());
        bindings.insert("search".to_string(), "Ctrl+F".to_string());
        bindings.insert("bookmark".to_string(), "B".to_string());
        bindings.insert("command_palette".to_string(), "Ctrl+K".to_string());
        bindings.insert("zoom_in".to_string(), "Ctrl+=".to_string());
        bindings.insert("zoom_out".to_string(), "Ctrl+-".to_string());
        Self { bindings }
    }
}

impl Keymap {
    pub fn action_for_key(&self, key_str: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|(_, v)| v.as_str() == key_str)
            .map(|(k, _)| k.as_str())
    }
}

/// Export the entire app state (library, prefs, stats, deck) as one
/// bundle for backup/transfer, independent of the per-file storage above.
#[derive(Serialize, serde::Deserialize)]
pub struct FullBackup {
    pub library: Library,
    pub prefs: AppPrefsBlob,
    pub stats: StatsLog,
    pub deck: Deck,
}

pub fn export_full_backup() -> FullBackup {
    FullBackup {
        library: load_library(),
        prefs: load_prefs(),
        stats: load_stats(),
        deck: load_deck(),
    }
}

pub fn import_full_backup(backup: &FullBackup) -> anyhow::Result<()> {
    save_library(&backup.library)?;
    save_prefs(&backup.prefs)?;
    save_stats(&backup.stats)?;
    save_deck(&backup.deck)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_default_has_core_actions() {
        let km = Keymap::default();
        assert!(km.bindings.contains_key("next_page"));
        assert!(km.bindings.contains_key("search"));
    }

    #[test]
    fn keymap_lookup_by_key_string() {
        let km = Keymap::default();
        assert_eq!(km.action_for_key("Ctrl+F"), Some("search"));
    }
}
