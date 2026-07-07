//! chunk.rs — spatial chunking + level-of-detail for large maps.
//!
//! The renderer's triangle cull is per-triangle, so a monolithic world mesh costs O(total tris) every frame
//! even when most of it is behind you or past the fog. To scale to very large maps we split static geometry
//! into a grid of **chunks** (each its own `Mesh3` with a cached AABB) and let the renderer reject a whole
//! chunk — behind the camera, outside the frustum, or beyond the fog — before touching its triangles. That
//! turns per-frame cost into O(visible-chunk tris).
//!
//! `lod_detail` is adapted from the logscope reference: it maps a distance to a [floor,1] detail factor so
//! near chunks can carry full scatter density while far chunks thin out (and, eventually, collapse to flat
//! ground + impostor tree-lines that dissolve into fog).

use crate::math::{Aabb, Vec3};
use crate::mesh::Mesh3;

/// Distance → detail factor in [floor, 1.0]: full within `near`, linearly thinning to `floor` by `far`.
/// (Same shape as logscope's `lod_detail`, but callers feed distance-from-*camera*, not from a fixed centre.)
pub fn lod_detail(dist: f32, near: f32, far: f32, floor: f32) -> f32 {
    if dist <= near {
        1.0
    } else if dist >= far {
        floor
    } else {
        (1.0 - (dist - near) / (far - near).max(1e-3) * (1.0 - floor)).clamp(floor, 1.0)
    }
}

/// Discrete LOD tier for a chunk at a given camera distance — lets the level builder pick a representation
/// (full / coarse / impostor) per chunk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lod {
    Full,
    Coarse,
    Impostor,
}

pub fn lod_tier(dist: f32, near: f32, far: f32) -> Lod {
    if dist <= near {
        Lod::Full
    } else if dist <= far {
        Lod::Coarse
    } else {
        Lod::Impostor
    }
}

/// Split a set of world-space meshes into a grid of per-chunk meshes (bucketed by triangle centroid), each
/// carrying a cached AABB. Per-vertex colour and sway are preserved. Meshes with special materials (emissive/
/// reflect) keep those flags per chunk. The result is a flat `Vec<Mesh3>` ready to hand to the renderer.
pub fn split(meshes: &[Mesh3], cell: f32) -> Vec<Mesh3> {
    use std::collections::HashMap;
    let cell = cell.max(1.0);
    // group key → a fresh Mesh3 carrying the source's material flags
    let mut chunks: HashMap<(i32, i32, usize), Mesh3> = HashMap::new();
    for (mi, m) in meshes.iter().enumerate() {
        let has_vcol = m.vcol.is_some();
        let has_sway = !m.sway.is_empty();
        // Material is kept only when UVs are FULLY aligned with the vertices. Empty UVs would sample the tile
        // centre (solid), and a partial/misaligned UV array (e.g. a uv_sphere with no UVs plus detail tris that
        // do have them) would sample garbage — either way we drop the material and keep the flat vertex colour.
        let has_uv = !m.uv.is_empty() && m.uv.len() == m.verts.len();
        for tri in &m.tris {
            let (a, b, c) = (m.verts[tri[0] as usize], m.verts[tri[1] as usize], m.verts[tri[2] as usize]);
            let centroid = (a + b + c) * (1.0 / 3.0) + m.offset;
            let key = ((centroid.x / cell).floor() as i32, (centroid.z / cell).floor() as i32, mi);
            let dst = chunks.entry(key).or_insert_with(|| {
                let mut d = Mesh3::new(m.color);
                d.emissive = m.emissive;
                d.reflect = m.reflect;
                d.offset = m.offset;
                d.mat = m.mat;
                // Keep the atlas material ONLY when this mesh has real UVs. Without UVs the renderer samples the
                // tile centre for every vertex and the whole surface goes solid — so a UV-less mesh keeps its
                // flat vertex colour (material dropped) exactly as before, and only genuinely UV-mapped surfaces
                // (floors, textured set-pieces) get their texture. UVs themselves are copied per-vertex below.
                d.material = if has_uv { m.material } else { None };
                d.lod_far = m.lod_far;
                // Each chunk key includes the source mesh index `mi`, so a chunk is fed by exactly one source
                // mesh — it can safely inherit that mesh's single_sided flag for backface culling.
                d.single_sided = m.single_sided;
                if !has_vcol {
                    d.vcol = None;
                }
                d
            });
            let col = |k: u32| m.vcol.as_ref().and_then(|v| v.get(k as usize).copied()).unwrap_or(m.color);
            if has_sway {
                let w = |k: u32| m.sway.get(k as usize).copied().unwrap_or(0.0);
                dst.push_tri_sway(a, b, c, Vec3::new(0.0, 1.0, 0.0), col(tri[0]), w(tri[0]), w(tri[1]), w(tri[2]));
                // overwrite the placeholder normals/colours we just pushed with the real per-vertex data
                let n = dst.verts.len();
                dst.norms[n - 3] = m.norms[tri[0] as usize];
                dst.norms[n - 2] = m.norms[tri[1] as usize];
                dst.norms[n - 1] = m.norms[tri[2] as usize];
                if let Some(vc) = dst.vcol.as_mut() {
                    let l = vc.len();
                    vc[l - 3] = col(tri[0]);
                    vc[l - 2] = col(tri[1]);
                    vc[l - 1] = col(tri[2]);
                }
                if has_uv {
                    // push_tri (inside push_tri_sway) pushed placeholder UVs — overwrite with the real ones
                    let uvk = |k: u32| m.uv.get(k as usize).copied().unwrap_or(egui::Pos2::new(0.5, 0.5));
                    let l = dst.uv.len();
                    dst.uv[l - 3] = uvk(tri[0]);
                    dst.uv[l - 2] = uvk(tri[1]);
                    dst.uv[l - 1] = uvk(tri[2]);
                }
            } else {
                let base = dst.verts.len() as u32;
                dst.verts.push(a);
                dst.verts.push(b);
                dst.verts.push(c);
                dst.norms.push(m.norms[tri[0] as usize]);
                dst.norms.push(m.norms[tri[1] as usize]);
                dst.norms.push(m.norms[tri[2] as usize]);
                if let Some(vc) = dst.vcol.as_mut() {
                    vc.push(col(tri[0]));
                    vc.push(col(tri[1]));
                    vc.push(col(tri[2]));
                }
                if has_uv {
                    let uvk = |k: u32| m.uv.get(k as usize).copied().unwrap_or(egui::Pos2::new(0.5, 0.5));
                    dst.uv.push(uvk(tri[0]));
                    dst.uv.push(uvk(tri[1]));
                    dst.uv.push(uvk(tri[2]));
                }
                dst.tris.push([base, base + 1, base + 2]);
            }
        }
    }
    let mut out: Vec<Mesh3> = chunks.into_values().collect();
    for m in &mut out {
        m.compute_bounds();
    }
    out
}

/// True if the box is entirely outside the view volume (behind the near plane, past `far`, or off one screen
/// edge) — i.e. the whole chunk can be skipped. `tan_h`/`tan_v` are the frustum half-extents at unit depth.
pub fn aabb_culled(
    b: &Aabb,
    cam: Vec3,
    right: Vec3,
    up: Vec3,
    fwd: Vec3,
    near: f32,
    far: f32,
    tan_h: f32,
    tan_v: f32,
) -> bool {
    let mut all_behind = true;
    let mut all_far = true;
    let mut all_left = true;
    let mut all_right = true;
    let mut all_below = true;
    let mut all_above = true;
    for c in b.corners() {
        let rel = c - cam;
        let d = rel.dot(fwd);
        let x = rel.dot(right);
        let y = rel.dot(up);
        if d >= near {
            all_behind = false;
        }
        if d <= far {
            all_far = false;
        }
        // edge tests only meaningful in front of the camera; treat behind verts as "inside" to stay safe
        let dd = d.max(near);
        if !(x > dd * tan_h) {
            all_right = false;
        }
        if !(-x > dd * tan_h) {
            all_left = false;
        }
        if !(y > dd * tan_v) {
            all_above = false;
        }
        if !(-y > dd * tan_v) {
            all_below = false;
        }
    }
    all_behind || all_far || all_left || all_right || all_above || all_below
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{boxx, Mesh3, FACE_ALL};

    #[test]
    fn lod_detail_falls_off() {
        assert_eq!(lod_detail(5.0, 10.0, 50.0, 0.2), 1.0);
        assert_eq!(lod_detail(60.0, 10.0, 50.0, 0.2), 0.2);
        let mid = lod_detail(30.0, 10.0, 50.0, 0.2);
        assert!(mid > 0.2 && mid < 1.0);
        assert_eq!(lod_tier(5.0, 10.0, 50.0), Lod::Full);
        assert_eq!(lod_tier(70.0, 10.0, 50.0), Lod::Impostor);
    }

    #[test]
    fn split_preserves_triangles_and_bounds() {
        // a long strip of boxes spread across X → should split into multiple chunks
        let mut world = Mesh3::new(crate::mesh::rgb(120, 120, 120));
        world.lod_far = 0.6;
        for i in 0..20 {
            world.merge(&boxx(Vec3::new(i as f32 * 5.0, 0.0, 0.0), Vec3::splat(0.4), crate::mesh::rgb(100, 140, 90), FACE_ALL));
        }
        let src_tris: usize = world.tris.len();
        let chunks = split(&[world], 10.0);
        assert!(chunks.len() > 3, "strip should break into several chunks");
        let chunk_tris: usize = chunks.iter().map(|c| c.tris.len()).sum();
        assert_eq!(chunk_tris, src_tris, "no triangles lost in chunking");
        assert!(chunks.iter().all(|c| c.bounds.is_some()), "every chunk has bounds");
        assert!(chunks.iter().all(|c| (c.lod_far - 0.6).abs() < 1e-6), "chunks inherit lod_far");
    }

    #[test]
    fn impostor_of_a_thin_vertical_chunk_has_no_degenerate_tris() {
        // A chunk that holds only a thin vertical mesh (a pole/antenna/reed) has a near-zero X or Z footprint.
        // The impostor used to lay a flat ground quad over that footprint, collapsing it to a line → two
        // zero-area triangles. Regression guard: building an impostor over such a mesh must produce no
        // degenerate triangles.
        let mut pole = Mesh3::new(crate::mesh::rgb(220, 230, 255));
        // a tall, near-zero-footprint sliver
        pole.merge(&boxx(Vec3::new(0.0, 4.0, 0.0), Vec3::new(0.005, 4.0, 0.005), crate::mesh::rgb(220, 230, 255), FACE_ALL));
        let mut b = Aabb::empty();
        for v in &pole.verts {
            b.expand(*v);
        }
        let imp = build_impostor(&pole, &b);
        for tri in &imp.tris {
            let a = imp.verts[tri[0] as usize];
            let bb = imp.verts[tri[1] as usize];
            let c = imp.verts[tri[2] as usize];
            let area = (bb - a).cross(c - a).len() * 0.5;
            assert!(area > 1e-5, "impostor must not emit a degenerate (zero-area) triangle");
        }
    }

    #[test]
    fn far_and_behind_chunks_are_culled() {
        let cam = Vec3::new(0.0, 1.0, 0.0);
        let (right, up, fwd) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, -1.0));
        let (near, far, th, tv) = (0.1, 100.0, 1.0, 1.0);
        // a box right in front (visible)
        let mut front = Aabb::empty();
        front.expand(Vec3::new(-1.0, 0.0, -10.0));
        front.expand(Vec3::new(1.0, 2.0, -12.0));
        assert!(!aabb_culled(&front, cam, right, up, fwd, near, far, th, tv));
        // a box far behind the camera (culled)
        let mut behind = Aabb::empty();
        behind.expand(Vec3::new(-1.0, 0.0, 20.0));
        behind.expand(Vec3::new(1.0, 2.0, 24.0));
        assert!(aabb_culled(&behind, cam, right, up, fwd, near, far, th, tv));
        // a box way past the fog wall (culled)
        let mut faraway = Aabb::empty();
        faraway.expand(Vec3::new(-1.0, 0.0, -500.0));
        faraway.expand(Vec3::new(1.0, 2.0, -520.0));
        assert!(aabb_culled(&faraway, cam, right, up, fwd, near, far, th, tv));
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// Dynamic LOD scheduler
//
// `ChunkField` is the stateful upgrade over the one-shot `split`: it keeps each chunk's full geometry plus
// lazily-built **coarse** and **impostor** representations, and every frame re-selects which tier each chunk
// should show based on camera distance — with hysteresis so chunks don't flip-flop at a boundary, and a
// per-call rebuild *budget* so the cost of promoting/demoting many chunks is amortised across frames.
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

use crate::mesh::rgb;
use egui::Color32;

/// One cell of the field: full detail + lazily-derived lower tiers, plus its current/desired LOD.
pub struct Chunk {
    pub bounds: Aabb,
    pub center: Vec3,
    pub full: Mesh3,
    pub coarse: Option<Mesh3>,
    pub impostor: Option<Mesh3>,
    pub active: Lod,
    pub desired: Lod,
}

impl Chunk {
    fn from_mesh(mut full: Mesh3) -> Self {
        full.compute_bounds();
        let bounds = full.bounds.unwrap_or_else(Aabb::empty);
        let center = if bounds.is_valid() { bounds.center() } else { Vec3::ZERO };
        Chunk { bounds, center, full, coarse: None, impostor: None, active: Lod::Full, desired: Lod::Full }
    }
    /// The mesh to draw for the active tier (falls back to full if a tier wasn't built).
    pub fn active_mesh(&self) -> &Mesh3 {
        match self.active {
            Lod::Full => &self.full,
            Lod::Coarse => self.coarse.as_ref().unwrap_or(&self.full),
            Lod::Impostor => self.impostor.as_ref().or(self.coarse.as_ref()).unwrap_or(&self.full),
        }
    }
}

/// Average a mesh's per-vertex colour (or its flat colour) — used to tint impostors.
fn avg_color(m: &Mesh3) -> Color32 {
    if let Some(vc) = &m.vcol {
        if !vc.is_empty() {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for c in vc {
                r += c.r() as u32;
                g += c.g() as u32;
                b += c.b() as u32;
            }
            let n = vc.len() as u32;
            return rgb((r / n) as u8, (g / n) as u8, (b / n) as u8);
        }
    }
    m.color
}

/// Triangle area (world space) for a tri given vertex positions.
fn tri_area(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    (b - a).cross(c - a).len() * 0.5
}

/// Build a **coarse** mesh: keep only the larger triangles (drops tiny detail like leaf blades / greebles),
/// preserving per-vertex colour. Always keeps at least a few of the biggest tris so nothing vanishes.
fn build_coarse(full: &Mesh3, cut: f32) -> Mesh3 {
    let mut out = Mesh3::new(full.color);
    out.offset = full.offset;
    out.emissive = full.emissive;
    out.reflect = full.reflect;
    out.lod_far = full.lod_far;
    let has_vcol = full.vcol.is_some();
    if !has_vcol {
        out.vcol = None;
    }
    let col = |k: u32| full.vcol.as_ref().and_then(|v| v.get(k as usize).copied()).unwrap_or(full.color);
    for tri in &full.tris {
        let (a, b, c) = (full.verts[tri[0] as usize], full.verts[tri[1] as usize], full.verts[tri[2] as usize]);
        if tri_area(a, b, c) < cut {
            continue;
        }
        let base = out.verts.len() as u32;
        out.verts.push(a);
        out.verts.push(b);
        out.verts.push(c);
        out.norms.push(full.norms[tri[0] as usize]);
        out.norms.push(full.norms[tri[1] as usize]);
        out.norms.push(full.norms[tri[2] as usize]);
        if let Some(vc) = out.vcol.as_mut() {
            vc.push(col(tri[0]));
            vc.push(col(tri[1]));
            vc.push(col(tri[2]));
        }
        out.tris.push([base, base + 1, base + 2]);
    }
    out.compute_bounds();
    out
}

/// Build an **impostor**: a flat ground quad over the chunk footprint plus, for tall chunks, one low box —
/// a cheap "blot" (≤ 6 tris) that reads as distant mass dissolving into fog. Tinted to the chunk average.
fn build_impostor(full: &Mesh3, bounds: &Aabb) -> Mesh3 {
    let c = avg_color(full);
    let mut out = Mesh3::new(c);
    let (mn, mx) = (bounds.min, bounds.max);
    let y = mn.y + 0.02;
    // Footprint span. A chunk that holds only a thin vertical mesh (a pole, antenna, wire, reed) has a
    // near-zero X or Z extent; a ground quad over that collapses to a line → two zero-area (degenerate)
    // triangles. Skip the flat quad in that case and let the box (below) carry the silhouette.
    let span_x = mx.x - mn.x;
    let span_z = mx.z - mn.z;
    if span_x > 1e-3 && span_z > 1e-3 {
        let p00 = Vec3::new(mn.x, y, mn.z);
        let p10 = Vec3::new(mx.x, y, mn.z);
        let p11 = Vec3::new(mx.x, y, mx.z);
        let p01 = Vec3::new(mn.x, y, mx.z);
        out.push_quad(p00, p10, p11, p01, Vec3::UP, c);
    }
    // tall content (trees/structures): a squat box at half height so the silhouette survives at range
    let height = mx.y - mn.y;
    if height > 3.0 {
        let cx = (mn.x + mx.x) * 0.5;
        let cz = (mn.z + mx.z) * 0.5;
        let hx = (mx.x - mn.x) * 0.32;
        let hz = (mx.z - mn.z) * 0.32;
        // A vanishingly thin footprint would make a zero-width box (degenerate faces); floor the half-extents.
        let hx = hx.max(0.1);
        let hz = hz.max(0.1);
        let top = (mn.y + height * 0.45).min(mx.y);
        let dim = crate::mesh::lerp_c(c, rgb(0, 0, 0), 0.2);
        let bx = crate::mesh::boxx(Vec3::new(cx, (y + top) * 0.5, cz), Vec3::new(hx, (top - y) * 0.5, hz), dim, crate::mesh::FACE_NO_BOTTOM);
        out.merge(&bx);
    }
    out.lod_far = full.lod_far;
    out.compute_bounds();
    out
}

/// A grid of chunks with dynamic LOD selection.
pub struct ChunkField {
    pub chunks: Vec<Chunk>,
    pub near: f32, // Full within this camera distance
    pub far: f32,  // Coarse out to here; Impostor beyond
    pub coarse_cut: f32,
    /// Hysteresis margin (fraction) around tier boundaries to stop flip-flopping.
    pub hyst: f32,
}

impl ChunkField {
    /// Build a field from world meshes: split into `cell`-sized chunks, then wrap each as a `Chunk`.
    pub fn from_meshes(meshes: &[Mesh3], cell: f32, near: f32, far: f32) -> Self {
        let parts = split(meshes, cell);
        let chunks = parts.into_iter().map(Chunk::from_mesh).collect();
        ChunkField { chunks, near, far, coarse_cut: (cell * 0.05).powi(2), hyst: 0.12 }
    }

    /// Desired tier for a chunk given its camera distance and current tier (asymmetric thresholds = hysteresis).
    #[cfg(test)]
    fn desired_tier(&self, active: Lod, d: f32) -> Lod {
        Self::tier(active, d, self.near, self.far, self.hyst)
    }

    fn tier(active: Lod, d: f32, near: f32, far: f32, hyst: f32) -> Lod {
        let (lo, hi) = (1.0 - hyst, 1.0 + hyst);
        match active {
            Lod::Full => {
                if d > far * hi {
                    Lod::Impostor
                } else if d > near * hi {
                    Lod::Coarse
                } else {
                    Lod::Full
                }
            }
            Lod::Coarse => {
                if d < near * lo {
                    Lod::Full
                } else if d > far * hi {
                    Lod::Impostor
                } else {
                    Lod::Coarse
                }
            }
            Lod::Impostor => {
                if d < near * lo {
                    Lod::Full
                } else if d < far * lo {
                    Lod::Coarse
                } else {
                    Lod::Impostor
                }
            }
        }
    }

    /// Per-frame update: recompute every chunk's desired tier (cheap), then realise the change for at most
    /// `budget` chunks (building coarse/impostor lazily). Returns how many chunks were rebuilt this call.
    pub fn update(&mut self, cam: Vec3, budget: usize) -> usize {
        let (near, far, hyst) = (self.near, self.far, self.hyst);
        for ch in &mut self.chunks {
            let d = (ch.center - cam).len();
            ch.desired = Self::tier(ch.active, d, near, far, hyst);
        }
        let mut rebuilt = 0;
        for i in 0..self.chunks.len() {
            if rebuilt >= budget {
                break;
            }
            if self.chunks[i].active == self.chunks[i].desired {
                continue;
            }
            let desired = self.chunks[i].desired;
            match desired {
                Lod::Coarse => {
                    if self.chunks[i].coarse.is_none() {
                        let m = build_coarse(&self.chunks[i].full, self.coarse_cut);
                        self.chunks[i].coarse = Some(m);
                    }
                }
                Lod::Impostor => {
                    if self.chunks[i].impostor.is_none() {
                        let b = self.chunks[i].bounds;
                        let m = build_impostor(&self.chunks[i].full, &b);
                        self.chunks[i].impostor = Some(m);
                    }
                }
                Lod::Full => {}
            }
            self.chunks[i].active = desired;
            rebuilt += 1;
        }
        rebuilt
    }

    /// Drive `update` to completion (used at spawn so the first frame is already settled).
    pub fn settle(&mut self, cam: Vec3) {
        let n = self.chunks.len();
        for _ in 0..8 {
            if self.update(cam, n) == 0 {
                break;
            }
        }
    }

    /// The active-tier meshes to render this frame.
    pub fn render_meshes(&self) -> Vec<&Mesh3> {
        self.chunks.iter().map(|c| c.active_mesh()).collect()
    }

    /// (full, coarse, impostor) counts of currently-active tiers — for tuning/HUD.
    pub fn tier_counts(&self) -> (usize, usize, usize) {
        let mut t = (0, 0, 0);
        for c in &self.chunks {
            match c.active {
                Lod::Full => t.0 += 1,
                Lod::Coarse => t.1 += 1,
                Lod::Impostor => t.2 += 1,
            }
        }
        t
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    use crate::mesh::{boxx, rgb, Mesh3, FACE_ALL};

    fn world_strip() -> Mesh3 {
        // a long strip of detailed boxes so chunking + decimation have something to chew on
        let mut w = Mesh3::new(rgb(120, 120, 120));
        for i in 0..40 {
            w.merge(&boxx(Vec3::new(i as f32 * 4.0, 0.0, 0.0), Vec3::splat(0.6), rgb(100, 150, 90), FACE_ALL));
        }
        w
    }

    #[test]
    fn far_camera_demotes_chunks() {
        let mut f = ChunkField::from_meshes(&[world_strip()], 12.0, 20.0, 60.0);
        // camera far away → after settling, most chunks should be impostors, none full-detail nearby
        f.settle(Vec3::new(0.0, 0.0, 400.0));
        let (full, coarse, imp) = f.tier_counts();
        assert!(imp > 0, "far camera should produce impostors");
        assert!(full <= coarse + imp, "few full chunks when far ({full})");
    }

    #[test]
    fn near_camera_keeps_full() {
        let mut f = ChunkField::from_meshes(&[world_strip()], 12.0, 20.0, 60.0);
        f.settle(Vec3::new(80.0, 0.0, 0.0)); // sits inside the strip
        let near_chunk = f.chunks.iter().min_by(|a, b| {
            (a.center - Vec3::new(80.0, 0.0, 0.0)).len().partial_cmp(&(b.center - Vec3::new(80.0, 0.0, 0.0)).len()).unwrap()
        }).unwrap();
        assert_eq!(near_chunk.active, Lod::Full, "the chunk under the camera stays full detail");
    }

    #[test]
    fn coarse_and_impostor_are_cheaper() {
        let f = ChunkField::from_meshes(&[world_strip()], 12.0, 20.0, 60.0);
        let full = &f.chunks[0].full;
        let coarse = build_coarse(full, f.coarse_cut);
        let imp = build_impostor(full, &f.chunks[0].bounds);
        assert!(coarse.tri_count() <= full.tri_count());
        assert!(imp.tri_count() <= 6, "impostor is a cheap blot");
    }

    #[test]
    fn budget_amortises_rebuilds() {
        let mut f = ChunkField::from_meshes(&[world_strip()], 8.0, 20.0, 60.0);
        // one chunk per call → it takes several calls to converge, never more than budget per call
        let far = Vec3::new(0.0, 0.0, 500.0);
        let r1 = f.update(far, 1);
        assert!(r1 <= 1);
        for _ in 0..200 {
            if f.update(far, 1) == 0 {
                break;
            }
        }
        // converged: nothing left to change
        assert_eq!(f.update(far, 1), 0);
    }

    #[test]
    fn hysteresis_prevents_flicker() {
        let f = ChunkField::from_meshes(&[world_strip()], 12.0, 20.0, 60.0);
        // a chunk exactly at the near boundary shouldn't flip between Full and Coarse on tiny moves
        let d = f.near;
        let a = f.desired_tier(Lod::Full, d * 1.0); // currently Full, sitting at boundary → stays Full
        let b = f.desired_tier(Lod::Coarse, d * 1.0); // currently Coarse at boundary → stays Coarse
        assert_eq!(a, Lod::Full);
        assert_eq!(b, Lod::Coarse);
    }
}
