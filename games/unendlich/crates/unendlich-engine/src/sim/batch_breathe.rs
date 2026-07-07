//! `sim::batch_breathe` — batched torso-breathing, mirroring `sim::breathe`
//! (round 14), with a scalar reference for parity.
//!
//! [`breathe_one`] advances one creature's breathing cycle and returns
//! `(new_phase, torso_scale)` exactly as the r14 core does; [`breathe_batch`]
//! steps a whole herd in one call, each with its own `(phase, depth, rate)` and
//! a shared frame `dt`. Both route through the same `#[inline]` `kernel`, so the
//! batch is **bit-for-bit identical** to calling `breathe_one` per creature.
//!
//! Perf shape: creatures are read from a contiguous slice of `(phase, depth,
//! rate)` and written into a contiguous `Vec` in order, sharing one `dt` kept in
//! a register, with no cross-creature dependency — one streaming, vectorizable
//! pass issued once per frame instead of a call per idle creature.
//!
//! Pure and deterministic: a sine, a wrap, a clamp — no clock, no randomness.

use std::f32::consts::PI;

const TAU: f32 = 2.0 * PI;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// The shared arithmetic. Both scalar and batch paths call exactly this.
#[inline]
fn kernel(phase: f32, depth: f32, rate: f32, dt: f32) -> (f32, f32) {
    let depth = clamp01(depth);
    let new_phase = (phase + rate * dt).rem_euclid(1.0);
    (new_phase, 1.0 + depth * (TAU * new_phase).sin())
}

/// Scalar reference: advance one creature's breathing one step.
pub fn breathe_one(phase: f32, depth: f32, rate: f32, dt: f32) -> (f32, f32) {
    kernel(phase, depth, rate, dt)
}

/// Batched: advance every creature's breathing by the shared `dt`. Each input
/// is `(phase, depth, rate)`; output is `(new_phase, torso_scale)` in order.
pub fn breathe_batch(states: &[(f32, f32, f32)], dt: f32) -> Vec<(f32, f32)> {
    crate::util::batch_kernel::map_collect(states, |&(phase, depth, rate)| kernel(phase, depth, rate, dt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn unit(seed: &mut u32) -> f32 {
        (lcg(seed) % 100_001) as f32 / 100_000.0
    }
    fn rand_state(seed: &mut u32) -> (f32, f32, f32) {
        (unit(seed), unit(seed), unit(seed) * 8.0 - 4.0)
    }
    fn same(a: (f32, f32), b: (f32, f32)) -> bool {
        a.0.to_bits() == b.0.to_bits() && a.1.to_bits() == b.1.to_bits()
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB3EA_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 16) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            let dt = unit(&mut seed) * 2.0;
            let batch = breathe_batch(&states, dt);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (p, d, r) = states[i];
                assert!(same(batch[i], breathe_one(p, d, r, dt)));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(breathe_batch(&[], 0.5).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let s = rand_state(&mut seed);
            let dt = unit(&mut seed) * 2.0;
            let b = breathe_batch(&[s], dt);
            assert_eq!(b.len(), 1);
            assert!(same(b[0], breathe_one(s.0, s.1, s.2, dt)));
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let s = rand_state(&mut seed);
            let dt = unit(&mut seed) * 2.0;
            let (phase, scale) = breathe_one(s.0, s.1, s.2, dt);
            let depth = clamp01(s.1);
            assert!(phase >= 0.0 && phase < 1.0, "phase {}", phase);
            assert!(scale >= 1.0 - depth - 1e-5 && scale <= 1.0 + depth + 1e-5);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            let dt = unit(&mut seed);
            assert_eq!(breathe_batch(&states, dt), breathe_batch(&states, dt));
        }
    }

    #[test]
    fn matches_known_quarter_phase() {
        // phase 0 + rate*dt = 0.25 → sin(pi/2) = 1 → scale = 1 + depth
        let (p, s) = breathe_one(0.0, 0.5, 1.0, 0.25);
        assert!((p - 0.25).abs() < 1e-6);
        assert!((s - 1.5).abs() < 1e-5, "got {}", s);
    }
}
