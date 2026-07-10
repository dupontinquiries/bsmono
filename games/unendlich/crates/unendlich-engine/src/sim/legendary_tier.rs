//! `sim::legendary_tier` — the tier framework for legendary items & dragon
//! artifacts, plus deterministic placement scoping.
//!
//! From `06_Crafting_and_Items/legendary_items_dragon_artifacts.docx`. Every
//! legendary item / dragon artifact sits in one of three tiers describing its
//! rarity, power, and how it's found. Tiers do NOT gate content behind progression
//! — a Tier III can appear at any point — but the conditions to encounter each
//! tier correlate naturally with experience.
//!
//! | Tier | Rarity    | Origin / density                         | Power signature                                   |
//! |------|-----------|------------------------------------------|---------------------------------------------------|
//! | I    | RARE      | World-generated, findable. 1 per region. | Single field amplification / minor extension. Does not change access. |
//! | II   | LEGENDARY | Bender-imbued or geological. 1 per biome type per world. | Discipline Anchor — extends a discipline into environments it normally can't reach. |
//! | III  | MYTHIC    | 1 per world, or 0 unless worldgen conditions met. | Combines properties; enables otherwise-impossible techniques; permanent. |

/// Rarity/power tier of a legendary item or dragon artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ItemTier {
    /// Tier I — RARE.
    Rare,
    /// Tier II — LEGENDARY.
    Legendary,
    /// Tier III — MYTHIC.
    Mythic,
}

/// How many of a tier's items exist and at what granularity they're placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnScope {
    /// One per world region.
    PerRegion,
    /// One per biome *type* per world.
    PerBiomeType,
    /// One per world (or zero, unless worldgen conditions were met).
    PerWorld,
}

/// Where a legendary item / artifact comes from (the doc's catalogue sections).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtifactSource {
    /// A discipline relic (Stormstrike, Heavy Brine, Thermal Phase, …).
    DisciplineRelic,
    /// Grown/shed by a dragon (Dragon Natural Artifacts).
    DragonNatural,
    /// Raised by a builder-lineage dragon (ties to `sim::builder_genome`).
    BuilderConstruct,
    /// A pre-existing site (Ancient Shrines).
    AncientShrine,
}

impl ItemTier {
    pub const fn roman(self) -> &'static str {
        match self {
            ItemTier::Rare => "I",
            ItemTier::Legendary => "II",
            ItemTier::Mythic => "III",
        }
    }
    pub const fn rarity(self) -> &'static str {
        match self {
            ItemTier::Rare => "RARE",
            ItemTier::Legendary => "LEGENDARY",
            ItemTier::Mythic => "MYTHIC",
        }
    }
    pub const fn scope(self) -> SpawnScope {
        match self {
            ItemTier::Rare => SpawnScope::PerRegion,
            ItemTier::Legendary => SpawnScope::PerBiomeType,
            ItemTier::Mythic => SpawnScope::PerWorld,
        }
    }
    /// Tier II+ act as Discipline Anchors — they extend a discipline into
    /// environments where it normally cannot be accessed.
    pub const fn is_discipline_anchor(self) -> bool {
        matches!(self, ItemTier::Legendary | ItemTier::Mythic)
    }
    /// Does the tier fundamentally change discipline *access*? Tier I only
    /// amplifies/extends within reach; II and III open new environments.
    pub const fn changes_access(self) -> bool {
        self.is_discipline_anchor()
    }
    /// Only Tier III enables otherwise-impossible techniques.
    pub const fn enables_impossible(self) -> bool {
        matches!(self, ItemTier::Mythic)
    }
    pub const fn power_signature(self) -> &'static str {
        match self {
            ItemTier::Rare => "single field amplification / minor technique extension",
            ItemTier::Legendary => "discipline anchor + gesture extension",
            ItemTier::Mythic => "multi-property; enables otherwise-impossible techniques",
        }
    }
}

// SplitMix64 — a tiny deterministic mixer so placement is reproducible from seeds.
#[inline]
const fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[inline]
fn roll01(seed: u64, salt: u64) -> f32 {
    (mix64(seed ^ mix64(salt)) >> 40) as f32 / (1u64 << 24) as f32
}

/// Tier I: does this region contain its (single) rare artifact? Findable but not
/// in every region — `rare_density` is the fraction of regions that have one
/// (default ~0.35 keeps them a genuine discovery). Deterministic per region seed.
pub fn region_has_rare(region_seed: u64, rare_density: f32) -> bool {
    roll01(region_seed, 0xA1) < rare_density.clamp(0.0, 1.0)
}

/// Tier II: each biome *type* gets exactly one Legendary anchor per world, placed
/// in a deterministic region drawn from `(world_seed, biome_id)`. Returns the
/// chosen region index within `region_count` (so exactly one region per biome
/// type hosts the anchor).
pub fn biome_anchor_region(world_seed: u64, biome_id: u32, region_count: u32) -> u32 {
    if region_count == 0 {
        return 0;
    }
    (mix64(world_seed ^ mix64(biome_id as u64 ^ 0xB2)) % region_count as u64) as u32
}

/// Tier III: does this world contain its single Mythic artifact? Present only when
/// the worldgen conditions were met (`conditions_met`) AND the rare mythic roll
/// hits — otherwise 0 per world. Deterministic per world seed.
pub fn world_has_mythic(world_seed: u64, conditions_met: bool, mythic_chance: f32) -> bool {
    conditions_met && roll01(world_seed, 0xC3) < mythic_chance.clamp(0.0, 1.0)
}

/// Convenience: classify a raw rarity roll in `[0,1)` into a tier, given the tier
/// mix (fraction that are Mythic, then Legendary, else Rare). Used when an artifact
/// slot is known to spawn and only its tier needs choosing.
pub fn tier_from_roll(roll: f32, mythic_frac: f32, legendary_frac: f32) -> ItemTier {
    let r = roll.clamp(0.0, 1.0);
    if r < mythic_frac {
        ItemTier::Mythic
    } else if r < mythic_frac + legendary_frac {
        ItemTier::Legendary
    } else {
        ItemTier::Rare
    }
}

/// r712: the artifact a builder-lineage dragon raises, keyed by its builder rank
/// (matching `sim::builder_genome::BuilderExpression` order:
/// 0 NonBuilder, 1 Shaper, 2 Neatener, 3 Builder, 4 Architect). Non-builders and
/// neateners raise nothing; the constructive phenotypes raise Builder Constructs
/// of increasing tier. Ties the r694 builder classification to this catalogue's
/// "Builder Constructs" category. Taken by rank so this core stays standalone.
impl ArtifactSource {
    /// r719: the lore-attribution source (`sim::lore_attribution`) an artifact of
    /// this origin uses in its tooltip. Discipline relics quote dojo inscriptions;
    /// dragon-natural artifacts read as field observations; builder constructs as
    /// recovered fragments; ancient shrines as shrine text.
    pub const fn lore_attribution(self) -> crate::sim::lore_attribution::LoreAttribution {
        use crate::sim::lore_attribution::LoreAttribution as L;
        match self {
            ArtifactSource::DisciplineRelic => L::DojoInscription,
            ArtifactSource::DragonNatural => L::FieldObservation,
            ArtifactSource::BuilderConstruct => L::RecoveredFragment,
            ArtifactSource::AncientShrine => L::AncientShrineText,
        }
    }
}

pub fn builder_construct_tier(builder_rank: u8) -> Option<(ArtifactSource, ItemTier)> {
    match builder_rank {
        1 => Some((ArtifactSource::BuilderConstruct, ItemTier::Rare)), // Shaper
        3 => Some((ArtifactSource::BuilderConstruct, ItemTier::Legendary)), // Builder
        4 => Some((ArtifactSource::BuilderConstruct, ItemTier::Mythic)), // Architect
        _ => None, // NonBuilder / Neatener raise no construct
    }
}

#[cfg(test)]
mod builder_bridge_tests {
    use super::*;
    #[test]
    fn builder_ranks_map_to_construct_tiers() {
        assert_eq!(builder_construct_tier(0), None); // NonBuilder
        assert_eq!(builder_construct_tier(2), None); // Neatener
        assert_eq!(
            builder_construct_tier(1),
            Some((ArtifactSource::BuilderConstruct, ItemTier::Rare))
        );
        assert_eq!(
            builder_construct_tier(4),
            Some((ArtifactSource::BuilderConstruct, ItemTier::Mythic))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_source_maps_to_lore() {
        use crate::sim::lore_attribution::LoreAttribution as L;
        assert_eq!(ArtifactSource::AncientShrine.lore_attribution(), L::AncientShrineText);
        assert_eq!(ArtifactSource::DisciplineRelic.lore_attribution(), L::DojoInscription);
        assert_eq!(ArtifactSource::DragonNatural.lore_attribution(), L::FieldObservation);
        assert_eq!(ArtifactSource::BuilderConstruct.lore_attribution(), L::RecoveredFragment);
    }

    #[test]
    fn tier_properties() {
        assert_eq!(ItemTier::Rare.roman(), "I");
        assert_eq!(ItemTier::Mythic.roman(), "III");
        assert_eq!(ItemTier::Rare.scope(), SpawnScope::PerRegion);
        assert_eq!(ItemTier::Legendary.scope(), SpawnScope::PerBiomeType);
        assert_eq!(ItemTier::Mythic.scope(), SpawnScope::PerWorld);
        // Discipline anchor / access change: II and III only.
        assert!(!ItemTier::Rare.is_discipline_anchor());
        assert!(ItemTier::Legendary.is_discipline_anchor() && ItemTier::Mythic.is_discipline_anchor());
        assert!(!ItemTier::Rare.changes_access() && ItemTier::Legendary.changes_access());
        // Only Mythic enables impossible techniques.
        assert!(ItemTier::Mythic.enables_impossible());
        assert!(!ItemTier::Legendary.enables_impossible());
        // Ordering: Rare < Legendary < Mythic.
        assert!(ItemTier::Rare < ItemTier::Legendary && ItemTier::Legendary < ItemTier::Mythic);
    }

    #[test]
    fn region_rare_is_deterministic_and_scaled() {
        // Deterministic.
        assert_eq!(region_has_rare(42, 0.35), region_has_rare(42, 0.35));
        // Density 0 → never; density 1 → always.
        assert!(!region_has_rare(42, 0.0));
        assert!(region_has_rare(42, 1.0));
        // Roughly `density` fraction of regions have one.
        let n = 20_000u64;
        let hits = (0..n).filter(|&s| region_has_rare(s.wrapping_mul(0x9E37), 0.35)).count();
        let frac = hits as f32 / n as f32;
        assert!((frac - 0.35).abs() < 0.03, "rare fraction {frac} off target");
    }

    #[test]
    fn biome_anchor_is_one_region_per_biome_type() {
        // Deterministic and in-range.
        let r = biome_anchor_region(1234, 5, 64);
        assert!(r < 64);
        assert_eq!(r, biome_anchor_region(1234, 5, 64));
        // Different biome types generally pick different regions.
        let a = biome_anchor_region(1234, 1, 64);
        let b = biome_anchor_region(1234, 2, 64);
        let c = biome_anchor_region(1234, 3, 64);
        assert!(!(a == b && b == c), "anchors should spread across biome types");
    }

    #[test]
    fn mythic_needs_conditions() {
        // Without conditions → never, even with chance 1.0.
        assert!(!world_has_mythic(7, false, 1.0));
        // With conditions + chance 1 → present.
        assert!(world_has_mythic(7, true, 1.0));
        // Rare when chance is low.
        let n = 20_000u64;
        let hits = (0..n).filter(|&s| world_has_mythic(s.wrapping_mul(0x9E37), true, 0.05)).count();
        assert!((hits as f32 / n as f32 - 0.05).abs() < 0.02);
    }

    #[test]
    fn tier_from_roll_partitions() {
        // 5% mythic, 20% legendary, rest rare.
        assert_eq!(tier_from_roll(0.01, 0.05, 0.20), ItemTier::Mythic);
        assert_eq!(tier_from_roll(0.10, 0.05, 0.20), ItemTier::Legendary);
        assert_eq!(tier_from_roll(0.90, 0.05, 0.20), ItemTier::Rare);
    }
}
