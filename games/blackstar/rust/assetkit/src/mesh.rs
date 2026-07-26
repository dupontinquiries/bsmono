//! Mesh loading + scoring. Pure std, no deps -- this is the Rust port of the
//! measurement half of the Python pipeline (part_harvest / library_score).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Default, Clone)]
pub struct Mesh {
    pub v: Vec<[f32; 3]>,
    pub f: Vec<[u32; 3]>,
}

#[derive(Clone, Debug)]
pub struct Score {
    pub tris: usize,
    pub verts: usize,
    pub compactness: f32,
    pub coherence: f32,
    pub bbox_fill: f32,
    pub members: usize,
    pub open_frac: f32,
    pub aspect: [f32; 3],
    pub class: &'static str,
    pub bony: f32,
    pub scorable: bool,
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// IQM: verified header offsets 44 num_va, 48 num_verts, 52 ofs_va,
/// 56 num_tris, 60 ofs_tris. (These were transposed in the first draft of
/// library_score.rs and it panicked on out-of-range triangle indices.)
pub fn load_iqm(p: &Path) -> Option<Mesh> {
    let b = fs::read(p).ok()?;
    if b.len() < 124 || &b[0..16] != b"INTERQUAKEMODEL\0" { return None; }
    let num_va = rd_u32(&b, 44) as usize;
    let num_verts = rd_u32(&b, 48) as usize;
    let ofs_va = rd_u32(&b, 52) as usize;
    let num_tris = rd_u32(&b, 56) as usize;
    let ofs_tris = rd_u32(&b, 60) as usize;
    let mut m = Mesh::default();
    for i in 0..num_va.min(8) {
        let base = ofs_va + i * 20;
        if base + 20 > b.len() { break; }
        if rd_u32(&b, base) == 0 && rd_u32(&b, base + 12) == 3 {
            let off = rd_u32(&b, base + 16) as usize;
            for k in 0..num_verts {
                let o = off + k * 12;
                if o + 12 > b.len() { break; }
                m.v.push([
                    f32::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3]]),
                    f32::from_le_bytes([b[o+4], b[o+5], b[o+6], b[o+7]]),
                    f32::from_le_bytes([b[o+8], b[o+9], b[o+10], b[o+11]]),
                ]);
            }
        }
    }
    let nv = m.v.len() as u32;
    for k in 0..num_tris {
        let o = ofs_tris + k * 12;
        if o + 12 > b.len() { break; }
        let t = [rd_u32(&b, o), rd_u32(&b, o + 4), rd_u32(&b, o + 8)];
        if t[0] < nv && t[1] < nv && t[2] < nv { m.f.push(t); }
    }
    if m.v.is_empty() || m.f.is_empty() { None } else { Some(m) }
}

pub fn load_obj(p: &Path) -> Option<Mesh> {
    let txt = fs::read_to_string(p).ok()?;
    let mut m = Mesh::default();
    for line in txt.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let a: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if a.len() >= 3 { m.v.push([a[0], a[1], a[2]]); }
            }
            Some("f") => {
                let idx: Vec<i64> = it
                    .filter_map(|s| s.split('/').next().and_then(|t| t.parse().ok()))
                    .collect();
                let nv = m.v.len() as i64;
                let res = |i: i64| -> u32 { if i > 0 { (i - 1) as u32 } else { (nv + i) as u32 } };
                for k in 1..idx.len().saturating_sub(1) {
                    let t = [res(idx[0]), res(idx[k]), res(idx[k + 1])];
                    if (t[0] as i64) < nv && (t[1] as i64) < nv && (t[2] as i64) < nv {
                        m.f.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    if m.f.is_empty() { None } else { Some(m) }
}

pub fn load_any(p: &Path) -> Option<Mesh> {
    match p.extension().and_then(|s| s.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("iqm") => load_iqm(p),
        Some(e) if e.eq_ignore_ascii_case("obj") => load_obj(p),
        Some(e) if e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb") =>
            crate::gltf::load(p),
        _ => None,
    }
}

pub fn normalise(m: &mut Mesh) -> [f32; 3] {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &m.v {
        for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); }
    }
    let mut span = [1e-6f32; 3];
    let mut big = 1e-6f32;
    for k in 0..3 { span[k] = (hi[k] - lo[k]).max(1e-6); big = big.max(span[k]); }
    for p in m.v.iter_mut() {
        for k in 0..3 { p[k] = (p[k] - (lo[k] + hi[k]) * 0.5) / big * 2.0; }
    }
    [span[0] / big, span[1] / big, span[2] / big]
}

fn weld(m: &Mesh) -> Vec<u32> {
    let mut map: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let mut w = vec![0u32; m.v.len()];
    for (i, p) in m.v.iter().enumerate() {
        let key = ((p[0] * 1e5).round() as i32, (p[1] * 1e5).round() as i32,
                   (p[2] * 1e5).round() as i32);
        let n = map.len() as u32;
        w[i] = *map.entry(key).or_insert(n);
    }
    w
}

fn face_normals(m: &Mesh) -> Vec<[f32; 3]> {
    m.f.iter().map(|t| {
        let (a, b, c) = (m.v[t[0] as usize], m.v[t[1] as usize], m.v[t[2] as usize]);
        let e1 = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
        let e2 = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
        let mut n = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]];
        let l = (n[0]*n[0]+n[1]*n[1]+n[2]*n[2]).sqrt().max(1e-12);
        for k in 0..3 { n[k] /= l; }
        n
    }).collect()
}

pub fn score(m: &mut Mesh, min_tris: usize) -> Score {
    let aspect = normalise(m);
    let mut area = 0.0f64;
    let mut vol = 0.0f64;
    for t in &m.f {
        let (a, b, c) = (m.v[t[0] as usize], m.v[t[1] as usize], m.v[t[2] as usize]);
        let e1 = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
        let e2 = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
        let cr = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]];
        area += 0.5 * ((cr[0]*cr[0] + cr[1]*cr[1] + cr[2]*cr[2]) as f64).sqrt();
        vol += (a[0] as f64) * ((b[1] as f64)*(c[2] as f64) - (b[2] as f64)*(c[1] as f64))
             + (a[1] as f64) * ((b[2] as f64)*(c[0] as f64) - (b[0] as f64)*(c[2] as f64))
             + (a[2] as f64) * ((b[0] as f64)*(c[1] as f64) - (b[1] as f64)*(c[0] as f64));
    }
    let area = area as f32;
    let vol = ((vol / 6.0).abs()) as f32;
    let w = weld(m);
    let nrm = face_normals(m);
    let mut edge: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut parent: Vec<u32> = (0..w.iter().copied().max().unwrap_or(0) + 1).collect();
    fn find(p: &mut Vec<u32>, mut x: u32) -> u32 {
        while p[x as usize] != x { p[x as usize] = p[p[x as usize] as usize]; x = p[x as usize]; }
        x
    }
    for (i, t) in m.f.iter().enumerate() {
        let (a, b, c) = (w[t[0] as usize], w[t[1] as usize], w[t[2] as usize]);
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let k = if x < y { (x, y) } else { (y, x) };
            let e = edge.entry(k).or_insert((u32::MAX, u32::MAX));
            if e.0 == u32::MAX { e.0 = i as u32; } else if e.1 == u32::MAX { e.1 = i as u32; }
            let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
            if rx != ry { parent[ry as usize] = rx; }
        }
    }
    let mut count: HashMap<u32, usize> = HashMap::new();
    for t in &m.f { *count.entry(find(&mut parent, w[t[0] as usize])).or_insert(0) += 1; }
    let members = count.values().filter(|&&c| c >= min_tris).count();
    let mut boundary = 0usize;
    let mut coh_sum = 0.0f64;
    let mut coh_n = 0usize;
    for (_k, (a, b)) in &edge {
        if *b == u32::MAX { boundary += 1; continue; }
        let (x, y) = (nrm[*a as usize], nrm[*b as usize]);
        coh_sum += (x[0]*y[0] + x[1]*y[1] + x[2]*y[2]).abs() as f64;
        coh_n += 1;
    }
    let open_frac = if edge.is_empty() { 0.0 } else { boundary as f32 / edge.len() as f32 };
    let coherence = if coh_n == 0 { 0.0 } else { (coh_sum / coh_n as f64) as f32 };
    let compactness = if vol > 1e-9 { area / vol.powf(2.0 / 3.0) } else { 0.0 };
    let bbox = (aspect[0] * aspect[1] * aspect[2] * 8.0).max(1e-9);
    // Compactness alone rewards NOISE -- a jittered slab of boxes measured 881 vs
    // Scorponok's 96. coherence^3 is the corrective. Still only a filter, not proof.
    let bony = compactness * coherence.powi(3) * (1.0 + (members as f32).max(1.0).ln());
    let scorable = m.f.len() >= 500 && open_frac < 0.25 && vol > 1e-6 && vol / bbox > 1e-4;
    let (x, y, z) = (aspect[0], aspect[1], aspect[2]);
    let flat = y / x.max(z).max(1e-6);
    let long = x.max(z) / x.min(z).max(1e-6);
    let class = if flat < 0.35 && long > 1.6 { "flyer" }
        else if flat < 0.45 { "tank/car" }
        else if y >= x.max(z) * 0.9 && compactness > 45.0 { "bot-bony" }
        else if y >= x.max(z) * 0.9 { "bot-shell" }
        else { "other" };
    Score { tris: m.f.len(), verts: m.v.len(), compactness, coherence,
            bbox_fill: vol / bbox, members, open_frac, aspect, class, bony, scorable }
}
