//! `batch_gen_estimate` — batched generation-cost estimates (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 23. Estimates many chunks' gen costs in
//! one call, producing **bit-identical** values to the scalar reference (a
//! verbatim mirror of R22 `gen_estimate`).
//!
//! PERF: the scheduler estimates a cost for every candidate chunk each frame
//! (the prefetch ring + load queue, hundreds to thousands of chunks). Batching
//! walks a contiguous `(dist, complexity)` slice; because each entry runs the
//! exact scalar `gen_one`, costs are identical to per-chunk calls — a batched
//! budgeting path can't admit a different set of chunks than a scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FLOOR: u32 = 8;
const COMPLEXITY_MAX: f32 = 200.0;
const LOD_SCALE: f32 = 64.0;

/// Scalar reference — identical to `gen_estimate::gen_cost` (R22).
pub fn gen_one(dist: f32, biome_complexity: f32) -> u32 {
    let c = clamp01(biome_complexity);
    let d = dist.max(0.0);
    let lod = 1.0 / (1.0 + d / LOD_SCALE);
    let detail = c * COMPLEXITY_MAX * lod;
    let cost = FLOOR as f32 + detail;
    let capped = cost.min(FLOOR as f32 + COMPLEXITY_MAX);
    capped.round() as u32
}

/// Batched gen costs over `(dist, complexity)` chunks.
/// Identical to calling `gen_one` per chunk.
pub fn gen_batch(chunks: &[(f32, f32)]) -> Vec<u32> {
    chunks.iter().map(|&(d, c)| gen_one(d, c)).collect()
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_matches_scalar() {
        let c = [
            (0.0f32, 1.0),
            (500.0, 1.0),
            (40.0, 0.5),
            (-10.0, 2.0), // clamps
            (64.0, 0.0),
        ];
        let batch = gen_batch(&c);
        for (i, &(d, cx)) in c.iter().enumerate() {
            assert_eq!(batch[i], gen_one(d, cx), "cost drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(gen_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = gen_one(40.0, 0.6);
        assert_eq!(gen_batch(&[(40.0, 0.6)]), vec![one]);
    }

    #[test]
    fn bounded() {
        for v in gen_batch(&[(0.0, 1.0), (1000.0, 1.0), (50.0, 0.5), (0.0, 0.0)]) {
            assert!(v >= FLOOR && v <= FLOOR + COMPLEXITY_MAX as u32);
        }
    }

    #[test]
    fn deterministic() {
        let c = [(40.0f32, 0.6), (40.0, 0.6)];
        assert_eq!(gen_batch(&c), gen_batch(&c));
    }

    #[test]
    fn zero_complexity_floor_parity() {
        let c = [(0.0f32, 0.0), (100.0, 0.0)];
        let b = gen_batch(&c);
        assert!(b.iter().all(|&v| v == FLOOR));
        assert_eq!(b[0], gen_one(0.0, 0.0));
    }
}
