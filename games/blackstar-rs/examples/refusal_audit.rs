// Why a chassis in fleet.json's "refused" list is refused, in its own
// words. `stripcheck.rs` answers accepted-or-not; this decodes the real
// pack data through the real loader (`model::load_raw_unoriented`, same
// path `main.rs` uses at runtime) and, for anything that DOES build a
// `Morph`, runs `foldaudit::audit` over it to say WHERE the fold goes
// wrong — self-intersection, volume collapse, a velocity spike, a NaN —
// rather than leaving "refused" as an unexplained verdict.
//
//   refusal_audit <bots dir> <chassis> [chassis...]
include!("shim.rs");

#[path = "../src/xform.rs"]
pub mod xform;
#[path = "../src/foldaudit.rs"]
pub mod foldaudit;
#[path = "../src/align.rs"]
pub mod align;

use std::path::{Path, PathBuf};

fn pose_dirs(chassis: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(chassis)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.join("scene.gltf").is_file()
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('f'))
        })
        .collect();
    v.sort();
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: refusal_audit <bots dir> <chassis> [chassis...]");
        eprintln!("       refusal_audit <bots dir> --sweep-all");
        std::process::exit(1);
    }
    let bots_dir = Path::new(&args[1]);
    let names: Vec<String> = if args.get(2).map(String::as_str) == Some("--sweep-all") {
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
    let sweep = args.get(2).map(String::as_str) == Some("--sweep-all");
    let mut clean = 0usize;
    let mut faulty = 0usize;
    let mut no_strip = 0usize;
    let mut no_align = 0usize;
    for name in &names {
        let cdir = bots_dir.join(name);
        let dirs = pose_dirs(&cdir);
        if !sweep {
            println!("\n== {name} ({} fold poses found) ==", dirs.len());
        }
        if dirs.len() < 2 {
            if !sweep {
                println!("  fewer than 2 fold poses on disk — this is a strip-count problem,");
                println!("  not a fold-quality one; nothing to audit.");
            }
            no_strip += 1;
            continue;
        }
        // Mirrors main.rs's real build_morph exactly: load_raw_at (oriented
        // and scaled, not load_raw_unoriented) at ONE shared scale for the
        // whole strip, then align::unify to find the prim correspondence —
        // skipping either step is what produced 1800+ bogus "pop" faults on
        // the FIRST version of this tool, on chassis fleet.json calls clean.
        let raw_span = |p: &PathBuf| -> f32 {
            let (_, _, _, s, _) = model::load_raw_at(p, 1.0, false);
            if s > 1e-9 { 1.0 / s } else { 1.0 }
        };
        let span0 = raw_span(&dirs[0]);
        let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
        let mut load_failed = false;
        for d in &dirs {
            let target = 1.32 * raw_span(d) / span0.max(1e-6);
            match std::panic::catch_unwind(|| model::load_raw(d, target)) {
                Ok((raws, _)) => strip.push(raws),
                Err(_) => {
                    if !sweep {
                        println!("  {} panicked loading (missing/corrupt pack data)", d.display());
                    }
                    load_failed = true;
                }
            }
        }
        if load_failed {
            no_strip += 1;
            continue;
        }
        let diag = align::diagnose(&strip);
        let unmatched_prims = align::diagnose_unmatched(&strip);
        let Some(u) = align::unify(strip) else {
            if !sweep {
                println!("  align::unify refused — no consistent UV correspondence across");
                println!("  poses. This IS the real refusal path main.rs falls back on");
                println!("  (\"strip mismatch — stepping\"): a whole-pose step, not a blend.");
                if let Some((matched, pair_slots)) = diag {
                    let pct = if pair_slots > 0 { 100.0 * matched as f32 / pair_slots as f32 } else { 0.0 };
                    println!("  matched {matched}/{pair_slots} adjacent-pose slots ({pct:.0}%, needs 50%)");
                    println!("  {} unmatched (pose, prim) pairs: {:?}", unmatched_prims.len(), unmatched_prims);
                }
            } else {
                let pct = diag
                    .filter(|(_, ps)| *ps > 0)
                    .map(|(m, ps)| 100.0 * m as f32 / ps as f32)
                    .unwrap_or(0.0);
                println!("{name:55} UNIFY-REFUSED ({pct:.0}% matched, needs 50%)");
            }
            no_align += 1;
            continue;
        };
        match xform::Morph::build(u.poses, u.idx) {
            None => {
                if !sweep {
                    println!("  align::unify succeeded but Morph::build still refused —");
                    println!("  should not happen; unify's contract is to guarantee this shape.");
                }
                no_align += 1;
            }
            Some(mut m) => {
                // SCALE-RELATIVE TOLERANCE, matching foldsim.rs::pop_tol_for
                // exactly: a fixed absolute tolerance is meaningless across
                // chassis of very different size, and using one (my first
                // version of this tool used 0.02 flat) is what manufactured
                // thousands of false "pop" faults on a chassis fleet.json
                // calls clean.
                let half = m.half_at(0.0);
                let longest = half.x.max(half.y).max(half.z).max(0.1);
                let tol = longest * 0.25;
                let report = foldaudit::audit(&mut m, tol);
                if report.clean() {
                    clean += 1;
                } else {
                    faulty += 1;
                }
                if sweep {
                    let mut spans: std::collections::HashMap<(usize, usize), (f32, f32)> =
                        std::collections::HashMap::new();
                    for f in &report.faults {
                        if let foldaudit::Fault::SelfIntersection { a, b, t, .. } = f {
                            let e = spans.entry((*a, *b)).or_insert((*t, *t));
                            e.0 = e.0.min(*t);
                            e.1 = e.1.max(*t);
                        }
                    }
                    // LOCALIZED vs DIFFUSE. A pair whose fault window covers
                    // most of the fold is the signature of an AABB ballooning
                    // during rotation, not real mesh contact — a box rotated
                    // 45 degrees can read as overlapping something it never
                    // touches for the whole time it's turning. A pair whose
                    // window is a short, contained slice is what a part
                    // actually sweeping through another looks like. Neither
                    // is certain without eyeballing the meshes, but the split
                    // is a real, cheap triage signal this sweep didn't have.
                    let localized = spans.values().filter(|(lo, hi)| hi - lo < 0.30).count();
                    let diffuse = spans.values().filter(|(lo, hi)| hi - lo >= 0.70).count();
                    let mid = spans.len().saturating_sub(localized).saturating_sub(diffuse);
                    println!(
                        "{name:55} {} pairs: {localized} localized (suspect real), {mid} mid, {diffuse} diffuse (suspect AABB/rotation)  worst_pop={:.3}",
                        spans.len(), report.worst_pop
                    );
                    continue;
                }
                println!("  Morph built. foldaudit over the full 0..1 sweep (tol={tol:.3}):");
                println!("{}", report.summary());
                if !report.clean() {
                    let mut ts: Vec<f32> = report.faults.iter().map(|f| match f {
                        foldaudit::Fault::Pop { t, .. } => *t,
                        foldaudit::Fault::SelfIntersection { t, .. } => *t,
                        foldaudit::Fault::VolumeCollapse { t, .. } => *t,
                        foldaudit::Fault::NonFinite { t, .. } => *t,
                    }).collect();
                    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let first = *ts.first().unwrap();
                    let last = *ts.last().unwrap();
                    let at_first_step = ts.iter().filter(|t| **t <= first + 1e-6).count();
                    println!(
                        "    fault t-range [{first:.2}..{last:.2}], {at_first_step}/{} at the earliest faulting sample",
                        ts.len()
                    );
                    for f in report.faults.iter().take(3) {
                        println!("    {}", f.line());
                    }
                    // WHAT ARE THESE PRIMS. base prims (index < u.base_prims)
                    // map onto the base model's own tags in prim order —
                    // reload pose 0 the cheap way (a throwaway Gpu stub,
                    // same one shim.rs gives model::load for exactly this)
                    // to get them. Extras beyond base_prims carry their own
                    // tag in u.extras. This is the difference between "a
                    // fault at P4/P16" and "the front-left wheel clips the
                    // fender" — the second is the one worth acting on.
                    if let Some(foldaudit::Fault::SelfIntersection { a, b, .. }) =
                        report.faults.first()
                    {
                        let mut gpu = render::Gpu;
                        let base = model::load(&dirs[0], &mut gpu, 1.32);
                        let tag_of = |p: usize| -> String {
                            if p < u.base_prims {
                                base.prims.get(p).map(|pr| format!("{:?}", pr.tag)).unwrap_or("?".into())
                            } else {
                                u.extras.get(p - u.base_prims).map(|e| format!("{:?} (pose-specific)", e.tag)).unwrap_or("?".into())
                            }
                        };
                        println!("    P{a} = {}, P{b} = {}", tag_of(*a), tag_of(*b));
                    }
                }
            }
        }
    }
    if sweep {
        println!(
            "\n{} clean, {faulty} with faults, {no_strip} unloadable/under-2-poses, {no_align} unify-refused, of {} total",
            clean, names.len()
        );
    }
}
