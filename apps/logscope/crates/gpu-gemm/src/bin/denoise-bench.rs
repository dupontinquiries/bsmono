//! denoise-bench — the real diffuser step, measured, plus mechanism checks.
//!
//! Composes the three gated kernels into the actual loop: bidirectional tiled
//! attention (gate-bidir-attn) -> output projection -> per-position stats
//! (gate-rowstats) -> ConfidenceTopK reveal -> repeat. Framed per step.
//!
//! HONESTY, up front: on an AR checkpoint (Qwen 0.5B) the revealed TEXT is
//! garbage by design — the model was never trained to denoise, and the mask id
//! is just some token. What this bin certifies is the MECHANISM (masking
//! bookkeeping, policy contract, termination) and the PRICE (ms per real
//! bidirectional step, and the tok/s that implies). Semantically real tokens
//! arrive with the RND1 loader alias — the only remaining blocker.
//!
//!   cargo run --release --bin denoise-bench -- <model.gguf> [L] [fill] [steps] [mask_id]
//!   cargo run --release --bin denoise-bench -- ~/models/qwen2.5-0.5b-instruct-q8_0.gguf 512 1.0 4
//!
//! Compare ms/step here against diffstep-bench at the same L:
//!   - this step adds full-row stats (~55 ms at L=512 per the gemmshape lm_head row)
//!     and swaps causal attention for non-causal (~2x the tiled-causal attention term);
//!   - this step is FRAMED; prefill_batched is not. If this lands far BELOW
//!     diffstep's flag-on number plus the stats bill, the 197 ms constant was
//!     per-dispatch submits, and prefill_batched wants the same begin/end_frame
//!     pair prefill_chunked already got.

use gpu_kernels::*;
use std::time::Instant;

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    // --threshold=0.9   adaptive policy: reveal everything with p_top >= tau (variable k/step);
    //                   `steps` becomes the CAP, and tok/s becomes model-confidence-dependent —
    //                   i.e. the honest number.
    // --eotp            after an EOS token is revealed in the fill, resolve every masked position
    //                   AFTER it as EOS immediately (End-of-Text Prediction: stop denoising padding).
    // --trace=FILE      record the Learn2PD oracle training set: one JSONL line per (step, masked
    //                   position) with stats + revealed + label(argmax_at_step == final_token).
    let threshold: Option<f32> = args.iter().find_map(|a| a.strip_prefix("--threshold=").and_then(|v| v.parse().ok()));
    let eotp = args.iter().any(|a| a == "--eotp");
    let trace_path: Option<String> = args.iter().find_map(|a| a.strip_prefix("--trace=").map(str::to_string));
    // --filter=FILE     use a trained Learn2PD filter (filter-train output) as the reveal policy.
    let filter_path: Option<String> = args.iter().find_map(|a| a.strip_prefix("--filter=").map(str::to_string));
    args.retain(|a| !a.starts_with("--"));
    let path = args.get(1).unwrap_or_else(|| {
        eprintln!("usage: denoise-bench <model.gguf> [L] [fill] [steps] [mask_id]");
        std::process::exit(2);
    });
    let l: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(512);
    let fill: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let steps: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(4);
    let mask_cli: Option<u32> = args.get(5).and_then(|s| s.parse().ok());

    let gpu = Gpu::new();
    eprintln!("denoise: adapter = {}", gpu.adapter_name);
    let bytes = std::fs::read(path).expect("read gguf");
    // Metadata pass first: mask token id + tokenizer, so real checkpoints print real text.
    let g = gguf::parse(&bytes).expect("parse gguf header");
    let arch = g.declared_arch().unwrap_or_else(|| "?".into());
    let mask_id: u32 = match mask_cli.or_else(|| gguf_mask_token_id(&g)) {
        Some(v) => v,
        None => {
            eprintln!(
                "denoise: no mask token id — the file carries no tokenizer.ggml.mask_token_id and none \
was given on the CLI. Denoising around a guessed mask id produces plausible nonsense, so refusing. \
Pass it as the 5th argument (e.g. 151643 for a Qwen mechanism run)."
            );
            std::process::exit(2);
        }
    };
    let tok = tokenizer::Tokenizer::from_gguf(&g).ok();
    let model = gpu.load_gguf_opt(&bytes, true).expect("load gguf");
    eprintln!(
        "denoise: loaded '{arch}' ({} layers) | L={l} fill={fill} steps={steps} mask_id={mask_id} ({})",
        model.n_layers,
        if mask_cli.is_some() { "CLI" } else { "from gguf metadata" }
    );

    let n_fill = ((l as f64) * fill).round() as usize;
    let n_ctx = l - n_fill;
    // context: fixed pseudo-tokens; fill region: mask_id everywhere
    let mut tokens: Vec<u32> = (0..n_ctx).map(|j| 1 + ((j as u32 * 7) % 999)).collect();
    tokens.extend(std::iter::repeat(mask_id).take(n_fill));
    let mut masked: Vec<bool> = (0..l).map(|i| i >= n_ctx).collect();

    let mut ok = true;
    let mut ck = |lbl: &str, v: bool| {
        ok &= v;
        println!("  [{}] {lbl}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };

    let eos_id = g.get_u32("tokenizer.ggml.eos_token_id");
    let k_fixed = n_fill.div_ceil(steps.max(1));
    let base_policy: Box<dyn DenoisePolicy> = match (&filter_path, threshold) {
        (Some(fp), _) => {
            let txt = std::fs::read_to_string(fp).expect("read filter file");
            let lp = LearnedPolicy::from_text(&txt).expect("parse l2pd file");
            eprintln!("denoise: learned filter from {fp} (tau={:.2})", lp.tau);
            Box::new(lp)
        }
        (None, Some(tau)) => Box::new(ThresholdPolicy { tau }),
        (None, None) => Box::new(ConfidenceTopK { k: k_fixed }),
    };
    let mut policy = TraceRecorder::new(base_policy);
    let _ = gpu.denoise_stats(&model, &tokens); // warm: pipelines + first-use compiles

    let mut step_ms = Vec::with_capacity(steps);
    let mut reveal_counts = Vec::with_capacity(steps);
    let mut reveal_violation = false;
    let mut count_mismatch = false;
    let mut steps_used = 0usize;
    for step in 0..steps {
        if masked.iter().all(|&b| !b) { break; }
        steps_used = step + 1;
        let t0 = Instant::now();
        let flat = gpu.denoise_stats(&model, &tokens);
        step_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        let stats: Vec<[f32; 4]> = flat.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let picks = policy.select(step, &stats, &masked);
        if threshold.is_none() {
            let expect = k_fixed.min(masked.iter().filter(|&&b| b).count());
            count_mismatch |= picks.len() != expect;
        } else {
            count_mismatch |= picks.is_empty(); // threshold policy's floor guarantees progress
        }
        for &i in &picks {
            reveal_violation |= !masked[i]; // policy must only reveal masked positions
            tokens[i] = stats[i][0] as u32; // commit the argmax
            masked[i] = false;
        }
        reveal_counts.push(picks.len());
        // EoTP: once an EOS is revealed in the fill, everything after it is padding —
        // resolve it now instead of spending forwards denoising it.
        if eotp {
            if let Some(eid) = eos_id {
                if let Some(ep) = (n_ctx..l).find(|&i| !masked[i] && tokens[i] == eid) {
                    for i in (ep + 1)..l {
                        if masked[i] { tokens[i] = eid; masked[i] = false; }
                    }
                }
            }
        }
    }
    // Final commit: if the cap ended a threshold run early, one more stats pass reveals the
    // remainder by argmax (the standard last-step commit), so coverage stays a real invariant.
    if masked.iter().any(|&b| b) {
        steps_used += 1;
        let t0 = Instant::now();
        let flat = gpu.denoise_stats(&model, &tokens);
        step_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        let stats: Vec<[f32; 4]> = flat.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let mut n = 0usize;
        for i in 0..l {
            if masked[i] { tokens[i] = stats[i][0] as u32; masked[i] = false; n += 1; }
        }
        reveal_counts.push(n);
    }
    ck("policy revealed only masked positions", !reveal_violation);
    ck(if threshold.is_some() { "threshold floor made progress every step" } else { "per-step reveal count == min(K, remaining)" }, !count_mismatch);
    ck("all fill positions revealed after the schedule", masked.iter().all(|&b| !b));
    ck("context positions untouched", tokens[..n_ctx].iter().zip(0..n_ctx).all(|(&t, j)| t == 1 + ((j as u32 * 7) % 999)));

    if let Some(tp) = &trace_path {
        match policy.finalize(&tokens, tp) {
            Ok(n) => ck(&format!("trace: {n} labeled records -> {tp}"), true),
            Err(e) => ck(&format!("trace write to {tp} failed: {e}"), false),
        }
    }
    let mean = step_ms.iter().sum::<f64>() / step_ms.len().max(1) as f64;
    println!("\n  steps used: {steps_used}/{steps} | reveals/step: {reveal_counts:?}");
    println!("  step ms: {:?}", step_ms.iter().map(|m| (m * 10.0).round() / 10.0).collect::<Vec<_>>());
    println!(
        "  mean {mean:.1} ms/step | {:.0} pos/s | {} tokens in {:.2} s -> {:.0} tok/s at this (steps, fill)",
        l as f64 / (mean / 1e3),
        n_fill,
        mean * steps_used as f64 / 1e3,
        n_fill as f64 / (mean * steps_used as f64 / 1e3)
    );
    if let Some(t) = &tok {
        let text = t.decode(&tokens[n_ctx..]);
        let shown: String = text.chars().take(600).collect();
        println!("\n  revealed text ({} tokens): {shown}{}", n_fill, if text.chars().count() > 600 { " …" } else { "" });
    }
    println!(
        "denoise-bench: {}",
        if ok { "mechanism \u{2713} — the number above is the real step, not a proxy" } else { "FAIL \u{2717}" }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
