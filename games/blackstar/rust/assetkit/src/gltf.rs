//! glTF / GLB loader — port of process_models.py's _load_gltf_geometry.
//!
//! Handles the parts the library actually uses: external and embedded buffers,
//! GLB containers, byte-strided accessors, all five index component types, the
//! primitive modes (skipping points/lines, converting strips and fans), and the
//! node hierarchy so parts land where they belong.
//!
//! Node transforms matter: a glTF stores each mesh in its own local space and
//! places it with a node matrix or TRS. Ignoring that gives a pile of overlapping
//! parts at the origin rather than a model.

use crate::json::{b64, parse, J};
use crate::mesh::Mesh;
use std::fs;
use std::path::Path;

struct Ctx {
    j: J,
    bufs: Vec<Vec<u8>>,
}

fn read_buffers(j: &J, base: &Path, glb_chunk: Option<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for (i, b) in j.get("buffers").map(|x| x.arr()).unwrap_or(&[]).iter().enumerate() {
        match b.s("uri") {
            Some(u) if u.starts_with("data:") => {
                let payload = u.splitn(2, ",").nth(1).unwrap_or("");
                out.push(b64(payload));
            }
            Some(u) => {
                // percent-decode: library paths contain spaces and parentheses
                let mut dec = String::new();
                let bytes = u.as_bytes();
                let mut k = 0;
                while k < bytes.len() {
                    if bytes[k] == b'%' && k + 2 < bytes.len() {
                        if let Ok(v) = u8::from_str_radix(
                            std::str::from_utf8(&bytes[k+1..k+3]).unwrap_or("zz"), 16) {
                            dec.push(v as char); k += 3; continue;
                        }
                    }
                    dec.push(bytes[k] as char); k += 1;
                }
                out.push(fs::read(base.join(&dec)).unwrap_or_default());
            }
            None => {
                // GLB: buffer 0 is the binary chunk
                if i == 0 {
                    out.push(glb_chunk.clone().unwrap_or_default());
                } else {
                    out.push(Vec::new());
                }
            }
        }
    }
    out
}

fn comp_size(t: usize) -> usize {
    match t { 5120 | 5121 => 1, 5122 | 5123 => 2, _ => 4 }
}
fn n_comp(t: &str) -> usize {
    match t { "SCALAR" => 1, "VEC2" => 2, "VEC3" => 3, "VEC4" => 4,
              "MAT4" => 16, _ => 1 }
}

/// Read an accessor as f32 triples or u32 scalars, honouring byteStride.
fn accessor_f32(c: &Ctx, idx: usize) -> Vec<f32> {
    let acc = match c.j.get("accessors").and_then(|a| a.idx(idx)) { Some(a) => a, None => return vec![] };
    let count = acc.u("count").unwrap_or(0);
    let ct = acc.u("componentType").unwrap_or(5126);
    let ty = acc.s("type").unwrap_or("VEC3").to_string();
    let nc = n_comp(&ty);
    let bv = match acc.u("bufferView") { Some(b) => b, None => return vec![0.0; count * nc] };
    let view = match c.j.get("bufferViews").and_then(|a| a.idx(bv)) { Some(v) => v, None => return vec![] };
    let buf = view.u("buffer").unwrap_or(0);
    let data = match c.bufs.get(buf) { Some(d) => d, None => return vec![] };
    let base = view.u("byteOffset").unwrap_or(0) + acc.u("byteOffset").unwrap_or(0);
    let stride = view.u("byteStride").unwrap_or(0);
    let esz = comp_size(ct);
    let step = if stride > 0 { stride } else { esz * nc };
    let mut out = Vec::with_capacity(count * nc);
    for i in 0..count {
        for k in 0..nc {
            let o = base + i * step + k * esz;
            if o + esz > data.len() { out.push(0.0); continue; }
            let v = match ct {
                5126 => f32::from_le_bytes([data[o], data[o+1], data[o+2], data[o+3]]),
                5125 => u32::from_le_bytes([data[o], data[o+1], data[o+2], data[o+3]]) as f32,
                5123 => u16::from_le_bytes([data[o], data[o+1]]) as f32,
                5122 => i16::from_le_bytes([data[o], data[o+1]]) as f32,
                5121 => data[o] as f32,
                5120 => data[o] as i8 as f32,
                _ => 0.0,
            };
            out.push(v);
        }
    }
    out
}

fn accessor_idx(c: &Ctx, idx: usize) -> Vec<u32> {
    accessor_f32(c, idx).into_iter().map(|v| v as u32).collect()
}

fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut m = [0.0f32; 16];
    for r in 0..4 { for col in 0..4 {
        let mut s = 0.0;
        for k in 0..4 { s += a[k * 4 + r] * b[col * 4 + k]; }
        m[col * 4 + r] = s;
    }}
    m
}

fn node_matrix(n: &J) -> [f32; 16] {
    if let Some(J::Arr(v)) = n.get("matrix") {
        let mut m = [0.0f32; 16];
        for (i, x) in v.iter().take(16).enumerate() {
            if let J::Num(f) = x { m[i] = *f as f32; }
        }
        return m;
    }
    // TRS: scale, then rotate by the quaternion, then translate
    let t = n.get("translation").map(|a| a.arr()).unwrap_or(&[]);
    let r = n.get("rotation").map(|a| a.arr()).unwrap_or(&[]);
    let s = n.get("scale").map(|a| a.arr()).unwrap_or(&[]);
    let g = |a: &[J], i: usize, d: f32| -> f32 {
        match a.get(i) { Some(J::Num(v)) => *v as f32, _ => d }
    };
    let (tx, ty, tz) = (g(t,0,0.0), g(t,1,0.0), g(t,2,0.0));
    let (qx, qy, qz, qw) = (g(r,0,0.0), g(r,1,0.0), g(r,2,0.0), g(r,3,1.0));
    let (sx, sy, sz) = (g(s,0,1.0), g(s,1,1.0), g(s,2,1.0));
    let (x2, y2, z2) = (qx + qx, qy + qy, qz + qz);
    let (xx, xy, xz) = (qx * x2, qx * y2, qx * z2);
    let (yy, yz, zz) = (qy * y2, qy * z2, qz * z2);
    let (wx, wy, wz) = (qw * x2, qw * y2, qw * z2);
    [ (1.0-(yy+zz))*sx, (xy+wz)*sx,       (xz-wy)*sx,       0.0,
      (xy-wz)*sy,       (1.0-(xx+zz))*sy, (yz+wx)*sy,       0.0,
      (xz+wy)*sz,       (yz-wx)*sz,       (1.0-(xx+yy))*sz, 0.0,
      tx,               ty,               tz,               1.0 ]
}

fn xform(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [ m[0]*p[0] + m[4]*p[1] + m[8]*p[2]  + m[12],
      m[1]*p[0] + m[5]*p[1] + m[9]*p[2]  + m[13],
      m[2]*p[0] + m[6]*p[1] + m[10]*p[2] + m[14] ]
}

fn add_mesh(c: &Ctx, mi: usize, world: &[f32; 16], out: &mut Mesh) {
    let mesh = match c.j.get("meshes").and_then(|a| a.idx(mi)) { Some(m) => m, None => return };
    for prim in mesh.get("primitives").map(|a| a.arr()).unwrap_or(&[]) {
        let mode = prim.u("mode").unwrap_or(4);
        if mode < 4 { continue; }                       // points and lines carry no surface
        let pos_i = match prim.get("attributes").and_then(|a| a.u("POSITION")) {
            Some(p) => p, None => continue };
        let pos = accessor_f32(c, pos_i);
        let nv = pos.len() / 3;
        if nv == 0 { continue; }
        let base = out.v.len() as u32;
        for i in 0..nv {
            out.v.push(xform(world, [pos[i*3], pos[i*3+1], pos[i*3+2]]));
        }
        let idx: Vec<u32> = match prim.u("indices") {
            Some(ai) => accessor_idx(c, ai),
            None => (0..nv as u32).collect(),
        };
        match mode {
            4 => for t in idx.chunks(3) {
                    if t.len() == 3 { out.f.push([base+t[0], base+t[1], base+t[2]]); }
                 },
            5 => for i in 0..idx.len().saturating_sub(2) {      // triangle strip
                    let (a, b, cc) = if i % 2 == 0 { (idx[i], idx[i+1], idx[i+2]) }
                                     else { (idx[i+1], idx[i], idx[i+2]) };
                    if a != b && b != cc && a != cc { out.f.push([base+a, base+b, base+cc]); }
                 },
            6 => for i in 1..idx.len().saturating_sub(1) {      // triangle fan
                    out.f.push([base+idx[0], base+idx[i], base+idx[i+1]]);
                 },
            _ => {}
        }
    }
}

fn walk_node(c: &Ctx, ni: usize, parent: &[f32; 16], out: &mut Mesh, depth: usize) {
    if depth > 32 { return; }
    let node = match c.j.get("nodes").and_then(|a| a.idx(ni)) { Some(n) => n, None => return };
    let world = mat_mul(parent, &node_matrix(node));
    if let Some(mi) = node.u("mesh") { add_mesh(c, mi, &world, out); }
    for ch in node.get("children").map(|a| a.arr()).unwrap_or(&[]) {
        if let J::Num(k) = ch { walk_node(c, *k as usize, &world, out, depth + 1); }
    }
}

pub fn load(path: &Path) -> Option<Mesh> {
    let raw = fs::read(path).ok()?;
    let (jbytes, chunk) = if raw.len() > 12 && &raw[0..4] == b"glTF" {
        // GLB: 12-byte header then JSON chunk then BIN chunk
        let mut off = 12usize;
        let mut js: Vec<u8> = Vec::new();
        let mut bin: Option<Vec<u8>> = None;
        while off + 8 <= raw.len() {
            let len = u32::from_le_bytes([raw[off], raw[off+1], raw[off+2], raw[off+3]]) as usize;
            let ty = u32::from_le_bytes([raw[off+4], raw[off+5], raw[off+6], raw[off+7]]);
            let s = off + 8;
            let e = (s + len).min(raw.len());
            if ty == 0x4E4F534A { js = raw[s..e].to_vec(); }
            else if ty == 0x004E4942 { bin = Some(raw[s..e].to_vec()); }
            off = e;
        }
        (js, bin)
    } else {
        (raw, None)
    };
    let j = parse(&jbytes)?;
    let base = path.parent().unwrap_or(Path::new("."));
    let bufs = read_buffers(&j, base, chunk);
    let c = Ctx { j, bufs };

    let mut out = Mesh::default();
    let ident = [1.0,0.0,0.0,0.0, 0.0,1.0,0.0,0.0, 0.0,0.0,1.0,0.0, 0.0,0.0,0.0,1.0];
    let scenes = c.j.get("scenes").map(|a| a.arr()).unwrap_or(&[]);
    let si = c.j.u("scene").unwrap_or(0);
    let roots: Vec<usize> = scenes.get(si)
        .and_then(|s| s.get("nodes"))
        .map(|a| a.arr().iter().filter_map(|n| match n { J::Num(v) => Some(*v as usize), _ => None }).collect())
        .unwrap_or_default();
    if roots.is_empty() {
        // no scene graph: take every mesh at identity
        let nm = c.j.get("meshes").map(|a| a.arr().len()).unwrap_or(0);
        for mi in 0..nm { add_mesh(&c, mi, &ident, &mut out); }
    } else {
        for r in roots { walk_node(&c, r, &ident, &mut out, 0); }
    }
    if out.f.is_empty() { None } else { Some(out) }
}
