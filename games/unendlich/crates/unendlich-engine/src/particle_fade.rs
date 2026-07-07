//! particle_fade.rs — pure particle alpha + size over its life.
//!
//! Alpha fades linearly from 1 at birth to 0 at end-of-life; size follows a
//! grow-then-shrink puff curve `4·t·(1-t)`. Pure, deterministic, panic-free
//! (R1/R11/R21).

/// `(alpha, size)` for a particle of age `age` out of total `life`. Alpha is
/// `1` at birth → `0` at death; size is a bounded `[0,1]` puff peaking at
/// mid-life. `life <= 0` yields `(0, 0)`.
pub fn pfade(age: f32, life: f32) -> (f32, f32) {
    if life <= 0.0 {
        return (0.0, 0.0);
    }
    let t = (age.max(0.0) / life).min(1.0); // [0,1]
    let alpha = (1.0 - t).max(0.0).min(1.0);
    let size = (4.0 * t * (1.0 - t)).max(0.0).min(1.0);
    (alpha, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birth_is_full_alpha() {
        let (a, _) = pfade(0.0, 2.0);
        assert!((a - 1.0).abs() < 1e-6, "alpha not full at birth");
    }

    #[test]
    fn end_is_zero_alpha() {
        assert_eq!(pfade(2.0, 2.0).0, 0.0);
        assert_eq!(pfade(5.0, 2.0).0, 0.0, "past end stays 0");
    }

    #[test]
    fn alpha_monotone_non_increasing() {
        let mut prev = 2.0f32;
        let mut age = 0.0f32;
        while age <= 2.0 {
            let a = pfade(age, 2.0).0;
            assert!(a <= prev + 1e-6, "alpha grew at age {}", age);
            prev = a;
            age += 0.02;
        }
    }

    #[test]
    fn size_curve_bounded() {
        let mut age = -0.5f32;
        while age <= 3.0 {
            let s = pfade(age, 2.0).1;
            assert!((0.0..=1.0).contains(&s), "size oob {}", s);
            age += 0.01;
        }
        // peaks near mid-life
        assert!(pfade(1.0, 2.0).1 > pfade(0.1, 2.0).1);
    }

    #[test]
    fn life_zero_guard() {
        let (a, s) = pfade(0.0, 0.0);
        assert!(a.is_finite() && s.is_finite() && a == 0.0 && s == 0.0);
        let (a2, s2) = pfade(1.0, -1.0);
        assert!(a2 == 0.0 && s2 == 0.0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(pfade(0.7, 2.0), pfade(0.7, 2.0));
    }
}
