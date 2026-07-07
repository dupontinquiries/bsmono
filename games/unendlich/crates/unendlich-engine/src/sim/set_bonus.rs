//! `sim::set_bonus` — the bonus granted by wearing pieces of an item set.
//!
//! [`bonus`] takes how many set pieces are equipped (`set_count`) and a list of
//! `(required_count, bonus)` thresholds, and returns the best bonus the count has
//! unlocked. Below the first threshold the bonus is `0`; as more pieces are
//! equipped, higher thresholds unlock and the bonus rises. It returns the
//! greatest bonus among all met thresholds, which makes it monotonic in
//! `set_count` regardless of how the threshold list is ordered.
//!
//! Pure and deterministic: a single scan, floored at `0` — no clock, no
//! randomness, no cross-imports.

/// Best set bonus unlocked at `set_count`.
///
/// Returns `max(0, greatest bonus among thresholds with required_count <=
/// set_count)`. Empty thresholds (or none met) give `0`.
pub fn bonus(set_count: u8, thresholds: &[(u8, f32)]) -> f32 {
    let mut best = 0.0f32;
    let mut i = 0;
    while i < thresholds.len() {
        let (req, b) = thresholds[i];
        if req <= set_count && b > best {
            best = b;
        }
        i += 1;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn below_first_threshold_is_zero() {
        let t = [(2u8, 0.1f32), (4, 0.25), (6, 0.5)];
        assert_eq!(bonus(0, &t), 0.0);
        assert_eq!(bonus(1, &t), 0.0);
    }

    #[test]
    fn crosses_thresholds() {
        let t = [(2u8, 0.1f32), (4, 0.25), (6, 0.5)];
        assert_eq!(bonus(2, &t), 0.1);
        assert_eq!(bonus(3, &t), 0.1);
        assert_eq!(bonus(4, &t), 0.25);
        assert_eq!(bonus(5, &t), 0.25);
        assert_eq!(bonus(6, &t), 0.5);
    }

    #[test]
    fn highest_met_wins() {
        let t = [(2u8, 0.1f32), (4, 0.25), (6, 0.5)];
        assert_eq!(bonus(10, &t), 0.5); // all met → top bonus
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(bonus(99, &[]), 0.0);
    }

    #[test]
    fn monotone_in_set_count() {
        // random non-negative tables; bonus must be non-decreasing in count
        let mut seed = 0x5111_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut t = Vec::new();
            for _ in 0..n {
                let req = (lcg(&mut seed) % 12) as u8;
                let b = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
                t.push((req, b));
            }
            let mut prev = bonus(0, &t);
            let mut c = 1u8;
            while c <= 12 {
                let cur = bonus(c, &t);
                assert!(cur >= prev - 1e-9, "dropped at count {}", c);
                prev = cur;
                c += 1;
            }
        }
    }

    #[test]
    fn deterministic() {
        let t = [(2u8, 0.1f32), (4, 0.25), (6, 0.5), (3, 0.05)];
        let mut c = 0u8;
        while c <= 12 {
            assert_eq!(bonus(c, &t), bonus(c, &t));
            c += 1;
        }
    }
}
