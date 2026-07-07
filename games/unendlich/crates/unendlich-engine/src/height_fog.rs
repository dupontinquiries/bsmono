//! Height-based fog amount in [0,1]: thick below `fog_top`, thinning above it
//! with an exponential falloff. Pure, deterministic.

pub fn fog(world_y: f32, fog_top: f32, falloff: f32) -> f32 {
    let k = falloff.max(0.0);
    if world_y <= fog_top {
        1.0
    } else {
        (-((world_y - fog_top) * k)).exp().clamp(0.0, 1.0)
    }
}

// ── r334-fog ────────────────────────────────────────────────────────────────
// Richer height fog with a configurable floor, plus the shared `FogParams`
// table the distance/seam helpers (fog_blend.rs) also read. Pure & finite.
//
// NOTE (base): grounded on the r330b tree; on the r335 base these modules live
// under `sim::` and a shared `mathx` (r332) provides clamp/lerp/smoothstep —
// the coordinator should swap the local `smoothstep01` below for `mathx`.

/// Per-look fog tuning. One table threaded through height + distance + seam.
/// All fields are clamped to sane ranges by [`FogParams::sane`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogParams {
    /// World-Y below which height fog is fully thick.
    pub height_top: f32,
    /// Exponential thinning rate above `height_top` (per metre).
    pub height_falloff: f32,
    /// GL_EXP2 distance-fog density (per metre).
    pub distance_density: f32,
    /// Minimum fog amount anywhere in `[0,1]` — a small ambient floor so a
    /// near-camera / unfilled region never reads as a hard clear "box" against
    /// a foggy surround (mitigates the froxel near-seam hole; see fog_blend).
    pub fog_floor: f32,
    /// Gain applied to sun in-scatter colour (scatter_inscatter).
    pub sun_inscatter_gain: f32,
}

impl Default for FogParams {
    fn default() -> Self {
        FogParams {
            height_top: 48.0,
            height_falloff: 0.045,
            distance_density: 0.012,
            fog_floor: 0.0,
            sun_inscatter_gain: 1.0,
        }
    }
}

impl FogParams {
    /// Build a clamped, finite `FogParams` (NaN/negative inputs are tamed).
    pub fn sane(
        height_top: f32,
        height_falloff: f32,
        distance_density: f32,
        fog_floor: f32,
        sun_inscatter_gain: f32,
    ) -> FogParams {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        FogParams {
            height_top: fin(height_top, 48.0),
            height_falloff: fin(height_falloff, 0.045).max(0.0),
            distance_density: fin(distance_density, 0.012).max(0.0),
            fog_floor: fin(fog_floor, 0.0).clamp(0.0, 1.0),
            sun_inscatter_gain: fin(sun_inscatter_gain, 1.0).max(0.0),
        }
    }
}

/// Height fog amount in `[floor,1]`: the bare [`fog`] curve lifted to never
/// fall below `params.fog_floor`. Monotonic non-increasing in `world_y`.
pub fn height_factor(world_y: f32, params: FogParams) -> f32 {
    let base = fog(world_y, params.height_top, params.height_falloff);
    base.max(params.fog_floor).clamp(0.0, 1.0)
}

#[cfg(test)]
mod r334_fog_tests {
    use super::*;

    #[test]
    fn height_factor_monotonic_non_increasing() {
        let p = FogParams::default();
        let mut prev = 2.0f32;
        let mut y = -20.0f32;
        while y < 300.0 {
            let f = height_factor(y, p);
            assert!(f <= prev + 1e-6, "height fog rose at y={y}");
            assert!((0.0..=1.0).contains(&f));
            prev = f;
            y += 2.0;
        }
    }

    #[test]
    fn floor_is_respected() {
        let p = FogParams::sane(48.0, 0.045, 0.012, 0.2, 1.0);
        // Far above the top the bare curve → ~0, but the floor holds it at 0.2.
        assert!((height_factor(5000.0, p) - 0.2).abs() < 1e-3);
    }

    #[test]
    fn params_sane_tames_garbage() {
        let p = FogParams::sane(f32::NAN, -1.0, f32::INFINITY, 5.0, -3.0);
        assert!(p.height_top.is_finite());
        assert_eq!(p.height_falloff, 0.0);
        assert!(p.distance_density.is_finite() && p.distance_density >= 0.0);
        assert_eq!(p.fog_floor, 1.0); // 5.0 clamped to 1
        assert_eq!(p.sun_inscatter_gain, 0.0);
    }

    #[test]
    fn deterministic() {
        let p = FogParams::default();
        assert_eq!(height_factor(60.0, p).to_bits(), height_factor(60.0, p).to_bits());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn below_top_full() { assert_eq!(fog(5.0, 50.0, 0.1), 1.0); }
    #[test] fn far_above_thin() { assert!(fog(500.0, 50.0, 0.1) < 0.01); }
    #[test] fn higher_less() { assert!(fog(100.0, 50.0, 0.1) < fog(60.0, 50.0, 0.1)); }
    #[test] fn at_top_full() { assert_eq!(fog(50.0, 50.0, 0.1), 1.0); }
    #[test] fn bounded() {
        let mut y = 0.0f32;
        while y < 200.0 { assert!((0.0..=1.0).contains(&fog(y, 50.0, 0.1))); y += 5.0; }
    }
    #[test] fn deterministic() { assert_eq!(fog(60.0, 50.0, 0.1).to_bits(), fog(60.0, 50.0, 0.1).to_bits()); }
}
