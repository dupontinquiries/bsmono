//! layout_study.rs — fork breadth against sibling depth, over the whole parameter range.
//!
//! The probe wants at least eight distinct forks per accumulation window. The search wanted deep
//! fanout from promising checkpoints. This prints what that trade actually costs.
//!
//! Run: `cargo run --example layout_study`

use gpu_kernels::probe::MIN_EFFECTIVE_N;
use gpu_kernels::reap::slot_yield;
use gpu_kernels::round::{best_k, best_layout, floor_cost, layouts, p_success, p_success_ranked, q_by_rank, Layout};

fn main() {
    let slots: usize = std::env::var("SLOTS").ok().and_then(|s| s.parse().ok()).unwrap_or(32);
    let min_forks = MIN_EFFECTIVE_N as usize;

    println!("{slots} slots. P(round yields >=1 success), by layout.");
    println!("q = fork viability, p = per-child success given a viable fork.\n");

    for (q, p) in [(1.0f32, 0.05f32), (0.6, 0.05), (0.3, 0.10), (0.1, 0.20)] {
        println!("q={q:.1} p={p:.2}");
        print!("  ");
        for l in layouts(slots) { print!("{:>10}", format!("{}x{}", l.forks, l.per_fork)); }
        println!();
        print!("  ");
        for l in layouts(slots) { print!("{:>10.4}", p_success(l, q, p)); }
        let b = best_layout(slots, q, p, 1);
        println!("\n  best unconstrained: {}x{}   cost of an {min_forks}-fork floor: {:.4}%\n",
                 b.forks, b.per_fork, 100.0 * floor_cost(slots, q, p, min_forks));
    }

    println!("worst floor cost anywhere in the grid:");
    let (mut worst, mut at) = (0.0f32, (0.0f32, 0.0f32));
    for qi in 1..=10 {
        for pi in 1..=20 {
            let (q, p) = (qi as f32 / 10.0, pi as f32 / 200.0);
            let c = floor_cost(slots, q, p, min_forks);
            if c > worst { worst = c; at = (q, p); }
        }
    }
    println!("  {:.6}% at q={:.1} p={:.3}\n", 100.0 * worst, at.0, at.1);

    println!("== but forks come from survivors, and survivor 8 is not survivor 1 ==");
    println!("q(rank i) = top * decay^i. Optimal k = survivors to keep, with slots/k children each.\n");
    println!("{:>8} {:>8} {:>10} {:>12} {:>16} {:>14}",
             "top q", "decay", "best k", "per fork", "P(success)", "rounds to probe");
    println!("{}", "-".repeat(72));
    for top in [0.8f32, 0.5] {
        for decay in [1.0f32, 0.9, 0.7, 0.55, 0.3] {
            let (k, per) = best_k(slots, top, decay, 0.10, 1);
            let ps = p_success_ranked(&q_by_rank(top, decay, k), per, 0.10);
            println!("{:>8.1} {:>8.2} {:>10} {:>12} {:>16.4} {:>14}",
                     top, decay, k, per, ps, Layout { forks: k, per_fork: per }.rounds_to_probe(min_forks));
        }
    }
    println!();
    println!("the design's \"keep top 4, fork x8\" against the {min_forks}-fork floor, at top=0.8 decay=0.55:");
    for k in [4usize, 8, 16] {
        let per = slots / k;
        let ps = p_success_ranked(&q_by_rank(0.8, 0.55, k), per, 0.10);
        let lay = Layout { forks: k, per_fork: per };
        println!("  keep {k:>2}, fork x{per:<2}  P={ps:.4}  probe needs {} round(s)", lay.rounds_to_probe(min_forks));
    }
    println!();

    println!("time to a first validated direction, and what each round buys the search:");
    println!("{:>10} {:>10} {:>16} {:>14}", "layout", "rounds", "children/slot", "P(success)");
    println!("{}", "-".repeat(54));
    let yield_ = slot_yield(0.9, 0.2);
    for l in layouts(slots) {
        let eff = Layout { forks: l.forks, per_fork: (l.per_fork as f32 * yield_).round() as usize };
        println!("{:>10} {:>10} {:>16.1} {:>14.4}",
                 format!("{}x{}", l.forks, l.per_fork),
                 l.rounds_to_probe(min_forks),
                 yield_,
                 p_success(eff, 0.3, 0.10));
    }
    println!("\nreaping multiplies per_fork, never forks — depth is bought by turnover, breadth by layout.");
}
