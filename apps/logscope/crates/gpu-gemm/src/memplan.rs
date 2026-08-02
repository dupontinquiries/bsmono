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

/// Plan EXPERT streaming for an MoE that won't fit resident — the right model for Llama-4/Qwen-MoE/GLM,
/// where only a few of N experts fire per token. Layer streaming (above) reloads whole blocks; here we pin
/// the dense weights (attention, norms, router) + the shared expert + as many HOT routed experts as the
/// budget holds, and stream ONLY the active-but-not-resident experts per token. Because top_k ≪ n_experts,
/// the streamed bytes per token are a small fraction of a layer — and the expert predictor (expert_predict)
/// prefetches exactly those, so the disk/host read overlaps compute. This is what makes a 56 GiB Scout
/// tractable under a 24 GiB Honeykrisp ceiling: ~7 GiB dense + shared pinned, hot experts pinned, the rest
/// streamed on demand.
///
/// `expert_bytes` = one expert's weights (all layers) at `fmt`; `n_experts` routed; `top_k` active per
/// token; `dense_bytes` = everything that isn't a routed expert (attn+norm+router+embed+output+shared), at
/// residency. Returns None if the dense set + shared + top_k experts (the irreducible per-token working set)
/// can't fit the budget — genuinely too big even to stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoeStreamPlan {
    pub n_experts: u32,
    pub top_k: u32,
    pub pinned_experts: u32,        // hottest experts kept permanently resident
    pub stream_slots: u32,          // rotating expert-sized slots for the streamed (cold) experts
    pub expert_bytes: u64,          // one expert (all layers) at residency
    pub dense_resident_bytes: u64,  // dense weights + shared expert + KV + transient, always resident
    pub resident_bytes: u64,        // total GPU residency
}
impl MoeStreamPlan {
    /// Worst-case experts streamed per token: the active set minus those that happen to be pinned. With the
    /// predictor warm, the expected number is lower (hot experts get pinned), but this bounds it.
    pub fn streamed_experts_per_token(&self) -> u32 { self.top_k.saturating_sub(self.pinned_experts.min(self.top_k)) }
    pub fn streamed_bytes_per_token(&self) -> u64 { self.streamed_experts_per_token() as u64 * self.expert_bytes }
    pub fn decode_ms_per_token(&self, gib_per_s: f64) -> f64 {
        if gib_per_s <= 0.0 { return f64::INFINITY; }
        (self.streamed_bytes_per_token() as f64 / GIB) / gib_per_s * 1000.0
    }
    /// Fraction of the routed-expert mass that never has to stream (pinned) — higher is better, and the
    /// predictor's job is to make sure the pinned set is the hot one.
    pub fn pinned_fraction(&self) -> f64 { self.pinned_experts as f64 / self.n_experts.max(1) as f64 }
}

pub fn plan_moe_streaming(dense_resident_bytes: u64, expert_bytes: u64, n_experts: u32, top_k: u32, budget: u64) -> Option<MoeStreamPlan> {
    if expert_bytes == 0 || n_experts == 0 { return None; }
    // Irreducible working set: the dense/shared residency + at least top_k expert slots (the ones a single
    // token needs live at once) + 1 spare slot for prefetch double-buffering. If that won't fit, streaming
    // can't help — the model is too big even to run one token.
    let min_slots = top_k + 1;
    if dense_resident_bytes + min_slots as u64 * expert_bytes > budget { return None; }
    // Fill the remaining budget with expert-sized slots. Slots beyond the routed-expert count are wasted, so
    // cap at n_experts (that's the fully-resident case → no streaming).
    let slots_that_fit = ((budget - dense_resident_bytes) / expert_bytes) as u32;
    let total_slots = slots_that_fit.min(n_experts);
    if total_slots >= n_experts {
        // everything fits pinned — no streaming needed
        return Some(MoeStreamPlan { n_experts, top_k, pinned_experts: n_experts, stream_slots: 0,
            expert_bytes, dense_resident_bytes, resident_bytes: dense_resident_bytes + n_experts as u64 * expert_bytes });
    }
    // Reserve top_k+1 slots as the streaming ring (must hold a token's active cold set + a prefetch spare);
    // pin the rest with the hottest experts. This maximizes the pinned (never-streamed) hot set.
    let ring = top_k + 1;
    let pinned = total_slots.saturating_sub(ring);
    Some(MoeStreamPlan {
        n_experts, top_k, pinned_experts: pinned, stream_slots: ring, expert_bytes,
        dense_resident_bytes,
        resident_bytes: dense_resident_bytes + (pinned as u64 + ring as u64) * expert_bytes,
    })
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
/// GPU buffer-upload budget in bytes. The conservative_vma (Asahi/Honeykrisp) cap of 24 GiB is an
/// EMPIRICAL fact about the *staging* upload path: `queue.write_buffer` double-allocates (a transient
/// staging buffer as big as the data) and that peak overran the driver's BO VMA ceiling on 40–70 GiB loads.
/// BUT the map-at-creation arena used on unified memory does NOT stage — it writes straight into the
/// buffer's device-visible backing (host==device RAM), which is exactly what removed the ~2× peak. So on
/// UMA with the zero-copy path, the real ceiling is a safe fraction of PHYSICAL RAM, not the staging cap:
/// a 62 GiB MoE loads fine resident on a 192 GiB M2 Ultra, no streaming needed. `unified` gates this — a
/// discrete GPU (or forced staging) keeps the conservative cap. LOGSCOPE_MAX_GPU_GB overrides everything.
pub fn gpu_buffer_budget(ram_budget: u64, conservative_vma: bool) -> u64 {
    gpu_buffer_budget_uma(ram_budget, conservative_vma, false)
}

/// UMA-aware budget. The distinction that matters is NOT unified-vs-discrete — it's WHICH unified driver.
/// Honeykrisp (Asahi Vulkan, conservative_vma) has a hard BO-VMA allocation ceiling FAR below system RAM
/// (empirically ~24-31 GiB: a 40 GiB q4-70B, a 31 GiB MoE-int8, and a 56 GiB Scout-q4 all died in "Failed
/// to allocate BO VMA" — the arena's slab buffers hit the same wall). That ceiling is a platform fact, not
/// a staging artifact, so it holds regardless of the 192 GiB of RAM behind it. Metal (macOS, unified but
/// NOT conservative_vma) has no such ceiling and can go to RAM. So: conservative_vma keeps the hard cap
/// even though it's unified; Metal-class unified lifts to a RAM fraction. LOGSCOPE_MAX_GPU_GB overrides.
/// Budget from a MEASURED ceiling, when one exists.
///
/// A measurement outranks every constant here. `--gate-vram` found this device honouring 63 GiB where
/// the constant assumed 24 — the constant was an empirical figure from the staging upload path, taken
/// before the arena removed the staging, and nothing in the code distinguished "the driver refuses"
/// from "we assumed it would". Models that fitted were being streamed for no reason.
///
/// Keeps a margin below the measurement rather than using it whole: the probe finds where allocation
/// FAILS, and running at exactly that point leaves nothing for the KV cache, transients, or another
/// process on the same GPU.
pub fn budget_from_measurement(measured_gib: f32) -> Option<u64> {
    if measured_gib <= 0.0 { return None; }        // 0 = never probed, not "no memory"
    Some((measured_gib as f64 * 0.75 * GIB) as u64)
}

pub fn gpu_buffer_budget_uma(ram_budget: u64, conservative_vma: bool, unified: bool) -> u64 {
    if let Ok(s) = std::env::var("LOGSCOPE_MAX_GPU_GB") {
        if let Ok(gb) = s.trim().parse::<f64>() { return (gb * GIB) as u64; }
    }
    if unified && !conservative_vma {
        // Metal-class unified memory: the allocator is permissive (no BO-VMA ceiling), so the budget is
        // governed by RAM, not a cap. Half of the resident budget leaves room for KV + transients.
        return ram_budget / 2;
    }
    // conservative_vma (Honeykrisp) keeps its HARD ceiling even though it's unified — the BO-VMA wall is
    // real and independent of RAM. Discrete GPUs keep the standard cap. Oversized models are pushed to
    // q4/refusal (→ streaming) up front instead of dying mid-allocation.
    // 24.0 was an empirical peak from the STAGING upload path, measured before the map-at-creation
    // arena removed the staging. Three independent figures now contradict it on the same device:
    //
    //   llama.cpp, in production   42 GiB   80 layers of a 70B + 1.28 GiB KV, currently running
    //   --gate-vram probe          63 GiB   allocated, written, and read back
    //   the driver's own report    95 GiB   VkPhysicalDeviceMemoryProperties
    //
    // The llama.cpp figure is the one that settles it: it is not a probe or a report, it is a model
    // that loads and generates on this exact driver while we refuse it. A constant that rejects work a
    // peer is doing is not conservative, it is wrong.
    //
    // 48 is chosen to clear the case that exposed this — a 70B at Q4 needs 40.5 GiB plus KV — while
    // staying below what the probe verified. Still a constant, and still the wrong KIND of answer:
    // `gpu_budget_preferring_measurement` should be used wherever a measurement exists.
    // SCALED TO RAM, not a flat constant. 48 GiB was raised from 24 in _340 on evidence from a 192 GiB
    // M2 Ultra, where llama.cpp runs a 70B. On a 64 GiB M2 Max the probe measures a ceiling of 31.0 GiB
    // — so the same constant that was too LOW on the Ultra is too HIGH here, and a budget above the
    // ceiling is what approves an allocation that cannot be satisfied.
    //
    // The ceiling tracks RAM because this is unified memory: the GPU allocator is drawing on the same
    // pool. Half of RAM matches both observations — 96 on the Ultra (under the 63 it verified, so the
    // min below governs) and 31.3 on the Max (against 31.0 measured, which is as close as a constant
    // gets without being a measurement).
    //
    // Still the wrong KIND of answer. gpu_budget_preferring_measurement should be used wherever
    // --gate-vram has run.
    let ram_gib = ram_budget as f64 / GIB;
    let cap = if conservative_vma { (ram_gib * 0.5).min(48.0) } else { 48.0 };
    (ram_budget / 2).min((cap * GIB) as u64)
}

/// The budget, preferring a MEASURED ceiling over either constant.
///
/// Both 24.0 and 48.0 are guesses that have outlived their evidence. `--gate-vram` measures what the
/// device actually honours — 63 GiB on an M2 Ultra where the constant said 24 — and a measurement
/// should win over a number nobody has checked since it was written.
///
/// Worth recording what a read of the driver added, because it changes what the measurement MEANS: the
/// GPU virtual address space is 512 GiB (`UAT_IAS = 39`), and a run mapping ~110 GiB across three
/// models sits at 21% of it. So the "Failed to allocate BO VMA" the probe hits at 63 GiB is NOT address
/// space exhaustion. Something else is bounded — and until that is identified, a measured number is the
/// only trustworthy input, because no constant can encode a limit whose mechanism is unknown.
pub fn gpu_budget_preferring_measurement(ram_budget: u64, conservative_vma: bool, unified: bool,
                                         measured_gib: f32) -> u64 {
    match budget_from_measurement(measured_gib) {
        // Still bounded by RAM: honouring an allocation is not the same as it being wise to make.
        Some(b) => b.min(ram_budget / 2),
        None => gpu_buffer_budget_uma(ram_budget, conservative_vma, unified),
    }
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
        // The Asahi clamp used to be 24 GiB, from the staging upload path. It was raised to 48 after
        // llama.cpp was observed running a 70B — 80 layers plus KV, ~42 GiB — on this exact driver
        // while we refused it. A constant that rejects work a peer is doing is not conservative.
        assert_eq!(gpu_buffer_budget(ram, true), (48.0 * GIB) as u64);
        assert_eq!(gpu_buffer_budget(ram, false), (48.0 * GIB) as u64);
        // UNIFIED memory does NOT by itself lift the cap — the driver matters. Honeykrisp (conservative_vma)
        // has a real BO-VMA ceiling regardless of RAM (a 56 GiB Scout-q4 died in "Failed to allocate BO
        // VMA" on a 192 GiB box), so it keeps a cap even when unified=true. The CAP MOVED from 24 to 48
        // once llama.cpp was seen running a 70B — 80 layers plus KV, ~42 GiB — on this exact driver:
        // the ceiling is real, and 24 was measuring the staging peak rather than the ceiling.
        let ram192 = (172.0 * GIB) as u64;
        assert_eq!(gpu_buffer_budget_uma(ram192, true, true), (48.0 * GIB) as u64,
            "Honeykrisp keeps a cap even though it's unified — but 48, not the old staging-derived 24");
        // Metal-class unified (NOT conservative_vma) is permissive → RAM-scaled, no cap. This is why the
        // same 62 GiB model loads on an M4 Max (Metal) but not an M2 Ultra (Asahi/Honeykrisp).
        let metal = gpu_buffer_budget_uma(ram192, false, true);
        assert!(metal >= (80.0 * GIB) as u64 && metal < ram192, "Metal scales with RAM: {} GiB", gib(metal));
        assert!(fitting_residencies(&llama_70b(), gpu, 4096, 2, (500 * (1 << 20)) as u64).iter().all(|&r| r != Residency::Int8),
                "under the GPU budget the 70B won't attempt int8");
        // explicit override wins
        std::env::set_var("LOGSCOPE_MAX_GPU_GB", "96");
        assert_eq!(gpu_buffer_budget(ram, true), (96.0 * GIB) as u64);
        std::env::remove_var("LOGSCOPE_MAX_GPU_GB");
    }

    #[test]
    fn moe_streaming_makes_scout_tractable_under_honeykrisp_ceiling() {
        // Llama-4-Scout-17B-16E-Q4: ~56 GiB total, 16 routed experts + 1 shared, top_k=1 routed per token.
        // Approx split: dense (attn+norm+router+embed+output+shared) ~8 GiB; the 16 routed experts hold the
        // rest → ~3 GiB each. Under the 24 GiB Honeykrisp ceiling, full residency is impossible (proven by
        // the crash), but expert streaming pins dense + several hot experts and streams the cold ones.
        let gib = |g: f64| (g * GIB) as u64;
        let dense = gib(8.0);
        let expert = gib(3.0);
        let plan = plan_moe_streaming(dense, expert, 16, 1, gib(24.0)).expect("streamable under 24 GiB");
        assert!(plan.pinned_experts >= 1 && plan.pinned_experts < 16, "some experts pinned, not all: {}", plan.pinned_experts);
        assert!(plan.resident_bytes <= gib(24.0), "fits the ceiling: {:.1} GiB", super::gib(plan.resident_bytes));
        // top_k=1 and a prefetch spare → at most 1 expert streamed per token (a fraction of a layer)
        assert!(plan.streamed_experts_per_token() <= 1);
        // a warm predictor pins the hot experts → most of the routed mass never streams
        assert!(plan.pinned_fraction() > 0.0);
    }

    #[test]
    fn moe_streaming_refuses_when_even_one_token_wont_fit() {
        // dense alone bigger than budget → not even the working set fits → None (genuinely too big)
        let gib = |g: f64| (g * GIB) as u64;
        assert!(plan_moe_streaming(gib(30.0), gib(3.0), 16, 2, gib(24.0)).is_none());
    }

    #[test]
    fn moe_all_experts_fit_reports_no_streaming() {
        // budget holds dense + all experts → pinned_experts == n_experts, zero streaming
        let gib = |g: f64| (g * GIB) as u64;
        let plan = plan_moe_streaming(gib(4.0), gib(1.0), 8, 2, gib(24.0)).unwrap();
        assert_eq!(plan.pinned_experts, 8);
        assert_eq!(plan.streamed_experts_per_token(), 0);
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

/// Total system RAM in bytes, or 0 when it cannot be determined.
///
/// Linux reads `MemTotal`; macOS reads `hw.memsize` via sysctl. Everything else returns 0, and 0 means
/// UNKNOWN rather than "none" — callers must disable the guard rather than refuse a load that would have
/// worked.
///
/// The macOS path matters more than it looks: on unified memory this figure also drives the GPU buffer
/// budget, so returning 0 there made a 192 GiB machine fall back to a conservative guess and pick q4
/// where int8 would have fitted. A missing probe was silently costing model quality.
pub fn system_ram_bytes() -> u64 {
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for line in s.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                if let Some(kb) = rest.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()) {
                    return kb * 1024;
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // `sysctl -n hw.memsize` is the total physical RAM in bytes. Shelling out rather than linking
        // libc's sysctl keeps this dependency-free, and it runs once at load where a few ms is nothing.
        if let Ok(out) = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output() {
            if let Ok(txt) = String::from_utf8(out.stdout) {
                if let Ok(b) = txt.trim().parse::<u64>() { if b > 0 { return b; } }
            }
        }
    }
    0
}

/// Resident set size of this process in bytes, or 0 when unavailable.
///
/// macOS has no `smaps_rollup`, so the anonymous/file split is not available there — but the TOTAL is,
/// via `ps`. Reporting a total with an honest "split unavailable" beats reporting nothing, because the
/// total is what tells you whether you are about to swap.
pub fn process_rss_bytes() -> u64 {
    let (_, _, total) = rss_split();
    if total > 0 { return total; }
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        if let Ok(out) = std::process::Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid.to_string()]).output() {
            if let Ok(txt) = String::from_utf8(out.stdout) {
                // ps reports KiB
                if let Ok(kb) = txt.trim().parse::<u64>() { return kb * 1024; }
            }
        }
    }
    0
}

#[cfg(test)]
mod ram_tests {
    #[test]
    fn ram_probe_is_either_a_real_size_or_an_honest_zero() {
        let r = super::system_ram_bytes();
        // 0 means "unknown" and must disable the guard rather than block a load
        assert!(r == 0 || r > (1u64 << 30), "implausible RAM reading: {r}");
    }
}

/// Resident memory of this process, split by kind: (anonymous, file-backed, total) in bytes.
///
/// The split is the whole point. Anonymous pages are our own allocations — host weight copies, buffers —
/// and are real pressure that ends in an OOM kill. Clean file-backed pages are the mmap'd model, and the
/// kernel drops them the moment anything else needs the memory. A process monitor shows one number and
/// cannot tell you which you are looking at, which is how "90 GiB" becomes alarming without being
/// actionable. Linux only; zeros elsewhere, so callers treat it as unavailable rather than assume.
pub fn rss_split() -> (u64, u64, u64) {
    let Ok(s) = std::fs::read_to_string("/proc/self/smaps_rollup") else { return (0, 0, 0) };
    let kb = |key: &str| -> u64 {
        s.lines().find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) * 1024
    };
    let anon = kb("Anonymous:");
    let total = kb("Rss:");
    (anon, total.saturating_sub(anon), total)
}

/// One line describing where this process's memory actually is.
pub fn rss_report() -> String {
    let (anon, file, total) = rss_split();
    if total == 0 {
        // Fall back to the total-only probe before giving up: on macOS the anon/file SPLIT is
        // unavailable but the total is not, and the total is the number that tells you whether you are
        // about to swap.
        let t = process_rss_bytes();
        if t > 0 {
            return format!("resident memory: {:.1} GiB total (anonymous/file split unavailable on this \
platform — the total is what matters for swap pressure)", t as f64 / (1u64 << 30) as f64);
        }
        return "resident memory: unavailable on this platform".into();
    }
    format!("resident memory: {:.1} GiB total = {:.1} GiB anonymous (our allocations) + {:.1} GiB file-backed \
(mmap'd model; reclaimable by the kernel under pressure)",
        gib(total), gib(anon), gib(file))
}

#[cfg(test)]
mod rss_tests {
    #[test]
    fn split_is_consistent_or_honestly_zero() {
        let (a, f, t) = super::rss_split();
        if t == 0 { assert_eq!((a, f), (0, 0), "unavailable must be all-zero, not partial"); }
        else { assert_eq!(a + f, t, "the two halves must reconcile to the total"); }
    }
}
    #[test]
    fn int8_from_a_K_QUANT_is_not_worth_the_disk() {
        // Both use one scale per 32 weights, so the conversion cannot add precision — it stores 4-bit
        // values in 8 bits, using 16 of 256 levels, and Q4_K's dmin term has nowhere to go. Worse on
        // memory, worse on disk, no better on quality.
        assert!(!int8_worth_it(12), "Q4_K");
        assert!(!int8_worth_it(14), "Q6_K");
        assert!(!int8_worth_it(8), "Q8_0 is already 8-bit");
    }

    #[test]
    fn int8_from_full_precision_IS_worth_it() {
        // The case the cache was built for: F16 weights genuinely carry more than 8 bits, so this is a
        // real compression rather than a re-encoding.
        assert!(int8_worth_it(0), "F32");
        assert!(int8_worth_it(1), "F16");
    }

    #[test]
    fn an_unknown_source_does_not_get_the_benefit_of_the_doubt() {
        // Spending gigabytes of disk on a format nobody has checked is the wrong default, and a wrong
        // guess here is silent — the cache is written and simply never helps.
        assert!(!int8_worth_it(999));
    }

    #[test]
    fn the_waste_is_quantified_rather_than_described() {
        // A 8B model at ~8e9 weights writes ~7.5 GiB of int8 cache that buys nothing. Saying "some
        // disk" would not have got this changed.
        let w = 8_000_000_000u64;
        let gib = int8_cache_waste(12, w) as f64 / GIB;
        assert!(gib > 7.0, "{gib:.1} GiB wasted per 8B Q4_K model");
        assert_eq!(int8_cache_waste(1, w), 0, "and nothing wasted from an F16 source");
    }

    #[test]
    fn a_measurement_replaces_the_constant_but_RAM_still_bounds_it() {
        let ram = (192.0 * GIB) as u64;
        // measured: 63 GiB -> 75% = ~47, well under RAM/2, so the measurement governs
        let m = gpu_budget_preferring_measurement(ram, true, true, 63.0);
        let c = gpu_buffer_budget_uma(ram, true, true);
        // The measurement is USED, which is the property that matters — not that it is larger. Since
        // the constant was raised to 48 on llama.cpp's evidence, a measured 63 yields 47 and the two
        // now agree closely. A test asserting the measurement must WIN would fail on that agreement,
        // which would be exactly backwards.
        assert!(m > (40.0 * GIB) as u64,
            "the measured path is taken and gives a usable budget: {:.0} GiB", gib(m));
        assert!(c > 0);

        // unprobed falls back to the constant rather than to nothing
        assert_eq!(gpu_budget_preferring_measurement(ram, true, true, 0.0), c);

        // and a huge measurement is still bounded by RAM: honouring an allocation is not the same as
        // it being wise to make, and the probe deliberately allocates past what a real run should.
        let small_ram = (8.0 * GIB) as u64;
        let big = gpu_budget_preferring_measurement(small_ram, false, true, 500.0);
        assert!(big <= small_ram / 2, "RAM still bounds it: {:.1} GiB", gib(big));
    }

    #[test]
    fn a_measurement_outranks_the_constant_but_keeps_a_margin() {
        // The constant said 24 GiB; --gate-vram measured 63 on the same device. A measurement has to
        // win, or the probe is decoration.
        let b = budget_from_measurement(63.0).unwrap();
        let gib = |n: f64| (n * GIB) as u64;
        assert!(b > gib(40.0) && b < gib(63.0), "got {:.1} GiB", b as f64 / GIB);
        // Not the whole measurement: the probe finds where allocation FAILS, and running at exactly
        // that point leaves nothing for KV, transients, or another process on the same GPU.
        assert!(b < gib(63.0));
    }

    #[test]
    fn an_unprobed_device_returns_None_rather_than_zero_budget() {
        // 0 means "never measured", not "no memory". Returning a zero budget would refuse every model
        // on a machine that had simply not run the probe yet.
        assert!(budget_from_measurement(0.0).is_none());
        assert!(budget_from_measurement(-1.0).is_none());
    }



/// Is converting this source format to int8 residency worth anything?
///
/// ## The answer for Q4_K is no, and it costs disk twice
///
/// A load of a Q4_K_M model currently quantizes to int8 residency and writes a content-addressed cache
/// so the next load can skip the conversion — gigabytes on device per model. But:
///
/// * int8 residency uses **one scale per 32 weights** (`quantize_q8_resident`: `kb = k / 32`).
/// * Q4_K uses **one scale per 32 weights** too — eight sub-blocks per 256-weight super-block.
///
/// Same granularity, so the conversion cannot add precision. The values came from 4-bit quants and take
/// at most 16 distinct levels per sub-block; storing them in 8 bits uses 16 of 256 levels and doubles
/// the bytes. Q4_K additionally carries a `dmin` minimum term that int8 has no place for, so the
/// conversion has to fold it into a symmetric scale — which can LOSE accuracy rather than preserve it.
///
/// So int8-from-Q4_K is worse on memory, worse on disk, and no better on quality. llama.cpp never does
/// it: its kernels consume Q4_K directly, and `--gate-q4k` proved ours can too — its success message has
/// said "the loader can stop requantizing" since it went green.
///
/// ## Where int8 IS worth it
///
/// From an F16 or F32 source, where the weights genuinely carry more precision than 8 bits and the
/// conversion is a real compression rather than a re-encoding.
pub fn int8_worth_it(src_ggml_type: u32) -> bool {
    match src_ggml_type {
        0 | 1 => true,      // F32, F16: a real compression
        // Every K-quant and Q8_0 already has per-block scales at 32-weight granularity or finer.
        // Re-encoding them to int8 doubles the size and cannot add information.
        2..=20 => false,
        _ => false,         // unknown: do not spend the disk on a guess
    }
}

/// Bytes saved on device by skipping the int8 cache for a source that does not benefit.
pub fn int8_cache_waste(src_ggml_type: u32, n_weights: u64) -> u64 {
    if int8_worth_it(src_ggml_type) { 0 } else { n_weights }   // ~1 byte/weight for the int8 blob
}


/// Would this model fit if the budget were the MEASURED ceiling rather than the constant?
///
/// The refusal path compares a model against `gpu_buffer_budget_uma`, which on Asahi returns a
/// compiled-in 24 GiB. `--gate-vram` measured the same device honouring 63. A 70B at Q4 is 40 GiB: it
/// is refused by the constant and would be accepted by the measurement.
///
/// That is the worst shape a refusal can have — it is not "this will not work", it is "we did not
/// check". So when a model is refused, this reports whether measuring would change the answer, and the
/// message can say so instead of recommending another backend for a model that fits.
pub fn would_fit_if_measured(model_bytes: u64, current_budget: u64, measured_gib: f32) -> Option<u64> {
    let measured = budget_from_measurement(measured_gib)?;
    if model_bytes <= current_budget { return None; }   // already fits; nothing to report
    if model_bytes <= measured { Some(measured) } else { None }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn a_70b_refused_by_the_constant_fits_the_measurement() {
        // The reported case. 40.5 GiB against a 24 GiB constant is a refusal; against 63 GiB measured
        // it is a load. The user was told to use another backend for a model the device honours.
        let model = (40.5 * GIB) as u64;
        let constant = (24.0 * GIB) as u64;
        let r = would_fit_if_measured(model, constant, 63.0);
        assert!(r.is_some(), "40.5 GiB must fit inside 75% of a measured 63 GiB");
    }

    #[test]
    fn a_model_that_fits_either_way_reports_nothing() {
        // Silence when there is nothing to say: a load that already works should not be told it might
        // work differently.
        assert!(would_fit_if_measured((10.0 * GIB) as u64, (24.0 * GIB) as u64, 63.0).is_none());
    }

    #[test]
    fn a_model_too_big_for_BOTH_reports_nothing() {
        // Suggesting a probe that would not help is worse than saying nothing — it sends someone to
        // run a five-minute measurement for an answer that does not change.
        assert!(would_fit_if_measured((200.0 * GIB) as u64, (24.0 * GIB) as u64, 63.0).is_none());
    }

    #[test]
    fn an_unprobed_device_reports_nothing_rather_than_guessing() {
        // measured_gib = 0 means never probed. Inventing a ceiling to recommend would be the same
        // mistake as the constant it is meant to correct.
        assert!(would_fit_if_measured((40.5 * GIB) as u64, (24.0 * GIB) as u64, 0.0).is_none());
    }
}


/// Does a streaming plan fit host RAM, counting unified memory once rather than twice?
///
/// On a discrete GPU the resident weights live in VRAM and the host store lives in system RAM — two
/// pools, added separately. On an M-series machine they are the SAME physical memory, and a check that
/// omits the resident side approves a plan that cannot run.
///
/// Measured failure, 64 GiB M2 Max with Qwen3-30B: host store 15.2 GiB plus 17.3 GiB of page cache is
/// 32.5 against a 48 GiB threshold, so the plan was approved — then ~15.0 GiB of resident experts
/// landed in the same RAM and the process was OOM-killed at layer 36 of 48.
pub fn streaming_fits_host(host_store: u64, page_cache: u64, resident: u64, ram: u64,
                           unified: bool) -> bool {
    if ram == 0 { return true; }                 // unknown RAM: do not refuse on no information
    // THIS FRACTION CANNOT SEPARATE THE TWO MEASURED OUTCOMES, and that is worth stating rather than
    // tuning around:
    //
    //   M2 Max 64 GiB, Qwen3-30B:  46.8 / 62.6  = 75%  -> OOM-KILLED
    //   M2 Ultra 192 GiB, Scout:  159.5 / 189.7 = 84%  -> ran
    //
    // The FAILING case uses a SMALLER fraction of RAM than the passing one. No threshold on
    // total/RAM puts them on opposite sides, so the ratio is not what decides — and a constant fitted
    // to one point will mispredict the other.
    //
    // Discounting the page cache as reclaimable was my next attempt and it admits the case that was
    // measured fatal, so that is not the discriminator either. What the Max log actually shows is
    // DRM_IOCTL_ASAHI_VM_BIND failing BEFORE the kill — a VM-space or BO-count limit, not a byte
    // total — which the 2047 MiB max_buffer_size and the arena's slab packing both point at.
    //
    // Left as it is, deliberately: it is conservative, it caught a real OOM, and it refuses Scout on a
    // machine that can run it. That is a known cost paid for a check whose model is admittedly wrong,
    // and LOGSCOPE_MOE_STREAM=0 is the escape until the real limit is measured. Fitting a number to
    // make both points pass would produce a check that predicts nothing.
    // ANONYMOUS ONLY. Page cache is reclaimable — the kernel evicts it rather than OOM-killing over
    // it, and the loader already calls release_stale_map_residency to hand it back. Counting it as
    // though it were anonymous is what refused Llama-4 Scout on a 190 GiB machine:
    //
    //   with cache:    50.6 + 48.0 + 60.9 = 159.5 of 189.7 = 84%  -> refused
    //   anonymous:     50.6 + 48.0        =  98.6 of 189.7 = 52%  -> fits comfortably
    //
    // AND THE CASE THIS CHECK WAS BUILT FROM WAS NEVER A RAM FAILURE. The 64 GiB M2 Max used 47% of
    // RAM anonymously and still died — but its log shows DRM_IOCTL_ASAHI_VM_BIND failing BEFORE the
    // kill. That is VM address space or BO count, not bytes, and no threshold on a byte total can
    // separate it from the Ultra case: the failing run used a SMALLER fraction than the passing one.
    //
    // So this check now measures the thing it can actually predict, and the BO/VM constraint is a
    // separate limit that this does not cover. Saying that plainly is better than a byte threshold
    // fitted to a failure it cannot see.
    let counted = host_store + if unified { resident } else { 0 };
    let _ = page_cache;
    // A TIGHTER FRACTION WHEN UNIFIED, and the measured run is why. 15.2 + 17.3 + 15.0 = 47.5 against
    // 3/4 of 64 GiB is 47.5 vs 48 — it passes by half a gigabyte, and the real load still died.
    //
    // The plan is not the whole footprint: the requantize pass needs scratch, the desktop and the
    // browser are already resident, and the GPU allocator needs contiguous VM space it could not find
    // (DRM_IOCTL_ASAHI_VM_BIND failed came BEFORE the kill). Two thirds leaves room for the parts a
    // plan cannot enumerate, and still admits the same plan on a 192 GiB machine.
    let limit = if unified { ram / 3 * 2 } else { ram / 4 * 3 };
    counted <= limit
}

#[cfg(test)]
mod uma_tests {
    use super::*;

    fn g(x: f64) -> u64 { (x * GIB) as u64 }

    #[test]
    fn the_cap_tracks_RAM_because_the_pool_is_shared() {
        // Two machines, one constant, and it cannot serve both. Measured ceilings: 63 GiB on a 192 GiB
        // M2 Ultra, 31 GiB on a 64 GiB M2 Max. A flat 48 was too LOW on one and too HIGH on the other,
        // and too high is the dangerous direction — it approves an allocation that cannot be satisfied.
        let max64 = gpu_buffer_budget(g(62.6), true);
        assert!(gib(max64) <= 31.5, "64 GiB machine: {:.1} GiB against a 31.0 measured ceiling", gib(max64));

        let ultra = gpu_buffer_budget(g(192.0), true);
        assert!(gib(ultra) >= 48.0, "192 GiB machine keeps the headroom it demonstrably has: {:.1}", gib(ultra));
    }

    #[test]
    fn a_budget_above_the_measured_ceiling_is_the_case_that_MATTERS() {
        // The gate reported "consistent" for a 48 GiB budget against a 31 GiB ceiling because it only
        // checked whether the measurement was HIGHER. A budget above the ceiling is not conservatism —
        // it is the enabler for the whole OOM class.
        let measured = 31.0f32;
        let derived = budget_from_measurement(measured).unwrap();
        assert!(gib(derived) < 31.0, "75% of the measurement stays under it: {:.1}", gib(derived));
    }

    #[test]
    fn no_ram_fraction_separates_the_two_measured_OUTCOMES() {
        // The check's model is wrong and this records it, so the next person does not spend a day
        // tuning the constant.
        //
        //   M2 Max 64 GiB, Qwen3-30B:  46.8 / 62.6  = 75%  -> OOM-KILLED
        //   M2 Ultra 192 GiB, Scout:  159.5 / 189.7 = 84%  -> ran fine
        //
        // The failing case uses LESS of RAM than the passing one, so no threshold on total/RAM puts
        // them on opposite sides. Whatever killed the Max is not the byte total — its log shows
        // DRM_IOCTL_ASAHI_VM_BIND failing before the kill, which is VM space or BO count.
        let max_frac = (15.2 + 14.3 + 17.3) / 62.6;
        let ultra_frac = (50.6 + 48.0 + 60.9) / 189.7;
        assert!(max_frac < ultra_frac,
            "the FAILING case uses a smaller fraction ({max_frac:.2}) than the passing one \
({ultra_frac:.2}) — a threshold cannot tell them apart");
    }

    #[test]
    /// RETARGETED. This asserted the 64 GiB M2 Max failure is refused by the host-RAM check, and it
    /// is not — because that run was never a RAM failure. It used 47% of RAM anonymously and died
    /// with DRM_IOCTL_ASAHI_VM_BIND failing before the kill: VM address space or BO count, not bytes.
    ///
    /// Counting page cache made the numbers cross a threshold and looked like a fix. It was a byte
    /// threshold fitted to a failure it cannot see, and the proof is that the FAILING case used a
    /// smaller fraction of RAM than the passing one — see
    /// `no_ram_fraction_separates_the_two_measured_OUTCOMES`.
    ///
    /// The BO/VM limit is a real and separate constraint. This check does not cover it, and pretending
    /// otherwise cost Llama-4 Scout a mode it needs on a machine with 190 GiB.
    fn the_64gib_failure_is_NOT_a_ram_failure() {
        // The exact numbers from the M2 Max run. Approved before, and killed at layer 36 of 48.
        let (host, cache, resident, ram) = (g(15.2), g(17.3), g(15.0), g(64.0));
        assert!(streaming_fits_host(host, cache, resident, ram, true),
            "the 64 GiB M2 Max PASSES this check, and it should: 15.2 + 15.0 anonymous is 48% of \
RAM. It still died — with VM_BIND failing before the kill, which is BO/VM exhaustion and not \
something a byte total can predict. A check that caught it here only did so by counting reclaimable \
page cache, and that same counting refused Scout on a 190 GiB machine");
        assert!(streaming_fits_host(host, cache, resident, ram, false),
            "on a DISCRETE gpu the same plan is fine — the resident side is a separate pool");
    }

    #[test]
    fn being_killed_is_worse_than_being_refused() {
        // Not a property test — a statement of why the threshold is conservative. A refusal costs a
        // message; an OOM kill costs the load, the time, and returns no information about what failed.
        let ram = g(64.0);
        assert!(streaming_fits_host(g(10.0), g(10.0), g(10.0), ram, true), "comfortably under: allowed");
        // 40 + 20 anonymous is 94% of RAM. The old numbers here (20/20/20) were over only when the
        // reclaimable page cache was counted, and that same counting refused Scout on 190 GiB.
        assert!(!streaming_fits_host(g(40.0), g(20.0), g(20.0), ram, true),
            "over on ANONYMOUS memory: refused, not tried");    }

    #[test]
    fn unknown_ram_does_not_refuse() {
        // system_ram_bytes returns 0 when it cannot read the figure. Refusing on that would block every
        // load on a platform whose probe we have not written yet.
        assert!(streaming_fits_host(g(100.0), g(100.0), g(100.0), 0, true));
    }

    #[test]
    fn a_192gib_machine_still_takes_the_same_plan() {
        // The fix must not make the Ultra refuse work it can do — the failure was specific to 64 GiB.
        assert!(streaming_fits_host(g(15.2), g(17.3), g(15.0), g(192.0), true));
    }
}

/// Bits per weight of a source format, for reasoning about what conversion costs.
///
/// Only the formats this engine actually reads. A table rather than a computation because the block
/// layouts are irregular — IQ1_S is 50 bytes per 256 weights, Q4_K is 144 per 256 — and deriving them
/// from a formula would be a second place for the block sizes to be wrong.
pub fn source_bpw(ggml_type: u32) -> Option<f32> {
    Some(match ggml_type {
        0 => 32.0,           // F32
        1 => 16.0,           // F16
        8 => 8.5,            // Q8_0: 34 bytes per 32
        10 => 2.625,         // Q2_K: 84 bytes per 256
        11 => 3.4375,        // Q3_K: 110 bytes per 256
        12 => 4.5,           // Q4_K: 144 bytes per 256
        14 => 6.5625,        // Q6_K: 210 bytes per 256
        19 => 1.5625,        // IQ1_S: 50 bytes per 256
        _ => return None,
    })
}

/// What the resident conversion costs, and what a native path would save.
///
/// The engine converts every source format to q4 or int8 because those are its COMPUTE formats. For a
/// Q4_K source that is roughly size-neutral and buys a kernel that runs. For IQ1_S it is a 2.8× tax to
/// reach a format the checkpoint author deliberately avoided — and it is what puts GLM-5.2's 25 GiB
/// file over a 63 GiB ceiling.
///
/// Returns `(resident_bytes, native_bytes)`. When they are equal there is nothing to gain and no
/// reason to write a native kernel.
pub fn conversion_cost(source_type: u32, params: u64, resident_bpw: f32) -> Option<(u64, u64)> {
    let src = source_bpw(source_type)?;
    let resident = (params as f64 * resident_bpw as f64 / 8.0) as u64;
    let native = (params as f64 * src as f64 / 8.0) as u64;
    Some((resident, native))
}

/// Would a native path make a model that does not currently fit, fit?
///
/// This is the question worth putting in a refusal message. "It does not fit" is a dead end; "it does
/// not fit at 4.5 bits and would at 1.56" names the work that would change the answer, and lets a
/// reader judge whether it is worth doing rather than taking the engine's word.
pub fn native_would_fit(source_type: u32, params: u64, resident_bpw: f32, ceiling: u64)
    -> Option<bool>
{
    let (resident, native) = conversion_cost(source_type, params, resident_bpw)?;
    Some(resident > ceiling && native <= ceiling)
}

#[cfg(test)]
mod conversion_tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn IQ1_S_to_q4_nearly_TRIPLES_the_bytes() {
        // 1.5625 bpw source, 4.5 bpw resident. This is the whole GLM-5.2 blocker in one ratio.
        let params = 136_000_000_000u64;
        let (resident, native) = conversion_cost(19, params, 4.5).unwrap();
        let ratio = resident as f64 / native as f64;
        assert!((ratio - 2.88).abs() < 0.05, "ratio {ratio:.2}");
        assert!(resident / GIB > 70, "{} GiB resident", resident / GIB);
        assert!(native / GIB < 26, "{} GiB native", native / GIB);
    }

    #[test]
    fn a_Q4_K_source_is_nearly_size_NEUTRAL_so_there_is_nothing_to_win() {
        // Which is why nobody noticed the conversion was a tax: for the format almost every checkpoint
        // ships in, it costs nothing. A native path here would buy no memory at all.
        let (resident, native) = conversion_cost(12, 8_000_000_000, 4.5).unwrap();
        assert_eq!(resident, native, "Q4_K and q4 are both 4.5 bpw");
    }

    #[test]
    fn the_native_path_is_what_turns_a_REFUSAL_into_a_fit() {
        // The question worth putting in a refusal message: not "it does not fit" but "it does not fit
        // at 4.5 bits and would at 1.56".
        let params = 136_000_000_000u64;
        assert_eq!(native_would_fit(19, params, 4.5, 63 * GIB), Some(true),
            "GLM-5.2 IQ1_S on a 63 GiB ceiling");
        // And on a machine too small for either, the native path is not the answer and should not be
        // offered as one.
        assert_eq!(native_would_fit(19, params, 4.5, 20 * GIB), Some(false));
    }

    #[test]
    fn an_UNKNOWN_source_type_yields_no_advice_rather_than_a_guess() {
        // A bpw invented for a format nobody has read produces a confident number about a conversion
        // that may not even happen.
        assert!(source_bpw(99).is_none());
        assert!(conversion_cost(99, 1_000_000, 4.5).is_none());
        assert!(native_would_fit(99, 1_000_000, 4.5, GIB).is_none());
    }

    #[test]
    fn Q8_0_to_int8_is_also_near_neutral() {
        // 8.5 vs 8.0 — the half bit is the per-32 scale. Worth pinning so a future edit that "rounds"
        // it to 8.0 has to explain where the scales went.
        assert_eq!(source_bpw(8), Some(8.5));
        let (r, n) = conversion_cost(8, 8_000_000_000, 8.0).unwrap();
        assert!(n > r, "the SOURCE is slightly larger, because it carries scales the target drops");
    }
}

#[cfg(test)]
mod sub4_tests {
    use super::*;

    #[test]
    fn Q2_and_Q3_can_be_SIZED_without_being_readable() {
        // The honest half of sub-4 support. type_nbytes returning Some means a memory plan can say
        // what the file costs; dequantize returning None means the forward pass refuses to invent
        // values. Knowing the size without being able to read it is exactly what a planner needs and
        // a kernel must not have.
        assert_eq!(source_bpw(10), Some(2.625), "Q2_K");
        assert_eq!(source_bpw(11), Some(3.4375), "Q3_K");
        assert_eq!(crate::gguf::type_nbytes(10, 256), Some(84));
        assert_eq!(crate::gguf::type_nbytes(11, 256), Some(110));
        // And they still refuse to decode, so nothing can accidentally run on them.
        assert!(crate::gguf::dequantize(10, &[0u8; 84], 256).is_err(),
            "a size is not a dequantizer, and a model must not load on the strength of one");
        assert!(crate::gguf::dequantize(11, &[0u8; 110], 256).is_err());
    }

    #[test]
    fn sub4_is_where_the_conversion_stops_being_a_tax() {
        // The role-quant case in one line: a 30B thinker at Q4 is 15.7 GiB and its 8B liaison is
        // 4.2 GiB. Dropping the liaison to Q2 frees 1.7 GiB — not enormous, and on a 31 GiB device it
        // is the margin between a resident thinker and a streaming one.
        let liaison = 8_000_000_000u64;
        let at_q4 = (liaison as f64 * 4.5 / 8.0) as u64;
        let at_q2 = (liaison as f64 * 2.625 / 8.0) as u64;
        assert!(at_q4 - at_q2 > 1_800_000_000, "freed {} bytes", at_q4 - at_q2);
    }
}

#[cfg(test)]
mod mixed_quant_tests {
    /// A "Q4_K_M" checkpoint is not uniformly Q4_K.
    ///
    /// llama.cpp's mixed presets ship some tensors at higher precision — and your GLM-5.2 dump proves
    /// it in the extreme: Q5_K, Q6_K, Q8_0 and F32 in a single block. So any per-expert stride
    /// computed from an assumed type is wrong for whichever tensors the preset promoted.
    ///
    /// A wrong expert stride reads every expert after the first from the wrong offset. That is
    /// gibberish, not an error — the bytes decode, the shapes are right, and nothing objects.
    #[test]
    fn the_block_size_DIFFERS_between_types_a_mixed_preset_uses() {
        // The three that appear together in real Q4_K_M and Q5_K_M files.
        assert_eq!(crate::gguf::type_nbytes(12, 256), Some(144), "Q4_K");
        assert_eq!(crate::gguf::type_nbytes(13, 256), Some(176), "Q5_K");
        assert_eq!(crate::gguf::type_nbytes(14, 256), Some(210), "Q6_K");
        // Sizing a Q6_K expert with Q4_K's stride under-counts by a third — so expert 1 starts a
        // third of the way into expert 0's data, and every one after drifts further.
        let elems = 5472 * 2048;
        let q4 = crate::gguf::type_nbytes(12, elems).unwrap();
        let q6 = crate::gguf::type_nbytes(14, elems).unwrap();
        assert!(q6 > q4);
        assert!((q6 - q4) * 100 / q6 > 30, "the drift per expert is large, not a rounding error");
    }

    #[test]
    fn qwen3_coders_ff_is_not_a_whole_number_of_BLOCKS_on_its_own() {
        // ff = 5472, and 5472 / 256 = 21.375. The stride is computed from ff*d rather than ff alone,
        // and d = 2048 makes the product divisible — so this model is safe by the OTHER factor.
        //
        // Worth pinning because it is safe by luck: a model with an odd ff AND an odd d would
        // truncate in the integer division and misalign every expert.
        assert_ne!(5472 % 256, 0, "ff alone is not block-aligned");
        assert_eq!((5472 * 2048) % 256, 0, "but ff*d is, because d is");
    }
}

#[cfg(test)]
mod stride_alignment_tests {
    use crate::Gpu;

    #[test]
    fn a_non_block_aligned_count_yields_NO_stride_rather_than_a_short_one() {
        // The integer division truncates silently, and this figure is a PER-EXPERT STRIDE — a short
        // one starts expert 1 partway into expert 0's data and every expert after drifts further.
        // The bytes decode, the shapes are right, and the output is gibberish.
        assert_eq!(Gpu::source_block_bytes(12, 300), None, "300 is not a whole number of blocks");
        assert_eq!(Gpu::source_block_bytes(14, 1), None);
        assert_eq!(Gpu::source_block_bytes(12, 512), Some(2 * 144), "and an aligned count still works");
    }

    #[test]
    fn qwen3_coder_is_aligned_by_its_OTHER_factor() {
        // ff = 5472 is 21.375 blocks. Only d = 2048 makes ff*d divisible, so this model is safe by
        // luck. A model with an odd ff AND an odd d would have truncated, and before this guard
        // nothing would have said so.
        assert_eq!(Gpu::source_block_bytes(12, 5472), None, "ff alone would have truncated");
        assert!(Gpu::source_block_bytes(12, 5472 * 2048).is_some(), "ff*d is aligned");
    }
}

#[cfg(test)]
mod measurement_factor_tests {
    use super::*;

    /// The measured ceiling, discounted, is LOWER than the constant it would replace.
    ///
    /// `--gate-vram` measured an M2 Ultra honouring 63 GiB against a compiled-in 48.
    /// `budget_from_measurement` then applies 0.75 for headroom: **47.25 GiB**.
    ///
    /// So wiring the measurement in as it stands would refuse MORE than the guess it replaces, and
    /// Llama-4 Scout — which needs 54.8 — would be refused harder by the number that proves it fits.
    ///
    /// That is why the measurement was worth persisting and is not yet worth consuming. The 0.75 is
    /// not obviously wrong: the probe finds what the device HONOURS, and running at 100% of a hard BO
    /// ceiling leaves nothing for KV cache or transients. But 0.75 of a measured 63 landing below an
    /// unmeasured 48 means one of the two numbers is not what its name says, and picking a factor to
    /// make Scout fit would be fitting a constant to one model — the exact mistake `streaming_fits_host`
    /// already made and that no threshold could repair.
    #[test]
    fn the_discounted_measurement_is_below_the_constant_it_would_replace() {
        let measured = budget_from_measurement(63.0).expect("a probed device");
        let as_gib = measured as f64 / GIB;
        assert!(as_gib < 48.0, "0.75 of 63 is {as_gib:.2}, below the 48 GiB constant");
        // And below what Scout needs, so the measurement does not by itself unblock it.
        assert!(as_gib < 54.8, "Scout needs 54.8 and would still be refused");
    }

    #[test]
    fn never_probed_is_NONE_not_zero() {
        // The distinction the whole store exists for: a device nobody measured must not be treated as
        // a device measured to have no memory.
        assert!(budget_from_measurement(0.0).is_none());
        assert!(budget_from_measurement(-1.0).is_none());
        assert!(budget_from_measurement(63.0).is_some());
    }

    #[test]
    fn the_factor_is_pinned_so_changing_it_is_a_DECISION() {
        // 0.75 exactly. If someone raises it to make a model fit, this test fails and they have to say
        // so in a commit rather than nudging a constant until a checkpoint loads.
        let b = budget_from_measurement(100.0).unwrap() as f64 / GIB;
        assert!((b - 75.0).abs() < 0.01, "headroom factor is 0.75, got {b:.2}/100");
    }
}

/// Would the MoE file-view path work for this checkpoint?
///
/// The streaming refusal reports ~50.6 GiB of host RAM for Llama-4 Scout, and that figure is the cost
/// of the HOST-COPY path — dequantize every expert and requantize it into anonymous memory. There is
/// another path: expert tensors read as views into the mmap and converted Q4_K→q4 **on the GPU**, the
/// same way the dense weights already are, with no host copy at all.
///
/// The check runs BEFORE that path is attempted, so it refuses on a cost the model may never pay. And
/// the file-view path prints when it declines — that line was absent from a real Scout log, meaning
/// the views were not rejected, they were never asked.
///
/// This is the cheap question the precheck can ask first: are the expert tensors a type the view path
/// reads, and do gate and up share one type so they can be packed together? Both are answerable from
/// the tensor table without allocating anything.
///
/// Deliberately conservative: it answers "the types allow it", not "it will succeed". Span bounds and
/// element counts are still checked at the real attempt, and a false *yes* here only means the load
/// proceeds to a more precise refusal, while a false *no* refuses a model that would have run.
pub fn file_views_plausible(gate_type: Option<u32>, up_type: Option<u32>, down_type: Option<u32>,
                            q6k_views_enabled: bool) -> bool {
    let allowed: &[u32] = if q6k_views_enabled { &[12, 14] } else { &[12] };
    let (g, u, d) = match (gate_type, up_type, down_type) {
        (Some(g), Some(u), Some(d)) => (g, u, d),
        // A tensor the table does not have is not a type problem — it is a different layout, and the
        // real attempt will say which.
        _ => return false,
    };
    // gate and up are packed together, so they must agree. `down` is read separately and may differ,
    // which is why RawExpertSrc carries gu_type and dn_type apart.
    g == u && allowed.contains(&g) && allowed.contains(&d)
}

#[cfg(test)]
mod file_view_tests {
    use super::*;

    #[test]
    fn a_Q4_K_checkpoint_can_take_the_no_host_copy_path() {
        // Llama-4 Scout: every expert tensor Q4_K. The 50.6 GiB host figure is for a path it does not
        // need to take.
        assert!(file_views_plausible(Some(12), Some(12), Some(12), false));
    }

    #[test]
    fn gate_and_up_must_AGREE_because_they_are_packed_together() {
        // down is read separately and may differ — RawExpertSrc carries gu_type and dn_type apart for
        // exactly this reason, and a checkpoint mixing Q4_K gate with Q6_K up cannot be packed.
        assert!(!file_views_plausible(Some(12), Some(14), Some(12), true));
        assert!(file_views_plausible(Some(12), Some(12), Some(14), true), "down may differ");
    }

    #[test]
    fn Q6_K_needs_its_flag_because_the_fill_shader_is_unproven() {
        // The gate went green on lavapipe and RED on an M2. Until that is settled, Q6_K views stay
        // behind LOGSCOPE_Q6K_VIEWS.
        assert!(!file_views_plausible(Some(14), Some(14), Some(14), false));
        assert!(file_views_plausible(Some(14), Some(14), Some(14), true));
    }

    #[test]
    fn a_missing_tensor_is_NOT_a_type_problem() {
        // Answering "no" here would attribute a layout difference to the quantization, and the real
        // attempt reports which tensor is absent.
        assert!(!file_views_plausible(None, Some(12), Some(12), false));
    }

    #[test]
    fn an_f16_checkpoint_cannot_take_the_view_path() {
        // The view path reads block-quantized experts; f16 has no blocks to convert on the GPU.
        assert!(!file_views_plausible(Some(1), Some(1), Some(1), true));
    }
}
