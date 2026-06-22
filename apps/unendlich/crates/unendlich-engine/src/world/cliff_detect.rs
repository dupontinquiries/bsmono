//! `cliff_detect` — flag cliffs/overhangs from the local height gradient.
//!
//! Worker-03 / WORLD pure core, round 18. Combines the two partial slopes into a
//! gradient magnitude; a cell is a cliff when that magnitude exceeds a
//! threshold. Used to switch the surface material to bare rock and to gate
//! overhang carving. The magnitude is the rate of steepest ascent, so it folds
//! both axes (a 45° diagonal slope is steeper than either component alone).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Steepness = gradient magnitude `√(dh_dx² + dh_dz²)`.
pub fn steepness(dh_dx: f32, dh_dz: f32) -> f32 {
    (dh_dx * dh_dx + dh_dz * dh_dz).sqrt()
}

/// Cliff iff the gradient magnitude exceeds `thresh` (clamped ≥ 0).
pub fn is_cliff(dh_dx: f32, dh_dz: f32, thresh: f32) -> bool {
    steepness(dh_dx, dh_dz) > thresh.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_not_a_cliff() {
        assert!(!is_cliff(0.0, 0.0, 1.0));
        assert!(!is_cliff(0.0, 0.0, 0.0));
    }

    #[test]
    fn steep_is_a_cliff() {
        assert!(is_cliff(5.0, 0.0, 1.0));
        assert!(is_cliff(0.0, -5.0, 1.0));
    }

    #[test]
    fn threshold_respected() {
        // magnitude exactly 3; threshold 3 → not greater; 2.9 → cliff.
        assert!(!is_cliff(3.0, 0.0, 3.0));
        assert!(is_cliff(3.0, 0.0, 2.9));
    }

    #[test]
    fn diagonal_combines_both_axes() {
        // 3-4-5: combined steeper than either component.
        assert!((steepness(3.0, 4.0) - 5.0).abs() < 1e-5);
        assert!(is_cliff(3.0, 4.0, 4.5)); // 5 > 4.5 though each axis < 4.5? (4<4.5)
    }

    #[test]
    fn symmetric() {
        assert_eq!(steepness(3.0, 4.0), steepness(-3.0, -4.0));
        assert_eq!(steepness(3.0, 4.0), steepness(4.0, 3.0));
        assert_eq!(steepness(2.0, -7.0), steepness(-2.0, 7.0));
    }

    #[test]
    fn deterministic() {
        assert_eq!(is_cliff(2.0, 2.0, 1.0), is_cliff(2.0, 2.0, 1.0));
        assert_eq!(steepness(1.5, 2.5), steepness(1.5, 2.5));
    }
}
