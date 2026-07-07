//! Archimedes **buoyancy + fluid drag** (r54-08, physics) — logs float, stones
//! sink, a swimming dragon bobs at a waterline.
//!
//! Pure scalar/vector physics: AABB-vs-water overlap → submerged fraction →
//! net buoyant acceleration, quadratic fluid drag opposing velocity, and a
//! one-step semi-implicit integrator that settles a lighter-than-water body at
//! its equilibrium waterline and sinks a denser one.
//!
//! Conventions: `+y` is up; `gravity` is the downward gravitational magnitude
//! (positive, e.g. 9.81). [`buoyant_accel`] returns the **net** vertical
//! acceleration (positive = up). All inputs are sanitised (non-finite → 0,
//! densities/mass floored at a tiny epsilon) so every output is finite and the
//! integrator never blows up or panics. Deterministic.

/// Floor for denominators (densities, mass) to keep ratios finite.
const EPS: f32 = 1.0e-6;

/// Finite-or-0.
#[inline]
fn fin(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Finite, non-negative.
#[inline]
fn nn(v: f32) -> f32 {
    fin(v).max(0.0)
}

/// Finite, strictly positive (floored at `EPS`) — for divisors.
#[inline]
fn pos(v: f32) -> f32 {
    fin(v).max(EPS)
}

/// Clamp `v`'s magnitude to `<= limit` (limit assumed `>= 0`).
#[inline]
fn clamp_abs(v: f32, limit: f32) -> f32 {
    v.clamp(-limit, limit)
}

/// Fraction of a body's vertical extent `[aabb_min_y, aabb_max_y]` that lies
/// below `water_level`, in `[0,1]`. Below water entirely → 1; above → 0; a
/// zero/degenerate-height body is fully submerged iff its top is at/under water.
pub fn submerged_fraction(aabb_min_y: f32, aabb_max_y: f32, water_level: f32) -> f32 {
    let lo = fin(aabb_min_y);
    let hi = fin(aabb_max_y);
    let w = fin(water_level);
    let height = hi - lo;
    if height <= 0.0 {
        return if w >= hi { 1.0 } else { 0.0 };
    }
    ((w - lo) / height).clamp(0.0, 1.0)
}

/// Net vertical acceleration (positive = up) from buoyancy minus weight:
/// `g · (fluid/body · submerged − 1)`. Fully submerged & lighter-than-water →
/// positive (rises); denser → negative (sinks); out of water (`submerged = 0`)
/// → `−g` (free fall).
pub fn buoyant_accel(submerged01: f32, body_density: f32, fluid_density: f32, gravity: f32) -> f32 {
    let sub = nn(submerged01).min(1.0);
    let body = pos(body_density);
    let fluid = nn(fluid_density);
    let g = fin(gravity);
    g * ((fluid / body) * sub - 1.0)
}

/// Quadratic fluid drag acceleration opposing `vel`:
/// `a = −(½ ρ C_d A |v| / m) · v` (per component), so the magnitude grows with
/// `v²` and the direction is exactly opposite the velocity. Zero at rest.
pub fn drag_accel(vel: [f32; 3], area: f32, drag_coeff: f32, fluid_density: f32, mass: f32) -> [f32; 3] {
    let v = [fin(vel[0]), fin(vel[1]), fin(vel[2])];
    let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if speed <= 0.0 {
        return [0.0, 0.0, 0.0];
    }
    let k = 0.5 * nn(fluid_density) * nn(drag_coeff) * nn(area) * speed / pos(mass);
    [-k * v[0], -k * v[1], -k * v[2]]
}

/// Advance a body's vertical state one step under buoyancy + quadratic vertical
/// drag, using semi-implicit Euler (update velocity, then position). Returns
/// `(new_y, new_vy)`.
///
/// `min_y`/`max_y` are the body's CURRENT world AABB extent; the caller shifts
/// them with the body between steps. Drag is applied so it can never reverse the
/// sign of the velocity within a step (unconditionally stable — drag only
/// dissipates). A lighter-than-water body settles at its equilibrium waterline
/// (`submerged = body/fluid`); a denser one keeps sinking.
#[allow(clippy::too_many_arguments)]
pub fn float_step(
    y: f32,
    vy: f32,
    min_y: f32,
    max_y: f32,
    water_level: f32,
    body_density: f32,
    fluid_density: f32,
    gravity: f32,
    drag: f32,
    dt: f32,
) -> (f32, f32) {
    let y0 = fin(y);
    let vy0 = fin(vy);
    let h = nn(dt);

    let sub = submerged_fraction(min_y, max_y, water_level);
    let a_buoy = buoyant_accel(sub, body_density, fluid_density, gravity);

    // velocity after buoyancy
    let mut nvy = vy0 + a_buoy * h;

    // quadratic vertical drag opposing nvy, clamped so it cannot overshoot 0
    // (guarantees stability for any dt — drag only removes energy).
    let d = nn(drag);
    let dv = d * nvy.abs() * nvy * h; // same sign as nvy → subtract to oppose
    let dv = clamp_abs(dv, nvy.abs());
    nvy -= dv;

    let ny = y0 + nvy * h;
    (ny, nvy)
}

// =============================================================================
// r335-water-physics — buoyant FORCE, swimming, flow push, and a head-above-water
// surfacing assist (the operator's bonus). Additive, reuses this module's
// sanitisation (`fin`/`nn`/`pos`/`clamp_abs`) and Archimedes core above.
//
// Scope note: `sim::water_physics` is the shallow-water *heightfield* hydrology
// step (flow/evaporation/moisture); BODY water-physics lives here in `buoyancy`
// (this module already owns submerged-fraction + buoyant accel + fluid drag +
// the float integrator). See HANDOFF_water_physics.md.
// =============================================================================

/// Tuning for body water-physics. All values are clamped to safe ranges where
/// they're used, so an out-of-range table can't destabilise the integrator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterPhysicsParams {
    /// Linear drag in water (per second) — the dominant settling term for a
    /// swimmer. Higher → reaches terminal speed sooner.
    pub drag_linear: f32,
    /// Quadratic drag (per m/s) — resists fast motion, shapes terminal speed.
    pub drag_quadratic: f32,
    /// Swim acceleration along the input direction (m/s²) at full immersion.
    pub swim_accel: f32,
    /// Terminal swim speed (m/s) the in-water velocity is clamped toward.
    pub max_swim_speed: f32,
    /// Multiplier on the net Archimedes buoyant accel (1.0 = physical).
    pub buoyancy_gain: f32,
    /// Target submerged fraction while actively surfacing/treading. `0.9` leaves
    /// the top 10 % of the body — the head — above the waterline. `< 1.0`.
    pub surface_submersion: f32,
    /// Restoring stiffness (per second²-ish, scaled by gravity) that lifts the
    /// swimmer toward `surface_submersion` when surfacing. Bounded in use.
    pub surface_stiffness: f32,
}

impl Default for WaterPhysicsParams {
    fn default() -> Self {
        // Tuned for a ~human/dragon-rider swimmer in water (fluid ρ≈1000).
        WaterPhysicsParams {
            drag_linear: 2.5,
            drag_quadratic: 0.6,
            swim_accel: 9.0,
            max_swim_speed: 5.0,
            buoyancy_gain: 1.0,
            surface_submersion: 0.9, // head (top 10%) above water when treading
            surface_stiffness: 6.0,
        }
    }
}

/// Maximum upward acceleration (in g multiples) the surfacing assist may apply,
/// so a treading swimmer rises to the waterline but never rockets out of it.
const MAX_SURFACE_G: f32 = 3.0;

#[inline]
fn norm3(v: [f32; 3]) -> Option<[f32; 3]> {
    let v = [fin(v[0]), fin(v[1]), fin(v[2])];
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 && l.is_finite() {
        Some([v[0] / l, v[1] / l, v[2] / l])
    } else {
        None
    }
}

/// Archimedes buoyant **force** (newtons, ≥ 0): `ρ_fluid · V_submerged · g`,
/// where `V_submerged = submerged_frac01 · body_volume`. Complements
/// [`buoyant_accel`] (which returns net accel); this is the raw upward force a
/// physics integrator can divide by mass or compare against weight. Finite,
/// non-negative, never panics.
pub fn buoyant_force(submerged_frac01: f32, body_volume: f32, fluid_density: f32, gravity: f32) -> f32 {
    let sub = nn(submerged_frac01).min(1.0);
    let vol = nn(body_volume);
    let rho = nn(fluid_density);
    let g = nn(gravity);
    rho * sub * vol * g
}

/// One swim velocity step: accelerate along `input_dir` (normalised) and apply
/// water drag, scaled by immersion `in_water01 ∈ [0,1]`. Returns the new
/// velocity. The in-water speed is clamped toward `max_swim_speed` so a swimmer
/// has a bounded terminal velocity; out of water (`in_water01 = 0`) this is a
/// no-op (land movement is handled elsewhere). Finite, NaN-safe, deterministic.
pub fn swim_step(
    vel: [f32; 3],
    input_dir: [f32; 3],
    in_water01: f32,
    dt: f32,
    p: &WaterPhysicsParams,
) -> [f32; 3] {
    let mut v = [fin(vel[0]), fin(vel[1]), fin(vel[2])];
    let imm = nn(in_water01).min(1.0);
    let h = nn(dt);
    if imm <= 0.0 || h <= 0.0 {
        return v;
    }

    // Swim acceleration along the (normalised) input, scaled by immersion.
    if let Some(dir) = norm3(input_dir) {
        let a = nn(p.swim_accel) * imm;
        v = [v[0] + dir[0] * a * h, v[1] + dir[1] * a * h, v[2] + dir[2] * a * h];
    }

    // Drag: linear + quadratic, opposing velocity, clamped so it only ever
    // dissipates (can't reverse the sign within a step) — unconditionally stable.
    let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if speed > 0.0 {
        let k = (nn(p.drag_linear) + nn(p.drag_quadratic) * speed) * imm * h;
        let k = k.min(1.0); // never overshoot zero
        v = [v[0] - v[0] * k, v[1] - v[1] * k, v[2] - v[2] * k];
    }

    // Clamp the in-water speed toward the terminal swim speed.
    let max_s = nn(p.max_swim_speed);
    if max_s > 0.0 {
        let s = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if s > max_s {
            let f = max_s / s;
            v = [v[0] * f, v[1] * f, v[2] * f];
        }
    }
    v
}

/// Lateral flow push (river current / waterfall) as a per-step impulse:
/// `flow_vec · in_water01 · dt`. Out of water → zero. Finite, NaN-safe.
pub fn flow_push(flow_vec: [f32; 3], in_water01: f32, dt: f32) -> [f32; 3] {
    let imm = nn(in_water01).min(1.0);
    let h = nn(dt);
    let s = imm * h;
    [fin(flow_vec[0]) * s, fin(flow_vec[1]) * s, fin(flow_vec[2]) * s]
}

/// Whether the top of the body is above the waterline (its head is out).
#[inline]
pub fn head_above_water(body_max_y: f32, water_level: f32) -> bool {
    fin(body_max_y) > fin(water_level)
}

/// Surfacing assist: extra **upward** acceleration that lifts an actively
/// surfacing (treading / swim-up) body toward the waterline where
/// `submerged == p.surface_submersion`, so its head clears the surface.
///
/// - `surfacing == false` → `0.0` (natural buoyancy alone decides the waterline,
///   which for a near-water-density body leaves it bobbing at the surface, and
///   for a denser one lets it sink).
/// - `surfacing == true` and the body is **deeper** than the target (head under)
///   → a restoring upward accel proportional to how far past the target it is,
///   capped at `MAX_SURFACE_G · gravity` so the swimmer rises to the surface but
///   never launches out of it.
/// - at/above the target (head already clear) → `0.0` (no extra push; gravity +
///   buoyancy hold it there).
///
/// This is what lets a swimmer **put their head above water**: even a
/// slightly-denser-than-water body, while treading, is held with the top
/// `1 - surface_submersion` of its height out of the water.
pub fn surface_accel(submerged01: f32, surfacing: bool, gravity: f32, p: &WaterPhysicsParams) -> f32 {
    if !surfacing {
        return 0.0;
    }
    let sub = nn(submerged01).min(1.0);
    let target = p.surface_submersion.clamp(0.0, 1.0);
    let over = sub - target; // > 0 ⇒ deeper than we want ⇒ lift
    if over <= 0.0 {
        return 0.0;
    }
    let g = nn(gravity);
    let a = nn(p.surface_stiffness) * over * g;
    a.min(MAX_SURFACE_G * g)
}

/// Vertical step for a swimming body: buoyancy (gained) + the surfacing assist,
/// then stable vertical drag, integrated semi-implicitly like [`float_step`].
/// With `surfacing == true` a body settles with its head above water (submerged
/// → `p.surface_submersion`); with `surfacing == false` it follows natural
/// buoyancy. Returns `(new_y, new_vy)`. Unconditionally stable (drag only
/// dissipates); finite; never panics.
#[allow(clippy::too_many_arguments)]
pub fn swim_vertical_step(
    y: f32,
    vy: f32,
    min_y: f32,
    max_y: f32,
    water_level: f32,
    surfacing: bool,
    body_density: f32,
    fluid_density: f32,
    gravity: f32,
    dt: f32,
    p: &WaterPhysicsParams,
) -> (f32, f32) {
    let y0 = fin(y);
    let vy0 = fin(vy);
    let h = nn(dt);

    let sub = submerged_fraction(min_y, max_y, water_level);
    let a_buoy = buoyant_accel(sub, body_density, fluid_density, gravity) * nn(p.buoyancy_gain);
    let a_surface = surface_accel(sub, surfacing, gravity, p);

    let mut nvy = vy0 + (a_buoy + a_surface) * h;

    // Quadratic vertical drag opposing nvy, clamped so it can't overshoot 0
    // (matches float_step's stability guarantee).
    let d = nn(p.drag_linear);
    let dv = d * nvy * h;
    let dv = clamp_abs(dv, nvy.abs());
    nvy -= dv;

    let ny = y0 + nvy * h;
    (ny, nvy)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: f32 = 9.81;

    #[test]
    fn submerged_endpoints_and_overlap() {
        // body spans y in [0,2]
        assert_eq!(submerged_fraction(0.0, 2.0, -1.0), 0.0, "water below body → dry");
        assert_eq!(submerged_fraction(0.0, 2.0, 5.0), 1.0, "water above body → full");
        assert!((submerged_fraction(0.0, 2.0, 1.0) - 0.5).abs() < 1e-6, "half submerged");
        assert!((submerged_fraction(0.0, 4.0, 1.0) - 0.25).abs() < 1e-6);
        // degenerate height
        assert_eq!(submerged_fraction(3.0, 3.0, 3.0), 1.0);
        assert_eq!(submerged_fraction(3.0, 3.0, 2.9), 0.0);
        // non-finite → finite result
        assert!(submerged_fraction(f32::NAN, 2.0, 1.0).is_finite());
    }

    #[test]
    fn fully_submerged_gives_full_buoyancy() {
        // lighter than water, fully submerged → net upward (positive)
        let a = buoyant_accel(1.0, 500.0, 1000.0, G);
        assert!((a - G * (2.0 - 1.0)).abs() < 1e-4);
        assert!(a > 0.0);
        // out of water → free fall (−g)
        assert!((buoyant_accel(0.0, 500.0, 1000.0, G) + G).abs() < 1e-4);
        // denser, fully submerged → net downward
        assert!(buoyant_accel(1.0, 2000.0, 1000.0, G) < 0.0);
    }

    #[test]
    fn buoyant_accel_finite_at_extremes() {
        for &b in &[0.0, -5.0, f32::NAN, f32::INFINITY, 1e9] {
            for &f in &[0.0, f32::NAN, 1000.0] {
                let a = buoyant_accel(1.0, b, f, G);
                assert!(a.is_finite(), "non-finite accel for body={b} fluid={f}");
            }
        }
    }

    #[test]
    fn drag_opposes_velocity_and_scales_quadratically() {
        let a1 = drag_accel([1.0, 0.0, 0.0], 1.0, 1.0, 1.0, 1.0);
        assert!(a1[0] < 0.0, "drag opposes +x velocity");
        let a2 = drag_accel([2.0, 0.0, 0.0], 1.0, 1.0, 1.0, 1.0);
        // |a| ∝ v² → doubling v quadruples drag accel
        assert!((a2[0] / a1[0] - 4.0).abs() < 1e-4);
        // opposes in every direction
        let a3 = drag_accel([-3.0, 4.0, 0.0], 1.0, 1.0, 1.0, 1.0);
        assert!(a3[0] > 0.0 && a3[1] < 0.0, "drag points opposite velocity");
        // at rest → zero
        assert_eq!(drag_accel([0.0, 0.0, 0.0], 1.0, 1.0, 1.0, 1.0), [0.0, 0.0, 0.0]);
        // finite even with mass 0
        assert!(drag_accel([1.0, 1.0, 1.0], 1.0, 1.0, 1.0, 0.0).iter().all(|v| v.is_finite()));
    }

    /// Simulate a floating body: returns the final submerged fraction and vy.
    fn settle(body_density: f32, fluid_density: f32, steps: usize) -> (f32, f32, f32) {
        // body 1 unit tall; start fully above water at y=5, water at 0.
        let mut y = 5.0f32;
        let mut vy = 0.0f32;
        let half = 0.5f32; // AABB = [y-half, y+half]
        let water = 0.0f32;
        let dt = 0.02f32;
        for _ in 0..steps {
            let (ny, nvy) = float_step(
                y, vy, y - half, y + half, water,
                body_density, fluid_density, G, 0.8, dt,
            );
            y = ny;
            vy = nvy;
        }
        let sub = submerged_fraction(y - half, y + half, water);
        (y, vy, sub)
    }

    #[test]
    fn lighter_body_rises_to_equilibrium_and_settles() {
        // body_density/fluid = 0.4 → equilibrium submerged fraction ≈ 0.4
        let (_y, vy, sub) = settle(400.0, 1000.0, 4000);
        assert!((sub - 0.4).abs() < 0.05, "should settle near submerged=0.4, got {sub}");
        assert!(vy.abs() < 0.05, "should come to rest, vy={vy}");
    }

    #[test]
    fn denser_body_sinks() {
        // denser than water → never reaches equilibrium, keeps sinking.
        let (y, vy, sub) = settle(2000.0, 1000.0, 600);
        assert!(sub >= 0.999, "denser body fully submerged");
        assert!(vy < 0.0, "denser body moving downward");
        assert!(y < 0.0, "denser body has sunk below the surface");
    }

    #[test]
    fn float_step_is_finite_and_stable_under_large_dt() {
        // Big dt must not blow up (drag clamp guarantees stability).
        let mut y = 5.0f32;
        let mut vy = 0.0f32;
        for _ in 0..1000 {
            let (ny, nvy) = float_step(y, vy, y - 0.5, y + 0.5, 0.0, 400.0, 1000.0, G, 0.8, 1.0);
            assert!(ny.is_finite() && nvy.is_finite(), "blew up: y={ny} vy={nvy}");
            y = ny;
            vy = nvy;
        }
    }

    #[test]
    fn deterministic() {
        let first = settle(400.0, 1000.0, 500);
        for _ in 0..50 {
            assert_eq!(settle(400.0, 1000.0, 500), first);
        }
        let d = drag_accel([1.5, -2.0, 0.5], 1.2, 0.9, 1000.0, 80.0);
        for _ in 0..50 {
            assert_eq!(drag_accel([1.5, -2.0, 0.5], 1.2, 0.9, 1000.0, 80.0), d);
        }
    }
}

#[cfg(test)]
mod r335_water_physics_tests {
    use super::*;

    const G: f32 = 9.81;

    fn params() -> WaterPhysicsParams {
        WaterPhysicsParams::default()
    }

    // ---- buoyant_force (Archimedes) ----

    #[test]
    fn buoyant_force_basic_and_monotone() {
        // fully submerged 1 m³ in water: ρ·V·g ≈ 1000·1·9.81
        let f = buoyant_force(1.0, 1.0, 1000.0, G);
        assert!((f - 9810.0).abs() < 1.0);
        // more submerged → more force; out of water → 0
        assert!(buoyant_force(1.0, 1.0, 1000.0, G) > buoyant_force(0.5, 1.0, 1000.0, G));
        assert_eq!(buoyant_force(0.0, 1.0, 1000.0, G), 0.0);
        // never negative / NaN
        assert!(buoyant_force(f32::NAN, -3.0, 1000.0, G).is_finite());
        assert!(buoyant_force(2.0, 1.0, 1000.0, G) >= 0.0);
    }

    // ---- swim_step ----

    #[test]
    fn swim_step_out_of_water_is_noop() {
        let v = [1.0, -2.0, 0.5];
        assert_eq!(swim_step(v, [1.0, 0.0, 0.0], 0.0, 0.016, &params()), v);
    }

    #[test]
    fn swim_step_accelerates_along_input_then_bounds_speed() {
        let p = params();
        let mut v = [0.0, 0.0, 0.0];
        for _ in 0..400 {
            v = swim_step(v, [1.0, 0.0, 0.0], 1.0, 0.016, &p);
        }
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        assert!(v[0] > 0.0, "should move along +x");
        assert!(speed <= p.max_swim_speed + 1e-3, "terminal speed bounded, got {speed}");
    }

    #[test]
    fn swim_step_drag_only_dissipates() {
        // no input, moving fast → drag slows but never reverses sign in a step
        let p = params();
        let v0 = [4.0, 0.0, 0.0];
        let v1 = swim_step(v0, [0.0, 0.0, 0.0], 1.0, 0.5, &p);
        assert!(v1[0] >= 0.0 && v1[0] <= v0[0], "drag dissipates, no reversal: {v1:?}");
    }

    #[test]
    fn swim_step_nan_safe() {
        let v = swim_step([f32::NAN, 1.0, 0.0], [f32::INFINITY, 0.0, 0.0], 1.0, 0.016, &params());
        assert!(v.iter().all(|c| c.is_finite()));
    }

    // ---- flow_push ----

    #[test]
    fn flow_push_scales_with_immersion_and_zero_when_dry() {
        let flow = [2.0, 0.0, 1.0];
        let full = flow_push(flow, 1.0, 0.5);
        let half = flow_push(flow, 0.5, 0.5);
        assert!(full[0] > half[0] && full[2] > half[2], "more immersion → more push");
        assert_eq!(flow_push(flow, 0.0, 0.5), [0.0, 0.0, 0.0], "dry → no push");
        assert!(flow_push([f32::NAN, 0.0, 0.0], 1.0, 0.5).iter().all(|c| c.is_finite()));
    }

    // ---- head above water (the bonus) ----

    #[test]
    fn head_above_water_predicate() {
        assert!(head_above_water(5.1, 5.0));
        assert!(!head_above_water(4.9, 5.0));
    }

    #[test]
    fn surface_accel_only_lifts_when_surfacing_and_deep() {
        let p = params();
        // not surfacing → no assist
        assert_eq!(surface_accel(1.0, false, G, &p), 0.0);
        // surfacing + fully submerged (deeper than target 0.9) → upward lift
        assert!(surface_accel(1.0, true, G, &p) > 0.0);
        // surfacing but head already clear (submerged below target) → no extra push
        assert_eq!(surface_accel(0.5, true, G, &p), 0.0);
        // lift is capped (never rockets out)
        let strong = WaterPhysicsParams { surface_stiffness: 1.0e6, ..p };
        assert!(surface_accel(1.0, true, G, &strong) <= MAX_SURFACE_G * G + 1e-3);
    }

    // Simulate a swimmer to its waterline. Body AABB is `height` tall, centred so
    // (min_y, max_y) follow y. Returns final (submerged_fraction, head_above).
    fn settle_swim(body_density: f32, surfacing: bool, height: f32, steps: usize) -> (f32, bool) {
        let p = params();
        let water = 64.0;
        let half = height * 0.5;
        let mut y = water - 2.0; // start fully submerged, 2 m under the surface
        let mut vy = 0.0;
        for _ in 0..steps {
            let (ny, nvy) = swim_vertical_step(
                y, vy, y - half, y + half, water, surfacing,
                body_density, 1000.0, G, 0.05, &p,
            );
            y = ny;
            vy = nvy;
        }
        let sub = submerged_fraction(y - half, y + half, water);
        (sub, head_above_water(y + half, water))
    }

    #[test]
    fn treading_swimmer_lifts_head_above_water() {
        // A near-water-density body (1005, slightly denser) that would otherwise
        // sit at the surface or slowly sink, while ACTIVELY surfacing, settles
        // with its head above water (submerged ≈ surface_submersion = 0.9).
        let (sub, head_out) = settle_swim(1005.0, /*surfacing*/ true, 1.8, 2000);
        assert!(head_out, "treading swimmer's head should be above water (sub={sub})");
        assert!((sub - params().surface_submersion).abs() < 0.08,
            "should hold near the surface waterline, got submerged={sub}");
    }

    #[test]
    fn dense_body_sinks_when_not_surfacing() {
        // Same dense body, NOT surfacing → it does not hold its head up (sinks /
        // stays under). This is the "stop treading and you go under" behaviour.
        let (_sub, head_out) = settle_swim(1005.0, /*surfacing*/ false, 1.8, 2000);
        assert!(!head_out, "without treading, a denser-than-water body's head goes under");
    }

    #[test]
    fn swim_vertical_step_is_finite_under_large_dt() {
        let p = params();
        let (y, vy) = swim_vertical_step(64.0, 0.0, 63.0, 65.0, 64.0, true, 1000.0, 1000.0, G, 1000.0, &p);
        assert!(y.is_finite() && vy.is_finite());
    }

    #[test]
    fn surfacing_is_deterministic() {
        let a = settle_swim(990.0, true, 1.8, 500);
        let b = settle_swim(990.0, true, 1.8, 500);
        assert_eq!(a, b);
    }
}
