//! damage_flash.rs — pure hit-flash intensity over time.
//!
//! Returns the white-flash overlay intensity after a hit: peaks at `1` on the
//! hit frame and decays to `0` over `duration`. Pure, deterministic, panic-free
//! (R1/R11/R21).

/// Flash intensity at `time_since_hit` seconds for a flash lasting `duration`
/// seconds. `1` at the hit, decaying linearly to `0` at (and beyond) `duration`.
/// `duration <= 0` yields `0` (no flash window). Result is in `[0,1]`.
pub fn flash(time_since_hit: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return 0.0;
    }
    let t = time_since_hit.max(0.0);
    if t >= duration {
        return 0.0;
    }
    (1.0 - t / duration).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_zero_is_peak() {
        assert!((flash(0.0, 0.5) - 1.0).abs() < 1e-6, "should peak at the hit");
    }

    #[test]
    fn after_duration_is_zero() {
        assert_eq!(flash(0.5, 0.5), 0.0, "at duration -> 0");
        assert_eq!(flash(2.0, 0.5), 0.0, "beyond duration -> 0");
    }

    #[test]
    fn monotone_decay() {
        let mut prev = 2.0f32;
        let mut t = 0.0f32;
        while t <= 1.0 {
            let f = flash(t, 1.0);
            assert!(f <= prev + 1e-6, "flash increased at t={}", t);
            prev = f;
            t += 0.01;
        }
    }

    #[test]
    fn in_unit_range() {
        let mut seed = 0xDF05_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..3000 {
            let t = lcg() * 3.0 - 0.5;
            let d = lcg() * 2.0;
            let f = flash(t, d);
            assert!((0.0..=1.0).contains(&f), "flash oob {}", f);
        }
    }

    #[test]
    fn zero_duration_safe() {
        let f = flash(0.0, 0.0);
        assert!(f.is_finite() && (0.0..=1.0).contains(&f));
        let g = flash(1.0, -1.0);
        assert!(g.is_finite() && (0.0..=1.0).contains(&g));
    }

    #[test]
    fn deterministic() {
        assert_eq!(flash(0.25, 0.5), flash(0.25, 0.5));
    }
}
