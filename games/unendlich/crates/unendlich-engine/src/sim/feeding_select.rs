//! `sim::feeding_select` — the block-level foraging choice: of the food in
//! reach, which (if any) should a creature go for?
//!
//! This is the *decision* only — actually removing the block or filling the
//! belly is integration done elsewhere. Given the [`Food`] in each neighbouring
//! cell, the creature's [`Diet`], and its hunger, [`pick`] returns the index of
//! the best edible neighbour, or `None` when the creature isn't hungry or
//! nothing in reach is edible for its diet.
//!
//! "Best" is the highest food value among the cells the diet can actually eat.
//! Ties go to the lowest index, so the choice is deterministic. Reactive tier:
//! cheap, allocation-free, no clock, no randomness. Types are self-contained.

/// Hunger at or above which a creature will bother to eat.
pub const HUNGRY_AT: f32 = 0.5;

/// What sits in a neighbouring cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Food {
    /// Nothing edible here.
    None,
    /// Ground cover.
    Grass,
    /// Foliage.
    Leaf,
    /// Fruit — the richest plant food.
    Fruit,
    /// Flesh.
    Meat,
}

/// A creature's dietary class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diet {
    /// Plants only.
    Herbivore,
    /// Meat only.
    Carnivore,
    /// Anything.
    Omnivore,
}

/// Nutritional ranking; higher is preferred. `None` is worthless.
fn value(f: Food) -> u8 {
    match f {
        Food::None => 0,
        Food::Grass => 1,
        Food::Leaf => 2,
        Food::Fruit => 3,
        Food::Meat => 4,
    }
}

/// Whether a diet can eat a given food.
fn edible(f: Food, diet: Diet) -> bool {
    match f {
        Food::None => false,
        Food::Meat => matches!(diet, Diet::Carnivore | Diet::Omnivore),
        Food::Grass | Food::Leaf | Food::Fruit => {
            matches!(diet, Diet::Herbivore | Diet::Omnivore)
        }
    }
}

/// Pick the index of the best edible neighbour, if the creature should eat.
///
/// Returns `None` when hunger is below [`HUNGRY_AT`] or no neighbour is edible
/// for `diet`. Otherwise returns the index of the highest-value edible cell,
/// breaking ties toward the lowest index.
pub fn pick(neighbours: &[Food], diet: Diet, hunger: f32) -> Option<usize> {
    if !(hunger >= HUNGRY_AT) {
        return None;
    }
    let mut best: Option<(usize, u8)> = None;
    let mut i = 0;
    while i < neighbours.len() {
        let f = neighbours[i];
        if edible(f, diet) {
            let v = value(f);
            match best {
                Some((_, bv)) if bv >= v => {}
                _ => best = Some((i, v)),
            }
        }
        i += 1;
    }
    match best {
        Some((idx, _)) => Some(idx),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_food(seed: &mut u32) -> Food {
        match lcg(seed) % 5 {
            0 => Food::None,
            1 => Food::Grass,
            2 => Food::Leaf,
            3 => Food::Fruit,
            _ => Food::Meat,
        }
    }
    fn rand_diet(seed: &mut u32) -> Diet {
        match lcg(seed) % 3 {
            0 => Diet::Herbivore,
            1 => Diet::Carnivore,
            _ => Diet::Omnivore,
        }
    }

    #[test]
    fn herbivore_ignores_meat() {
        assert_eq!(pick(&[Food::Meat], Diet::Herbivore, 1.0), None);
        // with a plant present, the herbivore takes the plant, not the meat
        assert_eq!(
            pick(&[Food::Meat, Food::Grass], Diet::Herbivore, 1.0),
            Some(1)
        );
    }

    #[test]
    fn carnivore_eats_meat_over_plants() {
        assert_eq!(
            pick(&[Food::Grass, Food::Meat, Food::Fruit], Diet::Carnivore, 1.0),
            Some(1)
        );
        assert_eq!(pick(&[Food::Grass, Food::Fruit], Diet::Carnivore, 1.0), None);
    }

    #[test]
    fn fruit_chosen_over_grass_and_leaf() {
        assert_eq!(
            pick(&[Food::Grass, Food::Leaf, Food::Fruit], Diet::Herbivore, 1.0),
            Some(2)
        );
        assert_eq!(
            pick(&[Food::Fruit, Food::Grass], Diet::Omnivore, 1.0),
            Some(0)
        );
    }

    #[test]
    fn none_when_full() {
        assert_eq!(pick(&[Food::Fruit, Food::Meat], Diet::Omnivore, 0.0), None);
        // just below the threshold is still "not hungry"
        assert_eq!(pick(&[Food::Fruit], Diet::Herbivore, 0.49), None);
    }

    #[test]
    fn none_when_empty_or_all_inedible() {
        assert_eq!(pick(&[], Diet::Omnivore, 1.0), None);
        assert_eq!(
            pick(&[Food::None, Food::None], Diet::Carnivore, 1.0),
            None
        );
    }

    #[test]
    fn prop_index_valid_and_edible() {
        let mut seed = 0xF00D_u32;
        for _ in 0..4000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut v = Vec::new();
            for _ in 0..n {
                v.push(rand_food(&mut seed));
            }
            let diet = rand_diet(&mut seed);
            let hunger = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            if let Some(idx) = pick(&v, diet, hunger) {
                assert!(idx < v.len());
                assert!(edible(v[idx], diet), "picked an inedible cell");
                // and it is the max value among edible cells
                let chosen = value(v[idx]);
                for (j, &f) in v.iter().enumerate() {
                    if edible(f, diet) && value(f) > chosen {
                        panic!("cell {} was better than the pick", j);
                    }
                }
            }
        }
    }

    #[test]
    fn prop_deterministic() {
        let mut seed = 0xBEAD_u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut v = Vec::new();
            for _ in 0..n {
                v.push(rand_food(&mut seed));
            }
            let diet = rand_diet(&mut seed);
            let hunger = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(pick(&v, diet, hunger), pick(&v, diet, hunger));
        }
    }

    #[test]
    fn ties_go_to_lowest_index() {
        // two equal-value edible cells: the earlier index wins
        assert_eq!(
            pick(&[Food::Grass, Food::Grass], Diet::Herbivore, 1.0),
            Some(0)
        );
    }
}
