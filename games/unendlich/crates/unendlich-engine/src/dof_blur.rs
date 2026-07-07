//! dof_blur.rs — pure depth-of-field circle-of-confusion factor.
//!
//! Blur strength (CoC) from how far a sample's depth is from the focal distance,
//! normalized by a focus `range`. Pure, deterministic, panic-free (R1/R11/R21).
//! Result in `[0,1]` (0 = in focus, 1 = max blur).

/// Circle-of-confusion blur at `depth` for focal distance `focus` and `range`.
/// `0` at focus, growing toward 1 with `|depth-focus|`; symmetric around focus.
/// `range` guarded `> 0`.
pub fn coc(depth: f32, focus: f32, range: f32) -> f32 {
    let r = range.max(1e-4);
    ((depth - focus).abs() / r).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_focus_is_zero() {
        assert_eq!(coc(10.0, 10.0, 5.0), 0.0);
    }

    #[test]
    fn near_and_far_grow() {
        assert!(coc(15.0, 10.0, 20.0) > 0.0, "far should blur");
        assert!(coc(5.0, 10.0, 20.0) > 0.0, "near should blur");
        assert!(coc(30.0, 10.0, 20.0) > coc(15.0, 10.0, 20.0), "farther blurs more");
    }

    #[test]
    fn range_scales() {
        // a bigger range gives less blur at a fixed depth offset
        assert!(coc(15.0, 10.0, 40.0) < coc(15.0, 10.0, 10.0));
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x2407_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let c = coc(lcg() * 100.0, lcg() * 100.0, lcg() * 30.0);
            assert!((0.0..=1.0).contains(&c), "coc oob {}", c);
        }
    }

    #[test]
    fn symmetric_around_focus() {
        for &d in &[1.0f32, 4.0, 9.0] {
            assert!((coc(10.0 + d, 10.0, 20.0) - coc(10.0 - d, 10.0, 20.0)).abs() < 1e-6);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(coc(12.0, 10.0, 5.0), coc(12.0, 10.0, 5.0));
    }
}
