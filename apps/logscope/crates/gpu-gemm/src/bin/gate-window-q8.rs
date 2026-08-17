//! gate-window-q8 — windowed decode with the INT8 EMBED TABLE, which is what hardware actually runs.
//! kernel-validate's stream-window parity check passes EmbedT::F32; every real run logs "int8 embed
//! table resident". So the Q8 gather inside the window has never been compared against anything.
//! A wrong embedding for the token the GPU just sampled corrupts EVERY model from the first
//! generated token onward — matching dense DeepSeek and diffusion LLaDA2 degenerating alike.
//!
//! Reference: stream_session (CPU sampling, f32 embed rows from the model). Contract at greedy
//! (temperature -> 0, no penalty): identical token sequences. Q8 quantization perturbs the
//! embedding slightly, so a late divergence is legitimate rounding; an IMMEDIATE divergence, or a
//! first-token mismatch, is the bug. Both are reported separately.
use gpu_kernels::*;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-qwen2.gguf".into());
    let n_new = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(24usize);
    let bytes = std::fs::read(&path).expect("read");
    let gpu = Gpu::new();
    eprintln!("gate-window-q8: adapter = {}", gpu.adapter_name);
    let m = gpu.load_gguf_opt(&bytes, false).expect("load f32");
    let mut ok = true;
    let mut ck = |l: &str, v: bool| { ok &= v; println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };

    // Greedy: temperature tiny, top_k 1, no penalty — both samplers must pick the argmax.
    let sp = SamplerParams { temperature: 1e-4, top_k: 1, top_p: 1.0, repeat_penalty: 1.0, repeat_last_n: 0, min_p: 0.0 };
    let prompt: Vec<u32> = (0..24u32).map(|i| 1 + (i * 37 % 200)).collect();

    let run = |mode: u8, k: usize| -> Vec<u32> {
        let mut s = gpu.session_new(&m, prompt.len() + n_new + 8, 7);
        gpu.session_prefill(&m, &mut s, &prompt);
        let mut seen = prompt.clone();
        let mut rng = 12345u64;
        let mut toks = Vec::new();
        let nc = || false;
        let mut cb = |t: u32| { toks.push(t); true };
        match mode {
            0 => { gpu.stream_session(&m, &mut s, &mut seen, n_new, None, &sp, &mut rng, &nc, &mut cb); }
            1 => { let re = gpu.resident_embed(&m);   // no LOGSCOPE_MOE_STREAM => f32 table
                   match &re {
                       ResidentEmbed::F32(t) => { gpu.stream_session_window(&m, &mut s, &mut seen, n_new, None, &sp, &mut rng, k, &EmbedT::F32(t), &nc, &mut cb); }
                       ResidentEmbed::Q8(q)  => { gpu.stream_session_window(&m, &mut s, &mut seen, n_new, None, &sp, &mut rng, k, &EmbedT::Q8(q), &nc, &mut cb); }
                   } }
            _ => { let q = gpu.embed_q8(&m);
                   gpu.stream_session_window(&m, &mut s, &mut seen, n_new, None, &sp, &mut rng, k, &EmbedT::Q8(&q), &nc, &mut cb); }
        }
        toks
    };

    let base = run(0, 1);
    for k in [4usize, 10] {
        let f32w = run(1, k);
        let q8w = run(2, k);
        let first_div = |a: &[u32], b: &[u32]| a.iter().zip(b).position(|(x, y)| x != y);
        ck(&format!("k={k} f32-embed window == CPU-sampled decode ({} tokens)", base.len()), f32w == base);
        match first_div(&base, &q8w) {
            None => ck(&format!("k={k} q8-embed window == CPU-sampled decode"), true),
            Some(i) => {
                // Distinguish rounding drift from a broken gather: a q8 embed perturbs logits
                // slightly, so divergence AFTER several tokens is expected on random-weight tinies.
                ck(&format!("k={k} q8-embed window: first divergence at token {i} of {} (immediate = broken gather, late = q8 rounding)", base.len()), i >= 3);
            }
        }
    }
    println!("gate-window-q8: {}", if ok { "PASS \u{2713} — the window's embed paths agree with CPU-sampled decode" }
             else { "FAIL \u{2717} — windowed decode diverges from the reference; every model decodes from wrong embeddings" });
    if !ok { std::process::exit(1); }
}
