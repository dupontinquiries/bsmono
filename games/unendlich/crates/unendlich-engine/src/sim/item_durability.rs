//! `sim::item_durability` — wear and repair of a tool's durability.
//!
//! [`apply_wear`] takes the current durability, the maximum, and a signed
//! `delta` (positive = wear taken, negative = repair applied) and returns the
//! new durability clamped to `[0, max]` plus whether the item is now broken
//! (durability `0`). Wear lowers durability, repair raises it but never past
//! `max`, and a `delta` of `0` is a no-op. Integer-only and total — it saturates
//! rather than overflowing on absurd inputs, and never panics.
//!
//! Pure and deterministic: a saturating subtract and a clamp, no clock, no
//! randomness, no cross-imports.

/// Apply `delta` wear (or, if negative, repair) to `cur` durability.
///
/// Returns `(new_durability, broke)` where `new_durability` is clamped to
/// `[0, max]` (with `max` floored at `0`) and `broke` is true when the result is
/// `0`.
pub fn apply_wear(cur: i32, max: i32, delta: i32) -> (i32, bool) {
    let m = if max > 0 { max } else { 0 };
    // wear lowers durability; saturating so extreme deltas can't overflow
    let raw = cur.saturating_sub(delta);
    let new = if raw < 0 {
        0
    } else if raw > m {
        m
    } else {
        raw
    };
    (new, new == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn wear_reduces_durability() {
        let (new, broke) = apply_wear(100, 100, 30);
        assert_eq!(new, 70);
        assert!(!broke);
    }

    #[test]
    fn repair_caps_at_max() {
        // negative delta repairs; can't exceed max
        let (new, broke) = apply_wear(80, 100, -50);
        assert_eq!(new, 100);
        assert!(!broke);
    }

    #[test]
    fn reaching_zero_breaks() {
        let (new, broke) = apply_wear(20, 100, 20);
        assert_eq!(new, 0);
        assert!(broke);
        // and over-wearing still floors at 0 and breaks
        let (new2, broke2) = apply_wear(5, 100, 999);
        assert_eq!(new2, 0);
        assert!(broke2);
    }

    #[test]
    fn clamps_both_ends() {
        let mut seed = 0xC1A3_u32;
        for _ in 0..5000 {
            let max = (lcg(&mut seed) % 1000) as i32;
            let cur = (lcg(&mut seed) % 2000) as i32 - 500; // may start out of range
            let delta = (lcg(&mut seed) % 4000) as i32 - 2000;
            let (new, broke) = apply_wear(cur, max, delta);
            let m = max.max(0);
            assert!(new >= 0 && new <= m, "out of range: {} (max {})", new, m);
            assert_eq!(broke, new == 0);
        }
    }

    #[test]
    fn delta_zero_is_noop_within_range() {
        let (new, broke) = apply_wear(55, 100, 0);
        assert_eq!(new, 55);
        assert!(!broke);
        // an already-broken item stays broken
        let (n0, b0) = apply_wear(0, 100, 0);
        assert_eq!(n0, 0);
        assert!(b0);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let cur = (lcg(&mut seed) % 1000) as i32;
            let max = (lcg(&mut seed) % 1000) as i32;
            let delta = (lcg(&mut seed) % 2000) as i32 - 1000;
            assert_eq!(apply_wear(cur, max, delta), apply_wear(cur, max, delta));
        }
    }
}
