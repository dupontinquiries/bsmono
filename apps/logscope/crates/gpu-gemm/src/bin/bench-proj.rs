//! bench-proj — time the OUTPUT PROJECTION alone at a real vocab, whole-canvas rows vs one block's
//! rows. This is the stage the diffusion row window removes work from; a full forward at this width
//! is too slow to time on lavapipe, but the projection is the term that scales with vocab and is
//! measurable on its own.
use gpu_kernels::*;
use std::time::Instant;
fn main() {
    let vocab: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(32768);
    let d: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(2048);
    let t: usize = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(2400);
    let blk: usize = std::env::args().nth(4).and_then(|a| a.parse().ok()).unwrap_or(32);
    let gpu = Gpu::new();
    let w: Vec<f32> = (0..vocab * d).map(|i| ((i % 251) as f32 - 125.0) * 0.001).collect();
    let qw = gpu.quantize_q8_resident(&w, vocab, d);
    drop(w);
    // Three timed passes after two warms: a per-shape autotune or pipeline compile inside the
    // timed region is a one-off and shows up as a large first sample, which is exactly what the
    // first version of this benchmark measured (32 rows "slower" than 2400).
    let run = |rows: usize| -> Vec<f64> {
        let a = gpu.upload(&vec![0.02f32; rows * d], rows, d);
        for _ in 0..2 { let _ = gpu.download(&gpu.linear_q8_0_t(&a, &qw, rows)); }
        (0..3).map(|_| {
            let s = Instant::now();
            let _ = gpu.download(&gpu.linear_q8_0_t(&a, &qw, rows));
            s.elapsed().as_secs_f64()
        }).collect()
    };
    let bigv = run(t); let smallv = run(blk);
    let med = |mut v: Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
    println!("  samples {t} rows: {bigv:?}");
    println!("  samples {blk} rows: {smallv:?}");
    let (big, small) = (med(bigv), med(smallv));
    let fl = |r: usize| (r * d * vocab * 2) as f64 / 1e9;
    println!("projection vocab={vocab} d={d}: {t} rows {big:.3}s ({:.0} GFLOP) | {blk} rows {small:.3}s ({:.1} GFLOP) | {:.1}x less time",
        fl(t), fl(blk), big / small.max(1e-9));
    println!("  logits tensor: {:.2} GB -> {:.3} GB", (t * vocab * 4) as f64 / 1e9, (blk * vocab * 4) as f64 / 1e9);
}
