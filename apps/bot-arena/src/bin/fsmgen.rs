//! fsmgen — generate a random weighted-FSM bot genome, print the command it emits,
//! and optionally export its structure to SVG (the heroes-gallery export).
//!
//!   fsmgen [--states N] [--max-conn M] [--steps K] [--seed S] [--svg out.svg]

#[path = "../fsm.rs"]
mod fsm;

use rand::{Rng, SeedableRng};

fn arg(name: &str, default: usize) -> usize {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
}
fn arg_str(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn main() {
    let states = arg("--states", 8);
    let max_conn = arg("--max-conn", 3);
    let steps = arg("--steps", 40);
    let seed = arg_str("--seed").and_then(|s| s.parse::<u64>().ok());

    // --evolve <target>: run the FSM GA toward a target string and export the winner.
    if let Some(target) = arg_str("--evolve") {
        let s = seed.unwrap_or(1);
        let (best, out, fit) = fsm::evolve_toward(&target, states.max(target.len()), max_conn, 60, 60, s);
        println!("evolved toward {target:?}: best output {out:?} (fitness {fit:.3})");
        if let Some(path) = arg_str("--svg") {
            let _ = std::fs::write(&path, best.to_svg());
            println!("wrote hero SVG → {path}");
        }
        return;
    }

    let mut rng: rand::rngs::StdRng = match seed {
        Some(s) => rand::rngs::StdRng::seed_from_u64(s),
        None => rand::rngs::StdRng::seed_from_u64(rand::thread_rng().gen()),
    };

    let f = fsm::Fsm::random(states, max_conn, &mut rng);
    println!("FSM: {} states, ≤{} connections/state", f.n_states(), max_conn);
    println!("emitted command:\n  {}", f.run(steps, &mut rng));

    if let Some(path) = arg_str("--svg") {
        match std::fs::write(&path, f.to_svg()) {
            Ok(_) => println!("wrote SVG → {path}"),
            Err(e) => { eprintln!("could not write {path}: {e}"); std::process::exit(1); }
        }
    }
}
