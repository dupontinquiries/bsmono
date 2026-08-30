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

/// Samples per fold.
///
/// This was 24 (about 18 Hz over `xform::BLEND_SEC`'s 1.3s), reasoned by
/// analogy to posecode's 12 Hz over a human motion clip. That analogy is
/// the bug: posecode samples a clip meant to be judged as a clip. A fold
/// is judged frame by frame, at whatever the game renders at — 60 Hz,
/// commonly — and 18 Hz is nearly 3.5x coarser than that. Measured
/// directly (`examples/sample_density_check.rs`): a chassis's worst
/// per-vertex speed came out at a constant 4.54 units/s across sample
/// counts from 24 to 384 — proof the vertex moves smoothly and the
/// "pop" `audit` reported at 24 samples was the sampler seeing one big
/// coarse step of ordinary motion, not a discontinuity. A discontinuity
/// would have shown RISING implied velocity as the sampling got finer
/// around it, not a flat line.
///
/// 120 covers real playback with margin across the chassis-duration
/// range `dur_mul` allows (0.8x-1.35x of 1.3s): even the slowest fold,
/// 1.75s, samples faster than 60 Hz (1.75 / 119 ≈ 68 Hz).
pub const SAMPLES: usize = 120;

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

    // THE AUTHORED POSES ARE THE BASELINE, not zero overlap.
    //
    // A tank's road wheels sit inside its hull; a shoulder sits in its
    // socket. That is how the models are built, and measuring against zero
    // reported it as a fault at t=0.00 before anything had moved. The sweep
    // said 251 of 254 chassis were broken with 3.5 MILLION faults — a number
    // so large and so wrong that the only possible response was to ignore the
    // tool, which is worse than not having it.
    //
    // What the module exists to find is a part passing THROUGH another during
    // transit. So the question is not "do these overlap" but "do they overlap
    // MORE than they do at rest in either end pose". Same reasoning as
    // `redistribute_length_to_height`, which caps mid-fold overshoot against
    // the endpoints rather than against nothing.
    let pair_baseline = |m: &mut Morph| -> Vec<f32> {
        let mut base = vec![0.0f32; prims * prims];
        for t in [0.0f32, 1.0] {
            let boxes: Vec<Option<Aabb>> =
                (0..prims).map(|p| Aabb::of(&m.sample(p, t).verts)).collect();
            for a in 0..prims {
                for b in (a + 1)..prims {
                    let (Some(ba), Some(bb)) = (boxes[a], boxes[b]) else { continue };
                    let d = ba.overlap_depth(&bb);
                    if d <= 0.0 {
                        continue;
                    }
                    let scale = ba.shortest_axis().min(bb.shortest_axis());
                    if scale > 1e-6 {
                        let rel = d / scale;
                        let e = &mut base[a * prims + b];
                        if rel > *e {
                            *e = rel;
                        }
                    }
                }
            }
        }
        base
    };
    let baseline = pair_baseline(m);

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
                // excess over what the end poses already do
                let excess = rel - baseline[a * prims + b];
                worst_overlap = worst_overlap.max(excess);
                if excess > OVERLAP_TOL {
                    faults.push(Fault::SelfIntersection { a, b, t, depth: excess });
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

    pub(super) fn cube(cx: f32, cy: f32, cz: f32, r: f32) -> Vec<Vertex> {
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

    /// The case this module exists for: an arm sweeping THROUGH a torso —
    /// apart at both ends, overlapping only in transit.
    ///
    /// The fixture used to end with both prims on the same point, and its own
    /// doc said so: "start apart and END on top of each other". That is not a
    /// sweep-through, it is a broken end pose, and once intersection is
    /// measured against the authored poses it is credited as construction.
    /// A swap tests the real thing: they cross at t=0.5 and are clear at
    /// both ends, so every bit of the overlap is transit.
    #[test]
    fn a_prim_passing_through_another_is_caught() {
        let a = vec![cube(-3.0, 0.0, 0.0, 0.5), cube(3.0, 0.0, 0.0, 0.5)];
        let b = vec![cube(3.0, 0.0, 0.0, 0.5), cube(-3.0, 0.0, 0.0, 0.5)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![a, b], idx).unwrap();
        let r = audit(&mut m, 100.0); // pop tolerance high: isolate the intersection
        assert!(
            r.faults.iter().any(|f| matches!(f, Fault::SelfIntersection { .. })),
            "interpenetration went unnoticed: worst overlap {:.2}",
            r.worst_overlap
        );
    }

    /// The other half of the same rule: overlap that is there in BOTH end
    /// poses is how the model is built and must not be reported.
    #[test]
    fn overlap_the_model_was_authored_with_is_not_a_fault() {
        // a wheel inside a hull: coincident at rest, coincident when folded,
        // and it never moves relative to the other prim
        let at_rest = vec![cube(0.0, 0.0, 0.0, 1.0), cube(0.0, 0.0, 0.0, 0.4)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![at_rest.clone(), at_rest], idx).unwrap();
        let r = audit(&mut m, 100.0);
        assert!(
            !r.faults.iter().any(|f| matches!(f, Fault::SelfIntersection { .. })),
            "a prim that never moves relative to another was called a fault: {:?}",
            r.faults.first()
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

    /// A regression for the sampling-rate bug SAMPLES's own doc comment
    /// now explains: fast-but-smooth motion, sampled too coarsely, reads
    /// as a "pop" that isn't there. A single vertex travelling at a
    /// constant, ordinary speed across the whole fold — no acceleration,
    /// no direction change, nothing a player could perceive as a
    /// discontinuity — must not trigger `Fault::Pop` at whatever `SAMPLES`
    /// is, or the sampling is still too coarse for real playback.
    #[test]
    fn constant_velocity_motion_does_not_read_as_a_pop() {
        let speed = 4.5; // units/sec, in the range measured on a real chassis
        let travel = speed * crate::xform::BLEND_SEC;
        let radius = 0.3;
        let a = vec![cube(0.0, 0.0, 0.0, radius)];
        let b = vec![cube(travel, 0.0, 0.0, radius)];
        let idx = vec![vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![a, b], idx).unwrap();
        // pop_tol_for's own formula (foldsim.rs): a quarter of the body's
        // shortest half-extent. A fixed lenient tolerance would pass at
        // ANY sample count and prove nothing about SAMPLES specifically.
        let tol = radius * 0.25;
        let r = audit(&mut m, tol);
        assert!(
            !r.faults.iter().any(|f| matches!(f, Fault::Pop { .. })),
            "smooth constant-speed motion ({speed} u/s) read as a pop at SAMPLES={SAMPLES}: {:?}",
            r.faults.iter().map(|f| f.line()).collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod chord_tests {
    use super::tests::cube;
    use super::*;

    /// WHY EVERY CHASSIS FAULTS, as a two-prim demonstration.
    ///
    /// The sweep says not one of 251 folds cleanly and the best still has 58
    /// faults. That is not 251 bad models; it is one property of the fold.
    /// `Morph::sample` lerps vertices, so a part travels the CHORD between
    /// its two poses — the straight line — and any part whose start and end
    /// are on opposite sides of another part goes through it.
    ///
    /// Nothing in the pipeline routes around anything. Worth stating as a
    /// test so the conclusion in `--fold sweep` is not just a sentence
    /// somebody wrote.
    #[test]
    fn a_lerp_takes_the_chord_and_the_chord_goes_through_things() {
        // A swings from one side to the other; B sits still in the middle.
        // Both end poses are clear; the straight path is not.
        let pose_a = vec![cube(-3.0, 0.0, 0.0, 0.5), cube(0.0, 2.5, 0.0, 0.5)];
        let pose_b = vec![cube(3.0, 0.0, 0.0, 0.5), cube(0.0, 2.5, 0.0, 0.5)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![pose_a, pose_b], idx).unwrap();
        let r = audit(&mut m, 100.0);
        // clear at both ends, so anything reported is transit
        assert!(
            !r.faults.iter().any(|f| matches!(f, Fault::SelfIntersection { .. })),
            "these two never meet; the chord passes below the stationary prim"
        );

        // now put the obstacle ON the chord
        let pose_a = vec![cube(-3.0, 0.0, 0.0, 0.5), cube(0.0, 0.0, 0.0, 0.6)];
        let pose_b = vec![cube(3.0, 0.0, 0.0, 0.5), cube(0.0, 0.0, 0.0, 0.6)];
        let idx = vec![vec![0u32, 1, 2], vec![0u32, 1, 2]];
        let mut m = Morph::build(vec![pose_a, pose_b], idx).unwrap();
        let r = audit(&mut m, 100.0);
        assert!(
            r.faults.iter().any(|f| matches!(f, Fault::SelfIntersection { .. })),
            "a part swinging across a stationary one did not register"
        );
        // and it happens in transit, not at an endpoint
        for f in &r.faults {
            if let Fault::SelfIntersection { t, .. } = f {
                assert!(*t > 0.0 && *t < 1.0, "reported at an endpoint: t={t}");
            }
        }
    }
}
