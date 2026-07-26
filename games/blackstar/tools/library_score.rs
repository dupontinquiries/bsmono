//! library_score -- walk a model library and score every model for how BONY it is.
//!
//! Companion to the shape-upscaling GAN: the training set lives or dies on donor
//! selection, and "pick the detailed ones" was being done by eye. This measures it.
//!
//! Scoring was chosen by testing candidates against models whose character is
//! already known. Scorponok is the reference for dense/skeletal; Grimlock and Jazz
//! are solid shells. Measured, normalised to a 2-unit model:
//!
//!     model       compactness   bbox_fill   members
//!     scorponok         95.7       0.016       384    <- bony
//!     grimlock          30.4       0.229        52
//!     jazz              29.2       0.150        42
//!     ravage            20.5       0.077        38
//!     shockwave         17.5       0.039        25
//!
//! COMPACTNESS = area / volume^(2/3) is the discriminator: it separates Scorponok
//! by 3x from everything else. A skeletal model has enormous surface area for its
//! enclosed volume; a hollow shell does not. BBOX_FILL alone is misleading --
//! Shockwave scores low simply from a sprawling pose, not from being bony.
//!
//! Reads IQM (the pipeline already emits one per model) and Wavefront OBJ. glTF is
//! deliberately not parsed here: process_models.py already converts the library to
//! IQM, so pointing this at that output covers everything without a JSON parser.
//!
//! Build: drop into the kBM3 workspace as a bin target, or
//!   rustc -O tools/library_score.rs -o library_score
//! Usage:
//!   ./library_score iqm/ --csv scores.csv --top 40

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
struct Mesh {
    v: Vec<[f32; 3]>,
    f: Vec<[u32; 3]>,
}

#[derive(Clone)]
struct Score {
    name: String,
    tris: usize,
    verts: usize,
    compactness: f32,
    bbox_fill: f32,
    members: usize,
    open_frac: f32,
    aspect: [f32; 3],
    category: &'static str,
    coherence: f32,
    bony: f32,
}

// ---------------------------------------------------------------- loaders

fn load_obj(p: &Path) -> Option<Mesh> {
    let txt = fs::read_to_string(p).ok()?;
    let mut m = Mesh::default();
    for line in txt.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let a: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if a.len() >= 3 {
                    m.v.push([a[0], a[1], a[2]]);
                }
            }
            Some("f") => {
                let idx: Vec<i64> = it
                    .filter_map(|s| s.split('/').next().and_then(|t| t.parse().ok()))
                    .collect();
                // OBJ indices are 1-based and may be negative (relative to the end)
                let nv = m.v.len() as i64;
                let resolve = |i: i64| -> u32 {
                    if i > 0 { (i - 1) as u32 } else { (nv + i) as u32 }
                };
                for k in 1..idx.len().saturating_sub(1) {
                    let tri = [resolve(idx[0]), resolve(idx[k]), resolve(idx[k + 1])];
                    if (tri[0] as usize) < m.v.len() && (tri[1] as usize) < m.v.len()
                        && (tri[2] as usize) < m.v.len() { m.f.push(tri); }
                }
            }
            _ => {}
        }
    }
    if m.f.is_empty() { None } else { Some(m) }
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Minimal IQM reader: positions (vertexarray type 0) + triangles.
fn load_iqm(p: &Path) -> Option<Mesh> {
    let b = fs::read(p).ok()?;
    if b.len() < 124 || &b[0..16] != b"INTERQUAKEMODEL\0" {
        return None;
    }
    // Verified header offsets (I had these transposed and it panicked with
    // triangle indices past the end of the vertex array):
    //   44 num_vertexarrays, 48 num_vertexes, 52 ofs_vertexarrays,
    //   56 num_triangles,    60 ofs_triangles
    let num_va = rd_u32(&b, 44) as usize;
    let num_verts = rd_u32(&b, 48) as usize;
    let ofs_va = rd_u32(&b, 52) as usize;
    let num_tris = rd_u32(&b, 56) as usize;
    let ofs_tris = rd_u32(&b, 60) as usize;
    let mut m = Mesh::default();
    // vertex arrays: type, flags, format, size, offset
    for i in 0..num_va.min(8) {
        let base = ofs_va + i * 20;
        if base + 20 > b.len() { break; }
        let kind = rd_u32(&b, base);
        let size = rd_u32(&b, base + 12) as usize;
        let off = rd_u32(&b, base + 16) as usize;
        if kind == 0 && size == 3 {
            m.v.reserve(num_verts);
            for k in 0..num_verts {
                let o = off + k * 12;
                if o + 12 > b.len() { break; }
                m.v.push([
                    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]),
                    f32::from_le_bytes([b[o + 4], b[o + 5], b[o + 6], b[o + 7]]),
                    f32::from_le_bytes([b[o + 8], b[o + 9], b[o + 10], b[o + 11]]),
                ]);
            }
        }
    }
    m.f.reserve(num_tris);
    let nv = m.v.len() as u32;
    for k in 0..num_tris {
        let o = ofs_tris + k * 12;
        if o + 12 > b.len() { break; }
        let t = [rd_u32(&b, o), rd_u32(&b, o + 4), rd_u32(&b, o + 8)];
        // a malformed or misread file must not panic the whole run
        if t[0] < nv && t[1] < nv && t[2] < nv { m.f.push(t); }
    }
    if m.v.is_empty() || m.f.is_empty() { None } else { Some(m) }
}

// ---------------------------------------------------------------- metrics

fn normalise(m: &mut Mesh) -> [f32; 3] {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &m.v {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let mut span = [0.0f32; 3];
    let mut big = 1e-6f32;
    for k in 0..3 {
        span[k] = (hi[k] - lo[k]).max(1e-6);
        big = big.max(span[k]);
    }
    for p in m.v.iter_mut() {
        for k in 0..3 {
            p[k] = (p[k] - (lo[k] + hi[k]) * 0.5) / big * 2.0;
        }
    }
    let mut a = [0.0f32; 3];
    for k in 0..3 { a[k] = span[k] / big; }
    a
}

fn area_and_volume(m: &Mesh) -> (f32, f32) {
    let mut area = 0.0f64;
    let mut vol = 0.0f64;
    for t in &m.f {
        let (a, b, c) = (
            m.v[t[0] as usize],
            m.v[t[1] as usize],
            m.v[t[2] as usize],
        );
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        area += 0.5 * ((cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]) as f64).sqrt();
        // signed tetrahedron volume about the origin
        vol += (a[0] as f64) * ((b[1] as f64) * (c[2] as f64) - (b[2] as f64) * (c[1] as f64))
            + (a[1] as f64) * ((b[2] as f64) * (c[0] as f64) - (b[0] as f64) * (c[2] as f64))
            + (a[2] as f64) * ((b[0] as f64) * (c[1] as f64) - (b[1] as f64) * (c[0] as f64));
    }
    (area as f32, (vol / 6.0).abs() as f32)
}

/// Connected components over welded vertices, and the fraction of edges that are
/// boundaries (an open shell has many; a solid has none).
fn components_and_openness(m: &Mesh, min_tris: usize) -> (usize, f32) {
    // weld by quantised position so split vertices do not fragment everything
    let mut map: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut weld = vec![0u32; m.v.len()];
    for (i, p) in m.v.iter().enumerate() {
        let key = (
            (p[0] * 1e5).round() as i32,
            (p[1] * 1e5).round() as i32,
            (p[2] * 1e5).round() as i32,
        );
        let n = map.len() as u32;
        weld[i] = *map.entry(key).or_insert(n);
    }
    let n = map.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(p: &mut Vec<u32>, mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &m.f {
        let a = weld[t[0] as usize];
        let b = weld[t[1] as usize];
        let c = weld[t[2] as usize];
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let k = if x < y { (x, y) } else { (y, x) };
            *edges.entry(k).or_insert(0) += 1;
            let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
            if rx != ry { parent[ry as usize] = rx; }
        }
    }
    let mut count: HashMap<u32, usize> = HashMap::new();
    for t in &m.f {
        let r = find(&mut parent, weld[t[0] as usize]);
        *count.entry(r).or_insert(0) += 1;
    }
    let members = count.values().filter(|&&c| c >= min_tris).count();
    let boundary = edges.values().filter(|&&c| c == 1).count();
    let open = if edges.is_empty() { 0.0 } else { boundary as f32 / edges.len() as f32 };
    (members, open)
}

/// Mean |dot| between the normals of edge-sharing faces.
///
/// COMPACTNESS ALONE IS GAMEABLE, which the user predicted: it rewards surface
/// area, and noise has plenty. Measured -- a jittered slab of boxes scores 881
/// against Scorponok's 96, so "bony" would select noise over structure. Coherence
/// is the corrective: real plating is locally smooth and meets at clean edges,
/// while high-frequency junk has neighbours pointing everywhere. A clean grid of
/// boxes drops from 111 to 4.1 once coherence is folded in.
/// It is a partial fix, not a complete one -- coarsely jittered boxes keep their
/// faces planar and so keep decent coherence. Treat a very high score as a
/// candidate to LOOK AT, never as proof of quality.
fn coherence(m: &Mesh) -> f32 {
    let mut map: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut weld = vec![0u32; m.v.len()];
    for (i, p) in m.v.iter().enumerate() {
        let key = ((p[0] * 1e5).round() as i32, (p[1] * 1e5).round() as i32,
                   (p[2] * 1e5).round() as i32);
        let n = map.len() as u32;
        weld[i] = *map.entry(key).or_insert(n);
    }
    let mut nrm: Vec<[f32; 3]> = Vec::with_capacity(m.f.len());
    for t in &m.f {
        let (a, b, c) = (m.v[t[0] as usize], m.v[t[1] as usize], m.v[t[2] as usize]);
        let e1 = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
        let e2 = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
        let mut n = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]];
        let l = (n[0]*n[0]+n[1]*n[1]+n[2]*n[2]).sqrt().max(1e-12);
        for k in 0..3 { n[k] /= l; }
        nrm.push(n);
    }
    let mut edge: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    for (i, t) in m.f.iter().enumerate() {
        let (a, b, c) = (weld[t[0] as usize], weld[t[1] as usize], weld[t[2] as usize]);
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let k = if x < y { (x, y) } else { (y, x) };
            let e = edge.entry(k).or_insert((u32::MAX, u32::MAX));
            if e.0 == u32::MAX { e.0 = i as u32; } else if e.1 == u32::MAX { e.1 = i as u32; }
        }
    }
    let mut sum = 0.0f64; let mut n = 0usize;
    for (_k, (a, b)) in edge {
        if a == u32::MAX || b == u32::MAX { continue; }
        let (x, y) = (nrm[a as usize], nrm[b as usize]);
        sum += (x[0]*y[0] + x[1]*y[1] + x[2]*y[2]).abs() as f64;
        n += 1;
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

/// Rough form class from proportions. Deliberately coarse -- it is a bucket for
/// comparing like with like, not a claim about what the model IS.
fn classify(a: [f32; 3], compact: f32) -> &'static str {
    let (x, y, z) = (a[0], a[1], a[2]);
    let flat = y / x.max(z).max(1e-6);
    let long = x.max(z) / x.min(z).max(1e-6);
    if flat < 0.35 && long > 1.6 { "flyer" }
    else if flat < 0.45 { "tank/car" }
    else if y >= x.max(z) * 0.9 && compact > 45.0 { "bot-bony" }
    else if y >= x.max(z) * 0.9 { "bot-shell" }
    else { "other" }
}

// ---------------------------------------------------------------- driver

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Some(x) = p.extension().and_then(|s| s.to_str()) {
                let x = x.to_ascii_lowercase();
                if x == "iqm" || x == "obj" { out.push(p); }
            }
        }
    }
}

/// Compactness needs an enclosed volume, and signed volume is meaningless on an
/// open surface: a flat panel encloses nothing, so area/vol^(2/3) explodes. The
/// first run ranked 80-triangle generated panels at 24,000 -- above Scorponok --
/// purely from that degeneracy. Gate on closedness and size instead of clamping.
fn scorable(tris: usize, open_frac: f32, vol: f32, bbox: f32, min_size: usize) -> bool {
    tris >= min_size && open_frac < 0.25 && vol > 1e-6 && vol / bbox > 1e-4
}

fn score_one(p: &Path, min_tris: usize) -> Option<Score> {
    let mut m = match p.extension().and_then(|s| s.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("iqm") => load_iqm(p)?,
        _ => load_obj(p)?,
    };
    if m.f.len() < 32 { return None; }
    let aspect = normalise(&mut m);
    let (area, vol) = area_and_volume(&m);
    if vol <= 1e-9 { return None; }
    let compactness = area / vol.powf(2.0 / 3.0);
    let bbox = (aspect[0] * aspect[1] * aspect[2] * 8.0).max(1e-9);
    let (members, open_frac) = components_and_openness(&m, min_tris);
    if !scorable(m.f.len(), open_frac, vol, bbox, 500) { return None; }
    let category = classify(aspect, compactness);
    let coh = coherence(&m);
    // compactness x coherence^3 x log(members): area alone selects noise, so the
    // cubed coherence term is what makes the score prefer structure over junk.
    let bony = compactness * coh.powi(3) * (1.0 + (members as f32).max(1.0).ln());
    Some(Score {
        name: p.file_stem().unwrap_or_default().to_string_lossy().to_string(),
        tris: m.f.len(),
        verts: m.v.len(),
        compactness,
        bbox_fill: vol / bbox,
        members,
        open_frac,
        aspect,
        category,
        coherence: coh,
        bony,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: library_score <dir> [--csv out.csv] [--top N] [--min-tris N]");
        std::process::exit(1);
    }
    let root = PathBuf::from(&args[1]);
    let mut csv: Option<String> = None;
    let mut top = 30usize;
    let mut min_tris = 50usize;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--csv" => { csv = args.get(i + 1).cloned(); i += 2; }
            "--top" => { top = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(30); i += 2; }
            "--min-tris" => { min_tris = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(50); i += 2; }
            _ => { i += 1; }
        }
    }

    let mut files = Vec::new();
    walk(&root, &mut files);
    eprintln!("{} candidate file(s) under {}", files.len(), root.display());

    let n_threads = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4);
    let chunk = (files.len() + n_threads - 1) / n_threads.max(1);
    let mut handles = Vec::new();
    for c in files.chunks(chunk.max(1)) {
        let c: Vec<PathBuf> = c.to_vec();
        handles.push(std::thread::spawn(move || {
            c.iter().filter_map(|p| score_one(p, min_tris)).collect::<Vec<Score>>()
        }));
    }
    let mut scores: Vec<Score> = Vec::new();
    for h in handles {
        if let Ok(mut v) = h.join() { scores.append(&mut v); }
    }
    scores.sort_by(|a, b| b.bony.partial_cmp(&a.bony).unwrap_or(std::cmp::Ordering::Equal));

    println!("{:<32} {:>9} {:>8} {:>6} {:>8} {:>7} {:>6} {:>8}  {}",
             "model", "bony", "compact", "coher", "members", "bboxfil", "open%", "tris", "class");
    for s in scores.iter().take(top) {
        println!("{:<32} {:>9.1} {:>8.1} {:>6.3} {:>8} {:>8.3} {:>5.0}% {:>8}  {}",
                 s.name.chars().take(32).collect::<String>(),
                 s.bony, s.compactness, s.coherence, s.members, s.bbox_fill,
                 s.open_frac * 100.0, s.tris, s.category);
    }

    // per-class summary: donors should be compared within a class, since a flyer
    // is legitimately flatter and less compact than a walker
    let mut by: HashMap<&str, (usize, f32)> = HashMap::new();
    for s in &scores {
        let e = by.entry(s.category).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += s.compactness;
    }
    println!("\nby class:");
    let mut ks: Vec<_> = by.into_iter().collect();
    ks.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (k, (n, sum)) in ks {
        println!("  {:<12} {:>5} models, mean compactness {:>7.1}", k, n, sum / n as f32);
    }

    if let Some(path) = csv {
        let mut out = String::from("name,bony,compactness,coherence,members,bbox_fill,open_frac,tris,verts,ax,ay,az,class\n");
        for s in &scores {
            out.push_str(&format!("{},{:.3},{:.3},{:.4},{},{:.5},{:.4},{},{},{:.3},{:.3},{:.3},{}\n",
                s.name, s.bony, s.compactness, s.coherence, s.members, s.bbox_fill,
                s.open_frac, s.tris, s.verts, s.aspect[0], s.aspect[1], s.aspect[2], s.category));
        }
        if fs::write(&path, out).is_ok() {
            eprintln!("wrote {}", path);
        }
    }
}
