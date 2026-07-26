//! Connected components, limb segmentation and rig validation — the Rust port of
//! part_harvest.py / limb_split.py.
//!
//! Two things here were expensive lessons in the python original and are carried
//! over deliberately:
//!   * WELD BEFORE LABELLING. Exported meshes frequently give every triangle its
//!     own vertices, in which case nothing shares an index and every triangle
//!     becomes its own island. A skeleton reference produced 115,386 one-triangle
//!     "components" that way.
//!   * VALIDATE THE SPLIT. Centroid classification fails badly on models that are
//!     not cleanly T-posed -- one arm got 6,818 triangles and the other 100, and
//!     animating that threw limbs a body-width off. A static correct model beats a
//!     broken articulated one, so a bad split is refused rather than shipped.

use crate::mesh::Mesh;
use std::collections::HashMap;

pub struct Component {
    pub v: Vec<[f32; 3]>,
    pub f: Vec<[u32; 3]>,
}

fn weld_ids(v: &[[f32; 3]], tol: f32) -> (Vec<u32>, usize) {
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut w = vec![0u32; v.len()];
    for (i, p) in v.iter().enumerate() {
        let k = (((p[0] / tol).round()) as i64,
                 ((p[1] / tol).round()) as i64,
                 ((p[2] / tol).round()) as i64);
        let n = map.len() as u32;
        w[i] = *map.entry(k).or_insert(n);
    }
    let n = map.len();
    (w, n)
}

fn find(p: &mut Vec<u32>, mut x: u32) -> u32 {
    while p[x as usize] != x { p[x as usize] = p[p[x as usize] as usize]; x = p[x as usize]; }
    x
}

/// Split into connected components, welding first. `min_tris` is applied during
/// the split so components destined to be discarded are never built -- in python
/// that alone was a 22x speedup on a fragmented model.
pub fn components(m: &Mesh, min_tris: usize) -> Vec<Component> {
    if m.f.is_empty() { return Vec::new(); }
    let (w, nw) = weld_ids(&m.v, 1e-5);
    let mut parent: Vec<u32> = (0..nw as u32).collect();
    for t in &m.f {
        let (a, b, c) = (w[t[0] as usize], w[t[1] as usize], w[t[2] as usize]);
        for (x, y) in [(a, b), (b, c)] {
            let (rx, ry) = (find(&mut parent, x), find(&mut parent, y));
            if rx != ry { parent[ry as usize] = rx; }
        }
    }
    let mut buckets: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, t) in m.f.iter().enumerate() {
        buckets.entry(find(&mut parent, w[t[0] as usize])).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_root, faces) in buckets {
        if faces.len() < min_tris { continue; }
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut cv = Vec::new();
        let mut cf = Vec::with_capacity(faces.len());
        for fi in faces {
            let t = m.f[fi];
            let mut nt = [0u32; 3];
            for k in 0..3 {
                let orig = t[k];
                let e = remap.entry(orig).or_insert_with(|| {
                    cv.push(m.v[orig as usize]);
                    (cv.len() - 1) as u32
                });
                nt[k] = *e;
            }
            cf.push(nt);
        }
        out.push(Component { v: cv, f: cf });
    }
    out
}

/// Which axis is the body's left/right? Not always X -- Zeta Prime mirrors about
/// Z, and assuming X put its limbs on the wrong sides.
pub fn mirror_axis(v: &[[f32; 3]]) -> usize {
    let n = v.len().min(900);
    let step = (v.len() / n.max(1)).max(1);
    let sample: Vec<[f32; 3]> = v.iter().step_by(step).copied().collect();
    let mut best = 0usize;
    let mut best_err = f32::MAX;
    for ax in [0usize, 2] {
        let mut err = 0.0f64;
        for p in &sample {
            let mut q = *p;
            q[ax] = -q[ax];
            let mut d = f32::MAX;
            for r in sample.iter().step_by(3) {
                let dd = (r[0]-q[0]).powi(2) + (r[1]-q[1]).powi(2) + (r[2]-q[2]).powi(2);
                if dd < d { d = dd; }
            }
            err += d as f64;
        }
        let err = (err / sample.len() as f64) as f32;
        if err < best_err { best_err = err; best = ax; }
    }
    best
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Limb { Body, Head, ArmL, ArmR, LegL, LegR }

impl Limb {
    pub fn tag(self) -> u32 {
        match self { Limb::Body => 0, Limb::Head => 9, Limb::ArmL => 5,
                     Limb::ArmR => 6, Limb::LegL => 7, Limb::LegR => 8 }
    }
    pub fn name(self) -> &'static str {
        match self { Limb::Body => "body", Limb::Head => "head", Limb::ArmL => "arm_l",
                     Limb::ArmR => "arm_r", Limb::LegL => "leg_l", Limb::LegR => "leg_r" }
    }
}

pub fn classify(centroid: [f32; 3], lo: [f32; 3], hi: [f32; 3], torso_hw: f32,
                ax: usize) -> Limb {
    let fy = (centroid[1] - lo[1]) / (hi[1] - lo[1]).max(1e-6);
    let x = centroid[ax] - (lo[ax] + hi[ax]) * 0.5;
    if fy > 0.86 { return Limb::Head; }
    if fy < 0.46 { return if x < 0.0 { Limb::LegL } else { Limb::LegR }; }
    if x.abs() > torso_hw { return if x < 0.0 { Limb::ArmL } else { Limb::ArmR }; }
    Limb::Body
}

/// Is this split safe to animate? Returns Err with the reason if not.
pub fn validate_split(groups: &HashMap<&'static str, Vec<usize>>,
                      tri_of: &dyn Fn(usize) -> usize) -> Result<(), String> {
    let bulk = |k: &str| -> usize {
        groups.get(k).map(|v| v.iter().map(|&i| tri_of(i)).sum()).unwrap_or(0)
    };
    for (l, r) in [("arm_l", "arm_r"), ("leg_l", "leg_r")] {
        let (a, b) = (bulk(l), bulk(r));
        if a.max(b) > 0 && a.min(b) < a.max(b) / 4 {
            return Err(format!("{}/{} split {} vs {} triangles -- too asymmetric to rig",
                               l, r, a, b));
        }
    }
    Ok(())
}
