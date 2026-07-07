//! `sim::world_catalog` — the in-memory model behind the menu's Load / New /
//! Delete screens.
//!
//! The bin owns disk IO (it scans `~/.unendlich/worlds/` and writes saves); this
//! module is the pure model: the list of saved worlds, name validation, and the
//! recency ordering the Load screen shows. [`WorldCatalog::slot_count`] /
//! [`WorldCatalog::has_save`] feed `sim::title_flow::TitleFlow::new` (whether the
//! title screen offers "Continue"/"Load"). Catalog invariant: **names are
//! unique** ([`add`](WorldCatalog::add) and [`from_entries`](WorldCatalog::from_entries)
//! both enforce it).
//!
//! Pure: data in, data out. No `std::fs`/IO, no `crate::render`, no `ash`/`vk`,
//! no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code; compiles clean
//! under `cargo check --release` (no `cfg(test)`/`cfg(debug_assertions)` gated
//! helpers).

/// How a world is played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameMode {
    Creative,
    Survival,
}

/// Metadata for one saved world (the bin maps this to a directory on disk).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldEntry {
    pub name: String,
    pub seed: u64,
    pub mode: GameMode,
    pub created_tick: u64,
    pub last_played_tick: u64,
}

/// The list of saved worlds (unique by `name`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorldCatalog {
    entries: Vec<WorldEntry>,
}

/// Maximum world-name length (in characters) the bin allows on disk.
pub const MAX_NAME_LEN: usize = 32;

/// Is `name` a legal world name? Non-empty (after trimming), at most
/// [`MAX_NAME_LEN`] characters, and only filesystem-safe characters: ASCII
/// letters/digits, space, `-`, `_`. Defined unconditionally (release-safe).
pub fn valid_world_name(name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    let n = name.chars().count();
    if n == 0 || n > MAX_NAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

impl WorldCatalog {
    /// An empty catalog.
    pub fn new() -> WorldCatalog {
        WorldCatalog { entries: Vec::new() }
    }

    /// Build from a list, dropping any later entry whose `name` duplicates an
    /// earlier one (first occurrence wins), preserving the unique-name invariant.
    pub fn from_entries(entries: Vec<WorldEntry>) -> WorldCatalog {
        let mut c = WorldCatalog::new();
        for e in entries {
            c.add(e);
        }
        c
    }

    /// Add a world. Returns `false` (and changes nothing) if an entry with the
    /// same `name` already exists.
    pub fn add(&mut self, e: WorldEntry) -> bool {
        if self.entries.iter().any(|x| x.name == e.name) {
            return false;
        }
        self.entries.push(e);
        true
    }

    /// Remove the world named `name`. Returns `true` if one was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        match self.entries.iter().position(|x| x.name == name) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    /// All entries, most-recently-played first. Ties (equal `last_played_tick`)
    /// are broken by `name` ascending (stable, deterministic).
    pub fn by_recent(&self) -> Vec<&WorldEntry> {
        let mut refs: Vec<&WorldEntry> = self.entries.iter().collect();
        refs.sort_by(|a, b| {
            b.last_played_tick
                .cmp(&a.last_played_tick)
                .then_with(|| a.name.cmp(&b.name))
        });
        refs
    }

    /// Number of saved worlds, saturating at `u8::MAX`.
    pub fn slot_count(&self) -> u8 {
        if self.entries.len() > u8::MAX as usize {
            u8::MAX
        } else {
            self.entries.len() as u8
        }
    }

    /// Whether any saved world exists (drives "Continue"/"Load" on the title).
    pub fn has_save(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Borrow the raw entries (unordered, in insertion order).
    pub fn entries(&self) -> &[WorldEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, last: u64) -> WorldEntry {
        WorldEntry {
            name: name.to_string(),
            seed: 1,
            mode: GameMode::Survival,
            created_tick: 0,
            last_played_tick: last,
        }
    }

    #[test]
    fn by_recent_orders_desc_with_name_tiebreak() {
        let c = WorldCatalog::from_entries(vec![
            entry("alpha", 100),
            entry("bravo", 300),
            entry("charlie", 100), // tie with alpha at 100
            entry("delta", 200),
        ]);
        let order: Vec<&str> = c.by_recent().iter().map(|e| e.name.as_str()).collect();
        // 300, 200, then the 100-tie alpha<charlie by name
        assert_eq!(order, vec!["bravo", "delta", "alpha", "charlie"]);
        assert_eq!(c.by_recent().len(), 4, "returns every entry");
    }

    #[test]
    fn duplicate_add_rejected() {
        let mut c = WorldCatalog::new();
        assert!(c.add(entry("world", 10)));
        assert!(!c.add(entry("world", 99)), "same name must be rejected");
        assert_eq!(c.slot_count(), 1);
        // the original entry is untouched
        assert_eq!(c.by_recent()[0].last_played_tick, 10);
    }

    #[test]
    fn remove_existing_and_missing() {
        let mut c = WorldCatalog::from_entries(vec![entry("a", 1), entry("b", 2)]);
        assert!(c.remove("a"));
        assert!(!c.remove("a"), "already removed");
        assert!(!c.remove("nope"), "missing name returns false");
        assert_eq!(c.slot_count(), 1);
        assert_eq!(c.by_recent()[0].name, "b");
    }

    #[test]
    fn slot_count_and_has_save_reflect_entries() {
        let mut c = WorldCatalog::new();
        assert_eq!(c.slot_count(), 0);
        assert!(!c.has_save());
        c.add(entry("one", 1));
        assert_eq!(c.slot_count(), 1);
        assert!(c.has_save());
        // saturating at u8::MAX
        let mut big = WorldCatalog::new();
        for i in 0..300u32 {
            big.add(entry(&format!("w{}", i), i as u64));
        }
        assert_eq!(big.slot_count(), 255, "slot_count saturates at 255");
        assert!(big.has_save());
    }

    #[test]
    fn from_entries_dedupes_keeping_first() {
        let c = WorldCatalog::from_entries(vec![
            entry("dup", 1),
            entry("other", 5),
            entry("dup", 99), // dropped
        ]);
        assert_eq!(c.slot_count(), 2);
        // first "dup" kept (last_played 1, not 99)
        let dup = c.entries().iter().find(|e| e.name == "dup").map(|e| e.last_played_tick);
        assert_eq!(dup, Some(1));
    }

    #[test]
    fn name_validation() {
        // valid
        assert!(valid_world_name("My World"));
        assert!(valid_world_name("save_1"));
        assert!(valid_world_name("level-2"));
        assert!(valid_world_name("a"));
        assert!(valid_world_name(&"x".repeat(MAX_NAME_LEN)));
        // invalid
        assert!(!valid_world_name(""), "empty");
        assert!(!valid_world_name("   "), "all whitespace");
        assert!(!valid_world_name(&"x".repeat(MAX_NAME_LEN + 1)), "over length");
        assert!(!valid_world_name("bad/name"), "path separator");
        assert!(!valid_world_name("back\\slash"));
        assert!(!valid_world_name("dot.name"), "dot");
        assert!(!valid_world_name("colon:name"));
        assert!(!valid_world_name("emoji \u{1F600}"));
        assert!(!valid_world_name("tab\tname"));
        assert!(!valid_world_name("null\0name"));
    }

    #[test]
    fn deterministic_ordering() {
        let c = WorldCatalog::from_entries(vec![
            entry("z", 5),
            entry("y", 5),
            entry("x", 9),
            entry("w", 1),
        ]);
        let a: Vec<&str> = c.by_recent().iter().map(|e| e.name.as_str()).collect();
        let b: Vec<&str> = c.by_recent().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(a, b);
        assert_eq!(a, vec!["x", "y", "z", "w"]); // 9, then 5-tie y<z, then 1
    }

    #[test]
    fn empty_catalog_views() {
        let c = WorldCatalog::new();
        assert!(c.by_recent().is_empty());
        assert_eq!(c.slot_count(), 0);
        assert!(!c.has_save());
        assert!(c.entries().is_empty());
    }
}
