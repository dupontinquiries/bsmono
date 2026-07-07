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
        | MossyCobblestone | PolishedStone | PolishedBasalt
        | Selenite | Gypsum | Dolomite | Sulfur | Magma => {
            T::Stone
        }
        GoldOre | IronOre | CoalOre | CopperOre | DiamondOre | Quartz | Amber | Crystal | Bone
        | GlowCrystal => {
            T::Ores
        }
        OakLog | OakLeaves | BirchLog | BirchLeaves | OakPlanks | Bookshelf | DarkOakPlanks => {
            T::Wood
        }
        StoneBricks | Bricks | Glass | IronBlock | GoldBlock | CoalBlock | Pumpkin
        | SmoothSandstone | Terracotta | ChiseledStoneBricks => T::Building,
        Flower | TallGrass | Fern | DeadBush | Vine | Cactus | Mushroom | Cobweb | Lichen
        | Carrot | Potato | Cabbage | Sprout => {
            T::Plants
        }
    })
}

/// The blocks in a tab, in canonical [`BlockId::ALL`] order.
pub fn tab_items(t: CreativeTab) -> Vec<BlockId> {
    BlockId::ALL.iter().copied().filter(|&b| tab_of(b) == Some(t)).collect()
}

// ── search (r604) ────────────────────────────────────────────────────────────
// The creative menu was missing a search page. These add a case-insensitive
// name search over a tab, or across every placeable block, so the UI can offer
// a search box. `name_of` maps a block to its display name (the bin already has
// this), keeping this module decoupled from naming.

/// Case-insensitive substring match for a creative search box. A blank query
/// matches everything. ASCII-lowercasing keeps it allocation-light and
/// locale-independent.
pub fn name_matches(name: &str, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    name.to_ascii_lowercase().contains(&q.to_ascii_lowercase())
}

/// A tab's blocks whose display name matches `query` (tab + search combined).
pub fn tab_items_matching(t: CreativeTab, query: &str, name_of: impl Fn(BlockId) -> String) -> Vec<BlockId> {
    tab_items(t).into_iter().filter(|&b| name_matches(&name_of(b), query)).collect()
}

/// Every placeable block whose name matches `query`, tab-independent — the
/// global search page. Preserves `BlockId::ALL` order and excludes `Air`.
pub fn search_all(query: &str, name_of: impl Fn(BlockId) -> String) -> Vec<BlockId> {
    BlockId::ALL
        .iter()
        .copied()
        .filter(|&b| tab_of(b).is_some() && name_matches(&name_of(b), query))
        .collect()
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

    #[test]
    fn search_is_case_insensitive_and_blank_matches_all() {
        assert!(name_matches("Oak Planks", "oak"));
        assert!(name_matches("Oak Planks", "PLANK"));
        assert!(!name_matches("Oak Planks", "stone"));
        assert!(name_matches("anything at all", "   "), "blank query matches everything");
        assert!(name_matches("Mossy Cobblestone", "cobble"));
    }

    #[test]
    fn tab_search_narrows_within_a_tab() {
        // Uses the real BlockId Debug name as a stand-in for display names.
        let hits = tab_items_matching(CreativeTab::Stone, "stone", |b| format!("{b:?}"));
        for b in &hits {
            assert!(format!("{b:?}").to_ascii_lowercase().contains("stone"), "{b:?} lacks 'stone'");
            assert_eq!(tab_of(*b), Some(CreativeTab::Stone));
        }
    }
}
