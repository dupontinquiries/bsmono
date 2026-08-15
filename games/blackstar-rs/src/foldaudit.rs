//! FOLD AUDIT — measuring a transformation instead of eyeballing it.
//!
//! Architecture borrowed from `posecode`'s evaluation layer (an
//! Apache-2.0/AGPL humanoid-motion project; see the note at the bottom
//! on what did and did not transfer). Its useful idea is not any
//! particular check — it is that a motion clip should be SAMPLED at a
//! fixed rate and measured against geometric invariants, producing a
//! per-phase report, rather than being judged by whether it looked
//! right once in a viewport.
//!
//! Until now a fold in this engine was correct if it looked correct.
//! `joints.rs` can already say "J11 is misparented" about the RIG, but
//! nothing can say anything about the MOTION: an arm can sweep through
//! the chest at t=0.4 and the only symptom is a frame someone might
//! notice. This module is the missing measurement.
//!
//! ## What it checks, and why these
//!
//! * **Self-intersection.** Prim AABBs that overlap by more than a
//!   tolerance, at any sampled point in the fold. Deliberately the
//!   first check built, because unlike joint range-of-motion it needs
//!   NO anatomical assumptions — a limb passing through a torso is
//!   wrong for a robot exactly as much as for a person, whereas a
//!   "knee" bending 200 degrees is wrong for a human and may be
//!   entirely correct for a machine that turns into a truck.
//! * **Volume collapse.** A prim whose bounding volume drops toward
//!   zero mid-fold has usually had its vertices lerped into a plane —
//!   the thin-shell failure (`compact.rs`, `thin_shell_check.py`)
//!   arriving through animation rather than through the asset.
//! * **Velocity spikes.** The largest per-vertex jump between adjacent
//!   samples. A fold with a discontinuity reads as a pop, and a pop is
//!   invisible in a still and obvious in motion — exactly the class of
//!   fault a sampled audit catches and a screenshot does not.
//! * **Non-finite output.** Any NaN or infinity. This is not
//!   theoretical: `compact.rs`'s curve produced NaN at exactly t=1.0 on
//!   its first version (f32 `sin(PI)` is slightly negative, and a
//!   negative base with a fractional exponent is NaN). That bug was
//!   caught by a unit test; this catches the same class across a whole
//!   fold, including whatever a future compaction style does.
//!
//! ## What did NOT transfer from posecode
//!
//! Its range-of-motion table is clinical human data — knee flex ceiling
//! 144 degrees, elbow 154, hip 135. Those numbers are wrong here and
//! importing them would be worse than having none: a transforming robot
//! violates human ROM BY DESIGN, and a validator that flags the correct
//! behaviour trains you to ignore it. The concept (declared per-joint
//! limits, validated, clamped) is right and this engine has none, but
//! the limits would have to come from each chassis's own fitted rig,
//! not from anatomy. Its grounding checks (plantigrade sole angle, heel
//! height, planted-foot drift) assume feet and a floor; a body halfway
//! through becoming a truck has neither.

use glam::Vec3;

use crate::xform::Morph;

/// Samples per fold. Posecode uses 12 Hz over a clip of arbitrary
/// length; a fold is `xform::BLEND_SEC` (1.3s), so 24 samples is about
/// 18 Hz — finer than posecode's, because a fold is short and dense
/// with motion where a squat is long and smooth.
pub const SAMPLES: usize = 24;

/// How deeply two prim AABBs may overlap before it counts, as a
/// fraction of the smaller prim's shortest axis. Not zero: prims that
/// SHOULD touch (a shoulder in its socket, a panel in its recess)
/// overlap slightly by construction, and flagging those would bury the
/// real faults. This is the same reasoning `joints.rs` already applies
/// with its `interior` flag.
pub const OVERLAP_TOL: f32 = 0.35;

/// A prim's bounding volume may not fall below this fraction of its
/// own volume at rest before it counts as collapsed.
pub const COLLAPSE_TOL: f32 = 0.08;

#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    /// two prims interpenetrating beyond tolerance
    SelfIntersection { a: usize, b: usize, t: f32, depth: f32 },
    /// a prim flattened toward a plane mid-fold
    VolumeCollapse { prim: usize, t: f32, ratio: f32 },
    /// a discontinuity between adjacent samples
    Pop { prim: usize, t: f32, jump: f32 },
    /// NaN or infinity in the output
    NonFinite { prim: usize, t: f32 },
}

impl Fault {
    /// One line, in the same voice `joints.rs` reports rig faults.
    pub fn line(&self) -> String {
        match self {
            Fault::SelfIntersection { a, b, t, depth } => {
                format!("t={t:.2}  P{a} through P{b}  depth {depth:.2}x")
            }
            Fault::VolumeCollapse { prim, t, ratio } => {
                format!("t={t:.2}  P{prim} COLLAPSED to {:.0}% of its resting volume", ratio * 100.0)
            }
            Fault::Pop { prim, t, jump } => format!("t={t:.2}  P{prim} POPS {jump:.2} units in one sample"),
            Fault::NonFinite { prim, t } => format!("t={t:.2}  P{prim} NON-FINITE (NaN or inf)"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Aabb {
    lo: Vec3,
    hi: Vec3,
}

impl Aabb {
    fn of(verts: &[crate::meshgen::Vertex]) -> Option<Aabb> {
        if verts.is_empty() {
            return None;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in verts {
            let p = Vec3::from_array(v.pos);
            if !p.is_finite() {
                return None;
            }
            lo = lo.min(p);
            hi = hi.max(p);
        }
        Some(Aabb { lo, hi })
    }
    fn size(&self) -> Vec3 {
        self.hi - self.lo
    }
    fn volume(&self) -> f32 {
        let s = self.size();
        (s.x.max(0.0)) * (s.y.max(0.0)) * (s.z.max(0.0))
    }
    fn shortest_axis(&self) -> f32 {
        let s = self.size();
        s.x.min(s.y).min(s.z).max(1e-6)
    }
    /// Overlap depth on the least-overlapping axis — the separating-axis
    /// quantity. Zero or negative means they do not overlap at all.
    fn overlap_depth(&self, other: &Aabb) -> f32 {
        let dx = (self.hi.x.min(other.hi.x) - self.lo.x.max(other.lo.x)).max(0.0);
        let dy = (self.hi.y.min(other.hi.y) - self.lo.y.max(other.lo.y)).max(0.0);
        let dz = (self.hi.z.min(other.hi.z) - self.lo.z.max(other.lo.z)).max(0.0);
        dx.min(dy).min(dz)
    }
}

/// The full report for one fold.
pub struct Report {
    pub faults: Vec<Fault>,
    pub samples: usize,
    pub prims: usize,
    /// worst overlap seen, as a fraction of the smaller prim's shortest
    /// axis — reported even when under tolerance, so a chassis creeping
    /// toward a fault is visible before it trips
    pub worst_overlap: f32,
    pub worst_pop: f32,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.faults.is_empty()
    }
    pub fn summary(&self) -> String {
        if self.clean() {
            format!(
                "FOLD CLEAN -- {} prims x {} samples, worst overlap {:.2}x, worst pop {:.2}",
                self.prims, self.samples, self.worst_overlap, self.worst_pop
            )
        } else {
            format!(
                "FOLD: {} fault(s) across {} prims x {} samples",
                self.faults.len(),
                self.prims,
                self.samples
            )
        }
    }
}

/// Sample a morph across its whole fold and measure it.
///
/// `pop_tol` is the per-sample jump a vertex may make before it counts
/// as a discontinuity, in model units. It is a caller argument rather
/// than a constant because it scales with the body: a bot the size of a
/// truck legitimately moves further per sample than a scout does.
pub fn audit(m: &mut Morph, pop_tol: f32) -> Report {
    let prims = m.prims();
    let mut faults = Vec::new();
    let mut worst_overlap: f32 = 0.0;
    let mut worst_pop: f32 = 0.0;

    // resting volumes, for the collapse check
    let rest: Vec<Option<Aabb>> = (0..prims).map(|p| Aabb::of(&m.sample(p, 0.0).verts)).collect();
    let mut prev: Vec<Option<Vec<Vec3>>> = vec![None; prims];

    for s in 0..SAMPLES {
        let t = s as f32 / (SAMPLES - 1) as f32;
        let mut boxes: Vec<Option<Aabb>> = Vec::with_capacity(prims);

        for p in 0..prims {
            let mesh = m.sample(p, t);
            let pts: Vec<Vec3> = mesh.verts.iter().map(|v| Vec3::from_array(v.pos)).collect();

            if pts.iter().any(|q| !q.is_finite()) {
                faults.push(Fault::NonFinite { prim: p, t });
                boxes.push(None);
                prev[p] = None;
                continue;
            }

            // pop: largest single-vertex jump since the previous sample
            if let Some(before) = &prev[p] {
                if before.len() == pts.len() {
                    let jump = before
                        .iter()
                        .zip(pts.iter())
                        .map(|(a, b)| (*b - *a).length())
                        .fold(0.0f32, f32::max);
                    worst_pop = worst_pop.max(jump);
                    if jump > pop_tol {
                        faults.push(Fault::Pop { prim: p, t, jump });
                    }
                }
            }
            prev[p] = Some(pts);

            let bb = Aabb::of(&mesh.verts);
            // collapse: volume against this prim's own resting volume
            if let (Some(now), Some(Some(r))) = (bb, rest.get(p)) {
                let rv = r.volume();
                if rv > 1e-9 {
                    let ratio = now.volume() / rv;
                    if ratio < COLLAPSE_TOL {
                        faults.push(Fault::VolumeCollapse { prim: p, t, ratio });
                    }
                }
            }
            boxes.push(bb);
        }

        // self-intersection: every prim pair, this sample
        for a in 0..prims {
            for b in (a + 1)..prims {
                let (Some(ba), Some(bb)) = (boxes[a], boxes[b]) else { continue };
                let depth = ba.overlap_depth(&bb);
                if depth <= 0.0 {
                    continue;
                }
                let scale = ba.shortest_axis().min(bb.shortest_axis());
                let rel = depth / scale;
                worst_overlap = worst_overlap.max(rel);
                if rel > OVERLAP_TOL {
                    faults.push(Fault::SelfIntersection { a, b, t, depth: rel });
                }
            }
        }
    }

    Report { faults, samples: SAMPLES, prims, worst_overlap, worst_pop }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshgen::Vertex;

    fn cube(cx: f32, cy: f32, cz: f32, r: f32) -> Vec<Vertex> {
        let mut out = Vec::new();
        for i in 0..8 {
            let sx = if i & 1 == 0 { -r } else { r };
            let sy = if i & 2 == 0 { -r } else { r };
            let sz = if i & 4 == 0 { -r } else { r };
            out.push(Vertex { pos: [cx + sx, cy + sy, cz + sz], nrm: [0.0, 1.0, 0.0], uv: [0.0, 0.0] });
        }
        out
    }

    /// two prims, well apart in both poses — nothing should fire
    #[test]
    fn a_clean_fold_reports_clean() {
        let a = vec![cube(-3.0, 0.0, 0.0, 0.5), cube(3.0, 0.0, 0.0, 0.5)];
        let b = vec![cube(-4.0, 0.0, 0.0, 0.5), cube(4.0, 0.0, 0.0, 0.5)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![a, b], idx).unwrap();
        let r = audit(&mut m, 1.0);
        assert!(r.clean(), "clean fold reported faults: {:?}", r.faults.iter().map(|f| f.line()).collect::<Vec<_>>());
        assert_eq!(r.prims, 2);
    }

    /// the case this module exists for: an arm sweeping through a torso.
    /// Two prims that start apart and END on top of each other.
    #[test]
    fn a_prim_passing_through_another_is_caught() {
        let a = vec![cube(-3.0, 0.0, 0.0, 0.5), cube(3.0, 0.0, 0.0, 0.5)];
        let b = vec![cube(0.0, 0.0, 0.0, 0.5), cube(0.0, 0.0, 0.0, 0.5)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![a, b], idx).unwrap();
        let r = audit(&mut m, 100.0); // pop tolerance high: isolate the intersection
        assert!(
            r.faults.iter().any(|f| matches!(f, Fault::SelfIntersection { .. })),
            "interpenetration went unnoticed: worst overlap {:.2}",
            r.worst_overlap
        );
    }

    /// a prim lerped into a plane — the thin-shell failure arriving
    /// through animation rather than through the asset
    #[test]
    fn a_prim_flattened_mid_fold_is_caught() {
        // pose A: a real cube. pose B: the same cube with zero Y extent.
        let a = vec![cube(0.0, 0.0, 0.0, 1.0)];
        let mut flat = cube(0.0, 0.0, 0.0, 1.0);
        for v in flat.iter_mut() {
            v.pos[1] = 0.0;
        }
        let b = vec![flat];
        let idx = vec![vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![a, b], idx).unwrap();
        let r = audit(&mut m, 100.0);
        assert!(
            r.faults.iter().any(|f| matches!(f, Fault::VolumeCollapse { .. })),
            "a prim flattened to a plane was not flagged"
        );
    }

    /// NaN must be caught rather than propagated — the exact class of
    /// bug compact.rs's curve had at t=1.0 on its first version
    #[test]
    fn non_finite_output_is_caught_rather_than_propagated() {
        let mut bad = cube(0.0, 0.0, 0.0, 1.0);
        bad[0].pos[0] = f32::NAN;
        let idx = vec![vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![vec![cube(0.0, 0.0, 0.0, 1.0)], vec![bad]], idx).unwrap();
        let r = audit(&mut m, 100.0);
        assert!(
            r.faults.iter().any(|f| matches!(f, Fault::NonFinite { .. })),
            "a NaN vertex passed the audit"
        );
    }

    /// tolerance is not zero on purpose: prims that should touch do
    #[test]
    fn prims_that_merely_touch_are_not_flagged() {
        // two unit cubes sharing a face — a shoulder in its socket
        let a = vec![cube(-1.0, 0.0, 0.0, 1.0), cube(1.0, 0.0, 0.0, 1.0)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![a.clone(), a], idx).unwrap();
        let r = audit(&mut m, 1.0);
        assert!(
            !r.faults.iter().any(|f| matches!(f, Fault::SelfIntersection { .. })),
            "touching prims were reported as interpenetrating"
        );
    }

    /// compaction must not be invisible to the audit -- it moves
    /// vertices, so whatever it does has to be measurable here, or the
    /// audit cannot protect the compaction work at all.
    ///
    /// Uses identical poses on purpose: a plain lerp between two
    /// identical poses produces NO motion whatsoever (worst_pop == 0),
    /// so any motion the audit measures is compaction's and only
    /// compaction's. An earlier version of this test asserted on
    /// overlap instead and failed for a real reason worth recording:
    /// `Style::Shrink` pulls each prim toward its OWN centroid, so two
    /// separated prims shrink individually and never approach each
    /// other. Compaction cannot make distant prims collide, and the
    /// test that assumed it could was wrong about the mechanism.
    #[test]
    fn the_audit_sees_the_motion_compaction_introduces() {
        let a = vec![cube(-1.2, 0.0, 0.0, 0.5), cube(1.2, 0.0, 0.0, 0.5)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];

        let mut plain = Morph::build(vec![a.clone(), a.clone()], idx.clone()).unwrap();
        let base = audit(&mut plain, 100.0);
        assert_eq!(base.worst_pop, 0.0, "identical poses should produce no motion at all");

        let mut hot = Morph::build(vec![a.clone(), a], idx).unwrap();
        hot.compaction = Some(crate::compact::Params {
            style: crate::compact::Style::Shrink,
            cell: 2.0,
            strength: 1.0,
            min_thickness: 0.0,
        });
        let after = audit(&mut hot, 100.0);
        assert!(
            after.worst_pop > 0.0,
            "compaction moved vertices but the audit measured none of it"
        );
    }
}
