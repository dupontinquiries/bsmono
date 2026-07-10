//! `sim::collections` — the character panel's Collections tab: factual,
//! encyclopaedic knowledge the player has unlocked.
//!
//! From `SPECULATIVE/SPEC-010_voxel_rendering_locations_collections.docx`. Unlike
//! the Journal (which records narrative events), Collections records *factual*
//! knowledge — every formation, creature genome, technique, item, and named
//! location the player has encountered becomes a collection entry, each with its own
//! unlock condition. This module is the unlock tracker + completion accounting.

/// A category of collectible knowledge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CollectionCategory {
    /// The 275+ formation types from the Formation Dictionary.
    Formations,
    /// Creature genomes, revealed by encounter / Genome Reader.
    Creatures,
    /// Bending techniques, unlocked by successful execution.
    Techniques,
    /// Items, unlocked when one enters the inventory / is identified.
    Items,
    /// Named physical locations, unlocked on first visit.
    Locations,
}

impl CollectionCategory {
    pub const ALL: [CollectionCategory; 5] = [
        CollectionCategory::Formations,
        CollectionCategory::Creatures,
        CollectionCategory::Techniques,
        CollectionCategory::Items,
        CollectionCategory::Locations,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            CollectionCategory::Formations => "Formations",
            CollectionCategory::Creatures => "Creatures",
            CollectionCategory::Techniques => "Techniques",
            CollectionCategory::Items => "Items",
            CollectionCategory::Locations => "Locations",
        }
    }

    /// How an entry in this category becomes unlocked (doc "Unlock condition").
    pub const fn unlock_condition(self) -> UnlockCondition {
        match self {
            CollectionCategory::Formations => UnlockCondition::Executed,
            CollectionCategory::Techniques => UnlockCondition::Executed,
            CollectionCategory::Creatures => UnlockCondition::Revealed,
            CollectionCategory::Items => UnlockCondition::EnteredInventory,
            CollectionCategory::Locations => UnlockCondition::Visited,
        }
    }
}

/// The action that unlocks a collection entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockCondition {
    /// Successfully executed (formations, techniques).
    Executed,
    /// The item entered the inventory / was identified.
    EnteredInventory,
    /// Revealed by Information bending or a Genome Reader (creatures).
    Revealed,
    /// The player visited the named location.
    Visited,
}

/// The player's unlocked encyclopaedic knowledge.
#[derive(Clone, Debug, Default)]
pub struct CollectionLog {
    unlocked: std::collections::HashSet<(CollectionCategory, u32)>,
}

impl CollectionLog {
    pub fn new() -> CollectionLog {
        CollectionLog { unlocked: std::collections::HashSet::new() }
    }

    /// Record that entry `id` in `category` is now known. Returns `true` if this is
    /// the first time (a *new* discovery worth surfacing to the player).
    pub fn unlock(&mut self, category: CollectionCategory, id: u32) -> bool {
        self.unlocked.insert((category, id))
    }

    pub fn is_unlocked(&self, category: CollectionCategory, id: u32) -> bool {
        self.unlocked.contains(&(category, id))
    }

    /// How many entries are unlocked in a category.
    pub fn count(&self, category: CollectionCategory) -> u32 {
        self.unlocked.iter().filter(|(c, _)| *c == category).count() as u32
    }

    /// Total unlocked across all categories.
    pub fn total(&self) -> u32 {
        self.unlocked.len() as u32
    }

    /// Completion fraction `0..1` for a category, given its content total. Totals
    /// are content-driven (e.g. Formations has 275+), so the caller supplies them.
    pub fn completion(&self, category: CollectionCategory, total_entries: u32) -> f32 {
        if total_entries == 0 {
            return 1.0;
        }
        (self.count(category).min(total_entries) as f32) / total_entries as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_is_idempotent_and_flags_new() {
        let mut log = CollectionLog::new();
        assert!(log.unlock(CollectionCategory::Techniques, 3)); // new
        assert!(!log.unlock(CollectionCategory::Techniques, 3)); // already known
        assert!(log.is_unlocked(CollectionCategory::Techniques, 3));
        assert!(!log.is_unlocked(CollectionCategory::Techniques, 4));
    }

    #[test]
    fn counts_are_per_category() {
        let mut log = CollectionLog::new();
        log.unlock(CollectionCategory::Formations, 1);
        log.unlock(CollectionCategory::Formations, 2);
        log.unlock(CollectionCategory::Creatures, 1);
        assert_eq!(log.count(CollectionCategory::Formations), 2);
        assert_eq!(log.count(CollectionCategory::Creatures), 1);
        assert_eq!(log.count(CollectionCategory::Items), 0);
        assert_eq!(log.total(), 3);
    }

    #[test]
    fn completion_fraction() {
        let mut log = CollectionLog::new();
        for id in 0..55 {
            log.unlock(CollectionCategory::Formations, id);
        }
        // 55 of 275 formations known → 20%.
        assert!((log.completion(CollectionCategory::Formations, 275) - 0.2).abs() < 1e-4);
        // An empty category is 0%; a zero-total category is trivially complete.
        assert_eq!(log.completion(CollectionCategory::Items, 100), 0.0);
        assert_eq!(log.completion(CollectionCategory::Items, 0), 1.0);
    }

    #[test]
    fn categories_have_the_doc_unlock_conditions() {
        assert_eq!(CollectionCategory::Techniques.unlock_condition(), UnlockCondition::Executed);
        assert_eq!(CollectionCategory::Items.unlock_condition(), UnlockCondition::EnteredInventory);
        assert_eq!(CollectionCategory::Creatures.unlock_condition(), UnlockCondition::Revealed);
        assert_eq!(CollectionCategory::Locations.unlock_condition(), UnlockCondition::Visited);
    }
}
