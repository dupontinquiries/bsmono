//! procgen_demo — exercise the recursive noise-driven generator and report what it built.
//!
//! Shows, for several seeds and a couple of "taste" nets, the recursion depth reached, the leaf count, the
//! archetype mix the net activated, and how the result scores on the composition metric — so the whole
//! feed-forward pipeline (noise → diff → net → FSM → recurse) is visible end to end.
//!
//! Usage: cargo run --example procgen_demo --release

use early_fps::composition;
use early_fps::mapeval;
use early_fps::procgen::{generate, ActivationNet, Archetype, ProcParams};

fn arch_name(i: usize) -> &'static str {
    match Archetype::ALL[i] {
        Archetype::SightlineBreaker => "break",
        Archetype::Vantage => "vant",
        Archetype::Sentinel => "sentl",
        Archetype::Spine => "spine",
        Archetype::Wreckage => "wreck",
        Archetype::Shelter => "shltr",
        Archetype::Void => "void",
    }
}

fn main() {
    println!("Recursive noise-driven generator — noise → diff → tiny net → FSM archetype → recurse to char scale\n");

    let params = ProcParams::default();

    // three taste nets: the default, a "dense cover" net, and a "sparse/open" net
    let default_net = ActivationNet::default();
    let mut dense = ActivationNet::default();
    dense.w2[0][8] += 1.2; // more sightline-breakers (output bias)
    dense.w2[5][8] += 1.2; // more shelters
    dense.w2[6][8] -= 2.0; // fewer voids
    let mut sparse = ActivationNet::default();
    sparse.w2[6][8] += 2.5; // more voids (open space)

    for (label, net) in [("default", &default_net), ("dense", &dense), ("sparse", &sparse)] {
        println!("── taste: {label} ──");
        println!("{:>5}  {:>5} {:>5} {:>5}   {:<38}  {:>6} {:>6} {:>6}", "seed", "depth", "leaf", "obst", "archetype mix", "comp", "noise", "grade");
        for seed in [1u64, 2, 3, 7, 42] {
            let (level, rep) = generate(seed, net, &params);
            let comp = composition::analyze(&level);
            let grade = composition::honest_grade(&level);
            let old_grade = mapeval::score(&level).grade();
            // archetype mix string
            let total: u32 = rep.placed.iter().sum();
            let mut mix = String::new();
            for (i, &c) in rep.placed.iter().enumerate() {
                if c > 0 {
                    mix.push_str(&format!("{}:{} ", arch_name(i), c));
                }
            }
            let _ = total;
            println!(
                "{:>5}  {:>5} {:>5} {:>5}   {:<38}  {:>6.2} {:>6.2}   {}/{}",
                seed, rep.max_depth, rep.leaves, level.obstacles.len(), mix.trim(), comp.score, comp.noise_likeness, grade, old_grade
            );
        }
        println!();
    }

    println!("comp = composition score (designed-ness). noise = noise-likeness (1=looks random).");
    println!("grade = honest/old. Swapping the net's weights (taste) visibly changes the archetype mix on the");
    println!("same seed — the net is a pure, swappable taste function over a fixed noise substrate + recursion.\n");

    // ── evolutionary tuning against the honest metric ──────────────────────────────────────────────────
    println!("── evolving taste against the HONEST composition fitness (swarm/genetic) ──");
    let train_seeds = [1u64, 2, 3, 4, 5];
    let base = ActivationNet::default();
    let base_fit: f32 = train_seeds.iter().map(|&s| {
        let (l, _) = early_fps::procgen::generate(s, &base, &params);
        composition::honest_quality(&l)
    }).sum::<f32>() / train_seeds.len() as f32;
    println!("hand-tuned default: mean honest_quality = {:.3}", base_fit);
    let (evolved, evolved_fit) = early_fps::procgen::evolve_taste(&train_seeds, &params, 8, 12, 7);
    println!("evolved net:        mean honest_quality = {:.3}  ({:+.3})", evolved_fit, evolved_fit - base_fit);
    // show the evolved net's behaviour on a held-out seed
    let (lvl, rep) = early_fps::procgen::generate(99, &evolved, &params);
    let comp = composition::analyze(&lvl);
    let mut mix = String::new();
    for (i, &c) in rep.placed.iter().enumerate() {
        if c > 0 { mix.push_str(&format!("{}:{} ", arch_name(i), c)); }
    }
    println!("evolved on held-out seed 99: obst={} comp={:.2} noise={:.2} grade={}  mix: {}",
        lvl.obstacles.len(), comp.score, comp.noise_likeness, composition::honest_grade(&lvl), mix.trim());
    println!("\nThe evolutionary loop tunes the net's taste automatically — and because the fitness (composition)");
    println!("can't be gamed by adding clutter, it converges toward genuinely well-composed layouts, not dense ones.");
}
