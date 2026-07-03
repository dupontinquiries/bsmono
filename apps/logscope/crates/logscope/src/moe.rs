//! moe.rs — mixture-of-experts routing: the pure gate/dispatch/combine bookkeeping that lets the engine
//! run MoE FFNs (Mixtral, DeepSeek-V2/V3, Qwen-MoE).
//!
//! Per token, a gate scores the experts; the router picks the top-`k`, normalizes their combine weights,
//! and the batch is permuted so each expert processes its assigned tokens in one grouped GEMM (the GPU
//! seam). Afterwards the per-expert outputs are scattered back and summed by the gate weights. The
//! selection, weighting, the per-expert token lists, and the weighted combine are pure and unit-tested.
#![allow(dead_code)]

/// How the gate logits become combine weights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateNorm {
    Softmax, // softmax over the chosen experts (Mixtral, DeepSeek-V2)
    Sigmoid, // per-expert sigmoid then (optionally) normalize (DeepSeek-V3)
}

/// A chosen expert and its combine weight for one token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expert { pub id: u32, pub weight: f32 }

/// Pick the top-`k` experts for one token and compute their combine weights. `renorm` rescales the chosen
/// weights to sum to 1 (a no-op for `Softmax`, which already does; the usual choice for `Sigmoid`).
pub fn route_token(logits: &[f32], k: usize, norm: GateNorm, renorm: bool) -> Vec<Expert> {
    if logits.is_empty() { return Vec::new(); }
    let k = k.clamp(1, logits.len());
    // indices of the k largest logits (desc by logit, ties broken by lower index for determinism)
    let mut idx: Vec<usize> = (0..logits.len()).collect();
    idx.sort_by(|&a, &b| logits[b].partial_cmp(&logits[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    let sel = &idx[..k];

    let mut w: Vec<f32> = match norm {
        GateNorm::Softmax => {
            // softmax over the selected logits == softmax-over-all then renormalize the subset
            let m = sel.iter().map(|&i| logits[i]).fold(f32::NEG_INFINITY, f32::max);
            let exps: Vec<f32> = sel.iter().map(|&i| (logits[i] - m).exp()).collect();
            let s: f32 = exps.iter().sum();
            exps.iter().map(|&e| e / s).collect()
        }
        GateNorm::Sigmoid => sel.iter().map(|&i| 1.0 / (1.0 + (-logits[i]).exp())).collect(),
    };
    if renorm {
        let s: f32 = w.iter().sum();
        if s > 0.0 { for x in &mut w { *x /= s; } }
    }
    sel.iter().zip(w).map(|(&i, weight)| Expert { id: i as u32, weight }).collect()
}

/// A whole batch's routing: each token's chosen experts, and the inverse map expert → its token indices
/// (the grouping the per-expert GEMM consumes).
#[derive(Debug)]
pub struct Plan { pub routes: Vec<Vec<Expert>>, pub per_expert: Vec<Vec<u32>> }

/// Route a batch. `token_logits[t]` is token `t`'s gate logits over `n_expert` experts.
pub fn plan(token_logits: &[&[f32]], n_expert: usize, k: usize, norm: GateNorm, renorm: bool) -> Plan {
    let mut routes = Vec::with_capacity(token_logits.len());
    let mut per_expert = vec![Vec::new(); n_expert];
    for (t, logits) in token_logits.iter().enumerate() {
        let r = route_token(logits, k, norm, renorm);
        for e in &r { if (e.id as usize) < n_expert { per_expert[e.id as usize].push(t as u32); } }
        routes.push(r);
    }
    Plan { routes, per_expert }
}

impl Plan {
    /// Tokens assigned to each expert — the load distribution (uneven load is the MoE scheduling problem).
    pub fn expert_load(&self) -> Vec<usize> { self.per_expert.iter().map(|v| v.len()).collect() }

    /// Combine per-(token, expert) scalar outputs by the gate weights, one result per token. The real engine
    /// does this over hidden vectors on the GPU; the weighting is identical, so this exercises that math.
    pub fn combine_scalar(&self, expert_out: impl Fn(u32, u32) -> f32) -> Vec<f32> {
        self.routes.iter().enumerate()
            .map(|(t, experts)| experts.iter().map(|e| e.weight * expert_out(t as u32, e.id)).sum())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool { (a - b).abs() < 1e-4 }

    #[test]
    fn top_k_softmax_picks_largest_and_weights_sum_to_one() {
        let r = route_token(&[1.0, 3.0, 2.0, 0.0], 2, GateNorm::Softmax, true);
        assert_eq!(r.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1, 2]); // logits 3 and 2
        let s: f32 = r.iter().map(|e| e.weight).sum();
        assert!(approx(s, 1.0));
        assert!(r[0].weight > r[1].weight, "the higher logit gets the larger weight");
        // softmax([3,2]) = [e/(e+1), 1/(e+1)] ≈ [0.7311, 0.2689]
        assert!(approx(r[0].weight, 0.7310586));
    }

    #[test]
    fn k_at_least_one_and_capped_at_n() {
        assert_eq!(route_token(&[0.1, 0.2, 0.3], 9, GateNorm::Softmax, true).len(), 3);
        assert_eq!(route_token(&[0.1, 0.2, 0.3], 0, GateNorm::Softmax, true).len(), 1);
        assert!(route_token(&[], 2, GateNorm::Softmax, true).is_empty());
    }

    #[test]
    fn sigmoid_gating_normalizes_when_asked() {
        let r = route_token(&[2.0, 1.0, -1.0], 2, GateNorm::Sigmoid, true);
        assert_eq!(r.iter().map(|e| e.id).collect::<Vec<_>>(), vec![0, 1]);
        assert!(approx(r.iter().map(|e| e.weight).sum(), 1.0));
        // without renorm the weights are the raw sigmoids (don't sum to 1)
        let raw = route_token(&[2.0, 1.0, -1.0], 2, GateNorm::Sigmoid, false);
        assert!(!approx(raw.iter().map(|e| e.weight).sum(), 1.0));
    }

    #[test]
    fn plan_groups_tokens_by_expert() {
        // 3 tokens over 4 experts, top-1: clear argmaxes 2, 0, 2
        let l0 = [0.0, 0.0, 9.0, 0.0];
        let l1 = [9.0, 0.0, 0.0, 0.0];
        let l2 = [0.0, 0.0, 9.0, 0.0];
        let p = plan(&[&l0, &l1, &l2], 4, 1, GateNorm::Softmax, true);
        assert_eq!(p.per_expert[2], vec![0, 2]);
        assert_eq!(p.per_expert[0], vec![1]);
        assert_eq!(p.expert_load(), vec![1, 0, 2, 0]);
    }

    #[test]
    fn combine_is_the_gate_weighted_sum() {
        // top-2 over 2 experts ⇒ both chosen; weights from softmax([1,0])
        let p = plan(&[&[1.0, 0.0]], 2, 2, GateNorm::Softmax, true);
        let w0 = p.routes[0].iter().find(|e| e.id == 0).unwrap().weight;
        let w1 = p.routes[0].iter().find(|e| e.id == 1).unwrap().weight;
        // expert e returns scalar (e+1) for the token; combine = w0*1 + w1*2
        let out = p.combine_scalar(|_t, e| (e + 1) as f32);
        assert!(approx(out[0], w0 * 1.0 + w1 * 2.0));
    }
}
