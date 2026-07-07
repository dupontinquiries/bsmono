//! Far-LOD plant shimmer — a cheap procedural stand-in for re-meshing distant
//! vegetation.
//!
//! When [`crate::world::remesh_policy`] skips a far chunk's plant growth, that
//! chunk's geometry is frozen. A perfectly static far field reads as *dead*,
//! which is the opposite of what the skipped growth was trying to convey. So
//! instead of paying for geometry, the shader overlays a position + time noise
//! on vegetation-tinted surfaces: a subtle drifting variation that *implies*
//! living, shifting plant cover for free.
//!
//! All pure, deterministic, no allocation — the fragment shader calls
//! [`plant_shimmer`] per far surface sample (or the mesher bakes a coarse
//! version per far vertex). The amplitude is tiny by design; this is a hint of
//! life at distance, not a visible texture.

/// Peak signed modulation returned by [`plant_shimmer`] (before the vegetation
/// scale). Small on purpose — distant grass shouldn't strobe.
pub const SHIMMER_AMPLITUDE: f32 = 0.06;

/// Deterministic hash of an integer lattice point → `[0, 1)`. Cheap integer
/// bit-mixing (no float transcendentals), stable across platforms.
fn hash2(ix: i32, iz: i32) -> f32 {
    let mut h = (ix as u32).wrapping_mul(0x27d4_eb2d) ^ (iz as u32).wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x2974_2c7d);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 0x0100_0000 as f32
}

fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Smooth 2D value noise in `[0, 1]` — bilinear-interpolated lattice hashes with
/// a smootherstep fade so there are no grid creases.
pub fn value_noise_2d(x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let (ix, iz) = (x0 as i32, z0 as i32);
    let fx = smootherstep(x - x0);
    let fz = smootherstep(z - z0);
    let c00 = hash2(ix, iz);
    let c10 = hash2(ix + 1, iz);
    let c01 = hash2(ix, iz + 1);
    let c11 = hash2(ix + 1, iz + 1);
    let bottom = c00 + (c10 - c00) * fx;
    let top = c01 + (c11 - c01) * fx;
    bottom + (top - bottom) * fz
}

/// A small signed brightness/tint modulation for vegetation at world column
/// `(world_x, world_z)` and `time` seconds, scaled by `vegetation` in `[0,1]`
/// (how grassy that surface is). Two noise octaves drift in opposite directions
/// over time so the field shimmers without an obvious repeat. Returns a value
/// in roughly `[-SHIMMER_AMPLITUDE, SHIMMER_AMPLITUDE] · vegetation`; the shader
/// adds it to the vegetation green/brightness of far surfaces.
pub fn plant_shimmer(world_x: f32, world_z: f32, time: f32, vegetation: f32) -> f32 {
    let v = vegetation.clamp(0.0, 1.0);
    if v <= 0.0 {
        return 0.0;
    }
    // Low frequencies (large patches) drifting slowly — coarse LOD can't show
    // anything finer anyway.
    let n1 = value_noise_2d(world_x * 0.08 + time * 0.15, world_z * 0.08);
    let n2 = value_noise_2d(world_x * 0.21, world_z * 0.21 - time * 0.10);
    let combined = n1 * 0.6 + n2 * 0.4; // [0,1]
    let signed = (combined - 0.5) * 2.0; // [-1,1]
    signed * SHIMMER_AMPLITUDE * v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_noise_is_bounded_and_smooth() {
        let mut prev = value_noise_2d(0.0, 0.0);
        for i in 0..200 {
            let x = i as f32 * 0.05;
            let n = value_noise_2d(x, 3.7);
            assert!((0.0..=1.0).contains(&n), "noise out of range: {n}");
            // Smoothness: small steps → small changes (no lattice creases).
            assert!((n - prev).abs() < 0.5, "noise jumped too far: {prev}→{n}");
            prev = n;
        }
    }

    #[test]
    fn shimmer_is_deterministic() {
        let a = plant_shimmer(123.0, 45.0, 7.5, 0.8);
        let b = plant_shimmer(123.0, 45.0, 7.5, 0.8);
        assert_eq!(a, b, "same inputs must give the same value");
    }

    #[test]
    fn shimmer_stays_within_amplitude() {
        // Scan space and time; never exceed the vegetation-scaled amplitude.
        for xi in 0..40 {
            for ti in 0..40 {
                let x = xi as f32 * 2.3;
                let t = ti as f32 * 0.7;
                let s = plant_shimmer(x, x * 0.5, t, 1.0);
                assert!(s.abs() <= SHIMMER_AMPLITUDE + 1e-6, "over amplitude: {s}");
            }
        }
    }

    #[test]
    fn no_vegetation_means_no_shimmer() {
        assert_eq!(plant_shimmer(10.0, 20.0, 5.0, 0.0), 0.0);
        // And the modulation scales with vegetation density.
        let light = plant_shimmer(10.0, 20.0, 5.0, 0.2).abs();
        let heavy = plant_shimmer(10.0, 20.0, 5.0, 1.0).abs();
        assert!(heavy >= light, "denser vegetation shimmers at least as much");
    }

    #[test]
    fn shimmer_animates_over_time() {
        // At a fixed point, the value should change across time (it's alive).
        let t0 = plant_shimmer(50.0, 50.0, 0.0, 1.0);
        let mut moved = false;
        for ti in 1..60 {
            if (plant_shimmer(50.0, 50.0, ti as f32 * 0.5, 1.0) - t0).abs() > 1e-4 {
                moved = true;
                break;
            }
        }
        assert!(moved, "shimmer should vary over time");
    }

    #[test]
    fn shimmer_varies_across_space() {
        let a = plant_shimmer(0.0, 0.0, 1.0, 1.0);
        let b = plant_shimmer(37.0, 12.0, 1.0, 1.0);
        assert!((a - b).abs() > 1e-4, "different columns should differ");
    }
}
