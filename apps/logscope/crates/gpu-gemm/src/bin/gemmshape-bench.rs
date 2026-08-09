//! gemmshape-bench — where does the 644 ms/step actually go?
//!
//! diffstep-bench measured the 0.5B filler step at ~795 pos/s ≈ 0.6–0.8 TF sustained,
//! vs ~1.0–1.15 TF for 4B shapes: S is shape-dependent. This bin prices each (n,k)
//! weight shape the Qwen2.5-0.5B step dispatches, at the filler width M=512, across
//! every prefill GEMM tile the device supports — then reconstructs the step time from
//! the parts so the gap to diffstep's measured ms/step (launch tax + attention + norms
//! + anything unaccounted) is one printed number instead of a guess.
//!
//! Notes:
//! - `gemm_tile_for(n,k)` already reflects what load-time `autotune_gemm` cached (it
//!   probes at a CAPPED m, trusting the winner transfers). The sweep re-times at the
//!   REAL m=512; disagreement between "current" and "winner" is the transfer error.
//! - A global tile env override shadows the per-shape cache — clear it before running.
//! - lm_head (151936×896) is behind --lm: ~0.5 GB of host f32 to stage, and whether
//!   prefill even runs unembed per-position (vs last-row-only) decides if it belongs
//!   in the step reconstruction at all. Run it once to price it either way.
//!
//! Run (M2):
//!   cargo run --release --bin gemmshape-bench            # layer shapes
//!   cargo run --release --bin gemmshape-bench -- --lm    # + the unembed shape

use gpu_kernels::*;
use std::time::Instant;

fn main() {
    let with_lm = std::env::args().any(|a| a == "--lm");
    const M: usize = 512;

    // (n, k, count-per-layer, label) — weights are [n_out, k_in].
    let mut shapes: Vec<(usize, usize, usize, &str)> = vec![
        (896, 896, 2, "q/o    "),
        (128, 896, 2, "k/v    "),
        (4864, 896, 2, "gate/up"),
        (896, 4864, 1, "down   "),
    ];
    if with_lm {
        shapes.push((151936, 896, 0, "lm_head")); // count 0: priced, not in the ×24 sum
    }

    let gpu = Gpu::new();
    eprintln!("gemmshape: adapter = {}", gpu.adapter_name);
    let cands = gpu.gemm_tile_candidates();
    eprintln!("gemmshape: tile candidates = {cands:?}, M = {M}");

    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut rnd = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };

    println!("\n  shape        n      k   cur   tile   ms/op   GFLOPS");
    let mut per_layer_ms = 0.0f64;
    for &(n, k, cnt, label) in &shapes {
        let w: Vec<f32> = (0..n * k).map(|_| rnd()).collect();
        let qw = gpu.quantize_q8_resident(&w, n, k);
        drop(w); // 0.5 GB for lm_head; the resident copy is on-GPU now
        let act: Vec<f32> = (0..M * k).map(|_| rnd()).collect();
        let a = gpu.upload(&act, M, k);
        let cur = gpu.gemm_tile_for(n, k);
        let gflop = 2.0 * (M * n * k) as f64 / 1e9;
        let reps = if n >= 32_768 { 4 } else { 16 };

        let mut best: Option<(u32, f64)> = None;
        for &tile in &cands {
            gpu.set_gemm_tile(n, k, tile);
            // warm (pipeline + any first-use compile at this tile), then timed reps;
            // the single download at the end is the sync point for the whole batch.
            let _ = gpu.download(&gpu.linear_q8_0_t(&a, &qw, M));
            let t = Instant::now();
            for _ in 0..reps - 1 {
                let _ = gpu.linear_q8_0_t(&a, &qw, M);
            }
            let _ = gpu.download(&gpu.linear_q8_0_t(&a, &qw, M));
            let ms = t.elapsed().as_secs_f64() * 1e3 / reps as f64;
            println!(
                "  {label} {n:>6} {k:>6} {:>5} {tile:>6} {ms:>8.2} {:>8.0}",
                if tile == cands[0] { format!("{cur}") } else { String::new() },
                gflop / (ms / 1e3)
            );
            if best.map_or(true, |(_, b)| ms < b) {
                best = Some((tile, ms));
            }
        }
        let (bt, bms) = best.unwrap();
        gpu.set_gemm_tile(n, k, bt); // leave the winner installed
        println!("  {label} winner: tile {bt} @ {bms:.2} ms {}", if bt == cur { "(= current)" } else { "(≠ current — capped-m probe mis-transferred)" });
        per_layer_ms += bms * cnt as f64;
    }

    let step_ms = per_layer_ms * 24.0;
    println!(
        "\nverdict: GEMM-only reconstruction = {per_layer_ms:.2} ms/layer × 24 = {step_ms:.0} ms/step at winners. \
         diffstep measured 644.5 ms; the difference is attention + norms + embedding + launch tax + \
         anything this sweep can't see. If reconstruction ≈ measured, the step is pure GEMM and kernel \
         work is the whole lever; if reconstruction ≪ measured, go find the rest before touching kernels."
    );
}
