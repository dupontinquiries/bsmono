//! Splitting a mesh into parts, and saying what each one is.
//!
//! A vehicle mesh arrives as one object. Everything downstream -- which panel
//! becomes a pauldron, where the armour's section comes from -- needs it in
//! pieces, and the pieces have to be found rather than read off, because the
//! files do not carry them: orion is 387 distinct parts under one node, one
//! mesh and one material.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Island {
    pub verts: Vec<[f32; 3]>,
    pub tris: Vec<[u32; 3]>,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

impl Island {
    pub fn size(&self) -> [f32; 3] {
        [self.hi[0] - self.lo[0], self.hi[1] - self.lo[1], self.hi[2] - self.lo[2]]
    }
    pub fn centre(&self) -> [f32; 3] {
        [(self.lo[0] + self.hi[0]) * 0.5,
         (self.lo[1] + self.hi[1]) * 0.5,
         (self.lo[2] + self.hi[2]) * 0.5]
    }
    /// Longest, middle, shortest extent.
    pub fn sorted_size(&self) -> [f32; 3] {
        let mut s = self.size();
        s.sort_by(|a, b| b.partial_cmp(a).unwrap());
        s
    }
}

struct Dsu(Vec<u32>);
impl Dsu {
    fn new(n: usize) -> Self { Dsu((0..n as u32).collect()) }
    fn find(&mut self, mut a: u32) -> u32 {
        while self.0[a as usize] != a {
            self.0[a as usize] = self.0[self.0[a as usize] as usize];
            a = self.0[a as usize];
        }
        a
    }
    fn join(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb { self.0[ra as usize] = rb; }
    }
}

/// Weld by quantised position, then union-find over triangle edges.
///
/// Welding first is not optional. An exported mesh duplicates a vertex per
/// face corner for normals and UVs, so without it every triangle is its own
/// island -- 115,386 one-triangle components on a model with 387 real parts.
pub fn split(pos: &[[f32; 3]], idx: &[u32]) -> Vec<Island> {
    if pos.is_empty() { return vec![]; }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in pos {
        for a in 0..3 { lo[a] = lo[a].min(p[a]); hi[a] = hi[a].max(p[a]); }
    }
    let scale = (0..3).map(|a| hi[a] - lo[a]).fold(0.0f32, f32::max).max(1e-6);
    let q = scale * 2e-4;

    let mut key: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut rep = vec![0u32; pos.len()];
    let mut welded: Vec<[f32; 3]> = Vec::new();
    for (i, p) in pos.iter().enumerate() {
        let k = ((p[0] / q).round() as i64, (p[1] / q).round() as i64,
                 (p[2] / q).round() as i64);
        let e = *key.entry(k).or_insert_with(|| {
            welded.push(*p);
            (welded.len() - 1) as u32
        });
        rep[i] = e;
    }
    let mut dsu = Dsu::new(welded.len());
    for t in idx.chunks_exact(3) {
        dsu.join(rep[t[0] as usize], rep[t[1] as usize]);
        dsu.join(rep[t[1] as usize], rep[t[2] as usize]);
    }
    let mut groups: HashMap<u32, Vec<[u32; 3]>> = HashMap::new();
    for t in idx.chunks_exact(3) {
        let r = dsu.find(rep[t[0] as usize]);
        groups.entry(r).or_default().push(
            [rep[t[0] as usize], rep[t[1] as usize], rep[t[2] as usize]]);
    }

    let mut out: Vec<Island> = groups.into_values().map(|tris| {
        let mut used: Vec<u32> = tris.iter().flatten().copied().collect();
        used.sort_unstable();
        used.dedup();
        let remap: HashMap<u32, u32> =
            used.iter().enumerate().map(|(i, &o)| (o, i as u32)).collect();
        let verts: Vec<[f32; 3]> = used.iter().map(|&i| welded[i as usize]).collect();
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &verts {
            for a in 0..3 { lo[a] = lo[a].min(v[a]); hi[a] = hi[a].max(v[a]); }
        }
        Island {
            tris: tris.iter().map(|t| [remap[&t[0]], remap[&t[1]], remap[&t[2]]]).collect(),
            verts, lo, hi,
        }
    }).collect();

    // DETERMINISTIC ORDER. Sorting by vertex count alone leaves ties broken by
    // hash iteration order, and a car has four tyres of near-identical size.
    // Anything that caches islands by index then disagrees with anything that
    // recomputes them -- a wheel group holding one wheel's box and another
    // wheel's mesh. Break ties on the centroid and the order is a property of
    // the geometry.
    out.sort_by(|a, b| {
        b.verts.len().cmp(&a.verts.len())
            .then_with(|| a.centre()[0].partial_cmp(&b.centre()[0]).unwrap())
            .then_with(|| a.centre()[1].partial_cmp(&b.centre()[1]).unwrap())
            .then_with(|| a.centre()[2].partial_cmp(&b.centre()[2]).unwrap())
    });
    out
}
