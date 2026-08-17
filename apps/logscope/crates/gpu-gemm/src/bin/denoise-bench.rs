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
    // --block=N  LLaDA-style block schedule (llama.cpp diffusion.cpp:165-180): the fill canvas is
    //            all-mask upfront; blocks are processed left->right and only the CURRENT block's
    //            masked positions are eligible to reveal. --block=0 (default) = whole-window.
    let block_len: usize = args.iter().find_map(|a| a.strip_prefix("--block=").and_then(|v| v.parse().ok())).unwrap_or(0);
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
    // --api  route through the lib's diffuse_generate (the serve/worker seam) instead of the
    //        bench's instrumented loop — same schedule, no tracing/EoTP; proves the API.
    let use_api = args.iter().any(|a| a == "--api");
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
    // Split-GGUF aware: expand siblings from the filename, mmap each shard for the process
    // lifetime (retain_map — no host-RAM copy of a 60 GB Q4 split; LOGSCOPE_NO_MMAP honored),
    // and hand the set to load_gguf_parts. Single files take the same path with one shard.
    let shards = gguf_shard_paths(path).expect("resolve gguf shards");
    let parts: Vec<&'static [u8]> = shards.iter().map(|q| retain_map(q).expect("map gguf shard")).collect();
    let mapped_gb = parts.iter().map(|b| b.len()).sum::<usize>() as f64 / 1e9;
    // Metadata pass first: mask token id + tokenizer, so real checkpoints print real text.
    let g = gguf::parse(parts[0]).expect("parse gguf header (shard 1 carries metadata)");
    // Logit alignment, mirroring llama.cpp exactly (vendored reference): diffusion-cli.cpp:182-186
    // reads `diffusion.shift_logits` from metadata and DEFAULTS TO TRUE when absent (Dream lineage);
    // diffusion.cpp:261-265 defines shifted as: position i's prediction lives in logits ROW i-1
    // (i==0 -> row 0). Selection features AND the committed argmax both move by the same rule.
    // --shift=0|1 overrides for mechanism runs on non-diffusion checkpoints.
    let shift_cli: Option<bool> = std::env::args().find_map(|a| a.strip_prefix("--shift=").map(|v| v == "1"));
    let shift_logits = shift_cli.unwrap_or_else(|| match g.get_str("diffusion.shift_logits") {
        Some(v) => v == "true",
        None => true,
    });
    eprintln!("denoise: shift_logits = {shift_logits} ({})",
        if shift_cli.is_some() { "CLI" } else if g.get_str("diffusion.shift_logits").is_some() { "gguf metadata" } else { "default-when-absent, per llama.cpp" });
    let arch = g.declared_arch().unwrap_or_else(|| "?".into());
    let mask_id: u32 = match mask_cli.or_else(|| gguf_mask_token_id(&g)) {
        Some(v) => v,
        None => {
            eprintln!(
                "denoise: no mask token id — the file carries no tokenizer.ggml.mask_token_id and none \
was given on the CLI. Denoising around a guessed mask id produces plausible nonsense, so refusing. \
Pass it as the 5th argument (e.g. 151643 for a Qwen mechanism run)."
            );
            // Self-service before the operator digs through HF repo JSONs: the token STRINGS are in
            // this file's own metadata. Search them for mask-like names and print the candidates —
            // a 9-month-old conversion that predates the mask_token_id key still carries its vocab.
            if let Some(gguf::Value::Arr(toks)) = g.metadata.get("tokenizer.ggml.tokens") {
                let mut hits = 0;
                for (id, v) in toks.iter().enumerate() {
                    if let gguf::Value::Str(t) = v {
                        if t.to_ascii_lowercase().contains("mask") {
                            if hits == 0 { eprintln!("denoise: mask-like tokens FOUND IN THIS FILE:"); }
                            eprintln!("           id {id}: {t:?}");
                            hits += 1;
                            if hits >= 8 { eprintln!("           …"); break; }
                        }
                    }
                }
                if hits == 0 {
                    eprintln!("denoise: no token string contains 'mask' — check the model card's special_tokens_map.json.");
                }
            }
            std::process::exit(2);
        }
    };
    let tok = tokenizer::Tokenizer::from_gguf(&g).ok();
    let model = gpu.load_gguf_parts(&parts, true).expect("load gguf");
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
            eprintln!("denoise: learned filter from '{fp}' (tau={:.2})", lp.tau);
            Box::new(lp)
        }
        (None, Some(tau)) => Box::new(ThresholdPolicy { tau }),
        (None, None) => Box::new(ConfidenceTopK { k: k_fixed }),
    };
    let mut policy = TraceRecorder::new(base_policy);
    if use_api {
        let prompt: Vec<u32> = tokens[..n_ctx].to_vec();
        let t0 = Instant::now();
        let out = gpu.diffuse_generate(&model, &prompt, n_fill, steps, block_len, threshold)
            .expect("diffuse_generate");
        let el = t0.elapsed().as_secs_f64();
        println!("  api: {} tokens in {:.2}s -> {:.0} tok/s | model.diffusion={} mask={:?}",
            out.len(), el, out.len() as f64 / el, model.diffusion, model.mask_token_id);
        if let Some(t) = &tok {
            let text = t.decode(&out);
            let shown: String = text.chars().take(300).collect();
            println!("  api text: {shown}");
        }
        println!("denoise-bench: --api path complete (mechanism checks live in the default path)");
        return;
    }
    let _ = gpu.denoise_stats(&model, &tokens); // warm: pipelines + first-use compiles

    let mut step_ms = Vec::with_capacity(steps);
    let mut disp_per_step: Vec<u64> = Vec::with_capacity(steps);
    let mut submits_per_step: Vec<u64> = Vec::with_capacity(steps);
    let mut reveal_counts = Vec::with_capacity(steps);
    let mut reveal_violation = false;
    let mut count_mismatch = false;
    let mut steps_used = 0usize;
    for step in 0..steps {
        if masked.iter().all(|&b| !b) { break; }
        steps_used = step + 1;
        let t0 = Instant::now();
        let d0 = DISPATCH_SEQ.load(std::sync::atomic::Ordering::Relaxed);
        let s0: u64 = [&SUBMIT_BY_PASSES, &SUBMIT_BY_RETAIN, &SUBMIT_BY_DOWNLOAD, &SUBMIT_BY_DIMS_WRAP, &SUBMIT_BY_END_FRAME, &SUBMIT_BY_NO_FRAME]
            .iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).sum();
        let flat = gpu.denoise_stats(&model, &tokens);
        step_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        disp_per_step.push(DISPATCH_SEQ.load(std::sync::atomic::Ordering::Relaxed) - d0);
        let s1: u64 = [&SUBMIT_BY_PASSES, &SUBMIT_BY_RETAIN, &SUBMIT_BY_DOWNLOAD, &SUBMIT_BY_DIMS_WRAP, &SUBMIT_BY_END_FRAME, &SUBMIT_BY_NO_FRAME]
            .iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).sum();
        submits_per_step.push(s1 - s0);
        let raw: Vec<[f32; 4]> = flat.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        // Shift view (see header): stats presented for position i come from row i-1 when shifted.
        let stats: Vec<[f32; 4]> = (0..l).map(|i| raw[if shift_logits && i > 0 { i - 1 } else { i }]).collect();
        // Block eligibility: restrict reveals to the current block's span (whole window when --block=0).
        let elig: Vec<bool> = if block_len == 0 { masked.clone() } else {
            let done_before = |b: usize| (n_ctx + b * block_len..(n_ctx + (b + 1) * block_len).min(l)).all(|i| !masked[i]);
            let mut cb = 0usize;
            while n_ctx + cb * block_len < l && done_before(cb) { cb += 1; }
            let (lo, hi) = (n_ctx + cb * block_len, (n_ctx + (cb + 1) * block_len).min(l));
            (0..l).map(|i| masked[i] && i >= lo && i < hi).collect()
        };
        let picks = policy.select(step, &stats, &elig);
        if threshold.is_none() {
            let expect = k_fixed.min(masked.iter().filter(|&&b| b).count());
            count_mismatch |= picks.len() != expect;
        } else {
            count_mismatch |= picks.is_empty(); // threshold policy's floor guarantees progress
        }
        for &i in &picks {
            reveal_violation |= !masked[i]; // policy must only reveal masked positions
            tokens[i] = stats[i][0] as u32; // commit the argmax (already shift-aligned above)
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
        let d0 = DISPATCH_SEQ.load(std::sync::atomic::Ordering::Relaxed);
        let s0: u64 = [&SUBMIT_BY_PASSES, &SUBMIT_BY_RETAIN, &SUBMIT_BY_DOWNLOAD, &SUBMIT_BY_DIMS_WRAP, &SUBMIT_BY_END_FRAME, &SUBMIT_BY_NO_FRAME]
            .iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).sum();
        let flat = gpu.denoise_stats(&model, &tokens);
        step_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        disp_per_step.push(DISPATCH_SEQ.load(std::sync::atomic::Ordering::Relaxed) - d0);
        let s1: u64 = [&SUBMIT_BY_PASSES, &SUBMIT_BY_RETAIN, &SUBMIT_BY_DOWNLOAD, &SUBMIT_BY_DIMS_WRAP, &SUBMIT_BY_END_FRAME, &SUBMIT_BY_NO_FRAME]
            .iter().map(|c| c.load(std::sync::atomic::Ordering::Relaxed)).sum();
        submits_per_step.push(s1 - s0);
        let raw: Vec<[f32; 4]> = flat.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        let stats: Vec<[f32; 4]> = (0..l).map(|i| raw[if shift_logits && i > 0 { i - 1 } else { i }]).collect();
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
    if let Some(su) = submits_per_step.first() {
        println!("  submits/step: {su} (pass fusion {}) — the Asahi cost is per SUBMIT; this is the number the tuner owns",
            if std::env::var("LOGSCOPE_PASS_FUSE").as_deref() == Ok("0") { "OFF by env" }
            else if std::env::var("LOGSCOPE_PASS_FUSE").as_deref() == Ok("1") { "ON by env" } else { "by verdict" });
    }
    if let Some(d) = disp_per_step.first() {
        println!("  disp/step: {d} ({:.2} ms/dispatch at the mean)", step_ms.iter().sum::<f64>() / step_ms.len().max(1) as f64 / *d as f64);
    }
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
