//! moe.rs — Mixture-of-Experts routing for the wgpu decode path (Qwen4 / Llama4). Pure, no wgpu, testable.
//!
//! The archcfg registry knows an arch is MoE and where the tensors live; this module is the *routing
//! decision* the engine needs at each token: given the router logits, which experts run and with what
//! weights. That decision is the only genuinely MoE-specific compute at DECODE time.
//!
//! Key simplification (from the MoE-inference literature, e.g. FusedXpert / RaMP / the Apple-Silicon MoE
//! work): the general MoE problem — routing many tokens across many experts, needing grouped-GEMM + token
//! dispatch/gather — is a PREFILL/batched concern. At batch=1 DECODE there is exactly one token, so:
//!
//!   1. router logits = x · Wg                    (a small matvec d → n_expert; existing kernel)
//!   2. weights, ids  = topk(gate(logits))        (this module — tiny, on the host)
//!   3. for each selected expert e:               (each expert FFN is a dense SwiGLU MLP)
//!        h_e = SwiGLU(x · gate[e], x · up[e]);  y += weight_e · (h_e · down[e])   (EXISTING matvec/swiglu
//!        kernels, indexed into the stacked ffn_*_exps tensors — see `expert_row_offset`)
//!   4. + shared expert(s) always at weight 1.0   (Llama4/DeepSeek)
//!
//! So MoE decode needs NO new grouped-GEMM kernel — it reuses the dense matvec/SwiGLU kernels per selected
//! expert. The residency story mirrors Colibri: the dense stack (attn, norms, embeddings, shared experts)
//! stays resident; routed experts stream on demand (memplan's Streaming residency), since only k of N are
//! touched per token.

/// How router logits become gate weights before top-k. Softmax is the Mixtral/Qwen family; sigmoid is the
/// DeepSeek-V3 / some-Llama4 family (independent per-expert gate, normalized across the selected top-k).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteNorm { Softmax, Sigmoid }

/// MoE routing config for a layer (from `{arch}.expert_count` / `expert_used_count` / `expert_shared_count`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoeConfig {
    pub n_routed: u32,  // total routed experts (N)
    pub n_used: u32,    // top-k activated per token (k)
    pub n_shared: u32,  // always-on shared experts (Llama4/DeepSeek); 0 for Qwen-style
    pub norm: RouteNorm,
    /// Renormalize the selected weights to sum to 1 (softmax gating and DeepSeek-style sigmoid).
    /// Llama4: FALSE — the combine weight is the raw sigmoid of the selected logit (with top-1
    /// routing, normalizing would erase the gate entirely: every weight would become 1.0).
    pub normalize: bool,
    /// GLM-5.2's `routed_scaling_factor`, applied to every routed weight AFTER normalization.
    ///
    /// ORDER IS LOAD-BEARING and the wrong order is silent. Normalizing divides by the sum, which
    /// removes any common factor — so scaling BEFORE normalizing cancels the scale entirely and the
    /// model runs, looking fine, with the factor doing nothing. Colibri's glm.c does norm at line 3036
    /// and scale at 3037, in that order.
    ///
    /// 1.0 for every architecture that does not set it, so this is inert unless a config asks for it.
    pub routed_scale: f32,
    /// Apply the combine weight to the expert's INPUT instead of its output (Llama4). NOT
    /// equivalent through SwiGLU's nonlinearity: silu(Wg·(w·x))·(Wu·(w·x)) ≠ w·silu(Wg·x)·(Wu·x).
    pub weight_before: bool,
}

impl MoeConfig {
    pub fn is_moe(&self) -> bool { self.n_routed > 0 }
}

/// One selected routed expert and its (renormalized) combine weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selected { pub expert: u32, pub weight: f32 }

fn softmax_in_place(v: &mut [f32]) {
    let m = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut s = 0.0f32;
    for x in v.iter_mut() { *x = (*x - m).exp(); s += *x; }
    if s > 0.0 { for x in v.iter_mut() { *x /= s; } }
}

#[inline]
fn sigmoid(x: f32) -> f32 { 1.0 / (1.0 + (-x).exp()) }

/// Route one token: `logits` are the router outputs (length `n_routed`). Returns the top-k selected experts
/// with combine weights that sum to 1 (the standard renormalization for both softmax and sigmoid gating).
///
/// Mathematical note: for softmax gating, "softmax over all → top-k → renormalize" is IDENTICAL to "top-k by
/// logit → softmax over just the top-k" (the normalizer cancels), so we select by score then normalize the
/// selected — one code path, numerically stable via the softmax max-subtraction.
pub fn route(logits: &[f32], cfg: &MoeConfig) -> Vec<Selected> {
    let n = cfg.n_routed.min(logits.len() as u32) as usize;
    if n == 0 { return Vec::new(); }
    let k = cfg.n_used.clamp(1, n as u32) as usize;

    // per-expert gate score (monotonic in logit for both norms, so top-k is by score)
    let scores: Vec<f32> = match cfg.norm {
        RouteNorm::Softmax => logits[..n].to_vec(), // softmax is monotonic in logit → rank by logit directly
        RouteNorm::Sigmoid => logits[..n].iter().map(|&l| sigmoid(l)).collect(),
    };

    // top-k indices by score (partial selection; n is small — tens to low hundreds)
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_unstable_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal));
    idx.truncate(k);

    // combine weights over the selected experts, normalized to sum to 1
    let mut sel: Vec<Selected> = match cfg.norm {
        RouteNorm::Softmax => {
            // softmax over just the selected logits (== global softmax renormalized to the top-k)
            let mut w: Vec<f32> = idx.iter().map(|&i| logits[i]).collect();
            softmax_in_place(&mut w);
            idx.iter().zip(w).map(|(&i, weight)| Selected { expert: i as u32, weight }).collect()
        }
        RouteNorm::Sigmoid => {
            let raw: Vec<f32> = idx.iter().map(|&i| sigmoid(logits[i])).collect();
            if cfg.normalize {
                // DeepSeek-style: selected sigmoid gates renormalized to sum to 1
                let s: f32 = raw.iter().sum();
                idx.iter().zip(raw).map(|(&i, r)| Selected { expert: i as u32, weight: if s > 0.0 { r / s } else { 0.0 } }).collect()
            } else {
                // Llama4-style: the RAW sigmoid is the gate (top-1: weight = sigmoid(top logit))
                idx.iter().zip(raw).map(|(&i, r)| Selected { expert: i as u32, weight: r }).collect()
            }
        }
    };
    // ROUTED SCALE, applied AFTER normalization. Colibri's glm.c normalizes at line 3036 and scales at
    // 3037, and the order is load-bearing: normalizing divides by the sum, which removes any common
    // factor, so scaling FIRST cancels the scale entirely. The model would run, look fine, and have
    // routed_scaling_factor doing nothing — a silent no-op rather than a visible error.
    if cfg.routed_scale != 1.0 {
        for x in sel.iter_mut() { x.weight *= cfg.routed_scale; }
    }
    // keep a stable, descending-weight order for deterministic accumulation
    sel.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    sel
}

/// Row offset (in units of ROWS, i.e. multiples of the inner dim) of expert `e`'s slice inside a stacked
/// `ffn_*_exps` tensor of shape `[n_expert, rows, inner]`. The engine adds this to bind the right slice for
/// expert `e`'s matvec — the one piece of indexing that differs from the dense path. `rows` is `ffn` for
/// gate/up and `d` for down.
pub fn expert_row_offset(expert: u32, rows: usize) -> usize { expert as usize * rows }

/// The per-token expert-compute budget: how many expert FFNs actually run (routed top-k + shared). Drives
/// the streaming/residency estimate (only this many experts are touched per token, per Colibri).
pub fn active_expert_count(cfg: &MoeConfig) -> u32 { cfg.n_used.min(cfg.n_routed) + cfg.n_shared }

/// Per-layer attention + expert shapes, derived from the real GGUF tensor dims (not assumed from `d`).
/// The Qwen3 gotcha this captures: `head_dim` is NOT `d / n_heads` — it comes from the K-projection width
/// (`attn_k` output) divided by `n_kv_heads`. So the attention operates on `attn_dim = n_heads * head_dim`
/// (e.g. 4096 for Qwen3-Coder-30B, while `d` = 2048), and `attn_output` projects that back down to `d`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerShape {
    pub d: usize,          // model/residual width (2048)
    pub head_dim: usize,   // per-head dim (128) — independent of d/n_heads
    pub n_heads: usize,    // query heads (32)
    pub n_kv_heads: usize, // kv heads (4, GQA)
    pub attn_dim: usize,   // n_heads * head_dim (4096) — the width Q/attn operate on
    pub kv_dim: usize,     // n_kv_heads * head_dim (512) — K/V projection + cache row width
    pub n_ff_expert: usize,// per-expert FFN width (768)
    pub n_expert: usize,   // total routed experts (128)
}

/// Derive the layer shape. `attn_k_out` is the K-projection output dim (`attn_k.weight`'s output rows);
/// `expert_ff` is the per-expert FFN width (`ffn_gate_exps` middle dim). head_dim = attn_k_out / n_kv_heads.
pub fn layer_shape(d: usize, n_heads: usize, n_kv_heads: usize, attn_k_out: usize, expert_ff: usize, n_expert: usize) -> LayerShape {
    let head_dim = if n_kv_heads > 0 && attn_k_out % n_kv_heads == 0 { attn_k_out / n_kv_heads } else { d / n_heads.max(1) };
    LayerShape {
        d, head_dim, n_heads, n_kv_heads,
        attn_dim: n_heads * head_dim,
        kv_dim: n_kv_heads * head_dim,
        n_ff_expert: expert_ff,
        n_expert,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_routed_scale_is_applied_AFTER_normalization() {
        // The trap, from Colibri's glm.c ordering. Normalizing divides by the sum, which removes any
        // common factor — so scaling BEFORE normalizing cancels the scale completely. The model runs,
        // looks fine, and routed_scaling_factor does nothing.
        let mut cfg = MoeConfig { n_routed: 4, n_used: 2, n_shared: 0, norm: RouteNorm::Sigmoid,
                                  normalize: true, weight_before: false, routed_scale: 1.0 };
        let logits = [2.0f32, 1.0, -1.0, -2.0];
        let plain: f32 = route(&logits, &cfg).iter().map(|s| s.weight).sum();
        assert!((plain - 1.0).abs() < 1e-5, "normalized weights sum to 1, got {plain}");

        cfg.routed_scale = 2.5;                    // GLM-5.2's value
        let scaled: f32 = route(&logits, &cfg).iter().map(|s| s.weight).sum();
        assert!((scaled - 2.5).abs() < 1e-4,
            "after normalizing to 1, a 2.5x scale must give 2.5 — got {scaled}. If this reads 1.0 the \
scale was applied before the normalize and cancelled");
    }

    #[test]
    fn a_scale_of_one_leaves_every_weight_untouched() {
        // Inert for every architecture that does not set it, so adding the field changes nothing for
        // Llama-4, Qwen or DeepSeek.
        let cfg = MoeConfig { n_routed: 4, n_used: 2, n_shared: 0, norm: RouteNorm::Softmax,
                              normalize: true, weight_before: false, routed_scale: 1.0 };
        let a = route(&[1.0, 2.0, 3.0, 0.5], &cfg);
        let b = route(&[1.0, 2.0, 3.0, 0.5], &MoeConfig { routed_scale: 1.0, ..cfg });
        assert_eq!(a, b);
    }

    use super::*;

    fn approx(a: f32, b: f32) -> bool { (a - b).abs() < 1e-5 }

    #[test]
    fn softmax_route_picks_top_k_and_normalizes() {
        // 4 experts, top-2. logits favor experts 2 and 0.
        let cfg = MoeConfig { n_routed: 4, n_used: 2, n_shared: 0, norm: RouteNorm::Softmax, normalize: true, weight_before: false, routed_scale: 1.0 };
        let sel = route(&[2.0, 0.5, 3.0, -1.0], &cfg);
        assert_eq!(sel.len(), 2);
        let ids: Vec<u32> = sel.iter().map(|s| s.expert).collect();
        assert!(ids.contains(&2) && ids.contains(&0), "the two highest logits are selected: {ids:?}");
        let sum: f32 = sel.iter().map(|s| s.weight).sum();
        assert!(approx(sum, 1.0), "combine weights renormalize to 1 (got {sum})");
        // expert 2 (highest logit) has the largest weight
        assert_eq!(sel[0].expert, 2);
        assert!(sel[0].weight > sel[1].weight);
    }

    #[test]
    fn softmax_topk_equals_global_softmax_renormalized() {
        // the identity the doc claims: top-k softmax == global softmax restricted to the top-k
        let logits = [1.0f32, 2.0, 0.0, 3.0, -1.0];
        let cfg = MoeConfig { n_routed: 5, n_used: 2, n_shared: 0, norm: RouteNorm::Softmax, normalize: true, weight_before: false, routed_scale: 1.0 };
        let sel = route(&logits, &cfg);
        // global softmax
        let mut g = logits.to_vec(); softmax_in_place(&mut g);
        // experts 3 and 1 are top-2; their global-softmax renormalized weights
        let denom = g[3] + g[1];
        let w3 = g[3] / denom; let w1 = g[1] / denom;
        let get = |e: u32| sel.iter().find(|s| s.expert == e).unwrap().weight;
        assert!(approx(get(3), w3) && approx(get(1), w1));
    }

    #[test]
    fn sigmoid_route_normalizes_selected_gates() {
        let cfg = MoeConfig { n_routed: 4, n_used: 2, n_shared: 1, norm: RouteNorm::Sigmoid, normalize: true, weight_before: false, routed_scale: 1.0 };
        let sel = route(&[3.0, -2.0, 1.0, 0.0], &cfg);
        assert_eq!(sel.len(), 2);
        assert_eq!(sel[0].expert, 0, "highest logit → highest sigmoid gate");
        let sum: f32 = sel.iter().map(|s| s.weight).sum();
        assert!(approx(sum, 1.0));
        // active experts = top-k + shared
        assert_eq!(active_expert_count(&cfg), 3);
    }

    #[test]
    fn k_clamped_and_offsets() {
        // k larger than n clamps to n; offsets index the stacked tensor by row
        let cfg = MoeConfig { n_routed: 3, n_used: 8, n_shared: 0, norm: RouteNorm::Softmax, normalize: true, weight_before: false, routed_scale: 1.0 };
        let sel = route(&[0.1, 0.2, 0.3], &cfg);
        assert_eq!(sel.len(), 3, "k clamps to the number of experts");
        assert_eq!(expert_row_offset(0, 4096), 0);
        assert_eq!(expert_row_offset(5, 4096), 5 * 4096);
    }

    #[test]
    fn layer_shape_matches_qwen3_coder_30b_dump() {
        // from the confirmed GGUF dump: d=2048, n_heads=32, n_kv_heads=4, attn_k out=512, expert ff=768, 128 experts
        let s = layer_shape(2048, 32, 4, 512, 768, 128);
        assert_eq!(s.head_dim, 128, "head_dim = attn_k_out / n_kv_heads = 512/4, NOT d/n_heads = 64");
        assert_eq!(s.attn_dim, 4096, "attention width = n_heads*head_dim (matches attn_q output)");
        assert_eq!(s.kv_dim, 512, "kv width = n_kv_heads*head_dim (matches attn_k/v output)");
        assert_eq!(s.n_ff_expert, 768);
        assert_eq!(s.n_expert, 128);
    }

    #[test]
    fn layer_shape_preserves_dense_head_dim() {
        // DeepSeek-8B (llama): d=4096, n_heads=32, n_kv_heads=8, attn_k out=1024 → head_dim=128=d/n_heads (unchanged)
        let s = layer_shape(4096, 32, 8, 1024, 0, 0);
        assert_eq!(s.head_dim, 128);
        assert_eq!(s.attn_dim, 4096, "== d for a dense model, so behavior is identical");
        assert_eq!(s.kv_dim, 1024);
    }

    #[test]
    fn dense_config_is_not_moe() {
        let dense = MoeConfig { n_routed: 0, n_used: 0, n_shared: 0, norm: RouteNorm::Softmax, normalize: true, weight_before: false, routed_scale: 1.0 };
        assert!(!dense.is_moe());
        assert!(route(&[], &dense).is_empty());
    }

    #[test]
    fn llama4_gating_is_raw_sigmoid_top1() {
        // Llama4: top-1 by logit, weight = sigmoid(top logit) UNNORMALIZED (normalizing top-1
        // would always give 1.0 and erase the gate).
        let cfg = MoeConfig { n_routed: 16, n_used: 1, n_shared: 1, norm: RouteNorm::Sigmoid, normalize: false, weight_before: true, routed_scale: 1.0 };
        let mut logits = vec![-1.0f32; 16];
        logits[7] = 0.6;
        let sel = route(&logits, &cfg);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].expert, 7);
        let want = 1.0 / (1.0 + (-0.6f32).exp());
        assert!((sel[0].weight - want).abs() < 1e-6, "raw sigmoid gate, got {}", sel[0].weight);
    }
}

/// Cross-check the routing hparams against the expert tensors.
///
/// Scout's loader line reports every Llama-4 value as `(default)` — the file carries none of the keys
/// — so `expert_count`, `expert_used_count` and `expert_shared_count` are all taken on trust. The
/// tensors are not: an expert-stacked weight is `[d, ff, n_expert]` and a shared expert has its own
/// `ffn_*_shexp` tensors or does not exist.
///
/// So two of the three guesses are checkable, and the third — `expert_used_count` — is not, which is
/// worth knowing on its own: nothing in the file constrains how many experts are routed to, so a wrong
/// top-k is invisible to any check and shows only as worse output.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RoutingDisagreement {
    /// The stacked expert tensor's count axis differs from `expert_count`.
    ExpertCount { declared: usize, from_tensor: usize },
    /// The metadata says there is a shared expert and no `_shexp` tensor exists, or vice versa.
    SharedExpert { declared: usize, tensors_present: bool },
    /// More experts routed to than exist.
    TopKExceedsCount { used: usize, count: usize },
}

impl RoutingDisagreement {
    pub fn explain(&self) -> String {
        match self {
            RoutingDisagreement::ExpertCount { declared, from_tensor } => format!(
                "metadata says {declared} experts and the stacked tensor has {from_tensor} — routing \
would index past the end or leave experts unreachable, and neither shows as an error"),
            RoutingDisagreement::SharedExpert { declared, tensors_present } => format!(
                "metadata says {declared} shared expert(s) and the checkpoint {} ffn_*_shexp tensors. \
A shared expert is ALWAYS active, so a missing one silently drops a contribution from every token, \
and an unexpected one is weights nobody reads",
                if *tensors_present { "HAS" } else { "has NO" }),
            RoutingDisagreement::TopKExceedsCount { used, count } => format!(
                "routing to {used} of {count} experts — the top-k cannot exceed the pool"),
        }
    }
}

/// Check routing metadata against what the file actually contains.
pub fn check_routing(
    expert_count: usize, expert_used: usize, expert_shared: usize,
    stacked_expert_axis: Option<usize>, shexp_tensors_present: bool,
) -> Vec<RoutingDisagreement> {
    let mut v = Vec::new();
    if let Some(n) = stacked_expert_axis {
        if n != expert_count {
            v.push(RoutingDisagreement::ExpertCount { declared: expert_count, from_tensor: n });
        }
    }
    if (expert_shared > 0) != shexp_tensors_present {
        v.push(RoutingDisagreement::SharedExpert {
            declared: expert_shared, tensors_present: shexp_tensors_present });
    }
    if expert_count > 0 && expert_used > expert_count {
        v.push(RoutingDisagreement::TopKExceedsCount { used: expert_used, count: expert_count });
    }
    v
}

#[cfg(test)]
mod routing_check_tests {
    use super::*;

    #[test]
    fn scouts_reported_configuration_is_self_consistent_IF_the_tensors_agree() {
        // 16 experts, 1 active, 1 shared — all three reported as (default) in a real load, so all
        // three are guesses. This says what the file would have to contain for them to be right.
        assert!(check_routing(16, 1, 1, Some(16), true).is_empty());
    }

    #[test]
    fn a_SHARED_expert_claimed_with_no_tensors_is_caught() {
        // The check that matters most for Scout: `shared experts 1 (default)` comes from an arch flag
        // rescuing a literal 0, not from the file. A shared expert is ALWAYS active, so claiming one
        // that does not exist drops a contribution from every single token — quietly.
        let d = check_routing(16, 1, 1, Some(16), false);
        assert_eq!(d.len(), 1);
        assert!(d[0].explain().contains("silently drops a contribution from every token"));
    }

    #[test]
    fn an_unexpected_shared_expert_is_also_caught() {
        // The other direction: tensors present and metadata saying none means weights nobody reads,
        // and a model quietly missing part of its FFN.
        let d = check_routing(128, 8, 0, Some(128), true);
        assert_eq!(d.len(), 1, "{d:?}");
    }

    #[test]
    fn an_expert_COUNT_mismatch_is_caught_from_the_stacked_tensor() {
        // Routing would index past the end or leave experts unreachable — neither is an error.
        let d = check_routing(128, 8, 0, Some(64), false);
        assert!(matches!(d[0], RoutingDisagreement::ExpertCount { from_tensor: 64, .. }));
    }

    #[test]
    fn a_top_k_larger_than_the_pool_is_impossible() {
        let d = check_routing(8, 16, 0, Some(8), false);
        assert!(d.iter().any(|x| matches!(x, RoutingDisagreement::TopKExceedsCount { .. })));
    }

    #[test]
    fn top_k_within_the_pool_cannot_be_checked_AT_ALL() {
        // Worth stating: nothing in a checkpoint constrains how many experts are routed to. 8 of 128
        // and 2 of 128 both produce a file that looks identical, so a wrong expert_used_count is
        // invisible to every check and shows only as worse output.
        assert!(check_routing(128, 8, 0, Some(128), false).is_empty());
        assert!(check_routing(128, 2, 0, Some(128), false).is_empty());
    }

    #[test]
    fn a_dense_model_is_not_penalised_for_having_no_expert_tensors() {
        assert!(check_routing(0, 0, 0, None, false).is_empty());
    }
}
