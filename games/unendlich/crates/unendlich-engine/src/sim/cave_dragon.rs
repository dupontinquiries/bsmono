//! r691: cave dragons — which dragons inhabit which caves, derived from
//! `01_World_and_Environment/cave_biomes.docx`.
//!
//! `cave_biomes.docx` catalogues seven subterranean biome types by temperature,
//! O₂, RT60 reverb, survivability, and dominant environmental properties (H/R/S/…)
//! but says nothing about dragons directly. A **cave dragon** is a dragon whose
//! bending element matches a cave's dominant property, roosting deep instead of on
//! a crag: a Fire dragon in the volcanic Steam Caverns, an Earth dragon in the
//! pressure-crushed CO₂ Basins, a Water dragon in the flooded River Networks.
//!
//! This pure core encodes each biome's key figures + the element that thrives
//! there, plus a `habitability` score (element fit × survivability) the spawner
//! can use to place cave-appropriate dragons, and which composes with
//! `sim::dragon_roost` (a cave roost) and `sim::bending_field` (the cave's
//! ambient properties). Deterministic, std-only.

/// The seven cave biome types (doc table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaveBiome {
    /// 8–15 °C, damp, stone-cathedral acoustics. High survivability.
    LimestoneCaverns,
    /// 6–14 °C, flooded circulation, very humid. Moderate.
    RiverNetworks,
    /// 15–40 °C, selenite/quartz geodes, some luminosity. Moderate.
    CrystalChambers,
    /// 12–25 °C, vast vertical voids ("subterranean sky"). Mod-low.
    AbyssalChasms,
    /// 40–120 °C, volcanic steam, near-total humidity. Very low.
    SteamCaverns,
    /// 10–20 °C, dense CO₂, suffocating (P 2.5, W ~0). Extreme-low.
    Co2Basins,
    /// 30–80 °C, mantle-adjacent, hot + seismic. Low.
    GeothermalDeep,
}

/// Coarse survivability tier (doc "Survival" column). Higher = safer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Survivability {
    ExtremeLow = 0,
    VeryLow = 1,
    Low = 2,
    ModLow = 3,
    Moderate = 4,
    High = 5,
}

impl Survivability {
    /// A `[0,1]` factor — how survivable the biome is for an unprotected creature.
    pub fn factor(self) -> f32 {
        (self as u8 as f32) / (Survivability::High as u8 as f32)
    }
}

impl CaveBiome {
    pub const ALL: [CaveBiome; 7] = [
        CaveBiome::LimestoneCaverns,
        CaveBiome::RiverNetworks,
        CaveBiome::CrystalChambers,
        CaveBiome::AbyssalChasms,
        CaveBiome::SteamCaverns,
        CaveBiome::Co2Basins,
        CaveBiome::GeothermalDeep,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CaveBiome::LimestoneCaverns => "Limestone Caverns",
            CaveBiome::RiverNetworks => "River Networks",
            CaveBiome::CrystalChambers => "Crystal Chambers",
            CaveBiome::AbyssalChasms => "Abyssal Chasms",
            CaveBiome::SteamCaverns => "Steam Caverns",
            CaveBiome::Co2Basins => "CO₂ Basins",
            CaveBiome::GeothermalDeep => "Geothermal Deep",
        }
    }

    /// Temperature range in °C (doc).
    pub fn temp_c(self) -> (f32, f32) {
        match self {
            CaveBiome::LimestoneCaverns => (8.0, 15.0),
            CaveBiome::RiverNetworks => (6.0, 14.0),
            CaveBiome::CrystalChambers => (15.0, 40.0),
            CaveBiome::AbyssalChasms => (12.0, 25.0),
            CaveBiome::SteamCaverns => (40.0, 120.0),
            CaveBiome::Co2Basins => (10.0, 20.0),
            CaveBiome::GeothermalDeep => (30.0, 80.0),
        }
    }

    /// O₂ range in percent (doc). Below ~16 % is dangerous for air-breathers.
    pub fn o2_pct(self) -> (f32, f32) {
        match self {
            CaveBiome::LimestoneCaverns => (19.0, 21.0),
            CaveBiome::RiverNetworks => (18.0, 21.0),
            CaveBiome::CrystalChambers => (15.0, 21.0),
            CaveBiome::AbyssalChasms => (10.0, 20.0),
            CaveBiome::SteamCaverns => (5.0, 18.0),
            CaveBiome::Co2Basins => (0.0, 15.0),
            CaveBiome::GeothermalDeep => (5.0, 18.0),
        }
    }

    /// RT60 reverberation range in seconds (doc) — feeds cave acoustics.
    pub fn rt60_s(self) -> (f32, f32) {
        match self {
            CaveBiome::LimestoneCaverns => (2.0, 8.0),
            CaveBiome::RiverNetworks => (1.0, 3.0),
            CaveBiome::CrystalChambers => (4.0, 10.0),
            CaveBiome::AbyssalChasms => (8.0, 20.0),
            CaveBiome::SteamCaverns => (0.5, 2.0),
            CaveBiome::Co2Basins => (1.0, 4.0),
            CaveBiome::GeothermalDeep => (3.0, 15.0),
        }
    }

    pub fn survivability(self) -> Survivability {
        match self {
            CaveBiome::LimestoneCaverns => Survivability::High,
            CaveBiome::RiverNetworks => Survivability::Moderate,
            CaveBiome::CrystalChambers => Survivability::Moderate,
            CaveBiome::AbyssalChasms => Survivability::ModLow,
            CaveBiome::SteamCaverns => Survivability::VeryLow,
            CaveBiome::Co2Basins => Survivability::ExtremeLow,
            CaveBiome::GeothermalDeep => Survivability::Low,
        }
    }

    /// Lethal to an unprotected air-breather (its O₂ can drop into the danger
    /// zone, or its heat can climb to scalding).
    pub fn is_hostile(self) -> bool {
        self.o2_pct().0 < 16.0 || self.temp_c().1 > 45.0
    }

    /// r717: acoustic enemy-detection range (blocks) — how far hostile mobs hear
    /// the player, set by the biome's acoustics (doc acoustic table). Cathedral
    /// rings and monumental chasms carry sound far; water-masked rivers and muffled
    /// steam vents smother it.
    pub fn enemy_detect_range_blocks(self) -> f32 {
        match self {
            CaveBiome::LimestoneCaverns => 60.0,
            CaveBiome::RiverNetworks => 10.0,
            CaveBiome::CrystalChambers => 40.0,
            CaveBiome::AbyssalChasms => 100.0,
            CaveBiome::SteamCaverns => 8.0,
            CaveBiome::Co2Basins => 15.0,
            CaveBiome::GeothermalDeep => 30.0,
        }
    }

    /// The dragon bending element that thrives here (0=Earth 1=Water 2=Fire
    /// 3=Air 6=Information), from the biome's dominant property + character.
    pub fn dominant_element(self) -> u8 {
        match self {
            CaveBiome::LimestoneCaverns => 0, // Earth — the stone cathedral
            CaveBiome::RiverNetworks => 1,    // Water — flooded circulation, H 0.95
            CaveBiome::CrystalChambers => 6,  // Information — selenite/quartz + luminosity
            CaveBiome::AbyssalChasms => 3,    // Air — the "subterranean sky"
            CaveBiome::SteamCaverns => 2,     // Fire — volcanic lung, R 0.90
            CaveBiome::Co2Basins => 0,        // Earth — crushing pressure (P 2.5)
            CaveBiome::GeothermalDeep => 2,   // Fire — mantle antechamber, R 0.95
        }
    }
}

/// How well a dragon of `element_idx` inhabits `biome` in `[0,1]`: a strong bonus
/// when its element matches the cave's dominant property, scaled by how survivable
/// the cave is (a Fire dragon suits the Steam Caverns elementally, but the extreme
/// heat/low-O₂ still caps how comfortably anything lives there). An off-element
/// dragon gets a small baseline.
pub fn habitability(biome: CaveBiome, element_idx: u8) -> f32 {
    let fit = if element_idx == biome.dominant_element() { 1.0 } else { 0.35 };
    // Fire dragons shrug off heat; Earth dragons shrug off pressure — the matched
    // element mitigates the biome's hostility somewhat.
    let surv = biome.survivability().factor();
    let mitigated = surv + (1.0 - surv) * 0.5 * (fit - 0.35) / 0.65;
    (fit * mitigated).clamp(0.0, 1.0)
}

/// Pick the element most at home in a cave biome (its `dominant_element`) — the
/// spawner uses this to place a cave-appropriate dragon.
pub fn cave_dragon_element(biome: CaveBiome) -> u8 {
    biome.dominant_element()
}

/// Map a cave biome to the **dragon genome** element index (0=Wood, 1=Water,
/// 2=Air, 3=Dust, 4=Fire, 5=Storm — `dragon_archetype::ELEMENT_NAMES`), bridging
/// this module's 7-element BENDING taxonomy (0=Earth..6=Information) to the
/// 6-element GENOME taxonomy the forge/spawn use. Earth→Dust (stone), Brine→Water
/// (mineral water), Information→Air (ethereal); the rest map by name. This is the
/// element `spawn::spawn_dragon_element` wants for a cave dragon.
pub fn genome_element(biome: CaveBiome) -> u8 {
    match cave_dragon_element(biome) {
        0 => 3, // Earth       → Dust
        1 => 1, // Water       → Water
        2 => 4, // Fire        → Fire
        3 => 2, // Air         → Air
        4 => 5, // Storm       → Storm
        5 => 1, // Brine       → Water (mineral water)
        6 => 2, // Information → Air
        _ => 3,
    }
}

/// r717: classify the cave biome from the **hidden fields** at a point — the doc's
/// core insight that the geometry is a consequence of the fields, not the other
/// way around. This is what cave generation calls to pick a biome from the physical
/// drivers. `temp_c` is ambient temperature; `groundwater01`, `toxic_gas01`,
/// `geothermal01`, `openness01` are 0..1. Hazards and heat dominate, then flowing
/// water, then vast openness, else the default limestone cathedral.
pub fn classify(
    temp_c: f32,
    groundwater01: f32,
    toxic_gas01: f32,
    geothermal01: f32,
    openness01: f32,
) -> CaveBiome {
    if toxic_gas01 >= 0.6 && temp_c < 30.0 {
        return CaveBiome::Co2Basins;
    }
    if temp_c >= 60.0 {
        return if groundwater01 >= 0.3 {
            CaveBiome::SteamCaverns
        } else {
            CaveBiome::GeothermalDeep
        };
    }
    if temp_c >= 40.0 && geothermal01 >= 0.5 {
        return CaveBiome::GeothermalDeep;
    }
    if temp_c >= 15.0 && toxic_gas01 >= 0.05 && groundwater01 < 0.4 {
        return CaveBiome::CrystalChambers;
    }
    if groundwater01 >= 0.5 {
        return CaveBiome::RiverNetworks;
    }
    if openness01 >= 0.7 {
        return CaveBiome::AbyssalChasms;
    }
    CaveBiome::LimestoneCaverns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_routes_hidden_fields_to_biomes() {
        // Gas pooling in cool ground → CO₂ basin (the silent killer wins outright).
        assert_eq!(classify(15.0, 0.1, 0.8, 0.1, 0.2), CaveBiome::Co2Basins);
        // Scalding + wet → steam; scalding + dry → mantle.
        assert_eq!(classify(90.0, 0.6, 0.0, 0.9, 0.1), CaveBiome::SteamCaverns);
        assert_eq!(classify(90.0, 0.05, 0.0, 0.9, 0.1), CaveBiome::GeothermalDeep);
        // Warm, some gas, dry → crystal geode.
        assert_eq!(classify(25.0, 0.1, 0.10, 0.2, 0.2), CaveBiome::CrystalChambers);
        // Flowing water → river; vast void → abyss; else limestone cathedral.
        assert_eq!(classify(10.0, 0.8, 0.0, 0.0, 0.2), CaveBiome::RiverNetworks);
        assert_eq!(classify(18.0, 0.1, 0.0, 0.0, 0.9), CaveBiome::AbyssalChasms);
        assert_eq!(classify(12.0, 0.1, 0.0, 0.0, 0.2), CaveBiome::LimestoneCaverns);
    }

    #[test]
    fn detection_range_tracks_acoustics() {
        // Monumental chasms carry sound furthest; water-masked rivers and muffled
        // steam vents smother it.
        assert!(CaveBiome::AbyssalChasms.enemy_detect_range_blocks() > CaveBiome::LimestoneCaverns.enemy_detect_range_blocks());
        assert!(CaveBiome::SteamCaverns.enemy_detect_range_blocks() < CaveBiome::LimestoneCaverns.enemy_detect_range_blocks());
        assert!(CaveBiome::RiverNetworks.enemy_detect_range_blocks() <= 10.0);
    }

    #[test]
    fn dominant_elements_match_the_biome_character() {
        assert_eq!(CaveBiome::SteamCaverns.dominant_element(), 2); // Fire        assert_eq!(CaveBiome::GeothermalDeep.dominant_element(), 2); // Fire
        assert_eq!(CaveBiome::RiverNetworks.dominant_element(), 1); // Water
        assert_eq!(CaveBiome::Co2Basins.dominant_element(), 0); // Earth (pressure)
        assert_eq!(CaveBiome::AbyssalChasms.dominant_element(), 3); // Air
    }

    #[test]
    fn hostile_caves_are_flagged() {
        assert!(CaveBiome::Co2Basins.is_hostile()); // suffocating O₂
        assert!(CaveBiome::SteamCaverns.is_hostile()); // scalding
        assert!(CaveBiome::GeothermalDeep.is_hostile()); // 30–80 °C
        assert!(!CaveBiome::LimestoneCaverns.is_hostile());
        assert!(!CaveBiome::RiverNetworks.is_hostile());
    }

    #[test]
    fn matched_element_is_more_habitable_than_off_element() {
        for b in CaveBiome::ALL {
            let matched = habitability(b, b.dominant_element());
            let off = habitability(b, (b.dominant_element() + 1) % 7);
            assert!(matched >= off, "{}", b.name());
            assert!((0.0..=1.0).contains(&matched));
        }
    }

    #[test]
    fn survivability_orders_and_scales() {
        assert!(CaveBiome::LimestoneCaverns.survivability() > CaveBiome::Co2Basins.survivability());
        assert!(Survivability::High.factor() > Survivability::ExtremeLow.factor());
        assert_eq!(Survivability::ExtremeLow.factor(), 0.0);
        assert_eq!(Survivability::High.factor(), 1.0);
    }

    #[test]
    fn safe_cave_is_more_habitable_than_deadly_one_for_its_dragon() {
        let lime = habitability(CaveBiome::LimestoneCaverns, 0); // Earth in a safe cave
        let co2 = habitability(CaveBiome::Co2Basins, 0); // Earth in a deadly cave
        assert!(lime > co2);
    }

    #[test]
    fn every_biome_has_sane_ranges() {
        for b in CaveBiome::ALL {
            let (lo, hi) = b.temp_c();
            assert!(lo <= hi);
            let (o0, o1) = b.o2_pct();
            assert!(o0 <= o1 && o1 <= 21.0);
            let (r0, r1) = b.rt60_s();
            assert!(r0 <= r1 && r0 > 0.0);
        }
    }

    #[test]
    fn genome_element_bridges_to_the_six_element_dragon_taxonomy() {
        // every biome maps to a valid genome element index (0..=5)
        for b in CaveBiome::ALL {
            assert!(genome_element(b) <= 5, "{} out of genome range", b.name());
        }
        // a Fire (bending 2) cave → Fire (genome 4); Earth (bending 0) → Dust (3)
        assert_eq!(genome_element(CaveBiome::SteamCaverns), 4); // Fire→Fire
        assert_eq!(genome_element(CaveBiome::LimestoneCaverns), 3); // Earth→Dust
    }
}
