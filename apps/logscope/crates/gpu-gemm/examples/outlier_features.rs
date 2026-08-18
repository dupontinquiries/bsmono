//! outlier_features.rs — what heavy-tailed per-dimension scale does to a difference-of-means probe.
//!
//! Every null study so far drew residuals as i.i.d. standard normals. Real residual streams are not
//! that. A trained transformer's residual has OUTLIER FEATURES: a small number of coordinates
//! carrying an order of magnitude or more variance than the rest, present in every token, and
//! nothing to do with whichever hypothesis the branch is pursuing.
//!
//! This is a stipulated structure, not a measured one — no trained checkpoint is available here, and
//! random weights do not produce outlier features because the phenomenon is learned. So the scale
//! profile below is asserted from what the literature reports, and the study says what WOULD happen
//! if it holds. Treat the shape of the result as the finding and the exact numbers as illustration.
//!
//! The failure mode is arithmetic and does not depend on the details. A coordinate with 50x the
//! scale contributes 2500x the squared mean difference, so an unstandardised difference of means
//! points almost entirely along the loudest few coordinates — whether or not anything is written
//! there.
//!
//! Run: `cargo run --example outlier_features`

use gpu_kernels::probe::{Fit, LayerSweep, Lcg};

/// Per-dimension scale profile: `n_outlier` coordinates at `scale`, the rest at 1.
fn scales(d: usize, n_outlier: usize, scale: f32) -> Vec<f32> {
    (0..d).map(|i| if i < n_outlier { scale } else { 1.0 }).collect()
}

/// `signal_dim` is where the label information actually lives. Deliberately an ordinary
/// coordinate — the interesting case is signal in a quiet dimension next to loud noise.
fn round(d: usize, forks: usize, per_fork: usize, sc: &[f32], effect: f32,
         signal_dim: usize, fit: Fit, rng: &mut Lcg) -> LayerSweep {
    let mut s = LayerSweep::new(20, d).with_fit(fit);
    let mut row = 0usize;
    for f in 0..forks {
        for _ in 0..per_fork {
            let ok = row % 2 == 0;
            let mut h: Vec<f32> = (0..d).map(|j| sc[j] * rng.normal()).collect();
            h[signal_dim] += if ok { effect } else { -effect };
            s.push(f, &h, ok);
            row += 1;
        }
    }
    s
}

fn main() {
    let d: usize = std::env::var("D").ok().and_then(|s| s.parse().ok()).unwrap_or(1024);
    let forks: usize = std::env::var("FORKS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let per_fork: usize = std::env::var("PER_FORK").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let trials: usize = std::env::var("TRIALS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let effect: f32 = std::env::var("EFFECT").ok().and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let n_out: usize = std::env::var("N_OUTLIER").ok().and_then(|s| s.parse().ok()).unwrap_or(8);

    println!("d={d}, {forks} forks x {per_fork} branches, {n_out} outlier dims, signal (effect {effect}) in an ordinary dim");
    println!("held-out AUC, grouped by fork, {trials} trials\n");
    println!("{:>14} {:>14} {:>14} {:>10}", "outlier scale", "MeanDiff", "DiagLda", "ratio");
    println!("{}", "-".repeat(56));

    for scale in [1.0f32, 3.0, 10.0, 30.0, 100.0] {
        let sc = scales(d, n_out, scale);
        let (mut a, mut b) = (0f64, 0f64);
        for t in 0..trials {
            let seed = 0xD1A6 + t as u64 * 104_729;
            let md = round(d, forks, per_fork, &sc, effect, d / 2, Fit::MeanDiff, &mut Lcg::new(seed));
            let dl = round(d, forks, per_fork, &sc, effect, d / 2, Fit::DiagLda, &mut Lcg::new(seed));
            a += md.evaluate(4, 0, 1).unwrap().cv_auc as f64;
            b += dl.evaluate(4, 0, 1).unwrap().cv_auc as f64;
        }
        let f = trials as f64;
        println!("{:>14.0} {:>14.3} {:>14.3} {:>10.2}x", scale, a / f, b / f, (b / f) / (a / f).max(1e-9));
    }

    println!("\nand the case that should worry anyone: signal IN an outlier dimension");
    println!("{:>14} {:>14} {:>14}", "outlier scale", "MeanDiff", "DiagLda");
    println!("{}", "-".repeat(46));
    for scale in [1.0f32, 10.0, 100.0] {
        let sc = scales(d, n_out, scale);
        let (mut a, mut b) = (0f64, 0f64);
        for t in 0..trials {
            let seed = 0xBEE5 + t as u64 * 104_729;
            let md = round(d, forks, per_fork, &sc, effect * scale, 0, Fit::MeanDiff, &mut Lcg::new(seed));
            let dl = round(d, forks, per_fork, &sc, effect * scale, 0, Fit::DiagLda, &mut Lcg::new(seed));
            a += md.evaluate(4, 0, 1).unwrap().cv_auc as f64;
            b += dl.evaluate(4, 0, 1).unwrap().cv_auc as f64;
        }
        let f = trials as f64;
        println!("{:>14.0} {:>14.3} {:>14.3}", scale, a / f, b / f);
    }

    println!("\nfalse positives on pure noise with outliers present (no signal at all):");
    println!("{:>14} {:>14} {:>14}", "outlier scale", "MeanDiff", "DiagLda");
    println!("{}", "-".repeat(46));
    for scale in [1.0f32, 100.0] {
        let sc = scales(d, n_out, scale);
        let (mut pa, mut pb) = (0usize, 0usize);
        for t in 0..trials {
            let seed = 0x0BAD + t as u64 * 104_729;
            let md = round(d, forks, per_fork, &sc, 0.0, d / 2, Fit::MeanDiff, &mut Lcg::new(seed));
            let dl = round(d, forks, per_fork, &sc, 0.0, d / 2, Fit::DiagLda, &mut Lcg::new(seed));
            if md.evaluate(4, 200, 7).unwrap().deployable() { pa += 1; }
            if dl.evaluate(4, 200, 7).unwrap().deployable() { pb += 1; }
        }
        println!("{:>14.0} {:>11}/{:<3} {:>11}/{:<3}", scale, pa, trials, pb, trials);
    }
}
