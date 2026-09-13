//! decode-census — what does ONE decode token cost in GPU commands, and are the logits the same?
//!
//! The engine is launch-bound on Honeykrisp (ENGINE_PERF_STATUS.md §2), so the number that
//! predicts tok/s is not FLOPs or bytes but how many commands the driver has to schedule per
//! token: compute dispatches, the compute PASSES they are wrapped in (each pass is its own
//! VkCommandBuffer plus a barrier buffer under wgpu-core), buffer copies, and submits. The
//! bench prints "~210 dispatches/token" as a fixed string; this counts.
//!
//! It also prints a hash of every decode step's logits, so two runs under different levers can
//! be diffed for bit-exactness with no model file larger than the ~1 MB fixture:
//!
//!   decode-census /tmp/tiny-qwen2.gguf [n_tokens]
//!   LOGSCOPE_PASS_FUSE=1 decode-census /tmp/tiny-qwen2.gguf
//!
//! Counters are read from the crate's own atomics; the harness mirrors `stream_session`'s loop
//! (prefill_chunked, then decode_step per token) so the census is of the path the app runs.
use gpu_kernels::*;
use std::sync::atomic::Ordering::Relaxed;

fn snap() -> [u64; 5] {
    [GPU_DISPATCHES.load(Relaxed), GPU_PASSES_OPENED.load(Relaxed), GPU_SUBMITS.load(Relaxed),
     GPU_COPIES.load(Relaxed), GPU_POLLS.load(Relaxed)]
}

fn hash(v: &[f32]) -> u64 {
    // FNV-1a over the raw bits: bit-exact or not, nothing in between.
    let mut h: u64 = 0xcbf29ce484222325;
    for x in v { for b in x.to_bits().to_le_bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } }
    h
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "/tmp/tiny-qwen2.gguf".into());
    let n_tok: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(8);
    let bytes = std::fs::read(&path).expect("read model");
    let gpu = Gpu::new();
    let m = gpu.load_gguf_opt(&bytes, true).expect("load");
    eprintln!("decode-census: adapter = {} | {} d={} layers={} kv_heads={} vocab={}",
        gpu.adapter_name, path, m.d, m.n_layers, m.n_kv_heads, m.vocab);

    let prompt: Vec<u32> = (0..16u32).map(|i| 1 + (i * 7919) % (m.vocab as u32 - 2)).collect();
    let cache = gpu.new_cache(m.n_layers, prompt.len() + n_tok + 4, m.kv_dim());
    let mut logits = gpu.prefill_chunked(&m, &cache, &prompt, 512);
    gpu.end_frame();

    let mut pos = prompt.len();
    let mut tot = [0u64; 5];
    let mut all = Vec::new();
    for t in 0..n_tok {
        let next = logits.iter().enumerate().fold((0usize, f32::NEG_INFINITY),
            |(bi, bv), (i, &x)| if x > bv { (i, x) } else { (bi, bv) }).0 as u32;
        let before = snap();
        if t + 1 == n_tok { dispatch_histogram_reset(); }
        logits = gpu.decode_step(&m, &cache, next, pos);
        let after = snap();
        let d: Vec<u64> = (0..5).map(|i| after[i] - before[i]).collect();
        for i in 0..5 { tot[i] += d[i]; }
        all.extend_from_slice(&logits);
        println!("tok {t:>2} pos {pos:>3} argmax {next:>5} logits {:016x} | dispatches {:>4} passes {:>4} submits {:>3} copies {:>4} polls {:>2}",
            hash(&logits), d[0], d[1], d[2], d[3], d[4]);
        pos += 1;
    }
    // Per-kernel breakdown of the LAST token (LOGSCOPE_DISPATCH_HIST=1 turns the tally on; the
    // harness resets it before the final step so the numbers are one token's, not the prefill's).
    let hist = dispatch_histogram();
    if !hist.is_empty() {
        println!("last-token dispatches by kernel:");
        for (id, c) in &hist {
            println!("  {c:>4}  {}", shader_label(*id).unwrap_or_else(|| format!("pipeline@{id:x}")));
        }
    }
    let n = n_tok as f64;
    println!("per token: dispatches {:.1} passes {:.1} submits {:.1} copies {:.1} polls {:.1} | per layer: dispatches {:.2} copies {:.2}",
        tot[0] as f64 / n, tot[1] as f64 / n, tot[2] as f64 / n, tot[3] as f64 / n, tot[4] as f64 / n,
        tot[0] as f64 / n / m.n_layers as f64, tot[3] as f64 / n / m.n_layers as f64);
    println!("all-logits hash {:016x}", hash(&all));
}
