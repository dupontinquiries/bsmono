//! crafting — match a 3×3 crafting grid against a recipe table and return the
//! output stack. Two recipe shapes, both position-independent the way players
//! expect:
//!
//! - **Shaped**: the recipe is a minimal pattern (its bounding box). A grid
//!   matches if the items it actually contains, cropped to *their* bounding box,
//!   line up cell-for-cell with the pattern — so a 2×1 recipe crafts whether you
//!   place it top-left or bottom-right, but a gap between the items changes the
//!   shape and won't match.
//! - **Shapeless**: only the multiset of ingredients matters; arrangement is
//!   ignored entirely.
//!
//! [`RecipeTable::craft`] returns the first matching recipe's output, so table
//! order is the tie-break. Pure and rustc-testable in isolation (R1): the grid
//! and recipe types are self-contained, no item config dependency. The match
//! semantics are pinned by `prop_*` tests.

/// Item kind id (see `inventory_grid::ItemId` — kept separate so this module
/// compiles standalone; a shared item type is a later unification).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u16);

/// A produced stack: a kind and a count.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemStack {
    pub id: ItemId,
    pub count: u32,
}

impl ItemStack {
    pub fn new(id: ItemId, count: u32) -> ItemStack {
        ItemStack { id, count }
    }
}

/// Side length of the (square) crafting grid.
pub const GRID: usize = 3;

/// A 3×3 crafting input. `None` is an empty cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CraftGrid {
    cells: [[Option<ItemId>; GRID]; GRID],
}

impl Default for CraftGrid {
    fn default() -> CraftGrid {
        CraftGrid::empty()
    }
}

impl CraftGrid {
    pub fn empty() -> CraftGrid {
        CraftGrid {
            cells: [[None; GRID]; GRID],
        }
    }

    /// Set a cell; out-of-range coordinates are ignored.
    pub fn set(&mut self, row: usize, col: usize, id: Option<ItemId>) {
        if row < GRID && col < GRID {
            self.cells[row][col] = id;
        }
    }

    pub fn get(&self, row: usize, col: usize) -> Option<ItemId> {
        if row < GRID && col < GRID {
            self.cells[row][col]
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cells
            .iter()
            .all(|r| r.iter().all(|c| c.is_none()))
    }

    /// The occupied bounding box as `(top, left, bottom, right)` inclusive, or
    /// `None` if the grid is empty.
    fn bounding_box(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut top, mut left, mut bottom, mut right) = (GRID, GRID, 0usize, 0usize);
        let mut any = false;
        for r in 0..GRID {
            for c in 0..GRID {
                if self.cells[r][c].is_some() {
                    any = true;
                    top = top.min(r);
                    left = left.min(c);
                    bottom = bottom.max(r);
                    right = right.max(c);
                }
            }
        }
        if any {
            Some((top, left, bottom, right))
        } else {
            None
        }
    }

    /// Sorted multiset of all occupied item ids (for shapeless matching).
    fn ingredient_multiset(&self) -> Vec<u16> {
        let mut v: Vec<u16> = Vec::new();
        for r in 0..GRID {
            for c in 0..GRID {
                if let Some(id) = self.cells[r][c] {
                    v.push(id.0);
                }
            }
        }
        v.sort_unstable();
        v
    }
}

/// A crafting recipe.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Recipe {
    /// A minimal pattern (rows of cells, `None` = required-empty) → output.
    /// Position-independent: matched against the grid's occupied bounding box.
    Shaped {
        pattern: Vec<Vec<Option<ItemId>>>,
        output: ItemStack,
    },
    /// A multiset of ingredients → output; arrangement is ignored.
    Shapeless {
        ingredients: Vec<ItemId>,
        output: ItemStack,
    },
}

impl Recipe {
    /// Does `grid` satisfy this recipe?
    pub fn matches(&self, grid: &CraftGrid) -> bool {
        match self {
            Recipe::Shaped { pattern, .. } => shaped_matches(pattern, grid),
            Recipe::Shapeless { ingredients, .. } => {
                let mut want: Vec<u16> = ingredients.iter().map(|i| i.0).collect();
                want.sort_unstable();
                !want.is_empty() && want == grid.ingredient_multiset()
            }
        }
    }

    fn output(&self) -> ItemStack {
        match self {
            Recipe::Shaped { output, .. } => *output,
            Recipe::Shapeless { output, .. } => *output,
        }
    }
}

/// Match a shaped pattern against the grid's occupied bounding box. An empty
/// pattern or empty grid never matches; the box dimensions must equal the
/// pattern's, and each cell must agree (both empty, or the same id).
fn shaped_matches(pattern: &[Vec<Option<ItemId>>], grid: &CraftGrid) -> bool {
    let ph = pattern.len();
    if ph == 0 {
        return false;
    }
    let pw = pattern[0].len();
    if pw == 0 || pattern.iter().any(|row| row.len() != pw) {
        return false;
    }
    // A pattern whose border is empty would be ambiguous; require it trimmed.
    if !pattern_is_trimmed(pattern) {
        return false;
    }
    let (top, left, bottom, right) = match grid.bounding_box() {
        Some(b) => b,
        None => return false,
    };
    let gh = bottom - top + 1;
    let gw = right - left + 1;
    if gh != ph || gw != pw {
        return false;
    }
    for i in 0..ph {
        for j in 0..pw {
            let want = pattern[i][j];
            let have = grid.get(top + i, left + j);
            if want != have {
                return false;
            }
        }
    }
    true
}

/// True if the pattern's outer rows/cols each contain at least one item, i.e. it
/// is already its own bounding box (no empty border to crop).
fn pattern_is_trimmed(pattern: &[Vec<Option<ItemId>>]) -> bool {
    let ph = pattern.len();
    let pw = pattern[0].len();
    let row_has = |i: usize| pattern[i].iter().any(|c| c.is_some());
    let col_has = |j: usize| (0..ph).any(|i| pattern[i][j].is_some());
    row_has(0) && row_has(ph - 1) && col_has(0) && col_has(pw - 1)
}

/// A table of recipes, searched in order.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RecipeTable {
    recipes: Vec<Recipe>,
}

impl RecipeTable {
    pub fn new(recipes: Vec<Recipe>) -> RecipeTable {
        RecipeTable { recipes }
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    /// The output of the first recipe that matches `grid`, or `None`.
    pub fn craft(&self, grid: &CraftGrid) -> Option<ItemStack> {
        self.recipes
            .iter()
            .find(|r| r.matches(grid))
            .map(|r| r.output())
    }

    /// Index of the first matching recipe (for a UI to highlight), or `None`.
    pub fn matching_index(&self, grid: &CraftGrid) -> Option<usize> {
        self.recipes.iter().position(|r| r.matches(grid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    // Demo item ids.
    const LOG: ItemId = ItemId(1);
    const PLANK: ItemId = ItemId(2);
    const STICK: ItemId = ItemId(3);
    const TABLE: ItemId = ItemId(4);
    const COAL: ItemId = ItemId(5);
    const TORCH: ItemId = ItemId(6);
    const PICKAXE: ItemId = ItemId(7);
    const BOWL: ItemId = ItemId(8);
    const RED_MUSH: ItemId = ItemId(9);
    const BROWN_MUSH: ItemId = ItemId(10);
    const STEW: ItemId = ItemId(11);
    const UNUSED: ItemId = ItemId(999);

    fn s(id: ItemId, n: u32) -> ItemStack {
        ItemStack::new(id, n)
    }
    fn cell(id: ItemId) -> Option<ItemId> {
        Some(id)
    }

    fn demo_table() -> RecipeTable {
        RecipeTable::new(vec![
            // sticks: two planks stacked vertically -> 4 sticks (2x1)
            Recipe::Shaped {
                pattern: vec![vec![cell(PLANK)], vec![cell(PLANK)]],
                output: s(STICK, 4),
            },
            // crafting table: 2x2 planks -> 1 table
            Recipe::Shaped {
                pattern: vec![
                    vec![cell(PLANK), cell(PLANK)],
                    vec![cell(PLANK), cell(PLANK)],
                ],
                output: s(TABLE, 1),
            },
            // torch: coal over stick (2x1) -> 4 torches
            Recipe::Shaped {
                pattern: vec![vec![cell(COAL)], vec![cell(STICK)]],
                output: s(TORCH, 4),
            },
            // pickaxe: 3 planks across, 2 sticks down the middle (3x3 T) -> 1 pick
            Recipe::Shaped {
                pattern: vec![
                    vec![cell(PLANK), cell(PLANK), cell(PLANK)],
                    vec![None, cell(STICK), None],
                    vec![None, cell(STICK), None],
                ],
                output: s(PICKAXE, 1),
            },
            // planks: 1 log -> 4 planks (shapeless, single ingredient)
            Recipe::Shapeless {
                ingredients: vec![LOG],
                output: s(PLANK, 4),
            },
            // stew: bowl + red + brown mushroom (shapeless, arrangement-free)
            Recipe::Shapeless {
                ingredients: vec![BOWL, RED_MUSH, BROWN_MUSH],
                output: s(STEW, 1),
            },
        ])
    }

    #[test]
    fn shaped_matches_at_any_offset() {
        let table = demo_table();
        // sticks placed top-left
        let mut g = CraftGrid::empty();
        g.set(0, 0, cell(PLANK));
        g.set(1, 0, cell(PLANK));
        assert_eq!(table.craft(&g), Some(s(STICK, 4)));
        // same recipe placed bottom-right -> still sticks
        let mut g2 = CraftGrid::empty();
        g2.set(1, 2, cell(PLANK));
        g2.set(2, 2, cell(PLANK));
        assert_eq!(table.craft(&g2), Some(s(STICK, 4)));
    }

    #[test]
    fn shaped_gap_changes_shape() {
        let table = demo_table();
        // planks at row 0 and row 2 (gap in the middle): a 3x1 shape, not 2x1.
        let mut g = CraftGrid::empty();
        g.set(0, 0, cell(PLANK));
        g.set(2, 0, cell(PLANK));
        assert_eq!(table.craft(&g), None);
    }

    #[test]
    fn pickaxe_shape() {
        let table = demo_table();
        let mut g = CraftGrid::empty();
        g.set(0, 0, cell(PLANK));
        g.set(0, 1, cell(PLANK));
        g.set(0, 2, cell(PLANK));
        g.set(1, 1, cell(STICK));
        g.set(2, 1, cell(STICK));
        assert_eq!(table.craft(&g), Some(s(PICKAXE, 1)));
    }

    #[test]
    fn shapeless_ignores_arrangement() {
        let table = demo_table();
        let mut a = CraftGrid::empty();
        a.set(0, 0, cell(BOWL));
        a.set(0, 1, cell(RED_MUSH));
        a.set(0, 2, cell(BROWN_MUSH));
        let mut b = CraftGrid::empty();
        b.set(2, 2, cell(RED_MUSH));
        b.set(1, 0, cell(BROWN_MUSH));
        b.set(0, 1, cell(BOWL));
        assert_eq!(table.craft(&a), Some(s(STEW, 1)));
        assert_eq!(table.craft(&a), table.craft(&b));
    }

    #[test]
    fn single_log_makes_planks() {
        let table = demo_table();
        let mut g = CraftGrid::empty();
        g.set(1, 1, cell(LOG));
        assert_eq!(table.craft(&g), Some(s(PLANK, 4)));
    }

    #[test]
    fn empty_and_unknown_grids_make_nothing() {
        let table = demo_table();
        assert_eq!(table.craft(&CraftGrid::empty()), None);
        let mut g = CraftGrid::empty();
        g.set(0, 0, cell(UNUSED));
        assert_eq!(table.craft(&g), None);
        // a partial pattern (one plank) matches no recipe
        let mut one = CraftGrid::empty();
        one.set(0, 0, cell(PLANK));
        assert_eq!(table.craft(&one), None);
    }

    // --- properties ---

    fn pattern_dims(p: &[Vec<Option<ItemId>>]) -> (usize, usize) {
        (p.len(), p[0].len())
    }

    fn place_shaped(p: &[Vec<Option<ItemId>>], top: usize, left: usize) -> CraftGrid {
        let mut g = CraftGrid::empty();
        for (i, row) in p.iter().enumerate() {
            for (j, c) in row.iter().enumerate() {
                if let Some(id) = c {
                    g.set(top + i, left + j, Some(*id));
                }
            }
        }
        g
    }

    #[test]
    fn prop_shaped_matches_at_every_offset() {
        let table = demo_table();
        for recipe in [
            Recipe::Shaped {
                pattern: vec![vec![cell(PLANK)], vec![cell(PLANK)]],
                output: s(STICK, 4),
            },
            Recipe::Shaped {
                pattern: vec![
                    vec![cell(PLANK), cell(PLANK)],
                    vec![cell(PLANK), cell(PLANK)],
                ],
                output: s(TABLE, 1),
            },
            Recipe::Shaped {
                pattern: vec![vec![cell(COAL)], vec![cell(STICK)]],
                output: s(TORCH, 4),
            },
        ] {
            if let Recipe::Shaped { pattern, output } = &recipe {
                let (ph, pw) = pattern_dims(pattern);
                for top in 0..=(GRID - ph) {
                    for left in 0..=(GRID - pw) {
                        let g = place_shaped(pattern, top, left);
                        assert!(recipe.matches(&g), "should match at ({top},{left})");
                        assert_eq!(table.craft(&g), Some(*output));
                    }
                }
            }
        }
    }

    #[test]
    fn prop_shapeless_any_permutation_same_output() {
        let table = demo_table();
        let ingredients = [BOWL, RED_MUSH, BROWN_MUSH];
        let positions: [(usize, usize); 9] = [
            (0, 0),
            (0, 1),
            (0, 2),
            (1, 0),
            (1, 1),
            (1, 2),
            (2, 0),
            (2, 1),
            (2, 2),
        ];
        let mut s_seed = 0x5151_2727u32;
        for _ in 0..3000 {
            // scatter the three ingredients into three distinct random cells
            let mut chosen = [0usize; 3];
            let mut k = 0;
            while k < 3 {
                let p = (lcg(&mut s_seed) as usize) % positions.len();
                if !chosen[..k].contains(&p) {
                    chosen[k] = p;
                    k += 1;
                }
            }
            let mut g = CraftGrid::empty();
            for (n, &slot) in chosen.iter().enumerate() {
                let (r, c) = positions[slot];
                g.set(r, c, cell(ingredients[n]));
            }
            assert_eq!(table.craft(&g), Some(s(STEW, 1)));
        }
    }

    #[test]
    fn prop_nonmatching_grids_yield_none_and_are_deterministic() {
        let table = demo_table();
        let mut s_seed = 0x9090_3131u32;
        for _ in 0..3000 {
            // Fill 1..=4 random cells all with the UNUSED id: cannot match any recipe.
            let mut g = CraftGrid::empty();
            let n = 1 + (lcg(&mut s_seed) as usize) % 4;
            for _ in 0..n {
                let r = (lcg(&mut s_seed) as usize) % GRID;
                let c = (lcg(&mut s_seed) as usize) % GRID;
                g.set(r, c, cell(UNUSED));
            }
            assert_eq!(table.craft(&g), None);
            assert_eq!(table.craft(&g), table.craft(&g)); // deterministic
        }
    }
}
