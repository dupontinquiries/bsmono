//! probe-bc — is forward TRUNCATION equivalent to the reference's BLOCK-CAUSAL mask?
//! Truncating at the block end gives the current block's rows the same VISIBLE TOKEN SET the
//! reference gives them. But inside a truncated forward the prefix rows attend FORWARD into the
//! block, so at layer 2 the block's rows read prefix activations the reference would never produce.
//! If the two disagree, our diffusion output is not the reference's, on any model with >1 layer.
use gpu_kernels::*;
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/tiny-llada2.gguf".into());
    let blk: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(4);
    let bytes = std::fs::read(&path).expect("read");
    let gpu = Gpu::new();
    let m = gpu.load_gguf_opt(&bytes, false).expect("load f32");
    let mask = m.mask_token_id.expect("mask id");
    let prompt: Vec<u32> = (0..10u32).map(|i| 1 + (i * 13) % 200).collect();
    let mut toks = prompt.clone();
    toks.extend(std::iter::repeat(mask).take(blk));
    let lo = prompt.len();
    let n = toks.len() - lo;
    // ALIGNMENT CHECK first: does a block row see its OWN block's last token? The kernel derives
    // the block index as i/bcz from position 0, but the loop's blocks start at n_ctx. When
    // n_ctx % bcz != 0 those boundaries disagree and a row is masked against part of its own block.
    {
        let mut p1 = prompt.clone(); p1.extend(std::iter::repeat(mask).take(blk));
        let mut p2 = p1.clone(); *p2.last_mut().unwrap() = 7;   // perturb the block's LAST token
        let a = gpu.denoise_stats_rows_bc(&m, &p1, lo, 1, blk, lo);
        let b = gpu.denoise_stats_rows_bc(&m, &p2, lo, 1, blk, lo);
        let moved = (a[1] - b[1]).abs() > 1e-7;
        println!("alignment: prompt={} blk={blk} (n_ctx%blk={}) — first block row {} its own block's last token",
            prompt.len(), prompt.len() % blk, if moved { "SEES" } else { "CANNOT SEE" });
    }
    let trunc = gpu.denoise_stats_rows(&m, &toks, lo, n);
    let bc = gpu.denoise_stats_rows_bc(&m, &toks, lo, n, blk, lo);
    println!("layers={} block={blk}: current-block rows, truncation vs block-causal", m.n_layers);
    let mut worst = 0.0f32; let mut argmax_diff = 0;
    for i in 0..n {
        let (ta, ba) = (trunc[i * 4] as u32, bc[i * 4] as u32);
        if ta != ba { argmax_diff += 1; }
        let d = (trunc[i * 4 + 1] - bc[i * 4 + 1]).abs();
        worst = worst.max(d);
        println!("  row {i}: argmax trunc={ta:<4} bc={ba:<4} {}  p_top {:.6} vs {:.6}",
            if ta == ba { "same " } else { "DIFFER" }, trunc[i * 4 + 1], bc[i * 4 + 1]);
    }
    println!("worst p_top delta {worst:.3e}, {argmax_diff}/{n} argmax differ");
    println!("{}", if worst > 1e-6 || argmax_diff > 0 {
        "VERDICT: truncation != block-causal. The reference masks block-causally at EVERY layer; our\n\
         truncated forward lets prefix rows see into the block, and layer 2+ carries that difference\n\
         into the block's own predictions. Our diffusion output is not the reference's."
    } else {
        "VERDICT: identical here — truncation reproduces block-causal at this depth."
    });
}
