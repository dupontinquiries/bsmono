//! fanplan.rs — how wide to fan out, given the headroom actually present.
//!
//! A 3B model on a 192 GiB machine leaves enormous room, and the coordinator was fanning as if it did
//! not. But "use the headroom" splits into two very different situations, and conflating them wastes
//! most of it:
//!
//! * **Same model, several prompts.** Needs ONE resident copy. `Gpu::decode_batched` already runs N
//!   sessions through it, sharing every weight read — the marginal cost of a lane is a KV cache and some
//!   activations, not another copy of the model. This is nearly free and is the case people usually mean
//!   by "fan out the same task".
//! * **Different models, same prompt.** Genuinely needs each model resident. This is what
//!   `codecompare` compares, and it is bounded by memory in the obvious way.
//!
//! Loading eight copies of one 3B model to answer eight prompts would use 8x the memory for the same
//! throughput a batch gets from one. So the plan distinguishes them, and the width for the cheap case is
//! bounded by KV growth rather than by weights.

/// What kind of fan is being planned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FanKind {
    /// One model, many prompts — batched through a single resident copy.
    SameModel,
    /// Several models, one prompt — each must be resident.
    DifferentModels,
}

#[derive(Clone, PartialEq, Debug)]
pub struct FanPlan {
    pub kind: FanKind,
    /// How many lanes to run at once.
    pub width: usize,
    /// Resident GiB the plan expects to consume.
    pub gib: f32,
    /// Why this width, in one line for the log.
    pub why: String,
}

/// Bytes of KV cache per sequence, in GiB — the marginal cost of a batched lane.
///
/// This is what bounds the cheap case. Weights are shared; KV is not.
pub fn kv_gib_per_seq(n_layers: usize, kv_dim: usize, max_seq: usize, bytes_per_elem: usize) -> f32 {
    // K and V, both [max_seq x kv_dim] per layer
    let b = 2.0 * n_layers as f64 * kv_dim as f64 * max_seq as f64 * bytes_per_elem as f64;
    (b / (1u64 << 30) as f64) as f32
}

/// Plan a same-model fan: one resident copy, N batched sessions.
///
/// Width is bounded by KV growth and by a hard cap, NOT by the weight size — that is the whole point.
/// The cap exists because batching stops paying once the GPU is saturated: past that, lanes queue behind
/// each other and the only thing that grows is latency for every one of them.
pub fn plan_same_model(model_gib: f32, kv_gib_per_seq: f32, budget_gib: f32, want: usize) -> FanPlan {
    let want = want.max(1);
    if model_gib > budget_gib {
        return FanPlan { kind: FanKind::SameModel, width: 0, gib: 0.0,
            why: format!("the model alone needs {model_gib:.1} GiB of a {budget_gib:.1} GiB budget") };
    }
    let spare = budget_gib - model_gib;
    let by_kv = if kv_gib_per_seq > 0.0 { (spare / kv_gib_per_seq).floor() as usize } else { want };
    const SATURATION_CAP: usize = 8;
    let width = want.min(by_kv.max(1)).min(SATURATION_CAP);
    let why = if width < want && by_kv < want {
        format!("KV cache is {kv_gib_per_seq:.2} GiB/lane and only {spare:.1} GiB is spare")
    } else if width < want {
        format!("capped at {SATURATION_CAP}: past that, lanes queue on the GPU and only latency grows")
    } else {
        format!("one resident copy ({model_gib:.1} GiB) shared by {width} batched lanes")
    };
    FanPlan { kind: FanKind::SameModel, width, gib: model_gib + width as f32 * kv_gib_per_seq, why }
}

/// Plan a different-models fan: each model resident, largest first.
///
/// Returns the models that fit, in the order given, and stops at the budget. Takes them in the caller's
/// order rather than sorting: the caller ranked them, and silently preferring small ones would change
/// which models get compared.
pub fn plan_different_models(sizes_gib: &[(String, f32)], budget_gib: f32) -> (FanPlan, Vec<String>) {
    let mut used = 0.0f32;
    let mut chosen = Vec::new();
    for (name, g) in sizes_gib {
        if used + g <= budget_gib { used += g; chosen.push(name.clone()); }
    }
    let why = if chosen.len() < sizes_gib.len() {
        format!("{} of {} models fit in {budget_gib:.1} GiB", chosen.len(), sizes_gib.len())
    } else {
        format!("all {} models fit ({used:.1} of {budget_gib:.1} GiB)", chosen.len())
    };
    (FanPlan { kind: FanKind::DifferentModels, width: chosen.len(), gib: used, why }, chosen)
}

/// Room to add BATCHED LANES of the model already loaded.
///
/// Cheap: weights are shared, so this only needs space for another KV cache and some activations.
/// Scout at 60 GiB in 95 GiB has this in abundance even though it could never hold a second copy.
pub fn has_batch_headroom(model_gib: f32, kv_gib_per_seq: f32, budget_gib: f32) -> bool {
    budget_gib - model_gib > kv_gib_per_seq.max(0.1) * 2.0
}

/// Room for ANOTHER MODEL alongside this one.
///
/// Expensive, and a different question entirely — which is the mistake this module exists to stop. The
/// first version of this function was one predicate for both, and it reported Scout-in-95-GiB as having
/// headroom because 35 GiB is genuinely a lot of slack; it just is not a second 60 GiB model. Requires
/// real slack rather than an exact fit, because a fan that just fits will OOM the moment a context
/// grows, and an OOM mid-run is worse than never having fanned.
pub fn has_model_headroom(model_gib: f32, budget_gib: f32) -> bool {
    budget_gib > model_gib * 2.0 + 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_same_model_fan_does_not_pay_for_extra_copies() {
        // The point of the whole module. Eight lanes of a 2 GiB model cost 2 GiB plus KV, not 16 GiB.
        let p = plan_same_model(2.0, 0.25, 180.0, 8);
        assert_eq!(p.width, 8);
        assert!(p.gib < 5.0, "one copy plus KV, got {:.1} GiB", p.gib);
        assert!(p.why.contains("one resident copy"));
    }

    #[test]
    fn same_model_width_is_bounded_by_KV_not_by_weights() {
        // Weights are shared, KV is not — so KV is what actually limits the width, and the message has
        // to say that or someone will go looking for a bigger model budget.
        let p = plan_same_model(2.0, 4.0, 10.0, 8);   // 8 GiB spare / 4 per lane = 2
        assert_eq!(p.width, 2);
        assert!(p.why.contains("KV cache"), "{}", p.why);
    }

    #[test]
    fn width_is_capped_even_with_infinite_memory() {
        // Batching stops paying once the GPU saturates; past that, lanes queue behind each other and the
        // only thing that grows is latency for every one of them.
        let p = plan_same_model(2.0, 0.01, 100_000.0, 64);
        assert_eq!(p.width, 8);
        assert!(p.why.contains("only latency grows"));
    }

    #[test]
    fn a_model_that_does_not_fit_yields_width_zero_not_one() {
        // Returning 1 would look like "we fanned" and then OOM.
        let p = plan_same_model(200.0, 0.1, 100.0, 4);
        assert_eq!(p.width, 0);
        assert!(p.why.contains("the model alone needs"));
    }

    #[test]
    fn different_models_keep_the_callers_order() {
        // The caller ranked them; silently preferring the small ones would change WHICH models get
        // compared, which is the one thing a comparison must not do behind your back.
        let m: Vec<(String, f32)> = vec![("big".into(), 60.0), ("mid".into(), 30.0), ("small".into(), 5.0)];
        let (p, chosen) = plan_different_models(&m, 95.0);
        assert_eq!(chosen, vec!["big", "mid", "small"]);
        assert_eq!(p.width, 3);
        // and when the budget bites, later entries are dropped rather than the list re-sorted
        let (p2, c2) = plan_different_models(&m, 65.0);
        assert_eq!(c2, vec!["big", "small"], "mid did not fit after big; small still did");
        assert_eq!(p2.width, 2);
        assert!(p2.why.contains("2 of 3"));
    }

    #[test]
    fn batch_headroom_and_model_headroom_are_DIFFERENT_questions() {
        // The module's own thesis, applied to itself. The first version was one predicate for both and
        // got Scout wrong: 60 GiB in 95 GiB has 35 GiB of slack — plenty for more batched lanes, nowhere
        // near a second copy. Answering "is there headroom" with one number hides exactly the
        // distinction that decides whether a fan is free or impossible.
        assert!(has_batch_headroom(60.0, 1.0, 95.0), "35 GiB is a lot of KV caches");
        assert!(!has_model_headroom(60.0, 95.0), "but it is not a second 60 GiB model");

        // A small model on a big machine has both.
        assert!(has_batch_headroom(3.0, 0.2, 192.0));
        assert!(has_model_headroom(3.0, 192.0), "a 3B model on 192 GiB can hold many peers");

        // An exact fit has neither: a fan that just fits OOMs when a context grows.
        assert!(!has_model_headroom(50.0, 52.0));
        assert!(!has_batch_headroom(50.0, 4.0, 52.0));
    }

    #[test]
    fn kv_cost_scales_the_way_a_reader_expects() {
        // Doubling layers, context or width doubles the cache; this is the number the width bound rests
        // on, so it has to be right.
        let a = kv_gib_per_seq(32, 1024, 4096, 4);
        let b = kv_gib_per_seq(64, 1024, 4096, 4);
        assert!((b / a - 2.0).abs() < 1e-3);
        let c = kv_gib_per_seq(32, 1024, 8192, 4);
        assert!((c / a - 2.0).abs() < 1e-3);
        // q8 KV halves it
        let d = kv_gib_per_seq(32, 1024, 4096, 2);
        assert!((a / d - 2.0).abs() < 1e-3);
    }
}
