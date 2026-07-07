//! area.rs — tile-area asset groups: a high-level building-block builder for composing large, detailed maps.
//!
//! Instead of scattering individual props, you author a map as a handful of **areas** — named region templates
//! (a grove, a rock field, a ruin cluster, a service yard…) each of which, given a world-space footprint and a
//! seed, stamps a dense, internally-varied cluster of assets by pulling many procedural calls. Compose them
//! with a fluent builder:
//!
//! ```ignore
//! let out = AreaComposer::new()
//!     .place(AreaKind::Grove, (0.0, 0.0, 60.0, 60.0), 1)
//!     .place(AreaKind::Clearing, (60.0, 0.0, 100.0, 40.0), 2)
//!     .scatter(AreaKind::RockField, (0.0, 60.0, 100.0, 120.0), 4, 3)
//!     .build(&ctx);
//! ```
//!
//! Each area merges its instances into a few material meshes (bark / foliage / stone / props / glow) so the
//! result stays cheap to chunk and render. Everything is seed-deterministic, so the same composition rebuilds
//! identically while different seeds give wildly different detail.

use crate::level::{Obstacle, Palette};
use crate::math::Vec3;
use crate::mesh::{boxx, cairn, crystal, leaf_cluster, lerp_c, rgb, rock, rotate_y_about, tree, tube, Mesh3, FACE_ALL, FACE_NO_BOTTOM};
use egui::Color32;

/// A tiny deterministic RNG (xorshift) so areas vary richly but reproducibly from their seed.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
    fn u(&mut self) -> u32 {
        self.next() as u32
    }
}

/// The kinds of building block you can stamp. Each pulls a different mix of assets + variation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AreaKind {
    Grove,
    Thicket,
    Clearing,
    RockField,
    RuinCluster,
    ServiceYard,
    Orchard,
    Campsite,
}

impl AreaKind {
    pub const ALL: [AreaKind; 8] = [
        AreaKind::Grove,
        AreaKind::Thicket,
        AreaKind::Clearing,
        AreaKind::RockField,
        AreaKind::RuinCluster,
        AreaKind::ServiceYard,
        AreaKind::Orchard,
        AreaKind::Campsite,
    ];
    pub fn name(self) -> &'static str {
        match self {
            AreaKind::Grove => "Grove",
            AreaKind::Thicket => "Thicket",
            AreaKind::Clearing => "Clearing",
            AreaKind::RockField => "Rock Field",
            AreaKind::RuinCluster => "Ruin Cluster",
            AreaKind::ServiceYard => "Service Yard",
            AreaKind::Orchard => "Orchard",
            AreaKind::Campsite => "Campsite",
        }
    }
}

/// Context an area builds against: palette + ground height + whether it sits under open sky.
pub struct AreaCtx {
    pub pal: Palette,
    pub ground_y: f32,
}

/// The geometry + collision an area produced. Merged into a few material meshes to keep mesh count low.
#[derive(Default)]
pub struct AreaOutput {
    pub meshes: Vec<Mesh3>,
    pub obstacles: Vec<Obstacle>,
}

impl AreaOutput {
    pub fn absorb(&mut self, other: AreaOutput) {
        self.meshes.extend(other.meshes);
        self.obstacles.extend(other.obstacles);
    }
    pub fn tri_count(&self) -> usize {
        self.meshes.iter().map(|m| m.tri_count()).sum()
    }
}

/// One placed building block: a kind, a world-space footprint (x0,z0,x1,z1), a seed and a density multiplier.
#[derive(Clone, Copy)]
pub struct Area {
    pub kind: AreaKind,
    pub x0: f32,
    pub z0: f32,
    pub x1: f32,
    pub z1: f32,
    pub seed: u64,
    pub density: f32,
}

impl Area {
    fn w(&self) -> f32 {
        (self.x1 - self.x0).abs()
    }
    fn d(&self) -> f32 {
        (self.z1 - self.z0).abs()
    }
    fn area(&self) -> f32 {
        self.w() * self.d()
    }
    /// Random world point inside the footprint, inset by `m` so instances don't spill the edge.
    fn pt(&self, rng: &mut Rng, m: f32) -> (f32, f32) {
        (rng.range(self.x0 + m, self.x1 - m), rng.range(self.z0 + m, self.z1 - m))
    }

    /// Build this area into meshes + obstacles.
    pub fn build(&self, ctx: &AreaCtx) -> AreaOutput {
        let mut rng = Rng::new(self.seed ^ (self.kind as u64).wrapping_mul(0x100000001B3));
        match self.kind {
            AreaKind::Grove => self.grove(ctx, &mut rng),
            AreaKind::Thicket => self.thicket(ctx, &mut rng),
            AreaKind::Clearing => self.clearing(ctx, &mut rng),
            AreaKind::RockField => self.rock_field(ctx, &mut rng),
            AreaKind::RuinCluster => self.ruin_cluster(ctx, &mut rng),
            AreaKind::ServiceYard => self.service_yard(ctx, &mut rng),
            AreaKind::Orchard => self.orchard(ctx, &mut rng),
            AreaKind::Campsite => self.campsite(ctx, &mut rng),
        }
    }

    fn leaf_palette(&self, rng: &mut Rng) -> Color32 {
        let base = [rgb(54, 92, 46), rgb(70, 104, 52), rgb(46, 80, 58), rgb(96, 116, 60)];
        base[(rng.u() as usize) % base.len()]
    }

    fn grove(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut bark = Mesh3::new(rgb(86, 64, 46));
        let mut leaf = Mesh3::new(rgb(60, 96, 50));
        leaf.lod_far = 0.62;
        let n = ((self.area() / 42.0) * self.density).clamp(3.0, 52.0) as usize;
        let mut obstacles = Vec::new();
        let wood = lerp_c(ctx.pal.rock, rgb(96, 70, 48), 0.5);
        for _ in 0..n {
            let (x, z) = self.pt(rng, 2.0);
            let len = rng.range(4.5, 9.5);
            let radius = rng.range(0.28, 0.6);
            let lc = self.leaf_palette(rng);
            let parts = tree(Vec3::new(x, ctx.ground_y, z), len, radius, 3, rng.u(), wood, lc);
            for (i, p) in parts.into_iter().enumerate() {
                if i == 0 {
                    bark.merge(&p);
                } else {
                    leaf.merge(&p);
                }
            }
            obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + 1.2, z), half: Vec3::new(radius + 0.2, 1.2, radius + 0.2) });
        }
        // understory leaf clusters between the trunks
        let u = (n as f32 * 0.5) as usize;
        for _ in 0..u {
            let (x, z) = self.pt(rng, 1.0);
            let r = rng.range(0.5, 1.2);
            leaf.merge(&leaf_cluster(Vec3::new(x, ctx.ground_y + r * 0.7, z), r, rng.u(), self.leaf_palette(rng)));
        }
        AreaOutput { meshes: vec![bark, leaf], obstacles }
    }

    fn thicket(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut leaf = Mesh3::new(rgb(52, 84, 48));
        leaf.lod_far = 0.6;
        let mut stone = Mesh3::new(ctx.pal.rock);
        let n = ((self.area() / 9.0) * self.density).clamp(6.0, 160.0) as usize;
        for _ in 0..n {
            let (x, z) = self.pt(rng, 0.6);
            let r = rng.range(0.4, 1.1);
            leaf.merge(&leaf_cluster(Vec3::new(x, ctx.ground_y + r * 0.6, z), r, rng.u(), self.leaf_palette(rng)));
        }
        let boulders = ((self.area() / 120.0) * self.density).clamp(1.0, 14.0) as usize;
        let mut obstacles = Vec::new();
        for _ in 0..boulders {
            let (x, z) = self.pt(rng, 1.2);
            let r = rng.range(0.7, 1.6);
            stone.merge(&rock(Vec3::new(x, ctx.ground_y + r * 0.4, z), r, rng.u(), ctx.pal.rock));
            obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + r * 0.4, z), half: Vec3::new(r, r, r) });
        }
        AreaOutput { meshes: vec![leaf, stone], obstacles }
    }

    fn clearing(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut flora = Mesh3::new(rgb(70, 110, 56));
        flora.lod_far = 0.55;
        let mut glow = Mesh3::new(rgb(240, 210, 120)).emit(0.7);
        let n = ((self.area() / 7.0) * self.density).clamp(8.0, 200.0) as usize;
        for _ in 0..n {
            let (x, z) = self.pt(rng, 0.4);
            let r = rng.range(0.18, 0.4);
            flora.merge(&leaf_cluster(Vec3::new(x, ctx.ground_y + r * 0.8, z), r, rng.u(), rgb(74, 118, 58)));
            // the occasional flower
            if rng.f() < 0.18 {
                let col = [rgb(232, 120, 140), rgb(240, 210, 90), rgb(150, 130, 230), rgb(240, 240, 240)][(rng.u() as usize) % 4];
                glow.merge(&boxx(Vec3::new(x, ctx.ground_y + r + 0.12, z), Vec3::splat(0.07), col, FACE_ALL));
            }
        }
        AreaOutput { meshes: vec![flora, glow], obstacles: Vec::new() }
    }

    fn rock_field(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut stone = Mesh3::new(ctx.pal.rock);
        let mut glow = Mesh3::new(ctx.pal.accent).emit(0.8);
        let n = ((self.area() / 28.0) * self.density).clamp(3.0, 60.0) as usize;
        let mut obstacles = Vec::new();
        for _ in 0..n {
            let (x, z) = self.pt(rng, 1.0);
            let r = rng.range(0.6, 2.4);
            stone.merge(&rock(Vec3::new(x, ctx.ground_y + r * 0.3, z), r, rng.u(), ctx.pal.rock));
            obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + r * 0.3, z), half: Vec3::new(r, r, r) });
            if rng.f() < 0.25 {
                for c in cairn(Vec3::new(x + rng.range(-1.5, 1.5), ctx.ground_y, z + rng.range(-1.5, 1.5)), 3 + (rng.u() % 3) as usize, 0.5, rng.u(), ctx.pal.rock) {
                    stone.merge(&c);
                }
            }
            if rng.f() < 0.15 {
                glow.merge(&crystal(Vec3::new(x, ctx.ground_y, z), rng.range(0.8, 1.8), rng.range(0.2, 0.4), 5, rng.u(), ctx.pal.accent));
            }
        }
        AreaOutput { meshes: vec![stone, glow], obstacles }
    }

    fn ruin_cluster(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut wall = Mesh3::new(ctx.pal.wall);
        let mut rubble = Mesh3::new(lerp_c(ctx.pal.wall, ctx.pal.rock, 0.5));
        let n = ((self.area() / 60.0) * self.density).clamp(2.0, 30.0) as usize;
        let mut obstacles = Vec::new();
        for _ in 0..n {
            let (x, z) = self.pt(rng, 2.0);
            match rng.u() % 3 {
                0 => {
                    // a broken wall segment
                    let len = rng.range(2.0, 5.0);
                    let hgt = rng.range(1.2, 3.0);
                    let mut w = boxx(Vec3::new(x, ctx.ground_y + hgt, z), Vec3::new(len * 0.5, hgt, 0.4), ctx.pal.wall, FACE_ALL);
                    rotate_y_about(&mut w, Vec3::new(x, ctx.ground_y, z), rng.range(0.0, 3.14));
                    wall.merge(&w);
                    obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + hgt, z), half: Vec3::new(len * 0.5, hgt, 0.5) });
                }
                1 => {
                    // a toppled pillar
                    let l = rng.range(2.5, 5.0);
                    let a = Vec3::new(x, ctx.ground_y + 0.4, z);
                    let b = Vec3::new(x + rng.range(-l, l), ctx.ground_y + 0.4, z + rng.range(-l, l));
                    wall.merge(&tube(a, b, 0.35, 7, ctx.pal.trim));
                    obstacles.push(Obstacle { center: (a + b) * 0.5, half: Vec3::new(l * 0.5, 0.5, l * 0.5) });
                }
                _ => {
                    for _ in 0..3 {
                        let r = rng.range(0.3, 0.8);
                        rubble.merge(&rock(Vec3::new(x + rng.range(-1.5, 1.5), ctx.ground_y + r * 0.3, z + rng.range(-1.5, 1.5)), r, rng.u(), rubble.color));
                    }
                }
            }
        }
        AreaOutput { meshes: vec![wall, rubble], obstacles }
    }

    fn service_yard(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut props = Mesh3::new(ctx.pal.cover);
        let crate_cols = [rgb(150, 110, 60), rgb(70, 120, 140), rgb(120, 90, 70), rgb(90, 120, 90)];
        let n = ((self.area() / 18.0) * self.density).clamp(3.0, 80.0) as usize;
        let mut obstacles = Vec::new();
        for _ in 0..n {
            let (x, z) = self.pt(rng, 1.0);
            match rng.u() % 3 {
                0 => {
                    // crate stack
                    let h = rng.range(0.5, 0.8);
                    let col = crate_cols[(rng.u() as usize) % crate_cols.len()];
                    let stack = 1 + (rng.u() % 3);
                    for s in 0..stack {
                        let mut c = boxx(Vec3::new(x, ctx.ground_y + h + s as f32 * h * 2.0, z), Vec3::new(0.7, h, 0.7), col, FACE_ALL);
                        rotate_y_about(&mut c, Vec3::new(x, ctx.ground_y, z), rng.range(-0.4, 0.4));
                        props.merge(&c);
                    }
                    obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + h, z), half: Vec3::new(0.8, h * stack as f32, 0.8) });
                }
                1 => {
                    // a barrel
                    let h = rng.range(0.5, 0.8);
                    props.merge(&tube(Vec3::new(x, ctx.ground_y, z), Vec3::new(x, ctx.ground_y + h * 2.0, z), 0.36, 8, lerp_c(ctx.pal.cover, rgb(120, 90, 60), 0.5)));
                    obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + h, z), half: Vec3::new(0.4, h, 0.4) });
                }
                _ => {
                    // a shipping container
                    let hw = rng.range(1.6, 2.6);
                    let col = crate_cols[(rng.u() as usize) % crate_cols.len()];
                    let mut c = boxx(Vec3::new(x, ctx.ground_y + 1.1, z), Vec3::new(hw, 1.1, 1.1), col, FACE_NO_BOTTOM);
                    rotate_y_about(&mut c, Vec3::new(x, ctx.ground_y, z), rng.range(0.0, 1.57));
                    props.merge(&c);
                    obstacles.push(Obstacle { center: Vec3::new(x, ctx.ground_y + 1.1, z), half: Vec3::new(hw, 1.1, 1.3) });
                }
            }
        }
        AreaOutput { meshes: vec![props], obstacles }
    }

    fn orchard(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut bark = Mesh3::new(rgb(92, 68, 48));
        let mut leaf = Mesh3::new(rgb(64, 100, 52));
        leaf.lod_far = 0.62;
        let spacing = 4.5 / self.density.max(0.4);
        let wood = lerp_c(ctx.pal.rock, rgb(96, 70, 48), 0.4);
        let mut obstacles = Vec::new();
        let (mut z, mut row) = (self.z0 + spacing, 0u32);
        while z < self.z1 - spacing {
            let off = if row % 2 == 0 { 0.0 } else { spacing * 0.5 };
            let mut x = self.x0 + spacing + off;
            while x < self.x1 - spacing {
                let jx = x + rng.range(-0.4, 0.4);
                let jz = z + rng.range(-0.4, 0.4);
                let parts = tree(Vec3::new(jx, ctx.ground_y, jz), rng.range(4.0, 5.5), 0.3, 2, rng.u(), wood, self.leaf_palette(rng));
                for (i, p) in parts.into_iter().enumerate() {
                    if i == 0 {
                        bark.merge(&p);
                    } else {
                        leaf.merge(&p);
                    }
                }
                obstacles.push(Obstacle { center: Vec3::new(jx, ctx.ground_y + 1.0, jz), half: Vec3::new(0.4, 1.0, 0.4) });
                x += spacing;
            }
            z += spacing;
            row += 1;
        }
        AreaOutput { meshes: vec![bark, leaf], obstacles }
    }

    fn campsite(&self, ctx: &AreaCtx, rng: &mut Rng) -> AreaOutput {
        let mut props = Mesh3::new(ctx.pal.cover);
        let mut glow = Mesh3::new(rgb(255, 150, 60)).emit(1.0);
        let (cx, cz) = ((self.x0 + self.x1) * 0.5, (self.z0 + self.z1) * 0.5);
        // a central campfire
        for c in cairn(Vec3::new(cx, ctx.ground_y, cz), 5, 0.5, rng.u(), ctx.pal.rock) {
            props.merge(&c);
        }
        glow.merge(&crystal(Vec3::new(cx, ctx.ground_y + 0.2, cz), 0.6, 0.25, 5, rng.u(), rgb(255, 150, 60)));
        let mut obstacles = Vec::new();
        // tents (a ridged box) + crates ringing the fire
        let tents = 2 + (rng.u() % 3);
        for i in 0..tents {
            let a = i as f32 / tents as f32 * std::f32::consts::TAU;
            let (tx, tz) = (cx + a.cos() * rng.range(3.0, 5.0), cz + a.sin() * rng.range(3.0, 5.0));
            let mut t = boxx(Vec3::new(tx, ctx.ground_y + 0.7, tz), Vec3::new(1.1, 0.7, 0.8), lerp_c(ctx.pal.cover, rgb(120, 110, 80), 0.6), FACE_NO_BOTTOM);
            rotate_y_about(&mut t, Vec3::new(tx, ctx.ground_y, tz), a);
            props.merge(&t);
            obstacles.push(Obstacle { center: Vec3::new(tx, ctx.ground_y + 0.7, tz), half: Vec3::new(1.2, 0.7, 0.9) });
        }
        AreaOutput { meshes: vec![props, glow], obstacles }
    }
}

/// The fluent composer: place building blocks, then `build` them all into one merged output.
#[derive(Default)]
pub struct AreaComposer {
    areas: Vec<Area>,
}

impl AreaComposer {
    pub fn new() -> Self {
        AreaComposer { areas: Vec::new() }
    }

    /// Place one area (footprint as world-space `(x0, z0, x1, z1)`). Chainable.
    pub fn place(&mut self, kind: AreaKind, rect: (f32, f32, f32, f32), seed: u64) -> &mut Self {
        self.areas.push(Area { kind, x0: rect.0, z0: rect.1, x1: rect.2, z1: rect.3, seed, density: 1.0 });
        self
    }

    /// Place one area with an explicit density multiplier. Chainable.
    pub fn place_dense(&mut self, kind: AreaKind, rect: (f32, f32, f32, f32), seed: u64, density: f32) -> &mut Self {
        self.areas.push(Area { kind, x0: rect.0, z0: rect.1, x1: rect.2, z1: rect.3, seed, density });
        self
    }

    /// Scatter `n` smaller sub-areas of one kind across a region (each a random patch). Chainable.
    pub fn scatter(&mut self, kind: AreaKind, region: (f32, f32, f32, f32), n: usize, seed: u64) -> &mut Self {
        let mut rng = Rng::new(seed ^ 0xA5A5);
        for _ in 0..n {
            let w = (region.2 - region.0) * rng.range(0.18, 0.4);
            let d = (region.3 - region.1) * rng.range(0.18, 0.4);
            let x0 = rng.range(region.0, region.2 - w);
            let z0 = rng.range(region.1, region.3 - d);
            self.areas.push(Area { kind, x0, z0, x1: x0 + w, z1: z0 + d, seed: rng.next(), density: rng.range(0.7, 1.3) });
        }
        self
    }

    pub fn area_count(&self) -> usize {
        self.areas.len()
    }

    /// Build every placed area against `ctx` and merge into one output.
    pub fn build(&self, ctx: &AreaCtx) -> AreaOutput {
        let mut out = AreaOutput::default();
        for a in &self.areas {
            out.absorb(a.build(ctx));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::MapTheme;

    fn ctx() -> AreaCtx {
        AreaCtx { pal: MapTheme::Canopy.palette(), ground_y: 0.0 }
    }

    #[test]
    fn every_kind_builds_within_budget() {
        let c = ctx();
        for k in AreaKind::ALL {
            let a = Area { kind: k, x0: 0.0, z0: 0.0, x1: 40.0, z1: 40.0, seed: 7, density: 1.0 };
            let out = a.build(&c);
            let tris = out.tri_count();
            assert!(tris > 0, "{} produced nothing", k.name());
            assert!(tris < 200_000, "{} over budget ({tris})", k.name());
            assert!(out.meshes.len() <= 4, "{} should merge into a few material meshes", k.name());
        }
    }

    #[test]
    fn seed_changes_layout_but_not_kind() {
        let c = ctx();
        let mk = |s| Area { kind: AreaKind::Grove, x0: 0.0, z0: 0.0, x1: 50.0, z1: 50.0, seed: s, density: 1.0 }.build(&c);
        let a = mk(1);
        let b = mk(2);
        // different seeds → different obstacle counts or positions (not bit-identical)
        let same = a.obstacles.len() == b.obstacles.len()
            && a.obstacles.iter().zip(&b.obstacles).all(|(p, q)| (p.center - q.center).len() < 1e-3);
        assert!(!same, "different seeds should vary the grove");
    }

    #[test]
    fn instances_stay_inside_footprint() {
        let c = ctx();
        let a = Area { kind: AreaKind::ServiceYard, x0: 10.0, z0: 10.0, x1: 50.0, z1: 50.0, seed: 3, density: 1.5 };
        let out = a.build(&c);
        for o in &out.obstacles {
            assert!(o.center.x > 5.0 && o.center.x < 55.0, "x within padded footprint");
            assert!(o.center.z > 5.0 && o.center.z < 55.0, "z within padded footprint");
        }
    }

    #[test]
    fn composer_chains_and_merges() {
        let c = ctx();
        let mut comp = AreaComposer::new();
        comp.place(AreaKind::Grove, (0.0, 0.0, 40.0, 40.0), 1)
            .place(AreaKind::Clearing, (40.0, 0.0, 80.0, 40.0), 2)
            .scatter(AreaKind::RockField, (0.0, 40.0, 80.0, 90.0), 3, 9);
        assert_eq!(comp.area_count(), 5);
        let out = comp.build(&c);
        assert!(out.tri_count() > 0);
        assert!(!out.obstacles.is_empty());
    }

    #[test]
    fn density_scales_instance_count() {
        let c = ctx();
        let sparse = Area { kind: AreaKind::Thicket, x0: 0.0, z0: 0.0, x1: 60.0, z1: 60.0, seed: 5, density: 0.4 }.build(&c);
        let dense = Area { kind: AreaKind::Thicket, x0: 0.0, z0: 0.0, x1: 60.0, z1: 60.0, seed: 5, density: 2.0 }.build(&c);
        assert!(dense.tri_count() > sparse.tri_count(), "higher density → more detail");
    }
}
