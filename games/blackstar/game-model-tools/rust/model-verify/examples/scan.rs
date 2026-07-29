//! Scan real IQM files and report instancing potential.
use model_verify::analyse;
use std::{env, fs};

fn load(p: &str) -> Option<(Vec<[f64; 3]>, Vec<u32>)> {
    let b = fs::read(p).ok()?;
    if b.len() < 64 || &b[..15] != b"INTERQUAKEMODEL" { return None; }
    let g = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
    let (nva, nv, ofs_va) = (g(44), g(48), g(52));
    let (nt, ofs_tri) = (g(56), g(60));
    let mut pos = None;
    for i in 0..nva {
        let e = ofs_va + i * 20;
        let (ty, fmt, size, off) = (g(e), g(e + 8), g(e + 12), g(e + 16));
        if ty == 0 && fmt == 7 {
            let mut v = Vec::with_capacity(nv);
            for k in 0..nv {
                let o = off + k * size * 4;
                let f = |j: usize| f32::from_le_bytes(b[o + j * 4..o + j * 4 + 4].try_into().unwrap()) as f64;
                v.push([f(0), f(1), f(2)]);
            }
            pos = Some(v);
        }
    }
    let mut tris = Vec::with_capacity(nt * 3);
    for i in 0..nt * 3 {
        let o = ofs_tri + i * 4;
        if o + 4 > b.len() { break; }
        tris.push(u32::from_le_bytes(b[o..o + 4].try_into().unwrap()));
    }
    Some((pos?, tris))
}

fn main() {
    let (mut tv, mut trd) = (0usize, 0usize);
    for p in env::args().skip(1) {
        let Some((v, t)) = load(&p) else { continue };
        // 1e-5 of the model's own extent: an absolute epsilon welds nothing on a
        // small model and everything on a large one.
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for q in &v { for i in 0..3 { lo[i] = lo[i].min(q[i]); hi[i] = hi[i].max(q[i]); } }
        let span = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f64, f64::max).max(1e-6);
        let r = analyse(&v, &t, span * 1e-5, span * 1e-4);
        tv += r.total_verts;
        trd += r.redundant_verts;
        let name = p.rsplit('/').next().unwrap_or(&p);
        println!("{:52} {:6} comps {:5} unique  {:5.1}% redundant",
                 &name[..name.len().min(52)], r.components, r.unique_shapes, r.saving() * 100.0);
    }
    if tv > 0 {
        println!("\nTOTAL {tv} verts, {trd} redundant ({:.1}%)", 100.0 * trd as f64 / tv as f64);
    }
}
