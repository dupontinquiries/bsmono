//! rim_light.rs — pure Fresnel rim term for silhouette glow.
//!
//! `(1 - clamp(n·v, 0, 1))^power`: bright at grazing angles (the silhouette
//! edge), dark when facing the viewer. Pure, deterministic, panic-free
//! (R1/R11/R21). Result in `[0,1]`.

/// Rim term for view alignment `n_dot_v` and falloff `power`. `1` at the edge
/// (`n·v → 0`), `0` facing (`n·v → 1`).
pub fn rim(n_dot_v: f32, power: f32) -> f32 {
    let ndv = crate::util::mathx::saturate(n_dot_v);
    let p = power.max(0.0);
    crate::util::mathx::saturate((1.0 - ndv).powf(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_is_bright() {
        assert!((rim(0.0, 3.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn facing_is_dark() {
        assert!(rim(1.0, 3.0) < 1e-6);
    }

    #[test]
    fn power_sharpens() {
        // higher power -> narrower rim -> smaller value at partial grazing
        assert!(rim(0.5, 8.0) < rim(0.5, 2.0));
    }

    #[test]
    fn clamped_unit_range() {
        let mut seed = 0x1917_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let r = rim(lcg() * 3.0 - 1.0, lcg() * 16.0);
            assert!((0.0..=1.0).contains(&r), "rim oob {}", r);
        }
    }

    #[test]
    fn monotone_decreasing_in_alignment() {
        let mut prev = 2.0f32;
        let mut x = 0.0f32;
        while x <= 1.0 {
            let r = rim(x, 4.0);
            assert!(r <= prev + 1e-6, "rim not decreasing at {}", x);
            prev = r;
            x += 0.01;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(rim(0.3, 5.0), rim(0.3, 5.0));
    }
}
