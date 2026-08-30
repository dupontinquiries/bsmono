//! count-dispatch — dispatches per decoded token, which is device-independent and therefore
//! measurable here.
//!
//! c4-820 and c4-821 each removed one dispatch per layer. On a 16-layer model that is 32 fewer per
//! token, and the operator's log went the other way: 11698 dispatches for 71 tokens before, 12095
//! after. Either the fusions are not firing on a real model, or something else grew. Counting is
//! the only way to tell, and dispatch count does not depend on the GPU.
//! DRIVES THE PATH THE APP USES. The first version called Gpu::generate and reported 32 dispatches
//! per layer where the operator's 1B shows about 10.6, with an f32 fixture and a Q8_0 one counting
//! IDENTICALLY — which cannot happen where the q8 fused kernels are reachable. That entry point is
//! not what the app decodes through; backend.rs calls stream_session_window, so this does too.
//!
//! On this path: 32 dispatches/token at 2 layers, 44 at 3 — 12 PER LAYER and 8 fixed, against the
//! operator'''s ~10.6 on a real 16-layer 1B. The legacy path said 32 per layer, so the discrepancy
//! was the entry point, not the model.
//!
//! An f32 fixture and a Q8_0 one still count identically here, and that is CORRECT rather than
//! suspicious: load_gguf_opt(bytes, true) quantises f32 source weights into int8 residency, so both
//! end up on the same fused path. Measuring the fusion'''s effect needs a before/after build, not two
//! source quantisations.
use gpu_kernels::{Gpu, EmbedT, ResidentEmbed, SamplerParams};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or("/tmp/tiny-qwen3-q8.gguf".into());
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let k: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let bytes = std::fs::read(&path).expect("model");
    let gpu = Gpu::new();
    // Quantize to resident, or the fused kernels are unreachable and this measures the fallback.
    let m = gpu.load_gguf_opt(&bytes, true).expect("load");

    // Raw ids: the tiny fixtures cannot encode English (c4-796), and the count does not depend on
    // WHICH tokens are decoded.
    let prompt: Vec<u32> = (1..9u32).map(|i| i % (m.vocab as u32).max(1)).collect();
    let sp = SamplerParams { temperature: 0.0, top_k: 1, top_p: 1.0,
                             repeat_penalty: 1.0, repeat_last_n: 0, min_p: 0.0 };
    let mut s = gpu.session_new(&m, prompt.len() + n + 8, 1);
    gpu.session_prefill(&m, &mut s, &prompt);

    use std::sync::atomic::Ordering::Relaxed;
    let mut seen = prompt.to_vec();
    let mut rng = 1u64;
    // The embed table the engine itself would use, f32 or int8 depending on residency.
    let re = gpu.resident_embed(&m);
    // Prefill dispatches scale with the prompt and would swamp the per-token figure, so the
    // baseline is taken AFTER prefill.
    let before = gpu_kernels::GPU_DISPATCHES.load(Relaxed);
    // SUBMIT CAUSES on this path, which is the whole of C11-21a and the one thing this harness
    // did not report. Dispatches alone cannot distinguish "the window batched and the readback
    // is per-window" from "something inside the loop flushed every token" — both leave the
    // dispatch count identical, and only the second explains a measured 1.0 submits/token
    // against a windowed prediction of 1/k. Same baseline point as the dispatch count, so
    // every number below shares one clock.
    let causes0 = gpu_kernels::SubmitCauseSnapshot::now();
    let passes0 = gpu_kernels::GPU_PASSES_OPENED.load(Relaxed);
    let submits0 = gpu_kernels::GPU_SUBMITS.load(Relaxed);
    let produced = match &re {
        ResidentEmbed::F32(t) => gpu.stream_session_window(&m, &mut s, &mut seen, n, None, &sp,
            &mut rng, k, &EmbedT::F32(t), &|| false, |_| true),
        ResidentEmbed::Q8(q) => gpu.stream_session_window(&m, &mut s, &mut seen, n, None, &sp,
            &mut rng, k, &EmbedT::Q8(q), &|| false, |_| true),
    };
    let after = gpu_kernels::GPU_DISPATCHES.load(Relaxed);

    // The tokens themselves, so a fusion can be checked for CORRECTNESS by the same command that
    // measures it: a dispatch reduction that changes output is not an optimisation.
    let ids: Vec<String> = seen[prompt.len()..].iter().map(|t| t.to_string()).collect();
    println!("tokens={}", ids.join(","));
    // Per-shader breakdown, so the remaining dispatches are a list rather than an argument.
    // Shader ids are source-pointer identities, not names — the engine keys pipelines that way —
    // so this reports the id and its share. Matching an id to a kernel is done by elimination
    // against the counts, which is enough to say WHICH is worth fusing next.
    let hist = gpu_kernels::dispatch_histogram();
    if !hist.is_empty() {
        let total: u64 = hist.iter().map(|(_, c)| *c).sum();
        println!("per-shader breakdown ({} distinct kernels, {total} dispatches):", hist.len());
        for (id, c) in hist.iter().take(12) {
            let name = gpu_kernels::shader_label(*id).unwrap_or_else(|| format!("{id:#x}"));
            println!("  {:<52} {c:>6}  {:>5.1}%  {:>5.2}/layer-token",
                name, 100.0 * *c as f64 / total as f64,
                *c as f64 / produced.max(1) as f64 / m.n_layers as f64);
        }
    }
    let p = produced.max(1);
        // Bind-group cache attribution, when the cache is on. This tool decodes through the same path
    // the app does, so the ratio it reports is the ratio the operator will see — and unlike the
    // app, it runs here.
    if std::env::var("LOGSCOPE_BG_CACHE").is_ok() {
        use std::sync::atomic::Ordering::Relaxed;
        let (h, mo, md, mi) = (gpu_kernels::BG_HITS.load(Relaxed),
                               gpu_kernels::BG_MISS_OUT.load(Relaxed),
                               gpu_kernels::BG_MISS_DIMS.load(Relaxed),
                               gpu_kernels::BG_MISS_INPUTS.load(Relaxed));
        println!("bg-cache: hits={h} repeat-misses: output={mo} dims={md} inputs={mi}");
    }
    println!("layers={} window={k} produced={produced} dispatches={} => {:.1} per token, {:.2} per layer-token",
        m.n_layers, after - before, (after - before) as f64 / p as f64,
        (after - before) as f64 / p as f64 / m.n_layers as f64);

    // ── the windowed path's submit budget ────────────────────────────────────────────────────
    let submits = gpu_kernels::GPU_SUBMITS.load(Relaxed).saturating_sub(submits0);
    let passes = gpu_kernels::GPU_PASSES_OPENED.load(Relaxed).saturating_sub(passes0);
    println!("{}", gpu_kernels::submit_attribution_since(causes0, submits));
    println!("submits={submits} => {:.2} per token (windowed prediction: {:.2} = 1 readback per \
              window of {k}) | passes={passes} => {:.1} per token",
        submits as f64 / p as f64, 1.0 / k as f64, passes as f64 / p as f64);

    // THE CEILING THIS MEASURES AGAINST. `stream_session_window` pins
    // `flush_every = k * per_tok` with `per_tok = 64`, commented as a "generous upper bound on
    // dispatches/token" — but the comparison it feeds is `frame.passes >= flush_every`, so the
    // quantity that has to fit under it is PASSES per token, not dispatches. When passes/token
    // exceeds 64 the pass counter fires inside the window and the window stops being one submit,
    // silently: nothing logs it, and the run just reports a poor tok/s.
    //
    // A tiny fixture has few layers and sits well under the ceiling, which is exactly why this
    // never showed up here. Extrapolating from the per-layer figure is what makes a 3-layer
    // measurement say something about a 16-layer model, so the extrapolation is printed rather
    // than left for the reader.
    let per_tok_budget = 64.0_f64;
    let passes_per_layer_token = passes as f64 / p as f64 / m.n_layers.max(1) as f64;
    let layers_until_overflow = per_tok_budget / passes_per_layer_token.max(f64::MIN_POSITIVE);
    println!("flush ceiling: per_tok={per_tok_budget:.0} passes/token; this model uses {:.1} \
              passes/token ({:.2} per layer-token) => the ceiling is crossed at ~{:.0} layers, \
              and above that the window flushes mid-loop and submits/token climbs toward 1.0",
        passes as f64 / p as f64, passes_per_layer_token, layers_until_overflow);
}
