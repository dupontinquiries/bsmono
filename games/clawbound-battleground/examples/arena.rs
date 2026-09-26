// The arena: the playbook's pages play each other headless, are rated, and
// breed (src/arena.rs).
//
//   cargo run --release --example arena -- [generations] [max_minutes] [--write] [--threads N]
//
// Prints every pairing as it finishes and the table after each
// generation. With --write, the best strains become league pages in
// dev/arena/booklet.txt (read by the playbook at build time).
use clawbound::arena::{real_match, League};
use clawbound::playbook::{self, Page};
use std::sync::mpsc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let nums: Vec<f32> = args.iter().filter_map(|a| a.parse().ok()).collect();
    let generations = nums.first().copied().unwrap_or(4.0) as u32;
    let max_t = nums.get(1).copied().unwrap_or(8.0) * 60.0;
    let write = args.iter().any(|a| a == "--write");
    let threads = args.iter().position(|a| a == "--threads").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(2usize);

    let mut league = League::new(playbook::old_ways(), 0xA4E7A);
    println!("arena: {} main pages, {} generations, {:.0} min cap, {} threads", league.entries.len(), generations, max_t / 60.0, threads);
    let t0 = std::time::Instant::now();
    for _ in 0..generations {
        for n in league.evolve(0.25) {
            println!("  {n}");
        }
        // Plan the round, play its games on `threads` workers, record in
        // order: the bookkeeping stays single-threaded and deterministic;
        // only the simulations run in parallel.
        let plan = league.plan();
        let games: Vec<(Page, Page, u64)> = plan
            .iter()
            .flat_map(|p| {
                let (a, b) = (league.entries[p.a].page.clone(), league.entries[p.b].page.clone());
                [(a.clone(), b.clone(), p.seed), (b, a, p.seed)]
            })
            .collect();
        let results = run_parallel(&games, threads, max_t);
        for (k, p) in plan.into_iter().enumerate() {
            let s = league.record_pairing(p, results[2 * k], results[2 * k + 1]);
            println!("  {:<28} vs {:<28} {:>4.0}%", league.entries[p.a].page.name, league.entries[p.b].page.name, s * 100.0);
        }
        print!("{}", league.table());
        println!("  ({:.0}s so far)", t0.elapsed().as_secs_f32());
    }
    if write {
        let champs = league.champions(4);
        let mut out = String::from("# League pages bred by the arena (cargo run --release --example arena).\n# name|lore|commit give retreat leash tower strafe reach focus group rally objective hold\n");
        for c in &champs {
            out += &format!("# elo {:.0} over {} games, generation {}\n", c.elo, c.games, c.generation);
            out += &playbook::page_line(&c.page);
            out += "\n";
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/dev/arena/booklet.txt");
        std::fs::write(path, out).expect("write booklet");
        println!("wrote {} league page(s) to {path}", champs.len());
    }
}

fn run_parallel(plan: &[(Page, Page, u64)], threads: usize, max_t: f32) -> Vec<f32> {
    let (tx, rx) = mpsc::channel();
    let chunks: Vec<Vec<(usize, (Page, Page, u64))>> = (0..threads.max(1))
        .map(|w| plan.iter().cloned().enumerate().filter(|(i, _)| i % threads.max(1) == w).collect())
        .collect();
    std::thread::scope(|s| {
        for chunk in chunks {
            let tx = tx.clone();
            s.spawn(move || {
                let mut m = real_match(5, max_t);
                for (i, (a, b, seed)) in chunk {
                    tx.send((i, m(&a, &b, seed))).unwrap();
                }
            });
        }
    });
    drop(tx);
    let mut out = vec![0.5; plan.len()];
    for (i, r) in rx {
        out[i] = r;
    }
    out
}
