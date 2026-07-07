//! spec_highlight.rs — pure Blinn-Phong specular term.
//!
//! `clamp(n·h, 0, 1)^power`, the classic Blinn-Phong specular highlight. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Specular term for half-vector alignment `n_dot_h` and exponent `power`.
/// Negative `n_dot_h` clamps to 0; result is in `[0,1]`.
pub fn spec(n_dot_h: f32, power: f32) -> f32 {
    let ndh = n_dot_h.max(0.0).min(1.0);
    let p = power.max(0.0);
    ndh.powf(p).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_is_one() {
        assert!((spec(1.0, 32.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn perpendicular_is_zero() {
        assert!(spec(0.0, 32.0) < 1e-6);
    }

    #[test]
    fn power_sharpens() {
        // higher power -> smaller value for the same partial alignment
        assert!(spec(0.5, 64.0) < spec(0.5, 8.0));
    }

    #[test]
    fn negative_clamps_to_zero() {
        assert_eq!(spec(-0.7, 16.0), 0.0);
    }

    #[test]
    fn monotone_in_alignment() {
        let mut prev = -1.0f32;
        let mut x = 0.0f32;
        while x <= 1.0 {
            let s = spec(x, 24.0);
            assert!(s >= prev - 1e-6, "spec not monotone at {}", x);
            assert!((0.0..=1.0).contains(&s));
            prev = s;
            x += 0.01;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(spec(0.8, 32.0), spec(0.8, 32.0));
    }
}
