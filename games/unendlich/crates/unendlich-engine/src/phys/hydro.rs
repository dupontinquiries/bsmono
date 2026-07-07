//! phys::hydro — hydrostatic fluid physics (r201). Models the pressure
//! and flow relationships a real fluid sim uses, complementing the
//! existing volume-conserving cellular settle (world::water_field) and
//! the flow-routing fields. This adds the classical hydraulics none of
//! those compute:
//!   - hydrostatic pressure with depth (p = ρ g h);
//!   - Torricelli outflow speed from a head of fluid (v = sqrt(2 g h));
//!   - head-difference driven flow rate between two connected cells.
//!
//! Units: 1 block = 1 m, water density ρ = 1000 kg/m³ by default but
//! expressed RELATIVE (water = 1.0) so callers stay in game units. Pure
//! f32, deterministic, std-only, tested against the textbook formulas.

/// Standard gravity magnitude (blocks/s²), matching the engine's player
/// physics scale where 1 block = 1 m.
pub const GRAVITY: f32 = 20.0;

/// Hydrostatic pressure (relative units) at `depth` below a fluid
/// surface, for a fluid of `rel_density` (water = 1.0). p = ρ·g·h.
/// Zero at/above the surface. Drives how hard fluid pushes outward
/// through a breach and how fast a deep column drains.
#[inline]
pub fn hydrostatic_pressure(depth: f32, rel_density: f32, gravity: f32) -> f32 {
    rel_density.max(0.0) * gravity * depth.max(0.0)
}

/// Torricelli's law: efflux speed of fluid escaping a hole under a head
/// `head` (height of fluid above the hole). v = sqrt(2·g·h). This is the
/// speed water shoots from a breached tank wall; deeper head = faster jet.
#[inline]
pub fn torricelli_speed(head: f32, gravity: f32) -> f32 {
    (2.0 * gravity * head.max(0.0)).sqrt()
}

/// Volumetric outflow rate through an opening of cross-section `area`
/// (blocks²) under `head`, with a discharge coefficient `cd` (~0.6 for a
/// sharp orifice, ~0.97 for a smooth nozzle). Q = cd · A · sqrt(2·g·h).
#[inline]
pub fn orifice_flow_rate(head: f32, area: f32, cd: f32, gravity: f32) -> f32 {
    cd.clamp(0.0, 1.0) * area.max(0.0) * torricelli_speed(head, gravity)
}

/// Signed flow rate between two connected fluid columns driven by their
/// surface-height difference (the "head"). Positive = flow from `a` to
/// `b` (a is higher). `conductance` lumps the channel cross-section and
/// roughness. Linearized for stable per-step cellular updates: the
/// settle step uses this to move fluid toward equilibrium without
/// overshoot when `conductance * dt <= 0.5`.
#[inline]
pub fn head_driven_flow(surface_a: f32, surface_b: f32, conductance: f32) -> f32 {
    (surface_a - surface_b) * conductance.max(0.0)
}

/// Equilibrium surface height two connected columns settle to, given
/// their current heights and equal footprints: the average. (With
/// unequal footprints the caller weights by area; this is the common
/// equal-cell case the cellular grid uses.)
#[inline]
pub fn equalized_surface(height_a: f32, height_b: f32) -> f32 {
    0.5 * (height_a + height_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_grows_linearly_with_depth() {
        let g = GRAVITY;
        assert_eq!(hydrostatic_pressure(0.0, 1.0, g), 0.0);
        let p1 = hydrostatic_pressure(1.0, 1.0, g);
        let p2 = hydrostatic_pressure(2.0, 1.0, g);
        assert!((p2 - 2.0 * p1).abs() < 1e-4, "pressure should double at 2x depth");
        // denser fluid -> more pressure at same depth.
        assert!(hydrostatic_pressure(2.0, 1.5, g) > hydrostatic_pressure(2.0, 1.0, g));
    }

    #[test]
    fn torricelli_deeper_head_faster_jet() {
        let g = GRAVITY;
        assert_eq!(torricelli_speed(0.0, g), 0.0);
        // v = sqrt(2*g*h): at h=2.5, g=20 -> sqrt(100) = 10.
        assert!((torricelli_speed(2.5, g) - 10.0).abs() < 1e-4);
        // 4x head -> 2x speed (sqrt relationship).
        let v1 = torricelli_speed(1.0, g);
        let v4 = torricelli_speed(4.0, g);
        assert!((v4 - 2.0 * v1).abs() < 1e-4);
    }

    #[test]
    fn orifice_rate_scales_with_area_and_cd() {
        let g = GRAVITY;
        let base = orifice_flow_rate(2.0, 1.0, 0.6, g);
        // double the area -> double the flow.
        assert!((orifice_flow_rate(2.0, 2.0, 0.6, g) - 2.0 * base).abs() < 1e-3);
        // higher discharge coeff -> more flow.
        assert!(orifice_flow_rate(2.0, 1.0, 0.95, g) > base);
        // no head -> no flow.
        assert_eq!(orifice_flow_rate(0.0, 1.0, 0.6, g), 0.0);
    }

    #[test]
    fn head_flow_is_signed_and_settles() {
        // a higher -> positive (a to b); equal -> zero.
        assert!(head_driven_flow(5.0, 3.0, 0.4) > 0.0);
        assert!(head_driven_flow(3.0, 5.0, 0.4) < 0.0);
        assert_eq!(head_driven_flow(4.0, 4.0, 0.4), 0.0);
        // equilibrium is the mean.
        assert_eq!(equalized_surface(6.0, 2.0), 4.0);
    }

    #[test]
    fn two_cell_settle_converges_without_overshoot() {
        // Two columns, conductance*dt <= 0.5 -> monotone convergence to mean.
        let mut a = 8.0f32;
        let mut b = 0.0f32;
        let mean = equalized_surface(a, b);
        let cond = 0.4; // *dt(=1) = 0.4 <= 0.5
        let mut prev_gap = (a - b).abs();
        for _ in 0..200 {
            let q = head_driven_flow(a, b, cond);
            a -= q;
            b += q;
            let gap = (a - b).abs();
            assert!(gap <= prev_gap + 1e-5, "no overshoot/oscillation");
            prev_gap = gap;
        }
        assert!((a - mean).abs() < 1e-2 && (b - mean).abs() < 1e-2, "converges to mean");
    }

    #[test]
    fn deterministic() {
        assert_eq!(torricelli_speed(3.3, GRAVITY).to_bits(),
                   torricelli_speed(3.3, GRAVITY).to_bits());
    }
}
