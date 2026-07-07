//! Damage resolution — the one call an attack makes, chaining the defensive
//! systems into a single hit result. An incoming elemental hit passes through the
//! shield (element matchup decides how much is blocked and what reaction fires),
//! then armor (absorbs a share by condition, wearing as it blocks), then what's
//! left reaches health. The same hit also inflicts an elemental affliction —
//! unless the shield quenched it — and the affliction system's own reactions
//! (e.g. lightning conducting through an existing soak) feed their burst back
//! into the health damage. So a soaked target hit by lightning takes the bolt
//! *plus* the conduct burst, in one resolve.
//!
//! Deterministic — u16 magnitude space, with affliction intensity bridged through
//! fixed-point. Rollback-safe.

use crate::sim::affliction::{Afflicted, Reaction as Afflict};
use crate::sim::armor::Armor;
use crate::sim::elemental::Element;
use crate::sim::fixed::Fixed;
use crate::sim::shield::{self, Reaction as Block, Shield};

/// An incoming hit.
#[derive(Debug, Clone, Copy)]
pub struct Attack {
    pub element: Element,
    pub magnitude: u16,
    /// How long the inflicted affliction lasts, in ticks.
    pub affliction_ticks: i32,
}

/// The fully-resolved outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitResult {
    /// Final damage to the bearer's health (post-shield, post-armor, plus any
    /// affliction-reaction burst).
    pub health_damage: u16,
    /// The shield's reaction, or `None` if no shield was involved.
    pub shield_reaction: Option<Block>,
    /// The affliction system's reaction (conduct/quench/evaporate/none).
    pub affliction_reaction: Afflict,
    pub armor_absorbed: u16,
    /// Whether an affliction was actually applied.
    pub afflicted: bool,
}

fn fixed_to_u16(f: Fixed) -> u16 {
    f.floor_int().clamp(0, u16::MAX as i64) as u16
}

/// Resolve `attack` against the bearer's optional shield, optional armor, and
/// affliction set (all mutated in place as they absorb / wear / accrue).
pub fn resolve(
    attack: &Attack,
    shield: Option<&mut Shield>,
    armor: Option<&mut Armor>,
    afflictions: &mut Afflicted,
) -> HitResult {
    // 1. Shield.
    let (mut dmg, shield_reaction) = match shield {
        Some(s) => {
            let d = shield::defend(s, attack.element, attack.magnitude);
            (d.passthrough, Some(d.reaction))
        }
        None => (attack.magnitude, None),
    };

    // 2. Armor — absorbs a share scaled by condition (full → half), wears as it
    //    blocks.
    let mut armor_absorbed = 0u16;
    if let Some(a) = armor {
        if !a.is_broken() && a.max_durability > 0 {
            let absorbed = (dmg as u32 * a.durability as u32 / (a.max_durability as u32 * 2)) as u16;
            a.damage(absorbed);
            armor_absorbed = absorbed;
            dmg -= absorbed;
        }
    }

    // 3. Affliction — unless the shield doused it.
    let mut affliction_reaction = Afflict::None;
    let mut afflicted = false;
    if shield_reaction != Some(Block::Quench) && dmg > 0 {
        let intensity = Fixed::from_int(dmg as i64) / Fixed::from_int(4);
        let out = afflictions.apply(attack.element, intensity, attack.affliction_ticks);
        affliction_reaction = out.reaction;
        afflicted = true;
        dmg = dmg.saturating_add(fixed_to_u16(out.burst_damage)); // conduct burst, etc.
    }

    HitResult {
        health_damage: dmg,
        shield_reaction,
        affliction_reaction,
        armor_absorbed,
        afflicted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::armor::{Armor, ArmorKind};

    fn atk(element: Element, magnitude: u16) -> Attack {
        Attack { element, magnitude, affliction_ticks: 4 }
    }

    #[test]
    fn a_bare_hit_lands_full_damage_and_an_affliction() {
        let mut af = Afflicted::new();
        let r = resolve(&atk(Element::Fire, 12), None, None, &mut af);
        assert_eq!(r.health_damage, 12);
        assert!(r.afflicted);
        assert!(af.has(Element::Fire), "fire left burning");
        assert_eq!(r.shield_reaction, None);
    }

    #[test]
    fn a_water_shield_quenches_fire_and_suppresses_the_burn() {
        let mut shield = Shield::new(Element::Water, 100);
        let mut af = Afflicted::new();
        let r = resolve(&atk(Element::Fire, 100), Some(&mut shield), None, &mut af);
        // 90% blocked → 10 through; quench suppresses the affliction.
        assert_eq!(r.health_damage, 10);
        assert_eq!(r.shield_reaction, Some(Block::Quench));
        assert!(!r.afflicted);
        assert!(!af.has(Element::Fire), "doused, no burning");
    }

    #[test]
    fn lightning_into_a_soaked_target_adds_the_conduct_burst_to_the_hit() {
        let mut af = Afflicted::new();
        af.apply(Element::Water, Fixed::from_int(4), 5); // already soaked
        // Lightning mag 8, no shield → 8 through. intensity 8/4 = 2.
        // affliction conduct burst = 2*1.5 + soak(4) = 7. total = 8 + 7 = 15.
        let r = resolve(&atk(Element::Lightning, 8), None, None, &mut af);
        assert_eq!(r.affliction_reaction, Afflict::Conduct);
        assert_eq!(r.health_damage, 15, "bolt plus the conduct burst");
        assert!(!af.has(Element::Water), "soak consumed");
        assert!(af.has(Element::Lightning));
    }

    #[test]
    fn armor_absorbs_a_share_and_wears_down() {
        let mut armor = Armor::new(ArmorKind::Plain, 100);
        let mut af = Afflicted::new();
        // Earth 40, no shield. Full armor absorbs 40*100/(100*2)=20.
        let r = resolve(&atk(Element::Earth, 40), None, Some(&mut armor), &mut af);
        assert_eq!(r.armor_absorbed, 20);
        assert_eq!(r.health_damage, 20, "earth has no DoT burst");
        assert_eq!(armor.durability, 80, "armor wore by what it blocked");
    }

    #[test]
    fn a_broken_shield_blocks_nothing() {
        let mut shield = Shield::new(Element::Water, 100);
        shield.integrity = 0; // broken
        let mut af = Afflicted::new();
        let r = resolve(&atk(Element::Fire, 50), Some(&mut shield), None, &mut af);
        assert_eq!(r.health_damage, 50, "full hit through a dead shield");
        assert_eq!(r.shield_reaction, Some(Block::None));
        assert!(r.afflicted, "and it still burns");
        assert!(af.has(Element::Fire));
    }

    #[test]
    fn shield_then_armor_stack() {
        // Water shield vs lightning: only 10% blocked (90 through), then armor.
        let mut shield = Shield::new(Element::Water, 100);
        let mut armor = Armor::new(ArmorKind::Plain, 100);
        let mut af = Afflicted::new();
        let r = resolve(&atk(Element::Lightning, 100), Some(&mut shield), Some(&mut armor), &mut af);
        // 90 through the shield, armor absorbs 90*100/200 = 45 → 45 to health
        // (+ lightning DoT affliction, no prior soak so no burst).
        assert_eq!(r.shield_reaction, Some(Block::Conduct));
        assert_eq!(r.armor_absorbed, 45);
        assert_eq!(r.health_damage, 45);
        assert!(af.has(Element::Lightning));
    }
}
