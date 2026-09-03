// SILHOUETTE AUDIT — does the robot pose actually look like a robot, or
// a bounding box with parts stuck to it?
//
// foldaudit.rs measures the MOTION of a fold; this measures the RESULT,
// specifically the two complaints that don't show up as a self-
// intersection or a pop because nothing crosses anything else — a part
// can sit in a genuinely dumb place without ever touching another prim:
//
//   * OVERHANG — something far from the core body in a direction that
//     isn't a limb doing what limbs do (legs go down, arms go out to the
//     sides near shoulder height). A door hinging out from the shoulder
//     at the robot's full arm's-length is this even though nothing
//     clips.
//   * CROTCH CLUTTER — a non-leg part occupying the corridor between
//     the two legs, below hip height. Anatomically there is nothing
//     there on a biped design, so anything found is almost always a
//     vehicle panel that had nowhere else to fold to.
//
// Reuses the real classifier (`partdetect::classify`) and the real
// per-chassis loader/scale (`model::load_raw_at`), so a chassis this
// flags is describing what the game itself would draw, not a fixture.
//
//   silhouette_audit <bots dir> <chassis> [chassis...]
//   silhouette_audit <bots dir> --sweep-all
include!("shim.rs");

// `orient` now comes from shim.rs: `model.rs` calls
// `crate::orient::up_score`, so every harness needs it, not just this one.
// Declaring it here as well is a duplicate definition.
#[path = "../src/partdetect.rs"]
pub mod partdetect;

use glam::Vec3;
use std::path::{Path, PathBuf};

fn pose_dirs(chassis: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(chassis)
        .into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir() && p.join("scene.gltf").is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('f'))
        })
        .collect();
    v.sort();
    v
}

struct PartGeom {
    center: Vec3,
    size: Vec3, // full extents, not half
}

struct Finding {
    overhangs: Vec<(usize, f32, partdetect::Kind, PartGeom)>,
    crotch: Vec<(usize, partdetect::Kind, PartGeom)>,
    core_half: Vec3,
}

fn audit_robot_pose(name: &str, dirs: &[PathBuf]) -> Option<Finding> {
    let last = dirs.last()?;
    // real loader, real scale search main.rs itself uses — see
    // refusal_audit.rs's comment for why this matters over the
    // unoriented reader.
    let raw_span = |p: &PathBuf| -> f32 {
        let (_, _, _, s, _) = model::load_raw_at(p, 1.0, false);
        if s > 1e-9 { 1.0 / s } else { 1.0 }
    };
    let span0 = raw_span(&dirs[0]);
    let target = 1.32 * raw_span(last) / span0.max(1e-6);
    let (raws, _) = model::load_raw(last, target);
    if raws.is_empty() {
        return None;
    }

    // whole-body box, for classify's body_lo/hi/center/half_width and
    // for the overhang ratio baseline
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for r in &raws {
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    let center = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let half_width = half.x.max(half.z).max(1e-3);
    let up = Vec3::Y;

    let kinds: Vec<partdetect::Kind> = raws
        .iter()
        .map(|r| partdetect::classify(&r.verts, up, lo.y, hi.y, center, half_width))
        .collect();

    // LEG CORRIDOR: the horizontal rectangle spanned by the Leg prims'
    // own extents, from the ground up to the higher of their tops (the
    // hip line) — nothing that isn't a leg has anatomical business here.
    let leg_idxs: Vec<usize> = kinds.iter().enumerate().filter(|(_, k)| **k == partdetect::Kind::Leg).map(|(i, _)| i).collect();
    let mut crotch = Vec::new();
    // KNOWN FALSE POSITIVE, and the guard below does not fix it.
    //
    // boat_mistral_class_lhd reports 35 "crotch" findings: the classifier
    // tags hull/keel detail as Leg, and every hull panel near the
    // centreline then reads as "between the legs". That much of the
    // original diagnosis is right.
    //
    // What is NOT right is the remedy it proposed. The note here used to
    // say a "double-digit leg count" identifies the failure and should
    // skip the corridor check. Measured: the boat has FOUR leg-tagged
    // prims, not double digits, so a count threshold never excluded it and
    // `>= 2` below has only ever been a lower bound.
    //
    // Leg GEOMETRY does not separate them either. The boat's leg-tagged
    // prims span 0.157 laterally; animal_bot_runningwind, a real
    // quadruped, spans 0.182. Both measured with the same probe. A
    // separation test would reject the animal before it rejected the boat.
    //
    // So the signal a working guard needs is not in the leg prims at all —
    // it is in whether the model is a legged thing in the first place, and
    // that belongs in partdetect rather than here. Recorded rather than
    // patched, with the two candidate fixes that have already been ruled
    // out, so the next attempt starts past them.
    //
    // The overhang half of the audit is unaffected and its numbers are
    // trustworthy.
    if leg_idxs.len() >= 2 {
        let mut leg_lo = Vec3::splat(f32::MAX);
        let mut leg_hi = Vec3::splat(f32::MIN);
        let mut hip_y = f32::MIN;
        for &i in &leg_idxs {
            for v in &raws[i].verts {
                let p = Vec3::from_array(v.pos);
                leg_lo = leg_lo.min(p);
                leg_hi = leg_hi.max(p);
                hip_y = hip_y.max(p.y);
            }
        }
        // require the two legs to actually straddle the centreline —
        // otherwise "between the legs" isn't a real corridor (e.g. a
        // single centred leg/skirt misclassified) and checking would
        // just flag the torso against itself
        let straddles = leg_lo.x < center.x && leg_hi.x > center.x;
        // COMPACT, not just present. A real leg is short along the
        // body's own long axis relative to the whole body — checked
        // directly on `boat_mistral_class_lhd`: 4 "Leg"-tagged prims
        // that were actually keel/hull struts running most of the
        // ship's length. Those inflated `leg_hi.z - leg_lo.z` to
        // nearly the hull's own length, and the OLD corridor code
        // added that span again as margin, so almost every hull panel
        // fell "between the legs" by construction. A genuine leg
        // shouldn't span more than ~40% of the body's total length in
        // either horizontal axis.
        let body_len_x = (hi.x - lo.x).max(1e-3);
        let body_len_z = (hi.z - lo.z).max(1e-3);
        let leg_span_x = leg_hi.x - leg_lo.x;
        let leg_span_z = leg_hi.z - leg_lo.z;
        let compact = leg_span_x / body_len_x < 0.9 && leg_span_z / body_len_z < 0.4;
        if straddles && compact {
            for (i, k) in kinds.iter().enumerate() {
                if leg_idxs.contains(&i) || *k == partdetect::Kind::Wheel {
                    continue; // wheels near legs are running gear, expected
                }
                let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for v in &raws[i].verts {
                    let p = Vec3::from_array(v.pos);
                    plo = plo.min(p);
                    phi = phi.max(p);
                }
                // inside the leg corridor's actual X-Z footprint (no
                // invented margin) AND below the hip line AND closer to
                // the centreline than the legs' own inner edge — a part
                // that merely brushes the corridor's edge isn't clutter
                let mid = (plo + phi) * 0.5;
                let in_x = mid.x > leg_lo.x + body_len_x * 0.05 && mid.x < leg_hi.x - body_len_x * 0.05;
                let in_z = mid.z > leg_lo.z && mid.z < leg_hi.z;
                let below_hip = mid.y < hip_y;
                if in_x && in_z && below_hip {
                    crotch.push((i, *k, PartGeom { center: mid, size: phi - plo }));
                }
            }
        }
    }

    // OVERHANG, mirroring skeleton.rs's own `to_core` measurement: distance
    // from the CORE BOX'S SURFACE (clamp-to-box), not from its centre.
    // Distance-from-centre doesn't discriminate (checked: raw ratios for
    // several real chassis, clean and messy alike, all landed in a tight
    // 1.0-1.8x band with no separation) because a big torso's own centre
    // is already far from a hand at rest. Distance beyond the box surface
    // is zero for anything inside it and only grows for genuine overhang,
    // which is the number that actually answers "how far past the body."
    let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (i, r) in raws.iter().enumerate() {
        if kinds[i] != partdetect::Kind::Body {
            continue;
        }
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            clo = clo.min(p);
            chi = chi.max(p);
        }
    }
    let (core_lo, core_hi) = if clo.x.is_finite() { (clo, chi) } else { (lo, hi) };
    let core_half = (core_hi - core_lo) * 0.5;
    let core_scale = core_half.x.max(core_half.y).max(core_half.z).max(1e-3);

    let mut overhangs = Vec::new();
    for (i, r) in raws.iter().enumerate() {
        let mut far = 0.0f32;
        let (mut plo, mut phi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &r.verts {
            let p = Vec3::from_array(v.pos);
            let clamped = p.clamp(core_lo, core_hi);
            far = far.max((p - clamped).length());
            plo = plo.min(p);
            phi = phi.max(p);
        }
        let ratio = far / core_scale;
        let discount = match kinds[i] {
            partdetect::Kind::Leg | partdetect::Kind::Wheel => 1.6,
            partdetect::Kind::Thruster | partdetect::Kind::Wing => 1.3,
            partdetect::Kind::Body => 1.0,
        };
        let adj = ratio / discount;
        if adj > 0.3 {
            let geom = PartGeom { center: (plo + phi) * 0.5, size: phi - plo };
            overhangs.push((i, adj, kinds[i], geom));
        }
    }
    overhangs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let _ = name;
    Some(Finding { overhangs, crotch, core_half })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: silhouette_audit <bots dir> <chassis> [chassis...]");
        eprintln!("       silhouette_audit <bots dir> --sweep-all");
        std::process::exit(1);
    }
    let bots_dir = Path::new(&args[1]);
    let sweep = args.get(2).map(String::as_str) == Some("--sweep-all");
    let names: Vec<String> = if sweep {
        let mut v: Vec<String> = std::fs::read_dir(bots_dir)
            .into_iter().flatten().flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        v.sort();
        v
    } else {
        args[2..].to_vec()
    };
    let mut crotch_total = 0usize;
    let mut overhang_total = 0usize;
    for name in &names {
        let dirs = pose_dirs(&bots_dir.join(name));
        if dirs.len() < 2 {
            continue;
        }
        let Some(f) = std::panic::catch_unwind(|| audit_robot_pose(name, &dirs)).ok().flatten() else {
            if !sweep {
                println!("{name}: could not load/classify robot pose");
            }
            continue;
        };
        if !f.crotch.is_empty() {
            crotch_total += 1;
        }
        if !f.overhangs.is_empty() {
            overhang_total += 1;
        }
        if sweep {
            if !f.crotch.is_empty() || !f.overhangs.is_empty() {
                println!(
                    "{name:55} crotch={} overhang={}",
                    f.crotch.len(),
                    f.overhangs.len()
                );
            }
        } else {
            println!("\n== {name} == (core half-extent {:.2},{:.2},{:.2})", f.core_half.x, f.core_half.y, f.core_half.z);
            if f.crotch.is_empty() {
                println!("  no crotch clutter found");
            } else {
                for (i, k, g) in &f.crotch {
                    println!(
                        "  CROTCH: P{i} ({k:?}) at ({:.2},{:.2},{:.2}), size {:.2}x{:.2}x{:.2} — between the legs, below hip height",
                        g.center.x, g.center.y, g.center.z, g.size.x, g.size.y, g.size.z
                    );
                }
            }
            if f.overhangs.is_empty() {
                println!("  no unexplained overhang found");
            } else {
                for (i, ratio, k, g) in f.overhangs.iter().take(6) {
                    println!(
                        "  OVERHANG: P{i} ({k:?}) at ({:.2},{:.2},{:.2}), size {:.2}x{:.2}x{:.2} — reaches {ratio:.2}x past the core, more than its kind explains",
                        g.center.x, g.center.y, g.center.z, g.size.x, g.size.y, g.size.z
                    );
                }
            }
        }
    }
    if sweep {
        println!(
            "\n{crotch_total} chassis with crotch clutter, {overhang_total} with unexplained overhang, of {} checked",
            names.len()
        );
    }
}
