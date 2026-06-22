//! phys::fluid — buoyancy and drag forces for bodies in a fluid (r200).
//! Models the general physics-sim treatment of an object partly or fully
//! submerged: an upward buoyant force proportional to displaced volume,
//! and a quadratic drag opposing motion through the medium. physics.rs
//! previously noted "No fluid drag yet" — this is that math, isolated and
//! tested so it can be wired into step_body deterministically.
//!
//! Conventions: +Y is up. Densities are relative (water = 1.0). All pure
//! f32, std-only, deterministic.

use crate::util::vecmath::{len3, V3};

/// Fraction (0..1) of an axis-aligned body that is below the fluid
/// surface at height `surface_y`. Linear in the body's vertical span.
pub fn submersion_fraction(body_min_y: f32, body_max_y: f32, surface_y: f32) -> f32 {
    if body_max_y <= body_min_y {
        // Degenerate (zero height): fully in or out by its position.
        return if body_max_y <= surface_y { 1.0 } else { 0.0 };
    }
    ((surface_y - body_min_y) / (body_max_y - body_min_y)).clamp(0.0, 1.0)
}

/// Net vertical buoyant acceleration on a body, given:
///   `submersion` in 0..1, `body_density` relative to the fluid (1.0 =
///   neutral, <1 floats, >1 sinks), and `gravity` magnitude (positive).
///
/// Returns an acceleration (not a force) so it adds directly to a
/// velocity integrator: a_buoy = g * submersion * (fluid/body - 1). At
/// full submersion a neutral-density body gets zero net (floats in
/// place); a cork (density 0.3) rises, a stone (density 3.0) still sinks
/// but slower than in air.
pub fn buoyant_accel(submersion: f32, body_density: f32, gravity: f32) -> f32 {
    let s = submersion.clamp(0.0, 1.0);
    let d = body_density.max(1e-3);
    // fluid density is the reference 1.0.
    gravity * s * (1.0 / d - 1.0)
}

/// Quadratic drag acceleration opposing a velocity through the fluid.
/// `drag_coeff` lumps shape + fluid viscosity + cross-section / mass;
/// `submersion` scales it so a barely-wet body feels little drag.
/// Returns the acceleration vector to ADD to velocity (already negated
/// to oppose motion). Magnitude ~ coeff * |v|^2 in the -v direction.
pub fn drag_accel(velocity: V3, drag_coeff: f32, submersion: f32) -> V3 {
    let speed = len3(velocity);
    if speed <= 1e-6 {
        return [0.0; 3];
    }
    let s = submersion.clamp(0.0, 1.0);
    // a = -coeff * s * speed^2 * (v / speed) = -coeff*s*speed * v
    let k = -drag_coeff * s * speed;
    [velocity[0] * k, velocity[1] * k, velocity[2] * k]
}

/// Convenience: combined per-step velocity delta from buoyancy + drag for
/// a submerged body. `vel` in/s, `dt` step. Caller adds gravity itself
/// (buoyancy here is the fluid's *counter* to gravity, so the integrator
/// is: v += gravity*dt; v += fluid_step(...).) Deterministic.
pub fn fluid_velocity_delta(
    vel: V3,
    submersion: f32,
    body_density: f32,
    gravity: f32,
    drag_coeff: f32,
    dt: f32,
) -> V3 {
    let buoy = buoyant_accel(submersion, body_density, gravity) * dt;
    let drag = drag_accel(vel, drag_coeff, submersion);
    [drag[0] * dt, buoy + drag[1] * dt, drag[2] * dt]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submersion_spans_zero_to_one() {
        // body from y=0..2, surface rising.
        assert_eq!(submersion_fraction(0.0, 2.0, -1.0), 0.0); // above water
        assert_eq!(submersion_fraction(0.0, 2.0, 1.0), 0.5); // half
        assert_eq!(submersion_fraction(0.0, 2.0, 5.0), 1.0); // fully under
    }

    #[test]
    fn neutral_body_floats_in_place() {
        // density 1.0, fully submerged -> zero net buoyant accel.
        assert!(buoyant_accel(1.0, 1.0, 20.0).abs() < 1e-5);
    }

    #[test]
    fn cork_rises_stone_sinks_slower() {
        let g = 20.0;
        // cork (0.3): net upward (positive accel) when submerged.
        assert!(buoyant_accel(1.0, 0.3, g) > 0.0);
        // stone (3.0): still net downward, but buoyancy reduces it vs g.
        let stone = buoyant_accel(1.0, 3.0, g);
        assert!(stone < 0.0 && stone > -g, "stone net {stone} should be > -g");
    }

    #[test]
    fn drag_opposes_motion_and_grows_with_speed() {
        let slow = drag_accel([1.0, 0.0, 0.0], 0.5, 1.0);
        let fast = drag_accel([4.0, 0.0, 0.0], 0.5, 1.0);
        assert!(slow[0] < 0.0, "drag opposes +x motion");
        // quadratic: 4x speed -> ~16x the magnitude.
        assert!(fast[0].abs() > slow[0].abs() * 10.0);
        // no submersion -> no drag.
        assert_eq!(drag_accel([4.0, 0.0, 0.0], 0.5, 0.0), [0.0; 3]);
    }

    #[test]
    fn terminal_velocity_emerges() {
        // A dense body dropped in fluid should approach a terminal speed
        // where drag cancels net gravity, not accelerate forever.
        let g = 20.0;
        let mut v: V3 = [0.0, 0.0, 0.0];
        let dt = 1.0 / 60.0;
        for _ in 0..2000 {
            v[1] -= g * dt; // gravity
            let d = fluid_velocity_delta(v, 1.0, 2.0, g, 0.8, dt);
            v[0] += d[0];
            v[1] += d[1];
            v[2] += d[2];
        }
        // settled: |dv| per step tiny -> near terminal.
        let before = v[1];
        v[1] -= g * dt;
        let d = fluid_velocity_delta(v, 1.0, 2.0, g, 0.8, dt);
        v[1] += d[1];
        assert!((v[1] - before).abs() < 0.05, "should be near terminal, dv={}", (v[1] - before).abs());
        assert!(v[1] < 0.0, "dense body sinks");
    }
}
