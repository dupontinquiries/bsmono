//! `sim::batch_appendage_ripple` — batched secondary motion, mirroring
//! `sim::appendage_ripple` (round 16), with a scalar reference for parity.
//!
//! [`ripple_one`] computes one appendage's offset exactly as the r16 core does;
//! [`ripple_batch`] computes a whole creature's worth of soft parts (ears, tail,
//! fronds) at a shared frame time `t`, each with its own `(vel, stiffness)`. Both
//! route through the same `#[inline]` `kernel`, so the batch is **bit-for-bit
//! identical** to calling `ripple_one` per appendage.
//!
//! Perf shape: appendages are read from a contiguous `(vel, stiffness)` slice and
//! written into a contiguous `Vec` at one shared `t`, with no cross-appendage
//! dependency — one streaming pass per frame rather than a call per wiggly bit.
//! Pure and deterministic.

const AMP_SCALE: f32 = 0.1;
const MAX_AMP: f32 = 1.0;
const BASE_FREQ: f32 = 4.0;

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// The shared arithmetic (identical to the r16 `appendage_ripple` core).
#[inline]
fn kernel(vel: f32, stiffness: f32, t: f32) -> f32 {
    let s = if stiffness > 0.0 { stiffness } else { 0.0 };
    let amp = clampf(vel.abs() * AMP_SCALE / (1.0 + s), 0.0, MAX_AMP);
    let freq = BASE_FREQ * (1.0 + s);
    clampf(amp * (freq * t).sin(), -MAX_AMP, MAX_AMP)
}

/// Scalar reference: one appendage's secondary-motion offset.
pub fn ripple_one(vel: f32, stiffness: f32, t: f32) -> f32 {
    kernel(vel, stiffness, t)
}

/// Batched: offsets for every appendage at shared time `t`. Each input is
/// `(vel, stiffness)`; output is in order.
pub fn ripple_batch(states: &[(f32, f32)], t: f32) -> Vec<f32> {
    crate::util::batch_kernel::map_collect(states, |&(vel, stiffness)| kernel(vel, stiffness, t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (f32, f32) {
        let vel = (lcg(seed) % 200_001) as f32 / 100.0 - 1000.0;
        let stiff = (lcg(seed) % 100_001) as f32 / 10_000.0;
        (vel, stiff)
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
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let batch = ripple_batch(&states, t);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (v, s) = states[i];
                assert_eq!(batch[i].to_bits(), ripple_one(v, s, t).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(ripple_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (v, s) = rand_state(&mut seed);
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let b = ripple_batch(&[(v, s)], t);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), ripple_one(v, s, t).to_bits());
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (v, s) = rand_state(&mut seed);
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let r = ripple_one(v, s, t);
            assert!(r >= -MAX_AMP && r <= MAX_AMP, "offset {}", r);
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
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            assert_eq!(ripple_batch(&states, t), ripple_batch(&states, t));
        }
    }

    #[test]
    fn zero_velocity_entry_is_still() {
        let b = ripple_batch(&[(0.0, 0.5), (4.0, 0.5)], 12.34);
        assert!(b[0].abs() < 1e-6);
        assert!(b[1] >= -MAX_AMP && b[1] <= MAX_AMP);
    }
}
