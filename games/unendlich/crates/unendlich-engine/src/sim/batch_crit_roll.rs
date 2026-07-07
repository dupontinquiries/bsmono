//! `sim::batch_crit_roll` — batched crit resolution, mirroring `sim::crit_roll`
//! (round 23), with a scalar reference for parity.
//!
//! [`crit_one`] resolves one `(roll, chance, mult)` into a damage scale exactly
//! as the r23 core does; [`crit_batch`] resolves many at once (every hit landing
//! on the same tick). Both route through the same `#[inline]` `kernel`, so the
//! batch is **bit-for-bit identical** to resolving each hit individually.
//!
//! Perf shape: hits are read from a contiguous `(roll, chance, mult)` slice and
//! written to a contiguous `Vec<f32>` of damage scales in one streaming pass, no
//! cross-hit dependency. Pure and deterministic — the caller supplies the rolls,
//! so there's no clock or RNG state, and no cross-imports.

/// The shared crit kernel (identical to r23 `crit_roll::crit`).
#[inline]
fn kernel(roll: f32, chance: f32, mult: f32) -> f32 {
    let m = if mult > 1.0 { mult } else { 1.0 };
    if chance >= 1.0 {
        m
    } else if chance <= 0.0 {
        1.0
    } else if roll < chance {
        m
    } else {
        1.0
    }
}

/// Scalar reference: damage scale for one hit.
pub fn crit_one(roll: f32, chance: f32, mult: f32) -> f32 {
    kernel(roll, chance, mult)
}

/// Batched: damage scale for every `(roll, chance, mult)` hit, in order.
// r330-refactor-batch: `_batch` loop folded into the shared helper (bit-identical).
use crate::util::batch_kernel::map_collect;
pub fn crit_batch(states: &[(f32, f32, f32)]) -> Vec<f32> {
    map_collect(states, |&(roll, chance, mult)| kernel(roll, chance, mult))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (f32, f32, f32) {
        let roll = (lcg(seed) % 100_000) as f32 / 100_000.0;
        let chance = (lcg(seed) % 200_001) as f32 / 100_000.0 - 0.5; // incl <0,>1
        let mult = (lcg(seed) % 100_001) as f32 / 1000.0 - 50.0; // incl < 1
        (roll, chance, mult)
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
            let batch = crit_batch(&states);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (r, c, m) = states[i];
                assert_eq!(batch[i].to_bits(), crit_one(r, c, m).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(crit_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (r, c, m) = rand_state(&mut seed);
            let b = crit_batch(&[(r, c, m)]);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), crit_one(r, c, m).to_bits());
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (r, c, m) = rand_state(&mut seed);
            let v = crit_one(r, c, m);
            let hi = if m > 1.0 { m } else { 1.0 };
            assert!(v >= 1.0 && v <= hi, "{} not in [1,{}]", v, hi);
        }
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
            assert_eq!(crit_batch(&states), crit_batch(&states));
        }
    }

    #[test]
    fn crit_and_normal_in_batch() {
        // roll below chance crits, roll above is normal
        let b = crit_batch(&[(0.1, 0.5, 2.0), (0.9, 0.5, 2.0)]);
        assert_eq!(b[0], 2.0);
        assert_eq!(b[1], 1.0);
    }
}
