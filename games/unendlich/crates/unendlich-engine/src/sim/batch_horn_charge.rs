//! `sim::batch_horn_charge` — batched horn-charge accrual, mirroring
//! `sim::horn_charge` (round 20), with a scalar reference for parity.
//!
//! [`charge_one`] advances one horn's stored charge exactly as the r20 core does
//! (`clamp(cur + rate·dt, 0, capacity)`); [`charge_batch`] advances a whole
//! herd's horns at a shared frame `dt`, each with its own `(cur, rate, capacity)`.
//! Both route through the same `#[inline]` `kernel`, so the batch is
//! **bit-for-bit identical** to charging each horn individually.
//!
//! Perf shape: horns are read from a contiguous `(cur, rate, capacity)` slice and
//! written to a contiguous `Vec` at one shared `dt`, no cross-horn dependency —
//! one streaming pass per frame instead of a call per creature. Pure and
//! deterministic.

/// The shared accrual kernel (identical to r20 `horn_charge::charge`).
#[inline]
fn kernel(cur: f32, dt: f32, rate: f32, capacity: f32) -> f32 {
    let cap = if capacity > 0.0 { capacity } else { 0.0 };
    let next = cur + rate * dt;
    if next < 0.0 {
        0.0
    } else if next > cap {
        cap
    } else {
        next
    }
}

/// Scalar reference: advance one horn's charge.
pub fn charge_one(cur: f32, dt: f32, rate: f32, capacity: f32) -> f32 {
    kernel(cur, dt, rate, capacity)
}

/// Batched: advance every horn's charge by the shared `dt`. Each input is
/// `(cur, rate, capacity)`; output is in order.
pub fn charge_batch(states: &[(f32, f32, f32)], dt: f32) -> Vec<f32> {
    crate::util::batch_kernel::map_collect(states, |&(cur, rate, capacity)| kernel(cur, dt, rate, capacity))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (f32, f32, f32) {
        let cur = (lcg(seed) % 2001) as f32 / 100.0;
        let rate = (lcg(seed) % 4001) as f32 / 100.0 - 20.0;
        let cap = (lcg(seed) % 2001) as f32 / 100.0 - 5.0;
        (cur, rate, cap)
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
            let dt = (lcg(&mut seed) % 2001) as f32 / 100.0;
            let batch = charge_batch(&states, dt);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (c, r, cap) = states[i];
                assert_eq!(batch[i].to_bits(), charge_one(c, dt, r, cap).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(charge_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (c, r, cap) = rand_state(&mut seed);
            let dt = (lcg(&mut seed) % 2001) as f32 / 100.0;
            let b = charge_batch(&[(c, r, cap)], dt);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), charge_one(c, dt, r, cap).to_bits());
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (c, r, cap) = rand_state(&mut seed);
            let dt = (lcg(&mut seed) % 2001) as f32 / 100.0;
            let v = charge_one(c, dt, r, cap);
            let hi = if cap > 0.0 { cap } else { 0.0 };
            assert!(v >= 0.0 && v <= hi, "{} not in [0,{}]", v, hi);
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
            let dt = (lcg(&mut seed) % 2001) as f32 / 100.0;
            assert_eq!(charge_batch(&states, dt), charge_batch(&states, dt));
        }
    }

    #[test]
    fn full_horn_in_batch_stays_capped() {
        let b = charge_batch(&[(10.0, 5.0, 10.0), (0.0, 5.0, 10.0)], 1.0);
        assert_eq!(b[0], 10.0); // already full, capped
        assert_eq!(b[1], 5.0); // charged from empty
    }
}
