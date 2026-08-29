//! THE BODY'S ACTUAL MASS, and the debris around it.
//!
//! Three symptoms with one cause. A pose arrives with a few prims flung
//! far from the body — a mis-parented hand, an exploded panel, a mesh
//! whose transform never got applied — and every measurement downstream
//! is taken with `lo.min()` / `hi.max()` over EVERY vertex:
//!
//! * `model::bounds_from_extent` — the hitbox, the SDF box, and `ride_h`,
//!   the distance from the origin to the lowest point. One panel hanging
//!   below the feet and the body rides on nothing; one flung sideways and
//!   the hitbox is twice the chassis. Both read as the body being in the
//!   wrong place relative to the floor.
//! * `xform::Morph::bounds_at` — the same computation again, live during
//!   a fold, so a blend can hand physics a ride height that jumps frame
//!   to frame as debris interpolates.
//! * `orient::find_orientation` — scores candidate frames over all
//!   vertices, so debris drags the up/forward guess with it and the pose
//!   arrives lying down or facing backwards.
//!
//! So: find the mass first, and let everything measure THAT.
//!
//! ## The method, and why this one
//!
//! A body's real geometry is the big connected lump near the middle;
//! debris is small and far. Both halves of that matter — "far" alone
//! would delete an outstretched arm, which is exactly what a robot pose
//! is full of, and "small" alone would delete bolts and vents.
//!
//! The centre is a MEDIAN, per axis, over prim centroids weighted by
//! vertex count — not a mean, because a mean is what debris moves and the
//! whole problem is debris moving things. The scale is the median
//! distance from that centre: a robust spread that a handful of outliers
//! cannot inflate, the same reason a MAD beats a standard deviation.
//!
//! A prim is debris when it is BOTH beyond `FAR` times that spread AND
//! carries less than `SLIGHT` of the body's vertices. A big far thing is
//! a limb and stays; a small near thing is detail and stays; only small
//! AND far goes.
//!
//! ## Honesty
//!
//! `FAR` and `SLIGHT` are reasoned from the shape of the problem, not
//! fitted against a labelled set of broken exports — this repo has no
//! such set. They are deliberately conservative: the failure mode of a
//! too-loose threshold is the status quo (debris survives, bounds are
//! wrong), and the failure mode of a too-tight one is deleting real
//! geometry, which is worse and harder to notice. `report` exists so a
//! driver can print what was dropped rather than have it happen silently.

use glam::Vec3;

/// How many robust spreads out a prim must sit before it can be debris.
/// A robot's outstretched arm reaches perhaps 2x the median radius; this
/// leaves that alone.
pub const FAR: f32 = 3.2;
/// The largest share of a body's vertices a prim can hold and still be
/// called debris. Above this it is structure, however far out it sits.
pub const SLIGHT: f32 = 0.06;

/// What the filter found.
#[derive(Clone, Debug)]
pub struct Mass {
    /// keep flag per prim, in the order they were handed over
    pub keep: Vec<bool>,
    /// the robust centre — a per-axis median of prim centroids
    pub centre: Vec3,
    /// the robust spread: median distance from `centre`
    pub spread: f32,
    /// how many prims were called debris
    pub dropped: usize,
}

impl Mass {
    /// One line for a driver, because a filter that silently eats geometry
    /// is worse than no filter.
    pub fn report(&self) -> String {
        format!(
            "mass: centre {:.2},{:.2},{:.2} spread {:.2}, {} of {} prims dropped as debris",
            self.centre.x,
            self.centre.y,
            self.centre.z,
            self.spread,
            self.dropped,
            self.keep.len()
        )
    }
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) * 0.5
    }
}

/// Classify prims into mass and debris. `prims` is each prim's points.
pub fn classify(prims: &[&[Vec3]]) -> Mass {
    let n = prims.len();
    let mut keep = vec![true; n];
    if n < 2 {
        // one prim is the body by definition: nothing to reject against
        return Mass { keep, centre: Vec3::ZERO, spread: 0.0, dropped: 0 };
    }
    let total: usize = prims.iter().map(|p| p.len()).sum();
    if total == 0 {
        return Mass { keep, centre: Vec3::ZERO, spread: 0.0, dropped: 0 };
    }
    // per-prim centroids, and a per-axis median over them WEIGHTED by
    // vertex count — a thousand-vertex torso should move the centre more
    // than a six-vertex bracket, and an unweighted median of centroids
    // treats them as equals
    let mut cents: Vec<(Vec3, usize)> = Vec::with_capacity(n);
    for p in prims {
        if p.is_empty() {
            cents.push((Vec3::ZERO, 0));
            continue;
        }
        let c = p.iter().copied().sum::<Vec3>() / p.len() as f32;
        cents.push((c, p.len()));
    }
    let axis_median = |pick: &dyn Fn(&Vec3) -> f32| -> f32 {
        let mut xs: Vec<f32> = Vec::with_capacity(total.min(4096));
        for (c, w) in &cents {
            // weight by repeating, capped so one huge prim cannot make the
            // vector enormous
            let reps = (*w).min(512).max(1) / 8 + 1;
            for _ in 0..reps {
                xs.push(pick(c));
            }
        }
        median(&mut xs)
    };
    let centre = Vec3::new(
        axis_median(&|v: &Vec3| v.x),
        axis_median(&|v: &Vec3| v.y),
        axis_median(&|v: &Vec3| v.z),
    );
    // WEIGHTED, like the centre above, and for the same reason. An
    // unweighted median of per-prim distances lets an outlier define the
    // scale it is then measured against: with a 300-point hull at
    // distance 0 and a 6-vertex panel at distance 18, the median of
    // [0, 18] is 18, so `FAR * spread` is 57.6 and the panel — the whole
    // reason this filter exists — is not far from anything. Weighting by
    // vertex count puts the median where the mass is, which is what
    // "spread" was always supposed to mean.
    let mut dists: Vec<f32> = Vec::with_capacity(total.min(4096));
    for (c, w) in &cents {
        if *w == 0 {
            continue;
        }
        let d = (*c - centre).length();
        let reps = (*w).min(512).max(1) / 8 + 1;
        for _ in 0..reps {
            dists.push(d);
        }
    }
    let spread = median(&mut dists).max(1e-4);

    let mut dropped = 0;
    for (i, (c, w)) in cents.iter().enumerate() {
        if *w == 0 {
            continue;
        }
        let far = (*c - centre).length() > spread * FAR;
        let slight = (*w as f32) / (total as f32) < SLIGHT;
        if far && slight {
            keep[i] = false;
            dropped += 1;
        }
    }
    // NEVER EAT THE BODY. If the filter wants to drop most of what it was
    // handed, the assumption behind it is wrong for this asset — a pose
    // built from many small pieces has no single lump — and dropping is
    // then far worse than the bad bounds it was fixing.
    //
    // Counted in POINTS, not prims. Prim count is the wrong measure of
    // "most of the body": a 300-point hull and a 6-vertex flung panel are
    // one prim each, so by count dropping the panel is dropping half the
    // asset and this guard fired and put the panel back — which is
    // exactly the case the filter exists for, and why it appeared to do
    // nothing on a two-prim model. By mass the panel is 2%, which is what
    // "most" should mean. This also removes the need for a separate
    // small-`n` bail: the guard now scales correctly at any prim count,
    // so only a single prim (nothing to compare against) short-circuits.
    let dropped_pts: usize = cents
        .iter()
        .enumerate()
        .filter(|(i, _)| !keep[*i])
        .map(|(_, (_, w))| *w)
        .sum();
    if dropped_pts * 3 > total {
        return Mass { keep: vec![true; n], centre, spread, dropped: 0 };
    }
    Mass { keep, centre, spread, dropped }
}

/// Bounds over the mass only: the same `(half, ride_h)` pair
/// `model::bounds_from_extent` and `xform::Morph::bounds_at` both return,
/// so the static load path and the live morph path measure identically.
/// `ride_h` is the drop from the origin to the lowest point of the MASS —
/// which is the number physics seats the body on.
pub fn bounds(prims: &[&[Vec3]], keep: &[bool]) -> Option<(Vec3, f32)> {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut any = false;
    for (i, p) in prims.iter().enumerate() {
        if !keep.get(i).copied().unwrap_or(true) {
            continue;
        }
        for v in p.iter() {
            lo = lo.min(*v);
            hi = hi.max(*v);
            any = true;
        }
    }
    if !any {
        return None;
    }
    Some(((hi - lo) * 0.5, -lo.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(centre: Vec3, r: f32, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let t = i as f32 * 0.7;
                centre + Vec3::new(t.sin(), (t * 1.3).cos(), (t * 0.6).sin()) * r
            })
            .collect()
    }

    #[test]
    fn debris_is_dropped_and_the_body_is_not() {
        let torso = blob(Vec3::ZERO, 1.0, 400);
        let arm_l = blob(Vec3::new(-1.6, 0.4, 0.0), 0.5, 120);
        let arm_r = blob(Vec3::new(1.6, 0.4, 0.0), 0.5, 120);
        let leg = blob(Vec3::new(0.0, -1.4, 0.0), 0.5, 150);
        // the fractured panel: small, and flung far
        let junk = blob(Vec3::new(0.0, -14.0, 9.0), 0.2, 8);
        let prims: Vec<&[Vec3]> = vec![&torso, &arm_l, &arm_r, &leg, &junk];
        let m = classify(&prims);
        assert!(!m.keep[4], "the debris survived: {}", m.report());
        for i in 0..4 {
            assert!(m.keep[i], "prim {i} was eaten as debris: {}", m.report());
        }
    }

    #[test]
    fn the_debris_was_what_broke_the_ride_height() {
        // this is the whole point: `ride_h` is -lo.y, so one panel hanging
        // below the feet makes the body ride on nothing
        let torso = blob(Vec3::ZERO, 1.0, 400);
        let leg = blob(Vec3::new(0.0, -1.4, 0.0), 0.5, 150);
        let junk = blob(Vec3::new(0.0, -14.0, 0.0), 0.2, 8);
        let prims: Vec<&[Vec3]> = vec![&torso, &leg, &junk];
        let m = classify(&prims);
        let all_true = vec![true; prims.len()];
        let (_, naive) = bounds(&prims, &all_true).unwrap();
        let (half, ride) = bounds(&prims, &m.keep).unwrap();
        assert!(naive > 12.0, "the fixture does not reproduce the bug: {naive}");
        assert!(ride < 2.5, "the filtered ride height is still debris-driven: {ride}");
        assert!(half.y < 2.5, "the hitbox is still debris-sized: {half:?}");
    }

    #[test]
    fn a_long_limb_is_structure_however_far_out_it_reaches() {
        // the failure mode that would be worse than the bug: deleting a
        // real outstretched arm because it is far from the middle
        let torso = blob(Vec3::ZERO, 1.0, 300);
        let hip = blob(Vec3::new(0.0, -1.0, 0.0), 0.6, 200);
        let arm = blob(Vec3::new(4.5, 0.0, 0.0), 0.6, 260); // far AND big
        let prims: Vec<&[Vec3]> = vec![&torso, &hip, &arm];
        let m = classify(&prims);
        assert!(m.keep[2], "an outstretched arm was called debris: {}", m.report());
    }

    #[test]
    fn a_body_of_many_small_pieces_is_left_alone() {
        // no single lump: the assumption behind the filter does not hold,
        // and eating most of the model is worse than bad bounds
        let mut owned: Vec<Vec3> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for i in 0..40 {
            let a = i as f32 * 0.9;
            let start = owned.len();
            owned.extend(blob(Vec3::new(a.cos() * 6.0, a.sin() * 6.0, 0.0), 0.2, 6));
            spans.push((start, owned.len()));
        }
        let prims: Vec<&[Vec3]> = spans.iter().map(|(a, b)| &owned[*a..*b]).collect();
        let m = classify(&prims);
        assert_eq!(m.dropped, 0, "the filter ate a legitimately scattered body");
    }

    #[test]
    fn a_two_part_body_is_never_split() {
        // The real thing worth protecting at two prims: a body that IS
        // two comparable pieces — a chassis and a trailer, a torso and a
        // cockpit — must survive whole however far apart they sit,
        // because neither one is debris and dropping either is dropping
        // half the asset.
        let a = blob(Vec3::ZERO, 1.0, 100);
        let b = blob(Vec3::new(30.0, 0.0, 0.0), 1.0, 80);
        let m = classify(&[&a, &b]);
        assert_eq!(m.dropped, 0, "a two-part body was split: {} dropped", m.dropped);
    }

    #[test]
    fn overwhelming_mass_asymmetry_still_rejects_debris_at_two_prims() {
        // This test replaces `two_prims_are_never_second_guessed`, which
        // asserted that a 4-vertex speck 30 units from a 100-vertex body
        // must be KEPT, on the reasoning that "with two prims there is no
        // majority to appeal to". That reasoning does not survive its own
        // commit: `classify` weights everything by vertex count precisely
        // so a large prim outvotes a small one, and 100 against 4 is that
        // majority. The two tests were mutually unsatisfiable — a flung
        // panel beside one body prim is the exact case the filter was
        // written for (physics seating a chassis on debris), and it could
        // not be fixed while the other test stood.
        let body = blob(Vec3::ZERO, 1.0, 100);
        let speck = blob(Vec3::new(30.0, 0.0, 0.0), 0.1, 4);
        let m = classify(&[&body, &speck]);
        assert_eq!(m.dropped, 1, "the speck is both far and slight");
        assert!(m.keep[0] && !m.keep[1], "the BODY survives, the speck does not");
    }
}
