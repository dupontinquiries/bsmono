//! `sim::batch_blink` — batched eyelid blinking, mirroring `sim::blink`, with a
//! scalar reference for parity.
//!
//! [`blink_one`] computes one creature's eyelid value exactly as the `blink`
//! core does; [`blink_batch`] computes a whole crowd at a shared frame time `t`,
//! each with its own `(rate_hz, seed)`. Both route through the same `#[inline]`
//! `kernel`, so the batch is **bit-for-bit identical** to calling `blink_one`
//! per creature.
//!
//! Perf shape: creatures are read from a contiguous `(rate_hz, seed)` slice and
//! written into a contiguous `Vec` at one shared `t`, with no cross-creature
//! dependency — a single streaming pass per frame instead of a call per face,
//! the kind of loop the autovectorizer and instruction cache like. Pure and
//! deterministic.

use std::f32::consts::PI;

const BLINK_DUR: f32 = 0.12;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

fn hash01(seed: u64, n: u64) -> f32 {
    let h = mix64(seed ^ n.wrapping_mul(GOLDEN));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// The shared arithmetic (identical to the `blink` core). Both paths call this.
#[inline]
fn kernel(t: f32, rate_hz: f32, seed: u64) -> f32 {
    if !(rate_hz > 0.0) {
        return 0.0;
    }
    let phase = t * rate_hz;
    let n = phase.floor();
    let frac = phase - n;
    let w = (BLINK_DUR * rate_hz).clamp(1e-4, 1.0);
    let start = hash01(seed, n as i64 as u64) * (1.0 - w);
    if frac >= start && frac < start + w {
        let u = (frac - start) / w;
        (PI * u).sin()
    } else {
        0.0
    }
}

/// Scalar reference: one creature's eyelid value.
pub fn blink_one(t: f32, rate_hz: f32, seed: u64) -> f32 {
    kernel(t, rate_hz, seed)
}

/// Batched: eyelid value for every creature at shared time `t`. Each input is
/// `(rate_hz, seed)`; output is in order.
pub fn blink_batch(states: &[(f32, u64)], t: f32) -> Vec<f32> {
    crate::util::batch_kernel::map_collect(states, |&(rate_hz, seed)| kernel(t, rate_hz, seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (f32, u64) {
        let rate = (lcg(seed) % 300) as f32 / 100.0; // [0, 3) Hz, includes 0
        let s = ((lcg(seed) as u64) << 20) ^ lcg(seed) as u64;
        (rate, s)
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB17C_u32;
        for _ in 0..1500 {
            let n = (lcg(&mut seed) % 20) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let batch = blink_batch(&states, t);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (r, s) = states[i];
                assert_eq!(batch[i].to_bits(), blink_one(t, r, s).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(blink_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (r, s) = rand_state(&mut seed);
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let b = blink_batch(&[(r, s)], t);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), blink_one(t, r, s).to_bits());
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (r, s) = rand_state(&mut seed);
            let t = (lcg(&mut seed) % 100_000) as f32 / 100.0;
            let v = blink_one(t, r, s);
            assert!(v >= 0.0 && v <= 1.0, "eyelid {}", v);
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
            assert_eq!(blink_batch(&states, t), blink_batch(&states, t));
        }
    }

    #[test]
    fn rate_zero_entry_is_open() {
        // a zero-rate creature in the batch reads fully open
        let b = blink_batch(&[(0.0, 0xAA), (1.0, 0xBB)], 12.34);
        assert_eq!(b[0], 0.0);
        assert!(b[1] >= 0.0 && b[1] <= 1.0);
    }
}
