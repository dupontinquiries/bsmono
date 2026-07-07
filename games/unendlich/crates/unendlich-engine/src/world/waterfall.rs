//! `waterfall` — intensity where flow drops over a ledge.
//!
//! Worker-03 / WORLD-FX pure core, round 21. A waterfall needs both water and a
//! drop: intensity rises with flow volume and with the height of the ledge, and
//! is zero if either is absent. Saturating in each so a huge river over a small
//! lip and a trickle over a cliff both read as modest. Pairs with
//! `flow_accumulation` (R8) / `cliff_detect` (R18).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FLOW_K: f32 = 10.0; // flow at which intensity is ~half
const DROP_K: f32 = 8.0; // drop at which intensity is ~half

/// Waterfall intensity in [0,1] from `flow` volume and ledge `drop_height`.
/// Zero if either is zero; rises (saturating) with both.
pub fn waterfall(flow: f32, drop_height: f32) -> f32 {
    let f = flow.max(0.0);
    let d = drop_height.max(0.0);
    let fi = f / (f + FLOW_K); // [0,1)
    let di = d / (d + DROP_K); // [0,1)
    clamp01(fi * di)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_flow_and_drop_is_strong() {
        assert!(waterfall(1000.0, 1000.0) > 0.9);
    }

    #[test]
    fn no_drop_is_zero() {
        assert_eq!(waterfall(500.0, 0.0), 0.0);
    }

    #[test]
    fn no_flow_is_zero() {
        assert_eq!(waterfall(0.0, 500.0), 0.0);
    }

    #[test]
    fn monotone_in_both() {
        assert!(waterfall(50.0, 10.0) > waterfall(5.0, 10.0)); // flow
        assert!(waterfall(10.0, 50.0) > waterfall(10.0, 5.0)); // drop
    }

    #[test]
    fn bounded() {
        for fi in 0..20 {
            for di in 0..20 {
                let v = waterfall(fi as f32 * 10.0, di as f32 * 10.0);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(waterfall(30.0, 12.0), waterfall(30.0, 12.0));
    }
}
