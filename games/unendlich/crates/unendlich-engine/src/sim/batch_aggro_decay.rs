//! `sim::batch_aggro_decay` — batched aggro decay, mirroring `sim::aggro_decay`
//! (round 18), with a scalar reference for parity.
//!
//! [`aggro_one`] decays one mob's aggro exactly as the r18 core does
//! (`aggro - BASE_RATE·dt·(1 + dist·DIST_WEIGHT)`, clamped to `[0, aggro]`);
//! [`aggro_batch`] decays a whole crowd of mobs at a shared frame `dt`, each with
//! its own `(aggro, dist)`. Both route through the same `#[inline]` `kernel`, so
//! the batch is **bit-for-bit identical** to decaying each mob individually.
//!
//! Perf shape: mobs are read from a contiguous `(aggro, dist)` slice and written
//! to a contiguous `Vec` at one shared `dt`, no cross-mob dependency — one
//! streaming pass per frame instead of a call per angry mob. Pure and
//! deterministic.

/// Base aggro lost per second (identical to the r18 core).
pub const BASE_RATE: f32 = 0.5;
/// How much distance accelerates the decay (identical to the r18 core).
pub const DIST_WEIGHT: f32 = 0.1;

// r330-refactor-batch: local `nonneg` + the `_batch` loop folded into the
// shared `util::batch_kernel` helper (bit-for-bit identical; same kernel/order).
use crate::util::batch_kernel::{map_collect, nonneg};

/// The shared arithmetic (identical to r18 `aggro_decay::decay`).
#[inline]
fn kernel(aggro: f32, dt: f32, dist: f32) -> f32 {
    let a = nonneg(aggro);
    let drop = BASE_RATE * nonneg(dt) * (1.0 + nonneg(dist) * DIST_WEIGHT);
    let new = a - drop;
    if new < 0.0 {
        0.0
    } else if new > a {
        a
    } else {
        new
    }
}

/// Scalar reference: decay one mob's aggro.
pub fn aggro_one(aggro: f32, dt: f32, dist: f32) -> f32 {
    kernel(aggro, dt, dist)
}

/// Batched: decay every mob's aggro by the shared `dt`. Each input is
/// `(aggro, dist)`; output is in order.
pub fn aggro_batch(states: &[(f32, f32)], dt: f32) -> Vec<f32> {
    map_collect(states, |&(aggro, dist)| kernel(aggro, dt, dist))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (f32, f32) {
        let aggro = (lcg(seed) % 100_001) as f32 / 1000.0;
        let dist = (lcg(seed) % 100_001) as f32 / 100.0;
        (aggro, dist)
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
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            let batch = aggro_batch(&states, dt);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (a, d) = states[i];
                assert_eq!(batch[i].to_bits(), aggro_one(a, dt, d).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(aggro_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (a, d) = rand_state(&mut seed);
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            let b = aggro_batch(&[(a, d)], dt);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), aggro_one(a, dt, d).to_bits());
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (a, d) = rand_state(&mut seed);
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            let r = aggro_one(a, dt, d);
            assert!(r >= 0.0 && r <= a + 1e-5, "{} not in [0,{}]", r, a);
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
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            assert_eq!(aggro_batch(&states, dt), aggro_batch(&states, dt));
        }
    }

    #[test]
    fn farther_mob_in_batch_decays_more() {
        // two mobs, same aggro, different distance, shared dt
        let b = aggro_batch(&[(1.0, 0.0), (1.0, 50.0)], 0.2);
        assert!(b[1] < b[0], "far mob {} should decay below near {}", b[1], b[0]);
    }
}
