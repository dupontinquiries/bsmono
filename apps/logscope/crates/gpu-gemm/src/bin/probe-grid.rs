//! probe-grid — exercise each op on the diffusion path at REAL-SCALE shapes and report which ones
//! exceed the 65535 workgroup-per-dimension limit. The limit bites as a NON-FATAL validation error
//! (dispatch dropped, output left stale) unless the grid guard catches it, and it only appears at
//! sizes no tiny model reaches: a whole-canvas forward at d=2048 crosses it at exactly t=2048.
//! GATHER was the one that bit us on hardware; this answers whether anything else is waiting.
//!
//! Each op runs under catch_unwind so one over-limit op does not hide the rest.
use gpu_kernels::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn probe(name: &str, wg_est: u64, f: impl FnOnce()) -> bool {
    let r = catch_unwind(AssertUnwindSafe(f));
    match r {
        Ok(_) => { println!("  [ok]    {name:<28} (largest grid dim ~{wg_est})"); true }
        Err(e) => {
            let msg = e.downcast_ref::<String>().map(|s| s.as_str())
                .or_else(|| e.downcast_ref::<&str>().copied()).unwrap_or("(non-string panic)");
            let first = msg.lines().next().unwrap_or("");
            println!("  [OVER]  {name:<28} {}", &first[..first.len().min(150)]);
            false
        }
    }
}

fn main() {
    let t: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(2400);
    let d: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(2048);
    let ff: usize = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(5120);
    let (nh, hd) = (16usize, 128usize);
    let gpu = Gpu::new();
    println!("probe-grid: t={t} d={d} ff={ff} on {}", gpu.adapter_name);
    println!("  (t*d/64 = {}, t*ff/64 = {}, limit 65535)", t * d / 64, t * ff / 64);
    let mut all = true;

    let table = gpu.upload(&vec![0.1f32; 512 * d], 512, d);
    let ids = gpu.upload_u32(&(0..t as u32).map(|i| i % 512).collect::<Vec<_>>());
    all &= probe("gather_t (embeddings)", (t * d / 64) as u64, || { let _ = gpu.download(&gpu.gather_t(&table, &ids, t, d)); });

    let x = gpu.upload(&vec![0.05f32; t * d], t, d);
    let w = gpu.upload(&vec![1.0f32; d], 1, d);
    all &= probe("rmsnorm_t", (t / 64) as u64, || { let _ = gpu.download(&gpu.rmsnorm_t(&x, &w, t, d, 1e-6)); });

    let ffac = gpu.upload(&vec![1.0f32; hd / 2], 1, hd / 2);
    all &= probe("rope_mh_t", (t * nh) as u64, || { let _ = gpu.download(&gpu.rope_mh_t(&x, &ffac, t, d, 10000.0, 0, hd)); });

    let gu = gpu.upload(&vec![0.1f32; t * 2 * ff], t, 2 * ff);
    all &= probe("swiglu_packed (t*ff)", (t * ff / 64) as u64, || { let _ = gpu.download(&gpu.swiglu_packed(&gu)); });

    // The output projection's logits are [t, vocab] — the largest tensor in a denoise step.
    let vocab: usize = std::env::args().nth(4).and_then(|a| a.parse().ok()).unwrap_or(32768);
    let z = gpu.upload(&vec![0.01f32; t * vocab], t, vocab);
    all &= probe("rowstats_t (t*vocab)", (t / 64) as u64, || { let _ = gpu.download(&gpu.rowstats_t(&z, t, vocab)); });

    println!("probe-grid: {}", if all { "PASS \u{2713} — every op on this path stays inside the grid limit at these shapes" }
             else { "FAIL \u{2717} — an op exceeds the limit; without the guard its dispatch would be dropped silently" });
    if !all { std::process::exit(1); }
}
