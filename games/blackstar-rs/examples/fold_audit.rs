//! Watch a fold happen, numerically. No renderer.
//!
//! Two failures a still frame cannot show and a running game shows only if
//! you happen to be looking at the right chassis at the right moment:
//!
//! **Parts decoupling.** The fold blends vertex positions between poses.
//! A part whose two end positions are far apart travels in a straight
//! line between them, and a straight line does not respect the body — an
//! arm can leave the torso, cross open space, and arrive back attached.
//! At t=0 and t=1 everything looks right, so nothing at the endpoints
//! catches it.
//!
//! **Skeleton bleeding out of the vehicle.** In vehicle mode the limbs
//! are supposed to be stowed inside the shell. If a limb pokes out at
//! t=0, the car has an arm sticking through its door.
//!
//! ## What is measured
//!
//! For each prim, its centroid's distance from the body centroid,
//! normalised by the body's own half-extent, sampled across the fold.
//!
//! * **Excursion** is how far past its own endpoint offsets a part
//!   travels mid-fold. A part that sits at 0.8 body-radii at both ends
//!   and reaches 2.4 in the middle has an excursion of 1.6: it left.
//! * **Bleed** is how far outside the t=0 body envelope any prim reaches
//!   at t=0 — measured against the LARGEST prim's box, taken as the
//!   shell, rather than against a skeleton the loader may not have.
//!
//! Both are ratios, so they are comparable across chassis of different
//! sizes, and both are zero for a fold that behaves.
//!
//!   fold_audit <bots dir> <chassis> [chassis...]
//!   fold_audit <bots dir> --sweep-all

include!("shim.rs");

#[path = "../src/xform.rs"]
pub mod xform;
#[path = "../src/align.rs"]
pub mod align;
#[path = "../src/hardpoints.rs"]
pub mod hardpoints2;

use glam::Vec3;
use std::path::{Path, PathBuf};

/// Fold positions to sample. The endpoints are included so an excursion
/// can be measured against them.
const STEPS: usize = 17;

struct Finding {
    worst_excursion: f32,
    worst_prim: usize,
    worst_t: f32,
    /// Largest gap, in body radii, between a prim and its nearest
    /// neighbour at an endpoint pose. See `island_gap`.
    island: f32,
    island_prim: usize,
    /// How much WORSE containment got, in envelope radii. `fold_pack`
    /// pulls limbs toward the shell; it must never push one further out.
    /// Positive means it did.
    contain_regress: f32,
    /// Fraction of limb geometry that can see open sky in VEHICLE mode.
    /// See `exposure`.
    exposure: f32,
    /// Exposure after `skeleton::fold_pack`'s containment is applied to
    /// the vehicle pose. The question this answers: would running the
    /// stowing machinery the codebase already has actually hide the robot?
    exposure_packed: f32,
    /// Exposure if each limb's shift is chosen to minimise VISIBLE limb
    /// samples instead of axis-aligned overhang. An experiment, not a
    /// shipped path — see `silhouette_shift`.
    exposure_silhouette: f32,
    /// The single limb contributing the most visible samples, and its
    /// share of the total. What an artist would look at first.
    worst_limb_prim: usize,
    worst_limb_share: f32,
}

/// Slab test: does the ray from `o` along `d` meet the box at a positive
/// distance? Axis directions only, so this reduces to a range check on the
/// two other axes plus a sign test on the travel axis.
fn ray_hits_box(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> bool {
    let (mut tmin, mut tmax) = (0.0f32, f32::MAX);
    for a in 0..3 {
        if d[a].abs() < 1e-6 {
            if o[a] < lo[a] || o[a] > hi[a] {
                return false;
            }
        } else {
            let inv = 1.0 / d[a];
            let (mut t0, mut t1) = ((lo[a] - o[a]) * inv, (hi[a] - o[a]) * inv);
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            tmin = tmin.max(t0);
            tmax = tmax.min(t1);
            if tmax < tmin {
                return false;
            }
        }
    }
    // A hit at the origin itself is the vertex touching its own occluder,
    // which does not hide it from anything.
    tmax > 1e-4
}

fn centroid(pts: &[Vec3]) -> Vec3 {
    if pts.is_empty() {
        return Vec3::ZERO;
    }
    pts.iter().copied().fold(Vec3::ZERO, |a, b| a + b) / pts.len() as f32
}

fn audit(pose_dirs: &[PathBuf]) -> Option<Finding> {
    // Load every pose through the game's own loader, then unify them the
    // way the game does — anything else would be auditing a different
    // transform from the one that ships.
    let strip: Vec<Vec<model::RawPrim>> =
        pose_dirs.iter().map(|d| model::load_raw_unoriented(d)).collect();
    if strip.iter().any(|p| p.is_empty()) {
        return None;
    }
    // The pose's own prims, kept alongside the unified strip: the stow the
    // game applies is derived from these, not from the unified set.
    let t0_own: Vec<Vec<Vec3>> = model::load_raw_unoriented(&pose_dirs[0])
        .iter()
        .map(|p| p.verts.iter().map(|v| Vec3::from_array(v.pos)).collect())
        .collect();

    let u = align::unify(strip)?;
    let mut morph = xform::Morph::build(u.poses.clone(), u.idx.clone())?;

    let n_prims = morph.prims();
    if n_prims == 0 {
        return None;
    }

    // offsets[p][s] — prim p's centroid distance from the body centroid,
    // in body radii, at step s.
    let mut offsets = vec![vec![0.0f32; STEPS]; n_prims];
    let mut t0_pts: Vec<Vec<Vec3>> = Vec::new();
    for s in 0..STEPS {
        let t = s as f32 / (STEPS - 1) as f32;
        let mut pts: Vec<Vec<Vec3>> = Vec::with_capacity(n_prims);
        for p in 0..n_prims {
            let m = morph.sample(p, t);
            pts.push(m.verts.iter().map(|v| Vec3::from_array(v.pos)).collect());
        }
        let all: Vec<Vec3> = pts.iter().flatten().copied().collect();
        let body = centroid(&all);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &all {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let radius = ((hi - lo) * 0.5).max_element().max(1e-4);
        for (p, pts_p) in pts.iter().enumerate() {
            offsets[p][s] = (centroid(pts_p) - body).length() / radius;
        }
        if s == 0 {
            t0_pts = pts;
        }
    }

    // EXCURSION: how far past its own endpoints a part wanders.
    let mut worst = Finding {
        worst_excursion: 0.0,
        worst_prim: 0,
        worst_t: 0.0,
        island: 0.0,
        island_prim: 0,
        contain_regress: 0.0,
        exposure: 0.0,
        exposure_packed: 0.0,
        exposure_silhouette: 0.0,
        worst_limb_prim: 0,
        worst_limb_share: 0.0,
    };
    for (p, row) in offsets.iter().enumerate() {
        let anchor = row[0].max(row[STEPS - 1]);
        for (s, o) in row.iter().enumerate() {
            let ex = o - anchor;
            if ex > worst.worst_excursion {
                worst.worst_excursion = ex;
                worst.worst_prim = p;
                worst.worst_t = s as f32 / (STEPS - 1) as f32;
            }
        }
    }

    // ISLANDS: a prim floating free of the body in an ENDPOINT pose.
    //
    // Found by rendering `tank_vanquish` and looking at it — a chunk sits
    // clear of the hull, about half a body away, in the authored f00 pose.
    // That is not a fold artifact; it is there before the fold starts, and
    // the excursion metric above cannot see it because excursion is
    // measured against the endpoints.
    //
    // Well defined, unlike the two bleed attempts below: the gap from each
    // prim's box to the NEAREST other prim's box, in body radii. A body
    // whose parts touch scores 0. This is the metric those attempts were
    // reaching for.
    {
        let boxes: Vec<(Vec3, Vec3)> = t0_pts
            .iter()
            .map(|pts| {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for p in pts {
                    lo = lo.min(*p);
                    hi = hi.max(*p);
                }
                (lo, hi)
            })
            .collect();
        let live: Vec<usize> = (0..boxes.len()).filter(|i| boxes[*i].1.x >= boxes[*i].0.x).collect();
        if live.len() > 1 {
            let (mut blo, mut bhi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for i in &live {
                blo = blo.min(boxes[*i].0);
                bhi = bhi.max(boxes[*i].1);
            }
            let radius = ((bhi - blo) * 0.5).max_element().max(1e-4);
            for &i in &live {
                let mut nearest = f32::MAX;
                for &j in &live {
                    if i == j {
                        continue;
                    }
                    // Axis-aligned box separation: zero if they overlap.
                    let d = (boxes[i].0 - boxes[j].1).max(boxes[j].0 - boxes[i].1).max(Vec3::ZERO);
                    nearest = nearest.min(d.length());
                }
                if nearest < f32::MAX && nearest / radius > worst.island {
                    worst.island = nearest / radius;
                    worst.island_prim = i;
                }
            }
        }
    }

    // EXPOSURE: how much of the robot can actually be SEEN in vehicle mode?
    //
    // Third attempt at this, and the first that agrees with a render.
    //
    // Attempt one asked "are limb points outside the core box". Attempt
    // two cast six axis rays against prim bounding boxes. Both are
    // containment tests wearing a visibility costume, and the second one
    // confidently reported that stowing `tank_vanquish` improved it from
    // 17% to 9% — while the stowed render is plainly worse, with the limbs
    // splayed around the hull instead of tucked into it.
    //
    // Visibility is a DEPTH question, so this resolves it with a depth
    // buffer. Three orthographic views (front, side, top), each a coarse
    // grid: rasterise the nearest hull depth per cell, then ask of every
    // limb sample whether anything is in front of it. A limb point that is
    // nearer to the camera than the hull at that cell is on screen. That
    // is what "visible" means, and it is why splaying a limb sideways --
    // which reduces axis overhang -- makes this number go UP rather than
    // down.
    {
        // THE POSE'S OWN PRIMS, NOT THE UNIFIED SET.
        //
        // `u.poses[0]` is the vehicle pose AFTER unification, which adds
        // placeholder entries for parts that only exist in later poses —
        // 64 slots where the pose itself has 57. The GAME computes
        // `Model::pack` in `load_raw_at` from the model's own prims, so a
        // stow derived from the unified set is a different transform from
        // the one that ships: measured on tank_vanquish, 23 limbs and 36
        // moved prims against the game's 16 and 29.
        //
        // That is why this metric disagreed with a render of the stowed
        // pose. It was not the metric being coarse; it was measuring a
        // stow nothing applies.
        let refs: Vec<&[Vec3]> = t0_own.iter().map(|v| v.as_slice()).collect();
        let sk = skeleton::skeletonize(&refs);
        let limb_prims: std::collections::BTreeSet<usize> =
            sk.limbs.iter().flat_map(|l| l.prims.iter().copied()).collect();
        if !limb_prims.is_empty() && limb_prims.len() < t0_own.len() {
            const SLACK: f32 = 0.25;
            let steps = skeleton::fold_pack(&sk, &refs, SLACK, Vec3::Y);
            let packed = |i: usize| -> Vec<Vec3> {
                let st = steps.get(i).copied().unwrap_or_default();
                t0_own
                    .get(i)
                    .map(|v| v.iter().map(|p| st.pivot + st.rot * (*p - st.pivot) + st.shift).collect())
                    .unwrap_or_default()
            };
            let raw = |i: usize| -> Vec<Vec3> { t0_own.get(i).cloned().unwrap_or_default() };

            let measure_for = |get: &dyn Fn(usize) -> Vec<Vec3>,
                               only: Option<&std::collections::BTreeSet<usize>>|
             -> f32 {
                // Bounds over everything, so all three views share a frame.
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for i in 0..t0_own.len() {
                    for p in get(i) {
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
                let size = (hi - lo).max(Vec3::splat(1e-4));
                const G: usize = 48;
                // (u, v, depth) axis triples: looking down -Z, -X, -Y.
                let views: [(usize, usize, usize); 3] = [(0, 1, 2), (2, 1, 0), (0, 2, 1)];
                let (mut seen, mut total) = (0usize, 0usize);
                for (ua, va, da) in views {
                    // Nearest hull depth per cell. Points only, so a sparse
                    // mesh leaves gaps -- accepted, and it errs toward
                    // calling limbs visible rather than hiding them.
                    let mut hull = vec![f32::MAX; G * G];
                    for i in 0..t0_own.len() {
                        if limb_prims.contains(&i) {
                            continue;
                        }
                        for p in get(i) {
                            let u = (((p[ua] - lo[ua]) / size[ua]) * (G - 1) as f32) as usize;
                            let v = (((p[va] - lo[va]) / size[va]) * (G - 1) as f32) as usize;
                            let cell = &mut hull[v.min(G - 1) * G + u.min(G - 1)];
                            *cell = cell.min(p[da]);
                        }
                    }
                    for i in &limb_prims {
                        if only.is_some_and(|o| !o.contains(i)) {
                            continue;
                        }
                        let pts = get(*i);
                        let stride = (pts.len() / 48).max(1);
                        for p in pts.iter().step_by(stride) {
                            let u = (((p[ua] - lo[ua]) / size[ua]) * (G - 1) as f32) as usize;
                            let v = (((p[va] - lo[va]) / size[va]) * (G - 1) as f32) as usize;
                            total += 1;
                            // Visible if no hull sample in this cell is in
                            // front of it. A small bias so coincident
                            // surfaces do not read as exposed.
                            if hull[v.min(G - 1) * G + u.min(G - 1)] > p[da] - 1e-3 {
                                seen += 1;
                            }
                        }
                    }
                }
                if total > 0 { seen as f32 / total as f32 } else { 0.0 }
            };
            let measure = |get: &dyn Fn(usize) -> Vec<Vec3>| measure_for(get, None);
            let measure_subset =
                |get: &dyn Fn(usize) -> Vec<Vec3>, only: &std::collections::BTreeSet<usize>| {
                    measure_for(get, Some(only))
                };

            worst.exposure = measure(&raw);
            worst.exposure_packed = measure(&packed);

            // WHICH LIMB IS THE PROBLEM. A fleet number says "remodel
            // this chassis"; a prim index says where to start.
            {
                let mut per_limb: Vec<(usize, f32)> = Vec::new();
                let mut total = 0.0f32;
                for limb in &sk.limbs {
                    let only: std::collections::BTreeSet<usize> =
                        limb.prims.iter().copied().collect();
                    let share = measure_subset(&packed, &only);
                    total += share;
                    per_limb.push((*limb.prims.first().unwrap_or(&0), share));
                }
                if let Some((p, v)) = per_limb
                    .iter()
                    .copied()
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                {
                    worst.worst_limb_prim = p;
                    worst.worst_limb_share = if total > 0.0 { v / total } else { 0.0 };
                }
            }

            // THE EXPERIMENT: optimise the objective we actually want.
            //
            // `fold_pack` minimises each limb's axis-aligned overhang
            // against an envelope. What the eye judges is whether the limb
            // sits inside the vehicle's SILHOUETTE. Those differ, and the
            // difference is why packing trades limbs overlapping the hull
            // for limbs on the outline — fewer visible samples, more
            // noticeable ones.
            //
            // Here each limb keeps `fold_pack`'s hinge but picks its shift
            // by direct search: candidates along the direction of the core
            // centre at several magnitudes, scored by how many of that
            // limb's samples the depth test still finds visible. Coarse
            // and slow, and that is fine — the question is whether the
            // objective is worth changing, not how to implement it well.
            let hull_depth = {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for i in 0..t0_own.len() {
                    for p in raw(i) {
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
                (lo, hi)
            };
            let mut chosen: Vec<Vec3> = vec![Vec3::ZERO; t0_own.len()];
            for limb in &sk.limbs {
                let toward = (sk.core_centre - limb.tip).normalize_or_zero();
                let reach = limb.reach.max(1e-3);
                let mut best = (f32::MAX, Vec3::ZERO);
                for k in 0..9 {
                    let cand = toward * (reach * k as f32 * 0.125);
                    // Score: visible samples of THIS limb under that shift.
                    let mut vis = 0usize;
                    let (lo, hi) = hull_depth;
                    let size = (hi - lo).max(Vec3::splat(1e-4));
                    const G: usize = 48;
                    for (ua, va, da) in [(0usize, 1usize, 2usize), (2, 1, 0), (0, 2, 1)] {
                        let mut hull = vec![f32::MAX; G * G];
                        for i in 0..t0_own.len() {
                            if limb_prims.contains(&i) {
                                continue;
                            }
                            for p in raw(i) {
                                let u = (((p[ua] - lo[ua]) / size[ua]) * (G - 1) as f32) as usize;
                                let v = (((p[va] - lo[va]) / size[va]) * (G - 1) as f32) as usize;
                                let c = &mut hull[v.min(G - 1) * G + u.min(G - 1)];
                                *c = c.min(p[da]);
                            }
                        }
                        for i in &limb.prims {
                            let pts = raw(*i);
                            let stride = (pts.len() / 24).max(1);
                            for p0 in pts.iter().step_by(stride) {
                                let st = steps.get(*i).copied().unwrap_or_default();
                                let p = st.pivot + st.rot * (*p0 - st.pivot) + cand;
                                let u = (((p[ua] - lo[ua]) / size[ua]) * (G - 1) as f32) as usize;
                                let v = (((p[va] - lo[va]) / size[va]) * (G - 1) as f32) as usize;
                                if hull[v.min(G - 1) * G + u.min(G - 1)] > p[da] - 1e-3 {
                                    vis += 1;
                                }
                            }
                        }
                    }
                    if (vis as f32) < best.0 {
                        best = (vis as f32, cand);
                    }
                }
                for i in &limb.prims {
                    if let Some(slot) = chosen.get_mut(*i) {
                        *slot = best.1;
                    }
                }
            }
            worst.exposure_silhouette = measure(&|i| {
                let st = steps.get(i).copied().unwrap_or_default();
                let extra = chosen.get(i).copied().unwrap_or(Vec3::ZERO);
                t0_own
                    .get(i)
                    .map(|v| v.iter().map(|p| st.pivot + st.rot * (*p - st.pivot) + extra).collect())
                    .unwrap_or_default()
            });
        }
    }

    // CONTAINMENT: does `fold_pack` actually stow the limbs?
    //
    // Not "is everything inside the envelope afterwards" — that is not the
    // contract and cannot be, because a limb wider than the envelope does
    // not fit at any offset (see `contain_shift`'s own note about the
    // jet wing spanning 5.2 across a 1.25 envelope). What IS promised is
    // weaker and checkable: the shift pulls a limb TOWARD the shell, so
    // the overhang after must not exceed the overhang before.
    //
    // This is the "skeleton bleeding out of the vehicle" question stated
    // in a form that has an answer. The two attempts before it —
    // "inside the largest prim", "inside the core box" — flagged 253 and
    // 254 of 254 because they asserted things that were never true.
    {
        let refs: Vec<&[Vec3]> = t0_pts.iter().map(|v| v.as_slice()).collect();
        let sk = skeleton::skeletonize(&refs);
        const SLACK: f32 = 0.25;
        let steps = skeleton::fold_pack(&sk, &refs, SLACK, Vec3::Y);
        if sk.core_half.max_element() > 1e-4 {
            let env = sk.core_half * (1.0 + SLACK);
            let (elo, ehi) = (sk.core_centre - env, sk.core_centre + env);
            let radius = env.max_element().max(1e-4);
            let overhang = |pts: &[Vec3]| -> f32 {
                let mut worst = 0.0f32;
                for p in pts {
                    let out = (elo - *p).max(*p - ehi).max(Vec3::ZERO);
                    worst = worst.max(out.max_element());
                }
                worst
            };
            for limb in &sk.limbs {
                let before: Vec<Vec3> = limb
                    .prims
                    .iter()
                    .flat_map(|i| t0_pts.get(*i).map(|v| v.as_slice()).unwrap_or(&[]).iter().copied())
                    .collect();
                let after: Vec<Vec3> = limb
                    .prims
                    .iter()
                    .flat_map(|i| {
                        let step = steps.get(*i).copied().unwrap_or_default();
                        t0_pts
                            .get(*i)
                            .map(|v| v.as_slice())
                            .unwrap_or(&[])
                            .iter()
                            .map(move |p| step.pivot + step.rot * (*p - step.pivot) + step.shift)
                    })
                    .collect();
                if before.is_empty() {
                    continue;
                }
                let regress = (overhang(&after) - overhang(&before)) / radius;
                worst.contain_regress = worst.contain_regress.max(regress);
            }
        }
    }

    // BLEED IS NOT MEASURED HERE, AND THAT IS A CORRECTION.
    //
    // Two attempts, both wrong, both worth recording so the third does not
    // repeat them:
    //
    // 1. "Everything should be inside the largest prim." Flagged 253 of
    //    254 — a vehicle body is many prims and nearly anything is outside
    //    the biggest single one.
    // 2. "Limbs should be inside the skeleton's core box." Flagged 254 of
    //    254, median 3.75 core-radii. Of course: a limb extending past the
    //    core is what makes it a limb.
    //
    // The question "does the skeleton bleed out of the vehicle" is not
    // answerable from the t=0 pose, because at t=0 the body IS the
    // authored vehicle mesh — whatever sticks out is what the artist
    // modelled, and that is an art review, not an assertion.
    //
    // Where it IS answerable is `skeleton::fold_pack` and
    // `skeleton::contain`, which exist to stow limbs inside an envelope
    // and take a slack parameter. Auditing those means checking their
    // OUTPUT against the envelope they were given, which is a real,
    // checkable contract. That belongs in its own harness against those
    // functions, not bolted onto a fold sampler.

    Some(worst)
}

/// A chassis is a directory of pose subdirectories; the fold strip is
/// their sorted order, which is what the loader uses.
fn poses_of(chassis: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(chassis)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: fold_audit <bots dir> <chassis>... | <bots dir> --sweep-all");
        std::process::exit(2);
    }
    if args.get(2).map(|a| a == "--unify").unwrap_or(false) {
        unify_report(&PathBuf::from(&args[1]));
        return;
    }
    if args.get(2).map(|a| a == "--cavity").unwrap_or(false) {
        cavity_report(&PathBuf::from(&args[1]));
        return;
    }
    if args.get(2).map(|a| a == "--colors").unwrap_or(false) {
        dump_colors(&PathBuf::from(&args[1]));
        return;
    }
    let root = PathBuf::from(&args[1]);
    let names: Vec<String> = if args[2] == "--sweep-all" {
        let mut v: Vec<String> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    } else {
        args[2..].to_vec()
    };

    // Thresholds are reported, not enforced: this is an audit, and the
    // first run is what tells anyone what "normal" looks like here.
    // Set ABOVE the observed p90 (0.56) rather than picked in advance. A
    // threshold below the median flags half the fleet and tells nobody
    // anything; the point of an audit is to name the outliers.
    const EXCURSION_FLAG: f32 = 0.70;
    // Above the observed p90 (0.67), NOT a number picked in advance.
    //
    // I set this to 0.10 first -- having just written the comment above
    // about not doing exactly that -- and it flagged 235 of 254. The
    // median gap in this fleet is 0.37 body radii, because these models
    // legitimately carry detached prims: greeble, antennae, wheels that
    // do not touch the hull. A separated part is NORMAL here, and only a
    // far-separated one is a finding.
    const ISLAND_FLAG: f32 = 0.75;
    // Containment getting WORSE is a contract violation, not a matter of
    // degree, so this flag is a tolerance for float noise rather than a
    // percentile.
    const CONTAIN_FLAG: f32 = 0.01;
    // Reported for every chassis rather than flagged: the fleet-wide
    // distribution is the finding here, not a handful of outliers.
    const EXPOSURE_FLAG: f32 = 0.90;

    // `--worst N` turns the audit into a work queue: the chassis whose
    // vehicle mode shows the most robot, worst first, with the limb to
    // look at. "It is art, not code" is only useful with a list attached.
    let worst_n: Option<usize> = args
        .iter()
        .position(|a| a == "--worst")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok());
    let mut ranked: Vec<(String, Finding)> = Vec::new();

    let (mut checked, mut ex_hits, mut isl_hits, mut con_hits) = (0, 0, 0, 0);
    let mut ex_all: Vec<f32> = Vec::new();
    let mut isl_all: Vec<f32> = Vec::new();
    let mut exp_all: Vec<f32> = Vec::new();
    let mut expp_all: Vec<f32> = Vec::new();
    let mut exps_all: Vec<f32> = Vec::new();
    for name in &names {
        let dir = root.join(name);
        let poses = poses_of(&dir);
        if poses.len() < 2 {
            continue;
        }
        let Some(f) = audit(&poses) else { continue };
        checked += 1;
        ex_all.push(f.worst_excursion);
        isl_all.push(f.island);
        exp_all.push(f.exposure);
        expp_all.push(f.exposure_packed);
        exps_all.push(f.exposure_silhouette);
        if worst_n.is_some() {
            ranked.push((name.clone(), f));
            continue;
        }
        let fe = f.worst_excursion > EXCURSION_FLAG;
        let fi = f.island > ISLAND_FLAG;
        let fc = f.contain_regress > CONTAIN_FLAG;
        let fx = f.exposure > EXPOSURE_FLAG;
        if fc {
            con_hits += 1;
        }
        if fe {
            ex_hits += 1;
        }
        if fi {
            isl_hits += 1;
        }
        if fe || fi || fc || fx {
            println!(
                "{name:<52} exposure={:.0}%->{:.0}%{} excursion={:.2}{} island={:.2}{} contain={:+.2}{}",
                f.exposure * 100.0,
                f.exposure_packed * 100.0,
                if fx { " <" } else { "  " },
                f.worst_excursion,
                if fe { " <" } else { "  " },
                f.island,
                if fi { " <" } else { "  " },
                f.contain_regress,
                if fc { " <" } else { "" },
            );
        }
    }
    if let Some(n) = worst_n {
        ranked.sort_by(|a, b| {
            b.1.exposure_packed
                .partial_cmp(&a.1.exposure_packed)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        println!("REMODEL FIRST — most robot visible in vehicle mode\n");
        println!("  NOTE: \"limb\" here is whatever skeleton::skeletonize separates from");
        println!("  the core, which is not the same as \"robot part\". On a plain car its");
        println!("  wheels and wings are limbs and are SUPPOSED to be visible, and a");
        println!("  chassis whose vehicle pose is already a robot scores high by");
        println!("  definition. Read this list with the render open, not instead of it.\n");
        for (name, f) in ranked.iter().take(n) {
            println!(
                "  {:<52} {:.0}% visible   worst limb: prim {} ({:.0}% of it)",
                name, f.exposure_packed * 100.0, f.worst_limb_prim, f.worst_limb_share * 100.0
            );
        }
        println!("\nREFERENCES — these already read as vehicles\n");
        for (name, f) in ranked.iter().rev().take(n) {
            println!("  {:<52} {:.0}% visible", name, f.exposure_packed * 100.0);
        }
        println!("\n{} chassis ranked", ranked.len());
        return;
    }

    // Percentiles, because a flag count only means something next to the
    // distribution it came from. A threshold picked before seeing the data
    // is a guess.
    let pct = |v: &mut Vec<f32>, q: f32| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        v[((v.len() - 1) as f32 * q) as usize]
    };
    println!(
        "\nexcursion  median {:.2}  p90 {:.2}  max {:.2}",
        pct(&mut ex_all, 0.5), pct(&mut ex_all, 0.9), pct(&mut ex_all, 1.0)
    );
    println!(
        "exposure   median {:.0}%  p10 {:.0}%  min {:.0}%",
        pct(&mut exp_all, 0.5) * 100.0, pct(&mut exp_all, 0.1) * 100.0, pct(&mut exp_all, 0.0) * 100.0
    );
    println!(
        "packed     median {:.0}%  p10 {:.0}%  min {:.0}%   (exposure after fold_pack)",
        pct(&mut expp_all, 0.5) * 100.0, pct(&mut expp_all, 0.1) * 100.0, pct(&mut expp_all, 0.0) * 100.0
    );
    println!(
        "silhouette median {:.0}%  p10 {:.0}%  min {:.0}%   (shift chosen to minimise visible samples)",
        pct(&mut exps_all, 0.5) * 100.0, pct(&mut exps_all, 0.1) * 100.0, pct(&mut exps_all, 0.0) * 100.0
    );
    println!(
        "island     median {:.2}  p90 {:.2}  max {:.2}",
        pct(&mut isl_all, 0.5), pct(&mut isl_all, 0.9), pct(&mut isl_all, 1.0)
    );
    println!("\n{ex_hits} with a part leaving the body mid-fold, {isl_hits} with a part floating free at rest, {con_hits} where containment made the overhang worse, of {checked} checked");
}

#[allow(dead_code)]
fn dump_colors(dir: &Path) {
    let raws = model::load_raw_unoriented(dir);
    let mut seen: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for p in &raws {
        let k = format!("{:.2},{:.2},{:.2}", p.color[0], p.color[1], p.color[2]);
        let e = seen.entry(k).or_insert((0, 0));
        e.0 += 1;
        e.1 += p.verts.len();
    }
    println!("{} prims, {} distinct colours:", raws.len(), seen.len());
    for (k, (n, v)) in seen {
        println!("   {k}   {n} prims, {v} verts");
    }
    let tex: std::collections::BTreeSet<&str> =
        raws.iter().filter_map(|p| p.tex_uri.as_deref()).collect();
    println!("   textures: {:?}", tex);
}

/// Does this vehicle have somewhere to PUT its robot?
///
/// Prompted by the observation that Blackout hides his body inside the
/// hangar bay and engine housing — the robot is not tucked against the
/// helicopter, it is inside it. That suggests a different question from
/// "how much is showing": does the hull enclose enough empty space to
/// hold the limbs at all?
///
/// If the cavity is smaller than the limbs, no stowing algorithm can
/// work and the asset needs remodelling. If it is larger, hiding is a
/// PLACEMENT problem and code can help. That is the split this measures,
/// and it is the one that decides whether `fold_pack` is worth improving
/// for a given chassis.
///
/// ## Frame
///
/// Axes come from the running gear, per the suggestion: the wheel prims
/// give a contact plane (their lowest extent) and an axle direction
/// (front group to back group), which is a more reliable "which way is
/// this vehicle standing" than a bounding box. Rotors and thrusters would
/// serve the same purpose on an aircraft; `hardpoints::find` already
/// locates thrusters and a cockpit, and this uses the wheel tags the
/// loader assigns.
///
/// ## Cavity
///
/// Voxelise the hull, flood-fill the empty cells from the outside, and
/// whatever empty space the fill cannot reach is enclosed interior. That
/// is the hangar bay.
fn cavity_report(dir: &Path) {
    let (raws, _half) = model::load_raw(dir, 4.2);
    if raws.is_empty() {
        println!("no prims");
        return;
    }
    let is_wheel = |t: model::Tag| matches!(t, model::Tag::WheelFront | model::Tag::WheelBack);

    let pts_of = |p: &model::RawPrim| -> Vec<Vec3> {
        p.verts.iter().map(|v| Vec3::from_array(v.pos)).collect()
    };
    let all: Vec<Vec3> = raws.iter().flat_map(|p| pts_of(p)).collect();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &all {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }

    // Running gear: the contact plane and the axle span.
    let wheels: Vec<Vec3> = raws.iter().filter(|p| is_wheel(p.tag)).flat_map(pts_of).collect();
    let ground = if wheels.is_empty() {
        lo.y
    } else {
        wheels.iter().fold(f32::MAX, |a, p| a.min(p.y))
    };
    let axle_span = if wheels.is_empty() {
        hi.z - lo.z
    } else {
        let zlo = wheels.iter().fold(f32::MAX, |a, p| a.min(p.z));
        let zhi = wheels.iter().fold(f32::MIN, |a, p| a.max(p.z));
        zhi - zlo
    };

    // Voxelise. 40^3 over the hull is coarse enough to run in a moment
    // and fine enough that a cabin-sized void survives.
    const G: usize = 64;
    let size = (hi - lo).max(Vec3::splat(1e-4));
    let cell = size / G as f32;
    let idx = |x: usize, y: usize, z: usize| (z * G + y) * G + x;
    let mut solid = vec![false; G * G * G];
    let mut limb_solid = vec![false; G * G * G];

    let refs_pts: Vec<Vec<Vec3>> = raws.iter().map(pts_of).collect();
    let refs: Vec<&[Vec3]> = refs_pts.iter().map(|v| v.as_slice()).collect();
    let sk = skeleton::skeletonize(&refs);
    let limb_prims: std::collections::BTreeSet<usize> =
        sk.limbs.iter().flat_map(|l| l.prims.iter().copied()).collect();

    // TRIANGLES, NOT VERTICES.
    //
    // The first version marked cells containing a vertex. On a coarse
    // hull that leaves the shell full of holes, the flood fill leaks
    // inside, and every vehicle reports a cavity of ~15 cells out of
    // 64000 — "no interior space", which is an artifact of the
    // voxelisation rather than a fact about the asset. A watertight-ish
    // shell needs the triangle FACES rasterised.
    //
    // Sampled barycentrically at a density set by the triangle's size in
    // cells, so a large face cannot step over one.
    let mut mark = |p: Vec3, limb: bool| {
        let c = ((p - lo) / cell).floor();
        let (x, y, z) = (
            (c.x.max(0.0) as usize).min(G - 1),
            (c.y.max(0.0) as usize).min(G - 1),
            (c.z.max(0.0) as usize).min(G - 1),
        );
        if limb {
            limb_solid[idx(x, y, z)] = true;
        } else {
            solid[idx(x, y, z)] = true;
        }
    };
    for (i, prim) in raws.iter().enumerate() {
        let limb = limb_prims.contains(&i);
        let vp: Vec<Vec3> = prim.verts.iter().map(|v| Vec3::from_array(v.pos)).collect();
        for t in prim.idx.chunks_exact(3) {
            let (a, b, c) = (
                vp[t[0] as usize],
                vp[t[1] as usize],
                vp[t[2] as usize],
            );
            let longest = (b - a).length().max((c - b).length()).max((a - c).length());
            let steps = ((longest / cell.min_element().max(1e-6)).ceil() as usize).clamp(1, 24);
            for u in 0..=steps {
                for v in 0..=(steps - u) {
                    let (fu, fv) = (u as f32 / steps as f32, v as f32 / steps as f32);
                    mark(a + (b - a) * fu + (c - a) * fv, limb);
                }
            }
        }
    }

    // Flood fill the empty space from the boundary. Anything empty the
    // fill never reaches is enclosed by hull.
    let mut outside = vec![false; G * G * G];
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    for a in 0..G {
        for b in 0..G {
            for (x, y, z) in [
                (0, a, b), (G - 1, a, b),
                (a, 0, b), (a, G - 1, b),
                (a, b, 0), (a, b, G - 1),
            ] {
                if !solid[idx(x, y, z)] && !outside[idx(x, y, z)] {
                    outside[idx(x, y, z)] = true;
                    stack.push((x, y, z));
                }
            }
        }
    }
    while let Some((x, y, z)) = stack.pop() {
        let mut push = |nx: usize, ny: usize, nz: usize, st: &mut Vec<_>, out: &mut Vec<bool>| {
            if !solid[idx(nx, ny, nz)] && !out[idx(nx, ny, nz)] {
                out[idx(nx, ny, nz)] = true;
                st.push((nx, ny, nz));
            }
        };
        if x > 0 { push(x - 1, y, z, &mut stack, &mut outside); }
        if x + 1 < G { push(x + 1, y, z, &mut stack, &mut outside); }
        if y > 0 { push(x, y - 1, z, &mut stack, &mut outside); }
        if y + 1 < G { push(x, y + 1, z, &mut stack, &mut outside); }
        if z > 0 { push(x, y, z - 1, &mut stack, &mut outside); }
        if z + 1 < G { push(x, y, z + 1, &mut stack, &mut outside); }
    }

    let mut cavity = 0usize;
    let mut cavity_above_ground = 0usize;
    let mut limb_cells = 0usize;
    let mut limb_in_cavity = 0usize;
    let gy = (((ground - lo.y) / cell.y).floor() as usize).min(G - 1);
    for z in 0..G {
        for y in 0..G {
            for x in 0..G {
                let i = idx(x, y, z);
                let enclosed = !solid[i] && !outside[i];
                if enclosed {
                    cavity += 1;
                    if y >= gy {
                        cavity_above_ground += 1;
                    }
                }
                if limb_solid[i] {
                    limb_cells += 1;
                    if enclosed {
                        limb_in_cavity += 1;
                    }
                }
            }
        }
    }

    // LIMB VOLUME, not limb surface.
    //
    // `limb_cells` counts rasterised FACES. Comparing that to a cavity
    // measured as empty enclosed VOLUME is apples to oranges and would
    // overstate what the limbs need. Fill the limb shells the same way
    // the hull was filled: flood from the boundary treating limb surfaces
    // as solid, and anything unreached is limb interior.
    let mut limb_outside = vec![false; G * G * G];
    let mut st: Vec<(usize, usize, usize)> = Vec::new();
    for a in 0..G {
        for b in 0..G {
            for (x, y, z) in [
                (0, a, b), (G - 1, a, b),
                (a, 0, b), (a, G - 1, b),
                (a, b, 0), (a, b, G - 1),
            ] {
                if !limb_solid[idx(x, y, z)] && !limb_outside[idx(x, y, z)] {
                    limb_outside[idx(x, y, z)] = true;
                    st.push((x, y, z));
                }
            }
        }
    }
    while let Some((x, y, z)) = st.pop() {
        let mut push = |nx: usize, ny: usize, nz: usize, s: &mut Vec<_>, o: &mut Vec<bool>| {
            if !limb_solid[idx(nx, ny, nz)] && !o[idx(nx, ny, nz)] {
                o[idx(nx, ny, nz)] = true;
                s.push((nx, ny, nz));
            }
        };
        if x > 0 { push(x - 1, y, z, &mut st, &mut limb_outside); }
        if x + 1 < G { push(x + 1, y, z, &mut st, &mut limb_outside); }
        if y > 0 { push(x, y - 1, z, &mut st, &mut limb_outside); }
        if y + 1 < G { push(x, y + 1, z, &mut st, &mut limb_outside); }
        if z > 0 { push(x, y, z - 1, &mut st, &mut limb_outside); }
        if z + 1 < G { push(x, y, z + 1, &mut st, &mut limb_outside); }
    }
    let limb_volume = (0..G * G * G)
        .filter(|i| limb_solid[*i] || !limb_outside[*i])
        .count();

    let cellv = 1.0 / (G * G * G) as f32;
    println!("{}", dir.display());
    println!("  wheels: {} verts, contact plane y={:.3}, axle span {:.3}",
        wheels.len(), ground, axle_span);
    println!("  hull cells {}  cavity {} ({:.1}% of the box, {:.1}% of it above the wheels)",
        solid.iter().filter(|s| **s).count(),
        cavity,
        cavity as f32 * cellv * 100.0,
        if cavity > 0 { cavity_above_ground as f32 / cavity as f32 * 100.0 } else { 0.0 });
    println!("  limb surface {}  limb VOLUME {}  already inside the cavity: {} ({:.0}%)",
        limb_cells, limb_volume, limb_in_cavity,
        if limb_cells > 0 { limb_in_cavity as f32 / limb_cells as f32 * 100.0 } else { 0.0 });
    println!("  cavity/limb volume ratio: {:.2}", cavity_above_ground as f32 / limb_volume.max(1) as f32);
    let verdict = if limb_cells == 0 {
        "no limbs detected"
    } else if cavity_above_ground >= limb_volume {
        "FITS — the hull encloses more space than the limbs occupy; hiding is a placement problem"
    } else {
        "DOES NOT FIT — the cavity is smaller than the limbs; no stow can hide them"
    };
    println!("  {verdict}");
}

/// How many chassis does `align::unify` decline, and therefore fall back
/// to the popping step-through?
///
/// `unify`'s doc records the contract: "On refusal (`None`) the caller
/// keeps stepping, which is today's behaviour". Stepping means the
/// transform jumps pose to pose instead of blending, so a refusal is a
/// visibly worse transform — and nothing reports how often it happens.
fn unify_report(root: &Path) {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let (mut ok, mut refused, mut skipped) = (0, 0, 0);
    let mut refusals: Vec<String> = Vec::new();
    for name in &names {
        let poses = poses_of(&root.join(name));
        if poses.len() < 2 {
            skipped += 1;
            continue;
        }
        let strip: Vec<Vec<model::RawPrim>> =
            poses.iter().map(|d| model::load_raw_unoriented(d)).collect();
        if strip.iter().any(|p| p.is_empty()) {
            skipped += 1;
            continue;
        }
        if align::unify(strip).is_some() {
            ok += 1;
        } else {
            refused += 1;
            refusals.push(name.clone());
        }
    }
    println!("unify: {ok} blended, {refused} REFUSED (stepping fallback), {skipped} skipped\n");
    for r in refusals.iter().take(40) {
        println!("  refused: {r}");
    }
    if refusals.len() > 40 {
        println!("  ... and {} more", refusals.len() - 40);
    }
}
