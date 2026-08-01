//! fold — put the body in the box.
//!
//! ## Why this replaces part matching
//!
//! Matching parts between a vehicle and a bot form did not work: a control run
//! matched 87% of parts between *unrelated* tanks, so the descriptor was mostly
//! reporting that small greebles resemble each other. Constraining the search by
//! position would help — but it breaks the cases that matter, because a
//! long-legged bot folds its legs and a leg can end up anywhere inside the
//! vehicle.
//!
//! So: stop trying to find correspondences. **Fold a known body into the
//! vehicle's volume instead**, the way a contortionist packs into a box, and
//! take the fold that fits best. That needs no correspondence, works on vehicles
//! that have no bot form at all, and the fold path *is* the transformation —
//! played backwards, the body unpacks into a robot.
//!
//! Anatomy is the prior that makes it tractable. A human skeleton has ~20 joints
//! with real limits; the search space is joint angles, not arbitrary rigid
//! motions per part, and every candidate is a pose a body could actually hold.
//!
//! ## The error being minimised
//!
//! Two terms, and the balance between them is the whole problem:
//!
//! * **Spill** — body volume outside the vehicle. A leg poking through the hull
//!   is an invalid fold, so this is weighted hard.
//! * **Vacancy** — vehicle volume the body fails to occupy. A body curled in one
//!   corner spills nothing and explains nothing, which is the degenerate answer
//!   this term exists to reject.
//!
//! Without vacancy the optimiser shrinks the body into a ball; without spill it
//! sprawls. Reporting both separately keeps that visible rather than hiding it
//! in one score.

/// A bone: a capsule from `a` to `b` with radius `r`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bone {
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub r: f64,
    /// Index of the parent bone, or -1 for the root.
    pub parent: i32,
}

impl Bone {
    pub fn length(&self) -> f64 {
        ((self.b[0]-self.a[0]).powi(2) + (self.b[1]-self.a[1]).powi(2)
            + (self.b[2]-self.a[2]).powi(2)).sqrt()
    }
    /// Distance from a point to the capsule's axis segment.
    pub fn dist_to(&self, p: [f64; 3]) -> f64 {
        let ab = [self.b[0]-self.a[0], self.b[1]-self.a[1], self.b[2]-self.a[2]];
        let ap = [p[0]-self.a[0], p[1]-self.a[1], p[2]-self.a[2]];
        let d2: f64 = ab.iter().map(|x| x*x).sum();
        let t = if d2 < 1e-12 { 0.0 } else {
            (ap[0]*ab[0] + ap[1]*ab[1] + ap[2]*ab[2]) / d2
        }.clamp(0.0, 1.0);
        let q = [self.a[0]+ab[0]*t, self.a[1]+ab[1]*t, self.a[2]+ab[2]*t];
        ((p[0]-q[0]).powi(2) + (p[1]-q[1]).powi(2) + (p[2]-q[2]).powi(2)).sqrt()
    }
    pub fn contains(&self, p: [f64; 3]) -> bool { self.dist_to(p) <= self.r }
}

/// The vehicle's occupied volume, as a voxel grid.
#[derive(Debug, Clone)]
pub struct Volume {
    pub res: usize,
    pub lo: [f64; 3],
    pub span: [f64; 3],
    pub filled: Vec<bool>,
}

impl Volume {
    /// Occupancy from a point cloud — cheap, and enough: what matters is the
    /// SHAPE of the space available, not a watertight solid.
    pub fn from_points(pts: &[[f64; 3]], res: usize) -> Volume {
        let res = res.max(2);
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for p in pts { for i in 0..3 { lo[i] = lo[i].min(p[i]); hi[i] = hi[i].max(p[i]); } }
        let span = [ (hi[0]-lo[0]).max(1e-9), (hi[1]-lo[1]).max(1e-9), (hi[2]-lo[2]).max(1e-9) ];
        let mut filled = vec![false; res*res*res];
        for p in pts {
            let g: Vec<usize> = (0..3).map(|i| {
                (((p[i]-lo[i])/span[i]) * res as f64).min(res as f64 - 1.0).max(0.0) as usize
            }).collect();
            filled[(g[2]*res + g[1])*res + g[0]] = true;
        }
        Volume { res, lo, span, filled }
    }

    pub fn count(&self) -> usize { self.filled.iter().filter(|f| **f).count() }

    pub fn cell_centre(&self, k: usize) -> [f64; 3] {
        let r = self.res;
        let (x, y, z) = (k % r, (k / r) % r, k / (r*r));
        [ self.lo[0] + (x as f64 + 0.5)/r as f64 * self.span[0],
          self.lo[1] + (y as f64 + 0.5)/r as f64 * self.span[1],
          self.lo[2] + (z as f64 + 0.5)/r as f64 * self.span[2] ]
    }

    pub fn contains(&self, p: [f64; 3]) -> bool {
        let r = self.res;
        let g: Vec<i64> = (0..3).map(|i| (((p[i]-self.lo[i])/self.span[i]) * r as f64) as i64).collect();
        if g.iter().any(|&v| v < 0 || v >= r as i64) { return false; }
        self.filled[(g[2] as usize * r + g[1] as usize)*r + g[0] as usize]
    }
}

/// How well a posed body fits a volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    /// Body volume outside the vehicle, as a fraction of the body.
    pub spill: f64,
    /// Vehicle volume the body does not occupy, as a fraction of the vehicle.
    pub vacancy: f64,
}

impl Fit {
    /// Spill is weighted harder: a limb through the hull is invalid, whereas
    /// unfilled space is merely unconvincing.
    pub fn error(&self) -> f64 { self.spill * 3.0 + self.vacancy }
}

/// Sample the posed body against the volume.
///
/// Samples along each bone rather than voxelising the body: a skeleton is a
/// handful of capsules, and sampling them is far cheaper than filling a second
/// grid — which matters because the optimiser evaluates this thousands of times.
pub fn evaluate(bones: &[Bone], vol: &Volume, samples_per_bone: usize) -> Fit {
    let n = samples_per_bone.max(2);
    let mut inside = 0usize;
    let mut total = 0usize;
    // mark which vehicle cells the body reaches, for the vacancy term
    let mut reached = vec![false; vol.filled.len()];
    for b in bones {
        for s in 0..n {
            let t = s as f64 / (n-1) as f64;
            let p = [ b.a[0] + (b.b[0]-b.a[0])*t,
                      b.a[1] + (b.b[1]-b.a[1])*t,
                      b.a[2] + (b.b[2]-b.a[2])*t ];
            total += 1;
            if vol.contains(p) { inside += 1; }
            // claim nearby vehicle cells within the bone's radius
            for k in 0..vol.filled.len() {
                if !vol.filled[k] || reached[k] { continue; }
                if b.dist_to(vol.cell_centre(k)) <= b.r { reached[k] = true; }
            }
        }
    }
    let filled = vol.count().max(1);
    let claimed = reached.iter().filter(|r| **r).count();
    Fit {
        spill: if total == 0 { 1.0 } else { 1.0 - inside as f64 / total as f64 },
        vacancy: 1.0 - claimed as f64 / filled as f64,
    }
}

/// Rotate a bone and everything downstream of it about the bone's own origin —
/// the fold. A joint moves its whole chain, which is what makes this anatomy
/// rather than free-floating parts.
pub fn bend(bones: &mut [Bone], joint: usize, axis: [f64; 3], angle: f64) {
    if joint >= bones.len() { return; }
    let pivot = bones[joint].a;
    let r = crate::motion::axis_angle(axis, angle);
    let rot = |p: [f64; 3]| -> [f64; 3] {
        let d = [p[0]-pivot[0], p[1]-pivot[1], p[2]-pivot[2]];
        [ pivot[0] + r[0][0]*d[0] + r[0][1]*d[1] + r[0][2]*d[2],
          pivot[1] + r[1][0]*d[0] + r[1][1]*d[1] + r[1][2]*d[2],
          pivot[2] + r[2][0]*d[0] + r[2][1]*d[1] + r[2][2]*d[2] ]
    };
    // descendants: any bone whose parent chain reaches `joint`
    let n = bones.len();
    let mut affected = vec![false; n];
    affected[joint] = true;
    for _ in 0..n {
        for i in 0..n {
            let p = bones[i].parent;
            if p >= 0 && affected[p as usize] { affected[i] = true; }
        }
    }
    for i in 0..n {
        if affected[i] { bones[i].a = rot(bones[i].a); bones[i].b = rot(bones[i].b); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(a: [f64;3], b: [f64;3], r: f64, parent: i32) -> Bone { Bone { a, b, r, parent } }

    /// A crude biped: spine, two arms, two legs.
    fn body() -> Vec<Bone> {
        vec![
            bone([0.0,0.0,0.0], [0.0,1.0,0.0], 0.18, -1),   // 0 spine
            bone([0.0,1.0,0.0], [0.0,1.4,0.0], 0.14, 0),    // 1 head
            bone([0.0,0.9,0.0], [-0.7,0.9,0.0], 0.10, 0),   // 2 arm L
            bone([0.0,0.9,0.0], [0.7,0.9,0.0], 0.10, 0),    // 3 arm R
            bone([0.0,0.0,0.0], [-0.2,-1.1,0.0], 0.13, 0),  // 4 leg L
            bone([0.0,0.0,0.0], [0.2,-1.1,0.0], 0.13, 0),   // 5 leg R
        ]
    }

    fn box_volume(w: f64, h: f64, d: f64, res: usize) -> Volume {
        let mut pts = Vec::new();
        let n = 12;
        for i in 0..=n { for j in 0..=n { for k in 0..=n {
            pts.push([ -w/2.0 + w*i as f64/n as f64,
                       -h/2.0 + h*j as f64/n as f64,
                       -d/2.0 + d*k as f64/n as f64 ]);
        }}}
        Volume::from_points(&pts, res)
    }

    #[test]
    fn a_standing_body_does_not_fit_a_flat_box_but_a_folded_one_does_better() {
        // The whole premise: the fold is what makes a person fit a vehicle.
        // A vehicle-ish box: wider and deeper than it is tall, but not so flat
        // that NO pose fits. The first version used 0.9 height against a
        // 2.5-tall body, where every configuration spills equally and the test
        // could not show a difference — it was measuring an impossible fit, not
        // a bad fold.
        let vol = box_volume(2.4, 1.2, 1.8, 10);
        // Scale the body to the volume first. A bot is roughly its vehicle's
        // size, and without this step the body is too big for the box in EVERY
        // configuration - both attempts above spilled identically because no
        // fold could fit, which measures an impossible problem rather than a
        // bad fold. A real fitter solves scale and pose together; the test fixes
        // scale so it can isolate pose.
        let upright: Vec<Bone> = body().iter().map(|b| Bone {
            a: [b.a[0]*0.5, b.a[1]*0.5, b.a[2]*0.5],
            b: [b.b[0]*0.5, b.b[1]*0.5, b.b[2]*0.5],
            r: b.r*0.5, parent: b.parent,
        }).collect();
        let before = evaluate(&upright, &vol, 6);

        let mut folded = upright.clone();
        // TUCK the legs, do not swing them. A quarter turn lays a leg
        // horizontally into the box's depth; the first attempt used 2.6 rad,
        // which rotates a leg up and BACK and leaves it sticking out of a box
        // that is wide but not tall. Folding badly is worse than not folding,
        // and the spill term said so.
        let quarter = std::f64::consts::FRAC_PI_2;
        bend(&mut folded, 4, [1.0, 0.0, 0.0], quarter);
        bend(&mut folded, 5, [1.0, 0.0, 0.0], quarter);
        bend(&mut folded, 2, [0.0, 1.0, 0.0], quarter);
        bend(&mut folded, 3, [0.0, 1.0, 0.0], -quarter);
        bend(&mut folded, 1, [1.0, 0.0, 0.0], quarter);   // and duck the head
        let after = evaluate(&folded, &vol, 6);

        assert!(after.spill < before.spill,
                "folding should reduce spill: {} -> {}", before.spill, after.spill);
        assert!(after.error() < before.error(),
                "and improve the fit overall: {} -> {}", before.error(), after.error());
    }

    #[test]
    fn bending_a_joint_carries_the_whole_chain() {
        // Anatomy rather than free parts: a rotation at the spine must take the
        // head and arms with it, or the body comes apart.
        let mut b = body();
        let head_before = b[1].b;
        bend(&mut b, 0, [0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2);
        assert!((b[1].b[0] - head_before[0]).abs() > 0.5, "the head moved with the spine");
        // and the arms too
        assert!((b[2].b[1]).abs() > 0.3, "the arm swung with it");
        // bone lengths are preserved — a fold does not stretch anyone
        for (i, bone) in b.iter().enumerate() {
            assert!((bone.length() - body()[i].length()).abs() < 1e-9,
                    "bone {i} changed length");
        }
    }

    #[test]
    fn curling_into_a_corner_is_rejected_by_the_vacancy_term() {
        // The degenerate answer: a body balled up in one corner spills nothing
        // and explains nothing. Without a vacancy term the optimiser finds it
        // every time.
        let vol = box_volume(3.0, 1.0, 2.0, 10);
        let mut tiny = body();
        for b in tiny.iter_mut() {
            for p in [&mut b.a, &mut b.b] {
                for i in 0..3 { p[i] *= 0.05; }
            }
        }
        let curled = evaluate(&tiny, &vol, 6);
        assert!(curled.spill < 0.05, "it is entirely inside");
        assert!(curled.vacancy > 0.8, "but it explains almost none of the volume");
        assert!(curled.error() > 0.8, "so the fit is bad despite zero spill");
    }

    #[test]
    fn spill_is_weighted_harder_than_vacancy() {
        // A limb through the hull is invalid; unfilled space is merely
        // unconvincing. The error must rank them that way.
        let a = Fit { spill: 0.30, vacancy: 0.0 };
        let b = Fit { spill: 0.0, vacancy: 0.30 };
        assert!(a.error() > b.error(), "spill must dominate: {} vs {}", a.error(), b.error());
    }

    #[test]
    fn an_empty_volume_does_not_divide_by_zero() {
        let vol = Volume::from_points(&[[0.0;3]], 4);
        let f = evaluate(&body(), &vol, 4);
        assert!(f.spill.is_finite() && f.vacancy.is_finite());
    }
}
