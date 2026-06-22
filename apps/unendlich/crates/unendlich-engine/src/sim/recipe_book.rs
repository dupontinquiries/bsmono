//! recipe_book — an ingredient → recipe index for a crafting UI ("what can I
//! make with this?"). Built once from each recipe's ingredient list; lookups are
//! cheap reads. Deterministic by construction: it uses a [`BTreeMap`] keyed by
//! item id and recipe-index lists are sorted and de-duplicated, so iteration and
//! query results are stable regardless of insertion quirks.
//!
//! Pure and rustc-testable in isolation (R1). It indexes recipes by the *id*
//! handles their ingredient lists carry, so it is agnostic to the recipe shape —
//! `crafting::Recipe` can feed it without this module depending on that type.
//! The contract — every indexed recipe is findable by each of its ingredients —
//! is pinned by a `prop_*` test.

use std::collections::BTreeMap;

/// Item kind id (kept local so the module compiles standalone).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u16);

/// An ingredient → recipe-index map. Recipes are referred to by their position
/// in the slice passed to [`RecipeBook::build`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RecipeBook {
    by_ingredient: BTreeMap<u16, Vec<usize>>,
    recipe_count: usize,
}

impl RecipeBook {
    /// Build an index from each recipe's ingredient list (`ingredient_sets[i]`
    /// is recipe `i`'s ingredients; duplicates within a list are fine). Resulting
    /// per-ingredient index lists are sorted ascending and de-duplicated.
    pub fn build(ingredient_sets: &[Vec<ItemId>]) -> RecipeBook {
        let mut by_ingredient: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
        for (idx, set) in ingredient_sets.iter().enumerate() {
            for ing in set {
                let bucket = by_ingredient.entry(ing.0).or_default();
                // recipes are visited in ascending idx order, so push-if-new
                // keeps each bucket sorted and free of duplicate indices.
                if bucket.last() != Some(&idx) {
                    bucket.push(idx);
                }
            }
        }
        RecipeBook {
            by_ingredient,
            recipe_count: ingredient_sets.len(),
        }
    }

    /// Number of recipes indexed.
    pub fn recipe_count(&self) -> usize {
        self.recipe_count
    }

    /// Number of distinct ingredients seen.
    pub fn ingredient_count(&self) -> usize {
        self.by_ingredient.len()
    }

    /// Recipe indices that use `ingredient`, sorted ascending. Empty slice if the
    /// ingredient is in no recipe.
    pub fn find_by_ingredient(&self, ingredient: ItemId) -> &[usize] {
        self.by_ingredient
            .get(&ingredient.0)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Recipe indices that use EVERY one of `ingredients` (set intersection),
    /// sorted ascending. An empty query yields no results.
    pub fn find_by_all(&self, ingredients: &[ItemId]) -> Vec<usize> {
        if ingredients.is_empty() {
            return Vec::new();
        }
        let mut iter = ingredients.iter();
        let mut acc: Vec<usize> = match iter.next() {
            Some(first) => self.find_by_ingredient(*first).to_vec(),
            None => return Vec::new(),
        };
        for ing in iter {
            let next = self.find_by_ingredient(*ing);
            acc.retain(|idx| next.contains(idx));
            if acc.is_empty() {
                break;
            }
        }
        acc
    }

    /// Recipe indices that use ANY of `ingredients` (set union), sorted ascending
    /// and de-duplicated.
    pub fn find_by_any(&self, ingredients: &[ItemId]) -> Vec<usize> {
        let mut acc: Vec<usize> = Vec::new();
        for ing in ingredients {
            for &idx in self.find_by_ingredient(*ing) {
                acc.push(idx);
            }
        }
        acc.sort_unstable();
        acc.dedup();
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn id(v: u16) -> ItemId {
        ItemId(v)
    }

    fn demo_sets() -> Vec<Vec<ItemId>> {
        vec![
            vec![id(2), id(2)],                 // 0: sticks (planks)
            vec![id(2)],                        // 1: planks-only thing
            vec![id(5), id(3)],                 // 2: torch (coal + stick)
            vec![id(2), id(2), id(2), id(3)],   // 3: pickaxe (planks + stick)
            vec![id(8), id(9), id(10)],         // 4: stew
        ]
    }

    #[test]
    fn finds_recipes_by_ingredient() {
        let book = RecipeBook::build(&demo_sets());
        assert_eq!(book.recipe_count(), 5);
        // planks (id 2) appear in recipes 0,1,3
        assert_eq!(book.find_by_ingredient(id(2)), &[0, 1, 3]);
        // stick (id 3) in 2,3
        assert_eq!(book.find_by_ingredient(id(3)), &[2, 3]);
        // coal (id 5) only in 2
        assert_eq!(book.find_by_ingredient(id(5)), &[2]);
        // an ingredient in nothing
        assert_eq!(book.find_by_ingredient(id(999)), &[] as &[usize]);
    }

    #[test]
    fn duplicate_ingredient_indexes_once() {
        let book = RecipeBook::build(&demo_sets());
        // recipe 0 lists planks twice; index must contain 0 exactly once.
        let planks = book.find_by_ingredient(id(2));
        assert_eq!(planks.iter().filter(|&&i| i == 0).count(), 1);
    }

    #[test]
    fn find_by_all_is_intersection() {
        let book = RecipeBook::build(&demo_sets());
        // planks AND stick -> only recipe 3 (pickaxe)
        assert_eq!(book.find_by_all(&[id(2), id(3)]), vec![3]);
        // coal AND stick -> recipe 2 (torch)
        assert_eq!(book.find_by_all(&[id(5), id(3)]), vec![2]);
        // planks AND coal -> none share
        assert!(book.find_by_all(&[id(2), id(5)]).is_empty());
        // empty query -> none
        assert!(book.find_by_all(&[]).is_empty());
    }

    #[test]
    fn find_by_any_is_union() {
        let book = RecipeBook::build(&demo_sets());
        // coal OR brown-mushroom -> recipes 2 and 4
        assert_eq!(book.find_by_any(&[id(5), id(10)]), vec![2, 4]);
        // duplicate query ids don't duplicate results
        assert_eq!(book.find_by_any(&[id(3), id(3)]), vec![2, 3]);
    }

    #[test]
    fn empty_book_is_safe() {
        let book = RecipeBook::build(&[]);
        assert_eq!(book.recipe_count(), 0);
        assert_eq!(book.ingredient_count(), 0);
        assert_eq!(book.find_by_ingredient(id(1)), &[] as &[usize]);
        assert!(book.find_by_all(&[id(1)]).is_empty());
    }

    #[test]
    fn prop_every_ingredient_finds_its_recipe() {
        let mut s = 0x4242_1111u32;
        for _ in 0..3000 {
            // build a random set of recipes, each with 1..=5 ingredients in 1..=12
            let n = 1 + (lcg(&mut s) as usize) % 8;
            let mut sets: Vec<Vec<ItemId>> = Vec::with_capacity(n);
            for _ in 0..n {
                let k = 1 + (lcg(&mut s) as usize) % 5;
                let set: Vec<ItemId> =
                    (0..k).map(|_| id(1 + (lcg(&mut s) % 12) as u16)).collect();
                sets.push(set);
            }
            let book = RecipeBook::build(&sets);
            for (i, set) in sets.iter().enumerate() {
                for ing in set {
                    assert!(
                        book.find_by_ingredient(*ing).contains(&i),
                        "recipe {i} not findable by its ingredient {:?}",
                        ing
                    );
                }
            }
        }
    }

    #[test]
    fn prop_lists_sorted_dedup_and_deterministic() {
        let mut s = 0x4242_2222u32;
        for _ in 0..3000 {
            let n = 1 + (lcg(&mut s) as usize) % 8;
            let mut sets: Vec<Vec<ItemId>> = Vec::with_capacity(n);
            for _ in 0..n {
                let k = 1 + (lcg(&mut s) as usize) % 5;
                let set: Vec<ItemId> =
                    (0..k).map(|_| id(1 + (lcg(&mut s) % 6) as u16)).collect();
                sets.push(set);
            }
            let a = RecipeBook::build(&sets);
            let b = RecipeBook::build(&sets);
            assert_eq!(a, b, "build is deterministic");
            // each bucket strictly increasing (sorted + deduped)
            for ing in 1..=6u16 {
                let list = a.find_by_ingredient(id(ing));
                assert!(list.windows(2).all(|w| w[0] < w[1]), "bucket sorted/deduped");
            }
        }
    }

    #[test]
    fn prop_find_by_all_results_contain_all_queried() {
        let book = RecipeBook::build(&demo_sets());
        let pool = [id(2), id(3), id(5), id(8), id(9), id(10)];
        let sets = demo_sets();
        let mut s = 0x4242_3333u32;
        for _ in 0..3000 {
            let k = 1 + (lcg(&mut s) as usize) % 3;
            let query: Vec<ItemId> =
                (0..k).map(|_| pool[(lcg(&mut s) as usize) % pool.len()]).collect();
            for &idx in &book.find_by_all(&query) {
                for q in &query {
                    assert!(sets[idx].contains(q), "result {idx} missing queried ingredient");
                }
            }
        }
    }
}
