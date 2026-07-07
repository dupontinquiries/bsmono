//! tornado_vortex.rs — pure vortex velocity field for debris advection.
//!
//! A Rankine-style vortex: solid-body rotation inside a core radius, falling off
//! as 1/r outside it, plus a weak inward pull and an upward lift. Sampled at a
//! position relative to the vortex axis; debris integrates this each step. Pure,
//! deterministic, panic-free (R1/R11/R21).

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct VortexParams {
    /// Radius of solid-body rotation; tangential speed peaks here.
    pub core_radius: f32,
    /// Peak tangential speed (at the core edge), m/s.
    pub peak_tangential: f32,
    /// Inward radial pull at the core edge, m/s.
    pub inflow: f32,
    /// Upward lift near the axis, m/s.
    pub lift: f32,
}

impl Default for VortexParams {
    fn default() -> Self {
        VortexParams { core_radius: 8.0, peak_tangential: 40.0, inflow: 6.0, lift: 12.0 }
    }
}

/// Tangential speed magnitude at horizontal radius `r` (Rankine profile):
/// linear up to the core, `1/r` falloff beyond. Monotonically non-increasing
/// for `r >= core_radius`.
pub fn tangential_speed(r: f32, p: &VortexParams) -> f32 {
    let core = p.core_radius.max(1e-3);
    let peak = p.peak_tangential;
    if r <= core {
        peak * (r.max(0.0) / core)
    } else {
        peak * (core / r)
    }
}

/// Velocity at a position `rel` relative to the vortex axis (axis is the world
/// +Y line through the vortex centre). Returns world-space `[vx, vy, vz]`.
pub fn velocity_at(rel: [f32; 3], p: &VortexParams) -> [f32; 3] {
    let core = p.core_radius.max(1e-3);
    let r = (rel[0] * rel[0] + rel[2] * rel[2]).sqrt();

    if r < 1e-4 {
        // On the axis: no defined tangential/radial direction; only lift.
        return [0.0, p.lift, 0.0];
    }

    // unit radial (outward) and tangential (CCW) directions in XZ.
    let rx = rel[0] / r;
    let rz = rel[2] / r;
    let tx = -rz;
    let tz = rx;

    let v_t = tangential_speed(r, p);
    // inward pull, also falling off outside the core.
    let v_r = -p.inflow * (core / r.max(core));
    // lift strongest near the axis, fading outward.
    let v_y = p.lift * (core / r.max(core));

    [tx * v_t + rx * v_r, v_y, tz * v_t + rz * v_r]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_tangential_falls_with_radius_outside_core() {
        let p = VortexParams::default();
        let mut prev = f32::INFINITY;
        let mut r = p.core_radius;
        while r <= 500.0 {
            let s = tangential_speed(r, &p);
            assert!(s <= prev + 1e-5, "tangential rose at r={} ({} > {})", r, s, prev);
            assert!(s.is_finite() && s >= 0.0);
            prev = s;
            r += 2.0;
        }
    }

    #[test]
    fn tangential_peaks_at_core() {
        let p = VortexParams::default();
        let peak = tangential_speed(p.core_radius, &p);
        assert!((peak - p.peak_tangential).abs() < 1e-4);
        // inside the core it ramps up linearly
        assert!(tangential_speed(p.core_radius * 0.5, &p) < peak);
        // outside it decays
        assert!(tangential_speed(p.core_radius * 4.0, &p) < peak);
    }

    #[test]
    fn prop_velocity_bounded_and_finite() {
        let p = VortexParams::default();
        let bound = p.peak_tangential + p.inflow + p.lift + 1.0;
        let mut x = -300.0f32;
        while x <= 300.0 {
            let mut z = -300.0f32;
            while z <= 300.0 {
                let v = velocity_at([x, 5.0, z], &p);
                for c in v {
                    assert!(c.is_finite(), "non-finite at {},{}", x, z);
                    assert!(c.abs() <= bound, "speed {} exceeds bound {}", c, bound);
                }
                z += 12.0;
            }
            x += 12.0;
        }
    }

    #[test]
    fn axis_is_pure_lift() {
        let p = VortexParams::default();
        let v = velocity_at([0.0, 10.0, 0.0], &p);
        assert_eq!(v[0], 0.0);
        assert_eq!(v[2], 0.0);
        assert!(v[1] > 0.0, "expected upward lift on the axis");
    }

    #[test]
    fn prop_deterministic() {
        let p = VortexParams::default();
        assert_eq!(velocity_at([3.0, 1.0, -4.0], &p), velocity_at([3.0, 1.0, -4.0], &p));
    }
}
