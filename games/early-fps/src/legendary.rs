//! legendary.rs — Destiny-style legendary weapons: a base archetype plus two stacking perks and a unique
//! appearance (name + accent colour). Unlike a plain weapon swap, a legendary rolls *traits* that modify
//! damage and handling, so "an M4 that reloads itself and hits harder after kills" is a distinct item.
//!
//! The perk effects here are distilled into two aggregate multipliers (damage, reload) plus a descriptive
//! blurb — enough to make legendaries feel meaningfully different and read clearly in the Arsenal, without
//! rebuilding the whole combat loop around per-bullet proc state.

use crate::cosmetics::Accent;
use crate::weapons::WeaponKind;

/// A legendary trait. Each contributes a passive damage and/or reload multiplier and has flavour text drawn
/// from the Destiny archetypes (Rampage, Kill Clip, Outlaw, …).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LegendaryPerk {
    Rampage,
    KillClip,
    Outlaw,
    RapidHit,
    ExplosivePayload,
    Incandescent,
    Voltshot,
    Reconstruction,
    Headstone,
    FieldPrep,
    FocusedFury,
    Frenzy,
}

impl LegendaryPerk {
    pub fn name(self) -> &'static str {
        match self {
            LegendaryPerk::Rampage => "Rampage",
            LegendaryPerk::KillClip => "Kill Clip",
            LegendaryPerk::Outlaw => "Outlaw",
            LegendaryPerk::RapidHit => "Rapid Hit",
            LegendaryPerk::ExplosivePayload => "Explosive Payload",
            LegendaryPerk::Incandescent => "Incandescent",
            LegendaryPerk::Voltshot => "Voltshot",
            LegendaryPerk::Reconstruction => "Reconstruction",
            LegendaryPerk::Headstone => "Headstone",
            LegendaryPerk::FieldPrep => "Field Prep",
            LegendaryPerk::FocusedFury => "Focused Fury",
            LegendaryPerk::Frenzy => "Frenzy",
        }
    }
    pub fn desc(self) -> &'static str {
        match self {
            LegendaryPerk::Rampage => "Kills stack a temporary damage boost.",
            LegendaryPerk::KillClip => "Reload after a kill for bonus damage.",
            LegendaryPerk::Outlaw => "Precision kills greatly speed reloads.",
            LegendaryPerk::RapidHit => "Precision hits improve reload and stability.",
            LegendaryPerk::ExplosivePayload => "Rounds detonate on impact for splash.",
            LegendaryPerk::Incandescent => "Kills spread scorching damage nearby.",
            LegendaryPerk::Voltshot => "Reload after a kill to chain lightning.",
            LegendaryPerk::Reconstruction => "Magazine refills itself over time.",
            LegendaryPerk::Headstone => "Precision kills leave shattering crystal.",
            LegendaryPerk::FieldPrep => "Faster reloads and deeper reserves.",
            LegendaryPerk::FocusedFury => "Hits charge a high-damage state.",
            LegendaryPerk::Frenzy => "In prolonged fights: more damage + handling.",
        }
    }
    /// Passive damage contribution (multiplier).
    fn damage_mul(self) -> f32 {
        match self {
            LegendaryPerk::Rampage => 1.15,
            LegendaryPerk::KillClip => 1.18,
            LegendaryPerk::ExplosivePayload => 1.12,
            LegendaryPerk::Incandescent => 1.10,
            LegendaryPerk::Voltshot => 1.12,
            LegendaryPerk::FocusedFury => 1.20,
            LegendaryPerk::Frenzy => 1.15,
            LegendaryPerk::Headstone => 1.08,
            _ => 1.0,
        }
    }
    /// Passive reload contribution (multiplier on reload *time*; <1 is faster).
    fn reload_mul(self) -> f32 {
        match self {
            LegendaryPerk::Outlaw => 0.7,
            LegendaryPerk::RapidHit => 0.82,
            LegendaryPerk::FieldPrep => 0.78,
            LegendaryPerk::Reconstruction => 0.85,
            LegendaryPerk::KillClip => 0.9,
            LegendaryPerk::Frenzy => 0.88,
            _ => 1.0,
        }
    }
}

/// A rolled legendary weapon: a named archetype with two perks and a signature colour.
#[derive(Clone, Copy, Debug)]
pub struct Legendary {
    pub name: &'static str,
    pub base: WeaponKind,
    pub perk1: LegendaryPerk,
    pub perk2: LegendaryPerk,
    pub accent: Accent,
}

impl Legendary {
    pub fn damage_mul(&self) -> f32 {
        self.perk1.damage_mul() * self.perk2.damage_mul()
    }
    pub fn reload_mul(&self) -> f32 {
        self.perk1.reload_mul() * self.perk2.reload_mul()
    }
}

/// The curated legendary pool — distinct looks and trait pairs across the archetypes.
pub const POOL: [Legendary; 12] = [
    Legendary { name: "Sunshot", base: WeaponKind::Ar, perk1: LegendaryPerk::Rampage, perk2: LegendaryPerk::ExplosivePayload, accent: Accent::Amber },
    Legendary { name: "Nightshade", base: WeaponKind::Ar, perk1: LegendaryPerk::KillClip, perk2: LegendaryPerk::Outlaw, accent: Accent::Crimson },
    Legendary { name: "Recluse", base: WeaponKind::Smg, perk1: LegendaryPerk::Frenzy, perk2: LegendaryPerk::RapidHit, accent: Accent::Magenta },
    Legendary { name: "Riptide", base: WeaponKind::Smg, perk1: LegendaryPerk::Voltshot, perk2: LegendaryPerk::FieldPrep, accent: Accent::Azure },
    Legendary { name: "Chaperone", base: WeaponKind::Shotgun, perk1: LegendaryPerk::FocusedFury, perk2: LegendaryPerk::RapidHit, accent: Accent::White },
    Legendary { name: "Matador", base: WeaponKind::Shotgun, perk1: LegendaryPerk::Rampage, perk2: LegendaryPerk::FieldPrep, accent: Accent::Crimson },
    Legendary { name: "Whisper", base: WeaponKind::Sniper, perk1: LegendaryPerk::Reconstruction, perk2: LegendaryPerk::FocusedFury, accent: Accent::Cyan },
    Legendary { name: "Longbow", base: WeaponKind::Sniper, perk1: LegendaryPerk::Outlaw, perk2: LegendaryPerk::Frenzy, accent: Accent::Green },
    Legendary { name: "Trinity", base: WeaponKind::Dmr, perk1: LegendaryPerk::Headstone, perk2: LegendaryPerk::KillClip, accent: Accent::Cyan },
    Legendary { name: "Vanguard", base: WeaponKind::Lmg, perk1: LegendaryPerk::Reconstruction, perk2: LegendaryPerk::Rampage, accent: Accent::Amber },
    Legendary { name: "Thunderlord", base: WeaponKind::Lmg, perk1: LegendaryPerk::Voltshot, perk2: LegendaryPerk::Frenzy, accent: Accent::Azure },
    Legendary { name: "Ace", base: WeaponKind::Pistol, perk1: LegendaryPerk::KillClip, perk2: LegendaryPerk::Outlaw, accent: Accent::White },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_mults_are_sane() {
        for l in POOL.iter() {
            assert!(l.damage_mul() >= 1.0 && l.damage_mul() < 2.0);
            assert!(l.reload_mul() > 0.3 && l.reload_mul() <= 1.0);
        }
    }
}
