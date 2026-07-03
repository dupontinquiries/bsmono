//! `sim::whitelist` — access control for a hosted world (r518).
//!
//! A host can run **open** (anyone who finds the advert may join) or **closed**
//! (only named players on the whitelist are admitted). This is the pure data +
//! policy layer: it answers "may this player name participate?" and manages the
//! allow-list. The networking layer calls [`Whitelist::admits`] when a peer's
//! identity (its `CharacterProfile.name`, carried on the avatar channel) arrives,
//! and ignores peers it rejects.
//!
//! Persisted per-world as `whitelist.json` under the world root, alongside the
//! flora store and chunk edits, written atomically. Pure + deterministic; name
//! matching is case-insensitive and trims surrounding whitespace so "Ada" and
//! " ada " are the same entry.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

fn norm(name: &str) -> String {
    name.trim().to_lowercase()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Whitelist {
    /// When true, anyone is admitted and the allow-list is ignored (but retained).
    pub open: bool,
    /// Normalised allowed names (lowercased, trimmed).
    allowed: BTreeSet<String>,
}

impl Default for Whitelist {
    fn default() -> Self {
        // Sensible default for a casual host: open, so friends can just join.
        Self { open: true, allowed: BTreeSet::new() }
    }
}

impl Whitelist {
    pub fn new() -> Self {
        Self::default()
    }

    /// May a player with this name participate? Open servers admit everyone; closed
    /// servers admit only listed names. The host itself should never be filtered by
    /// this — call it for *remote* peers only.
    pub fn admits(&self, name: &str) -> bool {
        self.open || self.allowed.contains(&norm(name))
    }

    /// Add a name to the allow-list. Returns true if it was newly added.
    pub fn allow(&mut self, name: &str) -> bool {
        let n = norm(name);
        if n.is_empty() {
            return false;
        }
        self.allowed.insert(n)
    }

    /// Remove a name. Returns true if it was present.
    pub fn revoke(&mut self, name: &str) -> bool {
        self.allowed.remove(&norm(name))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.allowed.contains(&norm(name))
    }

    pub fn len(&self) -> usize {
        self.allowed.len()
    }
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }

    /// The allow-list in sorted order, for display in the management UI.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(|s| s.as_str())
    }

    /// Flip open/closed; returns the new state.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }
    pub fn toggle_open(&mut self) -> bool {
        self.open = !self.open;
        self.open
    }

    fn store_path(world_root: &Path) -> PathBuf {
        world_root.join("whitelist.json")
    }

    pub fn save(&self, world_root: &Path) -> io::Result<()> {
        let path = Self::store_path(world_root);
        let tmp = path.with_extension("json.tmp");
        let bytes =
            serde_json::to_vec(self).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(world_root: &Path) -> Self {
        match std::fs::read(Self::store_path(world_root)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_admits_everyone() {
        let w = Whitelist { open: true, allowed: BTreeSet::new() };
        assert!(w.admits("anyone"));
        assert!(w.admits("Stranger"));
    }

    #[test]
    fn closed_admits_only_listed() {
        let mut w = Whitelist::new();
        w.set_open(false);
        assert!(!w.admits("Ada"));
        assert!(w.allow("Ada"));
        assert!(w.admits("Ada"));
        assert!(w.admits("  ada ")); // case + whitespace insensitive
        assert!(!w.admits("Bob"));
    }

    #[test]
    fn allow_revoke() {
        let mut w = Whitelist::new();
        assert!(w.allow("Grace"));
        assert!(!w.allow("grace")); // dup (normalised)
        assert_eq!(w.len(), 1);
        assert!(w.revoke("GRACE"));
        assert!(w.is_empty());
        assert!(!w.revoke("nobody"));
    }

    #[test]
    fn empty_name_rejected() {
        let mut w = Whitelist::new();
        assert!(!w.allow("   "));
        assert_eq!(w.len(), 0);
    }

    #[test]
    fn roundtrip_disk() {
        let dir = std::env::temp_dir().join(format!("wl_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut w = Whitelist::new();
        w.set_open(false);
        w.allow("Ada");
        w.allow("Grace");
        w.save(&dir).unwrap();
        let back = Whitelist::load(&dir);
        assert!(!back.open);
        assert!(back.contains("ada"));
        assert_eq!(back.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
