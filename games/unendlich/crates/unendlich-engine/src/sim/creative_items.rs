//! sim::creative_items — creative-menu tabs over the full item set.
//!
//! `sim::creative_tabs` covers the block palette; this widens it to the whole
//! `ItemKind` taxonomy so the creative inventory gets the tabs the design calls
//! for — including **Tools** (pickaxe/axe/shovel × metal) and **Weapons** (the
//! forged legendary classes), which were missing. It classifies each item's
//! [`ItemFacet`](crate::sim::item_category::ItemFacet) into a top-level creative
//! tab and filters a palette by tab + text search. The actual item enumeration
//! (which `ItemKind`s exist) is the caller's — it hands in `(facet, name)`
//! entries; this decides which tab each lands in and which are visible.
//! Deterministic, std-only.

use crate::sim::item_category::{BlockRole, ItemFacet};

/// Top-level creative-inventory tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CreativeCategory {
    All,
    Building,
    Natural,
    Stone,
    Ingots,
    Tools,
    Weapons,
    Creatures,
}

impl CreativeCategory {
    /// Selectable tabs in display order (All first).
    pub const ALL: [CreativeCategory; 8] = [
        CreativeCategory::All,
        CreativeCategory::Building,
        CreativeCategory::Natural,
        CreativeCategory::Stone,
        CreativeCategory::Ingots,
        CreativeCategory::Tools,
        CreativeCategory::Weapons,
        CreativeCategory::Creatures,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CreativeCategory::All => "All",
            CreativeCategory::Building => "Building",
            CreativeCategory::Natural => "Natural",
            CreativeCategory::Stone => "Stone",
            CreativeCategory::Ingots => "Ingots",
            CreativeCategory::Tools => "Tools",
            CreativeCategory::Weapons => "Weapons",
            CreativeCategory::Creatures => "Creatures",
        }
    }
}

/// Which tab an item facet belongs to.
pub fn category_of(facet: ItemFacet) -> CreativeCategory {
    match facet {
        ItemFacet::Block(BlockRole::Building) => CreativeCategory::Building,
        ItemFacet::Block(BlockRole::Ore) => CreativeCategory::Stone,
        ItemFacet::Block(BlockRole::Plant) => CreativeCategory::Natural,
        ItemFacet::Block(BlockRole::IceWater) => CreativeCategory::Natural,
        ItemFacet::Block(BlockRole::Terrain) => CreativeCategory::Natural,
        ItemFacet::Ingot => CreativeCategory::Ingots,
        ItemFacet::Tool => CreativeCategory::Tools,
        ItemFacet::Legendary => CreativeCategory::Weapons,
        ItemFacet::SpawnEgg => CreativeCategory::Creatures,
    }
}

/// Does an item on `item_cat` pass the selected `filter` tab? `All` passes all.
pub fn in_tab(item_cat: CreativeCategory, filter: CreativeCategory) -> bool {
    filter == CreativeCategory::All || filter == item_cat
}

/// Case-insensitive substring search; a blank query matches everything.
pub fn name_matches(name: &str, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    name.to_ascii_lowercase().contains(&q.to_ascii_lowercase())
}

/// One palette entry supplied by the caller.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub facet: ItemFacet,
    pub name: &'a str,
}

/// Indices of the palette entries under `filter` tab AND matching `query`, in
/// input order. `out` is cleared first (a display permutation; the palette is
/// never reordered).
pub fn visible_indices(entries: &[Entry<'_>], filter: CreativeCategory, query: &str, out: &mut Vec<usize>) {
    out.clear();
    for (i, e) in entries.iter().enumerate() {
        if in_tab(category_of(e.facet), filter) && name_matches(e.name, query) {
            out.push(i);
        }
    }
}

/// How many of `entries` fall under each tab (index-aligned with
/// [`CreativeCategory::ALL`]) — for showing counts / hiding empty tabs.
pub fn tab_counts(entries: &[Entry<'_>]) -> [usize; 8] {
    let mut counts = [0usize; 8];
    for e in entries {
        let cat = category_of(e.facet);
        counts[0] += 1; // All
        for (i, &c) in CreativeCategory::ALL.iter().enumerate() {
            if c == cat {
                counts[i] += 1;
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::item_category::{BlockRole, ItemFacet};

    fn e<'a>(facet: ItemFacet, name: &'a str) -> Entry<'a> {
        Entry { facet, name }
    }

    #[test]
    fn tools_and_weapons_get_their_own_tabs() {
        assert_eq!(category_of(ItemFacet::Tool), CreativeCategory::Tools);
        assert_eq!(category_of(ItemFacet::Legendary), CreativeCategory::Weapons);
    }

    #[test]
    fn blocks_split_and_eggs_ingots_route() {
        assert_eq!(category_of(ItemFacet::Block(BlockRole::Building)), CreativeCategory::Building);
        assert_eq!(category_of(ItemFacet::Block(BlockRole::Ore)), CreativeCategory::Stone);
        assert_eq!(category_of(ItemFacet::Block(BlockRole::Plant)), CreativeCategory::Natural);
        assert_eq!(category_of(ItemFacet::Ingot), CreativeCategory::Ingots);
        assert_eq!(category_of(ItemFacet::SpawnEgg), CreativeCategory::Creatures);
    }

    #[test]
    fn weapons_tab_shows_only_weapons() {
        let entries = [
            e(ItemFacet::Tool, "Iron Pickaxe"),
            e(ItemFacet::Legendary, "Tidal Greatsword"),
            e(ItemFacet::Legendary, "Ember Dagger"),
            e(ItemFacet::Block(BlockRole::Building), "Oak Planks"),
        ];
        let mut out = Vec::new();
        visible_indices(&entries, CreativeCategory::Weapons, "", &mut out);
        assert_eq!(out, vec![1, 2]);
    }

    #[test]
    fn tools_tab_with_search() {
        let entries = [
            e(ItemFacet::Tool, "Iron Pickaxe"),
            e(ItemFacet::Tool, "Gold Axe"),
            e(ItemFacet::Tool, "Copper Shovel"),
        ];
        let mut out = Vec::new();
        visible_indices(&entries, CreativeCategory::Tools, "", &mut out);
        assert_eq!(out.len(), 3, "all tools");
        visible_indices(&entries, CreativeCategory::Tools, "shovel", &mut out);
        assert_eq!(out, vec![2], "search narrows to the shovel");
    }

    #[test]
    fn all_tab_passes_everything() {
        let entries = [e(ItemFacet::Tool, "Iron Pickaxe"), e(ItemFacet::Legendary, "Blade")];
        let mut out = Vec::new();
        visible_indices(&entries, CreativeCategory::All, "", &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn counts_are_per_tab_plus_all() {
        let entries = [
            e(ItemFacet::Tool, "Iron Pickaxe"),
            e(ItemFacet::Tool, "Gold Axe"),
            e(ItemFacet::Legendary, "Blade"),
        ];
        let counts = tab_counts(&entries);
        assert_eq!(counts[0], 3, "All");
        let tools_i = CreativeCategory::ALL.iter().position(|&c| c == CreativeCategory::Tools).unwrap();
        let weapons_i = CreativeCategory::ALL.iter().position(|&c| c == CreativeCategory::Weapons).unwrap();
        assert_eq!(counts[tools_i], 2);
        assert_eq!(counts[weapons_i], 1);
    }

    #[test]
    fn deterministic() {
        let entries = [e(ItemFacet::Tool, "Axe"), e(ItemFacet::Tool, "Adze")];
        let mut a = Vec::new();
        let mut b = Vec::new();
        visible_indices(&entries, CreativeCategory::Tools, "a", &mut a);
        visible_indices(&entries, CreativeCategory::Tools, "a", &mut b);
        assert_eq!(a, b);
    }
}
