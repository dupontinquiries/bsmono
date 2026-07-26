//! IQM writer — the output half of the pipeline, ported from texture_export.py.
//!
//! Layout must match what the engine's loader expects exactly: header, text blob,
//! one mesh, three vertex arrays (position/uv/normal), then the triangle block.
//! Offsets are computed rather than hardcoded so a change in one section cannot
//! silently desynchronise the rest.

use std::fs;
use std::io::Write;
use std::path::Path;

fn u32s(v: &[u32], out: &mut Vec<u8>) {
    for x in v { out.extend_from_slice(&x.to_le_bytes()); }
}

/// Per-vertex normals from area-weighted face normals.
pub fn vertex_normals(v: &[[f32; 3]], f: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; v.len()];
    for t in f {
        let (a, b, c) = (v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]);
        let e1 = [b[0]-a[0], b[1]-a[1], b[2]-a[2]];
        let e2 = [c[0]-a[0], c[1]-a[1], c[2]-a[2]];
        let cr = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]];
        for &i in t.iter() {
            for k in 0..3 { n[i as usize][k] += cr[k]; }
        }
    }
    for q in n.iter_mut() {
        let l = (q[0]*q[0] + q[1]*q[1] + q[2]*q[2]).sqrt();
        if l > 1e-12 { for k in 0..3 { q[k] /= l; } } else { *q = [0.0, 1.0, 0.0]; }
    }
    n
}

pub fn write_iqm(path: &Path, v: &[[f32; 3]], uv: &[[f32; 2]], f: &[[u32; 3]],
                 texture: &str) -> std::io::Result<()> {
    let nrm = vertex_normals(v, f);
    // text blob: "" then mesh name then material
    let mut text = Vec::new();
    text.push(0u8);
    let name_ofs = text.len() as u32;
    text.extend_from_slice(b"mesh\0");
    let mat_ofs = text.len() as u32;
    text.extend_from_slice(texture.as_bytes());
    text.push(0);
    while text.len() % 4 != 0 { text.push(0); }

    const HDR: usize = 124;
    let ofs_text = HDR as u32;
    let ofs_meshes = ofs_text + text.len() as u32;
    let ofs_va = ofs_meshes + 24;                 // one iqmmesh = 6 u32
    let ofs_pos = ofs_va + 3 * 20;                // three vertexarrays
    let ofs_uv = ofs_pos + (v.len() * 12) as u32;
    let ofs_nrm = ofs_uv + (uv.len() * 8) as u32;
    let ofs_tris = ofs_nrm + (nrm.len() * 12) as u32;
    let filesize = ofs_tris + (f.len() * 12) as u32;

    let mut o: Vec<u8> = Vec::with_capacity(filesize as usize);
    o.extend_from_slice(b"INTERQUAKEMODEL\0");
    u32s(&[2, filesize, 0,
           text.len() as u32, ofs_text,
           1, ofs_meshes,
           3, v.len() as u32, ofs_va,
           f.len() as u32, ofs_tris, 0,
           0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], &mut o);
    o.truncate(HDR);
    while o.len() < HDR { o.push(0); }
    o.extend_from_slice(&text);
    // iqmmesh: name, material, first_vertex, num_vertexes, first_triangle, num_triangles
    u32s(&[name_ofs, mat_ofs, 0, v.len() as u32, 0, f.len() as u32], &mut o);
    // vertexarrays: type, flags, format, size, offset   (0=pos,1=uv,2=normal; 7=float)
    u32s(&[0, 0, 7, 3, ofs_pos,
           1, 0, 7, 2, ofs_uv,
           2, 0, 7, 3, ofs_nrm], &mut o);
    for p in v { for k in 0..3 { o.extend_from_slice(&p[k].to_le_bytes()); } }
    for p in uv { for k in 0..2 { o.extend_from_slice(&p[k].to_le_bytes()); } }
    for p in &nrm { for k in 0..3 { o.extend_from_slice(&p[k].to_le_bytes()); } }
    for t in f { for k in 0..3 { o.extend_from_slice(&t[k].to_le_bytes()); } }

    let mut fh = fs::File::create(path)?;
    fh.write_all(&o)
}

/// Box-projection UVs: per-face dominant-axis projection with the vertices
/// duplicated per face, so seams are clean. Emitted parts have no authored
/// unwrap, and without this any texture collapses to a single texel.
pub fn box_uvs(v: &[[f32; 3]], f: &[[u32; 3]])
    -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[u32; 3]>) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let span = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
    let mut ov = Vec::with_capacity(f.len() * 3);
    let mut ouv = Vec::with_capacity(f.len() * 3);
    let mut of = Vec::with_capacity(f.len());
    for t in f {
        let p = [v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]];
        let e1 = [p[1][0]-p[0][0], p[1][1]-p[0][1], p[1][2]-p[0][2]];
        let e2 = [p[2][0]-p[0][0], p[2][1]-p[0][1], p[2][2]-p[0][2]];
        let n = [(e1[1]*e2[2]-e1[2]*e2[1]).abs(),
                 (e1[2]*e2[0]-e1[0]*e2[2]).abs(),
                 (e1[0]*e2[1]-e1[1]*e2[0]).abs()];
        let ax = if n[0] >= n[1] && n[0] >= n[2] { 0 } else if n[1] >= n[2] { 1 } else { 2 };
        let (iu, iv) = match ax { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
        let base = ov.len() as u32;
        for q in p.iter() {
            ov.push(*q);
            ouv.push([(q[iu]-lo[iu]) / span[iu], (q[iv]-lo[iv]) / span[iv]]);
        }
        of.push([base, base + 1, base + 2]);
    }
    (ov, ouv, of)
}
