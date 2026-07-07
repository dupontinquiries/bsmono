//! sim::ability_bar — the five active technique bindings.
//!
//! From `character_panel_reference §5`: a horizontal row of **five** slots at the
//! screen bottom, each holding one active technique the player has equipped (the
//! ones mapped to gesture grammar + voice words). This is distinct from the item
//! `hotbar`; it binds *techniques* — nodes from `sim::skill_constellation`. A
//! technique should only be bindable if it's unlocked, and a bound technique is
//! only *usable* when its environmental condition holds (slot 1's Glacial Form
//! needs `R < 0.20 + H > 0.40`). To stay standalone-testable this module keeps
//! those checks as caller-supplied flags rather than importing the constellation
//! directly; the bin passes `constellation.is_unlocked(id)` and the world-time
//! condition in. Deterministic, std-only.

use std::collections::BTreeSet;

/// Number of ability-bar slots.
pub const SLOTS: usize = 5;

/// A technique id — the same space as `skill_constellation::NodeId`.
pub type TechniqueId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    /// Slot index must be `0..SLOTS`.
    SlotOutOfRange,
}

/// The five bindings. `None` = empty slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbilityBar {
    slots: [Option<TechniqueId>; SLOTS],
}

impl Default for AbilityBar {
    fn default() -> Self {
        AbilityBar { slots: [None; SLOTS] }
    }
}

impl AbilityBar {
    pub fn new() -> AbilityBar {
        AbilityBar::default()
    }

    fn check(slot: usize) -> Result<(), BindError> {
        if slot < SLOTS {
            Ok(())
        } else {
            Err(BindError::SlotOutOfRange)
        }
    }

    /// Bind `tech` to `slot`. A technique can only occupy one slot, so if it's
    /// already bound elsewhere it is *moved* here (the old slot is cleared) —
    /// this keeps the five bindings distinct.
    pub fn assign(&mut self, slot: usize, tech: TechniqueId) -> Result<(), BindError> {
        Self::check(slot)?;
        if let Some(prev) = self.slot_of(tech) {
            if prev != slot {
                self.slots[prev] = None;
            }
        }
        self.slots[slot] = Some(tech);
        Ok(())
    }

    /// Empty a slot, returning what was there.
    pub fn clear(&mut self, slot: usize) -> Result<Option<TechniqueId>, BindError> {
        Self::check(slot)?;
        Ok(self.slots[slot].take())
    }

    /// What's bound in `slot`.
    pub fn binding(&self, slot: usize) -> Option<TechniqueId> {
        self.slots.get(slot).copied().flatten()
    }

    /// Which slot holds `tech`, if any.
    pub fn slot_of(&self, tech: TechniqueId) -> Option<usize> {
        self.slots.iter().position(|s| *s == Some(tech))
    }

    /// Swap the contents of two slots.
    pub fn swap(&mut self, a: usize, b: usize) -> Result<(), BindError> {
        Self::check(a)?;
        Self::check(b)?;
        self.slots.swap(a, b);
        Ok(())
    }

    /// How many slots are filled.
    pub fn bound_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// The distinct techniques currently bound (ascending).
    pub fn bound_techniques(&self) -> BTreeSet<TechniqueId> {
        self.slots.iter().filter_map(|s| *s).collect()
    }

    /// Is the technique in `slot` usable right now? It must be bound, `unlocked`
    /// (from the constellation), and — if it's environmentally gated — have its
    /// world/time condition `env_ok` satisfied. Non-environmental techniques pass
    /// `env_ok = true` (or the caller passes true).
    pub fn usable(&self, slot: usize, unlocked: bool, env_ok: bool) -> bool {
        self.binding(slot).is_some() && unlocked && env_ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_and_read_back() {
        let mut bar = AbilityBar::new();
        assert_eq!(bar.assign(0, 101), Ok(()));
        assert_eq!(bar.binding(0), Some(101));
        assert_eq!(bar.binding(1), None);
        assert_eq!(bar.bound_count(), 1);
    }

    #[test]
    fn out_of_range_slots_error() {
        let mut bar = AbilityBar::new();
        assert_eq!(bar.assign(SLOTS, 1), Err(BindError::SlotOutOfRange));
        assert_eq!(bar.clear(99), Err(BindError::SlotOutOfRange));
        assert!(bar.binding(99).is_none());
    }

    #[test]
    fn a_technique_occupies_only_one_slot() {
        let mut bar = AbilityBar::new();
        bar.assign(0, 101).unwrap();
        bar.assign(3, 101).unwrap(); // move to slot 3
        assert_eq!(bar.binding(0), None, "moved out of slot 0");
        assert_eq!(bar.binding(3), Some(101));
        assert_eq!(bar.bound_count(), 1);
        assert_eq!(bar.slot_of(101), Some(3));
    }

    #[test]
    fn five_distinct_techniques_fill_the_bar() {
        let mut bar = AbilityBar::new();
        for (i, t) in [11, 22, 33, 44, 55].iter().enumerate() {
            bar.assign(i, *t).unwrap();
        }
        assert_eq!(bar.bound_count(), 5);
        assert_eq!(bar.bound_techniques().len(), 5);
    }

    #[test]
    fn clear_and_swap() {
        let mut bar = AbilityBar::new();
        bar.assign(0, 1).unwrap();
        bar.assign(1, 2).unwrap();
        assert_eq!(bar.clear(0), Ok(Some(1)));
        assert_eq!(bar.binding(0), None);
        bar.assign(0, 3).unwrap();
        bar.swap(0, 1).unwrap();
        assert_eq!(bar.binding(0), Some(2));
        assert_eq!(bar.binding(1), Some(3));
    }

    #[test]
    fn usable_requires_bound_unlocked_and_env() {
        let mut bar = AbilityBar::new();
        bar.assign(0, 1).unwrap();
        assert!(bar.usable(0, true, true), "bound + unlocked + env ⇒ usable");
        assert!(!bar.usable(0, false, true), "locked technique ⇒ not usable");
        assert!(!bar.usable(0, true, false), "env gate closed ⇒ not usable");
        assert!(!bar.usable(1, true, true), "empty slot ⇒ not usable");
    }

    #[test]
    fn bindings_are_deterministic() {
        let mut a = AbilityBar::new();
        let mut b = AbilityBar::new();
        for (i, t) in [5, 6, 7].iter().enumerate() {
            a.assign(i, *t).unwrap();
            b.assign(i, *t).unwrap();
        }
        assert_eq!(a, b);
        assert_eq!(a.bound_techniques(), b.bound_techniques());
    }
}
