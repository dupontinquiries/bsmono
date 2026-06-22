//! refraction_offset.rs — pure screen-space refraction UV offset.
//!
//! Bends the background sample by the surface normal's screen-space tilt, scaled
//! by an IOR `strength` and attenuated with `depth`. Pure, deterministic,
//! panic-free (R1/R11/R21). Offset clamped per component to `±0.5`.

/// Refraction UV offset from a surface normal's XY tilt `normal_xy`, `strength`,
/// and `depth`. Flat normal ⇒ 0; offset grows with tilt and strength and is
/// attenuated `1/(1+depth)`. Clamped to `±0.5` per component.
pub fn refract_uv(normal_xy: [f32; 2], strength: f32, depth: f32) -> [f32; 2] {
    let atten = 1.0 / (1.0 + depth.max(0.0)); // deeper -> less offset
    let s = strength * atten;
    let ox = normal_xy[0] * s;
    let oy = normal_xy[1] * s;
    [ox.max(-0.5).min(0.5), oy.max(-0.5).min(0.5)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mag(o: [f32; 2]) -> f32 {
        (o[0] * o[0] + o[1] * o[1]).sqrt()
    }

    #[test]
    fn flat_normal_is_zero() {
        assert_eq!(refract_uv([0.0, 0.0], 1.0, 0.0), [0.0, 0.0]);
    }

    #[test]
    fn tilt_offsets() {
        let o = refract_uv([0.4, -0.3], 0.5, 0.0);
        assert!(o[0] != 0.0 && o[1] != 0.0);
    }

    #[test]
    fn strength_scales() {
        let a = refract_uv([0.2, 0.1], 0.3, 0.0);
        let b = refract_uv([0.2, 0.1], 0.6, 0.0);
        assert!((b[0] - 2.0 * a[0]).abs() < 1e-6 && (b[1] - 2.0 * a[1]).abs() < 1e-6);
    }

    #[test]
    fn depth_attenuates() {
        let near = mag(refract_uv([0.3, 0.3], 0.4, 0.0));
        let far = mag(refract_uv([0.3, 0.3], 0.4, 10.0));
        assert!(far < near, "depth should attenuate the offset");
    }

    #[test]
    fn bounded() {
        let mut seed = 0x2527_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let o = refract_uv([lcg() * 4.0 - 2.0, lcg() * 4.0 - 2.0], lcg() * 10.0, lcg() * 20.0 - 1.0);
            assert!(o[0].abs() <= 0.5 + 1e-6 && o[1].abs() <= 0.5 + 1e-6, "offset oob {:?}", o);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(refract_uv([0.3, -0.2], 0.5, 2.0), refract_uv([0.3, -0.2], 0.5, 2.0));
    }
}
