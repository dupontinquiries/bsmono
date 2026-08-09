//! gate-bidir-attn — correctness oracle for `attention_mh_bidir_tiled`, the denoise
//! step's attention.
//!
//! Reference is an independent f64 CPU implementation (not a diff against the naive
//! GPU kernel), so a shared mistake can't pass both sides. Shapes cover GQA 4:1,
//! MQA, odd t not divisible by the 64-thread stride, hd=128 (three register slots),
//! and t=2100 — the case that crosses the 2048-row shared-memory tile and exercises
//! the cross-tile online-softmax rescale under the NON-causal bound, which no
//! existing gate reaches.
//!
//! Tolerance: 1e-3 worst relative. f32 online accumulation against an f64 reference
//! drifts with sqrt(t) — at t=2100 that is ~1e-5..1e-4 rounding scale; a mask bug or
//! rescale bug shows up at 1e-1 scale and cannot hide under this bar.
//!
//!   cargo run --release --bin gate-bidir-attn

use gpu_kernels::*;

fn main() {
    let gpu = Gpu::new();
    eprintln!("gate-bidir-attn: adapter = {}", gpu.adapter_name);
    let mut ok = true;
    let mut ck = |l: &str, v: bool| {
        ok &= v;
        println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };

    let mut rng = 0xb1d1_4770_0000_0001u64;
    let mut rnd = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };

    for &(t, nh, nkv, hd) in &[
        (8usize, 4usize, 4usize, 32usize),
        (32, 8, 2, 64),
        (64, 8, 8, 128),
        (129, 4, 1, 96),
        (2100, 4, 2, 64), // crosses the 2048-row tile: the rescale path, non-causal
    ] {
        let dm = nh * hd;
        let kvdm = nkv * hd;
        let qv: Vec<f32> = (0..t * dm).map(|_| rnd()).collect();
        let kv: Vec<f32> = (0..t * kvdm).map(|_| rnd()).collect();
        let vv: Vec<f32> = (0..t * kvdm).map(|_| rnd()).collect();
        let q = gpu.upload(&qv, t, dm);
        let k = gpu.upload(&kv, t, kvdm);
        let v = gpu.upload(&vv, t, kvdm);
        let got = gpu.download(&gpu.attention_mh_bidir_tiled(&q, &k, &v, t, dm, nh, nkv));

        let scale = 1.0f64 / (hd as f64).sqrt();
        let mut worst = 0.0f32;
        for i in 0..t {
            for h in 0..nh {
                let kb = (h / (nh / nkv)) * hd;
                // scores over ALL t keys — the non-causal contract under test
                let mut s: Vec<f64> = (0..t)
                    .map(|j| {
                        (0..hd)
                            .map(|c| qv[i * dm + h * hd + c] as f64 * kv[j * kvdm + kb + c] as f64)
                            .sum::<f64>()
                            * scale
                    })
                    .collect();
                let m = s.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                let den: f64 = s.iter().map(|z| (z - m).exp()).sum();
                for z in s.iter_mut() {
                    *z = (*z - m).exp() / den;
                }
                for c in 0..hd {
                    let e: f64 = (0..t).map(|j| s[j] * vv[j * kvdm + kb + c] as f64).sum();
                    let g = got[i * dm + h * hd + c];
                    let rel = (g as f64 - e).abs() / e.abs().max(1e-6);
                    worst = worst.max(rel as f32);
                }
            }
        }
        ck(&format!("t={t:>4} heads={nh}/{nkv} hd={hd:>3}: worst rel {worst:.2e} <= 1e-3"), worst <= 1e-3);
    }

    println!(
        "gate-bidir-attn: {}",
        if ok {
            "PASS \u{2713} — non-causal tiled attention agrees with the f64 reference; the denoise step may use it"
        } else {
            "FAIL \u{2717} — do NOT wire the diffuser to this kernel; the message above names the shape"
        }
    );
    std::process::exit(if ok { 0 } else { 1 });
}
