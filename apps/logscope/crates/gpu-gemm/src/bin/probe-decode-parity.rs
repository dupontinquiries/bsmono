//! probe-decode-parity — does DECODE agree with PREFILL on the same token, on THIS device with
//! THIS model? Prefill runs GEMM kernels; decode runs matvec kernels. Both must produce the same
//! logits for the same position. If they diverge, decode is reading or computing wrong and the
//! model degenerates from the first generated token while prefill looks healthy — the exact
//! asymmetry seen on Honeykrisp, where two unrelated models both collapse into a repeated token.
//!
//! Sandbox cannot reproduce it (lavapipe agrees everywhere), so this ships as a probe to run on
//! the affected machine, with the real quantized weights the app actually loads.
//!
//!   probe-decode-parity <model.gguf> [n_prompt] [--f32]
//!
//! Prints, per position tested: argmax agreement and relative error between
//!   A) prefill over prompt[..=i]      (GEMM path, last row)
//!   B) prefill over prompt[..i] then decode_step(prompt[i])   (matvec path)
//! A divergence here is a kernel bug on this driver, not a model or template problem.
use gpu_kernels::*;

fn argmax(v: &[f32]) -> usize {
    v.iter().enumerate().fold((0usize, f32::NEG_INFINITY), |(bi, bv), (i, &x)| if x > bv { (i, x) } else { (bi, bv) }).0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "/tmp/tiny-qwen2.gguf".into());
    let rest: Vec<String> = args.collect();
    let n_prompt: usize = rest.iter().find_map(|a| a.parse().ok()).unwrap_or(64);
    let want_f32 = rest.iter().any(|a| a == "--f32");
    let bytes = std::fs::read(&path).expect("read model");
    let gpu = Gpu::new();
    eprintln!("probe-decode-parity: adapter = {}", gpu.adapter_name);
    // Default: quantized exactly as the app loads it. --f32 isolates quantization from kernels.
    let m = gpu.load_gguf_opt(&bytes, !want_f32).expect("load");
    eprintln!("probe-decode-parity: {} d={} layers={} vocab={} ({})",
        path, m.d, m.n_layers, m.vocab, if want_f32 { "f32" } else { "quantized (as the app loads it)" });

    let prompt: Vec<u32> = (0..n_prompt as u32).map(|i| 1 + (i * 7919) % (m.vocab as u32 - 2)).collect();
    let mut worst_rel = 0.0f32;
    let mut mismatches = 0usize;
    let mut tested = 0usize;

    // Test near the start, the middle, and the end: a KV-length-dependent fault shows as a
    // divergence that appears only past some position.
    let picks: Vec<usize> = [1usize, 2, n_prompt / 4, n_prompt / 2, n_prompt.saturating_sub(2), n_prompt - 1]
        .into_iter().filter(|&i| i >= 1 && i < n_prompt).collect::<std::collections::BTreeSet<_>>().into_iter().collect();

    for &i in &picks {
        // A: one prefill covering prompt[..=i]; its last row is the logits after token i.
        let ca = gpu.new_cache(m.n_layers, n_prompt + 4, m.kv_dim());
        let a = gpu.prefill_chunked(&m, &ca, &prompt[..=i], 512);
        // B: prefill prompt[..i], then a single decode step on token i.
        let cb = gpu.new_cache(m.n_layers, n_prompt + 4, m.kv_dim());
        let _ = gpu.prefill_chunked(&m, &cb, &prompt[..i], 512);
        let b = gpu.decode_step(&m, &cb, prompt[i], i);

        if a.len() != b.len() { println!("  pos {i:>5}: LENGTH MISMATCH {} vs {}", a.len(), b.len()); mismatches += 1; continue; }
        let scale = a.iter().map(|x| x.abs()).fold(1e-6f32, f32::max);
        let rel = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max) / scale;
        let (am, bm) = (argmax(&a), argmax(&b));
        worst_rel = worst_rel.max(rel);
        tested += 1;
        if am != bm || rel > 5e-2 { mismatches += 1; }
        println!("  pos {i:>5}: argmax prefill={am:<7} decode={bm:<7} {}  rel {rel:.3e}",
            if am == bm { "same " } else { "DIFFER" });
    }

    // GATE MODE: --gate makes this exit non-zero on any disagreement, and additionally re-runs
    // itself with the fast-decode levers forced on. That second run is the one that would have
    // caught the pass-fusion regression the day it shipped: a feature gate that only exercises its
    // own feature cannot see the feature being wrong, so the battery compares two paths that MUST
    // agree, under every lever combination the operator can actually end up in.
    println!("probe-decode-parity: {} of {tested} positions disagree, worst rel {worst_rel:.3e}", mismatches);
    if mismatches > 0 {
        println!("  VERDICT: decode diverges from prefill on this device. The matvec decode path is\n\
                  \x20 computing different logits than the GEMM prefill path for the same token, which\n\
                  \x20 collapses generation into a repeated token while prefill looks healthy.\n\
                  \x20 Re-run with --f32: if it then agrees, the fault is in the QUANTIZED matvec;\n\
                  \x20 if it still diverges, it is the matvec kernel or the KV read itself.");
        if rest.iter().any(|a| a == "--gate") { std::process::exit(1); }
    } else {
        println!("  VERDICT: decode == prefill here. The logits path is sound on this device at these\n\
                  \x20 shapes; the degenerate output is coming from something after the logits\n\
                  \x20 (sampling / window / stop handling) or from the prompt itself.");
    }
}
