//! The genetic engine the others share.
//!
//! Three evolutions live in this crate -- greebles, the spec tuner's
//! placers and destroyers, plant breeding -- each with its own loop,
//! its own scalar fitness and a fixed generation count. The scalar was
//! the ceiling: "detailed but not noisy" became one number, and the
//! trade-off was lost before selection ever saw it. This is the shared
//! engine, and its ranking is MULTI-OBJECTIVE.
//!
//! RANKING
//!
//! Each candidate is scored on several objectives, all "lower is
//! better". A candidate DOMINATES another if it is no worse on every
//! objective and better on at least one. The non-dominated set is the
//! Pareto front: rank 0. Remove it and the next non-dominated set is
//! rank 1, and so on. Within a rank, CROWDING distance -- how far a
//! candidate is from its neighbours along each objective -- prefers
//! the spread-out ones, so the front stays diverse rather than
//! collapsing onto one corner. That is NSGA-II's ranking, and it is
//! what makes "show me three different good ones" possible: the front
//! is three different good ones.
//!
//! The final pick is by a WEIGHT vector over the objectives, applied
//! only to rank 0: the player's priorities choose from the front, they
//! never shape the search.
//!
//! BUDGET
//!
//! Evolution runs until a budget is spent: a number of evaluations, or
//! a deadline, whichever first. The drone can spend what it has rather
//! than a fixed twelve generations; a big surface gets fewer, a small
//! one more.
//!
//! ARCHIVE
//!
//! An elite archive keeps the best genomes seen for a key -- a phrase,
//! a surface class -- across the session, and seeds the next run with
//! them. A second "gothic" wall starts where the first one finished.
//!
//! Deterministic from a seed except the deadline, which is wall clock;
//! a test uses an evaluation budget.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub trait Genome: Clone {
    fn mutate(&self, seed: u64) -> Self;
    fn crossover(&self, other: &Self, seed: u64) -> Self;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    pub max_evals: usize,
    pub deadline: Option<Duration>,
    pub pop: usize,
}

impl Budget {
    pub fn evals(n: usize, pop: usize) -> Self { Self { max_evals: n, deadline: None, pop } }
    pub fn timed(d: Duration, pop: usize) -> Self { Self { max_evals: usize::MAX, deadline: Some(d), pop } }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ranked<G> {
    pub genome: G,
    pub objectives: Vec<f32>,
    pub rank: usize,
    pub crowding: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome<G> {
    /// Rank 0, sorted by crowding (most isolated first).
    pub front: Vec<Ranked<G>>,
    pub evaluations: usize,
    pub generations: usize,
    /// Best weighted score per generation, for the log.
    pub history: Vec<f32>,
}

fn hash(seed: u64, a: u64, b: u64) -> u64 {
    let mut h = seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31; h = h.wrapping_mul(0x7FB5_D329_728E_A185); h ^ (h >> 27)
}

fn dominates(a: &[f32], b: &[f32]) -> bool {
    let mut better = false;
    for (x, y) in a.iter().zip(b) {
        if x > y { return false; }
        if x < y { better = true; }
    }
    better
}

/// Non-dominated sorting with crowding distance. Returns each index's
/// (rank, crowding).
pub fn rank(objectives: &[Vec<f32>]) -> Vec<(usize, f32)> {
    let n = objectives.len();
    let mut out = vec![(usize::MAX, 0.0f32); n];
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut r = 0;
    while !remaining.is_empty() {
        let front: Vec<usize> = remaining.iter().copied()
            .filter(|&i| !remaining.iter().any(|&j| j != i && dominates(&objectives[j], &objectives[i])))
            .collect();
        // crowding within this front, per objective
        let m = objectives.first().map(|o| o.len()).unwrap_or(0);
        let mut crowd: BTreeMap<usize, f32> = front.iter().map(|&i| (i, 0.0)).collect();
        for k in 0..m {
            let mut by: Vec<usize> = front.clone();
            by.sort_by(|&a, &b| objectives[a][k].partial_cmp(&objectives[b][k]).unwrap_or(std::cmp::Ordering::Equal));
            if by.len() < 3 { for &i in &by { *crowd.get_mut(&i).unwrap() += f32::INFINITY; } continue; }
            let span = (objectives[*by.last().unwrap()][k] - objectives[by[0]][k]).max(1e-6);
            *crowd.get_mut(&by[0]).unwrap() += f32::INFINITY;
            *crowd.get_mut(by.last().unwrap()).unwrap() += f32::INFINITY;
            for w in 1..by.len() - 1 {
                let d = (objectives[by[w + 1]][k] - objectives[by[w - 1]][k]) / span;
                *crowd.get_mut(&by[w]).unwrap() += d;
            }
        }
        for &i in &front { out[i] = (r, crowd[&i]); }
        remaining.retain(|i| !front.contains(i));
        r += 1;
    }
    out
}

/// The weighted score of an objective vector: lower is better.
pub fn weighted(obj: &[f32], weights: &[f32]) -> f32 {
    obj.iter().zip(weights.iter().chain(std::iter::repeat(&1.0))).map(|(o, w)| o * w).sum()
}

/// Evolve. `eval` returns the objectives (lower is better). `seeds`
/// are genomes to include in the first population -- the archive's.
pub fn evolve<G: Genome>(
    seed: u64, budget: Budget, seeds: &[G], random: impl Fn(u64) -> G,
    eval: impl Fn(&G) -> Vec<f32>, weights: &[f32],
) -> Outcome<G> {
    let pop = budget.pop.clamp(4, 96);
    let t0 = Instant::now();
    let mut population: Vec<G> = seeds.iter().cloned().take(pop / 2).collect();
    let mut k = 0u64;
    while population.len() < pop { population.push(random(hash(seed, 1, k))); k += 1; }
    let mut evals = 0usize;
    let mut gens = 0usize;
    let mut history = Vec::new();
    let mut scored: Vec<Vec<f32>> = population.iter().map(|g| { evals += 1; eval(g) }).collect();
    let over = |evals: usize, t0: Instant| evals >= budget.max_evals
        || budget.deadline.map(|d| t0.elapsed() >= d).unwrap_or(false);
    loop {
        gens += 1;
        let ranks = rank(&scored);
        history.push(population.iter().zip(&scored).zip(&ranks)
            .filter(|(_, (r, _))| *r == 0).map(|((_, o), _)| weighted(o, weights))
            .fold(f32::INFINITY, f32::min));
        if over(evals, t0) { 
            let mut front: Vec<Ranked<G>> = population.iter().zip(&scored).zip(&ranks)
                .filter(|(_, (r, _))| *r == 0)
                .map(|((g, o), (r, c))| Ranked { genome: g.clone(), objectives: o.clone(), rank: *r, crowding: *c })
                .collect();
            front.sort_by(|a, b| b.crowding.partial_cmp(&a.crowding).unwrap_or(std::cmp::Ordering::Equal));
            return Outcome { front, evaluations: evals, generations: gens, history };
        }
        // Tournament by (rank, crowding): lower rank wins, then more crowding.
        let better = |a: usize, b: usize| -> usize {
            let (ra, ca) = ranks[a]; let (rb, cb) = ranks[b];
            if ra != rb { if ra < rb { a } else { b } } else if ca >= cb { a } else { b }
        };
        // Elitism: the rank-0 front carries over, capped at half the
        // population. The cap is ORDERED: the best by the weights first,
        // then the most isolated, so truncating a wide front never drops
        // the candidate the player would have picked. Unordered, the
        // history went up when a spread-out front pushed it off.
        let mut elite: Vec<usize> = (0..pop).filter(|&i| ranks[i].0 == 0).collect();
        elite.sort_by(|&a, &b| ranks[b].1.partial_cmp(&ranks[a].1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(best) = elite.iter().copied().min_by(|&a, &b|
            weighted(&scored[a], weights).partial_cmp(&weighted(&scored[b], weights)).unwrap_or(std::cmp::Ordering::Equal)) {
            elite.retain(|&i| i != best);
            elite.insert(0, best);
        }
        elite.truncate(pop / 2);
        let mut next: Vec<G> = elite.iter().map(|&i| population[i].clone()).collect();
        let mut next_scores: Vec<Vec<f32>> = elite.iter().map(|&i| scored[i].clone()).collect();
        let mut kk = 0u64;
        while next.len() < pop {
            let a = better((hash(seed, 100 + gens as u64, kk) % pop as u64) as usize,
                           (hash(seed, 200 + gens as u64, kk) % pop as u64) as usize);
            let child = if kk % 2 == 0 {
                let b = better((hash(seed, 300 + gens as u64, kk) % pop as u64) as usize,
                               (hash(seed, 400 + gens as u64, kk) % pop as u64) as usize);
                population[a].crossover(&population[b], hash(seed, 500 + gens as u64, kk))
            } else {
                population[a].mutate(hash(seed, 600 + gens as u64, kk))
            };
            evals += 1;
            next_scores.push(eval(&child));
            next.push(child);
            kk += 1;
            if over(evals, t0) { break; }
        }
        population = next;
        scored = next_scores;
    }
}

/// The front's best by a weight vector.
pub fn pick<'a, G>(front: &'a [Ranked<G>], weights: &[f32]) -> Option<&'a Ranked<G>> {
    front.iter().min_by(|a, b| weighted(&a.objectives, weights).partial_cmp(&weighted(&b.objectives, weights))
                                .unwrap_or(std::cmp::Ordering::Equal))
}

/// Up to n candidates from the front that differ most from each other:
/// the most isolated by crowding, then those farthest from the ones
/// already chosen. "Three different good ones."
pub fn diverse<'a, G>(front: &'a [Ranked<G>], n: usize) -> Vec<&'a Ranked<G>> {
    let mut out: Vec<&Ranked<G>> = Vec::new();
    let dist = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>().sqrt();
    while out.len() < n {
        let next = front.iter().filter(|c| !out.iter().any(|o| std::ptr::eq(*o, *c)))
            .max_by(|a, b| {
                let da = out.iter().map(|o| dist(&o.objectives, &a.objectives)).fold(f32::INFINITY, f32::min);
                let db = out.iter().map(|o| dist(&o.objectives, &b.objectives)).fold(f32::INFINITY, f32::min);
                (if out.is_empty() { a.crowding } else { da }).partial_cmp(&if out.is_empty() { b.crowding } else { db })
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        match next { Some(c) => out.push(c), None => break }
    }
    out
}

/// Elite genomes kept for the session, by key.
#[derive(Debug, Clone)]
pub struct Archive<G> {
    by_key: BTreeMap<String, Vec<(Vec<f32>, G)>>,
    pub cap: usize,
}

impl<G: Clone> Default for Archive<G> {
    fn default() -> Self { Self { by_key: BTreeMap::new(), cap: 6 } }
}

impl<G: Clone> Archive<G> {
    pub fn seeds(&self, key: &str) -> Vec<G> {
        self.by_key.get(key).map(|v| v.iter().map(|(_, g)| g.clone()).collect()).unwrap_or_default()
    }
    /// Offer a front. Kept if non-dominated among what is stored; the
    /// store is trimmed to `cap` by crowding.
    pub fn offer(&mut self, key: &str, front: &[Ranked<G>]) {
        let entry = self.by_key.entry(key.to_string()).or_default();
        for r in front {
            // the same objectives twice is the same point on the front
            if !entry.iter().any(|(o, _)| *o == r.objectives) { entry.push((r.objectives.clone(), r.genome.clone())); }
        }
        let objs: Vec<Vec<f32>> = entry.iter().map(|(o, _)| o.clone()).collect();
        let ranks = rank(&objs);
        let mut keep: Vec<(usize, (Vec<f32>, G))> = entry.drain(..).enumerate()
            .filter(|(i, _)| ranks[*i].0 == 0).collect();
        keep.sort_by(|a, b| ranks[b.0].1.partial_cmp(&ranks[a.0].1).unwrap_or(std::cmp::Ordering::Equal));
        *entry = keep.into_iter().map(|(_, e)| e).take(self.cap).collect();
    }
    pub fn len(&self) -> usize { self.by_key.values().map(|v| v.len()).sum() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct P(f32, f32);
    impl Genome for P {
        fn mutate(&self, seed: u64) -> Self {
            let d = |k: u64| ((hash(seed, k, 0) % 1000) as f32 / 1000.0 - 0.5) * 0.2;
            P((self.0 + d(1)).clamp(0.0, 1.0), (self.1 + d(2)).clamp(0.0, 1.0))
        }
        fn crossover(&self, o: &Self, seed: u64) -> Self {
            if hash(seed, 3, 0) % 2 == 0 { P(self.0, o.1) } else { P(o.0, self.1) }
        }
    }

    #[test]
    fn non_dominated_sorting_finds_the_front_and_the_next() {
        let objs = vec![vec![1.0, 5.0], vec![5.0, 1.0], vec![3.0, 3.0], vec![4.0, 4.0], vec![2.0, 2.0]];
        let r = rank(&objs);
        // (1,5), (5,1), (2,2) are non-dominated; (3,3) is dominated by (2,2); (4,4) by (3,3)
        assert_eq!(r[0].0, 0); assert_eq!(r[1].0, 0); assert_eq!(r[4].0, 0);
        assert_eq!(r[2].0, 1); assert_eq!(r[3].0, 2);
        assert!(r[0].1.is_infinite() && r[1].1.is_infinite(), "the ends of the front are unbounded");
    }

    #[test]
    fn evolution_reaches_a_front_with_a_trade_off_on_it() {
        // two objectives that fight: distance to (1,0) and distance to (0,1)
        let eval = |p: &P| vec![((p.0 - 1.0).powi(2) + p.1.powi(2)).sqrt(), (p.0.powi(2) + (p.1 - 1.0).powi(2)).sqrt()];
        let random = |s: u64| P((hash(s, 1, 0) % 1000) as f32 / 1000.0, (hash(s, 2, 0) % 1000) as f32 / 1000.0);
        let out = evolve(7, Budget::evals(600, 24), &[], random, eval, &[1.0, 1.0]);
        assert!(out.front.len() >= 3, "a spread front: {}", out.front.len());
        // the front spans the trade-off: some near (1,0), some near (0,1)
        let near_a = out.front.iter().any(|r| r.objectives[0] < 0.3);
        let near_b = out.front.iter().any(|r| r.objectives[1] < 0.3);
        assert!(near_a && near_b, "both ends represented: {:?}", out.front.iter().map(|r| &r.objectives).collect::<Vec<_>>());
        assert!(out.evaluations <= 600 + 24);
        assert!(*out.history.last().unwrap() <= out.history[0]);
        // weights pick different corners of the same front
        let a = pick(&out.front, &[1.0, 0.1]).unwrap();
        let b = pick(&out.front, &[0.1, 1.0]).unwrap();
        assert!(a.objectives[0] <= b.objectives[0]);
        // three diverse ones are three different ones
        let d = diverse(&out.front, 3);
        assert_eq!(d.len(), 3.min(out.front.len()));
        assert!(d.windows(2).all(|w| w[0].objectives != w[1].objectives));
    }

    #[test]
    fn a_budget_of_evaluations_is_honoured_and_a_deadline_ends_it() {
        let eval = |p: &P| vec![p.0, p.1];
        let random = |s: u64| P((hash(s, 1, 0) % 1000) as f32 / 1000.0, (hash(s, 2, 0) % 1000) as f32 / 1000.0);
        let out = evolve(1, Budget::evals(50, 10), &[], random, eval, &[1.0, 1.0]);
        assert!(out.evaluations >= 50 && out.evaluations < 70, "{}", out.evaluations);
        let slow = |p: &P| { std::thread::sleep(Duration::from_millis(2)); vec![p.0, p.1] };
        let t0 = Instant::now();
        let out = evolve(1, Budget::timed(Duration::from_millis(40), 8), &[], random, slow, &[1.0, 1.0]);
        assert!(t0.elapsed() < Duration::from_millis(120), "stopped near the deadline: {:?}", t0.elapsed());
        assert!(out.generations >= 1);
    }

    #[test]
    fn the_archive_seeds_the_next_run_and_keeps_only_the_front() {
        let eval = |p: &P| vec![((p.0 - 1.0).powi(2) + p.1.powi(2)).sqrt(), (p.0.powi(2) + (p.1 - 1.0).powi(2)).sqrt()];
        let random = |s: u64| P((hash(s, 1, 0) % 1000) as f32 / 1000.0, (hash(s, 2, 0) % 1000) as f32 / 1000.0);
        let mut arch: Archive<P> = Archive::default();
        let first = evolve(3, Budget::evals(200, 16), &[], random, eval, &[1.0, 1.0]);
        arch.offer("corner", &first.front);
        assert!(!arch.is_empty() && arch.len() <= arch.cap);
        // seeded: the first generation already contains the archive
        let seeded = evolve(4, Budget::evals(16, 16), &arch.seeds("corner"), random, eval, &[1.0, 1.0]);
        let unseeded = evolve(4, Budget::evals(16, 16), &[], random, eval, &[1.0, 1.0]);
        assert!(seeded.history[0] <= unseeded.history[0], "starts where the last one finished: {} vs {}",
                seeded.history[0], unseeded.history[0]);
        // a dominated offer does not displace the front
        let mut before = arch.seeds("corner").iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(); before.sort();
        arch.offer("corner", &[Ranked { genome: P(0.5, 0.5), objectives: vec![9.0, 9.0], rank: 0, crowding: 0.0 }]);
        let mut after = arch.seeds("corner").iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(); after.sort();
        assert_eq!(after, before, "the same set (order is by crowding, which is recomputed)");
        assert_eq!(after.iter().collect::<std::collections::BTreeSet<_>>().len(), after.len(), "no duplicates");
    }

    #[test]
    fn determinism_with_an_evaluation_budget() {
        let eval = |p: &P| vec![p.0 + p.1, (p.0 - p.1).abs()];
        let random = |s: u64| P((hash(s, 1, 0) % 1000) as f32 / 1000.0, (hash(s, 2, 0) % 1000) as f32 / 1000.0);
        let a = evolve(9, Budget::evals(120, 12), &[], random, eval, &[1.0, 1.0]);
        let b = evolve(9, Budget::evals(120, 12), &[], random, eval, &[1.0, 1.0]);
        assert_eq!(a.front, b.front);
    }
}
