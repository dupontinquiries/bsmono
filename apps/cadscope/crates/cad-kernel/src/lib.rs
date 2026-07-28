//! cad-kernel — CADscope's geometry kernel, v0. std + glam only; no egui, no wgpu, no I/O.
//!
//! The design commitment (CADSCOPE_PROMPT.md §5): **f-rep (analytic SDF) is the modeling truth,
//! meshes are the viewing/export truth**. This crate owns the truth side — `sdf` (primitives, CSG,
//! Blend, Shell) and `ray` (sphere tracing) — plus the thin `Part`/`Model` scene the app and the
//! forge evaluate into. Meshing (adaptive dual contouring) lands here at M1.
//!
//! Extraction discipline copied from `apps/logscope/crates/logscope-fan`: keep this crate
//! zero-heavy-deps so editing kernel logic recompiles in seconds and headless consumers (tests,
//! the liaison's verifiers, CI goldens) take just the logic.

pub mod mesh;
pub mod ray;
pub mod sdf;
pub mod spine;

use glam::Vec3;
pub use mesh::{dual_contour, MeshOpts, TriMesh};
pub use ray::{Hit, Ray, raycast};
pub use sdf::{Sdf, Section};
pub use spine::{Grower, Node, Spine, Turtle};

/// One named solid in the model. `id` is assigned by evaluation order (the forge document owns
/// NAMES; ids are ephemeral render/pick handles — "names not IDs" per games/EDITORS.md).
#[derive(Clone, Debug)]
pub struct Part {
    pub id: u32,
    pub name: String,
    pub sdf: Sdf,
    /// Linear-space RGBA tint the viewport multiplies into shading.
    pub color: [f32; 4],
}

/// The evaluated scene: what a forge `Doc` becomes, what the viewport draws, what rays probe.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub parts: Vec<Part>,
}

/// A scene-level pick result: which part, and the surface hit on it.
#[derive(Clone, Copy, Debug)]
pub struct Pick {
    pub part_id: u32,
    pub hit: Hit,
}

impl Model {
    /// Cast against every part; nearest hit wins. O(parts) sphere traces — fine for M0 scenes;
    /// a BVH over `bound_radius` spheres is the M1 upgrade when assemblies grow.
    pub fn pick(&self, ray: &Ray, max_t: f32) -> Option<Pick> {
        let mut best: Option<Pick> = None;
        for part in &self.parts {
            if let Some(hit) = raycast(&part.sdf, ray, max_t) {
                if best.map_or(true, |b| hit.t < b.hit.t) {
                    best = Some(Pick { part_id: part.id, hit });
                }
            }
        }
        best
    }

    /// Loose world bound of everything — drives camera framing.
    pub fn bound_radius(&self) -> f32 {
        self.parts.iter().map(|p| p.sdf.bound_radius()).fold(1.0f32, f32::max)
    }

    /// Union of every part's bounds — what "frame everything" should use, since `bound_radius`
    /// is measured from the origin and says nothing about where the model actually sits.
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.parts {
            let (l, h) = p.sdf.aabb();
            lo = lo.min(l);
            hi = hi.max(h);
        }
        if lo.is_finite() && hi.is_finite() { (lo, hi) } else { (Vec3::ZERO, Vec3::ZERO) }
    }

    pub fn part(&self, id: u32) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }
}

/// A probe the measure tool (M2) and tests share: exact distance from a point to a part's surface.
pub fn distance_to(part: &Part, p: Vec3) -> f32 {
    part.sdf.eval(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdf::{sphere, translate};
    use glam::Vec3;

    fn two_spheres() -> Model {
        Model {
            parts: vec![
                Part { id: 1, name: "near".into(), sdf: translate(Vec3::new(0.0, 0.0, -5.0), sphere(1.0)), color: [1.0; 4] },
                Part { id: 2, name: "far".into(), sdf: translate(Vec3::new(0.0, 0.0, -12.0), sphere(1.0)), color: [1.0; 4] },
            ],
        }
    }

    #[test]
    fn pick_prefers_the_nearest_part() {
        let m = two_spheres();
        let pick = m.pick(&Ray::new(Vec3::ZERO, Vec3::NEG_Z), 100.0).expect("hit");
        assert_eq!(pick.part_id, 1);
        assert!((pick.hit.t - 4.0).abs() < 5e-3);
    }

    #[test]
    fn pick_reaches_past_a_miss() {
        let mut m = two_spheres();
        m.parts[0].sdf = translate(Vec3::new(50.0, 0.0, -5.0), sphere(1.0)); // move near one aside
        let pick = m.pick(&Ray::new(Vec3::ZERO, Vec3::NEG_Z), 100.0).expect("hit");
        assert_eq!(pick.part_id, 2);
    }
}
