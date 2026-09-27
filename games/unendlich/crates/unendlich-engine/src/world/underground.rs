//! Underground post-pass: rock strata, ore veins, cave dressing and the surface
//! plants that give a vein away.
//!
//! Both terrain backends (the GPU `chunk_generation.comp` and the CPU
//! `WorldGenerator`) leave everything below the soil as plain `Stone`, sprinkle
//! the same three ores as noise blobs, and put grass on cave floors. This pass
//! runs on the finished block array of each chunk, after either backend, so the
//! two stay byte-identical.
//!
//! Everything here is a pure function of `(world cell, seed, surface height)`.
//! A chunk never reads its neighbours, so a cell decides the same way whichever
//! chunk it is generated in; the only chunk-boundary effect is that a hanging
//! stalactite or vine stops at the chunk edge rather than continuing into the
//! next chunk.
//!
//! Depth zones follow `cave_biomes.docx` (limestone and river caves nearest the
//! surface, then the crystal zone, then abyssal, then geothermal). The docx
//! depths (30 / 80 / 200 / 500 m) are compressed onto this world, whose
//! surface sits near y 64 over bedrock at y 0: the cells a player can reach at
//! sea level are at most ~60 deep, so only mountains reach the abyssal zone and
//! the geothermal zone is keyed to absolute height (the bottom 18 blocks)
//! rather than to depth.
use super::blocks::BlockId;
use super::noise::{hash_2d, hash_3d, noise_2d, noise_3d};
use crate::constants::CHUNK_SIZE;

const CS: i32 = CHUNK_SIZE;

/// Depth below the column's surface, in blocks, at which rock starts being
/// treated as underground. Above it, the backends' soil layers stand.
pub const SOIL_DEPTH: i32 = 4;

/// Cells at or below this absolute height are geothermal regardless of depth.
pub const GEOTHERMAL_Y: i32 = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Above `SOIL_DEPTH`: nothing here is touched.
    Soil,
    /// Limestone and river caves: dripstone, moss, clay, vines near the top.
    Limestone,
    /// Crystal caves: granite and marble, crystal clusters, glow crystal.
    Crystal,
    /// Abyssal: slate and schist, glowshrooms, old bone.
    Abyssal,
    /// Geothermal: basalt and tuff, magma floors, sulfur crusts.
    Geothermal,
}

/// Zone boundaries in blocks of depth: Soil | Limestone | Crystal | Abyssal.
const ZONE_BANDS: [f32; 3] = [SOIL_DEPTH as f32, 26.0, 70.0];

/// The zone of the cell `depth` blocks below the surface at height `y`.
/// `wobble` in [0, 1] waves the band boundaries by a couple of blocks so they
/// are not dead-flat (it goes through `strata::stratum`).
pub fn zone_at(depth: i32, y: i32, wobble: f32) -> Zone {
    if depth < SOIL_DEPTH {
        return Zone::Soil;
    }
    if y <= GEOTHERMAL_Y {
        return Zone::Geothermal;
    }
    let z = match super::strata::stratum(depth as f32, wobble, &ZONE_BANDS) {
        0 => Zone::Soil,
        1 => Zone::Limestone,
        2 => Zone::Crystal,
        _ => Zone::Abyssal,
    };
    z.max_soil_guard(depth)
}

impl Zone {
    /// `stratum` can wave a cell just below `SOIL_DEPTH` back into band 0; the
    /// soil boundary is a hard line (the backends placed soil to it), so a
    /// cell below it is never `Soil`.
    fn max_soil_guard(self, depth: i32) -> Zone {
        if self == Zone::Soil && depth >= SOIL_DEPTH { Zone::Limestone } else { self }
    }
}

/// Seed offsets, one per independent field.
mod seeds {
    pub const WOBBLE: u32 = 0x5a1e_0001;
    pub const BAND: u32 = 0x5a1e_0002;
    pub const BAND_WARP: u32 = 0x5a1e_0003;
    pub const COAL_WARP: u32 = 0x5a1e_0010;
    pub const COAL_MASK: u32 = 0x5a1e_0011;
    pub const COPPER_A: u32 = 0x5a1e_0020;
    pub const COPPER_B: u32 = 0x5a1e_0021;
    pub const IRON_A: u32 = 0x5a1e_0030;
    pub const IRON_B: u32 = 0x5a1e_0031;
    pub const GOLD_SHEET: u32 = 0x5a1e_0040;
    pub const GOLD_MASK: u32 = 0x5a1e_0041;
    pub const DIAMOND: u32 = 0x5a1e_0050;
    pub const PATCH: u32 = 0x5a1e_0060;
    pub const WET: u32 = 0x5a1e_0061;
    pub const PICK: u32 = 0x5a1e_0070;
}

/// Band thickness of the host-rock strata, in blocks.
const BAND: f64 = 7.0;

/// The rock a `Stone` cell in `zone` becomes. Strata are horizontal bands
/// about `BAND` thick whose floors wave with a slow 2D warp; each band picks
/// one rock from the zone's palette, and about 40% of bands stay plain stone
/// so the named rocks read as layers rather than as the whole mass.
pub fn host_rock(zone: Zone, x: i32, y: i32, z: i32, seed: u32) -> BlockId {
    use BlockId::*;
    let warp = noise_2d(x as f64 * 0.018, z as f64 * 0.018, seed.wrapping_add(seeds::BAND_WARP)) * 9.0;
    let band = ((y as f64 + warp) / BAND).floor() as i32;
    let h = hash_2d(band, zone as i32, seed.wrapping_add(seeds::BAND)) % 100;
    let palette: &[(u32, BlockId)] = match zone {
        Zone::Soil => return Stone,
        Zone::Limestone => &[(40, Stone), (30, Limestone), (15, Dolomite), (10, Andesite), (5, Calcite)],
        Zone::Crystal => &[(40, Stone), (22, Granite), (16, Marble), (14, Andesite), (8, Gypsum)],
        Zone::Abyssal => &[(40, Stone), (28, Slate), (22, Schist), (10, Basalt)],
        Zone::Geothermal => &[(35, Basalt), (25, Stone), (25, Tuff), (15, Andesite)],
    };
    let mut acc = 0;
    for &(w, b) in palette {
        acc += w;
        if h < acc {
            return b;
        }
    }
    Stone
}

/// Is the value-noise sample within `w` of its median 0.5? Two of these on
/// independent fields intersect along a curve, which is what makes a vein a
/// tube rather than a blob.
#[inline]
fn near_mid(n: f64, w: f64) -> bool {
    (n - 0.5).abs() < w
}

/// The ore (if any) in the rock cell at `(x, y, z)`, `depth` below the surface.
///
/// * Coal: flat seams one to two blocks thick, stacked 17 apart, broken up by
///   a 2D mask so a seam runs for tens of blocks and then pinches out.
/// * Copper and iron: winding tubes, where two independent noise fields both
///   sit at their median.
/// * Gold: a thin quartz sheet with a gold core, only where a mask allows.
/// * Diamond: rare pockets at the bottom of the world.
pub fn ore_at(x: i32, y: i32, z: i32, depth: i32, seed: u32) -> Option<BlockId> {
    use BlockId::*;
    let (fx, fy, fz) = (x as f64, y as f64, z as f64);

    if (5..=44).contains(&depth) {
        let warp = noise_2d(fx * 0.03, fz * 0.03, seed.wrapping_add(seeds::COAL_WARP)) * 6.0;
        let f = (fy + warp).rem_euclid(17.0);
        if f < 1.4 {
            let mask = noise_2d(fx * 0.045, fz * 0.045, seed.wrapping_add(seeds::COAL_MASK));
            if mask > 0.6 {
                return Some(CoalOre);
            }
        }
    }
    if (8..=80).contains(&depth) {
        let a = noise_3d(fx * 0.05, fy * 0.07, fz * 0.05, seed.wrapping_add(seeds::COPPER_A));
        if near_mid(a, 0.025) {
            let b = noise_3d(fx * 0.05, fy * 0.07, fz * 0.05, seed.wrapping_add(seeds::COPPER_B));
            if near_mid(b, 0.025) {
                return Some(CopperOre);
            }
        }
    }
    if depth >= 12 {
        let a = noise_3d(fx * 0.045, fy * 0.06, fz * 0.045, seed.wrapping_add(seeds::IRON_A));
        if near_mid(a, 0.028) {
            let b = noise_3d(fx * 0.045, fy * 0.06, fz * 0.045, seed.wrapping_add(seeds::IRON_B));
            if near_mid(b, 0.028) {
                return Some(IronOre);
            }
        }
    }
    if depth >= 26 {
        let sheet = noise_3d(fx * 0.035, fy * 0.035, fz * 0.035, seed.wrapping_add(seeds::GOLD_SHEET));
        if near_mid(sheet, 0.012) {
            let mask = noise_3d(fx * 0.02, fy * 0.02, fz * 0.02, seed.wrapping_add(seeds::GOLD_MASK));
            if mask > 0.62 {
                return Some(if near_mid(sheet, 0.0015) { GoldOre } else { Quartz });
            }
        }
    }
    if y <= 14 {
        let d = noise_3d(fx * 0.22, fy * 0.22, fz * 0.22, seed.wrapping_add(seeds::DIAMOND));
        if d > 0.93 {
            return Some(DiamondOre);
        }
    }
    None
}

/// What the column's surface plant says about the rock below: the ore the
/// indicator species responds to, scanning `ore_at` down the column from
/// `SCAN_TOP` to `SCAN_BOTTOM` blocks deep.
///
/// Iron Root Fern grows over iron. Stone Flower grows over copper and gold
/// (the docs place it "near minerals"; here that means the two soft metals).
pub fn indicator_for(x: i32, surface_y: i32, z: i32, seed: u32) -> Option<BlockId> {
    const SCAN_TOP: i32 = 12;
    const SCAN_BOTTOM: i32 = 24;
    const SCAN_STEP: i32 = 3;
    let mut d = SCAN_TOP;
    while d <= SCAN_BOTTOM {
        // Directly under the plant only: the roots follow the vein, so a
        // plant is a promise that digging straight down finds it.
        match ore_at(x, surface_y - d, z, d, seed) {
            Some(BlockId::IronOre) => return Some(BlockId::IronRootFern),
            Some(BlockId::CopperOre) | Some(BlockId::GoldOre) | Some(BlockId::Quartz) => {
                return Some(BlockId::StoneFlower)
            }
            _ => {}
        }
        d += SCAN_STEP;
    }
    None
}

/// Rock the ore and strata pass may rewrite: the backends' plain stone and the
/// three legacy ore blobs they place.
fn is_host(b: BlockId) -> bool {
    matches!(b, BlockId::Stone)
}

/// A cell a cave ornament can stand on or hang from.
fn is_rock(b: BlockId) -> bool {
    use BlockId::*;
    matches!(
        b,
        Stone | Dirt | Grass | Gravel | Clay | Cobblestone | Limestone | Marble | Basalt | Tuff | Granite
            | Slate | Calcite | Andesite | Schist | Dolomite | Gypsum | CoalOre | IronOre | GoldOre
            | CopperOre | Quartz | DiamondOre | Moss | Magma | Obsidian | Sandstone
    )
}

#[inline]
fn idx(x: i32, y: i32, z: i32) -> usize {
    (x + z * CS + y * CS * CS) as usize
}

/// Deterministic 0..1000 roll for a cell and purpose.
#[inline]
fn roll(x: i32, y: i32, z: i32, seed: u32, salt: u32) -> u32 {
    hash_3d(x, y, z, seed.wrapping_add(seeds::PICK).wrapping_add(salt.wrapping_mul(0x9e37_79b9))) % 1000
}

/// Counts of what one `decorate` call changed, for tests and the soak log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub strata: u32,
    pub ore: u32,
    pub floors: u32,
    pub ornaments: u32,
    pub moss: u32,
    pub indicators: u32,
    pub shore: u32,
    /// Lichen placed on the gas surface of a CO₂ basin.
    pub ring: u32,
}

/// Run the underground pass over one chunk's blocks.
///
/// `origin` is the chunk's block origin; `surface(x, z)` is the terrain's
/// surface height for a world column (the world model's `surface_target`,
/// rounded). Returns what changed.
pub fn decorate(
    blocks: &mut [BlockId],
    origin: (i32, i32, i32),
    seed: u32,
    surface: &dyn Fn(i32, i32) -> i32,
) -> Report {
    let mut rep = Report::default();
    debug_assert_eq!(blocks.len(), (CS * CS * CS) as usize);
    let (ox, oy, oz) = origin;

    let mut surf = [0i32; (CS * CS) as usize];
    let mut max_surf = i32::MIN;
    for lz in 0..CS {
        for lx in 0..CS {
            let s = surface(ox + lx, oz + lz);
            surf[(lx + lz * CS) as usize] = s;
            max_surf = max_surf.max(s);
        }
    }
    // Entirely above the highest surface point plus a margin for the plant
    // layer: nothing to do. (Terrain overhangs above the target surface are
    // left alone.)
    if oy > max_surf + 3 {
        return rep;
    }
    if blocks.iter().all(|&b| b == BlockId::Air) {
        return rep;
    }

    let snap: Vec<BlockId> = blocks.to_vec();
    let at = |lx: i32, ly: i32, lz: i32| -> Option<BlockId> {
        if lx < 0 || ly < 0 || lz < 0 || lx >= CS || ly >= CS || lz >= CS {
            None
        } else {
            Some(snap[idx(lx, ly, lz)])
        }
    };

    // ---- rock: strata and ore --------------------------------------------
    for ly in 0..CS {
        let y = oy + ly;
        for lz in 0..CS {
            for lx in 0..CS {
                let i = idx(lx, ly, lz);
                if !is_host(snap[i]) {
                    continue;
                }
                let (x, z) = (ox + lx, oz + lz);
                let depth = surf[(lx + lz * CS) as usize] - y;
                if depth < SOIL_DEPTH {
                    continue;
                }
                if let Some(o) = ore_at(x, y, z, depth, seed) {
                    blocks[i] = o;
                    rep.ore += 1;
                    continue;
                }
                let wob = noise_2d(x as f64 * 0.05, z as f64 * 0.05, seed.wrapping_add(seeds::WOBBLE)) as f32;
                let zone = zone_at(depth, y, wob);
                let r = host_rock(zone, x, y, z, seed);
                if r != BlockId::Stone {
                    blocks[i] = r;
                    rep.strata += 1;
                }
            }
        }
    }

    // ---- caves ------------------------------------------------------------
    // A cave cell is snapshot air at least SOIL_DEPTH under the surface that
    // has rock above it in this chunk, or that lies well below the chunk top
    // and the surface (a deep chunk whose roof is in the chunk above).
    let roofed = |lx: i32, ly: i32, lz: i32| -> bool {
        let mut yy = ly + 1;
        while yy < CS {
            if is_rock(snap[idx(lx, yy, lz)]) {
                return true;
            }
            yy += 1;
        }
        oy + CS < surf[(lx + lz * CS) as usize] - 8
    };
    for ly in 0..CS {
        let y = oy + ly;
        for lz in 0..CS {
            for lx in 0..CS {
                if snap[idx(lx, ly, lz)] != BlockId::Air {
                    continue;
                }
                let (x, z) = (ox + lx, oz + lz);
                let depth = surf[(lx + lz * CS) as usize] - y;
                if depth < SOIL_DEPTH || !roofed(lx, ly, lz) {
                    continue;
                }
                let wob = noise_2d(x as f64 * 0.05, z as f64 * 0.05, seed.wrapping_add(seeds::WOBBLE)) as f32;
                let zone = zone_at(depth, y, wob);
                let below = at(lx, ly - 1, lz);
                let above = at(lx, ly + 1, lz);
                let floor = below.map_or(false, is_rock);
                let ceiling = above.map_or(false, is_rock);
                let patch = noise_2d(x as f64 * 0.11, z as f64 * 0.11, seed.wrapping_add(seeds::PATCH));

                if floor {
                    let fi = idx(lx, ly - 1, lz);
                    let fl = floor_block(zone, blocks[fi], patch, roll(x, y, z, seed, 1));
                    if fl != blocks[fi] {
                        blocks[fi] = fl;
                        rep.floors += 1;
                    }
                    // CO₂ basins (world::cave_gas): the microbial ring glows on
                    // the floor at the gas surface; below it nothing grows
                    // ("the death of any organisms below the level"), only the
                    // odd bone.
                    if super::cave_gas::on_surface(x, y, z, seed) {
                        rep.ring += set_if_air(blocks, lx, ly, lz, BlockId::Lichen);
                    } else if super::cave_gas::co2_density(x, y, z, seed) > 0.0 {
                        if roll(x, y, z, seed, 9) < 8 {
                            rep.ornaments += set_if_air(blocks, lx, ly, lz, BlockId::Bone);
                        }
                    } else {
                        rep.ornaments += floor_ornament(blocks, lx, ly, lz, zone, patch, x, y, z, depth, seed);
                    }
                }
                if ceiling && blocks[idx(lx, ly, lz)] == BlockId::Air {
                    rep.ornaments += ceiling_ornament(blocks, lx, ly, lz, zone, x, y, z, depth, seed);
                }
                // Walls: moss where it is damp and shallow enough for the
                // limestone water table.
                if zone == Zone::Limestone {
                    let wet = noise_3d(x as f64 * 0.08, y as f64 * 0.08, z as f64 * 0.08, seed.wrapping_add(seeds::WET)) as f32;
                    let cover = super::cave_moss::moss(wet * 1.25 - 0.25, 0.0, hash_3d(x, y, z, seed) as u64);
                    if cover > 0.55 {
                        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                            if let Some(b) = at(lx + dx, ly, lz + dz) {
                                let wi = idx(lx + dx, ly, lz + dz);
                                if is_rock(b)
                                    && matches!(blocks[wi], BlockId::Stone | BlockId::Limestone | BlockId::Dolomite)
                                {
                                    blocks[wi] = BlockId::Moss;
                                    rep.moss += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- surface: indicator plants and shore reeds ---------------------------
    for lz in 0..CS {
        for lx in 0..CS {
            let s = surf[(lx + lz * CS) as usize];
            let (x, z) = (ox + lx, oz + lz);
            for ly in 0..CS - 1 {
                let y = oy + ly;
                if (y - s).abs() > 4 {
                    continue;
                }
                let g = snap[idx(lx, ly, lz)];
                if blocks[idx(lx, ly + 1, lz)] != BlockId::Air || snap[idx(lx, ly + 1, lz)] != BlockId::Air {
                    continue;
                }
                if g == BlockId::Grass && roll(x, y, z, seed, 7) < 350 {
                    if let Some(p) = indicator_for(x, y, z, seed) {
                        blocks[idx(lx, ly + 1, lz)] = p;
                        rep.indicators += 1;
                        continue;
                    }
                }
                if matches!(g, BlockId::Grass | BlockId::Dirt | BlockId::Sand | BlockId::Clay)
                    && roll(x, y, z, seed, 8) < 450
                {
                    let wet = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|&(dx, dz)| at(lx + dx, ly, lz + dz) == Some(BlockId::Water));
                    if wet {
                        blocks[idx(lx, ly + 1, lz)] = BlockId::Cattail;
                        rep.shore += 1;
                    }
                }
            }
        }
    }
    rep
}

/// What a cave floor cell becomes. Grass and dirt (which the GPU backend lays
/// on every floor, lit or not) always change; bare rock changes in patches.
fn floor_block(zone: Zone, cur: BlockId, patch: f64, r: u32) -> BlockId {
    use BlockId::*;
    let soft = matches!(cur, Grass | Dirt);
    let rocky = matches!(cur, Stone | Limestone | Dolomite | Andesite | Granite | Slate | Schist | Basalt | Tuff);
    if !soft && !(rocky && patch > 0.62) {
        return cur;
    }
    match zone {
        Zone::Soil => cur,
        Zone::Limestone => {
            if patch > 0.7 { Moss } else if r < 450 { Clay } else if r < 800 { Gravel } else { Limestone }
        }
        Zone::Crystal => if r < 500 { Gravel } else if r < 800 { Calcite } else { Gypsum },
        Zone::Abyssal => if r < 600 { Gravel } else { Slate },
        Zone::Geothermal => {
            if patch > 0.75 && r < 500 { Magma } else if r < 500 { Tuff } else { Basalt }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn floor_ornament(
    blocks: &mut [BlockId],
    lx: i32, ly: i32, lz: i32,
    zone: Zone, patch: f64,
    x: i32, y: i32, z: i32, depth: i32, seed: u32,
) -> u32 {
    use BlockId::*;
    let r = roll(x, y, z, seed, 2);
    let place = match zone {
        Zone::Soil => None,
        Zone::Limestone => {
            if r < 30 {
                return grow(blocks, lx, ly, lz, 1, Dripstone, 1 + (r % 3) as i32);
            } else if patch > 0.66 && r < 260 {
                Some(if depth > 16 && r % 3 == 0 { Glowshroom } else { Mushroom })
            } else {
                None
            }
        }
        Zone::Crystal => {
            if r < 40 {
                let c = match r % 4 { 0 => GlowCrystal, 1 => Selenite, 2 => Quartz, _ => Crystal };
                return grow(blocks, lx, ly, lz, 1, c, 1 + (r % 2) as i32);
            } else if patch > 0.72 && r < 160 {
                Some(Glowshroom)
            } else {
                None
            }
        }
        Zone::Abyssal => {
            if patch > 0.6 && r < 300 { Some(Glowshroom) } else if r < 6 { Some(Bone) } else { None }
        }
        Zone::Geothermal => if r < 25 { Some(Sulfur) } else if r < 35 { Some(Obsidian) } else { None },
    };
    match place {
        Some(b) if blocks[idx(lx, ly, lz)] == Air => {
            blocks[idx(lx, ly, lz)] = b;
            1
        }
        _ => 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn ceiling_ornament(
    blocks: &mut [BlockId],
    lx: i32, ly: i32, lz: i32,
    zone: Zone,
    x: i32, y: i32, z: i32, depth: i32, seed: u32,
) -> u32 {
    use BlockId::*;
    let r = roll(x, y, z, seed, 3);
    match zone {
        Zone::Soil => 0,
        Zone::Limestone => {
            if r < 50 {
                grow(blocks, lx, ly, lz, -1, Dripstone, 1 + (r % 3) as i32)
            } else if depth < 14 && r < 110 {
                grow(blocks, lx, ly, lz, -1, Vine, 1 + (r % 4) as i32)
            } else if r < 114 {
                set_if_air(blocks, lx, ly, lz, Cobweb)
            } else {
                0
            }
        }
        Zone::Crystal => {
            if r < 25 { grow(blocks, lx, ly, lz, -1, GlowCrystal, 1) } else if r < 29 { set_if_air(blocks, lx, ly, lz, Cobweb) } else { 0 }
        }
        Zone::Abyssal => {
            if r < 20 { grow(blocks, lx, ly, lz, -1, Dripstone, 1 + (r % 2) as i32) } else if r < 26 { set_if_air(blocks, lx, ly, lz, Cobweb) } else { 0 }
        }
        Zone::Geothermal => if r < 15 { grow(blocks, lx, ly, lz, -1, Basalt, 1 + (r % 2) as i32) } else { 0 },
    }
}

fn set_if_air(blocks: &mut [BlockId], lx: i32, ly: i32, lz: i32, b: BlockId) -> u32 {
    let i = idx(lx, ly, lz);
    if blocks[i] == BlockId::Air {
        blocks[i] = b;
        1
    } else {
        0
    }
}

/// Grow a column of `b` up (`dir` 1) or down (`dir` -1) from `(lx, ly, lz)`
/// for up to `len` cells, stopping at anything that is not air and at the
/// chunk edge. A column never fills its gap completely: it leaves at least one
/// air cell before the opposite side so a cave stays passable.
fn grow(blocks: &mut [BlockId], lx: i32, ly: i32, lz: i32, dir: i32, b: BlockId, len: i32) -> u32 {
    let mut n = 0;
    for k in 0..len {
        let yy = ly + dir * k;
        let next = yy + dir;
        if yy < 0 || yy >= CS || blocks[idx(lx, yy, lz)] != BlockId::Air {
            break;
        }
        // Keep a gap: the cell beyond must also be air (or off-chunk).
        if (0..CS).contains(&next) && blocks[idx(lx, next, lz)] != BlockId::Air {
            break;
        }
        blocks[idx(lx, yy, lz)] = b;
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u32 = 0x00c0_ffee;

    fn air_chunk() -> Vec<BlockId> {
        vec![BlockId::Air; (CS * CS * CS) as usize]
    }

    /// A chunk of solid stone with the surface far above it.
    fn deep_stone(oy: i32, surface: i32) -> (Vec<BlockId>, Report) {
        let mut b = vec![BlockId::Stone; (CS * CS * CS) as usize];
        let r = decorate(&mut b, (0, oy, 0), SEED, &|_, _| surface);
        (b, r)
    }

    fn count(b: &[BlockId], f: impl Fn(BlockId) -> bool) -> usize {
        b.iter().filter(|&&x| f(x)).count()
    }

    #[test]
    fn zones_follow_depth_and_the_geothermal_floor() {
        assert_eq!(zone_at(2, 60, 0.5), Zone::Soil);
        assert_eq!(zone_at(SOIL_DEPTH, 60, 0.0), Zone::Limestone, "soil line is hard");
        assert_eq!(zone_at(12, 50, 0.5), Zone::Limestone);
        assert_eq!(zone_at(40, 40, 0.5), Zone::Crystal);
        assert_eq!(zone_at(90, 30, 0.5), Zone::Abyssal);
        assert_eq!(zone_at(20, GEOTHERMAL_Y, 0.5), Zone::Geothermal);
        assert_eq!(zone_at(20, GEOTHERMAL_Y + 1, 0.5), Zone::Limestone);
        // Soil stays soil even down at geothermal heights (a deep valley floor).
        assert_eq!(zone_at(2, GEOTHERMAL_Y - 5, 0.5), Zone::Soil);
    }

    #[test]
    fn above_the_surface_nothing_changes() {
        let mut b = air_chunk();
        b[idx(3, 3, 3)] = BlockId::Stone;
        let before = b.clone();
        let r = decorate(&mut b, (0, 128, 0), SEED, &|_, _| 64);
        assert_eq!(r, Report::default());
        assert_eq!(b, before);
    }

    #[test]
    fn soil_band_is_left_alone() {
        // Chunk spans y 32..64, surface at 64: rows with depth < SOIL_DEPTH
        // (y 61..63) keep their stone.
        let (b, _) = deep_stone(32, 64);
        for ly in (CS - (SOIL_DEPTH - 1))..CS {
            for lz in 0..CS {
                for lx in 0..CS {
                    assert_eq!(b[idx(lx, ly, lz)], BlockId::Stone, "soil row {ly} changed");
                }
            }
        }
    }

    /// Ore share of rock over a slab of world, per ore, with ranges in both
    /// directions: too little and nobody finds it, too much and it is not ore.
    #[test]
    fn ore_shares_are_in_range() {
        let mut total = 0usize;
        let mut coal = 0usize;
        let mut copper = 0usize;
        let mut iron = 0usize;
        let mut gold = 0usize;
        let mut quartz = 0usize;
        for (cx, oy) in [(0, 0), (1, 0), (2, 32), (3, 32), (4, 32)] {
            for cz in 0..2 {
                let mut b = vec![BlockId::Stone; (CS * CS * CS) as usize];
                decorate(&mut b, (cx * CS, oy, cz * CS), SEED, &|_, _| 70);
                total += b.len();
                coal += count(&b, |x| x == BlockId::CoalOre);
                copper += count(&b, |x| x == BlockId::CopperOre);
                iron += count(&b, |x| x == BlockId::IronOre);
                gold += count(&b, |x| x == BlockId::GoldOre);
                quartz += count(&b, |x| x == BlockId::Quartz);
            }
        }
        let pct = |n: usize| n as f64 * 100.0 / total as f64;
        let all = pct(coal + copper + iron + gold + quartz);
        eprintln!(
            "coal {:.2}% copper {:.2}% iron {:.2}% gold {:.3}% quartz {:.2}% all {:.2}%",
            pct(coal), pct(copper), pct(iron), pct(gold), pct(quartz), all
        );
        assert!((0.3..=2.0).contains(&pct(coal)), "coal {}", pct(coal));
        assert!((0.1..=1.2).contains(&pct(copper)), "copper {}", pct(copper));
        assert!((0.1..=1.2).contains(&pct(iron)), "iron {}", pct(iron));
        assert!(gold > 0 && pct(gold) < 0.2, "gold {}", pct(gold));
        assert!(quartz > gold, "gold sits inside quartz: q {quartz} g {gold}");
        assert!((0.6..=4.0).contains(&all), "all ore {all}");
    }

    #[test]
    fn veins_are_connected_not_speckle() {
        // Most ore cells of a tube ore touch another cell of the same ore.
        let mut b = vec![BlockId::Stone; (CS * CS * CS) as usize];
        decorate(&mut b, (0, 32, 0), SEED, &|_, _| 70);
        for ore in [BlockId::IronOre, BlockId::CopperOre, BlockId::CoalOre] {
            let mut n = 0;
            let mut linked = 0;
            for ly in 1..CS - 1 {
                for lz in 1..CS - 1 {
                    for lx in 1..CS - 1 {
                        if b[idx(lx, ly, lz)] != ore {
                            continue;
                        }
                        n += 1;
                        let nb = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];
                        if nb.iter().any(|&(dx, dy, dz)| b[idx(lx + dx, ly + dy, lz + dz)] == ore) {
                            linked += 1;
                        }
                    }
                }
            }
            assert!(n >= 10, "{ore:?}: only {n} cells in the sample chunk");
            assert!(linked as f64 / n as f64 > 0.8, "{ore:?}: {linked}/{n} linked");
        }
    }

    #[test]
    fn strata_come_in_bands_and_leave_some_plain_stone() {
        let (b, rep) = deep_stone(0, 70);
        assert!(rep.strata > 0);
        let rock = b.len() - count(&b, |x| x == BlockId::Air);
        let plain = count(&b, |x| x == BlockId::Stone) as f64 / rock as f64;
        assert!((0.15..=0.75).contains(&plain), "plain stone share {plain}");
        // Along a vertical line, the rock changes rarely (bands), not every
        // cell (speckle): count changes in one column.
        let mut changes = 0;
        for ly in 1..CS {
            let (a, c) = (b[idx(5, ly - 1, 5)], b[idx(5, ly, 5)]);
            let orey = |x: BlockId| matches!(x, BlockId::CoalOre | BlockId::IronOre | BlockId::CopperOre | BlockId::GoldOre | BlockId::Quartz | BlockId::DiamondOre);
            if a != c && !orey(a) && !orey(c) {
                changes += 1;
            }
        }
        assert!(changes <= 10, "{changes} rock changes in one 32-cell column");
    }

    /// A box cave: stone shell, air inside, 'surface' well above.
    fn cave_box(oy: i32, surface: i32, floor: BlockId) -> (Vec<BlockId>, Report) {
        let mut b = vec![BlockId::Stone; (CS * CS * CS) as usize];
        for ly in 8..20 {
            for lz in 2..30 {
                for lx in 2..30 {
                    b[idx(lx, ly, lz)] = BlockId::Air;
                }
            }
        }
        for lz in 2..30 {
            for lx in 2..30 {
                b[idx(lx, 7, lz)] = floor;
            }
        }
        let r = decorate(&mut b, (0, oy, 0), SEED, &|_, _| surface);
        (b, r)
    }

    #[test]
    fn cave_floors_lose_their_grass() {
        let (b, rep) = cave_box(24, 64, BlockId::Grass);
        assert_eq!(count(&b, |x| x == BlockId::Grass), 0, "grass left on a cave floor");
        assert!(rep.floors as usize >= 28 * 28 - 50, "floors {}", rep.floors);
    }

    #[test]
    fn limestone_caves_grow_dripstone_both_ways_and_stay_passable() {
        let (b, rep) = cave_box(24, 54, BlockId::Stone); // y 32..43, depth 11..22
        assert!(rep.ornaments > 0);
        let mut up = 0;
        let mut down = 0;
        for lz in 2..30 {
            for lx in 2..30 {
                if b[idx(lx, 8, lz)] == BlockId::Dripstone {
                    up += 1;
                }
                if b[idx(lx, 19, lz)] == BlockId::Dripstone {
                    down += 1;
                }
                // No column fills floor to ceiling.
                assert!((8..20).any(|ly| b[idx(lx, ly, lz)] == BlockId::Air), "column {lx},{lz} sealed");
            }
        }
        assert!(up > 0 && down > 0, "stalagmites {up} stalactites {down}");
        assert!(up + down < 28 * 28 / 4, "dripstone everywhere: {}", up + down);
    }

    #[test]
    fn a_co2_basin_gets_a_glowing_ring_at_its_surface_and_a_dead_floor_below() {
        let b = (0..40)
            .flat_map(|rx| (0..40).map(move |rz| (rx, rz)))
            .find_map(|(rx, rz)| super::super::cave_gas::basin(rx, rz, SEED))
            .expect("a basin");
        // A chunk-sized cave around the basin centre: a floor that steps down
        // from above the gas surface to below it along x.
        let origin = (b.center[0] - 16, b.surface_y - 16, b.center[1] - 16);
        let mut blocks = vec![BlockId::Stone; (CS * CS * CS) as usize];
        for lz in 1..CS - 1 {
            for lx in 1..CS - 1 {
                let floor = 12 + lx / 4; // local floor height: 12..19, gas surface at local 16
                for ly in floor + 1..CS - 2 {
                    blocks[idx(lx, ly, lz)] = BlockId::Air;
                }
            }
        }
        let rep = decorate(&mut blocks, origin, SEED, &|_, _| 90);
        assert!(rep.ring > 0, "no ring placed");
        // What grew into the carved air, split by whether the cell is in the
        // gas. Only bone lies below the surface; above it the normal floor
        // life grows (the complement: without it this test would pass on a
        // cave where nothing grows at all).
        let (mut grown_in_gas, mut grown_above) = (Vec::new(), 0);
        for i in 0..blocks.len() {
            let (lx, ly, lz) = (i as i32 % CS, i as i32 / (CS * CS), (i as i32 / CS) % CS);
            let (x, y, z) = (origin.0 + lx, origin.1 + ly, origin.2 + lz);
            if blocks[i] == BlockId::Lichen {
                assert_eq!(y, b.surface_y, "lichen off the gas line at y {y}");
            }
            let carved = (1..CS - 1).contains(&lx) && (1..CS - 1).contains(&lz) && ly > 12 + lx / 4 && ly < CS - 2;
            if !carved || matches!(blocks[i], BlockId::Air | BlockId::Lichen) {
                continue;
            }
            if super::super::cave_gas::co2_density(x, y, z, SEED) > 0.0 {
                if blocks[i] != BlockId::Bone {
                    grown_in_gas.push(blocks[i]);
                }
            } else {
                grown_above += 1;
            }
        }
        assert!(grown_in_gas.is_empty(), "something grows in the gas: {grown_in_gas:?}");
        assert!(grown_above > 0, "nothing grew above the gas either, so this proves nothing");
    }

    #[test]
    fn low_crawlways_are_never_sealed() {
        // A two-high crawlway through limestone: stalactites and stalagmites
        // can each be up to three long, but must leave the passage open.
        let mut b = vec![BlockId::Stone; (CS * CS * CS) as usize];
        for ly in 10..12 {
            for lz in 1..31 {
                for lx in 1..31 {
                    b[idx(lx, ly, lz)] = BlockId::Air;
                }
            }
        }
        let rep = decorate(&mut b, (0, 24, 0), SEED, &|_, _| 54);
        assert!(rep.ornaments > 0, "nothing grew, so this proves nothing");
        for lz in 1..31 {
            for lx in 1..31 {
                // Passable, not necessarily empty: a mushroom or cobweb over a
                // stalagmite stub leaves the way open (the first cut of this
                // test demanded Air and failed on exactly that).
                assert!(
                    (10..12).any(|ly| !b[idx(lx, ly, lz)].is_solid()),
                    "crawlway sealed at {lx},{lz}: {:?}",
                    (10..12).map(|ly| b[idx(lx, ly, lz)]).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn deep_caves_differ_from_shallow_ones() {
        let (shallow, _) = cave_box(24, 54, BlockId::Stone);
        let (geo, _) = cave_box(0, 64, BlockId::Stone); // y 8..19 -> geothermal
        let deep_things = |b: &[BlockId]| count(b, |x| matches!(x, BlockId::Basalt | BlockId::Tuff | BlockId::Sulfur | BlockId::Magma));
        assert!(deep_things(&geo) > 100, "geothermal cave has {}", deep_things(&geo));
        assert_eq!(count(&shallow, |x| matches!(x, BlockId::Sulfur | BlockId::Magma)), 0);
        assert!(count(&shallow, |x| x == BlockId::Dripstone) > count(&geo, |x| x == BlockId::Dripstone));
    }

    #[test]
    fn open_air_over_a_low_surface_is_not_a_cave() {
        // The target surface says 64 but the real ground here is at y 40 with
        // open sky above: no vines, webs or dripstone in the open.
        let mut b = air_chunk();
        for ly in 0..8 {
            for lz in 0..CS {
                for lx in 0..CS {
                    b[idx(lx, ly, lz)] = BlockId::Stone;
                }
            }
        }
        decorate(&mut b, (0, 32, 0), SEED, &|_, _| 64);
        for ly in 8..CS {
            for lz in 0..CS {
                for lx in 0..CS {
                    assert_eq!(b[idx(lx, ly, lz)], BlockId::Air, "ornament in open air at {lx},{ly},{lz}");
                }
            }
        }
    }

    #[test]
    fn indicator_plants_sit_over_their_ore() {
        // Scan surface columns; every Iron Root Fern must have iron under it
        // in the scan window, and some columns must get one.
        let mut ferns = 0;
        let mut flowers = 0;
        for x in 0..400 {
            for z in 0..40 {
                match indicator_for(x, 64, z, SEED) {
                    Some(BlockId::IronRootFern) => ferns += 1,
                    Some(BlockId::StoneFlower) => flowers += 1,
                    Some(o) => panic!("unexpected indicator {o:?}"),
                    None => {}
                }
            }
        }
        let n = 400.0 * 40.0;
        eprintln!("fern cols {:.2}% flower cols {:.2}%", ferns as f64 * 100.0 / n, flowers as f64 * 100.0 / n);
        assert!(ferns > 0 && flowers > 0);
        // Not everywhere, or it tells you nothing. (The first cut scanned
        // five columns 12..44 deep and flagged 47% of the map; one column
        // 12..30 deep still flagged 19%.)
        assert!((ferns + flowers) as f64 / n < 0.15, "indicators on {} of {} columns", ferns + flowers, n);
        // And never where the column is barren.
        for x in 0..200 {
            let z = 7;
            if indicator_for(x, 64, z, SEED) == Some(BlockId::IronRootFern) {
                let mut found = false;
                let mut d = 12;
                while d <= 24 && !found {
                    found |= ore_at(x, 64 - d, z, d, SEED) == Some(BlockId::IronOre);
                    d += 3;
                }
                assert!(found, "fern at x {x} with no iron below");
            }
        }
    }

    #[test]
    fn cattails_grow_only_beside_water() {
        let mut b = air_chunk();
        // Ground at ly 10 (y 58, surface 58): grass, with a pond strip at lx 10.
        for lz in 0..CS {
            for lx in 0..CS {
                for ly in 0..=10 {
                    b[idx(lx, ly, lz)] = BlockId::Dirt;
                }
                b[idx(lx, 10, lz)] = if lx == 10 { BlockId::Water } else { BlockId::Grass };
            }
        }
        let rep = decorate(&mut b, (0, 48, 0), SEED, &|_, _| 58);
        assert!(rep.shore > 0);
        for lz in 0..CS {
            for lx in 0..CS {
                if b[idx(lx, 11, lz)] == BlockId::Cattail {
                    assert!(lx == 9 || lx == 11, "cattail away from water at {lx}");
                }
            }
        }
    }

    #[test]
    fn deterministic_and_chunk_independent() {
        let (a, _) = deep_stone(0, 70);
        let (b, _) = deep_stone(0, 70);
        assert_eq!(a, b);
        // The same world cell decides the same way in two different chunk
        // layouts: rock decisions do not depend on the chunk.
        let mut c1 = vec![BlockId::Stone; (CS * CS * CS) as usize];
        decorate(&mut c1, (CS, 0, 0), SEED, &|_, _| 70);
        for ly in 0..CS {
            for lz in 0..CS {
                let x = CS + 3;
                let depth = 70 - ly;
                let expect = ore_at(x, ly, lz, depth, SEED);
                if let Some(o) = expect {
                    assert_eq!(c1[idx(3, ly, lz)], o);
                }
            }
        }
    }
}
