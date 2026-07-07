//! sim::item_category — classify items into inventory filter categories.
//!
//! The crafting/inventory screen (design_library/06) has a category-filter
//! sidebar, and `sim::inventory_sort` already sorts by a `category: u8` — but
//! nothing *produces* that category from an item. This is that classifier,
//! grounded in the real `ItemKind` taxonomy (`Block | SpawnEgg | Ingot | Tool |
//! Legendary`) rather than the aspirational content list, so it's honest about
//! what exists today and extends as content lands.
//!
//! To stay std-only and unit-testable it takes a compact facet the caller maps
//! `ItemKind` onto in one line — [`ItemFacet`] — instead of importing the big
//! `ItemKind`/`BlockId` enums. `category` returns a stable [`Category`]; its
//! `u8` ordering is the sort key `inventory_sort` wants, and `matches` powers
//! the sidebar filter.

/// The coarse class of an item, mirroring `ItemKind`'s shape. The caller builds
/// this from an `ItemKind` (and, for blocks, a `BlockRole` derived from
/// `BlockId` predicates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemFacet {
    Block(BlockRole),
    /// A creature spawn egg.
    SpawnEgg,
    /// A smelted metal bar.
    Ingot,
    /// A finished working tool (pickaxe/axe/shovel).
    Tool,
    /// A forged legendary — treated as a weapon.
    Legendary,
}

/// What kind of block this is, from `BlockId`'s own predicates (`is_fluid`,
/// `is_foliage_colored`, ore membership, …). Only meaningful inside
/// [`ItemFacet::Block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRole {
    /// Crafted/shaped building material (planks, bricks, glass, stone).
    Building,
    /// Ore or raw mineral block.
    Ore,
    /// Plant/foliage block (leaves, grass, crops, coral).
    Plant,
    /// Ice or water-adjacent material.
    IceWater,
    /// Anything else (dirt, sand, gravel, misc terrain).
    Terrain,
}

/// The inventory filter categories. `u8` order is the display/sort order and the
/// stable key handed to [`crate::sim::inventory_sort`]. Kept intentionally close
/// to the design's sidebar while staying faithful to today's item set; new
/// variants (gems, bending instruments, consumables) slot in as that content
/// gains real `ItemKind`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Category {
    Building = 0,
    Terrain = 1,
    Plant = 2,
    IceWater = 3,
    RawMaterial = 4,
    Tool = 5,
    Weapon = 6,
    Creature = 7,
}

impl Category {
    /// Every category, in display order.
    pub const ALL: [Category; 8] = [
        Category::Building,
        Category::Terrain,
        Category::Plant,
        Category::IceWater,
        Category::RawMaterial,
        Category::Tool,
        Category::Weapon,
        Category::Creature,
    ];

    /// The stable sort/filter key (feeds `inventory_sort::Slot.category`).
    pub fn key(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Building => "Building",
            Category::Terrain => "Terrain",
            Category::Plant => "Flora",
            Category::IceWater => "Ice & Water",
            Category::RawMaterial => "Materials",
            Category::Tool => "Tools",
            Category::Weapon => "Weapons",
            Category::Creature => "Creatures",
        }
    }
}

/// Classify an item facet into its inventory [`Category`].
pub fn category(facet: ItemFacet) -> Category {
    match facet {
        ItemFacet::Block(role) => match role {
            BlockRole::Building => Category::Building,
            BlockRole::Ore => Category::RawMaterial,
            BlockRole::Plant => Category::Plant,
            BlockRole::IceWater => Category::IceWater,
            BlockRole::Terrain => Category::Terrain,
        },
        ItemFacet::Ingot => Category::RawMaterial,
        ItemFacet::Tool => Category::Tool,
        ItemFacet::Legendary => Category::Weapon,
        ItemFacet::SpawnEgg => Category::Creature,
    }
}

/// Does an item's category pass the active sidebar filter? `None` = "All" (no
/// filter), so everything shows.
pub fn matches(item: Category, active: Option<Category>) -> bool {
    active.map_or(true, |a| a == item)
}

/// Indices of the facets whose category passes `active`, in input order (a
/// display permutation; slot data is never moved). `out` is cleared first.
pub fn filter_indices(facets: &[ItemFacet], active: Option<Category>, out: &mut Vec<usize>) {
    out.clear();
    for (i, &f) in facets.iter().enumerate() {
        if matches(category(f), active) {
            out.push(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facets_map_to_expected_categories() {
        assert_eq!(category(ItemFacet::Block(BlockRole::Building)), Category::Building);
        assert_eq!(category(ItemFacet::Block(BlockRole::Ore)), Category::RawMaterial);
        assert_eq!(category(ItemFacet::Block(BlockRole::Plant)), Category::Plant);
        assert_eq!(category(ItemFacet::Block(BlockRole::IceWater)), Category::IceWater);
        assert_eq!(category(ItemFacet::Block(BlockRole::Terrain)), Category::Terrain);
        assert_eq!(category(ItemFacet::Ingot), Category::RawMaterial);
        assert_eq!(category(ItemFacet::Tool), Category::Tool);
        assert_eq!(category(ItemFacet::Legendary), Category::Weapon);
        assert_eq!(category(ItemFacet::SpawnEgg), Category::Creature);
    }

    #[test]
    fn ore_blocks_and_ingots_share_the_raw_material_bucket() {
        assert_eq!(category(ItemFacet::Block(BlockRole::Ore)), category(ItemFacet::Ingot));
    }

    #[test]
    fn category_keys_are_stable_and_ordered() {
        // key() == display order == the inventory_sort category key.
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(c.key() as usize, i, "{c:?} key should equal its ALL index");
        }
        // strictly increasing
        for w in Category::ALL.windows(2) {
            assert!(w[0].key() < w[1].key());
        }
    }

    #[test]
    fn the_all_filter_shows_everything() {
        for &c in &Category::ALL {
            assert!(matches(c, None), "None filter must pass {c:?}");
        }
    }

    #[test]
    fn an_active_filter_keeps_only_its_category() {
        assert!(matches(Category::Weapon, Some(Category::Weapon)));
        assert!(!matches(Category::Weapon, Some(Category::Tool)));
    }

    #[test]
    fn filter_indices_selects_the_right_slots_in_order() {
        let facets = [
            ItemFacet::Block(BlockRole::Building), // 0 Building
            ItemFacet::Tool,                       // 1 Tool
            ItemFacet::Legendary,                  // 2 Weapon
            ItemFacet::Ingot,                      // 3 RawMaterial
            ItemFacet::Tool,                       // 4 Tool
        ];
        let mut out = Vec::new();
        filter_indices(&facets, Some(Category::Tool), &mut out);
        assert_eq!(out, vec![1, 4], "only tools, in input order");

        filter_indices(&facets, None, &mut out);
        assert_eq!(out, vec![0, 1, 2, 3, 4], "None ⇒ all slots");

        filter_indices(&facets, Some(Category::Creature), &mut out);
        assert!(out.is_empty(), "no eggs present");
    }

    #[test]
    fn classification_is_deterministic() {
        let f = ItemFacet::Block(BlockRole::Ore);
        assert_eq!(category(f), category(f));
    }
}
