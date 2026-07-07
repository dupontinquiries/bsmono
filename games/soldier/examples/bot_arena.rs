//! bot_arena.rs — runs the headless genetic-AI simulator end to end: seeds a population of `Genome`s,
//! breeds them across generations of round-based duels in a real level, and prints the fitness/diversity
//! trace plus the final champions with their emergent "personality" read. This is the bot simulator itself,
//! runnable standalone:  cargo run --example bot_arena --release

use early_fps::evolve::{evolve, EvolutionConfig};
use early_fps::genome::Genome;
use early_fps::level::{Level, PRESETS};

fn main() {
    let preset = PRESETS.iter().find(|p| p.name == "Colosseum Trenches").expect("arena preset");
    let level = Level::generate(preset.config(11));

    let cfg = EvolutionConfig { pop: 30, generations: 25, mutation_rate: 0.14, mutation_amount: 0.28, elite: 2, rounds: 4, duel_time: 18.0, seed: 20260701 };
    println!("=== Genetic bot arena ===");
    println!("population {}  generations {}  arena '{}'", cfg.pop, cfg.generations, preset.name);
    println!();

    let (ranked, stats) = evolve(&level, cfg);

    println!("{:>4} {:>12} {:>12} {:>11}  champion", "gen", "best", "mean", "diversity");
    for s in &stats {
        println!(
            "{:>4} {:>12.2} {:>12.2} {:>11.3}  {}",
            s.gen, s.best_fitness, s.mean_fitness, s.diversity, s.best_genome.nickname()
        );
    }

    println!();
    println!("=== Final champions (ranked) ===");
    for (i, g) in ranked.iter().take(8).enumerate() {
        let a = g.as_array();
        println!(
            "{:>2}. {:<14} aggr {:.2}  aware {:.2}  flank {:.2}  react {:.2}  aim {:.2}  persist {:.2}",
            i + 1,
            g.nickname(),
            a[0],
            a[1],
            a[2],
            a[3],
            a[4],
            a[5]
        );
    }

    // How far the champion has drifted from every hand-authored archetype — a rough "did it discover
    // something new" signal versus "it just rediscovered a preset we already knew about".
    println!();
    println!("=== Champion vs hand-authored archetypes (trait distance) ===");
    let champ = ranked[0];
    for (name, arche) in Genome::ARCHETYPES {
        println!("{:<14} {:.3}", name, Genome::distance(&champ, &arche));
    }

    // Save the bred population to disk so it survives past this process — `enemy::populate` picks it up
    // automatically the next time the game (or any example) starts, via `load_default_pool`.
    match early_fps::evolve::save_pool(early_fps::evolve::DEFAULT_POOL_PATH, &ranked) {
        Ok(()) => println!("\nSaved {} evolved genomes to {}", ranked.len(), early_fps::evolve::DEFAULT_POOL_PATH),
        Err(e) => println!("\nCould not save evolved pool: {e}"),
    }
    // Also install it in this process, in case something downstream in the same run wants it immediately.
    early_fps::enemy::set_evolved_pool(ranked.clone());
}
