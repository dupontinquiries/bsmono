//! Elemental shields — defenses that answer an incoming element differently
//! depending on their own. The matchups are the satisfying part: water quenches
//! fire but conducts lightning straight into you; earth grounds a bolt and
//! shrugs off force; a wood shield drinks water and *regrows*, but a fire attack
//! sets it ablaze. Block well against what you counter, suffer against what
//! counters you.
//!
//! Builds on `elemental::Element`. Pure, integer, `Copy` — rolls back. Pairs
//! with `sim::lightning` (a water shield being a terrible idea against
//! lightning is the same physics as the bolt conducting through water).

use crate::sim::elemental::Element;

/// A shield that absorbs hits until its integrity is spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shield {
    pub element: Element,
    pub integrity: u16,
    pub max_integrity: u16,
}

impl Shield {
    pub fn new(element: Element, max_integrity: u16) -> Shield {
        Shield { element, integrity: max_integrity, max_integrity }
    }
    pub fn is_broken(&self) -> bool {
        self.integrity == 0
    }
}

/// The visible side-effect of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    None,
    /// Fire doused on a water shield.
    Quench,
    /// The shield itself caught fire (wood vs fire).
    Burn,
    /// A bolt earthed harmlessly (earth vs lightning/air).
    Ground,
    /// Charge conducted through (water vs lightning) — it shocks the bearer.
    Conduct,
    /// Wood shield drank the attack and grew back (wood vs water).
    Regrow,
}

/// The outcome of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defense {
    /// Damage the shield absorbed.
    pub mitigated: u16,
    /// Damage that still reaches the bearer.
    pub passthrough: u16,
    pub reaction: Reaction,
}

/// (percent of the hit blocked, the reaction, integrity cost as a percent of the
/// hit's magnitude). A negative-ish design note: `wear` is how hard the block is
/// on the shield — counters wear it fast; favourable matchups barely scratch it.
fn matchup(shield: Element, incoming: Element) -> (u16, Reaction, u16) {
    use Element::*;
    match (shield, incoming) {
        // Water: master of fire, victim of lightning.
        (Water, Fire) => (90, Reaction::Quench, 10),
        (Water, Lightning) => (10, Reaction::Conduct, 120), // conducts into the bearer
        // Fire: burns wood, doused by water.
        (Fire, Wood) => (85, Reaction::Burn, 15),
        (Fire, Water) => (30, Reaction::Quench, 90),
        // Wood: drinks water (regrows), burns to fire.
        (Wood, Water) => (80, Reaction::Regrow, 0),
        (Wood, Fire) => (20, Reaction::Burn, 140),
        // Earth: grounds lightning/air, solid vs fire.
        (Earth, Lightning) | (Earth, Air) => (88, Reaction::Ground, 8),
        (Earth, Fire) => (70, Reaction::None, 25),
        (Earth, Dust) => (75, Reaction::None, 15),
        // Like vs like: a partial, low-wear block.
        (a, b) if a == b => (60, Reaction::None, 12),
        // Everything else: an even, moderate block.
        _ => (50, Reaction::None, 40),
    }
}

/// Resolve an incoming elemental hit of `magnitude` against `shield`. Applies
/// integrity wear (or regrowth) and returns how much was blocked, how much got
/// through, and the reaction. A broken shield (integrity 0) blocks nothing.
pub fn defend(shield: &mut Shield, incoming: Element, magnitude: u16) -> Defense {
    if shield.is_broken() {
        return Defense { mitigated: 0, passthrough: magnitude, reaction: Reaction::None };
    }
    let (block_pct, reaction, wear_pct) = matchup(shield.element, incoming);
    let mitigated = magnitude * block_pct / 100;
    let passthrough = magnitude - mitigated;

    if reaction == Reaction::Regrow {
        // Drinking a favourable element mends the shield.
        let heal = magnitude / 4;
        shield.integrity = (shield.integrity + heal).min(shield.max_integrity);
    } else {
        let wear = (magnitude as u32 * wear_pct as u32 / 100) as u16;
        shield.integrity = shield.integrity.saturating_sub(wear);
    }
    Defense { mitigated, passthrough, reaction }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::elemental::Element;

    #[test]
    fn water_quenches_fire_but_conducts_lightning() {
        let mut w = Shield::new(Element::Water, 100);
        let fire = defend(&mut w, Element::Fire, 100);
        assert!(fire.mitigated >= 80, "water soaks up fire: {}", fire.mitigated);
        assert_eq!(fire.reaction, Reaction::Quench);

        let mut w2 = Shield::new(Element::Water, 100);
        let bolt = defend(&mut w2, Element::Lightning, 100);
        assert!(bolt.passthrough > 80, "water is a terrible lightning shield: {}", bolt.passthrough);
        assert_eq!(bolt.reaction, Reaction::Conduct);
        assert!(w2.integrity < w.integrity, "the bolt wrecks the water shield");
    }

    #[test]
    fn wood_drinks_water_and_burns_in_fire() {
        let mut wood = Shield::new(Element::Wood, 100);
        wood.integrity = 60;
        let water = defend(&mut wood, Element::Water, 80);
        assert_eq!(water.reaction, Reaction::Regrow);
        assert!(wood.integrity > 60, "wood regrows drinking water: {}", wood.integrity);

        let mut wood2 = Shield::new(Element::Wood, 100);
        let fire = defend(&mut wood2, Element::Fire, 100);
        assert_eq!(fire.reaction, Reaction::Burn);
        assert!(fire.passthrough > 70, "fire roars past a wood shield");
        assert!(wood2.integrity < 60, "and chars it badly: {}", wood2.integrity);
    }

    #[test]
    fn earth_grounds_lightning() {
        let mut e = Shield::new(Element::Earth, 100);
        let bolt = defend(&mut e, Element::Lightning, 100);
        assert!(bolt.mitigated >= 85, "earth grounds the bolt");
        assert_eq!(bolt.reaction, Reaction::Ground);
        assert!(e.integrity > 85, "and barely wears");
    }

    #[test]
    fn like_blocks_like_partially_and_broken_blocks_nothing() {
        let mut f = Shield::new(Element::Fire, 100);
        let d = defend(&mut f, Element::Fire, 100);
        assert_eq!(d.mitigated, 60, "same element: partial block");

        let mut dead = Shield::new(Element::Earth, 0);
        let d = defend(&mut dead, Element::Fire, 50);
        assert_eq!(d.mitigated, 0);
        assert_eq!(d.passthrough, 50, "a shattered shield stops nothing");
    }
}
