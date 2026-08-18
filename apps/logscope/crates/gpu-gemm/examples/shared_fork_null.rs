//! shared_fork_null.rs — what a shared prefix does to the sample size, and to the null.
//!
//! `probe_null.rs` drew 32 independent Gaussian rows. Real captures are not independent: branches
//! forked from one prefix run identical tokens through identical weights until they diverge, so
//! shortly after the fork their residuals are near-copies.
//!
//! The study compares two ways of cross-validating the same data. Ungrouped CV holds out ROWS, so a
//! fold is scored on siblings of its own training rows. Grouped CV holds out whole FORKS, which is
//! the only version that answers the question anyone cares about: will this direction work on the
//! next fork?
//!
//! Run: `cargo run --example shared_fork_null`

use gpu_kernels::probe::{self, LayerSweep, Lcg};

/// One round of captures with a shared component. `rho` is the fraction of each row's variance that
/// comes from the fork rather than from the branch, so 0.0 is the independent case and 0.9 is a
/// capture taken a few tokens past a long shared prefix.
/// `homogeneity` is the probability a branch takes its FORK's outcome rather than a coin flip.
/// This is the parameter that matters and the one the first version of this study left out: forking
/// from a promising checkpoint means the children mostly succeed together, so outcome is correlated
/// with fork identity. When it is, a row-held-out probe can score perfectly by learning which fork a
/// row came from — the shared prefix component tells it, and fork identity tells it the label.
fn round(d: usize, forks: usize, per_fork: usize, rho: f32, effect: f32, homogeneity: f32,
         rng: &mut Lcg, one_fork_per_row: bool) -> LayerSweep {
    let a = rho.sqrt();
    let b = (1.0 - rho).sqrt();
    let mut s = LayerSweep::new(20, d);
    let mut row = 0usize;
    for f in 0..forks {
        // Everything this fork's branches have in common: the prefix they all ran.
        let shared: Vec<f32> = (0..d).map(|_| rng.normal()).collect();
        let fork_ok = f % 2 == 0;
        for _ in 0..per_fork {
            let u = ((rng.next_u64() >> 11) as f64) / ((1u64 << 53) as f64);
            let follows_fork = u < homogeneity as f64;
            let ok = if follows_fork { fork_ok } else { row % 2 == 0 };
            let mut h: Vec<f32> = (0..d).map(|j| a * shared[j] + b * rng.normal()).collect();
            h[0] += if ok { effect } else { -effect };
            // `one_fork_per_row` is the mistake being demonstrated: tagging each branch as its own
            // fork, which makes grouped CV silently degrade to ungrouped.
            s.push(if one_fork_per_row { row } else { f }, &h, ok);
            row += 1;
        }
    }
    s
}

fn main() {
    let d: usize = std::env::var("D").ok().and_then(|s| s.parse().ok()).unwrap_or(1024);
    let forks: usize = std::env::var("FORKS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let per_fork: usize = std::env::var("PER_FORK").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let trials: usize = std::env::var("TRIALS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let perms: usize = std::env::var("PERMS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);

    let n = forks * per_fork;
    println!("shared-fork study: d={d}, {forks} forks x {per_fork} branches = {n} rows, trials={trials}");
    println!("rho is the share of each residual that is the fork's shared prefix component.\n");

    for (planted, homog, title) in [
        (0.0f32, 0.0f32, "no signal, forks label-balanced"),
        (0.0,    1.0,    "no signal, but each fork's branches share an outcome"),
        (1.5,    1.0,    "real signal, forks outcome-homogeneous"),
    ] {
        println!("== {title} ==");
        println!("{:>6} {:>12} {:>12} {:>14} {:>14}",
                 "rho", "AUC (rows)", "AUC (forks)", "pass (rows)", "pass (forks)");
        println!("{}", "-".repeat(62));
        for rho in [0.0f32, 0.3, 0.6, 0.8, 0.9] {
            let (mut au_r, mut au_f) = (0f64, 0f64);
            let (mut p_r, mut p_f) = (0usize, 0usize);
            for t in 0..trials {
                let seed = 0x5EED + t as u64 * 104_729;
                // Same data twice; only the fork tagging differs.
                let wrong = round(d, forks, per_fork, rho, planted, homog, &mut Lcg::new(seed), true);
                let right = round(d, forks, per_fork, rho, planted, homog, &mut Lcg::new(seed), false);
                let a = wrong.evaluate(4, perms, 7).unwrap();
                let b = right.evaluate(4, perms, 7).unwrap();
                au_r += a.cv_auc as f64; au_f += b.cv_auc as f64;
                if a.cv_auc >= probe::MIN_CV_AUC && a.null_p <= probe::MAX_NULL_P { p_r += 1; }
                if b.deployable() { p_f += 1; }
            }
            let f = trials as f64;
            println!("{:>6.2} {:>12.3} {:>12.3} {:>11}/{:<3} {:>11}/{:<3}",
                     rho, au_r / f, au_f / f, p_r, trials, p_f, trials);
        }
        println!();
    }

    println!("eff_n is the FORK count, not the row count:");
    let s = round(d, forks, per_fork, 0.8, 0.0, 0.0, &mut Lcg::new(1), false);
    let r = s.evaluate(4, 0, 1).unwrap();
    println!("  {} rows over {} forks -> eff_n {:.0} (gate needs {:.0})",
             n, forks, r.eff_n, probe::MIN_EFFECTIVE_N);
}
