//! Rock physicality for erosion — the material properties that decide how each
//! stone weathers, dissolves, and resists, and a model that turns them into an
//! erosion rate. This is the physical layer the `weathering` system can grow
//! into: instead of "soft vs hard" categories, every stone carries real
//! geology, so the world erodes the way the rock actually behaves.
//!
//! The properties (all per the existing natural-mineral palette — no new blocks):
//! - **hardness** — resistance to mechanical wear (Mohs-ish, 0..255). Quartz and
//!   granite shrug off abrasion; tuff and clay crumble.
//! - **solubility** — chemical dissolution by moving water (0..255). Carbonates
//!   (limestone, calcite, marble) dissolve — this is what carves karst caves and
//!   sinkholes; silicates barely budge.
//! - **density** — same units as `sim::medium` / buoyancy (≈ kg/m³).
//! - **bedding** — anisotropy (0 = massive, 255 = strongly bedded/foliated).
//!   Slate and schist split along cleavage; sandstone weathers along its beds.
//!
//! From these come two erosion behaviours geologists would recognise:
//! - **karst** — soluble rock under flowing water erodes fast (chemical term);
//! - **differential erosion** — a hard, insoluble *cap* shelters softer rock
//!   beneath it, the mechanism behind hoodoos, mesas, and cap-rock cliffs.
//!
//! Pure, integer, unit-tested. Returns `None` for non-rock (soils/wood/air keep
//! the existing soft-block weathering).

use crate::world::blocks::BlockId;

/// Physical properties of a rock for erosion purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RockPhysics {
    pub hardness: u8,
    pub solubility: u8,
    pub density: u16,
    pub bedding: u8,
}

const fn rock(hardness: u8, solubility: u8, density: u16, bedding: u8) -> RockPhysics {
    RockPhysics { hardness, solubility, density, bedding }
}

/// Erosion physics for a rock, or `None` if the block isn't rock (soils, wood,
/// fluids, decorative/crafted blocks are handled elsewhere). Ores take their
/// host stone's behaviour.
pub fn rock_physics(block: BlockId) -> Option<RockPhysics> {
    use BlockId::*;
    Some(match block {
        // Generic / clastic
        Stone => rock(150, 10, 2600, 0),
        Cobblestone => rock(120, 12, 2500, 0),
        Bedrock => rock(255, 0, 3000, 0), // immovable basement
        Clay => rock(30, 50, 1900, 120),  // soft, slightly soluble, bedded
        // Carbonates — soluble, the karst-formers
        Limestone => rock(90, 200, 2700, 60),
        Calcite => rock(80, 220, 2710, 20),
        Marble => rock(110, 170, 2700, 30), // recrystallised: harder than its parent
        // Clastic sedimentary — bedded, weather along the beds
        Sandstone => rock(100, 30, 2300, 180),
        RedSandstone => rock(100, 30, 2300, 180),
        // Foliated metamorphic — strong cleavage, split in sheets
        Slate => rock(130, 15, 2750, 230),
        Schist => rock(120, 15, 2700, 240),
        // Volcanic / igneous
        Tuff => rock(70, 40, 1800, 90),   // soft ash — classic hoodoo body
        Andesite => rock(160, 12, 2600, 10),
        Basalt => rock(185, 8, 3000, 30), // dense, hard — a good cap rock
        Granite => rock(200, 6, 2700, 5), // massive, tough
        // Very hard / glassy / insoluble
        Obsidian => rock(210, 5, 2400, 0),
        Quartz => rock(240, 2, 2650, 0),
        // Ore-bearing rock erodes like its (stony) host.
        GoldOre | IronOre | CoalOre | CopperOre | DiamondOre => rock(150, 10, 2700, 0),
        _ => return None,
    })
}

/// Per-tick erosion rate for an exposed rock, combining mechanical wear (soft +
/// exposed crumbles) with chemical dissolution (soluble + wet = karst), nudged
/// up by bedding (planes are weaknesses). `water_flow` and `exposure` are
/// `0..=255`. Higher = erodes faster. Non-rock → 0.
pub fn erosion_rate(block: BlockId, water_flow: u8, exposure: u8) -> u32 {
    let Some(r) = rock_physics(block) else { return 0 };
    let softness = (255 - r.hardness) as u32;
    let mechanical = softness * exposure as u32; // exposed, soft rock wears
    let chemical = r.solubility as u32 * water_flow as u32; // soluble rock dissolves in flow
    let base = mechanical + chemical;
    // Bedding/foliation opens preferential weaknesses: up to ~+25%.
    base + (base * r.bedding as u32) / 1024
}

/// Does `cap` shelter `below` from erosion — a harder, no-more-soluble rock
/// sitting on top? This is differential erosion: the cap survives while the
/// soft rock beneath retreats (hoodoos, mesas, cap-rock cliffs).
pub fn caps(cap: BlockId, below: BlockId) -> bool {
    match (rock_physics(cap), rock_physics(below)) {
        (Some(c), Some(b)) => c.hardness > b.hardness && c.solubility <= b.solubility,
        _ => false,
    }
}

/// Erosion rate accounting for an overhead cap: a protected column erodes far
/// slower than bare rock, so the cap and the spire under it persist.
pub fn capped_erosion_rate(block: BlockId, water_flow: u8, exposure: u8, capped: bool) -> u32 {
    let r = erosion_rate(block, water_flow, exposure);
    if capped {
        r / 4
    } else {
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId::*;

    #[test]
    fn only_rock_has_physics() {
        assert!(rock_physics(Granite).is_some());
        assert!(rock_physics(Limestone).is_some());
        assert!(rock_physics(IronOre).is_some(), "ore erodes like its host");
        assert!(rock_physics(Dirt).is_none());
        assert!(rock_physics(Air).is_none());
        assert!(rock_physics(OakLog).is_none());
    }

    #[test]
    fn hardness_and_solubility_match_geology() {
        let h = |b| rock_physics(b).unwrap().hardness;
        let s = |b| rock_physics(b).unwrap().solubility;
        // Hardness ordering.
        assert!(h(Quartz) > h(Granite));
        assert!(h(Granite) > h(Stone));
        assert!(h(Stone) > h(Limestone));
        assert!(h(Limestone) > h(Tuff));
        // Carbonates are soluble; silicates aren't.
        assert!(s(Limestone) > s(Granite) * 10, "limestone dissolves, granite doesn't");
        assert!(s(Calcite) >= s(Limestone));
        assert!(s(Quartz) < 10);
        // Foliated rock is strongly bedded; granite is massive.
        assert!(rock_physics(Slate).unwrap().bedding > rock_physics(Granite).unwrap().bedding);
    }

    #[test]
    fn karst_limestone_erodes_in_flowing_water() {
        // In moving water, soluble limestone erodes far faster than granite.
        let lime = erosion_rate(Limestone, 200, 40);
        let gran = erosion_rate(Granite, 200, 40);
        assert!(lime > gran * 3, "karst: limestone {lime} >> granite {gran} under flow");
        // With no water, the solubility advantage of attacking limestone vanishes;
        // both erode only mechanically (and granite is harder, so erodes least).
        assert!(erosion_rate(Limestone, 0, 40) > erosion_rate(Granite, 0, 40));
    }

    #[test]
    fn soft_exposed_rock_wears_faster() {
        assert!(erosion_rate(Tuff, 0, 200) > erosion_rate(Basalt, 0, 200), "exposed tuff crumbles, basalt holds");
    }

    #[test]
    fn differential_erosion_cap_protects_soft_rock() {
        // A basalt cap shelters tuff (hoodoo); tuff can't cap basalt; a soluble
        // limestone makes a poor cap even over softer rock.
        assert!(caps(Basalt, Tuff), "hard insoluble basalt caps soft tuff");
        assert!(!caps(Tuff, Basalt), "soft tuff can't shelter hard basalt");
        assert!(caps(Quartz, Sandstone), "quartz caps sandstone");
        assert!(!caps(Limestone, Tuff), "soluble limestone is a poor cap");
        // The protection shows up in the rate.
        let bare = capped_erosion_rate(Tuff, 50, 200, false);
        let sheltered = capped_erosion_rate(Tuff, 50, 200, true);
        assert!(sheltered < bare, "a capped spire erodes slower than bare tuff");
    }
}
