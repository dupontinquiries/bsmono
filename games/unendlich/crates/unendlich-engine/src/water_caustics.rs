//! water_caustics.rs — pure animated caustic intensity pattern.
//!
//! A cheap procedural caustic: a sum of moving sine waves whose ridges form the
//! bright filaments seen on a pool floor, animated by `t`. All time frequencies
//! are integer multiples of a base, so the pattern is exactly `2π`-periodic in
//! `t`. Output in `[0,1]`. Pure, deterministic, panic-free (R1/R11/R21).

/// Caustic intensity at screen/world `uv`, time `t`, spatial `scale` (higher =
/// finer pattern). Returns a value in `[0,1]`, `2π`-periodic in `t`.
pub fn caustic(uv: [f32; 2], t: f32, scale: f32) -> f32 {
    let x = uv[0] * scale;
    let y = uv[1] * scale;

    // moving wavefronts; t-coefficients are integers -> common period 2π
    let a = (x + t).sin();
    let b = (y - t).sin();
    let c = ((x + y) * 0.7 + 2.0 * t).sin();
    let d = ((x - y) * 0.9 - 2.0 * t).sin();

    let v = (a + b + c + d) * 0.25; // [-1, 1]
    // ridged: bright filaments where wavefronts reinforce
    let ridged = v.abs();
    (ridged * ridged).max(0.0).min(1.0) // sharpen, keep in [0,1]
}

// ---- r335-water-visual: caustic intensity from sun angle + depth -----------
// The raw `caustic` above is just the pattern. Real caustics need sunlight and
// fade with water depth (the focused light spreads and is absorbed before it
// reaches a deep bed). This modulates the pattern by both, → 0 at night.

/// Caustic depth tuning. `caustic_depth`: e-folding depth over which the caustic
/// brightness fades (shallow pools shimmer, deep beds go dark); `horizon_soft`:
/// how softly caustics fade as the sun nears the horizon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CausticParams {
    pub caustic_depth: f32,
    pub horizon_soft: f32,
}

impl CausticParams {
    pub fn default_params() -> CausticParams {
        CausticParams { caustic_depth: 6.0, horizon_soft: 0.15 }
    }
}

/// Sun factor in `[0,1]` from the sun direction (`+y` up): `0` at/below the
/// horizon (night ⇒ no caustics), rising as the sun climbs. `horizon_soft`
/// widens the dawn/dusk fade.
fn sun_factor(sun_dir: [f32; 3], horizon_soft: f32) -> f32 {
    let up = sun_dir[1]; // cos of zenith-ish; <=0 means at/below horizon
    if up <= 0.0 {
        return 0.0;
    }
    let soft = horizon_soft.max(1e-4);
    (up / soft).clamp(0.0, 1.0) * up.clamp(0.0, 1.0).sqrt()
}

/// Modulated caustic intensity at `uv`, time `t`, `scale`, for a given
/// `sun_dir` and water `depth`. `= caustic(uv,t,scale) · sun · exp(-depth/cd)`,
/// in `[0,1]`. Exactly `0` when the sun is at or below the horizon (night), and
/// fades to `0` with depth. Pure, deterministic.
pub fn caustic_intensity(
    uv: [f32; 2],
    t: f32,
    scale: f32,
    sun_dir: [f32; 3],
    depth: f32,
    params: CausticParams,
) -> f32 {
    let sun = sun_factor(sun_dir, params.horizon_soft);
    if sun <= 0.0 {
        return 0.0;
    }
    let cd = params.caustic_depth.max(1e-3);
    let depth_atten = (-(depth.max(0.0)) / cd).exp().clamp(0.0, 1.0);
    (caustic(uv, t, scale) * sun * depth_atten).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn output_in_unit_range() {
        let mut seed = 0xCA05_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..3000 {
            let uv = [lcg() * 4.0 - 2.0, lcg() * 4.0 - 2.0];
            let t = lcg() * 50.0;
            let scale = lcg() * 20.0 + 0.1;
            let v = caustic(uv, t, scale);
            assert!((0.0..=1.0).contains(&v), "caustic oob {}", v);
        }
    }

    #[test]
    fn periodic_in_t() {
        // integer t-frequencies => exactly 2π-periodic
        let mut seed = 0xCA05_0002u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..200 {
            let uv = [lcg() * 3.0, lcg() * 3.0];
            let t = lcg() * 10.0;
            let scale = 4.0;
            assert!((caustic(uv, t, scale) - caustic(uv, t + 2.0 * PI, scale)).abs() < 1e-4);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(caustic([0.4, 0.6], 1.3, 5.0), caustic([0.4, 0.6], 1.3, 5.0));
    }

    #[test]
    fn scale_changes_frequency() {
        // higher scale => more spatial variation along a scanline
        let total_variation = |scale: f32| -> f32 {
            let mut tv = 0.0f32;
            let n = 256;
            let mut prev = caustic([0.0, 0.5], 0.0, scale);
            for i in 1..=n {
                let u = i as f32 / n as f32;
                let cur = caustic([u, 0.5], 0.0, scale);
                tv += (cur - prev).abs();
                prev = cur;
            }
            tv
        };
        assert!(total_variation(12.0) > total_variation(2.0), "higher scale not higher frequency");
    }

    #[test]
    fn continuity_small_step_small_change() {
        let eps = 1e-4f32;
        let mut seed = 0xCA05_0003u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..300 {
            let uv = [lcg() * 5.0, lcg() * 5.0];
            let t = lcg() * 10.0;
            let a = caustic(uv, t, 6.0);
            let b = caustic([uv[0] + eps, uv[1]], t, 6.0);
            assert!((a - b).abs() < 1e-2, "discontinuity {} vs {}", a, b);
        }
    }

    #[test]
    fn non_constant() {
        // the field actually varies across uv and t
        let base = caustic([0.1, 0.2], 0.0, 6.0);
        let mut differs = false;
        for i in 0..32 {
            let p = i as f32 * 0.13;
            if (caustic([p, p * 0.7], p * 0.5, 6.0) - base).abs() > 1e-3 {
                differs = true;
                break;
            }
        }
        assert!(differs, "caustic is constant");
    }

    // ---- r335-water-visual: sun+depth caustic intensity tests ----
    fn noon() -> [f32; 3] { [0.0, 1.0, 0.0] }      // sun overhead
    fn night() -> [f32; 3] { [0.3, -0.6, 0.2] }    // sun below horizon

    #[test]
    fn intensity_zero_at_night() {
        let p = CausticParams::default_params();
        let mut seed = 0xCA05_9001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed>>8) as f32 / ((1u32<<24) as f32) };
        for _ in 0..1000 {
            let uv = [lcg()*6.0-3.0, lcg()*6.0-3.0];
            let t = lcg()*30.0;
            let depth = lcg()*4.0;
            assert_eq!(caustic_intensity(uv, t, 6.0, night(), depth, p), 0.0, "no caustics at night");
        }
        // sun exactly on the horizon ⇒ also 0.
        assert_eq!(caustic_intensity([1.0, 1.0], 2.0, 6.0, [1.0, 0.0, 0.0], 0.5, p), 0.0);
    }

    #[test]
    fn intensity_bounded_and_dims_with_depth() {
        let p = CausticParams::default_params();
        // pick a uv/t where the pattern is clearly nonzero so the depth trend shows.
        let (uv, t) = ([0.37, 0.81], 1.2);
        let shallow = caustic_intensity(uv, t, 6.0, noon(), 0.0, p);
        let mid = caustic_intensity(uv, t, 6.0, noon(), 4.0, p);
        let deep = caustic_intensity(uv, t, 6.0, noon(), 20.0, p);
        for v in [shallow, mid, deep] { assert!((0.0..=1.0).contains(&v) && v.is_finite()); }
        assert!(shallow >= mid && mid >= deep, "caustics must dim with depth: {shallow} {mid} {deep}");
        assert!(shallow > deep, "shallow caustics clearly brighter than deep");
    }

    #[test]
    fn intensity_rises_as_sun_climbs() {
        let p = CausticParams::default_params();
        let (uv, t) = ([0.37, 0.81], 1.2);
        let low = caustic_intensity(uv, t, 6.0, [0.7, 0.1, 0.0], 0.5, p);   // sun just up
        let high = caustic_intensity(uv, t, 6.0, [0.0, 1.0, 0.0], 0.5, p);  // sun overhead
        assert!(high >= low, "higher sun ⇒ stronger caustics");
    }

    #[test]
    fn intensity_never_exceeds_raw_pattern() {
        let p = CausticParams::default_params();
        let (uv, t) = ([0.37, 0.81], 1.2);
        let raw = caustic(uv, t, 6.0);
        let modded = caustic_intensity(uv, t, 6.0, noon(), 0.0, p);
        assert!(modded <= raw + 1e-6, "modulation only attenuates the pattern");
    }

    #[test]
    fn intensity_deterministic() {
        let p = CausticParams::default_params();
        assert_eq!(
            caustic_intensity([0.4, 0.6], 1.3, 5.0, noon(), 2.0, p).to_bits(),
            caustic_intensity([0.4, 0.6], 1.3, 5.0, noon(), 2.0, p).to_bits()
        );
    }
}
