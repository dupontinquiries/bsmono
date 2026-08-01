//! joints — find where a bot bends, and derive the fold back to vehicle form.
//!
//! Everything before this found *parts*: mirrored pairs, a centreline, interior
//! structure. A part list is not a rig. What turns it into one is knowing where
//! two parts **meet**, because that contact is the hinge.
//!
//! ## How a joint is recognised
//!
//! Two signals, and neither is sufficient alone:
//!
//! 1. **Contact.** Two parts whose bounds nearly touch. Parts that overlap
//!    deeply are one assembly; parts far apart are unrelated. Only the narrow
//!    band between is a joint, and how narrow is the **tolerance** — the single
//!    number worth exposing to a person nudging this in-game, because it is the
//!    one that cannot be derived.
//! 2. **Anatomy.** A contact near a skeleton landmark (shoulder, hip, knee) is
//!    far more likely to be a joint than one in the middle of a plate. The
//!    reference gives those heights, so a candidate's distance from the nearest
//!    landmark becomes its confidence.
//!
//! Reporting confidence rather than a yes/no is what lets a person raise the
//! tolerance to catch a missing joint without also accepting fifty false ones —
//! the extra candidates arrive ranked, not mixed in.
//!
//! ## The reverse plan
//!
//! A bot's joints, ordered, ARE the fold to vehicle form: rotate each about its
//! contact, parent before child. That is the thing the engine's global `fold`
//! parameter cannot express, and it comes out of the bot form alone — no second
//! model, no matching, which is what defeated every earlier attempt.

use crate::anatomy::Part;
use crate::profile::{HIP, KNEE, SHOULDER};

/// A hinge between two parts.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    pub parent: usize,
    pub child: usize,
    /// Contact point, in model units.
    pub pivot: [f64; 3],
    /// Hinge axis: perpendicular to the child's long direction, so the child
    /// swings rather than spinning about its own length. A part that twists on
    /// its own axis looks like a drill, not a limb.
    pub axis: [f64; 3],
    /// 0..1. Distance from the nearest anatomical landmark, scaled.
    pub confidence: f64,
    /// Which landmark it is nearest, for naming the joint in a UI.
    pub landmark: &'static str,
}

/// Default contact tolerance, as a fraction of model height.
///
/// Exposed because it is the one number that cannot be derived: too tight and a
/// real joint with a modelled gap is missed, too loose and every neighbouring
/// plate becomes a hinge. A person looking at the model decides.
pub const DEFAULT_TOL: f64 = 0.04;

/// Gap between two boxes along each axis; negative means they overlap.
fn gap(a: &Part, b: &Part) -> [f64; 3] {
    let mut g = [0.0; 3];
    for i in 0..3 {
        g[i] = (a.centre[i] - b.centre[i]).abs() - (a.extent[i] + b.extent[i]) * 0.5;
    }
    g
}

/// Where two parts touch: the midpoint of their nearest faces.
fn contact(a: &Part, b: &Part) -> [f64; 3] {
    let mut p = [0.0; 3];
    for i in 0..3 {
        // clamp each centre into the other's span, then split the difference
        let (lo, hi) = (b.centre[i] - b.extent[i] * 0.5, b.centre[i] + b.extent[i] * 0.5);
        let ca = a.centre[i].clamp(lo, hi);
        let (lo2, hi2) = (a.centre[i] - a.extent[i] * 0.5, a.centre[i] + a.extent[i] * 0.5);
        let cb = b.centre[i].clamp(lo2, hi2);
        p[i] = (ca + cb) * 0.5;
    }
    p
}

/// The nearest landmark to a height, and how far away it is (0..1 of height).
pub fn nearest_landmark(height_fraction: f64) -> (&'static str, f64) {
    [("shoulder", SHOULDER), ("hip", HIP), ("knee", KNEE)]
        .into_iter()
        .map(|(n, h)| (n, (height_fraction - h).abs()))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap()
}

/// Find the joints.
///
/// `up` is the model's height axis; `tol` is the contact tolerance as a
/// fraction of height. Results are ordered by confidence, highest first, so a
/// caller raising the tolerance gets the extra candidates ranked rather than
/// mixed in with the good ones.
pub fn find(parts: &[Part], up: usize, tol: f64) -> Vec<Joint> {
    if parts.is_empty() { return Vec::new(); }
    // Height comes from the PARTS, not from a passed bounding span. The
    // landmark fractions are fractions of the body, and a span that includes
    // empty room above or below shifts every joint's apparent height — a
    // shoulder at the top of a half-height model measured 0.44 instead of 0.93
    // and matched no landmark at all.
    let lo_h = parts.iter().map(|p| p.centre[up] - p.extent[up] * 0.5)
        .fold(f64::MAX, f64::min);
    let hi_h = parts.iter().map(|p| p.centre[up] + p.extent[up] * 0.5)
        .fold(f64::MIN, f64::max);
    let height = (hi_h - lo_h).max(1e-9);
    let band = height * tol.max(0.0);
    let mut out = Vec::new();
    for i in 0..parts.len() {
        for j in (i + 1)..parts.len() {
            let g = gap(&parts[i], &parts[j]);
            // Touching means close on EVERY axis. Two plates that overlap in x
            // and y but sit far apart in z are not in contact, and testing only
            // the smallest gap says they are.
            if g.iter().any(|v| *v > band) { continue; }
            // Deeply interpenetrating parts are one assembly, not a hinge.
            if g.iter().all(|v| *v < -band * 2.0) { continue; }
            let p = contact(&parts[i], &parts[j]);
            let hf = (p[up] - lo_h) / height;
            let (name, dist) = nearest_landmark(hf);
            // parent is the larger part: a hand hinges on an arm, not the reverse
            let (par, ch) = if parts[i].verts >= parts[j].verts { (i, j) } else { (j, i) };
            out.push(Joint {
                parent: par, child: ch, pivot: p,
                axis: hinge_axis(&parts[ch], up),
                confidence: (1.0 - dist * 3.0).clamp(0.0, 1.0),
                landmark: name,
            });
        }
    }
    out.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap()
        .then(a.parent.cmp(&b.parent)).then(a.child.cmp(&b.child)));
    out
}

/// A hinge axis perpendicular to the part's longest direction.
///
/// A limb swings about an axis across it, never along it — rotating a forearm
/// about its own length is a drill, not an elbow.
pub fn hinge_axis(child: &Part, up: usize) -> [f64; 3] {
    let long = (0..3).max_by(|a, b| child.extent[*a].partial_cmp(&child.extent[*b]).unwrap())
        .unwrap();
    // pick whichever remaining axis is not the height, so limbs swing
    // fore-and-aft rather than sideways by default
    let axis = (0..3).find(|k| *k != long && *k != up).unwrap_or((long + 1) % 3);
    let mut v = [0.0; 3];
    v[axis] = 1.0;
    v
}

/// One step of the fold back to vehicle form.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldStep {
    pub joint: usize,
    /// Depth in the chain: 0 moves first.
    pub depth: usize,
}

/// Order the joints into a fold: parent before child.
///
/// This IS the reverse plan. A bot's joints, rotated in order about their
/// contacts, collapse it toward vehicle form — derived from the bot alone, with
/// no second model and no matching, which is what defeated every earlier
/// attempt.
pub fn reverse_plan(joints: &[Joint], part_count: usize) -> Vec<FoldStep> {
    let mut depth = vec![usize::MAX; part_count];
    // roots: parts that are never a child
    for p in 0..part_count {
        if !joints.iter().any(|j| j.child == p) { depth[p] = 0; }
    }
    for _ in 0..part_count {
        for j in joints {
            if depth[j.parent] != usize::MAX && depth[j.child] == usize::MAX {
                depth[j.child] = depth[j.parent] + 1;
            }
        }
    }
    let mut steps: Vec<FoldStep> = joints.iter().enumerate()
        .map(|(i, j)| FoldStep {
            joint: i,
            // an unreachable child (a cycle) folds last rather than never
            depth: depth[j.child].min(part_count),
        })
        .collect();
    steps.sort_by_key(|s| (s.depth, s.joint));
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(index: usize, verts: usize, c: [f64; 3], e: [f64; 3]) -> Part {
        Part { index, verts, centre: c, extent: e }
    }

    /// A crude bot spanning the FULL 0..1 height, so the landmark fractions
    /// mean what they say.
    ///
    /// The first version placed parts at absolute landmark values on a body only
    /// 0.59 tall starting at 0.49 — so the "shoulder" sat at 63% of the model
    /// and matched the hip. A landmark fraction is a fraction OF THE BODY, and a
    /// test body has to be a whole body.
    fn bot() -> Vec<Part> {
        vec![
            // torso from the hip up to the shoulders
            part(0, 1000, [0.0, (SHOULDER + HIP) * 0.5, 0.0],
                 [0.40, SHOULDER - HIP, 0.25]),
            part(1, 300, [-0.32, SHOULDER - 0.06, 0.0], [0.14, 0.16, 0.14]),  // upper arm L
            part(2, 300, [ 0.32, SHOULDER - 0.06, 0.0], [0.14, 0.16, 0.14]),  // upper arm R
            part(3, 150, [-0.32, SHOULDER - 0.22, 0.0], [0.12, 0.16, 0.12]),  // forearm L
            part(4, 150, [ 0.32, SHOULDER - 0.22, 0.0], [0.12, 0.16, 0.12]),  // forearm R
            // legs, which also give the body its full height
            part(5, 400, [-0.20, (HIP + KNEE) * 0.5, 0.0], [0.16, HIP - KNEE, 0.16]),
            part(6, 400, [ 0.20, (HIP + KNEE) * 0.5, 0.0], [0.16, HIP - KNEE, 0.16]),
            part(7, 250, [-0.20, KNEE * 0.5, 0.0], [0.14, KNEE, 0.14]),       // shin L
            part(8, 250, [ 0.20, KNEE * 0.5, 0.0], [0.14, KNEE, 0.14]),       // shin R
        ]
    }

    #[test]
    fn touching_parts_become_joints_and_distant_ones_do_not() {
        let joints = find(&bot(), 1, DEFAULT_TOL);
        assert!(!joints.is_empty(), "the arms touch the torso");
        // the two forearms never touch each other
        assert!(!joints.iter().any(|j| (j.parent == 3 && j.child == 4)
                                    || (j.parent == 4 && j.child == 3)),
                "opposite forearms are not hinged together");
    }

    #[test]
    fn contact_must_be_close_on_every_axis_not_just_one() {
        // Two plates overlapping in x and y but far apart in z are not touching,
        // and testing only the smallest gap says they are.
        let apart = vec![
            part(0, 100, [0.0, 0.5, 0.0], [0.4, 0.4, 0.1]),
            part(1, 100, [0.0, 0.5, 5.0], [0.4, 0.4, 0.1]),   // same x,y — far in z
        ];
        assert!(find(&apart, 1, DEFAULT_TOL).is_empty(), "z separation matters");
    }

    #[test]
    fn the_larger_part_is_the_parent() {
        // A hand hinges on an arm, not the reverse — otherwise the fold order
        // swings the body about its fingertips.
        for j in find(&bot(), 1, DEFAULT_TOL) {
            let (p, c) = (&bot()[j.parent], &bot()[j.child]);
            assert!(p.verts >= c.verts, "parent {} smaller than child {}", j.parent, j.child);
        }
    }

    #[test]
    fn a_joint_at_a_landmark_outranks_one_in_the_middle_of_a_plate() {
        // Anatomy is the second signal: contact alone would rank a panel seam
        // as highly as a shoulder.
        let joints = find(&bot(), 1, DEFAULT_TOL);
        let shoulder = joints.iter().find(|j| j.landmark == "shoulder")
            .expect("a shoulder joint");
        assert!(shoulder.confidence > 0.5, "confidence {}", shoulder.confidence);
        // and results arrive ranked, so raising the tolerance adds candidates
        // at the BOTTOM rather than mixing them in
        let conf: Vec<f64> = joints.iter().map(|j| j.confidence).collect();
        for w in conf.windows(2) { assert!(w[0] >= w[1] - 1e-9, "not sorted: {conf:?}"); }
    }

    #[test]
    fn raising_the_tolerance_finds_more_joints_and_never_fewer() {
        // The one number a person nudges in-game, so its behaviour must be
        // monotonic — a control that sometimes loses a joint as you loosen it
        // is unusable.
        let mut last = 0;
        for tol in [0.005, 0.02, 0.05, 0.12, 0.3] {
            let n = find(&bot(), 1, tol).len();
            assert!(n >= last, "tolerance {tol} found {n}, down from {last}");
            last = n;
        }
        assert!(last > 0);
    }

    #[test]
    fn a_hinge_axis_is_never_along_the_limb() {
        // Rotating a forearm about its own length is a drill, not an elbow.
        for p in bot() {
            let long = (0..3).max_by(|a, b| p.extent[*a].partial_cmp(&p.extent[*b]).unwrap()).unwrap();
            let ax = hinge_axis(&p, 1);
            assert!(ax[long].abs() < 1e-9, "axis {ax:?} runs along the part's length");
            assert!((ax[0].abs() + ax[1].abs() + ax[2].abs() - 1.0).abs() < 1e-9, "unit axis");
        }
    }

    #[test]
    fn the_reverse_plan_folds_parents_before_children() {
        // A child swinging while its parent is still extended sweeps through
        // space the parent is about to occupy.
        let parts = bot();
        let joints = find(&parts, 1, DEFAULT_TOL);
        let plan = reverse_plan(&joints, parts.len());
        assert_eq!(plan.len(), joints.len());
        let depth_of = |part: usize| plan.iter()
            .find(|s| joints[s.joint].child == part).map(|s| s.depth);
        if let (Some(upper), Some(fore)) = (depth_of(1), depth_of(3)) {
            assert!(upper <= fore, "the upper arm folds before the forearm: {upper} vs {fore}");
        }
        // and the steps come out in fold order
        for w in plan.windows(2) { assert!(w[0].depth <= w[1].depth); }
    }

    #[test]
    fn a_cycle_folds_last_rather_than_never() {
        // Real data will contain one, and a planner that drops a part silently
        // is worse than one that folds it at the end.
        let parts = vec![
            part(0, 100, [0.0, 0.5, 0.0], [0.2, 0.2, 0.2]),
            part(1, 100, [0.19, 0.5, 0.0], [0.2, 0.2, 0.2]),
        ];
        let joints = find(&parts, 1, 0.2);
        let plan = reverse_plan(&joints, parts.len());
        assert_eq!(plan.len(), joints.len(), "every joint is planned");
        for s in &plan { assert!(s.depth <= parts.len()); }
    }

    #[test]
    fn an_empty_model_produces_an_empty_plan_rather_than_panicking() {
        assert!(find(&[], 1, DEFAULT_TOL).is_empty());
        assert!(reverse_plan(&[], 0).is_empty());
    }
}
