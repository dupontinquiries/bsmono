//! gemmadiff.rs — DiffusionGemma (google/diffusiongemma-26B-A4B): block-diffusion text generation
//! on the Gemma 4 26B-A4B MoE stack. The pure layer plan + the BlockRefinement scheduler, so the
//! parts that CAN be verified headless ARE, before any kernel exists.
//!
//! ## Provenance, stated because it bounds what this module may claim
//!
//! Two references, neither of which is the modeling code:
//! - the HF drop (config.json / generation_config.json / scheduler_config.json / README /
//!   model.safetensors.index.json) — shapes, the per-layer attention plan, the sampler constants,
//!   and the WEIGHT CENSUS (which tensors exist, per layer);
//! - llama.cpp `src/models/gemma4.cpp` — the Gemma 4 26B-A4B base this model is built on, which
//!   settles the wiring questions the census alone cannot: V-from-K on layers without `v_proj`
//!   (with a WEIGHTLESS rms on V), the twin-FFN parallel residual and its five norms, the router
//!   operating on a weightless-rms of attn_out times a learned scale vector, the per-layer output
//!   scalar, and proportional rope via per-layer `rope_freqs` on full-attention layers.
//!
//! The transformers 5.8.0.dev0 modeling code is NOT in the drop. Everything the base stack shares
//! with Gemma 4 is transcribed from gemma4.cpp; everything diffusion-specific (the scheduler, the
//! encoder-mode layer scalar, self-conditioning) is transcribed from the shipped configs and the
//! model card, and where the card underdetermines a formula the interpretation is NAMED at the
//! definition site rather than smuggled in. `archfeat` carries the implementation gaps; this
//! module is the plan those gaps will be closed against, plus the CPU reference the gates will
//! compare to.
//!
//! ## What is genuinely new here vs the archs already wired
//!
//! - **Per-layer attention geometry.** Sliding layers: head_dim 256, 8 KV heads, rope θ=1e4,
//!   full-dim rotation. Full layers (1 in 6): head_dim 512, 2 KV heads, θ=1e6, PARTIAL rotation
//!   (rotary factor 0.25 → n_rot 128) with a learned `rope_freqs` factor tensor. head_dim varying
//!   BY LAYER is structural for this engine (archfeat::PerLayerHeadDim).
//! - **V from K.** The five full-attention layers ship no `attn_v` — V is the K projection's
//!   output, weightless-rms-normalized, NOT rope'd (K itself gets the learned k_norm and rope).
//! - **Twin FFN, parallel residual.** From attn_out, TWO branches: the dense shared FFN
//!   (ffn_norm → GeGLU → post_ffw_norm_1) and the MoE (pre_ffw_norm_2 → routed GELU experts →
//!   post_ffw_norm_2), summed, post_ffw_norm'd, THEN added to attn_out. Not sequential.
//! - **Router input norm + scales.** Router logits = W_router · (rms(attn_out)/√d ⊙ s_router),
//!   softmax gating, top-8 of 128; expert outputs are additionally scaled by a learned
//!   per-expert vector before the weighted sum.
//! - **Mode-dependent layer scale.** One learned scalar per layer per PASS: the decoder set for
//!   canvas denoising, a SEPARATE encoder set for the causal pass that appends a finished canvas
//!   to the KV cache. Same weights otherwise — one stack, two scalars.
//! - **Block-refinement decoding.** Canvas of 256 in blocks of 32; per step, the entropy-bound
//!   sampler commits the lowest-entropy positions, fully renoises the rest, anneals temperature
//!   0.8 → 0.4, and stops a canvas early when it is both confident and stable.

/// The shape numbers the plan needs, read from GGUF metadata (see [`read_shape`] for the keys —
/// the converter in tools/convert-diffusion-gemma writes exactly these).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GemmaDiffShape {
    pub n_layer: u32,
    pub d: u32,            // embedding_length (2816)
    pub n_head: u32,       // 16, both layer kinds
    /// Full-attention layers' per-head dim (512) and KV head count (2).
    pub head_dim_full: u32,
    pub n_kv_full: u32,
    /// Sliding layers' per-head dim (256) and KV head count (8).
    pub head_dim_swa: u32,
    pub n_kv_swa: u32,
    /// Every `swa_period`-th layer is full-attention, LAST in each group (llama.cpp
    /// dense_first=false — the same rule `swa::swa_layers` already implements). 6 here.
    pub swa_period: u32,
    pub window: u32,       // sliding window, 1024
    /// Rope: full layers rotate only the first `n_rot_full` dims (partial factor 0.25 → 128) at
    /// θ=rope_base_full with a learned per-layer freq-factor tensor; sliding layers rotate the
    /// whole head at θ=rope_base_swa.
    pub rope_base_full: f32,
    pub rope_base_swa: f32,
    pub n_rot_full: u32,
    /// MoE: 128 experts, top-8, expert ff 704; the dense shared FFN is ff 2112.
    pub n_expert: u32,
    pub n_expert_used: u32,
    pub n_ff_shared: u32,
    pub n_ff_exp: u32,
    /// Gemma-2-style tanh cap on the FINAL logits (30.0 in the shipped config).
    pub logit_softcap: Option<f32>,
    /// Diffusion canvas: total canvas length (256) refined in blocks (32).
    pub canvas_len: u32,
    pub block_len: u32,
}

/// Which pass the stack is running — decides which per-layer output scalar applies.
///
/// `Denoise` is the bidirectional canvas pass (decoder scalar, `blk.N.layer_output_scale`);
/// `Encode` is the causal pass that appends a FINISHED canvas to the KV cache (encoder scalar,
/// `blk.N.layer_output_scale_enc`). The weights are otherwise shared, which is why this is an
/// argument and not a second model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass { Encode, Denoise }

impl GemmaDiffShape {
    /// Is layer `il` full-attention? Full layers sit LAST in each group of `swa_period`
    /// (indices 5, 11, 17, 23, 29 for period 6 over 30 layers) — the census confirms exactly
    /// those five layers ship no `attn_v`.
    pub fn is_full(&self, il: u32) -> bool {
        self.swa_period > 0 && (il + 1) % self.swa_period == 0
    }
    pub fn head_dim(&self, il: u32) -> u32 {
        if self.is_full(il) { self.head_dim_full } else { self.head_dim_swa }
    }
    pub fn n_kv(&self, il: u32) -> u32 {
        if self.is_full(il) { self.n_kv_full } else { self.n_kv_swa }
    }
    /// (rope base, rotated dims) for layer `il`. Sliding layers rotate the full head.
    pub fn rope(&self, il: u32) -> (f32, u32) {
        if self.is_full(il) { (self.rope_base_full, self.n_rot_full) }
        else { (self.rope_base_swa, self.head_dim_swa) }
    }
    /// Does this layer take V from the K projection (no `attn_v` tensor)? True exactly on the
    /// full-attention layers. gemma4.cpp: "if v_proj is not present, use Kcur as Vcur" — the
    /// RAW k projection output, then weightless-rms'd; K separately gets k_norm and rope, so
    /// the CACHED V and K still differ.
    pub fn v_from_k(&self, il: u32) -> bool { self.is_full(il) }

    /// K/V width for layer `il` — what one token adds to that layer's cache, per K and per V.
    pub fn kv_width(&self, il: u32) -> u32 { self.n_kv(il) * self.head_dim(il) }

    /// KV-cache bytes for ONE token across all layers, at f32 (what the engine stores). V is
    /// cached even where it is sourced from K — the two diverge after their norms — so the
    /// per-layer cost is 2·kv_width·4 regardless of `v_from_k`.
    pub fn kv_bytes_per_token(&self) -> u64 {
        (0..self.n_layer).map(|il| 2 * self.kv_width(il) as u64 * 4).sum()
    }

    /// Blocks per canvas, rejecting a canvas the blocks don't tile.
    pub fn blocks_per_canvas(&self) -> Result<u32, String> {
        if self.block_len == 0 || self.canvas_len == 0 {
            return Err("gemmadiff: zero canvas/block length".into());
        }
        if self.canvas_len % self.block_len != 0 {
            return Err(format!("gemmadiff: canvas {} not a multiple of block {}",
                               self.canvas_len, self.block_len));
        }
        Ok(self.canvas_len / self.block_len)
    }

    /// Sanity check, so a bad header is a message rather than wrong numbers.
    pub fn validate(&self) -> Result<(), String> {
        if self.n_head == 0 { return Err("gemmadiff: zero heads".into()); }
        for (name, kv, hd) in [("full", self.n_kv_full, self.head_dim_full),
                               ("swa", self.n_kv_swa, self.head_dim_swa)] {
            if kv == 0 || hd == 0 { return Err(format!("gemmadiff: zero {name} kv/head_dim")); }
            if self.n_head % kv != 0 {
                return Err(format!("gemmadiff: n_head {} not a multiple of {name} n_kv {kv}",
                                   self.n_head));
            }
        }
        if self.swa_period == 0 || self.n_layer % self.swa_period != 0 {
            // The census shows the pattern tiling the depth exactly (30 = 5×6). A period that
            // does not tile is not illegal in general, but for THIS arch it means the header
            // disagrees with every shipped checkpoint — worth stopping on.
            return Err(format!("gemmadiff: swa_period {} does not tile {} layers",
                               self.swa_period, self.n_layer));
        }
        if self.n_rot_full == 0 || self.n_rot_full > self.head_dim_full {
            return Err(format!("gemmadiff: n_rot_full {} vs head_dim_full {}",
                               self.n_rot_full, self.head_dim_full));
        }
        if self.n_expert == 0 || self.n_expert_used == 0 || self.n_expert_used > self.n_expert {
            return Err(format!("gemmadiff: expert counts {}/{}", self.n_expert_used, self.n_expert));
        }
        self.blocks_per_canvas().map(|_| ())
    }
}

/// Read the shape from GGUF metadata under the `diffusion_gemma.*` prefix.
///
/// Key set = llama.cpp's gemma4 keys where the concept is shared (sliding_window_pattern,
/// key_length / key_length_swa, rope.freq_base / freq_base_swa, expert counts), plus explicit
/// scalars where llama.cpp would use an array this reader has no helper for (head_count_kv /
/// head_count_kv_full), plus the diffusion block under a `diffusion.*` namespace shared with the
/// llada/rnd1 loaders (`diffusion.shift_logits` already lives there). The converter writes
/// exactly this set; a checkpoint from anywhere else that omits a key fails HERE with its name
/// rather than later with wrong numbers.
pub fn read_shape(g: &crate::gguf::Gguf) -> Result<GemmaDiffShape, String> {
    let p = "diffusion_gemma";
    let ku32 = |k: &str| g.get_u32(&format!("{p}.{k}"))
        .ok_or_else(|| format!("gemmadiff: missing {p}.{k}"));
    let kf32 = |k: &str| g.get_f32(&format!("{p}.{k}"))
        .ok_or_else(|| format!("gemmadiff: missing {p}.{k}"));
    let s = GemmaDiffShape {
        n_layer: ku32("block_count")?,
        d: ku32("embedding_length")?,
        n_head: ku32("attention.head_count")?,
        head_dim_full: ku32("attention.key_length")?,
        head_dim_swa: ku32("attention.key_length_swa")?,
        n_kv_full: ku32("attention.head_count_kv_full")?,
        n_kv_swa: ku32("attention.head_count_kv")?,
        swa_period: ku32("attention.sliding_window_pattern")?,
        window: ku32("attention.sliding_window")?,
        rope_base_full: kf32("rope.freq_base")?,
        rope_base_swa: kf32("rope.freq_base_swa")?,
        n_rot_full: ku32("rope.dimension_count")?,
        n_expert: ku32("expert_count")?,
        n_expert_used: ku32("expert_used_count")?,
        n_ff_shared: ku32("feed_forward_length")?,
        n_ff_exp: ku32("expert_feed_forward_length")?,
        logit_softcap: g.get_f32(&format!("{p}.final_logit_softcapping"))
            .filter(|c| c.is_finite() && *c > 0.0),
        canvas_len: g.get_u32("diffusion.canvas_length").unwrap_or(256),
        block_len: g.get_u32("diffusion.block_length").unwrap_or(32),
    };
    s.validate()?;
    Ok(s)
}

// ── CPU reference math for the ops the base archs don't have ────────────────────────────────────
// These are the oracle the GPU port will be gated against — the dsa.rs pattern. Plain f32 over
// slices, no deps, no engine types, so the gates can call them from anywhere.

/// Weightless RMS norm: x / rms(x), no learned weight, no (1+w). Used twice in this arch where
/// the learned norms are NOT: on V (always, where V exists at all) and on the router's input.
pub fn rms_weightless(x: &[f32], eps: f32) -> Vec<f32> {
    let ms = x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32;
    let inv = 1.0 / (ms + eps).sqrt();
    x.iter().map(|v| v * inv).collect()
}

/// Router logits for one token, per gemma4.cpp's "custom MoE logits calculation":
/// `W_router · ( rms(attn_out) · (1/√d) ⊙ s_router )`. The router reads ATTN_OUT (the residual
/// stream after the attention block), not the MoE branch's normed input — feeding it the normed
/// branch input is the natural mistake and routes every token subtly differently.
pub fn router_logits_reference(attn_out: &[f32], s_router: &[f32], w_router: &[f32],
                               n_expert: usize, eps: f32) -> Vec<f32> {
    let d = attn_out.len();
    debug_assert_eq!(s_router.len(), d);
    debug_assert_eq!(w_router.len(), n_expert * d);
    let scale = 1.0 / (d as f32).sqrt();
    let x: Vec<f32> = rms_weightless(attn_out, eps).iter().zip(s_router)
        .map(|(v, s)| v * scale * s).collect();
    (0..n_expert).map(|e| {
        let row = &w_router[e * d..(e + 1) * d];
        row.iter().zip(&x).map(|(w, v)| w * v).sum()
    }).collect()
}

/// Softmax over expert logits, then top-k (index, weight) pairs. Gemma 4 gates with SOFTMAX over
/// ALL experts (llama.cpp `LLAMA_EXPERT_GATING_FUNC_TYPE_SOFTMAX` with renorm=true over the
/// selected k — weights of the chosen experts are renormalized to sum to 1).
pub fn route_topk(logits: &[f32], k: usize) -> Vec<(usize, f32)> {
    let m = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|l| (l - m).exp()).collect();
    let z: f32 = exps.iter().sum();
    let mut probs: Vec<(usize, f32)> = exps.iter().map(|e| e / z).enumerate().collect();
    probs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    probs.truncate(k.min(probs.len()));
    let sel: f32 = probs.iter().map(|(_, p)| p).sum();
    if sel > 0.0 { for p in probs.iter_mut() { p.1 /= sel; } }
    probs
}

/// Apply the learned per-expert output scale (`blk.N.ffn_down_exps.scale`, one f32 per expert)
/// to a routed expert's output before the gate-weighted sum. Separate from
/// `ExpertWeightsScale` (a metadata CONSTANT) — this one is a trained vector.
pub fn per_expert_scale(expert_out: &mut [f32], expert_idx: usize, scales: &[f32]) {
    let s = scales.get(expert_idx).copied().unwrap_or(1.0);
    for v in expert_out.iter_mut() { *v *= s; }
}

/// The twin-FFN combine ORDER, which is the part a port gets wrong silently:
/// `post_ffw_norm(mlp_out + moe_out) + attn_out`, where the post norm is the Gemma (1+w) RMS.
/// Both branch inputs were attn_out; both branches carry their OWN post norms
/// (post_ffw_norm_1 / post_ffw_norm_2) before this combine — those are the caller's job, this
/// pins only the join. Sequential wiring (mlp feeding moe) produces plausible text and is wrong.
pub fn twin_ffn_combine(mlp_out: &[f32], moe_out: &[f32], attn_out: &[f32],
                        post_norm_w: &[f32], eps: f32) -> Vec<f32> {
    let sum: Vec<f32> = mlp_out.iter().zip(moe_out).map(|(a, b)| a + b).collect();
    let normed = rms_weightless(&sum, eps);
    normed.iter().zip(post_norm_w).zip(attn_out)
        .map(|((n, w), r)| n * (1.0 + w) + r).collect()
}

/// Gemma-2-style final-logit soft cap: `cap · tanh(x / cap)`. Already implemented engine-side
/// (Model::logit_softcap via logits_out); duplicated here only so the gemmadiff gate can check a
/// full reference pipeline without constructing a Model.
pub fn softcap(x: f32, cap: f32) -> f32 { cap * (x / cap).tanh() }

/// The per-layer output scalar for a pass. Encode has its OWN learned scalar per layer
/// (`layer_output_scale_enc`); its absence for an encode pass is an error, not a fallback —
/// scaling the causal pass with the denoise scalar is a different model that still runs.
pub fn layer_out_scale(pass: Pass, dec: f32, enc: Option<f32>) -> Result<f32, String> {
    match pass {
        Pass::Denoise => Ok(dec),
        Pass::Encode => enc.ok_or_else(|| {
            "gemmadiff: encode pass without layer_output_scale_enc — the encoder scalar set is \
             missing from this file (converter older than the tensor map?)".into()
        }),
    }
}

// ── BlockRefinement scheduler ────────────────────────────────────────────────────────────────────
// The decode loop's brain, pure over per-position probability rows so it tests headless. The
// engine's existing diffuse_generate loop supplies forwards and token plumbing; this decides,
// per step: which masked positions COMMIT, that the rest fully renoise, what temperature shapes
// the logits, and whether the canvas is DONE early.

pub mod blockrefine {
    /// Constants from the shipped scheduler_config.json + generation_config.json. Everything a
    /// caller might tune stays a field rather than a hardcode, so the env-var levers land in the
    /// loop, not here.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Schedule {
        /// Hard ceiling on denoising steps per block (generation_config max_denoising_steps: 48).
        pub max_steps: u32,
        /// Temperature anneal endpoints for logit shaping (t_max 0.8 → t_min 0.4, linear).
        pub t_max: f32,
        pub t_min: f32,
        /// Entropy-bound sampler: commit the lowest-entropy positions while the SUM of their
        /// entropies stays ≤ this bound (0.1). INTERPRETATION, stated: the card says "the sampler
        /// selects the lowest-entropy tokens such that their mutual information bound stays below
        /// entropy bound = 0.1"; the joint-vs-independent gap of positions decoded in parallel is
        /// bounded by the sum of their marginal entropies, so the sum rule is the reading
        /// implemented here. The modeling code is not in the drop to confirm the exact bound —
        /// if the reference turns out to use a different functional, this ONE function changes.
        pub entropy_bound: f32,
        /// At least this many positions commit per step even when the bound admits none
        /// (scheduler_config minimal_topk: 1) — the guarantee the loop terminates.
        pub minimal_topk: u32,
        /// Immediate-commit confidence: a position whose top probability reaches this commits
        /// regardless of the entropy budget (scheduler_config threshold: 0.95 — the same
        /// accelerator the existing loop calls tau).
        pub commit_threshold: Option<f32>,
        /// Adaptive stop, condition 1: mean entropy over the CANVAS below this
        /// (generation_config confidence_threshold: 0.005, in nats). The card says "average model
        /// entropy over the canvas" — the whole canvas, committed rows included, which is what the
        /// loop feeds `StopTracker::observe`.
        pub stop_entropy: f32,
        /// Adaptive stop, condition 2: argmax predictions identical across this many CONSECUTIVE
        /// steps (generation_config stability_threshold: 1 → one repeat, i.e. two equal reads).
        pub stability: u32,
    }

    impl Schedule {
        /// The shipped defaults (scheduler_config.json + generation_config.json of the 26B-A4B-it drop).
        pub fn shipped() -> Self {
            Schedule { max_steps: 48, t_max: 0.8, t_min: 0.4, entropy_bound: 0.1,
                       minimal_topk: 1, commit_threshold: Some(0.95),
                       stop_entropy: 0.005, stability: 1 }
        }
    }

    /// Temperature for `step` of `max_steps`: linear t_max → t_min, endpoint-inclusive. One step
    /// total means t_max (there is no second point to anneal toward).
    pub fn temperature(s: &Schedule, step: u32) -> f32 {
        if s.max_steps <= 1 { return s.t_max; }
        let f = (step.min(s.max_steps - 1)) as f32 / (s.max_steps - 1) as f32;
        s.t_max + (s.t_min - s.t_max) * f
    }

    /// Shannon entropy of one position's probability row, in nats. 0·ln0 = 0.
    pub fn entropy(probs: &[f32]) -> f32 {
        -probs.iter().filter(|p| **p > 0.0).map(|p| p * p.ln()).sum::<f32>()
    }

    /// One position's view for the selection step.
    #[derive(Clone, Copy, Debug)]
    pub struct Position {
        /// Index into the canvas (so commits name real positions, not row numbers).
        pub idx: usize,
        pub entropy: f32,
        /// Top probability at this position (for the commit_threshold fast path).
        pub p_top: f32,
    }

    /// What one refinement step decided.
    #[derive(Clone, Debug, PartialEq)]
    pub struct StepPlan {
        /// Canvas positions to COMMIT this step (their sampled tokens become fixed).
        pub commit: Vec<usize>,
        /// Canvas positions to fully RENOISE (back to the mask token) — always the complement of
        /// `commit` within the still-masked set. Carried explicitly so the loop cannot "keep"
        /// an uncommitted draft token by forgetting to reset it; partial keeping is a different
        /// sampler than the card describes.
        pub renoise: Vec<usize>,
    }

    /// Select which of the still-masked positions commit this step.
    ///
    /// Order: (1) every position at/above `commit_threshold` commits outright; (2) the remaining
    /// positions are taken lowest-entropy-first while the running entropy sum stays ≤
    /// `entropy_bound`; (3) if nothing committed, the single lowest-entropy position does
    /// (`minimal_topk`, generalized), so a step always makes progress. Everything else renoises.
    pub fn plan_step(s: &Schedule, masked: &[Position]) -> StepPlan {
        let mut commit: Vec<usize> = Vec::new();
        let mut rest: Vec<&Position> = Vec::new();
        for p in masked {
            match s.commit_threshold {
                Some(t) if p.p_top >= t => commit.push(p.idx),
                _ => rest.push(p),
            }
        }
        rest.sort_by(|a, b| a.entropy.partial_cmp(&b.entropy).unwrap_or(std::cmp::Ordering::Equal));
        let mut budget = s.entropy_bound;
        let mut took = 0usize;
        for p in &rest {
            if p.entropy <= budget { commit.push(p.idx); budget -= p.entropy; took += 1; }
            else { break; } // sorted ascending: nothing later fits either
        }
        // the floor: commit the most-certain leftovers up to minimal_topk so the loop terminates
        if commit.is_empty() {
            for p in rest.iter().take(s.minimal_topk.max(1) as usize) { commit.push(p.idx); }
        } else if took == 0 && commit.len() < s.minimal_topk as usize {
            for p in rest.iter().take(s.minimal_topk as usize - commit.len()) { commit.push(p.idx); }
        }
        let renoise = masked.iter().map(|p| p.idx).filter(|i| !commit.contains(i)).collect();
        StepPlan { commit, renoise }
    }

    /// Adaptive-stop tracker: the canvas ends early iff BOTH hold at once — mean entropy below
    /// `stop_entropy` AND the argmax reads identical for `stability` consecutive comparisons.
    /// Either alone is explicitly not enough (the card: "if and only if both ... simultaneously").
    #[derive(Clone, Debug, Default)]
    pub struct StopTracker {
        prev_argmax: Option<Vec<u32>>,
        stable_runs: u32,
    }

    impl StopTracker {
        /// Feed this step's canvas argmax + mean entropy; returns true when the canvas may stop.
        pub fn observe(&mut self, s: &Schedule, argmax: &[u32], mean_entropy: f32) -> bool {
            let stable_now = self.prev_argmax.as_deref() == Some(argmax);
            self.stable_runs = if stable_now { self.stable_runs + 1 } else { 0 };
            self.prev_argmax = Some(argmax.to_vec());
            mean_entropy < s.stop_entropy && self.stable_runs >= s.stability.max(1)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn pos(idx: usize, entropy: f32, p_top: f32) -> Position { Position { idx, entropy, p_top } }

        #[test]
        fn temperature_anneals_linearly_between_the_shipped_endpoints() {
            let s = Schedule::shipped();
            assert_eq!(temperature(&s, 0), 0.8);
            assert!((temperature(&s, 47) - 0.4).abs() < 1e-6);
            let mid = temperature(&s, 23);
            assert!(mid < 0.8 && mid > 0.4, "monotone between endpoints, got {mid}");
            // one-step schedule has no second point to anneal toward
            let one = Schedule { max_steps: 1, ..s };
            assert_eq!(temperature(&one, 0), 0.8);
        }

        #[test]
        fn entropy_is_zero_for_certainty_and_ln_n_for_uniform() {
            assert_eq!(entropy(&[1.0, 0.0, 0.0]), 0.0);
            let u = entropy(&[0.25; 4]);
            assert!((u - 4.0f32.ln()).abs() < 1e-6, "{u}");
        }

        #[test]
        fn selection_respects_the_entropy_budget_sum_rule() {
            let s = Schedule { commit_threshold: None, ..Schedule::shipped() }; // isolate the bound
            // entropies 0.04 + 0.05 fit the 0.1 budget; adding 0.03 more (=0.12) would not
            let masked = [pos(7, 0.05, 0.5), pos(3, 0.04, 0.5), pos(9, 0.03, 0.5)];
            let plan = plan_step(&s, &masked);
            let mut c = plan.commit.clone(); c.sort();
            assert_eq!(c, vec![3, 9], "lowest-entropy first: 0.03 + 0.04 = 0.07 ≤ 0.1, +0.05 breaks it");
            assert_eq!(plan.renoise, vec![7], "everything uncommitted renoises");
        }

        #[test]
        fn minimal_topk_guarantees_progress_when_the_bound_admits_nothing() {
            let s = Schedule { commit_threshold: None, ..Schedule::shipped() };
            let masked = [pos(0, 2.0, 0.2), pos(1, 1.5, 0.3)];
            let plan = plan_step(&s, &masked);
            assert_eq!(plan.commit, vec![1], "the single most-certain position commits anyway");
            assert_eq!(plan.renoise, vec![0]);
        }

        #[test]
        fn the_confidence_threshold_commits_outright_and_skips_the_budget() {
            let s = Schedule::shipped(); // commit_threshold 0.95 active
            // both entropies far exceed the 0.1 budget; only p_top decides
            let masked = [pos(2, 3.0, 0.97), pos(4, 3.0, 0.10), pos(6, 3.0, 0.96)];
            let mut c = plan_step(&s, &masked).commit; c.sort();
            assert_eq!(c, vec![2, 6]);
        }

        #[test]
        fn commit_and_renoise_partition_the_masked_set() {
            let s = Schedule::shipped();
            let masked: Vec<Position> = (0..10).map(|i| pos(i, 0.02 * i as f32, 0.5)).collect();
            let plan = plan_step(&s, &masked);
            let mut all: Vec<usize> = plan.commit.iter().chain(&plan.renoise).copied().collect();
            all.sort();
            assert_eq!(all, (0..10).collect::<Vec<_>>(), "no position kept half-denoised");
        }

        #[test]
        fn stopping_requires_both_conditions_and_consecutive_stability() {
            let s = Schedule::shipped(); // stop_entropy 0.005, stability 1
            let mut t = StopTracker::default();
            // confident but not yet stable (first observation has nothing to be stable against)
            assert!(!t.observe(&s, &[1, 2, 3], 0.001));
            // stable but not confident
            assert!(!t.observe(&s, &[1, 2, 3], 0.010));
            // both at once → stop
            assert!(t.observe(&s, &[1, 2, 3], 0.001));
            // a changed argmax resets the run even while confident
            assert!(!t.observe(&s, &[9, 2, 3], 0.001));
            assert!(t.observe(&s, &[9, 2, 3], 0.001));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape_26b() -> GemmaDiffShape {
        GemmaDiffShape {
            n_layer: 30, d: 2816, n_head: 16,
            head_dim_full: 512, n_kv_full: 2,
            head_dim_swa: 256, n_kv_swa: 8,
            swa_period: 6, window: 1024,
            rope_base_full: 1_000_000.0, rope_base_swa: 10_000.0, n_rot_full: 128,
            n_expert: 128, n_expert_used: 8, n_ff_shared: 2112, n_ff_exp: 704,
            logit_softcap: Some(30.0), canvas_len: 256, block_len: 32,
        }
    }

    #[test]
    fn the_five_full_layers_match_the_weight_census() {
        // model.safetensors.index.json ships attn_v on 25 of 30 layers; the five WITHOUT are the
        // full-attention layers, last in each group of six. This pins is_full and v_from_k to the
        // shipped file rather than to an off-by-one that still runs.
        let s = shape_26b();
        s.validate().unwrap();
        let fulls: Vec<u32> = (0..30).filter(|&i| s.is_full(i)).collect();
        assert_eq!(fulls, vec![5, 11, 17, 23, 29]);
        for il in 0..30 {
            assert_eq!(s.v_from_k(il), s.is_full(il), "V-from-K exactly on the full layers");
        }
    }

    #[test]
    fn per_layer_geometry_switches_all_four_numbers_together() {
        let s = shape_26b();
        assert_eq!((s.head_dim(0), s.n_kv(0)), (256, 8), "sliding layer");
        assert_eq!((s.head_dim(5), s.n_kv(5)), (512, 2), "full layer");
        assert_eq!(s.rope(0), (10_000.0, 256), "sliding: full-head rotation at 1e4");
        assert_eq!(s.rope(5), (1_000_000.0, 128), "full: PARTIAL rotation (0.25·512) at 1e6");
        // the cache widths are NOT equal: 8·256 = 2048 on sliding layers, 2·512 = 1024 on full
        // ones — a uniform per-layer KV allocation over-provisions every full layer by 2x, which
        // is exactly why PerLayerHeadDim is flagged STRUCTURAL rather than a knob
        assert_eq!(s.kv_width(0), 2048);
        assert_eq!(s.kv_width(5), 1024);
    }

    #[test]
    fn kv_bytes_count_v_even_where_v_is_sourced_from_k() {
        let s = shape_26b();
        // v_from_k changes the SOURCE, not the cache: both K and V rows are stored on every
        // layer. 25 sliding layers at width 2048 + 5 full layers at width 1024, ×2 (K,V) ×4 bytes.
        assert_eq!(s.kv_bytes_per_token(), (25 * 2048 + 5 * 1024) * 2 * 4);
    }

    #[test]
    fn validation_names_the_field_that_is_wrong() {
        let mut s = shape_26b();
        s.swa_period = 7; // does not tile 30
        let e = s.validate().unwrap_err();
        assert!(e.contains("swa_period 7"), "{e}");
        let mut s = shape_26b();
        s.n_rot_full = 600; // exceeds head_dim_full
        assert!(s.validate().unwrap_err().contains("n_rot_full"), "names the rope field");
        let mut s = shape_26b();
        s.canvas_len = 250; // 32 does not tile it
        assert!(s.validate().unwrap_err().contains("canvas"), "names the canvas");
    }

    #[test]
    fn router_reference_scale_vector_and_input_norm_both_bite() {
        // d=4, 2 experts. With unit scales and identity-ish weights the logits follow the normed
        // input; zeroing the scale vector zeroes every logit — the learned vector is load-bearing,
        // not cosmetic, which is what makes ignoring it a silent quality bug.
        let attn_out = [2.0, 0.0, 0.0, 0.0];
        let w = [1.0, 0.0, 0.0, 0.0,   0.0, 1.0, 0.0, 0.0]; // expert0 reads dim0, expert1 dim1
        let ones = [1.0; 4];
        let l = router_logits_reference(&attn_out, &ones, &w, 2, 1e-6);
        assert!(l[0] > 0.0 && l[1].abs() < 1e-6, "{l:?}");
        let zeros = [0.0; 4];
        let l0 = router_logits_reference(&attn_out, &zeros, &w, 2, 1e-6);
        assert!(l0.iter().all(|v| v.abs() < 1e-9), "zero scale ⇒ zero logits: {l0:?}");
    }

    #[test]
    fn routing_softmaxes_over_all_experts_then_renormalizes_the_top_k() {
        let l = [1.0, 0.0, -1.0, 3.0];
        let top = route_topk(&l, 2);
        assert_eq!(top[0].0, 3);
        assert_eq!(top[1].0, 0);
        let sum: f32 = top.iter().map(|(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1e-6, "selected weights renormalize to 1, got {sum}");
        assert!(top[0].1 > top[1].1);
    }

    #[test]
    fn per_expert_scale_is_a_vector_lookup_not_a_constant() {
        let scales = [1.0, 0.5, 2.0];
        let mut out = [4.0, 4.0];
        per_expert_scale(&mut out, 1, &scales);
        assert_eq!(out, [2.0, 2.0]);
        let mut out2 = [4.0];
        per_expert_scale(&mut out2, 2, &scales);
        assert_eq!(out2, [8.0]);
    }

    #[test]
    fn twin_ffn_joins_with_one_post_norm_then_the_residual() {
        // With a zero post-norm weight the (1+w) factor is 1, so the combine reduces to
        // rms(mlp+moe) + attn_out — pinning both the (1+w) fold and the residual ORDER.
        let (mlp, moe, res) = ([3.0f32, 0.0], [1.0f32, 0.0], [10.0f32, 10.0]);
        let out = twin_ffn_combine(&mlp, &moe, &res, &[0.0, 0.0], 0.0);
        // sum = [4,0]; rms = sqrt(8) = 2√2; normed = [√2, 0]
        assert!((out[0] - (10.0 + 2.0f32.sqrt())).abs() < 1e-5, "{out:?}");
        assert!((out[1] - 10.0).abs() < 1e-6);
    }

    #[test]
    fn softcap_bounds_and_preserves_sign() {
        // tanh saturates to exactly 1.0 in f32 well before 1e6/30, so the bound is INCLUSIVE
        assert!(softcap(1e6, 30.0) <= 30.0 && softcap(1e6, 30.0) > 29.9);
        assert!(softcap(-1e6, 30.0) >= -30.0 && softcap(-1e6, 30.0) < -29.9);
        assert!((softcap(0.1, 30.0) - 0.1).abs() < 1e-4, "near-linear at small x");
    }

    #[test]
    fn the_encode_pass_refuses_to_borrow_the_denoise_scalar() {
        assert_eq!(layer_out_scale(Pass::Denoise, 0.7, None).unwrap(), 0.7);
        assert_eq!(layer_out_scale(Pass::Encode, 0.7, Some(0.9)).unwrap(), 0.9);
        let e = layer_out_scale(Pass::Encode, 0.7, None).unwrap_err();
        assert!(e.contains("layer_output_scale_enc"), "the refusal names the missing tensor: {e}");
    }
}
