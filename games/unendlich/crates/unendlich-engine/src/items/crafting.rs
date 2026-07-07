//! Crafting — deterministic, shapeless recipes over the inventory.
//!
//! A [`Recipe`] is a set of required `(item, count)` inputs and one output
//! [`ItemStack`]. "Shapeless" means only the *quantities* matter, not a grid
//! arrangement — the simplest useful model; a shaped (pattern) variant can be
//! layered on later. All operations are integer-only and order-independent, so
//! crafting stays deterministic and could move into the rolled-back sim later.
//!
//! [`craft`] is transactional: it works on a clone and only commits if the
//! output fully fits, so a craft never half-consumes inputs or partially
//! places an output.

use super::{Inventory, ItemId, ItemRegistry, ItemStack};

/// A shapeless recipe: consume `inputs`, produce `output`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recipe {
    pub inputs: Vec<(ItemId, u16)>,
    pub output: ItemStack,
}

impl Recipe {
    pub fn new(inputs: Vec<(ItemId, u16)>, output: ItemStack) -> Recipe {
        Recipe { inputs, output }
    }
}

/// Does `inv` hold every input in the required quantity?
pub fn can_craft(inv: &Inventory, recipe: &Recipe) -> bool {
    recipe
        .inputs
        .iter()
        .all(|(item, n)| inv.count(*item) >= *n as u32)
}

/// Attempt to craft `recipe` from `inv`. Consumes the inputs and adds the
/// output, but only if the whole thing succeeds — including the output
/// fitting. Returns whether it crafted. Transactional: on failure `inv` is
/// untouched.
pub fn craft<R: ItemRegistry>(inv: &mut Inventory, recipe: &Recipe, reg: &R) -> bool {
    if !can_craft(inv, recipe) {
        return false;
    }
    let mut trial = inv.clone();
    for (item, n) in &recipe.inputs {
        trial.remove_item(*item, *n);
    }
    // If the output doesn't fully fit, abort without touching `inv`.
    if trial.add_item(recipe.output.item, recipe.output.count, reg) != 0 {
        return false;
    }
    *inv = trial;
    true
}

/// A shaped recipe: items must appear in a specific relative arrangement.
/// `cells` is a row-major `w × h` grid of optional ids. Matching is
/// position-independent — both the pattern and the crafting grid are trimmed to
/// their bounding box before comparison — and optionally mirror-tolerant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShapedRecipe {
    pub cells: Vec<Option<ItemId>>,
    pub w: usize,
    pub h: usize,
    pub mirror: bool,
    pub output: ItemStack,
}

impl ShapedRecipe {
    pub fn new(
        cells: Vec<Option<ItemId>>,
        w: usize,
        h: usize,
        mirror: bool,
        output: ItemStack,
    ) -> ShapedRecipe {
        debug_assert_eq!(cells.len(), w * h, "ShapedRecipe cells must be w*h");
        ShapedRecipe { cells, w, h, mirror, output }
    }

    /// Does this recipe match a crafting grid (row-major, `gw × gh`)?
    pub fn matches(&self, grid: &[Option<ItemId>], gw: usize, gh: usize) -> bool {
        let (g, tw, th) = trim(grid, gw, gh);
        let (p, ptw, pth) = trim(&self.cells, self.w, self.h);
        if (tw, th) == (ptw, pth) && g == p {
            return true;
        }
        if self.mirror && (tw, th) == (ptw, pth) {
            return g == mirror_cells(&p, ptw, pth);
        }
        false
    }

    /// Total count of each distinct ingredient, for consumption.
    pub fn ingredient_counts(&self) -> Vec<(ItemId, u16)> {
        let mut out: Vec<(ItemId, u16)> = Vec::new();
        for c in self.cells.iter().flatten().copied() {
            match out.iter_mut().find(|(it, _)| *it == c) {
                Some(e) => e.1 += 1,
                None => out.push((c, 1)),
            }
        }
        out
    }
}

/// Trim a row-major grid to the bounding box of its `Some` cells. Returns the
/// trimmed cells plus their width/height (`(_, 0, 0)` if entirely empty).
fn trim(cells: &[Option<ItemId>], w: usize, h: usize) -> (Vec<Option<ItemId>>, usize, usize) {
    if w == 0 || h == 0 {
        return (Vec::new(), 0, 0);
    }
    let occ = |r: usize, c: usize| cells.get(r * w + c).copied().flatten().is_some();
    let (mut r0, mut r1, mut c0, mut c1, mut any) = (h, 0usize, w, 0usize, false);
    for r in 0..h {
        for c in 0..w {
            if occ(r, c) {
                any = true;
                r0 = r0.min(r);
                r1 = r1.max(r);
                c0 = c0.min(c);
                c1 = c1.max(c);
            }
        }
    }
    if !any {
        return (Vec::new(), 0, 0);
    }
    let (tw, th) = (c1 - c0 + 1, r1 - r0 + 1);
    let mut out = Vec::with_capacity(tw * th);
    for r in 0..th {
        for c in 0..tw {
            out.push(cells[(r0 + r) * w + (c0 + c)]);
        }
    }
    (out, tw, th)
}

/// Mirror a row-major grid horizontally.
fn mirror_cells(cells: &[Option<ItemId>], w: usize, h: usize) -> Vec<Option<ItemId>> {
    let mut out = Vec::with_capacity(w * h);
    for r in 0..h {
        for c in 0..w {
            out.push(cells[r * w + (w - 1 - c)]);
        }
    }
    out
}

/// Craft a shaped `recipe` given the player's crafting `grid`. The pattern must
/// match the grid; ingredients are then consumed and the output added.
/// Transactional — `inv` is untouched on any failure. Until a dedicated
/// crafting-grid container exists, consumption draws from the main inventory by
/// ingredient count.
pub fn craft_shaped<R: ItemRegistry>(
    inv: &mut Inventory,
    grid: &[Option<ItemId>],
    gw: usize,
    gh: usize,
    recipe: &ShapedRecipe,
    reg: &R,
) -> bool {
    if !recipe.matches(grid, gw, gh) {
        return false;
    }
    let mut trial = inv.clone();
    for (item, n) in recipe.ingredient_counts() {
        if trial.count(item) < n as u32 {
            return false;
        }
        trial.remove_item(item, n);
    }
    if trial.add_item(recipe.output.item, recipe.output.count, reg) != 0 {
        return false;
    }
    *inv = trial;
    true
}

/// A collection of recipes, searched in insertion order.
#[derive(Clone, Default)]
pub struct RecipeBook {
    recipes: Vec<Recipe>,
    shaped: Vec<ShapedRecipe>,
}

impl RecipeBook {
    pub fn new() -> RecipeBook {
        RecipeBook { recipes: Vec::new(), shaped: Vec::new() }
    }
    pub fn add(&mut self, recipe: Recipe) {
        self.recipes.push(recipe);
    }
    /// Add a shaped (pattern) recipe.
    pub fn add_shaped(&mut self, recipe: ShapedRecipe) {
        self.shaped.push(recipe);
    }
    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }
    pub fn shaped(&self) -> &[ShapedRecipe] {
        &self.shaped
    }
    /// Index of the first recipe `inv` can currently craft.
    pub fn first_craftable(&self, inv: &Inventory) -> Option<usize> {
        self.recipes.iter().position(|r| can_craft(inv, r))
    }
    /// Indices of every currently-craftable recipe.
    pub fn craftable(&self, inv: &Inventory) -> Vec<usize> {
        (0..self.recipes.len())
            .filter(|&i| can_craft(inv, &self.recipes[i]))
            .collect()
    }
    /// Index of the first shaped recipe whose pattern matches the crafting grid.
    pub fn match_shaped(&self, grid: &[Option<ItemId>], gw: usize, gh: usize) -> Option<usize> {
        self.shaped.iter().position(|r| r.matches(grid, gw, gh))
    }
}

/// A few throwaway recipes over the default palette ids, for testing the loop.
/// (Quantities are arbitrary — this is debug content, not game balance.)
pub fn debug_recipes() -> RecipeBook {
    let mut book = RecipeBook::new();
    // 4 Dirt -> 1 Stone
    book.add(Recipe::new(vec![(ItemId(3), 4)], ItemStack::new(ItemId(1), 1)));
    // 2 Stone -> 1 Bedrock
    book.add(Recipe::new(vec![(ItemId(1), 2)], ItemStack::new(ItemId(7), 1)));
    // 1 Oak Log -> 4 Sand
    book.add(Recipe::new(vec![(ItemId(17), 1)], ItemStack::new(ItemId(12), 4)));
    // Shaped: a vertical pair of Oak Logs -> 4 Stone (debug).
    book.add_shaped(ShapedRecipe::new(
        vec![Some(ItemId(17)), Some(ItemId(17))],
        1,
        2,
        false,
        ItemStack::new(ItemId(1), 4),
    ));
    // Shaped: an L of Stone (2x2: [S,_ / S,S]) -> 1 Bedrock, mirror-tolerant.
    book.add_shaped(ShapedRecipe::new(
        vec![Some(ItemId(1)), None, Some(ItemId(1)), Some(ItemId(1))],
        2,
        2,
        true,
        ItemStack::new(ItemId(7), 1),
    ));
    book
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::UniformRegistry;

    const STONE: ItemId = ItemId(1);
    const DIRT: ItemId = ItemId(3);
    const BEDROCK: ItemId = ItemId(7);

    fn reg() -> UniformRegistry {
        UniformRegistry(64)
    }

    fn dirt_to_stone() -> Recipe {
        Recipe::new(vec![(DIRT, 4)], ItemStack::new(STONE, 1))
    }

    #[test]
    fn can_craft_checks_quantities() {
        let mut inv = Inventory::new(4, 4);
        inv.add_item(DIRT, 4, &reg());
        assert!(can_craft(&inv, &dirt_to_stone()));
        inv.remove_item(DIRT, 2); // now only 2
        assert!(!can_craft(&inv, &dirt_to_stone()));
    }

    #[test]
    fn craft_consumes_inputs_and_adds_output() {
        let mut inv = Inventory::new(4, 4);
        inv.add_item(DIRT, 10, &reg());
        assert!(craft(&mut inv, &dirt_to_stone(), &reg()));
        assert_eq!(inv.count(DIRT), 6, "4 dirt consumed");
        assert_eq!(inv.count(STONE), 1, "1 stone produced");
    }

    #[test]
    fn craft_fails_and_is_a_no_op_without_inputs() {
        let mut inv = Inventory::new(4, 4);
        inv.add_item(DIRT, 3, &reg()); // one short
        let before = inv.checksum();
        assert!(!craft(&mut inv, &dirt_to_stone(), &reg()));
        assert_eq!(inv.checksum(), before, "inventory untouched on failure");
    }

    #[test]
    fn craft_fails_when_output_has_no_room() {
        // Single slot, full of Bedrock; recipe consumes 1 Bedrock and outputs
        // Stone — which can't fit (no empty slot, different item).
        let mut inv = Inventory::new(1, 1);
        inv.add_item(BEDROCK, 64, &reg());
        let recipe = Recipe::new(vec![(BEDROCK, 1)], ItemStack::new(STONE, 1));
        let before = inv.checksum();
        assert!(!craft(&mut inv, &recipe, &reg()), "no room for output");
        assert_eq!(inv.checksum(), before, "untouched: not even the input consumed");
        assert_eq!(inv.count(BEDROCK), 64);
    }

    #[test]
    fn recipe_book_finds_craftable() {
        let book = debug_recipes(); // [dirt->stone, stone->bedrock, log->sand]
        let mut inv = Inventory::new(9, 9);
        assert_eq!(book.first_craftable(&inv), None, "empty inventory crafts nothing");
        inv.add_item(DIRT, 4, &reg());
        assert_eq!(book.first_craftable(&inv), Some(0), "dirt->stone is recipe 0");
        inv.add_item(STONE, 2, &reg());
        // Now both recipe 0 (dirt) and recipe 1 (stone) are craftable.
        assert_eq!(book.craftable(&inv), vec![0, 1]);
    }

    #[test]
    fn crafting_is_deterministic() {
        let run = || {
            let mut inv = Inventory::new(9, 9);
            inv.add_item(DIRT, 16, &reg());
            craft(&mut inv, &dirt_to_stone(), &reg());
            craft(&mut inv, &dirt_to_stone(), &reg());
            inv.checksum()
        };
        assert_eq!(run(), run());
    }

    const LOG: ItemId = ItemId(17);

    fn grid(gw: usize, gh: usize, placed: &[(usize, usize, ItemId)]) -> Vec<Option<ItemId>> {
        let mut g = vec![None; gw * gh];
        for &(r, c, it) in placed {
            g[r * gw + c] = Some(it);
        }
        g
    }

    #[test]
    fn shaped_match_is_position_independent() {
        let r = ShapedRecipe::new(vec![Some(LOG), Some(LOG)], 1, 2, false, ItemStack::new(STONE, 4));
        assert!(r.matches(&grid(3, 3, &[(0, 0, LOG), (1, 0, LOG)]), 3, 3), "top-left");
        assert!(r.matches(&grid(3, 3, &[(1, 1, LOG), (2, 1, LOG)]), 3, 3), "shifted");
        assert!(!r.matches(&grid(3, 3, &[(1, 1, LOG), (1, 2, LOG)]), 3, 3), "horizontal is wrong shape");
    }

    #[test]
    fn shaped_mirror_gating() {
        let cells = vec![Some(STONE), None, Some(STONE), Some(STONE)]; // L
        let exact = ShapedRecipe::new(cells.clone(), 2, 2, false, ItemStack::new(BEDROCK, 1));
        let mirr = ShapedRecipe::new(cells, 2, 2, true, ItemStack::new(BEDROCK, 1));
        let flipped = grid(3, 3, &[(0, 1, STONE), (1, 0, STONE), (1, 1, STONE)]);
        assert!(!exact.matches(&flipped, 3, 3), "mirror off rejects flipped");
        assert!(mirr.matches(&flipped, 3, 3), "mirror on accepts flipped");
    }

    #[test]
    fn shaped_ingredient_counts() {
        let r = ShapedRecipe::new(
            vec![Some(STONE), None, Some(STONE), Some(LOG)],
            2, 2, false, ItemStack::new(BEDROCK, 1),
        );
        let mut counts = r.ingredient_counts();
        counts.sort();
        let mut expect = vec![(STONE, 2u16), (LOG, 1u16)];
        expect.sort();
        assert_eq!(counts, expect);
    }

    #[test]
    fn craft_shaped_consumes_and_is_transactional() {
        let r = ShapedRecipe::new(vec![Some(LOG), Some(LOG)], 1, 2, false, ItemStack::new(STONE, 4));
        let g = grid(3, 3, &[(0, 0, LOG), (1, 0, LOG)]);
        let mut inv = Inventory::new(9, 9);
        inv.add_item(LOG, 5, &reg());
        assert!(craft_shaped(&mut inv, &g, 3, 3, &r, &reg()));
        assert_eq!(inv.count(LOG), 3, "2 logs consumed");
        assert_eq!(inv.count(STONE), 4, "4 stone produced");
        let before = inv.checksum();
        let bad = grid(3, 3, &[(0, 0, LOG), (0, 1, LOG)]); // horizontal -> no match
        assert!(!craft_shaped(&mut inv, &bad, 3, 3, &r, &reg()));
        assert_eq!(inv.checksum(), before, "no-op on non-matching grid");
    }

    #[test]
    fn debug_book_exposes_shaped() {
        let book = debug_recipes();
        let g = grid(3, 3, &[(0, 0, ItemId(17)), (1, 0, ItemId(17))]);
        assert_eq!(book.match_shaped(&g, 3, 3), Some(0), "log pair is shaped recipe 0");
    }
}
