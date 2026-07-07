//! `sim::wind_perturb` — how the WORLD deforms the global wind.
//!
//! [`crate::wind::wind_at`] gives the base field (direction, gusts, altitude gain).
//! This module layers the *object interactions* on top — the "things deform the
//! field" half of the wind system:
//!
//!   * **Movers break it.** A heavy body (mob, dragon) casts a downwind **shadow**
//!     (calmer air in its lee) and throws a **wake** — air shoved outward as it
//!     passes. Strength comes from the creature's heft (`sim::stature` mass /
//!     ground_shake), so a 15-block dragon carves the field and a person barely
//!     ripples it. This is what bends trees aside as the dragon goes by.
//!   * **Foliage filters it.** A canopy damps the through-flow and roughens it into
//!     small turbulence.
//!   * **Terrain lifts it.** Wind climbing a windward slope is forced **up**
//!     (orographic lift); the lee side sinks and eddies. (Distinct from the existing
//!     *thermal* `vertical_wind_at` — this is the mechanical mountain updraft.)
//!
//! Pure, deterministic, allocation-free, panic-free. Composes onto a base wind
//! vector so the CPU can sample it directly and a GPU froxel-fill can mirror it into
//! a low-res 3D texture the deform shaders (foliage, cloth, hair, wings) read.

use crate::util::hash_util::mix64_unit;

/// A heavy moving body that carves the wind. `strength` is its heft (typically the
/// stature `mass`/`ground_shake` scalar), `radius` its bulk in blocks.
#[derive(Clone, Copy, Debug)]
pub struct Mover {
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    pub radius: f32,
    pub strength: f32,
}

/// A mover's influence reaches this many radii before it's ignored.
pub const MOVER_REACH_RADII: f32 = 3.0;
/// How strongly the lee shadow calms the base wind (fraction, at the mover).
pub const SHADOW_STRENGTH: f32 = 0.8;
/// Wake shove magnitude coefficient (blocks/s per unit strength at the mover).
pub const WAKE_STRENGTH: f32 = 0.6;
/// Fraction of through-wind a full canopy removes.
pub const FOLIAGE_FILTER: f32 = 0.6;
/// Orographic lift coefficient (vertical b/s per unit of up-slope wind component).
pub const OROGRAPHIC_LIFT: f32 = 0.9;
/// e-folding height (blocks) over which the orographic lift fades with altitude.
pub const OROGRAPHIC_FADE_H: f32 = 18.0;

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len3(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}
fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = len3(a);
    if l > 1e-5 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 0.0]
    }
}
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// The wind delta a single mover imposes at `pos`: a downwind **shadow** (subtracts
/// from the base wind in the lee) plus a **wake** shove (radially outward from the
/// body, plus a tug along its own motion). Zero beyond its reach.
pub fn mover_delta(base_wind: [f32; 3], m: &Mover, pos: [f32; 3]) -> [f32; 3] {
    let to = [pos[0] - m.pos[0], pos[1] - m.pos[1], pos[2] - m.pos[2]];
    let dist = len3(to);
    let reach = (m.radius * MOVER_REACH_RADII).max(1e-3);
    if dist >= reach {
        return [0.0, 0.0, 0.0];
    }
    // smooth 1→0 falloff from the body out to `reach`, weighted by heft.
    let fall = (1.0 - dist / reach).clamp(0.0, 1.0);
    let fall = fall * fall; // softer tail
    let heft = m.strength.max(0.0);

    let wind_dir = norm3([base_wind[0], 0.0, base_wind[2]]);
    let to_dir = norm3(to);

    // SHADOW: in the lee (pos downwind of the mover) subtract from the base wind.
    let downwind = dot3(to_dir, wind_dir).max(0.0); // 1 = directly downwind
    let shadow = scale3(base_wind, -SHADOW_STRENGTH * fall * downwind);

    // WAKE: a radial shove outward from the body + a tug along the mover's motion.
    let radial = scale3(to_dir, WAKE_STRENGTH * heft * fall);
    let drag = scale3(m.vel, 0.15 * fall);

    add3(add3(shadow, radial), drag)
}

/// Sum the [`mover_delta`] of every mover at `pos`.
pub fn movers_delta(base_wind: [f32; 3], movers: &[Mover], pos: [f32; 3]) -> [f32; 3] {
    let mut acc = [0.0f32; 3];
    for m in movers {
        acc = add3(acc, mover_delta(base_wind, m, pos));
    }
    acc
}

/// Foliage filter delta: a canopy fraction `canopy01` (0..1) damps the through-wind
/// and adds a little deterministic cross-turbulence (leaves shed eddies).
pub fn foliage_delta(wind: [f32; 3], canopy01: f32, pos: [f32; 3], t: f32) -> [f32; 3] {
    let c = canopy01.clamp(0.0, 1.0);
    if c <= 0.0 {
        return [0.0, 0.0, 0.0];
    }
    // damping opposes the through-wind
    let damp = scale3(wind, -FOLIAGE_FILTER * c);
    // cross turbulence: a small perpendicular jitter, seeded by cell + time
    let mag = len3(wind);
    let seed = (pos[0].floor() as i64 as u64)
        .wrapping_mul(0x9E37_79B9)
        ^ ((pos[2].floor() as i64 as u64) << 21)
        ^ ((t * 4.0) as i64 as u64);
    let a = mix64_unit(seed) * std::f32::consts::TAU;
    let turb = scale3([a.cos(), 0.0, a.sin()], 0.12 * c * mag);
    add3(damp, turb)
}

/// Orographic lift delta from a terrain slope `slope = (d surface/dx, d surface/dz)`
/// at altitude `alt` above the surface. Wind climbing the windward face (positive
/// up-slope component) is forced upward; descending the lee gives a downdraft. The
/// effect fades with height above the ground.
pub fn orographic_delta(base_wind: [f32; 3], slope: [f32; 2], alt: f32) -> [f32; 3] {
    // up-slope component of the horizontal wind: >0 climbing, <0 descending.
    let climb = base_wind[0] * slope[0] + base_wind[2] * slope[1];
    let fade = (-(alt.max(0.0)) / OROGRAPHIC_FADE_H).exp();
    [0.0, OROGRAPHIC_LIFT * climb * fade, 0.0]
}

/// Full perturbed wind at `pos`: the base field, deformed by movers, foliage and
/// terrain. `canopy01` is the local foliage density, `slope` the terrain gradient,
/// `alt` the height above the surface, `t` seconds.
#[allow(clippy::too_many_arguments)]
pub fn perturbed(
    base_wind: [f32; 3],
    movers: &[Mover],
    canopy01: f32,
    slope: [f32; 2],
    alt: f32,
    pos: [f32; 3],
    t: f32,
) -> [f32; 3] {
    let mut w = add3(base_wind, movers_delta(base_wind, movers, pos));
    w = add3(w, foliage_delta(w, canopy01, pos, t));
    w = add3(w, orographic_delta(base_wind, slope, alt));
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, e: f32) -> bool {
        (a - b).abs() <= e
    }

    #[test]
    fn no_objects_returns_base() {
        let base = [3.0, 0.0, 1.0];
        let w = perturbed(base, &[], 0.0, [0.0, 0.0], 100.0, [0.0, 70.0, 0.0], 2.0);
        assert!(approx(w[0], base[0], 1e-4) && approx(w[1], 0.0, 1e-4) && approx(w[2], base[2], 1e-4));
    }

    #[test]
    fn mover_calms_its_lee() {
        // wind blows +x; a mover at origin; sample point downwind (+x) should see a
        // reduced +x wind (shadow), i.e. smaller magnitude than the base there.
        let base = [4.0, 0.0, 0.0];
        let m = Mover { pos: [0.0, 70.0, 0.0], vel: [0.0, 0.0, 0.0], radius: 4.0, strength: 3.0 };
        let lee = [6.0, 70.0, 0.0]; // downwind, within reach (12)
        let d = mover_delta(base, &m, lee);
        // shadow subtracts along +x
        assert!(d[0] < 0.0, "expected lee shadow to subtract +x wind, got {d:?}");
    }

    #[test]
    fn mover_wake_shoves_outward_and_fades_with_distance() {
        let base = [0.0, 0.0, 0.0]; // no wind → pure wake
        let m = Mover { pos: [0.0, 70.0, 0.0], vel: [0.0, 0.0, 0.0], radius: 4.0, strength: 5.0 };
        let near = mover_delta(base, &m, [2.0, 70.0, 0.0]);
        let far = mover_delta(base, &m, [10.0, 70.0, 0.0]);
        assert!(near[0] > 0.0, "wake should push +x away from body: {near:?}");
        assert!(len3(near) > len3(far), "wake should fade with distance");
        // beyond reach → nothing
        assert_eq!(mover_delta(base, &m, [50.0, 70.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn heavier_movers_carve_harder() {
        let base = [0.0, 0.0, 0.0];
        let light = Mover { pos: [0.0; 3], vel: [0.0; 3], radius: 4.0, strength: 1.0 };
        let heavy = Mover { pos: [0.0; 3], vel: [0.0; 3], radius: 4.0, strength: 8.0 };
        let p = [3.0, 0.0, 0.0];
        assert!(len3(mover_delta(base, &heavy, p)) > len3(mover_delta(base, &light, p)));
    }

    #[test]
    fn foliage_damps_and_zero_canopy_is_inert() {
        let wind = [5.0, 0.0, 0.0];
        assert_eq!(foliage_delta(wind, 0.0, [0.0, 0.0, 0.0], 1.0), [0.0, 0.0, 0.0]);
        let d = foliage_delta(wind, 1.0, [0.0, 0.0, 0.0], 1.0);
        // net through-wind after damping is smaller
        assert!((wind[0] + d[0]) < wind[0], "canopy should reduce +x through-wind");
    }

    #[test]
    fn windward_slope_lifts_lee_slope_sinks() {
        let base = [4.0, 0.0, 0.0]; // wind blows +x
        // windward: surface rises in +x (slope.x > 0) → updraft
        let up = orographic_delta(base, [0.5, 0.0], 0.0);
        assert!(up[1] > 0.0, "windward upslope should lift: {up:?}");
        // lee: surface falls in +x (slope.x < 0) → downdraft
        let down = orographic_delta(base, [-0.5, 0.0], 0.0);
        assert!(down[1] < 0.0, "lee downslope should sink: {down:?}");
    }

    #[test]
    fn orographic_lift_fades_with_altitude() {
        let base = [4.0, 0.0, 0.0];
        let low = orographic_delta(base, [0.5, 0.0], 0.0)[1];
        let high = orographic_delta(base, [0.5, 0.0], 100.0)[1];
        assert!(high.abs() < low.abs() * 0.2, "lift should fade aloft: {low} vs {high}");
    }

    #[test]
    fn full_perturbed_stacks_effects_finitely() {
        let base = [3.0, 0.0, 2.0];
        let movers = [
            Mover { pos: [1.0, 70.0, 0.0], vel: [2.0, 0.0, 0.0], radius: 5.0, strength: 6.0 },
            Mover { pos: [-4.0, 70.0, 3.0], vel: [0.0, 0.0, -1.0], radius: 3.0, strength: 2.0 },
        ];
        let w = perturbed(base, &movers, 0.4, [0.3, 0.0], 5.0, [2.0, 71.0, 1.0], 3.5);
        assert!(w.iter().all(|c| c.is_finite()));
    }
}
