//! `sim::forge_types` — the nine forge/workstation types from
//! `docs/design_library/06_Crafting_and_Items/crafting_infusion_genetics.docx`
//! §5. Each forge has a characteristic element, a passive sway bonus, genome
//! bonuses (resonance / purity / infusion-slots / mutation-rate), and a unique
//! capability. This catalog turns a `ForgeType` into the `ForgeProfile` the
//! genome system (`sim::item_genome`) consumes at craft/combine time, and
//! exposes the extra per-forge facts (resonance bonus, whether it supports
//! genome combination, whether it grants solo compound infusions).
//!
//! Pure, std-only. Note: the doc's Information / Brine forge elements have no
//! `craft_sway::SwayElement` variant (that enum is Storm/Fire/Water/Earth/Air/
//! Wood), so `element()` returns the best representable characteristic and
//! `None` where the primary element is Information/Brine/dynamic — flagged for a
//! follow-up if `SwayElement` gains those variants.

use crate::sim::craft_sway::SwayElement;
use crate::sim::item_genome::ForgeProfile;

/// The nine forges (doc §5.1), plus a stable id for `craft_signature`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgeType {
    Standard,
    Seismic,
    Tidal,
    Volcanic,
    StormTower,
    AncientShrine,
    RyujinRelay,
    ResonanceFont,
    DragonRoost,
}

impl ForgeType {
    /// Stable forge id, stamped into a genome's `craft_signature`.
    pub fn id(self) -> u16 {
        match self {
            ForgeType::Standard => 1,
            ForgeType::Seismic => 2,
            ForgeType::Tidal => 3,
            ForgeType::Volcanic => 4,
            ForgeType::StormTower => 5,
            ForgeType::AncientShrine => 6,
            ForgeType::RyujinRelay => 7,
            ForgeType::ResonanceFont => 8,
            ForgeType::DragonRoost => 9,
        }
    }

    /// The characteristic sway element, where representable in `SwayElement`.
    /// `None` for Info/Brine-primary or dynamic (Dragon Roost) forges.
    pub fn element(self) -> Option<SwayElement> {
        match self {
            ForgeType::Standard => None,
            ForgeType::Seismic => Some(SwayElement::Earth),
            ForgeType::Tidal => Some(SwayElement::Water),
            ForgeType::Volcanic => Some(SwayElement::Fire),
            ForgeType::StormTower => Some(SwayElement::Storm),
            ForgeType::AncientShrine => None, // all elements simultaneously
            ForgeType::RyujinRelay => Some(SwayElement::Storm), // Storm + Information
            ForgeType::ResonanceFont => None, // Information + Brine
            ForgeType::DragonRoost => None,   // matches most recent dragon
        }
    }

    /// `material_purity` offset the forge adds (doc §5.1).
    pub fn purity_bonus(self) -> f32 {
        match self {
            ForgeType::Volcanic => 0.06,     // fire purifies
            ForgeType::AncientShrine => 0.05,
            _ => 0.0,
        }
    }

    /// Extra `infusion_capacity` slots the forge grants.
    pub fn infusion_bonus(self) -> u8 {
        match self {
            ForgeType::Tidal => 1, // water makes metal receptive
            _ => 0,
        }
    }

    /// `resonance_depth` bonus the forge embeds (Seismic +0.10 for Earth items,
    /// Resonance Font +0.12 for all — the highest).
    pub fn resonance_bonus(self) -> f32 {
        match self {
            ForgeType::Seismic => 0.10,
            ForgeType::ResonanceFont => 0.12,
            _ => 0.0,
        }
    }

    /// Combination-mutation rate (doc §2.2): standard 8%, Seismic very stable 4%,
    /// Ancient Shrine 15%, Dragon Roost 20%.
    pub fn mutation_rate(self) -> f32 {
        match self {
            ForgeType::Seismic => 0.04,
            ForgeType::AncientShrine => 0.15,
            ForgeType::DragonRoost => 0.20,
            _ => 0.08,
        }
    }

    /// Does this forge support genome combination (recombining two items)? The
    /// doc notes "not all forges support combination"; the Standard Forge and the
    /// rare high-tier forges do.
    pub fn supports_combination(self) -> bool {
        matches!(
            self,
            ForgeType::Standard | ForgeType::AncientShrine | ForgeType::DragonRoost | ForgeType::RyujinRelay
        )
    }

    /// Can a solo bender perform COMPOUND infusions here? Only the Ancient Shrine.
    pub fn compound_infusions_solo(self) -> bool {
        matches!(self, ForgeType::AncientShrine)
    }

    /// Can Ancient-tier recipes be unlocked here?
    pub fn unlocks_ancient_recipes(self) -> bool {
        matches!(self, ForgeType::AncientShrine)
    }

    /// The `ForgeProfile` the genome system consumes (`create`/`combine`).
    pub fn profile(self) -> ForgeProfile {
        ForgeProfile {
            id: self.id(),
            element: self.element(),
            purity_bonus: self.purity_bonus(),
            infusion_bonus: self.infusion_bonus(),
            mutation_rate: self.mutation_rate(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_stable() {
        let all = [
            ForgeType::Standard,
            ForgeType::Seismic,
            ForgeType::Tidal,
            ForgeType::Volcanic,
            ForgeType::StormTower,
            ForgeType::AncientShrine,
            ForgeType::RyujinRelay,
            ForgeType::ResonanceFont,
            ForgeType::DragonRoost,
        ];
        let mut ids: Vec<u16> = all.iter().map(|f| f.id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 9, "all nine ids distinct");
    }

    #[test]
    fn volcanic_purifies_tidal_adds_slot() {
        assert!((ForgeType::Volcanic.purity_bonus() - 0.06).abs() < 1e-6);
        assert_eq!(ForgeType::Tidal.infusion_bonus(), 1);
        assert_eq!(ForgeType::Standard.purity_bonus(), 0.0);
        assert_eq!(ForgeType::Standard.infusion_bonus(), 0);
    }

    #[test]
    fn resonance_font_has_highest_resonance_bonus() {
        assert!(ForgeType::ResonanceFont.resonance_bonus() > ForgeType::Seismic.resonance_bonus());
        assert_eq!(ForgeType::Standard.resonance_bonus(), 0.0);
    }

    #[test]
    fn mutation_rates_match_doc() {
        assert!((ForgeType::Seismic.mutation_rate() - 0.04).abs() < 1e-6);
        assert!((ForgeType::Standard.mutation_rate() - 0.08).abs() < 1e-6);
        assert!((ForgeType::AncientShrine.mutation_rate() - 0.15).abs() < 1e-6);
        assert!((ForgeType::DragonRoost.mutation_rate() - 0.20).abs() < 1e-6);
    }

    #[test]
    fn only_shrine_does_solo_compounds_and_ancient_recipes() {
        assert!(ForgeType::AncientShrine.compound_infusions_solo());
        assert!(ForgeType::AncientShrine.unlocks_ancient_recipes());
        assert!(!ForgeType::Volcanic.compound_infusions_solo());
    }

    #[test]
    fn profile_carries_the_forge_facts() {
        let p = ForgeType::Volcanic.profile();
        assert_eq!(p.id, 4);
        assert_eq!(p.element, Some(SwayElement::Fire));
        assert!((p.purity_bonus - 0.06).abs() < 1e-6);
        assert!((p.mutation_rate - 0.08).abs() < 1e-6);
    }
}
