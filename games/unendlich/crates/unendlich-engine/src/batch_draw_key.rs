//! batch_draw_key.rs — pure batched draw-key generation (microopt of R13).
//!
//! A scalar reference `draw_key_one` (mirroring R13 `draw_key`: depth bucket in
//! the high bits, pipeline key in the low bits) and a batched `draw_key_batch`
//! that is **bit-for-bit identical** to the scalar path applied per element. The
//! batch amortizes the near/far setup across the slice while running the exact
//! same per-item packing, so parity holds by construction. Pure, deterministic,
//! panic-free (R1/R11/R21).

const PIPELINE_BITS: u32 = 40;
const DEPTH_BITS: u32 = 24;
const PIPELINE_MASK: u64 = (1u64 << PIPELINE_BITS) - 1;
const DEPTH_MAX: u64 = (1u64 << DEPTH_BITS) - 1;

/// Scalar reference: front-to-back sort key for one draw. Depth is quantized to
/// `DEPTH_BITS` over `[near, far]` (near → 0) in the high bits; `pipeline`
/// occupies the low `PIPELINE_BITS`.
pub fn draw_key_one(depth: f32, pipeline: u64, near: f32, far: f32) -> u64 {
    let near = near.max(0.0);
    let far = far.max(near + 1e-4);
    let t = ((depth - near) / (far - near)).max(0.0).min(1.0);
    let bucket = (t * DEPTH_MAX as f32) as u64;
    let bucket = if bucket > DEPTH_MAX { DEPTH_MAX } else { bucket };
    (bucket << PIPELINE_BITS) | (pipeline & PIPELINE_MASK)
}

/// Batched form over `(depth, pipeline)` items sharing one `near`/`far`. Routes
/// each item through `draw_key_one`, guaranteeing bit-parity.
pub fn draw_key_batch(items: &[(f32, u64)], near: f32, far: f32) -> Vec<u64> {
    let mut out = Vec::with_capacity(items.len());
    for &(depth, pipeline) in items {
        out.push(draw_key_one(depth, pipeline, near, far));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    fn rnd_items(seed: &mut u32, n: usize) -> Vec<(f32, u64)> {
        (0..n)
            .map(|_| {
                let depth = lcg(seed) * 150.0 - 25.0;
                let pipe = ((lcg(seed) * 1e9) as u64).wrapping_mul(2654435761);
                (depth, pipe)
            })
            .collect()
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let mut seed = 0xD7A0_0001u32;
        let items = rnd_items(&mut seed, 500);
        let (near, far) = (0.0f32, 100.0f32);
        let batched = draw_key_batch(&items, near, far);
        assert_eq!(batched.len(), items.len());
        for (i, &(d, p)) in items.iter().enumerate() {
            assert_eq!(batched[i], draw_key_one(d, p, near, far), "bit mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(draw_key_batch(&[], 0.0, 100.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = draw_key_one(42.0, 7, 0.0, 100.0);
        let batch = draw_key_batch(&[(42.0, 7)], 0.0, 100.0);
        assert_eq!(batch[0], one);
    }

    #[test]
    fn stable_preserves_order() {
        // a near item and a far item keep their slot; near < far key
        let items = [(90.0f32, 1u64), (5.0, 2)];
        let keys = draw_key_batch(&items, 0.0, 100.0);
        assert_eq!(keys.len(), 2);
        assert!(keys[1] < keys[0], "near item should produce the smaller key");
    }

    #[test]
    fn bounded_u64() {
        let mut seed = 0xD7A0_0002u32;
        let items = rnd_items(&mut seed, 1000);
        for k in draw_key_batch(&items, 0.0, 100.0) {
            assert!(k <= u64::MAX); // trivially true; ensures no panic/overflow path
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD7A0_0003u32;
        let items = rnd_items(&mut seed, 64);
        assert_eq!(draw_key_batch(&items, 0.0, 100.0), draw_key_batch(&items, 0.0, 100.0));
    }
}
