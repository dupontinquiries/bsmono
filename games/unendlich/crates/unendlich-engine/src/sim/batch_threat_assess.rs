//! `sim::batch_threat_assess` — batched threat scoring, mirroring
//! `sim::threat_assess` (round 17), with a scalar reference for parity.
//!
//! [`threat_one`] scores one `(distance, power)` threat exactly as the r17 core
//! does (`power / (1 + distance)`, inputs floored at 0); [`threat_batch`] scores
//! a whole perception list at once. Both route through the same `#[inline]`
//! `kernel`, so the batch is **bit-for-bit identical** to scoring each threat
//! individually — picking the max over the returned scores reproduces r17's
//! `top_threat`.
//!
//! Perf shape: threats are read from a contiguous `(dist, power)` slice and
//! written to a contiguous `Vec<f32>` of scores in one streaming pass, no
//! cross-threat dependency — the scoring an AI does for every nearby entity each
//! tick, amortized into a single call. Pure and deterministic.

/// The shared scoring kernel (identical to r17 `threat_assess::threat_score`).
#[inline]
fn kernel(dist: f32, power: f32) -> f32 {
    let d = if dist > 0.0 { dist } else { 0.0 };
    let p = if power > 0.0 { power } else { 0.0 };
    p / (1.0 + d)
}

/// Scalar reference: danger score for one threat.
pub fn threat_one(dist: f32, power: f32) -> f32 {
    kernel(dist, power)
}

/// Batched: danger score for every `(dist, power)` threat, in order.
// r330-refactor-batch: `_batch` loop folded into the shared helper (bit-identical).
use crate::util::batch_kernel::map_collect;
pub fn threat_batch(threats: &[(f32, f32)]) -> Vec<f32> {
    map_collect(threats, |&(d, p)| kernel(d, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rf(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 100.0 - 1000.0
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB17C_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 16) as usize;
            let mut threats = Vec::new();
            for _ in 0..n {
                threats.push((rf(&mut seed), rf(&mut seed)));
            }
            let batch = threat_batch(&threats);
            assert_eq!(batch.len(), threats.len());
            for i in 0..threats.len() {
                let (d, p) = threats[i];
                assert_eq!(batch[i].to_bits(), threat_one(d, p).to_bits());
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(threat_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let d = rf(&mut seed);
            let p = rf(&mut seed);
            let b = threat_batch(&[(d, p)]);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), threat_one(d, p).to_bits());
        }
    }

    #[test]
    fn bounded_nonnegative() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let d = rf(&mut seed);
            let p = rf(&mut seed);
            let s = threat_one(d, p);
            assert!(s >= 0.0 && s.is_finite());
            let pc = if p > 0.0 { p } else { 0.0 };
            assert!(s <= pc + 1e-3);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 10) as usize;
            let mut threats = Vec::new();
            for _ in 0..n {
                threats.push((rf(&mut seed), rf(&mut seed)));
            }
            assert_eq!(threat_batch(&threats), threat_batch(&threats));
        }
    }

    #[test]
    fn max_score_reproduces_top_threat() {
        // scoring then taking the argmax matches r17's intent
        let threats = [(10.0, 5.0), (1.0, 9.0), (8.0, 4.0)];
        let scores = threat_batch(&threats);
        let mut best = 0;
        let mut i = 1;
        while i < scores.len() {
            if scores[i] > scores[best] {
                best = i;
            }
            i += 1;
        }
        assert_eq!(best, 1); // nearest + strongest
    }
}
