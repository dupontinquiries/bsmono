//! gate-denoise-b — batched canvases against the single-canvas oracle.
//!
//! Two DIFFERENT 32-token sequences: run each through denoise_stats alone, then both
//! through denoise_stats_b(b=2) concatenated. Per-position stats must match — and at
//! L <= one attention tile the batched key walk is the same tile in the same order, so
//! the demand is bit-exactness, not tolerance. Also asserts the b==1 identity.
use gpu_kernels::*;
fn main() {
    let bytes = std::fs::read(std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2.gguf".into())).expect("run tools/make-tiny-gguf.py first");
    let gpu = Gpu::new();
    let model = gpu.load_gguf_opt(&bytes, std::env::args().nth(3).as_deref() != Some("f32")).expect("load");
    let l: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(32);
    let s1: Vec<u32> = (0..l as u32).map(|i| 1 + (i * 7) % 240).collect();
    let s2: Vec<u32> = (0..l as u32).map(|i| 3 + (i * 11) % 240).collect();
    let a1 = gpu.denoise_stats(&model, &s1);
    let a2 = gpu.denoise_stats(&model, &s2);
    let mut cat = s1.clone(); cat.extend_from_slice(&s2);
    let ab = gpu.denoise_stats_b(&model, &cat, 2);
    let id = gpu.denoise_stats_b(&model, &s1, 1);

    let mut ok = true;
    let mut ck = |lbl: &str, v: bool| { ok &= v; println!("  [{}] {lbl}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };
    ck("b=1 batched is the single path bit-identically", id == a1);
    let (h1, h2) = (&ab[..l * 4], &ab[l * 4..]);
    let worst = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    let dissect = |tag: &str, x: &[f32], y: &[f32]| {
        let names = ["argmax", "p", "margin", "entropy"];
        for c in 0..4 {
            let (mut w, mut n, mut at) = (0f32, 0usize, 0usize);
            for r in 0..x.len() / 4 {
                let d = (x[r * 4 + c] - y[r * 4 + c]).abs();
                if d > 0.0 { n += 1; if d > w { w = d; at = r; } }
            }
            if n > 0 { eprintln!("  {tag} {}: {n} rows differ, worst {w:.3e} at row {at} (batched {} vs solo {})", names[c], x[at * 4 + c], y[at * 4 + c]); }
        }
    };
    dissect("c2", &ab[l * 4..], &a2);
    ck(&format!("canvas 1 of 2 matches its solo run (worst abs {:.1e})", worst(h1, &a1)), h1 == &a1[..]);
    ck(&format!("canvas 2 of 2 matches its solo run (worst abs {:.1e})", worst(h2, &a2)), h2 == &a2[..]);
    // DISCRIMINATOR: same canvas twice. Canvas 2 == canvas 1 exactly -> the leak is
    // cross-canvas reads (clamp). Still diverges -> something row-absolute breaks the
    // second half regardless of content (Q rope, indexing).
    let mut twin = s1.clone(); twin.extend_from_slice(&s1);
    let tw = gpu.denoise_stats_b(&model, &twin, 2);
    ck("same-canvas-twice: halves identical (row-absolute vs cross-read bisector)", tw[..l * 4] == tw[l * 4..]);
    // RAW-KERNEL ISOLATION: the tiled attention alone, sl=32 batched vs per-half sl=0.
    {
        let (t2, nh, nkv, hd) = (64usize, 4usize, 2usize, 16usize);
        let (ad, kvw) = (nh * hd, nkv * hd);
        let f = |n: usize, seed: u32| -> Vec<f32> { (0..n).map(|i| (((i as u32).wrapping_mul(2654435761).wrapping_add(seed) >> 8) as f32 / 16777216.0 - 0.5) * 0.2).collect() };
        let (qh, kh, vh) = (f(t2 * ad, 1), f(t2 * kvw, 2), f(t2 * kvw, 3));
        let (q, k, v) = (gpu.upload(&qh, t2, ad), gpu.upload(&kh, t2, kvw), gpu.upload(&vh, t2, kvw));
        gpu.begin_batch_frame();
        let ob = gpu.download(&gpu.attention_mh_bidir_tiled(&q, &k, &v, t2, ad, nh, nkv, 32));
        gpu.end_frame();
        let half = |h: &[f32], w: usize, lo: usize| gpu.upload(&h[lo * w..(lo + 32) * w], 32, w);
        gpu.begin_batch_frame();
        let o0 = gpu.download(&gpu.attention_mh_bidir_tiled(&half(&qh, ad, 0), &half(&kh, kvw, 0), &half(&vh, kvw, 0), 32, ad, nh, nkv, 0));
        let o1 = gpu.download(&gpu.attention_mh_bidir_tiled(&half(&qh, ad, 32), &half(&kh, kvw, 32), &half(&vh, kvw, 32), 32, ad, nh, nkv, 0));
        gpu.end_frame();
        ck("raw kernel: batched half 0 == solo", ob[..32 * ad] == o0[..]);
        ck("raw kernel: batched half 1 == solo", ob[32 * ad..] == o1[..]);
    }
    println!("gate-denoise-b: {}", if ok {
        "PASS \u{2713} — canvases are independent under one forward; the fan-out shape is real"
    } else { "FAIL \u{2717} — the seq clamp leaks between canvases" });
    std::process::exit(if ok { 0 } else { 1 });
}
