//! `lava_flow` — lava spread rate down a slope.
//!
//! Worker-03 / WORLD-FX pure core, round 24. How much of a cell's molten lava
//! creeps downhill in a step: it rises with slope (gravity), falls with
//! viscosity (thick lava barely moves), is proportional to the amount present,
//! and is zero on flat ground or with no lava. The spread can never exceed the
//! lava available. Pairs with surface heat / `thermal_erosion` (R16).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const SLOPE_K: f32 = 1.0; // slope at which the slope term is ~half
const VISC_K: f32 = 1.0; // viscosity sensitivity

/// Lava spread amount for a cell with local `slope`, lava `viscosity`, and lava
/// `amount`. Zero when flat or empty; rises with slope; falls with viscosity;
/// never exceeds `amount`.
pub fn flow(slope: f32, viscosity: f32, amount: f32) -> f32 {
    let s = slope.max(0.0);
    let v = viscosity.max(0.0);
    let a = amount.max(0.0);
    let st = s / (s + SLOPE_K); // 0 at flat → 1 steep
    let vt = 1.0 / (1.0 + v * VISC_K); // 1 at thin → 0 viscous
    let f = a * st * vt;
    if f > a {
        a
    } else {
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_barely_flows() {
        assert_eq!(flow(0.0, 1.0, 10.0), 0.0);
    }

    #[test]
    fn steeper_flows_more() {
        assert!(flow(5.0, 1.0, 10.0) > flow(0.5, 1.0, 10.0));
    }

    #[test]
    fn viscous_flows_less() {
        assert!(flow(2.0, 0.1, 10.0) > flow(2.0, 5.0, 10.0));
    }

    #[test]
    fn no_amount_no_flow() {
        assert_eq!(flow(5.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn bounded_by_amount() {
        for si in 0..20 {
            for vi in 0..20 {
                for ai in 0..10 {
                    let a = ai as f32;
                    let f = flow(si as f32 * 0.5, vi as f32 * 0.5, a);
                    assert!(f >= 0.0 && f <= a + 1e-6);
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(flow(2.0, 1.5, 8.0), flow(2.0, 1.5, 8.0));
    }
}
