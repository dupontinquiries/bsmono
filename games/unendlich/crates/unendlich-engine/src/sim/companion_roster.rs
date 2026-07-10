//! `sim::companion_roster` — named companions across three slots, with bond ranks
//! that unlock abilities.
//!
//! From `SPECULATIVE/SPEC-007_companion_system_spec.docx`. Extends the dragon-rider
//! bond (`sim::bond_tame`) to multiple creatures at once: a player holds up to
//! **three** active bonds — one **Dragon** (any element), one **Fauna** (any bonded
//! wild creature), and one **Auxiliary** (a Lauscher or specialist cave/biome
//! creature). Each companion has three abilities that trigger on command or
//! auto-activate by combat/environment context; how many are available is gated by
//! the companion's **bond depth**.

/// Which roster slot a companion occupies (one companion per slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompanionSlot {
    /// Any bonded dragon (acquired via the Dragon Rider quest chain).
    Dragon,
    /// Any bonded wild creature (curiosity accumulator maxed).
    Fauna,
    /// A Lauscher or specialist creature (followed 10+ sessions / rare biome find).
    Auxiliary,
}

impl CompanionSlot {
    pub const ALL: [CompanionSlot; 3] =
        [CompanionSlot::Dragon, CompanionSlot::Fauna, CompanionSlot::Auxiliary];
}

/// Bond depth tier — deeper bonds unlock more of a companion's three abilities and
/// new passive behaviours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BondRank {
    /// ◇ base (depth 1–9): follows the player, uses Ability 1 in combat.
    Fledgling,
    /// depth 10–24: Ability 2 + passive behaviours unlock.
    Trusted,
    /// depth 25+: the full three-ability kit.
    Bonded,
}

impl BondRank {
    /// The rank for a given bond depth.
    pub const fn from_depth(depth: u32) -> BondRank {
        if depth >= 25 {
            BondRank::Bonded
        } else if depth >= 10 {
            BondRank::Trusted
        } else {
            BondRank::Fledgling
        }
    }
    /// How many of the three abilities are available at this rank.
    pub const fn abilities_unlocked(self) -> u8 {
        match self {
            BondRank::Fledgling => 1,
            BondRank::Trusted => 2,
            BondRank::Bonded => 3,
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            BondRank::Fledgling => "Fledgling",
            BondRank::Trusted => "Trusted",
            BondRank::Bonded => "Bonded",
        }
    }
}

/// One named, bonded companion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Companion {
    pub slot: CompanionSlot,
    /// Stable id of the named creature (name lives in the string table).
    pub creature_id: u32,
    /// Accumulated bond depth.
    pub bond_depth: u32,
}

impl Companion {
    pub fn new(slot: CompanionSlot, creature_id: u32) -> Companion {
        Companion { slot, creature_id, bond_depth: 0 }
    }
    pub const fn rank(&self) -> BondRank {
        BondRank::from_depth(self.bond_depth)
    }
    pub const fn abilities_unlocked(&self) -> u8 {
        self.rank().abilities_unlocked()
    }
    /// Is ability `i` (0-based, of 3) unlocked at the current bond?
    pub const fn can_use(&self, i: u8) -> bool {
        i < self.abilities_unlocked()
    }
    /// Deepen the bond (clamped so a companion can't exceed the top rank's floor
    /// by an unbounded amount — depth is meaningful, not a raw XP bar).
    pub fn deepen(&mut self, by: u32) {
        self.bond_depth = self.bond_depth.saturating_add(by).min(99);
    }
}

/// A player's active companions — at most one per slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompanionRoster {
    dragon: Option<Companion>,
    fauna: Option<Companion>,
    auxiliary: Option<Companion>,
}

impl CompanionRoster {
    pub const fn new() -> CompanionRoster {
        CompanionRoster { dragon: None, fauna: None, auxiliary: None }
    }

    fn slot_ref(&self, slot: CompanionSlot) -> &Option<Companion> {
        match slot {
            CompanionSlot::Dragon => &self.dragon,
            CompanionSlot::Fauna => &self.fauna,
            CompanionSlot::Auxiliary => &self.auxiliary,
        }
    }
    fn slot_mut(&mut self, slot: CompanionSlot) -> &mut Option<Companion> {
        match slot {
            CompanionSlot::Dragon => &mut self.dragon,
            CompanionSlot::Fauna => &mut self.fauna,
            CompanionSlot::Auxiliary => &mut self.auxiliary,
        }
    }

    /// Bond a companion into its slot, replacing any current occupant. Returns the
    /// displaced companion, if any.
    pub fn set(&mut self, c: Companion) -> Option<Companion> {
        self.slot_mut(c.slot).replace(c)
    }
    /// Dismiss the companion in `slot`, returning it.
    pub fn clear(&mut self, slot: CompanionSlot) -> Option<Companion> {
        self.slot_mut(slot).take()
    }
    pub fn get(&self, slot: CompanionSlot) -> Option<&Companion> {
        self.slot_ref(slot).as_ref()
    }
    pub fn get_mut(&mut self, slot: CompanionSlot) -> Option<&mut Companion> {
        self.slot_mut(slot).as_mut()
    }
    /// How many slots are filled (0..3).
    pub fn active_count(&self) -> usize {
        CompanionSlot::ALL.iter().filter(|&&s| self.slot_ref(s).is_some()).count()
    }
    pub fn is_full(&self) -> bool {
        self.active_count() == 3
    }
    /// Total abilities available across the whole roster right now — the size of the
    /// player's contextual ability bar from companions.
    pub fn total_abilities(&self) -> u8 {
        CompanionSlot::ALL
            .iter()
            .filter_map(|&s| self.slot_ref(s).map(|c| c.abilities_unlocked()))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bond_rank_gates_abilities() {
        assert_eq!(BondRank::from_depth(1).abilities_unlocked(), 1);
        assert_eq!(BondRank::from_depth(9), BondRank::Fledgling);
        assert_eq!(BondRank::from_depth(10), BondRank::Trusted);
        assert_eq!(BondRank::from_depth(10).abilities_unlocked(), 2);
        assert_eq!(BondRank::from_depth(25), BondRank::Bonded);
        assert_eq!(BondRank::from_depth(99).abilities_unlocked(), 3);
    }

    #[test]
    fn companion_ability_availability() {
        let mut c = Companion::new(CompanionSlot::Dragon, 7);
        // Fledgling: only ability 0.
        assert!(c.can_use(0) && !c.can_use(1) && !c.can_use(2));
        c.deepen(12); // → Trusted
        assert!(c.can_use(1) && !c.can_use(2));
        c.deepen(50); // → Bonded (clamped ≤ 99)
        assert!(c.can_use(2));
        assert!(c.bond_depth <= 99);
    }

    #[test]
    fn roster_one_per_slot() {
        let mut r = CompanionRoster::new();
        assert_eq!(r.active_count(), 0);
        r.set(Companion::new(CompanionSlot::Dragon, 1));
        r.set(Companion::new(CompanionSlot::Fauna, 2));
        assert_eq!(r.active_count(), 2);
        assert!(!r.is_full());
        // Setting the Dragon slot again displaces the old dragon, not a new slot.
        let displaced = r.set(Companion::new(CompanionSlot::Dragon, 9));
        assert_eq!(displaced.unwrap().creature_id, 1);
        assert_eq!(r.active_count(), 2);
        assert_eq!(r.get(CompanionSlot::Dragon).unwrap().creature_id, 9);
        // Fill the auxiliary slot → full.
        r.set(Companion::new(CompanionSlot::Auxiliary, 3));
        assert!(r.is_full());
    }

    #[test]
    fn total_abilities_sums_the_roster() {
        let mut r = CompanionRoster::new();
        let mut d = Companion::new(CompanionSlot::Dragon, 1);
        d.deepen(30); // Bonded → 3
        r.set(d);
        r.set(Companion::new(CompanionSlot::Fauna, 2)); // Fledgling → 1
        assert_eq!(r.total_abilities(), 4);
    }

    #[test]
    fn clear_frees_the_slot() {
        let mut r = CompanionRoster::new();
        r.set(Companion::new(CompanionSlot::Auxiliary, 5));
        assert_eq!(r.clear(CompanionSlot::Auxiliary).unwrap().creature_id, 5);
        assert_eq!(r.active_count(), 0);
        assert!(r.get(CompanionSlot::Auxiliary).is_none());
    }
}
