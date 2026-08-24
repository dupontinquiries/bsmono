//! ORIENT SIM — walk the bot roster, diagnose, and fix `orient.json`.
//!
//!     skydribble --orient list                    every bot family found
//!     skydribble --orient check <bot>              all poses, guess + confidence
//!     skydribble --orient show <bot> <pose>        ASCII front view + guess
//!     skydribble --orient fix <bot> [--commit]     write real orient.json
//!     skydribble --orient sweep [--commit] [--low-confidence-only]
//!
//! Dry-run by default, same convention as the raid/campaign sims —
//! `--commit` is required to actually touch a file on disk.

use std::path::{Path, PathBuf};

use crate::orient;

fn bots_dir() -> PathBuf {
    let local = PathBuf::from("assets/bots");
    if local.exists() {
        local
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/bots")
    }
}

/// Every pose directory under a bot family — the fold frames (f00..)
/// and walk frames (w00..), in the same order the shell loads them.
fn poses(bot_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(bot_dir) else { return out };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with('f') || n.starts_with('w'))
        .collect();
    names.sort();
    for n in names {
        out.push(bot_dir.join(n));
    }
    out
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--orient").unwrap();
    let cmd = argv.get(i + 1).map(|s| s.as_str());
    let commit = argv.iter().any(|a| a == "--commit");
    let low_only = argv.iter().any(|a| a == "--low-confidence-only");

    match cmd {
        Some("list") => list(),
        Some("check") => {
            let Some(bot) = argv.get(i + 2) else {
                println!("usage: --orient check <bot>");
                return;
            };
            check(bot);
        }
        Some("show") => {
            let (Some(bot), Some(pose)) = (argv.get(i + 2), argv.get(i + 3)) else {
                println!("usage: --orient show <bot> <pose>");
                return;
            };
            show(bot, pose);
        }
        Some("scores") => {
            let (Some(bot), Some(pose)) = (argv.get(i + 2), argv.get(i + 3)) else {
                println!("usage: --orient scores <bot> <pose>");
                return;
            };
            scores(bot, pose);
        }
        Some("fix") => {
            let Some(bot) = argv.get(i + 2) else {
                println!("usage: --orient fix <bot> [--commit]");
                return;
            };
            fix_bot(bot, commit);
        }
        Some("sweep") => sweep(commit, low_only, argv.iter().any(|a| a == "--up-only"), argv.iter().any(|a| a == "--full-only")),
        Some("reset") => reset(commit),
        _ => println!("usage: --orient [list|check <bot>|show <bot> <pose>|fix <bot>|sweep] [--commit]"),
    }
}

fn list() {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else {
        println!("no assets/bots directory found");
        return;
    };
    let mut names: Vec<String> =
        rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    println!("{} bot families", names.len());
    for n in names {
        let n_poses = poses(&bots_dir().join(&n)).len();
        println!("  {n:32} {n_poses} poses");
    }
}

fn guess_for(pose_dir: &Path) -> Option<(orient::Guess, Vec<crate::meshgen::Vertex>)> {
    if !pose_dir.join("scene.gltf").exists() && !pose_dir.with_extension("glb").exists() {
        return None;
    }
    let raws = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::model::load_raw_unoriented(pose_dir))).ok()?;
    // SAY WHAT THE FILTER DID, on real assets, where my synthetic fixtures
    // cannot reach. `massfilter`'s thresholds are reasoned rather than
    // fitted, so the only way to judge them is to print what they reject
    // per pose and look — which is what this driver is for.
    {
        let pts: Vec<Vec<glam::Vec3>> = raws
            .iter()
            .map(|r| r.verts.iter().map(|v| glam::Vec3::from_array(v.pos)).collect())
            .collect();
        let refs: Vec<&[glam::Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
        let m = crate::massfilter::classify(&refs);
        if m.dropped > 0 {
            println!("       {}", m.report());
            if let (Some(a), Some(b)) = (
                crate::massfilter::bounds(&refs, &vec![true; refs.len()]),
                crate::massfilter::bounds(&refs, &m.keep),
            ) {
                println!(
                    "       ride_h {:.2} -> {:.2}, half.y {:.2} -> {:.2}",
                    a.1, b.1, a.0.y, b.0.y
                );
            }
        }
    }
    // WHAT THE BODY WILL ACTUALLY SIT ON. Two rounds of guessing at the
    // feet have now cost more than measuring would have, so this reports
    // the seating numbers per pose: the loader's `ride_h` (how far the
    // lowest point hangs below the origin), `half_h`, and the gap between
    // them — which is exactly how far a body sinks when a clearance is
    // taken from the box instead of from where it stands.
    {
        let pts: Vec<Vec<glam::Vec3>> = raws
            .iter()
            .map(|r| r.verts.iter().map(|v| glam::Vec3::from_array(v.pos)).collect())
            .collect();
        let refs: Vec<&[glam::Vec3]> = pts.iter().map(|v| v.as_slice()).collect();
        let mass = crate::massfilter::classify(&refs);
        if let Some((half, ride)) = crate::massfilter::bounds(&refs, &mass.keep) {
            let sink = ride - half.y;
            println!(
                "       seat: ride_h {:.2}  half_h {:.2}  {}",
                ride,
                half.y,
                if sink.abs() < 0.02 {
                    "centred (box clearance would be right by luck)".to_string()
                } else if sink > 0.0 {
                    format!("origin is {sink:.2} ABOVE centre — a box clearance buries the feet by that much")
                } else {
                    format!("origin is {:.2} below centre — a box clearance floats it", -sink)
                }
            );
        }
        let sk = crate::skeleton::skeletonize(&refs);
        println!("       {}", sk.report());
        let f = crate::turret::find(&refs);
        if f.has_turret() || f.tracked() || !f.rollers.is_empty() {
            println!("       {}, {} rollers", f.report(), f.rollers.len());
        }
    }
    let verts = orient::all_verts(&raws);
    if verts.is_empty() {
        return None;
    }
    let g = orient::guess(&verts);
    Some((g, verts))
}

fn check(bot: &str) {
    let dir = bots_dir().join(bot);
    let ps = poses(&dir);
    if ps.is_empty() {
        println!("no poses found under {:?}", dir);
        return;
    }
    println!("{bot}: {} poses", ps.len());
    // WHAT THE LOADER WILL ACTUALLY DO. The per-pose lines below are
    // `orient::guess` — a second opinion, useful for seeing disagreement,
    // but not the frame the game applies. The game takes ONE frame per
    // chassis (declared sidecar, else `model::strip_guess` from the
    // vehicle pose) and rotates every pose by it, so a printout that only
    // showed the per-pose guesses could look broken when the loader was
    // fine, or fine when it was broken.
    match crate::model::loader_frame(&ps[0]) {
        Some((q, src)) => {
            let up = q.inverse() * glam::Vec3::Y;
            let fwd = q.inverse() * glam::Vec3::NEG_Z;
            println!(
                "  LOADER applies one frame to all poses ({src}): asset up={} fwd={}",
                orient::axis_name(up),
                orient::axis_name(fwd)
            );
        }
        None => println!("  LOADER falls back to a per-pose guess (not a strip)"),
    }
    for p in &ps {
        let name = p.file_name().unwrap().to_string_lossy();
        match guess_for(p) {
            Some((g, _)) => println!(
                "  {name:4} up={} (conf={}) fwd={} (conf={})",
                orient::axis_name(g.up),
                g.up_confident,
                orient::axis_name(g.forward),
                g.forward_confident
            ),
            None => println!("  {name:4} (could not load)"),
        }
    }
}

fn show(bot: &str, pose: &str) {
    let dir = bots_dir().join(bot).join(pose);
    let Some((g, verts)) = guess_for(&dir) else {
        println!("could not load {:?}", dir);
        return;
    };
    println!(
        "{bot}/{pose} -- guess: up={} fwd={} confident={}",
        orient::axis_name(g.up),
        orient::axis_name(g.forward),
        g.confident()
    );
    print!("{}", orient::ascii_front(&verts, g.up, g.forward));
}

fn fix_bot(bot: &str, commit: bool) {
    let dir = bots_dir().join(bot);
    let ps = poses(&dir);
    if ps.is_empty() {
        println!("no poses found under {:?}", dir);
        return;
    }
    let mut written = 0;
    let mut skipped = 0;
    for p in &ps {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        match guess_for(p) {
            Some((g, _)) => {
                if commit {
                    match orient::write_orient_json(p, g.up, g.forward, "orient_heuristic_v1") {
                        Ok(()) => written += 1,
                        Err(e) => println!("  {name}: write failed: {e}"),
                    }
                } else {
                    written += 1;
                }
                println!(
                    "  {name:4} -> up={} fwd={} confident={}{}",
                    orient::axis_name(g.up),
                    orient::axis_name(g.forward),
                    g.confident(),
                    if commit { "" } else { " (dry run)" }
                );
            }
            None => {
                skipped += 1;
                println!("  {name:4} could not load, left untouched");
            }
        }
    }
    println!("{bot}: {written} written, {skipped} skipped{}", if commit { "" } else { " -- add --commit to write" });
}

fn sweep(commit: bool, low_only: bool, up_only: bool, full_only: bool) {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else {
        println!("no assets/bots directory found");
        return;
    };
    let mut names: Vec<String> =
        rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    let (mut total, mut written, mut low_conf, mut failed) = (0, 0, 0, 0);
    let mut up_conf = 0;
    for name in &names {
        for p in poses(&bots_dir().join(name)) {
            total += 1;
            match guess_for(&p) {
                Some((g, _)) => {
                    if !g.confident() {
                        low_conf += 1;
                    }
                    if g.up_confident {
                        up_conf += 1;
                    }
                    if low_only && !g.confident() {
                        continue;
                    }
                    if up_only && !g.up_confident {
                        continue;
                    }
                    if full_only && !g.confident() {
                        continue;
                    }
                    if commit {
                        // up-only mode says so in the tag: forward on
                        // these poses is still a guess, not verified,
                        // and the tag is how a later pass finds them
                        let src = if full_only {
                            "orient_heuristic_v1"
                        } else if up_only {
                            "orient_heuristic_v1_up_only"
                        } else {
                            "orient_heuristic_v1_unverified"
                        };
                        if orient::write_orient_json(&p, g.up, g.forward, src).is_ok() {
                            written += 1;
                        } else {
                            failed += 1;
                        }
                    } else {
                        written += 1;
                    }
                }
                None => failed += 1,
            }
        }
    }
    println!(
        "{} families, {total} poses -- {written} {}, {up_conf} up-axis-confident, {low_conf} low-confidence overall, {failed} failed to load{}",
        names.len(),
        if commit { "written" } else { "would write" },
        if commit { "" } else { " (add --commit to write)" }
    );
}

/// Restore every `orient.json` under the whole roster to the original
/// stamped placeholder — a clean baseline before a fresh sweep, so a
/// stale write from an earlier, less-correct pass of this heuristic
/// can never survive under a tag that claims it was checked. Caught
/// exactly this: `ironforge/w02` and `w03` carried
/// `orient_heuristic_v1_up_only` from a run before `perp_pair`'s
/// same-axis bug was fixed, and the corrected heuristic no longer
/// calls them up-confident — a plain re-sweep skips writing them
/// (nothing changed to write), which silently LEAVES the stale,
/// wrongly-confident file in place instead of reverting it.
fn reset(commit: bool) {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else {
        println!("no assets/bots directory found");
        return;
    };
    let mut names: Vec<String> =
        rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    let mut n = 0;
    for name in &names {
        for p in poses(&bots_dir().join(name)) {
            if commit {
                let _ = orient::write_orient_json(&p, glam::Vec3::Y, -glam::Vec3::Z, "foldview");
            }
            n += 1;
        }
    }
    println!(
        "{n} poses {}{}",
        if commit { "reset to the original placeholder" } else { "would be reset" },
        if commit { "" } else { " -- add --commit" }
    );
}

fn scores(bot: &str, pose: &str) {
    let dir = bots_dir().join(bot).join(pose);
    let Some((_, verts)) = guess_for(&dir) else {
        println!("could not load {:?}", dir);
        return;
    };
    println!("{bot}/{pose} -- up_score per axis:");
    for ax in orient::AXES {
        println!("  {:4} {:.4}", orient::axis_name(ax), orient::up_score(&verts, ax));
    }
    println!("mirror asymmetry per horizontal axis (lower = more symmetric):");
    let g = orient::guess(&verts);
    let (a, b) = orient::perp_pair(g.up);
    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for v in &verts {
        let p = glam::Vec3::from_array(v.pos);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let center = (lo + hi) * 0.5;
    println!("  {:4} {:.4}", orient::axis_name(a), orient::mirror_asymmetry(&verts, a, center));
    println!("  {:4} {:.4}", orient::axis_name(b), orient::mirror_asymmetry(&verts, b, center));
}


