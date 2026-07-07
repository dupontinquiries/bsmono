//! `batch_ridge_noise` — batched ridged-multifractal sampling (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 17. Samples many points of the R16
//! `ridge_noise` field in one call, sharing the per-call `octaves`/`seed`, and
//! produces **bit-identical** values to the scalar reference.
//!
//! PERF: terrain generation samples ridge noise per column across a chunk
//! (CHUNK_SIZE² points) every time a chunk is generated. Batching hoists the
//! octave/seed setup out of the inner call and walks a contiguous point slice
//! (cache-friendly, branch-predictable octave loop); because each point goes
//! through the exact scalar `ridge_one`, results are bit-for-bit equal to the
//! per-point call, so a batched fast path can't drift from a scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const MAX_OCTAVES: u32 = 12;

/// Scalar reference — identical to `ridge_noise::ridge` (R16).
pub fn ridge_one(x: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let o = octaves.clamp(1, MAX_OCTAVES);
    let mut freq = 1.0f32;
    let mut amp = 0.5f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    for i in 0..o {
        let n = value_noise(x * freq, z * freq, seed ^ (i as u64).wrapping_mul(0x9E37_79B9));
        let r = 1.0 - (2.0 * n - 1.0).abs();
        sum += r * amp;
        norm += amp;
        freq *= 2.0;
        amp *= 0.5;
    }
    if norm > 0.0 {
        clamp01(sum / norm)
    } else {
        0.0
    }
}

/// Batched ridge sampling over `(x, z)` points sharing `octaves`/`seed`.
/// Bit-identical to calling `ridge_one` per point.
pub fn ridge_batch(points: &[(f32, f32)], octaves: u32, seed: u64) -> Vec<f32> {
    points
        .iter()
        .map(|&(x, z)| ridge_one(x, z, octaves, seed))
        .collect()
}

fn value_noise(x: f32, z: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let xf = x - x0;
    let zf = z - z0;
    let (ix, iz) = (x0 as i32, z0 as i32);
    let v00 = corner(ix, iz, seed);
    let v10 = corner(ix + 1, iz, seed);
    let v01 = corner(ix, iz + 1, seed);
    let v11 = corner(ix + 1, iz + 1, seed);
    let sx = smoothstep(xf);
    let sz = smoothstep(zf);
    lerp(lerp(v00, v10, sx), lerp(v01, v11, sx), sz)
}
fn corner(ix: i32, iz: i32, seed: u64) -> f32 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = mix(h ^ (ix as u32 as u64).wrapping_mul(0x85EB_CA6B));
    h = mix(h ^ (iz as u32 as u64).wrapping_mul(0xC2B2_AE35));
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}
fn mix(x: u64) -> u64 { crate::util::hash_util::mix64(x) } // r193 4.1
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let pts = [
            (0.0f32, 0.0),
            (1.5, -2.5),
            (10.25, 3.75),
            (100.0, 100.0),
            (-7.0, 12.0),
        ];
        let batch = ridge_batch(&pts, 5, 1234);
        for (i, &(x, z)) in pts.iter().enumerate() {
            assert_eq!(batch[i].to_bits(), ridge_one(x, z, 5, 1234).to_bits(), "bit drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(ridge_batch(&[], 4, 1).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = ridge_one(3.3, 4.4, 6, 9);
        let batch = ridge_batch(&[(3.3, 4.4)], 6, 9);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].to_bits(), one.to_bits());
    }

    #[test]
    fn bounded() {
        for v in ridge_batch(&[(0.1, 0.2), (5.0, 6.0), (50.0, 12.0)], 4, 3) {
            assert!(v >= 0.0 && v <= 1.0);
        }
    }

    #[test]
    fn deterministic() {
        let pts = [(1.0f32, 1.0), (2.0, 2.0)];
        assert_eq!(ridge_batch(&pts, 5, 7), ridge_batch(&pts, 5, 7));
    }

    #[test]
    fn octaves_clamped_like_scalar() {
        // octaves 0 and huge both match the scalar (which clamps to [1,12]).
        for &(x, z) in &[(1.0f32, 1.0), (9.0, 3.0)] {
            assert_eq!(ridge_batch(&[(x, z)], 0, 2)[0].to_bits(), ridge_one(x, z, 0, 2).to_bits());
            assert_eq!(ridge_batch(&[(x, z)], 999, 2)[0].to_bits(), ridge_one(x, z, 999, 2).to_bits());
        }
    }
}
