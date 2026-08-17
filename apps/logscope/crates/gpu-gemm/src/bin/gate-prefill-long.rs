//! gate-prefill-long — does CHUNKED prefill agree with token-by-token decode at REAL prompt
//! lengths? Every existing gate prefills 5-token prompts; the two models producing gibberish on
//! hardware both carried ~1700-1900 token prompts through a 512-token chunk, i.e. four chunks with
//! three boundaries. A defect that only appears at a chunk boundary (position bookkeeping, KV
//! append offset, mask start) is invisible to every gate we own and corrupts EVERY model — which is
//! exactly the symptom: dense DeepSeek and diffusion LLaDA2 both degenerate.
//!
//! Reference is `session_prefill`: one decode_step per token, the same path a single-token decode
//! uses, cross-verified against llama.cpp by the oracle. The comparison is the LAST token's logits
//! (what generation actually starts from) plus a follow-on decode_step, which reads the whole KV
//! cache and so catches a cache written correctly at the end but wrongly in the middle.
//!
//!   cargo run --release --bin gate-prefill-long -- /tmp/tiny-qwen2.gguf [lengths...]
use gpu_kernels::*;

fn argmax(v: &[f32]) -> usize { v.iter().enumerate().fold((0, f32::NEG_INFINITY), |(bi, bv), (i, &x)| if x > bv { (i, x) } else { (bi, bv) }).0 }

fn rel(a: &[f32], b: &[f32]) -> f32 {
    let scale = a.iter().map(|x| x.abs()).fold(1e-6f32, f32::max);
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max) / scale
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-qwen2.gguf".into());
    let lens: Vec<usize> = {
        let rest: Vec<usize> = std::env::args().skip(2).filter_map(|a| a.parse().ok()).collect();
        if rest.is_empty() { vec![5, 300, 512, 513, 600, 1024, 1700] } else { rest }
    };
    let bytes = std::fs::read(&path).expect("read model");
    let gpu = Gpu::new();
    eprintln!("gate-prefill-long: adapter = {}", gpu.adapter_name);
    let m = gpu.load_gguf_opt(&bytes, false).expect("load f32");

    let mut ok = true;
    let mut ck = |l: &str, v: bool| { ok &= v; println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };

    for &t in &lens {
        // Deterministic pseudo-prompt; ids stay inside the vocab.
        let prompt: Vec<u32> = (0..t as u32).map(|i| 1 + (i * 7919 % (m.vocab as u32 - 2))).collect();

        // Reference: token-by-token.
        let mut sref = gpu.session_new(&m, t + 8, 1);
        gpu.session_prefill(&m, &mut sref, &prompt);
        let want = sref.logits.clone();
        let want_next = gpu.decode_step(&m, &sref.cache, prompt[0], sref.pos);

        for chunk in [512usize, 256, 1 << 20] {
            let c = gpu.new_cache(m.n_layers, t + 8, m.kv_dim());
            let got = gpu.prefill_chunked(&m, &c, &prompt, chunk);
            let label = if chunk > t { format!("single-chunk") } else { format!("chunk={chunk}") };
            if got.len() != want.len() {
                ck(&format!("t={t:>5} {label}: logits length {} == {}", got.len(), want.len()), false);
                continue;
            }
            let r = rel(&want, &got);
            let am = argmax(&want) == argmax(&got);
            ck(&format!("t={t:>5} {label}: last-token logits match sequential (rel {r:.2e}, argmax {})",
                if am { "same" } else { "DIFFERENT" }), r < 2e-3 && am);

            // The cache itself: one more step reads every position written above.
            let got_next = gpu.decode_step(&m, &c, prompt[0], t);
            let r2 = rel(&want_next, &got_next);
            ck(&format!("t={t:>5} {label}: follow-on step over the whole cache (rel {r2:.2e}, argmax {})",
                if argmax(&want_next) == argmax(&got_next) { "same" } else { "DIFFERENT" }),
                r2 < 2e-3 && argmax(&want_next) == argmax(&got_next));
        }
    }

    println!("gate-prefill-long: {}", if ok {
        "PASS \u{2713} — chunked prefill agrees with token-by-token decode at every length tested"
    } else {
        "FAIL \u{2717} — chunked prefill DIVERGES; every model with a prompt this long decodes from wrong state"
    });
    if !ok { std::process::exit(1); }
}
