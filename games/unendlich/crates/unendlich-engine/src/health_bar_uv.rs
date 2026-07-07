//! health_bar_uv.rs — pure health-bar fill rect from an hp fraction.
//!
//! Returns the `[x, y, w, h]` fill rectangle (all in `[0,1]`) for a health bar
//! filled left-to-right by `frac`. Pure, deterministic, panic-free (R1/R11/R21).

/// Fill rect for hp fraction `frac` (clamped to `[0,1]`): `[0, 0, frac, 1]`.
pub fn fill_rect(frac: f32) -> [f32; 4] {
    let f = frac.max(0.0).min(1.0);
    [0.0, 0.0, f, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frac_zero_is_zero_width() {
        assert_eq!(fill_rect(0.0), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn frac_one_is_full() {
        assert_eq!(fill_rect(1.0), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn clamped() {
        assert_eq!(fill_rect(-0.5)[2], 0.0);
        assert_eq!(fill_rect(2.0)[2], 1.0);
    }

    #[test]
    fn width_tracks_frac() {
        for &f in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((fill_rect(f)[2] - f).abs() < 1e-6);
        }
    }

    #[test]
    fn in_unit_range() {
        let mut seed = 0x0217_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            for c in fill_rect(lcg() * 3.0 - 1.0) {
                assert!((0.0..=1.0).contains(&c), "component oob {}", c);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(fill_rect(0.42), fill_rect(0.42));
    }
}
