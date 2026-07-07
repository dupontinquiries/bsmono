//! `sediment_settle` — suspended sediment settling to the bed.
//!
//! Worker-03 / WORLD pure core, round 24. How much suspended sediment drops out
//! of the water column in a step: still water settles a `rate` fraction, while
//! fast flow keeps material in suspension (little settles). Never deposits more
//! than is suspended. Follow-on to `sediment_deposit` (R19) and `erosion_advect`
//! (R23) — those move/route load; this drops it when the water slows.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FLOW_K: f32 = 1.0; // flow at which the still-factor is ~half

/// Amount of `suspended` sediment deposited given `flow` speed and settling
/// `rate`. Maximal in still water, ~0 in fast flow, never exceeds `suspended`.
pub fn settle(suspended: f32, flow: f32, rate: f32) -> f32 {
    let s = suspended.max(0.0);
    let f = flow.max(0.0);
    let r = clamp01(rate);
    let still = 1.0 / (1.0 + f * FLOW_K); // 1 still → 0 fast
    let dep = s * r * still;
    if dep > s {
        s
    } else {
        dep
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn still_water_settles() {
        assert!(settle(10.0, 0.0, 0.5) > 0.0);
    }

    #[test]
    fn fast_flow_settles_little() {
        assert!(settle(10.0, 50.0, 0.5) < settle(10.0, 0.5, 0.5));
    }

    #[test]
    fn rate_scales() {
        assert!(settle(10.0, 1.0, 0.8) > settle(10.0, 1.0, 0.2));
    }

    #[test]
    fn never_exceeds_suspended() {
        for si in 0..20 {
            for fi in 0..20 {
                let s = si as f32;
                let dep = settle(s, fi as f32 * 0.5, 1.0);
                assert!(dep <= s + 1e-6);
            }
        }
    }

    #[test]
    fn bounded() {
        let dep = settle(7.0, 2.0, 0.6);
        assert!(dep >= 0.0 && dep <= 7.0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(settle(5.0, 1.5, 0.5), settle(5.0, 1.5, 0.5));
    }
}
