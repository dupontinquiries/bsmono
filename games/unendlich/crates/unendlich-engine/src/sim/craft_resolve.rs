//! `sim::craft_resolve` — the crafting pipeline resolver (round R29).
//!
//! The crafting sibling of [`crate::sim::item_resolve`] (the loot resolver):
//! one pure, seeded-deterministic entry point, [`resolve_craft`], that runs the
//! existing crafting cores in pipeline order and emits a finished
//! [`CraftedItem`] descriptor:
//!
//! ```text
//!   recipe_book (resolve inputs→recipe) → crafting (can-craft / produce)
//!     → craft_quality (roll quality) → CraftedItem
//! ```
//!
//! plus a repair path, [`repair_estimate`], over `repair_cost`.
//!
//! This module **composes** those cores; it does not reimplement their math.
//! Each stage's role here:
//! * `recipe_book` — the grid's distinct ingredients are looked up
//!   (`find_by_all`); if the book knows no recipe using all of them, nothing
//!   crafts. ("what can I make with this?")
//! * `crafting`    — `RecipeTable::craft(grid)` confirms the grid actually
//!   matches a recipe and yields its output stack (the can-craft / consume gate).
//! * `craft_quality` — quality from crafter `skill`, input `material_quality`,
//!   and a seeded luck roll.
//! * `repair_cost` — `repair_estimate` prices repairing a damaged crafted item.
//!
//! All randomness derives from the caller-supplied `seed` via a documented salt
//! (R20) and a local SplitMix64 — NO global RNG, no clock, no I/O, and no
//! `.unwrap()`/`.expect()` in non-test code. Logic in, data out.

use crate::sim::crafting::{CraftGrid, ItemStack, RecipeTable, GRID};
use crate::sim::item_appraise::{appraise, AppraiseInput};
use crate::sim::market_price;
use crate::sim::recipe_book::{ItemId as BookItemId, RecipeBook};
use crate::sim::{craft_quality, repair_cost};

/// Inputs to one crafting resolution. Borrowed tables keep it allocation-light;
/// everything is plain data so the result is reproducible across peers and
/// rollback from `seed` alone.
#[derive(Clone, Debug)]
pub struct CraftInput<'a> {
    /// Master seed; the quality luck-roll derives from this (documented salt).
    pub seed: u64,
    /// Ingredient→recipe index ("what can I make"); built from the same recipes
    /// as `table`.
    pub book: &'a RecipeBook,
    /// The crafting recipe table that produces the output stack.
    pub table: &'a RecipeTable,
    /// The placed 3×3 crafting grid.
    pub grid: &'a CraftGrid,
    /// Crafter skill `[0,1]`.
    pub skill: f32,
    /// Aggregate input material quality `[0,1]`.
    pub material_quality: f32,
    /// Base (max) durability of the produced item before quality scaling.
    pub base_durability: f32,
}

/// A fully-resolved crafted item descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftedItem {
    /// Produced item kind id.
    pub id: u16,
    /// Produced stack count.
    pub count: u32,
    /// Craft quality `[0,1]`.
    pub quality: f32,
    /// Max durability of the produced item (base scaled by quality).
    pub durability: f32,
}

// Documented roll salt (R20): the quality luck-roll mixes the master seed with
// this so it decorrelates from any other per-craft stream.
const SALT_LUCK: u64 = 0xC0FF_EEC0_FFEE_0029;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Deterministic uniform `[0,1)` for `(seed, salt)`.
fn roll_unit(seed: u64, salt: u64) -> f32 {
    let h = mix64(seed ^ salt.wrapping_mul(GOLDEN));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Distinct ingredient ids placed in the grid (as `recipe_book` ids), sorted
/// ascending. Used to query the book for a matching recipe.
fn grid_ingredients(grid: &CraftGrid) -> Vec<BookItemId> {
    let mut ids: Vec<u16> = Vec::new();
    let mut r = 0;
    while r < GRID {
        let mut c = 0;
        while c < GRID {
            if let Some(id) = grid.get(r, c) {
                ids.push(id.0);
            }
            c += 1;
        }
        r += 1;
    }
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter().map(BookItemId).collect()
}

/// Run the crafting pipeline. Returns `None` when nothing can be crafted — the
/// book knows no recipe using the grid's ingredients, or the grid matches no
/// recipe in the table (a missing/incorrect ingredient).
pub fn resolve_craft(input: &CraftInput) -> Option<CraftedItem> {
    // 1. recipe_book — resolve the grid's ingredients to candidate recipes.
    let ingredients = grid_ingredients(input.grid);
    if input.book.find_by_all(&ingredients).is_empty() {
        return None;
    }

    // 2. crafting — confirm the grid actually crafts, and get the output stack.
    let stack: ItemStack = match input.table.craft(input.grid) {
        Some(s) => s,
        None => return None,
    };

    // 3. craft_quality — quality from skill + material + a seeded luck roll.
    let luck = roll_unit(input.seed, SALT_LUCK);
    let quality = craft_quality::quality(input.skill, input.material_quality, luck);

    // Better crafts come out sturdier: scale base durability by quality.
    let durability = input.base_durability.max(0.0) * (0.5 + 0.5 * quality);

    Some(CraftedItem {
        id: stack.id.0,
        count: stack.count,
        quality,
        durability,
    })
}

/// Cost to repair a crafted item from `current_durability` back to full, at
/// `rate` per missing point (via `repair_cost`). Full durability ⇒ 0; cost
/// rises as the item takes more damage.
pub fn repair_estimate(item: &CraftedItem, current_durability: f32, rate: f32) -> f32 {
    repair_cost::repair_cost(item.durability, current_durability, rate)
}

/// Trade value of a *freshly* crafted item: bridges a [`CraftedItem`] into
/// [`item_appraise::appraise`]. A new craft carries no enchant or modifiers and
/// sits at full durability, so only its `quality`, the item kind's `base_value`,
/// and `rarity` tier move the price. Always finite and `≥ 0`; non-decreasing in
/// quality, base value, and rarity. Connects the crafting resolver to the
/// appraisal core (previously disjoint).
pub fn appraise_crafted(item: &CraftedItem, base_value: f32, rarity: u8) -> f32 {
    let max = item.durability.max(0.0).round() as i32;
    appraise(&AppraiseInput {
        base_value,
        rarity,
        quality: item.quality,
        enchant_level: 0,        // fresh craft — unenchanted
        durability_cur: max,     // full durability
        durability_max: max,
        modifiers: &[],          // fresh craft — no modifiers
    })
}

/// Live market sale price for a crafted item: its appraised value
/// ([`appraise_crafted`]) put through market `demand` via
/// [`market_price::market_value`] (which applies a demand multiplier and floor
/// and does **not** re-apply rarity, so it never double-counts the rarity
/// already folded into the appraisal). The craft-side twin of
/// `item_resolve::market_price_resolved`, completing the chain
/// resolve_craft → appraise_crafted → market_price_crafted. Pure.
pub fn market_price_crafted(item: &CraftedItem, base_value: f32, rarity: u8, demand: f32) -> f32 {
    market_price::market_value(appraise_crafted(item, base_value, rarity), demand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::crafting::{ItemId as CraftItemId, Recipe};
    use crate::sim::recipe_book::ItemId as BId;

    // A shapeless recipe {1,2} → item 100 (x1), with a matching book + grid.
    fn table() -> RecipeTable {
        RecipeTable::new(vec![Recipe::Shapeless {
            ingredients: vec![CraftItemId(1), CraftItemId(2)],
            output: ItemStack::new(CraftItemId(100), 1),
        }])
    }
    fn book() -> RecipeBook {
        RecipeBook::build(&[vec![BId(1), BId(2)]])
    }
    fn full_grid() -> CraftGrid {
        let mut g = CraftGrid::empty();
        g.set(0, 0, Some(CraftItemId(1)));
        g.set(0, 1, Some(CraftItemId(2)));
        g
    }
    fn missing_grid() -> CraftGrid {
        let mut g = CraftGrid::empty();
        g.set(0, 0, Some(CraftItemId(1))); // only one of the two ingredients
        g
    }
    fn input<'a>(
        seed: u64,
        skill: f32,
        bk: &'a RecipeBook,
        tb: &'a RecipeTable,
        gr: &'a CraftGrid,
    ) -> CraftInput<'a> {
        CraftInput {
            seed,
            book: bk,
            table: tb,
            grid: gr,
            skill,
            material_quality: 0.5,
            base_durability: 100.0,
        }
    }

    #[test]
    fn fixed_seed_yields_stable_output() {
        let (bk, tb, gr) = (book(), table(), full_grid());
        let a = resolve_craft(&input(0xABCD, 0.6, &bk, &tb, &gr));
        let b = resolve_craft(&input(0xABCD, 0.6, &bk, &tb, &gr));
        assert!(a.is_some());
        assert_eq!(a, b, "same input crafts identically");
        let item = a.unwrap();
        assert_eq!(item.id, 100);
        assert_eq!(item.count, 1);
        assert!((0.0..=1.0).contains(&item.quality));
    }

    #[test]
    fn missing_ingredient_no_craft() {
        let (bk, tb, gr) = (book(), table(), missing_grid());
        // Grid multiset {1} matches no recipe (the Shapeless wants {1,2}).
        for seed in 0..256u64 {
            assert!(resolve_craft(&input(seed, 0.5, &bk, &tb, &gr)).is_none(), "seed {seed}");
        }
    }

    #[test]
    fn unknown_recipe_no_craft() {
        // Book knows recipe {1,2}; a grid of {5,6} resolves to no recipe.
        let bk = book();
        let tb = table();
        let mut gr = CraftGrid::empty();
        gr.set(0, 0, Some(CraftItemId(5)));
        gr.set(0, 1, Some(CraftItemId(6)));
        assert!(resolve_craft(&input(1, 0.5, &bk, &tb, &gr)).is_none());
    }

    #[test]
    fn higher_skill_better_quality() {
        let (bk, tb, gr) = (book(), table(), full_grid());
        let low = resolve_craft(&input(7, 0.1, &bk, &tb, &gr)).expect("crafts");
        let high = resolve_craft(&input(7, 0.9, &bk, &tb, &gr)).expect("crafts");
        assert!(high.quality > low.quality, "more skill ⇒ higher quality");
        // and the sturdier-craft invariant follows quality
        assert!(high.durability > low.durability);
    }

    #[test]
    fn repair_cost_scales_with_damage() {
        let (bk, tb, gr) = (book(), table(), full_grid());
        let item = resolve_craft(&input(3, 0.7, &bk, &tb, &gr)).expect("crafts");
        let max = item.durability;
        assert_eq!(repair_estimate(&item, max, 2.0), 0.0, "full durability is free");
        let light = repair_estimate(&item, max * 0.8, 2.0);
        let heavy = repair_estimate(&item, max * 0.1, 2.0);
        assert!(heavy > light && light > 0.0, "more damage ⇒ higher cost");
        // and rate scales it
        assert!(repair_estimate(&item, max * 0.1, 4.0) > heavy);
    }

    #[test]
    fn invariants_hold_over_many_seeds() {
        let (bk, tb, gr) = (book(), table(), full_grid());
        for seed in 0..512u64 {
            if let Some(item) = resolve_craft(&input(seed, 0.6, &bk, &tb, &gr)) {
                assert!((0.0..=1.0).contains(&item.quality), "quality in range (seed {seed})");
                assert!(item.durability.is_finite() && item.durability >= 0.0);
                assert!(item.durability <= 100.0 + 1e-3, "durability ≤ base");
                assert_eq!(item.id, 100);
                assert_eq!(item.count, 1);
            }
        }
    }

    #[test]
    fn appraise_crafted_rises_with_quality_base_and_rarity() {
        let lo = CraftedItem { id: 1, count: 1, quality: 0.1, durability: 100.0 };
        let hi = CraftedItem { id: 1, count: 1, quality: 0.9, durability: 100.0 };
        let v_lo = appraise_crafted(&lo, 10.0, 1);
        let v_hi = appraise_crafted(&hi, 10.0, 1);
        assert!(v_hi > v_lo, "higher quality is worth more");
        assert!(appraise_crafted(&hi, 20.0, 1) > v_hi, "higher base value is worth more");
        assert!(appraise_crafted(&hi, 10.0, 5) > appraise_crafted(&hi, 10.0, 0), "rarer is worth more");
        for v in [v_lo, v_hi] {
            assert!(v.is_finite() && v >= 0.0);
        }
    }

    #[test]
    fn appraise_crafted_zero_base_is_zero() {
        let it = CraftedItem { id: 1, count: 1, quality: 0.8, durability: 50.0 };
        assert_eq!(appraise_crafted(&it, 0.0, 3), 0.0, "no base value ⇒ no trade value");
    }

    #[test]
    fn market_price_crafted_tracks_quality_and_demand() {
        let lo = CraftedItem { id: 1, count: 1, quality: 0.2, durability: 100.0 };
        let hi = CraftedItem { id: 1, count: 1, quality: 0.9, durability: 100.0 };
        let a = market_price_crafted(&lo, 20.0, 2, 1.0);
        let b = market_price_crafted(&hi, 20.0, 2, 1.0);
        assert!(b > a, "higher craft quality ⇒ higher sale price");
        assert!(market_price_crafted(&hi, 20.0, 2, 1.5) > b, "more demand ⇒ higher price");
        assert!(a >= 1.0 && a.is_finite(), "price respects the market floor and is finite");
    }
}
