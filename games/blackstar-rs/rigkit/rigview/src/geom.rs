//! The page's geometry steps for shader-dissolved panels, ported: subdivide
//! big triangles (tessellate), take smooth normals, then unshare so each
//! triangle carries its own centre as `aCell`.

use std::collections::HashMap;

/// Port of the page's `tessellate`: longest-edge bisection of triangles that
/// are both long and big, with a shared midpoint cache.
pub fn tessellate(p: &[[f32; 3]], idx: &[u32], max_edge: f32, max_area: f32) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut pos = p.to_vec();
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let d2 = |pos: &Vec<[f32; 3]>, a: u32, b: u32| {
        let (x, y) = (pos[a as usize], pos[b as usize]);
        (x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)
    };
    let area = |pos: &Vec<[f32; 3]>, a: u32, b: u32, c: u32| {
        let (pa, pb, pc) = (pos[a as usize], pos[b as usize], pos[c as usize]);
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let cx = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        0.5 * (cx[0] * cx[0] + cx[1] * cx[1] + cx[2] * cx[2]).sqrt()
    };
    let e2 = max_edge * max_edge;
    let mut st: Vec<[u32; 3]> = idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut out = Vec::with_capacity(idx.len());
    let mut guard = 0u32;
    while let Some([a, b, c]) = st.pop() {
        let (ab, bc, ca) = (d2(&pos, a, b), d2(&pos, b, c), d2(&pos, c, a));
        let mx = ab.max(bc).max(ca);
        guard += 1;
        if mx <= e2 || area(&pos, a, b, c) <= max_area || guard > 4_000_000 {
            out.extend_from_slice(&[a, b, c]);
            continue;
        }
        let mut midpoint = |x: u32, y: u32| -> u32 {
            let k = if x < y { (x, y) } else { (y, x) };
            *mid.entry(k).or_insert_with(|| {
                let (px, py) = (pos[x as usize], pos[y as usize]);
                pos.push([(px[0] + py[0]) * 0.5, (px[1] + py[1]) * 0.5, (px[2] + py[2]) * 0.5]);
                (pos.len() - 1) as u32
            })
        };
        if mx == ab {
            let m = midpoint(a, b);
            st.push([a, m, c]);
            st.push([m, b, c]);
        } else if mx == bc {
            let m = midpoint(b, c);
            st.push([a, b, m]);
            st.push([a, m, c]);
        } else {
            let m = midpoint(c, a);
            st.push([a, b, m]);
            st.push([m, b, c]);
        }
    }
    (pos, out)
}

/// Area-weighted smooth normals, as three.js computeVertexNormals.
pub fn normals(p: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    let mut n = vec![[0f32; 3]; p.len()];
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
        let cb = [c[0] - b[0], c[1] - b[1], c[2] - b[2]];
        let ab = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let f = [cb[1] * ab[2] - cb[2] * ab[1], cb[2] * ab[0] - cb[0] * ab[2], cb[0] * ab[1] - cb[1] * ab[0]];
        for &v in t {
            for k in 0..3 { n[v as usize][k] += f[k]; }
        }
    }
    for v in n.iter_mut() {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        for k in 0..3 { v[k] /= l; }
    }
    n
}

/// Interleaved [pos, normal, aCell] per vertex.
pub struct Mesh {
    pub verts: Vec<f32>,
    pub idx: Vec<u32>,
}

/// For a streamed panel: subdivide, smooth-normal, then unshare with aCell.
pub fn streamed(p: &[[f32; 3]], idx: &[u32], max_edge: f32, max_area: f32) -> Mesh {
    let (tp, ti) = tessellate(p, idx, max_edge, max_area);
    let tn = normals(&tp, &ti);
    let mut verts = Vec::with_capacity(ti.len() * 9);
    for t in ti.chunks_exact(3) {
        let c = [0usize, 1, 2].map(|k| (tp[t[0] as usize][k] + tp[t[1] as usize][k] + tp[t[2] as usize][k]) / 3.0);
        for &v in t {
            verts.extend_from_slice(&tp[v as usize]);
            verts.extend_from_slice(&tn[v as usize]);
            verts.extend_from_slice(&c);
        }
    }
    let n = (verts.len() / 9) as u32;
    Mesh { verts, idx: (0..n).collect() }
}

/// For a panel that is not dissolved: as-is, smooth normals, aCell unused.
pub fn plain(p: &[[f32; 3]], idx: &[u32]) -> Mesh {
    let n = normals(p, idx);
    let mut verts = Vec::with_capacity(p.len() * 9);
    for (i, v) in p.iter().enumerate() {
        verts.extend_from_slice(v);
        verts.extend_from_slice(&n[i]);
        verts.extend_from_slice(&[0.0, 0.0, 0.0]);
    }
    Mesh { verts, idx: idx.to_vec() }
}
