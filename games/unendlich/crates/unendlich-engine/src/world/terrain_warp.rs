//! `terrain_warp` — domain warp to break up grid-aligned noise artifacts.
//!
//! Worker-03 / WORLD pure core, round 16. Offsets sample coordinates by a
//! low-frequency value-noise vector before the terrain is evaluated, so ridges
//! and coastlines meander instead of betraying the underlying axis-aligned
//! lattice. The offset magnitude is bounded by `amp`. Pairs with `ridge_noise`
//! and the base height field (warp the coords, then sample).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Low frequency of the warp field (large-scale meander).
const WARP_FREQ: f32 = 0.01;

/// Warp world coords `(wx, wz)` by a noise vector of magnitude ≤ `amp` per axis.
/// `amp == 0` returns the input unchanged.
pub fn warp(wx: f32, wz: f32, amp: f32, seed: u64) -> (f32, f32) {
    if amp == 0.0 {
        return (wx, wz);
    }
    let nx = value_noise(wx * WARP_FREQ, wz * WARP_FREQ, seed ^ 0x1111) * 2.0 - 1.0; // [-1,1]
    let nz = value_noise(wx * WARP_FREQ, wz * WARP_FREQ, seed ^ 0x2222) * 2.0 - 1.0;
    (wx + amp * nx, wz + amp * nz)
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

// r193 (guide 4.1): shared splitmix.
fn mix(x: u64) -> u64 { crate::util::hash_util::mix64(x) }
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amp_zero_is_identity() {
        assert_eq!(warp(123.0, -45.0, 0.0, 7), (123.0, -45.0));
    }

    #[test]
    fn offset_bounded_by_amp() {
        let amp = 20.0;
        for i in 0..200 {
            let (x, z) = (i as f32 * 3.1, i as f32 * -2.7);
            let (ox, oz) = warp(x, z, amp, 42);
            assert!((ox - x).abs() <= amp + 1e-3);
            assert!((oz - z).abs() <= amp + 1e-3);
        }
    }

    #[test]
    fn continuous() {
        let (a, _) = warp(100.0, 50.0, 10.0, 1);
        let (b, _) = warp(100.01, 50.0, 10.0, 1);
        assert!((a - b).abs() < 0.5, "warp not continuous");
    }

    #[test]
    fn deterministic() {
        assert_eq!(warp(10.0, 20.0, 5.0, 9), warp(10.0, 20.0, 5.0, 9));
    }

    #[test]
    fn seed_sensitive() {
        assert_ne!(warp(10.0, 20.0, 5.0, 1), warp(10.0, 20.0, 5.0, 2));
    }

    #[test]
    fn offsets_go_both_directions() {
        // The warp isn't biased to one side: dx is positive somewhere and
        // negative somewhere (roughly symmetric).
        let mut pos = false;
        let mut neg = false;
        for i in 0..500 {
            let x = i as f32 * 13.0;
            let (ox, _) = warp(x, 0.0, 8.0, 3);
            if ox - x > 0.1 {
                pos = true;
            }
            if ox - x < -0.1 {
                neg = true;
            }
        }
        assert!(pos && neg, "warp offset is one-sided");
    }
}
