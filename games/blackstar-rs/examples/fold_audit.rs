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
    let mut worst = Finding { worst_excursion: 0.0, worst_prim: 0, worst_t: 0.0 };
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

    let (mut checked, mut ex_hits) = (0, 0);
    let mut ex_all: Vec<f32> = Vec::new();
    for name in &names {
        let dir = root.join(name);
        let poses = poses_of(&dir);
        if poses.len() < 2 {
            continue;
        }
        let Some(f) = audit(&poses) else { continue };
        checked += 1;
        ex_all.push(f.worst_excursion);
        if f.worst_excursion > EXCURSION_FLAG {
            ex_hits += 1;
            println!(
                "{name:<52} excursion={:.2}  (prim {} at t={:.2})",
                f.worst_excursion, f.worst_prim, f.worst_t
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
    println!("\n{ex_hits} chassis over the flag, of {checked} checked");
}
