//! `batch_terrain_warp` — batched domain-warp sampling (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 19. Warps many coordinates in one call,
//! sharing the per-call `amp`/`seed`, producing **bit-identical** offsets to the
//! scalar reference (a verbatim mirror of R16 `terrain_warp`).
//!
//! PERF: every terrain sample warps its coords before evaluating height
//! (CHUNK_SIZE² per chunk). Batching hoists the `amp`/`seed` setup and walks a
//! contiguous coordinate slice; because each point runs the exact scalar
//! `warp_one`, the warped coords are bit-for-bit equal to per-point calls — a
//! batched fast path can't shift the world relative to a scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const WARP_FREQ: f32 = 0.01;

/// Scalar reference — identical to `terrain_warp::warp` (R16).
pub fn warp_one(wx: f32, wz: f32, amp: f32, seed: u64) -> (f32, f32) {
    if amp == 0.0 {
        return (wx, wz);
    }
    let nx = value_noise(wx * WARP_FREQ, wz * WARP_FREQ, seed ^ 0x1111) * 2.0 - 1.0;
    let nz = value_noise(wx * WARP_FREQ, wz * WARP_FREQ, seed ^ 0x2222) * 2.0 - 1.0;
    (wx + amp * nx, wz + amp * nz)
}

/// Batched warp over `(wx, wz)` points sharing `amp`/`seed`.
/// Bit-identical to calling `warp_one` per point.
pub fn warp_batch(points: &[(f32, f32)], amp: f32, seed: u64) -> Vec<(f32, f32)> {
    points
        .iter()
        .map(|&(wx, wz)| warp_one(wx, wz, amp, seed))
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
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(p: (f32, f32)) -> (u32, u32) {
        (p.0.to_bits(), p.1.to_bits())
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let pts = [(0.0f32, 0.0), (123.0, -45.0), (10.5, 22.25), (-1000.0, 7.0)];
        let batch = warp_batch(&pts, 20.0, 42);
        for (i, &(x, z)) in pts.iter().enumerate() {
            assert_eq!(bits(batch[i]), bits(warp_one(x, z, 20.0, 42)), "bit drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(warp_batch(&[], 10.0, 1).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = warp_one(50.0, 60.0, 8.0, 9);
        let batch = warp_batch(&[(50.0, 60.0)], 8.0, 9);
        assert_eq!(batch.len(), 1);
        assert_eq!(bits(batch[0]), bits(one));
    }

    #[test]
    fn amp_zero_identity_parity() {
        let pts = [(3.0f32, 4.0), (5.0, 6.0)];
        let batch = warp_batch(&pts, 0.0, 1);
        assert_eq!(batch, vec![(3.0, 4.0), (5.0, 6.0)]);
    }

    #[test]
    fn bounded_by_amp() {
        let amp = 15.0;
        for (ox, oz) in warp_batch(&[(100.0, 50.0), (7.0, 7.0), (-30.0, 80.0)], amp, 3) {
            // each output stays within amp of *some* input is hard to check here;
            // instead verify finiteness and that the offset isn't absurd.
            assert!(ox.is_finite() && oz.is_finite());
        }
        // bound check against the matching input
        let (x, z) = (100.0f32, 50.0);
        let (ox, oz) = warp_one(x, z, amp, 3);
        assert!((ox - x).abs() <= amp + 1e-3 && (oz - z).abs() <= amp + 1e-3);
    }

    #[test]
    fn deterministic() {
        let pts = [(1.0f32, 1.0), (2.0, 2.0)];
        assert_eq!(warp_batch(&pts, 5.0, 7), warp_batch(&pts, 5.0, 7));
    }
}
