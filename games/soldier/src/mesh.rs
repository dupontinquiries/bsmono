//! mesh.rs — `Mesh3`, the one geometry type the whole game speaks, plus low-poly primitives and the
//! `Light`. This is a direct descendant of `logscope::mesh3d::Mesh3`: world-space verts + per-vertex
//! normals + triangle indices + a base albedo, with an optional per-vertex colour (`vcol`) used as a
//! procedural "texture" (no image textures — exactly the reference's approach), a per-frame `offset`
//! (so a thing can bob without rebuilding geometry), a material id, and an emissive term.
//!
//! In the shooter architecture from the GLM-5 port, *every* subsystem — the level, enemies, the muzzle
//! flash, blood mist, the gun viewmodel — outputs `Vec<Mesh3>`, and the renderer flattens them all into a
//! single depth-sorted `egui` mesh. Keeping one geometry type is what makes that possible.

use crate::math::Vec3;
use egui::Color32;
use std::cell::Cell;

thread_local! {
    /// Global geometry-detail multiplier (the "render resolution" knob). 1.0 = baseline; higher tessellates
    /// curved primitives more finely. Set by the app from the render-resolution setting before building a map.
    static DETAIL: Cell<f32> = const { Cell::new(1.0) };
}

/// Set the geometry-detail multiplier (clamped to a sane range).
pub fn set_detail(d: f32) {
    DETAIL.with(|c| c.set(d.clamp(0.4, 3.0)));
}
/// Current geometry-detail multiplier.
pub fn detail() -> f32 {
    DETAIL.with(|c| c.get())
}
/// Scale a baseline segment count by the detail multiplier (min 3).
#[inline]
fn seg(base: usize) -> usize {
    ((base as f32 * detail()).round() as usize).max(3)
}

#[derive(Clone, Debug, Default)]
pub struct Mesh3 {
    pub verts: Vec<Vec3>,
    pub norms: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub color: Color32,
    /// Per-vertex albedo. When present it overrides `color` per vertex — this is our "texture".
    pub vcol: Option<Vec<Color32>>,
    /// Per-vertex UV into the material atlas, in local face space [0,1]; the renderer remaps it into this
    /// mesh's `material` tile. Parallel to `verts`. Empty → the renderer samples the flat white atlas tile and
    /// the surface keeps its vertex colour (particles, glow, water, foliage).
    pub uv: Vec<egui::Pos2>,
    /// Atlas material for per-pixel texturing. `None` → untextured (vertex colour only).
    pub material: Option<Material>,
    /// Added to every vertex at render time (bobbing, recoil, billboard placement) — no geometry rebuild.
    pub offset: Vec3,
    /// Material id (cosmetic tag used by the level builder to pick palettes; the renderer ignores it).
    pub mat: f32,
    /// Self-illumination: added on top of lit colour so muzzle flashes / plasma / lamps glow.
    pub emissive: f32,
    /// Reflectivity 0..1: how strongly this surface mirrors the sky (Schlick-fresnel environment reflection
    /// plus a tight sun glint). Water, ice and polished metal set this; matte surfaces leave it at 0.
    pub reflect: f32,
    /// Per-vertex wind-sway weight (0 = rooted, 1 = bends fully). Empty means the mesh is rigid. Grass blades
    /// and leaf canopies fill this so the renderer can bow their tips in the wind while bases stay planted.
    pub sway: Vec<f32>,
    /// Cached world-space bounds (incl. `offset`) for whole-mesh frustum culling. `None` = always considered
    /// visible (computed lazily by `compute_bounds`). Chunked level geometry sets this so the renderer can
    /// skip an entire off-screen or far chunk before touching its triangles.
    pub bounds: Option<crate::math::Aabb>,
    /// Per-mesh far-cull multiplier (1.0 = cull at the fog wall). Foliage chunks set this below 1 so distant
    /// leaves dissolve into fog earlier than terrain — logscope's "blot-out" falloff, applied per chunk.
    pub lod_far: f32,
    /// When true, the renderer may backface-cull this mesh's triangles (skip ones whose outward face normal
    /// points away from the camera) — a ~2x shading win on closed, opaque geometry. Only set on primitives
    /// with correct, consistent outward winding (boxes, tubes, spheres, terrain). Left false for foliage,
    /// water, particles, billboards and anything that must show both faces. Default false = never cull.
    pub single_sided: bool,
}

impl Mesh3 {
    pub fn new(color: Color32) -> Self {
        Mesh3 {
            color,
            vcol: Some(Vec::new()),
            lod_far: 1.0,
            ..Default::default()
        }
    }
    /// Builder: flag this mesh as self-illuminated (matches `mesh3d::Mesh3::emit`).
    pub fn emit(mut self, e: f32) -> Self {
        self.emissive = e;
        self
    }
    pub fn with_mat(mut self, m: f32) -> Self {
        self.mat = m;
        self
    }
    /// Builder: set environment reflectivity (0..1).
    pub fn with_reflect(mut self, r: f32) -> Self {
        self.reflect = r;
        self
    }

    /// Append a triangle from three world points with a shared face normal and per-vertex colour.
    pub fn push_tri(&mut self, a: Vec3, b: Vec3, c: Vec3, n: Vec3, col: Color32) {
        let base = self.verts.len() as u32;
        self.verts.push(a);
        self.verts.push(b);
        self.verts.push(c);
        for _ in 0..3 {
            self.norms.push(n);
        }
        // default triangle UVs (overwritten by push_quad for proper quad mapping)
        self.uv.push(egui::Pos2::new(0.0, 0.0));
        self.uv.push(egui::Pos2::new(1.0, 0.0));
        self.uv.push(egui::Pos2::new(0.0, 1.0));
        if let Some(vc) = self.vcol.as_mut() {
            for _ in 0..3 {
                vc.push(col);
            }
        }
        self.tris.push([base, base + 1, base + 2]);
    }

    /// Compute and cache world-space bounds (vertices + offset). Sway can push verts a little past these, so
    /// callers that sway should pad the box; for static chunks it's exact.
    pub fn compute_bounds(&mut self) {
        let mut b = crate::math::Aabb::empty();
        for &v in &self.verts {
            b.expand(v + self.offset);
        }
        if b.is_valid() {
            self.bounds = Some(b);
        }
    }

    pub fn push_tri_sway(&mut self, a: Vec3, b: Vec3, c: Vec3, n: Vec3, col: Color32, wa: f32, wb: f32, wc: f32) {
        // keep the sway array aligned with any verts pushed before sway tracking began
        if self.sway.len() < self.verts.len() {
            self.sway.resize(self.verts.len(), 0.0);
        }
        self.push_tri(a, b, c, n, col);
        self.sway.push(wa);
        self.sway.push(wb);
        self.sway.push(wc);
    }

    /// Append a quad (two tris) wound a→b→c→d. Mirrors the reference `push_quad`.
    pub fn push_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, n: Vec3, col: Color32) {
        self.push_quad_tiled(a, b, c, d, n, col, 1.0, 1.0);
    }

    /// Like `push_quad`, but the UV corners span `tile_u`×`tile_v` instead of always a unit square.
    /// **Only safe for values ≤ 1.0** — the renderer maps local UV linearly into a material's atlas
    /// sub-rect with no fragment-level wrap, so a value >1.0 samples past that sub-rect into whatever
    /// material sits next to it in the atlas, not a repeat of this one. Real tiling on a face bigger than
    /// one material repeat needs actual geometry subdivision (see `push_quad_grid`), not a bigger UV value.
    pub fn push_quad_tiled(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, n: Vec3, col: Color32, tile_u: f32, tile_v: f32) {
        let s = self.uv.len();
        self.push_tri(a, b, c, n, col);
        self.push_tri(a, c, d, n, col);
        // map the four corners across the material tile: a=(0,0) b=(tile_u,0) c=(tile_u,tile_v) d=(0,tile_v)
        let q = [
            egui::Pos2::new(0.0, 0.0),
            egui::Pos2::new(tile_u, 0.0),
            egui::Pos2::new(tile_u, tile_v),
            egui::Pos2::new(0.0, 0.0),
            egui::Pos2::new(tile_u, tile_v),
            egui::Pos2::new(0.0, tile_v),
        ];
        for (i, uv) in q.iter().enumerate() {
            if let Some(slot) = self.uv.get_mut(s + i) {
                *slot = *uv;
            }
        }
    }

    /// Builder: texture this mesh with an atlas material (per-pixel surface detail).
    pub fn with_material(mut self, m: Material) -> Self {
        self.material = Some(m);
        self
    }

    /// Merge another mesh's geometry into this one (re-indexing). Used to batch the level into few meshes.
    pub fn merge(&mut self, other: &Mesh3) {
        let off = self.verts.len() as u32;
        self.verts.extend_from_slice(&other.verts);
        self.norms.extend_from_slice(&other.norms);
        // UVs stay parallel to verts; backfill defaults where either side lacks them.
        if self.uv.len() < off as usize {
            self.uv.resize(off as usize, egui::Pos2::ZERO);
        }
        if other.uv.len() == other.verts.len() {
            self.uv.extend_from_slice(&other.uv);
        } else {
            self.uv.extend(std::iter::repeat(egui::Pos2::ZERO).take(other.verts.len()));
        }
        if self.material.is_none() {
            self.material = other.material;
        }
        match (self.vcol.is_some(), other.vcol.is_some()) {
            (true, true) => self.vcol.as_mut().unwrap().extend_from_slice(other.vcol.as_ref().unwrap()),
            (true, false) => self.vcol.as_mut().unwrap().extend(std::iter::repeat(other.color).take(other.verts.len())),
            (false, true) => {
                // Backfill our own flat colour for the existing verts, then take the other's per-vertex colours.
                let mut v = vec![self.color; off as usize];
                v.extend_from_slice(other.vcol.as_ref().unwrap());
                self.vcol = Some(v);
            }
            (false, false) => {}
        }
        let was_empty = self.tris.is_empty();
        for t in &other.tris {
            self.tris.push([t[0] + off, t[1] + off, t[2] + off]);
        }
        // A merged mesh can only be safely backface-culled if BOTH inputs are single-sided (mixing a solid box
        // with double-sided foliage would wrongly cull the foliage). An empty accumulator inherits the
        // incoming flag, so a chunk that accumulates only solids stays cullable.
        self.single_sided = if was_empty {
            other.single_sided
        } else {
            self.single_sided && other.single_sided
        };
        // Sway weights, kept parallel to verts. Treat an empty array as "all rooted" (zeros).
        match (self.sway.is_empty(), other.sway.is_empty()) {
            (false, false) => self.sway.extend_from_slice(&other.sway),
            (false, true) => self.sway.extend(std::iter::repeat(0.0).take(other.verts.len())),
            (true, false) => {
                let mut v = vec![0.0f32; off as usize];
                v.extend_from_slice(&other.sway);
                self.sway = v;
            }
            (true, true) => {}
        }
    }

    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }
}

/// A sun + hemispherical ambient fill (cool sky from above, warm bounce from below) — the
/// "live sunshine" lighting model lifted straight from the reference. `dir` points TOWARD the sun.
#[derive(Clone, Copy)]
pub struct Light {
    pub dir: Vec3,
    pub color: Color32,
    pub sky: Color32,
    pub ground: Color32,
    pub ambient: f32,
}
impl Light {
    pub fn sun(dir: Vec3, color: Color32) -> Self {
        Light {
            dir: dir.normalized(),
            color,
            sky: Color32::from_rgb(140, 168, 205),
            ground: Color32::from_rgb(96, 82, 66),
            ambient: 0.72,
        }
    }
}

/// A dynamic point light — a coloured glow at a world position with a falloff radius and intensity. The
/// renderer adds each one's contribution to nearby vertices (distance falloff + a lambert term against the
/// vertex normal), so a muzzle flash, an explosion, a planted-bomb device, or a flag genuinely light the
/// surrounding geometry. Cheap: it's a per-vertex add, capped to a small handful of the strongest lights.
#[derive(Clone, Copy, Debug)]
pub struct PointLight {
    pub pos: Vec3,
    pub color: Color32,
    /// Peak intensity at the source (0..~2).
    pub intensity: f32,
    /// Distance at which the light fades to nothing.
    pub radius: f32,
}

impl PointLight {
    pub fn new(pos: Vec3, color: Color32, intensity: f32, radius: f32) -> Self {
        Self { pos, color, intensity, radius }
    }
    /// Light contribution (0..intensity) at a world point with a given normal — smooth distance falloff times
    /// a half-lambert term so faces turned toward the light get more but back-faces still catch a little.
    #[inline]
    pub fn contribution(&self, p: Vec3, n: Vec3) -> f32 {
        let d = p - self.pos;
        let dist2 = d.x * d.x + d.y * d.y + d.z * d.z;
        let r2 = self.radius * self.radius;
        if dist2 >= r2 {
            return 0.0;
        }
        let dist = dist2.sqrt().max(1e-3);
        // smooth inverse falloff: (1 - d/r)^2
        let fall = (1.0 - dist / self.radius).clamp(0.0, 1.0);
        let fall = fall * fall;
        // half-lambert: toward-light faces brightest, back-faces dimmed but not black
        let ldir = (self.pos - p) * (1.0 / dist);
        let lambert = (0.5 + 0.5 * n.dot(ldir)).clamp(0.0, 1.0);
        self.intensity * fall * lambert
    }
}

// ── colour helpers ──────────────────────────────────────────────────────────────

#[inline]
pub fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
#[inline]
pub fn lerp_c(a: Color32, b: Color32, f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 * (1.0 - f) + b.r() as f32 * f) as u8,
        (a.g() as f32 * (1.0 - f) + b.g() as f32 * f) as u8,
        (a.b() as f32 * (1.0 - f) + b.b() as f32 * f) as u8,
    )
}

/// Multiply a colour's brightness by `f` (can brighten above 1.0, clamped). Preserves alpha.
pub fn scale_rgb(c: Color32, f: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        (c.r() as f32 * f).clamp(0.0, 255.0) as u8,
        (c.g() as f32 * f).clamp(0.0, 255.0) as u8,
        (c.b() as f32 * f).clamp(0.0, 255.0) as u8,
        c.a(),
    )
}

/// Re-tint a body colour toward a **team hue** while preserving its own brightness and shading detail. This
/// is how a single character model reads as "friendly" or "hostile": we take the source colour's luminance
/// (so darks stay dark, highlights stay bright) and re-colour it with the team hue at that same luminance,
/// then blend by `amount` so the tint is strong but not a flat repaint. Used for enemy/ally body tinting so
/// the player can instantly tell factions apart, with the team colours user-configurable in settings.
pub fn team_tint(src: Color32, team: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    // source luminance 0..1
    let lum = (0.299 * src.r() as f32 + 0.587 * src.g() as f32 + 0.114 * src.b() as f32) / 255.0;
    // team colour's own luminance, to scale it to the source's brightness
    let tl = (0.299 * team.r() as f32 + 0.587 * team.g() as f32 + 0.114 * team.b() as f32).max(1.0) / 255.0;
    let k = lum / tl;
    let tinted = Color32::from_rgb(
        (team.r() as f32 * k).clamp(0.0, 255.0) as u8,
        (team.g() as f32 * k).clamp(0.0, 255.0) as u8,
        (team.b() as f32 * k).clamp(0.0, 255.0) as u8,
    );
    lerp_c(src, tinted, amount)
}

/// Deterministic 0..1 hash — the reference's mixing constants; used to dither vertex colour for that
/// noisy, low-budget "texture" look without any image assets. Now `noise-kit`'s
/// `hash01`, which adopted this function verbatim (golden-pinned there).
pub use noise_kit::hash01;
/// Jitter a base colour by ±`amt` using a coordinate-derived hash — cheap procedural grime.
pub fn dither(base: Color32, key: i32, amt: f32) -> Color32 {
    let d = (hash01(key, 17) - 0.5) * 2.0 * amt;
    let f = |c: u8| (c as f32 * (1.0 + d)).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(base.r()), f(base.g()), f(base.b()))
}

// ── Procedural surface texture ───────────────────────────────────────────────────────────────────────
// A small value-noise / fBm family sampled in WORLD space. Coincident tile-corner vertices share a world
// position, so the field is continuous across tiles even though the mesh isn't welded — the grain reads as
// one seamless material, not per-tile patches. This family was adapted from logscope texgen.rs and has
// since moved to `noise-kit`, which adopted BOTH copies bit-identically (same lattice constants;
// golden-pinned in the kit's tests): `fbm2` here is the kit's un-tiled world-space variant.
pub use noise_kit::fbm2;

/// A surface **material** — a distinct procedural treatment applied to a mesh's albedo. Biomes assign these to
/// floors and walls so the world's surfaces have variety instead of one shared mottle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Material {
    Concrete,
    Brick,
    Corrugated,
    StoneBlock,
    Cobble,
    Gravel,
    Grass,
    Sand,
    Snow,
    RustPlate,
    Tile,
    Planks,
    Dirt,
    Organic,
    Plaster,
    Cloth,
    /// Polished veined marble — for landmark floors, hotel lobbies, monuments. A pale stone with dark
    /// meandering veins and a glossy specular response.
    Marble,
    /// Woven/bundled thatch or dry reed — palm-roof huts, tiki structures, desert awnings. Warm straw with
    /// a strong directional weave and frayed ends.
    Thatch,
    /// Grey weathered dock/scaffold wood — salt-bleached planking with deep grain, splits, and dark knots.
    WeatheredWood,
    /// Industrial diamond/hex tread plate — a raised metal grip pattern, dark with bright edge highlights.
    HexPlate,
    /// Soot-blackened, oil-stained concrete — parking-garage decks and fire-scorched slabs. Grey base
    /// mottled dark with heavy irregular soot streaks and oil pooling.
    ScorchedConcrete,
    /// Automotive body paint — a saturated, glossy enamel with a strong specular sheen and clear-coat
    /// reflection. Used for car bodies (colour comes from the mesh; the material carries the gloss).
    CarPaint,
    /// Tree bark — deep vertical furrows and ridges in a warm red-brown, for redwood/conifer trunks and log
    /// bridges. Matte, strongly directional grain.
    Bark,
    /// Moss / mossy forest floor — a soft mottled green with darker damp patches and a fine organic speckle.
    /// Matte. For forest ground, damp stone, and shaded north faces.
    Moss,
}

#[inline]
fn fr(v: f32) -> f32 {
    v - v.floor()
}

#[inline]
fn ihash(xi: i32, zi: i32, s: u32) -> f32 {
    let mut h = (xi as u32).wrapping_mul(0x9E37_79B1) ^ (zi as u32).wrapping_mul(0x85EB_CA77) ^ s.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 13;
    (h & 0x00FF_FFFF) as f32 / 0x00FF_FFFF as f32
}

/// Masonry grid: dark mortar lines plus a per-unit tone jitter. `bond` shifts alternate rows (0.5 = running
/// bond/brick, 0 = stacked/tile); `mortar` is line thickness.
fn p_block(x: f32, z: f32, scale: f32, bond: f32, mortar: f32, seed: u32) -> f32 {
    let v = z * scale;
    let row = v.floor() as i32;
    let boff = if row.rem_euclid(2) == 0 { 0.0 } else { bond };
    let u = x * scale + boff;
    let col = u.floor() as i32;
    let (fu, fv) = (fr(u), fr(v));
    if fu < mortar || fu > 1.0 - mortar || fv < mortar || fv > 1.0 - mortar {
        -0.85
    } else {
        (ihash(col, row, seed) - 0.5) * 0.5
    }
}

fn p_ribs(x: f32, scale: f32) -> f32 {
    (x * scale * std::f32::consts::TAU).sin()
}

fn p_speckle(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    (ihash((x * scale) as i32, (z * scale) as i32, seed) - 0.5) * 2.0
}

fn p_streak(x: f32, y: f32, seed: u32) -> f32 {
    let col = ihash((x * 1.6) as i32, 0, seed);
    let drip = ((y * 1.7 + col * 12.0).sin() * 0.5 + 0.5) * col;
    -drip * 0.8
}

fn p_planks(x: f32, z: f32, scale: f32, seed: u32) -> f32 {
    let v = z * scale;
    let plank = v.floor() as i32;
    let fv = fr(v);
    let groove = if fv < 0.07 || fv > 0.93 { -0.7 } else { 0.0 };
    groove + (ihash(0, plank, seed) - 0.5) * 0.35 + (x * scale * 5.0).sin() * 0.05
}

/// Albedo multiplier-delta for a material at a world point (≈[-1,1], applied as 1+delta).
fn material_mod(mat: Material, x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let base = fbm2(x * 0.5 + 3.0, z * 0.5 - 5.0, 4, seed) - 0.5;
    match mat {
        Material::Concrete => base * 0.16 + p_speckle(x, z, 8.0, seed) * 0.03,
        Material::Brick => p_block(x, z, 0.7, 0.5, 0.10, seed) * 0.24 + base * 0.05,
        Material::Corrugated => p_ribs(x, 0.9) * 0.15 + p_ribs(z, 0.9) * 0.04,
        Material::StoneBlock => p_block(x, z, 1.2, 0.0, 0.10, seed) * 0.26 + base * 0.06,
        Material::Cobble => p_block(x, z, 0.85, 0.45, 0.16, seed) * 0.2 + p_speckle(x, z, 6.0, seed) * 0.05,
        Material::Gravel => p_speckle(x, z, 7.0, seed) * 0.17 + base * 0.05,
        Material::Grass => p_speckle(x, z, 9.0, seed ^ 0x7) * 0.1 + base * 0.13,
        Material::Sand => {
            // Real sand reads at three scales: soft dune shadow/highlight bands (large, slow), directional
            // wind-ripple lines (medium), and fine grain speckle (small). The old pattern here was almost
            // flat (~0.10 total amplitude) — by far the weakest of any material — which is why every desert
            // surface leaned on it read noticeably blander than the rest of the game.
            let dunes = (fbm2(x * 0.15, z * 0.15, 3, seed) - 0.5) * 0.22;
            let ripple = (x * 1.8 + (z * 0.4).sin() * 2.0).sin() * 0.10;
            let grain = p_speckle(x, z, 14.0, seed) * 0.09;
            dunes + ripple + grain
        }
        Material::Snow => {
            let drift = (fbm2(x * 3.5, z * 3.5, 3, seed) - 0.5) * 0.045;
            let sparkle = if p_speckle(x, z, 42.0, seed ^ 0x9) > 0.88 { 0.55 } else { 0.0 }; // sun glints off crystals
            drift + sparkle
        }
        Material::RustPlate => p_streak(x, y, seed) * 0.22 + base * 0.1 + p_block(x, z, 0.18, 0.0, 0.05, seed) * 0.06,
        Material::Tile => p_block(x, z, 0.5, 0.0, 0.06, seed) * 0.14,
        Material::Planks => p_planks(x, z, 0.45, seed) * 0.17,
        Material::Dirt => (fbm2(x * 6.0, z * 6.0, 4, seed) - 0.5) * 0.16 + p_speckle(x, z, 26.0, seed) * 0.03 + base * 0.05,
        Material::Organic => (fbm2(x * 0.9, z * 0.9, 5, seed) - 0.5) * 0.28,
        Material::Plaster => {
            // Hand-troweled stucco: broad uneven fbm undulation plus a subtler block-ish mottle. Was the
            // second-weakest pattern in the set (~0.19) and Desert Town's buildings are almost entirely
            // this material — boosted to be in line with StoneBlock/Brick instead of reading flat.
            (fbm2(x * 1.4, z * 1.4, 4, seed) - 0.5) * 0.24 + p_block(x, z, 0.24, 0.0, 0.04, seed) * 0.09
        }
        Material::Cloth => p_ribs(x, 7.0) * 0.05 + p_ribs(z, 7.0) * 0.05 + (fbm2(x * 0.5, z * 0.5, 3, seed) - 0.5) * 0.16,
        Material::Marble => {
            // Polished marble: a smooth pale field cut by dark meandering veins. The veins are thin ridges of
            // a warped fbm thresholded near a contour — turbulent domain-warping gives them the characteristic
            // branching wander instead of straight lines.
            let warp = (fbm2(x * 0.6, z * 0.6, 4, seed) - 0.5) * 2.2;
            let field = fbm2(x * 0.5 + warp, z * 0.5 - warp, 5, seed ^ 0x3);
            let vein = (1.0 - ((field - 0.5).abs() * 9.0).min(1.0)) * -0.5; // dark where field ≈ 0.5
            let base_mottle = (fbm2(x * 0.25, z * 0.25, 2, seed) - 0.5) * 0.06;
            vein + base_mottle
        }
        Material::Thatch => {
            // Bundled reed: a strong directional weave (long fibres running along X) plus per-bundle tone and
            // frayed speckle. Reads as dry straw laid in courses.
            let fibre = (z * 8.0 * std::f32::consts::TAU).sin() * 0.06;
            let bundle = p_block(x, z, 0.9, 0.0, 0.05, seed) * 0.14;
            let fray = p_speckle(x, z, 16.0, seed) * 0.08;
            fibre + bundle + fray
        }
        Material::WeatheredWood => {
            // Salt-bleached planking: deep straight grain along the board, dark knots, and split-open cracks.
            let grain = (x * 9.0 + (z * 2.0).sin() * 1.5).sin() * 0.10;
            let plank = p_planks(x, z, 0.5, seed) * 0.16;
            let knot = {
                let kh = ihash((x * 0.9) as i32, (z * 0.9) as i32, seed ^ 0x9);
                if kh > 0.94 { -0.4 } else { 0.0 } // sparse dark knots
            };
            grain + plank + knot
        }
        Material::HexPlate => {
            // Diamond tread plate: a raised diamond grid (via the block grid) with a bright top facet and dark
            // recess, plus a faint brushed-metal streak.
            let tread = p_block(x, z, 1.6, 0.5, 0.16, seed) * 0.3;
            let brush = (x * 20.0).sin() * 0.02;
            tread + brush
        }
        Material::ScorchedConcrete => {
            // Grey deck heavily darkened by irregular soot clouds and oil pooling: two octaves of low-freq
            // fbm drag it toward black in blotches, plus a fine speckle so it isn't a smooth gradient.
            let soot = (fbm2(x * 0.4, z * 0.4, 4, seed) - 0.65) * 0.55; // biased dark
            let oil = (fbm2(x * 1.1 + 20.0, z * 1.1, 3, seed ^ 0x3) - 0.7).min(0.0) * 0.4; // dark oil pools
            let grit = p_speckle(x, z, 9.0, seed) * 0.05;
            soot + oil + grit
        }
        Material::CarPaint => {
            // Smooth enamel: almost flat, just a faint large-scale panel undulation and a whisper of orange-
            // peel micro-texture so the clear-coat highlight isn't mirror-perfect.
            let panel = (fbm2(x * 0.6, z * 0.6, 2, seed) - 0.5) * 0.04;
            let peel = p_speckle(x, z, 26.0, seed) * 0.015;
            panel + peel
        }
        Material::Bark => {
            // Deep vertical furrows: strong ribbing along the trunk axis (y here maps to the vertical run),
            // broken up by fbm so ridges wander like real bark rather than a corduroy pattern.
            let furrow = p_ribs(z, 3.2) * 0.28;
            let wander = (fbm2(x * 1.4, z * 0.5, 3, seed) - 0.5) * 0.18;
            let grit = p_speckle(x, z, 11.0, seed) * 0.06;
            furrow + wander + grit
        }
        Material::Moss => {
            // Soft mottled clumps: low-frequency fbm patches (denser/darker damp spots) plus a fine organic
            // speckle so it reads as a living surface, not flat green paint.
            let clump = (fbm2(x * 0.8, z * 0.8, 3, seed) - 0.5) * 0.3;
            let damp = (fbm2(x * 2.2 + 10.0, z * 2.2, 2, seed ^ 0x5) - 0.6).min(0.0) * 0.25;
            let speck = p_speckle(x, z, 14.0, seed) * 0.08;
            clump + damp + speck
        }
    }
}

/// All materials in atlas order. The renderer maps a mesh's material to its tile here.
pub const ATLAS_MATERIALS: [Material; 24] = [
    Material::Concrete,
    Material::Brick,
    Material::Corrugated,
    Material::StoneBlock,
    Material::Cobble,
    Material::Gravel,
    Material::Grass,
    Material::Sand,
    Material::Snow,
    Material::RustPlate,
    Material::Tile,
    Material::Planks,
    Material::Dirt,
    Material::Organic,
    Material::Plaster,
    Material::Cloth,
    Material::Marble,
    Material::Thatch,
    Material::WeatheredWood,
    Material::HexPlate,
    Material::ScorchedConcrete,
    Material::CarPaint,
    Material::Bark,
    Material::Moss,
];

/// Base albedo for a material — the dominant colour the pattern modulates.
pub fn material_base(mat: Material) -> Color32 {
    match mat {
        Material::Concrete => Color32::from_rgb(150, 150, 154),
        Material::Brick => Color32::from_rgb(150, 84, 66),
        Material::Corrugated => Color32::from_rgb(140, 144, 150),
        Material::StoneBlock => Color32::from_rgb(140, 134, 122),
        Material::Cobble => Color32::from_rgb(128, 124, 118),
        Material::Gravel => Color32::from_rgb(132, 126, 116),
        Material::Grass => Color32::from_rgb(86, 132, 70),
        Material::Sand => Color32::from_rgb(206, 184, 138),
        Material::Snow => Color32::from_rgb(250, 252, 255),
        Material::RustPlate => Color32::from_rgb(150, 96, 70),
        Material::Tile => Color32::from_rgb(170, 172, 176),
        Material::Planks => Color32::from_rgb(146, 104, 62),
        Material::Dirt => Color32::from_rgb(120, 98, 74),
        Material::Organic => Color32::from_rgb(120, 150, 96),
        Material::Plaster => Color32::from_rgb(196, 162, 120),
        Material::Cloth => Color32::from_rgb(168, 96, 84),
        Material::Marble => Color32::from_rgb(224, 222, 216),
        Material::Thatch => Color32::from_rgb(178, 146, 84),
        Material::WeatheredWood => Color32::from_rgb(138, 132, 120),
        Material::HexPlate => Color32::from_rgb(96, 100, 106),
        Material::ScorchedConcrete => Color32::from_rgb(92, 90, 92),
        Material::CarPaint => Color32::from_rgb(150, 150, 155),
        Material::Bark => Color32::from_rgb(122, 78, 52),
        Material::Moss => Color32::from_rgb(78, 104, 58),
    }
}

/// A material's **secondary colour** — the mortar/grout/seam tone painted into the recessed grid lines of
/// masonry-style materials (or the shadowed groove of planks). Returning a genuinely *different hue* here
/// (a pale grey mortar between red bricks, dark grout between light tiles) is the single biggest jump from
/// "a red splotch with noise" to "individually laid bricks" — a flat scalar multiply of one colour can
/// never produce it. `None` means the material has no distinct seam colour (it's a continuous surface).
fn material_seam(mat: Material) -> Option<Color32> {
    match mat {
        Material::Brick => Some(rgb(196, 188, 170)),      // pale lime mortar
        Material::StoneBlock => Some(rgb(96, 90, 80)),     // dark set joint
        Material::Cobble => Some(rgb(74, 70, 64)),         // earth packed between stones
        Material::Tile => Some(rgb(70, 72, 78)),           // dark grout
        Material::Planks => Some(rgb(48, 34, 22)),         // shadowed board gap
        Material::Corrugated => Some(rgb(96, 100, 108)),   // shadow trough of the rib
        Material::RustPlate => Some(rgb(70, 44, 30)),      // deep rust in the panel seam
        Material::WeatheredWood => Some(rgb(70, 66, 60)),  // dark salt-split board gap
        Material::HexPlate => Some(rgb(52, 55, 60)),       // shadow between raised tread diamonds
        _ => None,
    }
}

/// Per-cell hue jitter magnitude (0 = every brick/stone identical, higher = more colour spread across the
/// individual units). Real masonry is never one flat colour — bricks fire browner/redder/greyer, stones
/// pull warm and cool. This is what stops a wall reading as a single uniform swatch.
fn material_cell_jitter(mat: Material) -> f32 {
    match mat {
        Material::Brick => 0.16,
        Material::StoneBlock => 0.14,
        Material::Cobble => 0.18,
        Material::Tile => 0.05,
        Material::Planks => 0.20,
        Material::RustPlate => 0.22,
        Material::WeatheredWood => 0.22,
        Material::Thatch => 0.18,
        Material::HexPlate => 0.06,
        _ => 0.0,
    }
}

/// The masonry grid a material lays down: `(scale, bond, mortar_thickness)` matching its `material_mod`
/// pattern, or `None` for materials with no grid. Used by the two-colour atlas bake so the seam colour lands
/// exactly on the recessed lines the height-field bevel also uses.
fn material_grid(mat: Material) -> Option<(f32, f32, f32, f32)> {
    // (cell frequency, running-bond offset, mortar width, vertical aspect). Higher frequency = smaller units.
    // The atlas tile spans ~2.6 m and is drawn over ~1.9 m of wall, so a unit's real size ≈ 1.9 / (2.6·freq).
    match mat {
        Material::Brick => Some((3.3, 0.5, 0.12, 2.9)),  // ~0.22 m × 0.075 m running-bond brick (was ~1 m squares)
        Material::StoneBlock => Some((1.2, 0.0, 0.10, 1.0)),
        Material::Cobble => Some((0.85, 0.45, 0.16, 1.0)),
        Material::Tile => Some((0.5, 0.0, 0.06, 1.0)),
        Material::HexPlate => Some((1.6, 0.5, 0.16, 1.0)),
        _ => None,
    }
}

/// Which masonry cell (column,row) a world point falls in, and its local fractional position — mirrors
/// `p_block`'s addressing so per-cell colour lines up with the pattern.
fn cell_of(x: f32, z: f32, scale: f32, bond: f32, aspect: f32) -> (i32, i32, f32, f32) {
    let v = z * scale * aspect;
    let row = v.floor() as i32;
    let boff = if row.rem_euclid(2) == 0 { 0.0 } else { bond };
    let u = x * scale + boff;
    let col = u.floor() as i32;
    (col, row, fr(u), fr(v))
}

/// Jitter a colour's hue/value per-cell: shifts warm↔cool and light↔dark by a hash of the cell id, so each
/// masonry unit gets its own tone. `amt` is the spread; `h` is the cell's 0..1 hash.
fn jitter_cell(base: Color32, amt: f32, h: f32, h2: f32) -> Color32 {
    // h drives value (light/dark), h2 drives warm/cool tilt.
    let val = 1.0 + (h - 0.5) * 2.0 * amt;
    let warm = (h2 - 0.5) * 2.0 * amt * 0.6;
    let r = (base.r() as f32 * val * (1.0 + warm)).clamp(0.0, 255.0) as u8;
    let g = (base.g() as f32 * val).clamp(0.0, 255.0) as u8;
    let b = (base.b() as f32 * val * (1.0 - warm)).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(r, g, b)
}

/// PBR-lite response for a material: `(spec_power, spec_intensity, reflect)`.
/// - `spec_power`: highlight tightness — low (rough, broad glow) to high (glossy, tight glint).
/// - `spec_intensity`: how bright that highlight gets.
/// - `reflect`: environment reflection amount (0 = none, drives the renderer's Schlick-fresnel sky
///   blend + mirror glint — the same mechanism glass/ice/water already use, now driven per-material).
///
/// This is what makes rusted corrugated metal shine differently from packed dirt under the exact same sun:
/// metals get a tight bright glint and real reflectivity, polished tile is glossy, and rough matte surfaces
/// (cloth, dirt, grass, sand) barely respond to specular light at all — the core of a roughness/metalness
/// workflow, expressed as three numbers a per-vertex Blinn-Phong-ish shader can actually use.
pub fn material_pbr(mat: Material) -> (f32, f32, f32) {
    match mat {
        Material::Concrete => (8.0, 0.14, 0.02),
        Material::Brick => (6.0, 0.10, 0.0),
        Material::Corrugated => (46.0, 0.55, 0.32),  // galvanized/rusted sheet metal — tight glint, real reflect
        Material::StoneBlock => (10.0, 0.16, 0.03),
        Material::Cobble => (8.0, 0.14, 0.02),
        Material::Gravel => (5.0, 0.08, 0.0),
        Material::Grass => (4.0, 0.04, 0.0),
        Material::Sand => (5.0, 0.07, 0.0),
        Material::Snow => (22.0, 0.34, 0.09),          // sun-glare sparkle
        Material::RustPlate => (18.0, 0.28, 0.14),      // metal, but rough with rust — softer than clean steel
        Material::Tile => (60.0, 0.6, 0.26),            // polished/glazed — the glossiest matte material
        Material::Planks => (10.0, 0.14, 0.02),
        Material::Dirt => (4.0, 0.04, 0.0),
        Material::Organic => (6.0, 0.07, 0.0),
        Material::Plaster => (8.0, 0.11, 0.02),
        Material::Cloth => (3.0, 0.03, 0.0),
        Material::Marble => (70.0, 0.62, 0.30),         // polished stone — glossiest of the stones, real sheen
        Material::Thatch => (3.0, 0.03, 0.0),            // dry reed — dead matte
        Material::WeatheredWood => (5.0, 0.06, 0.0),     // salt-bleached, no shine left
        Material::HexPlate => (40.0, 0.5, 0.24),         // oiled tread steel — bright tight glint
        Material::ScorchedConcrete => (6.0, 0.10, 0.02),  // matte, soot-dulled
        Material::CarPaint => (85.0, 0.7, 0.42),          // enamel + clear coat: tight bright glint, real reflect
        Material::Bark => (4.0, 0.05, 0.0),               // rough dry bark — dead matte
        Material::Moss => (3.0, 0.03, 0.0),               // soft moss — no shine
    }
}

/// Full per-pixel material colour at tile coordinate (u,v) in [0,1], tiling seamlessly. This is the crisp,
/// image-resolution version of the per-vertex `material_mod` — it's what the atlas bakes.
pub fn material_pixel(mat: Material, u: f32, v: f32) -> Color32 {
    let span = 2.6; // metres represented per atlas tile (a few pattern repeats)
    let (x, z) = (u * span, v * span);

    // ── Masonry path: per-cell albedo + a genuinely different seam colour ────────────────────────────────
    // For brick/stone/cobble/tile, paint each unit its own jittered hue and lay the mortar/grout colour into
    // the recessed grid lines. This is the real material read a flat scalar multiply can't produce.
    let mut base = material_base(mat);
    let mut on_seam = false;
    if let (Some((scale, bond, mortar, aspect)), Some(seam)) = (material_grid(mat), material_seam(mat)) {
        let (col, row, fu, fv) = cell_of(x, z, scale, bond, aspect);
        on_seam = fu < mortar || fu > 1.0 - mortar || fv < mortar || fv > 1.0 - mortar;
        if on_seam {
            // seam colour, with a touch of its own grime so it isn't a dead-flat line
            let grime = (fbm2(x * 3.0, z * 3.0, 2, 771) - 0.5) * 0.14;
            base = jitter_cell(seam, 0.06, fr(x * 4.0), grime + 0.5);
        } else {
            // this unit's own tone
            let h = ihash(col, row, 0x5A17);
            let h2 = ihash(col, row, 0x9F31);
            base = jitter_cell(base, material_cell_jitter(mat), h, h2);
        }
        // Brick weathering, wired from the brick_family study. Only touches Material::Brick and stays subtle so
        // the mortar-vs-face hue contrast the masonry test locks in is preserved: flashed (over-fired) bricks,
        // moss + efflorescence wicking into the damp joints, and faint soot mottling on the faces.
        if mat == Material::Brick {
            let bloom = fbm2(x * 6.0, z * 6.0, 3, 313);
            if on_seam {
                let moss = ((fbm2(x * 8.0, z * 8.0, 3, 91) - 0.52).max(0.0)) * 1.7;
                base = lerp_c(base, rgb(78, 106, 58), moss.clamp(0.0, 0.5));
                let efflor = ((bloom - 0.62).max(0.0)) * 1.2;
                base = lerp_c(base, rgb(220, 220, 224), efflor.clamp(0.0, 0.35));
            } else {
                if ihash(col, row, 0x1234) > 0.82 {
                    base = lerp_c(base, rgb(96, 52, 44), 0.55); // flashed brick
                }
                let soot = ((bloom - 0.72).max(0.0)) * 0.9;
                base = lerp_c(base, rgb(66, 58, 52), soot.clamp(0.0, 0.3));
            }
        }
    } else if material_cell_jitter(mat) > 0.0 {
        // planks / rust: linear cells rather than a full grid — still want per-board tone
        let (scale, seed) = match mat {
            Material::Planks => (0.45, 0x11u32),
            Material::WeatheredWood => (0.5, 0x33),
            Material::Thatch => (1.1, 0x44), // tighter bundles
            _ => (0.3, 0x22),
        };
        let plank = (z * scale).floor() as i32;
        let h = ihash(0, plank, seed);
        let h2 = ihash(plank, 0, seed ^ 0x7);
        base = jitter_cell(base, material_cell_jitter(mat), h, h2);
    }

    // pattern strength a touch stronger than the per-vertex use, since this is a real texture. On seam pixels
    // the recess colour is already baked into `base`, so we damp the pattern multiply hard (it would just
    // crush the pale mortar to black) and let the bevel below supply the recessed shading instead.
    let m = material_mod(mat, x, 0.0, z, 90210) * 1.45;
    let mul = if on_seam {
        (1.0 + m * 0.15).clamp(0.82, 1.1) // keep mortar near its true colour
    } else {
        (1.0 + m).clamp(0.4, 1.7)
    };
    let mut r = (base.r() as f32 * mul).clamp(0.0, 255.0) as u8;
    let mut g = (base.g() as f32 * mul).clamp(0.0, 255.0) as u8;
    let mut b = (base.b() as f32 * mul).clamp(0.0, 255.0) as u8;
    // a faint large-scale stain so big surfaces aren't uniform
    let stain = (fbm2(x * 0.7, z * 0.7, 3, 7) - 0.5) * 0.12;
    let sm = (1.0 + stain).clamp(0.85, 1.15);
    r = (r as f32 * sm) as u8;
    g = (g as f32 * sm) as u8;
    b = (b as f32 * sm) as u8;
    // Fine micro-grain: a much higher-frequency speckle than the base pattern — the kind of per-aggregate,
    // per-fibre noise (concrete aggregate, brick sand grain, cloth weave) that aliases into mush at low atlas
    // resolution but reads as genuine surface grain once the atlas has the texel density to resolve it.
    let grain_scale = 22.0; // cycles per tile — well above the base pattern's frequency
    let grain = (ihash((x * grain_scale) as i32, (z * grain_scale) as i32, 0xA17E) - 0.5) * 0.10;
    let gm = (1.0 + grain).clamp(0.9, 1.1);
    r = (r as f32 * gm).clamp(0.0, 255.0) as u8;
    g = (g as f32 * gm).clamp(0.0, 255.0) as u8;
    b = (b as f32 * gm).clamp(0.0, 255.0) as u8;
    // Baked bevel: treat the pattern itself as a height field (grout/grooves recessed, faces raised) and
    // emboss a fixed upper-left light direction into it via a finite-difference gradient. This is what turns
    // "a brick-colored splotch" into something that reads as individually laid bricks — the grout line gets
    // a genuine dark-then-light edge transition instead of a flat colour step, which is what a bump/normal
    // map buys a real texture and a flat multiply-by-pattern texture doesn't. Sized for the atlas's actual
    // texel density (512px/tile) so the transition stays crisp instead of blurring across several texels.
    let eps = (span / 512.0).max(0.001);
    let m_x1 = material_mod(mat, x + eps, 0.0, z, 90210);
    let m_z1 = material_mod(mat, x, 0.0, z + eps, 90210);
    let dx = (m_x1 - material_mod(mat, x - eps, 0.0, z, 90210)) / (2.0 * eps);
    let dz = (m_z1 - material_mod(mat, x, 0.0, z - eps, 90210)) / (2.0 * eps);
    let bevel = ((dx * 0.6 - dz * 0.8) * 0.010).clamp(-0.22, 0.22);
    let bevel_mul = (1.0 + bevel).clamp(0.72, 1.28);
    r = ((r as f32) * bevel_mul).clamp(0.0, 255.0) as u8;
    g = ((g as f32) * bevel_mul).clamp(0.0, 255.0) as u8;
    b = ((b as f32) * bevel_mul).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(r, g, b)
}

/// Build the material atlas as RGBA bytes: a `cols`×`cols` grid of `tile`-px material swatches (the first
/// tile is pure white so untextured geometry — water, glass, particles, glow — can sample a flat texel and
/// keep its vertex colour). Returns (rgba, width, height). One upload, one draw call, crisp per-pixel texture.
pub fn build_material_atlas(tile: usize, cols: usize) -> (Vec<u8>, usize, usize) {
    let size = tile * cols;
    let mut img = vec![255u8; size * size * 4];
    let put = |img: &mut [u8], px: usize, py: usize, c: Color32| {
        let i = (py * size + px) * 4;
        img[i] = c.r();
        img[i + 1] = c.g();
        img[i + 2] = c.b();
        img[i + 3] = 255;
    };
    for (idx, &mat) in ATLAS_MATERIALS.iter().enumerate() {
        let slot = idx + 1; // slot 0 reserved as the white tile
        if slot >= cols * cols {
            break;
        }
        let (tx, ty) = (slot % cols, slot / cols);
        for py in 0..tile {
            for px in 0..tile {
                let u = px as f32 / tile as f32;
                let v = py as f32 / tile as f32;
                put(&mut img, tx * tile + px, ty * tile + py, material_pixel(mat, u, v));
            }
        }
    }
    // slot 0 white tile is already 255 from the fill
    (img, size, size)
}

/// Standard atlas column count used across the renderers (a cols×cols grid of material tiles).
pub const ATLAS_COLS: usize = 5;

/// The atlas slot index for a material (0 = white tile). Mirrors `atlas_rect`'s slot logic; used by the GPU
/// renderer, which computes the cell in-shader from the slot.
pub fn atlas_slot(mat: Material) -> usize {
    ATLAS_MATERIALS.iter().position(|&m| m == mat).map(|i| i + 1).unwrap_or(0)
}

/// The atlas UV sub-rect (u0,v0,u1,v1) for a material in a `cols`×`cols` atlas. A small inset avoids bleeding
/// into neighbouring tiles under bilinear sampling.
pub fn atlas_rect(mat: Material, cols: usize) -> (f32, f32, f32, f32) {
    let slot = ATLAS_MATERIALS.iter().position(|&m| m == mat).map(|i| i + 1).unwrap_or(0);
    let (tx, ty) = (slot % cols, slot / cols);
    let s = 1.0 / cols as f32;
    // Bilinear sampling blends in neighbouring texels, so a UV right at a tile's edge can pick up a sliver
    // of the *next* material's tile. A ~1.5-texel inset (at the atlas's actual 512px/tile resolution) keeps
    // every sample comfortably inside this material's own tile.
    let inset = 1.5 / (cols as f32 * 512.0);
    (tx as f32 * s + inset, ty as f32 * s + inset, (tx as f32 + 1.0) * s - inset, (ty as f32 + 1.0) * s - inset)
}

/// The white-tile UV rect (slot 0) for untextured geometry that wants to keep its vertex colour.
pub fn atlas_white(cols: usize) -> (f32, f32) {
    let s = 1.0 / cols as f32;
    (s * 0.5, s * 0.5)
}

impl Mesh3 {
    /// Procedural surface texture: modulate each vertex's albedo by a world-space fBm field plus a finer
    /// second octave, so big flat surfaces gain material grain/mottling that the renderer interpolates
    /// smoothly across faces. `scale` is the base frequency (per world unit), `strength` the lightness swing
    /// (≈0.08–0.18 looks like worn concrete/stone). No-op if the mesh has no per-vertex colour buffer.
    pub fn texturize(&mut self, scale: f32, strength: f32, seed: u32) {
        let off = self.offset;
        let Some(vc) = self.vcol.as_mut() else { return };
        for (i, v) in self.verts.iter().enumerate() {
            let p = *v + off;
            let coarse = fbm2(p.x * scale + 13.0, p.z * scale - 7.0, 4, seed);
            let fine = fbm2(p.x * scale * 3.1 - 4.0, p.z * scale * 3.1 + 9.0, 2, seed ^ 0x51);
            let f = ((coarse - 0.5) * 1.6 + (fine - 0.5) * 0.8) * strength;
            let mul = (1.0 + f).clamp(0.5, 1.5);
            let c = vc[i];
            vc[i] = Color32::from_rgb(
                (c.r() as f32 * mul).clamp(0.0, 255.0) as u8,
                (c.g() as f32 * mul).clamp(0.0, 255.0) as u8,
                (c.b() as f32 * mul).clamp(0.0, 255.0) as u8,
            );
        }
    }

    /// Apply a **material** to the surface — a distinct procedural treatment (brick courses, corrugation,
    /// stone blocks, rust streaks, gravel grain, plank grooves…) layered on the per-vertex albedo. This is
    /// what makes a Bunker wall read as poured concrete while a Warehouse wall reads as ribbed steel and an
    /// Urban wall reads as brick — instead of every surface sharing one fBm mottle. World-space, so the pattern
    /// is continuous across tiles.
    pub fn apply_material(&mut self, mat: Material, seed: u32) {
        let off = self.offset;
        let Some(vc) = self.vcol.as_mut() else { return };
        for (i, v) in self.verts.iter().enumerate() {
            let p = *v + off;
            let mul = (1.0 + material_mod(mat, p.x, p.y, p.z, seed)).clamp(0.5, 1.5);
            let c = vc[i];
            vc[i] = Color32::from_rgb(
                (c.r() as f32 * mul).clamp(0.0, 255.0) as u8,
                (c.g() as f32 * mul).clamp(0.0, 255.0) as u8,
                (c.b() as f32 * mul).clamp(0.0, 255.0) as u8,
            );
        }
    }
}


// ── primitives ───────────────────────────────────────────────────────────────────

/// An axis-aligned box centred at `c` with half-extents `h`. `faces` masks which of the 6 faces are
/// emitted (so a room floor can skip its underside). Outward normals.
pub fn boxx(c: Vec3, h: Vec3, color: Color32, faces: u8) -> Mesh3 {
    let mut m = Mesh3::new(color);
    let p = |sx: f32, sy: f32, sz: f32| Vec3::new(c.x + sx * h.x, c.y + sy * h.y, c.z + sz * h.z);
    // (mask bit, normal, 4 corners CCW seen from outside, (face width, face height) for tiling subdivision)
    let f: [(u8, Vec3, [Vec3; 4], (f32, f32)); 6] = [
        (0x01, Vec3::new(0.0, 1.0, 0.0), [p(-1.0, 1.0, -1.0), p(-1.0, 1.0, 1.0), p(1.0, 1.0, 1.0), p(1.0, 1.0, -1.0)], (h.z, h.x)), // top
        (0x02, Vec3::new(0.0, -1.0, 0.0), [p(-1.0, -1.0, 1.0), p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0), p(1.0, -1.0, 1.0)], (h.z, h.x)), // bottom
        (0x04, Vec3::new(0.0, 0.0, 1.0), [p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)], (h.x, h.y)), // +z
        (0x08, Vec3::new(0.0, 0.0, -1.0), [p(1.0, -1.0, -1.0), p(-1.0, -1.0, -1.0), p(-1.0, 1.0, -1.0), p(1.0, 1.0, -1.0)], (h.x, h.y)), // -z
        (0x10, Vec3::new(1.0, 0.0, 0.0), [p(1.0, -1.0, 1.0), p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0), p(1.0, 1.0, 1.0)], (h.z, h.y)), // +x
        (0x20, Vec3::new(-1.0, 0.0, 0.0), [p(-1.0, -1.0, -1.0), p(-1.0, -1.0, 1.0), p(-1.0, 1.0, 1.0), p(-1.0, 1.0, -1.0)], (h.z, h.y)), // -x
    ];
    // egui has no fragment-level wrap-around sampling within an atlas sub-rect (a UV value past 1.0 would
    // sample into the *next* material's tile, not repeat this one), so true tiling on a large face has to be
    // real geometric subdivision — one grid cell per material repeat, each independently UV-mapped 0..1 into
    // this face's own tile. That's also exactly what gives the PBR shading more vertices to vary across
    // instead of Gouraud-washing one flat gradient over the whole face.
    const UV_TILE_SPAN: f32 = 1.9;
    for (bit, n, q, (fw, fh)) in f {
        if faces & bit != 0 {
            let key = (c.x as i32) * 73 + (c.z as i32) * 91 + (n.y as i32) * 7 + (n.x as i32) * 13;
            let (nu, nv) = (((fw * 2.0 / UV_TILE_SPAN).round() as i32).max(1), ((fh * 2.0 / UV_TILE_SPAN).round() as i32).max(1));
            if nu == 1 && nv == 1 {
                m.push_quad(q[0], q[1], q[2], q[3], n, dither(color, key, 0.06));
            } else {
                push_quad_grid(&mut m, q[0], q[1], q[2], q[3], n, dither(color, key, 0.06), nu.min(5), nv.min(5), key);
            }
        }
    }
    m.single_sided = true; // closed box with correct outward winding — safe to backface-cull
    m
}
pub const FACE_ALL: u8 = 0x3F;
pub const FACE_TOP: u8 = 0x01;
pub const FACE_NO_BOTTOM: u8 = 0x3F & !0x02;

/// A flat rectangular panel centred at `c`, spanning `right`×`up` with size `w`×`h`. Used for big
/// floor/ceiling slabs and signage. Two-sided look comes from the renderer's ambient term.
/// Push a quad as an `nu`×`nv` grid of sub-quads instead of one — bilinearly interpolating the four corners
/// (`bl`,`br`,`tr`,`tl`, matching `push_quad`'s winding) and dithering each cell a touch. A single quad only
/// has its shading evaluated at 4 corners and Gouraud-interpolates between them, so on a big flat wall/floor
/// panel any per-pixel-looking effect (specular glint, fresnel rim, PBR reflection) washes out into one flat
/// linear gradient. Subdividing gives the renderer more points to actually evaluate those terms at, which is
/// most of the gap between "flat-shaded plastic" and something that reads like it has real material response.
pub fn push_quad_grid(m: &mut Mesh3, bl: Vec3, br: Vec3, tr: Vec3, tl: Vec3, n: Vec3, color: Color32, nu: i32, nv: i32, seed: i32) {
    let nu = nu.max(1);
    let nv = nv.max(1);
    for i in 0..nu {
        for j in 0..nv {
            let (u0, u1) = (i as f32 / nu as f32, (i + 1) as f32 / nu as f32);
            let (v0, v1) = (j as f32 / nv as f32, (j + 1) as f32 / nv as f32);
            let at = |u: f32, v: f32| {
                let bottom = bl + (br - bl) * u;
                let top = tl + (tr - tl) * u;
                bottom + (top - bottom) * v
            };
            let c = dither(color, seed.wrapping_add(i.wrapping_mul(7)).wrapping_add(j.wrapping_mul(13)), 0.045);
            m.push_quad(at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1), n, c);
        }
    }
}

pub fn panel(c: Vec3, right: Vec3, up: Vec3, w: f32, h: f32, color: Color32) -> Mesh3 {
    let r = right.normalized() * (w * 0.5);
    let u = up.normalized() * (h * 0.5);
    let n = right.cross(up).normalized();
    let mut m = Mesh3::new(color);
    m.push_quad(c - r - u, c + r - u, c + r + u, c - r + u, n, color);
    m
}

/// A sloped ramp quad from a bottom edge (bl,br) to a top edge (tl,tr). From the parkour module.
pub fn ramp(bl: Vec3, br: Vec3, tr: Vec3, tl: Vec3, color: Color32) -> Mesh3 {
    let n = (br - bl).cross(tl - bl).normalized();
    let mut m = Mesh3::new(color);
    m.push_quad(bl, br, tr, tl, n, color);
    m
}

/// A camera-facing billboard quad of half-size `s` at `center`, oriented by the camera's right/up.
/// This is how the particle system draws everything (muzzle flash, sparks, blood) without point sprites
/// — exactly the technique described in `shooter_particles.rs`.
pub fn billboard(center: Vec3, s: f32, cam_right: Vec3, cam_up: Vec3, color: Color32, emissive: f32) -> Mesh3 {
    let r = cam_right * s;
    let u = cam_up * s;
    let n = cam_right.cross(cam_up).normalized();
    let mut m = Mesh3::new(color);
    m.emissive = emissive;
    m.push_quad(center - r - u, center + r - u, center + r + u, center - r + u, n, color);
    m
}

/// A blocky low-poly UV sphere — enemies, props, pickups. `segs`/`rings` kept small for that faceted
/// PlayStation-2-era silhouette. Mirrors `mesh3d::uv_sphere`.
pub fn uv_sphere(center: Vec3, r: f32, segs: usize, rings: usize, color: Color32) -> Mesh3 {
    let mut m = Mesh3::new(color);
    let segs = seg(segs);
    let rings = seg(rings);
    let rings = rings.max(2);
    // Repeat the atlas tile a few times around the surface so a textured sphere (rock/boulder/dome) reads at a
    // sensible unit size rather than one giant stretched tile. Rounded so the u-wrap stays seamless.
    let ru = (std::f32::consts::TAU * r / 1.7).round().max(1.0);
    let rv = (std::f32::consts::PI * r / 1.7).round().max(1.0);
    let uvat = |i: usize, j: usize| egui::Pos2::new(i as f32 / segs as f32 * ru, j as f32 / rings as f32 * rv);
    let vert = |i: usize, j: usize| -> (Vec3, Vec3) {
        let v = j as f32 / rings as f32;
        let u = i as f32 / segs as f32;
        let phi = v * std::f32::consts::PI;
        let theta = u * std::f32::consts::TAU;
        let nrm = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
        (center + nrm * r, nrm)
    };
    let set_uv = |m: &mut Mesh3, a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| {
        let l = m.uv.len();
        m.uv[l - 3] = a;
        m.uv[l - 2] = b;
        m.uv[l - 1] = c;
    };
    for j in 0..rings {
        for i in 0..segs {
            let (a, na) = vert(i, j);
            let (b, nb) = vert(i + 1, j);
            let (c, nc) = vert(i + 1, j + 1);
            let (d, nd) = vert(i, j + 1);
            if j == 0 {
                let n2 = (na + nc + nd).normalized();
                m.push_tri(a, c, d, n2, color);
                set_uv(&mut m, uvat(i, j), uvat(i + 1, j + 1), uvat(i, j + 1));
            } else if j == rings - 1 {
                let n1 = (na + nb + nc).normalized();
                m.push_tri(a, b, c, n1, color);
                set_uv(&mut m, uvat(i, j), uvat(i + 1, j), uvat(i + 1, j + 1));
            } else {
                let n1 = (na + nb + nc).normalized();
                let n2 = (na + nc + nd).normalized();
                m.push_tri(a, b, c, n1, color);
                set_uv(&mut m, uvat(i, j), uvat(i + 1, j), uvat(i + 1, j + 1));
                m.push_tri(a, c, d, n2, color);
                set_uv(&mut m, uvat(i, j), uvat(i + 1, j + 1), uvat(i, j + 1));
            }
        }
    }
    m.single_sided = true; // closed sphere with outward normals
    m
}

/// A capped cylinder between two points (pipes, gun barrels, crystal stalks). Mirrors `mesh3d::tube`.
pub fn tube(a: Vec3, b: Vec3, r: f32, segs: usize, color: Color32) -> Mesh3 {
    let mut m = Mesh3::new(color);
    let segs = seg(segs);
    let axis = (b - a).normalized();
    // a frame perpendicular to the axis
    let up = if axis.y.abs() < 0.9 { Vec3::UP } else { Vec3::new(1.0, 0.0, 0.0) };
    let rt = axis.cross(up).normalized();
    let fw = axis.cross(rt).normalized();
    let segs = segs.max(3);
    let ring = |p: Vec3, i: usize| {
        let t = i as f32 / segs as f32 * std::f32::consts::TAU;
        let dir = rt * t.cos() + fw * t.sin();
        (p + dir * r, dir)
    };
    for i in 0..segs {
        let (a0, n0) = ring(a, i);
        let (a1, n1) = ring(a, i + 1);
        let (b0, _) = ring(b, i);
        let (b1, _) = ring(b, i + 1);
        m.push_quad(a0, b0, b1, a1, (n0 + n1).normalized(), color);
    }
    // End caps: a triangle fan at each end so the cylinder is CLOSED. Without these, looking into either end
    // shows straight through the hollow interior — the "cylinder with a missing face" artifact (made worse by
    // backface culling, which removes the far inner wall). Cheap (2*segs tris) and always correct.
    let cap_a = -axis; // outward normal at end `a`
    let cap_b = axis; // outward normal at end `b`
    for i in 0..segs {
        let (a0, _) = ring(a, i);
        let (a1, _) = ring(a, i + 1);
        m.push_tri(a, a1, a0, cap_a, color); // wound so the cap faces outward from `a`
        let (b0, _) = ring(b, i);
        let (b1, _) = ring(b, i + 1);
        m.push_tri(b, b0, b1, cap_b, color);
    }
    m.single_sided = true; // now a fully closed capped cylinder — safe to backface-cull with no holes
    m
}

// ── procedural "texture" tints (from logscope mesh3d) ─────────────────────────────

/// Tint vertices facing up toward `top` (moss/snow/lit floor) and sideways toward `side` (darker stone/
/// grime). One call turns a flat slab into something that reads as lit-from-above. Overwrites `vcol`.
pub fn tint_up(m: &mut Mesh3, top: Color32, side: Color32) {
    let cols: Vec<Color32> = m
        .norms
        .iter()
        .map(|n| lerp_c(side, top, n.normalized().y.clamp(0.0, 1.0).powf(1.4)))
        .collect();
    m.vcol = Some(cols);
}

/// Tint vertices by world height between `lo`→`hi` (`low` at the base, `high` at the top). Fakes ambient
/// occlusion on walls — dark and grimy at the floor, brighter toward the light. Overwrites `vcol`.
pub fn tint_height(m: &mut Mesh3, lo: f32, hi: f32, low: Color32, high: Color32) {
    let span = (hi - lo).max(1e-3);
    let cols: Vec<Color32> = m
        .verts
        .iter()
        .map(|p| lerp_c(low, high, ((p.y - lo) / span).clamp(0.0, 1.0)))
        .collect();
    m.vcol = Some(cols);
}

/// Multiplicative ambient-occlusion by height: darken toward `lo`, brighten toward `hi`, but *preserve* each
/// vertex's existing colour (so per-zone / per-material variation survives instead of being flattened to one
/// gradient). `floor_mul`/`top_mul` are brightness multipliers at the bottom/top.
pub fn shade_ao(m: &mut Mesh3, lo: f32, hi: f32, floor_mul: f32, top_mul: f32) {
    let span = (hi - lo).max(1e-3);
    let base: Vec<Color32> = match &m.vcol {
        Some(v) => v.clone(),
        None => vec![m.color; m.verts.len()],
    };
    let cols: Vec<Color32> = m
        .verts
        .iter()
        .zip(base)
        .map(|(p, c)| {
            let t = ((p.y - lo) / span).clamp(0.0, 1.0);
            let f = floor_mul + (top_mul - floor_mul) * t;
            Color32::from_rgb(
                (c.r() as f32 * f).clamp(0.0, 255.0) as u8,
                (c.g() as f32 * f).clamp(0.0, 255.0) as u8,
                (c.b() as f32 * f).clamp(0.0, 255.0) as u8,
            )
        })
        .collect();
    m.vcol = Some(cols);
}

// ── organic geometry (faceted stone & crystal) ───────────────────────────────────

/// A faceted rock: a low-poly sphere whose radius is perturbed by a smooth low-frequency field and
/// squashed in Y, rebuilt with per-FACE normals so it reads as chunky weathered stone, not a ball.
/// Straight out of `mesh3d::rock`.
pub fn rock(center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    let base = uv_sphere(center, r, 9, 7, color);
    let s = seed as f32 * 0.137;
    let displaced: Vec<Vec3> = base
        .verts
        .iter()
        .map(|&p| {
            let d = (p - center).normalized();
            let lump = 0.50 * (d.x * 3.1 + s).sin() * (d.y * 2.3 + s * 1.7).cos()
                + 0.34 * (d.y * 2.7 + s * 0.6).sin()
                + 0.24 * (d.z * 3.5 + s * 2.1).sin() * (d.x * 1.9 + s).cos();
            let rr = r * (1.0 + 0.13 * lump);
            center + Vec3::new(d.x * rr, d.y * rr * 0.82, d.z * rr)
        })
        .collect();
    let mut m = Mesh3::new(color);
    for t in &base.tris {
        let a = displaced[t[0] as usize];
        let b = displaced[t[1] as usize];
        let c = displaced[t[2] as usize];
        let n = (b - a).cross(c - a).normalized();
        m.push_tri(a, b, c, n, color);
        // carry the sphere's UVs so the rock maps the stone smoothly instead of stamping the same patch on
        // every facet (push_tri would otherwise leave default corner UVs here).
        let z = egui::Pos2::ZERO;
        let l = m.uv.len();
        m.uv[l - 3] = base.uv.get(t[0] as usize).copied().unwrap_or(z);
        m.uv[l - 2] = base.uv.get(t[1] as usize).copied().unwrap_or(z);
        m.uv[l - 1] = base.uv.get(t[2] as usize).copied().unwrap_or(z);
    }
    m
}

/// A stack of decreasing rocks jittered off-axis — a zen cairn / rubble pile. One mesh per stone.
pub fn cairn(base: Vec3, stones: usize, r0: f32, seed: u32, color: Color32) -> Vec<Mesh3> {
    let mut out = Vec::new();
    let mut y = base.y;
    for k in 0..stones.max(1) {
        let r = (r0 * (1.0 - 0.13 * k as f32)).max(0.2);
        let jx = (hash01(k as i32, seed as i32 + 3) - 0.5) * r * 0.3;
        let jz = (hash01(k as i32, seed as i32 + 9) - 0.5) * r * 0.3;
        out.push(rock(Vec3::new(base.x + jx, y + r, base.z + jz), r, seed.wrapping_add(k as u32 * 97), color));
        y += r * 1.7;
    }
    out
}

/// A faceted crystal spike rising `height` from `base`, `sides` faces, slight waist bulge and per-edge
/// jitter. The "Crystal Forest" cover node from the Z.ai mystical map — make it emissive in the caller.
pub fn crystal(base: Vec3, height: f32, r: f32, sides: usize, seed: u32, color: Color32) -> Mesh3 {
    let mut m = Mesh3::new(color);
    let sides = sides.max(4);
    let apex = base + Vec3::new(0.0, height, 0.0);
    let waist_y = height * 0.34;
    let ringp = |i: usize, rr: f32, y: f32| -> Vec3 {
        let t = i as f32 / sides as f32 * std::f32::consts::TAU;
        let j = (hash01(i as i32, seed as i32) - 0.5) * 0.20;
        base + Vec3::new(t.cos() * rr * (1.0 + j), y, t.sin() * rr * (1.0 + j))
    };
    for i in 0..sides {
        let b0 = ringp(i, r, 0.0);
        let b1 = ringp(i + 1, r, 0.0);
        let w0 = ringp(i, r * 1.15, waist_y);
        let w1 = ringp(i + 1, r * 1.15, waist_y);
        let n1 = (b1 - b0).cross(w0 - b0).normalized();
        m.push_quad(b0, b1, w1, w0, n1, color);
        let n2 = (w1 - w0).cross(apex - w0).normalized();
        m.push_tri(w0, w1, apex, n2, color);
    }
    m
}

/// A bullet-impact decal oriented to an arbitrary surface normal: a small dark scorch crater with a lighter
/// chipped-debris ring, alpha-faded toward the rim so it reads as damage burned into the surface rather than
/// a sticker slapped on top. Offset a hair off the surface along `normal` to avoid z-fighting with the host
/// wall. Two flat-shaded rings (not a true gradient — this engine has no per-pixel fragment shading) is
/// enough to read as a crater at typical view distance, matching the low-poly aesthetic everywhere else.
pub fn bullet_decal(center: Vec3, normal: Vec3, radius: f32, seed: u32) -> Mesh3 {
    let n = normal.normalized();
    let helper = if n.y.abs() < 0.9 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(1.0, 0.0, 0.0) };
    let right = n.cross(helper).normalized();
    let up = n.cross(right).normalized();
    let p = center + n * 0.012; // lift off the surface a hair so it never z-fights the wall behind it
    let ang0 = hash01(seed as i32, 17) * std::f32::consts::TAU; // random rotation so decals don't all align
    let segs = 8;

    // inner scorch crater — dark, mostly opaque, jagged radius so the edge doesn't read as a perfect circle
    let mut core = Mesh3::new(rgb(26, 23, 20));
    for i in 0..segs {
        let t0 = ang0 + i as f32 / segs as f32 * std::f32::consts::TAU;
        let t1 = ang0 + (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let r0 = radius * 0.42 * (0.8 + hash01(i as i32, seed as i32 ^ 0x51) * 0.35);
        let r1 = radius * 0.42 * (0.8 + hash01(i as i32 + 1, seed as i32 ^ 0x51) * 0.35);
        let b = p + right * (t0.cos() * r0) + up * (t0.sin() * r0);
        let c = p + right * (t1.cos() * r1) + up * (t1.sin() * r1);
        m_push_tri_dithered(&mut core, p, b, c, n, rgb(26, 23, 20), i as i32, seed as i32);
    }
    set_alpha(&mut core, 205);

    // outer chip ring — lighter debris colour, fading out with lower alpha toward the rim
    let mut ring = Mesh3::new(rgb(78, 70, 60));
    for i in 0..segs {
        let t0 = ang0 + i as f32 / segs as f32 * std::f32::consts::TAU;
        let t1 = ang0 + (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let ri = radius * 0.40;
        let ro0 = radius * (0.8 + hash01(i as i32 + 50, seed as i32 ^ 0x33) * 0.35);
        let ro1 = radius * (0.8 + hash01(i as i32 + 51, seed as i32 ^ 0x33) * 0.35);
        let b0 = p + right * (t0.cos() * ri) + up * (t0.sin() * ri);
        let c0 = p + right * (t1.cos() * ri) + up * (t1.sin() * ri);
        let b1 = p + right * (t0.cos() * ro0) + up * (t0.sin() * ro0);
        let c1 = p + right * (t1.cos() * ro1) + up * (t1.sin() * ro1);
        ring.push_quad(b0, c0, c1, b1, n, dither(rgb(78, 70, 60), i as i32 + seed as i32, 0.16));
    }
    set_alpha(&mut ring, 108);

    core.merge(&ring);
    core
}

#[inline]
fn m_push_tri_dithered(m: &mut Mesh3, a: Vec3, b: Vec3, c: Vec3, n: Vec3, col: Color32, key: i32, seed: i32) {
    m.push_tri(a, b, c, n, dither(col, key.wrapping_mul(31).wrapping_add(seed), 0.10));
}

/// A flat horizontal disc (triangle fan) facing up — floor glow rings, pickup pads, skylights.
pub fn disc(center: Vec3, r: f32, segs: usize, color: Color32) -> Mesh3 {
    let mut m = Mesh3::new(color);
    let segs = seg(segs);
    for i in 0..segs {
        let t0 = i as f32 / segs as f32 * std::f32::consts::TAU;
        let t1 = (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
        let b = center + Vec3::new(t0.cos() * r, 0.0, t0.sin() * r);
        let c = center + Vec3::new(t1.cos() * r, 0.0, t1.sin() * r);
        m.push_tri(center, b, c, Vec3::UP, color);
    }
    m
}

/// Make a mesh translucent: stamp alpha `a` on the base colour and every per-vertex colour. The renderer
/// shades with alpha preserved and paints back-to-front, so `a < 255` shows the scene through it (glass,
/// drifting mist, skylight shafts). Straight from `mesh3d::set_alpha`.
pub fn set_alpha(m: &mut Mesh3, a: u8) {
    let wa = |c: Color32| Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
    m.color = wa(m.color);
    if let Some(v) = &mut m.vcol {
        for c in v.iter_mut() {
            *c = wa(*c);
        }
    }
}

/// Rotate a mesh's vertices (and normals) about the world-Y axis through `pivot` by `ang` radians —
/// used to spin pickups and oriented props without rebuilding geometry.
pub fn rotate_y_about(m: &mut Mesh3, pivot: Vec3, ang: f32) {
    let (s, c) = ang.sin_cos();
    for p in &mut m.verts {
        let dx = p.x - pivot.x;
        let dz = p.z - pivot.z;
        p.x = pivot.x + dx * c - dz * s;
        p.z = pivot.z + dx * s + dz * c;
    }
    for n in &mut m.norms {
        let nx = n.x * c - n.z * s;
        let nz = n.x * s + n.z * c;
        n.x = nx;
        n.z = nz;
    }
}

/// Rotate a mesh about a pivot in the X–Y plane (roll around the view/forward axis). Used to cant shoulder
/// pauldrons outward so armor reads as angled plating rather than flat slabs.
pub fn rotate_z_about(m: &mut Mesh3, pivot: Vec3, ang: f32) {
    let (s, c) = ang.sin_cos();
    for p in &mut m.verts {
        let dx = p.x - pivot.x;
        let dy = p.y - pivot.y;
        p.x = pivot.x + dx * c - dy * s;
        p.y = pivot.y + dx * s + dy * c;
    }
    for n in &mut m.norms {
        let nx = n.x * c - n.y * s;
        let ny = n.x * s + n.y * c;
        n.x = nx;
        n.y = ny;
    }
}

/// Rotate a mesh about an arbitrary unit `axis` through `pivot` by `ang` radians (Rodrigues' rotation). Used
/// for tumbling debris chunks that spin on a random axis.
pub fn rotate_about(m: &mut Mesh3, pivot: Vec3, axis: Vec3, ang: f32) {
    let k = axis.normalized();
    let (s, c) = ang.sin_cos();
    let rot = |v: Vec3| -> Vec3 {
        // v_rot = v*cos + (k×v)*sin + k*(k·v)*(1-cos)
        v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c))
    };
    for p in &mut m.verts {
        *p = pivot + rot(*p - pivot);
    }
    for n in &mut m.norms {
        *n = rot(*n);
    }
}

/// Scale a mesh on the ground plane (used to flatten stepping-stone discs into ellipses).
pub fn scale_xz(m: &mut Mesh3, sx: f32, sz: f32) {
    for p in &mut m.verts {
        p.x *= sx;
        p.z *= sz;
    }
}

/// A 3D hash in [0,1) — ported from logscope for foliage scatter.
#[inline]
fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841) ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}

/// A puff of foliage: many small cupped leaf blades scattered on a sphere around `center`. Ported from
/// logscope's `leaf_cluster` — the silhouette reads as a leafy crown rather than a smooth orb.
pub fn leaf_cluster(center: Vec3, r: f32, seed: u32, color: Color32) -> Mesh3 {
    let mut m = Mesh3::new(color);
    // a flat leaf-blade outline in (along, across) local coords: pointed tip → rounded body → base
    const BLADE: [(f32, f32); 8] = [
        (1.0, 0.0),
        (0.6, 0.22),
        (0.2, 0.32),
        (-0.35, 0.30),
        (-0.95, 0.0),
        (-0.35, -0.30),
        (0.2, -0.32),
        (0.6, -0.22),
    ];
    let leaves = seg(16) as i32;
    for k in 0..leaves {
        let s = seed as i32;
        let a = hash3(k, s, 1) * std::f32::consts::TAU;
        let b = (hash3(k, s, 2) * 2.0 - 1.0).acos(); // even spherical scatter
        let dir = Vec3::new(b.sin() * a.cos(), b.cos(), b.sin() * a.sin());
        let rad = r * (0.70 + 0.28 * hash3(k, s, 3));
        let cpt = center + dir * rad;
        let up = if dir.y.abs() < 0.9 { Vec3::UP } else { Vec3::new(1.0, 0.0, 0.0) };
        let along = dir.cross(up).normalized();
        let across = dir.cross(along);
        let len = r * (0.42 + 0.22 * hash3(k, s, 4));
        let curl = len * (0.30 + 0.22 * hash3(k, s, 5));
        let cv = cpt + dir * (curl * 0.45); // raised midrib (cup high point)
        let base = m.verts.len() as u32;
        // per-leaf tone: mostly green with brightness scatter and the occasional warmer/yellowing leaf
        let lc = {
            let v = 0.82 + hash3(k, s, 6) * 0.34;
            let warm = ((hash3(k, s, 7) - 0.62).max(0.0)) * 1.4;
            lerp_c(scale_rgb(color, v), rgb(196, 178, 84), warm.min(0.5))
        };
        m.verts.push(cv);
        m.norms.push(dir);
        if let Some(vc) = m.vcol.as_mut() {
            vc.push(lerp_c(lc, rgb(255, 255, 255), 0.16)); // bright midrib catching the sun
        }
        for &(lx, ly) in &BLADE {
            let edge = cpt + along * (lx * len) + across * (ly * len);
            let dip = curl * (0.35 * lx * lx + 1.1 * ly * ly);
            let p = edge - dir * dip;
            let nrm = (dir + (cv - p).normalized() * 0.55).normalized();
            m.verts.push(p);
            m.norms.push(nrm);
            if let Some(vc) = m.vcol.as_mut() {
                // tip (lx>0) lit, base and margins in self-shadow → the leaf reads dimensional, not flat
                let shade = (0.74 + 0.26 * (lx * 0.5 + 0.5) - ly.abs() * 0.22).clamp(0.6, 1.12);
                let mut ec = scale_rgb(lc, shade);
                // thin margins scatter light through the leaf → a faint warm-green subsurface glow at the edge
                let trans = ((ly.abs() - 0.18).max(0.0) * 0.55).min(0.32);
                ec = lerp_c(ec, rgb(192, 222, 120), trans);
                vc.push(ec);
            }
        }
        let n = BLADE.len() as u32;
        for j in 0..n {
            m.tris.push([base, base + 1 + j, base + 1 + ((j + 1) % n)]);
        }
    }
    m
}

/// Recursive triplet branch (trunk → 3 → 9): a tapering wooden tube splitting into three children splayed
/// around the parent, with a leaf cluster at each tip. Ported from logscope's `tree_branch`. Appends to `out`.
pub fn tree_branch(out: &mut Vec<Mesh3>, origin: Vec3, dir: Vec3, len: f32, rad: f32, depth: u32, seed: i32, wood: Color32, leaf: Color32) {
    let dir = dir.normalized();
    let end = origin + dir * len;
    let mut br = tube(origin, end, rad, 7, wood);
    tint_height(&mut br, origin.y, end.y, lerp_c(wood, rgb(0, 0, 0), 0.45), lerp_c(wood, rgb(255, 255, 255), 0.18));
    out.push(br);
    if depth == 0 {
        let mut puff = leaf_cluster(end, len * 0.85, (seed as u32).wrapping_mul(131).wrapping_add(7), leaf);
        tint_up(&mut puff, lerp_c(leaf, rgb(255, 255, 255), 0.22), lerp_c(leaf, rgb(0, 0, 0), 0.45));
        out.push(puff);
        return;
    }
    let up = if dir.y.abs() < 0.9 { Vec3::UP } else { Vec3::new(1.0, 0.0, 0.0) };
    let ax = dir.cross(up).normalized();
    let ay = dir.cross(ax);
    let spread = 0.6;
    for k in 0..3 {
        let ang = std::f32::consts::TAU * k as f32 / 3.0 + hash3(seed.wrapping_add(k), 5, 0) * 0.6;
        let nd = (dir + ax * (ang.cos() * spread) + ay * (ang.sin() * spread)).normalized();
        tree_branch(out, end, nd, len * 0.72, rad * 0.66, depth - 1, seed.wrapping_mul(3).wrapping_add(k + 1), wood, leaf);
    }
}

/// A leaning tree at `base`: a recursive branch system with leafy crowns. Returns its meshes.
pub fn tree(base: Vec3, len: f32, rad: f32, depth: u32, seed: u32, wood: Color32, leaf: Color32) -> Vec<Mesh3> {
    let mut out = Vec::new();
    // a gentle seed-driven lean off vertical
    let s = (seed & 0x7fff) as i32;
    let lean = Vec3::new((hash3(s, 1, 0) - 0.5) * 0.5, 1.0, (hash3(s, 2, 0) - 0.5) * 0.5).normalized();
    tree_branch(&mut out, base, lean, len, rad, depth, s, wood, leaf);
    out
}

/// Distinct tree silhouettes so biomes don't all share one canopy shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeKind {
    /// Recursive branching with a rounded crown (deciduous).
    Broadleaf,
    /// Tall straight trunk with stacked conical foliage layers (pine/fir/redwood).
    Conifer,
    /// Bare dead trunk with a few stripped branches and no leaves (blighted/toxic).
    Snag,
    /// Slim trunk crowned with radial drooping fronds (palm/alien).
    Frond,
}

/// Build a tree of a given silhouette. Same call shape as `tree` so the grove can swap styles per biome.
pub fn tree_styled(kind: TreeKind, base: Vec3, len: f32, rad: f32, depth: u32, seed: u32, wood: Color32, leaf: Color32) -> Vec<Mesh3> {
    let up = Vec3::new(0.0, 1.0, 0.0);
    match kind {
        TreeKind::Broadleaf => tree(base, len, rad, depth, seed, wood, leaf),
        TreeKind::Conifer => {
            let top = base + up * len;
            let mut out = vec![tube(base, top, rad * 0.9, 6, wood)];
            let layers = 4 + (seed & 1) as i32;
            for k in 0..layers {
                let t = k as f32 / layers as f32;
                let cy = base.y + len * (0.32 + t * 0.62);
                let cr = len * 0.34 * (1.0 - t * 0.72) + 0.2;
                out.push(leaf_cluster(Vec3::new(base.x, cy, base.z), cr, seed.wrapping_add(k as u32 * 7), leaf));
            }
            out
        }
        TreeKind::Snag => {
            let top = base + up * len;
            let mut out = vec![tube(base, top, rad, 6, wood)];
            let s = (seed & 0x7fff) as i32;
            for k in 0..4 {
                let a = k as f32 / 4.0 * std::f32::consts::TAU + hash3(s, k, 1);
                let h = base.y + len * (0.45 + (k as f32 % 2.0) * 0.25);
                let dir = Vec3::new(a.cos(), 0.7, a.sin()).normalized();
                let bl = len * (0.3 + hash3(s, k, 2) * 0.25);
                out.push(tube(Vec3::new(base.x, h, base.z), Vec3::new(base.x, h, base.z) + dir * bl, rad * 0.45, 5, wood));
            }
            out
        }
        TreeKind::Frond => {
            let top = base + up * len;
            let mut out = vec![tube(base, top, rad * 0.7, 6, wood)];
            let s = (seed & 0x7fff) as i32;
            for k in 0..6 {
                let a = k as f32 / 6.0 * std::f32::consts::TAU + hash3(s, k, 3);
                let tip = top + Vec3::new(a.cos() * len * 0.35, len * 0.05, a.sin() * len * 0.35);
                out.push(tube(top, tip, rad * 0.3, 4, leaf));
                out.push(leaf_cluster(tip, len * 0.12, seed.wrapping_add(k as u32 * 5), leaf));
            }
            out
        }
    }
}

/// A translucent shaft of coloured light: a soft glowing beam, a bright aperture disc up top, and a glow
/// pool where it lands. Ported from logscope's `light_shaft`. Returns its meshes (all emissive/translucent).
pub fn light_shaft(x: f32, z: f32, y_top: f32, y_floor: f32, col: Color32, seed: u32) -> Vec<Mesh3> {
    let mut beam = tube(Vec3::new(x, y_top, z), Vec3::new(x, y_floor + 0.08, z), 0.42, 12, col);
    beam.emissive = 1.0;
    set_alpha(&mut beam, 60);
    let mut ap = disc(Vec3::new(x, y_top, z), 0.5, 18, col);
    ap.emissive = 1.9;
    let mut pool = disc(Vec3::new(x, y_floor + 0.05, z), 0.85, 22, col);
    set_alpha(&mut pool, 140);
    pool.emissive = 0.9;
    let _ = seed;
    vec![beam, ap, pool]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_tint_shifts_hue_but_preserves_brightness_and_respects_amount() {
        let orange = rgb(230, 120, 40);
        let green = rgb(70, 190, 90);
        // a mid-grey body tinted toward each team should take on that team's hue character
        let body = rgb(140, 140, 140);
        let hostile = team_tint(body, orange, 0.55);
        let friendly = team_tint(body, green, 0.55);
        // hostile should read warm (more red than blue); friendly should read green-dominant
        assert!(hostile.r() > hostile.b(), "hostile tint should be warm (r>b)");
        assert!(friendly.g() > friendly.r() && friendly.g() > friendly.b(), "friendly tint should be green-dominant");
        // amount 0 is a no-op; amount 1 pulls fully to the tinted colour
        let none = team_tint(body, orange, 0.0);
        assert_eq!((none.r(), none.g(), none.b()), (body.r(), body.g(), body.b()), "amount 0 leaves the colour unchanged");
        // brightness is roughly preserved: a mid-grey stays mid, not blown out or crushed
        let lum = |c: Color32| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        let lb = lum(body);
        assert!((lum(hostile) - lb).abs() < 55.0, "tint should roughly preserve luminance");
        // a dark body stays darker than a bright body after the same tint (shading detail survives)
        let dark = team_tint(rgb(60, 60, 60), orange, 0.55);
        let bright = team_tint(rgb(210, 210, 210), orange, 0.55);
        assert!(lum(dark) < lum(bright), "tint preserves relative light/dark across the body");
    }

    #[test]
    fn point_light_falls_off_with_distance_and_faces() {
        let pl = PointLight::new(Vec3::new(0.0, 0.0, 0.0), rgb(255, 255, 255), 1.0, 10.0);
        // a point close to the light, normal facing it, gets strong contribution
        let near = pl.contribution(Vec3::new(1.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        // a point far away gets less
        let far = pl.contribution(Vec3::new(8.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        // beyond the radius, nothing
        let out = pl.contribution(Vec3::new(20.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        assert!(near > far, "closer to the light should be brighter ({near:.3} vs {far:.3})");
        assert_eq!(out, 0.0, "beyond the radius there is no light");
        // a face turned away from the light gets less than one facing it (half-lambert, not zero)
        let facing = pl.contribution(Vec3::new(2.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        let grazing = pl.contribution(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
        assert!(facing > grazing, "a face toward the light is brighter than a grazing one");
        assert!(grazing > 0.0, "half-lambert keeps grazing/side faces from going fully black");
    }

    #[test]
    fn masonry_materials_have_per_cell_variation_and_distinct_seams() {
        // The jump from "a coloured splotch with noise" to "individually laid bricks" needs two things a flat
        // scalar-multiply of one base colour can't give: (1) each masonry unit its own tone, and (2) a
        // mortar/grout colour that's genuinely a different hue, not just a darker version of the face.
        // This locks both in numerically so the material renderer can't silently regress to monochrome.
        for mat in [Material::Brick, Material::StoneBlock, Material::Cobble, Material::Tile, Material::HexPlate] {
            let n = 96;
            let mut lum = Vec::with_capacity(n * n);
            let mut rgbv = Vec::with_capacity(n * n);
            for py in 0..n {
                for px in 0..n {
                    let c = material_pixel(mat, px as f32 / n as f32, py as f32 / n as f32);
                    lum.push(0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32);
                    rgbv.push((c.r() as f32, c.g() as f32, c.b() as f32));
                }
            }
            // (1) real luminance spread across the tile (varied units + recessed seams), not a flat swatch
            let lmin = lum.iter().cloned().fold(f32::MAX, f32::min);
            let lmax = lum.iter().cloned().fold(f32::MIN, f32::max);
            assert!(
                lmax - lmin > 40.0,
                "{mat:?} should show real tonal range across its cells (got {:.0})",
                lmax - lmin
            );
            // (2) the darkest (seam) pixels must differ in *hue balance* from the brightest (face) pixels —
            // i.e. the grout isn't just the face colour scaled down. Compare mean (R-B) warm balance of the
            // lightest vs darkest deciles; a pure scalar multiply keeps that ratio identical.
            let mut idx: Vec<usize> = (0..lum.len()).collect();
            idx.sort_by(|&a, &b| lum[a].partial_cmp(&lum[b]).unwrap());
            let dec = lum.len() / 10;
            let warm_of = |slice: &[usize]| -> f32 {
                let n = slice.len() as f32;
                let rb = slice.iter().map(|&i| (rgbv[i].0 - rgbv[i].2) / rgbv[i].1.max(1.0)).sum::<f32>() / n;
                rb
            };
            let seam_warm = warm_of(&idx[..dec]);
            let face_warm = warm_of(&idx[idx.len() - dec..]);
            assert!(
                (seam_warm - face_warm).abs() > 0.02,
                "{mat:?} seam should differ in hue from the face, not just be darker (seam_warm={seam_warm:.3} face_warm={face_warm:.3})"
            );
        }
    }

    #[test]
    fn every_atlas_material_bakes_a_valid_distinct_tile() {
        // Each material in the atlas should bake to a non-trivial tile (not pure white/black/flat) and the
        // four new materials should carry their intended base hue. Guards against a new material being added
        // to the enum but forgotten in material_base/material_mod (which would bake a dead or default tile).
        for &mat in ATLAS_MATERIALS.iter() {
            let n = 32;
            let mut lum_sum = 0.0f32;
            let mut var = 0.0f32;
            let mut samples = Vec::new();
            for py in 0..n {
                for px in 0..n {
                    let c = material_pixel(mat, px as f32 / n as f32, py as f32 / n as f32);
                    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
                    lum_sum += l;
                    samples.push(l);
                }
            }
            let mean = lum_sum / samples.len() as f32;
            for l in &samples {
                var += (l - mean).powi(2);
            }
            var /= samples.len() as f32;
            assert!(mean > 20.0 && mean < 250.0, "{mat:?} tile is near-black or blown out (mean lum {mean:.0})");
            assert!(var.sqrt() > 1.0, "{mat:?} tile is dead flat (no surface detail baked)");
        }
        // the four newest materials specifically carry the right dominant hue
        let marble = material_base(Material::Marble);
        assert!(marble.r() > 200 && marble.g() > 200, "Marble should be a pale stone");
        let thatch = material_base(Material::Thatch);
        assert!(thatch.r() > thatch.b(), "Thatch should be warm straw");
        let hex = material_base(Material::HexPlate);
        assert!(hex.b() >= hex.r(), "HexPlate should read as cool metal");
    }

    #[test]
    fn continuous_materials_stay_uniform_in_hue() {
        // Concrete/plaster/sand are continuous surfaces — they should NOT get per-cell hue jitter (that would
        // read as noise, not material). Confirm their colour channels track together (no warm/cool cell split).
        for mat in [Material::Concrete, Material::Plaster] {
            let n = 64;
            let mut warm = Vec::new();
            for py in 0..n {
                for px in 0..n {
                    let c = material_pixel(mat, px as f32 / n as f32, py as f32 / n as f32);
                    warm.push((c.r() as f32 - c.b() as f32));
                }
            }
            let m = warm.iter().sum::<f32>() / warm.len() as f32;
            let sd = (warm.iter().map(|w| (w - m).powi(2)).sum::<f32>() / warm.len() as f32).sqrt();
            assert!(sd < 8.0, "{mat:?} should stay hue-uniform (warm sd={sd:.1})");
        }
    }

    #[test]
    fn uv_sphere_has_no_degenerate_pole_triangles() {
        // At each pole, an entire ring collapses to one point (phi=0 or phi=PI); naively emitting both
        // triangles of every polar "quad" produces one zero-area triangle per segment. This is what a mesh
        // audit harness (examples/mesh_audit.rs) caught across every rock/cairn in the game, since rock()
        // and cairn() both build directly on uv_sphere's topology.
        for (segs, rings) in [(8, 6), (5, 3), (12, 8), (4, 2)] {
            let m = uv_sphere(Vec3::ZERO, 1.0, segs, rings, rgb(200, 200, 200));
            for tri in &m.tris {
                let a = m.verts[tri[0] as usize];
                let b = m.verts[tri[1] as usize];
                let c = m.verts[tri[2] as usize];
                let area = (b - a).cross(c - a).len() * 0.5;
                assert!(area > 1e-6, "uv_sphere({segs},{rings}) produced a degenerate (zero-area) triangle");
            }
        }
    }

    #[test]
    fn box_face_masking() {
        // Large faces now subdivide for real texture tiling + finer PBR shading (see `boxx`'s doc comment),
        // so the exact tri count is an implementation detail — what this test actually checks is that face
        // masking selects the right faces: a symmetric cube's single-face count times 6 equals FACE_ALL's.
        let all = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(200, 200, 200), FACE_ALL);
        let top = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(200, 200, 200), FACE_TOP);
        assert!(top.tri_count() > 0, "the top face should have geometry");
        assert_eq!(all.tri_count(), top.tri_count() * 6, "a symmetric cube's 6 faces should each subdivide identically");
        assert!(top.tri_count() < all.tri_count(), "one face must have fewer triangles than all six");
    }

    #[test]
    fn primitives_keep_vcol_in_sync() {
        // Builders that set vcol=Some must keep it the same length as verts, or the renderer indexes out of
        // bounds. leaf_cluster used to push verts without colours — this guards against regressions.
        let leaves = leaf_cluster(Vec3::ZERO, 1.5, 7, rgb(40, 120, 50));
        let vc = leaves.vcol.as_ref().expect("leaf_cluster sets vcol");
        assert_eq!(vc.len(), leaves.verts.len());
    }

    #[test]
    fn merge_reindexes() {
        let mut a = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(1, 2, 3), FACE_TOP);
        let b = boxx(Vec3::new(5.0, 0.0, 0.0), Vec3::splat(1.0), rgb(1, 2, 3), FACE_TOP);
        let (va, ta) = (a.verts.len(), a.tris.len());
        a.merge(&b);
        assert_eq!(a.verts.len(), va + b.verts.len());
        assert_eq!(a.tris.len(), ta + b.tris.len());
        // the merged-in triangle must reference the appended vertices, not the originals
        let last = *a.tris.last().unwrap();
        assert!(last.iter().all(|&i| (i as usize) >= va));
    }

    #[test]
    fn billboard_faces_have_area() {
        let bb = billboard(Vec3::ZERO, 1.0, Vec3::new(1.0, 0.0, 0.0), Vec3::UP, rgb(255, 255, 255), 1.0);
        assert_eq!(bb.tri_count(), 2);
        assert!(bb.emissive > 0.0);
    }

    #[test]
    fn rock_is_displaced_and_closed() {
        let r = rock(Vec3::ZERO, 1.0, 7, rgb(120, 120, 120));
        assert!(r.tri_count() > 20);
        // displacement + squash means it is not a perfect unit sphere
        let maxr = r.verts.iter().map(|p| p.len()).fold(0.0_f32, f32::max);
        assert!(maxr > 0.5 && maxr < 1.6);
    }

    #[test]
    fn crystal_rises_to_an_apex() {
        let c = crystal(Vec3::ZERO, 3.0, 0.5, 6, 1, rgb(140, 200, 255));
        assert!(c.tri_count() >= 6);
        let top = c.verts.iter().map(|p| p.y).fold(0.0_f32, f32::max);
        assert!((top - 3.0).abs() < 1e-3);
    }

    #[test]
    fn tint_up_makes_top_brighter_than_side() {
        let mut m = boxx(Vec3::ZERO, Vec3::splat(1.0), rgb(128, 128, 128), FACE_ALL);
        tint_up(&mut m, rgb(240, 240, 240), rgb(40, 40, 40));
        let vc = m.vcol.as_ref().unwrap();
        // top-face vertices (normal.y≈1) should be near `top`, side faces near `side`
        let bright = m.norms.iter().zip(vc).filter(|(n, _)| n.y > 0.9).map(|(_, c)| c.r()).max().unwrap();
        let dark = m.norms.iter().zip(vc).filter(|(n, _)| n.y.abs() < 0.1).map(|(_, c)| c.r()).min().unwrap();
        assert!(bright > dark);
    }

    #[test]
    fn set_alpha_marks_translucent() {
        let mut d = disc(Vec3::ZERO, 1.0, 12, rgb(200, 200, 255));
        assert!(d.tri_count() >= 12);
        set_alpha(&mut d, 120);
        assert_eq!(d.color.a(), 120);
        assert!(d.vcol.as_ref().unwrap().iter().all(|c| c.a() == 120));
    }

    #[test]
    fn rotate_y_moves_offset_point() {
        let mut m = boxx(Vec3::new(2.0, 0.0, 0.0), Vec3::splat(0.5), rgb(255, 0, 0), FACE_ALL);
        let before = m.verts[0];
        rotate_y_about(&mut m, Vec3::ZERO, std::f32::consts::PI);
        let after = m.verts[0];
        // a 180° turn about the origin flips x and z sign
        assert!((after.x + before.x).abs() < 1e-3);
        assert!((after.z + before.z).abs() < 1e-3);
    }

    #[test]
    fn sway_weights_survive_merge() {
        let mut g = Mesh3::new(rgb(80, 160, 80));
        g.push_tri_sway(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.1, 0.0, 0.0),
            Vec3::new(0.05, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            rgb(80, 160, 80),
            0.0,
            0.0,
            1.0,
        );
        let rigid = boxx(Vec3::new(5.0, 0.5, 0.0), Vec3::new(0.5, 0.5, 0.5), rgb(120, 120, 120), FACE_ALL);
        let mut a = g.clone();
        a.merge(&rigid);
        assert_eq!(a.sway.len(), a.verts.len(), "sway parallel to verts after merge");
        assert!(a.sway[2] > 0.9 && a.sway[0] == 0.0, "tip bows, base rooted");
        assert!(a.sway[3..].iter().all(|&w| w == 0.0), "rigid verts zero sway");

        let mut b = rigid.clone();
        b.merge(&g);
        assert_eq!(b.sway.len(), b.verts.len());
        assert!(b.sway[..rigid.verts.len()].iter().all(|&w| w == 0.0));
        assert!(*b.sway.last().unwrap() > 0.9);
    }
}
