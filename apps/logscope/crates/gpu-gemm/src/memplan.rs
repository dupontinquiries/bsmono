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
    /// The next-smaller resident format to fall back to when this one won't allocate on the GPU. int8 → q4,
    /// q4 → None (past q4, the answer is layer streaming, not a smaller weight format).
    pub fn downgrade(self) -> Option<Residency> {
        match self { Residency::F32 => Some(Residency::Int8), Residency::Int8 => Some(Residency::Q4), Residency::Q4 => None }
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

/// The resident formats that fit `budget_bytes`, best-quality first (int8, then q4) — the DOWNGRADE LADDER
/// the loader walks: try each in order, and if the GPU refuses the allocation (BO/VMA exhaustion the byte
/// budget didn't predict), drop to the next. Empty ⇒ nothing fits resident (stream or refuse).
pub fn fitting_residencies(s: &ModelShape, budget_bytes: u64, ctx_len: usize, kv_dtype_bytes: usize, overhead_bytes: u64) -> Vec<Residency> {
    let kv = kv_cache_bytes(s, ctx_len, kv_dtype_bytes);
    [Residency::Int8, Residency::Q4].into_iter()
        .filter(|&fmt| resident_weight_bytes(s, fmt) + kv + overhead_bytes <= budget_bytes)
        .collect()
}

/// A concrete layer-streaming plan: pin as many transformer blocks resident as the budget allows, stream the
/// rest from the mmap through a small rotating buffer, double-buffered so block i+1 prefetches while block i
/// computes. The fallback when a model won't fit even at q4 AND zero-copy import isn't available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamPlan {
    pub residency: Residency,          // format the layer weights are stored/streamed in (q4 = less bandwidth)
    pub total_layers: u32,
    pub pinned_layers: u32,            // permanently resident (never re-streamed)
    pub ring_slots: u32,               // rotating buffers for the streamed layers (≥2 for double-buffering)
    pub layer_bytes: u64,              // bytes for one block at `residency`
    pub streamed_bytes_per_token: u64, // reloaded from mmap every DECODE token (the throughput bottleneck)
    pub resident_bytes: u64,           // total GPU residency: fixed overhead + (pinned + ring) blocks
}
impl StreamPlan {
    pub fn streamed_layers(&self) -> u32 { self.total_layers - self.pinned_layers }
    /// Lower bound on decode ms/token from streaming bandwidth alone — every non-pinned block is reloaded
    /// each decode step, so `streamed_bytes / bandwidth` dominates. Feed it the GiB/s caps-probe measured.
    pub fn decode_ms_per_token(&self, gib_per_s: f64) -> f64 {
        if gib_per_s <= 0.0 { return f64::INFINITY; }
        (self.streamed_bytes_per_token as f64 / GIB) / gib_per_s * 1000.0
    }
}

/// Plan layer streaming for a model that won't fit resident. Pins as many blocks as the budget holds after
/// the fixed residency (f32 embedding + output + KV + transient staging), keeps a 2-slot ring for the rest.
/// `fmt` is the streamed weight format — q4 is recommended (half the bandwidth of int8). Returns None if not
/// even the fixed overhead plus a 2-block ring fits (genuinely too big → TooBig).
pub fn plan_streaming(s: &ModelShape, budget: u64, fmt: Residency, ctx_len: usize, kv_dtype_bytes: usize, transient_overhead: u64) -> Option<StreamPlan> {
    let layer_bytes = (s.layer_params() as f64 * fmt.bytes_per_param()) as u64;
    if layer_bytes == 0 { return None; }
    let kv = kv_cache_bytes(s, ctx_len, kv_dtype_bytes);
    let embed = (s.embed_params() as f64 * 4.0) as u64;                 // embedding kept f32
    let output = (s.output_params() as f64 * fmt.bytes_per_param()) as u64;
    let fixed = embed + output + kv + transient_overhead;
    let total = s.layers as u32;
    if fixed + 2 * layer_bytes > budget { return None; }                // can't hold even a 2-block ring
    let slots = ((budget - fixed) / layer_bytes) as u32;               // layer buffers that fit
    if slots >= total {
        // everything actually fits pinned (caller normally uses Resident; report a no-streaming plan)
        return Some(StreamPlan { residency: fmt, total_layers: total, pinned_layers: total, ring_slots: 0,
            layer_bytes, streamed_bytes_per_token: 0, resident_bytes: fixed + total as u64 * layer_bytes });
    }
    let ring = 2u32;                                                    // double-buffer
    let pinned = slots - ring;                                          // slots ≥ 2 here (guarded above)
    let streamed = total - pinned;
    Some(StreamPlan {
        residency: fmt, total_layers: total, pinned_layers: pinned, ring_slots: ring, layer_bytes,
        streamed_bytes_per_token: streamed as u64 * layer_bytes,
        resident_bytes: fixed + (pinned as u64 + ring as u64) * layer_bytes,
    })
}

/// Which rotating buffer a streamed block uses (consecutive streamed blocks alternate so i+1 can prefetch
/// while i computes). `streamed_index` counts only the non-pinned blocks in traversal order.
pub fn ring_slot(streamed_index: u32, ring_slots: u32) -> u32 { streamed_index % ring_slots.max(1) }

/// A GPU buffer-allocation budget, which on unified-memory drivers (Asahi/Honeykrisp) is far below system
/// RAM: the driver caps total buffer-object address space, so a plan sized to RAM greenlights an int8
/// residency the GPU then can't allocate (the "70B → Failed to allocate BO VMA" crash). Takes the smaller
/// of the RAM budget and this cap. `LOGSCOPE_MAX_GPU_GB` overrides; otherwise a conservative fraction of RAM
/// that the loader's OOM-downgrade ladder tightens further at runtime.
/// `conservative_vma` = the Asahi/Honeykrisp case (Vulkan on an Apple GPU): that driver's BO-VMA ceiling
/// is far below what it advertises — a 40.3 GiB q4-70B and a ~31 GiB MoE-int8 both passed a 48 GiB cap and
/// then died in BO VMA allocation mid-upload — so it gets a 24 GiB cap that keeps every observed-working
/// load in and pushes oversized ones to q4/refusal up front. This is a PLATFORM fact, not a policy: Metal
/// (macOS) has no such ceiling and gets the standard 48 GiB cap so bigger residencies (q4-70B) stay
/// reachable there. LOGSCOPE_MAX_GPU_GB overrides everything.
pub fn gpu_buffer_budget(ram_budget: u64, conservative_vma: bool) -> u64 {
    if let Ok(s) = std::env::var("LOGSCOPE_MAX_GPU_GB") {
        if let Ok(gb) = s.trim().parse::<f64>() { return (gb * GIB) as u64; }
    }
    let cap = if conservative_vma { 24.0 } else { 48.0 };
    (ram_budget / 2).min((cap * GIB) as u64)
}

const GIB: f64 = (1u64 << 30) as f64;
pub fn gib(bytes: u64) -> f64 { bytes as f64 / GIB }

/// Turn an EMPIRICAL allocation ceiling (the total STORAGE bytes the GPU accepted before it OOM'd, found by
/// caps-probe) into a recommended resident-weight budget: leave 15% headroom for the KV cache + transient
/// staging buffers that also come out of the same GPU budget. This is the number to feed LOGSCOPE_MAX_GPU_GB
/// once the probe has measured the real ceiling — replacing the static 48 GiB guess with the box's truth.
pub fn recommend_gpu_budget(ceiling_bytes: u64) -> u64 {
    (ceiling_bytes as f64 * 0.85) as u64
}

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

    #[test]
    fn residency_downgrade_ladder() {
        assert_eq!(Residency::Int8.downgrade(), Some(Residency::Q4));
        assert_eq!(Residency::Q4.downgrade(), None); // past q4 → streaming, not a smaller weight
        assert_eq!(Residency::F32.downgrade(), Some(Residency::Int8));
    }

    #[test]
    fn fitting_residencies_is_the_ordered_ladder() {
        // a budget where int8 fits → both int8 and q4 are options, int8 first
        let big = (39_500u64) * (1 << 20);
        assert_eq!(fitting_residencies(&llama_8b(), big, 8192, 2, (400 * (1 << 20)) as u64), vec![Residency::Int8, Residency::Q4]);
        // a 70B on 39.5 GiB: int8 doesn't fit, q4 might — int8 must not appear
        let opts = fitting_residencies(&llama_70b(), big, 4096, 2, (500 * (1 << 20)) as u64);
        assert!(!opts.contains(&Residency::Int8), "70B int8 doesn't fit 39.5 GiB");
        // a tiny budget: nothing fits resident
        assert!(fitting_residencies(&llama_70b(), (4.0 * GIB) as u64, 4096, 2, 0).is_empty());
    }

    #[test]
    fn gpu_budget_is_capped_below_ram() {
        std::env::remove_var("LOGSCOPE_MAX_GPU_GB");
        // 170 GiB of RAM (M2 Ultra) → GPU budget capped at 48 GiB, so a 70B-int8 (~70 GiB) is rejected up
        // front instead of crashing the driver mid-upload.
        let ram = (170.0 * GIB) as u64;
        let gpu = gpu_buffer_budget(ram, false);
        assert!(gpu <= (48.0 * GIB) as u64 && gpu < ram, "GPU budget {} capped below RAM {}", gib(gpu), gib(ram));
        // the Asahi VMA clamp is PLATFORM-scoped: Vulkan-on-Apple gets 24 GiB, Metal keeps 48
        assert_eq!(gpu_buffer_budget(ram, true), (24.0 * GIB) as u64);
        assert_eq!(gpu_buffer_budget(ram, false), (48.0 * GIB) as u64);
        assert!(fitting_residencies(&llama_70b(), gpu, 4096, 2, (500 * (1 << 20)) as u64).iter().all(|&r| r != Residency::Int8),
                "under the GPU budget the 70B won't attempt int8");
        // explicit override wins
        std::env::set_var("LOGSCOPE_MAX_GPU_GB", "96");
        assert_eq!(gpu_buffer_budget(ram, true), (96.0 * GIB) as u64);
        std::env::remove_var("LOGSCOPE_MAX_GPU_GB");
    }

    #[test]
    fn recommended_budget_leaves_headroom() {
        // if the probe found the GPU accepts 60 GiB of buffers, budget the weights at ~51 GiB (15% for
        // KV + staging), and a 70B then fits int8 (~70? no) — check it's below the ceiling and non-zero.
        let ceiling = (60.0 * GIB) as u64;
        let b = recommend_gpu_budget(ceiling);
        assert!(b < ceiling && b > (50.0 * GIB) as u64, "budget {:.1} leaves headroom under {:.1}", gib(b), gib(ceiling));
        // and it feeds straight into the ladder: at a 60 GiB ceiling the 70B still can't do int8 (~70) → q4
        assert!(fitting_residencies(&llama_70b(), b, 4096, 2, (500 * (1 << 20)) as u64).iter().all(|&r| r != Residency::Int8));
    }

    #[test]
    fn streaming_pins_what_fits_and_streams_the_rest() {
        let s = llama_70b();
        let budget = (24.0 * GIB) as u64;
        let plan = plan_streaming(&s, budget, Residency::Q4, 4096, 2, (512 * (1 << 20)) as u64).expect("70B streams into 24 GiB");
        assert_eq!(plan.total_layers, s.layers as u32);
        assert!(plan.pinned_layers < plan.total_layers, "not everything fits pinned");
        assert!(plan.streamed_layers() > 0, "some layers stream");
        assert_eq!(plan.pinned_layers + plan.streamed_layers(), plan.total_layers);
        assert_eq!(plan.ring_slots, 2, "double-buffered");
        assert!(plan.resident_bytes <= budget, "within budget: {:.1} ≤ {:.1} GiB", gib(plan.resident_bytes), gib(budget));
        let ms = plan.decode_ms_per_token(100.0);
        assert!(ms > 0.0 && ms.is_finite(), "bandwidth-bound decode: {ms:.1} ms/tok");
        // fatter budget pins more → streams fewer bytes → faster
        let plan2 = plan_streaming(&s, (36.0 * GIB) as u64, Residency::Q4, 4096, 2, (512 * (1 << 20)) as u64).unwrap();
        assert!(plan2.streamed_bytes_per_token < plan.streamed_bytes_per_token, "more budget streams less");
        assert!(plan2.decode_ms_per_token(100.0) < ms);
    }

    #[test]
    fn streaming_refuses_when_even_the_ring_wont_fit() {
        assert!(plan_streaming(&llama_70b(), (4.0 * GIB) as u64, Residency::Q4, 4096, 2, 0).is_none());
    }

    #[test]
    fn ring_slot_alternates_for_double_buffering() {
        assert_eq!(ring_slot(0, 2), 0);
        assert_eq!(ring_slot(1, 2), 1);
        assert_eq!(ring_slot(2, 2), 0);
        assert_eq!(ring_slot(5, 1), 0);
    }
}
