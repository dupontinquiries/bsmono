//! enginestrip.rs — turning what the engine measures into what the strip says.
//!
//! `logscope::machinery::Concern` describes the engine's state and nothing produced it. The numbers
//! all exist — `moe_stream_stats`, `moe_pin_counts`, `moe_prefetch_stats`, the phase breakdown — and
//! have been going to stderr, which is why they have been read by hand from logs rather than seen.
//!
//! ## The judgement is here, not in the renderer
//!
//! Deciding that 99.9% sync matters more than a 46% hit rate is a claim about what an operator can do
//! next, and it belongs somewhere testable. A renderer that made that call would be untestable and
//! would make it differently in the strip and the landmark.
//!
//! ## Thresholds are stated, not implied
//!
//! Every cutoff below is a number someone will want to argue with, so each one says what it means
//! rather than sitting as a bare literal.

use logscope::machinery::Concern;

/// Above this share of the step spent waiting, nothing else is worth reporting: a cache does not
/// shorten a stall, and every other lever acts on what is left.
const SYNC_DOMINATES_PCT: u32 = 60;

/// Below this hit rate the stream is doing most of the work. Note it is compared against the
/// PINNED SHARE rather than a fixed number — a balanced MoE router puts the ceiling near
/// pinned/total, so 46% with 6 of 16 pinned is near the limit and 46% with 12 of 16 is a fault.
/// **Calibrated on one data point, which is worth saying out loud.** Scout measured 46.0% against a
/// 37.5% floor — 8.5 points of skew found by the policy on a router trained by a load-balancing loss
/// to have none. I first set this to 0.15, which would have flagged the only run I have evidence is
/// working, and the test caught it.
///
/// 0.05 means: complain when the policy finds less than five points of skew over what pinning alone
/// guarantees. A policy finding nothing at all is a real fault; one finding 8.5 is near the ceiling.
/// Whether the true line sits at 3 or 8 is unknown and needs more models than one.
const CHURN_MARGIN: f64 = 0.05;

/// What the engine currently has to say for itself.
///
/// `sync_pct` and `tok_s` come from the phase breakdown; the rest from the MoE counters. All are
/// already computed — this only decides which of them a person should be told about.
pub fn concerns(sync_pct: u32, tok_s: f32, hits: u64, fills: u64,
                pinned: usize, total: usize, clamped: Option<(u32, u32)>) -> Vec<Concern> {
    let mut v = Vec::new();

    if sync_pct >= SYNC_DOMINATES_PCT {
        v.push(Concern::SyncBound { pct: sync_pct });
    }
    if let Some((asked, held)) = clamped {
        // Only when the clamp actually bit. Reporting a budget that was never reduced trains people
        // to ignore the line.
        if held < asked { v.push(Concern::MemoryClamped { asked_gib: asked, held_gib: held }); }
    }
    let seen = hits + fills;
    if seen > 0 && total > 0 {
        let rate = hits as f64 / seen as f64;
        let floor = pinned as f64 / total as f64;
        // Against the FLOOR, not a constant: a balanced router caps the achievable rate near
        // pinned/total, so the question is whether the policy is finding skew, not whether the number
        // is large.
        if rate < floor + CHURN_MARGIN {
            v.push(Concern::ExpertChurn {
                hit_pct: (rate * 100.0).round() as u32,
                pinned: pinned as u32, total: total as u32,
            });
        }
    }
    let _ = tok_s;
    v
}

/// Build the strip, healthy line included when nothing else fires.
pub fn strip(sync_pct: u32, tok_s: f32, hits: u64, fills: u64, pinned: usize, total: usize,
             clamped: Option<(u32, u32)>) -> logscope::machinery::Strip {
    logscope::machinery::Strip::new(
        concerns(sync_pct, tok_s, hits, fills, pinned, total, clamped), tok_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real Scout numbers from a full run.
    const SCOUT: (u32, f32, u64, u64, usize, usize) = (99, 1.4, 2085, 2451, 6, 16);

    #[test]
    fn the_real_scout_run_reports_SYNC_first() {
        let (s, t, h, f, p, n) = SCOUT;
        let st = strip(s, t, h, f, p, n, Some((48, 31)));
        assert!(matches!(st.worst(), Concern::SyncBound { pct: 99 }), "{:?}", st.worst());
    }

    #[test]
    fn a_46_pct_hit_rate_with_SIX_pinned_is_not_flagged() {
        // 6/16 is a 37.5% floor and a balanced router caps near it, so 46% is the policy finding real
        // skew. Flagging it would send someone to fix something that is working — which is exactly
        // what I did before checking how MoE routers are trained.
        let c = concerns(10, 1.4, 2085, 2451, 6, 16, None);
        assert!(!c.iter().any(|x| matches!(x, Concern::ExpertChurn { .. })), "{c:?}");
    }

    #[test]
    fn a_policy_finding_NO_skew_is_flagged_at_the_same_pin_count() {
        // 6 of 16 pinned and a hit rate AT the 37.5% floor: the policy is finding nothing, which is
        // a real fault and distinct from the pin set being small. The number alone cannot separate
        // those two, which is why the floor is the comparison rather than a constant.
        let c = concerns(10, 1.4, 375, 625, 6, 16, None);
        assert!(c.iter().any(|x| matches!(x, Concern::ExpertChurn { .. })), "{c:?}");
    }

    #[test]
    fn a_budget_that_was_NOT_reduced_is_not_reported() {
        // Reporting a clamp that did not bite trains people to ignore the line.
        let c = concerns(10, 12.0, 900, 100, 6, 16, Some((31, 31)));
        assert!(!c.iter().any(|x| matches!(x, Concern::MemoryClamped { .. })), "{c:?}");
    }

    #[test]
    fn a_dense_model_with_no_expert_traffic_reports_no_churn() {
        // 0 hits and 0 fills is a model without experts, not a cache at 0%.
        let c = concerns(10, 20.0, 0, 0, 0, 0, None);
        assert!(c.is_empty(), "{c:?}");
    }

    #[test]
    fn a_healthy_engine_yields_a_strip_that_still_SAYS_something() {
        let mut st = strip(5, 24.0, 950, 50, 12, 16, None);
        assert!(st.next().contains("24.0 tok/s"));
    }

    #[test]
    fn moderate_sync_is_not_reported_as_domination() {
        // 30% waiting is normal and flagging it would bury the 99% case in noise.
        let c = concerns(30, 8.0, 950, 50, 12, 16, None);
        assert!(!c.iter().any(|x| matches!(x, Concern::SyncBound { .. })), "{c:?}");
    }
}
