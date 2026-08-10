//! diffstep-bench — price the diffusion-filler STEP SHAPE on real hardware.
//!
//! One denoise step == one full forward over [B seqs × L positions]. That is a
//! prefill-shaped workload (bidirectional vs causal changes the mask, not the
//! FLOPs at L=512), so `prefill_batched` is a valid throughput proxy TODAY with
//! zero engine changes.
//!
//! Honest bound: B sequences are issued as B sequential prefill submits, so each
//! step pays ~B× the per-pass launch tax ENGINE_PERF_STATUS measured (48.4 ms/pass,
//! ~210 dispatches, 84% gpu-sync). A packed single-submit multi-sequence prefill
//! (does not exist yet — ragged.rs is decode seat management) removes (B-1) of
//! those; what this bin prints is therefore the FLOOR, and the ms/step it reports
//! pins the one number nothing in-tree measures: sustained big-GEMM throughput S.
//!
//! Run on the M2 (never lavapipe — correctness box only):
//!   cargo run --release --bin diffstep-bench -- ~/models/qwen2.5-0.5b-instruct-q8_0.gguf
//!   cargo run --release --bin diffstep-bench -- <model.gguf> 512 1,4,8,16,32
//!
//! Read off per B:  ms/step, positions/s.
//! Filler tok/s = B·L·fill / (steps · t_step); printed for steps {4,8,32}, fill {1.0, 0.5}.
//! Pin S:        S ≈ positions/s × GFLOP/position (≈ 2·params_nonembed + 2·d·vocab).

use gpu_kernels::*;
use std::time::Instant;

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    // --chunked: route through prefill_chunked(chunk=L) — one block_prefill_ext pass per layer,
    // i.e. the TILED attention path. Against the default (naive ATTENTION_MH) this isolates the
    // quadratic term's kernel with zero engine edits; --gate-prefill is the bit-exactness oracle.
    let chunked = args.iter().any(|a| a == "--chunked");
    args.retain(|a| a != "--chunked");
    let path = args.get(1).unwrap_or_else(|| {
        eprintln!("usage: diffstep-bench <model.gguf> [L] [B,B,...]");
        std::process::exit(2);
    });
    let l: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(512);
    let bs: Vec<usize> = args
        .get(3)
        .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
        .unwrap_or_else(|| vec![1, 2, 4, 8, 16, 32]);
    const ITERS: usize = 3;

    let gpu = Gpu::new();
    eprintln!("diffstep: adapter = {} | attention path: {}", gpu.adapter_name,
        if chunked { "TILED (prefill_chunked, ext/online-softmax)" } else { "naive (prefill_batched, ATTENTION_MH)" });

    let t0 = Instant::now();
    // Split-GGUF aware: expand siblings from the filename, mmap each shard for the process
    // lifetime (retain_map — no host-RAM copy of a 60 GB Q4 split; LOGSCOPE_NO_MMAP honored),
    // and hand the set to load_gguf_parts. Single files take the same path with one shard.
    let shards = gguf_shard_paths(path).expect("resolve gguf shards");
    let parts: Vec<&'static [u8]> = shards.iter().map(|q| retain_map(q).expect("map gguf shard")).collect();
    let mapped_gb = parts.iter().map(|b| b.len()).sum::<usize>() as f64 / 1e9;
    let model = gpu.load_gguf_parts(&parts, true).expect("load gguf");
    eprintln!(
        "diffstep: loaded {} — {} shard(s), {:.1} GB mapped ({} layers, kv_dim {}) in {:.1}s",
        path, shards.len(), mapped_gb, model.n_layers, model.kv_dim(), t0.elapsed().as_secs_f32()
    );
    let pipes = gpu.warm(&model);
    eprintln!("diffstep: warmed {pipes} pipelines");

    // One-time probe: fixed cost of a single readback (map_async + poll) on this driver.
    // The L-sweep fit found a ~197 ms per-pass CONSTANT; the submit-governor banner says
    // completion callbacks are slow/absent on Honeykrisp. If this probe reports ~100+ ms,
    // the constant is the readback path; if ~1 ms, it's per-pass encode/alloc churn instead.
    let probe = gpu.upload(&[0.0f32, 0.0, 0.0, 0.0], 1, 4);
    let _ = gpu.download(&probe); // warm the mapping path
    let tp = Instant::now();
    for _ in 0..20 {
        let _ = gpu.download(&probe);
    }
    eprintln!(
        "diffstep: readback fixed cost ~= {:.2} ms (16-byte tensor, map+poll only)",
        tp.elapsed().as_secs_f64() * 1e3 / 20.0
    );

    // Token ids kept in 1..1000: valid under any real vocab; content is irrelevant
    // to the step price, only the shape [B × L] is.
    let prompts: Vec<Vec<u32>> = (0..bs.iter().copied().max().unwrap_or(1))
        .map(|s| (0..l).map(|j| 1 + ((j as u32 * 7 + s as u32 * 131) % 999)).collect())
        .collect();

    println!("\n  B     L    ms/step      pos/s   submits/step  disp/step");
    let mut best: Option<(usize, f64)> = None; // (B, s/step)
    for &b in &bs {
        // One cache per seat, reused across steps: prefill overwrites, and state
        // continuity is irrelevant to pricing the pass.
        let caches: Vec<_> = (0..b)
            .map(|_| gpu.new_cache(model.n_layers, l + 4, model.kv_dim()))
            .collect();
        // Warm step (autotune/pipeline residency), then timed steps. The logits
        // readback in prefill_batched is the sync point; it also charges one
        // [vocab] f32 readback per seq per step that a fused filler would not pay
        // (decode already has the gpu-argmax path; prefill does not, yet).
        for (c, p) in caches.iter().zip(&prompts) {
            if chunked { let _ = gpu.prefill_chunked(&model, c, p, l); } else { let _ = gpu.prefill_batched(&model, c, p); }
        }
        let d0 = DISPATCH_SEQ.load(std::sync::atomic::Ordering::Relaxed);
        let t = Instant::now();
        for _ in 0..ITERS {
            for (c, p) in caches.iter().zip(&prompts) {
                if chunked { let _ = gpu.prefill_chunked(&model, c, p, l); } else { let _ = gpu.prefill_batched(&model, c, p); }
            }
        }
        let step = t.elapsed().as_secs_f64() / ITERS as f64;
        let disp = (DISPATCH_SEQ.load(std::sync::atomic::Ordering::Relaxed) - d0) / ITERS as u64;
        println!(
            "{:>3} {:>5} {:>10.1} {:>10.0} {:>10} {:>10}",
            b, l, step * 1e3, (b * l) as f64 / step, b, disp
        );
        if best.map_or(true, |(_, s)| (b * l) as f64 / step > (best.unwrap().0 * l) as f64 / s) {
            best = Some((b, step));
        }
    }

    if let Some((b, step)) = best {
        println!("\nfiller projections at best shape (B={b}, L={l}):");
        for steps in [4usize, 8, 32] {
            for fill in [1.0f64, 0.5] {
                let tps = (b as f64 * l as f64 * fill) / (steps as f64 * step);
                println!("  {steps:>2}-step  fill {:.1}  ->  {tps:>7.0} tok/s", fill);
            }
        }
        println!(
            "\nverdict: {:.0} positions/s at the floor (B sequential submits). Multiply by \
             GFLOP/position for sustained S; a packed single-submit prefill kernel is the \
             next lever if ms/step ≈ B × single-seq ms/step (launch-bound, not GEMM-bound).",
            (b * l) as f64 / step
        );
    }
}
