//! Crafting — recipes that turn held items into new ones. Shapeless (an
//! ingredient is "N of this item", order-independent), which is all the block
//! palette needs today: logs into planks, stone into bricks, sand smelted to
//! glass, planks into a bookshelf. Pure: a static recipe table plus query and
//! apply functions over an [`crate::inventory::Inventory`]. No UI, no timers —
//! the crafting screen and the furnace-as-a-timed-recipe layer build on top.

use crate::inventory::{Inventory, ItemStack};
use crate::world::blocks::BlockId;

/// One ingredient: a required count of a specific item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingredient {
    pub item: BlockId,
    pub count: u16,
}

const fn ing(item: BlockId, count: u16) -> Ingredient {
    Ingredient { item, count }
}

/// A shapeless recipe: consume `inputs`, produce `output`.
#[derive(Debug, Clone, Copy)]
pub struct Recipe {
    pub name: &'static str,
    pub inputs: &'static [Ingredient],
    pub output_item: BlockId,
    pub output_count: u16,
}

impl Recipe {
    pub fn output(&self) -> ItemStack {
        ItemStack::new(self.output_item, self.output_count)
    }
}

// Recipe ingredient lists (must be 'static for the const table).
const R_PLANKS: [Ingredient; 1] = [ing(BlockId::OakLog, 1)];
const R_STONE_BRICKS: [Ingredient; 1] = [ing(BlockId::Stone, 4)];
const R_BRICKS: [Ingredient; 1] = [ing(BlockId::Clay, 4)];
const R_GLASS: [Ingredient; 1] = [ing(BlockId::Sand, 1)]; // "smelt"
const R_BOOKSHELF: [Ingredient; 1] = [ing(BlockId::OakPlanks, 6)];
const R_COBBLE_TO_STONE: [Ingredient; 1] = [ing(BlockId::Cobblestone, 1)]; // "smelt"

/// The recipe table.
pub static RECIPES: &[Recipe] = &[
    Recipe { name: "Oak Planks", inputs: &R_PLANKS, output_item: BlockId::OakPlanks, output_count: 4 },
    Recipe { name: "Stone Bricks", inputs: &R_STONE_BRICKS, output_item: BlockId::StoneBricks, output_count: 4 },
    Recipe { name: "Bricks", inputs: &R_BRICKS, output_item: BlockId::Bricks, output_count: 4 },
    Recipe { name: "Glass", inputs: &R_GLASS, output_item: BlockId::Glass, output_count: 1 },
    Recipe { name: "Bookshelf", inputs: &R_BOOKSHELF, output_item: BlockId::Bookshelf, output_count: 1 },
    Recipe { name: "Smelt Stone", inputs: &R_COBBLE_TO_STONE, output_item: BlockId::Stone, output_count: 1 },
];

/// Does `inv` hold enough of every ingredient to make `recipe` at least once?
pub fn can_craft(recipe: &Recipe, inv: &Inventory) -> bool {
    recipe
        .inputs
        .iter()
        .all(|i| inv.total(i.item) >= i.count as u32)
}

/// Craft `recipe` once: consume the inputs and add the output to `inv`.
/// Returns `None` if the ingredients aren't all present (inventory unchanged),
/// otherwise `Some(leftover)` — the amount of output that didn't fit (0 when it
/// all fit). Inputs are only consumed when the craft succeeds.
pub fn craft(recipe: &Recipe, inv: &mut Inventory) -> Option<u16> {
    if !can_craft(recipe, inv) {
        return None;
    }
    for i in recipe.inputs {
        // Guaranteed to succeed: can_craft verified the totals.
        inv.remove(i.item, i.count);
    }
    Some(inv.pickup(recipe.output()))
}

/// Every recipe craftable from the current inventory contents.
pub fn available(inv: &Inventory) -> Vec<&'static Recipe> {
    RECIPES.iter().filter(|r| can_craft(r, inv)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Inventory, ItemStack};
    use crate::world::blocks::BlockId;

    fn find(name: &str) -> &'static Recipe {
        RECIPES.iter().find(|r| r.name == name).expect("recipe exists")
    }

    #[test]
    fn can_craft_reflects_ingredients() {
        let mut inv = Inventory::new(9, 27);
        let planks = find("Oak Planks");
        assert!(!can_craft(planks, &inv), "no logs yet");
        inv.pickup(ItemStack::new(BlockId::OakLog, 1));
        assert!(can_craft(planks, &inv), "one log is enough");
    }

    #[test]
    fn craft_consumes_inputs_and_yields_output() {
        let mut inv = Inventory::new(9, 27);
        inv.pickup(ItemStack::new(BlockId::OakLog, 3));
        let planks = find("Oak Planks");
        assert_eq!(craft(planks, &mut inv), Some(0), "crafted, nothing overflowed");
        assert_eq!(inv.total(BlockId::OakLog), 2, "one log consumed");
        assert_eq!(inv.total(BlockId::OakPlanks), 4, "four planks produced");
    }

    #[test]
    fn craft_fails_without_ingredients_and_leaves_inventory_untouched() {
        let mut inv = Inventory::new(9, 27);
        inv.pickup(ItemStack::new(BlockId::Stone, 2)); // need 4 for stone bricks
        let bricks = find("Stone Bricks");
        assert_eq!(craft(bricks, &mut inv), None, "not enough stone");
        assert_eq!(inv.total(BlockId::Stone), 2, "stone untouched on failure");
        assert_eq!(inv.total(BlockId::StoneBricks), 0);
    }

    #[test]
    fn multi_step_chain_logs_to_bookshelf() {
        let mut inv = Inventory::new(9, 27);
        inv.pickup(ItemStack::new(BlockId::OakLog, 2)); // → 8 planks
        let planks = find("Oak Planks");
        craft(planks, &mut inv).unwrap();
        craft(planks, &mut inv).unwrap();
        assert_eq!(inv.total(BlockId::OakPlanks), 8);
        // 6 planks → 1 bookshelf, leaving 2 planks.
        let bookshelf = find("Bookshelf");
        assert_eq!(craft(bookshelf, &mut inv), Some(0));
        assert_eq!(inv.total(BlockId::OakPlanks), 2, "6 planks consumed");
        assert_eq!(inv.total(BlockId::Bookshelf), 1);
    }

    #[test]
    fn available_lists_only_what_is_craftable() {
        let mut inv = Inventory::new(9, 27);
        assert!(available(&inv).is_empty(), "empty inventory crafts nothing");
        inv.pickup(ItemStack::new(BlockId::Sand, 1));
        inv.pickup(ItemStack::new(BlockId::Clay, 4));
        let names: Vec<_> = available(&inv).iter().map(|r| r.name).collect();
        assert!(names.contains(&"Glass"), "sand → glass available");
        assert!(names.contains(&"Bricks"), "clay → bricks available");
        assert!(!names.contains(&"Oak Planks"), "no logs → no planks");
    }
}
