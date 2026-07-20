//! The open world: gentle noise hills plus a seeded scatter of playful shapes
//! — sky rings to thread air dribbles through, floating balloons, half-buried
//! domes, tilted ramp slabs, hovering platforms, lollipop trees, and gold
//! boost pads. Every shape is both a mesh instance and an analytic SDF, so the
//! ball and car collide with exactly what you see.
//!
//! v1.1: sixteen selectable map styles, adapted from the logscope scene3d
//! ROOMS table (apps/logscope, same monorepo) — each style recolors the world,
//! reshapes the terrain, reweights the shape families and builds a different
//! centerpiece at spawn.

use glam::{Mat4, Quat, Vec2, Vec3};

// mesh registry slots filled in this order by main()
pub const M_SPHERE: usize = 0;
pub const M_BOX: usize = 1;
pub const M_TORUS_THIN: usize = 2;
pub const M_TORUS_FAT: usize = 3;
#[allow(dead_code)]
pub const M_CAP_SHORT: usize = 4; // capsule hh = 1.5 (in radius units)
pub const M_CAP_TALL: usize = 5; // capsule hh = 4.0
#[allow(dead_code)]
pub const M_PAD: usize = 6;
pub const M_TERRAIN: usize = 7;
pub const M_SHELL: usize = 8; // hollow tube (drive the inside)
pub const M_SHELL_LO: usize = 9; // low-poly far-field tube (LOD)
/// Terrain ground-truth grid: physics, placement, the render mesh and the
/// ray-marched shadows all sample THESE nodes, so the floor you see is the
/// floor you drive on (the analytic fbm is only sampled once, node-wise).
pub const TG_N: usize = 221; // 220 cells, matching meshgen::terrain(_, 220, _)
pub const TG_HALF: f32 = 1300.0;

pub const CAP_SHORT_HH: f32 = 1.5;
pub const CAP_TALL_HH: f32 = 4.0;

#[derive(Clone, Copy)]
pub enum Sdf {
    #[allow(dead_code)]
    None,
    Sphere { r: f32 },
    Torus { big: f32, tube: f32 },
    Capsule { r: f32, half: f32 },
    RBox { half: Vec3, round: f32 },
    /// Cylindrical shell along local Y: mean radius r, wall half-thickness
    /// `thick`, half-length `half`. Hollow — the marble run primitive.
    Shell { r: f32, half: f32, thick: f32 },
}

pub struct Shape {
    pub mesh: usize,
    pub mat: Mat4,
    pub pos: Vec3,
    pub inv_rot: Quat,
    pub sdf: Sdf,
    pub color: [f32; 4],
    pub emissive: f32,
    pub flash: f32,
    pub spec: f32,
    /// CSG cutter: subtracts from the union (carves holes) and is not drawn
    pub cut: bool,
    /// smooth-union group: tubes and junction rooms FILLET into each other
    /// (the CAD-machining blend) instead of hard-creasing
    pub blend: bool,
}

pub struct Ring {
    pub shape: usize,
    pub pos: Vec3,
    pub inv_rot: Quat,
    pub hole_r: f32,
}

/// A goal to shoot at: an open-mouthed pocket of frame bars + glowing back
/// panel. The ball scoring plane is the opening rectangle (local z = 0,
/// pocket toward -z). Nets come in every size, height and tilt.
/// A gravity node: near it, "down" stops being world -Y. Tube nodes pull
/// radially outward from the spine (drive the inside of the pipe); ribbon
/// nodes carry their own baked down. Blend per the design spec: 0.9 x the
/// nearest node + 0.1 x the runner-up, then faded into world gravity by
/// distance.
/// Animated world piece: world.update drives it, physics and rays follow
/// automatically because everything reads the live shape transform.
pub enum Anim {
    Spin { shape: usize, pos: Vec3, scale: Vec3, base: Quat, axis: Vec3, rate: f32 },
    Door { shape: usize, base: Vec3, rot: Quat, scale: Vec3, axis: Vec3, amp: f32, rate: f32, phase: f32 },
    /// circles a center in the plane perpendicular to axis
    Orbit { shape: usize, center: Vec3, axis: Vec3, r: f32, rate: f32, phase: f32, rot: Quat, scale: Vec3 },
    /// shuttles along dir (a moving stepping-stone)
    Slide { shape: usize, base: Vec3, dir: Vec3, amp: f32, rate: f32, phase: f32, rot: Quat, scale: Vec3 },
}

/// Field zones, pinball-machine style.
#[derive(Clone, Copy, PartialEq)]
pub enum ZoneKind {
    Vibe,  // jitters everything inside (bumper field)
    Slip,  // grip evaporates (ice patch)
    Boost, // free forward thrust (speed strip)
}

pub struct Zone {
    pub pos: Vec3,
    pub r: f32,
    pub kind: ZoneKind,
}

/// Blast-off pad: step on it, get launched along its arrow.
pub struct Launcher {
    pub pos: Vec3,
    pub dir: Vec3,
    pub power: f32,
    pub timer: f32,
    pub shape: usize,
}

/// A floor tile (DROPSHOT) or a door panel (GATEWORKS): ball hits open it.
#[derive(Clone, Copy, PartialEq)]
pub enum TileKind {
    Drop, // takes hp hits, then falls away for a while
    Door, // one solid hit slides it open for a few seconds
}

pub struct Tile {
    pub shape: usize,
    pub kind: TileKind,
    pub hp: u8,
    pub max_hp: u8,
    pub down_t: f32, // >0 = currently open
    pub base: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub keep: Sdf, // the sdf to restore on close
}

/// A pushable free body (KUIPER asteroids): drifts, collides, gets shoved.
pub struct Body {
    pub shape: usize,
    pub pos: Vec3,
    pub vel: Vec3,
    pub r: f32,
    pub mass: f32,
}

pub struct GravNode {
    pub pos: Vec3,
    pub down: Vec3,
    pub r: f32,
    pub tube: bool,
    /// ride an animated shape: pos syncs to shapes[i] every update
    pub follow: Option<usize>,
}

pub struct NetGoal {
    pub pos: Vec3,       // center of the opening
    #[allow(dead_code)]
    pub rot: Quat,
    pub inv_rot: Quat,
    pub hw: f32,
    pub hh: f32,
    #[allow(dead_code)]
    pub depth: f32,
    pub shape0: usize,   // shapes[shape0..shape0+nshapes] flash on a goal
    pub nshapes: usize,
    pub timer: f32,      // per-net rescore cooldown
}

pub struct Pad {
    pub pos: Vec3,
    pub big: bool,
    pub timer: f32,
    pub phase: f32,
}

pub struct World {
    pub grav: Vec<GravNode>,
    /// XZ spatial hash over shapes: sdf() walks one cell, not the world
    sgrid: Vec<Vec<u16>>,
    scell: f32,
    sdim: i32,
    /// drivable tube routes for bot pathing: (waypoints, radius)
    pub routes: Vec<(Vec<Vec3>, f32)>,
    pub route_names: Vec<String>,
    /// shape indices belonging to each route (waterline glow targets)
    pub route_shapes: Vec<Vec<u32>>,
    /// route indices that can FLOOD
    pub floodable: Vec<usize>,
    /// active floods: (route, age, duration)
    pub floods: Vec<(usize, f32, f32)>,
    pub tiles: Vec<Tile>,
    pub bodies: Vec<Body>,
    /// DROPSHOT: platform height — a ball below it scores by z sign
    pub drop_pit: Option<f32>,
    pit_cd: f32,
    flood_cd: f32,
    /// a flood just started on this route (main shows the toast)
    pub flood_news: Option<usize>,
    pub anims: Vec<Anim>,
    pub zones: Vec<Zone>,
    pub launchers: Vec<Launcher>,
    pub time: f32,
    pub tgrid: Vec<f32>,
    pub shapes: Vec<Shape>,
    pub rings: Vec<Ring>,
    pub nets: Vec<NetGoal>,
    pub pads: Vec<Pad>,
    pub decor_cars: Vec<Mat4>,
    #[allow(dead_code)] // map identity metadata; the grid is the live ground
    pub seed: u64,
    #[allow(dead_code)]
    pub t_amp: f32,
    #[allow(dead_code)]
    pub t_freq: f32,
}

// ------------------------------------------------------------- random ----
/// polynomial smooth-min: the SDF fillet
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b * (1.0 - h) + a * h - k * h * (1.0 - h)
}

pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_add(0x9E3779B97F4A7C15))
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
    pub fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + self.f() * (b - a)
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() as usize) % xs.len()]
    }
}

// -------------------------------------------------------------- noise ----
fn hash2(ix: i32, iz: i32, seed: u64) -> f32 {
    let mut h = (ix as u64).wrapping_mul(0x8DA6B343)
        ^ (iz as u64).wrapping_mul(0xD8163841)
        ^ seed.wrapping_mul(0x9E3779B9);
    h = (h ^ (h >> 13)).wrapping_mul(0x5BD1E995);
    ((h ^ (h >> 15)) & 0xFFFF) as f32 / 65535.0
}

fn value_noise(x: f32, z: f32, seed: u64) -> f32 {
    let (ix, iz) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - x.floor(), z - z.floor());
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sz = fz * fz * (3.0 - 2.0 * fz);
    let a = hash2(ix, iz, seed);
    let b = hash2(ix + 1, iz, seed);
    let c = hash2(ix, iz + 1, seed);
    let d = hash2(ix + 1, iz + 1, seed);
    let ab = a + (b - a) * sx;
    let cd = c + (d - c) * sx;
    (ab + (cd - ab) * sz) * 2.0 - 1.0
}

/// Terrain height with a style's amplitude/frequency scale: two octaves of
/// value noise, flattened around spawn so the dribbling lawn stays clean.
/// Bilinear sample of the ground-truth grid (edge-clamped).
pub fn grid_h(g: &[f32], x: f32, z: f32) -> f32 {
    let n = TG_N as f32 - 1.0;
    let fx = ((x / TG_HALF) * 0.5 + 0.5).clamp(0.0, 1.0) * n;
    let fz = ((z / TG_HALF) * 0.5 + 0.5).clamp(0.0, 1.0) * n;
    let (i0, j0) = (fx.floor() as usize, fz.floor() as usize);
    let (i1, j1) = ((i0 + 1).min(TG_N - 1), (j0 + 1).min(TG_N - 1));
    let (tx, tz) = (fx - i0 as f32, fz - j0 as f32);
    let a = g[j0 * TG_N + i0] * (1.0 - tx) + g[j0 * TG_N + i1] * tx;
    let b = g[j1 * TG_N + i0] * (1.0 - tx) + g[j1 * TG_N + i1] * tx;
    a * (1.0 - tz) + b * tz
}

pub fn terrain_h_s(x: f32, z: f32, seed: u64, amp: f32, freq: f32) -> f32 {
    let n = value_noise(x * 0.011 * freq, z * 0.011 * freq, seed) * 2.6
        + value_noise(x * 0.043 * freq, z * 0.043 * freq, seed.wrapping_add(77)) * 0.7;
    let d = (x * x + z * z).sqrt();
    let t = ((d - 32.0) / 60.0).clamp(0.0, 1.0);
    let flat = t * t * (3.0 - 2.0 * t);
    n * amp * flat
}

// ------------------------------------------------------------ palette ----
fn rgb(hex: u32) -> [f32; 4] {
    // sRGB-ish hex to linear-ish values (cheap gamma 2.2)
    let c = |v: u32| ((v & 255) as f32 / 255.0).powf(2.2);
    [c(hex >> 16), c(hex >> 8), c(hex), 1.0]
}
fn rgb3(hex: u32) -> [f32; 3] {
    let c = rgb(hex);
    [c[0], c[1], c[2]]
}
fn mix4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        1.0,
    ]
}

fn base_palette() -> Vec<[f32; 4]> {
    vec![
        rgb(0xFF8FAB), // coral pink
        rgb(0x7BE0AD), // mint
        rgb(0xFFD98A), // butter
        rgb(0x8FD3FE), // sky
        rgb(0xC9A7EB), // lilac
        rgb(0xFFB08A), // peach
        rgb(0x8AF0DF), // aqua
        rgb(0xF6A6FF), // bubblegum
    ]
}

pub const GOLD: [f32; 4] = [0.95, 0.66, 0.12, 1.0];
pub const PLINTH: [f32; 4] = [0.62, 0.60, 0.66, 1.0];

// --------------------------------------------------------- map styles ----
/// Focal centerpieces at spawn (adapted from ROOMS' focal column):
/// 0 plinth plaza (the classic), 1 obelisk, 2 fountain, 3 great tree,
/// 4 brazier, 5 lighthouse, 6 orrery, 7 colonnade ring.
pub struct MapStyle {
    pub name: &'static str,
    pub grass: [f32; 4],
    pub zenith: [f32; 3],
    pub fog: [f32; 3],
    pub fog_d: f32,
    pub sun: [f32; 3],
    pub accent: [f32; 4],
    pub pal_mix: f32,
    pub amp: f32,
    pub freq: f32,
    pub glow: f32, // extra emissive on rings/balloons
    pub wet: f32,  // ground mirror-wetness for the ray tracer
    pub rings: u32,
    pub halos: u32,
    pub balloons: u32,
    pub domes: u32,
    pub ramps: u32,
    pub platforms: u32,
    pub trees: u32,
    pub small_pads: u32,
    pub nets: u32,
    pub focal: u8,
}

pub const STYLE_COUNT: usize = 30;

/// Adapted from logscope scene3d's ROOMS table — same sixteen names, each
/// translated from an indoor room into an open-world mood.
pub fn styles() -> &'static [MapStyle; STYLE_COUNT] {
    use std::sync::OnceLock;
    static S: OnceLock<[MapStyle; STYLE_COUNT]> = OnceLock::new();
    S.get_or_init(|| {
        let g = |hex: u32| {
            let c = rgb(hex);
            [c[0], c[1], c[2], 1.0]
        };
        [
            MapStyle { name: "MEADOW", grass: [0.30, 0.52, 0.28, 1.0], zenith: rgb3(0x66ADFF), fog: rgb3(0xFFB8CC), fog_d: 0.0011, sun: [0.45, 0.75, 0.35], accent: g(0xFFD98A), pal_mix: 0.0, amp: 1.0, freq: 1.0, wet: 0.08, glow: 0.0, rings: 26, halos: 6, balloons: 30, domes: 14, ramps: 16, platforms: 20, trees: 22, small_pads: 44, nets: 2, focal: 0 },
            MapStyle { name: "CATHEDRAL", grass: g(0x8D8776), zenith: rgb3(0xBFD2E8), fog: rgb3(0xE8DFC8), fog_d: 0.0013, sun: [0.3, 0.8, 0.4], accent: g(0xFFE6B0), pal_mix: 0.45, amp: 0.8, freq: 0.9, wet: 0.14, glow: 0.1, rings: 22, halos: 12, balloons: 10, domes: 8, ramps: 12, platforms: 16, trees: 6, small_pads: 40, nets: 0, focal: 7 },
            MapStyle { name: "OBSERVATORY", grass: g(0x1A2030), zenith: rgb3(0x060A18), fog: rgb3(0x233052), fog_d: 0.0016, sun: [-0.4, 0.35, 0.5], accent: g(0x88C6FF), pal_mix: 0.6, amp: 1.0, freq: 1.0, wet: 0.50, glow: 0.5, rings: 34, halos: 8, balloons: 20, domes: 8, ramps: 10, platforms: 18, trees: 0, small_pads: 44, nets: 1, focal: 6 },
            MapStyle { name: "GREENHOUSE", grass: g(0x37522C), zenith: rgb3(0x9FE3C8), fog: rgb3(0xD9F0DC), fog_d: 0.0012, sun: [0.4, 0.8, 0.2], accent: g(0x9AE060), pal_mix: 0.5, amp: 0.9, freq: 1.2, wet: 0.10, glow: 0.05, rings: 22, halos: 5, balloons: 24, domes: 10, ramps: 10, platforms: 22, trees: 40, small_pads: 48, nets: 1, focal: 3 },
            MapStyle { name: "CRYPT", grass: g(0x232028), zenith: rgb3(0x12101A), fog: rgb3(0x2C2836), fog_d: 0.0022, sun: [-0.2, 0.4, -0.6], accent: g(0x6A90A0), pal_mix: 0.65, amp: 1.1, freq: 1.1, wet: 0.30, glow: 0.4, rings: 20, halos: 4, balloons: 34, domes: 16, ramps: 12, platforms: 14, trees: 4, small_pads: 40, nets: 0, focal: 1 },
            MapStyle { name: "SCRIPTORIUM", grass: g(0x4A3822), zenith: rgb3(0xE8D8B8), fog: rgb3(0xEFDFBE), fog_d: 0.0012, sun: [0.5, 0.6, 0.3], accent: g(0xFFD080), pal_mix: 0.5, amp: 0.7, freq: 0.9, wet: 0.10, glow: 0.1, rings: 22, halos: 6, balloons: 18, domes: 6, ramps: 14, platforms: 28, trees: 8, small_pads: 44, nets: 0, focal: 0 },
            MapStyle { name: "FOUNDRY", grass: g(0x2A2220), zenith: rgb3(0x3A2418), fog: rgb3(0x53301E), fog_d: 0.0018, sun: [-0.5, 0.45, 0.2], accent: g(0xFF7A2A), pal_mix: 0.6, amp: 1.2, freq: 1.3, wet: 0.18, glow: 0.45, rings: 24, halos: 5, balloons: 14, domes: 10, ramps: 26, platforms: 18, trees: 0, small_pads: 48, nets: 2, focal: 4 },
            MapStyle { name: "ZEN GARDEN", grass: g(0xCFC6AE), zenith: rgb3(0xBFE0EA), fog: rgb3(0xEFE9DA), fog_d: 0.0009, sun: [0.45, 0.75, 0.35], accent: g(0x86B086), pal_mix: 0.55, amp: 0.35, freq: 0.7, wet: 0.06, glow: 0.0, rings: 16, halos: 4, balloons: 8, domes: 26, ramps: 6, platforms: 10, trees: 12, small_pads: 36, nets: 0, focal: 2 },
            MapStyle { name: "LIGHTHOUSE", grass: g(0x2C3A42), zenith: rgb3(0x4C6E86), fog: rgb3(0x93A8B4), fog_d: 0.0022, sun: [0.6, 0.4, 0.4], accent: g(0xFFD27A), pal_mix: 0.45, amp: 1.6, freq: 1.1, wet: 0.40, glow: 0.25, rings: 24, halos: 6, balloons: 16, domes: 12, ramps: 14, platforms: 16, trees: 6, small_pads: 42, nets: 1, focal: 5 },
            MapStyle { name: "TEMPLE", grass: g(0x524630), zenith: rgb3(0xF0D8A8), fog: rgb3(0xE8C890), fog_d: 0.0013, sun: [0.55, 0.55, 0.25], accent: g(0xFFB050), pal_mix: 0.5, amp: 0.9, freq: 0.8, wet: 0.10, glow: 0.15, rings: 24, halos: 7, balloons: 12, domes: 10, ramps: 24, platforms: 16, trees: 10, small_pads: 44, nets: 2, focal: 7 },
            MapStyle { name: "CONSERVATORY", grass: g(0x4A3A26), zenith: rgb3(0xCFE8D8), fog: rgb3(0xE6E0C8), fog_d: 0.0012, sun: [0.35, 0.75, 0.35], accent: g(0xFFD6A0), pal_mix: 0.4, amp: 0.8, freq: 1.0, wet: 0.12, glow: 0.1, rings: 24, halos: 6, balloons: 20, domes: 8, ramps: 12, platforms: 28, trees: 30, small_pads: 46, nets: 1, focal: 0 },
            MapStyle { name: "CAVERN", grass: g(0x20242A), zenith: rgb3(0x0A0E12), fog: rgb3(0x1C2830), fog_d: 0.0026, sun: [-0.3, 0.5, -0.4], accent: g(0x70D0C0), pal_mix: 0.7, amp: 1.6, freq: 1.4, wet: 0.45, glow: 0.55, rings: 22, halos: 4, balloons: 30, domes: 30, ramps: 12, platforms: 16, trees: 0, small_pads: 44, nets: 1, focal: 1 },
            MapStyle { name: "PLANETARIUM", grass: g(0x141628), zenith: rgb3(0x080614), fog: rgb3(0x1E1A36), fog_d: 0.0014, sun: [-0.5, 0.3, 0.6], accent: g(0xC0B0FF), pal_mix: 0.65, amp: 0.4, freq: 0.8, wet: 0.55, glow: 0.6, rings: 40, halos: 10, balloons: 18, domes: 6, ramps: 8, platforms: 16, trees: 0, small_pads: 44, nets: 2, focal: 6 },
            MapStyle { name: "DOJO", grass: g(0x5A4228), zenith: rgb3(0xE8E0C8), fog: rgb3(0xE0D2B0), fog_d: 0.0011, sun: [0.4, 0.7, 0.3], accent: g(0xFFE0A0), pal_mix: 0.5, amp: 0.45, freq: 0.9, wet: 0.16, glow: 0.1, rings: 20, halos: 6, balloons: 10, domes: 8, ramps: 24, platforms: 24, trees: 10, small_pads: 42, nets: 2, focal: 4 },
            MapStyle { name: "VAULT", grass: g(0x2A2E32), zenith: rgb3(0x38404A), fog: rgb3(0x4C565E), fog_d: 0.0016, sun: [0.2, 0.6, -0.5], accent: g(0x9AC0D0), pal_mix: 0.6, amp: 0.8, freq: 1.2, wet: 0.28, glow: 0.2, rings: 22, halos: 5, balloons: 12, domes: 8, ramps: 24, platforms: 30, trees: 0, small_pads: 42, nets: 1, focal: 1 },
            MapStyle { name: "HEARTH HALL", grass: g(0x4A3A2A), zenith: rgb3(0x5A3C28), fog: rgb3(0x6E4630), fog_d: 0.0017, sun: [-0.45, 0.5, 0.3], accent: g(0xFF9A40), pal_mix: 0.55, amp: 0.9, freq: 1.0, wet: 0.12, glow: 0.35, rings: 22, halos: 6, balloons: 22, domes: 10, ramps: 14, platforms: 20, trees: 14, small_pads: 44, nets: 1, focal: 4 },
            MapStyle { name: "STADIUM", grass: g(0x2E7A34), zenith: rgb3(0x8FD3FE), fog: rgb3(0xDCEFE0), fog_d: 0.0009, sun: [0.4, 0.8, 0.3], accent: g(0xFFFFFF), pal_mix: 0.25, amp: 0.25, freq: 0.7, wet: 0.10, glow: 0.10, rings: 14, halos: 4, balloons: 10, domes: 4, ramps: 10, platforms: 10, trees: 8, small_pads: 48, nets: 2, focal: 8 },
            MapStyle { name: "SKY GOALS", grass: g(0x3C6B4A), zenith: rgb3(0x79B8FF), fog: rgb3(0xEFEAD8), fog_d: 0.0010, sun: [0.5, 0.7, 0.3], accent: g(0xFFD24A), pal_mix: 0.4, amp: 1.1, freq: 1.0, wet: 0.10, glow: 0.20, rings: 18, halos: 8, balloons: 18, domes: 8, ramps: 12, platforms: 26, trees: 8, small_pads: 46, nets: 9, focal: 0 },
            MapStyle { name: "CARNIVAL", grass: g(0x6E3B7A), zenith: rgb3(0xFFB8E0), fog: rgb3(0xFFD9EC), fog_d: 0.0013, sun: [0.3, 0.7, 0.4], accent: g(0xFF5FA8), pal_mix: 0.55, amp: 0.8, freq: 1.3, wet: 0.18, glow: 0.45, rings: 24, halos: 6, balloons: 42, domes: 8, ramps: 16, platforms: 18, trees: 10, small_pads: 50, nets: 6, focal: 2 },
            MapStyle { name: "AURORA", grass: g(0x0E2226), zenith: rgb3(0x06121E), fog: rgb3(0x14343A), fog_d: 0.0015, sun: [-0.4, 0.35, 0.5], accent: g(0x5FF0C8), pal_mix: 0.65, amp: 1.3, freq: 0.9, wet: 0.50, glow: 0.55, rings: 26, halos: 6, balloons: 20, domes: 12, ramps: 10, platforms: 18, trees: 0, small_pads: 44, nets: 3, focal: 6 },
            MapStyle { name: "ARENA CLASSIC", grass: g(0x2C7A38), zenith: rgb3(0x9AD6FF), fog: rgb3(0xD8E8F0), fog_d: 0.0008, sun: [0.35, 0.85, 0.25], accent: g(0xF2F2F2), pal_mix: 0.2, amp: 0.02, freq: 1.0, wet: 0.14, glow: 0.12, rings: 6, halos: 2, balloons: 4, domes: 0, ramps: 0, platforms: 2, trees: 0, small_pads: 40, nets: 0, focal: 9 },
            MapStyle { name: "PERPLEXUS", grass: g(0x1A1830), zenith: rgb3(0x100E24), fog: rgb3(0x241F44), fog_d: 0.0013, sun: [-0.3, 0.5, 0.4], accent: g(0x7FE8FF), pal_mix: 0.6, amp: 0.5, freq: 0.8, wet: 0.35, glow: 0.6, rings: 16, halos: 4, balloons: 10, domes: 4, ramps: 6, platforms: 10, trees: 0, small_pads: 40, nets: 2, focal: 11 },
            MapStyle { name: "RIBBONWORKS", grass: g(0x3A4A5E), zenith: rgb3(0xFFC98A), fog: rgb3(0xE8C9A8), fog_d: 0.0010, sun: [0.5, 0.45, 0.3], accent: g(0xFF7A5C), pal_mix: 0.5, amp: 0.7, freq: 0.9, wet: 0.20, glow: 0.35, rings: 20, halos: 6, balloons: 14, domes: 6, ramps: 8, platforms: 14, trees: 4, small_pads: 42, nets: 3, focal: 12 },
            MapStyle { name: "COLISEUM", grass: g(0xB89B62), zenith: rgb3(0xFFE3B0), fog: rgb3(0xEAD2A0), fog_d: 0.0012, sun: [0.55, 0.5, 0.2], accent: g(0xFF9A3C), pal_mix: 0.45, amp: 0.45, freq: 0.8, wet: 0.08, glow: 0.25, rings: 18, halos: 4, balloons: 8, domes: 6, ramps: 12, platforms: 8, trees: 4, small_pads: 44, nets: 2, focal: 10 },
            MapStyle { name: "MONKEY RUN", grass: g(0x27431F), zenith: rgb3(0x2E4A66), fog: rgb3(0x3A5A4A), fog_d: 0.0016, sun: [0.35, 0.45, 0.15], accent: g(0xFFD34D), pal_mix: 0.5, amp: 1.0, freq: 1.1, wet: 0.18, glow: 0.35, rings: 8, halos: 3, balloons: 6, domes: 0, ramps: 6, platforms: 8, trees: 60, small_pads: 36, nets: 2, focal: 13 },
            MapStyle { name: "BOXBOWL", grass: g(0x2B4A2A), zenith: rgb3(0x8FC1FF), fog: rgb3(0xD8E6F5), fog_d: 0.0009, sun: [0.4, 0.8, 0.3], accent: g(0xFF8A3C), pal_mix: 0.35, amp: 0.0, freq: 1.0, wet: 0.20, glow: 0.15, rings: 0, halos: 2, balloons: 0, domes: 0, ramps: 0, platforms: 0, trees: 0, small_pads: 34, nets: 2, focal: 14 },
            MapStyle { name: "DROPSHOT", grass: g(0x241B2E), zenith: rgb3(0x140F1E), fog: rgb3(0x2E2440), fog_d: 0.0013, sun: [-0.2, 0.6, 0.4], accent: g(0xFF5A78), pal_mix: 0.55, amp: 0.1, freq: 1.0, wet: 0.30, glow: 0.5, rings: 0, halos: 3, balloons: 0, domes: 0, ramps: 0, platforms: 0, trees: 0, small_pads: 20, nets: 0, focal: 15 },
            MapStyle { name: "GATEWORKS", grass: g(0x33302A), zenith: rgb3(0xC9B98F), fog: rgb3(0xD9CBA5), fog_d: 0.0012, sun: [0.5, 0.6, 0.2], accent: g(0x74E0B0), pal_mix: 0.45, amp: 0.15, freq: 1.0, wet: 0.18, glow: 0.35, rings: 4, halos: 2, balloons: 0, domes: 0, ramps: 4, platforms: 4, trees: 0, small_pads: 28, nets: 2, focal: 16 },
            MapStyle { name: "KUIPER", grass: g(0x0A0C14), zenith: rgb3(0x05060C), fog: rgb3(0x10142A), fog_d: 0.0010, sun: [-0.5, 0.4, 0.5], accent: g(0x9FD4FF), pal_mix: 0.65, amp: 0.15, freq: 0.6, wet: 0.05, glow: 0.6, rings: 6, halos: 6, balloons: 0, domes: 0, ramps: 0, platforms: 0, trees: 0, small_pads: 24, nets: 2, focal: 17 },
            MapStyle { name: "ZOETROPE", grass: g(0x2A2432), zenith: rgb3(0x1A1526), fog: rgb3(0x352B4A), fog_d: 0.0014, sun: [0.3, 0.5, 0.4], accent: g(0xFFC24D), pal_mix: 0.5, amp: 0.1, freq: 1.0, wet: 0.28, glow: 0.5, rings: 0, halos: 3, balloons: 0, domes: 0, ramps: 0, platforms: 0, trees: 0, small_pads: 26, nets: 1, focal: 18 },
        ]
    })
}

pub fn style(idx: usize) -> &'static MapStyle {
    &styles()[idx % STYLE_COUNT]
}

// ---------------------------------------------------------- gen world ----
impl World {
    pub fn generate(seed: u64, style: &MapStyle, car_half_h: f32, wscale: f32) -> World {
        let mut routes: Vec<(Vec<Vec3>, f32)> = Vec::new();
        let mut route_names: Vec<String> = Vec::new();
        let mut route_shapes: Vec<Vec<u32>> = Vec::new();
        let mut floodable: Vec<usize> = Vec::new();
        let mut tiles: Vec<Tile> = Vec::new();
        let mut bodies: Vec<Body> = Vec::new();
        let mut drop_pit: Option<f32> = None;
        let ws = wscale.clamp(0.5, 1.7);
        let mut rng = Rng::new(seed);
        let pal: Vec<[f32; 4]> = base_palette()
            .into_iter()
            .map(|c| mix4(c, style.accent, style.pal_mix * 0.6))
            .collect();
        let mut shapes: Vec<Shape> = Vec::new();
        let mut rings: Vec<Ring> = Vec::new();
        let mut pads: Vec<Pad> = Vec::new();
        let mut decor_cars: Vec<Mat4> = Vec::new();
        let mut spots: Vec<Vec2> = Vec::new();
        let (amp, freq) = (style.amp, style.freq);
        // bake the ground-truth grid first; everything below samples it
        let mut tgrid = vec![0.0f32; TG_N * TG_N];
        for j in 0..TG_N {
            for i in 0..TG_N {
                let x = (i as f32 / (TG_N - 1) as f32 * 2.0 - 1.0) * TG_HALF;
                let z = (j as f32 / (TG_N - 1) as f32 * 2.0 - 1.0) * TG_HALF;
                tgrid[j * TG_N + i] = terrain_h_s(x, z, seed, amp, freq);
            }
        }
        let th = |x: f32, z: f32| grid_h(&tgrid, x, z);
        // nodal gravity (perplexus-family maps fill this; empty = plain -Y)
        let mut grav: Vec<GravNode> = Vec::new();
        let mut anims: Vec<Anim> = Vec::new();
        let mut zones: Vec<Zone> = Vec::new();
        let mut launchers: Vec<Launcher> = Vec::new();

        let place = |rng: &mut Rng, spots: &mut Vec<Vec2>, rmin: f32, rmax: f32, min_gap: f32| -> Option<Vec2> {
            for _ in 0..40 {
                let a = rng.range(0.0, std::f32::consts::TAU);
                let r = rng.range(rmin, rmax);
                let p = Vec2::new(a.cos() * r, a.sin() * r);
                if spots.iter().all(|s| s.distance(p) > min_gap) {
                    spots.push(p);
                    return Some(p);
                }
            }
            None
        };

        let push = |shapes: &mut Vec<Shape>, mesh: usize, pos: Vec3, rot: Quat, scale: Vec3, sdf: Sdf, color: [f32; 4], emissive: f32, spec: f32| -> usize {
            shapes.push(Shape {
                mesh,
                mat: Mat4::from_scale_rotation_translation(scale, rot, pos),
                pos,
                inv_rot: rot.inverse(),
                sdf,
                color,
                emissive,
                flash: 0.0,
                spec,
                blend: matches!(sdf, Sdf::Shell { .. }),
        cut: false,
        });
            shapes.len() - 1
        };

        // ---- sky rings: the air-dribble targets --------------------------
        for i in 0..style.rings {
            let Some(xz) = place(&mut rng, &mut spots, 20.0 * ws, 420.0 * ws, 14.0 * ws) else { continue };
            let big = rng.range(2.4, 5.2) + (i % 5) as f32 * 0.35;
            let tube = big * 0.07;
            let h = th(xz.x, xz.y) + rng.range(5.0, 12.0) + big + i as f32 * 1.15;
            let rot = Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU))
                * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2 + rng.range(-0.25, 0.25));
            let color = *rng.pick(&pal);
            let idx = push(
                &mut shapes,
                M_TORUS_THIN,
                Vec3::new(xz.x, h, xz.y),
                rot,
                Vec3::splat(big),
                Sdf::Torus { big, tube },
                color,
                0.28 + style.glow * 0.5,
                0.4,
            );
            rings.push(Ring { shape: idx, pos: Vec3::new(xz.x, h, xz.y), inv_rot: rot.inverse(), hole_r: big - tube * 2.0 });
        }

        // ---- halos: big flat rings to land on / bounce through -----------
        for _ in 0..style.halos {
            let Some(xz) = place(&mut rng, &mut spots, 60.0 * ws, 380.0 * ws, 26.0 * ws) else { continue };
            let big = rng.range(6.0, 10.0);
            let tube = big * 0.16;
            let h = rng.range(14.0, 38.0);
            let rot = Quat::from_rotation_y(rng.range(0.0, 6.0)) * Quat::from_rotation_x(rng.range(-0.12, 0.12));
            let color = *rng.pick(&pal);
            let idx = push(
                &mut shapes,
                M_TORUS_FAT,
                Vec3::new(xz.x, h, xz.y),
                rot,
                Vec3::splat(big),
                Sdf::Torus { big, tube },
                color,
                0.18 + style.glow * 0.3,
                0.3,
            );
            rings.push(Ring { shape: idx, pos: Vec3::new(xz.x, h, xz.y), inv_rot: rot.inverse(), hole_r: big - tube * 2.0 });
        }

        // ---- balloons: floating spheres, sometimes clustered -------------
        for _ in 0..style.balloons {
            let Some(xz) = place(&mut rng, &mut spots, 26.0 * ws, 430.0 * ws, 10.0 * ws) else { continue };
            let n = if rng.f() < 0.3 { 3 } else { 1 };
            for k in 0..n {
                let r = rng.range(1.2, 3.2);
                let off = if k == 0 {
                    Vec3::ZERO
                } else {
                    Vec3::new(rng.range(-3.0, 3.0), rng.range(-2.0, 2.5), rng.range(-3.0, 3.0))
                };
                let h = rng.range(7.0, 32.0);
                let p = Vec3::new(xz.x, h, xz.y) + off;
                push(&mut shapes, M_SPHERE, p, Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, *rng.pick(&pal), 0.06 + style.glow * 0.6, 0.5);
            }
        }

        // ---- domes: half-buried spheres, big soft hills to drive over ----
        for _ in 0..style.domes {
            let Some(xz) = place(&mut rng, &mut spots, 45.0 * ws, 420.0 * ws, 30.0 * ws) else { continue };
            let r = rng.range(7.0, 17.0);
            let ground = th(xz.x, xz.y);
            let p = Vec3::new(xz.x, ground - r * 0.45, xz.y);
            let mut c = *rng.pick(&pal);
            c[0] *= 0.8;
            c[1] *= 0.8;
            c[2] *= 0.8;
            push(&mut shapes, M_SPHERE, p, Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, c, 0.0, 0.15);
        }

        // ---- ramps: tilted slabs resting on the ground -------------------
        for _ in 0..style.ramps {
            let Some(xz) = place(&mut rng, &mut spots, 22.0 * ws, 380.0 * ws, 16.0 * ws) else { continue };
            let half = Vec3::new(rng.range(2.2, 3.4), 0.28, rng.range(4.2, 6.5));
            let pitch = rng.range(0.30, 0.52); // 17..30 degrees
            let yaw = rng.range(0.0, std::f32::consts::TAU);
            let rot = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch);
            let ground = th(xz.x, xz.y);
            let p = Vec3::new(xz.x, ground + half.z * pitch.sin() * 0.55 + 0.15, xz.y);
            push(&mut shapes, M_BOX, p, rot, half, Sdf::RBox { half, round: 0.12 }, *rng.pick(&pal), 0.0, 0.25);
        }

        // ---- platforms: floating slabs to land dribbles on ---------------
        for _ in 0..style.platforms {
            let Some(xz) = place(&mut rng, &mut spots, 30.0 * ws, 420.0 * ws, 18.0 * ws) else { continue };
            let half = Vec3::new(rng.range(2.6, 6.0), 0.35, rng.range(2.6, 6.0));
            let h = rng.range(5.0, 26.0);
            let rot = Quat::from_rotation_y(rng.range(0.0, 6.0)) * Quat::from_rotation_x(rng.range(-0.06, 0.06));
            push(
                &mut shapes,
                M_BOX,
                Vec3::new(xz.x, h, xz.y),
                rot,
                half,
                Sdf::RBox { half, round: 0.1 },
                *rng.pick(&pal),
                0.02 + style.glow * 0.15,
                0.3,
            );
        }

        // ---- lollipop trees: capsule stem + sphere pop -------------------
        for _ in 0..style.trees {
            let Some(xz) = place(&mut rng, &mut spots, 24.0 * ws, 430.0 * ws, 9.0 * ws) else { continue };
            let r_stem = rng.range(0.28, 0.45);
            let ground = th(xz.x, xz.y);
            let half = r_stem * CAP_TALL_HH;
            let stem_c = Vec3::new(xz.x, ground + half + r_stem * 0.4, xz.y);
            push(
                &mut shapes,
                M_CAP_TALL,
                stem_c,
                Quat::IDENTITY,
                Vec3::splat(r_stem),
                Sdf::Capsule { r: r_stem, half },
                rgb(0xB08968),
                0.0,
                0.1,
            );
            let rp = rng.range(1.1, 2.3);
            let pp = stem_c + Vec3::Y * (half + r_stem + rp * 0.55);
            push(&mut shapes, M_SPHERE, pp, Quat::IDENTITY, Vec3::splat(rp), Sdf::Sphere { r: rp }, *rng.pick(&pal), 0.05, 0.45);
        }

        // ---- spawn centerpiece (the ROOMS focal, translated) -------------
        let acc = style.accent;
        match style.focal {
            1 => {
                // obelisk: one tall glowing four-square spire
                let r = 1.1;
                let half = r * CAP_TALL_HH;
                let g0 = th(0.0, 0.0);
                push(&mut shapes, M_CAP_TALL, Vec3::new(0.0, g0 + half, 0.0), Quat::IDENTITY, Vec3::splat(r), Sdf::Capsule { r, half }, acc, 0.35 + style.glow * 0.5, 0.6);
            }
            2 => {
                // fountain: buried dome bowl + a fat ring hovering over it
                let g0 = th(0.0, 0.0);
                let r = 5.0;
                push(&mut shapes, M_SPHERE, Vec3::new(0.0, g0 - r * 0.7, 0.0), Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, mix4(PLINTH, acc, 0.3), 0.05, 0.3);
                let big = 3.2;
                let tube = big * 0.16;
                let rot = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2 * 0.02);
                let idx = push(&mut shapes, M_TORUS_FAT, Vec3::new(0.0, g0 + 3.4, 0.0), rot, Vec3::splat(big), Sdf::Torus { big, tube }, acc, 0.4, 0.5);
                rings.push(Ring { shape: idx, pos: Vec3::new(0.0, g0 + 3.4, 0.0), inv_rot: rot.inverse(), hole_r: big - tube * 2.0 });
            }
            3 => {
                // great tree: thick stem, huge canopy
                let r_stem = 1.3;
                let g0 = th(0.0, 0.0);
                let half = r_stem * CAP_TALL_HH;
                let stem_c = Vec3::new(0.0, g0 + half, 0.0);
                push(&mut shapes, M_CAP_TALL, stem_c, Quat::IDENTITY, Vec3::splat(r_stem), Sdf::Capsule { r: r_stem, half }, rgb(0xB08968), 0.0, 0.1);
                let rp = 6.5;
                push(&mut shapes, M_SPHERE, stem_c + Vec3::Y * (half + rp * 0.6), Quat::IDENTITY, Vec3::splat(rp), Sdf::Sphere { r: rp }, acc, 0.12, 0.4);
            }
            4 => {
                // brazier: stone box with a big fire orb
                let g0 = th(0.0, 0.0);
                let half = Vec3::new(1.6, 0.8, 1.6);
                push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + half.y, 0.0), Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, mix4(PLINTH, [0.1, 0.09, 0.09, 1.0], 0.5), 0.0, 0.3);
                let r = 1.3;
                push(&mut shapes, M_SPHERE, Vec3::new(0.0, g0 + half.y * 2.0 + r * 0.8, 0.0), Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, acc, 1.0, 0.2);
            }
            5 => {
                // lighthouse: tall tower with a glowing lamp
                let r = 1.5;
                let half = r * CAP_TALL_HH;
                let g0 = th(0.0, 0.0);
                let c0 = Vec3::new(0.0, g0 + half, 0.0);
                push(&mut shapes, M_CAP_TALL, c0, Quat::IDENTITY, Vec3::splat(r), Sdf::Capsule { r, half }, mix4(PLINTH, [1.0, 1.0, 1.0, 1.0], 0.4), 0.0, 0.4);
                let lr = 1.1;
                push(&mut shapes, M_SPHERE, c0 + Vec3::Y * (half + lr * 0.8), Quat::IDENTITY, Vec3::splat(lr), Sdf::Sphere { r: lr }, acc, 1.0, 0.5);
            }
            6 => {
                // orrery: glowing core + three concentric tilted rings (score rings!)
                let g0 = th(0.0, 0.0);
                let c0 = Vec3::new(0.0, g0 + 9.0, 0.0);
                push(&mut shapes, M_SPHERE, c0, Quat::IDENTITY, Vec3::splat(1.2), Sdf::Sphere { r: 1.2 }, acc, 1.0, 0.5);
                for k in 0..3u32 {
                    let big = 3.2 + k as f32 * 1.7;
                    let tube: f32 = 0.22;
                    let rot = Quat::from_rotation_y(k as f32 * 1.1)
                        * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2 + 0.35 - k as f32 * 0.35);
                    let idx = push(&mut shapes, M_TORUS_THIN, c0, rot, Vec3::splat(big), Sdf::Torus { big, tube: tube.max(big * 0.07) }, mix4(acc, [1.0; 4], 0.2), 0.5 + style.glow * 0.4, 0.5);
                    rings.push(Ring { shape: idx, pos: c0, inv_rot: rot.inverse(), hole_r: big - 0.5 });
                }
            }
            11 => {
                fn mouth_dir(a: Vec3, b: Vec3) -> Vec3 {
                    (b - a).normalize_or(Vec3::Y)
                }
                // PERPLEXUS: a full marble-run sculpture. Grammar pieces:
                // approach tube -> helix climb -> junction -> (drop shaft
                // into a wall-of-death DRUM -> exit tube) or (S-bend descent
                // -> gap jump -> stepped funnel -> figure-eight crossing).
                // Plus a balance-beam rail shortcut. Emissive lane stripes
                // alternate segments; boost pads run the spines.
                let tube_r = 5.8; // wider: room to dribble inside
                let mut spine_pads: Vec<Vec3> = Vec::new();
                let rim = |shapes: &mut Vec<Shape>, p: Vec3, axis: Vec3, r: f32, col: [f32; 4]| {
                    let rot = Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y));
                    push(shapes, M_TORUS_FAT, p, rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(r * 1.02), Sdf::Torus { big: r * 1.02, tube: r * 0.14 }, col, 0.25, 0.45);
                };
                let tube = |shapes: &mut Vec<Shape>,
                                grav: &mut Vec<GravNode>,
                                a: Vec3,
                                b: Vec3,
                                r: f32,
                                col: [f32; 4],
                                glow: f32| {
                    let mid = (a + b) * 0.5;
                    let axis = (b - a).normalize_or(Vec3::Z);
                    let half = (b - a).length() * 0.62;
                    let rot = Quat::from_rotation_arc(Vec3::Y, axis);
                    push(shapes, M_SHELL, mid, rot, Vec3::new(r, half, r), Sdf::Shell { r: r * 0.95, half, thick: r * 0.05 }, [col[0], col[1], col[2], 0.55], glow, 0.45);
                    for t in [0.2f32, 0.5, 0.8] {
                        grav.push(GravNode { pos: a.lerp(b, t), down: Vec3::NEG_Y, r: r * 1.55, tube: true, follow: None });
                    }
                };
                let cut_s = |shapes: &mut Vec<Shape>, p: Vec3, r: f32| {
                    let i = push(shapes, M_SPHERE, p, Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, [0.0, 0.0, 0.0, 0.0], 0.0, 0.0);
                    shapes[i].cut = true;
                };

                // 1) helix climb (1.5 turns), striped
                let helix_r = 26.0 * ws;
                let segs = 12;
                let climb = 3.4;
                let mut helix_pts: Vec<Vec3> = Vec::new();
                for k in 0..=segs {
                    let a = k as f32 / segs as f32 * std::f32::consts::TAU * 1.5;
                    helix_pts.push(Vec3::new(a.cos() * helix_r, 8.0 + k as f32 * climb, a.sin() * helix_r));
                }
                for k in 0..segs {
                    let glow = if k % 2 == 0 { 0.55 } else { 0.10 };
                    let col = if k % 2 == 0 { mix4(acc, [1.0; 4], 0.15) } else { mix4(*rng.pick(&pal), acc, 0.2) };
                    tube(&mut shapes, &mut grav, helix_pts[k], helix_pts[k + 1], tube_r, col, glow);
                    spine_pads.push(helix_pts[k].lerp(helix_pts[k + 1], 0.5) - Vec3::Y * (tube_r * 0.82));
                }
                let mouth = helix_pts[0];
                let top = helix_pts[segs];
                spots.push(Vec2::new(top.x, top.z));

                // 2) approach tube from the plaza
                let start = Vec3::new(mouth.x + 34.0 * ws, th(mouth.x + 34.0 * ws, mouth.z) + tube_r * 0.8, mouth.z);
                tube(&mut shapes, &mut grav, start, mouth, tube_r, mix4(PLINTH, acc, 0.3), 0.12);

                // 3) junction platform at the summit
                let jc = top + Vec3::Y * 0.4;
                let jh = Vec3::new(6.0, 0.35, 6.0);
                push(&mut shapes, M_BOX, jc, Quat::IDENTITY, jh, Sdf::RBox { half: jh, round: 0.1 }, mix4(PLINTH, acc, 0.25), 0.2, 0.4);

                // 4) drop shaft: straight down into the drum
                let drum_c = Vec3::new(-30.0 * ws, 0.0, -20.0 * ws);
                let drum_r = 13.0;
                let drum_h = 4.6;
                let g0 = th(drum_c.x, drum_c.z);
                let shaft_top = jc + Vec3::new(-4.0, 1.0, -3.0);
                let shaft_bot = Vec3::new(drum_c.x, g0 + drum_h * 2.0 + 4.0, drum_c.z);
                tube(&mut shapes, &mut grav, shaft_top, shaft_top.lerp(shaft_bot, 0.5), 3.6, mix4(acc, [1.0; 4], 0.3), 0.5);
                tube(&mut shapes, &mut grav, shaft_top.lerp(shaft_bot, 0.5), shaft_bot, 3.6, mix4(*rng.pick(&pal), acc, 0.2), 0.1);

                // 5) the DRUM: wall-of-death cylinder, open top
                let dc = Vec3::new(drum_c.x, g0 + drum_h, drum_c.z);
                push(&mut shapes, M_SHELL, dc, Quat::IDENTITY, Vec3::new(drum_r, drum_h, drum_r), Sdf::Shell { r: drum_r * 0.96, half: drum_h, thick: drum_r * 0.04 }, { let c = mix4(*rng.pick(&pal), acc, 0.3); [c[0], c[1], c[2], 0.6] }, 0.25, 0.5);
                for y in [0.35f32, 0.65] {
                    grav.push(GravNode { pos: Vec3::new(dc.x, g0 + drum_h * 2.0 * y, dc.z), down: Vec3::NEG_Y, r: drum_r * 1.35, tube: true, follow: None });
                }
                for k in 0..6 {
                    let a = k as f32 / 6.0 * std::f32::consts::TAU;
                    spine_pads.push(Vec3::new(dc.x + a.cos() * drum_r * 0.8, g0 + 0.4, dc.z + a.sin() * drum_r * 0.8));
                }
                spots.push(Vec2::new(dc.x, dc.z));

                // 6) exit tube: tangential escape back toward the plaza
                let ex0 = Vec3::new(dc.x + drum_r * 0.9, g0 + 2.2, dc.z);
                let ex1 = Vec3::new(dc.x + drum_r + 26.0, th(dc.x + drum_r + 26.0, dc.z + 8.0) + tube_r * 0.8, dc.z + 8.0);
                tube(&mut shapes, &mut grav, ex0, ex1, 3.8, mix4(PLINTH, acc, 0.3), 0.15);

                // 7) S-bend descent off the junction's other side
                let mut sp = jc + Vec3::new(4.5, 0.5, 3.5);
                let mut sdir = 1.0f32;
                let mut sbend_end = sp;
                for k in 0..4 {
                    let nxt = sp + Vec3::new(sdir * 14.0, -6.0, 10.0);
                    let glow = if k % 2 == 0 { 0.45 } else { 0.08 };
                    tube(&mut shapes, &mut grav, sp, nxt, 3.8, mix4(*rng.pick(&pal), acc, 0.25), glow);
                    sp = nxt;
                    sdir = -sdir;
                    sbend_end = nxt;
                }

                // 8) GAP JUMP into 9) a stepped funnel (three shrinking hoops)
                let fun_top = sbend_end + Vec3::new(0.0, -2.0, 12.0); // 12 m of air
                for (i, fr) in [7.5f32, 5.8, 4.4].iter().enumerate() {
                    let c = fun_top - Vec3::Y * (i as f32 * 3.4);
                    push(&mut shapes, M_SHELL, c, Quat::IDENTITY, Vec3::new(*fr, 1.5, *fr), Sdf::Shell { r: fr * 0.94, half: 1.5, thick: fr * 0.06 }, { let c = mix4(acc, [1.0; 4], 0.2); [c[0], c[1], c[2], 0.55] }, 0.35, 0.45);
                    grav.push(GravNode { pos: c, down: Vec3::NEG_Y, r: fr * 1.4, tube: true, follow: None });
                }

                // 10) figure-eight: two long tubes crossing at different
                // heights mid-field ("I was just there")
                let fa0 = Vec3::new(-46.0 * ws, 9.0, 30.0 * ws);
                let fa1 = Vec3::new(46.0 * ws, 9.0, -6.0 * ws);
                let fb0 = Vec3::new(-40.0 * ws, 15.5, -14.0 * ws);
                let fb1 = Vec3::new(40.0 * ws, 15.5, 34.0 * ws);
                tube(&mut shapes, &mut grav, fa0, fa1, 4.0, mix4(*rng.pick(&pal), acc, 0.2), 0.12);
                tube(&mut shapes, &mut grav, fb0, fb1, 4.0, mix4(*rng.pick(&pal), acc, 0.2), 0.40);

                // 11) balance beam: a bare rail from the summit to the drum rim
                let beam_a = jc + Vec3::new(-5.5, -0.2, -4.0);
                let beam_b = Vec3::new(dc.x, g0 + drum_h * 2.0 + 0.4, dc.z + drum_r * 0.9);
                let bm = (beam_a + beam_b) * 0.5;
                let baxis = (beam_b - beam_a).normalize_or(Vec3::X);
                let bhalf = (beam_b - beam_a).length() * 0.5;
                let brot = Quat::from_rotation_arc(Vec3::Y, baxis);
                let br = 0.55;
                push(&mut shapes, M_CAP_TALL, bm, brot, Vec3::new(br, bhalf / CAP_TALL_HH.max(1.0), br), Sdf::Capsule { r: br, half: bhalf }, mix4(acc, [1.0; 4], 0.3), 0.4, 0.5);

                // mouths: rims + ground aprons at the big openings
                let ap = |shapes: &mut Vec<Shape>, x: f32, z: f32, toward: Vec3| {
                    let g = th(x, z);
                    let half = Vec3::new(5.0, 0.3, 4.0);
                    let rot = Quat::from_rotation_arc(Vec3::Y, (Vec3::Y + toward * 0.35).normalize());
                    push(shapes, M_BOX, Vec3::new(x, g + 0.7, z), rot, half, Sdf::RBox { half, round: 0.15 }, mix4(PLINTH, acc, 0.2), 0.05, 0.35);
                };
                rim(&mut shapes, start, mouth_dir(start, helix_pts[0]), tube_r, mix4(acc, [1.0; 4], 0.3));
                ap(&mut shapes, start.x + 4.0, start.z, Vec3::X);
                rim(&mut shapes, ex1, mouth_dir(ex0, ex1), 3.8, mix4(acc, [1.0; 4], 0.3));
                ap(&mut shapes, ex1.x + 3.0, ex1.z, Vec3::X);
                rim(&mut shapes, helix_pts[segs], mouth_dir(helix_pts[segs - 1], helix_pts[segs]), tube_r, acc);

                // ---- jungle-gym lattice: a tall random tube network with
                // switchbacks, drops and knuckle joints everywhere ----
                let mut anchors: Vec<Vec3> = Vec::new();
                for k in 0..20 {
                    let a = rng.range(0.0, std::f32::consts::TAU);
                    let rr = rng.range(34.0, 118.0) * ws; // BIGGER playground
                    let y = rng.range(6.0, 58.0);
                    let p = Vec3::new(a.cos() * rr, y, a.sin() * rr);
                    anchors.push(p);
                    let _ = k;
                }
                // pass 1 — pick links: THREE nearest per anchor, longer reach
                let mut links: Vec<(usize, usize, f32, f32)> = Vec::new();
                for i in 0..anchors.len() {
                    let mut near: Vec<(f32, usize)> = (0..anchors.len())
                        .filter(|&j| j != i)
                        .map(|j| (anchors[i].distance(anchors[j]), j))
                        .collect();
                    near.sort_by(|a, b| a.0.total_cmp(&b.0));
                    for &(d, j) in near.iter().take(3) {
                        if j < i || d > 85.0 {
                            continue;
                        }
                        let r = if rng.f() < 0.25 { rng.range(9.0, 15.0) } else { rng.range(4.2, 7.0) };
                        let glow = if rng.f() < 0.4 { 0.5 } else { 0.08 };
                        links.push((i, j, r, glow));
                    }
                }
                // grand ring: peripheral anchors joined in a walkable loop
                {
                    let mut per: Vec<usize> = (0..anchors.len())
                        .filter(|&i| Vec2::new(anchors[i].x, anchors[i].z).length() > 70.0)
                        .collect();
                    per.sort_by(|&a, &b| anchors[a].z.atan2(anchors[a].x).total_cmp(&anchors[b].z.atan2(anchors[b].x)));
                    for k in 0..per.len() {
                        let (i, j) = (per[k], per[(k + 1) % per.len()]);
                        if anchors[i].distance(anchors[j]) < 150.0 {
                            links.push((i.min(j), i.max(j), 7.6, 0.35));
                        }
                    }
                }
                // pass 2 — JUNCTION CHAMBERS: tubes used to cross straight
                // THROUGH each other at anchors (a wall at every joint, the
                // whole "hard to navigate"). Every linked anchor is now a
                // hollow glass room, carved open toward each of its tubes.
                let mut knuckle_r = vec![0.0f32; anchors.len()];
                {
                    let mut amax = vec![0.0f32; anchors.len()];
                    for &(i, j, r, _) in &links {
                        amax[i] = amax[i].max(r);
                        amax[j] = amax[j].max(r);
                    }
                    for i in 0..anchors.len() {
                        if amax[i] <= 0.0 {
                            continue;
                        }
                        let cr = amax[i] * 1.45 + 1.2;
                        knuckle_r[i] = cr;
                        let ci = push(&mut shapes, M_SPHERE, anchors[i], Quat::IDENTITY, Vec3::splat(cr), Sdf::Sphere { r: cr }, [acc[0], acc[1], acc[2], 0.16], 0.10, 0.5);
                        shapes[ci].blend = true;
                        for hoop in [Quat::IDENTITY, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)] {
                            push(&mut shapes, M_TORUS_THIN, anchors[i], hoop, Vec3::splat(cr * 0.99), Sdf::Torus { big: cr * 0.99, tube: (cr * 0.05).max(0.16) }, mix4(acc, [1.0; 4], 0.25), 0.55, 0.4);
                        }
                        cut_s(&mut shapes, anchors[i], cr - 0.9);
                        grav.push(GravNode { pos: anchors[i], down: Vec3::NEG_Y, r: cr * 1.8, tube: true, follow: None });
                    }
                }
                // pass 3 — tubes pulled back to chamber walls, mouths carved,
                // emissive guide hoops lighting every big bore
                for &(i, j, r, glow) in &links {
                    let dir = (anchors[j] - anchors[i]).normalize_or(Vec3::X);
                    let a = anchors[i] + dir * (knuckle_r[i] * 0.94);
                    let b = anchors[j] - dir * (knuckle_r[j] * 0.94);
                    tube(&mut shapes, &mut grav, a, b, r, mix4(*rng.pick(&pal), acc, 0.25), glow);
                    cut_s(&mut shapes, anchors[i] + dir * knuckle_r[i], r * 1.12);
                    cut_s(&mut shapes, anchors[j] - dir * knuckle_r[j], r * 1.12);
                    if r >= 5.0 {
                        let len = a.distance(b);
                        let n = (len / 20.0) as i32;
                        let hrot = Quat::from_rotation_arc(Vec3::Y, dir) * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
                        for k in 1..n {
                            let p = a.lerp(b, k as f32 / n as f32);
                            push(&mut shapes, M_TORUS_THIN, p, hrot, Vec3::splat(r * 0.88), Sdf::Torus { big: r * 0.88, tube: (r * 0.05).max(0.14) }, mix4(acc, [1.0; 4], 0.3), 0.75, 0.4);
                        }
                    }
                    spine_pads.push(a.lerp(b, 0.5) - Vec3::Y * (r * 0.8));
                }
                // a few sheer drop shafts off high anchors
                for _ in 0..3 {
                    let a = *rng.pick(&anchors);
                    if a.y > 24.0 {
                        let bot = Vec3::new(a.x, th(a.x, a.z) + 4.0, a.z);
                        tube(&mut shapes, &mut grav, a, bot, 3.6, mix4(acc, [1.0; 4], 0.25), 0.45);
                        rim(&mut shapes, bot, Vec3::NEG_Y, 3.6, acc);
                    }
                }

                // ---- spinners: pinwheel rotors to dodge (or get slapped by)
                for _ in 0..3 {
                    let a = *rng.pick(&anchors);
                    let g = th(a.x, a.z);
                    let p = Vec3::new(a.x, g + 3.0, a.z);
                    let half = Vec3::new(9.0, 0.35, 1.1);
                    let idx = push(&mut shapes, M_BOX, p, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, mix4(acc, [1.0; 4], 0.3), 0.35, 0.5);
                    anims.push(Anim::Spin { shape: idx, pos: p, scale: half, base: Quat::IDENTITY, axis: Vec3::Y, rate: rng.range(0.5, 1.4) });
                }
                // rotating platforms: slow discs to ride (they'll shove you)
                for _ in 0..2 {
                    let a = *rng.pick(&anchors);
                    let g = th(a.x, a.z);
                    let p = Vec3::new(a.x + 10.0, g + rng.range(4.0, 10.0), a.z + 10.0);
                    let half = Vec3::new(6.0, 0.4, 6.0);
                    let idx = push(&mut shapes, M_BOX, p, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.2 }, mix4(*rng.pick(&pal), acc, 0.2), 0.1, 0.4);
                    anims.push(Anim::Spin { shape: idx, pos: p, scale: half, base: Quat::IDENTITY, axis: Vec3::Y, rate: 0.35 });
                    spots.push(Vec2::new(p.x, p.z));
                }
                // moving closing doors across the figure-eight tubes
                for (k, fp) in [((fa0 + fa1) * 0.5), ((fb0 + fb1) * 0.5)].iter().enumerate() {
                    let half = Vec3::new(0.5, 4.2, 4.2);
                    let idx = push(&mut shapes, M_BOX, *fp, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, mix4(acc, [1.0; 4], 0.25), 0.4, 0.5);
                    anims.push(Anim::Door { shape: idx, base: *fp, rot: Quat::IDENTITY, scale: half, axis: Vec3::Y, amp: 8.0, rate: 0.7, phase: k as f32 * 1.7 });
                }

                // ---- pinball zones: vibe, slip, boost ----
                let zone_disc = |shapes: &mut Vec<Shape>, p: Vec3, r: f32, col: [f32; 4], em: f32| {
                    let half = Vec3::new(r, 0.08, r);
                    push(shapes, M_BOX, p, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.05 }, col, em, 0.3);
                };
                for (kind, col) in [
                    (ZoneKind::Vibe, [1.0, 0.3, 0.8, 0.55]),
                    (ZoneKind::Slip, [0.5, 0.85, 1.0, 0.5]),
                    (ZoneKind::Boost, [1.0, 0.6, 0.15, 0.55]),
                ] {
                    for _ in 0..2 {
                        let a = rng.range(0.0, std::f32::consts::TAU);
                        let rr = rng.range(14.0, 60.0) * ws;
                        let (x, z) = (a.cos() * rr, a.sin() * rr);
                        let g = th(x, z);
                        let zr = rng.range(6.0, 10.0);
                        zone_disc(&mut shapes, Vec3::new(x, g + 0.1, z), zr, col, 0.45);
                        zones.push(Zone { pos: Vec3::new(x, g + 1.0, z), r: zr, kind });
                    }
                }

                // ---- blast-off launchers: ride the arrow ----
                for _ in 0..3 {
                    let a = rng.range(0.0, std::f32::consts::TAU);
                    let rr = rng.range(16.0, 50.0) * ws;
                    let (x, z) = (a.cos() * rr, a.sin() * rr);
                    let g = th(x, z);
                    let dir = (Vec3::new(-a.cos() * 0.4, 1.0, -a.sin() * 0.4)).normalize();
                    let idx = push(&mut shapes, M_TORUS_FAT, Vec3::new(x, g + 0.3, z), Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(2.2), Sdf::Torus { big: 2.2, tube: 0.35 }, [1.0, 0.85, 0.2, 0.9], 0.8, 0.5);
                    launchers.push(Launcher { pos: Vec3::new(x, g + 0.8, z), dir, power: 26.0, timer: 0.0, shape: idx });
                }

                // ---- gravity wrap-around orbs: loop the sphere mid-air ----
                for _ in 0..3 {
                    let a = rng.range(0.0, std::f32::consts::TAU);
                    let rr = rng.range(28.0, 64.0) * ws;
                    let p = Vec3::new(a.cos() * rr, rng.range(18.0, 40.0), a.sin() * rr);
                    push(&mut shapes, M_SPHERE, p, Quat::IDENTITY, Vec3::splat(4.5), Sdf::Sphere { r: 4.5 }, mix4(acc, [1.0; 4], 0.4), 0.7, 0.5);
                    grav.push(GravNode { pos: p, down: Vec3::NEG_Y, r: 11.0, tube: true, follow: None });
                    spots.push(Vec2::new(p.x, p.z));
                }

                // boost pads along the spines
                for p in spine_pads {
                    pads.push(Pad { pos: p, big: false, timer: 0.0, phase: 0.0 });
                }
                // ================= PERPLEXUS v4: CLUSTERS =================
                // The sculpture above is the TECHNICAL heart. Around it: a
                // COASTER cluster (huge smooth-curved pipes, full vertical
                // loops) and a PLAZA cluster (hollow junction chambers with
                // carved mouths), tied together by freight connectors, with
                // ground entrances everywhere. Curves are chains of short
                // shells whose joint seams are CARVED OPEN by cutter
                // spheres — union minus cut = continuous drivable bore.
                // (cut_s is defined up with the tube closure: the whole
                // sculpture carves now, not just the clusters)
                // arc chain: center c, plane (u,v), radius big_r, angles a0->a1
                let arc = |shapes: &mut Vec<Shape>, grav: &mut Vec<GravNode>, route: &mut Vec<Vec3>, c: Vec3, u: Vec3, v: Vec3, big_r: f32, a0: f32, a1: f32, r: f32, col: [f32; 4], glow: f32| {
                    let n = (((a1 - a0).abs() / 0.24).ceil() as usize).max(2);
                    let st = (a1 - a0) / n as f32;
                    let at = |k: usize| {
                        let a = a0 + st * k as f32;
                        c + (u * a.cos() + v * a.sin()) * big_r
                    };
                    for k in 0..n {
                        let p0 = at(k);
                        let p1 = at(k + 1);
                        tube(shapes, grav, p0, p1, r, col, glow);
                        if k + 1 < n {
                            cut_s(shapes, p1, r * 0.92); // open the elbow seam
                        }
                        route.push(p0);
                    }
                    route.push(at(n));
                };
                let launcher_up = |shapes: &mut Vec<Shape>, launchers: &mut Vec<Launcher>, p: Vec3, dir: Vec3, power: f32| {
                    let rot = Quat::from_rotation_arc(Vec3::Y, dir.normalize_or(Vec3::Y));
                    let i = push(shapes, M_TORUS_FAT, p, rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(2.2), Sdf::Torus { big: 2.2, tube: 0.4 }, mix4(acc, [1.0; 4], 0.4), 0.9, 0.5);
                    launchers.push(Launcher { pos: p, dir: dir.normalize_or(Vec3::Y), power, timer: 0.0, shape: i });
                };

                // ---------------- COASTER cluster ----------------
                let cc = Vec3::new(205.0 * ws, 0.0, 70.0 * ws);
                let big = rng.range(11.0, 17.0); // HUGE bore
                let ccol = mix4(*rng.pick(&pal), acc, 0.35);
                let mut r_main: Vec<Vec3> = Vec::new();
                // ground mouth (entrance from below), long climb, grand loop,
                // swooping S, second loop, descent to a second ground mouth
                let m0 = Vec3::new(cc.x - 70.0, th(cc.x - 70.0, cc.z) + big * 0.85, cc.z);
                let top = Vec3::new(cc.x - 18.0, m0.y + 30.0, cc.z);
                tube(&mut shapes, &mut grav, m0, top, big, ccol, 0.18);
                r_main.push(m0);
                r_main.push(top);
                cut_s(&mut shapes, top, big * 0.92);
                rim(&mut shapes, m0, mouth_dir(top, m0), big, mix4(acc, [1.0; 4], 0.3));
                launcher_up(&mut shapes, &mut launchers, Vec3::new(m0.x, th(m0.x, m0.z) + 1.2, m0.z), (m0 - Vec3::new(m0.x, th(m0.x, m0.z), m0.z) + (top - m0) * 0.2).normalize(), 30.0);
                // grand loop: full vertical circle tangent to the run
                let dirf = (top - m0).normalize();
                let flat_f = Vec3::new(dirf.x, 0.0, dirf.z).normalize_or(Vec3::X);
                let loop_r = big * 2.4;
                let lc = top + flat_f * loop_r * 0.2 + Vec3::Y * loop_r;
                arc(&mut shapes, &mut grav, &mut r_main, lc, -Vec3::Y, flat_f, loop_r, 0.0, std::f32::consts::TAU, big, mix4(ccol, [1.0; 4], 0.2), 0.30);
                cut_s(&mut shapes, top, big * 0.92);
                // swooping S off the loop bottom
                let s0 = top + flat_f * 8.0;
                let sc1 = s0 + flat_f * 22.0 + Vec3::Y * 6.0;
                arc(&mut shapes, &mut grav, &mut r_main, sc1, Vec3::Y, flat_f, 22.0, 3.6, 4.9, big * 0.85, ccol, 0.16);
                let s_end = *r_main.last().unwrap();
                // second, smaller loop
                let loop2_r = big * 1.6;
                let lc2 = s_end + flat_f * loop2_r * 0.3 + Vec3::Y * loop2_r;
                arc(&mut shapes, &mut grav, &mut r_main, lc2, -Vec3::Y, flat_f, loop2_r, 0.0, std::f32::consts::TAU, big * 0.85, mix4(ccol, acc, 0.3), 0.34);
                cut_s(&mut shapes, s_end, big * 0.8);
                // descent to ground mouth two
                let m1g = s_end + flat_f * 46.0;
                let m1 = Vec3::new(m1g.x, th(m1g.x, m1g.z) + big * 0.8, m1g.z);
                tube(&mut shapes, &mut grav, s_end, m1, big * 0.85, ccol, 0.14);
                cut_s(&mut shapes, s_end, big * 0.78);
                r_main.push(m1);
                rim(&mut shapes, m1, mouth_dir(s_end, m1), big * 0.85, acc);
                ap(&mut shapes, m1.x + 4.0, m1.z, flat_f);
                // boost strips in the throat + a rotor to dodge at the top
                zones.push(Zone { pos: m0.lerp(top, 0.4), r: big * 0.9, kind: ZoneKind::Boost });
                zones.push(Zone { pos: lc - Vec3::Y * loop_r, r: big * 0.9, kind: ZoneKind::Boost });
                {
                    let half = Vec3::new(big * 0.7, 0.3, 1.0);
                    let i = push(&mut shapes, M_BOX, top + Vec3::Y * 2.0, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, mix4(acc, [1.0; 4], 0.2), 0.5, 0.4);
                    anims.push(Anim::Spin { shape: i, pos: top + Vec3::Y * 2.0, scale: half, base: Quat::IDENTITY, axis: Vec3::Y, rate: 1.4 });
                }
                routes.push((r_main.clone(), big));

                // ---------------- PLAZA cluster: hollow chambers ----------------
                let pc = Vec3::new(-195.0 * ws, 0.0, -165.0 * ws);
                let ch_r = [15.0f32, 12.0, 10.0];
                let mut centers: Vec<Vec3> = Vec::new();
                for (k, &cr) in ch_r.iter().enumerate() {
                    let a = k as f32 * 2.1 + 0.6;
                    let cpos = pc + Vec3::new(a.cos() * 34.0, 14.0 + k as f32 * 9.0, a.sin() * 34.0);
                    centers.push(cpos);
                    // glass sphere, hollowed by an inner cutter
                    let ci = push(&mut shapes, M_SPHERE, cpos, Quat::IDENTITY, Vec3::splat(cr), Sdf::Sphere { r: cr }, [acc[0], acc[1], acc[2], 0.18], 0.10, 0.5);
                    shapes[ci].blend = true;
                    for hoop in [Quat::IDENTITY, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)] {
                        push(&mut shapes, M_TORUS_THIN, cpos, hoop, Vec3::splat(cr * 0.99), Sdf::Torus { big: cr * 0.99, tube: (cr * 0.05).max(0.16) }, mix4(acc, [1.0; 4], 0.25), 0.55, 0.4);
                    }
                    cut_s(&mut shapes, cpos, cr - 1.1);
                    // radial interior gravity: drive the inside of the ball
                    grav.push(GravNode { pos: cpos, down: Vec3::NEG_Y, r: cr * 1.7, tube: true, follow: None });
                }
                // chamber-to-chamber tubes with carved mouths
                let mut r_plaza: Vec<Vec3> = Vec::new();
                for k in 0..centers.len() {
                    let (a, b) = (centers[k], centers[(k + 1) % centers.len()]);
                    let d = (b - a).normalize();
                    let (ra, rb) = (ch_r[k], ch_r[(k + 1) % centers.len()]);
                    let tr = 5.6;
                    tube(&mut shapes, &mut grav, a + d * (ra - 0.6), b - d * (rb - 0.6), tr, mix4(*rng.pick(&pal), acc, 0.3), 0.25);
                    cut_s(&mut shapes, a + d * ra, tr * 1.12); // mouth through the wall
                    cut_s(&mut shapes, b - d * rb, tr * 1.12);
                    r_plaza.push(a);
                }
                r_plaza.push(centers[0]);
                routes.push((r_plaza, 8.0));
                // MANY ground entrances: a riser into each chamber from below
                for (k, &cpos) in centers.iter().enumerate() {
                    let gp = Vec3::new(cpos.x + (k as f32 * 2.7).cos() * 16.0, 0.0, cpos.z + (k as f32 * 2.7).sin() * 16.0);
                    let gm = Vec3::new(gp.x, th(gp.x, gp.z) + 4.6, gp.z);
                    let att = cpos + (gm - cpos).normalize() * (ch_r[k] - 0.5);
                    let tr = 4.6;
                    tube(&mut shapes, &mut grav, gm, att, tr, mix4(acc, [1.0; 4], 0.25), 0.4);
                    cut_s(&mut shapes, cpos + (gm - cpos).normalize() * ch_r[k], tr * 1.12);
                    rim(&mut shapes, gm, (cpos - gm).normalize(), tr, acc);
                    ap(&mut shapes, gm.x + 3.0, gm.z, Vec3::X);
                    launcher_up(&mut shapes, &mut launchers, Vec3::new(gp.x, th(gp.x, gp.z) + 1.1, gp.z), (att - gm).normalize(), 27.0);
                    routes.push((vec![gm, att, cpos], tr));
                }
                zones.push(Zone { pos: centers[0], r: ch_r[0] * 0.9, kind: ZoneKind::Vibe });

                // lattice -> cluster ties: the heart wired into both wings
                {
                    let mut hi: Vec<usize> = (0..anchors.len()).filter(|&i| knuckle_r[i] > 0.0).collect();
                    hi.sort_by(|&a, &b| anchors[b].y.total_cmp(&anchors[a].y));
                    let plaza_wall = |from: Vec3| centers[1] + (from - centers[1]).normalize_or(Vec3::X) * (12.0 - 0.6);
                    for (k, raw_tgt) in [(0usize, top), (1usize, Vec3::ZERO)] {
                        if let Some(&ai) = hi.get(k) {
                            let tgt = if k == 1 { plaza_wall(anchors[ai]) } else { raw_tgt };
                            let dir = (tgt - anchors[ai]).normalize_or(Vec3::X);
                            let a = anchors[ai] + dir * (knuckle_r[ai] * 0.94);
                            let tr = 6.8;
                            tube(&mut shapes, &mut grav, a, tgt, tr, mix4(acc, [1.0; 4], 0.25), 0.3);
                            cut_s(&mut shapes, anchors[ai] + dir * knuckle_r[ai], tr * 1.12);
                            cut_s(&mut shapes, tgt + dir * 0.4, tr * 1.08);
                            routes.push((vec![anchors[ai], tgt], tr));
                        }
                    }
                }

                // ---------------- freight connectors (cluster triangle) ----------------
                let hubs = [Vec3::new(0.0, 34.0, 0.0), Vec3::new(cc.x - 18.0, m0.y + 30.0, cc.z), centers[0]];
                for k in 0..3 {
                    let (a, b) = (hubs[k], hubs[(k + 1) % 3]);
                    let mid = (a + b) * 0.5 + Vec3::Y * rng.range(6.0, 16.0);
                    let fr = rng.range(7.5, 9.5);
                    let col = mix4(*rng.pick(&pal), acc, 0.3);
                    tube(&mut shapes, &mut grav, a, mid, fr, col, 0.12);
                    tube(&mut shapes, &mut grav, mid, b, fr, col, 0.12);
                    cut_s(&mut shapes, mid, fr * 0.92);
                    rim(&mut shapes, a, mouth_dir(mid, a), fr, mix4(acc, [1.0; 4], 0.25));
                    rim(&mut shapes, b, mouth_dir(mid, b), fr, mix4(acc, [1.0; 4], 0.25));
                    routes.push((vec![a, mid, b], fr));
                }

                // ---------------- sky machinery ----------------
                for (k, &hub) in hubs.iter().enumerate() {
                    for j in 0..3u32 {
                        let half = Vec3::new(3.0, 0.32, 3.0);
                        let i = push(&mut shapes, M_BOX, hub, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, mix4(*rng.pick(&pal), [1.0; 4], 0.2), 0.35, 0.4);
                        anims.push(Anim::Orbit { shape: i, center: hub + Vec3::Y * (10.0 + j as f32 * 5.0), axis: Vec3::Y, r: 24.0 + j as f32 * 9.0, rate: 0.22 + j as f32 * 0.07, phase: k as f32 * 2.1 + j as f32 * 1.7, rot: Quat::IDENTITY, scale: half });
                    }
                }
                // sliding stepping-stones across the two big gaps
                for k in 0..2 {
                    let (a, b) = (hubs[k], hubs[(k + 1) % 3]);
                    let base = a.lerp(b, 0.5) - Vec3::Y * 6.0;
                    let dir = (b - a).normalize();
                    let half = Vec3::new(4.2, 0.35, 2.6);
                    let i = push(&mut shapes, M_BOX, base, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.12 }, mix4(acc, [1.0; 4], 0.15), 0.3, 0.4);
                    anims.push(Anim::Slide { shape: i, base, dir, amp: 13.0, rate: 0.5 + k as f32 * 0.17, phase: k as f32 * 1.3, rot: Quat::IDENTITY, scale: half });
                }
                // the roaming MOON: an orbiting emissive orb that drags its
                // own gravity well around the plaza — chaos on a leash
                {
                    let mr = 2.4;
                    let i = push(&mut shapes, M_SPHERE, pc + Vec3::Y * 40.0, Quat::IDENTITY, Vec3::splat(mr), Sdf::Sphere { r: mr }, mix4(acc, [1.0; 4], 0.5), 1.0, 0.3);
                    anims.push(Anim::Orbit { shape: i, center: pc + Vec3::Y * 42.0, axis: Vec3::new(0.2, 1.0, 0.1).normalize(), r: 58.0, rate: 0.11, phase: 0.0, rot: Quat::IDENTITY, scale: Vec3::splat(mr) });
                    grav.push(GravNode { pos: pc + Vec3::Y * 40.0, down: Vec3::NEG_Y, r: 24.0, tube: true, follow: Some(i) });
                }

            }

            12 => {
                // RIBBONWORKS: a corkscrew ribbon around a great tower, banked
                // ever harder — its gravity nodes carry the ribbon's own
                // "down", so the top of the corkscrew drives upside-down.
                let tower_r = 3.4;
                let thalf = tower_r * CAP_TALL_HH;
                push(&mut shapes, M_CAP_TALL, Vec3::new(0.0, th(0.0, 0.0) + thalf * 0.8, 0.0), Quat::IDENTITY, Vec3::splat(tower_r), Sdf::Capsule { r: tower_r, half: thalf }, mix4(PLINTH, acc, 0.2), 0.15, 0.4);
                let rib_r = 17.0 * ws;
                let n = 16;
                for k in 0..n {
                    let a = k as f32 / n as f32 * std::f32::consts::TAU * 2.0;
                    let y = 3.0 + k as f32 * 2.1;
                    let p = Vec3::new(a.cos() * rib_r, y, a.sin() * rib_r);
                    let tangent = Vec3::new(-a.sin(), 2.1 / (rib_r * 0.39), a.cos()).normalize();
                    let bank = (k as f32 / n as f32) * 2.6; // flat -> past vertical
                    let inward = Vec3::new(-a.cos(), 0.0, -a.sin());
                    let up0 = Vec3::Y;
                    let up = (up0 * bank.cos() + inward * bank.sin()).normalize();
                    let rot = Quat::from_rotation_arc(Vec3::Y, up) * Quat::from_rotation_arc(Vec3::NEG_Z, {
                        let f = tangent - up * tangent.dot(up);
                        f.normalize_or(Vec3::NEG_Z)
                    });
                    let half = Vec3::new(3.4, 0.22, 4.4);
                    push(&mut shapes, M_BOX, p, rot, half, Sdf::RBox { half, round: 0.1 }, mix4(*rng.pick(&pal), acc, 0.2), 0.06 + style.glow * 0.2, 0.4);
                    grav.push(GravNode { pos: p + up * 0.6, down: -up, r: 7.5, tube: false, follow: None });
                    if k % 4 == 2 {
                        spots.push(Vec2::new(p.x, p.z));
                    }
                }
            }
            9 => {
                // ARENA CLASSIC: soccer pitch in a box. Walls, a ceiling ring
                // with an open skylight (flip-reset real estate), corner
                // banks, two half-buried spine tubes, bleachers + light
                // towers. Goals arrive with the nets pass (paired, big).
                let hx = 52.0 * ws;
                let hz = 62.0 * ws;
                let wall_h = 3.4;
                let g0 = th(0.0, 0.0);
                let wall_c = mix4(PLINTH, [1.0, 1.0, 1.0, 1.0], 0.55);
                for (off, half) in [
                    (Vec3::new(0.0, g0 + wall_h, -hz), Vec3::new(hx + 0.6, wall_h, 0.6)),
                    (Vec3::new(0.0, g0 + wall_h, hz), Vec3::new(hx + 0.6, wall_h, 0.6)),
                    (Vec3::new(-hx, g0 + wall_h, 0.0), Vec3::new(0.6, wall_h, hz + 0.6)),
                    (Vec3::new(hx, g0 + wall_h, 0.0), Vec3::new(0.6, wall_h, hz + 0.6)),
                ] {
                    push(&mut shapes, M_BOX, off, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, wall_c, 0.02, 0.35);
                }
                // ceiling ring at 15m, skylight hole in the middle
                let cy = g0 + 15.0;
                let hole = 26.0 * ws;
                let band_z = (hz - hole) * 0.5;
                let band_x = (hx - hole) * 0.5;
                for (off, half) in [
                    (Vec3::new(0.0, cy, -(hole + band_z)), Vec3::new(hx, 0.45, band_z)),
                    (Vec3::new(0.0, cy, hole + band_z), Vec3::new(hx, 0.45, band_z)),
                    (Vec3::new(-(hole + band_x), cy, 0.0), Vec3::new(band_x, 0.45, hole)),
                    (Vec3::new(hole + band_x, cy, 0.0), Vec3::new(band_x, 0.45, hole)),
                ] {
                    push(&mut shapes, M_BOX, off, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, mix4(wall_c, acc, 0.2), 0.04, 0.3);
                }
                // corner banks: wide tilted slabs, skate-park style
                for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let half = Vec3::new(7.5, 0.35, 5.5);
                    let yaw = (sz).atan2(sx) + std::f32::consts::FRAC_PI_4 * if sx * sz > 0.0 { 1.0 } else { -1.0 };
                    let rot = Quat::from_rotation_y(-yaw) * Quat::from_rotation_x(-0.55);
                    let p = Vec3::new(sx * (hx - 6.5), g0 + 2.0, sz * (hz - 6.5));
                    push(&mut shapes, M_BOX, p, rot, half, Sdf::RBox { half, round: 0.15 }, *rng.pick(&pal), 0.02, 0.3);
                }
                // spines: two big half-buried tubes to roll up
                for sx in [-1.0f32, 1.0] {
                    let r = 2.3;
                    let half = r * CAP_TALL_HH;
                    let rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
                    let p = Vec3::new(sx * 26.0 * ws, g0 - r * 0.35, 0.0);
                    push(&mut shapes, M_CAP_TALL, p, rot, Vec3::splat(r), Sdf::Capsule { r, half }, *rng.pick(&pal), 0.03, 0.35);
                    spots.push(Vec2::new(p.x, p.z));
                }
                // bleachers + light towers outside the walls
                for side in [-1.0f32, 1.0] {
                    for tier in 0..3 {
                        let half = Vec3::new(hx * 0.8, 0.5, 1.4);
                        let p = Vec3::new(0.0, g0 + 1.0 + tier as f32 * 1.4, side * (hz + 3.0 + tier as f32 * 2.2));
                        push(&mut shapes, M_BOX, p, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.08 }, mix4(PLINTH, acc, 0.15), 0.0, 0.2);
                    }
                }
                for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let r = 0.55;
                    let half = r * CAP_TALL_HH;
                    let base_p = Vec3::new(sx * (hx + 2.5), g0 + half, sz * (hz + 2.5));
                    push(&mut shapes, M_CAP_TALL, base_p, Quat::IDENTITY, Vec3::splat(r), Sdf::Capsule { r, half }, PLINTH, 0.0, 0.3);
                    push(&mut shapes, M_SPHERE, base_p + Vec3::Y * (half + 0.8), Quat::IDENTITY, Vec3::splat(0.9), Sdf::Sphere { r: 0.9 }, [1.0, 0.97, 0.85, 1.0], 1.0, 0.4);
                }
                // center circle
                let big = 6.0;
                let idx = push(&mut shapes, M_TORUS_THIN, Vec3::new(0.0, g0 + 0.12, 0.0), Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(big), Sdf::Torus { big, tube: big * 0.07 }, acc, 0.25, 0.4);
                let _ = idx;
            }
            10 => {
                // COLISEUM: arcing bleacher ring + torch towers around the pit
                for k in 0..14 {
                    let a = k as f32 / 14.0 * std::f32::consts::TAU;
                    let rr = 46.0 * ws;
                    let p2 = Vec2::new(a.cos() * rr, a.sin() * rr);
                    let yaw = Quat::from_rotation_y(-a + std::f32::consts::FRAC_PI_2);
                    for tier in 0..2 {
                        let half = Vec3::new(9.0, 0.6, 1.6);
                        let g0 = th(p2.x, p2.y);
                        push(&mut shapes, M_BOX, Vec3::new(p2.x, g0 + 1.2 + tier as f32 * 1.6, p2.y) + (yaw * Vec3::Z) * tier as f32 * 2.0, yaw, half, Sdf::RBox { half, round: 0.1 }, mix4(PLINTH, acc, 0.2 + tier as f32 * 0.1), 0.0, 0.2);
                    }
                    spots.push(p2);
                }
                for k in 0..8 {
                    let a = k as f32 / 8.0 * std::f32::consts::TAU + 0.2;
                    let rr = 38.0 * ws;
                    let p2 = Vec2::new(a.cos() * rr, a.sin() * rr);
                    let r = 0.6;
                    let half = r * CAP_TALL_HH;
                    let g0 = th(p2.x, p2.y);
                    let base_p = Vec3::new(p2.x, g0 + half, p2.y);
                    push(&mut shapes, M_CAP_TALL, base_p, Quat::IDENTITY, Vec3::splat(r), Sdf::Capsule { r, half }, mix4(PLINTH, acc, 0.3), 0.0, 0.3);
                    push(&mut shapes, M_SPHERE, base_p + Vec3::Y * (half + 0.7), Quat::IDENTITY, Vec3::splat(0.8), Sdf::Sphere { r: 0.8 }, acc, 1.0, 0.3);
                    spots.push(p2);
                }
            }
            7 => {
                // colonnade: a ring of columns around the plaza
                for i in 0..10 {
                    let a = i as f32 / 10.0 * std::f32::consts::TAU;
                    let p2 = Vec2::new(a.cos() * 21.0 * ws, a.sin() * 21.0 * ws);
                    let r = 0.7;
                    let half = r * CAP_TALL_HH;
                    let g0 = th(p2.x, p2.y);
                    push(&mut shapes, M_CAP_TALL, Vec3::new(p2.x, g0 + half, p2.y), Quat::IDENTITY, Vec3::splat(r), Sdf::Capsule { r, half }, mix4(PLINTH, acc, 0.35), 0.05, 0.35);
                    spots.push(p2);
                }
            }
            13 => {
                // ============== MONKEY RUN: canopy coaster jungle ==============
                // Dusk forest, eight giant trees, three LONG vine-line
                // coasters swooping between canopies (each one floodable —
                // when the water comes, the whole crew gets flushed
                // downstream), treehouse chambers, rope bridges, swaying
                // vines, parrots, banana straights and mud wallows.
                let tube = |shapes: &mut Vec<Shape>, grav: &mut Vec<GravNode>, a: Vec3, b: Vec3, r: f32, col: [f32; 4], glow: f32| {
                    let mid = (a + b) * 0.5;
                    let axis = (b - a).normalize_or(Vec3::Z);
                    let half = (b - a).length() * 0.62;
                    let rot = Quat::from_rotation_arc(Vec3::Y, axis);
                    push(shapes, M_SHELL, mid, rot, Vec3::new(r, half, r), Sdf::Shell { r: r * 0.95, half, thick: r * 0.05 }, [col[0], col[1], col[2], 0.55], glow, 0.45);
                    for t in [0.25f32, 0.75] {
                        grav.push(GravNode { pos: a.lerp(b, t), down: Vec3::NEG_Y, r: r * 1.55, tube: true, follow: None });
                    }
                };
                let cut_s = |shapes: &mut Vec<Shape>, p: Vec3, r: f32| {
                    let i = push(shapes, M_SPHERE, p, Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, [0.0, 0.0, 0.0, 0.0], 0.0, 0.0);
                    shapes[i].cut = true;
                };
                let rim = |shapes: &mut Vec<Shape>, p: Vec3, axis: Vec3, r: f32, col: [f32; 4]| {
                    let rot = Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y));
                    push(shapes, M_TORUS_FAT, p, rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(r * 1.02), Sdf::Torus { big: r * 1.02, tube: r * 0.14 }, col, 0.25, 0.45);
                };
                let swoop = |shapes: &mut Vec<Shape>, grav: &mut Vec<GravNode>, wp: &mut Vec<Vec3>, a: Vec3, b: Vec3, side: f32, sag: f32, r: f32, col: [f32; 4], glow: f32| {
                    let d = b - a;
                    let len = d.length().max(0.01);
                    let dirf = d / len;
                    let lat = Vec3::new(-dirf.z, 0.0, dirf.x).normalize_or(Vec3::X);
                    let ctrl = (a + b) * 0.5 + lat * (side * len) + Vec3::Y * (sag * len);
                    let n = ((len / 16.0) as usize).clamp(4, 22);
                    let bez = |t: f32| a * (1.0 - t) * (1.0 - t) + ctrl * (2.0 * (1.0 - t) * t) + b * (t * t);
                    for k in 0..n {
                        let p0 = bez(k as f32 / n as f32);
                        let p1 = bez((k + 1) as f32 / n as f32);
                        tube(shapes, grav, p0, p1, r, col, glow);
                        if k + 1 < n {
                            cut_s(shapes, p1, r * 0.94);
                        }
                        wp.push(p0);
                    }
                    wp.push(b);
                };
                let arc = |shapes: &mut Vec<Shape>, grav: &mut Vec<GravNode>, wp: &mut Vec<Vec3>, c: Vec3, u: Vec3, v: Vec3, big_r: f32, a0: f32, a1: f32, r: f32, col: [f32; 4], glow: f32| {
                    let n = (((a1 - a0).abs() / 0.24).ceil() as usize).max(2);
                    let st = (a1 - a0) / n as f32;
                    let at = |k: usize| {
                        let a = a0 + st * k as f32;
                        c + (u * a.cos() + v * a.sin()) * big_r
                    };
                    for k in 0..n {
                        tube(shapes, grav, at(k), at(k + 1), r, col, glow);
                        if k + 1 < n {
                            cut_s(shapes, at(k + 1), r * 0.94);
                        }
                        wp.push(at(k));
                    }
                    wp.push(at(n));
                };

                // ---- the forest giants ----
                let bark = mix4(PLINTH, [0.30, 0.20, 0.12, 1.0], 0.7);
                let leaf = [0.16, 0.34, 0.14, 1.0];
                let mut ttop: Vec<Vec3> = Vec::new();
                let mut house_r = vec![0.0f32; 8];
                let mut house_c = vec![Vec3::ZERO; 8];
                for k in 0..8usize {
                    let a = k as f32 / 8.0 * std::f32::consts::TAU + rng.range(-0.2, 0.2);
                    let rr = rng.range(210.0, 340.0) * ws;
                    let (x, z) = (a.cos() * rr, a.sin() * rr);
                    let g0 = th(x, z);
                    let tr = rng.range(3.4, 5.2);
                    let thh = rng.range(34.0, 52.0);
                    let half = thh * 0.5;
                    push(&mut shapes, M_CAP_TALL, Vec3::new(x, g0 + half, z), Quat::IDENTITY, Vec3::new(tr, half / CAP_TALL_HH.max(1.0), tr), Sdf::Capsule { r: tr, half }, bark, 0.0, 0.25);
                    let top = Vec3::new(x, g0 + thh, z);
                    for c in 0..3u32 {
                        let ca = c as f32 * 2.1 + k as f32;
                        let cr = rng.range(9.0, 15.0);
                        let cp = top + Vec3::new(ca.cos() * cr * 0.6, rng.range(1.0, 6.0), ca.sin() * cr * 0.6);
                        push(&mut shapes, M_SPHERE, cp, Quat::IDENTITY, Vec3::splat(cr), Sdf::Sphere { r: cr }, mix4(leaf, acc, 0.08 * c as f32), 0.02, 0.15);
                    }
                    if k % 2 == 0 {
                        // treehouse: a hollow room in the canopy
                        let hr = 8.0;
                        let hc = top + Vec3::new(0.0, 4.0, 0.0);
                        let hi2 = push(&mut shapes, M_SPHERE, hc, Quat::IDENTITY, Vec3::splat(hr), Sdf::Sphere { r: hr }, [acc[0], acc[1], acc[2], 0.18], 0.10, 0.5);
                        shapes[hi2].blend = true;
                        for hoop in [Quat::IDENTITY, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)] {
                            push(&mut shapes, M_TORUS_THIN, hc, hoop, Vec3::splat(hr * 0.99), Sdf::Torus { big: hr * 0.99, tube: 0.32 }, mix4(acc, [1.0; 4], 0.25), 0.55, 0.4);
                        }
                        cut_s(&mut shapes, hc, hr - 1.0);
                        grav.push(GravNode { pos: hc, down: Vec3::NEG_Y, r: hr * 1.8, tube: true, follow: None });
                        house_r[k] = hr;
                        house_c[k] = hc;
                    }
                    ttop.push(top);
                }

                // ---- three vine lines: long floodable coasters ----
                let lines: [(&str, [usize; 5], f32, bool); 3] = [
                    ("THE CONSTRICTOR", [0, 1, 2, 3, 4], 8.0, false),
                    ("CANOPY EXPRESS", [4, 5, 6, 7, 0], 10.0, true),
                    ("THE BIG SQUEEZE", [2, 7, 5, 1, 3], 12.5, true),
                ];
                for (name, seq, r, wants_loop) in lines {
                    let s0 = shapes.len();
                    let mut wp: Vec<Vec3> = Vec::new();
                    let col = mix4(*rng.pick(&pal), acc, 0.35);
                    // root mouth: ground entrance + slingshot up the first climb
                    let base = ttop[seq[0]];
                    let gm = Vec3::new(base.x + 20.0, th(base.x + 20.0, base.z) + r * 0.85, base.z + 14.0);
                    let mut cur = if house_r[seq[0]] > 0.0 {
                        house_c[seq[0]] + (gm - house_c[seq[0]]).normalize_or(Vec3::Y) * (house_r[seq[0]] - 0.5)
                    } else {
                        base + Vec3::Y * 3.0
                    };
                    swoop(&mut shapes, &mut grav, &mut wp, gm, cur, 0.10, 0.16, r, col, 0.22);
                    if house_r[seq[0]] > 0.0 {
                        cut_s(&mut shapes, house_c[seq[0]] + (gm - house_c[seq[0]]).normalize_or(Vec3::Y) * house_r[seq[0]], r * 1.12);
                        cur = house_c[seq[0]];
                        wp.push(cur);
                    }
                    rim(&mut shapes, gm, (cur - gm).normalize_or(Vec3::Y), r, mix4(acc, [1.0; 4], 0.3));
                    {
                        let lp = Vec3::new(gm.x, th(gm.x, gm.z) + 1.1, gm.z);
                        let ld = (cur - gm).normalize_or(Vec3::Y);
                        let li = push(&mut shapes, M_TORUS_FAT, lp, Quat::from_rotation_arc(Vec3::Y, ld) * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(2.2), Sdf::Torus { big: 2.2, tube: 0.4 }, mix4(acc, [1.0; 4], 0.4), 0.9, 0.5);
                        launchers.push(Launcher { pos: lp, dir: ld, power: 30.0, timer: 0.0, shape: li });
                    }
                    // canopy to canopy, banking alternately
                    for (si, wpair) in seq.windows(2).enumerate() {
                        let (ta, tb) = (wpair[0], wpair[1]);
                        let side = if si % 2 == 0 { 0.17 } else { -0.17 };
                        let from = if house_r[ta] > 0.0 { house_c[ta] } else { ttop[ta] + Vec3::Y * 3.0 };
                        let mut to = if house_r[tb] > 0.0 {
                            house_c[tb] + (from - house_c[tb]).normalize_or(Vec3::X) * (house_r[tb] - 0.5)
                        } else {
                            ttop[tb] + Vec3::Y * 3.0
                        };
                        if house_r[ta] > 0.0 {
                            let exit = house_c[ta] + (to - house_c[ta]).normalize_or(Vec3::X) * (house_r[ta] - 0.5);
                            cut_s(&mut shapes, house_c[ta] + (to - house_c[ta]).normalize_or(Vec3::X) * house_r[ta], r * 1.12);
                            swoop(&mut shapes, &mut grav, &mut wp, exit, to, side, -0.10, r, col, 0.16);
                        } else {
                            swoop(&mut shapes, &mut grav, &mut wp, from, to, side, -0.10, r, col, 0.16);
                        }
                        if house_r[tb] > 0.0 {
                            cut_s(&mut shapes, house_c[tb] + (from - house_c[tb]).normalize_or(Vec3::X) * house_r[tb], r * 1.12);
                            to = house_c[tb];
                            wp.push(to);
                        }
                        // one grand vertical loop mid-line
                        if wants_loop && si == 2 {
                            let last = *wp.last().unwrap();
                            let dirf = (to - from).normalize_or(Vec3::X);
                            let flat_f = Vec3::new(dirf.x, 0.0, dirf.z).normalize_or(Vec3::X);
                            let lr = r * 2.3;
                            let lc = last + flat_f * (lr * 0.2) + Vec3::Y * lr;
                            arc(&mut shapes, &mut grav, &mut wp, lc, -Vec3::Y, flat_f, lr, 0.0, std::f32::consts::TAU, r, mix4(col, [1.0; 4], 0.2), 0.3);
                            cut_s(&mut shapes, last, r * 0.9);
                        }
                    }
                    // final descent to a second ground mouth
                    let last = *wp.last().unwrap();
                    let out = Vec3::new(last.x - 55.0, 0.0, last.z + 40.0);
                    let m1 = Vec3::new(out.x, th(out.x, out.z) + r * 0.85, out.z);
                    swoop(&mut shapes, &mut grav, &mut wp, last, m1, -0.12, -0.06, r, col, 0.2);
                    rim(&mut shapes, m1, (last - m1).normalize_or(Vec3::Y), r, acc);
                    // banana straights: boost inside the bore
                    if wp.len() > 8 {
                        zones.push(Zone { pos: wp[wp.len() / 3], r: r * 0.95, kind: ZoneKind::Boost });
                        zones.push(Zone { pos: wp[wp.len() * 2 / 3], r: r * 0.95, kind: ZoneKind::Boost });
                    }
                    let ids: Vec<u32> = (s0..shapes.len()).filter(|&x| !shapes[x].cut).map(|x| x as u32).collect();
                    routes.push((wp, r));
                    route_names.push(name.to_string());
                    route_shapes.push(ids);
                    floodable.push(routes.len() - 1);
                }

                // ---- jungle dressing ----
                for k in 0..5usize {
                    let (a, b) = (ttop[k], ttop[(k + 3) % 8]);
                    let bm = (a + b) * 0.5 + Vec3::Y * 1.5;
                    let baxis = (b - a).normalize_or(Vec3::X);
                    let bhalf = a.distance(b) * 0.5;
                    push(&mut shapes, M_CAP_TALL, bm, Quat::from_rotation_arc(Vec3::Y, baxis), Vec3::new(0.5, bhalf / CAP_TALL_HH.max(1.0), 0.5), Sdf::Capsule { r: 0.5, half: bhalf }, mix4(bark, acc, 0.2), 0.15, 0.4);
                }
                for k in 0..6usize {
                    let t = ttop[k % 8];
                    let base = t + Vec3::new(rng.range(-8.0, 8.0), -2.0, rng.range(-8.0, 8.0));
                    let half = 5.0;
                    let i = push(&mut shapes, M_CAP_TALL, base - Vec3::Y * half, Quat::IDENTITY, Vec3::new(0.35, half / CAP_TALL_HH.max(1.0), 0.35), Sdf::Capsule { r: 0.35, half }, mix4(leaf, [0.0, 0.0, 0.0, 1.0], 0.3), 0.05, 0.3);
                    anims.push(Anim::Slide { shape: i, base: base - Vec3::Y * half, dir: Vec3::new(1.0, 0.0, 0.3).normalize(), amp: 3.5, rate: 0.55 + k as f32 * 0.08, phase: k as f32 * 1.1, rot: Quat::IDENTITY, scale: Vec3::new(0.35, half / CAP_TALL_HH.max(1.0), 0.35) });
                }
                for k in 0..4usize {
                    let hb = ttop[(k * 2 + 1) % 8];
                    let half = Vec3::new(0.8, 0.35, 1.2);
                    let pc = [[0.9, 0.2, 0.2, 1.0], [0.2, 0.5, 0.95, 1.0], [0.95, 0.8, 0.2, 1.0], [0.3, 0.85, 0.4, 1.0]][k];
                    let i = push(&mut shapes, M_BOX, hb, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.1 }, pc, 0.4, 0.4);
                    anims.push(Anim::Orbit { shape: i, center: hb + Vec3::Y * 6.0, axis: Vec3::Y, r: 16.0 + k as f32 * 3.0, rate: 0.4 + k as f32 * 0.1, phase: k as f32 * 1.6, rot: Quat::IDENTITY, scale: half });
                }
                for k in 0..3usize {
                    let a = k as f32 * 2.4 + 0.9;
                    let (x, z) = (a.cos() * 120.0, a.sin() * 120.0);
                    zones.push(Zone { pos: Vec3::new(x, th(x, z) + 1.0, z), r: 11.0, kind: ZoneKind::Slip });
                }
                zones.push(Zone { pos: Vec3::new(0.0, th(0.0, 0.0) + 2.0, 0.0), r: 13.0, kind: ZoneKind::Vibe });
            }
            14 => {
                // ============ BOXBOWL: the classic enclosed pitch ============
                // Flat floor, four walls, a ceiling — all in the smin blend
                // group, so every wall-floor seam FILLETS into a drivable
                // quarter-pipe transition. 300 sparta cars stay IN.
                let g0 = th(0.0, 0.0);
                let (hx, hz, wh) = (56.0f32, 52.0f32, 13.0f32);
                let wallc = mix4(PLINTH, acc, 0.18);
                let mut wall = |shapes: &mut Vec<Shape>, pos: Vec3, half: Vec3, col: [f32; 4], al: f32| {
                    let i = push(shapes, M_BOX, pos, Quat::IDENTITY, half, Sdf::RBox { half, round: 0.4 }, [col[0], col[1], col[2], al], 0.05, 0.4);
                    shapes[i].blend = true;
                    i
                };
                // floor slab (the fillet partner)
                wall(&mut shapes, Vec3::new(0.0, g0 + 0.35, 0.0), Vec3::new(hx, 0.6, hz), mix4(style.grass, acc, 0.1), 1.0);
                wall(&mut shapes, Vec3::new(-hx, g0 + wh * 0.5, 0.0), Vec3::new(1.2, wh * 0.5 + 1.0, hz), wallc, 1.0);
                wall(&mut shapes, Vec3::new(hx, g0 + wh * 0.5, 0.0), Vec3::new(1.2, wh * 0.5 + 1.0, hz), wallc, 1.0);
                wall(&mut shapes, Vec3::new(0.0, g0 + wh * 0.5, -hz), Vec3::new(hx, wh * 0.5 + 1.0, 1.2), wallc, 1.0);
                wall(&mut shapes, Vec3::new(0.0, g0 + wh * 0.5, hz), Vec3::new(hx, wh * 0.5 + 1.0, 1.2), wallc, 1.0);
                // quarter-pipe wedges along every floor edge (blend melts them in)
                for (px, pz, ry) in [(-hx + 1.6, 0.0, 0.0f32), (hx - 1.6, 0.0, 0.0), (0.0, -hz + 1.6, 1.5707), (0.0, hz - 1.6, 1.5707)] {
                    let half = Vec3::new(2.2, 2.2, if ry == 0.0 { hz } else { hx });
                    let rot = Quat::from_rotation_y(ry) * Quat::from_rotation_z(0.785);
                    let i = push(&mut shapes, M_BOX, Vec3::new(px, g0 + 0.8, pz), rot, half, Sdf::RBox { half, round: 0.5 }, wallc, 0.03, 0.4);
                    shapes[i].blend = true;
                }
                // glass ceiling: the lid on the box
                let ci = push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + wh + 1.4, 0.0), Quat::IDENTITY, Vec3::new(hx + 1.0, 0.8, hz + 1.0), Sdf::RBox { half: Vec3::new(hx + 1.0, 0.8, hz + 1.0), round: 0.3 }, [acc[0], acc[1], acc[2], 0.28], 0.08, 0.5);
                shapes[ci].blend = true;
                // corner light towers
                for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    push(&mut shapes, M_CAP_TALL, Vec3::new(sx * (hx - 3.0), g0 + 7.0, sz * (hz - 3.0)), Quat::IDENTITY, Vec3::new(0.7, 7.0 / CAP_TALL_HH.max(1.0), 0.7), Sdf::Capsule { r: 0.7, half: 7.0 }, mix4(acc, [1.0; 4], 0.4), 0.8, 0.4);
                }
            }
            15 => {
                // ============ DROPSHOT: the floor is the goal ============
                // A raised tile platform over a glowing basin. Ball strikes
                // damage tiles; broken tiles FALL AWAY (real holes in the
                // SDF); a ball through the floor scores against that side.
                let g0 = th(0.0, 0.0);
                let py = g0 + 6.0;
                drop_pit = Some(py);
                // the basin below: dark catch floor + glow ribs
                push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + 0.4, 0.0), Quat::IDENTITY, Vec3::new(30.0, 0.6, 30.0), Sdf::RBox { half: Vec3::new(30.0, 0.6, 30.0), round: 0.3 }, [0.06, 0.05, 0.10, 1.0], 0.05, 0.3);
                for k in 0..6 {
                    let z = -25.0 + k as f32 * 10.0;
                    push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + 0.9, z), Quat::IDENTITY, Vec3::new(28.0, 0.12, 0.35), Sdf::RBox { half: Vec3::new(28.0, 0.12, 0.35), round: 0.05 }, mix4(acc, [1.0; 4], 0.3), 0.8, 0.4);
                }
                // the tile deck: 12 x 7, team-tinted halves, hp by depth
                let (tw, td) = (4.1f32, 3.35f32);
                for iz in 0..7i32 {
                    for ix in 0..12i32 {
                        let x = (ix as f32 - 5.5) * (tw * 2.0 + 0.22);
                        let z = (iz as f32 - 3.0) * (td * 2.0 + 0.22);
                        let team_col = if z < -0.1 { [0.22, 0.42, 0.95, 1.0] } else if z > 0.1 { [0.95, 0.48, 0.16, 1.0] } else { [0.5, 0.5, 0.55, 1.0] };
                        let hp = if iz == 3 { 1 } else if (iz - 3).abs() == 1 { 2 } else { 3 };
                        let half = Vec3::new(tw, 0.5, td);
                        let pos = Vec3::new(x, py, z);
                        let sdf = Sdf::RBox { half, round: 0.12 };
                        let i = push(&mut shapes, M_BOX, pos, Quat::IDENTITY, half, sdf, mix4(team_col, PLINTH, 0.35), 0.10, 0.45);
                        tiles.push(Tile { shape: i, kind: TileKind::Drop, hp, max_hp: hp, down_t: 0.0, base: pos, rot: Quat::IDENTITY, scale: half, keep: sdf });
                    }
                }
                // glass rails keep the ball over the deck; open ends for cars
                for sx in [-1.0f32, 1.0] {
                    push(&mut shapes, M_BOX, Vec3::new(sx * 51.5, py + 3.4, 0.0), Quat::IDENTITY, Vec3::new(0.5, 3.6, 26.0), Sdf::RBox { half: Vec3::new(0.5, 3.6, 26.0), round: 0.2 }, [acc[0], acc[1], acc[2], 0.30], 0.12, 0.5);
                }
                for sz in [-1.0f32, 1.0] {
                    push(&mut shapes, M_BOX, Vec3::new(0.0, py + 3.4, sz * 26.0), Quat::IDENTITY, Vec3::new(51.5, 3.6, 0.5), Sdf::RBox { half: Vec3::new(51.5, 3.6, 0.5), round: 0.2 }, [acc[0], acc[1], acc[2], 0.30], 0.12, 0.5);
                    // entry ramps up from the plain at both ends
                    let rz = sz * 34.0;
                    let rot = Quat::from_rotation_x(-sz * 0.42);
                    let half = Vec3::new(9.0, 0.5, 9.5);
                    push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + 3.2, rz), rot, half, Sdf::RBox { half, round: 0.2 }, mix4(PLINTH, acc, 0.2), 0.05, 0.4);
                }
            }
            16 => {
                // ============ GATEWORKS: the ball is the key ============
                // Each net hides behind a gate tunnel sealed by a DOOR panel.
                // Hit the door with the BALL and it slides open for six
                // seconds — then it seals again. Time your push.
                let g0 = th(0.0, 0.0);
                for sz in [-1.0f32, 1.0] {
                    let z = sz * 27.0;
                    let flankc = mix4(PLINTH, acc, 0.25);
                    for sx in [-1.0f32, 1.0] {
                        let half = Vec3::new(9.0, 4.5, 1.0);
                        push(&mut shapes, M_BOX, Vec3::new(sx * 14.0, g0 + 4.5, z), Quat::IDENTITY, half, Sdf::RBox { half, round: 0.3 }, flankc, 0.05, 0.4);
                    }
                    let lin = Vec3::new(5.2, 0.8, 1.0);
                    push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + 8.6, z), Quat::IDENTITY, lin, Sdf::RBox { half: lin, round: 0.2 }, flankc, 0.05, 0.4);
                    // the door itself: a tile the ball can open
                    let half = Vec3::new(5.0, 3.9, 0.55);
                    let pos = Vec3::new(0.0, g0 + 3.9, z);
                    let sdf = Sdf::RBox { half, round: 0.15 };
                    let i = push(&mut shapes, M_BOX, pos, Quat::IDENTITY, half, sdf, mix4(acc, [1.0; 4], 0.15), 0.35, 0.5);
                    tiles.push(Tile { shape: i, kind: TileKind::Door, hp: 1, max_hp: 1, down_t: 0.0, base: pos, rot: Quat::IDENTITY, scale: half, keep: sdf });
                }
                // side vault doors hiding boost alcoves
                for sx in [-1.0f32, 1.0] {
                    let pos = Vec3::new(sx * 34.0, g0 + 2.6, 0.0);
                    let half = Vec3::new(0.5, 2.6, 4.0);
                    let sdf = Sdf::RBox { half, round: 0.15 };
                    let i = push(&mut shapes, M_BOX, pos, Quat::from_rotation_y(0.0), half, sdf, mix4(acc, [1.0; 4], 0.2), 0.3, 0.5);
                    tiles.push(Tile { shape: i, kind: TileKind::Door, hp: 1, max_hp: 1, down_t: 0.0, base: pos, rot: Quat::IDENTITY, scale: half, keep: sdf });
                    push(&mut shapes, M_BOX, Vec3::new(sx * 38.5, g0 + 2.6, 0.0), Quat::IDENTITY, Vec3::new(4.0, 2.6, 4.2), Sdf::RBox { half: Vec3::new(4.0, 2.6, 4.2), round: 0.3 }, mix4(PLINTH, acc, 0.2), 0.04, 0.4);
                    zones.push(Zone { pos: Vec3::new(sx * 37.0, g0 + 1.2, 0.0), r: 3.0, kind: ZoneKind::Boost });
                }
                // a slow revolving centre gate to weave around
                {
                    let half = Vec3::new(10.0, 3.2, 0.6);
                    let i = push(&mut shapes, M_BOX, Vec3::new(0.0, g0 + 3.2, 0.0), Quat::IDENTITY, half, Sdf::RBox { half, round: 0.2 }, mix4(*rng.pick(&pal), acc, 0.3), 0.15, 0.4);
                    anims.push(Anim::Spin { shape: i, pos: Vec3::new(0.0, g0 + 3.2, 0.0), scale: half, base: Quat::IDENTITY, axis: Vec3::Y, rate: 0.35 });
                }
            }
            17 => {
                // ============ KUIPER: the asteroid field ============
                // Mini planets with their own gravity, carved throughways,
                // spinning rings — and FREE asteroids: light ones you shove,
                // heavy ones that shove back, all colliding out there.
                let cut_s = |shapes: &mut Vec<Shape>, p: Vec3, r: f32| {
                    let i = push(shapes, M_SPHERE, p, Quat::IDENTITY, Vec3::splat(r), Sdf::Sphere { r }, [0.0, 0.0, 0.0, 0.0], 0.0, 0.0);
                    shapes[i].cut = true;
                };
                let rock = [0.22, 0.20, 0.24, 1.0];
                for k in 0..12u32 {
                    let a = k as f32 * 0.5236 + rng.range(-0.2, 0.2);
                    let rr = rng.range(60.0, 200.0) * ws;
                    let pr = rng.range(7.0, 18.0);
                    let cp = Vec3::new(a.cos() * rr, rng.range(18.0, 70.0), a.sin() * rr);
                    let col = mix4(rock, *rng.pick(&pal), 0.25);
                    push(&mut shapes, M_SPHERE, cp, Quat::IDENTITY, Vec3::splat(pr), Sdf::Sphere { r: pr }, col, 0.02, 0.3);
                    if k % 2 == 0 {
                        grav.push(GravNode { pos: cp, down: Vec3::NEG_Y, r: pr * 2.2, tube: true, follow: None });
                    }
                    if k % 4 == 0 && pr > 10.0 {
                        // a throughway carved clean through the planet
                        let ax = Vec3::new(rng.range(-1.0, 1.0), rng.range(-0.4, 0.4), rng.range(-1.0, 1.0)).normalize_or(Vec3::X);
                        let steps = 7;
                        for t in 0..=steps {
                            let f = t as f32 / steps as f32 * 2.0 - 1.0;
                            cut_s(&mut shapes, cp + ax * (f * pr * 1.02), (pr * 0.28).max(2.6));
                        }
                    }
                    if k % 3 == 0 {
                        let i = push(&mut shapes, M_TORUS_THIN, cp, Quat::from_rotation_x(rng.range(0.0, 1.5)), Vec3::splat(pr * 1.5), Sdf::Torus { big: pr * 1.5, tube: (pr * 0.06).max(0.3) }, mix4(acc, [1.0; 4], 0.3), 0.6, 0.4);
                        anims.push(Anim::Spin { shape: i, pos: cp, scale: Vec3::splat(pr * 1.5), base: Quat::from_rotation_x(rng.range(0.0, 1.5)), axis: Vec3::new(0.3, 1.0, 0.2).normalize(), rate: 0.3 + k as f32 * 0.05 });
                    }
                }
                // the free bodies: three weight classes
                for k in 0..8u32 {
                    let (r, mass) = match k {
                        0..=2 => (rng.range(1.5, 2.4), rng.range(2.0, 4.0)),
                        3..=5 => (rng.range(3.0, 4.0), rng.range(9.0, 14.0)),
                        _ => (rng.range(5.4, 6.6), rng.range(34.0, 46.0)),
                    };
                    let a = k as f32 * 0.9 + 0.4;
                    let pos = Vec3::new(a.cos() * rng.range(30.0, 90.0), rng.range(10.0, 40.0), a.sin() * rng.range(30.0, 90.0));
                    // Sdf::None: the MESH renders, but the static world SDF
                    // must not know these exist — otherwise the ball bounces
                    // off an asteroid-as-scenery before the shove logic ever
                    // sees closing velocity (and each body fights its own
                    // SDF). step_bodies owns ALL their physics.
                    let i = push(&mut shapes, M_SPHERE, pos, Quat::IDENTITY, Vec3::splat(r), Sdf::None, mix4(rock, [1.0, 0.9, 0.75, 1.0], 0.15), 0.03, 0.35);
                    bodies.push(Body { shape: i, pos, vel: Vec3::new(a.sin(), 0.0, -a.cos()) * rng.range(0.4, 1.6), r, mass });
                }
                for k in 0..3u32 {
                    let a = k as f32 * 2.1;
                    let (x, z) = (a.cos() * 40.0, a.sin() * 40.0);
                    let lp = Vec3::new(x, th(x, z) + 1.1, z);
                    let dir = Vec3::new(-x * 0.2, 40.0, -z * 0.2).normalize();
                    let i = push(&mut shapes, M_TORUS_FAT, lp, Quat::from_rotation_arc(Vec3::Y, dir) * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::splat(2.2), Sdf::Torus { big: 2.2, tube: 0.4 }, mix4(acc, [1.0; 4], 0.4), 0.9, 0.5);
                    launchers.push(Launcher { pos: lp, dir, power: 34.0, timer: 0.0, shape: i });
                }
            }
            18 => {
                // ============ ZOETROPE: the rotating corridors ============
                // Three concentric rings of wall segments turning at
                // different speeds and directions — passages line up,
                // then they don't. Time the gaps to the core.
                let g0 = th(0.0, 0.0);
                let rings_def: [(f32, usize, f32, f32); 3] = [
                    (16.0, 5, 0.26, 6.4),
                    (26.0, 6, -0.17, 7.2),
                    (36.0, 7, 0.11, 8.0),
                ];
                for (ri, &(rad, slots, rate, wh)) in rings_def.iter().enumerate() {
                    let arc = rad * std::f32::consts::TAU / slots as f32;
                    let half = Vec3::new(arc * 0.34, wh * 0.5, 0.9);
                    for k in 0..slots {
                        let ph = k as f32 / slots as f32 * std::f32::consts::TAU;
                        let col = mix4(*rng.pick(&pal), acc, 0.25 + ri as f32 * 0.1);
                        let i = push(&mut shapes, M_BOX, Vec3::new(rad, g0 + wh * 0.5, 0.0), Quat::IDENTITY, half, Sdf::RBox { half, round: 0.25 }, col, 0.10, 0.45);
                        anims.push(Anim::Orbit {
                            shape: i,
                            center: Vec3::new(0.0, g0 + wh * 0.5, 0.0),
                            axis: Vec3::Y,
                            r: rad,
                            rate,
                            phase: ph,
                            rot: Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                            scale: half,
                        });
                    }
                }
                // the prize core: emissive column + a vibe pocket
                push(&mut shapes, M_CAP_TALL, Vec3::new(0.0, g0 + 5.0, 0.0), Quat::IDENTITY, Vec3::new(1.2, 5.0 / CAP_TALL_HH.max(1.0), 1.2), Sdf::Capsule { r: 1.2, half: 5.0 }, mix4(acc, [1.0; 4], 0.4), 0.9, 0.4);
                zones.push(Zone { pos: Vec3::new(0.0, g0 + 2.0, 0.0), r: 9.0, kind: ZoneKind::Boost });
            }
            _ => {}
        }

        // ---- spawn plaza: plinths with the red display Dominus -----------
        if style.focal == 0 {
            for i in 0..5 {
                let a = i as f32 / 5.0 * std::f32::consts::TAU + 0.4;
                let p = Vec3::new(a.cos() * 17.0 * ws, 0.0, a.sin() * 17.0 * ws);
                let half = Vec3::new(1.05, 0.42, 2.0);
                let ground = th(p.x, p.z);
                let plinth_pos = Vec3::new(p.x, ground + half.y, p.z);
                let yaw = Quat::from_rotation_y(-a + std::f32::consts::FRAC_PI_2);
                push(&mut shapes, M_BOX, plinth_pos, yaw, half, Sdf::RBox { half, round: 0.06 }, PLINTH, 0.0, 0.35);
                decor_cars.push(Mat4::from_rotation_translation(
                    yaw,
                    plinth_pos + Vec3::Y * (half.y + car_half_h + 0.01),
                ));
            }
        }

        // ---- net goals: pockets of frame bars to shoot the ball into -----
        let mut nets: Vec<NetGoal> = Vec::new();
        {
            let frame_c = mix4(PLINTH, [1.0, 1.0, 1.0, 1.0], 0.5);
            let add_net = |shapes: &mut Vec<Shape>,
                               nets: &mut Vec<NetGoal>,
                               center: Vec3,
                               rot: Quat,
                               hw: f32,
                               hh: f32,
                               depth: f32| {
                let shape0 = shapes.len();
                let l = |p: Vec3| center + rot * p;
                let bar = 0.09;
                let parts: [(Vec3, Vec3); 6] = [
                    (Vec3::new(hw + bar, 0.0, 0.0), Vec3::new(bar, hh + bar * 2.0, bar)), // posts
                    (Vec3::new(-(hw + bar), 0.0, 0.0), Vec3::new(bar, hh + bar * 2.0, bar)),
                    (Vec3::new(0.0, hh + bar, 0.0), Vec3::new(hw + bar * 2.0, bar, bar)), // crossbars
                    (Vec3::new(0.0, -(hh + bar), 0.0), Vec3::new(hw + bar * 2.0, bar, bar)),
                    (Vec3::new(hw, 0.0, -depth * 0.5), Vec3::new(0.05, hh, depth * 0.5)), // side panels
                    (Vec3::new(-hw, 0.0, -depth * 0.5), Vec3::new(0.05, hh, depth * 0.5)),
                ];
                for (off, half) in parts {
                    push(shapes, M_BOX, l(off), rot, half, Sdf::RBox { half, round: 0.04 }, frame_c, 0.06, 0.4);
                }
                // glowing back panel: the thing you aim at
                let bh = Vec3::new(hw, hh, 0.05);
                push(shapes, M_BOX, l(Vec3::new(0.0, 0.0, -depth)), rot, bh, Sdf::RBox { half: bh, round: 0.03 }, acc, 0.30 + style.glow * 0.4, 0.5);
                nets.push(NetGoal {
                    pos: center,
                    rot,
                    inv_rot: rot.inverse(),
                    hw,
                    hh,
                    depth,
                    shape0,
                    nshapes: shapes.len() - shape0,
                    timer: 0.0,
                });
            };

            if style.focal == 8 || style.focal == 9 {
                // paired goals facing each other across the pitch
                let (z0, hw, hh) = if style.focal == 9 { (56.0, 5.4, 2.1) } else { (38.0, 4.6, 1.9) };
                for side in [-1.0f32, 1.0] {
                    let z = z0 * ws * side;
                    let g0 = th(0.0, z);
                    let rot = Quat::from_rotation_y(if side > 0.0 { 0.0 } else { std::f32::consts::PI });
                    add_net(&mut shapes, &mut nets, Vec3::new(0.0, g0 + hh + 0.05, z), rot, hw, hh, 2.6);
                    spots.push(Vec2::new(0.0, z));
                }
            }
            for _ in 0..style.nets {
                let Some(xz) = place(&mut rng, &mut spots, 30.0 * ws, 380.0 * ws, 22.0 * ws) else { continue };
                let hw = rng.range(1.2, 4.0);
                let hh = rng.range(0.9, 2.4);
                let depth = rng.range(1.2, 2.6);
                let yaw = Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU));
                if rng.f() < 0.45 {
                    // floating tilted target: shoot up through the mouth
                    let lift = rng.range(6.0, 22.0);
                    let tilt = Quat::from_rotation_x(rng.range(-0.7, -0.15));
                    let rot = yaw * tilt;
                    let c0 = Vec3::new(xz.x, th(xz.x, xz.y) + lift, xz.y);
                    add_net(&mut shapes, &mut nets, c0, rot, hw, hh, depth);
                } else {
                    // grounded goal
                    let g0 = th(xz.x, xz.y);
                    add_net(&mut shapes, &mut nets, Vec3::new(xz.x, g0 + hh + 0.05, xz.y), yaw, hw, hh, depth);
                }
            }
        }

        // ---- boost pads: 6 big around the plaza, small ones everywhere ---
        for i in 0..6 {
            let a = i as f32 / 6.0 * std::f32::consts::TAU;
            let p = Vec2::new(a.cos() * 42.0 * ws, a.sin() * 42.0 * ws);
            spots.push(p);
            pads.push(Pad {
                pos: Vec3::new(p.x, th(p.x, p.y), p.y),
                big: true,
                timer: 0.0,
                phase: rng.range(0.0, 6.0),
            });
        }
        for _ in 0..style.small_pads {
            let Some(p) = place(&mut rng, &mut spots, 12.0 * ws, 400.0 * ws, 8.0 * ws) else { continue };
            pads.push(Pad {
                pos: Vec3::new(p.x, th(p.x, p.y), p.y),
                big: false,
                timer: 0.0,
                phase: rng.range(0.0, 6.0),
            });
        }

        // ---- physics accel: shape grid + gravity-node thinning ----
        let scell = 56.0f32;
        let sdim = ((TG_HALF * 2.0) / scell).ceil() as i32 + 2;
        let mut sgrid: Vec<Vec<u16>> = vec![Vec::new(); (sdim * sdim) as usize];
        for (i, s) in shapes.iter().enumerate() {
            let reach = match s.sdf {
                Sdf::None => continue,
                Sdf::Sphere { r } => r + 4.0,
                Sdf::Torus { big, tube } => big + tube + 4.0,
                Sdf::Capsule { r, half } => r + half + 4.0,
                Sdf::RBox { half, .. } => half.length() + 4.0,
                Sdf::Shell { r, half, thick } => ((r + thick) * (r + thick) + half * half).sqrt() + 4.0,
            } + 6.0; // margin: probe/normal offsets never cross a cell edge blind
            let c0x = (((s.pos.x - reach + TG_HALF) / scell).floor() as i32).clamp(0, sdim - 1);
            let c1x = (((s.pos.x + reach + TG_HALF) / scell).floor() as i32).clamp(0, sdim - 1);
            let c0z = (((s.pos.z - reach + TG_HALF) / scell).floor() as i32).clamp(0, sdim - 1);
            let c1z = (((s.pos.z + reach + TG_HALF) / scell).floor() as i32).clamp(0, sdim - 1);
            for cz in c0z..=c1z {
                for cx in c0x..=c1x {
                    sgrid[(cz * sdim + cx) as usize].push(i as u16);
                }
            }
        }
        {
            // dense tube chains stack redundant gravity nodes: thin to 5.5 m
            let mut kept: Vec<GravNode> = Vec::with_capacity(grav.len());
            'g: for n in grav.drain(..) {
                for k in &kept {
                    if k.tube == n.tube && k.follow.is_none() && n.follow.is_none() && k.pos.distance_squared(n.pos) < 5.5 * 5.5 {
                        continue 'g;
                    }
                }
                kept.push(n);
            }
            grav = kept;
        }

        while route_names.len() < routes.len() {
            route_names.push(String::new());
            route_shapes.push(Vec::new());
        }
        World { grav, sgrid, scell, sdim, routes, route_names, route_shapes, floodable, floods: Vec::new(), flood_cd: 34.0, flood_news: None, tiles, bodies, drop_pit, pit_cd: 0.0, anims, zones, launchers, time: 0.0, tgrid, shapes, rings, nets, pads, decor_cars, seed, t_amp: amp, t_freq: freq }
    }

    /// Grip multiplier / boost / vibe queries for the pinball zones.
    pub fn zone(&self, p: Vec3) -> Option<ZoneKind> {
        self.zones.iter().find(|z| z.pos.distance_squared(p) < z.r * z.r).map(|z| z.kind)
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        let t = self.time;
        for a in &self.anims {
            match *a {
                Anim::Spin { shape, pos, scale, base, axis, rate } => {
                    let rot = Quat::from_axis_angle(axis, t * rate) * base;
                    let sh = &mut self.shapes[shape];
                    sh.mat = glam::Mat4::from_scale_rotation_translation(scale, rot, pos);
                    sh.inv_rot = rot.inverse();
                }
                Anim::Door { shape, base, rot, scale, axis, amp, rate, phase } => {
                    let p = base + axis * ((t * rate + phase).sin() * amp);
                    let sh = &mut self.shapes[shape];
                    sh.pos = p;
                    sh.mat = glam::Mat4::from_scale_rotation_translation(scale, rot, p);
                }
                Anim::Orbit { shape, center, axis, r, rate, phase, rot, scale } => {
                    let ax = axis.normalize_or(Vec3::Y);
                    let u = ax.any_orthonormal_vector();
                    let v = ax.cross(u);
                    let a = t * rate + phase;
                    let p = center + (u * a.cos() + v * a.sin()) * r;
                    let spin = Quat::from_axis_angle(ax, a) * rot;
                    let sh = &mut self.shapes[shape];
                    sh.pos = p;
                    sh.inv_rot = spin.inverse();
                    sh.mat = glam::Mat4::from_scale_rotation_translation(scale, spin, p);
                }
                Anim::Slide { shape, base, dir, amp, rate, phase, rot, scale } => {
                    let p = base + dir * ((t * rate + phase).sin() * amp);
                    let sh = &mut self.shapes[shape];
                    sh.pos = p;
                    sh.mat = glam::Mat4::from_scale_rotation_translation(scale, rot, p);
                }
            }
        }
        // gravity nodes riding on animated shapes (the roaming moon)
        for gi in 0..self.grav.len() {
            if let Some(fi) = self.grav[gi].follow {
                self.grav[gi].pos = self.shapes[fi].pos;
            }
        }
        // tiles: count open panels back in
        self.pit_cd = (self.pit_cd - dt).max(0.0);
        for ti in 0..self.tiles.len() {
            if self.tiles[ti].down_t > 0.0 {
                self.tiles[ti].down_t -= dt;
                if self.tiles[ti].down_t <= 0.0 {
                    let (shape, base, rot, scale, keep) = {
                        let t = &self.tiles[ti];
                        (t.shape, t.base, t.rot, t.scale, t.keep)
                    };
                    let sh = &mut self.shapes[shape];
                    sh.sdf = keep;
                    sh.pos = base;
                    sh.mat = glam::Mat4::from_scale_rotation_translation(scale, rot, base);
                    sh.flash = 0.6;
                }
            }
        }
        // ---- FLOODWATER: occasional tubes fill and flush the crew ----
        if !self.floodable.is_empty() {
            self.flood_cd -= dt;
            if self.flood_cd <= 0.0 {
                let pick = self.floodable[(self.time * 977.7) as usize % self.floodable.len()];
                self.floods.push((pick, 0.0, 11.0));
                self.flood_news = Some(pick);
                self.flood_cd = 48.0 + ((self.time * 131.3) as u32 % 32) as f32;
            }
        }
        if !self.floods.is_empty() {
            let mut lit: Vec<u32> = Vec::new();
            for f in self.floods.iter_mut() {
                f.1 += dt;
                if f.1 < f.2 {
                    lit.extend(self.route_shapes[f.0].iter().copied());
                }
            }
            for si in lit {
                let sh = &mut self.shapes[si as usize];
                sh.flash = sh.flash.max(0.85); // the waterline glow
            }
            self.floods.retain(|f| f.1 < f.2);
        }
        for l in self.launchers.iter_mut() {
            l.timer = (l.timer - dt).max(0.0);
        }
        for p in self.pads.iter_mut() {
            p.timer = (p.timer - dt).max(0.0);
        }
        for n in self.nets.iter_mut() {
            n.timer = (n.timer - dt).max(0.0);
        }
        for s in self.shapes.iter_mut() {
            s.flash = (s.flash - dt * 0.9).max(0.0);
        }
    }

    // ------------------------------------------------------------ SDF ----
    pub fn sdf(&self, p: Vec3) -> f32 {
        let mut d = p.y - grid_h(&self.tgrid, p.x, p.z);
        let mut dcut = f32::MAX; // CSG: solid = max(union, -nearest cutter)
        let mut db = f32::MAX; // blend group accumulates through smin
        let cx = (((p.x + TG_HALF) / self.scell).floor() as i32).clamp(0, self.sdim - 1);
        let cz = (((p.z + TG_HALF) / self.scell).floor() as i32).clamp(0, self.sdim - 1);
        for &si in &self.sgrid[(cz * self.sdim + cx) as usize] {
            let s = &self.shapes[si as usize];
            // cheap broad-phase: skip shapes far away
            let rel = p - s.pos;
            let l2 = rel.length_squared();
            let reach = match s.sdf {
                Sdf::None => continue,
                Sdf::Sphere { r } => r + 4.0,
                Sdf::Torus { big, tube } => big + tube + 4.0,
                Sdf::Capsule { r, half } => r + half + 4.0,
                Sdf::RBox { half, .. } => half.length() + 4.0,
                Sdf::Shell { r, half, thick } => ((r + thick) * (r + thick) + half * half).sqrt() + 4.0,
            };
            if l2 > reach * reach {
                continue;
            }
            let q = s.inv_rot * rel;
            let ds = match s.sdf {
                Sdf::None => f32::MAX,
                Sdf::Sphere { r } => q.length() - r,
                Sdf::Torus { big, tube } => {
                    let ring = Vec2::new(Vec2::new(q.x, q.z).length() - big, q.y);
                    ring.length() - tube
                }
                Sdf::Capsule { r, half } => {
                    let y = q.y.clamp(-half, half);
                    Vec3::new(q.x, q.y - y, q.z).length() - r
                }
                Sdf::Shell { r, half, thick } => {
                    let ring = (q.x * q.x + q.z * q.z).sqrt() - r;
                    (ring.abs() - thick).max(q.y.abs() - half)
                }
                Sdf::RBox { half, round } => {
                    let a = q.abs() - half + Vec3::splat(round);
                    a.max(Vec3::ZERO).length() + a.max_element().min(0.0) - round
                }
            };
            if s.cut {
                dcut = dcut.min(ds);
            } else if s.blend {
                db = smin(db, ds, 2.2);
            } else {
                d = d.min(ds);
            }
        }
        if db < 1.0e8 {
            d = d.min(db);
        }
        if dcut < 1.0e8 {
            d = d.max(-dcut);
        }
        d
    }

    /// A ball strike at `p` with impact `power`: damage/open any tile there.
    /// DROPSHOT pit: rate-limited score check (true = count it).
    pub fn pit_score_ok(&mut self) -> bool {
        if self.pit_cd <= 0.0 {
            self.pit_cd = 3.0;
            true
        } else {
            false
        }
    }

    pub fn hit_tile(&mut self, p: Vec3, power: f32) {
        for ti in 0..self.tiles.len() {
            let (reach, down, shape) = {
                let t = &self.tiles[ti];
                if t.down_t > 0.0 {
                    continue;
                }
                (t.scale.max_element() * 1.5 + 0.4, t.down_t, t.shape)
            };
            let _ = down;
            if self.shapes[shape].pos.distance(p) > reach {
                continue;
            }
            let t = &mut self.tiles[ti];
            let opened = match t.kind {
                TileKind::Door => {
                    t.down_t = 6.0;
                    true
                }
                TileKind::Drop => {
                    let dmg = if power > 12.0 { 2 } else { 1 };
                    t.hp = t.hp.saturating_sub(dmg);
                    if t.hp == 0 {
                        t.hp = t.max_hp;
                        t.down_t = 12.0;
                        true
                    } else {
                        false
                    }
                }
            };
            let sh = &mut self.shapes[t.shape];
            sh.flash = 1.0;
            if opened {
                sh.sdf = Sdf::None; // the hole is REAL
                let sunk = t.base - Vec3::Y * (t.scale.y * 2.0 + 2.6);
                sh.pos = sunk;
                sh.mat = glam::Mat4::from_scale_rotation_translation(t.scale, t.rot, sunk);
            }
            return;
        }
    }

    /// Floodwater current at p: a strong downstream drag while a flooded
    /// route contains the point (tubes take ~1 s to fill first).
    pub fn current(&self, p: Vec3) -> Option<Vec3> {
        for &(ri, age, dur) in &self.floods {
            if age < 1.1 || age > dur {
                continue;
            }
            let (pts, r) = &self.routes[ri];
            for wi in 0..pts.len().saturating_sub(1) {
                let (a, b) = (pts[wi], pts[wi + 1]);
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1.0e-5)).clamp(0.0, 1.0);
                let axis = a + ab * t;
                if axis.distance(p) < r * 1.08 {
                    let dirn = ab.normalize_or(Vec3::Y);
                    let center = (axis - p) * 3.0;
                    return Some(dirn * 26.0 + center.clamp_length_max(8.0));
                }
            }
        }
        None
    }

    /// Which route segment (if any) contains p? -> (route idx, seg idx)
    pub fn route_locate(&self, p: Vec3) -> Option<(usize, usize)> {
        for (ri, (pts, r)) in self.routes.iter().enumerate() {
            for wi in 0..pts.len().saturating_sub(1) {
                let (a, b) = (pts[wi], pts[wi + 1]);
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1.0e-5)).clamp(0.0, 1.0);
                if (a + ab * t).distance(p) < r * 1.15 {
                    return Some((ri, wi));
                }
            }
        }
        None
    }

    pub fn normal(&self, p: Vec3) -> Vec3 {
        let e = 0.07;
        let k0 = Vec3::new(1.0, -1.0, -1.0);
        let k1 = Vec3::new(-1.0, -1.0, 1.0);
        let k2 = Vec3::new(-1.0, 1.0, -1.0);
        let k3 = Vec3::new(1.0, 1.0, 1.0);
        (k0 * self.sdf(p + k0 * e)
            + k1 * self.sdf(p + k1 * e)
            + k2 * self.sdf(p + k2 * e)
            + k3 * self.sdf(p + k3 * e))
        .normalize_or(Vec3::Y)
    }

    /// Local gravity direction at a point. Empty node list = plain -Y.
    pub fn gravity_dir(&self, p: Vec3) -> Vec3 {
        if self.grav.is_empty() {
            return Vec3::NEG_Y;
        }
        let (mut b0, mut b1): (Option<(f32, usize)>, Option<(f32, usize)>) = (None, None);
        for (i, n) in self.grav.iter().enumerate() {
            let d = n.pos.distance(p);
            if b0.map_or(true, |(bd, _)| d < bd) {
                b1 = b0;
                b0 = Some((d, i));
            } else if b1.map_or(true, |(bd, _)| d < bd) {
                b1 = Some((d, i));
            }
        }
        let dir_of = |i: usize| {
            let n = &self.grav[i];
            if n.tube {
                (p - n.pos).normalize_or(Vec3::NEG_Y)
            } else {
                n.down
            }
        };
        let (d0, i0) = b0.unwrap();
        let local = match b1 {
            Some((_, i1)) => (dir_of(i0) * 0.9 + dir_of(i1) * 0.1).normalize_or(Vec3::NEG_Y),
            None => dir_of(i0),
        };
        let t = d0 / self.grav[i0].r.max(0.01);
        let w = ((1.5 - t) / 0.75).clamp(0.0, 1.0); // full inside 0.75r, gone by 1.5r
        (Vec3::NEG_Y * (1.0 - w) + local * w).normalize_or(Vec3::NEG_Y)
    }

    pub fn ground_h(&self, x: f32, z: f32) -> f32 {
        grid_h(&self.tgrid, x, z)
    }
}
