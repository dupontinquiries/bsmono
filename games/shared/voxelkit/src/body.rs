//! body — one description of a creature, two outputs.
//!
//! A [`BodyPlan`] is a list of [`BodyPart`]s: a [`Role`], a name, and a
//! [`Solid`] describing the shape. [`BodyPlan::build`] rasterises every part
//! and, from the SAME rasterisation, derives the `RigPart` the rig needs.
//!
//! Deriving the box from the voxels rather than asking the author for both is
//! the point. If a plan carried a hand-written half-extent alongside its
//! solid, the two would drift the first time somebody widened a leg — and the
//! symptom would be a creature that draws correctly and poses wrongly, which
//! is near-impossible to attribute. Here the box IS the voxels' bounding box,
//! so widening the leg moves the hip.
//!
//! ## Leg order is load-bearing
//!
//! `creature_rig`'s gait tables are indexed `[FL, FR, HL, HR]` and the rig
//! reads legs in the order the parts arrive. Getting that wrong yields a
//! different real gait (a pace instead of a trot) rather than an error — the
//! exact trap the rig's own built-in rule fell into. So [`BodyPlan::build`]
//! **sorts legs into that canonical order** from their own positions rather
//! than trusting declaration order, and [`PlanError::LegCount`] rejects a
//! quadruped that does not have four.

use std::collections::BTreeMap;

use creature_rig::anim_rig::{RigPart, Role};
use voxel_tools::solid::{rasterize, Solid};

/// One labelled piece of a body.
#[derive(Debug, Clone)]
pub struct BodyPart {
    /// What the rig should do with it.
    pub role: Role,
    /// For diagnostics and for an authoring tool to address a part by name.
    pub name: String,
    pub solid: Solid,
}

impl BodyPart {
    pub fn new(role: Role, name: impl Into<String>, solid: Solid) -> BodyPart {
        BodyPart { role, name: name.into(), solid }
    }
}

/// A whole creature, before rasterisation.
#[derive(Debug, Clone, Default)]
pub struct BodyPlan {
    pub name: String,
    /// The torso. Kept separate because the rig takes it separately
    /// (`pose(parts, torso_half, ..)`) and because it defines the body-local
    /// origin everything else is measured from.
    pub torso: Option<Solid>,
    pub parts: Vec<BodyPart>,
    /// World units per voxel. Sets how fine the rasterisation is; the rig's
    /// geometry is in world units either way.
    pub voxel: f32,
}

/// Why a plan could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    NoTorso,
    /// A part rasterised to nothing — a solid entirely outside its own
    /// bounds, or a `Diff` that removed everything. Caught because an empty
    /// part would otherwise become a zero-extent box and a leg that never
    /// plants.
    EmptyPart(String),
    /// A quadruped needs four legs for the gait tables to mean anything.
    LegCount(usize),
    /// Voxel size must be positive and finite.
    BadVoxelSize,
    EmptyName,
}

/// The result: what to draw, and what to pose.
#[derive(Debug, Clone)]
pub struct BuiltBody {
    /// Occupied cells per part, in grid coordinates, keyed by part index
    /// (`None` = torso). A mesher turns these into geometry.
    pub cells: Vec<(Option<usize>, BTreeMap<(i32, i32, i32), u16>)>,
    /// Half-extents of the torso, in world units — the rig's `torso_half`.
    pub torso_half: [f32; 3],
    /// The rig's anatomy, legs already in `[FL, FR, HL, HR]` order.
    pub rig: Vec<RigPart>,
    /// Names parallel to `rig`, for diagnostics.
    pub names: Vec<String>,
}

/// Bounding box of a rasterised part, in world units: `(centre, half)`.
fn bounds_of(cells: &BTreeMap<(i32, i32, i32), u16>, voxel: f32) -> Option<([f32; 3], [f32; 3])> {
    let mut lo = [i32::MAX; 3];
    let mut hi = [i32::MIN; 3];
    for &(x, y, z) in cells.keys() {
        for (i, v) in [x, y, z].into_iter().enumerate() {
            lo[i] = lo[i].min(v);
            hi[i] = hi[i].max(v);
        }
    }
    if lo[0] > hi[0] {
        return None;
    }
    let mut centre = [0.0f32; 3];
    let mut half = [0.0f32; 3];
    for i in 0..3 {
        // A single occupied cell spans one voxel, not zero — hence the +1.
        // Without it a one-cell part gets a zero half-extent, and the rig
        // divides by leg length.
        let span = (hi[i] - lo[i] + 1) as f32 * voxel;
        half[i] = span * 0.5;
        centre[i] = (lo[i] as f32 * voxel) + half[i];
    }
    Some((centre, half))
}

/// Canonical leg order: front-left, front-right, hind-left, hind-right.
///
/// Body-local convention, matching `creature_rig`: **X forward, Y up,
/// Z right**. So "front" is greater X and "right" is greater Z.
///
/// Sorted from the legs' own positions rather than taken on trust. The rig
/// indexes gait tables positionally, and a mis-ordered pair silently becomes
/// a different gait — there is no error to catch it downstream.
fn leg_rank(offset: [f32; 3]) -> u8 {
    let front = offset[0] >= 0.0;
    let right = offset[2] >= 0.0;
    match (front, right) {
        (true, false) => 0,  // FL
        (true, true) => 1,   // FR
        (false, false) => 2, // HL
        (false, true) => 3,  // HR
    }
}

impl BodyPlan {
    pub fn new(name: impl Into<String>, voxel: f32) -> BodyPlan {
        BodyPlan { name: name.into(), torso: None, parts: Vec::new(), voxel }
    }

    pub fn with_torso(mut self, s: Solid) -> BodyPlan {
        self.torso = Some(s);
        self
    }

    pub fn part(mut self, role: Role, name: impl Into<String>, s: Solid) -> BodyPlan {
        self.parts.push(BodyPart::new(role, name, s));
        self
    }

    pub fn leg_count(&self) -> usize {
        self.parts.iter().filter(|p| p.role == Role::Leg).count()
    }

    /// Rasterise, and derive the rig anatomy from the same cells.
    ///
    /// `require_quadruped` rejects anything that is not four-legged. Pass
    /// false for a biped, a bird or a torso-only prop; pass true when the
    /// result will be fed gait tables, which assume four.
    pub fn build(&self, require_quadruped: bool) -> Result<BuiltBody, PlanError> {
        if self.name.trim().is_empty() {
            return Err(PlanError::EmptyName);
        }
        if !self.voxel.is_finite() || self.voxel <= 0.0 {
            return Err(PlanError::BadVoxelSize);
        }
        let torso = self.torso.as_ref().ok_or(PlanError::NoTorso)?;
        if require_quadruped && self.leg_count() != 4 {
            return Err(PlanError::LegCount(self.leg_count()));
        }

        let torso_cells = rasterize(torso);
        let (torso_centre, torso_half) =
            bounds_of(&torso_cells, self.voxel).ok_or_else(|| PlanError::EmptyPart("torso".into()))?;

        let mut cells = vec![(None, torso_cells)];
        // (rank, role, offset, half, name) — rank orders legs, everything
        // else keeps declaration order after them.
        let mut limbs: Vec<(u8, Role, [f32; 3], [f32; 3], String)> = Vec::new();

        for (i, p) in self.parts.iter().enumerate() {
            let c = rasterize(&p.solid);
            let (centre, half) =
                bounds_of(&c, self.voxel).ok_or_else(|| PlanError::EmptyPart(p.name.clone()))?;
            // Body-local: every part is measured from the torso's centre, so
            // moving the whole creature moves nothing relative to the rig.
            let offset = [
                centre[0] - torso_centre[0],
                centre[1] - torso_centre[1],
                centre[2] - torso_centre[2],
            ];
            let rank = if p.role == Role::Leg { leg_rank(offset) } else { 4 };
            limbs.push((rank, p.role, offset, half, p.name.clone()));
            cells.push((Some(i), c));
        }

        // Stable sort: legs into [FL, FR, HL, HR], everything else after them
        // in the order it was declared.
        limbs.sort_by_key(|(rank, _, _, _, _)| *rank);

        let rig = limbs
            .iter()
            .map(|(_, role, offset, half, _)| RigPart { role: *role, offset: *offset, half: *half })
            .collect();
        let names = limbs.into_iter().map(|(_, _, _, _, n)| n).collect();

        Ok(BuiltBody { cells, torso_half, rig, names })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_tools::solid::Material;

    fn boxx(min: [f32; 3], max: [f32; 3]) -> Solid {
        Solid::Box { min, max, mat: Material::structural(1) }
    }

    /// A quadruped authored once yields both the voxels to draw and the
    /// anatomy to pose — the whole point of the crate.
    fn quadruped() -> BodyPlan {
        BodyPlan::new("test cat", 1.0)
            .with_torso(boxx([-3.0, 0.0, -2.0], [3.0, 3.0, 2.0]))
            // Declared in a DELIBERATELY jumbled order to prove the sort.
            .part(Role::Leg, "hind-right", boxx([-3.0, -3.0, 1.0], [-2.0, 0.0, 2.0]))
            .part(Role::Leg, "front-left", boxx([2.0, -3.0, -2.0], [3.0, 0.0, -1.0]))
            .part(Role::Tail, "tail", boxx([-5.0, 1.0, -1.0], [-3.0, 2.0, 1.0]))
            .part(Role::Leg, "hind-left", boxx([-3.0, -3.0, -2.0], [-2.0, 0.0, -1.0]))
            .part(Role::Head, "head", boxx([3.0, 2.0, -1.0], [5.0, 4.0, 1.0]))
            .part(Role::Leg, "front-right", boxx([2.0, -3.0, 1.0], [3.0, 0.0, 2.0]))
    }

    /// THE ORDERING TRAP. The rig indexes gait tables positionally, so a
    /// mis-ordered pair silently becomes a different gait rather than an
    /// error. Legs must come out [FL, FR, HL, HR] regardless of how they
    /// were declared.
    #[test]
    fn legs_are_sorted_into_the_canonical_order_whatever_the_author_did() {
        let b = quadruped().build(true).unwrap();
        let legs: Vec<&String> = b
            .rig
            .iter()
            .zip(&b.names)
            .filter(|(p, _)| p.role == Role::Leg)
            .map(|(_, n)| n)
            .collect();
        assert_eq!(legs, vec!["front-left", "front-right", "hind-left", "hind-right"]);
        // And they are the FIRST four parts, which is what makes positional
        // indexing safe for the caller.
        for p in b.rig.iter().take(4) {
            assert_eq!(p.role, Role::Leg);
        }
    }

    /// X forward, Z right — the convention the sort depends on.
    #[test]
    fn leg_rank_follows_the_body_local_convention() {
        assert_eq!(leg_rank([1.0, 0.0, -1.0]), 0, "front-left");
        assert_eq!(leg_rank([1.0, 0.0, 1.0]), 1, "front-right");
        assert_eq!(leg_rank([-1.0, 0.0, -1.0]), 2, "hind-left");
        assert_eq!(leg_rank([-1.0, 0.0, 1.0]), 3, "hind-right");
    }

    /// Offsets are relative to the torso, so translating the whole creature
    /// must not change the rig at all.
    #[test]
    fn the_anatomy_is_body_local_not_world_space() {
        let a = quadruped().build(true).unwrap();
        let shifted = BodyPlan::new("test cat", 1.0)
            .with_torso(boxx([97.0, 50.0, -2.0], [103.0, 53.0, 2.0]))
            .part(Role::Leg, "front-left", boxx([102.0, 47.0, -2.0], [103.0, 50.0, -1.0]));
        let s = shifted.build(false).unwrap();
        let a_fl = a.rig.iter().position(|p| p.role == Role::Leg).unwrap();
        // Same limb, same local offset, despite being 100 units away.
        assert!((a.rig[a_fl].offset[0] - s.rig[0].offset[0]).abs() < 1e-3,
            "{:?} vs {:?}", a.rig[a_fl].offset, s.rig[0].offset);
        assert!((a.rig[a_fl].offset[2] - s.rig[0].offset[2]).abs() < 1e-3);
    }

    /// One occupied cell spans one voxel, not zero. A zero half-extent would
    /// give the rig a leg of length zero to divide by.
    #[test]
    fn a_single_cell_part_has_a_real_extent() {
        let p = BodyPlan::new("tiny", 0.5)
            .with_torso(boxx([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
            .part(Role::Head, "nub", boxx([1.0, 1.0, 1.0], [1.4, 1.4, 1.4]));
        let b = p.build(false).unwrap();
        for h in b.rig[0].half {
            assert!(h > 0.0, "zero extent: {:?}", b.rig[0].half);
        }
        assert!(b.torso_half.iter().all(|h| *h > 0.0));
    }

    /// Voxel size scales the geometry; the rig works in world units.
    #[test]
    fn voxel_size_scales_the_anatomy() {
        let mk = |v: f32| {
            BodyPlan::new("s", v)
                .with_torso(boxx([0.0, 0.0, 0.0], [4.0, 4.0, 4.0]))
                .build(false)
                .unwrap()
                .torso_half[0]
        };
        let coarse = mk(1.0);
        let fine = mk(0.5);
        assert!((coarse - fine * 2.0).abs() < 1e-3, "{coarse} vs {fine}");
    }

    /// A part that rasterises to nothing must be refused. Silently becoming a
    /// zero-extent box gives a leg that never plants, which presents as an
    /// animation bug a long way from its cause.
    #[test]
    fn an_empty_part_is_refused_by_name() {
        let plan = BodyPlan::new("x", 1.0)
            .with_torso(boxx([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]))
            .part(Role::Leg, "ghost", Solid::Empty);
        assert_eq!(plan.build(false).unwrap_err(), PlanError::EmptyPart("ghost".into()));
    }

    #[test]
    fn a_plan_without_a_torso_is_refused() {
        let plan = BodyPlan::new("x", 1.0).part(Role::Leg, "l", boxx([0.0; 3], [1.0; 3]));
        assert_eq!(plan.build(false).unwrap_err(), PlanError::NoTorso);
    }

    /// Gait tables assume four legs. A three-legged creature fed to them
    /// indexes a table that does not describe it.
    #[test]
    fn a_quadruped_must_actually_have_four_legs() {
        let plan = BodyPlan::new("x", 1.0)
            .with_torso(boxx([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]))
            .part(Role::Leg, "a", boxx([0.0, -2.0, 0.0], [1.0, 0.0, 1.0]));
        assert_eq!(plan.build(true).unwrap_err(), PlanError::LegCount(1));
        // ...but the same plan is fine when nothing will index it by leg.
        assert!(plan.build(false).is_ok());
    }

    #[test]
    fn a_bad_voxel_size_or_name_is_refused() {
        let ok = boxx([0.0; 3], [2.0; 3]);
        for v in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let p = BodyPlan::new("x", v).with_torso(ok.clone());
            assert_eq!(p.build(false).unwrap_err(), PlanError::BadVoxelSize, "voxel {v}");
        }
        let p = BodyPlan::new("  ", 1.0).with_torso(ok);
        assert_eq!(p.build(false).unwrap_err(), PlanError::EmptyName);
    }

    /// Both outputs come from ONE rasterisation, so what is drawn and what is
    /// posed cannot disagree — the failure this crate exists to prevent.
    #[test]
    fn the_drawn_cells_and_the_posed_parts_describe_the_same_body() {
        let b = quadruped().build(true).unwrap();
        // torso + six parts
        assert_eq!(b.cells.len(), 7);
        assert_eq!(b.rig.len(), 6);
        assert_eq!(b.names.len(), b.rig.len());
        assert!(b.cells.iter().all(|(_, c)| !c.is_empty()), "no part drew nothing");
        // The torso entry is the one with no source index.
        assert_eq!(b.cells.iter().filter(|(i, _)| i.is_none()).count(), 1);
    }
}
