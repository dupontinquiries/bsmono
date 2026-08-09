//! gate-rowstats — correctness oracle for `unembed_stats` (the diffuser confidence path).
//!
//! Method: compute logits with the PROVEN tiled q8 GEMM, download them, and build the
//! reference stats on CPU from those exact values — so this gate tests only the new
//! reduce kernel and its host wiring, not the quantization or the GEMM. Runs on
//! lavapipe (correctness box) and Honeykrisp alike; agreement on lavapipe is evidence
//! about lavapipe, so run it on the Ultra before wiring anything to it.
//!
//! Tolerances: argmax id exact; p_top / margin / entropy to 1e-4 relative — an online
//! softmax accumulates in a different order, so rounding-scale difference is expected
//! and wrong-answer-scale is what fails.
//!
//!   cargo run --release --bin gate-rowstats

use gpu_kernels::*;

fn main() {
    let gpu = Gpu::new();
    eprintln!("gate-rowstats: adapter = {}", gpu.adapter_name);
    let mut ok = true;
    let mut ck = |l: &str, v: bool| {
        ok &= v;
        println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };

    let mut rng = 0x5eed_c0de_1234_5678u64;
    let mut rnd = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };

    // (m, k, n): a square-ish case, an odd vocab not divisible by the 64-thread stride,
    // a single row, and a tall case shaped like a real fill window.
    for &(m, k, n) in &[(5usize, 64usize, 1000usize), (3, 96, 513), (1, 64, 1000), (128, 64, 4096)] {
        let w: Vec<f32> = (0..n * k).map(|_| rnd()).collect();
        let qw = gpu.quantize_q8_resident(&w, n, k);
        let hv: Vec<f32> = (0..m * k).map(|_| rnd() * 4.0).collect();
        let h = gpu.upload(&hv, m, k);

        // Reference logits from the proven path; reference stats on CPU from those values.
        let z = gpu.download(&gpu.linear_q8_0_t(&h, &qw, m));
        let got = gpu.download(&gpu.unembed_stats(&h, &qw, m));

        let mut worst = 0.0f32;
        let mut ids_ok = true;
        for r in 0..m {
            let row = &z[r * n..(r + 1) * n];
            let (mut i1, mut z1, mut z2) = (0usize, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for (j, &v) in row.iter().enumerate() {
                if v > z1 { z2 = z1; z1 = v; i1 = j; } else if v > z2 { z2 = v; }
            }
            let s: f64 = row.iter().map(|&v| ((v - z1) as f64).exp()).sum();
            let mean_z: f64 = row.iter().map(|&v| v as f64 * ((v - z1) as f64).exp()).sum::<f64>() / s;
            let p_top = (1.0 / s) as f32;
            let margin = z1 - z2;
            let ent = ((z1 as f64 + s.ln()) - mean_z) as f32;

            ids_ok &= got[r * 4] as usize == i1;
            for (g, e) in [(got[r * 4 + 1], p_top), (got[r * 4 + 2], margin), (got[r * 4 + 3], ent)] {
                let rel = (g - e).abs() / e.abs().max(1e-6);
                worst = worst.max(rel);
            }
        }
        ck(&format!("m={m:>3} k={k:>3} n={n:>4}: argmax ids exact"), ids_ok);
        ck(&format!("m={m:>3} k={k:>3} n={n:>4}: p/margin/entropy rel <= 1e-4 (worst {worst:.2e})"), worst <= 1e-4);
    }

    println!(
        "gate-rowstats: {}",
        if ok { "PASS \u{2713} — unembed_stats agrees with the CPU reference; safe to wire" } else { "FAIL \u{2717}" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
