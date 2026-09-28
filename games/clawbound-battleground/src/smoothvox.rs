//! smoothvox -- a character part's voxels as a smooth surface, for the
//! ultra graphics preview.
//!
//! A part's mesh stores only its HULL voxels (the inside is never seen), so
//! the solid is recovered first: a flood fill from outside marks the air, and
//! everything it cannot reach is solid. Then surface nets: one vertex per
//! grid cell the surface passes through, placed at the mean of its crossing
//! edges, and a quad for every solid/air face -- a watertight mesh. A few
//! rounds of Laplacian smoothing take the voxel steps out. Colours are the
//! mean of each vertex's solid neighbours; normals come from the smoothed
//! faces.
//!
//! Headless and tested; `render` uploads the result and draws it with PBR
//! shading and shell fur (each vertex carries a fur length).

use std::collections::HashMap;

use crate::voxel::VoxelMesh;

/// How far (in luminance) a voxel must stand out from its neighbours to be
/// a feature -- an eye, a nose, a stripe -- that keeps its colour and shape.
pub const FEATURE_CONTRAST: f32 = 0.22;

/// One vertex of a smooth mesh (matches the smooth pipeline's layout).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SmoothVert {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 4],
    /// How long the fur is here, world units (0: bare).
    pub fur: f32,
    pub _pad: [f32; 3],
}

#[derive(Debug, Clone, Default)]
pub struct SmoothMesh {
    pub verts: Vec<SmoothVert>,
    pub indices: Vec<u32>,
}

impl SmoothMesh {
    pub fn tris(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Smooth a voxel hull: `mesh`'s instance offsets are voxel centres in
/// model units, `voxel` the voxel size; `iterations` of Laplacian smoothing
/// (2-3 rounds a voxel model without melting its features).
pub fn smooth(mesh: &VoxelMesh, voxel: f32, iterations: usize) -> SmoothMesh {
    if mesh.inst.is_empty() || voxel <= 0.0 {
        return SmoothMesh::default();
    }
    // integer cells, padded by 2 so the outside is connected all round
    let cell = |o: [f32; 4]| -> [i32; 3] { [(o[0] / voxel - 0.5).round() as i32, (o[1] / voxel - 0.5).round() as i32, (o[2] / voxel - 0.5).round() as i32] };
    let cells: Vec<[i32; 3]> = mesh.inst.iter().map(|i| cell(i.off_ao)).collect();
    let mut lo = [i32::MAX; 3];
    let mut hi = [i32::MIN; 3];
    for c in &cells {
        for a in 0..3 {
            lo[a] = lo[a].min(c[a]);
            hi[a] = hi[a].max(c[a]);
        }
    }
    let pad = 2;
    let dims = [(hi[0] - lo[0] + 1 + 2 * pad) as usize, (hi[1] - lo[1] + 1 + 2 * pad) as usize, (hi[2] - lo[2] + 1 + 2 * pad) as usize];
    let idx = |x: usize, y: usize, z: usize| (z * dims[1] + y) * dims[0] + x;
    let n = dims[0] * dims[1] * dims[2];
    // 0 unknown, 1 hull, 2 air
    let mut state = vec![0u8; n];
    let mut color = vec![[0.0f32; 4]; n];
    for (c, inst) in cells.iter().zip(&mesh.inst) {
        let (x, y, z) = ((c[0] - lo[0] + pad) as usize, (c[1] - lo[1] + pad) as usize, (c[2] - lo[2] + pad) as usize);
        state[idx(x, y, z)] = 1;
        color[idx(x, y, z)] = inst.color;
    }
    // flood the air from a corner (the padding makes the outside one piece)
    let mut stack = vec![(0usize, 0usize, 0usize)];
    state[0] = 2;
    while let Some((x, y, z)) = stack.pop() {
        let mut visit = |x: usize, y: usize, z: usize| {
            let i = idx(x, y, z);
            if state[i] == 0 {
                state[i] = 2;
                stack.push((x, y, z));
            }
        };
        if x > 0 { visit(x - 1, y, z); }
        if y > 0 { visit(x, y - 1, z); }
        if z > 0 { visit(x, y, z - 1); }
        if x + 1 < dims[0] { visit(x + 1, y, z); }
        if y + 1 < dims[1] { visit(x, y + 1, z); }
        if z + 1 < dims[2] { visit(x, y, z + 1); }
    }
    // the interior takes the colour of the nearest hull voxel (a layer at a
    // time, so every solid cell has one)
    let solid = |s: u8| s != 2;
    let mut filled: Vec<bool> = state.iter().map(|&s| s == 1).collect();
    loop {
        let mut changed = false;
        for z in 1..dims[2] - 1 {
            for y in 1..dims[1] - 1 {
                for x in 1..dims[0] - 1 {
                    let i = idx(x, y, z);
                    if state[i] == 0 && !filled[i] {
                        for j in [idx(x - 1, y, z), idx(x + 1, y, z), idx(x, y - 1, z), idx(x, y + 1, z), idx(x, y, z - 1), idx(x, y, z + 1)] {
                            if filled[j] {
                                color[i] = color[j];
                                filled[i] = true;
                                changed = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // surface nets: a vertex per cell of 8 samples with both solid and air
    let mut vert_of: HashMap<(usize, usize, usize), u32> = HashMap::new();
    let mut pos: Vec<[f32; 3]> = Vec::new();
    // vertices on a feature (see below): smoothed only a little, so an eye
    // keeps its shape
    let mut feature: Vec<bool> = Vec::new();
    let mut col: Vec<[f32; 4]> = Vec::new();
    for z in 0..dims[2] - 1 {
        for y in 0..dims[1] - 1 {
            for x in 0..dims[0] - 1 {
                let corners = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1), (1, 1, 1)];
                let s: Vec<bool> = corners.iter().map(|(a, b, c)| solid(state[idx(x + a, y + b, z + c)])).collect();
                if s.iter().all(|&v| v) || s.iter().all(|&v| !v) {
                    continue;
                }
                // mean of the crossing edges' midpoints
                let mut p = [0.0f32; 3];
                let mut k = 0.0;
                let edges = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
                for (a, b) in edges {
                    if s[a] != s[b] {
                        let (ca, cb) = (corners[a], corners[b]);
                        p[0] += (ca.0 + cb.0) as f32 * 0.5;
                        p[1] += (ca.1 + cb.1) as f32 * 0.5;
                        p[2] += (ca.2 + cb.2) as f32 * 0.5;
                        k += 1.0;
                    }
                }
                let mut c = [0.0f32; 4];
                let mut kc = 0.0;
                let mut solid_cols: Vec<[f32; 4]> = Vec::with_capacity(8);
                for (j, (a, b, cz)) in corners.iter().enumerate() {
                    if s[j] {
                        let cc = color[idx(x + a, y + b, z + cz)];
                        for t in 0..4 {
                            c[t] += cc[t];
                        }
                        kc += 1.0;
                        solid_cols.push(cc);
                    }
                }
                // A feature -- an eye, a nose, a stripe: one voxel standing out
                // from those round it -- keeps its colour instead of being
                // averaged into the fur (the first close-up had no eyes).
                let lum = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let mean_l = lum([c[0] / kc, c[1] / kc, c[2] / kc, 1.0]);
                let standout = solid_cols.iter().copied().max_by(|a, b| (lum(*a) - mean_l).abs().partial_cmp(&(lum(*b) - mean_l).abs()).unwrap());
                let is_feature = standout.is_some_and(|f| (lum(f) - mean_l).abs() > FEATURE_CONTRAST);
                if let (true, Some(f)) = (is_feature, standout) {
                    c = [f[0] * kc, f[1] * kc, f[2] * kc, f[3] * kc];
                }
                feature.push(is_feature);
                vert_of.insert((x, y, z), pos.len() as u32);
                // sample (x,y,z) sits at the voxel centre of cell (x-pad+lo)
                let world = |a: usize, v: f32, axis: usize| ((a as i32 - pad + lo[axis]) as f32 + 0.5 + v) * voxel;
                pos.push([world(x, p[0] / k, 0), world(y, p[1] / k, 1), world(z, p[2] / k, 2)]);
                col.push([c[0] / kc, c[1] / kc, c[2] / kc, c[3] / kc]);
            }
        }
    }
    // a quad for every solid/air face between two samples, joining the four
    // cells that share that edge; wound so the face looks into the air
    let mut indices: Vec<u32> = Vec::new();
    for z in 1..dims[2] - 1 {
        for y in 1..dims[1] - 1 {
            for x in 1..dims[0] - 1 {
                let here = solid(state[idx(x, y, z)]);
                for axis in 0..3 {
                    let (nx, ny, nz) = match axis {
                        0 => (x + 1, y, z),
                        1 => (x, y + 1, z),
                        _ => (x, y, z + 1),
                    };
                    if nx >= dims[0] || ny >= dims[1] || nz >= dims[2] {
                        continue;
                    }
                    let there = solid(state[idx(nx, ny, nz)]);
                    if here == there {
                        continue;
                    }
                    // the four cells round the edge between the two samples
                    let quad: [(usize, usize, usize); 4] = match axis {
                        0 => [(x, y - 1, z - 1), (x, y, z - 1), (x, y, z), (x, y - 1, z)],
                        1 => [(x - 1, y, z - 1), (x - 1, y, z), (x, y, z), (x, y, z - 1)],
                        _ => [(x - 1, y - 1, z), (x, y - 1, z), (x, y, z), (x - 1, y, z)],
                    };
                    let v: Option<Vec<u32>> = quad.iter().map(|c| vert_of.get(c).copied()).collect();
                    let Some(v) = v else { continue };
                    let (a, b, c, d) = (v[0], v[1], v[2], v[3]);
                    if here {
                        indices.extend_from_slice(&[a, b, c, a, c, d]);
                    } else {
                        indices.extend_from_slice(&[a, c, b, a, d, c]);
                    }
                }
            }
        }
    }
    // Laplacian smoothing over the mesh's edges
    let mut nbrs: Vec<Vec<u32>> = vec![Vec::new(); pos.len()];
    for t in indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if !nbrs[a as usize].contains(&b) {
                nbrs[a as usize].push(b);
            }
            if !nbrs[b as usize].contains(&a) {
                nbrs[b as usize].push(a);
            }
        }
    }
    for _ in 0..iterations {
        let prev = pos.clone();
        for (i, ns) in nbrs.iter().enumerate() {
            if ns.is_empty() {
                continue;
            }
            let mut m = [0.0f32; 3];
            for &j in ns {
                for a in 0..3 {
                    m[a] += prev[j as usize][a];
                }
            }
            let w = if feature[i] { 0.12 } else { 0.5 };
            for a in 0..3 {
                pos[i][a] = prev[i][a] * (1.0 - w) + m[a] / ns.len() as f32 * w;
            }
        }
    }
    // normals from the faces
    let mut nrm = vec![[0.0f32; 3]; pos.len()];
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (glam::Vec3::from(pos[t[0] as usize]), glam::Vec3::from(pos[t[1] as usize]), glam::Vec3::from(pos[t[2] as usize]));
        let f = (b - a).cross(c - a);
        for &k in t {
            for ax in 0..3 {
                nrm[k as usize][ax] += f[ax];
            }
        }
    }
    let verts = pos
        .iter()
        .zip(&nrm)
        .zip(&col)
        .map(|((p, n), c)| {
            let n = glam::Vec3::from(*n).normalize_or_zero();
            SmoothVert { pos: *p, normal: n.into(), color: *c, fur: 0.0, _pad: [0.0; 3] }
        })
        .collect();
    SmoothMesh { verts, indices }
}

/// Fur for a part: a base length everywhere, with `extra` more on vertices
/// whose model-space position satisfies `more` (a mane round the head and
/// neck, a brush at the tail's tip).
pub fn set_fur(m: &mut SmoothMesh, base: f32, extra: f32, more: impl Fn(glam::Vec3) -> f32) {
    for v in m.verts.iter_mut() {
        v.fur = base + extra * more(glam::Vec3::from(v.pos)).clamp(0.0, 1.0);
    }
}

/// Fur on one part of an ancestor (a `rigged::Slot` index), in world
/// units. Short on the legs, medium on body and head, lengthening down the
/// tail to a brush at its tip. A MANE only on the maned (Goldenmane's Mane
/// pelt, Oldroar's Hulk -- "a ragged mane"; dev/LORE.md holds the same
/// line): long fur round the back of the head -- not the face -- and on the
/// front of the chest. The model faces +z.
/// `bare_face`: the locks style keeps the face short and sleek; tape hair
/// (the default, the player's pick) furs the whole head, as it first did --
/// the eyes still show, their colour kept in the vertices the strands take.
pub fn fur_for_part(def: &crate::gods::GodDef, slot: usize, m: &mut SmoothMesh, bare_face: bool) {
    use crate::rigged::Slot;
    if m.verts.is_empty() {
        return;
    }
    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for v in &m.verts {
        lo = lo.min(glam::Vec3::from(v.pos));
        hi = hi.max(glam::Vec3::from(v.pos));
    }
    let span_z = (hi.z - lo.z).max(1e-4);
    let maned = matches!(def.pelt, crate::gods::PeltKind::Mane | crate::gods::PeltKind::Hulk);
    let base = match slot {
        s if s == Slot::Torso as usize => 0.05,
        s if s == Slot::Head as usize => 0.04,
        s if s == Slot::Tail0 as usize => 0.045,
        s if s == Slot::Tail1 as usize => 0.05,
        s if s == Slot::Tail2 as usize => 0.06,
        s if s == Slot::Tail3 as usize => 0.07,
        _ => 0.03, // legs
    };
    if slot == Slot::Tail3 as usize {
        // the brush: longest toward the free end (the tail runs away, -z)
        set_fur(m, base, 0.06, |p| 1.0 - (p.z - lo.z) / span_z);
    } else if slot == Slot::Head as usize {
        // The face is BARE -- a cat's face fur is short and sleek -- and a
        // mane frames it from behind. Fur over the whole head buried the
        // eyes and made a featureless mask (the first close-up).
        let mane = if maned { 0.13 } else { 0.0 };
        for v in m.verts.iter_mut() {
            let f = (v.pos[2] - lo.z) / span_z; // 0 back .. 1 face
            let behind = (1.0 - (f - 0.1) / 0.45).clamp(0.0, 1.0);
            v.fur = if !bare_face {
                base + mane * behind
            } else if f > 0.55 {
                0.004
            } else {
                base * behind + mane * behind
            };
        }
    } else if maned && slot == Slot::Torso as usize {
        // the front of the chest and shoulders
        set_fur(m, base, 0.09, |p| ((p.z - lo.z) / span_z - 0.65) / 0.25);
    } else {
        set_fur(m, base, 0.0, |_| 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel::Grid;
    use glam::Vec3;

    fn ball(r: f32) -> (VoxelMesh, f32, Vec3) {
        let n = (r * 2.0 + 4.0) as usize;
        let mut g = Grid::new(n, n, n);
        let c = n as f32 * 0.5;
        g.ellipsoid(c, c, c, r, r, r, [0.8, 0.5, 0.2, 1.0]);
        let v = 0.1;
        let m = g.mesh(v, false);
        (m, v, Vec3::ZERO)
    }

    /// A voxel ball becomes a smooth, closed surface on the ball: every
    /// edge shared by exactly two triangles, vertices near the radius,
    /// normals facing out, the colour kept.
    #[test]
    fn a_voxel_ball_becomes_a_smooth_closed_ball() {
        let (m, v, _) = ball(7.0);
        let s = smooth(&m, v, 3);
        assert!(s.tris() > 200, "{} triangles", s.tris());
        // closed: each undirected edge in exactly two triangles
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for t in s.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        let open = count.values().filter(|&&c| c != 2).count();
        assert_eq!(open, 0, "{open} edges not shared by exactly two faces");
        // on the ball, facing out
        let centre = s.verts.iter().fold(Vec3::ZERO, |a, v| a + Vec3::from(v.pos)) / s.verts.len() as f32;
        let radii: Vec<f32> = s.verts.iter().map(|v| (Vec3::from(v.pos) - centre).length()).collect();
        let mean = radii.iter().sum::<f32>() / radii.len() as f32;
        let dev = radii.iter().map(|r| (r - mean).abs()).fold(0.0f32, f32::max);
        assert!(dev < 0.9 * v, "not round: max deviation {dev} ({} voxels)", dev / v);
        let outward = s.verts.iter().filter(|x| Vec3::from(x.normal).dot(Vec3::from(x.pos) - centre) > 0.0).count();
        assert!(outward as f32 / s.verts.len() as f32 > 0.99, "{outward} of {} normals face out", s.verts.len());
        assert!(s.verts.iter().all(|x| (x.color[0] - 0.8).abs() < 0.2 && x.color[3] > 0.99), "colour lost");
    }

    /// Smoothing takes the voxel steps out: the ball is rounder after it.
    #[test]
    fn smoothing_rounds_the_steps() {
        let (m, v, _) = ball(6.0);
        let dev = |s: &SmoothMesh| {
            let c = s.verts.iter().fold(Vec3::ZERO, |a, x| a + Vec3::from(x.pos)) / s.verts.len() as f32;
            let r: Vec<f32> = s.verts.iter().map(|x| (Vec3::from(x.pos) - c).length()).collect();
            let mean = r.iter().sum::<f32>() / r.len() as f32;
            r.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / r.len() as f32
        };
        let rough = dev(&smooth(&m, v, 0));
        let round = dev(&smooth(&m, v, 3));
        assert!(round < rough * 0.6, "variance {round} vs {rough}");
    }

    /// Part meshes are hulls (the mesher keeps only visible voxels): the
    /// smoothing fills the inside instead of wrapping a second, inward
    /// surface round the hollow -- the result is ONE connected surface.
    #[test]
    fn a_hollow_hull_is_filled_not_doubled() {
        let (m, v, _) = ball(6.0);
        let volume = (4.0 / 3.0 * std::f32::consts::PI * 6.0f32.powi(3)) as usize;
        assert!(m.inst.len() < volume * 3 / 4, "the mesh is a hull: {} of ~{volume}", m.inst.len());
        let s = smooth(&m, v, 0);
        // connected components over the triangle edges
        let mut parent: Vec<usize> = (0..s.verts.len()).collect();
        fn find(p: &mut Vec<usize>, x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            let mut y = x;
            while p[y] != r {
                let n = p[y];
                p[y] = r;
                y = n;
            }
            r
        }
        for t in s.indices.chunks_exact(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2])] {
                let (ra, rb) = (find(&mut parent, a as usize), find(&mut parent, b as usize));
                parent[ra] = rb;
            }
        }
        let roots: std::collections::HashSet<usize> = (0..s.verts.len()).map(|i| find(&mut parent, i)).collect();
        assert_eq!(roots.len(), 1, "{} separate surfaces", roots.len());
    }

    #[test]
    fn fur_follows_its_rule_and_empty_is_empty() {
        let (m, v, _) = ball(5.0);
        let mut s = smooth(&m, v, 2);
        set_fur(&mut s, 0.05, 0.1, |p| if p.y > 0.2 { 1.0 } else { 0.0 });
        assert!(s.verts.iter().all(|x| x.fur >= 0.05 && x.fur <= 0.15 + 1e-6));
        assert!(s.verts.iter().any(|x| x.fur > 0.14) && s.verts.iter().any(|x| x.fur < 0.06));
        assert!(smooth(&VoxelMesh::default(), 0.1, 2).verts.is_empty());
    }

    /// The real thing: every part of every ancestor, both team colourings,
    /// smooths to a closed, non-empty surface.
    #[test]
    fn every_part_of_every_ancestor_smooths_closed() {
        let mut total = 0;
        for def in crate::gods::ROSTER.iter() {
            for team in [crate::consts::Team::Blue, crate::consts::Team::Red] {
                let rigged = crate::models::build_god_rigged(def, team);
                for (k, m) in rigged.meshes.iter().enumerate() {
                    if m.inst.is_empty() {
                        continue;
                    }
                    let s = smooth(m, crate::models::draw_voxel(def), 2);
                    assert!(s.tris() > 8, "{} part {k}: {} triangles", def.name, s.tris());
                    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
                    for t in s.indices.chunks_exact(3) {
                        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                            *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                        }
                    }
                    let holes = count.values().filter(|&&c| c == 1 || c == 3).count();
                    let pinches = count.values().filter(|&&c| c == 4).count();
                    if pinches > 0 {
                        println!("{} part {k}: {pinches} pinch edges (diagonal voxel contacts)", def.name);
                    }
                    assert_eq!(holes, 0, "{} part {k}: {holes} open edges (holes)", def.name);
                    total += s.tris();
                }
            }
        }
        println!("all parts smoothed: {total} triangles");
    }

    /// Manes on the maned only; legs short; every tail ends in a brush.
    #[test]
    fn fur_follows_the_anatomy_and_the_lore() {
        use crate::rigged::Slot;
        let longest = |def: &crate::gods::GodDef, slot: Slot| {
            let rigged = crate::models::build_god_rigged(def, crate::consts::Team::Blue);
            let mut m = smooth(&rigged.meshes[slot as usize], crate::models::GOD_DRAW_VOXEL, 2);
            fur_for_part(def, slot as usize, &mut m, false);
            m.verts.iter().map(|v| v.fur).fold(0.0f32, f32::max)
        };
        let find = |n: &str| crate::gods::ROSTER.iter().find(|d| d.name == n).unwrap();
        let (lion, tiger, cave) = (find("Goldenmane"), find("Duskstripe"), find("Oldroar"));
        assert!(longest(lion, Slot::Head) > longest(tiger, Slot::Head) * 2.0, "Goldenmane has a mane, Duskstripe does not");
        assert!(longest(cave, Slot::Head) > longest(tiger, Slot::Head) * 2.0, "Oldroar's ragged mane");
        for d in [lion, tiger, cave] {
            assert!(longest(d, Slot::LegFL) < longest(d, Slot::Torso), "{}: legs shorter than body", d.name);
            assert!(longest(d, Slot::Tail3) > longest(d, Slot::Tail0), "{}: the tail ends in a brush", d.name);
        }
    }
}
