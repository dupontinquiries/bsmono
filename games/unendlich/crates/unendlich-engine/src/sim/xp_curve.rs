//! `sim::xp_curve` — the cumulative XP needed to reach a given level.
//!
//! [`xp_for_level`] is a superlinear curve: `A·level² + B·level`. Level 0 is the
//! baseline (0 XP), each level costs strictly more than the last, and the gap
//! between consecutive levels grows (the second difference is positive), so
//! leveling slows down as players climb. All arithmetic saturates, so even
//! `u32::MAX` levels return a finite `u64` instead of overflowing.
//!
//! Pure and deterministic: two multiplies and an add in saturating `u64`.

/// Quadratic coefficient (dominant, superlinear term).
const A: u64 = 100;
/// Linear coefficient (keeps low levels sanely small).
const B: u64 = 50;

/// Cumulative XP required to reach `level`. Monotone increasing, superlinear,
/// saturating at `u64::MAX`.
pub fn xp_for_level(level: u32) -> u64 {
    let l = level as u64;
    let quad = A.saturating_mul(l.saturating_mul(l));
    let lin = B.saturating_mul(l);
    quad.saturating_add(lin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_zero_is_base() {
        assert_eq!(xp_for_level(0), 0);
    }

    #[test]
    fn level_one_is_sane() {
        // small, positive, and equal to A + B
        assert_eq!(xp_for_level(1), A + B);
        assert!(xp_for_level(1) > 0 && xp_for_level(1) < 1000);
    }

    #[test]
    fn strictly_increasing() {
        let mut l = 0u32;
        while l < 5000 {
            assert!(xp_for_level(l + 1) > xp_for_level(l), "not increasing at {}", l);
            l += 1;
        }
    }

    #[test]
    fn superlinear_growing_gaps() {
        // the increment between consecutive levels strictly increases
        let mut l = 1u32;
        while l < 5000 {
            let gap_prev = xp_for_level(l) - xp_for_level(l - 1);
            let gap_next = xp_for_level(l + 1) - xp_for_level(l);
            assert!(gap_next > gap_prev, "gaps not growing at {}", l);
            l += 1;
        }
        // and doubling the level more than doubles the cost
        assert!(xp_for_level(200) > 2 * xp_for_level(100));
    }

    #[test]
    fn large_level_saturates_without_panic() {
        let top = xp_for_level(u32::MAX);
        // computed something finite; near the top it must still be monotone
        assert!(top >= xp_for_level(u32::MAX - 1));
        // and very large levels saturate to the u64 ceiling
        assert_eq!(xp_for_level(u32::MAX), u64::MAX);
    }

    #[test]
    fn deterministic() {
        let mut l = 0u32;
        while l < 10_000 {
            assert_eq!(xp_for_level(l), xp_for_level(l));
            l += 7;
        }
    }
}
