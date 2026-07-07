//! sim::armor_set — aggregate worn armour into a total modifier.
//!
//! From `design_library/06/item_armor_lore_tooltips`: elemental armour comes in
//! sets. Each worn piece contributes its own modifier (every piece is
//! independently useful), and wearing more of one set adds a **set bonus on
//! top** — a full set gives the full bonus, a partial set a partial one. The
//! damage/resist/durability maths already exist (`damage_after_armor`,
//! `resist_apply`, `item_durability`); what was missing is the step that turns a
//! *loadout* into the aggregate modifier those systems consume. That's this
//! module.
//!
//! The per-piece `modifier` is a plain `f32` so the caller picks what it means
//! (a threshold shift, a defense value, a chi discount) and aggregates one stat
//! at a time. Partial-set bonuses scale with completion fraction, matching the
//! design's "partial sets provide partial bonuses". Pure, deterministic,
//! std-only.

use std::collections::BTreeMap;

/// The armour slots (per the crafting doc: helm, chest, arms, hands, legs,
/// feet). Belt/accessory are separate and not part of a set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Head,
    Chest,
    Arms,
    Hands,
    Legs,
    Feet,
}

impl Slot {
    pub const ALL: [Slot; 6] = [Slot::Head, Slot::Chest, Slot::Arms, Slot::Hands, Slot::Legs, Slot::Feet];
    fn index(self) -> usize {
        match self {
            Slot::Head => 0,
            Slot::Chest => 1,
            Slot::Arms => 2,
            Slot::Hands => 3,
            Slot::Legs => 4,
            Slot::Feet => 5,
        }
    }
}

/// Number of slots in a complete set.
pub const SET_SIZE: u8 = 6;

/// A set/discipline identifier (Earth, Fire, …). Opaque to this module.
pub type SetId = u8;

/// One equipped armour piece: which set it belongs to and the stat it adds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub set: SetId,
    pub modifier: f32,
}

/// What's worn, one optional [`Piece`] per [`Slot`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loadout {
    slots: [Option<Piece>; 6],
}

impl Loadout {
    pub fn new() -> Loadout {
        Loadout { slots: [None; 6] }
    }

    /// Equip (replacing anything already in that slot); returns the displaced piece.
    pub fn equip(&mut self, slot: Slot, piece: Piece) -> Option<Piece> {
        self.slots[slot.index()].replace(piece)
    }

    pub fn unequip(&mut self, slot: Slot) -> Option<Piece> {
        self.slots[slot.index()].take()
    }

    pub fn get(&self, slot: Slot) -> Option<Piece> {
        self.slots[slot.index()]
    }

    /// Sum of every worn piece's modifier — always applied, regardless of sets.
    pub fn individual_total(&self) -> f32 {
        self.slots.iter().flatten().map(|p| p.modifier).sum()
    }

    /// How many pieces of each set are worn (deterministic order).
    pub fn set_counts(&self) -> BTreeMap<SetId, u8> {
        let mut m = BTreeMap::new();
        for p in self.slots.iter().flatten() {
            *m.entry(p.set).or_insert(0) += 1;
        }
        m
    }

    /// Fraction of `set` that's worn, in `[0, 1]` (`worn / SET_SIZE`).
    pub fn completion(&self, set: SetId) -> f32 {
        let worn = self.slots.iter().flatten().filter(|p| p.set == set).count() as f32;
        worn / SET_SIZE as f32
    }

    /// The set bonus currently granted by `set`, scaled by completion — a full
    /// set yields `full_bonus`, half a set half of it.
    pub fn set_bonus(&self, set: SetId, full_bonus: f32) -> f32 {
        full_bonus * self.completion(set)
    }

    /// The complete aggregate modifier: every piece's own contribution plus each
    /// worn set's completion-scaled bonus. `full_bonus` looks up a set's full
    /// bonus (missing sets contribute no bonus, only their piece modifiers).
    pub fn aggregate(&self, full_bonus: impl Fn(SetId) -> f32) -> f32 {
        let mut total = self.individual_total();
        for (&set, _) in self.set_counts().iter() {
            total += self.set_bonus(set, full_bonus(set));
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(set: SetId, m: f32) -> Piece {
        Piece { set, modifier: m }
    }

    const EARTH: SetId = 0;
    const FIRE: SetId = 1;

    #[test]
    fn each_piece_contributes_independently() {
        let mut l = Loadout::new();
        l.equip(Slot::Head, piece(EARTH, 0.04));
        l.equip(Slot::Chest, piece(EARTH, 0.06));
        assert!((l.individual_total() - 0.10).abs() < 1e-6);
    }

    #[test]
    fn a_full_set_grants_the_full_bonus() {
        let mut l = Loadout::new();
        for s in Slot::ALL {
            l.equip(s, piece(EARTH, 0.05));
        }
        assert!((l.completion(EARTH) - 1.0).abs() < 1e-6);
        assert!((l.set_bonus(EARTH, 0.30) - 0.30).abs() < 1e-6, "full set ⇒ full bonus");
    }

    #[test]
    fn a_partial_set_grants_a_proportional_bonus() {
        let mut l = Loadout::new();
        l.equip(Slot::Head, piece(EARTH, 0.05));
        l.equip(Slot::Chest, piece(EARTH, 0.05));
        l.equip(Slot::Legs, piece(EARTH, 0.05)); // 3 of 6
        assert!((l.completion(EARTH) - 0.5).abs() < 1e-6);
        assert!((l.set_bonus(EARTH, 0.30) - 0.15).abs() < 1e-6, "half a set ⇒ half the bonus");
    }

    #[test]
    fn aggregate_sums_pieces_and_set_bonuses() {
        let mut l = Loadout::new();
        // 3 Earth pieces (completion .5) + 1 Fire piece (completion 1/6).
        l.equip(Slot::Head, piece(EARTH, 0.04));
        l.equip(Slot::Chest, piece(EARTH, 0.04));
        l.equip(Slot::Legs, piece(EARTH, 0.04));
        l.equip(Slot::Feet, piece(FIRE, 0.03));
        let bonus = |s: SetId| if s == EARTH { 0.30 } else { 0.12 };
        let expected_individual = 0.04 * 3.0 + 0.03;
        let expected_earth_bonus = 0.30 * 0.5;
        let expected_fire_bonus = 0.12 * (1.0 / 6.0);
        let expected = expected_individual + expected_earth_bonus + expected_fire_bonus;
        assert!((l.aggregate(bonus) - expected).abs() < 1e-6, "got {}", l.aggregate(bonus));
    }

    #[test]
    fn mixed_sets_are_counted_separately() {
        let mut l = Loadout::new();
        l.equip(Slot::Head, piece(EARTH, 0.0));
        l.equip(Slot::Chest, piece(FIRE, 0.0));
        l.equip(Slot::Legs, piece(FIRE, 0.0));
        let counts = l.set_counts();
        assert_eq!(counts.get(&EARTH), Some(&1));
        assert_eq!(counts.get(&FIRE), Some(&2));
    }

    #[test]
    fn equipping_replaces_and_returns_the_old_piece() {
        let mut l = Loadout::new();
        assert_eq!(l.equip(Slot::Head, piece(EARTH, 0.05)), None);
        let old = l.equip(Slot::Head, piece(FIRE, 0.02));
        assert_eq!(old, Some(piece(EARTH, 0.05)));
        assert_eq!(l.get(Slot::Head), Some(piece(FIRE, 0.02)));
    }

    #[test]
    fn unequipping_removes_contribution() {
        let mut l = Loadout::new();
        l.equip(Slot::Head, piece(EARTH, 0.05));
        l.equip(Slot::Chest, piece(EARTH, 0.05));
        l.unequip(Slot::Head);
        assert!((l.individual_total() - 0.05).abs() < 1e-6);
        assert!((l.completion(EARTH) - (1.0 / 6.0)).abs() < 1e-6);
    }

    #[test]
    fn an_empty_loadout_is_all_zero() {
        let l = Loadout::new();
        assert_eq!(l.individual_total(), 0.0);
        assert_eq!(l.completion(EARTH), 0.0);
        assert_eq!(l.aggregate(|_| 0.30), 0.0);
        assert!(l.set_counts().is_empty());
    }

    #[test]
    fn aggregate_is_deterministic() {
        let mut l = Loadout::new();
        l.equip(Slot::Head, piece(EARTH, 0.04));
        l.equip(Slot::Chest, piece(FIRE, 0.03));
        let f = |s: SetId| if s == EARTH { 0.30 } else { 0.12 };
        assert_eq!(l.aggregate(&f), l.aggregate(&f));
    }
}
