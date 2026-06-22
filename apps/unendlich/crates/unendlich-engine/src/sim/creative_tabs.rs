//! `sim::creative_tabs` (r454) — the creative palette's tab taxonomy.
//!
//! Groups every placeable [`BlockId`] into a small set of category tabs (Terrain, Stone, Ores,
//! Wood, Building, Plants) so a creative-mode player can browse blocks by kind instead of scrolling
//! one flat list. Pure + host-agnostic: [`tab_of`] classifies a block, [`tab_items`] lists a tab's
//! blocks in canonical [`BlockId::ALL`] order, and [`ALL_TABS`]/[`label`] drive the UI. The bin owns
//! the actual browsing keys + placement; this is just the data model, so it is fully testable.
//!
//! Invariant (tested): the tabs PARTITION `BlockId::ALL` minus `Air` — every placeable block is in
//! exactly one tab, nothing lost or duplicated. Add a new BlockId ⇒ add it to a tab arm or the
//! partition test fails.

use crate::world::blocks::BlockId;

/// A creative-palette category tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CreativeTab {
    Terrain,
    Stone,
    Ores,
    Wood,
    Building,
    Plants,
}

/// All tabs in display order.
pub const ALL_TABS: [CreativeTab; 6] = [
    CreativeTab::Terrain,
    CreativeTab::Stone,
    CreativeTab::Ores,
    CreativeTab::Wood,
    CreativeTab::Building,
    CreativeTab::Plants,
];

/// Human-readable tab label for the UI.
pub fn label(t: CreativeTab) -> &'static str {
    match t {
        CreativeTab::Terrain => "Terrain",
        CreativeTab::Stone => "Stone",
        CreativeTab::Ores => "Ores & Gems",
        CreativeTab::Wood => "Wood",
        CreativeTab::Building => "Building",
        CreativeTab::Plants => "Plants",
    }
}

/// Which tab a block belongs to. `None` for `Air` (not a placeable palette entry).
pub fn tab_of(b: BlockId) -> Option<CreativeTab> {
    use BlockId::*;
    use CreativeTab as T;
    Some(match b {
        Air => return None,
        Grass | Dirt | Sand | Gravel | Clay | Snow | SnowBlock | Moss | Water | Lava | Ice => {
            T::Terrain
        }
        Stone | Cobblestone | Bedrock | RedSandstone | Limestone | Marble | Basalt | Tuff
        | Granite | Slate | Calcite | Andesite | Schist | Sandstone | Obsidian | Dripstone
        | MossyCobblestone | PolishedStone | PolishedBasalt => {
            T::Stone
        }
        GoldOre | IronOre | CoalOre | CopperOre | DiamondOre | Quartz | Amber | Crystal | Bone => {
            T::Ores
        }
        OakLog | OakLeaves | BirchLog | BirchLeaves | OakPlanks | Bookshelf | DarkOakPlanks => {
            T::Wood
        }
        StoneBricks | Bricks | Glass | IronBlock | GoldBlock | CoalBlock | Pumpkin
        | SmoothSandstone | Terracotta | ChiseledStoneBricks => T::Building,
        Flower | TallGrass | Fern | DeadBush | Vine | Cactus | Mushroom | Cobweb | Lichen => {
            T::Plants
        }
    })
}

/// The blocks in a tab, in canonical [`BlockId::ALL`] order.
pub fn tab_items(t: CreativeTab) -> Vec<BlockId> {
    BlockId::ALL.iter().copied().filter(|&b| tab_of(b) == Some(t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_block_except_air_has_one_tab() {
        for b in BlockId::ALL {
            if b == BlockId::Air {
                assert_eq!(tab_of(b), None);
            } else {
                assert!(tab_of(b).is_some(), "{b:?} has no tab");
            }
        }
    }

    #[test]
    fn tabs_partition_all_blocks() {
        let mut seen = 0usize;
        for t in ALL_TABS {
            let items = tab_items(t);
            assert!(!items.is_empty(), "{:?} empty", label(t));
            seen += items.len();
        }
        assert_eq!(seen, BlockId::ALL.len() - 1, "tabs must partition ALL minus Air");
    }

    #[test]
    fn no_duplicates_across_tabs() {
        let mut all: Vec<BlockId> = Vec::new();
        for t in ALL_TABS {
            all.extend(tab_items(t));
        }
        let mut ids: Vec<u8> = all.iter().map(|b| *b as u8).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "a block appears in two tabs");
    }

    #[test]
    fn labels_present() {
        for t in ALL_TABS {
            assert!(!label(t).is_empty());
        }
    }
}
