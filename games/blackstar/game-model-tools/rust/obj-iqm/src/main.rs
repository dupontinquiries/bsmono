//! obj-iqm -- convert a Wavefront .obj to an IQM the engine loads.
//! Rust port of the OBJ->IQM core of process_models.py (load + normalize +
//! vertex-normals + IQM write). Pure std.
//!
//! NOTE: quadric decimation (the tri-budget path in the Python tool) is NOT yet
//! ported -- it needs a mesh-simplification implementation. This crate does the
//! load/normalize/write core so OBJ models convert; decimate upstream (Python) or
//! add a simplifier crate. Marked clearly so it isn't mistaken for feature parity.

use std::env;
use std::fs;
use std::io::Write;

struct Mesh {
    verts: Vec<[f32; 3]>,
    faces: Vec<[u32; 3]>,
}

fn load_obj(path: &str) -> Option<Mesh> {
    let text = fs::read_to_string(path).ok()?;
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("v ") {
            let mut it = rest.split_whitespace();
            let x: f32 = it.next()?.parse().ok()?;
            let y: f32 = it.next()?.parse().ok()?;
            let z: f32 = it.next()?.parse().ok()?;
            verts.push([x, y, z]);
        } else if let Some(rest) = line.strip_prefix("f ") {
            let mut idx: Vec<i64> = Vec::new();
            for tok in rest.split_whitespace() {
                let vi = tok.split('/').next().unwrap_or("");
                if vi.is_empty() {
                    continue;
                }
                if let Ok(n) = vi.parse::<i64>() {
                    idx.push(if n > 0 { n - 1 } else { verts.len() as i64 + n });
                }
            }
            // fan-triangulate
            for k in 1..idx.len().saturating_sub(1) {
                faces.push([idx[0] as u32, idx[k] as u32, idx[k + 1] as u32]);
            }
        }
    }
    if verts.is_empty() || faces.is_empty() {
        return None;
    }
    // drop out-of-range faces defensively
    let nv = verts.len() as u32;
    faces.retain(|f| f[0] < nv && f[1] < nv && f[2] < nv);
    if faces.is_empty() {
        return None;
    }
    Some(Mesh { verts, faces })
}

/// Normalize into a [-1,1] box centered on the mesh bounds (matches the Python
/// _normalize_slot: center, scale by longest extent).
fn normalize(m: &mut Mesh) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for v in &m.verts {
        for i in 0..3 {
            lo[i] = lo[i].min(v[i]);
            hi[i] = hi[i].max(v[i]);
        }
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let mut s = (hi[0] - lo[0]).max((hi[1] - lo[1]).max(hi[2] - lo[2]));
    if s == 0.0 {
        s = 1.0;
    }
    for v in &mut m.verts {
        for i in 0..3 {
            v[i] = (v[i] - c[i]) / s * 2.0;
        }
    }
}

fn vertex_normals(m: &Mesh) -> Vec<[f32; 3]> {
    let mut n = vec![[0.0f32; 3]; m.verts.len()];
    for f in &m.faces {
        let a = m.verts[f[0] as usize];
        let b = m.verts[f[1] as usize];
        let c = m.verts[f[2] as usize];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let fn_ = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for &vi in f {
            for i in 0..3 {
                n[vi as usize][i] += fn_[i];
            }
        }
    }
    for nn in &mut n {
        let len = (nn[0] * nn[0] + nn[1] * nn[1] + nn[2] * nn[2]).sqrt();
        let len = if len == 0.0 { 1.0 } else { len };
        for i in 0..3 {
            nn[i] /= len;
        }
    }
    n
}

fn u32s(v: &[u32]) -> Vec<u8> {
    let mut o = Vec::with_capacity(v.len() * 4);
    for x in v {
        o.extend_from_slice(&x.to_le_bytes());
    }
    o
}

/// Write a minimal IQM (1 mesh, position+uv+normal), material name = `tex`.
/// Mirrors the Python _write_iqm byte layout.
fn write_iqm(path: &str, m: &Mesh, normals: &[[f32; 3]], tex: &str) -> std::io::Result<()> {
    let nv = m.verts.len() as u32;
    let nt = m.faces.len() as u32;

    // text blob: \0 <tex>\0 prop\0
    let mut text: Vec<u8> = vec![0];
    let mat_off = text.len() as u32;
    text.extend_from_slice(tex.as_bytes());
    text.push(0);
    let name_off = text.len() as u32;
    text.extend_from_slice(b"prop");
    text.push(0);

    let hdr_size = 16 + 4 + 4 * 26;
    let mut ofs = hdr_size as u32;
    let ofs_text = ofs;
    ofs += text.len() as u32;
    ofs += (4 - (ofs % 4)) % 4;
    let ofs_meshes = ofs;
    ofs += 6 * 4;
    let ofs_va = ofs;
    ofs += 15 * 4;
    let ofs_pos = ofs;
    ofs += nv * 12;
    let ofs_uv = ofs;
    ofs += nv * 8;
    let ofs_nrm = ofs;
    ofs += nv * 12;
    let ofs_tri = ofs;
    // ofs += nt*12; // end

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"INTERQUAKEMODEL\0");
    // version, filesize(=0 placeholder), flags
    out.extend_from_slice(&u32s(&[2, ofs + nt * 12, 0]));
    out.extend_from_slice(&u32s(&[text.len() as u32, ofs_text]));
    out.extend_from_slice(&u32s(&[1, ofs_meshes]));
    out.extend_from_slice(&u32s(&[3, nv, ofs_va]));
    out.extend_from_slice(&u32s(&[nt, ofs_tri, 0]));
    // remaining header counts/offsets (joints, poses, anims, frames, bounds,
    // comment, extensions) -- all zero for a static mesh.
    out.extend_from_slice(&u32s(&[0, 0])); // joints
    out.extend_from_slice(&u32s(&[0, 0])); // poses
    out.extend_from_slice(&u32s(&[0, 0])); // anims
    out.extend_from_slice(&u32s(&[0, 0, 0, 0])); // frames
    out.extend_from_slice(&u32s(&[0, 0])); // comment
    out.extend_from_slice(&u32s(&[0, 0])); // extensions

    // text
    out.extend_from_slice(&text);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    // mesh: name, material, first_vertex, num_vertexes, first_triangle, num_triangles
    out.extend_from_slice(&u32s(&[name_off, mat_off, 0, nv, 0, nt]));
    // vertexarrays: (type, flags, format, size, offset) x3
    out.extend_from_slice(&u32s(&[0, 0, 7, 3, ofs_pos])); // position, float, 3
    out.extend_from_slice(&u32s(&[1, 0, 7, 2, ofs_uv])); // texcoord, float, 2
    out.extend_from_slice(&u32s(&[2, 0, 7, 3, ofs_nrm])); // normal, float, 3

    // vertex data
    for v in &m.verts {
        for c in v {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    for _ in 0..nv {
        out.extend_from_slice(&0f32.to_le_bytes());
        out.extend_from_slice(&0f32.to_le_bytes());
    }
    for n in normals {
        for c in n {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    // triangles
    for f in &m.faces {
        for i in f {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }

    let mut fh = fs::File::create(path)?;
    fh.write_all(&out)?;
    Ok(())
}

/// Vertex-clustering decimation: snap vertices to a grid, collapse each occupied
/// cell to one vertex, remap + drop degenerate faces. Coarsen until under budget.
/// (Port of process_models.py's _cluster_decimate fallback -- robust, dep-free.
/// Not quadric: preserves silhouette well, loses fine surface detail faster.)
fn decimate_cluster(m: &mut Mesh, budget: usize) {
    if m.faces.len() <= budget {
        return;
    }
    // work in the mesh's own bounds
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for v in &m.verts {
        for i in 0..3 {
            lo[i] = lo[i].min(v[i]);
            hi[i] = hi[i].max(v[i]);
        }
    }
    let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let maxext = ext[0].max(ext[1]).max(ext[2]).max(1e-6);

    // start grid ~ cube-root of a vertex target, coarsen (halve res) until under budget
    let mut grid: i64 = 128;
    for _ in 0..12 {
        let cell = maxext / grid as f32;
        if cell <= 0.0 {
            break;
        }
        // map each vertex to a cell key -> representative (centroid accumulation)
        use std::collections::HashMap;
        let mut cell_of: Vec<u32> = Vec::with_capacity(m.verts.len());
        let mut keys: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut sum: Vec<[f64; 3]> = Vec::new();
        let mut cnt: Vec<u32> = Vec::new();
        for v in &m.verts {
            let k = (
                ((v[0] - lo[0]) / cell) as i64,
                ((v[1] - lo[1]) / cell) as i64,
                ((v[2] - lo[2]) / cell) as i64,
            );
            let id = *keys.entry(k).or_insert_with(|| {
                sum.push([0.0; 3]);
                cnt.push(0);
                (sum.len() - 1) as u32
            });
            let idx = id as usize;
            for c in 0..3 {
                sum[idx][c] += v[c] as f64;
            }
            cnt[idx] += 1;
            cell_of.push(id);
        }
        // build new faces, dropping degenerates
        let mut nf: Vec<[u32; 3]> = Vec::with_capacity(m.faces.len());
        for f in &m.faces {
            let a = cell_of[f[0] as usize];
            let b = cell_of[f[1] as usize];
            let c = cell_of[f[2] as usize];
            if a != b && b != c && a != c {
                nf.push([a, b, c]);
            }
        }
        if nf.len() <= budget || grid <= 2 {
            // commit this level
            let mut nv: Vec<[f32; 3]> = Vec::with_capacity(sum.len());
            for i in 0..sum.len() {
                let c = cnt[i].max(1) as f64;
                nv.push([
                    (sum[i][0] / c) as f32,
                    (sum[i][1] / c) as f32,
                    (sum[i][2] / c) as f32,
                ]);
            }
            m.verts = nv;
            m.faces = nf;
            return;
        }
        grid /= 2; // coarsen and retry
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: obj-iqm INPUT.obj [OUTPUT.iqm] [--tex NAME.png]");
        std::process::exit(1);
    }
    let input = &args[1];
    let mut output = args
        .get(2)
        .filter(|s| !s.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| input.replace(".obj", ".iqm"));
    if !output.ends_with(".iqm") {
        output = input.replace(".obj", ".iqm");
    }
    let mut tex = "plate_blue.png".to_string();
    let mut tri_budget: usize = 0; // 0 = no decimation
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--tex" {
            i += 1;
            if let Some(t) = args.get(i) {
                tex = t.clone();
            }
        } else if args[i] == "--tri-budget" {
            i += 1;
            if let Some(t) = args.get(i) {
                tri_budget = t.parse().unwrap_or(0);
            }
        }
        i += 1;
    }

    let mut mesh = match load_obj(input) {
        Some(m) => m,
        None => {
            eprintln!("FAIL: could not load {}", input);
            std::process::exit(1);
        }
    };
    let src_tris = mesh.faces.len();
    if tri_budget > 0 {
        decimate_cluster(&mut mesh, tri_budget);
    }
    normalize(&mut mesh);
    let normals = vertex_normals(&mesh);
    match write_iqm(&output, &mesh, &normals, &tex) {
        Ok(()) => {
            let dec = if tri_budget > 0 && mesh.faces.len() < src_tris {
                format!(" (decim {}->{}t)", src_tris, mesh.faces.len())
            } else {
                String::new()
            };
            println!(
                "{} -> {}  ({} verts, {} tris){}",
                input,
                output,
                mesh.verts.len(),
                mesh.faces.len(),
                dec
            )
        }
        Err(e) => {
            eprintln!("FAIL writing {}: {}", output, e);
            std::process::exit(1);
        }
    }
}
