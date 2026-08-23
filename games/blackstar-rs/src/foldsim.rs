//! FOLD SIM — run the fold audit against real chassis, headless.
//!
//!     skydribble --fold list                  every chassis with a strip
//!     skydribble --fold check <bot>           audit one fold
//!     skydribble --fold check <bot> --compact  audit it with compaction on
//!     skydribble --fold sweep [--compact]     audit the whole roster
//!
//! No window and no GPU: the audit is pure geometry over CPU-side
//! vertices, which is the whole reason it can run in CI where a
//! viewport check cannot.

use std::path::{Path, PathBuf};

use crate::foldaudit;
use crate::meshgen::Vertex;
use crate::model;
use crate::xform::Morph;

fn bots_dir() -> PathBuf {
    let local = PathBuf::from("assets/bots");
    if local.exists() {
        local
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/bots")
    }
}

/// The fold strip for a chassis: f00, f01, ... in order. Walk poses
/// (w*) are locomotion, not the fold, and are deliberately excluded.
fn fold_poses(bot_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(bot_dir) else { return Vec::new() };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with('f'))
        .collect();
    names.sort();
    names.into_iter().map(|n| bot_dir.join(n)).collect()
}

/// STRIP ALIGNMENT DIAGNOSTIC — why a fold does or does not blend.
///
/// `xform::Morph::build` requires the poses of a strip to agree prim
/// for prim and vertex for vertex, and its doc says they do, "because
/// every pose in a strip comes from the same source mesh through the
/// same colour merge." They do not. Measured across the roster: prim
/// COUNT and total vertex count match exactly, and the sorted
/// multiset of per-prim vertex counts is identical — but the ORDER
/// differs per pose. The strips are exact PERMUTATIONS of each other,
/// so the positional shape check correctly refuses them, `build`
/// returns None, and every chassis silently falls back to stepping.
/// The flipbook `xform.rs` says it replaced is what actually ships.
///
/// Fixing it needs a correspondence key: something that identifies
/// "the same prim" across two poses regardless of emission order.
/// What this prints, per pose:
/// * `prims`/`verts` — the counts that already agree
/// * `order_sig` — a hash of per-prim vertex counts IN ORDER. Differs
///   between poses; that difference is the bug.
/// * `sorted_sig` — the same counts sorted. Identical between poses;
///   that identity is what makes a fix possible.
/// * `key_uniq` — whether quantised (colour, texture) identifies each
///   prim uniquely WITHIN a pose. It does: N of N, every chassis.
/// * `key_sig` — a hash of that key set. When it matches across all
///   poses, colour-keyed matching will align the strip; when it does
///   not, that chassis needs a stronger key.
///
/// Colour must be QUANTISED to 8 bits before hashing — raw f32 colours
/// differ in the last bits between poses and defeat exact matching.
///
/// ## Three keys tried, and what the failures prove
///
/// 1. **Quantised colour + texture.** Stable across poses for
///    `blackstar` and `hussar_tank` (4/5) but NOT `analog`, and only
///    ~27 of 34 prims unique. Necessary, not sufficient.
/// 2. **Composite: colour + texture + vertex count.** Uniqueness rises
///    (31/34) and the signature is stable across all five poses for
///    `blackstar` — but still differs per pose for `analog` and
///    `beetlejuice`. So colour genuinely varies between poses on some
///    chassis, and NO colour-derived key can fix those.
/// 3. **Rigid-motion invariants** (vertex count + radius of gyration +
///    max radius). This was the principled attempt: a part that merely
///    rotates and translates during a fold keeps its intrinsic size,
///    so these should be constant. THEY ARE NOT — uniqueness itself
///    varies per pose (28, 28, 26, 27, 27 on `blackstar`) and every
///    signature differs.
///
/// That third failure is the informative one, and it changes the
/// diagnosis. If prims were rigid parts, their radius of gyration
/// could not change between poses. It changes, so **a prim is not a
/// part** — it is a MATERIAL GROUP produced by the colour merge, and
/// one material spans many pieces that move independently during the
/// fold. A material group therefore deforms, which is why no per-prim
/// shape key can identify it across poses, and why the sorted vertex
/// counts still match (the same triangles exist; only their grouping
/// order and their positions move).
///
/// The consequence for a fix: aligning strips at the PRIM level is the
/// wrong layer. Either the exporter emits material groups in a
/// deterministic order (cheapest, and `blackstar`'s stable composite
/// signature suggests it nearly does already), or the blend has to
/// happen below the material group, at the triangle level, where the
/// correspondence actually exists. Not attempted here: guessing wrong
/// would blend a shoulder into a wheel, which is far worse than the
/// stepping the engine currently falls back to.
pub fn shapes(bot: &str) {
    let dirs = fold_poses(&bots_dir().join(bot));
    if dirs.is_empty() {
        println!("{bot}: no fold poses");
        return;
    }
    println!("{bot}");
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let Ok((raws, _)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0)))
        else {
            println!("  {name}  LOAD PANICKED");
            continue;
        };
        let counts: Vec<usize> = raws.iter().map(|r| r.verts.len()).collect();
        let total: usize = counts.iter().sum();

        let order_sig = fnv(&counts.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","));
        let mut sorted = counts.clone();
        sorted.sort_unstable();
        let sorted_sig = fnv(&sorted.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","));

        // quantise colour to 8 bits: raw f32 differs in the last bits
        // between poses and defeats exact matching
        let q = |c: f32| (c * 255.0).round() as i32;
        let mut keys: Vec<String> = raws
            .iter()
            .map(|r| {
                format!("{},{},{},{}|{}", q(r.color[0]), q(r.color[1]), q(r.color[2]), q(r.color[3]),
                    r.tex_uri.as_deref().unwrap_or("-"))
            })
            .collect();
        let uniq = keys.iter().collect::<std::collections::HashSet<_>>().len();
        keys.sort();
        let key_sig = fnv(&keys.join(","));

        // RIGID-MOTION INVARIANT probe: during a fold each part rotates
        // and translates but does not deform, so its intrinsic size is
        // constant across poses even though its position is not.
        let mut inv: Vec<String> = raws
            .iter()
            .map(|r| {
                let pts: Vec<glam::Vec3> = r.verts.iter().map(|v| glam::Vec3::from_array(v.pos)).collect();
                let c = if pts.is_empty() { glam::Vec3::ZERO } else { pts.iter().fold(glam::Vec3::ZERO, |a, b| a + *b) / pts.len() as f32 };
                // radius of gyration: rigid-invariant, cheap, and far more
                // discriminating than a bounding box (which is axis-aligned
                // and therefore changes as a part rotates)
                let rg = if pts.is_empty() { 0.0 } else { (pts.iter().map(|p| (*p - c).length_squared()).sum::<f32>() / pts.len() as f32).sqrt() };
                let rmax = pts.iter().map(|p| (*p - c).length()).fold(0.0f32, f32::max);
                format!("{}|{:.3}|{:.3}", r.verts.len(), rg, rmax)
            })
            .collect();
        let inv_uniq = inv.iter().collect::<std::collections::HashSet<_>>().len();
        inv.sort();
        let inv_sig = fnv(&inv.join(","));
        println!("      invariant: uniq={}/{} sig={:x}", inv_uniq, counts.len(), inv_sig);

        // composite key probe: colour+texture ties broken by vertex count
        let mut ck: Vec<String> = raws
            .iter()
            .map(|r| {
                format!("{},{},{},{}|{}|{}", q(r.color[0]), q(r.color[1]), q(r.color[2]), q(r.color[3]),
                    r.tex_uri.as_deref().unwrap_or("-"), r.verts.len())
            })
            .collect();
        let ck_uniq = ck.iter().collect::<std::collections::HashSet<_>>().len();
        ck.sort();
        let ck_sig = fnv(&ck.join(","));

        println!(
            "  {name}  prims={} verts={}  order_sig={:x}  sorted_sig={:x}  colour_uniq={}/{}  composite_uniq={}/{}  composite_sig={:x}",
            counts.len(), total, order_sig, sorted_sig, uniq, counts.len(), ck_uniq, counts.len(), ck_sig
        );
        let _ = key_sig;
    }
    println!("  (order_sig differing + sorted_sig matching = a permuted strip: the blend is off)");
}

/// Reference values for the browser demo's JS port of `compact.rs`.
///
/// The demo reimplements the compaction maths in JavaScript so it can
/// run in a viewport this environment does not have. A reimplementation
/// that merely looks right is worth nothing — it would show a fold the
/// engine does not produce. So the engine emits these, the demo runs
/// its own maths against the same inputs, and any divergence is
/// reported in the page rather than hidden.
fn golden() {
    println!("{{");
    println!("  \"curve\": [");
    let cs: Vec<String> = (0..=20).map(|i| format!("{:.6}", crate::compact::curve(i as f32 / 20.0))).collect();
    println!("    {}", cs.join(", "));
    println!("  ],");
    let cases: [(&str, crate::compact::Style); 2] =
        [("shrink", crate::compact::Style::Shrink), ("grow", crate::compact::Style::Grow)];
    println!("  \"displace\": {{");
    for (n, (name, style)) in cases.iter().enumerate() {
        let p = crate::compact::Params { style: *style, cell: 0.5, strength: 1.0, min_thickness: 0.0 };
        let mut rows: Vec<String> = Vec::new();
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let pos = glam::Vec3::new(0.37, -1.24, 2.9);
            let out = crate::compact::displace(pos, glam::Vec3::Y, glam::Vec3::ZERO, t, &p);
            rows.push(format!("[{:.6},{:.6},{:.6}]", out.x, out.y, out.z));
        }
        println!("    \"{}\": [{}]{}", name, rows.join(","), if n == 0 { "," } else { "" });
    }
    println!("  }}");
    println!("}}");
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 1469598103934665603;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

fn build(bot: &str) -> Option<Morph> {
    let dirs = fold_poses(&bots_dir().join(bot));
    if dirs.len() < 2 {
        return None;
    }
    let mut poses: Vec<Vec<Vec<Vertex>>> = Vec::new();
    let mut idx: Vec<Vec<u32>> = Vec::new();
    for (k, d) in dirs.iter().enumerate() {
        let raws = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0))).ok()?;
        let (raws, _) = raws;
        if k == 0 {
            idx = raws.iter().map(|r| r.idx.clone()).collect();
        }
        poses.push(raws.into_iter().map(|r| r.verts).collect());
    }
    Morph::build(poses, idx)
}

/// Pop tolerance scaled to the body: a truck-sized bot legitimately
/// moves further per sample than a scout. Derived from the resting
/// bounding box rather than guessed.
fn pop_tol_for(m: &mut Morph) -> f32 {
    let half = m.half_at(0.0);
    let longest = half.x.max(half.y).max(half.z).max(0.1);
    // a vertex may cross a quarter of the body's half-extent in one
    // sample (~1/18 s) before it counts as a pop
    longest * 0.25
}

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--fold").unwrap();
    let cmd = argv.get(i + 1).map(|s| s.as_str());
    let with_compaction = argv.iter().any(|a| a == "--compact");

    match cmd {
        Some("list") => list(),
        Some("check") => match argv.get(i + 2) {
            Some(bot) => {
                check(bot, with_compaction, true);
            }
            None => println!("usage: --fold check <bot> [--compact]"),
        },
        Some("sweep") => sweep(with_compaction),
        Some("golden") => golden(),
        Some("shapes") => match argv.get(i + 2) {
            Some(bot) => shapes(bot),
            None => println!("usage: --fold shapes <bot>"),
        },
        _ => println!("usage: --fold [list|check <bot>|sweep] [--compact]"),
    }
}

fn list() {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else {
        println!("no assets/bots directory");
        return;
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut n = 0;
    for name in &names {
        let poses = fold_poses(&bots_dir().join(name)).len();
        if poses >= 2 {
            println!("  {name:34} {poses} fold poses");
            n += 1;
        }
    }
    println!("{n} chassis with a foldable strip");
}

fn compaction_params() -> crate::compact::Params {
    crate::compact::Params::default()
}

fn check(bot: &str, with_compaction: bool, verbose: bool) -> Option<usize> {
    let Some(mut m) = build(bot) else {
        if verbose {
            println!("{bot}: no usable fold strip (need 2+ poses that agree prim-for-prim)");
        }
        return None;
    };
    if with_compaction {
        m.compaction = Some(compaction_params());
    }
    let tol = pop_tol_for(&mut m);
    let r = foldaudit::audit(&mut m, tol);
    if verbose {
        println!("{bot}{}", if with_compaction { "  [compaction ON]" } else { "" });
        println!("  {}", r.summary());
        // cap the printed faults: a badly broken fold can produce
        // hundreds and the first few are what get acted on
        for f in r.faults.iter().take(12) {
            println!("    {}", f.line());
        }
        if r.faults.len() > 12 {
            println!("    ... and {} more", r.faults.len() - 12);
        }
    }
    Some(r.faults.len())
}

fn sweep(with_compaction: bool) {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else {
        println!("no assets/bots directory");
        return;
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let (mut clean, mut dirty, mut skipped, mut total_faults) = (0, 0, 0, 0);
    let mut worst: Vec<(usize, String)> = Vec::new();
    for name in &names {
        match check(name, with_compaction, false) {
            Some(0) => clean += 1,
            Some(n) => {
                dirty += 1;
                total_faults += n;
                worst.push((n, name.clone()));
            }
            None => skipped += 1,
        }
    }
    worst.sort_by(|a, b| b.0.cmp(&a.0));
    println!(
        "{} chassis{}: {clean} clean, {dirty} with faults, {skipped} skipped -- {total_faults} faults total",
        clean + dirty + skipped,
        if with_compaction { " [compaction ON]" } else { "" }
    );
    for (n, name) in worst.iter().take(10) {
        println!("  {n:4} faults  {name}");
    }

    // A top-ten alone cannot tell you whether ten chassis fold badly or all
    // of them do, and those want completely different responses: fix ten
    // models, or fix the fold. The distribution answers it.
    if !worst.is_empty() {
        // `worst` is sorted DESCENDING, so index len/10 is the ninetieth
        // percentile and len*9/10 is the tenth. My first version labelled
        // them the other way round and then drew a conclusion from the
        // labels, which is how you get a confident wrong answer out of
        // correct data.
        let counts: Vec<usize> = worst.iter().map(|(n, _)| *n).collect();
        let n = counts.len();
        println!(
            "\n  spread: worst {}, p90 {}, median {}, p10 {}, best {}",
            counts[0],
            counts[n / 10],
            counts[n / 2],
            counts[n * 9 / 10],
            counts[n - 1]
        );
        // The question is not the spread, it is whether ANY chassis folds
        // cleanly. A ratio between worst and median can look like a tail
        // while every single model is still faulty.
        if clean == 0 {
            println!(
                "  NOT ONE of the {} folds cleanly, and the best still has {} faults.",
                clean + dirty,
                counts[n - 1]
            );
            println!("  this is the FOLD, not the models: a straight vertex lerp moves every");
            println!("  part along a chord, so parts that pass each other on the way cross.");
            println!("  fixing the worst ten would change nothing about the other {}.", n - 10);
        } else {
            println!("  {clean} fold cleanly; the rest are worth looking at individually.");
        }
    }
}
