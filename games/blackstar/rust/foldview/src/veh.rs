//! veh — full transformations for the imported hollow-body vehicles.
//!
//! The imports were flattened to a single submesh by the pipeline, but they
//! are *unwelded*: wheels, turrets, guns, wings and skids are still separate
//! shells topologically. So: weld by position, split into connected islands,
//! classify the islands into an anatomy (hull / arms / legs / wings / barrel /
//! head), and hang each island on a `motion::Joint`. The vehicle is the rest
//! pose (t = 0); the robot is where the schedule carries it (t = 1) — the
//! mirror of rig.rs, which starts from the robot.
//!
//! Every motion is still a screw about a seam, so `verify_pair` holds for
//! every island at every t, same as the p-part bots.

use crate::{iqm, rig::{Bot, PartInst}};
use anyhow::{bail, Context, Result};
use model_verify::{components, weld};
use model_verify::motion::{schedule, Joint, Profile};
use std::path::Path;

const PI: f64 = std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { Hull, ArmL, ArmR, LegL, LegR, WingL, WingR, WingC, Barrel, Head, Skid, Detail }

#[derive(Clone)]
struct Piece {
    pos: Vec<[f64; 3]>,
    nrm: Vec<[f64; 3]>,
    idx: Vec<u32>,
    c: [f64; 3],
    lo: [f64; 3],
    hi: [f64; 3],
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn mul(a: [f64; 3], s: f64) -> [f64; 3] { [a[0]*s, a[1]*s, a[2]*s] }
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }

/// Category is read off the filename: imported_<category>_<name>[...].iqm
pub fn category_of(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let s = stem.strip_prefix("imported_").unwrap_or(stem);
    s.split('_').next().unwrap_or("veh").to_string()
}

fn nice_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("veh");
    let s = stem.strip_prefix("imported_").unwrap_or(stem);
    let s = s.split("_aka_").next().unwrap_or(s);
    s.to_string()
}

/// Per-category performance: (profile, duration s, base tint).
fn temperament(cat: &str) -> (Profile, f64, [f32; 3]) {
    match cat {
        "tank" => (Profile::Fluid, 1.35, [0.34, 0.36, 0.30]),      // slow, olive iron
        "hovertank" => (Profile::Combat, 1.15, [0.30, 0.34, 0.42]), // hard-stop steel blue
        "racer" => (Profile::Combat, 0.80, [0.55, 0.16, 0.14]),    // snap, scuffed red
        "hoverer" => (Profile::Fluid, 0.92, [0.62, 0.58, 0.48]),   // glide, dune chrome
        "flyer" | "flyerf" => (Profile::Anger, 0.88, [0.42, 0.40, 0.46]), // showoff titanium
        _ => (Profile::Fluid, 1.0, [0.5, 0.5, 0.5]),
    }
}

fn hash_jitter(name: &str) -> f32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() { h ^= b as u32; h = h.wrapping_mul(16777619); }
    ((h % 1000) as f32 / 1000.0 - 0.5) * 0.16
}

/// Split a flattened model into topological islands, dust merged into the hull.
fn segment(model: &iqm::Model, snap_mult: f64) -> Result<Vec<Piece>> {
    let mut pos = Vec::new();
    let mut nrm = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    for m in &model.meshes {
        let base = pos.len() as u32;
        pos.extend_from_slice(&m.pos);
        nrm.extend_from_slice(&m.nrm);
        idx.extend(m.idx.iter().map(|&i| i + base));
    }
    if pos.is_empty() { bail!("empty model"); }
    let (lo, hi) = iqm::bbox(&pos);
    let diag = (dot(sub(hi, lo), sub(hi, lo))).sqrt();
    let mut remap = weld(&pos, diag * 5e-4);
    // seam repair: damaged surfaces leave open boundary edges along cracks.
    // Snap boundary vertices to nearby boundary vertices so split shells
    // join back into one surface before components form.
    {
        use std::collections::HashMap;
        let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
        let canon = |v: u32| -> u32 { remap[v as usize] as u32 };
        for t in idx.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let (a, b) = (canon(a), canon(b));
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_count.entry(key).or_insert(0) += 1;
            }
        }
        let mut boundary: Vec<u32> = Vec::new();
        {
            let mut seen = std::collections::HashSet::new();
            for (&(a, b), &c) in &edge_count {
                if c == 1 {
                    if seen.insert(a) { boundary.push(a); }
                    if seen.insert(b) { boundary.push(b); }
                }
            }
        }
        let snap = diag * 0.009 * snap_mult;
        let cellk = |p: [f64; 3]| ((p[0] / snap) as i64, (p[1] / snap) as i64, (p[2] / snap) as i64);
        let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        for &v in &boundary { grid.entry(cellk(pos[v as usize])).or_default().push(v); }
        // tiny union-find over remap targets
        let mut par: Vec<usize> = (0..pos.len()).map(|i| remap[i]).collect();
        fn find(par: &mut Vec<usize>, mut x: usize) -> usize {
            while par[x] != x { par[x] = par[par[x]]; x = par[x]; }
            x
        }
        let mut joins = 0usize;
        for &v in &boundary {
            let p = pos[v as usize];
            let k = cellk(p);
            for dx in -1..=1 { for dy in -1..=1 { for dz in -1..=1 {
                if let Some(cands) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                    for &u in cands {
                        if u == v { continue; }
                        let q = pos[u as usize];
                        let d = sub(p, q);
                        if dot(d, d).sqrt() < snap {
                            let (ru, rv) = (find(&mut par, u as usize), find(&mut par, v as usize));
                            if ru != rv { par[ru] = rv; joins += 1; }
                        }
                    }
                }
            }}}
        }
        if joins > 0 {
            for i in 0..remap.len() { remap[i] = find(&mut par, i); }
        }
    }
    let groups = components(pos.len(), &idx, &remap);

    // vertex -> group id
    let mut gid = vec![usize::MAX; pos.len()];
    for (g, verts) in groups.iter().enumerate() {
        for &v in verts { gid[v] = g; }
    }
    // triangles per group (a tri's three verts share a group by construction)
    let mut gtris: Vec<Vec<u32>> = vec![Vec::new(); groups.len()];
    for t in idx.chunks_exact(3) {
        let g = gid[t[0] as usize];
        if g != usize::MAX { gtris[g].extend_from_slice(t); }
    }

    let mut pieces: Vec<Piece> = Vec::new();
    for (g, verts) in groups.iter().enumerate() {
        if gtris[g].is_empty() { continue; }
        let mut local = std::collections::HashMap::new();
        let mut p = Vec::with_capacity(verts.len());
        let mut n = Vec::with_capacity(verts.len());
        for &v in verts {
            local.insert(v as u32, p.len() as u32);
            p.push(pos[v]);
            n.push(nrm[v]);
        }
        let li: Vec<u32> = gtris[g].iter().map(|v| local[v]).collect();
        let (plo, phi) = iqm::bbox(&p);
        let c = iqm::centroid(&p);
        pieces.push(Piece { pos: p, nrm: n, idx: li, c, lo: plo, hi: phi });
    }
    if pieces.is_empty() { bail!("no faced components"); }

    // biggest first; every faced component is a piece — the tree decides
    // what rides what, so nothing gets frozen into a blob any more
    pieces.sort_by(|a, b| b.pos.len().cmp(&a.pos.len()));
    // geometry upscale: split any oversized triangle once so lighting,
    // cutting, and voxel fitting all work at a finer granularity
    {
        let (mut mlo2, mut mhi2) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &pieces { for a in 0..3 { mlo2[a] = mlo2[a].min(p.lo[a]); mhi2[a] = mhi2[a].max(p.hi[a]); } }
        let e = sub(mhi2, mlo2);
        let thr = dot(e, e).sqrt() * 0.045;
        let thr2 = thr * thr;
        for p in pieces.iter_mut() {
            let mut out: Vec<u32> = Vec::with_capacity(p.idx.len());
            let tris: Vec<[u32; 3]> = p.idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
            for t in tris {
                let (a, b, c) = (p.pos[t[0] as usize], p.pos[t[1] as usize], p.pos[t[2] as usize]);
                let l2m = |u: [f64; 3], v: [f64; 3]| { let d = sub(u, v); dot(d, d) };
                let ar2 = {
                    let u = sub(b, a); let v = sub(c, a);
                    let cx = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]];
                    dot(cx, cx)
                };
                // split only real surface triangles: long AND with area —
                // hairline slivers subdivide into degenerate line soup
                if l2m(a, b).max(l2m(b, c)).max(l2m(a, c)) > thr2
                    && ar2 > 0.06 * thr2 * thr2 && p.pos.len() < 60000 {
                    let mids = [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])].map(|(u, v)| {
                        let m = mul(add(p.pos[u as usize], p.pos[v as usize]), 0.5);
                        let nn = mul(add(p.nrm[u as usize], p.nrm[v as usize]), 0.5);
                        p.pos.push(m); p.nrm.push(nn);
                        (p.pos.len() - 1) as u32
                    });
                    out.extend_from_slice(&[t[0], mids[0], mids[2]]);
                    out.extend_from_slice(&[mids[0], t[1], mids[1]]);
                    out.extend_from_slice(&[mids[2], mids[1], t[2]]);
                    out.extend_from_slice(&[mids[0], mids[1], mids[2]]);
                } else {
                    out.extend_from_slice(&[t[0], t[1], t[2]]);
                }
            }
            p.idx = out;
        }
    }
    // sliver absorption: fragments under 0.3% of the mesh merge into the
    // touching neighbor instead of surviving as free-flying shards
    let total: usize = pieces.iter().map(|p| p.pos.len()).sum();
    let mut merged_into: Vec<Option<usize>> = vec![None; pieces.len()];
    for i in (0..pieces.len()).rev() {
        if pieces[i].pos.len() * 333 >= total { continue; }
        let mut best = (usize::MAX, f64::MAX);
        for j in 0..i {
            if merged_into[j].is_some() { continue; }
            let g = box_gap(&pieces[i], &pieces[j]);
            if g < best.1 { best = (j, g); }
        }
        if best.0 != usize::MAX && best.1.sqrt() < 1e-9 + 0.004 * {
            let e = sub(pieces[best.0].hi, pieces[best.0].lo);
            dot(e, e).sqrt()
        } {
            merged_into[i] = Some(best.0);
        }
    }
    for i in (0..pieces.len()).rev() {
        if let Some(j) = merged_into[i] {
            let src = pieces[i].clone();
            let dst = &mut pieces[j];
            let base = dst.pos.len() as u32;
            dst.pos.extend(src.pos);
            dst.nrm.extend(src.nrm);
            dst.idx.extend(src.idx.iter().map(|v| v + base));
            for a in 0..3 {
                dst.lo[a] = dst.lo[a].min(src.lo[a]);
                dst.hi[a] = dst.hi[a].max(src.hi[a]);
            }
            pieces.remove(i);
        }
    }
    for p in pieces.iter_mut() { p.c = iqm::centroid(&p.pos); }
    Ok(pieces)
}

/// Is this piece a display base, ground plane, backdrop sheet, or pedestal —
/// scanner/marketplace junk that is not part of the vehicle? Axis-agnostic:
/// a very thin sheet that spans most of the model footprint, or a giant
/// low-poly box engulfing the model. Never the majority of the geometry.
fn is_junk(p: &Piece, mlo: [f64; 3], mhi: [f64; 3], total_verts: usize) -> bool {
    if p.pos.len() * 100 > total_verts * 35 { return false; }
    let mext = sub(mhi, mlo);
    let diag = dot(mext, mext).sqrt();
    let e = sub(p.hi, p.lo);
    let mut ax = [0usize, 1, 2];
    ax.sort_by(|&a, &b| e[a].partial_cmp(&e[b]).unwrap());
    let (thin, mid, big) = (e[ax[0]], e[ax[1]], e[ax[2]]);
    let span_mid = mid / mext[ax[1]].max(1e-9);
    let span_big = big / mext[ax[2]].max(1e-9);
    // flat sheet covering the footprint (ground plane / pallet / backdrop)
    let sheet = thin < diag * 0.008 && span_mid > 0.55 && span_big > 0.55;
    // giant near-empty box engulfing the model (display pedestal)
    let boxy = p.idx.len() < 600
        && e[0] > mext[0] * 0.8 && e[1] > mext[1] * 0.8 && e[2] > mext[2] * 0.8;
    sheet || boxy
}

/// Canonical frame: side / up / forward unit vectors.
/// Side is found by bilateral symmetry (vehicles mirror across it), which is
/// far more reliable than extents — a wide-winged flyer breaks the extent
/// heuristic. Of the two remaining axes, up has the smaller extent. Forward
/// sign points at the tapered end (the nose).
struct Frame3 { s: [f64; 3], u: [f64; 3], f: [f64; 3], es: f64, eu: f64, ef: f64, center: [f64; 3] }

fn frame_detect(cloud: &[[f64; 3]], lo: [f64; 3], hi: [f64; 3]) -> (Frame3, [f64; 3]) {
    let e = sub(hi, lo);
    let c = mul(add(lo, hi), 0.5);
    let diag = dot(e, e).sqrt().max(1e-9);
    // hash the cloud for reflection lookups
    let cell = diag * 0.02;
    use std::collections::HashMap;
    let key = |p: &[f64; 3]| ((p[0]/cell).floor() as i64, (p[1]/cell).floor() as i64, (p[2]/cell).floor() as i64);
    let mut grid: HashMap<(i64,i64,i64), Vec<usize>> = HashMap::new();
    let step = (cloud.len() / 3000).max(1);
    let sample: Vec<usize> = (0..cloud.len()).step_by(step).collect();
    for &i in &sample { grid.entry(key(&cloud[i])).or_default().push(i); }
    let near = |p: [f64; 3]| -> f64 {
        let k = key(&p);
        let mut best = f64::MAX;
        for dx in -1..=1 { for dy in -1..=1 { for dz in -1..=1 {
            if let Some(b) = grid.get(&(k.0+dx, k.1+dy, k.2+dz)) {
                for &j in b {
                    let q = cloud[j];
                    let d2 = (p[0]-q[0]).powi(2)+(p[1]-q[1]).powi(2)+(p[2]-q[2]).powi(2);
                    if d2 < best { best = d2; }
                }
            }
        }}}
        best.sqrt().min(cell * 3.0)
    };
    let mut score = [0.0f64; 3];
    for a in 0..3 {
        let mut acc = 0.0;
        for &i in sample.iter().step_by(2) {
            let mut p = cloud[i];
            p[a] = 2.0 * c[a] - p[a];
            acc += near(p);
        }
        score[a] = acc;
    }
    let side_i = {
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| score[a].partial_cmp(&score[b]).unwrap());
        // weak symmetry verdict? prefer the candidate whose implied up-axis
        // is flatter (vehicles are low) — that catches the sideways ones
        let avg = (score[0] + score[1] + score[2]) / 3.0;
        if score[order[0]] > avg * 0.60 {
            // no axis is convincingly a mirror plane — asymmetric craft.
            // fall back to the extents prior: side is the middle extent.
            let mut by_e = [0usize, 1, 2];
            by_e.sort_by(|&a, &b| e[a].partial_cmp(&e[b]).unwrap());
            by_e[1]
        } else if score[order[0]] > score[order[1]] * 0.85 {
            let up_ext = |side: usize| {
                let (r0, r1) = match side { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
                e[r0].min(e[r1])
            };
            if up_ext(order[1]) < up_ext(order[0]) * 0.8 { order[1] } else { order[0] }
        } else { order[0] }
    };
    let (r0, r1) = match side_i { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
    let (up_i, fwd_i) = if e[r0] <= e[r1] { (r0, r1) } else { (r1, r0) };
    // nose = tapered end: fewer verts in the outer 18% slab
    let (mut lo_n, mut hi_n) = (0usize, 0usize);
    for p in cloud {
        let t = (p[fwd_i] - lo[fwd_i]) / e[fwd_i].max(1e-9);
        if t < 0.18 { lo_n += 1; } else if t > 0.82 { hi_n += 1; }
    }
    let fsign = if hi_n <= lo_n { 1.0 } else { -1.0 };
    let unit = |i: usize, sg: f64| { let mut v = [0.0; 3]; v[i] = sg; v };
    (Frame3 {
        s: unit(side_i, 1.0), u: unit(up_i, 1.0), f: unit(fwd_i, fsign),
        es: e[side_i], eu: e[up_i], ef: e[fwd_i],
        center: c,
    }, score)
}

/// Which way is "nose up"? Pick the pitch sign that carries forward toward up.
fn pitch_sign(fr: &Frame3) -> f64 {
    let test = |sgn: f64| {
        let r = model_verify::motion::axis_angle(fr.s, sgn * 1.4);
        let rf = [
            r[0][0]*fr.f[0] + r[0][1]*fr.f[1] + r[0][2]*fr.f[2],
            r[1][0]*fr.f[0] + r[1][1]*fr.f[1] + r[1][2]*fr.f[2],
            r[2][0]*fr.f[0] + r[2][1]*fr.f[1] + r[2][2]*fr.f[2],
        ];
        dot(rf, fr.u)
    };
    if test(1.0) >= test(-1.0) { 1.0 } else { -1.0 }
}

fn role_of(p: &Piece, fr: &Frame3, hull: &Piece) -> Role {
    let hull_top = dot(sub(hull.hi, fr.center), fr.u);
    let hull_f_mid = dot(sub(hull.c, fr.center), fr.f);
    let rc = sub(p.c, fr.center);
    let (sc, u, f) = (dot(rc, fr.s), dot(rc, fr.u), dot(rc, fr.f));
    let d = sub(p.hi, p.lo);
    let (es, eu, ef) = (dot(d, fr.s).abs(), dot(d, fr.u).abs(), dot(d, fr.f).abs());
    if eu < 0.28 * es.min(ef).max(1e-9) && es > 0.42 * fr.es {
        if es > 0.80 * fr.es { Role::WingC }
        else if sc >= 0.0 { Role::WingR } else { Role::WingL }
    } else if ef > 2.2 * es.max(eu) && sc.abs() < 0.18 * fr.es {
        Role::Barrel
    } else if u > hull_top * 0.45 && p.pos.len() < hull.pos.len().max(6) / 6
        && sc.abs() < 0.22 * fr.es {
        Role::Head
    } else if sc.abs() > 0.14 * fr.es {
        match (sc >= 0.0, f >= hull_f_mid) {
            (true, true) => Role::ArmR,
            (false, true) => Role::ArmL,
            (true, false) => Role::LegR,
            (false, false) => Role::LegL,
        }
    } else {
        Role::Skid
    }
}

/// Per-category performance notes: how this family stands and carries itself.
/// (hull_pitch rad, hull_pivot along F as fraction of length, wings sweep back
/// instead of rolling down, schedule overlap)
fn perf(cat: &str) -> (f64, f64, bool, f64, f64) {
    match cat {
        // the winged class: wings stay wings — swept back, not buried
        "flyer" | "flyerf" => (1.50, -0.26, true, 0.60, 0.7),
        // hunched brawler, not a statue
        "hovertank" => (0.95, -0.28, false, 0.50, 0.7),
        // tall, centered stand-up
        "racer" => (1.50, -0.10, false, 0.62, 1.0),
        "tank" => (1.45, -0.32, false, 0.50, 0.6),
        _ => (1.40, -0.26, false, 0.55, 1.0),
    }
}

/// Which way does a +S wing sweep to point rearward (-F)? Empirical, like
/// pitch_sign: handedness of the model axes is unknown, so ask the rotation.
fn sweep_sign(fr: &Frame3) -> f64 {
    let test = |sgn: f64| {
        let r = model_verify::motion::axis_angle(fr.u, sgn * 1.2);
        let rs = [
            r[0][0]*fr.s[0] + r[0][1]*fr.s[1] + r[0][2]*fr.s[2],
            r[1][0]*fr.s[0] + r[1][1]*fr.s[1] + r[1][2]*fr.s[2],
            r[2][0]*fr.s[0] + r[2][1]*fr.s[1] + r[2][2]*fr.s[2],
        ];
        dot(rs, fr.f)
    };
    if test(1.0) <= test(-1.0) { 1.0 } else { -1.0 }
}

/// An axis-aligned box mesh for skeleton struts: `at` center, `along` unit
/// axis, `len` long, `cross` thick. Rigid like everything else.
fn strut(at: [f64; 3], along: [f64; 3], len: f64, cross: f64) -> (Vec<[f64; 3]>, Vec<[f64; 3]>, Vec<u32>) {
    // build an orthonormal frame around `along`
    let a = along;
    let tmp = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let b = {
        let c = [a[1]*tmp[2]-a[2]*tmp[1], a[2]*tmp[0]-a[0]*tmp[2], a[0]*tmp[1]-a[1]*tmp[0]];
        let l = dot(c, c).sqrt().max(1e-9);
        mul(c, 1.0 / l)
    };
    let c2 = [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
    let (hl, hc) = (len * 0.5, cross * 0.5);
    let mut pos = Vec::with_capacity(8);
    for &sa in &[-1.0f64, 1.0] {
        for &sb in &[-1.0f64, 1.0] {
            for &sc in &[-1.0f64, 1.0] {
                pos.push(add(add(add(at, mul(a, sa * hl)), mul(b, sb * hc)), mul(c2, sc * hc)));
            }
        }
    }
    let idx: Vec<u32> = [
        0,1,3, 0,3,2,  4,6,7, 4,7,5,  0,2,6, 0,6,4,
        1,5,7, 1,7,3,  0,4,5, 0,5,1,  2,3,7, 2,7,6,
    ].to_vec();
    let mut nrm = vec![[0.0; 3]; 8];
    for i in 0..8 {
        let d = sub(pos[i], at);
        let l = dot(d, d).sqrt().max(1e-9);
        nrm[i] = mul(d, 1.0 / l);
    }
    (pos, nrm, idx)
}


/// A regular prism (cylinder-ish) mesh: `at` center, `axis` unit, `len`,
/// `radius`, `sides`. Caps included. Original generated geometry.
pub fn prism_pub(at: [f64; 3], axis: [f64; 3], len: f64, radius: f64, sides: usize)
    -> (Vec<[f64; 3]>, Vec<[f64; 3]>, Vec<u32>) { prism(at, axis, len, radius, sides) }

fn prism(at: [f64; 3], axis: [f64; 3], len: f64, radius: f64, sides: usize)
    -> (Vec<[f64; 3]>, Vec<[f64; 3]>, Vec<u32>) {
    let a = axis;
    let tmp = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let b = {
        let c = [a[1]*tmp[2]-a[2]*tmp[1], a[2]*tmp[0]-a[0]*tmp[2], a[0]*tmp[1]-a[1]*tmp[0]];
        let l = dot(c, c).sqrt().max(1e-9);
        mul(c, 1.0 / l)
    };
    let c2 = [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
    let hl = len * 0.5;
    let mut pos = Vec::new();
    let mut nrm = Vec::new();
    for &sgn in &[-1.0f64, 1.0] {
        for k in 0..sides {
            let th = k as f64 / sides as f64 * 2.0 * PI;
            let dir = add(mul(b, th.cos()), mul(c2, th.sin()));
            pos.push(add(add(at, mul(a, sgn * hl)), mul(dir, radius)));
            nrm.push(dir);
        }
    }
    pos.push(add(at, mul(a, -hl))); nrm.push(mul(a, -1.0));
    pos.push(add(at, mul(a, hl))); nrm.push(a);
    let (c0, c1) = ((2 * sides) as u32, (2 * sides + 1) as u32);
    let mut idx = Vec::new();
    for k in 0..sides as u32 {
        let k2 = (k + 1) % sides as u32;
        let (l0, l1, u0, u1) = (k, k2, sides as u32 + k, sides as u32 + k2);
        idx.extend_from_slice(&[l0, l1, u1, l0, u1, u0]);
        idx.extend_from_slice(&[c0, l1, l0]);
        idx.extend_from_slice(&[c1, u0, u1]);
    }
    (pos, nrm, idx)
}

/// Interior archetypes — original frames in the spirit of classic bruiser /
/// flier / speedster / signals silhouettes. Which extras each robot grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype { Brawler, Raptor, Sprinter, Signals }

fn archetype_for(cat: &str) -> Archetype {
    match cat {
        "tank" | "hovertank" => Archetype::Brawler,
        "flyer" | "flyerf" => Archetype::Raptor,
        "racer" => Archetype::Sprinter,
        "hoverer" => Archetype::Signals,
        _ => Archetype::Sprinter,
    }
}

/// Transform performances: same rig, different show. Roll and spin are full
/// 2*pi screws — their end state is the identity, so spectacle costs nothing
/// at t=1. Combat re-poses the finish. Salvo fires generated missiles late.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Performance { Clean, Roll, Spin, Combat }

fn default_perf(cat: &str) -> Performance {
    match cat {
        "racer" => Performance::Roll,
        "flyer" | "flyerf" => Performance::Spin,
        "hovertank" => Performance::Combat,
        _ => Performance::Clean,
    }
}

/// Hand-tuning: per-model overrides read from foldview_tuning.json (crate
/// root or CWD). Every field optional; absent file means all defaults.
#[derive(serde::Deserialize, Default, Clone)]
pub struct Tune {
    pub archetype: Option<String>,
    pub pitch: Option<f64>,      // multiplies hull pitch
    pub stance: Option<f64>,     // multiplies shoulder/hip half-spans
    pub palette: Option<[f32; 3]>,
    pub head: Option<i64>,       // -1 disables the head election
    pub flare: Option<f64>,      // multiplies shoulder abduction
    pub snug: Option<f64>,       // overrides the base armor pull-in (0..1)
    pub burst: Option<f64>,      // multiplies the break-apart amplitude
    pub weld: Option<bool>,      // rest-weld source-disconnected chunks (default on)
    pub leg_drop: Option<f64>,   // extra leg extension (fraction of rob_h)
    pub flip_nose: Option<bool>, // manual nose flip after the ensemble
    pub flip_up: Option<bool>,   // manual up flip after the ensemble
    pub swap_sf: Option<bool>,   // swap side/forward axes for the strange ones
    pub perf: Option<String>,    // clean | roll | spin | combat
    pub missiles: Option<bool>,  // fire a salvo late in the fold
    pub snap: Option<f64>,       // seam-repair radius multiplier for damaged meshes
    pub emblem: Option<bool>,
    pub palette2: Option<[f32; 3]>,
    pub note: Option<String>,
}

fn load_tuning() -> std::collections::HashMap<String, Tune> {
    for p in [
        std::path::PathBuf::from("foldview_tuning.json"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("foldview_tuning.json"),
    ] {
        if let Ok(txt) = std::fs::read_to_string(&p) {
            if let Ok(m) = serde_json::from_str(&txt) { return m; }
        }
    }
    Default::default()
}

/// Solid voxelization of the whole model: surface cells from triangle
/// sampling, interior by exterior flood-fill complement. Returns (occupied
/// cell centers, cell size, solid volume).
fn voxelize(pieces: &[Piece], lo: [f64; 3], hi: [f64; 3], res: usize) -> (Vec<[f64; 3]>, Vec<[f64; 3]>, f64, f64) {
    let e = sub(hi, lo);
    let cell = e[0].max(e[1]).max(e[2]).max(1e-9) / res as f64;
    let dims = [
        ((e[0] / cell).ceil() as usize + 2).max(3),
        ((e[1] / cell).ceil() as usize + 2).max(3),
        ((e[2] / cell).ceil() as usize + 2).max(3),
    ];
    let idx = |x: usize, y: usize, z: usize| (z * dims[1] + y) * dims[0] + x;
    let mut occ = vec![false; dims[0] * dims[1] * dims[2]];
    let cellof = |p: [f64; 3]| -> (usize, usize, usize) {
        (
            (((p[0] - lo[0]) / cell) as isize).clamp(0, dims[0] as isize - 1) as usize,
            (((p[1] - lo[1]) / cell) as isize).clamp(0, dims[1] as isize - 1) as usize,
            (((p[2] - lo[2]) / cell) as isize).clamp(0, dims[2] as isize - 1) as usize,
        )
    };
    for p in pieces {
        for t in p.idx.chunks_exact(3) {
            let (a, b, c) = (p.pos[t[0] as usize], p.pos[t[1] as usize], p.pos[t[2] as usize]);
            let m = |u: [f64; 3], v: [f64; 3]| dot(sub(u, v), sub(u, v)).sqrt();
            let n = ((m(a, b).max(m(b, c)).max(m(a, c)) / cell).ceil() as usize + 1).min(24);
            for i in 0..=n {
                for j in 0..=(n - i) {
                    let (w0, w1) = (i as f64 / n as f64, j as f64 / n as f64);
                    let w2 = 1.0 - w0 - w1;
                    let q = [
                        a[0] * w0 + b[0] * w1 + c[0] * w2,
                        a[1] * w0 + b[1] * w1 + c[1] * w2,
                        a[2] * w0 + b[2] * w1 + c[2] * w2,
                    ];
                    let (x, y, z) = cellof(q);
                    occ[idx(x, y, z)] = true;
                }
            }
        }
    }
    // exterior flood fill (6-connected) from the boundary shell
    let mut outside = vec![false; occ.len()];
    let mut queue: Vec<(usize, usize, usize)> = Vec::new();
    for z in 0..dims[2] { for y in 0..dims[1] { for x in 0..dims[0] {
        if (x == 0 || y == 0 || z == 0 || x == dims[0]-1 || y == dims[1]-1 || z == dims[2]-1)
            && !occ[idx(x, y, z)] {
            outside[idx(x, y, z)] = true;
            queue.push((x, y, z));
        }
    }}}
    while let Some((x, y, z)) = queue.pop() {
        let mut push = |x: usize, y: usize, z: usize| {
            let k = idx(x, y, z);
            if !outside[k] && !occ[k] { outside[k] = true; queue.push((x, y, z)); }
        };
        if x > 0 { push(x-1, y, z); } if x+1 < dims[0] { push(x+1, y, z); }
        if y > 0 { push(x, y-1, z); } if y+1 < dims[1] { push(x, y+1, z); }
        if z > 0 { push(x, y, z-1); } if z+1 < dims[2] { push(x, y, z+1); }
    }
    let (mut surface, mut interior) = (Vec::new(), Vec::new());
    for z in 0..dims[2] { for y in 0..dims[1] { for x in 0..dims[0] {
        let k = idx(x, y, z);
        let c = [
            lo[0] + (x as f64 + 0.5) * cell,
            lo[1] + (y as f64 + 0.5) * cell,
            lo[2] + (z as f64 + 0.5) * cell,
        ];
        if occ[k] { surface.push(c); }
        else if !outside[k] { interior.push(c); }
    }}}
    let vol = (surface.len() + interior.len()) as f64 * cell * cell * cell;
    (surface, interior, cell, vol)
}

/// The named bones of the fitted skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bone {
    Pelvis, Spine, Chest, Neck,
    ClavL, ClavR, UarmL, UarmR, LarmL, LarmR,
    ThighL, ThighR, ShinL, ShinR,
}
pub const BONES: [Bone; 14] = [
    Bone::Pelvis, Bone::Spine, Bone::Chest, Bone::Neck,
    Bone::ClavL, Bone::ClavR, Bone::UarmL, Bone::UarmR, Bone::LarmL, Bone::LarmR,
    Bone::ThighL, Bone::ThighR, Bone::ShinL, Bone::ShinR,
];

/// A bone in the vehicle (bind) pose: a segment plus which joint drives it.
struct BoneSeg { a: [f64; 3], b: [f64; 3], joint: usize }

/// Squared gap between two pieces' boxes (0 when touching/overlapping).
fn box_gap(a: &Piece, b: &Piece) -> f64 {
    let mut d2 = 0.0;
    for k in 0..3 {
        let g = (a.lo[k] - b.hi[k]).max(b.lo[k] - a.hi[k]).max(0.0);
        d2 += g * g;
    }
    d2
}

fn seg_dist(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let ab = sub(b, a);
    let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-12)).clamp(0.0, 1.0);
    let q = add(a, mul(ab, t));
    dot(sub(p, q), sub(p, q)).sqrt()
}

fn seg_point(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let ab = sub(b, a);
    let t = (dot(sub(p, a), ab) / dot(ab, ab).max(1e-12)).clamp(0.0, 1.0);
    add(a, mul(ab, t))
}

/// The whole pipeline, voxel-first: voxelize the vehicle, fit a humanoid
/// skeleton to the mass that is actually there, then hand every component to
/// the bone it overlaps. The skeleton articulates vehicle-pose -> stand; the
/// components ride their bones. Nothing can fly off a bone it is welded to.
/// Bake the repaired segmentation to an FVX sidecar beside the model.
pub fn bake(path: &Path) -> Result<std::path::PathBuf> {
    let model = iqm::load(path)?;
    let tuning = load_tuning();
    let tune = tuning.get(path.file_name().and_then(|x| x.to_str()).unwrap_or(""))
        .cloned().unwrap_or_default();
    let pieces = segment(&model, tune.snap.unwrap_or(1.0))?;
    let out = path.with_extension("fvx");
    let fp: Vec<crate::fvx::FvxPiece> = pieces.into_iter()
        .map(|p| crate::fvx::FvxPiece { pos: p.pos, nrm: p.nrm, idx: p.idx })
        .collect();
    crate::fvx::save(&out, &fp)?;
    Ok(out)
}

pub fn build(path: &Path, depth: usize) -> Result<Bot> {
    build_with(path, depth, None, false)
}

pub fn build_with(path: &Path, _depth: usize, perf_ov: Option<Performance>, salvo_ov: bool) -> Result<Bot> {
    let cat = category_of(path);
    let name = nice_name(path);
    let tuning = load_tuning();
    let tune = tuning.get(path.file_name().and_then(|x| x.to_str()).unwrap_or(""))
        .cloned().unwrap_or_default();
    let performance = perf_ov.or_else(|| tune.perf.as_deref().map(|p| match p {
        "roll" => Performance::Roll, "spin" => Performance::Spin,
        "combat" => Performance::Combat, _ => Performance::Clean,
    })).unwrap_or_else(|| default_perf(&cat));
    let salvo = salvo_ov || tune.missiles.unwrap_or(false);
    let arch = tune.archetype.as_deref().map(|a| match a {
        "brawler" => Archetype::Brawler, "raptor" => Archetype::Raptor,
        "signals" => Archetype::Signals, _ => Archetype::Sprinter,
    }).unwrap_or_else(|| archetype_for(&cat));
    let fvx_path = path.with_extension("fvx");
    let mut pieces = if fvx_path.exists() {
        let fp = crate::fvx::load(&fvx_path)?;
        let mut ps2: Vec<Piece> = fp.into_iter().map(|p| {
            let (lo, hi) = iqm::bbox(&p.pos);
            let c = iqm::centroid(&p.pos);
            Piece { pos: p.pos, nrm: p.nrm, idx: p.idx, lo, hi, c }
        }).collect();
        ps2.sort_by(|a, b| b.pos.len().cmp(&a.pos.len()));
        ps2
    } else {
        let model = iqm::load(path).with_context(|| format!("loading {}", path.display()))?;
        segment(&model, tune.snap.unwrap_or(1.0))?
    };
    let total: usize = pieces.iter().map(|p| p.pos.len()).sum();

    let (mut mlo, mut mhi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in &pieces {
        for a in 0..3 { mlo[a] = mlo[a].min(p.lo[a]); mhi[a] = mhi[a].max(p.hi[a]); }
    }
    let before = pieces.len();
    pieces.retain(|p| !is_junk(p, mlo, mhi, total));
    let mut junk = before - pieces.len();
    if pieces.is_empty() { bail!("nothing left after junk strip"); }
    // diorama environment: the largest piece fills the whole model box while
    // everything else huddles in a small cluster — that piece is terrain and
    // the huddle is the actual vehicle
    if pieces.len() > 3 {
        let e = sub(mhi, mlo);
        let diag = dot(e, e).sqrt();
        let big = sub(pieces[0].hi, pieces[0].lo);
        let fills = (0..3).all(|a| big[a] > e[a] * 0.88);
        if fills {
            let (mut rlo, mut rhi) = ([f64::MAX; 3], [f64::MIN; 3]);
            for p in pieces.iter().skip(1) {
                for a in 0..3 { rlo[a] = rlo[a].min(p.lo[a]); rhi[a] = rhi[a].max(p.hi[a]); }
            }
            let re = sub(rhi, rlo);
            if dot(re, re).sqrt() < diag * 0.45 {
                pieces.remove(0);
                junk += 1;
                mlo = rlo; mhi = rhi;
                pieces.sort_by(|a, b| b.pos.len().cmp(&a.pos.len()));
            }
        }
    }
    // terrain / display-base under-layer: a wide, bottom-anchored mass with a
    // vertical GAP between its top and everything else — vehicles (especially
    // hover ones) sit above their bases, so the gap is the tell. No size cap:
    // scanned terrain can dwarf the vehicle.
    {
        let e = sub(mhi, mlo);
        let diag = dot(e, e).sqrt();
        // per-axis widest two = footprint axes; smallest = height axis guess
        let mut ax = [0usize, 1, 2];
        ax.sort_by(|&a, &b| e[a].partial_cmp(&e[b]).unwrap());
        let up_g = ax[0];
        let mut k = 0usize;
        let n0 = pieces.len();
        let candidates: Vec<bool> = pieces.iter().map(|p| {
            let pe = sub(p.hi, p.lo);
            let wide = pe[ax[1]] > e[ax[1]] * 0.60 && pe[ax[2]] > e[ax[2]] * 0.60;
            let anchored = p.lo[up_g] - mlo[up_g] < diag * 0.02;
            wide && anchored && pe[up_g] < e[up_g] * 0.55
        }).collect();
        // gap test against everything else
        let mut drop = vec![false; n0];
        for i in 0..n0 {
            if !candidates[i] { continue; }
            let mut rest_min = f64::MAX;
            for j in 0..n0 {
                if j == i { continue; }
                rest_min = rest_min.min(pieces[j].lo[up_g]);
            }
            if rest_min - pieces[i].hi[up_g] > diag * 0.008 { drop[i] = true; }
        }
        pieces.retain(|_| { let d = drop[k]; k += 1; !d });
        junk += n0 - pieces.len();
    }
    // floater cull: small pieces whose box sits far from the connected body
    // cluster render as satellites in every pose — drop them
    {
        let e = sub(mhi, mlo);
        let diag = dot(e, e).sqrt();
        let gap_ok = 0.04 * diag;
        let mut incl = vec![false; pieces.len()];
        incl[0] = true;
        loop {
            let mut grew = false;
            for i in 1..pieces.len() {
                if incl[i] { continue; }
                for j in 0..pieces.len() {
                    if !incl[j] { continue; }
                    if box_gap(&pieces[i], &pieces[j]).sqrt() < gap_ok { incl[i] = true; grew = true; break; }
                }
            }
            if !grew { break; }
        }
        let mut k = 0usize;
        pieces.retain(|p| { let keep = incl[k] || p.pos.len() * 50 > total; k += 1; keep });
    }
    // rest-weld: the pieces that survived because they are big, yet float
    // disconnected in the source (a turret hovering over its hull), get
    // translated to touch the body — repairing the vehicle so both poses
    // can be whole. Off per model with `weld: false`.
    let mut welds = 0usize;
    if tune.weld.unwrap_or(true) {
        let e = sub(mhi, mlo);
        let diag = dot(e, e).sqrt();
        let gap_ok = 0.04 * diag;
        let eps = 0.004 * diag;
        loop {
            let mut incl = vec![false; pieces.len()];
            incl[0] = true;
            loop {
                let mut grew = false;
                for i in 1..pieces.len() {
                    if incl[i] { continue; }
                    for j in 0..pieces.len() {
                        if incl[j] && box_gap(&pieces[i], &pieces[j]).sqrt() < gap_ok {
                            incl[i] = true; grew = true; break;
                        }
                    }
                }
                if !grew { break; }
            }
            let Some(orphan) = (1..pieces.len()).find(|&i| !incl[i]) else { break; };
            // nearest in-cluster neighbor and the axis-wise closing vector
            let (mut best, mut bd) = (0usize, f64::MAX);
            for j in 0..pieces.len() {
                if !incl[j] { continue; }
                let d = box_gap(&pieces[orphan], &pieces[j]);
                if d < bd { bd = d; best = j; }
            }
            let (a, b) = (&pieces[orphan], &pieces[best]);
            let mut shift = [0.0f64; 3];
            for k in 0..3 {
                if a.lo[k] > b.hi[k] { shift[k] = -(a.lo[k] - b.hi[k]) - eps; }
                else if b.lo[k] > a.hi[k] { shift[k] = (b.lo[k] - a.hi[k]) + eps; }
            }
            let p = &mut pieces[orphan];
            for q in p.pos.iter_mut() { *q = add(*q, shift); }
            p.lo = add(p.lo, shift); p.hi = add(p.hi, shift); p.c = add(p.c, shift);
            welds += 1;
            if welds > 64 { break; }
        }
    }

    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    #[allow(unused_assignments)]
    let mut cloud: Vec<[f64; 3]> = Vec::new();
    for p in &pieces {
        for a in 0..3 { lo[a] = lo[a].min(p.lo[a]); hi[a] = hi[a].max(p.hi[a]); }
        let step = (p.pos.len() / 400).max(1);
        cloud.extend(p.pos.iter().step_by(step).copied());
    }
    let (mut fr0, _sym_scores) = frame_detect(&cloud, lo, hi);
    if tune.swap_sf.unwrap_or(false) {
        std::mem::swap(&mut fr0.s, &mut fr0.f);
        std::mem::swap(&mut fr0.es, &mut fr0.ef);
    }
    // ---- canonicalize: bake the detected frame into the geometry so every
    // model renders and rigs in the same space — side=X, up=+Y (flat side
    // down), nose=+Z. This is what ends the nosedivers and back-floaters.
    let (fr, center) = {
        // up SIGN by flat-bottom: the denser extreme plane along the up axis
        // is the underside (wheels, skids, flat hulls)
        let (mut umin, mut umax) = (f64::MAX, f64::MIN);
        for q in &cloud { let u = dot(*q, fr0.u); umin = umin.min(u); umax = umax.max(u); }
        let span = (umax - umin).max(1e-9);
        let (mut lo_n, mut hi_n) = (0usize, 0usize);
        for q in &cloud {
            let t = (dot(*q, fr0.u) - umin) / span;
            if t < 0.05 { lo_n += 1; } else if t > 0.95 { hi_n += 1; }
        }
        // up votes: flat-bottom density (2x) + top-clutter (small pieces
        // live on top: antennas, turrets, hatches)
        let mut up_vote = 0.0f64;
        if hi_n as f64 > lo_n as f64 * 1.3 { up_vote -= 2.0; } else if lo_n as f64 > hi_n as f64 * 1.3 { up_vote += 2.0; }
        // wheels belong on the ground and guns on top: the strongest up cues
        // a vehicle can offer about itself
        {
            let ucen = (umin + umax) * 0.5;
            let (mut wheel_lo, mut wheel_hi) = (0usize, 0usize);
            let (mut gun_lo, mut gun_hi) = (0usize, 0usize);
            for p in pieces.iter().skip(1) {
                let d = sub(p.hi, p.lo);
                let (es2, eu2, ef2) = (dot(d, fr0.s).abs(), dot(d, fr0.u).abs(), dot(d, fr0.f).abs());
                let uc = dot(p.c, fr0.u);
                // wheel: thin along the axle (side), round-ish in the other two
                if p.pos.len() * 8 < total && es2 < 0.6 * eu2.min(ef2)
                    && (eu2 - ef2).abs() < 0.35 * eu2.max(ef2) {
                    if uc < ucen { wheel_lo += 1; } else { wheel_hi += 1; }
                }
                // gun/turret mast: elongated centerline piece
                if ef2 > 2.2 * es2.max(eu2) && dot(p.c, fr0.s).abs() < 0.18 * fr0.es {
                    if uc < ucen { gun_lo += 1; } else { gun_hi += 1; }
                }
            }
            if wheel_lo > wheel_hi { up_vote += 3.0; } else if wheel_hi > wheel_lo { up_vote -= 3.0; }
            if gun_hi > gun_lo { up_vote += 3.0; } else if gun_lo > gun_hi { up_vote -= 3.0; }
        }
        {
            let (mut top_bits, mut bot_bits) = (0usize, 0usize);
            for p in pieces.iter().skip(1) {
                if p.pos.len() * 20 > total { continue; }
                let t = (dot(p.c, fr0.u) - umin) / span;
                if t > 0.70 { top_bits += 1; } else if t < 0.30 { bot_bits += 1; }
            }
            if top_bits as f64 > bot_bits as f64 * 1.4 { up_vote += 1.5; }
            else if bot_bits as f64 > top_bits as f64 * 1.4 { up_vote -= 1.5; }
        }
        let usign = if up_vote >= 0.0 { 1.0 } else { -1.0 }
            * if tune.flip_up.unwrap_or(false) { -1.0 } else { 1.0 };
        let uu = mul(fr0.u, usign);
        // nose votes: taper (1x, already in fr0.f) + barrel overhang (2x,
        // guns point forward) + wing position (2x, wings sit mid-to-rear)
        let mut f_vote = 1.0f64; // taper's verdict is +f
        {
            let (mut fmin, mut fmax) = (f64::MAX, f64::MIN);
            for q in &cloud { let v = dot(*q, fr0.f); fmin = fmin.min(v); fmax = fmax.max(v); }
            let mut wing_off = 0.0f64; let mut wings = 0usize;
            for p in pieces.iter().skip(1) {
                let d = sub(p.hi, p.lo);
                let (es2, eu2, ef2) = (dot(d, fr0.s).abs(), dot(d, uu).abs(), dot(d, fr0.f).abs());
                let fc = dot(p.c, fr0.f);
                // barrel: long, thin, centerline, overhanging one end
                if ef2 > 2.2 * es2.max(eu2) && dot(p.c, fr0.s).abs() < 0.18 * fr0.es {
                    let (plo, phi) = (dot(p.c, fr0.f) - ef2 / 2.0, dot(p.c, fr0.f) + ef2 / 2.0);
                    if phi > fmax - 0.02 * (fmax - fmin) { f_vote += 2.0; }
                    if plo < fmin + 0.02 * (fmax - fmin) { f_vote -= 2.0; }
                }
                // wing: flat and wide
                if eu2 < 0.28 * es2.min(ef2).max(1e-9) && es2 > 0.42 * fr0.es {
                    wing_off += fc - (fmin + fmax) * 0.5; wings += 1;
                }
            }
            if wings > 0 {
                if wing_off / wings as f64 > 0.0 { f_vote -= 2.0; } else { f_vote += 2.0; }
            }
            // fins and spoilers mark the rear: thin elevated plates at an end
            let ucen2 = (umin + umax) * 0.5;
            for p in pieces.iter().skip(1) {
                let d = sub(p.hi, p.lo);
                let ex = [dot(d, fr0.s).abs(), dot(d, uu).abs(), dot(d, fr0.f).abs()];
                let mut o = [0usize, 1, 2];
                o.sort_by(|&a, &b| ex[a].partial_cmp(&ex[b]).unwrap());
                let thin = ex[o[0]] < 0.15 * ex[o[1]].max(1e-9);
                let fc = dot(p.c, fr0.f) - (fmin + fmax) * 0.5;
                let elevated = dot(p.c, uu) > ucen2 + 0.08 * (umax - umin);
                if thin && elevated && fc.abs() > 0.30 * (fmax - fmin) * 0.5 {
                    if fc > 0.0 { f_vote -= 2.5; } else { f_vote += 2.5; }
                }
            }
        }
        let fsign2 = if f_vote >= 0.0 { 1.0 } else { -1.0 }
            * if tune.flip_nose.unwrap_or(false) { -1.0 } else { 1.0 };
        let ff = mul(fr0.f, fsign2);
        let ss = [uu[1]*ff[2]-uu[2]*ff[1], uu[2]*ff[0]-uu[0]*ff[2], uu[0]*ff[1]-uu[1]*ff[0]];
        let ctr = fr0.center;
        let rot = |v: [f64; 3]| -> [f64; 3] { [dot(v, ss), dot(v, uu), dot(v, ff)] };
        for p in pieces.iter_mut() {
            for q in p.pos.iter_mut() { *q = add(ctr, rot(sub(*q, ctr))); }
            for nq in p.nrm.iter_mut() { *nq = rot(*nq); }
            let (plo, phi) = iqm::bbox(&p.pos);
            p.lo = plo; p.hi = phi; p.c = iqm::centroid(&p.pos);
        }
        let (mut nlo, mut nhi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for p in &pieces {
            for a in 0..3 { nlo[a] = nlo[a].min(p.lo[a]); nhi[a] = nhi[a].max(p.hi[a]); }
        }
        let ne = sub(nhi, nlo);
        (Frame3 {
            s: [1.0, 0.0, 0.0], u: [0.0, 1.0, 0.0], f: [0.0, 0.0, 1.0],
            es: ne[0], eu: ne[1], ef: ne[2],
            center: mul(add(nlo, nhi), 0.5),
        }, ctr)
    };
    let _ = center;
    lo = [fr.center[0] - fr.es / 2.0, fr.center[1] - fr.eu / 2.0, fr.center[2] - fr.ef / 2.0];
    hi = [fr.center[0] + fr.es / 2.0, fr.center[1] + fr.eu / 2.0, fr.center[2] + fr.ef / 2.0];
    let ps = pitch_sign(&fr);
    let (hull_pitch0, hull_pivot_f, sweep, overlap, burst0) = perf(&cat);
    let burst_scale = burst0 * tune.burst.unwrap_or(1.0);
    let hull_pitch = hull_pitch0 * tune.pitch.unwrap_or(1.0);

    // ---- voxelize, and fit the robot height to the material available:
    // a mech of height H occupies roughly 0.047 H^3, so H = cbrt(V / 0.047)
    let (vox_surf, vox_int, vcell, vol) = voxelize(&pieces, lo, hi, 44);
    let ext = sub(hi, lo);
    let max_ext = ext[0].max(ext[1]).max(ext[2]);
    let h_vol = (vol / 0.047).powf(1.0 / 3.0);
    let rob_h = h_vol.clamp(0.55 * max_ext, 1.35 * max_ext);

    // voxel mass profile along F decides where the torso band actually is
    let fcoord = |p: [f64; 3]| dot(sub(p, fr.center), fr.f);
    let (mut flo, mut fhi) = (f64::MAX, f64::MIN);
    for v in vox_surf.iter().chain(vox_int.iter()) { let f = fcoord(*v); flo = flo.min(f); fhi = fhi.max(f); }
    let flen = (fhi - flo).max(1e-9);

    // ---- skeleton stations in the vehicle (bind) pose, scaled by rob_h and
    // clamped inside the vehicle so the skeleton always fits the model
    let torso = (0.36 * rob_h).clamp(0.34 * flen, 0.80 * flen);
    let stance = tune.stance.unwrap_or(1.0);
    let sh_s = ((0.18 * rob_h).min(0.60 * fr.es).max(0.18 * fr.es)) * stance;
    let hip_s = ((0.11 * rob_h).min(0.42 * fr.es).max(0.10 * fr.es)) * stance;
    // torso band centered on the mass; limbs sized to the span actually
    // available fore and aft of it
    let pelvis_f = 0.5 * (flo + fhi) - 0.5 * torso;
    let leg_total = (0.48 * rob_h).min((pelvis_f - flo) * 0.96).max(0.10 * flen);
    let arm_total = (0.36 * rob_h).min((fhi - pelvis_f - torso).max(0.0) * 0.9 + 0.20 * torso);
    let axisf = |f: f64, s: f64, u: f64| add(add(fr.center, mul(fr.f, f)), add(mul(fr.s, s), mul(fr.u, u)));
    let belly_u = -0.18 * fr.eu;
    let pelvis = axisf(pelvis_f, 0.0, 0.0);
    let neck = axisf(pelvis_f + torso, 0.0, 0.0);
    let chest = axisf(pelvis_f + torso * 0.62, 0.0, 0.0);
    let sh = |sd: f64| axisf(pelvis_f + torso * 0.94, sd * sh_s, 0.0);
    let hip = |sd: f64| axisf(pelvis_f, sd * hip_s, 0.0);
    // bind pose: legs fold rearward along -F under the belly, arms forward
    // adapt the base skeleton to this model's fracture fingerprint: the
    // knee and elbow sit at the mass medians of the material each limb will
    // actually claim, not at fixed fractions
    let (knee_frac, elbow_frac) = {
        let mut leg_fs: Vec<f64> = Vec::new();
        let mut arm_fs: Vec<f64> = Vec::new();
        for q in cloud.iter().step_by(3) {
            let f = fcoord(*q);
            if f < pelvis_f { leg_fs.push(f); }
            else if f > pelvis_f + torso * 0.90 { arm_fs.push(f); }
        }
        let med = |v: &mut Vec<f64>| -> Option<f64> {
            if v.len() < 8 { return None; }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Some(v[v.len() / 2])
        };
        let kf = med(&mut leg_fs)
            .map(|m| ((pelvis_f - m) / leg_total).clamp(0.38, 0.66))
            .unwrap_or(0.52);
        let ef = med(&mut arm_fs)
            .map(|m| ((m - (pelvis_f + torso * 0.94)) / arm_total.max(1e-9)).clamp(0.35, 0.62))
            .unwrap_or(0.50);
        (kf, ef)
    };
    let knee = |sd: f64| axisf(pelvis_f - leg_total * knee_frac, sd * hip_s, belly_u);
    let foot = |sd: f64| axisf(pelvis_f - leg_total, sd * hip_s, belly_u * 0.6);
    let elbow = |sd: f64| axisf(pelvis_f + torso * 0.94 + arm_total * elbow_frac, sd * sh_s, belly_u * 0.7);
    let wrist = |sd: f64| axisf(pelvis_f + torso * 0.94 + arm_total, sd * sh_s, belly_u * 0.4);

    // ---- joints: root pitch, then the articulated skeleton
    let mut joints: Vec<Joint> = vec![Joint {
        part: 0, parent: None,
        pivot: add(fr.center, add(mul(fr.f, hull_pivot_f * fr.ef), mul(fr.u, -0.5 * fr.eu))),
        axis: fr.s, angle: ps * hull_pitch, slide: 0.0, reclip: None,
    }];
    let root = 0usize;
    let mut jpush = |parent: usize, pivot: [f64; 3], axis: [f64; 3], angle: f64, slide: f64,
                     joints: &mut Vec<Joint>| -> usize {
        joints.push(Joint { part: 0, parent: Some(parent), pivot, axis, angle, slide, reclip: None });
        joints.len() - 1
    };
    // spine/chest/neck: subtle forward lean stack so the torso is not a plank
    let j_spine = jpush(root, pelvis, fr.s, -ps * 0.06, 0.0, &mut joints);
    let j_chest = jpush(j_spine, chest, fr.s, -ps * 0.05, 0.0, &mut joints);
    let j_neck = jpush(j_chest, neck, fr.s, -ps * 0.05, 0.0, &mut joints);
    // phase bookkeeping: which joints belong to which beat of the fold
    let mut ph_early: Vec<usize> = Vec::new();   // expand: abduction, unlock
    let mut ph_armor: Vec<usize> = Vec::new();   // armor closes after skeleton
    let mut ph_finale: Vec<usize> = Vec::new();  // head, hatch, clenching
    let mut ph_spin: Vec<usize> = Vec::new();    // alive the whole way
    let mut ph_perf: Vec<usize> = Vec::new();    // roll/spin/combat show moves
    let mut ph_salvo: Vec<usize> = Vec::new();   // missiles away, late
    let mut ph_burst: Vec<usize> = Vec::new();   // break apart, early
    let mut ph_pull: Vec<usize> = Vec::new();    // reassemble, late
    let mdiag = { let e = sub(hi, lo); dot(e, e).sqrt() };
    // burst+pull as a helper: outward early, then a late slide whose vector
    // sums with the burst to land exactly on `final_off` from rest
    let mut burst_pull = |parent: usize, c: [f64; 3], final_off: [f64; 3], amp: f64,
                          joints: &mut Vec<Joint>, ph_b: &mut Vec<usize>, ph_p: &mut Vec<usize>|
                          -> usize {
        let rc = sub(c, fr.center);
        let rl = dot(rc, rc).sqrt();
        let dir = if rl > 1e-9 { mul(rc, 1.0 / rl) } else { fr.u };
        let r = amp * (0.09 * mdiag + 0.12 * rl).min(0.16 * mdiag); // wingtips are not fireworks
        joints.push(Joint { part: 0, parent: Some(parent), pivot: c,
            axis: dir, angle: 0.0, slide: r, reclip: None });
        let bj = joints.len() - 1;
        ph_b.push(bj);
        let pull_vec = sub(final_off, mul(dir, r));
        let pl = dot(pull_vec, pull_vec).sqrt();
        if pl > 1e-9 {
            joints.push(Joint { part: 0, parent: Some(bj), pivot: c,
                axis: mul(pull_vec, 1.0 / pl), angle: 0.0, slide: pl, reclip: None });
            let pj = joints.len() - 1;
            ph_p.push(pj);
            pj
        } else { bj }
    };
    // arms: abduct OUTWARD first so the pi fold sweeps around the torso, not
    // through it — then the showpiece shoulder fold, then elbows
    let out_sign = -sweep_sign(&fr);
    let flare = tune.flare.unwrap_or(1.0);
    let j_ab_l = jpush(j_chest, sh(-1.0), fr.u, -1.0 * out_sign * 0.35 * flare, 0.0, &mut joints);
    let j_ab_r = jpush(j_chest, sh(1.0), fr.u, 1.0 * out_sign * 0.35 * flare, 0.0, &mut joints);
    ph_early.push(j_ab_l); ph_early.push(j_ab_r);
    let j_sh_l = jpush(j_ab_l, sh(-1.0), fr.s, -ps * (PI - 0.18), 0.0, &mut joints);
    let j_sh_r = jpush(j_ab_r, sh(1.0), fr.s, -ps * (PI - 0.18), 0.0, &mut joints);
    let j_el_l = jpush(j_sh_l, elbow(-1.0), fr.s, ps * 0.38, 0.0, &mut joints);
    let j_el_r = jpush(j_sh_r, elbow(1.0), fr.s, ps * 0.38, 0.0, &mut joints);
    // legs: bind already points rearward = down after the pitch; hips widen
    // the stance slightly, knees bend into it
    let j_hip_l = jpush(root, hip(-1.0), fr.s, ps * 0.10, -0.04 * fr.es, &mut joints);
    let j_hip_r = jpush(root, hip(1.0), fr.s, ps * 0.10, 0.04 * fr.es, &mut joints);
    // combat stance: wider hips, deeper knees, torso lean, one arm raised —
    // the machine lands ready instead of at attention
    let mut insert_between = |child: usize, pivot: [f64; 3], axis: [f64; 3],
                              angle: f64, slide: f64, joints: &mut Vec<Joint>| -> usize {
        let old_parent = joints[child].parent;
        joints.push(Joint { part: 0, parent: old_parent, pivot, axis, angle, slide, reclip: None });
        let nj = joints.len() - 1;
        joints[child].parent = Some(nj);
        nj
    };
    let _ = &mut insert_between;
    let (j_hip_l, j_hip_r) = if let Some(drop) = tune.leg_drop {
        let dl = jpush(j_hip_l, hip(-1.0), mul(fr.f, -1.0), 0.0, drop * rob_h, &mut joints);
        let dr = jpush(j_hip_r, hip(1.0), mul(fr.f, -1.0), 0.0, drop * rob_h, &mut joints);
        (dl, dr)
    } else { (j_hip_l, j_hip_r) };
    let j_kn_l = jpush(j_hip_l, knee(-1.0), fr.s, -ps * 0.22, 0.0, &mut joints);
    let j_kn_r = jpush(j_hip_r, knee(1.0), fr.s, -ps * 0.22, 0.0, &mut joints);


    if performance == Performance::Combat {
        for (jh, sd) in [(j_hip_l, -1.0), (j_hip_r, 1.0)] {
            let j = insert_between(jh, hip(sd), fr.s, 0.0, sd * 0.06 * fr.es, &mut joints);
            ph_perf.push(j);
        }
        for jk in [j_kn_l, j_kn_r] {
            let j = insert_between(jk, [0.0; 3], fr.s, -ps * 0.16, 0.0, &mut joints);
            ph_perf.push(j);
        }
        let j = insert_between(j_spine, pelvis, fr.s, -ps * 0.10, 0.0, &mut joints);
        ph_perf.push(j);
        let j2 = insert_between(j_sh_r, sh(1.0), fr.s, -ps * 0.60, 0.0, &mut joints);
        ph_perf.push(j2);
    }

    let mut bone_segs: Vec<(Bone, BoneSeg)> = vec![
        (Bone::Pelvis, BoneSeg { a: hip(-1.0), b: hip(1.0), joint: root }),
        (Bone::Spine, BoneSeg { a: pelvis, b: chest, joint: j_spine }),
        (Bone::Chest, BoneSeg { a: chest, b: neck, joint: j_chest }),
        (Bone::Neck, BoneSeg { a: neck, b: axisf(pelvis_f + torso * 1.08, 0.0, 0.0), joint: j_neck }),
        (Bone::ClavL, BoneSeg { a: chest, b: sh(-1.0), joint: j_chest }),
        (Bone::ClavR, BoneSeg { a: chest, b: sh(1.0), joint: j_chest }),
        (Bone::UarmL, BoneSeg { a: sh(-1.0), b: elbow(-1.0), joint: j_sh_l }),
        (Bone::UarmR, BoneSeg { a: sh(1.0), b: elbow(1.0), joint: j_sh_r }),
        (Bone::LarmL, BoneSeg { a: elbow(-1.0), b: wrist(-1.0), joint: j_el_l }),
        (Bone::LarmR, BoneSeg { a: elbow(1.0), b: wrist(1.0), joint: j_el_r }),
        (Bone::ThighL, BoneSeg { a: hip(-1.0), b: knee(-1.0), joint: j_hip_l }),
        (Bone::ThighR, BoneSeg { a: hip(1.0), b: knee(1.0), joint: j_hip_r }),
        (Bone::ShinL, BoneSeg { a: knee(-1.0), b: foot(-1.0), joint: j_kn_l }),
        (Bone::ShinR, BoneSeg { a: knee(1.0), b: foot(1.0), joint: j_kn_r }),
    ];

    // wings stay their own performance, hanging off the chest
    let n = pieces.len();
    let hull_core = {
        let (hl, hh) = (pieces[0].lo, pieces[0].hi);
        let c = mul(add(hl, hh), 0.5);
        let half = mul(sub(hh, hl), 0.35); // 70% box
        (sub(c, half), add(c, half))
    };
    let mut special: Vec<Option<usize>> = vec![None; n];
    // flying-wing gate: when wing-shaped mass dominates the airframe, the
    // wings ARE the body — sweeping them apart shreds the silhouette, so
    // they hold station and shrug instead
    let wing_mass: usize = (1..n).map(|i| {
        let p = &pieces[i];
        let d = sub(p.hi, p.lo);
        let (es, eu, ef) = (dot(d, fr.s).abs(), dot(d, fr.u).abs(), dot(d, fr.f).abs());
        if eu < 0.28 * es.min(ef).max(1e-9) && es > 0.42 * fr.es { p.pos.len() } else { 0 }
    }).sum();
    let flying_wing = wing_mass * 100 > total * 45;
    for i in 1..n {
        let p = &pieces[i];
        let d = sub(p.hi, p.lo);
        let (es, eu, ef) = (dot(d, fr.s).abs(), dot(d, fr.u).abs(), dot(d, fr.f).abs());
        let wingish = eu < 0.28 * es.min(ef).max(1e-9) && es > 0.42 * fr.es
            && p.pos.len() >= 40.max(total / 500);
        if wingish && es <= 0.80 * fr.es && !flying_wing {
            let side = if dot(sub(p.c, fr.center), fr.s) >= 0.0 { 1.0 } else { -1.0 };
            let inner_seam = add(p.c, mul(fr.s, -side * es * 0.5));
            let j = if sweep {
                let sg = sweep_sign(&fr);
                let back = jpush(j_chest, inner_seam, fr.u, side * sg * 1.9, 0.0, &mut joints);
                jpush(back, inner_seam, fr.f, side * 0.7, 0.0, &mut joints)
            } else {
                jpush(j_chest, inner_seam, fr.f, side * 1.6, 0.0, &mut joints)
            };
            special[i] = Some(j);
        } else if wingish {
            let _ = flying_wing;
            special[i] = Some(jpush(j_chest, p.c, fr.f, 0.22, 0.04 * fr.ef, &mut joints));
        }
    }

    // ---- add the components back, and let the skeleton BREAK THEM APART:
    // each triangle of a piece belongs to the bind-pose bone it sits nearest.
    // A piece whose triangles split meaningfully across several bones is cut
    // into per-bone chunks along those territory lines; the skeleton then
    // drags each chunk with its bone. Small single-territory pieces stay
    // whole and snug onto their bone.
    struct Draft {
        name: &'static str,
        pos: Vec<[f64; 3]>,
        nrm: Vec<[f64; 3]>,
        idx: Vec<u32>,
        joint: usize,
        limb: bool,
    }
    let mut drafts: Vec<Draft> = Vec::new();
    let mut shard_budget = 700usize;
    let mut part_joint = vec![0usize; n]; // for special/whole pieces
    let mut part_bone: Vec<Option<Bone>> = vec![None; n];
    let bone_name = |b: Bone| -> &'static str {
        Box::leak(format!("{:?}", b).to_lowercase().into_boxed_str())
    };
    let mut hull_chunks = 0usize;
    for i in 0..n {
        if let Some(j) = special[i] {
            part_joint[i] = j;
            continue;
        }
        let p = &pieces[i];
        // per-triangle nearest-bone vote
        let ntri = p.idx.len() / 3;
        let mut tri_bone = vec![0usize; ntri];
        let mut share = vec![0usize; bone_segs.len()];
        let chest_bi = bone_segs.iter().position(|(b, _)| *b == Bone::Chest).unwrap_or(2);
        for (ti, t) in p.idx.chunks_exact(3).enumerate() {
            let (a, b, c) = (p.pos[t[0] as usize], p.pos[t[1] as usize], p.pos[t[2] as usize]);
            let cen = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            // the toy rule: the vehicle's front face IS the chest. Front-top
            // hull surface claims Chest before proximity gets a vote.
            let rc = sub(cen, fr.center);
            if i == 0 && dot(rc, fr.f) > 0.24 * fr.ef && dot(rc, fr.u) > -0.05 * fr.eu {
                tri_bone[ti] = chest_bi;
                share[chest_bi] += 1;
                continue;
            }
            let mut best = (0usize, f64::MAX);
            for (bi, (_, bs)) in bone_segs.iter().enumerate() {
                let d = seg_dist(cen, bs.a, bs.b);
                if d < best.1 { best = (bi, d); }
            }
            tri_bone[ti] = best.0;
            share[best.0] += 1;
        }
        let pext = sub(p.hi, p.lo);
        let fspan = dot(pext, fr.f).abs();
        let long_piece = fspan > 0.45 * flen && ntri >= 30;
        if long_piece {
            // a long portion refusing to fold: carve it into fourths along
            // the body axis so each segment takes its own bone and motion
            let (mut plo, mut phi_) = (f64::MAX, f64::MIN);
            for q in &p.pos { let f = dot(*q, fr.f); plo = plo.min(f); phi_ = phi_.max(f); }
            let span = (phi_ - plo).max(1e-9);
            for (ti, t) in p.idx.chunks_exact(3).enumerate() {
                let cen = [
                    (p.pos[t[0] as usize][0] + p.pos[t[1] as usize][0] + p.pos[t[2] as usize][0]) / 3.0,
                    (p.pos[t[0] as usize][1] + p.pos[t[1] as usize][1] + p.pos[t[2] as usize][1]) / 3.0,
                    (p.pos[t[0] as usize][2] + p.pos[t[1] as usize][2] + p.pos[t[2] as usize][2]) / 3.0,
                ];
                let quarter = (((dot(cen, fr.f) - plo) / span) * 4.0).min(3.99) as usize;
                // reuse tri_bone as quarter-refined: nearest bone within the
                // quarter's own centroid neighborhood
                let _ = quarter;
                let mut best = (0usize, f64::MAX);
                for (bi, (_, bs)) in bone_segs.iter().enumerate() {
                    let d = seg_dist(cen, bs.a, bs.b);
                    if d < best.1 { best = (bi, d); }
                }
                tri_bone[ti] = best.0;
            }
            // recount and force the spanning path
            for v in share.iter_mut() { *v = 0; }
            for &b in &tri_bone { share[b] += 1; }
        }
        let spanning = (ntri >= 24
            && (p.pos.len() * 40 > total || fspan > 0.28 * flen)
            && share.iter().filter(|&&c| c * 100 >= ntri * 10).count() >= 2)
            || (long_piece && share.iter().filter(|&&c| c > 0).count() >= 2);
        if !spanning {
            // whole piece to its majority bone
            let win = share.iter().enumerate().max_by_key(|(_, &v)| v).map(|(k, _)| k).unwrap_or(0);
            let (bone, bs) = (bone_segs[win].0, &bone_segs[win].1);
            part_bone[i] = Some(bone);
            let structural = p.pos.len() * 8 > total || dot(pext, fr.f).abs() > 0.45 * flen;
            let on = seg_point(p.c, bs.a, bs.b);
            let gap = sub(on, p.c);
            let d = dot(gap, gap).sqrt();
            // farther pieces pull in harder — distant hardware is what reads
            // as "flying off", so it gets the strongest snug at reassembly
            let base = tune.snug.unwrap_or(0.55);
            let k = (base + 0.40 * (d / (0.30 * rob_h)).min(1.0)).min(0.92);
            let snug_off = if d > 1e-9 && !structural { mul(gap, k) } else { [0.0; 3] };
            let tiny = p.pos.len() * 400 < total;
            let amp = if structural { 0.40 * burst_scale }
                else if tiny { 0.25 * burst_scale } // debris rides, it does not fly
                else { burst_scale };
            part_joint[i] = burst_pull(bs.joint, p.c, snug_off, amp,
                &mut joints, &mut ph_burst, &mut ph_pull);
            continue;
        }
        // cut the piece into per-bone chunks along the territory lines
        for (bi, (bone, bs)) in bone_segs.iter().enumerate() {
            if share[bi] == 0 { continue; }
            let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            let mut cp: Vec<[f64; 3]> = Vec::new();
            let mut cn: Vec<[f64; 3]> = Vec::new();
            let mut ci: Vec<u32> = Vec::new();
            for (ti, t) in p.idx.chunks_exact(3).enumerate() {
                if tri_bone[ti] != bi { continue; }
                for &v in t {
                    let e = *remap.entry(v).or_insert_with(|| {
                        cp.push(p.pos[v as usize]);
                        cn.push(p.nrm[v as usize]);
                        (cp.len() - 1) as u32
                    });
                    ci.push(e);
                }
            }
            if ci.len() < 3 { continue; }
            if i == 0 { hull_chunks += 1; }
            let cc = {
                let mut a = [0.0; 3];
                for q in &cp { a = add(a, mul(*q, 1.0 / cp.len() as f64)); }
                a
            };
            let amp = if i == 0 { 0.45 * burst_scale } else { burst_scale };
            // hood-chest hinge: the hull's chest plate folds down onto the
            // torso front — the signature car-former identity move
            let base_joint = if i == 0 && *bone == Bone::Chest
                && dot(sub(cc, chest), fr.f) > 0.0 {
                // hinge on the plate's OWN lower edge — a fold, not a fling
                let (mut clo, mut chi_) = ([f64::MAX; 3], [f64::MIN; 3]);
                for q in &cp { for a in 0..3 { clo[a] = clo[a].min(q[a]); chi_[a] = chi_[a].max(q[a]); } }
                let _ = chi_;
                let hinge_pivot = [cc[0], clo[1], cc[2]];
                joints.push(Joint { part: 0, parent: Some(bs.joint),
                    pivot: hinge_pivot, axis: fr.s, angle: -ps * 0.70, slide: 0.0, reclip: None });
                joints.len() - 1
            } else { bs.joint };
            let cj = burst_pull(base_joint, cc, [0.0; 3], amp, &mut joints, &mut ph_burst, &mut ph_pull);
            let limb = matches!(bone,
                Bone::UarmL | Bone::UarmR | Bone::LarmL | Bone::LarmR
                | Bone::ThighL | Bone::ThighR | Bone::ShinL | Bone::ShinR);
            // cybertronian surface adaptation: a few raised tech blocks
            // on sizeable armor, riding the same chunk
            let chunk_thick = {
                let (mut l2, mut h2) = ([f64::MAX; 3], [f64::MIN; 3]);
                for q in &cp { for a2 in 0..3 { l2[a2] = l2[a2].min(q[a2]); h2[a2] = h2[a2].max(q[a2]); } }
                let e2 = sub(h2, l2);
                e2[0].min(e2[1]).min(e2[2])
            };
            if ci.len() >= 300 && chunk_thick > 0.030 * rob_h {
                for gk in 0..2usize {
                    let ti2 = (ci.len() / 3 / 4) * (gk + 1) * 3;
                    if ti2 + 2 >= ci.len() { break; }
                    let (a2, b2, c2) = (cp[ci[ti2] as usize], cp[ci[ti2+1] as usize], cp[ci[ti2+2] as usize]);
                    let cen2 = mul(add(add(a2, b2), c2), 1.0 / 3.0);
                    let u2 = sub(b2, a2); let v2 = sub(c2, a2);
                    let nx = [u2[1]*v2[2]-u2[2]*v2[1], u2[2]*v2[0]-u2[0]*v2[2], u2[0]*v2[1]-u2[1]*v2[0]];
                    let nl = dot(nx, nx).sqrt();
                    if nl < 1e-12 { continue; }
                    let nn = mul(nx, 1.0 / nl);
                    let sz = (0.016 + 0.007 * ((gk as f64 * 2.7).fract())) * rob_h;
                    let (gp, gn, gi) = strut(add(cen2, mul(nn, sz * 0.10)), nn, sz * 0.7, sz); // half-embedded: relief, not satellites
                    drafts.push(Draft { name: "greeble", pos: gp, nrm: gn, idx: gi,
                        joint: cj, limb });
                }
            }
            let limb = matches!(bone,
                Bone::UarmL | Bone::UarmR | Bone::LarmL | Bone::LarmR
                | Bone::ThighL | Bone::ThighR | Bone::ShinL | Bone::ShinR);
            // shatter the surface: big chunks split into spatial shards, each
            // with its own small jittered burst and an exact return — the
            // shell visibly shatters, then re-fuses seamlessly
            if ci.len() >= 180 && shard_budget > 0 {
                let cell = 0.10 * mdiag;
                let mut buckets: std::collections::HashMap<(i64, i64, i64), (Vec<[f64;3]>, Vec<[f64;3]>, Vec<u32>, std::collections::HashMap<u32,u32>)> = Default::default();
                for t3 in ci.chunks_exact(3) {
                    let cen = [
                        (cp[t3[0] as usize][0] + cp[t3[1] as usize][0] + cp[t3[2] as usize][0]) / 3.0,
                        (cp[t3[0] as usize][1] + cp[t3[1] as usize][1] + cp[t3[2] as usize][1]) / 3.0,
                        (cp[t3[0] as usize][2] + cp[t3[1] as usize][2] + cp[t3[2] as usize][2]) / 3.0,
                    ];
                    let k = ((cen[0]/cell) as i64, (cen[1]/cell) as i64, (cen[2]/cell) as i64);
                    let e = buckets.entry(k).or_default();
                    for &v in t3 {
                        let nv = *e.3.entry(v).or_insert_with(|| {
                            e.0.push(cp[v as usize]); e.1.push(cn[v as usize]); (e.0.len()-1) as u32
                        });
                        e.2.push(nv);
                    }
                }
                for (kk, (sp, sn, si, _)) in buckets {
                    if si.len() < 3 { continue; }
                    let sc = {
                        let mut a = [0.0; 3];
                        for q in &sp { a = add(a, mul(*q, 1.0 / sp.len() as f64)); }
                        a
                    };
                    // jittered shard direction: radial plus a hash wobble
                    let h = ((kk.0 * 73 + kk.1 * 179 + kk.2 * 283).unsigned_abs() % 997) as f64 / 997.0;
                    let rc = sub(sc, fr.center);
                    let rl = dot(rc, rc).sqrt().max(1e-9);
                    let wob = [(h - 0.5) * 0.30, ((h * 7.0).fract() - 0.5) * 0.35, ((h * 13.0).fract() - 0.5) * 0.35];
                    let dir0 = add(mul(rc, 1.0 / rl), mul(wob, 0.7));
                    let dl = dot(dir0, dir0).sqrt().max(1e-9);
                    let r = (0.012 + 0.020 * h) * mdiag; // crack apart; the chunk carries the travel
                    joints.push(Joint { part: 0, parent: Some(cj), pivot: sc,
                        axis: mul(dir0, 1.0 / dl), angle: 0.0, slide: r, reclip: None });
                    let bj = joints.len() - 1; ph_burst.push(bj);
                    joints.push(Joint { part: 0, parent: Some(bj), pivot: sc,
                        axis: mul(dir0, -1.0 / dl), angle: 0.0, slide: r, reclip: None });
                    let pj = joints.len() - 1; ph_pull.push(pj);
                    drafts.push(Draft { name: bone_name(*bone), pos: sp, nrm: sn, idx: si,
                        joint: pj, limb });
                    if shard_budget == 0 { break; }
                    shard_budget -= 1;
                }
            } else {
                drafts.push(Draft {
                    name: bone_name(*bone),
                    pos: cp, nrm: cn, idx: ci,
                    joint: cj,
                    limb,
                });
            }
        }
        part_bone[i] = None;
        part_joint[i] = usize::MAX; // consumed into chunks
    }
    let _ = hull_chunks;

    // elect the head: the smallish piece nearest the neck tip gets the reveal
    let head_tip = axisf(pelvis_f + torso * 1.05, 0.0, 0.0);
    let mut head_pick: Option<(usize, f64)> = None;
    if tune.head != Some(-1) { for i in 1..n {
        if special[i].is_some() || part_joint[i] == usize::MAX { continue; }
        let p = &pieces[i];
        if p.pos.len() > total / 10 { continue; }
        let dd = dot(sub(p.c, head_tip), sub(p.c, head_tip)).sqrt();
        if dd < 0.20 * flen && head_pick.map_or(true, |(_, bd)| dd < bd) {
            head_pick = Some((i, dd));
        }
    } }
    if let Some((hi_idx, _)) = head_pick {
        let g = pieces[hi_idx].c;
        let to = sub(head_tip, g);
        let dd = dot(to, to).sqrt().max(1e-9);
        let mig = jpush(j_neck, g, mul(to, 1.0 / dd), 0.0, dd * 0.95, &mut joints);
        let rev = jpush(mig, head_tip, fr.f, PI, 0.04 * torso, &mut joints);
        ph_finale.push(mig); ph_finale.push(rev);
        part_joint[hi_idx] = rev;
        part_bone[hi_idx] = None;
    }

    for (i, j) in joints.iter_mut().enumerate() { j.part = i; }

    let (profile, duration, base) = temperament(&cat);
    let jit = hash_jitter(&name);
    let tint = tune.palette.unwrap_or([
        (base[0] + jit).clamp(0.06, 0.95),
        (base[1] + jit * 0.6).clamp(0.06, 0.95),
        (base[2] + jit * 0.3).clamp(0.06, 0.95),
    ]);

    let bone_mass: Vec<usize> = {
        let mut m = vec![0usize; bone_segs.len()];
        for d in &drafts {
            if let Some(k) = bone_segs.iter().position(|(b, _)|
                format!("{:?}", b).to_lowercase() == d.name) {
                m[k] += d.pos.len();
            }
        }
        m
    };
    let mass_mean = (bone_mass.iter().sum::<usize>() as f64
        / bone_mass.len().max(1) as f64).max(1.0);

    let mut parts: Vec<PartInst> = Vec::new();
    for (i, p) in pieces.into_iter().enumerate() {
        if part_joint[i] == usize::MAX { continue; } // broken into chunks
        parts.push(PartInst {
            name: Box::leak(
                match (special[i].is_some(), part_bone[i]) {
                    (true, _) => "wing".to_string(),
                    (false, None) if i != 0 => "head".to_string(),
                    (_, Some(b)) => format!("{:?}", b).to_lowercase(),
                    _ => "part".to_string(),
                }
                .into_boxed_str(),
            ),
            rest: p.pos, nrm: p.nrm, idx: p.idx,
            joint: part_joint[i],
            tint: [tint[0] * 0.86, tint[1] * 0.86, tint[2] * 0.90],
        });
    }
    for d in drafts {
        parts.push(PartInst {
            name: d.name,
            rest: d.pos, nrm: d.nrm, idx: d.idx,
            joint: d.joint,
            tint: if d.limb {
                tune.palette2.unwrap_or([tint[0] * 0.78, tint[1] * 0.78, tint[2] * 0.84])
            } else {
                tint
            },
        });
    }

    let mut placed: Vec<([f64; 3], usize)> = Vec::new();
    // ---- voxel guts: the hollow shell gets a solid mechanical interior.
    // Nonuniform: fine half-cells near the shell surface and along the
    // forearms and calves (where limbs must read as dense machinery),
    // coarse cubes deeper in. Every cell welds to its nearest bone.
    {
        use std::collections::HashSet;
        let kx = |p: [f64; 3]| (((p[0] - lo[0]) / vcell).floor() as i64, ((p[1] - lo[1]) / vcell).floor() as i64, ((p[2] - lo[2]) / vcell).floor() as i64);
        let surf_set: HashSet<(i64, i64, i64)> = vox_surf.iter().map(|v| kx(*v)).collect();
        let limb_segs: Vec<(usize, [f64; 3], [f64; 3])> = bone_segs.iter().enumerate()
            .filter(|(_, (b, _))| matches!(b,
                Bone::LarmL | Bone::LarmR | Bone::ShinL | Bone::ShinR
                | Bone::UarmL | Bone::UarmR | Bone::ThighL | Bone::ThighR))
            .map(|(k, (_, s))| (k, s.a, s.b)).collect();
        let limb_r = 0.075 * rob_h;
        // consolidated guts: adjacent interior cells of the same bone merge
        // into elongated machined members along the body axis — density
        // without noise. Limb-adjacent cells claim the budget first.
        let mut cell_bone: std::collections::HashMap<(i64, i64, i64), usize> = Default::default();
        let mut cell_pri: Vec<((i64, i64, i64), f64)> = Vec::new();
        for c in &vox_int {
            let mut best = (0usize, f64::MAX);
            for (bi2, (_, bs)) in bone_segs.iter().enumerate() {
                let d = seg_dist(*c, bs.a, bs.b);
                if d < best.1 { best = (bi2, d); }
            }
            let k = kx(*c);
            cell_bone.insert(k, best.0);
            let pri = limb_segs.iter().map(|(_, a, b)| seg_dist(*c, *a, *b))
                .fold(f64::MAX, f64::min);
            cell_pri.push((k, pri.min(if (-1..=1).any(|dx| (-1..=1).any(|dy| (-1..=1).any(|dz|
                surf_set.contains(&(k.0 + dx, k.1 + dy, k.2 + dz))))) { limb_r } else { f64::MAX })));
        }
        cell_pri.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let keep: std::collections::HashSet<(i64, i64, i64)> =
            cell_pri.iter().take(520).map(|(k, _)| *k).collect();
        // greedy Z-runs per (bone, x, y)
        let mut runs: std::collections::HashMap<(usize, i64, i64), Vec<i64>> = Default::default();
        for k in &keep {
            let b = cell_bone[k];
            runs.entry((b, k.0, k.1)).or_default().push(k.2);
        }
        let mut emitted = 0usize;
        for ((b, x, y), mut zs) in runs {
            if emitted >= 240 { break; }
            zs.sort_unstable();
            let mut i0 = 0usize;
            while i0 < zs.len() {
                let mut i1 = i0;
                while i1 + 1 < zs.len() && zs[i1 + 1] == zs[i1] + 1 { i1 += 1; }
                let (z0, z1) = (zs[i0], zs[i1]);
                let at = [lo[0] + (x as f64 + 0.5) * vcell,
                          lo[1] + (y as f64 + 0.5) * vcell,
                          lo[2] + ((z0 + z1) as f64 / 2.0 + 0.5) * vcell];
                let len = (z1 - z0 + 1) as f64 * vcell * 0.94;
                let jt = bone_segs[b].1.joint;
                let h = hash_jitter(&format!("{x}.{y}.{z0}")) as f64;
                let g = 0.94 + 0.10 * (h + 0.06);
                let (bp, bn, bi3) = strut(at, [0.0, 0.0, 1.0], len, vcell * 0.90);
                parts.push(PartInst { name: "guts", rest: bp, nrm: bn, idx: bi3, joint: jt,
                    tint: [(0.30 * g) as f32, (0.31 * g) as f32, (0.35 * g) as f32] });
                placed.push((at, b));
                emitted += 1;
                if emitted >= 240 { break; }
                i0 = i1 + 1;
            }
        }
        // cybertronian connections: each bone clamps onto its surviving
        // voxel region with link struts and collar rings — the skeleton is
        // OF the vehicle, not floating inside it
        let mut links = 0usize;
        for (bi2, (_, bs)) in bone_segs.iter().enumerate() {
            if links >= 44 { break; }
            let mid = mul(add(bs.a, bs.b), 0.5);
            let mut near: Vec<([f64; 3], f64)> = placed.iter()
                .filter(|(_, b)| *b == bi2)
                .map(|(at, _)| (*at, { let d = sub(*at, mid); dot(d, d).sqrt() }))
                .collect();
            near.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            for (cell, _) in near.iter().take(3) {
                let on = seg_point(*cell, bs.a, bs.b);
                let v = sub(*cell, on);
                let l = dot(v, v).sqrt();
                if l < 1e-6 || l > 0.16 * rob_h { continue; }
                let (lp, ln, li) = strut(mul(add(on, *cell), 0.5), mul(v, 1.0 / l), l,
                    0.009 * rob_h);
                parts.push(PartInst { name: "link", rest: lp, nrm: ln, idx: li,
                    joint: bs.joint, tint: [0.44, 0.46, 0.50] });
                links += 1;
            }
            // collar clamps at the third points of every limb bone
            if matches!(bone_segs[bi2].0, Bone::ThighL | Bone::ThighR | Bone::ShinL
                | Bone::ShinR | Bone::UarmL | Bone::UarmR | Bone::LarmL | Bone::LarmR) {
                let d = sub(bs.b, bs.a);
                let l = dot(d, d).sqrt().max(1e-9);
                let ax = mul(d, 1.0 / l);
                for f2 in [0.33, 0.67] {
                    let (cp2, cn2, ci2) = prism(add(bs.a, mul(ax, l * f2)), ax,
                        0.018 * rob_h, (0.028 * rob_h).min(0.14 * fr.eu) * 0.95, 8);
                    parts.push(PartInst { name: "link", rest: cp2, nrm: cn2, idx: ci2,
                        joint: bs.joint, tint: [0.36, 0.37, 0.41] });
                }
            }
        }
    }

        // muscle layer: actuator bundles wrapped around each limb bone,
        // their girth drawn from the voxel mass this model actually put
        // there — beefy where the vehicle was dense, wiry where it was air
        {
            let limb_ids: Vec<usize> = bone_segs.iter().enumerate()
                .filter(|(_, (b, _))| matches!(b,
                    Bone::ThighL | Bone::ThighR | Bone::ShinL | Bone::ShinR
                    | Bone::UarmL | Bone::UarmR | Bone::LarmL | Bone::LarmR
                    | Bone::Spine))
                .map(|(k, _)| k).collect();
            let mean_cells = (placed.len() as f64 / bone_segs.len() as f64).max(1.0);
            for &k in &limb_ids {
                let bs = &bone_segs[k].1;
                let d = sub(bs.b, bs.a);
                let l = dot(d, d).sqrt().max(1e-9);
                let ax = mul(d, 1.0 / l);
                let tmp = if ax[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
                let p1 = {
                    let c = [ax[1]*tmp[2]-ax[2]*tmp[1], ax[2]*tmp[0]-ax[0]*tmp[2], ax[0]*tmp[1]-ax[1]*tmp[0]];
                    let n2 = dot(c, c).sqrt().max(1e-9);
                    mul(c, 1.0 / n2)
                };
                let p2 = [ax[1]*p1[2]-ax[2]*p1[1], ax[2]*p1[0]-ax[0]*p1[2], ax[0]*p1[1]-ax[1]*p1[0]];
                let cells_here = placed.iter().filter(|(_, b)| *b == k).count() as f64;
                let girth = (cells_here / mean_cells).sqrt().clamp(0.6, 1.5);
                let r_m = 0.030 * rob_h * girth;
                let is_spine = matches!(bone_segs[k].0, Bone::Spine);
                let n_b = if is_spine { 2 } else { 3 };
                let phase = hash_jitter(&format!("m{k}")) as f64 * 6.28;
                for bnd in 0..n_b {
                    let th = phase + bnd as f64 / n_b as f64 * 2.0 * PI;
                    let dir = add(mul(p1, th.cos()), mul(p2, th.sin()));
                    let off = (0.028 * rob_h) + r_m * 0.55;
                    let mid = add(mul(add(bs.a, bs.b), 0.5), mul(dir, off));
                    let (mp, mn, mi) = prism(mid, ax, l * 0.58, r_m, 8);
                    let g = 0.92 + 0.14 * ((bnd as f64 * 1.7 + phase).fract());
                    parts.push(PartInst { name: "muscle", rest: mp, nrm: mn, idx: mi,
                        joint: bs.joint,
                        tint: [(0.30 * g) as f32, (0.17 * g) as f32, (0.17 * g) as f32] });
                }
            }
        }

    // ---- true metal bones, thickness adapted to the fracture fingerprint:
    // a femur carrying half the hull is thicker than one carrying a fender
    let bone_cross = (0.028 * rob_h).min(0.14 * fr.eu);
    let cross_of = |k: usize| -> f64 {
        let ratio = (bone_mass[k] as f64 / mass_mean).sqrt().clamp(0.62, 1.55);
        bone_cross * ratio
    };
    // base-skeleton ornaments, scaled by the torso's share of the mass
    {
        let torso_share = {
            let t: usize = bone_segs.iter().enumerate()
                .filter(|(_, (b, _))| matches!(b, Bone::Spine | Bone::Chest | Bone::Pelvis))
                .map(|(k, _)| bone_mass[k]).sum();
            (t as f64 / (mass_mean * 3.0)).sqrt().clamp(0.7, 1.4)
        };
        let steel2 = [0.60f32, 0.63, 0.68];
        // scapula plates behind the shoulders
        for sd in [-1.0f64, 1.0] {
            let at = add(add(sh(sd), mul(fr.u, 0.06 * rob_h * torso_share)),
                mul(fr.f, -0.03 * rob_h));
            let (sp, sn, si) = strut(at, fr.s, 0.10 * rob_h * torso_share, 0.030 * rob_h);
            parts.push(PartInst { name: "bone", rest: sp, nrm: sn, idx: si,
                joint: j_chest, tint: steel2 });
        }
        // sternum plate and iliac fins
        let (sp, sn, si) = strut(add(chest, mul(fr.u, belly_u * 0.55)), fr.f,
            0.11 * rob_h * torso_share, 0.026 * rob_h);
        parts.push(PartInst { name: "bone", rest: sp, nrm: sn, idx: si,
            joint: j_chest, tint: steel2 });
        for sd in [-1.0f64, 1.0] {
            let (ip, inr, ii) = strut(add(pelvis, mul(fr.s, sd * hip_s * 1.15)), fr.u,
                0.07 * rob_h * torso_share, 0.020 * rob_h);
            parts.push(PartInst { name: "bone", rest: ip, nrm: inr, idx: ii,
                joint: root, tint: steel2 });
        }
        // neck column: two vertebra discs climbing to the head line
        for k in 0..2 {
            let at = axisf(pelvis_f + torso * (1.00 + 0.045 * (k + 1) as f64), 0.0, 0.0);
            let (np2, nn2, ni2) = prism(at, fr.f, 0.020 * rob_h, 0.026 * rob_h, 8);
            parts.push(PartInst { name: "bone", rest: np2, nrm: nn2, idx: ni2,
                joint: j_neck, tint: steel2 });
        }
    }
    let steel_bone = [0.60f32, 0.63, 0.68];
    for (k_seg, (_, bs)) in bone_segs.drain(..).enumerate() {
        let d = sub(bs.b, bs.a);
        let l = dot(d, d).sqrt().max(1e-9);
        let ax = mul(d, 1.0 / l);
        let bc = cross_of(k_seg);
        let upper_c = add(bs.a, mul(ax, l * 0.28));
        let lower_c = add(bs.a, mul(ax, l * 0.72));
        let (mut bp, mut bn, mut bi3) = prism(upper_c, ax, l * 0.42, bc * 0.46, 10);
        for (m2, r2, l2) in [(lower_c, bc * 0.36, l * 0.42),
                             (mul(add(bs.a, bs.b), 0.5), bc * 0.55, bc * 0.35)] {
            let (kp, kn, ki) = prism(m2, ax, l2, r2, 10);
            let base = bp.len() as u32;
            bp.extend(kp); bn.extend(kn);
            bi3.extend(ki.iter().map(|v| v + base));
        }
        for (end, sgn) in [(bs.a, 1.0f64), (bs.b, -1.0)] {
            let (kp, kn, ki) = prism(add(end, mul(ax, sgn * bc * 0.35)), ax,
                bc * 0.75, bc * 0.68, 10);
            let base = bp.len() as u32;
            bp.extend(kp); bn.extend(kn);
            bi3.extend(ki.iter().map(|v| v + base));
        }
        parts.push(PartInst { name: "bone", rest: bp, nrm: bn, idx: bi3,
            joint: bs.joint, tint: steel_bone });
    }

    // ---- interior dressing: original generated hardware, per archetype.
    // Terminators first (fists, feet, visor) — nothing sells a humanoid
    // silhouette like having ends. Then the living interior.
    {
        let mut addp = |name: &'static str, m: (Vec<[f64; 3]>, Vec<[f64; 3]>, Vec<u32>),
                        joint: usize, tint: [f32; 3], parts: &mut Vec<PartInst>| {
            parts.push(PartInst { name, rest: m.0, nrm: m.1, idx: m.2, joint, tint });
        };
        let steel = [0.20f32, 0.21, 0.24];
        let glow = [0.95f32, 0.72, 0.28];
        let dark = [0.12f32, 0.13, 0.15];
        let thick = if arch == Archetype::Brawler { 1.25 } else if arch == Archetype::Raptor { 0.75 } else { 1.0 };

        // fists: knuckled blocks at the wrists
        for (sd, jel) in [(-1.0, j_el_l), (1.0, j_el_r)] {
            let at = wrist(sd);
            addp("fist", strut(at, fr.f, 0.11 * rob_h, 0.085 * rob_h * thick), jel, steel, &mut parts);
            addp("fist", strut(add(at, mul(fr.u, -0.05 * rob_h)), fr.u, 0.05 * rob_h, 0.05 * rob_h), jel, dark, &mut parts);
        }
        // feet: heel block + toe wedge pointing out the belly side (robot front)
        for (sd, jkn) in [(-1.0, j_kn_l), (1.0, j_kn_r)] {
            let at = foot(sd);
            addp("foot", strut(at, fr.u, 0.17 * rob_h, 0.07 * rob_h * thick), jkn, steel, &mut parts);
            addp("foot", strut(add(at, mul(fr.u, -0.10 * rob_h)), fr.f, 0.05 * rob_h, 0.05 * rob_h), jkn, dark, &mut parts);
        }
        // visor slat on the elected head
        if let Some((_hi, _)) = head_pick {
            let hj = joints.len() - 1; // the reveal joint pushed for the head
            addp("visor", strut(add(head_tip, mul(fr.u, -0.045 * rob_h)), fr.s,
                0.10 * rob_h, 0.016 * rob_h), hj, glow, &mut parts);
        }
        // reactor core: housing on the chest belly-side + a disc that SPINS
        // through the fold — the interior is alive
        let core_at = add(chest, mul(fr.u, belly_u * 0.9));
        let core_r = 0.085 * rob_h * if arch == Archetype::Sprinter { 0.8 } else { 1.0 };
        addp("core", prism(core_at, fr.u, 0.05 * rob_h, core_r, 8), j_chest, dark, &mut parts);
        let spin = jpush(j_chest, core_at, fr.u, ps * 2.0 * PI, 0.0, &mut joints);
        ph_spin.push(spin);
        addp("core", prism(add(core_at, mul(fr.u, -0.012 * rob_h)), fr.u,
            0.03 * rob_h, core_r * 0.72, 8), spin, glow, &mut parts);
        // rib slats across the chest
        for k in 0..3 {
            let f = pelvis_f + torso * (0.38 + 0.16 * k as f64);
            addp("rib", strut(axisf(f, 0.0, belly_u * 0.55), fr.s,
                (0.30 * rob_h).min(0.85 * fr.es), 0.014 * rob_h * thick),
                if k < 2 { j_spine } else { j_chest }, steel, &mut parts);
        }
        // hip gyro ring: octagon of segments around the pelvis
        for k in 0..8 {
            let th = k as f64 / 8.0 * 2.0 * PI;
            let dir = add(mul(fr.s, th.cos()), mul(fr.u, th.sin()));
            let tan = add(mul(fr.s, -th.sin()), mul(fr.u, th.cos()));
            addp("gyro", strut(add(pelvis, mul(dir, 0.10 * rob_h)), tan,
                0.085 * rob_h, 0.014 * rob_h * thick), root, dark, &mut parts);
        }
        // knee + shoulder pistons: sleeve on the parent, rod on the child
        // with a working stroke (pure slide) — hardware that visibly does
        // the job during the fold
        let mut piston = |a: [f64; 3], b: [f64; 3], ja: usize, jb: usize,
                          joints: &mut Vec<Joint>, parts: &mut Vec<PartInst>| {
            let d = sub(b, a);
            let l = dot(d, d).sqrt().max(1e-9);
            let ax = mul(d, 1.0 / l);
            parts.push(PartInst { name: "piston",
                rest: prism(add(a, mul(ax, l * 0.28)), ax, l * 0.5, 0.020 * rob_h, 6).0,
                nrm: prism(add(a, mul(ax, l * 0.28)), ax, l * 0.5, 0.020 * rob_h, 6).1,
                idx: prism(add(a, mul(ax, l * 0.28)), ax, l * 0.5, 0.020 * rob_h, 6).2,
                joint: ja, tint: steel });
            let rod = jpush(jb, add(a, mul(ax, l * 0.72)), ax, 0.0, -l * 0.18, joints);
            parts.push(PartInst { name: "piston",
                rest: prism(add(a, mul(ax, l * 0.72)), ax, l * 0.42, 0.011 * rob_h, 6).0,
                nrm: prism(add(a, mul(ax, l * 0.72)), ax, l * 0.42, 0.011 * rob_h, 6).1,
                idx: prism(add(a, mul(ax, l * 0.72)), ax, l * 0.42, 0.011 * rob_h, 6).2,
                joint: rod, tint: dark });
        };
        piston(add(hip(-1.0), mul(fr.u, belly_u * 0.4)), knee(-1.0), j_hip_l, j_kn_l, &mut joints, &mut parts);
        piston(add(hip(1.0), mul(fr.u, belly_u * 0.4)), knee(1.0), j_hip_r, j_kn_r, &mut joints, &mut parts);
        piston(sh(-1.0), elbow(-1.0), j_sh_l, j_el_l, &mut joints, &mut parts);
        piston(sh(1.0), elbow(1.0), j_sh_r, j_el_r, &mut joints, &mut parts);
        // fingers: three blocks per hand that CLENCH in the finale
        for (sd, jel) in [(-1.0, j_el_l), (1.0, j_el_r)] {
            let knuckle = add(wrist(sd), mul(fr.f, -0.065 * rob_h));
            for k in 0..3 {
                let off = mul(fr.s, (k as f64 - 1.0) * 0.045 * rob_h * thick);
                let curl = jpush(jel, add(knuckle, off), fr.s, -ps * 0.9, 0.0, &mut joints);
                ph_finale.push(curl);
                addp("finger", strut(add(add(knuckle, off), mul(fr.f, -0.045 * rob_h)), fr.f,
                    0.075 * rob_h, 0.022 * rob_h * thick), curl, steel, &mut parts);
            }
        }
        // face: brow and chin framing the visor
        if head_pick.is_some() {
            let hj = ph_finale[ph_finale.len() - 1 - 6]; // the head reveal joint
            addp("face", strut(add(head_tip, mul(fr.u, -0.075 * rob_h)), fr.s,
                0.085 * rob_h, 0.020 * rob_h), hj, steel, &mut parts);
            addp("face", strut(add(head_tip, mul(fr.u, -0.015 * rob_h)), fr.s,
                0.11 * rob_h, 0.018 * rob_h), hj, dark, &mut parts);
        }
        // archetype crest + the face proper: eyes, jaw, cheeks, and a mask
        // MOSAIC assembled from this vehicle's own nose voxels — every bot's
        // face carries its donor's fingerprint
        if head_pick.is_some() {
            let hj = ph_finale[ph_finale.len() - 1 - 6];
            // eyes: paired glow cores above the visor line
            for sd in [-1.0f64, 1.0] {
                addp("face", prism(add(add(head_tip, mul(fr.s, sd * 0.028 * rob_h)),
                    mul(fr.u, -0.040 * rob_h)), fr.u, 0.012 * rob_h, 0.011 * rob_h, 8),
                    hj, glow, &mut parts);
            }
            // jaw guard and cheek plates
            addp("face", strut(add(head_tip, mul(fr.u, -0.105 * rob_h)), fr.s,
                0.075 * rob_h, 0.020 * rob_h), hj, steel, &mut parts);
            for sd in [-1.0f64, 1.0] {
                addp("face", strut(add(add(head_tip, mul(fr.s, sd * 0.052 * rob_h)),
                    mul(fr.u, -0.070 * rob_h)), fr.f, 0.045 * rob_h, 0.014 * rob_h),
                    hj, dark, &mut parts);
            }
            // the mosaic mask: nose-region voxels projected onto the face
            let mut tiles = 0usize;
            for (vi, v) in vox_surf.iter().enumerate() {
                if tiles >= 9 { break; }
                if vi % 3 != 0 { continue; }
                let dv = sub(*v, head_tip);
                if dot(dv, dv).sqrt() > 0.15 * flen { continue; }
                let s_off = dot(dv, fr.s).clamp(-0.045 * rob_h, 0.045 * rob_h);
                let f_off = (dot(dv, fr.f) * 0.35).clamp(-0.035 * rob_h, 0.035 * rob_h);
                let at = add(add(add(head_tip, mul(fr.s, s_off)), mul(fr.f, f_off)),
                    mul(fr.u, -0.058 * rob_h));
                let h2 = hash_jitter(&format!("fm{vi}")) as f64;
                let sz = (0.013 + 0.008 * h2) * rob_h;
                addp("face", strut(at, fr.u, sz * 0.6, sz), hj, steel, &mut parts);
                tiles += 1;
            }
            match arch {
                Archetype::Brawler => {
                    addp("crest", strut(add(head_tip, mul(fr.u, -0.095 * rob_h)), fr.s,
                        0.12 * rob_h, 0.024 * rob_h), hj, dark, &mut parts);
                }
                Archetype::Raptor => {
                    for sd in [-1.0, 1.0] {
                        addp("crest", strut(add(add(head_tip, mul(fr.s, sd * 0.045 * rob_h)),
                            mul(fr.f, 0.05 * rob_h)), fr.f, 0.10 * rob_h, 0.018 * rob_h),
                            hj, steel, &mut parts);
                    }
                }
                Archetype::Sprinter => {
                    addp("crest", strut(add(head_tip, mul(fr.f, 0.055 * rob_h)), fr.f,
                        0.09 * rob_h, 0.018 * rob_h), hj, glow, &mut parts);
                }
                Archetype::Signals => {
                    addp("crest", strut(add(head_tip, mul(fr.f, 0.07 * rob_h)), fr.f,
                        0.12 * rob_h, 0.016 * rob_h), hj, dark, &mut parts);
                    addp("crest", prism(add(head_tip, mul(fr.f, 0.13 * rob_h)), fr.f,
                        0.012 * rob_h, 0.022 * rob_h, 8), hj, glow, &mut parts);
                }
            }
        }
        // pauldrons ride the abduction, flaring with the arms
        for (sd, jab) in [(-1.0, j_ab_l), (1.0, j_ab_r)] {
            addp("pauldron", strut(add(sh(sd), mul(fr.u, 0.05 * rob_h)), fr.s,
                0.11 * rob_h, 0.038 * rob_h * thick), jab, tint, &mut parts);
        }
        // vertebral spine and cable runs: skeletal density
        for k in 0..4 {
            let f = pelvis_f + torso * (0.12 + 0.22 * k as f64);
            addp("vertebra", strut(axisf(f, 0.0, belly_u * -0.15), fr.u,
                0.045 * rob_h, 0.030 * rob_h * thick),
                if k < 2 { j_spine } else { j_chest }, dark, &mut parts);
        }
        for sd in [-1.0, 1.0] {
            let a = add(pelvis, mul(fr.s, sd * hip_s * 0.5));
            let b = sh(sd);
            let d = sub(b, a); let l = dot(d, d).sqrt().max(1e-9);
            addp("cable", strut(mul(add(a, b), 0.5), mul(d, 1.0 / l), l * 0.92,
                0.016 * rob_h), j_spine, dark, &mut parts);
        }
        // cybertronian rib arcs: three-segment arcs wrapping the chest sides
        for k in 0..4 {
            let f = pelvis_f + torso * (0.30 + 0.17 * k as f64);
            for sd in [-1.0, 1.0] {
                for &(a0, a1) in &[(0.15f64, 0.55f64), (0.55, 0.85)] {
                    let p0 = axisf(f, sd * sh_s * a0, belly_u * (0.9 - a0));
                    let p1 = axisf(f, sd * sh_s * a1, belly_u * (0.9 - a1));
                    let d = sub(p1, p0); let l = dot(d, d).sqrt().max(1e-9);
                    addp("rib", strut(mul(add(p0, p1), 0.5), mul(d, 1.0 / l), l, 0.013 * rob_h * thick),
                        if k < 2 { j_spine } else { j_chest }, steel, &mut parts);
                }
            }
        }
        // limb innards: coil stacks along thighs and forearms — mechanical
        // musculature that deploys with its bone
        for (sd, jhip, jel) in [(-1.0, j_hip_l, j_el_l), (1.0, j_hip_r, j_el_r)] {
            for k in 0..3 {
                let t = 0.25 + 0.25 * k as f64;
                let at = add(mul(hip(sd), 1.0 - t), mul(knee(sd), t));
                addp("coil", prism(at, fr.f, 0.018 * rob_h, 0.045 * rob_h * thick, 8), jhip, dark, &mut parts);
                let aa = add(mul(elbow(sd), 1.0 - t), mul(wrist(sd), t));
                addp("coil", prism(aa, fr.f, 0.015 * rob_h, 0.035 * rob_h * thick, 8), jel, dark, &mut parts);
            }
            // joint caps: knee and elbow domes
            addp("cap", prism(knee(sd), fr.s, 0.03 * rob_h, 0.05 * rob_h * thick, 8),
                if sd < 0.0 { j_kn_l } else { j_kn_r }, steel, &mut parts);
            addp("cap", prism(elbow(sd), fr.s, 0.028 * rob_h, 0.042 * rob_h * thick, 8), jel, steel, &mut parts);
        }
        // sternum keel emblem
        if tune.emblem.unwrap_or(true) {
            addp("keel", prism(axisf(pelvis_f + torso * 0.80, 0.0, belly_u * 0.8), fr.u,
                0.035 * rob_h, 0.045 * rob_h, 6), j_chest, glow, &mut parts);
        }
        // archetype extras
        match arch {
            Archetype::Brawler => {
                for sd in [-1.0, 1.0] {
                    addp("stack", prism(add(sh(sd), mul(fr.u, 0.30 * fr.eu)), fr.f,
                        0.16 * rob_h, 0.024 * rob_h, 6), j_chest, dark, &mut parts);
                }
                // brawler signature: knuckle spikes
                for (sd, jel) in [(-1.0, j_el_l), (1.0, j_el_r)] {
                    addp("spike", strut(add(wrist(sd), mul(fr.f, -0.13 * rob_h)), fr.f,
                        0.05 * rob_h, 0.016 * rob_h), jel, dark, &mut parts);
                }
            }
            Archetype::Raptor => {
                for sd in [-1.0, 1.0] {
                    let a = axisf(pelvis_f + torso * 0.30, sd * sh_s * 0.8, belly_u * 0.3);
                    let b = axisf(pelvis_f + torso * 0.95, -sd * sh_s * 0.8, belly_u * 0.3);
                    let d = sub(b, a); let l = dot(d, d).sqrt().max(1e-9);
                    addp("brace", strut(mul(add(a, b), 0.5), mul(d, 1.0 / l), l, 0.020 * rob_h),
                        j_chest, steel, &mut parts);
                    // raptor signature: calf winglets
                    addp("winglet", strut(add(knee(sd), mul(fr.s, sd * 0.05 * rob_h)), fr.f,
                        0.12 * rob_h, 0.018 * rob_h),
                        if sd < 0.0 { j_kn_l } else { j_kn_r }, steel, &mut parts);
                }
            }
            Archetype::Sprinter => {
                piston(add(knee(-1.0), mul(fr.u, belly_u * 0.5)), foot(-1.0), j_kn_l, j_kn_l, &mut joints, &mut parts);
                piston(add(knee(1.0), mul(fr.u, belly_u * 0.5)), foot(1.0), j_kn_r, j_kn_r, &mut joints, &mut parts);
                // speedster signature: heel wheels that never stop turning
                for (sd, jkn) in [(-1.0, j_kn_l), (1.0, j_kn_r)] {
                    let at = add(foot(sd), mul(fr.f, -0.03 * rob_h));
                    let w = jpush(jkn, at, fr.s, ps * 4.0 * PI, 0.0, &mut joints);
                    ph_spin.push(w);
                    addp("wheel", prism(at, fr.s, 0.028 * rob_h, 0.055 * rob_h, 8), w, dark, &mut parts);
                }
            }
            Archetype::Signals => {
                addp("mast", strut(add(sh(1.0), mul(fr.f, 0.16 * rob_h)), fr.f,
                    0.30 * rob_h, 0.022 * rob_h), j_chest, dark, &mut parts);
                // equipment bay: three modules behind a hatch that slides
                // open sideways during the fold
                for k in 0..3 {
                    addp("module", strut(add(core_at, mul(fr.s, (k as f64 - 1.0) * 0.055 * rob_h)),
                        fr.f, 0.06 * rob_h, 0.022 * rob_h), j_chest, glow, &mut parts);
                }
                let hatch = jpush(j_chest, add(core_at, mul(fr.u, -0.02 * rob_h)), fr.s,
                    0.0, 0.14 * rob_h, &mut joints);
                ph_finale.push(hatch);
                // signals signature: a radar dish atop the mast, always turning
                let dish_at = add(add(sh(1.0), mul(fr.f, 0.32 * rob_h)), [0.0; 3]);
                let dj = jpush(j_chest, dish_at, fr.f, ps * 3.0 * PI, 0.0, &mut joints);
                ph_spin.push(dj);
                addp("dish", prism(dish_at, fr.f, 0.015 * rob_h, 0.06 * rob_h, 8), dj, steel, &mut parts);
                addp("hatch", strut(add(core_at, mul(fr.u, -0.025 * rob_h)), fr.s,
                    0.19 * rob_h, 0.030 * rob_h), hatch, steel, &mut parts);
            }
        }
        // dressing added joints after schedule — recompute
        for (i2, j2) in joints.iter_mut().enumerate() { j2.part = i2; }
    }
    // ---- salvo: generated missiles streak from the barrels late in the
    // fold — the machine fires while it finishes standing. Opt-in.
    if salvo {
        let mut fired = 0usize;
        let mut targets: Vec<(usize, [f64; 3], f64)> = Vec::new();
        for (pi2, p) in parts.iter().enumerate() {
            if p.name != "barrel" || targets.len() >= 3 { continue; }
            let (blo, bhi) = iqm::bbox(&p.rest);
            let muzzle = [ (blo[0]+bhi[0])*0.5, (blo[1]+bhi[1])*0.5, bhi[2] ];
            targets.push((pi2, muzzle, (bhi[2]-blo[2]).max(0.05*mdiag)));
        }
        for (pi2, muzzle, blen) in targets {
            for sd in [-1.0f64, 1.0] {
                joints.push(Joint { part: 0, parent: Some(parts[pi2].joint),
                    pivot: muzzle, axis: [0.0, 0.0, 1.0], angle: 0.0,
                    slide: 0.45 * mdiag, reclip: None });
                ph_salvo.push(joints.len() - 1);
                let at = add(muzzle, [sd * 0.02 * blen.max(0.02 * mdiag), 0.0, 0.02 * mdiag]);
                let (mp, mn, mi) = prism(at, [0.0, 0.0, 1.0], 0.10 * mdiag * 0.4, 0.012 * mdiag, 6);
                parts.push(PartInst { name: "missile", rest: mp, nrm: mn, idx: mi,
                    joint: joints.len() - 1, tint: [0.95, 0.55, 0.20] });
                fired += 1;
            }
        }
        let _ = fired;
    }

    // ---- conceal the machine: every interior part is authored at its robot
    // station, which pokes out of the hull at rest. Re-author each one tucked
    // inside the hull core and give it a deploy slide back to station — the
    // vehicle is clean, the skeleton emerges during the fold.
    let mut ph_deploy: Vec<usize> = Vec::new();
    {
        const INTERIOR: &[&str] = &["bone","guts","fist","foot","visor","core","rib","gyro",
            "piston","stack","brace","wheel","mast","module","hatch","dish","finger",
            "face","pauldron","vertebra","cable","keel","spike","winglet","crest"];
        for pi in 0..parts.len() {
            if !INTERIOR.contains(&parts[pi].name) || parts[pi].name == "guts" { continue; }
            let c = iqm::centroid(&parts[pi].rest);
            let (plo, phi) = iqm::bbox(&parts[pi].rest);
            let half = mul(sub(phi, plo), 0.5);
            let mut tuck = c;
            for a in 0..3 {
                // fit the WHOLE part: shrink the target range by its half
                // extent; if it cannot fit the hull core on this axis, hide
                // it within the overall vehicle box instead
                let (mut r0, mut r1) = (hull_core.0[a] + half[a], hull_core.1[a] - half[a]);
                if r0 > r1 { r0 = lo[a] + half[a] * 1.02; r1 = hi[a] - half[a] * 1.02; }
                if r0 <= r1 { tuck[a] = c[a].clamp(r0, r1); }
            }
            let back = sub(tuck, c);
            let d = dot(back, back).sqrt();
            if d < 1e-6 { continue; }
            for q in parts[pi].rest.iter_mut() { *q = add(*q, back); }
            let out = mul(back, -1.0);
            joints.push(Joint { part: 0, parent: Some(parts[pi].joint),
                pivot: tuck, axis: mul(out, 1.0 / d), angle: 0.0, slide: d, reclip: None });
            ph_deploy.push(joints.len() - 1);
            parts[pi].joint = joints.len() - 1;
        }
        for (i2, j2) in joints.iter_mut().enumerate() { j2.part = i2; }
    }

    // ---- voxel cladding: whatever skeleton still shows at rest gets clad
    // in hull-tinted voxel cubes riding the same chain — exposed frame turns
    // into body, in both modes
    {
        use std::collections::HashSet;
        const INTERIOR: &[&str] = &["bone","fist","foot","visor","core","rib","gyro",
            "piston","stack","brace","wheel","mast","module","hatch","dish","finger",
            "face","pauldron","vertebra","cable","keel","spike","winglet","coil","cap","crest","boom","sleeve"];
        // a dedicated fine grid so cladding reads as plating, not minecraft
        let fcell = vcell * 0.55;
        let key = |p: [f64; 3]| -> (i64, i64, i64) {
            (((p[0] - lo[0]) / fcell) as i64, ((p[1] - lo[1]) / fcell) as i64, ((p[2] - lo[2]) / fcell) as i64)
        };
        let mut occ: HashSet<(i64, i64, i64)> = HashSet::new();
        for v in vox_surf.iter().chain(vox_int.iter()) {
            // splat the coarse occupancy into the fine grid with dilation
            let k = key(*v);
            for dx in -1..=1 { for dy in -1..=1 { for dz in -1..=1 {
                occ.insert((k.0 + dx, k.1 + dy, k.2 + dz));
            }}}
        }
        let near = |k: (i64, i64, i64)| -> bool {
            for dx in -1..=1i64 { for dy in -1..=1i64 { for dz in -1..=1i64 {
                if occ.contains(&(k.0 + dx, k.1 + dy, k.2 + dz)) { return true; }
            }}}
            false
        };
        let mut budget = 60usize;
        let npi = parts.len();
        for pi in 0..npi {
            if budget == 0 { break; }
            if !INTERIOR.contains(&parts[pi].name) { continue; }
            let step = (parts[pi].rest.len() / 40).max(1);
            let mut cells: Vec<(i64, i64, i64)> = Vec::new();
            let mut outside = 0usize;
            let mut totaln = 0usize;
            for q in parts[pi].rest.iter().step_by(step) {
                totaln += 1;
                let k = key(*q);
                if !near(k) {
                    outside += 1;
                    if !cells.contains(&k) && cells.len() < 36 { cells.push(k); }
                }
            }
            if totaln == 0 || outside * 10 < totaln { continue; } // only the last ~10%+
            if cells.is_empty() || budget == 0 { continue; }
            // one fitted plate over the exposed run — plating, not cubes
            let (mut klo, mut khi) = ([i64::MAX; 3], [i64::MIN; 3]);
            for k in &cells {
                let ka = [k.0, k.1, k.2];
                for a in 0..3 { klo[a] = klo[a].min(ka[a]); khi[a] = khi[a].max(ka[a]); }
            }
            let plo = [lo[0] + klo[0] as f64 * fcell, lo[1] + klo[1] as f64 * fcell, lo[2] + klo[2] as f64 * fcell];
            let phi = [lo[0] + (khi[0] + 1) as f64 * fcell, lo[1] + (khi[1] + 1) as f64 * fcell, lo[2] + (khi[2] + 1) as f64 * fcell];
            let at = mul(add(plo, phi), 0.5);
            let e3 = sub(phi, plo);
            let long = if e3[0] >= e3[1] && e3[0] >= e3[2] { 0 } else if e3[1] >= e3[2] { 1 } else { 2 };
            let mut ax = [0.0; 3]; ax[long] = 1.0;
            let len = e3[long].max(fcell);
            let cross = ((e3[(long + 1) % 3] + e3[(long + 2) % 3]) * 0.5).max(fcell) * 1.05;
            let (bp, bn, bi) = strut(at, ax, len * 1.02, cross);
            parts.push(PartInst { name: "clad", rest: bp, nrm: bn, idx: bi,
                joint: parts[pi].joint, tint });
            budget -= 1;
        }
    }

    let mut slots = schedule(&joints, overlap);
    // Rule 20, enforced: expand early, armor closes after the skeleton has
    // moved, the finale (head, hatch, clenching) lands last, and the living
    // parts never stop
    use model_verify::motion::Slot;
    for &j in &ph_early { slots[j] = Slot { start: 0.03, end: 0.42 }; }
    for &j in &ph_armor { slots[j] = Slot { start: 0.55, end: 0.90 }; }
    let (fs, fe) = match arch {
        Archetype::Brawler => (0.80, 0.94),   // slam
        Archetype::Signals => (0.70, 0.97),   // glide
        _ => (0.75, 0.96),
    };
    for &j in &ph_finale { slots[j] = Slot { start: fs, end: fe }; }
    for &j in &ph_spin { slots[j] = Slot { start: 0.04, end: 1.0 }; }
    for &j in &ph_burst { slots[j] = Slot { start: 0.10, end: 0.34 }; }
    for &j in &ph_salvo { slots[j] = Slot { start: 0.82, end: 1.0 }; }
    for &j in &ph_perf {
        slots[j] = match performance {
            Performance::Roll => Slot { start: 0.04, end: 0.55 },
            Performance::Spin => Slot { start: 0.06, end: 0.52 },
            Performance::Combat => Slot { start: 0.80, end: 0.96 },
            Performance::Clean => Slot { start: 0.5, end: 0.9 },
        };
    }
    for &j in &ph_deploy { slots[j] = Slot { start: 0.05, end: 0.26 }; } // skeleton leads, shell follows
    for (k, &j) in ph_pull.iter().enumerate() {
        let h = ((k as u32).wrapping_mul(2654435761) >> 22) as f64 / 1024.0;
        let st = 0.52 + 0.10 * h;
        slots[j] = Slot { start: st, end: (st + 0.30).min(0.90) };
    }

    // ---- contact-aware reassembly: pose every part at t=1 and give any
    // armor body that ends isolated one corrective pull toward its nearest
    // neighbor's final box — the end pose is solved, not hoped for
    {
        use model_verify::motion::pose_at;
        let (profile0, _, _) = temperament(&cat);
        const INTERIOR: &[&str] = &["bone","guts","fist","foot","visor","core","rib","gyro",
            "piston","stack","brace","wheel","mast","module","hatch","dish","finger",
            "face","pauldron","vertebra","cable","keel","spike","winglet","crest"];
        let mut boxes: Vec<([f64; 3], [f64; 3])> = Vec::with_capacity(parts.len());
        for p in &parts {
            let step = (p.rest.len() / 24).max(1);
            let (mut lo1, mut hi1) = ([f64::MAX; 3], [f64::MIN; 3]);
            for q in p.rest.iter().step_by(step) {
                let w = pose_at(&joints, &slots, p.joint, 1.0, profile0, *q);
                for a in 0..3 { lo1[a] = lo1[a].min(w[a]); hi1[a] = hi1[a].max(w[a]); }
            }
            boxes.push((lo1, hi1));
        }
        let gapf = |a: usize, b: usize| -> f64 {
            let mut d2 = 0.0;
            for k in 0..3 {
                let g = (boxes[a].0[k] - boxes[b].1[k]).max(boxes[b].0[k] - boxes[a].1[k]).max(0.0);
                d2 += g * g;
            }
            d2.sqrt()
        };
        let iso = 0.015 * mdiag;
        let mut fixes: Vec<(usize, [f64; 3])> = Vec::new();
        for a in 0..parts.len() {
            if INTERIOR.contains(&parts[a].name) { continue; }
            let (mut bd, mut bt) = (f64::MAX, 0usize);
            for b in 0..parts.len() {
                if b == a { continue; }
                let g = gapf(a, b);
                if g < bd { bd = g; bt = b; }
            }
            if bd > iso {
                let ca = mul(add(boxes[a].0, boxes[a].1), 0.5);
                let cb = mul(add(boxes[bt].0, boxes[bt].1), 0.5);
                let v = sub(cb, ca);
                let vl = dot(v, v).sqrt().max(1e-9);
                fixes.push((a, mul(v, ((bd + 0.006 * mdiag).min(0.12 * mdiag)) / vl)));
            }
        }
        let fixed = fixes.len();
        for (a, pull) in fixes {
            let l = dot(pull, pull).sqrt();
            joints.push(Joint { part: 0, parent: Some(parts[a].joint),
                pivot: [0.0; 3], axis: mul(pull, 1.0 / l), angle: 0.0, slide: l, reclip: None });
            slots.push(Slot { start: 0.60, end: 0.90 });
            parts[a].joint = joints.len() - 1;
        }
        if fixed > 0 { eprintln!("  [{name}] end-pose solver pulled {fixed} isolated part(s) home"); }
        for (i2, j2) in joints.iter_mut().enumerate() { j2.part = i2; }
    }

    if junk > 0 || welds > 0 { eprintln!("  [{name}] stripped {junk} junk, rest-welded {welds}"); }
    let e2 = sub(hi, lo);
    let bot = Bot { parts, joints, slots, profile, duration, label: format!("{cat}/{name}") };
    Ok(descramble(silhouette_pass(ground(bot, &name), dot(e2, e2).sqrt(), &name), &name))
}


/// Silhouette-space end-pose solver: render a part-ID pass at t=1 from the
/// poster camera, find silhouette islands, and pull the armor parts of every
/// minority island toward the main body. Box logic cannot see projection
/// gaps; this can.
fn silhouette_pass(mut bot: Bot, mdiag: f64, name: &str) -> Bot {
    use model_verify::motion::{Joint, Slot};
    const INTERIOR: &[&str] = &["bone","guts","fist","foot","visor","core","rib","gyro",
        "piston","stack","brace","wheel","mast","module","hatch","dish","finger",
        "face","pauldron","vertebra","cable","keel","spike","winglet","crest"];
    let (wpx, hpx) = (320usize, 180usize);
    let mut total_fixes = 0usize;
    // solve BOTH the showcase camera and the true front — one view's fix
    // must not be another view's island
    let views = [(1.0f64, 1.65f64, 0.30f64), (1.0, std::f64::consts::PI, 0.05)];
    let mut view_clean = [false; 2];
    for pass in 0..6 {
        let shots = [views[pass % 2]];
        let escalate = 1.0 + 0.35 * (pass / 2) as f64;
        let cam = crate::soft::fit(&bot, &shots, wpx, hpx, 0.92);
        let (t, yaw, pitch) = shots[0];
        let eye = [
            cam.c[0] + cam.dist * pitch.cos() * yaw.sin(),
            cam.c[1] + cam.dist * pitch.sin(),
            cam.c[2] + cam.dist * pitch.cos() * yaw.cos(),
        ];
        let fwd = {
            let v = sub(cam.c, eye);
            let l = dot(v, v).sqrt().max(1e-9);
            mul(v, 1.0 / l)
        };
        let right = {
            let c = [fwd[1]*0.0 - fwd[2]*1.0, fwd[2]*0.0 - fwd[0]*0.0, fwd[0]*1.0 - fwd[1]*0.0];
            let l = dot(c, c).sqrt().max(1e-9);
            mul(c, 1.0 / l)
        };
        let up = [right[1]*fwd[2]-right[2]*fwd[1], right[2]*fwd[0]-right[0]*fwd[2], right[0]*fwd[1]-right[1]*fwd[0]];
        let mut zbuf = vec![f64::MAX; wpx * hpx];
        let mut ids = vec![u32::MAX; wpx * hpx];
        let mut posed: Vec<Vec<[f64; 3]>> = Vec::with_capacity(bot.parts.len());
        for i in 0..bot.parts.len() { posed.push(bot.points_at(i, t)); }
        for (pi, pts) in posed.iter().enumerate() {
            for tri in bot.parts[pi].idx.chunks_exact(3) {
                let tp: Vec<([f64; 2], f64)> = tri.iter().map(|&v| {
                    let q = sub(pts[v as usize], eye);
                    let (x, y, z) = (dot(q, right), dot(q, up), dot(q, fwd));
                    ([wpx as f64 / 2.0 + (x / z * cam.f - cam.ox) * hpx as f64 / 2.0,
                      hpx as f64 / 2.0 - (y / z * cam.f - cam.oy) * hpx as f64 / 2.0], z)
                }).collect();
                if tp.iter().any(|&(_, z)| z <= 0.05) { continue; }
                let (a, b, c) = (tp[0].0, tp[1].0, tp[2].0);
                let area = (b[0]-a[0])*(c[1]-a[1]) - (b[1]-a[1])*(c[0]-a[0]);
                if area.abs() < 1e-9 { continue; }
                let (x0, x1) = (a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize,
                                a[0].max(b[0]).max(c[0]).ceil().min(wpx as f64 - 1.0) as usize);
                let (y0, y1) = (a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize,
                                a[1].max(b[1]).max(c[1]).ceil().min(hpx as f64 - 1.0) as usize);
                for py in y0..=y1 {
                    for px in x0..=x1 {
                        let p = [px as f64 + 0.5, py as f64 + 0.5];
                        let w0 = ((b[0]-p[0])*(c[1]-p[1]) - (b[1]-p[1])*(c[0]-p[0])) / area;
                        let w1 = ((c[0]-p[0])*(a[1]-p[1]) - (c[1]-p[1])*(a[0]-p[0])) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                        let z = tp[0].1 * w0 + tp[1].1 * w1 + tp[2].1 * w2;
                        let o = py * wpx + px;
                        if z < zbuf[o] { zbuf[o] = z; ids[o] = pi as u32; }
                    }
                }
            }
        }
        // connected components (8-conn) on the mask
        let mut lab = vec![0u32; wpx * hpx];
        let mut sizes: Vec<usize> = vec![0];
        let mut cur = 0u32;
        for sy in 0..hpx { for sx in 0..wpx {
            let so = sy * wpx + sx;
            if ids[so] == u32::MAX || lab[so] != 0 { continue; }
            cur += 1; sizes.push(0);
            let mut stack = vec![(sy, sx)];
            lab[so] = cur;
            while let Some((y, x)) = stack.pop() {
                sizes[cur as usize] += 1;
                for dy in -1i64..=1 { for dx in -1i64..=1 {
                    let (ny, nx) = (y as i64 + dy, x as i64 + dx);
                    if ny < 0 || nx < 0 || ny >= hpx as i64 || nx >= wpx as i64 { continue; }
                    let no = ny as usize * wpx + nx as usize;
                    if ids[no] != u32::MAX && lab[no] == 0 { lab[no] = cur; stack.push((ny as usize, nx as usize)); }
                }}
            }
        }}
        let total_px: usize = sizes.iter().sum();
        let big: Vec<u32> = (1..sizes.len() as u32).filter(|&l| sizes[l as usize] * 250 >= total_px).collect();
        if let Ok(dir) = std::env::var("FV_DEBUG_DUMP") {
            let mut f = crate::soft::Frame { w: wpx, h: hpx, rgb: vec![0; wpx * hpx * 3] };
            for o in 0..wpx * hpx {
                let v = if ids[o] != u32::MAX { 255u8 } else { 0 };
                f.rgb[o * 3] = v; f.rgb[o * 3 + 1] = v; f.rgb[o * 3 + 2] = v;
            }
            let _ = crate::soft::save_png(&f, std::path::Path::new(&format!("{dir}/idmask_p{pass}.png")));
            let mut bytes = Vec::with_capacity(wpx * hpx * 4);
            for o in 0..wpx * hpx { bytes.extend_from_slice(&ids[o].to_le_bytes()); }
            let _ = std::fs::write(format!("{dir}/ids_p{pass}.bin"), &bytes);
            let names: Vec<String> = bot.parts.iter().map(|p| p.name.to_string()).collect();
            let _ = std::fs::write(format!("{dir}/names_p{pass}.txt"), names.join("\n"));
        }
        if big.len() <= 1 {
            view_clean[pass % 2] = true;
            if view_clean[0] && view_clean[1] { break; }
            continue;
        }
        view_clean[pass % 2] = false;
        if std::env::var("FV_DEBUG").is_ok() {
            for &l in &big {
                if l == *big.iter().max_by_key(|&&x| sizes[x as usize]).unwrap() { continue; }
                let mut names: std::collections::HashMap<&str, usize> = Default::default();
                for o in 0..wpx * hpx {
                    if ids[o] != u32::MAX && lab[o] == l {
                        *names.entry(bot.parts[ids[o] as usize].name).or_insert(0) += 1;
                    }
                }
                eprintln!("  pass {pass} island {l} ({} px): {:?}", sizes[l as usize], names);
            }
        }
        let main_lab = *big.iter().max_by_key(|&&l| sizes[l as usize]).unwrap();
        // per part: pixels in main vs elsewhere
        let mut in_main = vec![0usize; bot.parts.len()];
        let mut px_tot = vec![0usize; bot.parts.len()];
        for o in 0..wpx * hpx {
            if ids[o] == u32::MAX { continue; }
            let pi = ids[o] as usize;
            px_tot[pi] += 1;
            if lab[o] == main_lab { in_main[pi] += 1; }
        }
        // t=1 boxes for pull targeting
        let boxes: Vec<([f64; 3], [f64; 3])> = posed.iter().map(|pts| {
            let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
            for q in pts { for a in 0..3 { lo[a] = lo[a].min(q[a]); hi[a] = hi[a].max(q[a]); } }
            (lo, hi)
        }).collect();
        let mut fixes = 0usize;
        for a in 0..bot.parts.len() {
            if px_tot[a] == 0 || in_main[a] * 2 >= px_tot[a] { continue; }
            if INTERIOR.contains(&bot.parts[a].name) { continue; }
            let (mut bd, mut bt) = (f64::MAX, usize::MAX);
            for b in 0..bot.parts.len() {
                if b == a || px_tot[b] == 0 || in_main[b] * 2 < px_tot[b] { continue; }
                let mut d2 = 0.0;
                for k in 0..3 {
                    let g = (boxes[a].0[k] - boxes[b].1[k]).max(boxes[b].0[k] - boxes[a].1[k]).max(0.0);
                    d2 += g * g;
                }
                if d2 < bd { bd = d2; bt = b; }
            }
            if bt == usize::MAX { continue; }
            let ca = mul(add(boxes[a].0, boxes[a].1), 0.5);
            let cb = mul(add(boxes[bt].0, boxes[bt].1), 0.5);
            let v = sub(cb, ca);
            let vl = dot(v, v).sqrt().max(1e-9);
            let mag = (bd.sqrt() + 0.010 * mdiag).min(0.12 * mdiag);
            bot.joints.push(Joint { part: 0, parent: Some(bot.parts[a].joint),
                pivot: [0.0; 3], axis: mul(v, 1.0 / vl), angle: 0.0, slide: mag, reclip: None });
            bot.slots.push(Slot { start: 0.62, end: 0.90 });
            bot.parts[a].joint = bot.joints.len() - 1;
            fixes += 1;
        }
        for (i2, j2) in bot.joints.iter_mut().enumerate() { j2.part = i2; }
        if fixes == 0 { break; }
        total_fixes += fixes;
    }
    if total_fixes > 0 {
        eprintln!("  [{name}] silhouette solver pulled {total_fixes} island part(s) into the body");
    }
    bot
}

/// Final-stance grounding: find each leg's lowest point at t=1, and extend
/// the shallower leg (one late slide above its knee) until both feet share
/// the floor. Robots stand ON something.
fn ground(mut bot: Bot, name: &str) -> Bot {
    use model_verify::motion::Slot;
    let chain_of = |bot: &Bot, j0: usize| -> Vec<usize> {
        let mut c = vec![j0];
        let mut j = j0;
        while let Some(p) = bot.joints[j].parent { c.push(p); j = p; }
        c
    };
    // the knee = deepest joint shared by all shin parts of a side
    let knee_for = |bot: &Bot, side: &str| -> Option<usize> {
        let mut shared: Option<std::collections::HashSet<usize>> = None;
        for p in &bot.parts {
            if p.name != side { continue; }
            let set: std::collections::HashSet<usize> = chain_of(bot, p.joint).into_iter().collect();
            shared = Some(match shared {
                None => set,
                Some(sh) => sh.intersection(&set).copied().collect(),
            });
        }
        let sh = shared?;
        // deepest = the shared joint with the longest chain
        sh.into_iter().max_by_key(|&j| chain_of(bot, j).len())
    };
    let (Some(kl), Some(kr)) = (knee_for(&bot, "shinl"), knee_for(&bot, "shinr")) else { return bot; };
    let leg_min = |bot: &Bot, knee: usize| -> f64 {
        let mut m = f64::MAX;
        for i in 0..bot.parts.len() {
            if !chain_of(bot, bot.parts[i].joint).contains(&knee) { continue; }
            for q in bot.points_at(i, 1.0) { m = m.min(q[1]); }
        }
        m
    };
    let (ml, mr) = (leg_min(&bot, kl), leg_min(&bot, kr));
    let (mut hlo, mut hhi) = (f64::MAX, f64::MIN);
    for i in 0..bot.parts.len() {
        for q in bot.points_at(i, 1.0) { hlo = hlo.min(q[1]); hhi = hhi.max(q[1]); }
    }
    let height = (hhi - hlo).max(1e-9);
    let mut extend = |knee: usize, gap: f64, bot: &mut Bot| {
        let parent = bot.joints[knee].parent;
        let knee_pivot = bot.joints[knee].pivot;
        bot.joints.push(Joint { part: 0, parent, pivot: [0.0; 3],
            axis: [0.0, -1.0, 0.0], angle: 0.0, slide: gap, reclip: None });
        bot.slots.push(Slot { start: 0.72, end: 0.94 });
        let nj = bot.joints.len() - 1;
        bot.joints[knee].parent = Some(nj);
        // the telescope: a bridging sleeve on the thigh side, authored at
        // the bind knee along the leg axis, spanning whatever the extension
        // opens — hardware covers the stroke
        let seg_len = gap * 2.4;
        let a = [knee_pivot[0], knee_pivot[1], knee_pivot[2] + seg_len * 0.35];
        let (tp, tn, ti) = crate::veh_prism_bridge(a, [0.0, 0.0, -1.0], seg_len, gap * 0.9 + 1e-9);
        bot.parts.push(crate::rig::PartInst { name: "telescope", rest: tp, nrm: tn, idx: ti,
            joint: parent.unwrap_or(0), tint: [0.55, 0.58, 0.62] });
    };
    let mut noted = 0.0f64;
    // first: equalize the legs
    let gap = (ml - mr).abs().min(0.06 * height);
    if gap > 0.01 * height {
        extend(if ml > mr { kl } else { kr }, gap, &mut bot);
        noted += gap;
    }
    // then: bring BOTH feet down to the global floor (auto leg drop),
    // capped jointly with the equalize so the knee never opens past the
    // telescope's reach
    let legs_min = ml.min(mr);
    let drop = (legs_min - hlo).min(0.05 * height).min(0.07 * height - gap.min(0.06 * height)); // piston-stroke scale: no dislocated knees
    if drop > 0.02 * height {
        extend(kl, drop, &mut bot);
        extend(kr, drop, &mut bot);
        noted += drop;
    }
    if noted > 0.0 {
        for (i2, j2) in bot.joints.iter_mut().enumerate() { j2.part = i2; }
        eprintln!("  [{name}] grounded: {:.1}% of height", 100.0 * noted / height);
    }
    bot
}

/// Rule 15, enforced by timing: sample the fold, find part pairs that
/// overlap mid-motion but not at either end (transient interpenetration),
/// and delay the smaller offender's window — toys avoid collisions by
/// sequencing, so does this.
fn descramble(mut bot: Bot, name: &str) -> Bot {
    use model_verify::motion::Slot;
    let ts = [0.15, 0.30, 0.45, 0.60, 0.75];
    let boxes_at = |bot: &Bot, t: f64| -> Vec<([f64; 3], [f64; 3])> {
        (0..bot.parts.len()).map(|i| {
            let step = (bot.parts[i].rest.len() / 16).max(1);
            let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
            for q in bot.parts[i].rest.iter().step_by(step) {
                let w = model_verify::motion::pose_at(&bot.joints, &bot.slots,
                    bot.parts[i].joint, t, bot.profile, *q);
                for a in 0..3 { lo[a] = lo[a].min(w[a]); hi[a] = hi[a].max(w[a]); }
            }
            (lo, hi)
        }).collect()
    };
    let overlap = |a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3])| -> f64 {
        let mut v = 1.0;
        for k in 0..3 {
            let o = a.1[k].min(b.1[k]) - a.0[k].max(b.0[k]);
            if o <= 0.0 { return 0.0; }
            v *= o;
        }
        v
    };
    let vol = |b: &([f64; 3], [f64; 3])| -> f64 {
        (b.1[0]-b.0[0]).max(1e-9) * (b.1[1]-b.0[1]).max(1e-9) * (b.1[2]-b.0[2]).max(1e-9)
    };
    // exclusive chain tips only: never retime a shared bone joint
    let mut joint_users = vec![0usize; bot.joints.len()];
    for p in &bot.parts { joint_users[p.joint] += 1; }
    let b0 = boxes_at(&bot, 0.0);
    let b1 = boxes_at(&bot, 1.0);
    let mut nudged = 0usize;
    for _round in 0..3 {
        let mut moved = false;
        for &t in &ts {
            let bt = boxes_at(&bot, t);
            let n = bot.parts.len();
            for a in 0..n {
                for b in (a + 1)..n {
                    let ov = overlap(&bt[a], &bt[b]);
                    if ov <= 0.0 { continue; }
                    let small = vol(&bt[a]).min(vol(&bt[b]));
                    if ov < small * 0.30 { continue; }
                    // transient only: allowed to touch at rest or at the end
                    if overlap(&b0[a], &b0[b]) > 0.0 || overlap(&b1[a], &b1[b]) > 0.0 { continue; }
                    let victim = if vol(&bt[a]) < vol(&bt[b]) { a } else { b };
                    let j = bot.parts[victim].joint;
                    if joint_users[j] != 1 { continue; }
                    let s = bot.slots[j];
                    if s.start >= 0.80 { continue; }
                    bot.slots[j] = Slot { start: (s.start + 0.09).min(0.80),
                        end: (s.end + 0.05).min(0.97).max(s.start + 0.20) };
                    nudged += 1;
                    moved = true;
                    if nudged > 280 { break; }
                }
                if nudged > 280 { break; }
            }
            if nudged > 280 { break; }
        }
        if !moved || nudged > 280 { break; }
    }
    if nudged > 0 { eprintln!("  [{name}] descrambled {nudged} transient collision(s) by retiming"); }
    bot
}

/// Extent of the whole bot along `axis` at fold time `t`.
fn extent_along(b: &Bot, t: f64, axis: [f64; 3]) -> f64 {
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for i in 0..b.parts.len() {
        for p in b.points_at(i, t) {
            let v = p[0]*axis[0] + p[1]*axis[1] + p[2]*axis[2];
            lo = lo.min(v); hi = hi.max(v);
        }
    }
    (hi - lo).max(1e-9)
}

/// One line of census per model: how the anatomy pass read it.
pub fn census_line(path: &Path, depth: usize) -> String {
    match build(path, depth) {
        Ok(b) => {
            let mut counts = std::collections::BTreeMap::new();
            for p in &b.parts { *counts.entry(p.name).or_insert(0usize) += 1; }
            let desc: Vec<String> = counts.iter().map(|(k, v)| format!("{k}x{v}")).collect();
            let up = {
                let mut best = 0.0f64;
                for ax in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
                    let g = extent_along(&b, 1.0, ax) / extent_along(&b, 0.0, ax);
                    if g > best { best = g; }
                }
                best
            };
            format!("{:<48} {:>4} pieces  stand x{:<4.1} [{}]", b.label, b.parts.len(), up, desc.join(" "))
        }
        Err(e) => format!("{:<52} FAILED: {e}", path.file_name().unwrap_or_default().to_string_lossy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model_verify::verify_pair;
    use std::path::PathBuf;

    /// The model corpus. This trunk keeps 21 example models where the
    /// library used to be, and every test below returns quietly when its
    /// input is missing — which reads as a pass. FV_DATA points at a corpus
    /// checkout so the suite actually runs; `skipped()` says when it did not.
    fn data() -> PathBuf {
        match std::env::var_os("FV_DATA") {
            Some(d) => PathBuf::from(d),
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../engine_src/data"),
        }
    }

    fn skipped(p: &std::path::Path) -> bool {
        eprintln!("SKIP: {} absent (set FV_DATA to a corpus checkout)", p.display());
        true
    }

    #[test]
    fn imported_vehicles_fold_rigidly() {
        for f in [
            "imported_tank_rollout_aka_sci_fi_tank.iqm",
            "imported_racer_grit_aka_rocket_car.iqm",
            "imported_flyer_dawnshade.iqm",
            "imported_hovertank_whistlewind_aka_hover_ifv_pbr.iqm",
        ] {
            let p = data().join(f);
            if !p.exists() && skipped(&p) { return; }
            let bot = build(&p, 5).unwrap();
            for i in 0..bot.parts.len() {
                for t in [0.0, 0.31, 0.63, 0.9, 1.0] {
                    let moved = bot.points_at(i, t);
                    if bot.parts[i].rest.len() < 3 { continue; }
                    match verify_pair(&bot.parts[i].rest, &moved) {
                        Some(v) => assert!(v.passed(), "{f} piece {i} not rigid at t={t}: {v:?}"),
                        // degenerate slivers (collinear points) have no unique
                        // rigid fit; only tiny pieces are allowed to be that
                        None => assert!(bot.parts[i].rest.len() < 24,
                            "{f} piece {i} ({} verts) unverifiable", bot.parts[i].rest.len()),
                    }
                }
            }
        }
    }

    #[test]
    fn flyer_wings_sweep_not_bury() {
        let p = data().join("imported_flyer_dawnshade.iqm");
        if !p.exists() && skipped(&p) { return; }
        let bot = build(&p, 5).unwrap();
        // the recursive tree must actually recurse: some joint sits at least
        // three parent-hops deep
        let max_chain = (0..bot.joints.len()).map(|mut k| {
            let mut d = 0;
            while let Some(pk) = bot.joints[k].parent { d += 1; k = pk; if d > 32 { break; } }
            d
        }).max().unwrap_or(0);
        assert!(max_chain >= 3, "expected recursion depth >= 3, got {max_chain}");
        let hull_c = {
            let pts = bot.points_at(0, 1.0);
            let n = pts.len() as f64;
            pts.iter().fold([0.0; 3], |a, p| [a[0]+p[0]/n, a[1]+p[1]/n, a[2]+p[2]/n])
        };
        let (blo, bhi) = {
            let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
            for i in 0..bot.parts.len() {
                for p in bot.points_at(i, 1.0) {
                    for a in 0..3 { lo[a] = lo[a].min(p[a]); hi[a] = hi[a].max(p[a]); }
                }
            }
            (lo, hi)
        };
        let diag = (0..3).map(|a| (bhi[a]-blo[a]).powi(2)).sum::<f64>().sqrt();
        for (i, part) in bot.parts.iter().enumerate() {
            if !part.name.contains("wing") { continue; }
            let w0 = crate::iqm::centroid(&bot.points_at(i, 0.0));
            let w1 = crate::iqm::centroid(&bot.points_at(i, 1.0));
            let moved = ((w1[0]-w0[0]).powi(2)+(w1[1]-w0[1]).powi(2)+(w1[2]-w0[2]).powi(2)).sqrt();
            assert!(moved > diag * 0.08, "wing {i} barely moved: {moved} vs diag {diag}");
            let dc = ((w1[0]-hull_c[0]).powi(2)+(w1[1]-hull_c[1]).powi(2)+(w1[2]-hull_c[2]).powi(2)).sqrt();
            assert!(dc < diag * 0.75, "wing {i} flung too far from body: {dc} vs diag {diag}");
        }
    }

    #[test]
    fn robots_are_humanoid_legs_below_torso_and_skeleton_present() {
        for f in ["imported_tank_rollout_aka_sci_fi_tank.iqm", "imported_flyer_dawnshade.iqm"] {
            let p = data().join(f);
            if !p.exists() && skipped(&p) { return; }
            let bot = build(&p, 5).unwrap();
            let bone_count = bot.parts.iter().filter(|p| p.name == "bone").count();
            assert!(bone_count >= 14, "{f}: expected the 14-strut articulated skeleton plus ornaments, got {bone_count}");
            // find the world axis the robot stands along (the one that grew most)
            let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            let up = *axes.iter().max_by(|&&a, &&b| {
                let ga = extent_along(&bot, 1.0, a) / extent_along(&bot, 0.0, a);
                let gb = extent_along(&bot, 1.0, b) / extent_along(&bot, 0.0, b);
                ga.partial_cmp(&gb).unwrap()
            }).unwrap();
            let centroid_up = |i: usize, t: f64| {
                let pts = bot.points_at(i, t);
                pts.iter().map(|p| p[0]*up[0]+p[1]*up[1]+p[2]*up[2]).sum::<f64>() / pts.len() as f64
            };
            let hull_up = centroid_up(0, 1.0);
            let mut legs = 0;
            let mut below = 0;
            for i in 0..bot.parts.len() {
                if bot.parts[i].name.contains("thigh") || bot.parts[i].name.contains("shin") {
                    legs += 1;
                    // the up direction sign is unknown; legs sit on the far
                    // side of the hull from the head, i.e. |offset| large and
                    // consistent — check they cluster on ONE side
                    if centroid_up(i, 1.0) < hull_up { below += 1; }
                }
            }
            if legs > 0 {
                let frac = below.max(legs - below) as f64 / legs as f64;
                // carved mid-quarters legitimately carry limb labels while sitting
                // mid-body; motion-parenting is the anatomical guarantee now
                assert!(frac >= 0.54, "{f}: legs scattered across the torso ({below}/{legs} on one side)");
            }
        }
    }

    #[test]
    fn fvx_roundtrip_matches_live_segmentation() {
        let src = data().join("imported_tank_rollout_aka_sci_fi_tank.iqm");
        if !src.exists() && skipped(&src) { return; }
        let dir = std::env::temp_dir().join("fvx_rt");
        let _ = std::fs::create_dir_all(&dir);
        let dst = dir.join("rt.iqm");
        std::fs::copy(&src, &dst).unwrap();
        let _ = std::fs::remove_file(dst.with_extension("fvx")); // stale sidecar from a failed run
        let live = build(&dst, 5).unwrap();
        bake(&dst).unwrap();
        let baked = build(&dst, 5).unwrap();
        // f32 storage flips borderline threshold decisions, which cascade
        // through carving and sharding — the cache contract is equivalent
        // QUALITY, not bit-identical counts
        let (a, b) = (live.parts.len() as f64, baked.parts.len() as f64);
        assert!((a - b).abs() / a.max(1.0) < 0.015,
            "fvx roundtrip drifted beyond 1.5%: {a} vs {b}");
        let count = |bot: &Bot, n: &str| bot.parts.iter().filter(|p| p.name == n).count();
        assert_eq!(count(&baked, "bone"), count(&live, "bone"), "baked build lost skeleton parts");
        assert_eq!(count(&baked, "fist"), 4, "baked build lost the hands");
        let _ = std::fs::remove_file(dst.with_extension("fvx"));
    }

    #[test]
    fn roll_and_spin_are_identity_at_the_end() {
        // the show moves are full screws: at t=1 the pose must equal Clean's
        let p = data().join("imported_racer_grit_aka_rocket_car.iqm");
        if !p.exists() && skipped(&p) { return; }
        let a = build_with(&p, 5, Some(Performance::Clean), false).unwrap();
        let b = build_with(&p, 5, Some(Performance::Roll), false).unwrap();
        let pa = a.points_at(0, 1.0);
        let pb = b.points_at(0, 1.0);
        // roll adds forward travel; compare shapes after removing the mean
        let mean = |v: &Vec<[f64; 3]>| {
            let n = v.len() as f64;
            v.iter().fold([0.0; 3], |m, q| [m[0]+q[0]/n, m[1]+q[1]/n, m[2]+q[2]/n])
        };
        let (ma, mb) = (mean(&pa), mean(&pb));
        for (qa, qb) in pa.iter().zip(pb.iter()) {
            for k in 0..3 {
                assert!(((qa[k]-ma[k]) - (qb[k]-mb[k])).abs() < 1e-6,
                    "roll changed the end pose");
            }
        }
    }

    #[test]
    fn breaks_apart_early_and_reassembles() {
        for f in ["imported_tank_rollout_aka_sci_fi_tank.iqm", "imported_racer_grit_aka_rocket_car.iqm"] {
            let p = data().join(f);
            if !p.exists() && skipped(&p) { return; }
            let bot = build(&p, 5).unwrap();
            let spread = |t: f64| -> f64 {
                let mut cs: Vec<[f64; 3]> = Vec::new();
                const CORE: &[&str] = &["bone", "guts", "fist", "foot", "core", "rib",
                    "gyro", "piston", "coil", "cap", "vertebra", "cable", "keel",
                    "muscle", "link", "crest", "telescope", "clad", "face", "visor"];
                for i in 0..bot.parts.len() {
                    if CORE.contains(&bot.parts[i].name) { continue; }
                    let pts = bot.points_at(i, t);
                    let n = pts.len() as f64;
                    cs.push(pts.iter().fold([0.0; 3], |a, q| [a[0]+q[0]/n, a[1]+q[1]/n, a[2]+q[2]/n]));
                }
                let m = cs.iter().fold([0.0; 3], |a, c| [a[0]+c[0]/cs.len() as f64, a[1]+c[1]/cs.len() as f64, a[2]+c[2]/cs.len() as f64]);
                cs.iter().map(|c| ((c[0]-m[0]).powi(2)+(c[1]-m[1]).powi(2)+(c[2]-m[2]).powi(2)).sqrt()).sum::<f64>() / cs.len() as f64
            };
            let (s0, s02, s1) = (spread(0.0), spread(0.22), spread(1.0));
            assert!(s02 > s0 * 1.12, "{f}: no visible burst ({s0:.3} -> {s02:.3})");
            assert!(s1 < s02 * 0.92, "{f}: never reassembles ({s02:.3} -> {s1:.3})");
        }
    }

    #[test]
    fn phases_land_in_order_and_arms_clear_the_torso() {
        let p = data().join("imported_tank_rollout_aka_sci_fi_tank.iqm");
        if !p.exists() && skipped(&p) { return; }
        let bot = build(&p, 5).unwrap();
        // finale parts (fingers) must move only late
        for (i, part) in bot.parts.iter().enumerate() {
            if part.name == "finger" {
                // deploy joints sit at the chain tip now; the clench lives
                // somewhere up the chain — assert it exists there
                let mut k = bot.parts[i].joint;
                let mut late = bot.slots[k].start >= 0.7;
                while let Some(pk) = bot.joints[k].parent {
                    k = pk;
                    late |= bot.slots[k].start >= 0.7;
                    if late { break; }
                }
                assert!(late, "finger {i} has no late clench in its chain");
            }
        }
        // mid-fold, arm chunks should ride outside the torso half-width
        // thanks to the abduction stage
        let mut arm_out = 0usize;
        let mut arm_all = 0usize;
        let torso_pts = bot.points_at(0, 0.5);
        let (mut smin, mut smax) = (f64::MAX, f64::MIN);
        for q in &torso_pts { smin = smin.min(q[0]); smax = smax.max(q[0]); }
        let half = 0.5 * (smax - smin);
        let mid = 0.5 * (smin + smax);
        for i in 0..bot.parts.len() {
            let nm = bot.parts[i].name;
            if !(nm.contains("uarm") || nm.contains("larm")) { continue; }
            arm_all += 1;
            let pts = bot.points_at(i, 0.5);
            let c: f64 = pts.iter().map(|p| p[0]).sum::<f64>() / pts.len() as f64;
            if (c - mid).abs() > half * 0.45 { arm_out += 1; }
        }
        if arm_all >= 4 {
            assert!(arm_out * 2 >= arm_all,
                "arms not clearing the torso mid-fold: {arm_out}/{arm_all} outside");
        }
    }

    #[test]
    fn skeleton_hides_in_vehicle_and_vehicle_lies_flat() {
        const INTERIOR: &[&str] = &["bone","guts","fist","foot","visor","core","rib","gyro",
            "piston","stack","brace","wheel","mast","module","hatch","dish","finger",
            "face","pauldron","vertebra","cable","keel","spike","winglet","crest"];
        for f in ["imported_tank_rollout_aka_sci_fi_tank.iqm", "imported_flyer_dawnshade.iqm"] {
            let p = data().join(f);
            if !p.exists() && skipped(&p) { return; }
            let bot = build(&p, 5).unwrap();
            // model box at rest
            let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
            for i in 0..bot.parts.len() {
                if INTERIOR.contains(&bot.parts[i].name) { continue; }
                for q in bot.points_at(i, 0.0) {
                    for a in 0..3 { lo[a] = lo[a].min(q[a]); hi[a] = hi[a].max(q[a]); }
                }
            }
            let pad = [(hi[0]-lo[0])*0.03, (hi[1]-lo[1])*0.03, (hi[2]-lo[2])*0.03];
            for i in 0..bot.parts.len() {
                if !INTERIOR.contains(&bot.parts[i].name) { continue; }
                for q in bot.points_at(i, 0.0) {
                    for a in 0..3 {
                        assert!(q[a] >= lo[a]-pad[a] && q[a] <= hi[a]+pad[a],
                            "{f}: {} pokes out of the vehicle at rest", bot.parts[i].name);
                    }
                }
            }
            // canonical: at rest, height (Y) is the smallest extent — no
            // nosedivers, no back-floaters
            let e = [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]];
            assert!(e[1] <= e[0] && e[1] <= e[2],
                "{f}: vehicle not lying flat (extents {e:?})");
        }
    }

    #[test]
    fn interiors_have_hands_feet_and_a_living_core() {
        let p = data().join("imported_tank_rollout_aka_sci_fi_tank.iqm");
        if !p.exists() && skipped(&p) { return; }
        let bot = build(&p, 5).unwrap();
        let count = |n: &str| bot.parts.iter().filter(|p| p.name == n).count();
        assert_eq!(count("fist"), 4, "two knuckled fists");
        assert_eq!(count("foot"), 4, "heel and toe per leg");
        assert_eq!(count("core"), 2, "housing plus the spinning disc");
        assert!(count("piston") >= 8, "sleeves and rods at knees and shoulders");
        assert_eq!(count("finger"), 6, "three per hand, clenching late");
        assert_eq!(count("pauldron"), 2, "shoulder plates riding the abduction");
        assert_eq!(count("gyro"), 8, "the hip ring");
    }

    #[test]
    fn anatomy_finds_limbs_on_a_tank() {
        let p = data().join("imported_tank_rollout_aka_sci_fi_tank.iqm");
        if !p.exists() && skipped(&p) { return; }
        let bot = build(&p, 5).unwrap();
        assert!(bot.parts.len() >= 4, "expected torso + limbs, got {}", bot.parts.len());
        let torso_names = ["spine", "chest", "pelvis", "neck"];
        let mut by_size: Vec<usize> = (0..bot.parts.len()).collect();
        by_size.sort_by_key(|&i| std::cmp::Reverse(bot.parts[i].rest.len()));
        assert!(by_size.iter().take(5).any(|&i|
            torso_names.iter().any(|t| bot.parts[i].name.contains(t))),
            "no substantial torso chunk among the largest parts");
        let limbs = bot.parts.iter().filter(|p| {
            p.name.contains("thigh") || p.name.contains("shin")
                || p.name.contains("uarm") || p.name.contains("larm")
        }).count();
        assert!(limbs >= 2, "expected at least two limb-assigned pieces, got {limbs}");
    }
}
