//! `items::recipe_build` — the glue the r740/r741 notes flagged: turn the
//! id-agnostic `recipe_gen` output (`GenRecipe`, whose ids are bare `u32`) into
//! the game's real `crafting::RecipeBook` (`Recipe` / `ShapedRecipe`, whose ids
//! are the `items::ItemId` newtype). `recipe_gen` stays decoupled from
//! `crafting`; THIS module is the caller-side adapter, and it is where the
//! documented "adapt to `items::ItemId` at the call site" conversion happens.
//! It also holds the bespoke "exotic-bench" recipes (elemental staves, living
//! weapons, parkour grippers) that layer on the auto-generated material tiers.
//!
//! Pure and id-agnostic: exotic builders take bare `u32` ids (matching
//! `recipe_gen`); the adapter wraps them into `items::ItemId` when emitting
//! crafting recipes.

use crate::items::crafting::{Recipe, RecipeBook, ShapedRecipe};
use crate::items::recipe_gen::{material_family, GenRecipe, MaterialSpec, Station};
use crate::items::{ItemId, ItemStack};

/// Wrap a `recipe_gen` bare id into the crafting `ItemId` newtype.
#[inline]
fn wrap(id: u32) -> ItemId {
    ItemId(id)
}

/// Convert one generated recipe into the crafting form and file it in `book`.
/// `shape == Some` → shaped recipe (row-major grid); `None` → shapeless. The
/// `u32 → ItemId` conversion happens right here at the boundary.
pub fn add_gen(book: &mut RecipeBook, g: &GenRecipe) {
    let output = ItemStack::new(wrap(g.output.0), g.output.1);
    match &g.shape {
        Some((cells, w, h)) => {
            let cells: Vec<Option<ItemId>> = cells.iter().map(|c| c.map(wrap)).collect();
            book.add_shaped(ShapedRecipe::new(cells, *w, *h, false, output));
        }
        None => {
            let inputs: Vec<(ItemId, u16)> = g.inputs.iter().map(|&(i, n)| (wrap(i), n)).collect();
            book.add(Recipe::new(inputs, output));
        }
    }
}

/// Build a fresh `RecipeBook` from a batch of generated recipes.
pub fn to_recipe_book(gens: &[GenRecipe]) -> RecipeBook {
    let mut book = RecipeBook::new();
    for g in gens {
        add_gen(&mut book, g);
    }
    book
}

/// The standard book: every material tier's family (`material_family` per spec)
/// plus any `extra` bespoke recipes, all in one `RecipeBook`.
pub fn standard_book(specs: &[MaterialSpec], extra: &[GenRecipe]) -> RecipeBook {
    let mut book = RecipeBook::new();
    for m in specs {
        for g in material_family(m) {
            add_gen(&mut book, &g);
        }
    }
    for g in extra {
        add_gen(&mut book, g);
    }
    book
}

// ── Exotic-bench recipes (bespoke, layered on the material tiers) ─────────────
// These build `GenRecipe`s, so their ids are bare `u32` like the rest of
// `recipe_gen`. `None`/unknown pattern chars are empty cells.

/// The flat ingredient multiset of a shaped grid (mirrors `recipe_gen`'s own
/// shaped constructor, which callers/tests may read for a tally).
fn cells_to_inputs(cells: &[Option<u32>]) -> Vec<(u32, u16)> {
    let mut inputs: Vec<(u32, u16)> = Vec::new();
    for c in cells.iter().flatten() {
        match inputs.iter_mut().find(|(i, _)| i == c) {
            Some((_, n)) => *n += 1,
            None => inputs.push((*c, 1)),
        }
    }
    inputs
}

/// A 3×3 shaped recipe from a compact ASCII pattern + a legend, targeting
/// `output`. `pattern` is 3 rows of 3 chars; `' '` = empty; any other char is
/// looked up in `legend` (unknown char → empty). Row-major, 3×3.
pub fn shaped_3x3(pattern: [&str; 3], legend: &[(char, u32)], output: (u32, u16)) -> GenRecipe {
    let mut cells: Vec<Option<u32>> = Vec::with_capacity(9);
    for row in pattern.iter() {
        let chars: Vec<char> = row.chars().collect();
        for c in 0..3 {
            let ch = chars.get(c).copied().unwrap_or(' ');
            let id = if ch == ' ' {
                None
            } else {
                legend.iter().find(|(k, _)| *k == ch).map(|(_, v)| *v)
            };
            cells.push(id);
        }
    }
    GenRecipe {
        station: Station::Bench,
        inputs: cells_to_inputs(&cells),
        shape: Some((cells, 3, 3)),
        output,
    }
}

/// Elemental stave: a focus `gem` atop a two-piece `shaft` (` G `/` S `/` S `).
pub fn elemental_stave(gem: u32, shaft: u32, stave: u32) -> GenRecipe {
    shaped_3x3([" G ", " S ", " S "], &[('G', gem), ('S', shaft)], (stave, 1))
}

/// Living weapon: a `core` cradled by a `frame` (`FFF`/`FCF`/`FFF`).
pub fn living_weapon(core: u32, frame: u32, weapon: u32) -> GenRecipe {
    shaped_3x3(["FFF", "FCF", "FFF"], &[('F', frame), ('C', core)], (weapon, 1))
}

/// Parkour grippers: `pad`s over a `binding` pair (`P P`/`B B`).
pub fn parkour_grippers(pad: u32, binding: u32, grippers: u32) -> GenRecipe {
    shaped_3x3(["P P", "B B", "   "], &[('P', pad), ('B', binding)], (grippers, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapeless_gen_becomes_shapeless_recipe() {
        let g = GenRecipe {
            station: Station::Furnace,
            inputs: vec![(10, 1)],
            shape: None,
            output: (11, 1),
        };
        let book = to_recipe_book(&[g]);
        assert_eq!(book.recipes().len(), 1);
        assert_eq!(book.shaped().len(), 0);
        assert_eq!(book.recipes()[0].output.item, ItemId(11));
        assert_eq!(book.recipes()[0].inputs, vec![(ItemId(10), 1)]);
    }

    #[test]
    fn shaped_gen_becomes_shaped_recipe() {
        let book = to_recipe_book(&[elemental_stave(100, 101, 102)]);
        assert_eq!(book.shaped().len(), 1);
        assert_eq!(book.recipes().len(), 0);
        let s = &book.shaped()[0];
        assert_eq!((s.w, s.h), (3, 3));
        assert_eq!(s.output.item, ItemId(102));
        assert_eq!(s.cells[1], Some(ItemId(100)));
        assert_eq!(s.cells[4], Some(ItemId(101)));
        assert_eq!(s.cells[7], Some(ItemId(101)));
        assert_eq!(s.cells[0], None);
    }

    #[test]
    fn standard_book_merges_family_and_extras() {
        let spec = MaterialSpec {
            ore: Some(1),
            ingot: Some(2),
            block: Some(3),
            stick: None,
            pickaxe: None,
            axe: None,
            shovel: None,
            sword: None,
            hoe: None,
        };
        // material_family(ore/ingot/block) = smelt(shapeless) + pack9(shaped) +
        // unpack9(shapeless) = 2 shapeless + 1 shaped; plus one stave (shaped).
        let extra = [elemental_stave(50, 51, 52)];
        let book = standard_book(&[spec], &extra);
        assert_eq!(book.recipes().len(), 2, "smelt + unpack (shapeless)");
        assert_eq!(book.shaped().len(), 2, "pack9 + the stave");
    }

    #[test]
    fn living_weapon_frames_the_core() {
        let g = living_weapon(7, 8, 9);
        if let Some((cells, w, h)) = g.shape {
            assert_eq!((w, h), (3, 3));
            assert_eq!(cells[4], Some(7), "core in the centre");
            assert_eq!(cells.iter().filter(|c| **c == Some(8)).count(), 8, "frame fills the ring");
        } else {
            panic!("living weapon must be shaped");
        }
    }

    #[test]
    fn unknown_pattern_chars_are_empty() {
        let g = shaped_3x3([" ? ", "   ", "   "], &[('X', 1)], (2, 1));
        if let Some((cells, _, _)) = g.shape {
            assert!(cells.iter().all(|c| c.is_none()));
        } else {
            panic!();
        }
    }
}
