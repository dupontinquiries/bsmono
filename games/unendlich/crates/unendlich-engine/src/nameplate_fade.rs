//! nameplate_fade.rs — pure nameplate alpha by distance.
//!
//! Nameplate opacity is 1 up to `fade_start`, ramps linearly to 0 by `fade_end`,
//! and stays 0 beyond. Pure, deterministic, panic-free (R1/R11/R21).

/// Nameplate alpha at `dist`, fading over `[fade_start, fade_end]`. `1` near, `0`
/// far, linear between. `fade_end` is guarded to exceed `fade_start`. Result in
/// `[0,1]`.
pub fn alpha(dist: f32, fade_start: f32, fade_end: f32) -> f32 {
    let start = fade_start.max(0.0);
    let end = fade_end.max(start + 1e-4); // guard end > start
    let d = dist.max(0.0);
    if d <= start {
        1.0
    } else if d >= end {
        0.0
    } else {
        (1.0 - (d - start) / (end - start)).max(0.0).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_is_full() {
        assert_eq!(alpha(0.0, 20.0, 40.0), 1.0);
        assert_eq!(alpha(20.0, 20.0, 40.0), 1.0);
    }

    #[test]
    fn past_end_is_zero() {
        assert_eq!(alpha(40.0, 20.0, 40.0), 0.0);
        assert_eq!(alpha(100.0, 20.0, 40.0), 0.0);
    }

    #[test]
    fn linear_between() {
        let mid = alpha(30.0, 20.0, 40.0); // halfway
        assert!((mid - 0.5).abs() < 1e-5, "midpoint not 0.5: {}", mid);
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x0317_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let a = alpha(lcg() * 100.0, lcg() * 50.0, lcg() * 50.0 + 25.0);
            assert!((0.0..=1.0).contains(&a), "alpha oob {}", a);
        }
    }

    #[test]
    fn end_not_greater_than_start_guard() {
        // end <= start must not panic / divide by zero
        let a = alpha(25.0, 30.0, 10.0);
        assert!(a.is_finite() && (0.0..=1.0).contains(&a));
    }

    #[test]
    fn deterministic() {
        assert_eq!(alpha(28.0, 20.0, 40.0), alpha(28.0, 20.0, 40.0));
    }
}
