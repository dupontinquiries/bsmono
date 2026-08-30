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
    poses_with(bot_dir, 'f')
}

/// Pose directories with a given prefix, sorted — 'f' is the fold strip,
/// 'w' the walk strip.
fn poses_with(bot_dir: &Path, prefix: char) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(bot_dir) else { return Vec::new() };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort();
    names.into_iter().map(|n| bot_dir.join(n)).collect()
}

/// WALK STRIPS through the same alignment the runtime uses — with pose 0
/// appended again, because the runtime closes the loop that way (a
/// stride wraps). One verdict per chassis: how much of the union
/// persists, how much fades, or why it refused. This is the headless
/// stand-in for watching 46 bots walk on hardware this sandbox lacks.
pub fn walkcheck() {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else { return };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let (mut ok, mut refused, mut none) = (0usize, 0usize, 0usize);
    for name in &names {
        let dirs = poses_with(&bots_dir().join(name), 'w');
        if dirs.len() < 2 {
            none += 1;
            continue;
        }
        let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
        let mut bad = false;
        for d in dirs.iter().chain(std::iter::once(&dirs[0])) {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0))) {
                Ok((raws, _)) => strip.push(raws),
                Err(_) => {
                    bad = true;
                    break;
                }
            }
        }
        if bad {
            refused += 1;
            println!("   LOAD-PANIC  {name}");
            continue;
        }
        match crate::align::unify(strip) {
            Some(u) => {
                ok += 1;
                println!(
                    "   ok  {name:44} {} union ({} extra)  {}",
                    u.poses[0].len(),
                    u.extras.len(),
                    u.report.line()
                );
            }
            None => {
                refused += 1;
                println!("   REFUSED     {name}  (flipbook stays)");
            }
        }
    }
    println!("---\n{ok} walk strips blend, {refused} refused, {none} without walks");
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

/// THE KEY `shapes` NEVER TRIED: texture coordinates.
///
/// Three keys were probed above — colour, colour+count, rigid shape
/// invariants — and each failed for a reason `shapes` documents. All
/// three read attributes a fold CHANGES (colour re-merges, positions
/// move, groups deform). UVs are the one per-vertex attribute a fold
/// cannot touch: a triangle keeps its texture mapping while its corners
/// travel, or the paint would swim during the animation. So a prim's
/// UV MULTISET is a candidate identity that survives both regrouping
/// order and geometric motion. This prints, per pose pair:
///  * `key_uniq` — quantised-UV-multiset keys unique within a pose
///  * `stable` — whether the key multiset matches the previous pose
///  * `matched` — prims matched one-to-one by key across the pair
///  * `order_ok` — matched prims whose intra-prim VERTEX ORDER also
///    agrees (per-index UV sequence equal), i.e. lerp-ready as-is
///  * `move` — mean centroid travel of matched prims (sanity: adjacent
///    poses should move modestly, and a mismatch shows up as travel
///    on the scale of the whole body)
pub fn align_probe(bot: &str) {
    let dirs = fold_poses(&bots_dir().join(bot));
    if dirs.len() < 2 {
        println!("{bot}: fewer than 2 fold poses");
        return;
    }
    println!("{bot}");
    let mut prev: Option<Vec<(u64, usize, glam::Vec3, Vec<u64>)>> = None;
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let Ok((raws, _)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0)))
        else {
            println!("  {name}  LOAD PANICKED");
            prev = None;
            continue;
        };
        // Per prim: (uv-multiset key, vert count, centroid, per-vertex uv hashes IN ORDER).
        // Quantised to 1/4096 so re-emitted floats differing in the last
        // bits do not defeat exact matching (same reason `shapes`
        // quantises colour).
        let info: Vec<(u64, usize, glam::Vec3, Vec<u64>)> = raws
            .iter()
            .map(|r| {
                let q = |x: f32| (x * 4096.0).round() as i64;
                let mut seq: Vec<u64> = r
                    .verts
                    .iter()
                    .map(|v| fnv(&format!("{},{}", q(v.uv[0]), q(v.uv[1]))))
                    .collect();
                let ordered = seq.clone();
                seq.sort_unstable();
                let key = fnv(&format!(
                    "{}|{}",
                    r.verts.len(),
                    seq.iter().map(|h| format!("{h:x}")).collect::<Vec<_>>().join(",")
                ));
                let c = if r.verts.is_empty() {
                    glam::Vec3::ZERO
                } else {
                    r.verts.iter().fold(glam::Vec3::ZERO, |a, v| a + glam::Vec3::from_array(v.pos))
                        / r.verts.len() as f32
                };
                (key, r.verts.len(), c, ordered)
            })
            .collect();
        let n = info.len();
        let uniq = info.iter().map(|i| i.0).collect::<std::collections::HashSet<_>>().len();
        let line = match &prev {
            None => format!("  {name}  prims={n} key_uniq={uniq}/{n}"),
            Some(p) => {
                let mut a: Vec<u64> = p.iter().map(|i| i.0).collect();
                let mut b: Vec<u64> = info.iter().map(|i| i.0).collect();
                a.sort_unstable();
                b.sort_unstable();
                let stable = a == b;
                // one-to-one match on key, first-come within duplicate classes
                let mut used = vec![false; p.len()];
                let mut matched = 0usize;
                let mut order_ok = 0usize;
                let mut travel = 0.0f32;
                for cur in &info {
                    if let Some((j, hit)) = p
                        .iter()
                        .enumerate()
                        .filter(|(j, q)| !used[*j] && q.0 == cur.0)
                        // among duplicate keys, take the nearest centroid
                        .min_by(|(_, x), (_, y)| {
                            (x.2 - cur.2)
                                .length_squared()
                                .partial_cmp(&(y.2 - cur.2).length_squared())
                                .unwrap()
                        })
                    {
                        used[j] = true;
                        matched += 1;
                        travel += (hit.2 - cur.2).length();
                        if hit.3 == cur.3 {
                            order_ok += 1;
                        }
                    }
                }
                let mv = if matched > 0 { travel / matched as f32 } else { 0.0 };
                // THE RESIDUE: prims without an exact partner. If a group
                // SPLIT between poses, the two sides hold the same vertices
                // under different grouping — their per-vertex UV multisets
                // agree — and a merged prim's UV sequence is the
                // CONCATENATION of its pieces' sequences (merging appends
                // buffers). Both are checkable.
                let mut p_used = vec![false; p.len()];
                let mut c_used = vec![false; info.len()];
                // re-run the same matching to mark used on both sides
                for (ci, cur) in info.iter().enumerate() {
                    if let Some((j, _)) = p
                        .iter()
                        .enumerate()
                        .filter(|(j, q)| !p_used[*j] && q.0 == cur.0)
                        .min_by(|(_, x), (_, y)| {
                            (x.2 - cur.2)
                                .length_squared()
                                .partial_cmp(&(y.2 - cur.2).length_squared())
                                .unwrap()
                        })
                    {
                        p_used[j] = true;
                        c_used[ci] = true;
                    }
                }
                let pl: Vec<&(u64, usize, glam::Vec3, Vec<u64>)> =
                    p.iter().enumerate().filter(|(j, _)| !p_used[*j]).map(|(_, q)| q).collect();
                let cl: Vec<&(u64, usize, glam::Vec3, Vec<u64>)> =
                    info.iter().enumerate().filter(|(j, _)| !c_used[*j]).map(|(_, q)| q).collect();
                let count_hashes = |side: &[&(u64, usize, glam::Vec3, Vec<u64>)]| {
                    let mut m = std::collections::HashMap::new();
                    for pr in side {
                        for h in &pr.3 {
                            *m.entry(*h).or_insert(0usize) += 1;
                        }
                    }
                    m
                };
                let union_eq = count_hashes(&pl) == count_hashes(&cl);
                // segment each prim on the merged side (fewer prims) into
                // contiguous runs of whole sequences from the split side
                let (merged, pieces) = if pl.len() <= cl.len() { (&pl, &cl) } else { (&cl, &pl) };
                let mut seg_ok = 0usize;
                let mut taken = vec![false; pieces.len()];
                for m in merged.iter() {
                    let seq = &m.3;
                    let mut i = 0usize;
                    let mut ok = true;
                    while i < seq.len() {
                        let hit = pieces.iter().enumerate().position(|(k, pc)| {
                            !taken[k]
                                && i + pc.3.len() <= seq.len()
                                && seq[i..i + pc.3.len()] == pc.3[..]
                        });
                        match hit {
                            Some(k) => {
                                taken[k] = true;
                                i += pieces[k].3.len();
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        seg_ok += 1;
                    }
                }
                format!(
                    "  {name}  prims={n} key_uniq={uniq}/{n} stable={} matched={matched}/{n} order_ok={order_ok}/{matched} move={mv:.3} residue={}v{} union_eq={} seg_ok={seg_ok}/{}",
                    if stable { "YES" } else { "no" },
                    pl.len(),
                    cl.len(),
                    if union_eq { "YES" } else { "no" },
                    merged.len()
                )
            }
        };
        println!("{line}");
        prev = Some(info);
    }
}

/// The probe over every chassis with a strip, one verdict line each.
pub fn align_probe_all() {
    let Ok(rd) = std::fs::read_dir(bots_dir()) else { return };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let (mut exact, mut unstable, mut thin) = (0usize, 0usize, 0usize);
    for name in &names {
        let dirs = fold_poses(&bots_dir().join(name));
        if dirs.len() < 2 {
            thin += 1;
            continue;
        }
        let mut prev: Option<Vec<u64>> = None;
        let mut all_stable = true;
        let mut all_uniq = true;
        for d in &dirs {
            let Ok((raws, _)) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0)))
            else {
                all_stable = false;
                break;
            };
            let q = |x: f32| (x * 4096.0).round() as i64;
            let mut keys: Vec<u64> = raws
                .iter()
                .map(|r| {
                    let mut seq: Vec<u64> = r
                        .verts
                        .iter()
                        .map(|v| fnv(&format!("{},{}", q(v.uv[0]), q(v.uv[1]))))
                        .collect();
                    seq.sort_unstable();
                    fnv(&format!(
                        "{}|{}",
                        r.verts.len(),
                        seq.iter().map(|h| format!("{h:x}")).collect::<Vec<_>>().join(",")
                    ))
                })
                .collect();
            let uniq = keys.iter().collect::<std::collections::HashSet<_>>().len();
            if uniq != keys.len() {
                all_uniq = false;
            }
            keys.sort_unstable();
            if let Some(p) = &prev {
                if *p != keys {
                    all_stable = false;
                }
            }
            prev = Some(keys);
        }
        let verdict = match (all_stable, all_uniq) {
            (true, true) => { exact += 1; "EXACT" }
            (true, false) => { exact += 1; "EXACT-dup" }
            _ => { unstable += 1; "unstable" }
        };
        println!("{verdict:>9}  {name}");
    }
    println!("---\n{} alignable by UV key, {} unstable, {} without a strip", exact, unstable, thin);
}

/// THE UNION, PER PRIM — which parts persist and which fade, with sizes.
///
/// A fade is the intended effect for a small transitional panel and a
/// defect for a fuselage. The audit's POP fault cannot tell those apart;
/// this table can: any entry with a `.` in its presence mask fades in or
/// out across the adjacent segment, and `r` is how big a thing is doing
/// the fading.
pub fn union_table(bot: &str) {
    let dirs = fold_poses(&bots_dir().join(bot));
    if dirs.len() < 2 {
        println!("{bot}: fewer than 2 fold poses");
        return;
    }
    let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
    for d in &dirs {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0))) {
            Ok((raws, _)) => strip.push(raws),
            Err(_) => {
                println!("{bot}: load panicked");
                return;
            }
        }
    }
    let Some(u) = crate::align::unify(strip) else {
        println!("{bot}: alignment refused ({})", "stepping fallback");
        return;
    };
    println!("{bot}: {} union prims ({} base + {} extra) — {}", u.poses[0].len(), u.base_prims, u.extras.len(), u.report.line());
    // presence: a placeholder is a collapsed copy, detectable as all
    // vertices within epsilon of each other
    let collapsed = |vs: &[Vertex]| -> bool {
        let Some(f) = vs.first() else { return true };
        let f = glam::Vec3::from_array(f.pos);
        vs.iter().all(|v| (glam::Vec3::from_array(v.pos) - f).length() < 1e-5)
    };
    let mut rows: Vec<(usize, String, usize, f32)> = (0..u.poses[0].len())
        .map(|p| {
            let mask: String = (0..u.poses.len())
                .map(|k| if collapsed(&u.poses[k][p]) { '.' } else { '#' })
                .collect();
            let (mut nv, mut r) = (0usize, 0.0f32);
            for k in 0..u.poses.len() {
                let vs = &u.poses[k][p];
                if collapsed(vs) {
                    continue;
                }
                nv = vs.len();
                let c = vs.iter().fold(glam::Vec3::ZERO, |a, v| a + glam::Vec3::from_array(v.pos))
                    / vs.len() as f32;
                let rr = vs
                    .iter()
                    .map(|v| (glam::Vec3::from_array(v.pos) - c).length())
                    .fold(0.0f32, f32::max);
                r = r.max(rr);
            }
            (p, mask, nv, r)
        })
        .collect();
    // faders first, biggest first — the rows worth reading
    rows.sort_by(|a, b| {
        let fa = a.1.contains('.');
        let fb = b.1.contains('.');
        fb.cmp(&fa).then(b.3.partial_cmp(&a.3).unwrap())
    });
    for (p, mask, nv, r) in rows {
        println!("  P{p:<3} {mask}  verts={nv:<6} r={r:.2}");
    }
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
    let mut strip: Vec<Vec<model::RawPrim>> = Vec::new();
    for d in dirs.iter() {
        let raws = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| model::load_raw(d, 4.0))).ok()?;
        strip.push(raws.0);
    }
    // Alignment first: no shipped strip agrees prim-for-prim, so calling
    // Morph::build on the raw poses refuses every chassis (measured —
    // see `align.rs`). Unify finds the union-prim correspondence; on ITS
    // refusal the audit correctly reports no morph, same as before.
    let u = crate::align::unify(strip)?;
    Morph::build(u.poses, u.idx)
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
        Some("align") => match argv.get(i + 2) {
            Some(bot) => align_probe(bot),
            None => align_probe_all(),
        },
        Some("walkcheck") => walkcheck(),
        Some("union") => match argv.get(i + 2) {
            Some(bot) => union_table(bot),
            None => println!("usage: --fold union <bot>"),
        },
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
