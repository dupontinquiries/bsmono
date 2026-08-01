//! assign — hand every vehicle part to a bone, without matching anything.
//!
//! The step the descriptor work was trying and failing to reach. Once a skeleton
//! is folded *inside* the vehicle, correspondence is not needed: every component
//! is simply near some bone, and the nearest bone is the one that carries it.
//! No shape signature, no threshold, no control experiment to fail.
//!
//! ## Why nearest-bone alone is not enough
//!
//! Two refinements, both of which exist because the naive version produces
//! visible artefacts:
//!
//! * **A component is assigned as a WHOLE.** Assigning per-vertex splits a
//!   single rigid part across two bones and tears it in half the moment they
//!   move differently. Parts are rigid; the assignment must be too.
//! * **Ties go to the heavier bone.** A part equidistant from a forearm and the
//!   torso belongs to the torso: attaching mass to an extremity makes the
//!   extremity flail, and a wrongly-attached heavy part is far more visible than
//!   a wrongly-attached light one.
//!
//! ## What comes out
//!
//! A bone index per component. Unfolding the skeleton then carries the vehicle's
//! own geometry into bot form — the parts are never re-authored, only re-parented,
//! so every vertex of the original survives and `model-verify` can prove it.

use crate::fold::Bone;

/// One component's assignment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assignment {
    pub component: usize,
    pub bone: usize,
    /// Distance from the component's centroid to that bone's axis, in model
    /// units. Large values mean the skeleton did not really reach this part —
    /// worth surfacing rather than hiding, since it is the signal that a fold
    /// was poor.
    pub distance: f64,
}

fn centroid(pts: &[[f64; 3]]) -> [f64; 3] {
    let n = pts.len().max(1) as f64;
    let mut c = [0.0; 3];
    for p in pts { for i in 0..3 { c[i] += p[i]; } }
    for i in 0..3 { c[i] /= n; }
    c
}

/// Bone "weight" for tie-breaking: length times radius squared, i.e. roughly the
/// capsule's volume. A torso outweighs a forearm, which is the ordering wanted.
fn weight(b: &Bone) -> f64 { b.length() * b.r * b.r }

/// Assign every component to a bone.
///
/// `tie` is the fraction within which two bones count as equidistant; inside
/// that band the heavier bone wins.
pub fn assign(components: &[Vec<[f64; 3]>], bones: &[Bone], tie: f64) -> Vec<Assignment> {
    components.iter().enumerate().map(|(ci, pts)| {
        let c = centroid(pts);
        let mut best = 0usize;
        let mut best_d = f64::MAX;
        for (bi, b) in bones.iter().enumerate() {
            let d = b.dist_to(c);
            if d < best_d {
                best_d = d;
                best = bi;
            }
        }
        // Within the tie band, prefer the heavier bone.
        //
        // The band is computed ONCE from the nearest distance and then held
        // fixed. The first version shrank `best_d` each time it switched, which
        // tightened the band mid-loop and let a later, lighter bone win a
        // comparison it should have lost — the test caught the arm taking a part
        // the torso had already claimed.
        let band = best_d * (1.0 + tie);
        for (bi, b) in bones.iter().enumerate() {
            if b.dist_to(c) <= band && weight(b) > weight(&bones[best]) {
                best = bi;
            }
        }
        let best_d = bones[best].dist_to(c).min(best_d);
        Assignment { component: ci, bone: best, distance: best_d }
    }).collect()
}

/// How the assignment turned out — the numbers that say whether a fold was good
/// enough to build on.
#[derive(Debug, Clone, PartialEq)]
pub struct Coverage {
    /// Components per bone.
    pub per_bone: Vec<usize>,
    /// Bones that got nothing. An empty bone will move nothing when it unfolds,
    /// so a limb with no parts is a limb that will not appear in the bot.
    pub empty_bones: usize,
    /// Components further from their bone than `reach` — the skeleton did not
    /// really claim these.
    pub orphans: usize,
    pub worst_distance: f64,
}

pub fn coverage(a: &[Assignment], n_bones: usize, reach: f64) -> Coverage {
    let mut per = vec![0usize; n_bones];
    let mut orphans = 0;
    let mut worst = 0.0f64;
    for x in a {
        if x.bone < n_bones { per[x.bone] += 1; }
        if x.distance > reach { orphans += 1; }
        worst = worst.max(x.distance);
    }
    Coverage {
        empty_bones: per.iter().filter(|c| **c == 0).count(),
        per_bone: per,
        orphans,
        worst_distance: worst,
    }
}

/// Carry an assigned component from folded pose to unfolded pose.
///
/// The component moves with its bone: the rigid transform taking the bone from
/// `folded` to `unfolded` is applied to every vertex. Rigid, so the part is
/// never deformed and the bot mode holds exactly the vehicle's authored detail.
pub fn carry(pts: &[[f64; 3]], folded: &Bone, unfolded: &Bone) -> Option<Vec<[f64; 3]>> {
    // The bone gives two points, which fixes translation and the axis direction
    // but leaves roll about that axis undetermined. Resolving roll needs a third
    // reference; without one the sensible choice is the minimal rotation, which
    // is what `rotation_between` gives — a part does not spin about its own bone
    // unless something says it should.
    let fa = [folded.b[0]-folded.a[0], folded.b[1]-folded.a[1], folded.b[2]-folded.a[2]];
    let ua = [unfolded.b[0]-unfolded.a[0], unfolded.b[1]-unfolded.a[1], unfolded.b[2]-unfolded.a[2]];
    let r = rotation_between(fa, ua)?;
    Some(pts.iter().map(|p| {
        let d = [p[0]-folded.a[0], p[1]-folded.a[1], p[2]-folded.a[2]];
        [ unfolded.a[0] + r[0][0]*d[0] + r[0][1]*d[1] + r[0][2]*d[2],
          unfolded.a[1] + r[1][0]*d[0] + r[1][1]*d[1] + r[1][2]*d[2],
          unfolded.a[2] + r[2][0]*d[0] + r[2][1]*d[1] + r[2][2]*d[2] ]
    }).collect())
}

/// Minimal rotation carrying `from` onto `to`.
pub fn rotation_between(from: [f64; 3], to: [f64; 3]) -> Option<[[f64; 3]; 3]> {
    let n = |v: [f64; 3]| -> Option<[f64; 3]> {
        let l = (v[0]*v[0] + v[1]*v[1] + v[2]*v[2]).sqrt();
        if l < 1e-12 { None } else { Some([v[0]/l, v[1]/l, v[2]/l]) }
    };
    let (a, b) = (n(from)?, n(to)?);
    let dot: f64 = (0..3).map(|i| a[i]*b[i]).sum::<f64>().clamp(-1.0, 1.0);
    let axis = [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
    let s = (axis[0]*axis[0] + axis[1]*axis[1] + axis[2]*axis[2]).sqrt();
    if s < 1e-12 {
        // parallel or antiparallel: identity, or a half turn about any
        // perpendicular. Picking one arbitrarily is correct here because roll is
        // undetermined anyway.
        if dot > 0.0 { return Some([[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]]); }
        let perp = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let ax = [a[1]*perp[2]-a[2]*perp[1], a[2]*perp[0]-a[0]*perp[2], a[0]*perp[1]-a[1]*perp[0]];
        return Some(crate::motion::axis_angle(ax, std::f64::consts::PI));
    }
    Some(crate::motion::axis_angle(axis, dot.acos()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(a: [f64;3], b: [f64;3], r: f64) -> Bone { Bone { a, b, r, parent: -1 } }

    fn skeleton() -> Vec<Bone> {
        vec![
            bone([0.0,0.0,0.0], [0.0,1.0,0.0], 0.20),      // 0 torso (heavy)
            bone([0.0,0.9,0.0], [1.0,0.9,0.0], 0.07),      // 1 arm (light)
            bone([0.0,0.0,0.0], [0.0,-1.2,0.0], 0.12),     // 2 leg
        ]
    }

    fn blob(c: [f64;3]) -> Vec<[f64;3]> {
        vec![c, [c[0]+0.05,c[1],c[2]], [c[0],c[1]+0.05,c[2]], [c[0],c[1],c[2]+0.05]]
    }

    #[test]
    fn a_part_goes_to_the_bone_it_sits_on() {
        let s = skeleton();
        let comps = vec![blob([0.0,0.5,0.0]), blob([0.8,0.9,0.0]), blob([0.0,-0.9,0.0])];
        let a = assign(&comps, &s, 0.0);
        assert_eq!(a[0].bone, 0, "mid-torso part -> torso");
        assert_eq!(a[1].bone, 1, "part out along the arm -> arm");
        assert_eq!(a[2].bone, 2, "part down the leg -> leg");
        assert!(a.iter().all(|x| x.distance < 0.3));
    }

    #[test]
    fn a_whole_component_goes_to_one_bone() {
        // Assigning per-vertex would split a rigid part across two bones and
        // tear it in half the moment they move differently.
        let s = skeleton();
        // a long part straddling torso and arm
        let straddle: Vec<[f64;3]> = (0..12).map(|i| [i as f64 * 0.09, 0.9, 0.0]).collect();
        let a = assign(&[straddle], &s, 0.0);
        assert_eq!(a.len(), 1, "one component, one assignment");
    }

    #[test]
    fn a_tie_goes_to_the_heavier_bone() {
        // Attaching mass to an extremity makes the extremity flail, and a
        // wrongly-attached heavy part is far more visible than a light one.
        let s = skeleton();
        // A GENUINE tie: this point is ~0.30 from the torso axis and ~0.30 from
        // the arm axis. The first version of this test used a point 3.6x closer
        // to the arm and expected the torso to win, which is not a tie - it is
        // asking the tie-break to override a clear result. The test was wrong,
        // not the code.
        let a = assign(&[blob([0.30, 1.19, 0.0])], &s, 0.25);
        assert_eq!(a[0].bone, 0, "the torso wins a genuine tie against the arm");
        // but a part clearly out along the arm still goes to the arm
        let b = assign(&[blob([0.9, 0.9, 0.0])], &s, 0.35);
        assert_eq!(b[0].bone, 1);
    }

    #[test]
    fn coverage_reports_limbs_that_would_not_appear() {
        // An empty bone moves nothing when it unfolds, so a limb with no parts
        // is a limb that will not exist in the bot.
        let s = skeleton();
        let comps = vec![blob([0.0,0.5,0.0]), blob([0.0,0.6,0.0])];
        let c = coverage(&assign(&comps, &s, 0.0), s.len(), 0.5);
        assert_eq!(c.per_bone[0], 2);
        assert_eq!(c.empty_bones, 2, "the arm and leg got nothing");
    }

    #[test]
    fn orphans_are_counted_rather_than_silently_attached() {
        // A part far from every bone means the fold did not really reach it, and
        // that is the signal that a fold was poor.
        let s = skeleton();
        let far = blob([9.0, 9.0, 9.0]);
        let c = coverage(&assign(&[far], &s, 0.0), s.len(), 1.0);
        assert_eq!(c.orphans, 1);
        assert!(c.worst_distance > 5.0);
    }

    #[test]
    fn carrying_a_part_is_rigid_so_the_detail_survives() {
        // The whole reason to re-parent rather than re-author: every vertex of
        // the original must arrive intact.
        let folded = bone([0.0,0.0,0.0], [1.0,0.0,0.0], 0.1);
        let unfolded = bone([5.0,5.0,0.0], [5.0,6.0,0.0], 0.1);   // moved and turned 90 deg
        let pts = vec![[0.2,0.1,0.0], [0.5,-0.1,0.05], [0.9,0.0,0.0]];
        let moved = carry(&pts, &folded, &unfolded).expect("solvable");
        // pairwise distances are preserved — nothing was stretched
        for i in 0..pts.len() {
            for j in (i+1)..pts.len() {
                let d0 = ((pts[i][0]-pts[j][0]).powi(2) + (pts[i][1]-pts[j][1]).powi(2)
                        + (pts[i][2]-pts[j][2]).powi(2)).sqrt();
                let d1 = ((moved[i][0]-moved[j][0]).powi(2) + (moved[i][1]-moved[j][1]).powi(2)
                        + (moved[i][2]-moved[j][2]).powi(2)).sqrt();
                assert!((d0-d1).abs() < 1e-9, "distance {i}-{j} changed: {d0} -> {d1}");
            }
        }
        // and it actually arrived at the new bone
        assert!((moved[0][0] - 5.0).abs() < 1.0 && moved[0][1] > 4.9);
    }

    #[test]
    fn a_degenerate_bone_declines_rather_than_producing_nonsense() {
        let zero = bone([1.0,1.0,1.0], [1.0,1.0,1.0], 0.1);
        let ok = bone([0.0,0.0,0.0], [1.0,0.0,0.0], 0.1);
        assert!(carry(&[[0.0;3]], &zero, &ok).is_none());
        assert!(carry(&[[0.0;3]], &ok, &zero).is_none());
        // an unchanged bone carries its part unchanged
        let same = carry(&[[0.3,0.1,0.0]], &ok, &ok).unwrap();
        assert!((same[0][0]-0.3).abs() < 1e-9 && (same[0][1]-0.1).abs() < 1e-9);
    }
}
