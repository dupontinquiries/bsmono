//! sim::item_breeding — the generational breeding *loop* for evolving gear.
//!
//! [`crate::sim::item_genetics`] gives the per-item primitives — a genome that
//! `express`es into stats, plus `mutate` and `breed`. What it doesn't give is
//! the *loop*: how a whole population of crafted/found items advances toward
//! what a smith actually wants (sharper blades, lighter armour, more valuable
//! trinkets). This module is that policy — a small, deterministic evolutionary
//! step:
//!
//!   1. score every item by a caller-chosen [`Objective`] (weights over the
//!      expressed stats — negative to *penalise*, e.g. weight),
//!   2. keep the best few unchanged (**elitism**),
//!   3. fill the rest by breeding pairs drawn from the elites and mutating.
//!
//! Elitism is the load-bearing choice: it guarantees the best fitness in the
//! population never *drops* from one generation to the next, so a breeding run
//! only ever improves (or holds) the frontier. Pure and seeded — same inputs,
//! same lineage — no clock, no rng crate.

use crate::sim::item_genetics::ItemGenes;

/// What the breeder is optimising for: a weight per expressed stat. Positive
/// rewards the stat, negative penalises it (set `weight` negative to prefer
/// lighter gear). Fitness is the weighted sum of `ItemGenes::express()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Objective {
    pub damage: f32,
    pub weight: f32,
    pub durability: f32,
    pub reach: f32,
    pub value: f32,
}

impl Objective {
    /// A sensible default: reward damage/durability/reach/value, mildly penalise
    /// weight.
    pub fn all_round() -> Objective {
        Objective { damage: 1.0, weight: -0.25, durability: 1.0, reach: 0.5, value: 0.5 }
    }
    /// Optimise a single stat (others zero).
    pub fn damage_only() -> Objective {
        Objective { damage: 1.0, weight: 0.0, durability: 0.0, reach: 0.0, value: 0.0 }
    }
}

/// Fitness of one genome under an objective: the weighted sum of its expressed
/// stats. Higher is better.
pub fn fitness(genes: &ItemGenes, obj: &Objective) -> f32 {
    let s = genes.express();
    obj.damage * s.damage
        + obj.weight * s.weight
        + obj.durability * s.durability
        + obj.reach * s.reach
        + obj.value * s.value
}

/// Knobs for one generation.
#[derive(Clone, Copy, Debug)]
pub struct BreedParams {
    /// How many top performers survive unchanged. Clamped to `[1, pop_len]`.
    pub elite: usize,
    /// Size of the next generation. If `0`, the current population size is kept.
    pub pop_size: usize,
    /// Per-trait mutation rate handed to `ItemGenes::mutate` (`0` = faithful
    /// recombination only).
    pub mutation_rate: f32,
    /// Base seed for this generation's pairings/mutations.
    pub seed: u64,
}

/// Deterministic 64-bit mix (splitmix64-style) for deriving per-offspring seeds.
fn mix(mut x: u64, y: u64) -> u64 {
    x = x.wrapping_add(y).wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Indices of the top-`k` genomes by fitness, best first. Ties resolve to the
/// lower index, so the ranking is deterministic. `k` is clamped to the
/// population size.
pub fn select_elites(pop: &[ItemGenes], obj: &Objective, k: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..pop.len()).collect();
    idx.sort_by(|&a, &b| {
        let (fa, fb) = (fitness(&pop[a], obj), fitness(&pop[b], obj));
        // higher fitness first; NaN-safe; ties → lower index first
        fb.partial_cmp(&fa).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
    });
    idx.truncate(k.min(pop.len()));
    idx
}

/// The best genome in a population under `obj`: `(index, fitness)`, or `None`
/// for an empty population.
pub fn best(pop: &[ItemGenes], obj: &Objective) -> Option<(usize, f32)> {
    select_elites(pop, obj, 1).first().map(|&i| (i, fitness(&pop[i], obj)))
}

/// Advance one generation: carry the elites through unchanged, then fill the
/// rest by breeding elite pairs (chosen deterministically from the seed) and
/// mutating. Returns the new population of `pop_size` (or the old size if
/// `pop_size == 0`). An empty input yields an empty output.
pub fn advance_generation(pop: &[ItemGenes], obj: &Objective, params: &BreedParams) -> Vec<ItemGenes> {
    if pop.is_empty() {
        return Vec::new();
    }
    let target = if params.pop_size == 0 { pop.len() } else { params.pop_size };
    let elite_k = params.elite.clamp(1, pop.len());
    let elites = select_elites(pop, obj, elite_k);

    let mut out: Vec<ItemGenes> = elites.iter().map(|&i| pop[i]).collect();
    out.truncate(target); // in case elite_k > target

    let e = elites.len();
    while out.len() < target {
        let j = out.len() as u64;
        let s = mix(params.seed, j);
        let pa = elites[(mix(s, 1) as usize) % e];
        let mut pb = elites[(mix(s, 2) as usize) % e];
        if e > 1 && pb == pa {
            pb = elites[(mix(s, 3) as usize) % e];
        }
        let child = ItemGenes::breed(&pop[pa], &pop[pb], s);
        let child = child.mutate(mix(s, 4), params.mutation_rate);
        out.push(child);
    }
    out
}

/// Run the loop for `generations` steps, re-seeding each step so successive
/// generations don't reuse the same pairings. Returns the final population.
pub fn run(pop: &[ItemGenes], obj: &Objective, params: &BreedParams, generations: usize) -> Vec<ItemGenes> {
    let mut cur = pop.to_vec();
    for g in 0..generations {
        let p = BreedParams { seed: mix(params.seed, g as u64), ..*params };
        cur = advance_generation(&cur, obj, &p);
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::item_genetics::ItemGenes;

    fn genome(sharpness: f32, heft: f32, balance: f32, resilience: f32, flair: f32) -> ItemGenes {
        ItemGenes { sharpness, heft, balance, resilience, flair }
    }

    // A deterministic spread of starting genomes.
    fn seed_population(n: usize) -> Vec<ItemGenes> {
        (0..n)
            .map(|i| {
                let f = |k: u64| ((mix(i as u64, k) >> 11) as f32 / (1u64 << 53) as f32);
                genome(f(1), f(2), f(3), f(4), f(5))
            })
            .collect()
    }

    #[test]
    fn fitness_follows_the_objective() {
        let sharp = genome(0.9, 0.6, 0.5, 0.5, 0.5);
        let dull = genome(0.1, 0.6, 0.5, 0.5, 0.5);
        let obj = Objective::damage_only();
        assert!(fitness(&sharp, &obj) > fitness(&dull, &obj), "damage objective should rank the sharper blade higher");
    }

    #[test]
    fn select_elites_is_ranked_and_deterministic() {
        let pop = seed_population(12);
        let obj = Objective::all_round();
        let e1 = select_elites(&pop, &obj, 4);
        let e2 = select_elites(&pop, &obj, 4);
        assert_eq!(e1, e2, "deterministic");
        assert_eq!(e1.len(), 4);
        // fitness is non-increasing down the elite list
        for w in e1.windows(2) {
            assert!(fitness(&pop[w[0]], &obj) >= fitness(&pop[w[1]], &obj), "elites must be sorted best-first");
        }
        // the top elite is the global best
        assert_eq!(e1[0], best(&pop, &obj).unwrap().0);
    }

    #[test]
    fn advance_preserves_the_elites_and_the_size() {
        let pop = seed_population(10);
        let obj = Objective::all_round();
        let params = BreedParams { elite: 3, pop_size: 10, mutation_rate: 0.0, seed: 42 };
        let elites = select_elites(&pop, &obj, 3);
        let next = advance_generation(&pop, &obj, &params);
        assert_eq!(next.len(), 10, "population size held");
        for (k, &ei) in elites.iter().enumerate() {
            assert_eq!(next[k], pop[ei], "elite {k} carried through unchanged");
        }
    }

    #[test]
    fn best_fitness_never_regresses_across_generations() {
        // Elitism's guarantee: the frontier only improves or holds.
        let pop = seed_population(16);
        let obj = Objective::damage_only();
        let params = BreedParams { elite: 3, pop_size: 16, mutation_rate: 0.15, seed: 7 };
        let mut cur = pop.clone();
        let mut prev_best = best(&cur, &obj).unwrap().1;
        let start_best = prev_best;
        for _ in 0..12 {
            let p = BreedParams { seed: mix(params.seed, cur.len() as u64), ..params };
            cur = advance_generation(&cur, &obj, &p);
            let b = best(&cur, &obj).unwrap().1;
            assert!(b >= prev_best - 1e-6, "best fitness regressed: {prev_best} -> {b}");
            prev_best = b;
        }
        assert!(prev_best >= start_best, "a breeding run should not end worse than it began");
    }

    #[test]
    fn run_is_deterministic() {
        let pop = seed_population(12);
        let obj = Objective::all_round();
        let params = BreedParams { elite: 2, pop_size: 12, mutation_rate: 0.1, seed: 99 };
        let a = run(&pop, &obj, &params, 8);
        let b = run(&pop, &obj, &params, 8);
        assert_eq!(a, b, "same seed ⇒ identical lineage");
    }

    #[test]
    fn an_empty_population_stays_empty() {
        let obj = Objective::all_round();
        let params = BreedParams { elite: 2, pop_size: 8, mutation_rate: 0.1, seed: 1 };
        assert!(advance_generation(&[], &obj, &params).is_empty());
        assert!(best(&[], &obj).is_none());
    }

    #[test]
    fn evolution_actually_improves_a_directed_objective() {
        // Over enough generations, directed selection should raise the best
        // fitness above the starting frontier (not just hold it).
        let pop = seed_population(24);
        let obj = Objective::damage_only();
        let params = BreedParams { elite: 4, pop_size: 24, mutation_rate: 0.2, seed: 2024 };
        let start = best(&pop, &obj).unwrap().1;
        let evolved = run(&pop, &obj, &params, 30);
        let end = best(&evolved, &obj).unwrap().1;
        assert!(end >= start, "directed evolution should not lose ground: {start} -> {end}");
    }
}
