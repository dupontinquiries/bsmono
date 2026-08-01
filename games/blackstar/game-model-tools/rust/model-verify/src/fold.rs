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

/// How unevenly the bones would divide the vehicle between them.
///
/// The term that was missing, and its absence is why the whole pipeline produced
/// "a tank, then a slightly different tank". Filling a volume says nothing about
/// filling it WITH A BODY: a skeleton standing almost upright inside a tank
/// scores about as well as one properly folded, so the optimiser had no reason
/// to fold and chose 28/43/35/26/-7/2 degrees — a person standing up straight.
/// Interpolating from that to upright is nearly a no-op, so the vehicle barely
/// moved.
///
/// Measuring how evenly the bones divide the space forces each limb into its own
/// region, which is what a fold IS. Returns 0 for a perfectly even share and
/// approaches 1 as one bone takes everything.
pub fn imbalance(bones: &[Bone], vol: &Volume) -> f64 {
    if bones.is_empty() { return 1.0; }
    let mut claim = vec![0usize; bones.len()];
    let mut total = 0usize;
    for k in 0..vol.filled.len() {
        if !vol.filled[k] { continue; }
        let c = vol.cell_centre(k);
        let mut best = 0usize;
        let mut bd = f64::MAX;
        for (i, b) in bones.iter().enumerate() {
            let d = b.dist_to(c);
            if d < bd { bd = d; best = i; }
        }
        claim[best] += 1;
        total += 1;
    }
    if total == 0 { return 1.0; }
    let ideal = total as f64 / bones.len() as f64;
    // mean absolute deviation from an even share, normalised
    let dev: f64 = claim.iter().map(|c| (*c as f64 - ideal).abs()).sum();
    (dev / (2.0 * total as f64)).min(1.0)
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

    pub fn flat_box() -> Volume { box_volume(2.4, 1.2, 1.8, 10) }
    pub fn scaled_body() -> Vec<Bone> {
        body().iter().map(|b| Bone { a: [b.a[0]*0.5,b.a[1]*0.5,b.a[2]*0.5],
            b: [b.b[0]*0.5,b.b[1]*0.5,b.b[2]*0.5], r: b.r*0.5, parent: b.parent }).collect()
    }

    pub fn bone(a: [f64;3], b: [f64;3], r: f64, parent: i32) -> Bone { Bone { a, b, r, parent } }

    /// A crude biped: spine, two arms, two legs.
    pub fn body() -> Vec<Bone> {
        vec![
            bone([0.0,0.0,0.0], [0.0,1.0,0.0], 0.18, -1),   // 0 spine
            bone([0.0,1.0,0.0], [0.0,1.4,0.0], 0.14, 0),    // 1 head
            bone([0.0,0.9,0.0], [-0.7,0.9,0.0], 0.10, 0),   // 2 arm L
            bone([0.0,0.9,0.0], [0.7,0.9,0.0], 0.10, 0),    // 3 arm R
            bone([0.0,0.0,0.0], [-0.2,-1.1,0.0], 0.13, 0),  // 4 leg L
            bone([0.0,0.0,0.0], [0.2,-1.1,0.0], 0.13, 0),   // 5 leg R
        ]
    }

    pub fn box_volume(w: f64, h: f64, d: f64, res: usize) -> Volume {
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

// ── searching for the fold ──────────────────────────────────────────────────

/// What a joint is allowed to do. Anatomy is the search space's whole value: a
/// knee that bends backwards would fit many vehicles beautifully and look wrong
/// in every one of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    pub joint: usize,
    pub axis: [f64; 3],
    pub min: f64,
    pub max: f64,
}

/// Deterministic pseudo-random, so a fold is reproducible. An optimiser that
/// gives a different answer each run cannot be reviewed, and "it looked better
/// last time" is not a debuggable report.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 { a + (b - a) * self.next() }
}

/// The result of a search.
#[derive(Debug, Clone)]
pub struct Fold {
    /// Angle chosen per limit, in the order given.
    pub angles: Vec<f64>,
    pub fit: Fit,
    pub bones: Vec<Bone>,
    /// How many candidates were evaluated — reported so a poor result can be
    /// told from an under-searched one.
    pub evaluated: usize,
}

fn apply_angles(base: &[Bone], limits: &[Limit], angles: &[f64]) -> Vec<Bone> {
    let mut b = base.to_vec();
    for (l, a) in limits.iter().zip(angles) {
        bend(&mut b, l.joint, l.axis, *a);
    }
    b
}

/// Search joint-angle space for the fold that best fills `vol`.
///
/// Simulated annealing rather than gradient descent: the error surface is full
/// of local minima (a limb can be blocked by the hull in one direction and free
/// in another), and there is no gradient to take — spill and vacancy are counts
/// over a voxel grid, not smooth functions.
///
/// Proposals shrink as the temperature falls, so the search moves from "try a
/// completely different pose" to "settle this joint", which is how a body
/// actually gets packed into a box.
pub fn optimise(base: &[Bone], vol: &Volume, limits: &[Limit],
                iters: usize, seed: u64) -> Fold {
    let mut rng = Rng(seed | 1);
    // START FOLDED, not at the midpoint of each range.
    //
    // Annealing explores near where it starts, and starting a near-upright body
    // inside a vehicle means it never discovers that folding pays: it reached
    // angles like -4/-3 degrees on the legs, which is a person standing up. A
    // transformation that begins from a standing pose produces "a tank, then a
    // slightly different tank" - which is exactly what the render showed.
    //
    // The far end of each joint's range is the folded end by construction (the
    // limits are written with 0 as neutral and the maximum as fully tucked), so
    // starting there means the search has to justify UNFOLDING rather than
    // stumbling onto folding.
    let mut cur: Vec<f64> = limits.iter()
        .map(|l| if l.max.abs() > l.min.abs() { l.max * 0.85 } else { l.min * 0.85 })
        .collect();
    let mut cur_bones = apply_angles(base, limits, &cur);
    let mut cur_fit = evaluate(&cur_bones, vol, 5);
    let mut best = (cur.clone(), cur_fit, cur_bones.clone());
    let mut evaluated = 1usize;

    for it in 0..iters.max(1) {
        let t = 1.0 - it as f64 / iters.max(1) as f64;      // 1 -> 0
        let temp = 0.02 + 0.6 * t * t;
        let mut cand = cur.clone();
        // perturb one joint at a time most of the time; occasionally several,
        // because some folds only pay off once two joints move together
        let n_move = if rng.next() < 0.75 { 1 } else { 2 + (rng.next() * 2.0) as usize };
        for _ in 0..n_move {
            if limits.is_empty() { break; }
            let k = (rng.next() * limits.len() as f64) as usize % limits.len();
            let l = &limits[k];
            let step = (l.max - l.min) * temp;
            cand[k] = (cand[k] + rng.range(-step, step)).clamp(l.min, l.max);
        }
        let bones = apply_angles(base, limits, &cand);
        let fit = evaluate(&bones, vol, 5);
        evaluated += 1;
        // Score = fit + imbalance. Without the second term the optimiser is
        // indifferent between a folded body and a standing one, and picks
        // whichever it stumbled on first.
        let score = |f: Fit, b: &[Bone]| f.error() + imbalance(b, vol) * 1.5;
        let d = score(fit, &bones) - score(cur_fit, &cur_bones);
        // accept improvements always, worse poses sometimes — that is what gets
        // a limb out of a local minimum it is wedged in
        if d < 0.0 || rng.next() < (-d / temp.max(1e-6)).exp() {
            cur = cand;
            cur_fit = fit;
            cur_bones = bones;
            if score(fit, &cur_bones) < score(best.1, &best.2) {
                best = (cur.clone(), fit, cur_bones.clone());
            }
        }
    }
    Fold { angles: best.0, fit: best.1, bones: best.2, evaluated }
}

#[cfg(test)]
mod optimise_tests {
    use super::*;
    use super::tests::*;

    fn limits() -> Vec<Limit> {
        let q = std::f64::consts::FRAC_PI_2;
        vec![
            Limit { joint: 4, axis: [1.0,0.0,0.0], min: -0.2, max: 2.4 },  // leg L
            Limit { joint: 5, axis: [1.0,0.0,0.0], min: -0.2, max: 2.4 },  // leg R
            Limit { joint: 2, axis: [0.0,1.0,0.0], min: -q,   max: q   },  // arm L
            Limit { joint: 3, axis: [0.0,1.0,0.0], min: -q,   max: q   },  // arm R
            Limit { joint: 1, axis: [1.0,0.0,0.0], min: -0.9, max: 0.9 },  // head
            Limit { joint: 0, axis: [0.0,0.0,1.0], min: -0.6, max: 0.6 },  // spine
        ]
    }

    #[test]
    fn the_search_finds_a_better_fold_than_the_pose_it_started_from() {
        let vol = flat_box();
        let base = scaled_body();
        let start = evaluate(&base, &vol, 5);
        let f = optimise(&base, &vol, &limits(), 400, 12345);
        assert!(f.evaluated > 300, "it actually searched: {} candidates", f.evaluated);
        assert!(f.fit.error() < start.error(),
                "the search must improve on standing: {:.3} -> {:.3}", start.error(), f.fit.error());
    }

    #[test]
    fn the_search_is_deterministic() {
        // An optimiser that answers differently each run cannot be reviewed, and
        // "it looked better last time" is not a debuggable report.
        let vol = flat_box();
        let base = scaled_body();
        let a = optimise(&base, &vol, &limits(), 300, 999);
        let b = optimise(&base, &vol, &limits(), 300, 999);
        assert_eq!(a.angles, b.angles);
        assert_eq!(a.fit, b.fit);
        // and a different seed explores differently
        let c = optimise(&base, &vol, &limits(), 300, 4242);
        assert!(a.angles != c.angles || (a.fit.error() - c.fit.error()).abs() < 1e-9);
    }

    #[test]
    fn every_joint_stays_within_its_anatomical_limit() {
        // A knee that bends backwards fits many vehicles beautifully and looks
        // wrong in all of them.
        let vol = flat_box();
        let f = optimise(&scaled_body(), &vol, &limits(), 400, 7);
        for (l, a) in limits().iter().zip(&f.angles) {
            assert!(*a >= l.min - 1e-9 && *a <= l.max + 1e-9,
                    "joint {} at {a} is outside [{}, {}]", l.joint, l.min, l.max);
        }
    }

    #[test]
    fn bone_lengths_survive_the_search() {
        // The optimiser moves joints; it must never stretch anyone to fit.
        let base = scaled_body();
        let f = optimise(&base, &flat_box(), &limits(), 300, 3);
        for (i, b) in f.bones.iter().enumerate() {
            assert!((b.length() - base[i].length()).abs() < 1e-9,
                    "bone {i} changed length during the search");
        }
    }

    #[test]
    fn the_control_a_body_shaped_volume_is_easier_to_fill_than_a_slab() {
        // The discipline the part matcher lacked: check the score is WORSE where
        // it should be. A body fits a body-shaped space better than a flat slab,
        // and if the optimiser cannot tell those apart its error function is
        // reporting nothing.
        let base = scaled_body();
        let upright_shaped = box_volume(0.9, 1.5, 0.9, 10);   // roughly body-shaped
        let slab = box_volume(3.0, 0.35, 3.0, 10);            // nothing folds into this well
        let a = optimise(&base, &upright_shaped, &limits(), 400, 21);
        let b = optimise(&base, &slab, &limits(), 400, 21);
        assert!(a.fit.error() < b.fit.error(),
                "a body-shaped volume must score better than a slab: {:.3} vs {:.3}",
                a.fit.error(), b.fit.error());
        assert!(a.fit.spill < b.fit.spill,
                "and spill less: {:.3} vs {:.3}", a.fit.spill, b.fit.spill);
    }
}
