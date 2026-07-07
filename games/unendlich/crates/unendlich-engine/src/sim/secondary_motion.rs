//! `sim::secondary_motion` — the SHARED wind/force-driven flutter core.
//!
//! One response model for every deformable thing that trails, hangs or waves —
//! foliage, fabric, hair, a dragon's wing membrane — so they all bend from the same
//! influence and the same material description. (Distinct from `sim::deform`, which
//! is volume-preserving squash & stretch of solid parts.) Two tiers, matching the
//! architecture we designed:
//!
//!   * **Tier 1 — stateless quasi-static.** A point bends under the total influence,
//!     pivoting about its anchor: [`deflect`]. No history, one evaluation — what a
//!     *vertex shader* runs per frame for millions of leaves, or a cloak that only
//!     needs to lift in the wind.
//!   * **Tier 2 — stateful Verlet.** [`verlet_step`] integrates a particle under the
//!     influence and satisfies a distance constraint back to its anchor, giving real
//!     lag / overshoot / settle — what a *GPU compute pass* runs for the small
//!     particle counts of a hero cloak or long hair.
//!
//! Both tiers consume the SAME influence vector (perturbed wind drag + gravity +
//! inertial lag) via [`influence`], and the SAME [`Material`]. The wind already folds
//! in movers/foliage/terrain through [`crate::sim::wind_perturb`]; disturber
//! *strength* comes from a creature's heft via [`disturber_strength`]. Pure,
//! deterministic, allocation-free, panic-free — the CPU reference and the spec the
//! GPU code mirrors.

/// How a deformable element responds. `stiffness` resists bending (limp cloth low,
/// a rigid branch high); `damping` bleeds off oscillation (Tier 2 only).
#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub stiffness: f32,
    pub damping: f32,
}

impl Material {
    pub fn new(stiffness: f32, damping: f32) -> Material {
        Material { stiffness: stiffness.max(0.05), damping: damping.clamp(0.0, 1.0) }
    }
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }
fn len3(a: [f32; 3]) -> f32 { dot3(a, a).sqrt() }
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn scale3(a: [f32; 3], s: f32) -> [f32; 3] { [a[0]*s, a[1]*s, a[2]*s] }
fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = len3(a);
    if l > 1e-5 { [a[0]/l, a[1]/l, a[2]/l] } else { [0.0, 0.0, 0.0] }
}

/// Total influence on a surface point. Wind imparts drag scaled by how much the
/// surface FACES it (`|n·ŵ|` — an edge-on patch catches little, the cloth-wind idea),
/// plus gravity pulling down, minus the element's own acceleration (so a cloak lags
/// behind a sprinting hero instead of teleporting with them).
pub fn influence(wind: [f32; 3], normal: [f32; 3], area: f32, gravity: f32, self_accel: [f32; 3]) -> [f32; 3] {
    let n = norm3(normal);
    let wdir = norm3(wind);
    let face = dot3(n, wdir).abs(); // 0 edge-on … 1 face-on
    let drag = scale3(wind, face * area.max(0.0));
    let grav = [0.0, -gravity.max(0.0), 0.0];
    let lag = scale3(self_accel, -1.0);
    add3(add3(drag, grav), lag)
}

/// **Tier 1** — quasi-static deflection offset for a point at `lever` distance from
/// its anchor, with `anchor_weight` (`0` pinned root … `1` free tip), under a total
/// `force`. Bend rises with force, lever arm and freedom, falls with stiffness, and
/// is clamped to the lever so an element bends but never inverts through its anchor.
pub fn deflect(force: [f32; 3], material: &Material, anchor_weight: f32, lever: f32) -> [f32; 3] {
    let w = anchor_weight.clamp(0.0, 1.0);
    let l = lever.max(0.0);
    let mag = len3(force);
    if mag < 1e-6 || w < 1e-6 || l < 1e-6 { return [0.0, 0.0, 0.0]; }
    let dir = scale3(force, 1.0 / mag);
    let bend = (mag * l * w / material.stiffness).min(l);
    scale3(dir, bend)
}

/// A Verlet particle: current + previous position encode velocity implicitly.
#[derive(Clone, Copy, Debug)]
pub struct Particle { pub pos: [f32; 3], pub prev: [f32; 3] }

impl Particle {
    pub fn at(pos: [f32; 3]) -> Particle { Particle { pos, prev: pos } }
}

/// **Tier 2** — one Verlet step: integrate `p` under `force` with velocity damping,
/// then satisfy a distance constraint pulling it to within `rest_len` of its
/// `anchor` (the bone/root it hangs from). Stateful — feed the returned particle back
/// next frame. This is what gives cloth and hair their lag, swing and settle.
pub fn verlet_step(p: &Particle, anchor: [f32; 3], rest_len: f32, force: [f32; 3], material: &Material, dt: f32) -> Particle {
    let dt = dt.max(1e-4);
    let vel = scale3(sub3(p.pos, p.prev), 1.0 - material.damping);
    let mut next = add3(add3(p.pos, vel), scale3(force, dt * dt));
    let to = sub3(next, anchor);
    let d = len3(to);
    if d > 1e-5 {
        let rest = rest_len.max(0.0);
        next = add3(anchor, scale3(to, rest / d));
    }
    Particle { pos: next, prev: p.pos }
}

/// Map a creature's heft to a wind-disturber `strength` for
/// [`crate::sim::wind_perturb::Mover`]: bigger mass and a heavier footfall
/// (`ground_shake`, height-driven since r580) shove the air harder. A default
/// 1.8-block person (`mass ≈ 70`, `ground_shake = 0`) barely registers; a giant carves.
pub fn disturber_strength(mass: f32, ground_shake: f32) -> f32 {
    let m = (mass.max(0.0) / 70.0 - 1.0).max(0.0);
    (m + ground_shake.max(0.0) * 1.5).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, e: f32) -> bool { (a - b).abs() <= e }

    #[test]
    fn deflect_scales_and_clamps() {
        let m = Material::new(2.0, 0.1);
        assert_eq!(deflect([5.0, 0.0, 0.0], &m, 0.0, 3.0), [0.0, 0.0, 0.0]);
        assert_eq!(deflect([5.0, 0.0, 0.0], &m, 1.0, 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(deflect([0.0, 0.0, 0.0], &m, 1.0, 3.0), [0.0, 0.0, 0.0]);
        let a = len3(deflect([2.0, 0.0, 0.0], &m, 1.0, 3.0));
        let b = len3(deflect([4.0, 0.0, 0.0], &m, 1.0, 3.0));
        assert!(b > a);
        let big = deflect([1000.0, 0.0, 0.0], &m, 1.0, 3.0);
        assert!(len3(big) <= 3.0 + 1e-4);
    }

    #[test]
    fn stiffer_bends_less() {
        let soft = Material::new(1.0, 0.1);
        let rigid = Material::new(8.0, 0.1);
        assert!(len3(deflect([3.0, 0.0, 0.0], &soft, 1.0, 2.0)) > len3(deflect([3.0, 0.0, 0.0], &rigid, 1.0, 2.0)));
    }

    #[test]
    fn influence_faces_the_wind() {
        let wind = [5.0, 0.0, 0.0];
        let face_on = influence(wind, [1.0, 0.0, 0.0], 1.0, 0.0, [0.0; 3]);
        let edge_on = influence(wind, [0.0, 0.0, 1.0], 1.0, 0.0, [0.0; 3]);
        assert!(len3(face_on) > len3(edge_on));
        assert!(approx(len3(edge_on), 0.0, 1e-4));
    }

    #[test]
    fn influence_includes_gravity_and_inertial_lag() {
        let g = influence([0.0; 3], [1.0, 0.0, 0.0], 1.0, 9.8, [0.0; 3]);
        assert!(g[1] < 0.0);
        let lag = influence([0.0; 3], [1.0, 0.0, 0.0], 1.0, 0.0, [4.0, 0.0, 0.0]);
        assert!(lag[0] < 0.0);
    }

    #[test]
    fn verlet_relaxes_to_the_constraint() {
        let m = Material::new(1.0, 0.2);
        let anchor = [0.0, 0.0, 0.0];
        let mut p = Particle::at([5.0, 0.0, 0.0]);
        for _ in 0..200 { p = verlet_step(&p, anchor, 1.0, [0.0; 3], &m, 1.0 / 60.0); }
        assert!(approx(len3(sub3(p.pos, anchor)), 1.0, 1e-3));
        assert!(p.pos.iter().all(|c| c.is_finite()));
    }

    #[test]
    fn verlet_swings_under_gravity() {
        let m = Material::new(1.0, 0.05);
        let anchor = [0.0, 0.0, 0.0];
        let mut p = Particle::at([1.0, 0.0, 0.0]);
        for _ in 0..30 { p = verlet_step(&p, anchor, 1.0, [0.0, -9.8, 0.0], &m, 1.0 / 60.0); }
        assert!(p.pos[1] < 0.0);
        assert!(approx(len3(sub3(p.pos, anchor)), 1.0, 1e-2));
    }

    #[test]
    fn disturber_strength_from_stature() {
        assert_eq!(disturber_strength(70.0, 0.0), 0.0);
        assert!(disturber_strength(2000.0, 5.0) > disturber_strength(200.0, 0.5));
    }
}
