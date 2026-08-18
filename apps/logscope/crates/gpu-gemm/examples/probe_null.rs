//! probe_null.rs — what a steering direction fitted to pure noise looks like at n=32, d=4096.
//!
//! `steer::Contrast` gates on `sep`, an in-sample Cohen's d measured along the direction that was
//! chosen to maximise it. This study measures how often that gate passes when there is nothing to
//! find, and compares it against `probe`'s held-out AUC plus permutation null on identical data.
//!
//! Run: `cargo run --release --example probe_null`

use gpu_kernels::probe::{self, LayerSweep, Lcg};
use gpu_kernels::steer::{Contrast, Outcome};

/// The threshold `steer.rs` used to gate on, kept here so the study can report what it would have
/// done. It is deliberately no longer exported by `steer`.
const OLD_MIN_SEPARATION: f32 = 0.8;

fn main() {
    let d: usize = std::env::var("D").ok().and_then(|s| s.parse().ok()).unwrap_or(4096);
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(32);
    let trials: usize = std::env::var("TRIALS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let perm_trials: usize = std::env::var("PERM_TRIALS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let perms: usize = std::env::var("PERMS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);

    println!("null study: d={d} n={n} trials={trials}  (no signal in the data at all)\n");

    let mut sep_pass = 0usize;
    let mut seps: Vec<f32> = Vec::with_capacity(trials);
    let mut aucs: Vec<f32> = Vec::with_capacity(trials);
    let mut in_aucs: Vec<f32> = Vec::with_capacity(trials);

    for t in 0..trials {
        let mut rng = Lcg::new(0xC0FFEE + t as u64);
        let mut c = Contrast::new(20, d);
        let mut s = LayerSweep::new(20, d);
        for i in 0..n {
            let ok = i % 2 == 0;
            let h: Vec<f32> = (0..d).map(|_| rng.normal()).collect();
            c.push(if ok { Outcome::Survived } else { Outcome::Died }, &h);
            s.push(i, &h, ok);
        }
        let st = c.solve().expect("both sides populated");
        seps.push(st.sep);
        if st.sep >= OLD_MIN_SEPARATION { sep_pass += 1; }
        let r = s.evaluate(4, 0, 1).expect("both classes present");
        aucs.push(r.cv_auc);
        in_aucs.push(r.in_sample_auc);
    }

    println!("steer::Contrast  in-sample sep      : {}", summarize(&mut seps));
    println!("probe            in-sample AUC      : {}", summarize(&mut in_aucs));
    println!("probe            held-out (CV) AUC  : {}", summarize(&mut aucs));
    println!();
    println!("OLD steer gate (sep >= {OLD_MIN_SEPARATION:.1}) would pass on noise : {sep_pass}/{trials}  ({:.0}%)",
             100.0 * sep_pass as f32 / trials as f32);

    // Full probe gate, including the permutation null. Fewer trials because each one refits the
    // direction `perms * folds` times.
    let mut gate_pass = 0usize;
    for t in 0..perm_trials {
        let mut rng = Lcg::new(0xBEEF + t as u64);
        let mut s = LayerSweep::new(20, d);
        for i in 0..n {
            let ok = i % 2 == 0;
            let h: Vec<f32> = (0..d).map(|_| rng.normal()).collect();
            s.push(i, &h, ok);
        }
        let r = s.evaluate(4, perms, 17).unwrap();
        if r.deployable() { gate_pass += 1; }
    }
    println!("probe gate (CV AUC >= {:.2} AND p <= {:.2}) passed on noise : {gate_pass}/{perm_trials}",
             probe::MIN_CV_AUC, probe::MAX_NULL_P);

    println!("\ncausal trial cost, two-sided 5% at 80% power:");
    for (p0, p1) in [(0.22f32, 0.48f32), (0.22, 0.35), (0.22, 0.30)] {
        let arms = probe::trial_arms_needed(p0, p1, probe::Z_95, probe::Z_80);
        println!("  {:.0}% -> {:.0}% : {arms} branches per arm, {} rounds at width 32",
                 p0 * 100.0, p1 * 100.0, probe::rounds_needed(arms, 32));
    }
}

fn summarize(v: &mut Vec<f32>) -> String {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |f: f32| v[((v.len() - 1) as f32 * f) as usize];
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    format!("mean {:>6.3}   p05 {:>6.3}   median {:>6.3}   p95 {:>6.3}", mean, q(0.05), q(0.5), q(0.95))
}
