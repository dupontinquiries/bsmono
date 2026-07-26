//! GAN training-pair encoder — port of shape_pairs.py.
//!
//! Encodes a component as six orthographic DEPTH views at 64x64, tiled 2 rows x 4
//! columns = 128x256 single channel in [-1,1]. That is deliberately the same
//! tensor shape and range as the kBM3 mel cache, so the existing Rust trainer
//! reads it without touching its data loader.
//!
//! The low-detail side is produced by DECIMATION, not voxelisation. Measured in
//! the python original: voxelising carried MORE depth variation than the detail it
//! was meant to degrade (0.506 vs 0.415), because cube edges are hard steps —
//! training on that teaches the model to remove blockiness rather than add detail.

use crate::mesh::Mesh;
use std::collections::HashMap;

pub const RES: usize = 64;
pub const H: usize = 128;
pub const W: usize = 256;

/// (u, v, w, sign) for +Z, -Z, +X, -X, +Y, -Y
const VIEWS: [(usize, usize, usize, f32); 6] = [
    (0, 1, 2, 1.0), (0, 1, 2, -1.0),
    (2, 1, 0, 1.0), (2, 1, 0, -1.0),
    (0, 2, 1, 1.0), (0, 2, 1, -1.0),
];

fn depth_view(m: &Mesh, u: usize, v: usize, w: usize, sign: f32) -> Vec<f32> {
    let mut img = vec![-1.0f32; RES * RES];
    if m.f.is_empty() { return img; }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &m.v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let span = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
    // far-to-near so nearer surfaces overwrite
    let mut order: Vec<usize> = (0..m.f.len()).collect();
    let dep = |t: &[u32; 3]| -> f32 {
        let d = (m.v[t[0] as usize][w] + m.v[t[1] as usize][w] + m.v[t[2] as usize][w]) / 3.0;
        let n = (d - lo[w]) / span[w];
        if sign < 0.0 { 1.0 - n } else { n }
    };
    order.sort_by(|&a, &b| dep(&m.f[a]).total_cmp(&dep(&m.f[b])));
    for &fi in &order {
        let t = m.f[fi];
        let p = [m.v[t[0] as usize], m.v[t[1] as usize], m.v[t[2] as usize]];
        let c = [(p[0][0]+p[1][0]+p[2][0])/3.0, (p[0][1]+p[1][1]+p[2][1])/3.0,
                 (p[0][2]+p[1][2]+p[2][2])/3.0];
        let mut r = 0.0f32;
        for q in &p { for k in 0..3 { r = r.max((q[k]-c[k]).abs()); } }
        let x = (((c[u]-lo[u]) / span[u]) * (RES as f32 - 1.0)) as i32;
        let y = (((c[v]-lo[v]) / span[v]) * (RES as f32 - 1.0)) as i32;
        let rad = (((r / ((span[u] + span[v]) * 0.5)) * RES as f32 * 0.6) as i32).max(1);
        let d = dep(&t) * 2.0 - 1.0;
        for yy in (y-rad).max(0)..=(y+rad).min(RES as i32 - 1) {
            for xx in (x-rad).max(0)..=(x+rad).min(RES as i32 - 1) {
                let i = yy as usize * RES + xx as usize;
                if d > img[i] { img[i] = d; }
            }
        }
    }
    img
}

/// Six views tiled 2x4 into a 128x256 plane.
pub fn encode(m: &Mesh) -> Vec<f32> {
    let tiles: Vec<Vec<f32>> = VIEWS.iter()
        .map(|&(u, v, w, s)| depth_view(m, u, v, w, s)).collect();
    let mut out = vec![-1.0f32; H * W];
    for (ti, tile) in tiles.iter().enumerate() {
        let (tr, tc) = (ti / 4, ti % 4);
        for y in 0..RES {
            for x in 0..RES {
                out[(tr * RES + y) * W + tc * RES + x] = tile[y * RES + x];
            }
        }
    }
    out
}

/// The user's proposal: degrade a part to its BOUNDING BOX. This is a far stronger
/// degradation than decimation, which is what the measured failure needed --
/// decimated blobs sat within 3-14% of the target, so a residual net just copied
/// its input. A box shares only the part's extent, so "what fills a box of this
/// shape" is a real question with a real answer.
pub fn boxify(m: &Mesh) -> Mesh {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &m.v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let v = vec![
        [lo[0], lo[1], lo[2]], [hi[0], lo[1], lo[2]], [hi[0], hi[1], lo[2]], [lo[0], hi[1], lo[2]],
        [lo[0], lo[1], hi[2]], [hi[0], lo[1], hi[2]], [hi[0], hi[1], hi[2]], [lo[0], hi[1], hi[2]],
    ];
    let f = vec![[0,2,1],[0,3,2],[4,5,6],[4,6,7],[0,1,5],[0,5,4],
                 [1,2,6],[1,6,5],[2,3,7],[2,7,6],[3,0,4],[3,4,7]];
    Mesh { v, f }
}

/// Placement descriptor for the FIRST stage of the two-stage design: where a part
/// sits, how big it is, how it is turned. Nine numbers instead of a 32k image,
/// which is a far better-posed learning problem than pixel-to-pixel.
pub fn placement(m: &Mesh, parent_lo: [f32; 3], parent_span: [f32; 3]) -> [f32; 9] {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut c = [0.0f64; 3];
    for p in &m.v {
        for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); c[k] += p[k] as f64; }
    }
    let n = m.v.len().max(1) as f64;
    let ext = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
    let big = ext[0].max(ext[1]).max(ext[2]);
    [ // position, normalised into the parent box
      ((c[0]/n) as f32 - parent_lo[0]) / parent_span[0],
      ((c[1]/n) as f32 - parent_lo[1]) / parent_span[1],
      ((c[2]/n) as f32 - parent_lo[2]) / parent_span[2],
      // scale relative to the parent
      ext[0] / parent_span[0], ext[1] / parent_span[1], ext[2] / parent_span[2],
      // orientation proxy: normalised aspect, which is rotation-revealing without
      // needing a full frame estimate
      ext[0] / big, ext[1] / big, ext[2] / big ]
}

/// Vertex-clustering decimation: snap vertices to a grid, drop degenerate faces.
/// Simple, fast, and it produces exactly what a low-poly model is — fewer, larger
/// triangles following the same silhouette.
pub fn decimate(m: &Mesh, target: usize) -> Mesh {
    // Vertex clustering is a coarse control -- one pass overshoots the target by
    // 2-4x because the grid resolution is quantised. Iterate the grid down until
    // the face count actually lands, or the mesh stops shrinking.
    let mut cur = decimate_once(m, target);
    for _ in 0..6 {
        if cur.f.len() <= target { break; }
        let next = decimate_once(&cur, target);
        if next.f.len() >= cur.f.len() { break; }
        cur = next;
    }
    cur
}

fn decimate_once(m: &Mesh, target: usize) -> Mesh {
    if m.f.len() <= target || target == 0 { return m.clone(); }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &m.v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let span = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
    // grid resolution chosen so the surviving face count lands near the target
    let ratio = (target as f32 / m.f.len() as f32).max(1e-4);
    let g = ((m.v.len() as f32 * ratio).cbrt().max(2.0) * 2.0) as i64;
    let mut cell: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut nv: Vec<[f32; 3]> = Vec::new();
    let mut acc: Vec<([f64; 3], usize)> = Vec::new();
    let mut map = vec![0u32; m.v.len()];
    for (i, p) in m.v.iter().enumerate() {
        let k = ((((p[0]-lo[0]) / span[0]) * g as f32) as i64,
                 (((p[1]-lo[1]) / span[1]) * g as f32) as i64,
                 (((p[2]-lo[2]) / span[2]) * g as f32) as i64);
        let id = *cell.entry(k).or_insert_with(|| {
            nv.push([0.0; 3]);
            acc.push(([0.0; 3], 0));
            (nv.len() - 1) as u32
        });
        map[i] = id;
        let a = &mut acc[id as usize];
        for kk in 0..3 { a.0[kk] += p[kk] as f64; }
        a.1 += 1;
    }
    for (i, (s, n)) in acc.iter().enumerate() {
        for k in 0..3 { nv[i][k] = (s[k] / (*n).max(1) as f64) as f32; }
    }
    let mut nf = Vec::new();
    let mut seen: HashMap<(u32, u32, u32), ()> = HashMap::new();
    for t in &m.f {
        let a = map[t[0] as usize];
        let b = map[t[1] as usize];
        let c = map[t[2] as usize];
        if a == b || b == c || a == c { continue; }
        let mut k = [a, b, c];
        k.sort();
        if seen.insert((k[0], k[1], k[2]), ()).is_some() { continue; }
        nf.push([a, b, c]);
    }
    Mesh { v: nv, f: nf }
}

/// Encode a part's CONTEXT as a second plane: where it sits within its model, how
/// big it is relative to the model, and its aspect.
///
/// This is the missing conditioning. A lone bounding box is genuinely ambiguous --
/// many different parts fit a box of the same proportions -- so a net given only
/// the box cannot do better than emit the average part, which is exactly what it
/// did (+34% vs identity, +1% vs mean, and visibly the same texture for
/// structurally different targets). Position within the model is what
/// disambiguates: a box at the shoulder and an identical box at the ankle should
/// not be filled the same way.
///
/// Laid out as horizontal bands so a convolution can read it locally rather than
/// having to integrate a constant across the whole plane.
pub fn context_plane(pl: &[f32; 9]) -> Vec<f32> {
    let mut o = vec![-1.0f32; H * W];
    let band = H / 9;
    for (i, v) in pl.iter().enumerate() {
        // descriptors are 0..1; map to [-1,1] to match the depth planes
        let val = (v.clamp(0.0, 1.0) * 2.0 - 1.0).clamp(-1.0, 1.0);
        for y in i * band..((i + 1) * band).min(H) {
            for x in 0..W { o[y * W + x] = val; }
        }
    }
    o
}

/// Write the interleaved f32 stream the trainer reads: blob, detail, blob, ...
pub fn write_pairs(path: &std::path::Path, pairs: &[(Vec<f32>, Vec<f32>)])
    -> std::io::Result<()> {
    use std::io::Write;
    let mut fh = std::io::BufWriter::new(std::fs::File::create(path)?);
    for (b, d) in pairs {
        for x in b { fh.write_all(&x.to_le_bytes())?; }
        for x in d { fh.write_all(&x.to_le_bytes())?; }
    }
    fh.flush()
}

/// blob, context, detail -- three planes per sample.
pub fn write_triples(path: &std::path::Path, t: &[(Vec<f32>, Vec<f32>, Vec<f32>)])
    -> std::io::Result<()> {
    use std::io::Write;
    let mut fh = std::io::BufWriter::new(std::fs::File::create(path)?);
    for (b, c, d) in t {
        for x in b { fh.write_all(&x.to_le_bytes())?; }
        for x in c { fh.write_all(&x.to_le_bytes())?; }
        for x in d { fh.write_all(&x.to_le_bytes())?; }
    }
    fh.flush()
}

/// PGM preview, so the encoding can be eyeballed before any GPU time is spent.
pub fn write_pgm(path: &std::path::Path, img: &[f32]) -> std::io::Result<()> {
    use std::io::Write;
    let mut fh = std::fs::File::create(path)?;
    write!(fh, "P5\n{} {}\n255\n", W, H)?;
    let b: Vec<u8> = img.iter()
        .map(|v| (((v.clamp(-1.0, 1.0) + 1.0) * 127.5) as u8)).collect();
    fh.write_all(&b)
}
