//! Spatial population diffusion — gene flow across a barrier.
//!
//! Two founding populations, A and B, start on opposite banks of a transect.
//! Each generation individuals migrate (how readily is set by their **curiosity**
//! gene), and where lineages meet they interbreed — an offspring's *admixture*
//! (0 = pure A, 1 = pure B) is the average of its two parents. The middle cell is
//! a river: either passable (lineages can mix across it) or an impassable barrier
//! (they can't). Same starting layout both ways, so the barrier is the only thing
//! that changes — which lets us measure how much a river actually slows the
//! blending of two populations, and how much curiosity moves that needle.
//!
//! Pure and deterministic in its seeds, so a run replays identically and the
//! across-runs variance is a real property of the model, not noise in the harness.

use crate::sim::fixed::Fixed;

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One creature on the transect: which cell it's in and its ancestry proportion.
#[derive(Debug, Clone, Copy)]
pub struct Individual {
    pub cell: i32,
    /// 0 = pure population A, 1 = pure population B.
    pub admixture: Fixed,
}

/// The transect and its rules.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub width: i32,
    /// The river cell; A founds the cells left of it, B the cells right of it.
    pub barrier_cell: i32,
    /// Whether the river is an impassable barrier (true) or just open water (false).
    pub barrier: bool,
    /// Carrying capacity per cell.
    pub per_cell: usize,
    /// Migration tendency, the curiosity gene — chance per generation of stepping.
    pub curiosity: Fixed,
    pub generations: u32,
}

impl Config {
    /// A 9-cell transect, river at cell 4, capacity 8, 100 generations.
    pub fn new(curiosity: Fixed, barrier: bool) -> Config {
        Config { width: 9, barrier_cell: 4, barrier, per_cell: 8, curiosity, generations: 100 }
    }
}

/// Outcome of one run.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// How blended the whole population is: 0 = fully segregated (everyone pure
    /// A or pure B), 1 = everyone an even 50/50 mix.
    pub mixing: f32,
    /// Mean admixture on the A bank (left of the river) — B-ancestry that crossed.
    pub left_admixture: f32,
    /// Mean admixture on the B bank (right of the river).
    pub right_admixture: f32,
    pub population: usize,
}

/// Founding layout: A (admixture 0) fills cells left of the river, B (1) the
/// cells right of it; the river cell itself starts empty.
pub fn seed_pop(cfg: &Config) -> Vec<Individual> {
    let mut pop = Vec::new();
    for cell in 0..cfg.width {
        if cell == cfg.barrier_cell {
            continue;
        }
        let admixture = if cell < cfg.barrier_cell { Fixed::ZERO } else { Fixed::ONE };
        for _ in 0..cfg.per_cell {
            pop.push(Individual { cell, admixture });
        }
    }
    pop
}

fn migrate(pop: &mut [Individual], cfg: &Config, gen: u32, run_seed: u64) {
    let thresh = (cfg.curiosity.to_f32() * 1000.0) as u64;
    for (i, ind) in pop.iter_mut().enumerate() {
        let h = splitmix(run_seed ^ (gen as u64).wrapping_mul(0x9E37) ^ (i as u64).wrapping_mul(0x2545_F491));
        if h % 1000 >= thresh {
            continue; // stays put this generation
        }
        let dir = if (h >> 12) & 1 == 0 { -1 } else { 1 };
        let target = ind.cell + dir;
        if target < 0 || target >= cfg.width {
            continue;
        }
        if cfg.barrier && target == cfg.barrier_cell {
            continue; // the river blocks the crossing
        }
        ind.cell = target;
    }
}

/// Within-cell panmictic reproduction at carrying capacity; offspring admixture is
/// the average of two parents from the same cell. Empty cells produce nothing
/// (recolonized only by migration), so the population stays bounded.
fn reproduce(pop: &mut Vec<Individual>, cfg: &Config, gen: u32, run_seed: u64) {
    // Bucket residents by cell.
    let mut by_cell: Vec<Vec<Fixed>> = vec![Vec::new(); cfg.width as usize];
    for ind in pop.iter() {
        by_cell[ind.cell as usize].push(ind.admixture);
    }
    let mut next = Vec::with_capacity(pop.len());
    let half = Fixed::from_frac(1, 2);
    for (cell, residents) in by_cell.iter().enumerate() {
        if residents.is_empty() {
            continue;
        }
        for k in 0..cfg.per_cell {
            let h = splitmix(
                run_seed
                    ^ (gen as u64).wrapping_mul(0x1000_0033)
                    ^ (cell as u64).wrapping_mul(0x9E37_79B9)
                    ^ (k as u64).wrapping_mul(0xD1B5_4A32),
            );
            let pa = residents[(h % residents.len() as u64) as usize];
            let pb = residents[((h >> 20) % residents.len() as u64) as usize];
            next.push(Individual { cell: cell as i32, admixture: (pa + pb) * half });
        }
    }
    *pop = next;
}

/// Run one simulation to completion and report how blended the populations got.
pub fn run(cfg: &Config, run_seed: u64) -> Report {
    let mut pop = seed_pop(cfg);
    for gen in 0..cfg.generations {
        migrate(&mut pop, cfg, gen, run_seed);
        reproduce(&mut pop, cfg, gen, run_seed);
    }
    let n = pop.len().max(1) as f32;
    let mut mix_sum = 0.0f32;
    let (mut left_sum, mut left_n, mut right_sum, mut right_n) = (0.0f32, 0u32, 0.0f32, 0u32);
    for ind in &pop {
        let a = ind.admixture.to_f32();
        mix_sum += 2.0 * a.min(1.0 - a); // 0 if pure, 1 if 50/50
        if ind.cell < cfg.barrier_cell {
            left_sum += a;
            left_n += 1;
        } else if ind.cell > cfg.barrier_cell {
            right_sum += a;
            right_n += 1;
        }
    }
    Report {
        mixing: mix_sum / n,
        left_admixture: if left_n > 0 { left_sum / left_n as f32 } else { 0.0 },
        right_admixture: if right_n > 0 { right_sum / right_n as f32 } else { 0.0 },
        population: pop.len(),
    }
}

/// Mean and standard deviation of a metric over `runs` seeds.
#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub mean: f32,
    pub std: f32,
}

/// Run `runs` independent simulations and summarize the mixing outcome's mean and
/// across-run variance — the headline experiment.
pub fn run_many(cfg: &Config, runs: u64) -> Stats {
    let xs: Vec<f32> = (0..runs).map(|s| run(cfg, s.wrapping_mul(0x100_0001).wrapping_add(1)).mixing).collect();
    let n = xs.len().max(1) as f32;
    let mean = xs.iter().sum::<f32>() / n;
    let var = xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    Stats { mean, std: var.sqrt() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur(pct: i64) -> Fixed {
        Fixed::from_frac(pct, 100)
    }

    #[test]
    fn an_impassable_river_keeps_the_populations_segregated() {
        let cfg = Config::new(cur(40), true);
        let r = run(&cfg, 12345);
        // With the river blocking crossings, A stays A and B stays B.
        assert!(r.mixing < 0.05, "barrier should keep mixing near zero, got {}", r.mixing);
        assert!(r.left_admixture < 0.1, "the A bank stays A: {}", r.left_admixture);
        assert!(r.right_admixture > 0.9, "the B bank stays B: {}", r.right_admixture);
    }

    #[test]
    fn open_water_lets_the_populations_blend() {
        let open = run(&Config::new(cur(40), false), 12345);
        let walled = run(&Config::new(cur(40), true), 12345);
        assert!(
            open.mixing > walled.mixing + 0.1,
            "without the river the populations blend more (open {} vs barrier {})",
            open.mixing,
            walled.mixing
        );
    }

    #[test]
    fn more_curiosity_drives_more_diffusion() {
        // Across open water, a more curious population blends faster/further.
        let calm = run_many(&Config::new(cur(10), false), 20);
        let curious = run_many(&Config::new(cur(70), false), 20);
        assert!(
            curious.mean > calm.mean,
            "higher curiosity → more mixing (curious {} vs calm {})",
            curious.mean,
            calm.mean
        );
    }

    #[test]
    fn runs_are_deterministic() {
        let cfg = Config::new(cur(40), false);
        assert_eq!(run(&cfg, 7).mixing, run(&cfg, 7).mixing);
    }

    #[test]
    #[ignore = "diagnostic: run with --ignored --nocapture for the diffusion numbers"]
    fn print_diffusion() {
        println!("\n  population blending after 100 generations (mean ± std over 20 runs):");
        println!("  curiosity |   river (barrier)   |   open water (no river)");
        for c in [10i64, 30, 50, 70, 90] {
            let walled = run_many(&Config::new(cur(c), true), 20);
            let open = run_many(&Config::new(cur(c), false), 20);
            println!(
                "     {:>3}%   |   {:.3} ± {:.3}     |   {:.3} ± {:.3}",
                c, walled.mean, walled.std, open.mean, open.std
            );
        }
    }
}
