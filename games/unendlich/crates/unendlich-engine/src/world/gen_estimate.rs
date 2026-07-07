//! `gen_estimate` — per-chunk generation-cost estimate for budgeting.
//!
//! Worker-03 / WORLD pure core, round 22 (GPU_COMPUTE_PLAN #5). Estimates the
//! relative cost of generating a chunk so `chunk_schedule` can stay within a
//! per-frame budget. A floor covers the fixed per-chunk overhead; biome
//! complexity adds detail cost; distance scales the detail down (far chunks
//! generate at coarser LOD), so the estimate is highest for near, complex
//! chunks and floors out far away.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const FLOOR: u32 = 8; // fixed per-chunk overhead
const COMPLEXITY_MAX: f32 = 200.0; // extra cost at full complexity, near
const LOD_SCALE: f32 = 64.0; // distance at which detail cost ~halves

/// Estimated generation cost (arbitrary units) for a chunk at `dist` with
/// `biome_complexity` (≈[0,1]). In `[FLOOR, FLOOR + COMPLEXITY_MAX]`.
pub fn gen_cost(dist: f32, biome_complexity: f32) -> u32 {
    let c = clamp01(biome_complexity);
    let d = dist.max(0.0);
    let lod = 1.0 / (1.0 + d / LOD_SCALE); // (0,1], 1 near → 0 far
    let detail = c * COMPLEXITY_MAX * lod;
    let cost = FLOOR as f32 + detail;
    let capped = cost.min(FLOOR as f32 + COMPLEXITY_MAX);
    capped.round() as u32
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn farther_costs_less_for_complex_chunks() {
        // Detail (and thus cost) shrinks with distance for a complex chunk.
        assert!(gen_cost(0.0, 1.0) > gen_cost(500.0, 1.0));
    }

    #[test]
    fn complex_costs_higher() {
        assert!(gen_cost(0.0, 0.9) > gen_cost(0.0, 0.1));
    }

    #[test]
    fn bounded() {
        for di in 0..50 {
            for ci in 0..=10 {
                let v = gen_cost(di as f32 * 20.0, ci as f32 / 10.0);
                assert!(v >= FLOOR && v <= FLOOR + COMPLEXITY_MAX as u32);
            }
        }
    }

    #[test]
    fn monotone_decreasing_in_distance() {
        let mut prev = u32::MAX;
        for d in 0..400 {
            let v = gen_cost(d as f32, 1.0);
            assert!(v <= prev, "cost rose with distance");
            prev = v;
        }
    }

    #[test]
    fn zero_complexity_is_the_floor() {
        for d in [0.0f32, 50.0, 1000.0] {
            assert_eq!(gen_cost(d, 0.0), FLOOR);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(gen_cost(40.0, 0.6), gen_cost(40.0, 0.6));
    }
}
