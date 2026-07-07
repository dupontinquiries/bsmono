//! `sim::status_stack` — add stacks to a status effect, up to a cap.
//!
//! [`add_stack`] adds `add` stacks to the current `cur`, clamped to `cap`. The
//! addition saturates before capping, so even absurd inputs can't overflow the
//! `u8`; a `cap` of 0 holds no stacks. Pairs with the round-18/20 status-effect
//! ticking, which consumes the stack count this produces.
//!
//! Pure and deterministic: a saturating add and a min — no clock, no randomness,
//! no cross-imports.

/// Add `add` stacks to `cur`, capped at `cap`.
///
/// `min(cur.saturating_add(add), cap)`.
pub fn add_stack(cur: u8, add: u8, cap: u8) -> u8 {
    let summed = cur.saturating_add(add);
    if summed < cap {
        summed
    } else {
        cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn stacks_add() {
        assert_eq!(add_stack(2, 3, 10), 5);
        assert_eq!(add_stack(0, 1, 10), 1);
    }

    #[test]
    fn capped() {
        assert_eq!(add_stack(8, 5, 10), 10);
        assert_eq!(add_stack(10, 1, 10), 10);
        // already over cap stays at cap
        assert_eq!(add_stack(20, 0, 10), 10);
    }

    #[test]
    fn zero_add() {
        assert_eq!(add_stack(3, 0, 10), 3);
        assert_eq!(add_stack(0, 0, 10), 0);
    }

    #[test]
    fn cap_zero_guard() {
        assert_eq!(add_stack(5, 5, 0), 0);
        assert_eq!(add_stack(0, 0, 0), 0);
    }

    #[test]
    fn saturating_no_overflow() {
        // cur + add would overflow u8 → saturates, then caps
        assert_eq!(add_stack(250, 250, 255), 255);
        assert_eq!(add_stack(200, 100, 255), 255);
        assert_eq!(add_stack(255, 255, 100), 100);
    }

    #[test]
    fn deterministic_and_bounded() {
        let mut seed = 0xD37E_u32;
        for _ in 0..5000 {
            let cur = (lcg(&mut seed) & 0xFF) as u8;
            let add = (lcg(&mut seed) & 0xFF) as u8;
            let cap = (lcg(&mut seed) & 0xFF) as u8;
            let r = add_stack(cur, add, cap);
            assert!(r <= cap);
            assert_eq!(r, add_stack(cur, add, cap));
        }
    }
}
