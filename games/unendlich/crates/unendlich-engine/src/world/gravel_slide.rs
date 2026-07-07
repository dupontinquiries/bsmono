//! `gravel_slide` — scree (loose rock) fraction on a slope past repose.
//!
//! Worker-03 / WORLD pure core, round 21. Below the angle of repose a slope is
//! stable bedrock; past it, loose scree accumulates, ramping to a full cover by
//! roughly twice the repose angle. Normalizing the excess by the repose angle
//! means a higher repose (more stable material) tolerates a steeper slope before
//! scree appears. Pairs with `erosion_settle` (R20).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const EPS: f32 = 1e-6;

/// Scree fraction in [0,1] for a `slope` against the `repose` angle. Zero at or
/// below repose; rises with steepness; full (1) by about `2·repose`.
pub fn scree(slope: f32, repose: f32) -> f32 {
    let s = slope.max(0.0);
    let r = repose.max(0.0);
    if s <= r {
        return 0.0;
    }
    clamp01((s - r) / r.max(EPS))
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_repose_is_zero() {
        assert_eq!(scree(0.5, 1.0), 0.0);
        assert_eq!(scree(1.0, 1.0), 0.0); // exactly at repose is stable
    }

    #[test]
    fn steeper_is_more_scree() {
        assert!(scree(1.5, 1.0) < scree(1.8, 1.0));
    }

    #[test]
    fn clamped_at_one() {
        assert_eq!(scree(100.0, 1.0), 1.0);
        assert!((scree(2.0, 1.0) - 1.0).abs() < 1e-6); // 2× repose → full
    }

    #[test]
    fn repose_scales() {
        // Same slope, higher repose ⇒ less scree (more tolerant material).
        assert!(scree(3.0, 1.0) > scree(3.0, 2.0));
    }

    #[test]
    fn bounded() {
        for si in 0..40 {
            for ri in 1..10 {
                let v = scree(si as f32 * 0.25, ri as f32 * 0.5);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(scree(1.7, 1.0), scree(1.7, 1.0));
    }
}
