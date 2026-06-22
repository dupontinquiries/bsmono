//! memplan.rs — SCRATCHWORK (2026-06-12) toward bigger-model support. Pure, no wgpu, fully testable.
//!
//! The loader today dequantizes every tensor to f32 and keeps the projections int8-resident, with no
//! up-front check of whether the model fits — so an oversized model just fails late (the "70B refused"
//! case). This module computes a model's parameter count and resident footprint from GGUF metadata
//! BEFORE loading, and turns the binary refuse into a graded plan: fits int8 (best, the current path),
//! fits only at q4 (needs q4 matmul kernels — GPU work, M2-gated), needs layer streaming from disk, or
//! genuinely too big — always with the byte figure so the UI can say *why*.
//!
//! Calibration anchor (real M2 log): SmolLM2-360M q4_k_m → int8 resident ≈ 0.56 GiB. The model below
//! (f32 embedding kept for the lookup + tied output, int8 layers) lands ~0.5 GiB, matching within the
//! noise of KV/scale overhead.

/// The shape numbers the planner needs (all from GGUF metadata).
#[derive(Clone, Copy, Debug)]
pub struct ModelShape {
    pub d: usize,            // embedding_length
    pub ffn: usize,          // feed_forward_length
    pub layers: usize,       // block_count
    pub vocab: usize,        // vocab size (or embed.len()/d)
    pub n_heads: usize,      // attention.head_count
    pub n_kv_heads: usize,   // attention.head_count_kv (GQA when < n_heads)
    pub tied_embed: bool,    // output projection shares the embedding (no separate output.weight)
}

impl ModelShape {
    pub fn head_dim(&self) -> usize { if self.n_heads == 0 { 0 } else { self.d / self.n_heads } }
    /// K/V projection width (GQA narrows this below d).
    pub fn kv_width(&self) -> usize { self.n_kv_heads * self.head_dim() }

    /// Parameters in the token-embedding matrix.
    pub fn embed_params(&self) -> u64 { (self.vocab as u64) * (self.d as u64) }

    /// Parameters in one transformer block: attn (q,k,v,o) + ffn (gate,up,down).
    pub fn layer_params(&self) -> u64 {
        let d = self.d as u64;
        let kv = self.kv_width() as u64;
        let ffn = self.ffn as u64;
        let attn = 2 * d * d + 2 * kv * d;     // wq + wo (d×d) + wk + wv (kv×d)
        let ffn_p = 3 * ffn * d;               // gate + up + down
        attn + ffn_p
    }

    /// Parameters in the output projection (0 when tied to the embedding).
    pub fn output_params(&self) -> u64 { if self.tied_embed { 0 } else { self.embed_params() } }

    pub fn total_params(&self) -> u64 {
        self.embed_params() + (self.layers as u64) * self.layer_params() + self.output_params()
    }
}

/// Residency format for the bulk projection weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency { F32, Int8, Q4 }
impl Residency {
    /// Bytes per weight (Int8 ≈ 1 + tiny per-row scale; Q4_K ≈ 4.5 bits = 0.5625 B).
    pub fn bytes_per_param(self) -> f64 {
        match self { Residency::F32 => 4.0, Residency::Int8 => 1.0, Residency::Q4 => 0.5625 }
    }
    pub fn label(self) -> &'static str {
        match self { Residency::F32 => "f32", Residency::Int8 => "int8", Residency::Q4 => "q4" }
    }
}

/// Resident weight bytes if the projection layers are stored as `fmt`. The embedding is kept f32 (it's
/// the lookup table, and the tied output reuses it); norms are f32 but negligible — included anyway.
pub fn resident_weight_bytes(s: &ModelShape, fmt: Residency) -> u64 {
    let embed = (s.embed_params() as f64) * 4.0; // f32 lookup table
    let layers = (s.layers as u64 * s.layer_params()) as f64 * fmt.bytes_per_param();
    let output = (s.output_params() as f64) * fmt.bytes_per_param(); // 0 when tied
    let norms = (s.layers as u64 * 2 * s.d as u64 + s.d as u64) as f64 * 4.0;
    (embed + layers + output + norms) as u64
}

/// KV-cache bytes for a context window: 2 (K,V) × layers × kv_width × ctx × dtype.
pub fn kv_cache_bytes(s: &ModelShape, ctx_len: usize, kv_dtype_bytes: usize) -> u64 {
    2 * s.layers as u64 * s.kv_width() as u64 * ctx_len as u64 * kv_dtype_bytes as u64
}

/// Transient peak during the CURRENT loader: it dequantizes one tensor to f32 at a time, so the spike
/// above resident is roughly the largest single tensor as f32 (usually the embedding/output).
pub fn peak_stage_bytes(s: &ModelShape) -> u64 {
    let embed = s.embed_params();
    let biggest_layer_tensor = (3 * s.ffn as u64 * s.d as u64).max(s.d as u64 * s.d as u64);
    embed.max(biggest_layer_tensor) * 4
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fit {
    /// Fits fully resident; `fmt` is the best (highest-quality) format that fits within budget.
    Resident { fmt: Residency, weight_bytes: u64, kv_bytes: u64 },
    /// Won't fit resident even at q4, but a single layer is small enough to stream from disk.
    Streaming { per_layer_bytes: u64, resident_overhead: u64 },
    /// Cannot fit even one layer + kv + overhead in budget.
    TooBig { min_needed: u64 },
}

/// Decide how (and whether) a model can run in `budget_bytes` with a `ctx_len` window. Prefers the
/// best-quality residency that fits (int8 over q4), falls back to streaming, then refuses with a number.
pub fn plan(s: &ModelShape, budget_bytes: u64, ctx_len: usize, kv_dtype_bytes: usize, overhead_bytes: u64) -> Fit {
    let kv = kv_cache_bytes(s, ctx_len, kv_dtype_bytes);
    for fmt in [Residency::Int8, Residency::Q4] {
        let w = resident_weight_bytes(s, fmt);
        if w + kv + overhead_bytes <= budget_bytes {
            return Fit::Resident { fmt, weight_bytes: w, kv_bytes: kv };
        }
    }
    // streaming: keep only a working set of layers resident; the rest stream from mmap. Estimate the
    // per-layer resident cost at q4 (the streamed copy) + the fixed overhead (embedding f32 + kv).
    let per_layer = (s.layer_params() as f64 * Residency::Q4.bytes_per_param()) as u64;
    let fixed = (s.embed_params() as f64 * 4.0) as u64 + kv + overhead_bytes;
    if fixed + per_layer * 2 <= budget_bytes {
        return Fit::Streaming { per_layer_bytes: per_layer, resident_overhead: fixed };
    }
    Fit::TooBig { min_needed: fixed + per_layer * 2 }
}

const GIB: f64 = (1u64 << 30) as f64;
pub fn gib(bytes: u64) -> f64 { bytes as f64 / GIB }

#[cfg(test)]
mod tests {
    use super::*;

    // SmolLM2-360M: d=960 ffn=2560 layers=32 heads=15/5 vocab=49152 tied.
    fn smollm2_360m() -> ModelShape { ModelShape { d: 960, ffn: 2560, layers: 32, vocab: 49152, n_heads: 15, n_kv_heads: 5, tied_embed: true } }
    // A 70B-class llama: d=8192 ffn=28672 layers=80 heads=64/8 vocab=128256 untied.
    fn llama_70b() -> ModelShape { ModelShape { d: 8192, ffn: 28672, layers: 80, vocab: 128256, n_heads: 64, n_kv_heads: 8, tied_embed: false } }
    // Llama-3.1-8B: d=4096 ffn=14336 layers=32 heads=32/8 vocab=128256 untied.
    fn llama_8b() -> ModelShape { ModelShape { d: 4096, ffn: 14336, layers: 32, vocab: 128256, n_heads: 32, n_kv_heads: 8, tied_embed: false } }

    #[test]
    fn param_counts_match_known_models() {
        let p = smollm2_360m().total_params();
        assert!((360_000_000..=365_000_000).contains(&p), "SmolLM2-360M ≈ 362M params, got {p}");
        let p8 = llama_8b().total_params();
        assert!((7_900_000_000..=8_200_000_000).contains(&p8), "Llama-8B ≈ 8.0B, got {p8}");
        let p70 = llama_70b().total_params();
        assert!((69_000_000_000..=71_500_000_000).contains(&p70), "Llama-70B ≈ 70B, got {p70}");
    }

    #[test]
    fn smollm2_int8_resident_matches_the_m2_log() {
        // M2 log: int8 resident ≈ 0.56 GiB. f32 embedding + int8 layers should land near there.
        let w = resident_weight_bytes(&smollm2_360m(), Residency::Int8);
        let g = gib(w);
        assert!((0.4..0.65).contains(&g), "int8 resident ≈ 0.5 GiB, got {g:.3} GiB");
    }

    #[test]
    fn small_model_fits_int8() {
        let budget = (8.0 * GIB) as u64;
        match plan(&smollm2_360m(), budget, 2560, 2, (300 * (1 << 20)) as u64) {
            Fit::Resident { fmt, .. } => assert_eq!(fmt, Residency::Int8, "small model takes the best (int8) path"),
            other => panic!("expected Resident int8, got {other:?}"),
        }
    }

    #[test]
    fn eight_b_fits_int8_on_the_m2() {
        // 8B int8 ≈ 8 GiB + kv; fits comfortably in the M2's ~39.5 GiB.
        let budget = (39_500u64) * (1 << 20);
        match plan(&llama_8b(), budget, 8192, 2, (400 * (1 << 20)) as u64) {
            Fit::Resident { fmt, weight_bytes, .. } => {
                assert_eq!(fmt, Residency::Int8);
                assert!((7.0..10.0).contains(&gib(weight_bytes)), "8B int8 ≈ 8 GiB, got {:.2}", gib(weight_bytes));
            }
            other => panic!("expected Resident int8, got {other:?}"),
        }
    }

    #[test]
    fn seventy_b_does_not_fit_int8_but_planner_is_graded() {
        // 70B int8 ≈ 65+ GiB ≫ 39.5 GiB → must NOT be Resident int8. Either q4-resident (≈39 GiB,
        // borderline) or streaming — never a silent fail.
        let budget = (39_500u64) * (1 << 20);
        let fit = plan(&llama_70b(), budget, 4096, 2, (500 * (1 << 20)) as u64);
        match fit {
            Fit::Resident { fmt, .. } => assert_ne!(fmt, Residency::Int8, "70B can't be int8-resident on 39.5 GiB"),
            Fit::Streaming { .. } => {} // acceptable graded outcome
            Fit::TooBig { min_needed } => assert!(gib(min_needed) > 1.0, "reports a real number"),
        }
        // and with a tiny budget it's honestly too big
        match plan(&llama_70b(), (4.0 * GIB) as u64, 4096, 2, (500 * (1 << 20)) as u64) {
            Fit::TooBig { min_needed } => assert!(gib(min_needed) > 4.0, "too big with a number, got {:.1} GiB", gib(min_needed)),
            Fit::Streaming { .. } => {} // streaming a 70B in 4 GiB is implausible but not asserted-against here
            other => panic!("70B in 4 GiB should not fit resident: {other:?}"),
        }
    }

    #[test]
    fn kv_cache_grows_with_context() {
        let s = llama_8b();
        let small = kv_cache_bytes(&s, 2048, 2);
        let big = kv_cache_bytes(&s, 32768, 2);
        assert_eq!(big, small * 16, "kv scales linearly with context");
    }
}
