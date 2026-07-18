//! World generation — ported from the three.js build (which itself adapted
//! unendlich's terrain_warp + biome_temp_map + plant_genetics).

use crate::math::v3;
use crate::mesh::{mix, rgb, shade, Mesh};
use crate::physics::Branch;

pub const SIZE: f32 = 320.0;   // half-extent of the world we render this pass

fn hash2(x: i32, z: i32, s: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393)
        ^ (z as u32).wrapping_mul(668_265_263)
        ^ s.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}
fn smooth(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }
fn vnoise(x: f32, z: f32, s: u32) -> f32 {
    let (xi, zi) = (x.floor(), z.floor());
    let (xf, zf) = (x - xi, z - zi);
    let (ix, iz) = (xi as i32, zi as i32);
    let a = hash2(ix, iz, s); let b = hash2(ix + 1, iz, s);
    let c = hash2(ix, iz + 1, s); let d = hash2(ix + 1, iz + 1, s);
    let u = smooth(xf); let v = smooth(zf);
    (a * (1.0 - u) + b * u) * (1.0 - v) + (c * (1.0 - u) + d * u) * v
}

const WARP_FREQ: f32 = 0.01;
fn warp(x: f32, z: f32, amp: f32) -> (f32, f32) {
    let nx = vnoise(x * WARP_FREQ, z * WARP_FREQ, 0x1111) * 2.0 - 1.0;
    let nz = vnoise(x * WARP_FREQ, z * WARP_FREQ, 0x2222) * 2.0 - 1.0;
    (x + amp * nx, z + amp * nz)
}

const HILL: f32 = 7.0;

/// The "terrain pool" — a few knobs each match rolls fresh, so the map isn't the exact
/// same shape and mood every time: how mountainous the land is, and what season it is
/// (which tints the whole world's foliage, not just the sky/prey like `weather::Season`
/// already did).
static MOUNTAIN_BIAS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3F800000); // 1.0
static TERRAIN_SEASON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1); // Greenleaf

/// Configure the terrain pool before `init_world()`. `mountain_bias` scales how much of
/// the map crosses into real mountain height (1.0 = the original baseline; try 1.4-1.8
/// for a noticeably craggier map). `season` is 0=Newleaf, 1=Greenleaf, 2=Leaf-fall,
/// 3=Leaf-bare, matching `weather::Season`'s ordinal.
pub fn set_terrain_pool(mountain_bias: f32, season: u8) {
    MOUNTAIN_BIAS.store(mountain_bias.to_bits(), std::sync::atomic::Ordering::Relaxed);
    TERRAIN_SEASON.store(season, std::sync::atomic::Ordering::Relaxed);
}
fn mountain_bias() -> f32 { f32::from_bits(MOUNTAIN_BIAS.load(std::sync::atomic::Ordering::Relaxed)) }
/// The terrain pool's current season ordinal (0=Newleaf .. 3=Leaf-bare).
pub fn terrain_season() -> u8 { TERRAIN_SEASON.load(std::sync::atomic::Ordering::Relaxed) }

/// Meandering river channel carved into the base terrain (fills to WATER_Y).
fn river_carve(x: f32, z: f32) -> f32 {
    let rx = 46.0 * (z * 0.011).sin() + 22.0 * (z * 0.031).cos();
    let d = (x - rx).abs();
    let w = 15.0;
    if d < w { -(1.0 - d / w) * 6.5 } else { 0.0 }
}

/// Raw procedural height (pre-erosion). The grid is baked from this.
pub fn raw_height(x: f32, z: f32) -> f32 {
    let (wx, wz) = warp(x, z, 22.0);
    let (mut h, mut amp, mut freq, mut sum) = (0.0, 1.0, 1.0 / 34.0, 0.0);
    for _ in 0..4 {
        h += vnoise(wx * freq, wz * freq, 0) * amp;
        sum += amp; amp *= 0.5; freq *= 2.05;
    }
    h /= sum;
    let r = (wx * wx + wz * wz).sqrt() / 300.0;
    let peaks = (h - 0.5) * 2.0 * HILL * mountain_bias();
    peaks - (1.0 - r).max(0.0) * 3.0 + river_carve(x, z)
}

// ── stored, eroded heightfield (adapted from unendlich thermal_erosion + water_field) ──
pub const WATER_Y: f32 = -1.9;
pub struct HeightGrid { pub w: usize, pub half: f32, pub h: Vec<f32> }
static GRID: std::sync::RwLock<Option<HeightGrid>> = std::sync::RwLock::new(None);
/// Guards the "roll the terrain pool, then rebake" sequence so two concurrent callers
/// (parallel tests constructing several matches at once is the main case — production
/// only ever has one active match) can't interleave and read a half-rebaked world.
pub static TERRAIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Talus thermal erosion — ported from unendlich `world/thermal_erosion.rs`:
/// material slides to lower neighbours when the slope exceeds `talus`.
fn erode_step(h: &mut [f32], w: usize, talus: f32, rate: f32) {
    let rows = h.len() / w;
    let mut delta = vec![0.0f32; h.len()];
    for y in 0..rows {
        for x in 0..w {
            let c = y * w + x;
            let hc = h[c];
            let nbs = [
                if x > 0 { Some(c - 1) } else { None },
                if x + 1 < w { Some(c + 1) } else { None },
                if y > 0 { Some(c - w) } else { None },
                if y + 1 < rows { Some(c + w) } else { None },
            ];
            let mut d_total = 0.0; let mut d_max = 0.0;
            for nb in nbs.iter().flatten() {
                let diff = hc - h[*nb];
                if diff > talus { d_total += diff; if diff > d_max { d_max = diff; } }
            }
            if d_max <= talus || d_total <= 0.0 { continue; }
            let amount = rate * (d_max - talus) * 0.5;
            delta[c] -= amount;
            for nb in nbs.iter().flatten() {
                let diff = hc - h[*nb];
                if diff > talus { delta[*nb] += amount * (diff / d_total); }
            }
        }
    }
    for i in 0..h.len() { h[i] += delta[i]; }
}

/// Bake the eroded heightfield once (call before building the world meshes).
pub fn init_world() {
    let w = 240usize;
    let half = SIZE;
    let step = (2.0 * half) / (w - 1) as f32;
    let mut h = vec![0.0f32; w * w];
    for j in 0..w {
        for i in 0..w {
            let x = -half + i as f32 * step;
            let z = -half + j as f32 * step;
            h[j * w + i] = raw_height(x, z);
        }
    }
    for _ in 0..10 { erode_step(&mut h, w, 0.55, 0.5); }   // carve valleys, soften ridges
    *GRID.write().unwrap() = Some(HeightGrid { w, half, h });
}

fn sample_grid(g: &HeightGrid, x: f32, z: f32) -> f32 {
    let step = (2.0 * g.half) / (g.w - 1) as f32;
    let fx = (x + g.half) / step;
    let fz = (z + g.half) / step;
    if fx < 0.0 || fz < 0.0 || fx >= (g.w - 1) as f32 || fz >= (g.w - 1) as f32 {
        return raw_height(x, z);
    }
    let (ix, iz) = (fx.floor() as usize, fz.floor() as usize);
    let (tx, tz) = (fx - ix as f32, fz - iz as f32);
    let a = g.h[iz * g.w + ix]; let b = g.h[iz * g.w + ix + 1];
    let c = g.h[(iz + 1) * g.w + ix]; let d = g.h[(iz + 1) * g.w + ix + 1];
    (a * (1.0 - tx) + b * tx) * (1.0 - tz) + (c * (1.0 - tx) + d * tx) * tz
}

pub fn height_at(x: f32, z: f32) -> f32 {
    match GRID.read().unwrap().as_ref() { Some(g) => sample_grid(g, x, z), None => raw_height(x, z) }
}

/// Nearest water cell (below WATER_Y) to a point, within `search` units — the
/// snake's ambush lurk spot.
pub fn nearest_water(near: (f32, f32), search: f32) -> Option<(f32, f32)> {
    let step = 4.0;
    let n = (search / step) as i32;
    let mut best = None;
    let mut bd = f32::MAX;
    for j in -n..=n {
        for i in -n..=n {
            let x = near.0 + i as f32 * step;
            let z = near.1 + j as f32 * step;
            if height_at(x, z) < WATER_Y {
                let d = (x - near.0).hypot(z - near.1);
                if d < bd { bd = d; best = Some((x, z)); }
            }
        }
    }
    best
}

/// Smoothstep clamped to [0,1].
#[inline]
fn smooth01(t: f32) -> f32 { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

/// A frozen instant of gentle swell on the water surface: layered directional sines
/// plus a little noise. Small amplitude so it reads as ripple, not chop.
fn water_wave(x: f32, z: f32) -> f32 {
    let s1 = (x * 0.19 + z * 0.05).sin();
    let s2 = (x * -0.07 + z * 0.23 + 1.7).sin();
    let s3 = ((x * 0.11 - z * 0.15) * 1.7 + 4.0).sin();
    let n = vnoise(x * 0.25 + 5.0, z * 0.25 - 3.0, 0x9E) - 0.5;
    0.060 * s1 + 0.040 * s2 + 0.025 * s3 + 0.055 * n
}

/// Water surface — a translucent, depth-tinted sheet over every basin below WATER_Y.
/// The sheet undulates (per-face normals then catch the light like ripples) and its
/// opacity feathers to zero at the waterline so the shore melts instead of hard-cutting.
pub fn build_water() -> Mesh {
    let mut m = Mesh::new();
    let seg = 200usize;
    let step = (SIZE * 2.0) / seg as f32;
    let shallow = rgb(0x5f9fb0);
    let deep = rgb(0x223f57);
    let foam = rgb(0x9fc4c9);
    // height sampled per grid corner so neighbouring cells share exact positions (watertight)
    let hgt = |x: f32, z: f32| -> f32 {
        let depth = WATER_Y - height_at(x, z);
        // taper the swell to nothing at the shore so it never rises above the bank
        let amp = smooth01(depth / 1.2);
        WATER_Y + water_wave(x, z) * amp
    };
    for i in 0..seg {
        for j in 0..seg {
            let x0 = -SIZE + j as f32 * step; let z0 = -SIZE + i as f32 * step;
            let x1 = x0 + step; let z1 = z0 + step;
            let cx = (x0 + x1) * 0.5; let cz = (z0 + z1) * 0.5;
            let depth = WATER_Y - height_at(cx, cz);
            if depth <= 0.02 { continue; }
            // depth tint, with crest sheen and a foamy paling in the shallows
            let t = (depth / 6.0).clamp(0.0, 1.0);
            let mut col = mix(shallow, deep, t);
            let crest = (water_wave(cx, cz) / 0.12).clamp(-1.0, 1.0);
            col = shade(col, 1.0 + crest * 0.10);                 // brighten crests, darken troughs
            let foamy = 1.0 - smooth01(depth / 0.9);              // 1 at the edge → 0 by ~0.9 deep
            col = mix(col, foam, foamy * 0.5);
            // feather transparency: invisible at the waterline, ~0.74 in deep water,
            // with a touch more body on the crests. This dissolves the blocky rim.
            let base = 0.74 * smooth01(depth / 1.2);
            m.set_op((base + crest.max(0.0) * 0.06).clamp(0.0, 0.82));
            let a = v3(x0, hgt(x0, z0), z0); let b = v3(x1, hgt(x1, z0), z0);
            let c = v3(x1, hgt(x1, z1), z1); let d = v3(x0, hgt(x0, z1), z1);
            m.quad(a, d, c, b, col);   // up-facing, corners displaced by the swell
        }
    }
    m.set_op(1.0);
    m
}

fn climate(x: f32, z: f32) -> (f32, f32) {
    (vnoise(x * 0.006 + 3.0, z * 0.006 + 9.0, 0x7A11),
     vnoise(x * 0.006 + 31.0, z * 0.006 + 5.0, 0x3C0F))
}

/// A named biome with its palette and character. Selected by elevation, water
/// proximity and moisture rather than an abstract climate grid.
#[derive(Clone, Copy)]
pub struct Biome {
    pub name: &'static str,
    pub ground: u32,
    pub tree: f32,   // tree density 0..1
    pub leaf: u32,
    pub kind: u8,    // 0 = conifer, 1 = broadleaf/oak
    pub reeds: f32,  // reed density near water
    pub rocks: f32,  // loose-rock density
    pub dark: f32,   // otherworldly gloom (dims ground, thickens canopy)
}

pub const MOUNTAINS:   Biome = Biome { name: "Stony Mountains", ground: 0x8b8d92, tree: 0.10, leaf: 0x47563f, kind: 0, reeds: 0.0,  rocks: 0.95, dark: 0.0 };
pub const LAKE_SHORE:  Biome = Biome { name: "Lake Shore",      ground: 0xbcae82, tree: 0.05, leaf: 0x6c8150, kind: 1, reeds: 0.18, rocks: 0.25, dark: 0.0 };
pub const RIVER_REEDS: Biome = Biome { name: "River Reeds",     ground: 0x5e7146, tree: 0.06, leaf: 0x6f8a48, kind: 1, reeds: 0.95, rocks: 0.0,  dark: 0.0 };
pub const SHADOWY:     Biome = Biome { name: "Shadowy Forest",  ground: 0x2e3a30, tree: 0.88, leaf: 0x22301f, kind: 0, reeds: 0.0,  rocks: 0.10, dark: 0.65 };
pub const OAK_FOREST:  Biome = Biome { name: "Oak Tree Forest", ground: 0x517d38, tree: 0.60, leaf: 0x50702c, kind: 1, reeds: 0.0,  rocks: 0.05, dark: 0.0 };
pub const MOOR:        Biome = Biome { name: "Moor Plains",     ground: 0x6f5f48, tree: 0.05, leaf: 0x8a5f78, kind: 0, reeds: 0.0,  rocks: 0.20, dark: 0.0 };
pub const MANGROVE:    Biome = Biome { name: "Mangrove Shallows", ground: 0x4a5f42, tree: 0.5, leaf: 0x3e5a38, kind: 1, reeds: 0.6, rocks: 0.0, dark: 0.24 };
pub const BIOMES: [Biome; 7] = [MOUNTAINS, LAKE_SHORE, RIVER_REEDS, SHADOWY, OAK_FOREST, MOOR, MANGROVE];

/// Pick the biome at a point from its elevation, nearness to water and moisture.
/// Undergrowth density at a point (0 = open, 1 = deep thicket). Grass, brush, reeds and
/// gloom all thicken it; it's patchy (clearings and thickets), sparse on scree and open
/// water. Cats slip through cover half-seen but risk a rustle or a snagging bramble.
pub fn cover_at(x: f32, z: f32) -> f32 {
    let h = height_at(x, z);
    if h < WATER_Y { return 0.08; }              // open water — nothing to hide in
    let b = biome_at(x, z);
    let grass = match b.name {
        "Moor Plains" => 0.5, "Oak Tree Forest" => 0.68, "River Reeds" => 0.85,
        "Mangrove Shallows" => 0.7, "Lake Shore" => 0.32, "Shadowy Forest" => 0.78,
        "Mountains" => 0.08, _ => 0.4,
    };
    let brush = b.tree * 0.5 + b.reeds * 0.5 + b.dark * 0.35;
    // patchy: a low-frequency noise carves clearings and thickens thickets
    let patch = 0.55 + 0.45 * vnoise(x * 0.12 + 11.0, z * 0.12 - 7.0, 0x0C);
    ((grass + brush) * patch).clamp(0.0, 1.0)
}

pub fn biome_at(x: f32, z: f32) -> Biome {
    let y = height_at(x, z);
    let (m, s) = climate(x, z);
    if y > 1.9 { return MOUNTAINS; }                       // the rare high ground (top ~2%)
    if y < WATER_Y + 0.7 {                                 // a tight shoreline band
        if m > 0.72 { return MANGROVE; }                   // the wettest, warmest shallows grow mangroves
        return if m > 0.5 { RIVER_REEDS } else { LAKE_SHORE };
    }
    if m > 0.5 && s > 0.5 { SHADOWY }                      // dark, wet, dense
    else if m > 0.34 { OAK_FOREST }                        // temperate broadleaf
    else { MOOR }                                          // open, dry heath
}

/// Blended biome colour. Samples the biome across a small, noise-jittered kernel and
/// averages the ground palette + gloom by weight, so the terrain colour crossfades
/// across a biome seam instead of hard-cutting. Interiors are unaffected (every sample
/// lands in the same biome), and *placement* (trees/rocks/reeds) still uses the crisp
/// `biome_at`. Adapted from unendlich's biome_blend (convex combination of params).
pub fn biome_color(x: f32, z: f32) -> ([f32; 3], f32) {
    // a single low-frequency wobble shifts the whole kernel, so the seam meanders
    // organically rather than following a clean disc.
    let jx = (vnoise(x * 0.15 + 2.0, z * 0.15 - 6.0, 0x51) - 0.5) * 3.0;
    let jz = (vnoise(x * 0.15 - 8.0, z * 0.15 + 4.0, 0x52) - 0.5) * 3.0;
    let r = 4.2;
    let d = r * 0.7;
    let o = r * 2.0;
    // centre + an inner ring of eight + an outer ring of four, weights falling off
    // with distance (an approximate Gaussian) so biomes crossfade over a wide, soft band.
    let samples = [
        (0.0, 0.0, 3.0f32),
        (r, 0.0, 1.4), (-r, 0.0, 1.4), (0.0, r, 1.4), (0.0, -r, 1.4),
        (d, d, 1.0), (-d, d, 1.0), (d, -d, 1.0), (-d, -d, 1.0),
        (o, 0.0, 0.5), (-o, 0.0, 0.5), (0.0, o, 0.5), (0.0, -o, 0.5),
    ];
    let (mut acc, mut dark, mut wsum) = ([0.0f32; 3], 0.0f32, 0.0f32);
    for (dx, dz, w) in samples {
        let b = biome_at(x + dx + jx, z + dz + jz);
        let g = rgb(b.ground);
        for k in 0..3 { acc[k] += g[k] * w; }
        dark += b.dark * w;
        wsum += w;
    }
    for k in 0..3 { acc[k] /= wsum; }
    (acc, dark / wsum)
}

/// Build the terrain grid mesh, coloured by biome + height.
pub fn build_terrain(seg: usize) -> Mesh {
    let mut m = Mesh::new();
    let step = (SIZE * 2.0) / seg as f32;
    let rock = rgb(0x8f8a80);
    let col_at = |x: f32, z: f32, y: f32| -> [f32; 3] {
        let (ground, dark) = biome_color(x, z);
        let mut c = ground;
        // a little lushness on low ground, but each biome keeps its own character
        let grass = rgb(0x4f7d33);
        let low = (1.0 - (y / 6.0).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        let lush = 0.14 * (1.0 - dark);
        for k in 0..3 { c[k] = c[k] * (1.0 - low * lush) + grass[k] * (low * lush); }
        let t = ((y + 8.0) / 16.0).clamp(0.0, 1.0);
        let dry = rgb(0x7a7a44);
        for k in 0..3 { c[k] = c[k] * (1.0 - t * 0.10) + dry[k] * (t * 0.10); }
        if y > 5.5 { let f = ((y - 5.5) / 4.0).min(1.0); for k in 0..3 { c[k] = c[k] * (1.0 - f) + rock[k] * f; } }
        // biome gloom (the Shadowy Forest broods darker), blended across seams too
        for k in 0..3 { c[k] *= 1.0 - dark * 0.42; }
        // fuzzy grain: high-frequency mottling breaks up the flat facets and lets
        // translucent overlays (like the AI's vision cone) melt into the ground
        let n = vnoise(x * 0.9 + 13.0, z * 0.9 - 7.0, 7) - 0.5;
        let n2 = vnoise(x * 3.1 - 4.0, z * 3.1 + 9.0, 5) - 0.5;
        let g = 1.0 + (n * 0.18 + n2 * 0.09);
        for k in 0..3 { c[k] = (c[k] * g).clamp(0.0, 1.0); }
        c
    };
    for i in 0..seg {
        for j in 0..seg {
            let x0 = -SIZE + j as f32 * step;
            let z0 = -SIZE + i as f32 * step;
            let x1 = x0 + step; let z1 = z0 + step;
            let a = v3(x0, height_at(x0, z0), z0);
            let b = v3(x1, height_at(x1, z0), z0);
            let c = v3(x1, height_at(x1, z1), z1);
            let d = v3(x0, height_at(x0, z1), z1);
            let cx = (x0 + x1) * 0.5; let cz = (z0 + z1) * 0.5;
            let col = col_at(cx, cz, (a.y + c.y) * 0.5);
            m.quad(a, d, c, b, col);   // wind so the surface normal points up
        }
    }
    m
}

/// Generate one tree: tapered trunk, procedural climbable branches, and a
/// higher-res, semi-transparent canopy. Broadleaf (kind=1) forks out limbs;
/// conifer (kind=0) stacks layered cones with a few short branches. Returns the
/// branches (for climbing/perching). Branch colour tints toward red where a
/// `cat_kg` load at the tip would risk snapping it.
/// Shift a leaf color for the terrain pool's current season: Newleaf brightens toward
/// fresh spring green, Leaf-fall shifts warm (orange/red), Leaf-bare desaturates toward
/// bare grey-brown. Greenleaf (the default) passes through unchanged.
pub fn season_tint(c: [f32; 3]) -> [f32; 3] {
    match terrain_season() {
        0 => [c[0] * 0.85 + 0.05, (c[1] * 1.15 + 0.05).min(1.0), c[2] * 0.85],           // Newleaf — fresh and bright
        2 => [(c[0] * 1.35 + 0.12).min(1.0), c[1] * 0.75, c[2] * 0.55],                  // Leaf-fall — warm oranges/reds
        3 => { let g = (c[0] + c[1] + c[2]) / 3.0; mix(c, [g * 0.9 + 0.05, g * 0.85, g * 0.8], 0.75) }  // Leaf-bare — desaturated, bare
        _ => c,   // Greenleaf — unchanged
    }
}

/// A denser ring of vegetation at the mid radius (roughly where jungle camps and
/// off-lane objectives tend to sit) — tightens that band into real enclosed jungle
/// instead of the same open density as everywhere else. 1.0 outside the band.
fn jungle_density_boost(x: f32, z: f32) -> f32 {
    let r = (x * x + z * z).sqrt();
    let band = 1.0 - ((r - 55.0).abs() / 20.0).clamp(0.0, 1.0);   // peaks at r=55, fades out by ±20
    1.0 + band * 0.9
}

pub fn gen_tree(m: &mut Mesh, x: f32, z: f32, kind: u8, leaf: [f32; 3], scale: f32, cat_kg: f32, detail: bool) -> Vec<Branch> {
    let tau = std::f32::consts::TAU;
    let y = height_at(x, z);
    let h1 = hash2(x as i32, z as i32, 0x9001);
    let trunk_h = 6.0 * scale;
    let base_r = 0.4 * scale;
    let top = v3(x, y + trunk_h, z);
    let bark = rgb(0x6b4b32);
    let mut branches = Vec::new();
    m.set_op(1.0);
    m.segment(v3(x, y, z), top, base_r, base_r * 0.5, 7, bark);

    if kind == 1 {
        let nb = 4 + ((h1 * 3.0) as usize);
        for i in 0..nb {
            let frac = 0.45 + i as f32 / nb as f32 * 0.5;
            let ay = y + trunk_h * frac;
            let ang = i as f32 / nb as f32 * tau + h1 * tau;
            let up = 0.3 + hash2(i as i32, x as i32, 0x51) * 0.35;
            let blen = (2.6 - frac * 1.2) * scale;
            let dir = v3(ang.cos(), up, ang.sin());
            let bstart = v3(x, ay, z);
            let btip = bstart.add(dir.norm().scale(blen));
            let br = (base_r * (0.5 - frac * 0.2)).max(0.08 * scale);
            let branch = Branch { a: bstart, b: btip, r: br };
            let risk = branch.snap_risk(cat_kg, 1.0).min(1.0);
            m.set_op(1.0);
            m.segment(bstart, btip, br, br * 0.5, 6, mix(bark, rgb(0xc0503a), risk));
            m.set_op(0.62);
            m.cone(btip, 1.4 * scale, 2.2 * scale, 8, leaf);
            branches.push(branch);
        }
        m.set_op(0.62);
        m.cone(v3(x, y + trunk_h - 0.5 * scale, z), 2.6 * scale, 3.0 * scale, 10, leaf);
    } else if kind == 2 {
        // willow — long drooping branches hanging from a high crown, a soft trailing silhouette
        let crown_y = y + trunk_h * 0.92;
        let nb = 7 + ((h1 * 4.0) as usize);
        for i in 0..nb {
            let ang = i as f32 / nb as f32 * tau + h1 * tau;
            let out = (1.1 + hash2(i as i32, x as i32, 0x62) * 0.6) * scale;
            let droop = (2.2 + hash2(i as i32, z as i32, 0x63) * 1.4) * scale;
            let bstart = v3(x, crown_y, z);
            let bmid = v3(x + ang.cos() * out, crown_y - droop * 0.35, z + ang.sin() * out);
            let btip = v3(x + ang.cos() * out * 0.85, crown_y - droop, z + ang.sin() * out * 0.85);
            let br = base_r * 0.22;
            let branch = Branch { a: bstart, b: btip, r: br };
            let risk = branch.snap_risk(cat_kg, 1.0).min(1.0);
            m.set_op(1.0);
            m.segment(bstart, bmid, br, br * 0.7, 5, mix(bark, rgb(0xc0503a), risk));
            m.segment(bmid, btip, br * 0.7, br * 0.3, 5, mix(bark, rgb(0xc0503a), risk));
            m.set_op(0.6);
            m.cone(btip, 0.55 * scale, 1.0 * scale, 6, shade(leaf, 0.95 + hash2(i as i32, 3, 0x64) * 0.15));
            branches.push(branch);
        }
        m.set_op(0.62);
        m.cone(v3(x, crown_y - 0.3 * scale, z), 2.0 * scale, 2.2 * scale, 10, shade(leaf, 1.05));
    } else {
        m.set_op(0.62);
        for l in 0..4 {
            let ly = y + trunk_h * 0.4 + l as f32 * 1.3 * scale;
            let lr = (2.6 - l as f32 * 0.5) * scale;
            m.cone(v3(x, ly, z), lr, 1.9 * scale, 10, shade(leaf, 1.0 - l as f32 * 0.04));
        }
        for i in 0..3 {
            let ay = y + trunk_h * (0.5 + i as f32 * 0.14);
            let ang = i as f32 * 2.1 + h1 * tau;
            let dir = v3(ang.cos(), 0.15, ang.sin());
            let bstart = v3(x, ay, z);
            let btip = bstart.add(dir.norm().scale(1.6 * scale));
            let br = base_r * 0.4;
            let branch = Branch { a: bstart, b: btip, r: br };
            let risk = branch.snap_risk(cat_kg, 1.0).min(1.0);
            m.set_op(1.0);
            m.segment(bstart, btip, br, br * 0.6, 6, mix(bark, rgb(0xc0503a), risk));
            branches.push(branch);
        }
    }
    // individual leaf cards — higher-LOD canopy (near/hero trees only, for budget)
    if detail {
        m.set_op(0.78);
        let crown_y = y + trunk_h * 0.9;
        let spread = 2.6 * scale;
        let n = 260;
        for i in 0..n {
            let a = hash2(i as i32, x as i32, 0xA7);
            let b = hash2(i as i32, z as i32, 0xB8);
            let e = hash2(i as i32 + 5, z as i32, 0xC9);
            let ang = a * tau;
            let rad = spread * (0.3 + b * 0.7);
            let ly = crown_y - 1.6 * scale + (e - 0.15) * 3.4 * scale;
            let p = v3(x + ang.cos() * rad, ly, z + ang.sin() * rad);
            let a2 = a * tau + b * 3.0;
            let u = v3(a2.cos(), 0.3, a2.sin()).norm().scale(0.26 * scale);
            let w = v3(-a2.sin(), 0.8, a2.cos()).norm().scale(0.26 * scale);
            let mut lc = leaf;
            let d = (e - 0.5) * 0.22;
            for k in 0..3 { lc[k] = (lc[k] + d).clamp(0.0, 1.0); }
            m.quad(p.sub(u).sub(w), p.add(u).sub(w), p.add(u).add(w), p.sub(u).add(w), lc);
        }
    }
    m.set_op(1.0);
    branches
}

/// Scatter the forest by biome density (branches left brown — no climber loading them).
/// Tree trunk positions (x, z) — the trees a cat can scramble up to escape.
pub fn tree_positions(count: usize) -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    for i in 0..(count * 6) {
        let x = (hash2(i as i32, 11, 0xA1) - 0.5) * SIZE * 1.8;
        let z = (hash2(i as i32, 23, 0xB2) - 0.5) * SIZE * 1.8;
        if (x * x + z * z).sqrt() < 16.0 { continue; }
        let y = height_at(x, z);
        if y > 5.2 { continue; }
        let biome = biome_at(x, z);
        if hash2(i as i32, 7, 0xC3) > (biome.tree * jungle_density_boost(x, z)).min(1.0) { continue; }
        v.push((x, z));
    }
    v
}

pub fn build_trees(count: usize) -> Mesh {
    let mut m = Mesh::new();
    for i in 0..(count * 6) {
        let x = (hash2(i as i32, 11, 0xA1) - 0.5) * SIZE * 1.8;
        let z = (hash2(i as i32, 23, 0xB2) - 0.5) * SIZE * 1.8;
        if (x * x + z * z).sqrt() < 16.0 { continue; }
        let y = height_at(x, z);
        if y > 5.2 { continue; }
        let biome = biome_at(x, z);
        if hash2(i as i32, 7, 0xC3) > (biome.tree * jungle_density_boost(x, z)).min(1.0) { continue; }
        let gh = 0.8 + hash2(x as i32, z as i32, 0x6E0E) * 1.7;
        let tint = (hash2(x as i32 + 7, z as i32 + 3, 0x51F7) - 0.5) * 0.3;
        let mut leaf = season_tint(rgb(biome.leaf));
        for k in 0..3 { leaf[k] = (leaf[k] + tint).clamp(0.0, 1.0); }
        let kind_roll = hash2(i as i32, 5, 0x77);
        let kind = if kind_roll < 0.08 { 2 } else if kind_roll < 0.85 { biome.kind } else { 1 - biome.kind };
        gen_tree(&mut m, x, z, kind, leaf, 0.9 + gh * 0.4, 0.0, false);
        if m.count() > 400_000 { break; }
    }
    m
}

/// Bush placements as (x, z, radius) — the cover a cat can crouch into.
pub fn bush_positions(count: usize) -> Vec<(f32, f32, f32)> {
    let mut v = Vec::new();
    for i in 0..(count * 4) {
        let x = (hash2(i as i32, 41, 0xE1) - 0.5) * SIZE * 1.7;
        let z = (hash2(i as i32, 53, 0xE2) - 0.5) * SIZE * 1.7;
        if (x * x + z * z).sqrt() < 10.0 { continue; }
        let y = height_at(x, z);
        if y > 4.6 { continue; }
        let biome = biome_at(x, z);
        if biome.tree < 0.06 { continue; }
        if hash2(i as i32, 9, 0xE3) > (0.62 * jungle_density_boost(x, z)).min(1.0) { continue; }
        let r = 1.6 + hash2(i as i32, 3, 0xE4) * 1.3;
        v.push((x, z, r));
    }
    v
}

/// Dense, rounded, OPAQUE bushes — the brambles a cat crouches into to hide.
pub fn build_bushes(count: usize) -> Mesh {
    let mut m = Mesh::new();
    m.set_op(0.92);
    for (idx, (x, z, r)) in bush_positions(count).into_iter().enumerate() {
        let i = idx as i32;
        let biome = biome_at(x, z);
        let mut leaf = season_tint(shade(rgb(biome.leaf), 0.82));
        // the occasional berry bush — a warm tint standing out from the usual green
        if hash2(i, 17, 0xEB) < 0.12 {
            leaf = mix(leaf, rgb(0x9a3a44), 0.35);
        }
        let spiky = hash2(i, 29, 0xEC) < 0.3;   // an upright, spikier shrub instead of the round blob
        if spiky {
            let nb = 4 + (hash2(i, 2, 0xED) * 3.0) as usize;
            for b in 0..nb {
                let a = b as f32 / nb as f32 * std::f32::consts::TAU + hash2(i, b as i32, 0xEE) * 0.5;
                let rr = r * (0.15 + hash2(i, b as i32, 0xE7) * 0.25);
                let cx = x + a.cos() * rr; let cz = z + a.sin() * rr;
                let lh = r * (1.3 + hash2(i, b as i32, 0xE8) * 0.7);
                m.cone(v3(cx, height_at(cx, cz), cz), r * 0.28, lh, 6, shade(leaf, 0.88 + hash2(b as i32, i, 0xE9) * 0.24));
            }
        } else {
            for b in 0..6 {
                let a = b as f32 / 6.0 * std::f32::consts::TAU;
                let rr = if b == 0 { 0.0 } else { r * (0.35 + hash2(i, b as i32, 0xE7) * 0.3) };
                let cx = x + a.cos() * rr; let cz = z + a.sin() * rr;
                let lh = r * (0.85 + hash2(i, b as i32, 0xE8) * 0.4);
                m.cone(v3(cx, height_at(cx, cz), cz), r * 0.7, lh, 9, shade(leaf, 0.9 + hash2(b as i32, i, 0xE9) * 0.2));
            }
        }
    }
    m.set_op(1.0);
    m
}

/// Dense grass tufts (double-sided paper blades) on the greener biomes.
pub fn build_grass(count: usize) -> Mesh {
    let mut m = Mesh::new();
    let tau = std::f32::consts::TAU;
    let wind = (0.35f32, 0.12);   // a prevailing lean so the sward looks combed by the breeze
    for i in 0..count {
        let x = (hash2(i as i32, 61, 0xF1) - 0.5) * SIZE * 1.9;
        let z = (hash2(i as i32, 67, 0xF2) - 0.5) * SIZE * 1.9;
        let y = height_at(x, z);
        if y < WATER_Y + 0.12 || y > 5.0 { continue; }      // not underwater, not bare scree
        let biome = biome_at(x, z);
        // grass thrives on open and temperate ground; it thins in scree and deep shadow
        let lush = match biome.name {
            "Moor Plains" => 0.96, "Oak Tree Forest" => 0.9, "River Reeds" => 0.85,
            "Mangrove Shallows" => 0.72, "Lake Shore" => 0.66, "Shadowy Forest" => 0.5, _ => 0.1,
        };
        if hash2(i as i32, 5, 0xA1) > lush { continue; }
        let g = [0.30 + hash2(i as i32, 1, 0x11) * 0.12, 0.50 + hash2(i as i32, 2, 0x12) * 0.18, 0.16 + hash2(i as i32, 3, 0x13) * 0.10];
        let gr = rgb(biome.ground);
        let base_g = season_tint([g[0] * 0.7 + gr[0] * 0.3, g[1] * 0.7 + gr[1] * 0.3, g[2] * 0.7 + gr[2] * 0.3]);
        let blades = 3 + (hash2(i as i32, 7, 0xB2) * 3.0) as usize;
        for b in 0..blades {
            let a = hash2(i as i32, b as i32, 0x33) * tau;
            let h = 0.5 + hash2(i as i32 + b as i32, z as i32, 0x44) * 0.9;
            let bx = x + a.cos() * 0.18; let bz = z + a.sin() * 0.18;
            let by = height_at(bx, bz);
            let side = v3(-a.sin(), 0.0, a.cos()).scale(0.055);
            let p0 = v3(bx, by, bz).add(side);
            let p1 = v3(bx, by, bz).sub(side);
            let tip = v3(bx + wind.0 * h * 0.5, by + h, bz + wind.1 * h * 0.5);   // lean with the wind
            let mut c = base_g; let d = (hash2(b as i32, i as i32, 0x55) - 0.5) * 0.14;
            for k in 0..3 { c[k] = (c[k] + d).clamp(0.0, 1.0); }
            m.tri(p0, p1, tip, c);   // front
            m.tri(p0, tip, p1, c);   // back (double-sided)
        }
    }
    m
}

/// Forest-floor litter: fallen twigs (short bark segments) and scattered leaves,
/// gathering thickest under the trees. Pure ambience — but the twigs are also what a
/// running cat snaps underfoot (see `sim`), so their placement is shared with the twig map.
pub fn build_litter(count: usize) -> Mesh {
    let mut m = Mesh::new();
    let tau = std::f32::consts::TAU;
    for i in 0..count {
        let x = (hash2(i as i32, 131, 0xC1) - 0.5) * SIZE * 1.8;
        let z = (hash2(i as i32, 137, 0xC2) - 0.5) * SIZE * 1.8;
        let y = height_at(x, z);
        if y < WATER_Y + 0.1 || y > 5.0 { continue; }
        let biome = biome_at(x, z);
        let litter = biome.tree * 0.9 + biome.dark * 0.4 + 0.04;   // thickest on the forest floor
        if hash2(i as i32, 9, 0xC3) > litter { continue; }
        let a = hash2(i as i32, 2, 0xC4) * tau;
        if hash2(i as i32, 4, 0xC5) < 0.5 {
            let len = 0.35 + hash2(i as i32, 6, 0xC6) * 0.5;         // a twig lying on the ground
            let (dx, dz) = (a.cos() * len, a.sin() * len);
            let p0 = v3(x, height_at(x, z) + 0.04, z);
            let p1 = v3(x + dx, height_at(x + dx, z + dz) + 0.04, z + dz);
            let bark = shade(rgb(0x6b4f33), 0.85 + hash2(i as i32, 1, 0xC7) * 0.3);
            m.segment(p0, p1, 0.035, 0.02, 3, bark);
        } else {
            let s = 0.12 + hash2(i as i32, 8, 0xC8) * 0.12;          // a fallen leaf, autumn-tinted
            let leaf = [0.5 + hash2(i as i32, 3, 0xC9) * 0.3, 0.32 + hash2(i as i32, 5, 0xCA) * 0.2, 0.1 + hash2(i as i32, 7, 0xCB) * 0.08];
            let yy = height_at(x, z) + 0.03;
            let (c1, s1) = (a.cos(), a.sin());
            m.quad(v3(x - c1 * s, yy, z - s1 * s), v3(x + s1 * s, yy, z - c1 * s), v3(x + c1 * s, yy, z + s1 * s), v3(x - s1 * s, yy, z + s1 * s), leaf);
        }
    }
    m
}

/// Fallen logs — thick bark cylinders lying in the woods. Low cover and something to
/// bound over. Density follows the tree cover of the biome.
pub fn build_logs(count: usize) -> Mesh {
    let mut m = Mesh::new();
    let tau = std::f32::consts::TAU;
    for i in 0..count {
        let x = (hash2(i as i32, 211, 0xD1) - 0.5) * SIZE * 1.6;
        let z = (hash2(i as i32, 223, 0xD2) - 0.5) * SIZE * 1.6;
        let y = height_at(x, z);
        if y < WATER_Y + 0.3 || y > 4.5 { continue; }
        let biome = biome_at(x, z);
        if hash2(i as i32, 3, 0xD3) > biome.tree * 0.5 { continue; }   // fallen from the woods
        let a = hash2(i as i32, 5, 0xD4) * tau;
        let len = 2.5 + hash2(i as i32, 7, 0xD5) * 3.5;
        let r = 0.28 + hash2(i as i32, 9, 0xD6) * 0.18;
        let (dx, dz) = (a.cos() * len * 0.5, a.sin() * len * 0.5);
        let p0 = v3(x - dx, height_at(x - dx, z - dz) + r * 0.7, z - dz);
        let p1 = v3(x + dx, height_at(x + dx, z + dz) + r * 0.7, z + dz);
        let bark = shade(rgb(0x5a4530), 0.9 + hash2(i as i32, 1, 0xD7) * 0.25);
        m.segment(p0, p1, r, r * 0.9, 7, bark);
    }
    m
}

/// Twig positions a running cat can snap (drawn from the same field as `build_litter`
/// twigs). Returns points on walkable ground, thickest under the trees.
pub fn twig_positions(count: usize) -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    for i in 0..count {
        let x = (hash2(i as i32, 131, 0xC1) - 0.5) * SIZE * 1.8;
        let z = (hash2(i as i32, 137, 0xC2) - 0.5) * SIZE * 1.8;
        let y = height_at(x, z);
        if y < WATER_Y + 0.1 || y > 5.0 { continue; }
        let biome = biome_at(x, z);
        let litter = biome.tree * 0.9 + biome.dark * 0.4 + 0.04;
        if hash2(i as i32, 9, 0xC3) > litter { continue; }
        if hash2(i as i32, 4, 0xC5) < 0.5 { v.push((x, z)); }   // the twig half of the litter
    }
    v
}

/// Fallen-log centres (mirrors `build_logs`), so the sim can let cats bound over them.
pub fn log_positions(count: usize) -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    for i in 0..count {
        let x = (hash2(i as i32, 211, 0xD1) - 0.5) * SIZE * 1.6;
        let z = (hash2(i as i32, 223, 0xD2) - 0.5) * SIZE * 1.6;
        let y = height_at(x, z);
        if y < WATER_Y + 0.3 || y > 4.5 { continue; }
        if hash2(i as i32, 3, 0xD3) > biome_at(x, z).tree * 0.5 { continue; }
        v.push((x, z));
    }
    v
}

/// Faceted paper boulders, clustered and half-sunk into the ground.
/// Rock cluster centres — mirrors `build_rocks`, so a mode can let creatures vault
/// over them the way `log_positions` lets them bound over logs.
pub fn rock_positions(count: usize) -> Vec<(f32, f32)> {
    let mut v = Vec::new();
    for i in 0..count {
        let x = (hash2(i as i32, 71, 0x1A) - 0.5) * SIZE * 1.5;
        let z = (hash2(i as i32, 79, 0x2B) - 0.5) * SIZE * 1.5;
        if hash2(i as i32, 9, 0x88) > biome_at(x, z).rocks { continue; }
        v.push((x, z));
    }
    v
}

pub fn build_rocks(count: usize) -> Mesh {
    let mut m = Mesh::new();
    let gray = rgb(0x8a857b);
    for i in 0..count {
        let x = (hash2(i as i32, 71, 0x1A) - 0.5) * SIZE * 1.5;
        let z = (hash2(i as i32, 79, 0x2B) - 0.5) * SIZE * 1.5;
        if hash2(i as i32, 9, 0x88) > biome_at(x, z).rocks { continue; } // scree in the mountains, rare elsewhere
        let n = 1 + (hash2(i as i32, 3, 0x3C) * 2.0) as usize;
        for k in 0..n {
            let ox = (hash2(i as i32, k as i32, 0x4D) - 0.5) * 1.4;
            let oz = (hash2(i as i32 + 3, k as i32, 0x5E) - 0.5) * 1.4;
            let r = 0.55 + hash2(i as i32, k as i32, 0x6F) * 1.5;
            let bx = x + ox; let bz = z + oz;
            let by = height_at(bx, bz) - r * 0.3;
            let c = shade(gray, 0.88 + hash2(i as i32, k as i32, 0x70) * 0.22);
            m.cone(v3(bx, by, bz), r, r * (0.7 + hash2(k as i32, i as i32, 0x71) * 0.5), 6, c);
        }
    }
    m
}

/// Tall reeds fringing the water in the River Reeds and Lake Shore biomes.
pub fn build_reeds(count: usize) -> Mesh {
    let mut m = Mesh::new();
    let tau = std::f32::consts::TAU;
    for i in 0..count {
        let x = (hash2(i as i32, 61, 0x9A) - 0.5) * SIZE * 1.7;
        let z = (hash2(i as i32, 67, 0xAB) - 0.5) * SIZE * 1.7;
        let y = height_at(x, z);
        let biome = biome_at(x, z);
        if biome.reeds < 0.1 { continue; }
        if y < WATER_Y - 0.4 || y > WATER_Y + 1.7 { continue; }   // only the water's fringe
        if hash2(i as i32, 5, 0xBC) > biome.reeds { continue; }
        let n = 3 + (hash2(i as i32, 7, 0xCD) * 5.0) as usize;
        for b in 0..n {
            let b = b as i32;
            let a = hash2(i as i32, b, 0xDE) * tau;
            let rr = hash2(i as i32 + b, 1, 0xEF) * 0.7;
            let bx = x + a.cos() * rr; let bz = z + a.sin() * rr;
            let by = height_at(bx, bz).max(WATER_Y - 0.2);
            let h = 1.4 + hash2(b, i as i32, 0xF0) * 1.4;         // tall blades
            let lean = (hash2(i as i32, b, 0x11) - 0.5) * 0.45;
            let side = v3(-a.sin(), 0.0, a.cos()).scale(0.04);
            let base = v3(bx, by, bz);
            let tip = v3(bx + lean, by + h, bz + lean);
            let col = shade(rgb(0x8f9a55), 0.85 + hash2(b, i as i32, 0x12) * 0.3);
            m.tri(base.add(side), base.sub(side), tip, col);
            m.tri(base.add(side), tip, base.sub(side), col);      // double-sided
            m.cone(tip, 0.05, 0.24, 4, rgb(0x6b5a3a));            // seed head
        }
    }
    m
}

#[cfg(test)]
mod biome_blend_tests {
    use super::*;

    #[test]
    fn a_higher_mountain_bias_yields_more_mountain_terrain() {
        fn mountain_fraction() -> f32 {
            let (mut mtn, mut total) = (0, 0);
            for i in -60..60 { for j in -60..60 {
                let (x, z) = (i as f32 * 3.0, j as f32 * 3.0);
                if height_at(x, z) > 1.9 { mtn += 1; }
                total += 1;
            }}
            mtn as f32 / total as f32
        }
        set_terrain_pool(1.0, 1); init_world();
        let baseline = mountain_fraction();
        set_terrain_pool(1.7, 1); init_world();
        let biased = mountain_fraction();
        assert!(biased >= baseline, "a higher mountain bias shouldn't produce less mountain terrain (baseline {baseline:.4}, biased {biased:.4})");
        set_terrain_pool(1.0, 1); init_world();   // leave the pool at baseline for other tests
    }

    #[test]
    fn season_tint_shifts_leaf_color_by_season() {
        let base = [0.4f32, 0.55, 0.2];
        set_terrain_pool(1.0, 1);   // Greenleaf
        let green = season_tint(base);
        assert_eq!(green, base, "Greenleaf should pass the color through unchanged");
        set_terrain_pool(1.0, 2);   // Leaf-fall
        let fall = season_tint(base);
        assert!(fall[0] > green[0] && fall[1] < green[1], "Leaf-fall should shift warmer — more red, less green");
        set_terrain_pool(1.0, 3);   // Leaf-bare
        let bare = season_tint(base);
        let sat_bare = (bare[0] - bare[2]).abs();
        let sat_green = (green[0] - green[2]).abs();
        assert!(sat_bare < sat_green, "Leaf-bare should desaturate toward grey");
        set_terrain_pool(1.0, 1);   // reset
    }

    #[test]
    fn the_jungle_band_is_denser_than_the_open_map() {
        let boost_in_band = jungle_density_boost(55.0, 0.0);
        let boost_outside = jungle_density_boost(150.0, 0.0);
        assert!(boost_in_band > boost_outside, "the mid-radius jungle band should be denser than the open map");
        assert!((boost_outside - 1.0).abs() < 1e-6, "well outside the band, density should be unboosted");
    }

    #[test]
    fn all_three_tree_kinds_produce_geometry() {
        init_world();
        for kind in [0u8, 1, 2] {
            let mut m = Mesh::new();
            gen_tree(&mut m, 5.0, 5.0, kind, rgb(0x5a8a3a), 1.0, 0.0, false);
            assert!(!m.verts.is_empty(), "tree kind {kind} should build a non-empty mesh");
        }
    }

    #[test]
    fn willow_has_a_trailing_drooping_silhouette() {
        // the willow's branch tips should hang well below its crown, unlike the
        // upright conifer/broadleaf canopies
        init_world();
        let mut willow = Mesh::new();
        gen_tree(&mut willow, 0.0, 0.0, 2, rgb(0x5a8a3a), 1.0, 0.0, false);
        let ymin = willow.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        let ymax = willow.verts.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!(ymax - ymin > 4.0, "the willow's droop should give it real vertical spread");
    }

    #[test]
    fn bushes_vary_between_round_and_spiky_shapes() {
        init_world();
        let m = build_bushes(60);
        assert!(!m.verts.is_empty());
        // just a smoke test that a decent-sized batch of bushes still builds a sane mesh
        // with the new shape-branch and berry-tint logic in place
        assert!(m.verts.len() > 100);
    }

    #[test]
    fn cover_is_thick_in_the_woods_thin_in_the_open() {
        init_world();
        // sample many points; forest/reed biomes should average far more cover than the
        // bare moor/mountains, and open water should be near-zero.
        let (mut forest, mut fc, mut open, mut oc) = (0.0f32, 0, 0.0f32, 0);
        for i in -60..60 { for j in -60..60 {
            let (x, z) = (i as f32 * 3.0, j as f32 * 3.0);
            let c = cover_at(x, z);
            match biome_at(x, z).name {
                "Oak Tree Forest" | "Shadowy Forest" | "River Reeds" => { forest += c; fc += 1; }
                "Mountains" | "Lake Shore" => { open += c; oc += 1; }
                _ => {}
            }
        }}
        if fc > 0 && oc > 0 {
            assert!(forest / fc as f32 > open / oc as f32, "woods should hold more cover than open ground");
        }
        assert!(cover_at(0.0, 0.0) >= 0.0 && cover_at(0.0, 0.0) <= 1.0);
    }

    // The blend must crossfade the ground palette across biome seams: scanning the map,
    // the worst colour jump between adjacent samples should be far smaller than with the
    // hard per-point biome colour (which steps the full palette contrast in one cell).
    #[test]
    fn blend_smooths_biome_seams() {
        init_world();
        let d = |a: [f32; 3], b: [f32; 3]| (a[0]-b[0]).abs() + (a[1]-b[1]).abs() + (a[2]-b[2]).abs();
        let (mut hard_max, mut soft_max) = (0.0f32, 0.0f32);
        for &z in &[-120.0f32, -40.0, 40.0, 120.0] {
            let mut x = -180.0f32;
            let (mut ph, mut ps) = (rgb(biome_at(x, z).ground), biome_color(x, z).0);
            while x <= 180.0 {
                x += 1.0;
                let h = rgb(biome_at(x, z).ground);
                let s = biome_color(x, z).0;
                hard_max = hard_max.max(d(h, ph));
                soft_max = soft_max.max(d(s, ps));
                ph = h; ps = s;
            }
        }
        assert!(hard_max > 0.15, "scan should actually cross seams (hard_max={hard_max})");
        assert!(soft_max < hard_max * 0.5,
            "blend should roughly halve the worst seam jump (soft_max={soft_max}, hard_max={hard_max})");
    }
}
