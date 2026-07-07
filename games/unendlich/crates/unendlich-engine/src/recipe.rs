//! Recipes — the crafting/smelting graph that turns items into other items.
//! A `Recipe` consumes input stacks at a `Station` (your hand, a bench, a
//! furnace) and yields an output stack. This is the data + matching logic only;
//! the inventory UI and the act of placing a bench/furnace in the world are
//! engine wiring.
//!
//! The registry here covers the block-to-block transforms the current palette
//! supports (planks from logs, glass and bricks from the furnace, stone from
//! cobble, stone bricks from stone). Tools and ingots will slot in cleanly once
//! `ItemKind` grows those variants — the matching engine below is agnostic to
//! what an item *is*, so no logic changes when the catalog expands.

use crate::item::{ItemKind, MetalKind, ToolKind};
use crate::world::blocks::BlockId;

/// Where a recipe can be performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Station {
    /// Craftable in the inventory grid with no station.
    Hand,
    /// Needs a crafting bench.
    Bench,
    /// Needs a furnace (smelting).
    Furnace,
}

/// One input or output: an item and how many.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ingredient {
    pub item: ItemKind,
    pub count: u16,
}

/// A transformation: consume `inputs` at `station`, produce `output`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub station: Station,
    pub inputs: Vec<Ingredient>,
    pub output: Ingredient,
}

fn ing(b: BlockId, count: u16) -> Ingredient {
    Ingredient { item: ItemKind::Block(b), count }
}

/// An ingredient that is any `ItemKind` (ingots, tools), not just a block.
fn item_ing(item: ItemKind, count: u16) -> Ingredient {
    Ingredient { item, count }
}

impl Recipe {
    fn new(station: Station, inputs: Vec<Ingredient>, out: BlockId, out_count: u16) -> Self {
        Recipe { station, inputs, output: ing(out, out_count) }
    }

    /// Like [`Recipe::new`] but the output is an arbitrary `ItemKind` (an ingot
    /// or a tool), not a block.
    fn new_item(station: Station, inputs: Vec<Ingredient>, out: ItemKind, out_count: u16) -> Self {
        Recipe { station, inputs, output: item_ing(out, out_count) }
    }

    /// How many of `item` are available in a (item, count) tally.
    fn available(tally: &[(ItemKind, u16)], item: ItemKind) -> u16 {
        tally.iter().filter(|(i, _)| *i == item).map(|(_, n)| *n).sum()
    }

    /// Can this recipe be made from the given inventory tally?
    pub fn can_make_from(&self, tally: &[(ItemKind, u16)]) -> bool {
        self.inputs.iter().all(|inp| Self::available(tally, inp.item) >= inp.count)
    }

    /// Consume this recipe's inputs from `tally` and add its output. Returns
    /// false (leaving `tally` untouched) if the inputs aren't available.
    pub fn apply(&self, tally: &mut Vec<(ItemKind, u16)>) -> bool {
        if !self.can_make_from(tally) {
            return false;
        }
        for inp in &self.inputs {
            let mut remaining = inp.count;
            for (item, n) in tally.iter_mut() {
                if *item == inp.item && remaining > 0 {
                    let take = remaining.min(*n);
                    *n -= take;
                    remaining -= take;
                }
            }
        }
        tally.retain(|(_, n)| *n > 0);
        if let Some((_, n)) = tally.iter_mut().find(|(i, _)| *i == self.output.item) {
            *n = n.saturating_add(self.output.count);
        } else {
            tally.push((self.output.item, self.output.count));
        }
        true
    }
}

/// The full recipe registry.
pub fn recipes() -> Vec<Recipe> {
    use Station::*;
    let mut v = vec![
        // Bench
        Recipe::new(Bench, vec![ing(BlockId::OakLog, 1)], BlockId::OakPlanks, 4),
        Recipe::new(Bench, vec![ing(BlockId::Stone, 4)], BlockId::StoneBricks, 4),
        // Furnace (smelting)
        Recipe::new(Furnace, vec![ing(BlockId::Sand, 1)], BlockId::Glass, 1),
        Recipe::new(Furnace, vec![ing(BlockId::Cobblestone, 1)], BlockId::Stone, 1),
        Recipe::new(Furnace, vec![ing(BlockId::Clay, 1)], BlockId::Bricks, 1),
    ];
    // Smelt each ore into its ingot (furnace).
    for m in MetalKind::ALL {
        v.push(Recipe::new_item(Furnace, vec![ing(m.ore(), 1)], ItemKind::Ingot(m), 1));
    }
    // Forge each tool from ingots + plank handles (bench). The handle is two
    // planks for every tool; the head's ingot cost varies by shape.
    for m in MetalKind::ALL {
        for t in ToolKind::ALL {
            v.push(Recipe::new_item(
                Bench,
                vec![item_ing(ItemKind::Ingot(m), t.ingot_cost()), ing(BlockId::OakPlanks, 2)],
                ItemKind::Tool(t, m),
                1,
            ));
        }
    }
    v
}

/// Recipes whose output is `item` (how do I make this?).
pub fn recipes_for(item: ItemKind) -> Vec<Recipe> {
    recipes().into_iter().filter(|r| r.output.item == item).collect()
}

/// Recipes performable at `station`.
pub fn recipes_at(station: Station) -> Vec<Recipe> {
    recipes().into_iter().filter(|r| r.station == station).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(b: BlockId) -> ItemKind { ItemKind::Block(b) }

    #[test]
    fn ore_smelts_into_its_ingot_at_the_furnace() {
        for m in MetalKind::ALL {
            let rs = recipes_for(ItemKind::Ingot(m));
            assert_eq!(rs.len(), 1, "exactly one way to smelt {}", m.name());
            let r = &rs[0];
            assert_eq!(r.station, Station::Furnace);
            assert_eq!(r.inputs, vec![ing(m.ore(), 1)]);
            assert_eq!(r.output, item_ing(ItemKind::Ingot(m), 1));
        }
    }

    #[test]
    fn tools_need_ingots_and_planks_at_the_bench() {
        for m in MetalKind::ALL {
            for t in ToolKind::ALL {
                let rs = recipes_for(ItemKind::Tool(t, m));
                assert_eq!(rs.len(), 1, "one recipe per tool/metal");
                let r = &rs[0];
                assert_eq!(r.station, Station::Bench);
                // ingot cost matches the tool shape, plus two plank handle.
                assert!(r.inputs.contains(&item_ing(ItemKind::Ingot(m), t.ingot_cost())));
                assert!(r.inputs.contains(&ing(BlockId::OakPlanks, 2)));
            }
        }
    }

    #[test]
    fn full_chain_ore_to_ingot_to_tool() {
        // Start with three iron ore and two planks. Raw ore can't become a tool
        // directly; we must smelt first.
        let pick = recipes_for(ItemKind::Tool(ToolKind::Pickaxe, MetalKind::Iron)).remove(0);
        let mut tally = vec![(block(BlockId::IronOre), 3), (block(BlockId::OakPlanks), 2)];
        assert!(!pick.can_make_from(&tally), "can't forge a tool straight from ore");

        // Smelt all three ores into ingots.
        let smelt = recipes_for(ItemKind::Ingot(MetalKind::Iron)).remove(0);
        for _ in 0..3 {
            assert!(smelt.apply(&mut tally));
        }
        let ingots = tally.iter().find(|(i, _)| *i == ItemKind::Ingot(MetalKind::Iron)).map(|(_, n)| *n);
        assert_eq!(ingots, Some(3));

        // Now the pickaxe is craftable; forging it consumes 3 ingots + 2 planks.
        assert!(pick.can_make_from(&tally));
        assert!(pick.apply(&mut tally));
        let tools = tally.iter().find(|(i, _)| *i == ItemKind::Tool(ToolKind::Pickaxe, MetalKind::Iron));
        assert_eq!(tools.map(|(_, n)| *n), Some(1));
        // Ingots and planks are spent.
        assert!(tally.iter().all(|(i, _)| *i != ItemKind::Ingot(MetalKind::Iron)));
    }

    #[test]
    fn shovel_is_cheaper_than_pickaxe() {
        assert!(ToolKind::Shovel.ingot_cost() < ToolKind::Pickaxe.ingot_cost());
        let shovel = recipes_for(ItemKind::Tool(ToolKind::Shovel, MetalKind::Copper)).remove(0);
        assert!(shovel.can_make_from(&[
            (ItemKind::Ingot(MetalKind::Copper), 1),
            (block(BlockId::OakPlanks), 2),
        ]));
    }

    #[test]
    fn metal_ore_roundtrip() {
        for m in MetalKind::ALL {
            assert_eq!(MetalKind::from_ore(m.ore()), Some(m));
        }
        assert_eq!(MetalKind::from_ore(BlockId::Stone), None);
    }

    #[test]
    fn one_log_makes_four_planks() {
        let r = recipes_for(block(BlockId::OakPlanks));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].station, Station::Bench);
        assert_eq!(r[0].inputs, vec![ing(BlockId::OakLog, 1)]);
        assert_eq!(r[0].output.count, 4);
    }

    #[test]
    fn smelting_lives_at_the_furnace() {
        let f = recipes_at(Station::Furnace);
        assert!(f.iter().any(|r| r.output.item == block(BlockId::Glass)));
        assert!(f.iter().all(|r| r.station == Station::Furnace));
        // Glass is not a bench recipe.
        assert!(recipes_at(Station::Bench).iter().all(|r| r.output.item != block(BlockId::Glass)));
    }

    #[test]
    fn craftability_depends_on_inputs() {
        let plank = recipes_for(block(BlockId::OakPlanks)).remove(0);
        assert!(!plank.can_make_from(&[]));
        assert!(!plank.can_make_from(&[(block(BlockId::Stone), 64)]));
        assert!(plank.can_make_from(&[(block(BlockId::OakLog), 1)]));
    }

    #[test]
    fn apply_consumes_inputs_and_adds_output() {
        let plank = recipes_for(block(BlockId::OakPlanks)).remove(0);
        let mut bag = vec![(block(BlockId::OakLog), 3)];
        assert!(plank.apply(&mut bag));
        // One log consumed -> 2 logs left, 4 planks gained.
        assert_eq!(Recipe::available(&bag, block(BlockId::OakLog)), 2);
        assert_eq!(Recipe::available(&bag, block(BlockId::OakPlanks)), 4);
    }

    #[test]
    fn apply_fails_cleanly_when_short() {
        let bricks = recipes_for(block(BlockId::StoneBricks)).remove(0); // needs 4 stone
        let mut bag = vec![(block(BlockId::Stone), 2)];
        assert!(!bricks.apply(&mut bag));
        assert_eq!(bag, vec![(block(BlockId::Stone), 2)], "tally untouched on failure");
    }

    #[test]
    fn every_recipe_is_well_formed() {
        for r in recipes() {
            assert!(!r.inputs.is_empty(), "a recipe needs inputs");
            assert!(r.inputs.iter().all(|i| i.count > 0));
            assert!(r.output.count > 0);
        }
    }
}
