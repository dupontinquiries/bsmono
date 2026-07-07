//! vision_encoder.rs — the CPU reference for the ViT vision encoder forward pass.
//!
//! This is the piece between `vision_preprocess::patchify` (raw pixels → `[n_patches × patch_dim]`) and the
//! image slots `multimodal::assemble` reserves: patch-embed → +positional → N transformer blocks
//! (LN → MHA → residual → LN → MLP → residual) → final LN → projector, yielding `[n_patches × out_dim]`
//! embeddings to drop into the language model. Weights are INJECTED (no file format assumed), so the math is
//! pure and unit-testable on tiny dims; this is the reference a wgpu port would match numerically — the same
//! role the naive kernels play for the GEMM work. GELU is the tanh approximation used by CLIP/ViT.

/// Encoder shape. `patch_dim` = 3·patch·patch (the flattened patch), `out_dim` = the LM embedding width the
/// projector maps into.
#[derive(Clone, Copy, Debug)]
pub struct EncoderConfig {
    pub patch_dim: usize,
    pub hidden: usize,
    pub heads: usize,
    pub layers: usize,
    pub mlp: usize,
    pub out_dim: usize,
}

/// Per-block parameters. All matrices are row-major `[in × out]`; biases length `out`. LayerNorm gains/biases
/// length `hidden`.
#[derive(Clone, Debug)]
pub struct Block {
    pub ln1_g: Vec<f32>, pub ln1_b: Vec<f32>,
    pub qkv_w: Vec<f32>, pub qkv_b: Vec<f32>,   // [hidden × 3*hidden]
    pub proj_w: Vec<f32>, pub proj_b: Vec<f32>, // [hidden × hidden]
    pub ln2_g: Vec<f32>, pub ln2_b: Vec<f32>,
    pub fc1_w: Vec<f32>, pub fc1_b: Vec<f32>,   // [hidden × mlp]
    pub fc2_w: Vec<f32>, pub fc2_b: Vec<f32>,   // [mlp × hidden]
}

/// Full encoder weights.
#[derive(Clone, Debug)]
pub struct Weights {
    pub patch_w: Vec<f32>, pub patch_b: Vec<f32>, // [patch_dim × hidden]
    pub pos: Vec<f32>,                             // [n_patches × hidden] (added per position)
    pub blocks: Vec<Block>,
    pub lnf_g: Vec<f32>, pub lnf_b: Vec<f32>,      // final LN
    pub proj_w: Vec<f32>, pub proj_b: Vec<f32>,    // projector [hidden × out_dim]
}

/// Row-major matmul: `a` is `[m × k]`, `w` is `[k × n]`, `bias` length `n` (or empty). Returns `[m × n]`.
fn linear(a: &[f32], m: usize, k: usize, w: &[f32], n: usize, bias: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; m * n];
    for i in 0..m {
        for j in 0..n {
            let mut acc = if bias.len() == n { bias[j] } else { 0.0 };
            for p in 0..k { acc += a[i * k + p] * w[p * n + j]; }
            out[i * n + j] = acc;
        }
    }
    out
}

/// LayerNorm each row of `[m × d]` with gain/bias length `d`.
fn layernorm(x: &[f32], m: usize, d: usize, g: &[f32], b: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; m * d];
    for i in 0..m {
        let row = &x[i * d..i * d + d];
        let mean = row.iter().sum::<f32>() / d as f32;
        let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / d as f32;
        let inv = 1.0 / (var + 1e-5).sqrt();
        for j in 0..d { out[i * d + j] = (row[j] - mean) * inv * g[j] + b[j]; }
    }
    out
}

/// GELU (tanh approximation).
fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + ((2.0f32 / std::f32::consts::PI).sqrt() * (x + 0.044715 * x * x * x)).tanh())
}

/// Multi-head self-attention over `[n × hidden]` given fused QKV `[n × 3*hidden]`. Returns `[n × hidden]`.
fn attention(qkv: &[f32], n: usize, hidden: usize, heads: usize) -> Vec<f32> {
    let hd = hidden / heads.max(1);
    let scale = 1.0 / (hd as f32).sqrt();
    let mut out = vec![0.0f32; n * hidden];
    // qkv row layout: [q(hidden) | k(hidden) | v(hidden)]
    let q_off = 0;
    let k_off = hidden;
    let v_off = 2 * hidden;
    for h in 0..heads.max(1) {
        let hstart = h * hd;
        for i in 0..n {
            // scores over j, softmax, weighted sum of v
            let mut scores = vec![0.0f32; n];
            let mut maxs = f32::NEG_INFINITY;
            for j in 0..n {
                let mut dot = 0.0;
                for d in 0..hd {
                    dot += qkv[i * 3 * hidden + q_off + hstart + d] * qkv[j * 3 * hidden + k_off + hstart + d];
                }
                dot *= scale;
                scores[j] = dot;
                if dot > maxs { maxs = dot; }
            }
            let mut sum = 0.0;
            for s in scores.iter_mut() { *s = (*s - maxs).exp(); sum += *s; }
            let inv = 1.0 / sum.max(1e-9);
            for d in 0..hd {
                let mut acc = 0.0;
                for j in 0..n { acc += scores[j] * inv * qkv[j * 3 * hidden + v_off + hstart + d]; }
                out[i * hidden + hstart + d] = acc;
            }
        }
    }
    out
}

/// Run the encoder over one tile's patches `[n_patches × patch_dim]`; returns `[n_patches × out_dim]`.
pub fn forward(patches: &[f32], n: usize, cfg: &EncoderConfig, w: &Weights) -> Vec<f32> {
    let h = cfg.hidden;
    // patch embed + positional
    let mut x = linear(patches, n, cfg.patch_dim, &w.patch_w, h, &w.patch_b);
    if w.pos.len() >= n * h { for i in 0..n * h { x[i] += w.pos[i]; } }
    // transformer blocks
    for blk in &w.blocks {
        let a = layernorm(&x, n, h, &blk.ln1_g, &blk.ln1_b);
        let qkv = linear(&a, n, h, &blk.qkv_w, 3 * h, &blk.qkv_b);
        let att = attention(&qkv, n, h, cfg.heads);
        let att = linear(&att, n, h, &blk.proj_w, h, &blk.proj_b);
        for i in 0..n * h { x[i] += att[i]; }               // residual
        let m = layernorm(&x, n, h, &blk.ln2_g, &blk.ln2_b);
        let mut f = linear(&m, n, h, &blk.fc1_w, cfg.mlp, &blk.fc1_b);
        for v in f.iter_mut() { *v = gelu(*v); }
        let f = linear(&f, n, cfg.mlp, &blk.fc2_w, h, &blk.fc2_b);
        for i in 0..n * h { x[i] += f[i]; }                 // residual
    }
    let x = layernorm(&x, n, h, &w.lnf_g, &w.lnf_b);
    linear(&x, n, h, &w.proj_w, cfg.out_dim, &w.proj_b)
}

/// Zero/identity weights for testing: patch-embed and projector are identity (require patch_dim==hidden==
/// out_dim), positional and all block sublayer OUTPUT weights zero (so blocks are pure residual pass-through),
/// LayerNorm gain=1/bias=0. With these, `forward` reduces to `finalLN(input)` — a clean invariant.
pub fn passthrough_weights(cfg: &EncoderConfig, n: usize) -> Weights {
    let h = cfg.hidden;
    let eye = |d: usize| { let mut m = vec![0.0f32; d * d]; for i in 0..d { m[i * d + i] = 1.0; } m };
    let ln = |d: usize| (vec![1.0f32; d], vec![0.0f32; d]);
    let (g, b) = ln(h);
    let block = Block {
        ln1_g: g.clone(), ln1_b: b.clone(),
        qkv_w: vec![0.0; h * 3 * h], qkv_b: vec![0.0; 3 * h],
        proj_w: vec![0.0; h * h], proj_b: vec![0.0; h], // zero attn output ⇒ residual only
        ln2_g: g.clone(), ln2_b: b.clone(),
        fc1_w: vec![0.0; h * cfg.mlp], fc1_b: vec![0.0; cfg.mlp],
        fc2_w: vec![0.0; cfg.mlp * h], fc2_b: vec![0.0; h], // zero mlp output ⇒ residual only
    };
    Weights {
        patch_w: eye(cfg.patch_dim), patch_b: vec![0.0; h],
        pos: vec![0.0; n * h],
        blocks: (0..cfg.layers).map(|_| block.clone()).collect(),
        lnf_g: g, lnf_b: b,
        proj_w: eye(h), proj_b: vec![0.0; cfg.out_dim],
    }
}

/// The full native image→embeddings pipeline: resample the source into tiles, patchify each, run the ViT
/// forward over each tile, and concatenate into one `[total_tokens × out_dim]` matrix — the embeddings that
/// fill the image slots `multimodal::assemble` reserved. `patch` must match the encoder's `patch_dim`
/// (= 3·patch·patch). Returns (total_tokens, matrix). This is the single call the engine makes to turn a
/// decoded image into language-model-ready vision tokens.
pub fn encode_image(
    rgb: &[u8], w_px: u32, h_px: u32, patch: u32,
    vcfg: &crate::multimodal::VisionConfig, ecfg: &EncoderConfig, weights: &Weights,
) -> (usize, Vec<f32>) {
    let vin = crate::vision_preprocess::preprocess(rgb, w_px, h_px, vcfg);
    let mut all = Vec::new();
    let mut total = 0usize;
    for tile in &vin.tiles {
        let (patches, n) = crate::vision_preprocess::patchify(tile, vin.edge, patch);
        let emb = forward(&patches, n, ecfg, weights);
        all.extend_from_slice(&emb);
        total += n;
    }
    (total, all)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(n_dim: usize) -> EncoderConfig {
        EncoderConfig { patch_dim: n_dim, hidden: n_dim, heads: 2, layers: 2, mlp: n_dim * 2, out_dim: n_dim }
    }

    #[test]
    fn encode_image_matches_the_layout_token_count() {
        use crate::multimodal::{VisionConfig, image_tokens};
        // small tiles so the test is fast: 8px tiles, 4px patches → patch_dim = 3*4*4 = 48, 4 patches/tile
        let vcfg = VisionConfig { tile: 8, patch: 4, max_tiles: 4, thumbnail: true };
        let patch = 4u32;
        let ecfg = EncoderConfig { patch_dim: (3 * patch * patch) as usize, hidden: 16, heads: 2, layers: 1, mlp: 32, out_dim: 10 };
        // enough positional rows for the largest per-tile patch count (here 4)
        let w = passthrough_weights_like(&ecfg, 4);
        let (wpx, hpx) = (20u32, 12u32);
        let rgb = vec![100u8; (wpx * hpx * 3) as usize];
        let (total, mat) = encode_image(&rgb, wpx, hpx, patch, &vcfg, &ecfg, &w);
        // total tokens = hi-res tiles' patches + thumbnail's patches; the hi-res part == image_tokens(...)
        let per_tile = ((vcfg.tile / patch) * (vcfg.tile / patch)) as usize;
        let hi_res_tiles = { let p = crate::multimodal::plan_image(wpx, hpx, &vcfg); p.tiles.len() };
        assert_eq!(image_tokens(wpx, hpx, &vcfg) as usize, hi_res_tiles * per_tile + per_tile, "layout accounts for thumbnail");
        assert_eq!(total, (hi_res_tiles + 1) * per_tile, "encoder emitted one token per patch across all tiles");
        assert_eq!(mat.len(), total * ecfg.out_dim);
    }

    // passthrough weights sized for an encoder where patch_dim != hidden (so patch_embed can't be identity):
    // use small constants; we only check shapes/counts here, not the LN invariant.
    fn passthrough_weights_like(c: &EncoderConfig, n: usize) -> Weights {
        Weights {
            patch_w: vec![0.02; c.patch_dim * c.hidden], patch_b: vec![0.0; c.hidden],
            pos: vec![0.0; n * c.hidden],
            blocks: (0..c.layers).map(|_| Block {
                ln1_g: vec![1.0; c.hidden], ln1_b: vec![0.0; c.hidden],
                qkv_w: vec![0.0; c.hidden * 3 * c.hidden], qkv_b: vec![0.0; 3 * c.hidden],
                proj_w: vec![0.0; c.hidden * c.hidden], proj_b: vec![0.0; c.hidden],
                ln2_g: vec![1.0; c.hidden], ln2_b: vec![0.0; c.hidden],
                fc1_w: vec![0.0; c.hidden * c.mlp], fc1_b: vec![0.0; c.mlp],
                fc2_w: vec![0.0; c.mlp * c.hidden], fc2_b: vec![0.0; c.hidden],
            }).collect(),
            lnf_g: vec![1.0; c.hidden], lnf_b: vec![0.0; c.hidden],
            proj_w: vec![0.02; c.hidden * c.out_dim], proj_b: vec![0.0; c.out_dim],
        }
    }

    #[test]
    fn forward_shape_is_n_by_out() {
        let c = EncoderConfig { patch_dim: 12, hidden: 8, heads: 2, layers: 1, mlp: 16, out_dim: 5 };
        let n = 4;
        // minimal random-ish weights (all small constants) just to exercise shapes
        let w = Weights {
            patch_w: vec![0.01; c.patch_dim * c.hidden], patch_b: vec![0.0; c.hidden],
            pos: vec![0.0; n * c.hidden],
            blocks: vec![Block {
                ln1_g: vec![1.0; c.hidden], ln1_b: vec![0.0; c.hidden],
                qkv_w: vec![0.01; c.hidden * 3 * c.hidden], qkv_b: vec![0.0; 3 * c.hidden],
                proj_w: vec![0.01; c.hidden * c.hidden], proj_b: vec![0.0; c.hidden],
                ln2_g: vec![1.0; c.hidden], ln2_b: vec![0.0; c.hidden],
                fc1_w: vec![0.01; c.hidden * c.mlp], fc1_b: vec![0.0; c.mlp],
                fc2_w: vec![0.01; c.mlp * c.hidden], fc2_b: vec![0.0; c.hidden],
            }],
            lnf_g: vec![1.0; c.hidden], lnf_b: vec![0.0; c.hidden],
            proj_w: vec![0.01; c.hidden * c.out_dim], proj_b: vec![0.0; c.out_dim],
        };
        let out = forward(&vec![0.5; n * c.patch_dim], n, &c, &w);
        assert_eq!(out.len(), n * c.out_dim);
    }

    #[test]
    fn passthrough_reduces_to_final_layernorm() {
        // with pass-through weights, forward(x) == layernorm(x) row-wise → each output row has ~0 mean, ~1 var
        let c = cfg(8);
        let n = 3;
        let w = passthrough_weights(&c, n);
        // distinct per-row inputs
        let mut x = vec![0.0f32; n * c.patch_dim];
        for i in 0..n { for j in 0..c.patch_dim { x[i * c.patch_dim + j] = (i as f32 + 1.0) * (j as f32 - 3.0); } }
        let out = forward(&x, n, &c, &w);
        for i in 0..n {
            let row = &out[i * c.out_dim..i * c.out_dim + c.out_dim];
            let mean = row.iter().sum::<f32>() / c.out_dim as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / c.out_dim as f32;
            assert!(mean.abs() < 1e-4, "row {i} normalized mean ~0 (got {mean})");
            assert!((var - 1.0).abs() < 1e-2, "row {i} normalized var ~1 (got {var})");
        }
    }

    #[test]
    fn deterministic() {
        let c = cfg(6);
        let n = 4;
        let w = passthrough_weights(&c, n);
        let x: Vec<f32> = (0..n * c.patch_dim).map(|i| (i as f32 * 0.3).sin()).collect();
        assert_eq!(forward(&x, n, &c, &w), forward(&x, n, &c, &w));
    }

    #[test]
    fn attention_rows_are_convex_combinations() {
        // uniform Q,K ⇒ uniform softmax ⇒ each output row is the MEAN of the V rows.
        let (n, hidden, heads) = (3usize, 4usize, 2usize);
        let mut qkv = vec![0.0f32; n * 3 * hidden];
        // q,k all equal (→ uniform attention); v distinct per row
        for i in 0..n {
            for d in 0..hidden {
                qkv[i * 3 * hidden + d] = 1.0;            // q
                qkv[i * 3 * hidden + hidden + d] = 1.0;   // k
                qkv[i * 3 * hidden + 2 * hidden + d] = (i * hidden + d) as f32; // v
            }
        }
        let out = attention(&qkv, n, hidden, heads);
        // mean of v over rows, per dim
        for d in 0..hidden {
            let mean = (0..n).map(|i| (i * hidden + d) as f32).sum::<f32>() / n as f32;
            for i in 0..n { assert!((out[i * hidden + d] - mean).abs() < 1e-4, "uniform attn ⇒ mean of V"); }
        }
    }
}
