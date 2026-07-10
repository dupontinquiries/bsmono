//! `items::recipe_gen` — auto-generate the standard recipe families so every item
//! and block that *should* be craftable gets a recipe without hand-authoring each.
//!
//! Most of a voxel game's recipes are mechanical: a raw ore smelts to an ingot,
//! nine ingots pack into a storage block and unpack back, a log becomes planks,
//! planks become sticks, a block cuts into slabs and stairs, and each material tier
//! stamps out the same tool shapes (pickaxe, axe, shovel, sword, hoe). This module
//! encodes those *families* as pure functions over item ids, so the crafting
//! registry can be populated by describing materials rather than listing recipes.
//!
//! It is deliberately id-agnostic (`ItemId = u32`) and produces a neutral
//! [`GenRecipe`]; the caller adapts to `items::crafting::{Recipe, ShapedRecipe}` at
//! the boundary (shapeless when `shape` is `None`, shaped otherwise). Pure &
//! deterministic — fully unit-testable without the item registry.

/// Neutral item id; adapt to `items::ItemId` at the call site.
pub type ItemId = u32;

/// Where a generated recipe is crafted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Station {
    /// 2×2 inventory grid, no station.
    Hand,
    /// 3×3 crafting bench.
    Bench,
    /// Furnace (smelting).
    Furnace,
    /// Anvil (repair / combine / rename).
    Anvil,
}

/// A generated recipe. `shape == None` is shapeless (any arrangement); `Some((cells,
/// w, h))` is a shaped `w×h` grid (`None` cell = empty). Row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenRecipe {
    pub station: Station,
    pub inputs: Vec<(ItemId, u16)>,
    pub shape: Option<(Vec<Option<ItemId>>, usize, usize)>,
    pub output: (ItemId, u16),
}

impl GenRecipe {
    fn shapeless(station: Station, inputs: Vec<(ItemId, u16)>, output: (ItemId, u16)) -> GenRecipe {
        GenRecipe { station, inputs, shape: None, output }
    }
    fn shaped(
        station: Station,
        cells: Vec<Option<ItemId>>,
        w: usize,
        h: usize,
        output: (ItemId, u16),
    ) -> GenRecipe {
        // Inputs are the multiset of non-empty cells, for stations/tests that want
        // a flat tally regardless of arrangement.
        let mut inputs: Vec<(ItemId, u16)> = Vec::new();
        for c in cells.iter().flatten() {
            match inputs.iter_mut().find(|(i, _)| i == c) {
                Some((_, n)) => *n += 1,
                None => inputs.push((*c, 1)),
            }
        }
        GenRecipe { station, inputs, shape: Some((cells, w, h)), output }
    }
}

// --- Single-family generators -------------------------------------------------

/// Raw → smelted (furnace). e.g. iron ore → iron ingot, sand → glass.
pub fn smelt(raw: ItemId, smelted: ItemId) -> GenRecipe {
    GenRecipe::shapeless(Station::Furnace, vec![(raw, 1)], (smelted, 1))
}

/// Nine `unit` in a 3×3 → one storage `block` (e.g. 9 ingots → block).
pub fn pack9(unit: ItemId, block: ItemId) -> GenRecipe {
    GenRecipe::shaped(Station::Bench, vec![Some(unit); 9], 3, 3, (block, 1))
}

/// One storage `block` → nine `unit` (shapeless).
pub fn unpack9(block: ItemId, unit: ItemId) -> GenRecipe {
    GenRecipe::shapeless(Station::Hand, vec![(block, 1)], (unit, 9))
}

/// One `log` → four `planks` (shapeless, hand).
pub fn planks(log: ItemId, plank: ItemId) -> GenRecipe {
    GenRecipe::shapeless(Station::Hand, vec![(log, 1)], (plank, 4))
}

/// Two `plank` stacked → four `stick` (shaped 1×2, hand).
pub fn sticks(plank: ItemId, stick: ItemId) -> GenRecipe {
    GenRecipe::shaped(Station::Hand, vec![Some(plank), Some(plank)], 1, 2, (stick, 4))
}

/// Three `block` in a row → six `slab`.
pub fn slab(block: ItemId, slab: ItemId) -> GenRecipe {
    GenRecipe::shaped(Station::Bench, vec![Some(block); 3], 3, 1, (slab, 6))
}

/// Staircase of six `block` → four `stair`.
pub fn stairs(block: ItemId, stair: ItemId) -> GenRecipe {
    let cells = vec![
        Some(block), None, None,
        Some(block), Some(block), None,
        Some(block), Some(block), Some(block),
    ];
    GenRecipe::shaped(Station::Bench, cells, 3, 3, (stair, 4))
}

/// The five standard tool shapes for a material (`ingot` head + `stick` handle).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pickaxe,
    Axe,
    Shovel,
    Sword,
    Hoe,
}

/// A shaped tool recipe: `ingot` for the head, `stick` for the handle.
pub fn tool(kind: Tool, ingot: ItemId, stick: ItemId, output: ItemId) -> GenRecipe {
    let i = Some(ingot);
    let s = Some(stick);
    let n = None;
    let (cells, w, h) = match kind {
        // top row three ingots, then stick, stick
        Tool::Pickaxe => (vec![i, i, i, n, s, n, n, s, n], 3, 3),
        // two ingots + one, then stick down the side
        Tool::Axe => (vec![i, i, s, i, n, n /* mirror covers the flip */], 2, 3),
        Tool::Shovel => (vec![i, s, s], 1, 3),
        Tool::Sword => (vec![i, i, s], 1, 3),
        Tool::Hoe => (vec![i, i, n, s, n, s], 2, 3),
    };
    GenRecipe::shaped(Station::Bench, cells, w, h, (output, 1))
}

// --- Material family ----------------------------------------------------------

/// A material tier's ids, enough to stamp out its whole recipe family. Any field
/// left `None` skips that recipe (e.g. a material with no ore skips smelting).
#[derive(Clone, Copy, Debug, Default)]
pub struct MaterialSpec {
    pub ore: Option<ItemId>,
    pub ingot: Option<ItemId>,
    pub block: Option<ItemId>,
    pub stick: Option<ItemId>,
    pub pickaxe: Option<ItemId>,
    pub axe: Option<ItemId>,
    pub shovel: Option<ItemId>,
    pub sword: Option<ItemId>,
    pub hoe: Option<ItemId>,
}

/// Generate every recipe implied by a material tier: smelt ore→ingot, pack/unpack
/// ingot↔block, and each tool present. Skips any recipe whose ids aren't provided.
pub fn material_family(m: &MaterialSpec) -> Vec<GenRecipe> {
    let mut out = Vec::new();
    if let (Some(ore), Some(ingot)) = (m.ore, m.ingot) {
        out.push(smelt(ore, ingot));
    }
    if let (Some(ingot), Some(block)) = (m.ingot, m.block) {
        out.push(pack9(ingot, block));
        out.push(unpack9(block, ingot));
    }
    if let (Some(ingot), Some(stick)) = (m.ingot, m.stick) {
        for (kind, id) in [
            (Tool::Pickaxe, m.pickaxe),
            (Tool::Axe, m.axe),
            (Tool::Shovel, m.shovel),
            (Tool::Sword, m.sword),
            (Tool::Hoe, m.hoe),
        ] {
            if let Some(out_id) = id {
                out.push(tool(kind, ingot, stick, out_id));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smelt_is_one_to_one_at_furnace() {
        let r = smelt(10, 20);
        assert_eq!(r.station, Station::Furnace);
        assert_eq!(r.inputs, vec![(10, 1)]);
        assert_eq!(r.output, (20, 1));
        assert!(r.shape.is_none());
    }

    #[test]
    fn pack_and_unpack_are_inverse_counts() {
        let pack = pack9(1, 2);
        assert_eq!(pack.output, (2, 1));
        assert_eq!(pack.inputs, vec![(1, 9)], "3×3 of the unit");
        let (cells, w, h) = pack.shape.unwrap();
        assert_eq!((w, h), (3, 3));
        assert_eq!(cells.iter().flatten().count(), 9);

        let unpack = unpack9(2, 1);
        assert_eq!(unpack.inputs, vec![(2, 1)]);
        assert_eq!(unpack.output, (1, 9), "block yields the nine back");
    }

    #[test]
    fn planks_and_sticks_yields() {
        assert_eq!(planks(5, 6).output, (6, 4));
        let st = sticks(6, 7);
        assert_eq!(st.output, (7, 4));
        assert_eq!(st.inputs, vec![(6, 2)]);
    }

    #[test]
    fn slab_and_stairs_shapes() {
        let s = slab(3, 30);
        assert_eq!(s.output, (30, 6));
        assert_eq!(s.shape.unwrap().0.iter().flatten().count(), 3);
        let st = stairs(3, 31);
        assert_eq!(st.output, (31, 4));
        // Staircase uses six blocks.
        assert_eq!(st.inputs, vec![(3, 6)]);
    }

    #[test]
    fn tool_shapes_use_ingot_and_stick() {
        let p = tool(Tool::Pickaxe, 100, 7, 200);
        assert_eq!(p.output, (200, 1));
        // pickaxe: 3 ingots + 2 sticks.
        assert_eq!(p.inputs.iter().find(|(i, _)| *i == 100).unwrap().1, 3);
        assert_eq!(p.inputs.iter().find(|(i, _)| *i == 7).unwrap().1, 2);
        let sw = tool(Tool::Sword, 100, 7, 201);
        assert_eq!(sw.inputs.iter().find(|(i, _)| *i == 100).unwrap().1, 2);
        assert_eq!(sw.inputs.iter().find(|(i, _)| *i == 7).unwrap().1, 1);
    }

    #[test]
    fn material_family_covers_the_tier() {
        let m = MaterialSpec {
            ore: Some(1),
            ingot: Some(2),
            block: Some(3),
            stick: Some(9),
            pickaxe: Some(10),
            axe: Some(11),
            shovel: Some(12),
            sword: Some(13),
            hoe: Some(14),
        };
        let fam = material_family(&m);
        // smelt + pack + unpack + 5 tools = 8.
        assert_eq!(fam.len(), 8);
        assert!(fam.iter().any(|r| r.station == Station::Furnace)); // smelt
        assert_eq!(fam.iter().filter(|r| r.output.0 == 3).count(), 1); // one pack→block
    }

    #[test]
    fn partial_material_skips_missing_recipes() {
        // A stone tier: no ore/ingot, just a block and tools from the block itself.
        let m = MaterialSpec { block: Some(3), ..Default::default() };
        let fam = material_family(&m);
        assert!(fam.is_empty(), "no ingot → no smelt/pack/tools generated");
    }
}
