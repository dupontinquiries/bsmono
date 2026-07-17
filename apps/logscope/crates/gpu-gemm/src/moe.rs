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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoeConfig {
    pub n_routed: u32,  // total routed experts (N)
    pub n_used: u32,    // top-k activated per token (k)
    pub n_shared: u32,  // always-on shared experts (Llama4/DeepSeek); 0 for Qwen-style
    pub norm: RouteNorm,
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
            // normalize the selected sigmoid gates to sum to 1
            let raw: Vec<f32> = idx.iter().map(|&i| sigmoid(logits[i])).collect();
            let s: f32 = raw.iter().sum();
            idx.iter().zip(raw).map(|(&i, r)| Selected { expert: i as u32, weight: if s > 0.0 { r / s } else { 0.0 } }).collect()
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool { (a - b).abs() < 1e-5 }

    #[test]
    fn softmax_route_picks_top_k_and_normalizes() {
        // 4 experts, top-2. logits favor experts 2 and 0.
        let cfg = MoeConfig { n_routed: 4, n_used: 2, n_shared: 0, norm: RouteNorm::Softmax };
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
        let cfg = MoeConfig { n_routed: 5, n_used: 2, n_shared: 0, norm: RouteNorm::Softmax };
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
        let cfg = MoeConfig { n_routed: 4, n_used: 2, n_shared: 1, norm: RouteNorm::Sigmoid };
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
        let cfg = MoeConfig { n_routed: 3, n_used: 8, n_shared: 0, norm: RouteNorm::Softmax };
        let sel = route(&[0.1, 0.2, 0.3], &cfg);
        assert_eq!(sel.len(), 3, "k clamps to the number of experts");
        assert_eq!(expert_row_offset(0, 4096), 0);
        assert_eq!(expert_row_offset(5, 4096), 5 * 4096);
    }

    #[test]
    fn dense_config_is_not_moe() {
        let dense = MoeConfig { n_routed: 0, n_used: 0, n_shared: 0, norm: RouteNorm::Softmax };
        assert!(!dense.is_moe());
        assert!(route(&[], &dense).is_empty());
    }
}
