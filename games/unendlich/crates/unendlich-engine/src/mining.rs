//! Mining — which tool harvests which block, gated by the tool's metal tier,
//! and what a broken block drops. This is what gives the crafted tools
//! (`ItemKind::Tool`) an actual job: the ore→ingot→tool chain in `recipe.rs`
//! produces a pick/axe/shovel, and that tool decides whether breaking a block
//! yields its item.
//!
//! The classification is exhaustive over every `BlockId` (no wildcard arm), so
//! adding a block forces a deliberate decision about how it's harvested — the
//! same falsification discipline the block-color table uses.
//!
//! ## Model
//!
//! - [`proper_tool`] — the tool class a block wants (`None` = breaks by hand,
//!   e.g. plants, leaves, fluids).
//! - [`required_tier`] — the minimum metal tier (`MetalKind::mining_tier`) the
//!   tool must have. `0` = no tool needed; `UNBREAKABLE` = never harvestable.
//! - [`harvests`] — does breaking `block` with this tool (or bare hand) yield a
//!   drop? Wrong tool class, or too low a tier, means no drop.
//! - [`block_drop`] — the actual item produced (Stone→Cobblestone, Grass→Dirt,
//!   leaves/fluids→nothing, otherwise the block itself).

use crate::item::{ItemKind, MetalKind, ToolKind};
use crate::world::blocks::BlockId;

/// Sentinel tier for blocks that can never be harvested (Bedrock).
pub const UNBREAKABLE: u8 = u8::MAX;

/// The tool class that properly harvests `block`. `None` means it breaks by
/// hand with no tool advantage (plants, leaves, fluids, bedrock).
pub fn proper_tool(block: BlockId) -> Option<ToolKind> {
    use BlockId::*;
    match block {
        // Pickaxe: stone, ores, minerals, fired/metal building blocks, glass.
        Stone | Cobblestone | GoldOre | IronOre | CoalOre | CopperOre | DiamondOre
        | RedSandstone | Dripstone | Ice | Limestone | Marble | Basalt | Tuff
        | Granite | Slate | Calcite | Andesite | Schist | Amber | Crystal
        | Sandstone | Obsidian | Quartz | StoneBricks | Bricks | Glass
        | IronBlock | GoldBlock | CoalBlock | Bone | MossyCobblestone | PolishedStone | PolishedBasalt | ChiseledStoneBricks | SmoothSandstone | Terracotta
        | Selenite | Gypsum | Dolomite | Sulfur | Magma | GlowCrystal => Some(ToolKind::Pickaxe),

        // Axe: wood and woody decoration.
        OakLog | BirchLog | OakPlanks | Bookshelf | Pumpkin | DarkOakPlanks => Some(ToolKind::Axe),

        // Shovel: loose ground.
        Dirt | Grass | Sand | Gravel | Clay | Snow | SnowBlock | Moss => Some(ToolKind::Shovel),

        // Hand: fluids, air, decorative plants, foliage, cactus, bedrock.
        Air | Water | Lava | OakLeaves | BirchLeaves | Cactus | Flower | TallGrass
        | Fern | DeadBush | Vine | Cobweb | Mushroom | Lichen | Bedrock
        | Carrot | Potato | Cabbage | Sprout => None,
    }
}

/// Minimum tool tier needed to harvest `block`. `0` = hand-harvestable;
/// `UNBREAKABLE` = never.
pub fn required_tier(block: BlockId) -> u8 {
    use BlockId::*;
    match block {
        Bedrock => UNBREAKABLE,

        // Tier 3: the hardest materials need the top metal (Gold).
        DiamondOre | Obsidian => 3,

        // Tier 2: ores and metal blocks need a mid metal (Iron+).
        GoldOre | IronOre | CopperOre | IronBlock | GoldBlock | Crystal => 2,

        // Tier 1: ordinary stone/minerals (pickaxe) and all tool-required
        // ground/wood (shovel/axe) — any tool of the right class works.
        Stone | Cobblestone | CoalOre | RedSandstone | Dripstone | Ice | Limestone
        | Marble | Basalt | Tuff | Granite | Slate | Calcite | Andesite | Schist
        | Amber | Sandstone | Quartz | StoneBricks | Bricks | Glass | CoalBlock | Bone
        | OakLog | BirchLog | OakPlanks | Bookshelf | Pumpkin | DarkOakPlanks
        | MossyCobblestone | PolishedStone | PolishedBasalt | ChiseledStoneBricks | SmoothSandstone | Terracotta
        | Dirt | Grass | Sand | Gravel | Clay | Snow | SnowBlock | Moss
        | Selenite | Gypsum | Dolomite | Sulfur | Magma | GlowCrystal => 1,

        // Tier 0: hand-harvestable.
        Air | Water | Lava | OakLeaves | BirchLeaves | Cactus | Flower | TallGrass
        | Fern | DeadBush | Vine | Cobweb | Mushroom | Lichen | Carrot | Potato | Cabbage | Sprout => 0,
    }
}

/// Does breaking `block` with `tool` (or bare hand, `None`) yield its drop?
/// Unbreakable blocks never do. Hand-harvestable blocks always do. Otherwise the
/// tool must be the proper class AND at least the required tier.
pub fn harvests(tool: Option<(ToolKind, MetalKind)>, block: BlockId) -> bool {
    let tier = required_tier(block);
    if tier == UNBREAKABLE {
        return false;
    }
    match proper_tool(block) {
        None => true, // hand-harvestable
        Some(needed) => match tool {
            Some((kind, metal)) => kind == needed && metal.mining_tier() >= tier,
            None => false, // needs a tool, used a bare hand
        },
    }
}

/// The item a broken block yields, or `None` if nothing drops (wrong/missing
/// tool, unbreakable, or a block that yields nothing — fluids, air, leaves).
pub fn block_drop(block: BlockId, tool: Option<(ToolKind, MetalKind)>) -> Option<ItemKind> {
    use BlockId::*;
    if !harvests(tool, block) {
        return None;
    }
    match block {
        Air | Water | Lava => None,
        OakLeaves | BirchLeaves => None, // canopy yields nothing here (no sapling item yet)
        Sprout => None, // r520: a seedling yields nothing if smashed; harvest returns the seed
        Stone => Some(ItemKind::Block(Cobblestone)),
        Grass => Some(ItemKind::Block(Dirt)),
        other => Some(ItemKind::Block(other)),
    }
}

/// Relative break hardness in "ticks at 1× speed". `UNBREAKABLE_HARDNESS` marks a
/// block that can't be broken at all. Tuned by feel, not physics: plants snap
/// instantly, ground is soft, stone is medium, ores hard, diamond/obsidian very
/// hard. Exhaustive (no wildcard) so a new block must be assigned a hardness.
pub const UNBREAKABLE_HARDNESS: u16 = u16::MAX;

pub fn base_hardness(block: BlockId) -> u16 {
    use BlockId::*;
    const INSTANT: u16 = 0;
    const SOFT: u16 = 6;
    const WOOD: u16 = 10;
    const FRAGILE: u16 = 3;
    const STONE: u16 = 30;
    const HARD: u16 = 45;
    const VERYHARD: u16 = 90;
    match block {
        // Fluids + decorative plants: snap / no resistance.
        Air | Water | Lava | Flower | TallGrass | Fern | DeadBush | Vine | Cobweb
        | Mushroom | Lichen | Carrot | Potato | Cabbage | Sprout => INSTANT,
        // Leaves + loose ground: soft.
        OakLeaves | BirchLeaves | Dirt | Grass | Sand | Gravel | Clay | Snow | SnowBlock
        | Moss => SOFT,
        // Wood + woody decoration + cactus.
        OakLog | BirchLog | OakPlanks | Bookshelf | Pumpkin | Cactus | DarkOakPlanks => WOOD,
        Glass => FRAGILE,
        // Ordinary stone, soft minerals, coal, organic stone.
        Stone | Cobblestone | Sandstone | RedSandstone | Limestone | Tuff | Calcite
        | Andesite | Ice | StoneBricks | Bricks | CoalOre | CoalBlock | Bone | Amber
        | MossyCobblestone | PolishedStone | PolishedBasalt | ChiseledStoneBricks | SmoothSandstone | Terracotta
        | Selenite | Gypsum | Dolomite | Sulfur => STONE,
        // Dense rock, metal ores + blocks, crystal.
        Marble | Basalt | Granite | Slate | Schist | Quartz | Dripstone | IronOre
        | GoldOre | CopperOre | IronBlock | GoldBlock | Crystal | Magma | GlowCrystal => HARD,
        DiamondOre | Obsidian => VERYHARD,
        Bedrock => UNBREAKABLE_HARDNESS,
    }
}

/// How many ticks it takes to break `block` with `tool` (or bare hand, `None`).
/// `None` = unbreakable. The proper tool at sufficient tier is fast (speed scales
/// with metal tier); a wrong/weak tool or bare hand breaks at base speed and
/// (per [`harvests`]) yields no drop. Instant blocks always return `Some(0)`.
pub fn break_ticks(block: BlockId, tool: Option<(ToolKind, MetalKind)>) -> Option<u32> {
    let h = base_hardness(block);
    if h == UNBREAKABLE_HARDNESS {
        return None;
    }
    if h == 0 {
        return Some(0);
    }
    // Speed factor: the proper tool at/above the required tier mines fast
    // (1 + tier); anything else is base speed.
    let speed: u32 = match proper_tool(block) {
        Some(needed) => match tool {
            Some((kind, metal))
                if kind == needed && metal.mining_tier() >= required_tier(block) =>
            {
                1 + metal.mining_tier() as u32
            }
            _ => 1,
        },
        None => 1,
    };
    Some(((h as u32) + speed - 1) / speed) // ceil division
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;

    /// The strongest tool of the proper class for a block (for speed checks).
    fn best_tool(b: BlockId) -> Option<(ToolKind, MetalKind)> {
        proper_tool(b).map(|k| (k, MetalKind::Gold))
    }

    #[test]
    fn only_bedrock_is_unbreakable_by_time() {
        for b in BlockId::ALL {
            let t = break_ticks(b, best_tool(b));
            if b == BlockId::Bedrock {
                assert_eq!(t, None, "bedrock unbreakable");
            } else {
                assert!(t.is_some(), "{b:?} should be breakable, got None");
            }
        }
    }

    #[test]
    fn higher_tier_breaks_faster() {
        // Stone with progressively better picks: strictly faster (or equal at the
        // floor of ceil-division), never slower.
        let copper = break_ticks(BlockId::Stone, Some((ToolKind::Pickaxe, MetalKind::Copper))).unwrap();
        let iron = break_ticks(BlockId::Stone, Some((ToolKind::Pickaxe, MetalKind::Iron))).unwrap();
        let gold = break_ticks(BlockId::Stone, Some((ToolKind::Pickaxe, MetalKind::Gold))).unwrap();
        let hand = break_ticks(BlockId::Stone, None).unwrap();
        assert!(gold < iron, "gold {gold} should beat iron {iron}");
        assert!(iron < copper, "iron {iron} should beat copper {copper}");
        assert!(copper < hand, "any pick {copper} should beat the hand {hand}");
    }

    #[test]
    fn wrong_tool_breaks_at_base_speed() {
        // An axe on stone is no faster than a bare hand (base speed, and per
        // harvests() it yields nothing).
        let axe = break_ticks(BlockId::Stone, Some((ToolKind::Axe, MetalKind::Gold))).unwrap();
        let hand = break_ticks(BlockId::Stone, None).unwrap();
        assert_eq!(axe, hand);
        // An under-tier pick on diamond ore is base speed too (and no drop).
        let weak = break_ticks(BlockId::DiamondOre, Some((ToolKind::Pickaxe, MetalKind::Iron))).unwrap();
        let hand_dia = break_ticks(BlockId::DiamondOre, None).unwrap();
        assert_eq!(weak, hand_dia);
    }

    #[test]
    fn instant_blocks_take_no_time() {
        for b in [BlockId::Flower, BlockId::TallGrass, BlockId::Water, BlockId::Air] {
            assert_eq!(break_ticks(b, None), Some(0));
        }
    }

    #[test]
    fn harder_blocks_take_longer_with_the_same_tool() {
        let gold_pick = Some((ToolKind::Pickaxe, MetalKind::Gold));
        let stone = break_ticks(BlockId::Stone, gold_pick).unwrap();
        let diamond = break_ticks(BlockId::DiamondOre, gold_pick).unwrap();
        assert!(diamond > stone, "diamond {diamond} should outlast stone {stone} even with gold");
    }

    #[test]
    fn every_block_classified_consistently() {
        // Exhaustive coverage via BlockId::ALL: proper_tool and required_tier
        // both answer for every block, and the two agree on the hand case.
        for b in BlockId::ALL {
            let tier = required_tier(b);
            match proper_tool(b) {
                // Hand-harvestable (None) must be tier 0 or unbreakable (bedrock).
                None => assert!(tier == 0 || tier == UNBREAKABLE,
                    "{b:?}: hand-harvestable but tier {tier}"),
                // Tool-required blocks need a real tier in 1..=3.
                Some(_) => assert!((1..=3).contains(&tier),
                    "{b:?}: tool-required but tier {tier}"),
            }
        }
    }

    #[test]
    fn bedrock_is_unbreakable() {
        assert_eq!(required_tier(BlockId::Bedrock), UNBREAKABLE);
        // Nothing harvests it — not even a gold pick, not the hand.
        assert!(!harvests(Some((ToolKind::Pickaxe, MetalKind::Gold)), BlockId::Bedrock));
        assert!(!harvests(None, BlockId::Bedrock));
        assert_eq!(block_drop(BlockId::Bedrock, Some((ToolKind::Pickaxe, MetalKind::Gold))), None);
    }

    #[test]
    fn stone_needs_a_pickaxe_and_drops_cobblestone() {
        // Bare hand: no drop.
        assert!(!harvests(None, BlockId::Stone));
        assert_eq!(block_drop(BlockId::Stone, None), None);
        // Wrong tool class: no drop.
        assert!(!harvests(Some((ToolKind::Axe, MetalKind::Iron)), BlockId::Stone));
        // Proper pickaxe (tier 1 satisfied by Copper): drops cobblestone.
        let pick = Some((ToolKind::Pickaxe, MetalKind::Copper));
        assert!(harvests(pick, BlockId::Stone));
        assert_eq!(block_drop(BlockId::Stone, pick), Some(ItemKind::Block(BlockId::Cobblestone)));
    }

    #[test]
    fn ore_tier_gating() {
        // IronOre is tier 2: a Copper pick (tier 1) fails, Iron/Gold succeed.
        assert!(!harvests(Some((ToolKind::Pickaxe, MetalKind::Copper)), BlockId::IronOre));
        assert!(harvests(Some((ToolKind::Pickaxe, MetalKind::Iron)), BlockId::IronOre));
        assert!(harvests(Some((ToolKind::Pickaxe, MetalKind::Gold)), BlockId::IronOre));
        // DiamondOre is tier 3: only Gold (tier 3) clears it.
        assert!(!harvests(Some((ToolKind::Pickaxe, MetalKind::Iron)), BlockId::DiamondOre));
        assert!(harvests(Some((ToolKind::Pickaxe, MetalKind::Gold)), BlockId::DiamondOre));
        // And it yields the ore block when properly mined.
        assert_eq!(
            block_drop(BlockId::DiamondOre, Some((ToolKind::Pickaxe, MetalKind::Gold))),
            Some(ItemKind::Block(BlockId::DiamondOre))
        );
    }

    #[test]
    fn ground_and_wood_use_their_classes() {
        // Grass: shovel → Dirt.
        let shovel = Some((ToolKind::Shovel, MetalKind::Copper));
        assert!(harvests(shovel, BlockId::Grass));
        assert_eq!(block_drop(BlockId::Grass, shovel), Some(ItemKind::Block(BlockId::Dirt)));
        // Logs: axe → the log.
        let axe = Some((ToolKind::Axe, MetalKind::Copper));
        assert!(harvests(axe, BlockId::OakLog));
        assert_eq!(block_drop(BlockId::OakLog, axe), Some(ItemKind::Block(BlockId::OakLog)));
        // A pickaxe doesn't harvest dirt.
        assert!(!harvests(Some((ToolKind::Pickaxe, MetalKind::Gold)), BlockId::Dirt));
    }

    #[test]
    fn hand_harvestable_plants_and_empty_drops() {
        // Plants break by hand and drop themselves.
        assert!(harvests(None, BlockId::Flower));
        assert_eq!(block_drop(BlockId::Flower, None), Some(ItemKind::Block(BlockId::Flower)));
        // Leaves break by hand but yield nothing (no sapling item yet).
        assert!(harvests(None, BlockId::OakLeaves));
        assert_eq!(block_drop(BlockId::OakLeaves, None), None);
        // Fluids/air yield nothing.
        assert_eq!(block_drop(BlockId::Water, None), None);
        assert_eq!(block_drop(BlockId::Air, None), None);
    }

    #[test]
    fn a_tool_ties_the_chain_together() {
        // The recipe chain forges Tool(Pickaxe, Iron); that pick should be able
        // to mine the very ore it came from.
        let iron_pick = Some((ToolKind::Pickaxe, MetalKind::Iron));
        assert!(harvests(iron_pick, MetalKind::Iron.ore()));
    }
}
