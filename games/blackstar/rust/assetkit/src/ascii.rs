//! Text renderer — port of ascii_view.py.
//!
//! Worth its weight: every visual fault in this project (ribs placed out at the arm
//! span, models lying on their backs, greebles reading as scattered debris) was
//! invisible to the numeric checks and obvious within seconds of rendering the
//! silhouette as text.

use crate::mesh::Mesh;

const RAMP: &[u8] = b" .:-=+*#%@";

pub fn rotate(v: &[[f32; 3]], rx: f32, ry: f32, rz: f32) -> Vec<[f32; 3]> {
    let mut p = v.to_vec();
    for (ax, deg) in [(1usize, ry), (0, rx), (2, rz)] {
        if deg.abs() < 1e-6 { continue; }
        let a = deg.to_radians();
        let (c, s) = (a.cos(), a.sin());
        let (i, j) = match ax { 0 => (1, 2), 1 => (2, 0), _ => (0, 1) };
        for q in p.iter_mut() {
            let (x, y) = (q[i], q[j]);
            q[i] = x * c - y * s;
            q[j] = x * s + y * c;
        }
    }
    p
}

/// front | side | top
pub fn render(m: &Mesh, v: &[[f32; 3]], width: usize, view: &str) -> Vec<String> {
    let (u, vv, w) = match view {
        "side" => (2usize, 1usize, 0usize),
        "top" => (0, 2, 1),
        _ => (0, 1, 2),
    };
    if m.f.is_empty() { return vec!["(no geometry)".into()]; }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let span = [(hi[0]-lo[0]).max(1e-6), (hi[1]-lo[1]).max(1e-6), (hi[2]-lo[2]).max(1e-6)];
    let h = ((width as f32) * (span[vv] / span[u]) * 0.5).max(6.0) as usize;
    let mut img = vec![f32::MIN; h * width];
    // far-to-near so nearer surfaces win
    let mut order: Vec<usize> = (0..m.f.len()).collect();
    let depth = |t: &[u32; 3]| -> f32 {
        (v[t[0] as usize][w] + v[t[1] as usize][w] + v[t[2] as usize][w]) / 3.0
    };
    order.sort_by(|&a, &b| depth(&m.f[a]).total_cmp(&depth(&m.f[b])));
    for &fi in &order {
        let t = m.f[fi];
        let p: Vec<[f32; 3]> = t.iter().map(|&i| v[i as usize]).collect();
        let c = [(p[0][0]+p[1][0]+p[2][0])/3.0, (p[0][1]+p[1][1]+p[2][1])/3.0,
                 (p[0][2]+p[1][2]+p[2][2])/3.0];
        let mut r = 0.0f32;
        for q in &p { for k in 0..3 { r = r.max((q[k]-c[k]).abs()); } }
        let x = (((c[u]-lo[u]) / span[u]) * (width as f32 - 1.0)) as i32;
        let y = ((1.0 - (c[vv]-lo[vv]) / span[vv]) * (h as f32 - 1.0)) as i32;
        let rad = ((r / span[u]) * width as f32 * 0.5).max(0.0) as i32;
        let d = (c[w]-lo[w]) / span[w];
        for yy in (y-rad).max(0)..=(y+rad).min(h as i32 - 1) {
            for xx in (x-rad).max(0)..=(x+rad).min(width as i32 - 1) {
                let idx = yy as usize * width + xx as usize;
                if d > img[idx] { img[idx] = d; }
            }
        }
    }
    (0..h).map(|r| {
        let s: String = (0..width).map(|c| {
            let q = img[r * width + c];
            if q == f32::MIN { ' ' }
            else {
                let k = (((1.0 - q) * (RAMP.len() - 1) as f32) as usize).min(RAMP.len() - 1);
                RAMP[k] as char
            }
        }).collect();
        s.trim_end().to_string()
    }).collect()
}

/// Lying-down test. Height being the SMALLEST dimension by a clear margin is the
/// reliable signal -- "tallest axis is not Y" flags every T-posed bot, whose arm
/// span legitimately exceeds its height.
pub fn lying_down(ext: [f32; 3]) -> bool {
    let min = ext[0].min(ext[1]).min(ext[2]);
    ext[1] <= min + 1e-6 && ext[1] < 0.75 * ext[0].max(ext[2])
}
