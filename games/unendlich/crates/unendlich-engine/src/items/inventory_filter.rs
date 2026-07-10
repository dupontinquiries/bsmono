//! `items::inventory_filter` — the survival crafting-screen category sidebar.
//!
//! From `docs/design_library/08_UI_and_UX/crafting_inventory_layout.docx`: the
//! survival inventory screen has a left sidebar of **eight category filter buttons**
//! that repopulate the item grid to a single category. The existing
//! `sim::creative_items::CreativeCategory` is for *creative* block placement
//! (Building/Natural/Stone/…) and does not match this survival taxonomy, so this is
//! its own classifier.
//!
//! The eight categories, in sidebar order (matching the doc's glyphs):
//!   1. ◆ Gems & Crystals      5. ❄ Ice & Water Materials
//!   2. 🌿 Flora & Reagents     6. 🍃 Raw Materials & Components
//!   3. ✦ Bending Instruments   7. ⊙ Weapons
//!   4. 🔨 Equipment & Armour    8. 👝 Consumables & Draughts
//!
//! Classification is by a small [`ItemFacets`] descriptor the caller derives from a
//! real item, so this module stays pure and independent of the concrete item enum.
//! An item can plausibly satisfy several facets (a gem-hilted sword is a weapon and
//! a gem); [`category_of`] resolves that with a fixed priority taken from the doc —
//! notably Weapons is category 7 and is *excluded* from Equipment (category 4).

/// The eight survival inventory categories, in sidebar display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvCategory {
    Gems,
    Flora,
    BendingInstruments,
    Equipment,
    IceWater,
    RawMaterials,
    Weapons,
    Consumables,
}

impl InvCategory {
    /// All eight, in sidebar order.
    pub const ALL: [InvCategory; 8] = [
        InvCategory::Gems,
        InvCategory::Flora,
        InvCategory::BendingInstruments,
        InvCategory::Equipment,
        InvCategory::IceWater,
        InvCategory::RawMaterials,
        InvCategory::Weapons,
        InvCategory::Consumables,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InvCategory::Gems => "Gems & Crystals",
            InvCategory::Flora => "Flora & Reagents",
            InvCategory::BendingInstruments => "Bending Instruments",
            InvCategory::Equipment => "Equipment & Armour",
            InvCategory::IceWater => "Ice & Water",
            InvCategory::RawMaterials => "Raw Materials",
            InvCategory::Weapons => "Weapons",
            InvCategory::Consumables => "Consumables",
        }
    }

    /// The sidebar glyph from the layout doc.
    pub fn glyph(self) -> char {
        match self {
            InvCategory::Gems => '◆',
            InvCategory::Flora => '🌿',
            InvCategory::BendingInstruments => '✦',
            InvCategory::Equipment => '🔨',
            InvCategory::IceWater => '❄',
            InvCategory::RawMaterials => '🍃',
            InvCategory::Weapons => '⊙',
            InvCategory::Consumables => '👝',
        }
    }

    /// Sidebar slot index (0-based), matching the doc's numbering 1..8.
    pub fn slot(self) -> usize {
        InvCategory::ALL.iter().position(|&c| c == self).unwrap()
    }
}

/// Facets of an item used to sort it into a category. The caller sets whichever are
/// true for a given item; all false → [`InvCategory::RawMaterials`] (the catch-all).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemFacets {
    pub is_gem: bool,
    pub is_flora: bool,
    pub is_bending_focus: bool,
    pub is_armour: bool,
    pub is_ice_water: bool,
    pub is_weapon: bool,
    pub is_consumable: bool,
}

/// Classify an item into exactly one category. Priority (highest first) follows the
/// doc: a weapon is category 7 even if it is also a gem/armour; consumables and the
/// specialised material groups outrank the generic Raw Materials catch-all.
pub fn category_of(f: &ItemFacets) -> InvCategory {
    if f.is_weapon {
        InvCategory::Weapons
    } else if f.is_armour {
        InvCategory::Equipment
    } else if f.is_bending_focus {
        InvCategory::BendingInstruments
    } else if f.is_consumable {
        InvCategory::Consumables
    } else if f.is_gem {
        InvCategory::Gems
    } else if f.is_ice_water {
        InvCategory::IceWater
    } else if f.is_flora {
        InvCategory::Flora
    } else {
        InvCategory::RawMaterials
    }
}

/// Filter a slice of items to those in `category`, preserving order. `facet_of`
/// derives the facets for each item. Returns indices so the caller can map back to
/// its own slots.
pub fn filter_indices<T>(
    items: &[T],
    category: InvCategory,
    facet_of: impl Fn(&T) -> ItemFacets,
) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, it)| category_of(&facet_of(it)) == category)
        .map(|(i, _)| i)
        .collect()
}

/// Count how many items fall in each category (sidebar badge counts), indexed by
/// [`InvCategory::slot`].
pub fn category_counts<T>(items: &[T], facet_of: impl Fn(&T) -> ItemFacets) -> [usize; 8] {
    let mut counts = [0usize; 8];
    for it in items {
        counts[category_of(&facet_of(it)).slot()] += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gem() -> ItemFacets { ItemFacets { is_gem: true, ..Default::default() } }
    fn weapon() -> ItemFacets { ItemFacets { is_weapon: true, ..Default::default() } }
    fn armour() -> ItemFacets { ItemFacets { is_armour: true, ..Default::default() } }
    fn food() -> ItemFacets { ItemFacets { is_consumable: true, ..Default::default() } }
    fn raw() -> ItemFacets { ItemFacets::default() }

    #[test]
    fn eight_categories_in_order() {
        assert_eq!(InvCategory::ALL.len(), 8);
        assert_eq!(InvCategory::Gems.slot(), 0);
        assert_eq!(InvCategory::Consumables.slot(), 7);
    }

    #[test]
    fn raw_is_the_catch_all() {
        assert_eq!(category_of(&raw()), InvCategory::RawMaterials);
    }

    #[test]
    fn weapon_outranks_gem_and_armour() {
        // A gem-hilted, armour-classed weapon is still filed under Weapons (cat 7).
        let f = ItemFacets { is_weapon: true, is_gem: true, is_armour: true, ..Default::default() };
        assert_eq!(category_of(&f), InvCategory::Weapons);
    }

    #[test]
    fn armour_is_equipment_not_weapon() {
        assert_eq!(category_of(&armour()), InvCategory::Equipment);
    }

    #[test]
    fn each_facet_maps_to_its_category() {
        assert_eq!(category_of(&gem()), InvCategory::Gems);
        assert_eq!(category_of(&food()), InvCategory::Consumables);
        assert_eq!(
            category_of(&ItemFacets { is_flora: true, ..Default::default() }),
            InvCategory::Flora
        );
        assert_eq!(
            category_of(&ItemFacets { is_bending_focus: true, ..Default::default() }),
            InvCategory::BendingInstruments
        );
        assert_eq!(
            category_of(&ItemFacets { is_ice_water: true, ..Default::default() }),
            InvCategory::IceWater
        );
    }

    #[test]
    fn filter_and_counts() {
        let items = vec![gem(), weapon(), gem(), food(), raw()];
        let gems = filter_indices(&items, InvCategory::Gems, |f| *f);
        assert_eq!(gems, vec![0, 2]);
        let counts = category_counts(&items, |f| *f);
        assert_eq!(counts[InvCategory::Gems.slot()], 2);
        assert_eq!(counts[InvCategory::Weapons.slot()], 1);
        assert_eq!(counts[InvCategory::Consumables.slot()], 1);
        assert_eq!(counts[InvCategory::RawMaterials.slot()], 1);
    }
}
