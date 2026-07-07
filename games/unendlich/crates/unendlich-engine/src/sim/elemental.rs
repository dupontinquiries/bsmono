//! Elemental items — the staves, blades, and tools that channel the world's
//! elements, and the questable progression that grows them. This is the
//! backbone the individual items (earth wielder, fire/lightning/air staves, the
//! wolf blade, the hero's blade) plug into, so playtesters can feel different
//! elements and watch a weapon level up.
//!
//! Two axes:
//! - **Element** — what the item channels, which sets its cast profile (range,
//!   potency, charge cost). An earth tool heaves walls up close; a lightning
//!   staff strikes far for a lot of charge; dust is cheap and fast.
//! - **Progression** — the questable part. An item carries a *tier*, a *level*
//!   within that tier, and a *charge* pool. Deeds done while it's carried —
//!   mining, slaying — attune it (matching deeds attune faster: mining earth for
//!   an earth tool). Enough attunement levels it up, which recharges it and
//!   raises its ceiling; at max level it can **evolve** into the next tier, the
//!   hook a recipe turns into the item's next form.
//!
//! Pure, integer, `Copy` — snapshots and rolls back with the sim.

use crate::world::blocks::BlockId;
use crate::world::rock::rock_physics;

/// The elemental palette. Extend freely — every match here is non-exhaustive
/// at the call sites (profiles/affinity have sensible defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Earth,
    Dust,
    Fire,
    Lightning,
    Air,
    Water,
    /// Living wood — growth and regeneration; weak offence, strong renewal.
    Wood,
}

/// How an element casts — the knobs that make variants feel different.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastProfile {
    /// Charge spent per cast.
    pub charge_cost: u16,
    /// Base effect strength.
    pub potency: u16,
    /// Reach in blocks.
    pub range: u8,
    /// Ticks between casts.
    pub cooldown: u8,
}

/// Base cast profile for an element (level/tier scale it up elsewhere).
pub fn base_profile(element: Element) -> CastProfile {
    match element {
        // Heavy, short, expensive: heaving earth walls.
        Element::Earth => CastProfile { charge_cost: 40, potency: 80, range: 6, cooldown: 24 },
        // Cheap, fast, weak: flinging grit.
        Element::Dust => CastProfile { charge_cost: 8, potency: 20, range: 10, cooldown: 6 },
        // Mid AoE.
        Element::Fire => CastProfile { charge_cost: 25, potency: 60, range: 16, cooldown: 16 },
        // Long, instant, costly.
        Element::Lightning => CastProfile { charge_cost: 50, potency: 90, range: 32, cooldown: 30 },
        // Cheap utility: push/lift.
        Element::Air => CastProfile { charge_cost: 12, potency: 30, range: 14, cooldown: 8 },
        // Balanced.
        Element::Water => CastProfile { charge_cost: 20, potency: 45, range: 18, cooldown: 12 },
        // Low offence, long reach — utility/renewal (its strength shows on shields).
        Element::Wood => CastProfile { charge_cost: 15, potency: 25, range: 20, cooldown: 14 },
    }
}

/// The physical form an elemental item takes — same element, different feel. A
/// staff casts at range; an axe or blade channels the element through a melee
/// swing (short reach, heavier hit, slower); a tool is utility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemForm {
    #[default]
    Staff,
    Axe,
    Blade,
    Tool,
}

/// Adjust a base (element) profile for the item's form. Melee forms trade reach
/// for a heavier strike; an axe is the heaviest and slowest.
pub fn form_adjust(form: ItemForm, p: CastProfile) -> CastProfile {
    match form {
        ItemForm::Staff => p,
        ItemForm::Axe => CastProfile {
            range: 2,                       // melee reach
            potency: p.potency * 3 / 2,     // a heavy cleave that channels the element
            cooldown: p.cooldown + p.cooldown / 2,
            charge_cost: p.charge_cost * 3 / 4, // a swing channels a little less than a full cast
        },
        ItemForm::Blade => CastProfile {
            range: 2,
            potency: p.potency * 6 / 5,
            cooldown: p.cooldown.saturating_sub(p.cooldown / 4),
            charge_cost: p.charge_cost * 3 / 4,
        },
        ItemForm::Tool => CastProfile { potency: p.potency / 2, ..p },
    }
}

/// A deed the wielder performed while carrying the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deed {
    /// Mined a block of this kind.
    BlockMined(BlockId),
    /// Slew a creature.
    MobSlain,
    /// Travelled this many blocks (utility/exploration attunement).
    Travelled(u32),
}

fn is_earthy(b: BlockId) -> bool {
    rock_physics(b).is_some() || matches!(b, BlockId::Dirt | BlockId::Grass | BlockId::Sand | BlockId::Gravel | BlockId::Clay)
}
fn is_ore(b: BlockId) -> bool {
    matches!(b, BlockId::GoldOre | BlockId::IronOre | BlockId::CoalOre | BlockId::CopperOre | BlockId::DiamondOre)
}

/// Attunement a deed grants an item of `element`. Matching deeds attune faster:
/// an earth tool drinks in stone you break; a fire staff favours ore; any
/// weapon attunes by slaying.
pub fn attunement_for(element: Element, deed: Deed) -> u32 {
    match deed {
        Deed::MobSlain => 25, // combat attunes any item
        Deed::Travelled(blocks) => blocks / 20, // slow trickle from exploring
        Deed::BlockMined(b) => {
            let base = 8;
            let bonus = match element {
                Element::Earth | Element::Dust if is_earthy(b) => 22,
                Element::Fire if is_ore(b) => 16,
                Element::Water if matches!(b, BlockId::Ice | BlockId::Snow | BlockId::Water) => 18,
                _ => 0,
            };
            base + bonus
        }
    }
}

/// Highest level within a tier; reaching it unlocks evolution.
pub const MAX_LEVEL: u8 = 5;

/// Attunement needed to go from `level` to `level + 1` (rises with level).
pub fn attune_threshold(level: u8) -> u32 {
    100 * (level as u32 + 1)
}

/// An item instance with its element and progression state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementalItem {
    pub element: Element,
    pub form: ItemForm,
    pub tier: u8,
    pub level: u8,
    pub charge: u16,
    pub attunement: u32,
}

impl ElementalItem {
    /// A fresh tier-1 staff, fully charged.
    pub fn new(element: Element) -> ElementalItem {
        ElementalItem::new_form(element, ItemForm::Staff)
    }

    /// A fresh tier-1 item of a given form, fully charged. e.g. a lightning
    /// greataxe is `new_form(Element::Lightning, ItemForm::Axe)`.
    pub fn new_form(element: Element, form: ItemForm) -> ElementalItem {
        let mut it = ElementalItem { element, form, tier: 1, level: 0, charge: 0, attunement: 0 };
        it.charge = it.max_charge();
        it
    }

    /// Charge ceiling, rising with level and tier.
    pub fn max_charge(&self) -> u16 {
        100 + self.level as u16 * 20 + self.tier.saturating_sub(1) as u16 * 120
    }

    /// Record a deed; accrues attunement and levels up (recharging and raising
    /// the ceiling) when thresholds are crossed. Returns true if it levelled.
    pub fn record_deed(&mut self, deed: Deed) -> bool {
        self.attunement += attunement_for(self.element, deed);
        let mut levelled = false;
        while self.level < MAX_LEVEL && self.attunement >= attune_threshold(self.level) {
            self.attunement -= attune_threshold(self.level);
            self.level += 1;
            levelled = true;
        }
        if levelled {
            self.charge = self.max_charge(); // a level-up tops it off
        }
        levelled
    }

    /// Spend charge for a cast; returns false (and spends nothing) if short.
    pub fn use_charge(&mut self, cost: u16) -> bool {
        if self.charge >= cost {
            self.charge -= cost;
            true
        } else {
            false
        }
    }

    /// Try to cast/swing with this item's element + form (scaled by level).
    /// Returns the resolved profile on success, `None` if undercharged.
    pub fn cast(&mut self) -> Option<CastProfile> {
        let mut p = form_adjust(self.form, base_profile(self.element));
        // Level sharpens potency and trims cooldown.
        p.potency += p.potency * self.level as u16 / 10;
        p.cooldown = p.cooldown.saturating_sub(self.level);
        if self.use_charge(p.charge_cost) {
            Some(p)
        } else {
            None
        }
    }

    /// Refill charge (e.g. resting at a shrine, or passive regen), capped.
    pub fn recharge(&mut self, amount: u16) {
        self.charge = (self.charge + amount).min(self.max_charge());
    }

    /// Maxed within its tier → ready to be crafted into its next form.
    pub fn can_evolve(&self) -> bool {
        self.level >= MAX_LEVEL
    }

    /// Advance to the next tier (the recipe result): tier up, level reset, fresh
    /// attunement, topped-off (bigger) charge pool. No-op if not yet evolvable.
    pub fn evolve(&mut self) -> bool {
        if !self.can_evolve() {
            return false;
        }
        self.tier += 1;
        self.level = 0;
        self.attunement = 0;
        self.charge = self.max_charge();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;

    #[test]
    fn elements_cast_differently() {
        let earth = base_profile(Element::Earth);
        let light = base_profile(Element::Lightning);
        let dust = base_profile(Element::Dust);
        assert!(light.range > earth.range, "lightning reaches far, earth is close");
        assert!(dust.charge_cost < earth.charge_cost, "dust is cheap, earth is heavy");
        assert!(dust.cooldown < light.cooldown, "dust fires fast, lightning is slow");
    }

    #[test]
    fn matching_deeds_attune_faster() {
        // Mining stone feeds an earth tool more than an air staff.
        let earth = attunement_for(Element::Earth, Deed::BlockMined(BlockId::Stone));
        let air = attunement_for(Element::Air, Deed::BlockMined(BlockId::Stone));
        assert!(earth > air, "earth drinks in mined stone");
        // Ore favours fire; ice/snow favour water.
        assert!(attunement_for(Element::Fire, Deed::BlockMined(BlockId::IronOre)) > air);
        assert!(attunement_for(Element::Water, Deed::BlockMined(BlockId::Ice)) > air);
        // Slaying attunes any item.
        assert_eq!(attunement_for(Element::Air, Deed::MobSlain), 25);
    }

    #[test]
    fn deeds_level_the_item_and_recharge_it() {
        let mut it = ElementalItem::new(Element::Earth);
        let start_max = it.max_charge();
        it.use_charge(50); // spend some
        assert!(it.charge < start_max);
        // Mine a lot of stone (30 attunement each) to cross the level-1 threshold (100).
        let mut levelled = false;
        for _ in 0..4 {
            levelled |= it.record_deed(Deed::BlockMined(BlockId::Stone));
        }
        assert!(levelled, "enough mining levels it up");
        assert_eq!(it.level, 1);
        assert!(it.max_charge() > start_max, "ceiling rose with level");
        assert_eq!(it.charge, it.max_charge(), "level-up topped it off");
    }

    #[test]
    fn casting_spends_charge_and_fails_when_empty() {
        let mut it = ElementalItem::new(Element::Lightning);
        it.charge = base_profile(Element::Lightning).charge_cost; // exactly one cast
        assert!(it.cast().is_some(), "one cast lands");
        assert!(it.cast().is_none(), "second cast is undercharged");
        it.recharge(10_000);
        assert_eq!(it.charge, it.max_charge(), "recharge caps at the ceiling");
    }

    #[test]
    fn lightning_axe_is_a_melee_variant_of_the_staff() {
        // Same element, different form: the staff strikes far, the greataxe
        // cleaves up close for a heavier hit.
        let mut staff = ElementalItem::new(Element::Lightning);
        let mut axe = ElementalItem::new_form(Element::Lightning, ItemForm::Axe);
        let sp = staff.cast().expect("staff has charge");
        let ap = axe.cast().expect("axe has charge");
        assert!(sp.range > ap.range, "staff reaches far ({}), axe is melee ({})", sp.range, ap.range);
        assert!(ap.potency > sp.potency, "the greataxe hits harder per swing");
        assert_eq!(ap.range, 2, "axe is melee reach");
        // Both still channel Lightning (the element drives conduction).
        assert_eq!(axe.element, Element::Lightning);
        // The axe's swing channels a little less charge than a full staff cast.
        assert!(form_adjust(ItemForm::Axe, base_profile(Element::Lightning)).charge_cost
            < base_profile(Element::Lightning).charge_cost);
    }

    #[test]
    fn evolves_only_when_maxed() {
        let mut it = ElementalItem::new(Element::Fire);
        assert!(!it.can_evolve());
        assert!(!it.evolve(), "can't evolve early");
        it.level = MAX_LEVEL;
        let pre = it.max_charge();
        assert!(it.evolve(), "maxed → evolves");
        assert_eq!(it.tier, 2);
        assert_eq!(it.level, 0);
        assert!(it.max_charge() > pre, "next tier has a bigger pool");
        assert_eq!(it.charge, it.max_charge());
    }
}
