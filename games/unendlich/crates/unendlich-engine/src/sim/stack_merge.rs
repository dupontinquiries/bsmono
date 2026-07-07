//! `sim::stack_merge` — combine two item stacks, respecting a stack cap.
//!
//! [`merge`] adds stack `a` into stack `b` (or vice versa — it's symmetric) up to
//! `max`, returning the combined amount and any overflow that didn't fit. If the
//! total fits, overflow is `0`; if it exceeds `max`, the combined amount is
//! capped at `max` and the remainder is the overflow. A `max` of `0` accepts
//! nothing, so everything overflows.
//!
//! Pure and total: the sum is computed in `u64` and the overflow saturates, so
//! even `u32::MAX + u32::MAX` can't overflow or panic. No clock, no randomness.

/// Merge stacks `a` and `b` up to `max`.
///
/// Returns `(combined, overflow)` with `combined = min(a + b, max)` and
/// `overflow = (a + b) - combined` (saturated into `u32`).
pub fn merge(a: u32, b: u32, max: u32) -> (u32, u32) {
    let total = a as u64 + b as u64;
    let cap = max as u64;
    let combined = if total < cap { total } else { cap };
    let overflow = total - combined; // >= 0 by construction
    let overflow = if overflow > u32::MAX as u64 {
        u32::MAX
    } else {
        overflow as u32
    };
    (combined as u32, overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn fits_with_no_overflow() {
        assert_eq!(merge(10, 20, 64), (30, 0));
        assert_eq!(merge(0, 0, 64), (0, 0));
        assert_eq!(merge(64, 0, 64), (64, 0)); // exactly full
    }

    #[test]
    fn overflows_past_max() {
        assert_eq!(merge(40, 40, 64), (64, 16));
        assert_eq!(merge(64, 64, 64), (64, 64));
    }

    #[test]
    fn cap_is_respected() {
        let mut seed = 0xCA9_u32;
        for _ in 0..5000 {
            let a = lcg(&mut seed) % 100_000;
            let b = lcg(&mut seed) % 100_000;
            let max = lcg(&mut seed) % 100_000;
            let (combined, overflow) = merge(a, b, max);
            assert!(combined <= max);
            // conservation: combined + overflow == a + b (no overflow at this scale)
            assert_eq!(combined as u64 + overflow as u64, a as u64 + b as u64);
        }
    }

    #[test]
    fn a_or_b_zero() {
        assert_eq!(merge(0, 50, 64), (50, 0));
        assert_eq!(merge(50, 0, 64), (50, 0));
        assert_eq!(merge(0, 100, 64), (64, 36));
    }

    #[test]
    fn max_zero_guard() {
        assert_eq!(merge(5, 7, 0), (0, 12));
        assert_eq!(merge(0, 0, 0), (0, 0));
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        // u32::MAX + u32::MAX into max 0 → combined 0, overflow saturates
        let (c, o) = merge(u32::MAX, u32::MAX, 0);
        assert_eq!(c, 0);
        assert_eq!(o, u32::MAX); // saturated, no panic
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let a = lcg(&mut seed);
            let b = lcg(&mut seed);
            let max = lcg(&mut seed);
            assert_eq!(merge(a, b, max), merge(a, b, max));
        }
    }
}
