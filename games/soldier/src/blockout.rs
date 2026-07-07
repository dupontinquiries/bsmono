//! blockout.rs — the "Volumetric Decomposition → Instructional Reversal" pipeline.
//!
//! Maps are authored in two phases. First you place abstract **volumes tagged with a spatial verb** — the
//! grey-block chessboard that fixes flow and sightlines before any detail exists (Block, Channel, Expose,
//! Conceal, Elevate, LowCover, HighCover). Then a **dressing pass** materialises each tagged volume into
//! detailed geometry, choosing its material *by context* rather than at random: a low-cover block beside mud
//! becomes rotten wood or pitted concrete, never clean metal. Decay is bound to geometry too — rust streaks
//! run down steel, moss gathers at a roof's lower edge, rebar bristles from a breach.
//!
//! This makes detail "the logical consequence of geometry": you state the spatial necessity ("an elongated
//! mass here to break this sightline") and the recipe dresses it consistently with its surroundings.

use crate::level::{Obstacle, Palette};
use crate::math::Vec3;
use crate::mesh::{boxx, hash01, lerp_c, rgb, tint_height, tube, uv_sphere, Mesh3, FACE_ALL, FACE_NO_BOTTOM};
use egui::Color32;

/// Layer-3 spatial verbs: what a volume *does* to the player, independent of what it's made of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// An elongated mass that breaks a sightline across a lane (a wreck, a fallen mast).
    Block,
    /// Low-profile crouch cover.
    LowCover,
    /// A stand-height wall segment — cover + a hard edge to peek.
    HighCover,
    /// A wall that forces movement around it (channels flow).
    Channel,
    /// A tall, thin mass granting (and exposing) a vertical vantage.
    Expose,
    /// A raised platform — elevation advantage you can stand on.
    Elevate,
    /// A solid mass that conceals/denies a whole pocket of space.
    Conceal,
}

impl Role {
    pub const ALL: [Role; 7] = [Role::Block, Role::LowCover, Role::HighCover, Role::Channel, Role::Expose, Role::Elevate, Role::Conceal];
    pub fn name(self) -> &'static str {
        match self {
            Role::Block => "Block",
            Role::LowCover => "LowCover",
            Role::HighCover => "HighCover",
            Role::Channel => "Channel",
            Role::Expose => "Expose",
            Role::Elevate => "Elevate",
            Role::Conceal => "Conceal",
        }
    }
}

/// Material classes a volume can be dressed in. Chosen by context, never at random.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaterialClass {
    RottenWood,
    PittedConcrete,
    RustedSteel,
    CleanMetal,
    Granite,
    Sandbag,
}

impl MaterialClass {
    /// Base colour, tinted toward the biome palette so dressing stays coherent.
    fn color(self, pal: &Palette) -> Color32 {
        match self {
            MaterialClass::RottenWood => rgb(96, 72, 46),
            MaterialClass::PittedConcrete => lerp_c(pal.wall, rgb(132, 132, 126), 0.5),
            MaterialClass::RustedSteel => rgb(128, 78, 46),
            MaterialClass::CleanMetal => lerp_c(pal.wall, rgb(176, 184, 196), 0.4),
            MaterialClass::Granite => lerp_c(pal.rock, rgb(120, 122, 118), 0.4),
            MaterialClass::Sandbag => rgb(146, 128, 90),
        }
    }
    /// How weathered/streaked the dressing should read (0 clean → 1 heavily decayed).
    fn decay(self) -> f32 {
        match self {
            MaterialClass::CleanMetal => 0.1,
            MaterialClass::Granite => 0.3,
            MaterialClass::Sandbag => 0.35,
            MaterialClass::PittedConcrete => 0.5,
            MaterialClass::RustedSteel => 0.75,
            MaterialClass::RottenWood => 0.7,
        }
    }
    /// The atlas texture this cover class is surfaced with, so walls/cover read as real material rather than a
    /// flat tint. The per-class `color()` above stays on as a biome tint that multiplies over the texel.
    pub fn material(self) -> crate::mesh::Material {
        use crate::mesh::Material;
        match self {
            MaterialClass::RottenWood => Material::WeatheredWood,
            MaterialClass::PittedConcrete => Material::Concrete,
            MaterialClass::RustedSteel => Material::RustPlate,
            MaterialClass::CleanMetal => Material::HexPlate,
            MaterialClass::Granite => Material::StoneBlock,
            MaterialClass::Sandbag => Material::Cloth,
        }
    }
}

/// The environmental context a block is dressed against — the bindings read from this.
#[derive(Clone, Copy)]
pub struct Context {
    pub pal: Palette,
    /// Near water/mud: forbids clean metal, favours rot/pitting/moss.
    pub wet: bool,
    /// Indoors (an enclosed, dry, built interior): favours metal/concrete over wood/stone.
    pub indoor: bool,
    pub ground_y: f32,
}

/// **The material binding rule** (essay Phase 2, Step 2). Material is a function of role + environment, so the
/// AI never drops a pristine metal crate in a muddy riverbed.
pub fn material_for(role: Role, ctx: &Context, seed: u32) -> MaterialClass {
    let r = hash01(seed as i32, 7);
    match role {
        // an elongated wreck breaking a sightline → rusted hulk outdoors, granite boulder if natural & dry
        Role::Block => {
            if ctx.wet || !ctx.indoor {
                MaterialClass::RustedSteel
            } else {
                MaterialClass::CleanMetal
            }
        }
        Role::LowCover => {
            if ctx.wet {
                if r < 0.5 { MaterialClass::RottenWood } else { MaterialClass::PittedConcrete } // never clean metal near mud
            } else if ctx.indoor {
                if r < 0.5 { MaterialClass::CleanMetal } else { MaterialClass::PittedConcrete }
            } else {
                if r < 0.4 { MaterialClass::Sandbag } else { MaterialClass::PittedConcrete }
            }
        }
        Role::HighCover | Role::Channel | Role::Conceal => {
            if ctx.wet {
                MaterialClass::PittedConcrete
            } else if ctx.indoor {
                if r < 0.5 { MaterialClass::CleanMetal } else { MaterialClass::PittedConcrete }
            } else {
                if r < 0.5 { MaterialClass::Granite } else { MaterialClass::PittedConcrete }
            }
        }
        Role::Expose => {
            if ctx.indoor {
                MaterialClass::CleanMetal
            } else if ctx.wet {
                MaterialClass::PittedConcrete
            } else {
                MaterialClass::RustedSteel
            }
        }
        Role::Elevate => {
            if ctx.indoor {
                MaterialClass::CleanMetal
            } else if ctx.wet {
                MaterialClass::RottenWood
            } else {
                if r < 0.5 { MaterialClass::RottenWood } else { MaterialClass::PittedConcrete }
            }
        }
    }
}

/// An abstract tagged volume (footprint in world XZ + extents), dressed at build time.
#[derive(Clone, Copy)]
pub struct Block {
    pub role: Role,
    pub x: f32,
    pub z: f32,
    pub sx: f32,
    pub sy: f32,
    pub sz: f32,
    pub rot: f32,
    pub seed: u32,
}

/// Geometry + collision produced by dressing.
#[derive(Default)]
pub struct Dressing {
    pub meshes: Vec<Mesh3>,
    pub obstacles: Vec<Obstacle>,
}
impl Dressing {
    fn absorb(&mut self, o: Dressing) {
        self.meshes.extend(o.meshes);
        self.obstacles.extend(o.obstacles);
    }
    pub fn tri_count(&self) -> usize {
        self.meshes.iter().map(|m| m.tri_count()).sum()
    }
}

fn darken(c: Color32, f: f32) -> Color32 {
    lerp_c(c, rgb(0, 0, 0), f)
}

impl Block {
    pub fn dress(&self, ctx: &Context) -> Dressing {
        let mat = material_for(self.role, ctx, self.seed);
        let base = mat.color(&ctx.pal);
        let gy = ctx.ground_y;
        let c = Vec3::new(self.x, gy, self.z);
        let mut solid = Mesh3::new(base);
        let mut detail = Mesh3::new(darken(base, 0.3));
        let mut obs = Vec::new();
        match self.role {
            Role::LowCover => {
                let h = self.sy.clamp(0.4, 1.0);
                let mut b = boxx(Vec3::new(c.x, gy + h, c.z), Vec3::new(self.sx, h, self.sz), base, FACE_ALL);
                weather(&mut b, mat, gy, gy + h * 2.0, ctx.wet);
                solid.merge(&b);
                obs.push(Obstacle { center: Vec3::new(c.x, gy + h, c.z), half: Vec3::new(self.sx, h, self.sz) });
            }
            Role::HighCover | Role::Channel => {
                let h = self.sy.clamp(1.2, 2.4);
                let mut b = boxx(Vec3::new(c.x, gy + h, c.z), Vec3::new(self.sx, h, self.sz), base, FACE_ALL);
                weather(&mut b, mat, gy, gy + h * 2.0, ctx.wet);
                solid.merge(&b);
                // a breach in a wall section spawns perpendicular rebar + a dust skirt (geometry-bound decay)
                if matches!(mat, MaterialClass::PittedConcrete) && hash01(self.seed as i32, 3) < 0.4 {
                    let bx = c.x;
                    let bz = c.z;
                    for k in 0..4 {
                        let off = -0.4 + k as f32 * 0.27;
                        detail.merge(&tube(Vec3::new(bx, gy + h * 0.6, bz + off), Vec3::new(bx + self.sx * 0.6, gy + h * 0.6 + 0.3, bz + off), 0.03, 3, rgb(70, 60, 50)));
                    }
                }
                obs.push(Obstacle { center: Vec3::new(c.x, gy + h, c.z), half: Vec3::new(self.sx, h, self.sz) });
            }
            Role::Conceal => {
                let h = self.sy.clamp(1.6, 3.2);
                let mut b = boxx(Vec3::new(c.x, gy + h, c.z), Vec3::new(self.sx, h, self.sz), base, FACE_NO_BOTTOM);
                weather(&mut b, mat, gy, gy + h * 2.0, ctx.wet);
                solid.merge(&b);
                obs.push(Obstacle { center: Vec3::new(c.x, gy + h, c.z), half: Vec3::new(self.sx, h, self.sz) });
            }
            Role::Block => {
                // an elongated ellipsoid wreck resting at an angle, severed at the midpoint (sightline breaker)
                let mut hull = uv_sphere(Vec3::new(c.x, gy + self.sy * 0.7, c.z), 1.0, 10, 6, base);
                for v in &mut hull.verts {
                    let p = *v - Vec3::new(c.x, gy + self.sy * 0.7, c.z);
                    *v = Vec3::new(c.x + p.x * self.sx, gy + self.sy * 0.7 + p.y * self.sy, c.z + p.z * self.sz);
                }
                weather(&mut hull, mat, gy, gy + self.sy * 2.0, ctx.wet);
                solid.merge(&hull);
                // environmental reaction: an oil-slick decal under it + scattered fragment shards
                detail.merge(&{
                    let mut slick = Mesh3::new(rgb(20, 18, 22));
                    slick.push_quad(
                        Vec3::new(c.x - self.sx * 1.2, gy + 0.03, c.z - self.sz * 1.2),
                        Vec3::new(c.x + self.sx * 1.2, gy + 0.03, c.z - self.sz * 1.2),
                        Vec3::new(c.x + self.sx * 1.2, gy + 0.03, c.z + self.sz * 1.2),
                        Vec3::new(c.x - self.sx * 1.2, gy + 0.03, c.z + self.sz * 1.2),
                        Vec3::UP,
                        rgb(22, 20, 24),
                    );
                    slick
                });
                for k in 0..8 {
                    let a = hash01(k, self.seed as i32) * std::f32::consts::TAU;
                    let d = self.sx * (1.0 + hash01(k, 9) * 1.5);
                    detail.merge(&boxx(Vec3::new(c.x + a.cos() * d, gy + 0.05, c.z + a.sin() * d), Vec3::splat(0.12), rgb(40, 40, 44), FACE_ALL));
                }
                obs.push(Obstacle { center: Vec3::new(c.x, gy + self.sy, c.z), half: Vec3::new(self.sx * 1.1, self.sy, self.sz * 1.1) });
            }
            Role::Expose => {
                // tall thin cylinder topped by a small cube — vertical exposure
                let h = self.sy.clamp(3.0, 8.0);
                let r = self.sx.clamp(0.4, 1.2);
                let mut col = tube(Vec3::new(c.x, gy, c.z), Vec3::new(c.x, gy + h, c.z), r, 8, base);
                weather(&mut col, mat, gy, gy + h, ctx.wet);
                solid.merge(&col);
                solid.merge(&boxx(Vec3::new(c.x, gy + h + 0.3, c.z), Vec3::new(r * 1.4, 0.3, r * 1.4), darken(base, 0.15), FACE_ALL));
                obs.push(Obstacle { center: Vec3::new(c.x, gy + h * 0.5, c.z), half: Vec3::new(r, h * 0.5, r) });
            }
            Role::Elevate => {
                // a raised platform on legs you can stand on
                let h = self.sy.clamp(1.0, 3.0);
                solid.merge(&boxx(Vec3::new(c.x, gy + h, c.z), Vec3::new(self.sx, 0.15, self.sz), base, FACE_ALL));
                for &(lx, lz) in &[(-1.0_f32, -1.0_f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let px = c.x + lx * (self.sx - 0.2);
                    let pz = c.z + lz * (self.sz - 0.2);
                    detail.merge(&tube(Vec3::new(px, gy, pz), Vec3::new(px, gy + h, pz), 0.12, 4, darken(base, 0.3)));
                }
                obs.push(Obstacle { center: Vec3::new(c.x, gy + h * 0.5, c.z), half: Vec3::new(self.sx, h * 0.5, self.sz) });
            }
        }
        // Texture the box-built cover with its class's atlas material — walls, cover, and platforms have real
        // tiling UVs from boxx, so they can carry a surface now instead of a flat tint. The class colour rides
        // on as a biome tint multiplied over the texel. Sphere/tube roles (Block, Expose) have no UVs, so they
        // keep the flat colour rather than sampling one arbitrary atlas texel.
        if matches!(self.role, Role::LowCover | Role::HighCover | Role::Channel | Role::Conceal | Role::Elevate) {
            solid.material = Some(mat.material());
        }
        let mut out = Dressing { meshes: vec![solid], obstacles: obs };
        if detail.tri_count() > 0 {
            out.meshes.push(detail);
        }
        out
    }
}

/// Geometry-bound weathering: rust/rot streaks down the sides, moss/wet darkening at the base.
fn weather(m: &mut Mesh3, mat: MaterialClass, lo: f32, hi: f32, wet: bool) {
    let d = mat.decay();
    let col = m.color;
    let top = lerp_c(col, rgb(255, 255, 255), 0.08 * (1.0 - d));
    let mut base_col = darken(col, 0.25 + 0.4 * d);
    if wet {
        base_col = lerp_c(base_col, rgb(54, 74, 50), 0.4); // moss/algae at the damp base
    }
    if matches!(mat, MaterialClass::RustedSteel) {
        base_col = lerp_c(base_col, rgb(150, 80, 36), 0.5); // weeping rust
    }
    tint_height(m, lo, hi, base_col, top);
}

/// The fluent block-out composer: place tagged volumes (the chessboard), then dress them all in context.
#[derive(Default)]
pub struct Blockout {
    blocks: Vec<Block>,
}

impl Blockout {
    pub fn new() -> Self {
        Blockout { blocks: Vec::new() }
    }
    pub fn place(&mut self, role: Role, x: f32, z: f32, sx: f32, sy: f32, sz: f32, seed: u32) -> &mut Self {
        self.blocks.push(Block { role, x, z, sx, sy, sz, rot: 0.0, seed });
        self
    }
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
    pub fn dress(&self, ctx: &Context) -> Dressing {
        let mut out = Dressing::default();
        for b in &self.blocks {
            out.absorb(b.dress(ctx));
        }
        out
    }

    /// Dress each block against its *own* surroundings: `ctx_at(x, z)` returns `(wet, ground_y)` sampled at the
    /// block's footprint, so a crate sitting in mud rots while the same crate on dry concrete stays clean. This
    /// is the essay's contextual binding made local — material follows the shape's relationship to its place.
    pub fn dress_each(&self, pal: Palette, indoor: bool, ctx_at: impl Fn(f32, f32) -> (bool, f32)) -> Dressing {
        let mut out = Dressing::default();
        for b in &self.blocks {
            let (wet, ground_y) = ctx_at(b.x, b.z);
            out.absorb(b.dress(&Context { pal, wet, indoor, ground_y }));
        }
        out
    }

    /// The essay's worked example: a crashed-vehicle chokepoint — an elongated Block breaking the lane,
    /// flanked by two Expose towers forming a canyon, with LowCover staggered along the approach.
    pub fn crashed_chokepoint(cx: f32, cz: f32, seed: u32) -> Blockout {
        let mut b = Blockout::new();
        b.place(Role::Block, cx, cz, 3.0, 1.4, 1.2, seed) // the wreck across the lane
            .place(Role::Expose, cx - 6.0, cz, 0.9, 6.0, 0.9, seed ^ 1) // canyon wall L
            .place(Role::Expose, cx + 6.0, cz, 0.9, 6.0, 0.9, seed ^ 2) // canyon wall R
            .place(Role::LowCover, cx - 3.0, cz - 4.0, 0.8, 0.6, 0.8, seed ^ 3)
            .place(Role::LowCover, cx + 3.0, cz + 4.0, 0.8, 0.6, 0.8, seed ^ 4);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::MapTheme;

    fn ctx(wet: bool, indoor: bool) -> Context {
        Context { pal: MapTheme::Urban.palette(), wet, indoor, ground_y: 0.0 }
    }

    #[test]
    fn never_clean_metal_in_the_mud() {
        // the binding rule the essay exists to enforce: low cover beside mud is never pristine metal
        for s in 0..64u32 {
            let m = material_for(Role::LowCover, &ctx(true, false), s);
            assert_ne!(m, MaterialClass::CleanMetal, "low cover near mud must not be clean metal");
            assert!(matches!(m, MaterialClass::RottenWood | MaterialClass::PittedConcrete));
        }
    }

    #[test]
    fn indoor_dry_favours_built_materials() {
        let mut metal_or_concrete = 0;
        for s in 0..64u32 {
            let m = material_for(Role::LowCover, &ctx(false, true), s);
            if matches!(m, MaterialClass::CleanMetal | MaterialClass::PittedConcrete) {
                metal_or_concrete += 1;
            }
        }
        assert_eq!(metal_or_concrete, 64, "indoor dry cover should be metal/concrete, not wood/sandbag");
    }

    #[test]
    fn every_role_dresses_into_geometry_and_collision() {
        let c = ctx(false, false);
        for role in Role::ALL {
            let blk = Block { role, x: 0.0, z: 0.0, sx: 1.0, sy: 1.5, sz: 1.0, rot: 0.0, seed: 5 };
            let d = blk.dress(&c);
            assert!(d.tri_count() > 0, "{} produced no geometry", role.name());
            assert!(!d.obstacles.is_empty(), "{} produced no collision", role.name());
        }
    }

    #[test]
    fn chokepoint_recipe_builds_canyon() {
        let bo = Blockout::crashed_chokepoint(0.0, 0.0, 9);
        assert_eq!(bo.block_count(), 5);
        let d = bo.dress(&ctx(false, false));
        // the wreck is a sightline breaker → rusted steel outdoors, plus oil-slick + shards
        assert!(d.tri_count() > 100);
        assert!(d.obstacles.len() >= 5);
    }

    #[test]
    fn wet_block_is_rusted_not_clean() {
        assert_eq!(material_for(Role::Block, &ctx(true, false), 0), MaterialClass::RustedSteel);
    }
}
