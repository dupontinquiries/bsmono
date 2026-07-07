//! `sim::forage_harvest` — the gather→bag seam (r78-07). A small catalogue of
//! gatherable survival resources (berries, wood, fibre, water, mushrooms, herbs,
//! resin) with per-biome availability, a gather action that turns a resource
//! (and, for plant resources, a [`flora_harvest::PlantState`]) into item stacks,
//! and an atomic deposit into inventory regions.
//!
//! Composes — never forks — the existing cores:
//! - plant yields reuse `super::flora_harvest::harvest` (so an *immature* plant
//!   forages ~nothing, exactly as `flora_harvest` already encodes via
//!   `MATURITY`; we do not re-derive the maturity ramp here);
//! - deposit uses `crate::inventory::SlotGrid::add` (inventory.rs:97) — the
//!   conservation-preserving atomic add (`add` returns the leftover that did not
//!   fit), the same primitive `sim::inventory_txn` builds its moves on — so the
//!   item-count invariant `stored + overflow == produced` holds by construction.
//!
//! Item ids in [`GatherResult`] are `BlockId` discriminants (`u16`); the gather
//! palette maps each [`Forageable`] to the nearest existing block
//! (`world::blocks::BlockId`) as a documented placeholder until dedicated forage
//! items exist — the coordinator can repoint [`Forageable::item_id`] without
//! touching the gather/deposit logic. Deposit converts via `BlockId::try_from`
//! (blocks.rs:555); an id with no block is reported as overflow, never panics.
//!
//! Ref: flora modules (`flora_harvest`, `germination`, `canopy_field_shift`),
//! `physical_mob_engine.docx`. Pure & deterministic (the `roll` is the caller's
//! seeded input — R20); no IO/clock/RNG/`unwrap`; release-safe (all helpers
//! unconditional).

use crate::inventory::{ItemStack, SlotGrid};
use crate::world::blocks::BlockId;
use super::flora_harvest::{self, PlantState};
use core::convert::TryFrom;

/// A gatherable survival resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forageable {
    Berries,
    Mushroom,
    Wood,
    Fibre,
    Water,
    Herb,
    Resin,
}

/// Fraction of a full plant harvest that a single forage action takes.
pub const PLANT_FORAGE_FRACTION: f32 = 0.30;

impl Forageable {
    /// The item id deposited for this resource — a `BlockId` discriminant
    /// (placeholder mapping to the nearest existing palette block; repoint when
    /// dedicated forage items land).
    pub fn item_id(self) -> u16 {
        let b = match self {
            Forageable::Berries => BlockId::Flower,    // 25 — edible-plant placeholder
            Forageable::Mushroom => BlockId::Mushroom, // 33
            Forageable::Wood => BlockId::OakLog,       // 17
            Forageable::Fibre => BlockId::TallGrass,   // 26 — plant fibre
            Forageable::Water => BlockId::Water,       // 9
            Forageable::Herb => BlockId::Fern,         // 27
            Forageable::Resin => BlockId::OakLeaves,   // 18 — placeholder
        };
        b as u16
    }

    /// Plant resources whose yield is tied to `flora_harvest` (need a
    /// `PlantState`; immature → ~nothing).
    pub fn is_plant(self) -> bool {
        matches!(self, Forageable::Berries | Forageable::Mushroom | Forageable::Herb)
    }

    /// One-shot resources: taking them consumes the node (sets `depleted`).
    /// Water (a source) and resin (a regrowing tap) are not one-shot.
    pub fn one_shot(self) -> bool {
        matches!(
            self,
            Forageable::Berries
                | Forageable::Mushroom
                | Forageable::Herb
                | Forageable::Fibre
                | Forageable::Wood
        )
    }

    /// Base units for a non-plant resource at a neutral roll (plant resources
    /// derive their count from `flora_harvest` instead).
    fn base_units(self) -> u32 {
        match self {
            Forageable::Wood => 3,
            Forageable::Fibre => 4,
            Forageable::Water => 8,
            Forageable::Resin => 2,
            // plant resources don't use this path
            _ => 0,
        }
    }
}

// Per-biome availability. Biome ids are an independent `u8` (see brief). Water
// appears ONLY in water-adjacent biomes (Plains ponds, Wetland, Cave) — never
// Desert/Forest/Tundra.
const PLAINS: &[Forageable] =
    &[Forageable::Berries, Forageable::Fibre, Forageable::Herb, Forageable::Water];
const FOREST: &[Forageable] = &[
    Forageable::Berries,
    Forageable::Mushroom,
    Forageable::Wood,
    Forageable::Resin,
    Forageable::Herb,
];
const WETLAND: &[Forageable] =
    &[Forageable::Water, Forageable::Fibre, Forageable::Mushroom, Forageable::Berries];
const DESERT: &[Forageable] = &[Forageable::Fibre, Forageable::Resin];
const TUNDRA: &[Forageable] = &[Forageable::Wood, Forageable::Herb];
const CAVE: &[Forageable] = &[Forageable::Mushroom, Forageable::Water];
const DEFAULT_BIOME: &[Forageable] = &[Forageable::Fibre, Forageable::Wood];

/// What can be gathered in `biome`. Total over all `u8` (unknown → a safe
/// default set). Water is restricted to water-adjacent biomes.
pub fn available_in(biome: u8) -> &'static [Forageable] {
    match biome {
        0 => PLAINS,
        1 => FOREST,
        2 => WETLAND,
        3 => DESERT,
        4 => TUNDRA,
        5 => CAVE,
        _ => DEFAULT_BIOME,
    }
}

/// The product of a gather action: item stacks (`item id`, `count`) and whether
/// the resource node was depleted by the action.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GatherResult {
    pub items: Vec<(u16, u32)>,
    pub depleted: bool,
}

#[inline]
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Gather `f`. For plant resources, `plant` supplies the [`PlantState`] whose
/// `flora_harvest::harvest` yield is scaled into the forage count (immature →
/// ~zero); a plant resource with `plant == None` yields nothing. For non-plant
/// resources `plant` is ignored. `roll ∈ [0,1]` is the caller's seeded variation
/// (deterministic: same `roll` → same result).
pub fn gather(f: Forageable, plant: Option<&PlantState>, roll: f32) -> GatherResult {
    let r = clamp01(roll);
    // roll scales the take to 0.5..1.0× so a gather is never wholly random away.
    let roll_scale = 0.5 + 0.5 * r;

    let count: u32 = if f.is_plant() {
        match plant {
            Some(p) => {
                // Tie directly to flora_harvest's yield: total reagents + essence
                // is already 0 below MATURITY, so immature plants forage ~nothing.
                let hy = flora_harvest::harvest(p);
                let mut total: u32 = hy.essence;
                for u in hy.reagent_units.iter() {
                    total = total.saturating_add(*u);
                }
                // saturating f32→u32 cast (NaN/neg→0, huge→u32::MAX)
                ((total as f32) * roll_scale * PLANT_FORAGE_FRACTION).round() as u32
            }
            None => 0,
        }
    } else {
        ((f.base_units() as f32) * roll_scale).round() as u32
    };

    let items = if count > 0 { vec![(f.item_id(), count)] } else { Vec::new() };
    GatherResult { items, depleted: f.one_shot() }
}

/// Deposit a gather result into `regions` (filled in order), returning the total
/// count actually stored. Anything that doesn't fit — full grids, or item ids
/// with no corresponding block — is the reported overflow (`produced - stored`);
/// nothing is dropped silently and a full grid never panics.
///
/// Composes `SlotGrid::add` (the conservation-preserving atomic add) so the
/// invariant `stored + overflow == produced` holds.
pub fn deposit(result: &GatherResult, regions: &mut [&mut SlotGrid]) -> u32 {
    let mut stored: u32 = 0;

    for &(id, count) in result.items.iter() {
        // u16 id → BlockId (ids are <256 discriminants); unknown → overflow.
        let block = match u8::try_from(id).ok().and_then(|b| BlockId::try_from(b).ok()) {
            Some(b) => b,
            None => continue, // no such block → cannot store → counts as overflow
        };

        let mut remaining = count;
        for region in regions.iter_mut() {
            while remaining > 0 {
                // SlotGrid::add takes a u16 stack; deposit in u16-sized batches.
                let batch = remaining.min(u16::MAX as u32) as u16;
                let leftover = region.add(ItemStack::new(block, batch));
                let put = batch - leftover;
                stored += put as u32;
                remaining -= put as u32;
                if leftover > 0 {
                    break; // this region is full for this item → try the next
                }
            }
            if remaining == 0 {
                break;
            }
        }
        // any `remaining` here is overflow (all regions full) — left unstored.
    }

    stored
}

/// Apply a resolved harvest to the world + mob: deplete the food source and
/// feed the mob. `units_taken` is the total count from [`gather`]'s `items`;
/// `depleted` is the gather's `depleted` flag (one-shot sources vanish).
/// Returns `(new_source_mass, new_hunger)`: the source loses the taken mass (or
/// all of it if depleted), and hunger drops by `units_taken * feed_per_unit`.
/// Both clamped to `>= 0`; hunger stays in `[0,1]`. Pure.
///
/// Wiring (T3): when a foraging mob reaches its `forage_target`, the roamer tick
/// calls `gather(forageable, plant, roll)` then `apply_harvest(source_mass,
/// mob.hunger, total_units(&res.items), res.depleted, feed_per_unit)` to commit
/// the result — depleting the source cell and reducing the mob's hunger.
pub fn apply_harvest(
    source_mass: f32,
    hunger: f32,
    units_taken: u32,
    depleted: bool,
    feed_per_unit: f32,
) -> (f32, f32) {
    let taken = units_taken as f32;
    let new_mass = if depleted { 0.0 } else { (source_mass - taken).max(0.0) };
    let fed = (taken * feed_per_unit.max(0.0)).max(0.0);
    let new_hunger = (hunger - fed).clamp(0.0, 1.0);
    (new_mass, new_hunger)
}

/// Total unit count across a gather's `items` (the `units_taken` for
/// [`apply_harvest`]).
pub fn total_units(items: &[(u16, u32)]) -> u32 {
    items.iter().fold(0u32, |a, &(_, c)| a.saturating_add(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_harvest_depletes_source_and_feeds_mob() {
        // took 3 units from a 10-mass source, not one-shot, 0.1 hunger/unit
        let (mass, hunger) = apply_harvest(10.0, 0.8, 3, false, 0.1);
        assert!((mass - 7.0).abs() < 1e-6, "source mass reduced by take: {mass}");
        assert!((hunger - 0.5).abs() < 1e-6, "hunger reduced by food: {hunger}");
        // a one-shot source is fully depleted regardless of take
        let (m2, _) = apply_harvest(10.0, 0.5, 1, true, 0.1);
        assert_eq!(m2, 0.0, "one-shot source vanishes");
        // never goes negative; hunger floors at 0
        let (m3, h3) = apply_harvest(1.0, 0.05, 100, false, 1.0);
        assert_eq!(m3, 0.0);
        assert_eq!(h3, 0.0);
        // a zero-yield gather changes nothing
        let (m4, h4) = apply_harvest(5.0, 0.6, 0, false, 0.1);
        assert_eq!((m4, h4), (5.0, 0.6));
    }

    #[test]
    fn total_units_sums_item_counts() {
        assert_eq!(total_units(&[(1, 3), (2, 4)]), 7);
        assert_eq!(total_units(&[]), 0);
    }

    fn mature(element: u8) -> PlantState {
        PlantState { element, strain_bias: 0.5, growth01: 1.0 }
    }
    fn immature(element: u8) -> PlantState {
        PlantState { element, strain_bias: 0.5, growth01: 0.2 } // below MATURITY 0.4
    }

    fn produced(g: &GatherResult) -> u32 {
        g.items.iter().map(|&(_, c)| c).sum()
    }

    #[test]
    fn gather_is_deterministic_in_roll() {
        let p = mature(1);
        for &roll in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let a = gather(Forageable::Berries, Some(&p), roll);
            for _ in 0..20 {
                assert_eq!(gather(Forageable::Berries, Some(&p), roll), a);
            }
        }
        // non-plant determinism too
        let w = gather(Forageable::Wood, None, 0.6);
        for _ in 0..20 {
            assert_eq!(gather(Forageable::Wood, None, 0.6), w);
        }
    }

    #[test]
    fn immature_plant_yields_less_than_mature() {
        for &el in &[0u8, 1, 4] {
            let mat = produced(&gather(Forageable::Berries, Some(&mature(el)), 1.0));
            let imm = produced(&gather(Forageable::Berries, Some(&immature(el)), 1.0));
            assert!(imm < mat, "immature {imm} should be < mature {mat}");
            assert_eq!(imm, 0, "below-maturity forage is ~zero (flora_harvest tie)");
        }
    }

    #[test]
    fn plant_forageable_without_plant_state_is_empty() {
        let g = gather(Forageable::Herb, None, 1.0);
        assert!(g.items.is_empty());
        assert_eq!(produced(&g), 0);
    }

    #[test]
    fn nonplant_scales_with_roll_and_ignores_plant() {
        let lo = produced(&gather(Forageable::Water, None, 0.0));
        let hi = produced(&gather(Forageable::Water, None, 1.0));
        assert!(hi > lo && lo > 0, "roll scales non-plant yield: {lo}..{hi}");
        // passing a plant to a non-plant resource changes nothing
        let p = mature(1);
        assert_eq!(
            gather(Forageable::Water, Some(&p), 0.7),
            gather(Forageable::Water, None, 0.7)
        );
    }

    #[test]
    fn depleted_flag_only_for_one_shot_resources() {
        assert!(gather(Forageable::Berries, Some(&mature(1)), 1.0).depleted);
        assert!(gather(Forageable::Wood, None, 1.0).depleted);
        assert!(!gather(Forageable::Water, None, 1.0).depleted, "water source persists");
        assert!(!gather(Forageable::Resin, None, 1.0).depleted, "resin tap regrows");
    }

    #[test]
    fn available_in_is_total_and_water_restricted() {
        // total over all u8; never panics, always a (possibly default) slice
        for b in 0..=255u8 {
            let list = available_in(b);
            assert!(!list.is_empty());
            // Water only in water-adjacent biomes (0 Plains, 2 Wetland, 5 Cave)
            if list.contains(&Forageable::Water) {
                assert!(
                    matches!(b, 0 | 2 | 5),
                    "Water listed in non-water biome {b}"
                );
            }
        }
        // explicit dry-biome checks
        assert!(!available_in(3).contains(&Forageable::Water), "no water in desert");
        assert!(!available_in(4).contains(&Forageable::Water), "no water in tundra");
        assert!(!available_in(1).contains(&Forageable::Water), "no water in forest");
        assert!(available_in(2).contains(&Forageable::Water), "wetland has water");
    }

    #[test]
    fn deposit_conserves_count_and_reports_overflow() {
        let g = GatherResult { items: vec![(BlockId::OakLog as u16, 200)], depleted: true };
        let prod = produced(&g); // 200
        // one region, one slot: holds MAX_STACK (64) of a stackable block
        let mut grid = SlotGrid::new(1);
        let stored = {
            let mut refs: Vec<&mut SlotGrid> = vec![&mut grid];
            deposit(&g, &mut refs)
        };
        let overflow = prod - stored;
        assert_eq!(stored, grid.total(BlockId::OakLog), "stored == what the grid gained");
        assert_eq!(stored + overflow, prod, "conservation: stored + overflow == produced");
        assert!(stored > 0 && overflow > 0, "partial fit: stored {stored}, overflow {overflow}");
    }

    #[test]
    fn deposit_spills_across_regions() {
        let g = GatherResult { items: vec![(BlockId::Stone as u16, 100)], depleted: false };
        let mut a = SlotGrid::new(1); // 64 cap
        let mut b = SlotGrid::new(2); // plenty
        let stored = {
            let mut refs: Vec<&mut SlotGrid> = vec![&mut a, &mut b];
            deposit(&g, &mut refs)
        };
        assert_eq!(stored, 100, "all stored across two regions");
        assert_eq!(a.total(BlockId::Stone) + b.total(BlockId::Stone), 100);
    }

    #[test]
    fn deposit_full_grid_and_unknown_id_never_panic() {
        // zero-slot region: everything overflows, no panic
        let g = GatherResult { items: vec![(BlockId::Stone as u16, 50)], depleted: false };
        let mut empty = SlotGrid::new(0);
        let stored = {
            let mut refs: Vec<&mut SlotGrid> = vec![&mut empty];
            deposit(&g, &mut refs)
        };
        assert_eq!(stored, 0, "no slots → all overflow");

        // unknown item id (no such block) → counts as overflow, no panic
        let bad = GatherResult { items: vec![(60000, 10)], depleted: false };
        let mut grid = SlotGrid::new(4);
        let stored2 = {
            let mut refs: Vec<&mut SlotGrid> = vec![&mut grid];
            deposit(&bad, &mut refs)
        };
        assert_eq!(stored2, 0, "unknown id stored nothing, no panic");
    }

    #[test]
    fn end_to_end_gather_then_deposit_conserves() {
        let p = mature(0); // Wood element → plenty of berries
        let g = gather(Forageable::Berries, Some(&p), 1.0);
        let prod = produced(&g);
        assert!(prod > 0);
        let mut grid = SlotGrid::new(8);
        let stored = {
            let mut refs: Vec<&mut SlotGrid> = vec![&mut grid];
            deposit(&g, &mut refs)
        };
        assert_eq!(stored, prod, "roomy bag stores everything");
        assert_eq!(grid.total(BlockId::Flower), prod);
    }
}
