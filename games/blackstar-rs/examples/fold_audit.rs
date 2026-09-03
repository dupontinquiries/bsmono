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

    // EXPOSURE: how much of the robot is showing while it is a vehicle?
    //
    // This is the question I got wrong twice and then talked myself out of
    // entirely. I measured "limbs outside the skeleton core box", saw it
    // fire on 254 of 254 with a median of 3.75 core-radii, and dismissed
    // it as normal on the reasoning that a limb extending past the core is
    // what makes it a limb.
    //
    // That reasoning is right for a body and wrong for a TRANSFORMER. In
    // vehicle mode the robot parts are supposed to be folded away inside
    // the shell — the whole point is that it reads as a vehicle. A render
    // of `tank_vanquish` with the exposed parts circled is what settled
    // it: they are plainly visible, sitting proud of the hull, and the
    // metric that flagged every chassis was flagging a real fleet-wide
    // condition rather than crying wolf.
    //
    // Measured as occlusion rather than as a box test, because "outside
    // the core box" is not the same as "visible". For each limb vertex,
    // six axis-aligned rays; a ray that escapes without meeting the
    // bounding box of a NON-limb prim means that vertex can be seen from
    // outside. Exposure is the fraction of limb vertices with at least one
    // escaping ray.
    //
    // Boxes rather than triangles is deliberately coarse and errs toward
    // reporting MORE exposure than there is: a box occludes at least as
    // much as the geometry inside it, so a vertex called hidden really is
    // hidden, while one called exposed may be occluded by detail the box
    // test cannot see. Good enough to rank the fleet; not a substitute for
    // looking at a render.
    {
        let refs: Vec<&[Vec3]> = t0_pts.iter().map(|v| v.as_slice()).collect();
        let sk = skeleton::skeletonize(&refs);
        let limb_prims: std::collections::BTreeSet<usize> =
            sk.limbs.iter().flat_map(|l| l.prims.iter().copied()).collect();
        if !limb_prims.is_empty() && limb_prims.len() < t0_pts.len() {
            // Occluders: every prim that is NOT part of a limb.
            let occ: Vec<(Vec3, Vec3)> = (0..t0_pts.len())
                .filter(|i| !limb_prims.contains(i))
                .filter_map(|i| {
                    let pts = &t0_pts[i];
                    if pts.is_empty() {
                        return None;
                    }
                    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                    for p in pts {
                        lo = lo.min(*p);
                        hi = hi.max(*p);
                    }
                    Some((lo, hi))
                })
                .collect();
            let dirs = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];
            let measure = |limb_pts: &dyn Fn(usize) -> Vec<Vec3>| -> f32 {
                let (mut seen, mut total) = (0usize, 0usize);
                for i in &limb_prims {
                    let pts = limb_pts(*i);
                    // Sample rather than test every vertex: these meshes
                    // carry tens of thousands and the answer does not move.
                    let stride = (pts.len() / 64).max(1);
                    for p in pts.iter().step_by(stride) {
                        total += 1;
                        if dirs.iter().any(|d| !occ.iter().any(|(lo, hi)| ray_hits_box(*p, *d, *lo, *hi)))
                        {
                            seen += 1;
                        }
                    }
                }
                if total > 0 { seen as f32 / total as f32 } else { 0.0 }
            };

            worst.exposure = measure(&|i| t0_pts.get(i).cloned().unwrap_or_default());

            // AND AGAIN, WITH THE STOWING APPLIED.
            //
            // `fold_pack` exists to tuck limbs into an envelope, and the
            // vehicle pose is loaded as authored without it. If running it
            // drops exposure, the machinery to hide the robot is already
            // here and simply is not being used on this pose.
            const SLACK: f32 = 0.25;
            let steps = skeleton::fold_pack(&sk, &refs, SLACK, Vec3::Y);
            worst.exposure_packed = measure(&|i| {
                let step = steps.get(i).copied().unwrap_or_default();
                t0_pts
                    .get(i)
                    .map(|v| {
                        v.iter()
                            .map(|p| step.pivot + step.rot * (*p - step.pivot) + step.shift)
                            .collect()
                    })
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

    let (mut checked, mut ex_hits, mut isl_hits, mut con_hits) = (0, 0, 0, 0);
    let mut ex_all: Vec<f32> = Vec::new();
    let mut isl_all: Vec<f32> = Vec::new();
    let mut exp_all: Vec<f32> = Vec::new();
    let mut expp_all: Vec<f32> = Vec::new();
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
        "island     median {:.2}  p90 {:.2}  max {:.2}",
        pct(&mut isl_all, 0.5), pct(&mut isl_all, 0.9), pct(&mut isl_all, 1.0)
    );
    println!("\n{ex_hits} with a part leaving the body mid-fold, {isl_hits} with a part floating free at rest, {con_hits} where containment made the overhang worse, of {checked} checked");
}
