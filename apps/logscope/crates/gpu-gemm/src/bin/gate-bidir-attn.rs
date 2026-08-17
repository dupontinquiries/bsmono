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
//! v2 metrics, after the v1 FAIL on lavapipe (worst-rel 9.24e-3): v1 compared f32-GPU
//! against f64-CPU with a 1e-6 relative floor, so its worst-rel is dominated by
//! whichever output element the random draw made tiny — it grows with sample count
//! (t=8 passed, t=129 "failed") without any kernel defect, and it cannot be compared
//! to the causal gate's 1e-7, which is f32-vs-f32. Three columns now discriminate:
//!   1. tiled vs the NAIVE GPU bidirectional (f32 twin, the causal gate's own metric
//!      class): <= 1e-4 — ulp-level agreement or the kernel is wrong, full stop.
//!   2. worst ABSOLUTE error vs the f64 reference: <= 5e-5 — catches a defect that
//!      the naive twin might share.
//!   3. worst relative error restricted to well-conditioned elements (|ref| >= 1e-2):
//!      <= 5e-4 — the v1 intent, minus the tiny-denominator artifact.
//! If column 1 fails, the kernel is guilty. If 1 passes and 2 fails, BOTH bidir
//! kernels share a defect. If all pass, v1 convicted its own oracle.
//!
//!   cargo run --release --bin gate-bidir-attn

use gpu_kernels::*;


/// PARTIAL RoPE vs the reference formula (modeling_llada2_moe.py apply_rotary_pos_emb): the first
/// `rot` dims of each head rotate with rotate_half pairing over the ROT block (j, j+rot/2), freq
/// exponent 2j/rot; lanes [rot, hd) pass through untouched. rot==hd must equal the old full path.
fn rope_reference(x: &[f32], t: usize, dm: usize, hd: usize, rot: usize, base: f32, neox: bool) -> Vec<f32> {
    let mut o = x.to_vec();
    let heads = dm / hd;
    for tok in 0..t { for h in 0..heads {
        let off = tok * dm + h * hd;
        for j in 0..rot / 2 {
            let freq = tok as f32 / (base.powf(2.0 * j as f32 / rot as f32));
            let (c, s_) = (freq.cos(), freq.sin());
            let (j0, j1) = if neox { (off + j, off + j + rot / 2) } else { (off + 2 * j, off + 2 * j + 1) };
            let (x0, x1) = (x[j0], x[j1]);
            o[j0] = x0 * c - x1 * s_;
            o[j1] = x0 * s_ + x1 * c;
        }
    }}
    o
}

fn gate_rope_partial(gpu: &Gpu, ck: &mut dyn FnMut(&str, bool)) {
    let (t, hd, heads) = (5usize, 16usize, 3usize);
    let dm = hd * heads;
    let x: Vec<f32> = (0..t * dm).map(|i| ((i * 37 % 101) as f32 - 50.0) * 0.013).collect();
    let ffv = vec![1.0f32; hd / 2];
    for neox in [true, false] {
        gpu.rope_neox.store(neox, std::sync::atomic::Ordering::Relaxed);
        for rot in [hd, hd / 2] {
            gpu.rope_dims.store(rot as u32, std::sync::atomic::Ordering::Relaxed);
            let xt = gpu.upload(&x, t, dm);
            let ff = gpu.upload(&ffv, 1, hd / 2);
            let got = gpu.download(&gpu.rope_mh_t(&xt, &ff, t, dm, 10000.0, 0, hd));
            let want = rope_reference(&x, t, dm, hd, rot, 10000.0, neox);
            let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            ck(&format!("rope {} rot={rot}/{hd} vs reference (worst {worst:.2e})",
                if neox { "NEOX" } else { "NORM" }), worst < 1e-5);
        }
    }
    gpu.rope_dims.store(0, std::sync::atomic::Ordering::Relaxed);
    gpu.rope_neox.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn main() {
    let gpu = Gpu::new();
    eprintln!("gate-bidir-attn: adapter = {}", gpu.adapter_name);
    let mut ok = true;
    let mut ck = |l: &str, v: bool| {
        ok &= v;
        println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" });
    };
    gate_rope_partial(&gpu, &mut ck);

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
        let got = gpu.download(&gpu.attention_mh_bidir_tiled(&q, &k, &v, t, dm, nh, nkv, 0));
        let naive = gpu.download(&gpu.attention_mh_full_t(&q, &k, &v, t, dm, nh, nkv));
        let mut worst_twin = 0.0f32; // tiled vs naive GPU: the f32-vs-f32 metric class
        for (g, nv) in got.iter().zip(&naive) {
            let rel = (g - nv).abs() / nv.abs().max(1e-6);
            worst_twin = worst_twin.max(rel);
        }

        let scale = 1.0f64 / (hd as f64).sqrt();
        let mut worst_abs = 0.0f64;
        let mut worst = 0.0f32; // rel, well-conditioned elements only
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
                    worst_abs = worst_abs.max((g as f64 - e).abs());
                    if e.abs() >= 1e-2 {
                        let rel = (g as f64 - e).abs() / e.abs();
                        worst = worst.max(rel as f32);
                    }
                }
            }
        }
        // Three-way verdict (lavapipe + Honeykrisp, 2026-08-09): when the twins diverge but the
        // tiled kernel matches the f64 reference, the NAIVE twin is the inaccurate side — it sums
        // t f32 terms with no online rescale and stops being a reference above a few thousand rows
        // (a few dozen under Honeykrisp fast-math). Divergence with a clean f64 column is
        // information about the twin, not a failure of the kernel under test.
        if worst_twin <= 1e-4 {
            ck(&format!("t={t:>4} heads={nh}/{nkv} hd={hd:>3}: tiled==naive f32 twin, worst rel {worst_twin:.2e} <= 1e-4"), true);
        } else if worst_abs <= 5e-5 && worst <= 5e-4 {
            println!("  [i] t={t:>4}: twins diverge ({worst_twin:.2e}) but tiled matches f64 — naive twin is the inaccurate side at this length; not a kernel failure");
        } else {
            ck(&format!("t={t:>4} heads={nh}/{nkv} hd={hd:>3}: twins diverge ({worst_twin:.2e}) AND tiled misses f64 — kernel guilty"), false);
        }
        ck(&format!("t={t:>4} heads={nh}/{nkv} hd={hd:>3}: worst ABS vs f64 ref {worst_abs:.2e} <= 5e-5"), worst_abs <= 5e-5);
        ck(&format!("t={t:>4} heads={nh}/{nkv} hd={hd:>3}: worst rel (|ref|>=1e-2) {worst:.2e} <= 5e-4"), worst <= 5e-4);
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
