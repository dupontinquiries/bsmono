//! Achievement / coverage tracking — "what did this session actually exercise?"
//!
//! Gameplay and sim code register the achievements they *expect* a session to
//! cover (ideally from a test-spec file), then call [`Achievements::unlock`]
//! when the corresponding thing happens. The session report serializes which
//! expected achievements were hit, so Claude can see at a glance what a
//! playtester touched — and, crucially, what they never tried.
//!
//! Pure `std` — no external crates.

use std::collections::BTreeMap;

/// One trackable thing-to-test.
#[derive(Clone, Debug)]
pub struct Achievement {
    pub id: String,
    pub label: String,
    pub category: String,
    pub unlocked: bool,
    /// Sim tick (or frame) at first unlock, if known.
    pub first_at: Option<u64>,
    /// How many times the underlying event fired (even after first unlock).
    pub count: u64,
}

impl Achievement {
    fn new(id: &str, label: &str, category: &str) -> Self {
        Achievement {
            id: id.to_string(),
            label: label.to_string(),
            category: category.to_string(),
            unlocked: false,
            first_at: None,
            count: 0,
        }
    }
}

/// A set of expected achievements plus their live unlock state.
///
/// Insertion-independent ordering (BTreeMap by id) keeps the emitted report
/// deterministic across runs and machines.
#[derive(Clone, Debug, Default)]
pub struct Achievements {
    items: BTreeMap<String, Achievement>,
}

impl Achievements {
    pub fn new() -> Self {
        Achievements::default()
    }

    /// Declare an expected achievement. Re-registering keeps existing state
    /// (so a spec reload doesn't wipe progress) but refreshes label/category.
    pub fn register(&mut self, id: &str, label: &str, category: &str) {
        match self.items.get_mut(id) {
            Some(a) => {
                a.label = label.to_string();
                a.category = category.to_string();
            }
            None => {
                self.items.insert(id.to_string(), Achievement::new(id, label, category));
            }
        }
    }

    /// Register many at once: `(id, label, category)`.
    pub fn register_all(&mut self, specs: &[(&str, &str, &str)]) {
        for (id, label, cat) in specs {
            self.register(id, label, cat);
        }
    }

    /// Record that the thing behind `id` happened. Unknown ids are
    /// auto-registered (uncategorized) so nothing silently vanishes — better
    /// to surface an unexpected event than to drop it. Returns `true` on the
    /// transition from locked→unlocked (the first time).
    pub fn unlock(&mut self, id: &str, at_tick: u64) -> bool {
        let a = self
            .items
            .entry(id.to_string())
            .or_insert_with(|| Achievement::new(id, id, "uncategorized"));
        a.count = a.count.saturating_add(1);
        if !a.unlocked {
            a.unlocked = true;
            a.first_at = Some(at_tick);
            true
        } else {
            false
        }
    }

    pub fn is_unlocked(&self, id: &str) -> bool {
        self.items.get(id).map(|a| a.unlocked).unwrap_or(false)
    }

    pub fn get(&self, id: &str) -> Option<&Achievement> {
        self.items.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Achievement> {
        self.items.values()
    }

    /// `(unlocked, total)`.
    pub fn coverage(&self) -> (usize, usize) {
        let total = self.items.len();
        let got = self.items.values().filter(|a| a.unlocked).count();
        (got, total)
    }

    pub fn unlocked_ids(&self) -> Vec<&str> {
        self.items
            .values()
            .filter(|a| a.unlocked)
            .map(|a| a.id.as_str())
            .collect()
    }

    pub fn missing_ids(&self) -> Vec<&str> {
        self.items
            .values()
            .filter(|a| !a.unlocked)
            .map(|a| a.id.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_transition_and_count() {
        let mut a = Achievements::new();
        a.register("placed_block", "Place a block", "build");
        assert!(!a.is_unlocked("placed_block"));
        assert!(a.unlock("placed_block", 100)); // first time -> true
        assert!(!a.unlock("placed_block", 105)); // again -> false
        let it = a.get("placed_block").unwrap();
        assert_eq!(it.count, 2);
        assert_eq!(it.first_at, Some(100));
    }

    #[test]
    fn coverage_and_missing() {
        let mut a = Achievements::new();
        a.register_all(&[
            ("a", "A", "x"),
            ("b", "B", "x"),
            ("c", "C", "y"),
        ]);
        a.unlock("a", 1);
        assert_eq!(a.coverage(), (1, 3));
        let mut miss = a.missing_ids();
        miss.sort();
        assert_eq!(miss, vec!["b", "c"]);
    }

    #[test]
    fn unknown_id_autoregisters() {
        let mut a = Achievements::new();
        assert!(a.unlock("surprise_event", 7));
        assert_eq!(a.coverage(), (1, 1));
        assert_eq!(a.get("surprise_event").unwrap().category, "uncategorized");
    }
}
