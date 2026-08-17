//! gate-moe-t — the t>1 expert-indexed MoE path against the v1 grouped oracle.
//!
//! Same model, same tokens, same forward twice: LOGSCOPE_MOE_V2T unset runs the v1
//! grouped path (readback routing — the reference); set runs the token-axis kernels.
//! Attention and everything non-MoE is byte-identical between runs, so any divergence
//! is the new path's. Compares per-position argmax (strict where margin is healthy),
//! conditioned relative error on p/entropy, and prints the dispatch collapse — the
//! entire point of the kernel.
//!
//!   cargo run --release --bin gate-moe-t            (expects /tmp/tiny-llada2.gguf;
//!   arg1 overrides the model path)

use gpu_kernels::*;

fn main() {
    // Default is the GROUPS-FREE llada2 tiny: with group-limited routing active the GPU t-path
    // correctly DECLINES (kernels select globally until the port), which makes this gate's v1-vs-v2t
    // comparison degenerate. The groups-free variant keeps the v2t kernels under test; the grouped
    // route itself is proven against the reference transcription in the lib suite.
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2-nogroups.gguf".into());
    let gpu = Gpu::new();
    eprintln!("gate-moe-t: adapter = {}", gpu.adapter_name);
    let bytes = std::fs::read(&path).expect("read gguf (run tools/make-tiny-gguf.py first)");
    let model = gpu.load_gguf_opt(&bytes, std::env::args().nth(2).as_deref() != Some("f32")).expect("load gguf");
    assert!(model.diffusion, "expected the tiny llada2 (any MoE model works; this one is generated)");

    // The t-path requires the fused MoE kernels; a fresh process has no cached A/B verdict,
    // so arm them explicitly. v1 grouped does not read these — the baseline is unaffected.
    std::env::set_var("LOGSCOPE_FUSE_GLU", "1");
    std::env::set_var("LOGSCOPE_FUSE_DOWN", "1");
    let t = 48usize;
    let tokens: Vec<u32> = (0..t as u32).map(|i| 1 + (i * 7) % 240).collect();
    use std::sync::atomic::Ordering::Relaxed;

    std::env::remove_var("LOGSCOPE_MOE_V2T");
    let d0 = DISPATCH_SEQ.load(Relaxed);
    let base = gpu.denoise_stats(&model, &tokens);
    let d_v1 = DISPATCH_SEQ.load(Relaxed) - d0;

    std::env::set_var("LOGSCOPE_MOE_V2T", "1");
    let d1 = DISPATCH_SEQ.load(Relaxed);
    let test = gpu.denoise_stats(&model, &tokens);
    for r in [0usize, 24] {
        eprintln!("  row{r}: base [id {} p {:.4} m {:.4} h {:.4}] test [id {} p {:.4} m {:.4} h {:.4}]",
            base[r*4] as u32, base[r*4+1], base[r*4+2], base[r*4+3],
            test[r*4] as u32, test[r*4+1], test[r*4+2], test[r*4+3]);
    }
    let d_v2t = DISPATCH_SEQ.load(Relaxed) - d1;

    let mut ok = true;
    let mut ck = |l: &str, v: bool| { ok &= v; println!("  [{}] {l}", if v { "\u{2713}" } else { "\u{2717} FAIL" }); };

    let rows = t;
    let (mut argmax_mism, mut argmax_lowmargin, mut worst_p, mut worst_h) = (0usize, 0usize, 0f32, 0f32);
    for i in 0..rows {
        let (b, x) = (&base[i * 4..i * 4 + 4], &test[i * 4..i * 4 + 4]);
        if b[0] != x[0] {
            if b[2] > 0.05 { argmax_mism += 1; } else { argmax_lowmargin += 1; }
        }
        worst_p = worst_p.max((b[1] - x[1]).abs() / b[1].abs().max(1e-2));
        worst_h = worst_h.max((b[3] - x[3]).abs() / b[3].abs().max(1e-2));
    }
    ck(&format!("argmax identical on healthy-margin rows ({argmax_mism} mismatches; {argmax_lowmargin} near-tie flips tolerated)"), argmax_mism == 0);
    ck(&format!("p_top conditioned rel {worst_p:.2e} <= 2e-3"), worst_p <= 2e-3);
    ck(&format!("entropy conditioned rel {worst_h:.2e} <= 2e-3"), worst_h <= 2e-3);
    ck(&format!("dispatch collapse: v1 {d_v1} -> v2t {d_v2t} per step"), d_v2t < d_v1);

    println!("gate-moe-t: {}", if ok {
        "PASS \u{2713} — the token-axis MoE path matches the v1 oracle; LOGSCOPE_MOE_V2T is safe on this box"
    } else { "FAIL \u{2717} — keep LOGSCOPE_MOE_V2T off; the divergence is in the new kernels" });
    std::process::exit(if ok { 0 } else { 1 });
}
