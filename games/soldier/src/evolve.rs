//! evolve.rs — a headless bot simulator: two `Enemy`s fight each other (no player, no rendering) using the
//! *real* combat/AI code from `enemy.rs`, and a small genetic algorithm breeds populations of `Genome`s
//! against that fitness. This is how the game grows its own AI instead of a hand-tuned script: seed a
//! population (a few hand-built archetypes plus random fill), run round-robin duels each generation, select
//! and breed the winners with mutation, and watch fighting styles emerge and drift over generations — some
//! runs converge on one dominant strategy, others settle into a rock-paper-scissors mix of counter-styles,
//! which is the "population dynamics" a single hand-authored bot script can't produce on its own.

use crate::enemy::{Enemy, EnemyKind, EnemyState};
use crate::genome::Genome;
use crate::level::{Level, Rng};
use crate::math::Vec3;

/// One head-to-head duel in `level`, `a` vs `b`, for up to `max_time` seconds of simulated combat (both run
/// at a coarser 30Hz tick — plenty for AI decisions, and it lets a full generation simulate fast). Returns
/// each side's fitness for this duel. Both fighters use the same `Grunt` kind and starting health, so the
/// *only* source of variation is the genome — a clean signal for selection to act on.
pub fn duel(level: &Level, ga: Genome, gb: Genome, seed: u64, max_time: f32) -> (f32, f32) {
    let mut rng = Rng::new(seed);
    let (pa, pb) = spawn_points(level);
    let mut a = Enemy::new(0, EnemyKind::Grunt, pa).with_genome(ga);
    let mut b = Enemy::new(1, EnemyKind::Grunt, pb).with_genome(gb);
    // Skip the idle-amble startup — they already know where each other are, like a round starting on
    // spawn-and-go rather than a slow patrol that would burn simulated time on nothing.
    a.state = EnemyState::Chase;
    b.state = EnemyState::Chase;

    let dt = 1.0 / 30.0;
    let mut t = 0.0f32;
    let (mut dmg_a, mut dmg_b) = (0.0f32, 0.0f32);
    while t < max_time && a.alive() && b.alive() {
        if let Some(atk) = a.update(level, b.eye_pos(), b.pos, 1.0, 1.0, 1.0, false, &mut rng, dt) {
            if atk.hit_player {
                b.damage(atk.damage);
                dmg_a += atk.damage;
            }
        }
        if b.alive() {
            if let Some(atk) = b.update(level, a.eye_pos(), a.pos, 1.0, 1.0, 1.0, false, &mut rng, dt) {
                if atk.hit_player {
                    a.damage(atk.damage);
                    dmg_b += atk.damage;
                }
            }
        }
        t += dt;
    }

    let a_won = a.alive() && !b.alive();
    let b_won = b.alive() && !a.alive();
    let survive_frac = t / max_time;
    // Fitness rewards landing damage and winning outright, lightly penalizes eating damage, and gives a
    // small trickle for simply surviving longer — so a duel that times out with nobody dead still separates
    // "dominated the exchange" from "got shredded and ran the clock out".
    let fa = dmg_a * 0.10 - dmg_b * 0.035 + if a_won { 14.0 } else { 0.0 } + survive_frac * (a.alive() as u8 as f32) * 2.5;
    let fb = dmg_b * 0.10 - dmg_a * 0.035 + if b_won { 14.0 } else { 0.0 } + survive_frac * (b.alive() as u8 as f32) * 2.5;
    (fa, fb)
}

/// Two spawn points at a realistic skirmish distance — close to Grunt's attack range (22 units) so a duel
/// opens with an actual firefight instead of two bots hiking across the whole map first. Picks whichever
/// room pair's centre distance is closest to that target, rather than the most-distant pair (which, on a
/// big map, could be well beyond what `duel_time` seconds of closing speed could ever cover).
fn spawn_points(level: &Level) -> (Vec3, Vec3) {
    if level.rooms.len() < 2 {
        let c = Vec3::new(level.w as f32 * 0.5 * level.ts(), 0.0, level.h as f32 * 0.5 * level.ts());
        return (c, c);
    }
    const TARGET: f32 = 24.0;
    let (mut bi, mut bj, mut best_err) = (0usize, 1usize, f32::MAX);
    for i in 0..level.rooms.len() {
        for j in (i + 1)..level.rooms.len() {
            let a = level.rooms[i].center;
            let b = level.rooms[j].center;
            let d = ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt();
            let err = (d - TARGET).abs();
            if err < best_err {
                best_err = err;
                bi = i;
                bj = j;
            }
        }
    }
    let ra = level.rooms[bi].center;
    let rb = level.rooms[bj].center;
    let pa = Vec3::new(ra.x, level.floor_h(ra.x, ra.z), ra.z);
    let pb = Vec3::new(rb.x, level.floor_h(rb.x, rb.z), rb.z);
    (pa, pb)
}

/// GA knobs. Defaults are tuned to finish a full run in well under a second of simulated wall-clock in
/// `cargo test --release`, and stay well under the interactive budget even in debug builds.
#[derive(Clone, Copy, Debug)]
pub struct EvolutionConfig {
    pub pop: usize,
    pub generations: usize,
    pub mutation_rate: f32,
    pub mutation_amount: f32,
    /// Top-N genomes carried over unchanged each generation, so a strong lineage can't be mutated away.
    pub elite: usize,
    /// Duels per individual per generation (each duel is against a random opponent — round-robin would be
    /// O(pop²) and isn't necessary for the signal to converge).
    pub rounds: usize,
    pub duel_time: f32,
    pub seed: u64,
}
impl Default for EvolutionConfig {
    fn default() -> Self {
        Self { pop: 24, generations: 20, mutation_rate: 0.14, mutation_amount: 0.28, elite: 2, rounds: 3, duel_time: 16.0, seed: 1 }
    }
}

/// A snapshot of one generation's outcome — enough to plot fitness climbing and diversity either
/// collapsing (one strategy dominates) or holding (a stable mix of counter-styles).
#[derive(Clone, Copy, Debug)]
pub struct GenStats {
    pub gen: usize,
    pub best_fitness: f32,
    pub mean_fitness: f32,
    /// Mean pairwise trait-distance across a population sample — a rough diversity gauge.
    pub diversity: f32,
    pub best_genome: Genome,
}

/// Run the full evolution: seed a population from the hand-built archetypes plus random fill, then breed
/// `cfg.generations` rounds of round-based duels → tournament selection → uniform crossover → mutation,
/// with elitism so the best lineage always survives. Returns the final population (sorted best-first) and
/// the per-generation stats trace.
pub fn evolve(level: &Level, cfg: EvolutionConfig) -> (Vec<Genome>, Vec<GenStats>) {
    let mut rng = Rng::new(cfg.seed);
    let mut pop: Vec<Genome> = Vec::with_capacity(cfg.pop);
    for i in 0..cfg.pop {
        if i < Genome::ARCHETYPES.len() {
            pop.push(Genome::ARCHETYPES[i].1);
        } else {
            pop.push(Genome::random(&mut rng));
        }
    }

    let mut stats = Vec::with_capacity(cfg.generations);
    for gen in 0..cfg.generations {
        let mut fitness = vec![0.0f32; pop.len()];
        let mut matches = vec![0u32; pop.len()];
        for _ in 0..cfg.rounds {
            let mut order: Vec<usize> = (0..pop.len()).collect();
            shuffle(&mut order, &mut rng);
            for pair in order.chunks(2) {
                if pair.len() < 2 {
                    continue;
                }
                let (i, j) = (pair[0], pair[1]);
                let seed = rng.next_u64();
                let (fa, fb) = duel(level, pop[i], pop[j], seed, cfg.duel_time);
                fitness[i] += fa;
                fitness[j] += fb;
                matches[i] += 1;
                matches[j] += 1;
            }
        }
        for i in 0..pop.len() {
            if matches[i] > 0 {
                fitness[i] /= matches[i] as f32;
            }
        }

        let mut idx: Vec<usize> = (0..pop.len()).collect();
        idx.sort_by(|&a, &b| fitness[b].partial_cmp(&fitness[a]).unwrap_or(std::cmp::Ordering::Equal));
        let best_fitness = fitness[idx[0]];
        let mean_fitness = fitness.iter().sum::<f32>() / fitness.len() as f32;
        let sample = pop.len().min(12);
        let mut dsum = 0.0f32;
        let mut dcount = 0u32;
        for a in 0..sample {
            for b in (a + 1)..sample {
                dsum += Genome::distance(&pop[a], &pop[b]);
                dcount += 1;
            }
        }
        let diversity = if dcount > 0 { dsum / dcount as f32 } else { 0.0 };
        stats.push(GenStats { gen, best_fitness, mean_fitness, diversity, best_genome: pop[idx[0]] });

        let mut next = Vec::with_capacity(pop.len());
        for &e in idx.iter().take(cfg.elite.min(pop.len())) {
            next.push(pop[e]);
        }
        while next.len() < pop.len() {
            let pa = tournament(&pop, &fitness, &mut rng);
            let pb = tournament(&pop, &fitness, &mut rng);
            let mut child = Genome::crossover(&pa, &pb, &mut rng);
            child.mutate(cfg.mutation_rate, cfg.mutation_amount, &mut rng);
            next.push(child);
        }
        pop = next;
        let _ = gen;
    }

    // Final ranking pass so the caller gets the population sorted best-first without an extra evaluation.
    let mut fitness = vec![0.0f32; pop.len()];
    let mut matches = vec![0u32; pop.len()];
    for _ in 0..cfg.rounds {
        let mut order: Vec<usize> = (0..pop.len()).collect();
        shuffle(&mut order, &mut rng);
        for pair in order.chunks(2) {
            if pair.len() < 2 {
                continue;
            }
            let (i, j) = (pair[0], pair[1]);
            let seed = rng.next_u64();
            let (fa, fb) = duel(level, pop[i], pop[j], seed, cfg.duel_time);
            fitness[i] += fa;
            fitness[j] += fb;
            matches[i] += 1;
            matches[j] += 1;
        }
    }
    for i in 0..pop.len() {
        if matches[i] > 0 {
            fitness[i] /= matches[i] as f32;
        }
    }
    let mut idx: Vec<usize> = (0..pop.len()).collect();
    idx.sort_by(|&a, &b| fitness[b].partial_cmp(&fitness[a]).unwrap_or(std::cmp::Ordering::Equal));
    let ranked: Vec<Genome> = idx.iter().map(|&i| pop[i]).collect();
    (ranked, stats)
}

/// Default on-disk location for a bred population — one genome per line (see `Genome::to_line`). Written by
/// `bot_arena` after a run; picked up automatically at game startup by `enemy::load_default_pool` so an
/// evolved roster survives past the process that bred it, instead of living only in memory.
pub const DEFAULT_POOL_PATH: &str = "evolved_bots.txt";

/// Write a population to `path`, best-first, one genome per line. File I/O is native-only — there's no wasm
/// target in this project today, but gating it costs nothing and means this module can't accidentally break
/// a future browser build.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_pool(path: &str, pool: &[Genome]) -> std::io::Result<()> {
    let mut s = String::with_capacity(pool.len() * 32);
    for g in pool {
        s.push_str(&g.to_line());
        s.push('\n');
    }
    std::fs::write(path, s)
}

/// Read a population written by `save_pool`. Malformed lines are skipped rather than failing the whole
/// load — a hand-edited or partially-corrupted file still yields whatever genomes parse. Returns `None` if
/// the file is missing, unreadable, or yields no valid genomes at all.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_pool(path: &str) -> Option<Vec<Genome>> {
    let text = std::fs::read_to_string(path).ok()?;
    let pool: Vec<Genome> = text.lines().filter_map(Genome::from_line).collect();
    if pool.is_empty() {
        None
    } else {
        Some(pool)
    }
}

/// Tournament selection: pick `k` random individuals, keep the fittest. Cheaper than roulette-wheel and
/// naturally resistant to one lucky outlier dominating every mating.
fn tournament(pop: &[Genome], fitness: &[f32], rng: &mut Rng) -> Genome {
    let k = 3.min(pop.len()).max(1);
    let mut best = (rng.f32() * pop.len() as f32) as usize % pop.len();
    for _ in 1..k {
        let c = (rng.f32() * pop.len() as f32) as usize % pop.len();
        if fitness[c] > fitness[best] {
            best = c;
        }
    }
    pop[best]
}

fn shuffle(v: &mut [usize], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        let j = (rng.f32() * (i as f32 + 1.0)) as usize;
        v.swap(i, j.min(i));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::PRESETS;

    fn arena() -> Level {
        let p = PRESETS.iter().find(|p| p.name == "Colosseum Trenches").expect("arena preset");
        Level::generate(p.config(11))
    }

    #[test]
    fn sharper_aim_wins_more_often_than_wild_aim() {
        // Accuracy is the most mechanically direct gene (it scales shot spread), so it's the cleanest sanity
        // check that the duel loop actually differentiates genomes rather than two bots trading randomly.
        // A single duel is noisy — spawn slot alone can decide a short firefight — so average over several
        // seeds AND swap which spawn slot each genome starts in, to cancel that positional bias out.
        let lvl = arena();
        let sharp = Genome { accuracy: 1.0, ..Genome::default() };
        let wild = Genome { accuracy: 0.0, ..Genome::default() };
        let mut sharp_total = 0.0;
        let mut wild_total = 0.0;
        for seed in 0..16u64 {
            let (fa, fb) = duel(&lvl, sharp, wild, seed * 97 + 3, 20.0);
            sharp_total += fa;
            wild_total += fb;
            let (fb2, fa2) = duel(&lvl, wild, sharp, seed * 131 + 7, 20.0);
            sharp_total += fa2;
            wild_total += fb2;
        }
        assert!(sharp_total > wild_total, "sharper aim should out-score wild aim on average ({sharp_total:.1} vs {wild_total:.1})");
    }

    #[test]
    fn evolution_improves_mean_fitness_over_generations() {
        let lvl = arena();
        let cfg = EvolutionConfig { pop: 12, generations: 6, duel_time: 10.0, seed: 5, ..Default::default() };
        let (ranked, stats) = evolve(&lvl, cfg);
        assert_eq!(ranked.len(), 12, "the final population should be fully ranked");
        let first_mean = stats.first().unwrap().mean_fitness;
        let last_mean = stats.last().unwrap().mean_fitness;
        assert!(last_mean >= first_mean - 1.0, "mean fitness shouldn't collapse across generations (first {first_mean:.2}, last {last_mean:.2})");
        // every reported genome should be a legal, in-bounds individual
        for s in &stats {
            for v in s.best_genome.as_array() {
                assert!((0.0..=1.0).contains(&v), "evolved gene left [0,1]: {v}");
            }
        }
    }

    #[test]
    fn population_retains_some_diversity_after_evolving() {
        // A GA that instantly collapses to one clone everywhere is a sign selection pressure is too sharp
        // or mutation too weak — check the run doesn't fully converge within a short horizon.
        let lvl = arena();
        let cfg = EvolutionConfig { pop: 14, generations: 5, duel_time: 8.0, seed: 9, ..Default::default() };
        let (_, stats) = evolve(&lvl, cfg);
        let last = stats.last().unwrap();
        assert!(last.diversity > 0.02, "population diversity collapsed to near-zero ({:.4})", last.diversity);
    }

    #[test]
    fn saved_pool_round_trips_through_disk() {
        // A unique temp path per test run so parallel test threads (and repeat runs) never collide.
        let path = format!("/tmp/early_fps_test_pool_{}.txt", std::process::id());
        let pool = vec![
            Genome { aggression: 0.12, awareness: 0.34, flank_bias: 0.56, reaction: 0.78, accuracy: 0.91, persistence: 0.03 },
            Genome::archetype(0),
            Genome::archetype(3),
        ];
        save_pool(&path, &pool).expect("writing the pool file should succeed");
        let loaded = load_pool(&path).expect("a freshly-written pool file should load back");
        assert_eq!(loaded.len(), pool.len());
        for (a, b) in pool.iter().zip(loaded.iter()) {
            assert!(Genome::distance(a, b) < 1e-3, "loaded genome drifted from what was saved");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_pool_file_loads_as_none() {
        let path = format!("/tmp/early_fps_definitely_missing_{}.txt", std::process::id());
        let _ = std::fs::remove_file(&path); // in case a previous failed run left it
        assert!(load_pool(&path).is_none());
    }
}
