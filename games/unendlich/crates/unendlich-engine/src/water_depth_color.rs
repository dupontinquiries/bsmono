//! Depth-based water color blend factor: 0 = shallow tint, 1 = deep tint,
//! saturating with depth over `depth_scale`. Pure, deterministic.

pub fn deep_factor(depth: f32, depth_scale: f32) -> f32 {
    let ds = depth_scale.max(1e-6);
    let d = depth.max(0.0);
    (1.0 - (-(d / ds)).exp()).clamp(0.0, 1.0)
}

// ---- r335-water-visual: depth color ramp + per-channel absorption ----------
// (brief header is `r334-water-visual`; re-grounded under the live WID. Grounds
//  the `water_color(depth, turbidity)` absorption-tint contract in docs/WATER.md.)

/// Shallow→deep surface tint (linear RGB 0..1). `shallow` is the near-shore
/// colour, `deep` the colour the surface saturates toward in deep water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterTint {
    pub shallow: [f32; 3],
    pub deep: [f32; 3],
}

impl WaterTint {
    /// A sensible default: clear teal shallows easing to a dark blue-green deep.
    pub fn default_tint() -> WaterTint {
        WaterTint { shallow: [0.30, 0.62, 0.66], deep: [0.02, 0.10, 0.18] }
    }
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// The surface tint at a given depth: lerp `shallow→deep` by [`deep_factor`].
/// Bounded per channel to the `[shallow, deep]` interval (so a 0..1 tint stays
/// 0..1). The shader mixes this over the refracted scene with Fresnel.
pub fn depth_ramp(depth: f32, depth_scale: f32, tint: WaterTint) -> [f32; 3] {
    lerp3(tint.shallow, tint.deep, deep_factor(depth, depth_scale))
}

/// Beer–Lambert per-channel transmittance through `depth` of water given
/// per-channel absorption `coeffs` (1/length). Red is absorbed fastest, so deep
/// water reads blue/teal. Each channel is `exp(-coeff·depth)` in `[0,1]`; at
/// `depth = 0` it is `1` (no absorption). This multiplies the refracted scene.
pub fn absorption(depth: f32, coeffs: [f32; 3]) -> [f32; 3] {
    let d = depth.max(0.0);
    let ch = |c: f32| (-c.max(0.0) * d).exp().clamp(0.0, 1.0);
    [ch(coeffs[0]), ch(coeffs[1]), ch(coeffs[2])]
}

/// Typical clear-water absorption coefficients (red ≫ green > blue).
pub fn default_absorption() -> [f32; 3] {
    [0.45, 0.10, 0.04]
}

/// Convenience: the refracted colour the shader shows looking into the water —
/// the bed/scene colour attenuated by [`absorption`] over `depth`. Each channel
/// stays in `[0, scene]`, never amplifying.
pub fn apply_absorption(scene: [f32; 3], depth: f32, coeffs: [f32; 3]) -> [f32; 3] {
    let a = absorption(depth, coeffs);
    [scene[0] * a[0], scene[1] * a[1], scene[2] * a[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn surface_is_shallow() { assert_eq!(deep_factor(0.0, 10.0), 0.0); }
    #[test] fn deep_approaches_one() { assert!(deep_factor(1000.0, 10.0) > 0.99); }
    #[test] fn monotone_increasing() {
        let (mut prev, mut d) = (f32::MIN, 0.0f32);
        while d < 50.0 { let v = deep_factor(d, 10.0); assert!(v >= prev - 1e-6); prev = v; d += 1.0; }
    }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 100.0 { assert!((0.0..=1.0).contains(&deep_factor(d, 10.0))); d += 2.0; }
    }
    #[test] fn scale_shifts() { assert!(deep_factor(10.0, 5.0) > deep_factor(10.0, 50.0)); }
    #[test] fn deterministic() { assert_eq!(deep_factor(10.0, 10.0).to_bits(), deep_factor(10.0, 10.0).to_bits()); }

    // ---- r335-water-visual tests ----
    fn in01(c: [f32; 3]) -> bool { c.iter().all(|v| (0.0..=1.0).contains(v) && v.is_finite()) }

    #[test]
    fn ramp_endpoints_and_monotone() {
        let tint = WaterTint::default_tint();
        // at depth 0 the ramp is the shallow tint; very deep → ~deep tint.
        assert_eq!(depth_ramp(0.0, 10.0, tint), tint.shallow);
        let deep = depth_ramp(10_000.0, 10.0, tint);
        for k in 0..3 { assert!((deep[k] - tint.deep[k]).abs() < 1e-3); }
        // each channel moves monotonically from shallow toward deep with depth.
        let (mut prev, mut d) = ([f32::MIN; 3], 0.0f32);
        while d < 60.0 {
            let c = depth_ramp(d, 10.0, tint);
            assert!(in01(c));
            for k in 0..3 {
                // deep channel < shallow channel here, so the ramp decreases; allow either dir.
                let toward_deep = (c[k] - tint.deep[k]).abs();
                let prev_toward_deep = (prev[k] - tint.deep[k]).abs();
                assert!(toward_deep <= prev_toward_deep + 1e-6, "ramp not monotone toward deep");
            }
            prev = c; d += 1.0;
        }
    }

    #[test]
    fn absorption_decreases_with_depth_and_bounded() {
        let coeffs = default_absorption();
        assert_eq!(absorption(0.0, coeffs), [1.0, 1.0, 1.0], "no absorption at the surface");
        let (mut prev, mut d) = ([f32::MAX; 3], 0.0f32);
        while d < 30.0 {
            let a = absorption(d, coeffs);
            assert!(in01(a));
            for k in 0..3 { assert!(a[k] <= prev[k] + 1e-6, "absorption not monotone"); }
            prev = a; d += 0.5;
        }
        // red absorbed faster than blue ⇒ deep water tints blue.
        let deep = absorption(8.0, coeffs);
        assert!(deep[0] < deep[2], "red should attenuate faster than blue");
    }

    #[test]
    fn apply_absorption_never_amplifies() {
        let scene = [0.6, 0.7, 0.8];
        let out = apply_absorption(scene, 5.0, default_absorption());
        for k in 0..3 { assert!(out[k] <= scene[k] + 1e-6 && out[k] >= 0.0); }
    }

    #[test]
    fn ramp_and_absorption_deterministic() {
        let t = WaterTint::default_tint();
        assert_eq!(depth_ramp(7.0, 12.0, t), depth_ramp(7.0, 12.0, t));
        assert_eq!(absorption(7.0, default_absorption()), absorption(7.0, default_absorption()));
    }
}
