//! exposure_adapt.rs — pure eye auto-exposure adaptation.
//!
//! Moves the current exposure (EV) toward the target implied by scene luminance,
//! a bounded fraction per step (rate = speed·dt clamped to [0,1]). Brighter
//! scenes pull EV down. Pure, deterministic, panic-free (R1/R11/R21).

/// Target EV for a scene luminance: brighter scene ⇒ lower EV.
pub fn target_ev(scene_lum: f32) -> f32 {
    -scene_lum.max(1e-4).log2()
}

/// Adapt `cur_ev` one step toward the target implied by `scene_lum`, over `dt`
/// at adaptation `speed`. The step is a clamped `[0,1]` fraction of the gap, so
/// it never overshoots the target.
pub fn adapt(cur_ev: f32, scene_lum: f32, dt: f32, speed: f32) -> f32 {
    let target = target_ev(scene_lum);
    let rate = (speed.max(0.0) * dt.max(0.0)).max(0.0).min(1.0);
    cur_ev + (target - cur_ev) * rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brighter_scene_lowers_exposure() {
        let bright = adapt(0.0, 4.0, 1.0, 1.0);
        let dim = adapt(0.0, 0.25, 1.0, 1.0);
        assert!(bright < dim, "brighter scene should give lower EV ({} !< {})", bright, dim);
    }

    #[test]
    fn converges_to_target() {
        let lum = 2.0f32;
        let target = target_ev(lum);
        let mut ev = 10.0f32;
        for _ in 0..500 {
            ev = adapt(ev, lum, 0.1, 2.0);
        }
        assert!((ev - target).abs() < 1e-3, "did not converge: {} vs {}", ev, target);
    }

    #[test]
    fn dt_and_speed_scale_rate() {
        let slow = adapt(0.0, 4.0, 0.1, 1.0);
        let fast = adapt(0.0, 4.0, 1.0, 1.0);
        // both move toward a negative target; faster moves farther
        assert!(fast.abs() > slow.abs(), "higher dt should move farther");
        let s1 = adapt(0.0, 4.0, 0.1, 1.0);
        let s2 = adapt(0.0, 4.0, 0.1, 5.0);
        assert!(s2.abs() > s1.abs(), "higher speed should move farther");
    }

    #[test]
    fn stable_at_target() {
        let lum = 1.0f32;
        let target = target_ev(lum);
        assert!((adapt(target, lum, 1.0, 1.0) - target).abs() < 1e-5, "should be stable at target");
    }

    #[test]
    fn bounded_step_no_overshoot() {
        let mut seed = 0x2507_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let cur = lcg() * 20.0 - 10.0;
            let lum = lcg() * 8.0;
            let next = adapt(cur, lum, lcg() * 3.0, lcg() * 5.0);
            let target = target_ev(lum);
            // next lies between cur and target (inclusive): no overshoot
            assert!((next - cur).abs() <= (target - cur).abs() + 1e-4, "overshoot");
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(adapt(0.0, 2.0, 0.5, 1.0), adapt(0.0, 2.0, 0.5, 1.0));
    }
}
