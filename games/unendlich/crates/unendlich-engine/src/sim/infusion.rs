//! sim::infusion — the infusion-slot economy for crafted items.
//!
//! From `design_library/06/crafting_infusion_genetics`: every crafted item has a
//! hidden genome with a small number of **infusion slots** (0–4, set at creation
//! from `material_purity × forge_bonus`) that infusions *and* enchantments draw
//! from, plus up to three **sway marks** (elemental imprints from the craft
//! context). The spec's key rule: the **third sway mark is dominant** — once an
//! item carries three, that third mark overrides the primary affinity for the
//! purpose of infusion/enchant compatibility.
//!
//! `sim::enchant_apply` already does the *stat* math (level·per_level clamped);
//! this module owns the orthogonal *slot & affinity* bookkeeping it doesn't. The
//! amplify/clean/resist verdict itself depends on an elemental cycle that isn't
//! yet canon in one place (the tree has several `Element` enums), so this core
//! keeps that verdict **pluggable**: the caller passes the [`Relation`] it
//! computed from whatever elemental system, and this module handles capacity,
//! consumption, dominance, and outcome plumbing. Elements are opaque `u8` ids
//! here; named constants mirror the doc's six. Pure, std-only, deterministic.

/// Opaque elemental id. Constants below mirror the design's six attunements;
/// any consistent numbering works since this module never interprets them.
pub type ElementId = u8;

pub const WOOD: ElementId = 0;
pub const WATER: ElementId = 1;
pub const AIR: ElementId = 2;
pub const DUST: ElementId = 3;
pub const FIRE: ElementId = 4;
pub const STORM: ElementId = 5;

/// The maximum infusion slots any item can hold, per the spec (0–4).
pub const MAX_SLOTS: u8 = 4;
/// The maximum sway marks an item can accumulate (0–3).
pub const MAX_SWAY_MARKS: usize = 3;

/// How an infusion's element sits with the item's effective affinity. The caller
/// computes this from its elemental cycle; this module just carries it into the
/// outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// Same/generative element — the infusion takes at full (or boosted) effect.
    Amplified,
    /// Neutral — the infusion takes plainly.
    Clean,
    /// Opposed/destructive element — the infusion is dampened.
    Resisted,
}

/// The result of attempting an infusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfuseOutcome {
    /// Applied, consuming one slot; `slots_left` remain.
    Applied { relation: Relation, slots_left: u8 },
    /// No free slots — nothing changed.
    NoSlots,
}

/// Slots at creation from base material quality and the forge's slot grant.
/// `material_purity` is clamped to `[0, 1]`; `forge_bonus` is the forge's
/// maximum grant (e.g. a village forge 2, an Ancient Shrine Forge 4). Result is
/// clamped to `[0, MAX_SLOTS]`.
pub fn infusion_capacity(material_purity: f32, forge_bonus: u8) -> u8 {
    let p = material_purity.clamp(0.0, 1.0);
    let raw = (p * forge_bonus as f32).round();
    (raw as i32).clamp(0, MAX_SLOTS as i32) as u8
}

/// The infusion/affinity bookkeeping for one item's genome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Infusable {
    /// Primary element alignment set at creation.
    pub primary: ElementId,
    /// Up to three accumulated sway marks (imprints from the craft context).
    pub sway_marks: [Option<ElementId>; MAX_SWAY_MARKS],
    /// Total infusion slots (capacity).
    pub capacity: u8,
    /// Slots already consumed (by infusions and enchantments alike).
    pub used: u8,
}

impl Infusable {
    pub fn new(primary: ElementId, capacity: u8) -> Infusable {
        Infusable {
            primary,
            sway_marks: [None; MAX_SWAY_MARKS],
            capacity: capacity.min(MAX_SLOTS),
            used: 0,
        }
    }

    /// Free slots remaining.
    pub fn remaining(&self) -> u8 {
        self.capacity.saturating_sub(self.used)
    }

    /// Is there room for one more infusion or enchantment?
    pub fn has_slot(&self) -> bool {
        self.remaining() > 0
    }

    /// The affinity that governs compatibility: normally the primary, but once a
    /// **third** sway mark is present it dominates and overrides the primary.
    pub fn effective_affinity(&self) -> ElementId {
        match self.sway_marks[2] {
            Some(third) => third,
            None => self.primary,
        }
    }

    /// Record a sway mark in the first free slot (up to three). Returns `false`
    /// if all three are already taken.
    pub fn add_sway_mark(&mut self, element: ElementId) -> bool {
        for m in self.sway_marks.iter_mut() {
            if m.is_none() {
                *m = Some(element);
                return true;
            }
        }
        false
    }

    /// How many sway marks are present.
    pub fn sway_mark_count(&self) -> usize {
        self.sway_marks.iter().filter(|m| m.is_some()).count()
    }

    /// Consume a slot for an enchantment (which draws from the same pool as
    /// infusions). Returns `true` if a slot was available and consumed.
    pub fn consume_slot(&mut self) -> bool {
        if self.has_slot() {
            self.used += 1;
            true
        } else {
            false
        }
    }

    /// Apply an infusion whose compatibility `relation` the caller has computed.
    /// Consumes one slot on success. `NoSlots` (no state change) when full.
    pub fn apply_infusion(&mut self, relation: Relation) -> InfuseOutcome {
        if !self.has_slot() {
            return InfuseOutcome::NoSlots;
        }
        self.used += 1;
        InfuseOutcome::Applied { relation, slots_left: self.remaining() }
    }
}

/// A safe default compatibility rule that makes **no** claims about an elemental
/// cycle: same element ⇒ `Amplified`, otherwise `Clean`. There is no `Resisted`
/// here because opposition isn't canon yet — swap this for a real cycle once one
/// exists, without touching [`Infusable`].
pub fn relation_same_or_clean(infusion: ElementId, affinity: ElementId) -> Relation {
    if infusion == affinity {
        Relation::Amplified
    } else {
        Relation::Clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_scales_with_purity_and_forge_and_clamps() {
        assert_eq!(infusion_capacity(0.0, 4), 0, "junk material ⇒ no slots");
        assert_eq!(infusion_capacity(1.0, 4), 4, "pure + shrine forge ⇒ 4");
        assert_eq!(infusion_capacity(0.5, 4), 2, "half purity halves the grant");
        assert_eq!(infusion_capacity(1.0, 2), 2, "a weaker forge caps lower");
        assert_eq!(infusion_capacity(1.0, 9), MAX_SLOTS, "never exceeds MAX_SLOTS");
        assert_eq!(infusion_capacity(-3.0, 4), 0, "purity clamped up from below");
    }

    #[test]
    fn a_fresh_item_has_all_its_slots_free() {
        let it = Infusable::new(FIRE, 3);
        assert_eq!(it.remaining(), 3);
        assert!(it.has_slot());
        assert_eq!(it.used, 0);
    }

    #[test]
    fn infusions_consume_slots_until_full() {
        let mut it = Infusable::new(WATER, 2);
        assert_eq!(it.apply_infusion(Relation::Clean), InfuseOutcome::Applied { relation: Relation::Clean, slots_left: 1 });
        assert_eq!(it.apply_infusion(Relation::Amplified), InfuseOutcome::Applied { relation: Relation::Amplified, slots_left: 0 });
        assert_eq!(it.apply_infusion(Relation::Clean), InfuseOutcome::NoSlots, "no slots ⇒ rejected");
        assert_eq!(it.used, 2, "a rejected infusion must not consume anything");
    }

    #[test]
    fn enchantments_and_infusions_share_the_same_pool() {
        let mut it = Infusable::new(AIR, 2);
        assert!(it.consume_slot(), "enchant takes a slot");
        assert!(matches!(it.apply_infusion(Relation::Clean), InfuseOutcome::Applied { slots_left: 0, .. }));
        assert!(!it.consume_slot(), "pool exhausted by 1 enchant + 1 infusion");
    }

    #[test]
    fn sway_marks_fill_three_then_refuse() {
        let mut it = Infusable::new(WOOD, 4);
        assert!(it.add_sway_mark(FIRE));
        assert!(it.add_sway_mark(WATER));
        assert!(it.add_sway_mark(STORM));
        assert!(!it.add_sway_mark(DUST), "only three marks");
        assert_eq!(it.sway_mark_count(), 3);
    }

    #[test]
    fn the_third_sway_mark_dominates_the_affinity() {
        let mut it = Infusable::new(WOOD, 4);
        assert_eq!(it.effective_affinity(), WOOD, "primary governs at first");
        it.add_sway_mark(FIRE);
        it.add_sway_mark(WATER);
        assert_eq!(it.effective_affinity(), WOOD, "one/two marks don't override");
        it.add_sway_mark(STORM);
        assert_eq!(it.effective_affinity(), STORM, "the third mark overrides the primary");
    }

    #[test]
    fn default_relation_only_amplifies_on_a_match() {
        assert_eq!(relation_same_or_clean(FIRE, FIRE), Relation::Amplified);
        assert_eq!(relation_same_or_clean(FIRE, WATER), Relation::Clean);
    }

    #[test]
    fn relation_flows_through_to_the_outcome() {
        let mut it = Infusable::new(STORM, 3);
        let elem = STORM;
        let rel = relation_same_or_clean(elem, it.effective_affinity());
        assert_eq!(it.apply_infusion(rel), InfuseOutcome::Applied { relation: Relation::Amplified, slots_left: 2 });
    }

    #[test]
    fn the_dominant_mark_changes_which_infusions_amplify() {
        // A wood item swayed thrice to Storm should now amplify Storm infusions,
        // not Wood ones.
        let mut it = Infusable::new(WOOD, 4);
        it.add_sway_mark(FIRE);
        it.add_sway_mark(WATER);
        it.add_sway_mark(STORM);
        assert_eq!(relation_same_or_clean(WOOD, it.effective_affinity()), Relation::Clean);
        assert_eq!(relation_same_or_clean(STORM, it.effective_affinity()), Relation::Amplified);
    }

    #[test]
    fn bookkeeping_is_deterministic() {
        let mut a = Infusable::new(FIRE, 3);
        let mut b = Infusable::new(FIRE, 3);
        for r in [Relation::Clean, Relation::Amplified] {
            assert_eq!(a.apply_infusion(r), b.apply_infusion(r));
        }
        assert_eq!(a, b);
    }
}
