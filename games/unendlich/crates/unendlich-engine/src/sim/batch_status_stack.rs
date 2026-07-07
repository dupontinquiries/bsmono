//! `sim::batch_status_stack` — batched status-stack adds, mirroring
//! `sim::status_stack` (round 23), with a scalar reference for parity.
//!
//! [`stack_one`] adds stacks to one effect exactly as the r23 core does
//! (`min(cur.saturating_add(add), cap)`); [`stack_batch`] applies a stack add to
//! a whole list of effects at once (an AoE that stacks a debuff on every target).
//! Both route through the same `#[inline]` `kernel`, so the batch is
//! **bit-for-bit identical** to stacking each effect individually.
//!
//! Perf shape: effects are read from a contiguous `(cur, add, cap)` slice and
//! written to a contiguous `Vec<u8>` in one streaming pass, no cross-effect
//! dependency. Pure and deterministic — no clock, no randomness, no cross-imports.

/// The shared stack kernel (identical to r23 `status_stack::add_stack`).
#[inline]
fn kernel(cur: u8, add: u8, cap: u8) -> u8 {
    let summed = cur.saturating_add(add);
    if summed < cap {
        summed
    } else {
        cap
    }
}

/// Scalar reference: add stacks to one effect.
pub fn stack_one(cur: u8, add: u8, cap: u8) -> u8 {
    kernel(cur, add, cap)
}

/// Batched: add stacks to every `(cur, add, cap)` effect, in order.
// r330-refactor-batch: `_batch` loop folded into the shared helper (bit-identical).
use crate::util::batch_kernel::map_collect;
pub fn stack_batch(states: &[(u8, u8, u8)]) -> Vec<u8> {
    map_collect(states, |&(cur, add, cap)| kernel(cur, add, cap))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (u8, u8, u8) {
        (
            (lcg(seed) & 0xFF) as u8,
            (lcg(seed) & 0xFF) as u8,
            (lcg(seed) & 0xFF) as u8,
        )
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB17C_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 16) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            let batch = stack_batch(&states);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (c, a, cap) = states[i];
                assert_eq!(batch[i], stack_one(c, a, cap));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(stack_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (c, a, cap) = rand_state(&mut seed);
            let b = stack_batch(&[(c, a, cap)]);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0], stack_one(c, a, cap));
        }
    }

    #[test]
    fn saturating() {
        // would overflow u8 → saturates, then caps
        let b = stack_batch(&[(250, 250, 255), (200, 100, 100)]);
        assert_eq!(b[0], 255);
        assert_eq!(b[1], 100);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 10) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            assert_eq!(stack_batch(&states), stack_batch(&states));
        }
    }

    #[test]
    fn capped_in_batch() {
        let b = stack_batch(&[(8, 5, 10), (0, 1, 10)]);
        assert_eq!(b[0], 10); // capped
        assert_eq!(b[1], 1);
    }
}
