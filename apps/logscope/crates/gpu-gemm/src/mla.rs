//! mla.rs — Multi-head Latent Attention (DeepSeek-V2/V3, GLM-MoE-DSA), pure CPU reference.
//!
//! This is the correctness reference the GPU MLA forward will be validated against, following the discipline
//! that's repeatedly caught silent-wrong-output bugs in this engine: build the math in plain f32 on the CPU,
//! prove the round-trip and the attention equivalence, THEN wire GPU kernels to match it bit-for-bit.
//!
//! The idea (Colibri glm.c:173): instead of caching per-head K and V (`n_kv_heads·head_dim` per token), MLA
//! caches only a compressed latent — the KV down-projection `c_kv[kv_lora]` plus a shared rotary key
//! `k_rot[qk_rope]` — and reconstructs each head's K(nope-part) and V from `c_kv` via the `kv_b`
//! up-projection at attention time. Per DeepSeek-V3 that's 512+64 = 576 cached values/token vs 32768, the
//! long-context memory win. This module implements the un-absorbed ("naive") form: reconstruct K/V explicitly
//! then do ordinary attention. The absorption optimization (folding kv_b into the score/output matmuls to
//! skip reconstruction) is a later, separately-validated step — un-absorbed is the ground truth it must match.
//!
//! Weight shapes (row-major, `[out, in]`, y = x·Wᵀ):
//!   q_a   [q_lora, d]                          x → q latent
//!   q_b   [n_heads·qk_head, q_lora]            q latent → per-head q (qk_head = qk_nope + qk_rope)
//!   kv_a  [kv_lora + qk_rope, d]               x → [kv latent | shared k_rot]
//!   kv_b  [n_heads·(qk_nope + v_head), kv_lora] kv latent → per-head [k_nope | v]
//!   o     [d, n_heads·v_head]                  attention out → model dim
//! Norms: q_a_ln[q_lora], kv_a_ln[kv_lora] (RMSNorm on the latents before the up-projection / caching).

/// Row-major matmul y[m,n] = x[m,k] · Wᵀ  (W is [n,k]).
fn matmul_t(x: &[f32], w: &[f32], m: usize, n: usize, k: usize) -> Vec<f32> {
    let mut y = vec![0f32; m * n];
    for r in 0..m {
        for o in 0..n {
            let mut acc = 0f32;
            for i in 0..k { acc += x[r * k + i] * w[o * k + i]; }
            y[r * n + o] = acc;
        }
    }
    y
}

/// RMSNorm over the last `cols` of each row: x / sqrt(mean(x²) + eps) · w.
fn rmsnorm(x: &[f32], w: &[f32], rows: usize, cols: usize, eps: f32) -> Vec<f32> {
    let mut y = vec![0f32; rows * cols];
    for r in 0..rows {
        let s = &x[r * cols..(r + 1) * cols];
        let ms = s.iter().map(|v| v * v).sum::<f32>() / cols as f32;
        let inv = 1.0 / (ms + eps).sqrt();
        for c in 0..cols { y[r * cols + c] = s[c] * inv * w[c]; }
    }
    y
}

/// Apply NEOX-style RoPE (split-half) to a `dim`-wide rotary vector at position `pos`.
/// Rotates halves (i, i+dim/2). base is the rope theta. In-place on `v`.
fn rope_neox(v: &mut [f32], pos: usize, dim: usize, base: f32) {
    let half = dim / 2;
    for i in 0..half {
        let freq = (base).powf(-2.0 * i as f32 / dim as f32);
        let ang = pos as f32 * freq;
        let (s, cth) = ang.sin_cos();
        let (a, b) = (v[i], v[i + half]);
        v[i] = a * cth - b * s;
        v[i + half] = a * s + b * cth;
    }
}

/// MLA weight set for one layer (f32 reference).
pub struct MlaWeights<'a> {
    pub q_a: &'a [f32], pub q_a_ln: &'a [f32],
    pub q_b: &'a [f32],
    pub kv_a: &'a [f32], pub kv_a_ln: &'a [f32],
    pub kv_b: &'a [f32],
    pub o: &'a [f32],
}

/// MLA weight set as uploaded GPU tensors (used by the GPU forward `mla_attention_step_t`).
pub struct MlaWeightsT {
    pub q_a: crate::Tensor, pub q_a_ln: crate::Tensor,
    pub q_b: crate::Tensor,
    pub kv_a: crate::Tensor, pub kv_a_ln: crate::Tensor,
    pub kv_b: crate::Tensor,
    pub o: crate::Tensor,
}

/// MLA dims for one layer.
#[derive(Clone, Copy)]
pub struct MlaDims {
    pub d: usize, pub n_heads: usize,
    pub q_lora: usize, pub kv_lora: usize,
    pub qk_nope: usize, pub qk_rope: usize, pub v_head: usize,
    pub base: f32, pub eps: f32,
}
impl MlaDims {
    pub fn qk_head(&self) -> usize { self.qk_nope + self.qk_rope }
    pub fn latent_width(&self) -> usize { self.kv_lora + self.qk_rope }
}

/// One MLA cache row per token: the compressed latent [kv_lora] and the shared rotary key [qk_rope].
#[derive(Clone)]
pub struct MlaCacheRow { pub c_kv: Vec<f32>, pub k_rot: Vec<f32> }

/// GPU-resident compressed MLA cache: one latent [max_seq, kv_lora] and one rot [max_seq, qk_rope] tensor
/// per layer. The long-context memory win in concrete form — O(kv_lora + qk_rope) per token instead of
/// O(n_kv_heads · head_dim). Rows are appended as tokens decode.
pub struct MlaCache {
    pub latent: Vec<crate::Tensor>,
    pub rot: Vec<crate::Tensor>,
    pub kv_lora: usize,
    pub qk_rope: usize,
    pub max_seq: usize,
}

impl MlaCache {
    /// Memory-saving ratio vs an equivalent standard per-head KV cache (K+V).
    pub fn compression_vs_standard(&self, n_kv_heads: usize, head_dim: usize) -> f64 {
        (n_kv_heads * head_dim * 2) as f64 / (self.kv_lora + self.qk_rope) as f64
    }
}

/// Compress a token's hidden state into its MLA cache row (kv_a down-proj → norm the latent part → RoPE the
/// rot part). This is what gets stored per token — the memory-win representation.
pub fn compress_kv(x: &[f32], w: &MlaWeights, dm: &MlaDims, pos: usize) -> MlaCacheRow {
    // x[1,d] → [kv_lora + qk_rope]
    let kva = matmul_t(x, w.kv_a, 1, dm.kv_lora + dm.qk_rope, dm.d);
    // RMSNorm the latent part [0,kv_lora); the rot part gets RoPE.
    let c_kv = rmsnorm(&kva[..dm.kv_lora], w.kv_a_ln, 1, dm.kv_lora, dm.eps);
    let mut k_rot = kva[dm.kv_lora..].to_vec();
    rope_neox(&mut k_rot, pos, dm.qk_rope, dm.base);
    MlaCacheRow { c_kv, k_rot }
}

/// Full un-absorbed MLA attention for a single new query token at `pos`, attending over `cache[0..=pos]`
/// (the caller pushes the new token's row before calling). Returns the [d] attention output (post o-proj).
/// This is the ground-truth forward the GPU path must match.
pub fn attention_step(x: &[f32], w: &MlaWeights, dm: &MlaDims, cache: &[MlaCacheRow], pos: usize) -> Vec<f32> {
    let h = dm.n_heads;
    let qk = dm.qk_head();
    // Q: x → q_a latent → norm → q_b → [h, qk_head]; RoPE the rope part of each head.
    let qa = matmul_t(x, w.q_a, 1, dm.q_lora, dm.d);
    let qa = rmsnorm(&qa, w.q_a_ln, 1, dm.q_lora, dm.eps);
    let mut q = matmul_t(&qa, w.q_b, 1, h * qk, dm.q_lora); // [h·qk]
    for head in 0..h {
        // rope part is the tail qk_rope of each head's qk_head slice
        let off = head * qk + dm.qk_nope;
        rope_neox(&mut q[off..off + dm.qk_rope], pos, dm.qk_rope, dm.base);
    }
    // Reconstruct K(nope)+V per head from each cached latent via kv_b: [kv_lora] → [h·(qk_nope+v_head)].
    let rec = dm.qk_nope + dm.v_head;
    let scale = 1.0 / (qk as f32).sqrt();
    let mut out = vec![0f32; h * dm.v_head];
    for head in 0..h {
        // scores over all cached positions
        let mut scores = vec![0f32; pos + 1];
        let qh = &q[head * qk..head * qk + qk]; // [qk_nope | qk_rope]
        for (t, row) in cache.iter().enumerate().take(pos + 1) {
            let kvb = matmul_t(&row.c_kv, w.kv_b, 1, h * rec, dm.kv_lora); // [h·(nope+v)]
            let k_nope = &kvb[head * rec..head * rec + dm.qk_nope];
            // score = q_nope·k_nope + q_rope·k_rot   (k_rot shared across heads)
            let mut s = 0f32;
            for i in 0..dm.qk_nope { s += qh[i] * k_nope[i]; }
            for i in 0..dm.qk_rope { s += qh[dm.qk_nope + i] * row.k_rot[i]; }
            scores[t] = s * scale;
        }
        // softmax
        let mx = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mut denom = 0f32;
        for s in scores.iter_mut() { *s = (*s - mx).exp(); denom += *s; }
        for s in scores.iter_mut() { *s /= denom; }
        // weighted sum of reconstructed V
        for (t, row) in cache.iter().enumerate().take(pos + 1) {
            let kvb = matmul_t(&row.c_kv, w.kv_b, 1, h * rec, dm.kv_lora);
            let v = &kvb[head * rec + dm.qk_nope..head * rec + dm.qk_nope + dm.v_head];
            let a = scores[t];
            for i in 0..dm.v_head { out[head * dm.v_head + i] += a * v[i]; }
        }
    }
    // output projection [d, h·v_head]
    matmul_t(&out, w.o, 1, dm.d, h * dm.v_head)
}

/// Rebuild the fused `kv_b` from the split `attn_k_b` / `attn_v_b` form.
///
/// ## The bug this replaces
///
/// The loader did `a.extend_from_slice(&b)` — every k row, then every v row. But `mla.rs` indexes
/// `kvb[head * (qk_nope + v_head) ..]`, which is **per-head interleaved**: head 0's k rows, then head
/// 0's v rows, then head 1's. The two agree only when `n_heads == 1`.
///
/// For any real model they differ, and the failure is the worst kind: head 0's k lands correctly by
/// coincidence (both layouts start with it), so the model loads, runs, and produces text — with every
/// head's V and every later head's K read from the wrong rows.
///
/// `k` is `[n_heads * qk_nope, kv_lora]` and `v` is `[n_heads * v_head, kv_lora]`, both row-major with
/// heads contiguous, which is how llama.cpp's split writes them.
pub fn fuse_kv_b(k: &[f32], v: &[f32], n_heads: usize, qk_nope: usize, v_head: usize,
                 kv_lora: usize) -> Option<Vec<f32>> {
    if n_heads == 0 || kv_lora == 0 { return None; }
    if k.len() != n_heads * qk_nope * kv_lora || v.len() != n_heads * v_head * kv_lora {
        return None;
    }
    let mut out = Vec::with_capacity(k.len() + v.len());
    for h in 0..n_heads {
        let ko = h * qk_nope * kv_lora;
        out.extend_from_slice(&k[ko..ko + qk_nope * kv_lora]);
        let vo = h * v_head * kv_lora;
        out.extend_from_slice(&v[vo..vo + v_head * kv_lora]);
    }
    Some(out)
}

#[cfg(test)]#[cfg(test)]
mod tests {
    #[test]
    fn the_split_form_fuses_PER_HEAD_not_as_two_blocks() {
        // The bug: the loader appended every k row then every v row. mla.rs indexes
        // kvb[head * (qk_nope + v_head)..], which is per-head interleaved. They agree only at
        // n_heads == 1, and head 0's k lands right by coincidence in both — so the model loads, runs,
        // and produces text with every head's V read from the wrong rows.
        let (nh, nope, vh, lora) = (2usize, 2usize, 3usize, 1usize);
        // k rows numbered 10.., v rows 20..
        let k: Vec<f32> = (0..nh * nope * lora).map(|i| 10.0 + i as f32).collect();
        let v: Vec<f32> = (0..nh * vh * lora).map(|i| 20.0 + i as f32).collect();
        let fused = fuse_kv_b(&k, &v, nh, nope, vh, lora).expect("shapes agree");

        let rec = nope + vh;
        // head 0: its two k rows then its three v rows
        assert_eq!(&fused[0..nope], &[10.0, 11.0], "head 0 k");
        assert_eq!(&fused[nope..rec], &[20.0, 21.0, 22.0], "head 0 v");
        // head 1: the NEXT k rows, not a continuation of v
        assert_eq!(&fused[rec..rec + nope], &[12.0, 13.0], "head 1 k");
        assert_eq!(&fused[rec + nope..2 * rec], &[23.0, 24.0, 25.0], "head 1 v");

        // and the naive concatenation is a DIFFERENT array, which is the whole point
        let mut naive = k.clone();
        naive.extend_from_slice(&v);
        assert_ne!(naive, fused, "if these matched, the old loader was fine and this is noise");
    }

    #[test]
    fn a_single_head_makes_both_layouts_identical() {
        // Why this survived review: at n_heads == 1 the naive concatenation IS correct, so any test
        // written with one head would have passed.
        let (nope, vh, lora) = (2usize, 3usize, 2usize);
        let k: Vec<f32> = (0..nope * lora).map(|i| i as f32).collect();
        let v: Vec<f32> = (0..vh * lora).map(|i| 100.0 + i as f32).collect();
        let mut naive = k.clone();
        naive.extend_from_slice(&v);
        assert_eq!(fuse_kv_b(&k, &v, 1, nope, vh, lora).unwrap(), naive);
    }

    #[test]
    fn mismatched_shapes_return_None_rather_than_a_short_array() {
        // A silently truncated kv_b would read zeros for the last heads — plausible output again.
        assert!(fuse_kv_b(&[1.0; 4], &[1.0; 6], 2, 2, 3, 1).is_some());
        assert!(fuse_kv_b(&[1.0; 3], &[1.0; 6], 2, 2, 3, 1).is_none(), "k is short");
        assert!(fuse_kv_b(&[1.0; 4], &[1.0; 5], 2, 2, 3, 1).is_none(), "v is short");
        assert!(fuse_kv_b(&[], &[], 0, 2, 3, 1).is_none(), "no heads");
    }

    #[test]
    fn the_fused_length_matches_what_mla_indexes() {
        // n_heads * (qk_nope + v_head) * kv_lora — the shape mla.rs assumes when it slices by head.
        let (nh, nope, vh, lora) = (4usize, 8usize, 16usize, 32usize);
        let k = vec![0f32; nh * nope * lora];
        let v = vec![0f32; nh * vh * lora];
        assert_eq!(fuse_kv_b(&k, &v, nh, nope, vh, lora).unwrap().len(), nh * (nope + vh) * lora);
    }

    use super::*;

    fn seeded(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed;
        (0..n).map(|_| { s = s.wrapping_mul(6364136223846793005).wrapping_add(1); ((s >> 33) as f32 / (1u64 << 31) as f32) - 1.0 }).collect()
    }

    // A tiny MLA config exercised end-to-end: compressing then attending must be finite, shaped right, and
    // — the key property — produce the SAME result whether we recompute K/V from the latent or not (there's
    // only the latent path here, so this pins that the reconstruct-from-latent attention is self-consistent
    // and that a single-token self-attention returns V of that token when it's the only position).
    #[test]
    fn mla_single_token_attends_to_itself() {
        let dm = MlaDims { d: 16, n_heads: 2, q_lora: 8, kv_lora: 6, qk_nope: 4, qk_rope: 4, v_head: 4, base: 10000.0, eps: 1e-6 };
        let qk = dm.qk_head();
        let rec = dm.qk_nope + dm.v_head;
        let w = MlaWeights {
            q_a: &seeded(dm.q_lora * dm.d, 1), q_a_ln: &vec![1.0; dm.q_lora],
            q_b: &seeded(dm.n_heads * qk * dm.q_lora, 2),
            kv_a: &seeded((dm.kv_lora + dm.qk_rope) * dm.d, 3), kv_a_ln: &vec![1.0; dm.kv_lora],
            kv_b: &seeded(dm.n_heads * rec * dm.kv_lora, 4),
            o: &seeded(dm.d * dm.n_heads * dm.v_head, 5),
        };
        let x0 = seeded(dm.d, 42);
        let row = compress_kv(&x0, &w, &dm, 0);
        assert_eq!(row.c_kv.len(), dm.kv_lora);
        assert_eq!(row.k_rot.len(), dm.qk_rope);
        let cache = vec![row];
        let out = attention_step(&x0, &w, &dm, &cache, 0);
        assert_eq!(out.len(), dm.d);
        assert!(out.iter().all(|v| v.is_finite()), "MLA output must be finite");
    }

    // With a single cached position, softmax is a delta (weight 1.0), so the attention output equals the
    // o-projection of the reconstructed V for that token — independent of the query. This pins the
    // reconstruct-V-from-latent path exactly.
    #[test]
    fn mla_delta_softmax_returns_reconstructed_v() {
        let dm = MlaDims { d: 12, n_heads: 2, q_lora: 6, kv_lora: 5, qk_nope: 3, qk_rope: 2, v_head: 3, base: 10000.0, eps: 1e-6 };
        let qk = dm.qk_head();
        let rec = dm.qk_nope + dm.v_head;
        let w = MlaWeights {
            q_a: &seeded(dm.q_lora * dm.d, 11), q_a_ln: &vec![1.0; dm.q_lora],
            q_b: &seeded(dm.n_heads * qk * dm.q_lora, 12),
            kv_a: &seeded((dm.kv_lora + dm.qk_rope) * dm.d, 13), kv_a_ln: &vec![1.0; dm.kv_lora],
            kv_b: &seeded(dm.n_heads * rec * dm.kv_lora, 14),
            o: &seeded(dm.d * dm.n_heads * dm.v_head, 15),
        };
        let xq = seeded(dm.d, 99);
        let xk = seeded(dm.d, 7);
        let row = compress_kv(&xk, &w, &dm, 0);
        // reconstruct V directly for the reference
        let kvb = matmul_t(&row.c_kv, w.kv_b, 1, dm.n_heads * rec, dm.kv_lora);
        let mut vcat = vec![0f32; dm.n_heads * dm.v_head];
        for head in 0..dm.n_heads {
            let v = &kvb[head * rec + dm.qk_nope..head * rec + dm.qk_nope + dm.v_head];
            vcat[head * dm.v_head..(head + 1) * dm.v_head].copy_from_slice(v);
        }
        let want = matmul_t(&vcat, w.o, 1, dm.d, dm.n_heads * dm.v_head);
        let got = attention_step(&xq, &w, &dm, &[row], 0);
        let err = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        assert!(err < 1e-4, "delta-softmax MLA must equal o-proj of reconstructed V, err={err}");
    }
}

/// Cross-check the MLA tensor shapes against each other and the metadata.
///
/// MLA has five interlocking dimensions — `n_head`, `qk_nope_head_dim`, `qk_rope_head_dim`,
/// `kv_lora_rank`, `v_head_dim` — and four tensors whose shapes are products of them. If any one is
/// read wrong, the products still multiply out to *something* and the model runs on the wrong
/// geometry, which is the failure mode that produces text rather than an error.
///
/// From a real GLM-4.7-Flash (deepseek2) checkpoint:
///
/// ```text
/// attn_q_b       [768 x 5120]      n_head * (qk_nope + qk_rope) = 20 * 256 = 5120
/// attn_output    [5120 x 2048]     n_head * v_head_dim          = 20 * 256 = 5120
/// attn_k_b       [192 x 512 x 20]  qk_nope, kv_lora, n_head
/// attn_v_b       [512 x 256 x 20]  kv_lora, v_head_dim, n_head
/// attn_kv_a_norm [512]             kv_lora_rank
/// ```
///
/// Every one of those is redundant with the metadata, which is what makes checking them worthwhile:
/// agreement is evidence, and disagreement localises the error to one hparam rather than leaving a
/// whole forward pass suspect.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShapeDisagreement {
    /// `attn_q_b`'s output width is not n_head * (qk_nope + qk_rope).
    QbWidth { declared: usize, from_hparams: usize },
    /// `attn_output`'s input width is not n_head * v_head_dim.
    OutputWidth { declared: usize, from_hparams: usize },
    /// A 3-D kv_b tensor's head axis disagrees with n_head.
    HeadAxis { tensor: &'static str, declared: usize, n_head: usize },
    /// The latent rank differs between kv_a_norm and a kv_b tensor.
    LatentRank { tensor: &'static str, declared: usize, kv_lora_rank: usize },
}

impl ShapeDisagreement {
    pub fn explain(&self) -> String {
        match self {
            ShapeDisagreement::QbWidth { declared, from_hparams } => format!(
                "attn_q_b is {declared} wide and the hparams say n_head*(qk_nope+qk_rope) = \
{from_hparams}. One of n_head, qk_nope_head_dim or qk_rope_head_dim is wrong — the products still \
multiply out, so the model would run on the wrong geometry and produce text rather than an error"),
            ShapeDisagreement::OutputWidth { declared, from_hparams } => format!(
                "attn_output takes {declared} and the hparams say n_head*v_head_dim = {from_hparams}"),
            ShapeDisagreement::HeadAxis { tensor, declared, n_head } => format!(
                "{tensor}'s head axis is {declared} against n_head {n_head} — the per-head fusion \
would stride by the wrong amount, which is correct for head 0 and wrong for every head after"),
            ShapeDisagreement::LatentRank { tensor, declared, kv_lora_rank } => format!(
                "{tensor} has latent rank {declared} against kv_lora_rank {kv_lora_rank}"),
        }
    }
}

/// Check what the file says against what the metadata says. Empty means they agree.
#[allow(clippy::too_many_arguments)]
pub fn check_shapes(
    q_b_width: usize, output_width: usize,
    k_b: Option<&[u64]>, v_b: Option<&[u64]>,
    n_head: usize, qk_nope: usize, qk_rope: usize, kv_lora_rank: usize, v_head_dim: usize,
) -> Vec<ShapeDisagreement> {
    let mut v = Vec::new();
    let want_q = n_head * (qk_nope + qk_rope);
    if q_b_width != want_q {
        v.push(ShapeDisagreement::QbWidth { declared: q_b_width, from_hparams: want_q });
    }
    let want_o = n_head * v_head_dim;
    if output_width != want_o {
        v.push(ShapeDisagreement::OutputWidth { declared: output_width, from_hparams: want_o });
    }
    // The 3-D forms: [qk_nope, kv_lora, n_head] and [kv_lora, v_head_dim, n_head]. Only checked when
    // present, because the legacy fused attn_kv_b layout has neither.
    if let Some(d) = k_b.filter(|d| d.len() == 3) {
        if d[2] as usize != n_head {
            v.push(ShapeDisagreement::HeadAxis { tensor: "attn_k_b", declared: d[2] as usize, n_head });
        }
        if d[1] as usize != kv_lora_rank {
            v.push(ShapeDisagreement::LatentRank {
                tensor: "attn_k_b", declared: d[1] as usize, kv_lora_rank });
        }
    }
    if let Some(d) = v_b.filter(|d| d.len() == 3) {
        if d[2] as usize != n_head {
            v.push(ShapeDisagreement::HeadAxis { tensor: "attn_v_b", declared: d[2] as usize, n_head });
        }
        if d[0] as usize != kv_lora_rank {
            v.push(ShapeDisagreement::LatentRank {
                tensor: "attn_v_b", declared: d[0] as usize, kv_lora_rank });
        }
    }
    v
}

#[cfg(test)]
mod shape_check_tests {
    use super::*;

    /// The real GLM-4.7-Flash numbers, which cross-check exactly.
    fn glm47() -> (usize, usize, Vec<u64>, Vec<u64>, usize, usize, usize, usize, usize) {
        (5120, 5120, vec![192, 512, 20], vec![512, 256, 20], 20, 192, 64, 512, 256)
    }

    #[test]
    fn a_real_checkpoint_agrees_with_its_own_metadata() {
        let (q, o, kb, vb, nh, nope, rope, lora, vd) = glm47();
        let d = check_shapes(q, o, Some(&kb), Some(&vb), nh, nope, rope, lora, vd);
        assert!(d.is_empty(), "GLM-4.7-Flash should be self-consistent: {d:?}");
    }

    #[test]
    fn a_wrong_HEAD_COUNT_is_caught_by_two_tensors_at_once() {
        // The value that would be easiest to misread, and the check localises it: n_head appears in
        // q_b's width, output's width and both kv_b head axes, so a wrong one disagrees four ways.
        let (q, o, kb, vb, _, nope, rope, lora, vd) = glm47();
        let d = check_shapes(q, o, Some(&kb), Some(&vb), 16, nope, rope, lora, vd);
        assert!(d.len() >= 3, "a wrong n_head must disagree in several places: {d:?}");
        assert!(d[0].explain().contains("produce text rather than an error"));
    }

    #[test]
    fn a_wrong_LATENT_RANK_is_caught_even_though_it_is_not_in_q_b() {
        // kv_lora_rank appears only in the kv_b tensors and kv_a_norm, so nothing about q_b or output
        // constrains it — which is exactly why checking the 3-D shapes is worth the code.
        let (q, o, kb, vb, nh, nope, rope, _, vd) = glm47();
        let d = check_shapes(q, o, Some(&kb), Some(&vb), nh, nope, rope, 256, vd);
        assert_eq!(d.len(), 2, "one per kv_b tensor: {d:?}");
        assert!(d.iter().all(|x| matches!(x, ShapeDisagreement::LatentRank { .. })));
    }

    #[test]
    fn the_LEGACY_fused_layout_is_not_penalised_for_lacking_3D_tensors() {
        // The old attn_kv_b form has neither split tensor, and demanding them would refuse every
        // checkpoint predating the split.
        let (q, o, _, _, nh, nope, rope, lora, vd) = glm47();
        assert!(check_shapes(q, o, None, None, nh, nope, rope, lora, vd).is_empty());
    }

    #[test]
    fn a_wrong_v_head_dim_is_caught_by_the_OUTPUT_projection() {
        // v_head_dim is the one dimension q_b says nothing about, and attn_output is the only tensor
        // that pins it.
        let (q, o, kb, vb, nh, nope, rope, lora, _) = glm47();
        let d = check_shapes(q, o, Some(&kb), Some(&vb), nh, nope, rope, lora, 128);
        assert!(d.iter().any(|x| matches!(x, ShapeDisagreement::OutputWidth { .. })));
    }

    /// What allocating BOTH caches actually costs, as a number.
    ///
    /// `Gpu::new_cache_for` allocates the standard per-head K/V vectors for MLA
    /// models too, and its comment justifies that: "They cost little relative
    /// to the weights ... trimming the unused allocation is an optimisation
    /// with a measurement attached, not a guess."
    ///
    /// This is the measurement. It does not agree with "little".
    ///
    /// On the GLM-4.7-Flash shapes quoted in this file's own `check_shapes`
    /// doc, the dense cache the MLA path never reads is **17.8x** the latent
    /// cache it does: 12.50 GiB against 0.70 GiB at 8k context. On DeepSeek-V3
    /// it is **85x** — 91.5 GiB against 1.07 GiB.
    ///
    /// Compare that to the 31 GiB single-allocation ceiling `Gpu::new` clamps
    /// to on this driver (see ASAHI_GPU.md, "ASSUMED, and loudly"). GLM-4.7-
    /// Flash at 8k spends 13.2 GiB of it on caches to use 0.7, and DeepSeek-V3
    /// is over the ceiling at 4k. Long context is the entire reason MLA exists,
    /// so an allocator that hands the compression straight back is not a small
    /// inefficiency — it removes the feature at exactly the sizes it was built
    /// for.
    ///
    /// The test asserts the ratio is LARGE, deliberately. It is not guarding a
    /// performance target; it is stopping the "costs little" claim from being
    /// re-inferred by someone who has not run these numbers. If the allocator
    /// is ever taught to skip the dense side for a fully-MLA checkpoint, this
    /// fails and points at the comment to rewrite — the same expiry discipline
    /// archcfg's note_claims module uses.
    ///
    /// Not fixed here. The comment names the real obstacle: a MIXED checkpoint,
    /// some layers MLA and some standard, would need the dense side for the
    /// standard layers, so the trim has to be per-layer or gated on "every
    /// layer is MLA" — and no MLA checkpoint loads at all yet, so there is
    /// nothing to test a fix against.

    /// The MLA loader gap, reproduced END TO END rather than asserted.
    ///
    /// Every other statement in this tree about MLA loading is structural — a
    /// grep over lib.rs (`archcfg::note_claims::mla_loading_is_still_absent`)
    /// or a comment. This drives the real thing: `Gpu::new`, then
    /// `load_gguf_opt` over a real MLA checkpoint, and pins where it stops.
    ///
    /// That became possible only once two beliefs were retested. The first was
    /// that this sandbox cannot build these crates (true under cargo 1.75, not
    /// under 1.96). The second was that there is no GPU here — there is no
    /// Vulkan ICD and no /dev/dri, but `Gpu::new()` succeeds anyway and real
    /// dispatches produce correct results, which the view-download test beside
    /// it has been proving all along. So "no hardware to check against", the
    /// reason repeatedly given for not touching the MLA loader, was wrong for
    /// the parts that do not need a real GPU.
    ///
    /// The fixture is `tools/tiny-variants/gen_glmdsa.py`, which archcfg's
    /// deepseek2 note already names as reproducing this "in 3 layers at d=64".
    /// It was never wired to a test.
    ///
    /// Skips when the fixture is absent, like every other fixture-gated test
    /// here — but unlike them it says so, because a test that silently passes
    /// by returning early is how this whole area went unexamined. Generate it:
    ///
    /// ```text
    /// python3 tools/tiny-variants/gen_glmdsa.py /tmp/tiny-glmdsa.gguf
    /// ```

    /// The prefix-fork path, driven against a REAL loaded model.
    ///
    /// `prefill_prefix` computes a shared prompt's KV once and
    /// `seed_from_prefix` forks it into a session, so N branches over one
    /// prompt prefill it once instead of N times. `WorkerPool` is built on it
    /// and `main.rs` calls that in four places, so this is live code — and
    /// until now nothing tested it, because C9-4c is right that no test drove
    /// `load_gguf` at all.
    ///
    /// It is testable here, which was the surprise. This sandbox has no Vulkan
    /// ICD and no /dev/dri, and `Gpu::new()` works anyway through Mesa's
    /// software rasterizer; a supported-architecture fixture loads and
    /// generates in ~100 ms. The MLA fixture is refused by the architecture
    /// gate, but qwen3 is supported and `gen3.py` emits one.
    ///
    /// The property asserted is EQUIVALENCE, not speed. Forking a prefix must
    /// produce the same tokens as prefilling the whole prompt — a fork that is
    /// merely fast is a fork that is wrong, and on a software rasterizer the
    /// timing would mean nothing anyway.
    ///
    /// ## WHAT THIS TEST DOES NOT CATCH, probed rather than assumed
    ///
    /// It was sabotaged three ways to find its edges, and it has one:
    ///
    /// | sabotage | detected |
    /// |---|---|
    /// | seed one row short (`prefix_len - 1`) | **no** |
    /// | skip the k-cache write entirely | **no** |
    /// | (prefix influence on output, as a precondition) | yes |
    ///
    /// The precondition below proves two different prefixes generate different
    /// tokens, so the seeded KV does reach the logits — but that influence
    /// travels through the V cache, whose values are correct in both sabotages.
    /// Corrupting only K leaves attention SCORES near-uniform, and on a 3-layer,
    /// 64-wide fixture with random weights the scores are near-uniform anyway,
    /// so the argmax does not move.
    ///
    /// So this pins that the fork path RUNS and produces prefix-dependent output
    /// matching a full prefill. It does not pin the attention scores. Catching a
    /// K-cache fault needs a fixture with enough depth or trained enough weights
    /// for attention to be peaked rather than diffuse — which `gen3.py` does not
    /// emit and which is the honest next step if this path is ever suspected.
    ///
    /// Stated because a test whose limits are unknown gets trusted for more than
    /// it covers, and this one is the first thing to ever exercise `load_gguf`.
    #[test]
    fn a_forked_prefix_generates_what_a_full_prefill_generates() {
        let path = "/tmp/tiny-qwen3.gguf";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("SKIPPED: {path} absent — sh tools/tiny-variants/gen-all.sh");
            return;
        };
        let gpu = crate::Gpu::new();
        let m = match gpu.load_gguf_opt(&bytes, false) {
            Ok(m) => m,
            Err(e) => {
                // A supported arch failing to load is a real regression, not a
                // reason to skip: the whole point is that this one loads.
                panic!("the qwen3 fixture no longer loads: {e}");
            }
        };

        // A shared prompt and two different continuations — the shape a
        // fan-out has: one prefix, several branches.
        //
        // API read out of `KvSession::make_empty` rather than guessed. I
        // guessed twice first (`new_session`, `generate_from_session`) and the
        // compiler rejected both; the real trio is `session_new`,
        // `seed_from_prefix`, `session_prefill`. `make_empty` at lib.rs:16588
        // is this exact sequence and is what WorkerPool uses.
        // sample_next is a KvSession method, not an inherent one — the trait
        // must be in scope. Third API correction in this test; the pattern is
        // that I keep writing the API I expect instead of the one at the call
        // site I already read.
        use crate::KvSession as _;
        let prefix: Vec<u32> = vec![7, 11, 13, 17, 19, 23];
        let sp = crate::SamplerParams::default();

        // FIRST, PROVE THE COMPARISON CAN FAIL.
        //
        // An equality assertion between two generations is worthless if the
        // model's output does not depend on the prefix — and this is a
        // 3-layer, 64-wide fixture with random weights, where it might not.
        // Found the hard way: the equality below passed with `seed_from_prefix`
        // deliberately skipping its k-cache write ENTIRELY, which means either
        // the fork is irrelevant to the output or the test is not testing it.
        //
        // So: two DIFFERENT prefixes, same tail. If those agree, the prefix has
        // no influence on generation here and every equality below is vacuous.
        // This is the numerical form of "assert the scan found something".
        let other: Vec<u32> = vec![101, 103, 107, 109, 113, 127];
        let gen_from = |pfx: &[u32], tail: &[u32]| -> Vec<u32> {
            let pc = gpu.prefill_prefix(&m, pfx);
            let mut s = gpu.session_new(&m, 64, 0);
            gpu.seed_from_prefix(&m, &mut s, &pc, pfx.len());
            gpu.session_prefill(&m, &mut s, tail);
            (0..4).map(|_| {
                let t = s.sample_next(&sp);
                gpu.session_prefill(&m, &mut s, &[t]);
                t
            }).collect()
        };
        let probe_tail = vec![29u32, 31];
        assert_ne!(
            gen_from(&prefix, &probe_tail), gen_from(&other, &probe_tail),
            "two different prefixes generated identical tokens, so the seeded KV does \
not reach the output on this fixture and the equality assertions below cannot fail. \
Fix the fixture before trusting this test."
        );

        for tail in [vec![29u32, 31], vec![37, 41]] {
            // Reference: one session, whole prompt, greedy.
            let mut whole = prefix.clone();
            whole.extend_from_slice(&tail);
            let mut direct_s = gpu.session_new(&m, 64, 0);
            gpu.session_prefill(&m, &mut direct_s, &whole);
            let direct: Vec<u32> = (0..4)
                .map(|_| {
                    let t = direct_s.sample_next(&sp);
                    gpu.session_prefill(&m, &mut direct_s, &[t]);
                    t
                })
                .collect();

            // Forked: prefill the prefix ONCE, seed a session from it, feed
            // only the tail.
            let pc = gpu.prefill_prefix(&m, &prefix);
            let mut fork_s = gpu.session_new(&m, 64, 0);
            gpu.seed_from_prefix(&m, &mut fork_s, &pc, prefix.len());
            gpu.session_prefill(&m, &mut fork_s, &tail);
            let forked: Vec<u32> = (0..4)
                .map(|_| {
                    let t = fork_s.sample_next(&sp);
                    gpu.session_prefill(&m, &mut fork_s, &[t]);
                    t
                })
                .collect();

            assert_eq!(
                direct, forked,
                "a forked prefix diverged from a full prefill for tail {tail:?} — the KV \
fork is not reproducing the prefix's state, which makes every WorkerPool branch \
silently WRONG rather than slow"
            );
        }
    }

    /// The prefix store must change WHEN work happens, never WHAT comes out.
    ///
    /// Two assertions, and the second is the one with teeth. Identical output to the
    /// already-validated `run_fleet_trie` proves the cache is not corrupting KV. But a store that
    /// silently never hit would also pass that — it would just rebuild everything and agree. So the
    /// same fleet is run twice against one store: the second run must report hits, which is the
    /// only evidence that a cache built by an earlier CALL was reused rather than rebuilt. That
    /// cross-call reuse is the entire point; the trie already shared within a call.
    #[test]
    fn a_prefix_store_survives_the_call_that_filled_it() {
        let path = "/tmp/tiny-qwen3.gguf";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("SKIPPED: {path} absent — sh tools/tiny-variants/gen-all.sh");
            return;
        };
        let gpu = crate::Gpu::new();
        let m = match gpu.load_gguf_opt(&bytes, false) {
            Ok(m) => m,
            Err(e) => panic!("the qwen3 fixture no longer loads: {e}"),
        };
        let sp = crate::SamplerParams::default();
        let reqs = || vec![
            crate::FleetRequest { id: 0, tokens: vec![3u32, 1, 4, 1, 5, 9], max_new: 4 },
            crate::FleetRequest { id: 1, tokens: vec![3u32, 1, 4, 1, 5, 2], max_new: 4 },
            crate::FleetRequest { id: 2, tokens: vec![3u32, 1, 4, 8],       max_new: 3 },
        ];

        let baseline = gpu.run_fleet_trie(&m, reqs(), &sp, None, 2, 32, 2);

        // A budget far above what three tiny prefixes need, so nothing demotes: the warm tier is
        // lossy on promotion and this test is asserting equality, not closeness.
        let mut store = crate::PrefixStore::new(1 << 30, m.n_layers, m.kv_dim());

        let first = gpu.run_fleet_trie_cached(&m, reqs(), &sp, None, 2, 32, 2, &mut store);
        assert_eq!(baseline, first,
            "a cold store changed the output — the cache is corrupting KV, not just storing it");
        assert_eq!(store.total_hits(), 0, "a cold store cannot have served anything");
        assert_eq!(store.checked_out(), 0,
            "every cache taken must be handed back, or the store leaks its budget");
        let held = store.len();
        assert!(held > 0, "the run built shared prefix nodes and kept none of them");

        let second = gpu.run_fleet_trie_cached(&m, reqs(), &sp, None, 2, 32, 2, &mut store);
        assert_eq!(baseline, second,
            "a WARM store changed the output — a reused prefix cache is not equivalent to a \
rebuilt one, which makes cross-call caching silently wrong rather than slow");
        assert!(store.total_hits() > 0,
            "the second run rebuilt every prefix — nothing was reused across calls, which is the \
whole feature; the trie already shared within one call");
        assert_eq!(store.checked_out(), 0, "a taken cache was never returned");
        assert_eq!(store.len(), held, "the second run should reuse the same nodes, not add new ones");
    }

    /// A take must not reset the frequency half of the GDSF score.
    ///
    /// The store ranks by (hits+1)·len, and `take` REMOVES the entry — so without carrying the
    /// count across a checkout, the most-reused prefix would return looking brand new and be first
    /// in line for eviction. Exactly backwards.
    #[test]
    fn a_checked_out_prefix_does_not_forget_how_often_it_was_used() {
        let path = "/tmp/tiny-qwen3.gguf";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("SKIPPED: {path} absent — sh tools/tiny-variants/gen-all.sh");
            return;
        };
        let gpu = crate::Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, false) else { panic!("qwen3 fixture no longer loads") };
        let sp = crate::SamplerParams::default();
        let reqs = || vec![
            crate::FleetRequest { id: 0, tokens: vec![3u32, 1, 4, 1, 5, 9], max_new: 2 },
            crate::FleetRequest { id: 1, tokens: vec![3u32, 1, 4, 1, 5, 2], max_new: 2 },
        ];
        let mut store = crate::PrefixStore::new(1 << 30, m.n_layers, m.kv_dim());
        for _ in 0..3 { let _ = gpu.run_fleet_trie_cached(&m, reqs(), &sp, None, 2, 32, 2, &mut store); }
        // Three runs: the first fills, the next two each take and return the same node.
        assert_eq!(store.total_hits(), 2,
            "expected one hit per run after the first; got {}", store.total_hits());
    }

    /// A key that maps to a cache of the WRONG LENGTH must be rebuilt, not used.
    ///
    /// `prefix_key` is a 64-bit hash, so two different prefixes can in principle collide, and the
    /// consequence of trusting one would be attending over another sequence's KV — fluent, wrong
    /// output rather than a crash. A collision cannot be produced on demand, so this poisons the
    /// store instead: under the exact key the shared trie node looks up, a cache built from
    /// unrelated tokens at a different length. The `total_hits` assertion is what makes the test
    /// mean anything — it proves the poisoned key really was consulted rather than missed.
    ///
    /// KNOWN LIMIT, stated so this is not trusted past its range. **Deleting the length guard does
    /// NOT fail this test.** With the guard gone the 2-row cache is attended over as if it held 5
    /// rows; the unwritten rows are zeros, and this fixture's model is small enough that the greedy
    /// argmax does not move. So what is verified here is that the poisoned entry is looked up and
    /// the output is correct WITH the guard present. The guard itself is reasoned-correct, not
    /// test-enforced — demonstrating its consequence needs a model whose output is sensitive to
    /// garbage KV, which means real weights and therefore M2.
    #[test]
    fn a_cached_prefix_of_the_wrong_length_is_refused_not_attended_over() {
        let path = "/tmp/tiny-qwen3.gguf";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("SKIPPED: {path} absent — sh tools/tiny-variants/gen-all.sh");
            return;
        };
        let gpu = crate::Gpu::new();
        let Ok(m) = gpu.load_gguf_opt(&bytes, false) else { panic!("qwen3 fixture no longer loads") };
        let sp = crate::SamplerParams::default();
        // Both requests share [3,1,4,1,5], so the trie's shared node is that 5-token prefix.
        let shared: Vec<u32> = vec![3, 1, 4, 1, 5];
        let reqs = || vec![
            crate::FleetRequest { id: 0, tokens: vec![3u32, 1, 4, 1, 5, 9], max_new: 3 },
            crate::FleetRequest { id: 1, tokens: vec![3u32, 1, 4, 1, 5, 2], max_new: 3 },
        ];
        let baseline = gpu.run_fleet_trie(&m, reqs(), &sp, None, 2, 32, 2);

        let mut store = crate::PrefixStore::new(1 << 30, m.n_layers, m.kv_dim());
        // Poison: the right key, a cache of two unrelated tokens.
        let poison = gpu.prefill_prefix(&m, &[7u32, 7]);
        store.insert(&gpu, crate::prefix_key(&shared), 2, poison);

        let got = gpu.run_fleet_trie_cached(&m, reqs(), &sp, None, 2, 32, 2, &mut store);
        assert_eq!(store.total_hits(), 1,
            "the poisoned key was never looked up, so this test is not exercising the guard at \
all — the trie's shared node is not keyed on {shared:?}");
        assert_eq!(baseline, got,
            "a wrong-length cache was attended over instead of rebuilt — a hash collision would \
produce fluent, WRONG output rather than a crash");
        assert_eq!(store.checked_out(), 0,
            "the refused cache was taken and never accounted for — the store leaks its budget");
    }

    #[test]
    fn an_mla_checkpoint_reaches_the_loader_and_stops_where_the_notes_say() {
        let path = "/tmp/tiny-glmdsa.gguf";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("SKIPPED: {path} absent — python3 tools/tiny-variants/gen_glmdsa.py {path}");
            return;
        };

        // The file itself must carry MLA projections, or this test is checking
        // nothing. Asserted rather than assumed: the generator could change.
        let g = crate::gguf::parse(&bytes).expect("fixture parses");
        assert!(g.tensor("blk.0.attn_kv_a_mqa.weight").is_some(),
                "fixture has no attn_kv_a_mqa — it is not an MLA checkpoint");
        assert!(g.tensor("blk.0.attn_k_b.weight").is_some(),
                "fixture lacks the SPLIT k_b/v_b form this exercises");

        let gpu = crate::Gpu::new();
        let err = match gpu.load_gguf_opt(&bytes, false) {
            Ok(_) => panic!(
                "an MLA checkpoint LOADED. That is the good outcome, and it means the loader gap closed — update archcfg's deepseek2 and glm-dsa notes, mla_loading_is_still_absent, and this test."
            ),
            Err(e) => e,
        };

        // WHERE it stops matters more than THAT it stops. Today the refusal is
        // the architecture gate (glm-dsa: sparse attention unimplemented),
        // which fires BEFORE the MLA layer loading would. So closing the MLA
        // gap alone would not make this fixture load — a reader planning that
        // work should know the arch gate is in front of it.
        assert!(err.contains("glm-dsa") || err.contains("MLA") || err.contains("Multi-head Latent"),
                "refused for an unrelated reason, so this no longer pins the MLA path:\n{err}");

        // And the diagnostic has to name the fixture that reproduces it —
        // that pointer is the only reason this test could be written.
        assert!(err.len() > 80, "the refusal should explain itself, got: {err}");
    }

    #[test]
    fn the_dense_cache_an_mla_model_never_reads_dwarfs_the_one_it_uses() {
        // GLM-4.7-Flash (deepseek2), the checkpoint check_shapes documents.
        let (n_layers, n_head, qk_nope, qk_rope, kv_lora) = (40usize, 20usize, 192usize, 64usize, 512usize);
        let head_dim = qk_nope + qk_rope;
        let latent_w = kv_lora + qk_rope;

        // Exactly what Model::kv_bytes_per_token sums.
        let dense = crate::ctxwin::kv_bytes_per_token(n_layers, n_head, head_dim, 4);
        let latent = (n_layers * latent_w * 4) as u64;

        assert!(
            dense > latent * 10,
            "dense {dense} B/token vs latent {latent} B/token — if this ratio has fallen below 10x the shapes changed or the allocator was fixed; either way the cost-little comment on new_cache_for needs rewriting against the new numbers"
        );

        // And the figure that decides whether it matters: against the driver's
        // clamped 31 GiB buffer ceiling, at a context people actually use.
        let ctx = 8192u64;
        let wasted_gib = (dense * ctx) as f64 / (1u64 << 30) as f64;
        assert!(
            wasted_gib > 10.0,
            "the unread dense cache is {wasted_gib:.2} GiB at {ctx} context; it was 12.50 GiB when this was measured"
        );
    }
}
