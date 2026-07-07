//! contact_shadow.rs — pure fake contact-shadow darkening by ground proximity.
//!
//! A cheap grounding cue: a darkening factor that's strongest when an entity
//! touches the ground and fades to nothing by `max_h`. Pure, deterministic,
//! panic-free (R1/R11/R21).

/// Darkening at `height_above_ground` for a falloff of `max_h`: `1` touching,
/// linearly to `0` at (and above) `max_h`. `max_h <= 0` is guarded. In `[0,1]`.
pub fn darken(height_above_ground: f32, max_h: f32) -> f32 {
    let h = height_above_ground.max(0.0);
    let m = max_h.max(1e-4); // guard max_h > 0
    if h >= m {
        return 0.0;
    }
    (1.0 - h / m).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_is_max_dark() {
        assert!((darken(0.0, 2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn above_max_is_zero() {
        assert_eq!(darken(2.0, 2.0), 0.0);
        assert_eq!(darken(10.0, 2.0), 0.0);
    }

    #[test]
    fn monotone_non_increasing() {
        let mut prev = 2.0f32;
        let mut h = 0.0f32;
        while h <= 3.0 {
            let d = darken(h, 2.0);
            assert!(d <= prev + 1e-6, "darkening grew at h={}", h);
            prev = d;
            h += 0.02;
        }
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x1907_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let d = darken(lcg() * 10.0 - 2.0, lcg() * 5.0);
            assert!((0.0..=1.0).contains(&d), "darken oob {}", d);
        }
    }

    #[test]
    fn max_h_guard() {
        let d = darken(0.0, 0.0);
        assert!(d.is_finite() && (0.0..=1.0).contains(&d));
        let e = darken(1.0, -3.0);
        assert!(e.is_finite() && (0.0..=1.0).contains(&e));
    }

    #[test]
    fn deterministic() {
        assert_eq!(darken(0.5, 2.0), darken(0.5, 2.0));
    }
}
