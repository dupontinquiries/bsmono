//! Dynamic per-cell **wetness** — the first dynamic-material field. Water and
//! rain wet nearby surfaces; wetness wicks outward through porous blocks, caps
//! at each block's absorbency, and dries over time (slower in humid air). At
//! render time it modulates the static [`crate::material::Material`]: wet
//! surfaces are glossier, more reflective, and darker.
//!
//! **Cullable by construction.** Only damp cells are stored, so a dry world
//! costs nothing; drying removes cells (the field self-prunes back to empty);
//! and [`WetnessField::retain_within`] drops everything outside an active region
//! so the simulation can follow the player and forget the rest. The propagation
//! step is pure and order-independent (max-based spread from a snapshot, uniform
//! evaporation), so it's fully deterministic and unit-tested.

use crate::material::Material;
use crate::world::blocks::BlockId;
use std::collections::HashMap;

pub const DRY: u8 = 0;
pub const SATURATED: u8 = 255;

/// Cell coordinate in chunk-independent block space.
pub type Cell = (i32, i32, i32);

/// Per-block water-holding capacity (max wetness, 0..=255). Porous ground and
/// wood hold a lot; stone takes a film; glass/metal/ice barely wet; air and lava
/// don't wet at all. Exhaustive — a new block must declare its absorbency.
pub fn absorbency(block: BlockId) -> u8 {
    use BlockId::*;
    match block {
        Air | Lava => 0,
        Water => SATURATED,
        Dirt | Grass | Sand | Gravel | Clay | Moss => 220, // porous ground soaks
        Snow | SnowBlock => 120,
        OakLog | BirchLog | OakPlanks | Bookshelf | DarkOakPlanks => 180, // wood drinks
        OakLeaves | BirchLeaves | Cactus | Pumpkin | Flower | TallGrass | Fern | DeadBush
        | Vine | Mushroom | Cobweb | Lichen | Carrot | Potato | Cabbage | Sprout => 90, // foliage holds droplets
        Stone | Cobblestone | Bedrock | RedSandstone | Dripstone | Limestone | Basalt | Tuff
        | Granite | Slate | Andesite | Schist | Sandstone | StoneBricks | Bricks | CoalBlock
        | Marble | Calcite | MossyCobblestone | PolishedStone | PolishedBasalt | ChiseledStoneBricks | SmoothSandstone | Terracotta => 60, // stone takes a film
        GoldOre | IronOre | CoalOre | CopperOre | DiamondOre => 60,
        Glass | Ice | Obsidian | Quartz | Crystal | Amber | IronBlock | GoldBlock | Bone => 30,
    }
}

/// Tunables for one wetness step.
#[derive(Debug, Clone, Copy)]
pub struct WetnessParams {
    /// Wetness lost per cell of horizontal/vertical spread (wicking falloff).
    pub spread_falloff: u8,
    /// Base evaporation per step in fully dry air (humidity 0). Scaled down by
    /// humidity, to 0 at humidity 255 (saturated air → no net drying).
    pub evaporation: u8,
}

impl Default for WetnessParams {
    fn default() -> Self {
        Self { spread_falloff: 60, evaporation: 12 }
    }
}

/// Sparse wetness field. Absent cell = [`DRY`].
#[derive(Default, Clone, Debug)]
pub struct WetnessField {
    cells: HashMap<Cell, u8>,
}

const NEIGHBORS: [Cell; 6] = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];

impl WetnessField {
    pub fn new() -> Self {
        Self { cells: HashMap::new() }
    }

    /// Number of currently-damp cells — the active set size (what culling bounds).
    pub fn len(&self) -> usize {
        self.cells.len()
    }
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn get(&self, c: Cell) -> u8 {
        self.cells.get(&c).copied().unwrap_or(DRY)
    }

    /// Set a cell's wetness; storing `DRY` removes it (keeps the map sparse).
    pub fn set(&mut self, c: Cell, w: u8) {
        if w == DRY {
            self.cells.remove(&c);
        } else {
            self.cells.insert(c, w);
        }
    }

    pub fn active_cells(&self) -> impl Iterator<Item = (&Cell, &u8)> {
        self.cells.iter()
    }

    /// Cull: drop every damp cell outside the inclusive box `[min, max]`. The
    /// caller runs this around the active region (e.g. the player's loaded
    /// chunks) so the field never tracks the whole world.
    pub fn retain_within(&mut self, min: Cell, max: Cell) {
        self.cells.retain(|&(x, y, z), _| {
            x >= min.0 && x <= max.0 && y >= min.1 && y <= max.1 && z >= min.2 && z <= max.2
        });
    }

    /// Materialize a `dims`-sized region anchored at `origin` into the compact
    /// nibble-packed upload form (4 bits/cell, two cells/byte). The simulation
    /// keeps full `u8` for stable diffusion; this is the render/upload form, and
    /// 16 levels of wetness sheen is below the perceptual threshold — so it
    /// halves the per-chunk upload at no visible cost. Cells are row-major with
    /// x fastest, matching [`PackedWetnessGrid::idx`].
    pub fn pack_region(&self, origin: Cell, dims: [usize; 3]) -> PackedWetnessGrid {
        let (nx, ny, nz) = (dims[0], dims[1], dims[2]);
        let mut levels = Vec::with_capacity(nx * ny * nz);
        for z in 0..nz {
            for y in 0..ny {
                for x in 0..nx {
                    let c = (origin.0 + x as i32, origin.1 + y as i32, origin.2 + z as i32);
                    levels.push(wet_level(self.get(c)));
                }
            }
        }
        PackedWetnessGrid::from_levels(nx, ny, nz, &levels)
    }

    /// Advance one tick. `sources` are cells touching water (re-saturated to
    /// their capacity each step); `humidity` (0..=255) slows evaporation;
    /// `block_at` supplies the block per cell for absorbency. Pure and
    /// deterministic: spread reads a snapshot (max-based, so order-independent),
    /// then uniform evaporation, then prune to keep the field sparse.
    pub fn step(
        &mut self,
        params: &WetnessParams,
        humidity: u8,
        sources: &[Cell],
        block_at: impl Fn(Cell) -> BlockId,
    ) {
        // Snapshot with sources saturated to capacity — these are the donors.
        let mut snap = self.cells.clone();
        for &s in sources {
            let cap = absorbency(block_at(s));
            if cap > 0 {
                let e = snap.entry(s).or_insert(0);
                *e = (*e).max(cap);
            }
        }

        // Spread: each donor offers (wetness - falloff) to each neighbor, capped
        // by the neighbor's absorbency. Max-based, written into `next`, so the
        // result doesn't depend on iteration order.
        let mut next = snap.clone();
        for (&cell, &w) in &snap {
            let offer = w.saturating_sub(params.spread_falloff);
            if offer == 0 {
                continue;
            }
            for d in NEIGHBORS {
                let n = (cell.0 + d.0, cell.1 + d.1, cell.2 + d.2);
                let cap = absorbency(block_at(n));
                if cap == 0 {
                    continue;
                }
                let give = offer.min(cap);
                let slot = next.entry(n).or_insert(0);
                *slot = (*slot).max(give);
            }
        }

        // Evaporate everything except re-fed sources; humidity throttles it.
        let evap = ((params.evaporation as u16 * (255 - humidity as u16)) / 255) as u8;
        if evap > 0 {
            let src: std::collections::HashSet<Cell> = sources.iter().copied().collect();
            for (cell, w) in next.iter_mut() {
                if !src.contains(cell) {
                    *w = w.saturating_sub(evap);
                }
            }
        }

        next.retain(|_, &mut w| w > DRY);
        self.cells = next;
    }
}

/// Quantize full-range wetness (0..=255) to a 4-bit level (0..=15), rounded.
#[inline]
pub fn wet_level(w: u8) -> u8 {
    (((w as u16) * 15 + 127) / 255) as u8
}

/// Expand a 4-bit level (0..=15) back into the 0..=255 range, evenly spaced.
#[inline]
pub fn wet_from_level(level: u8) -> u8 {
    ((level.min(15) as u16 * 255 + 7) / 15) as u8
}

/// Compact upload form of a wetness region: 4 bits per cell, two cells per byte
/// — half a dense `u8` grid. Deliberately lossy (the [`WetnessField`] sim keeps
/// `u8`); 16 levels of sheen is imperceptible. Random-accessible like the packed
/// light grid, so it's GPU-friendly.
#[derive(Clone, Debug)]
pub struct PackedWetnessGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Nibble-packed levels; cell `i` lives in byte `i/2`, low nibble if `i` even.
    pub data: Vec<u8>,
}

impl PackedWetnessGrid {
    /// Pack a dense slice of 4-bit levels (row-major, x fastest).
    pub fn from_levels(nx: usize, ny: usize, nz: usize, levels: &[u8]) -> Self {
        let n = nx * ny * nz;
        debug_assert_eq!(levels.len(), n, "level count must equal nx*ny*nz");
        let mut data = vec![0u8; n.div_ceil(2)];
        for (i, &lvl) in levels.iter().enumerate() {
            let l = lvl.min(15);
            if i & 1 == 0 {
                data[i / 2] |= l;
            } else {
                data[i / 2] |= l << 4;
            }
        }
        Self { nx, ny, nz, data }
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }

    /// The raw 4-bit level (0..=15) at a cell.
    #[inline]
    pub fn level(&self, x: usize, y: usize, z: usize) -> u8 {
        let i = self.idx(x, y, z);
        if i & 1 == 0 {
            self.data[i / 2] & 0xF
        } else {
            self.data[i / 2] >> 4
        }
    }

    /// Wetness at a cell expanded back to 0..=255.
    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> u8 {
        wet_from_level(self.level(x, y, z))
    }

    /// Bytes occupied — about half a dense `u8` grid of the same region.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }
}

/// Apply wetness to a block's static material: wet surfaces are glossier (lower
/// roughness) and more reflective (higher F0). Metalness and emission are
/// untouched. Pure; `wetness == 0` returns the base material unchanged.
pub fn apply_wetness(base: Material, wetness: u8) -> Material {
    if wetness == DRY {
        return base;
    }
    let w = wetness as f32 / 255.0;
    Material {
        roughness: (base.roughness * (1.0 - 0.7 * w)).clamp(0.0, 1.0),
        metalness: base.metalness,
        reflectance: (base.reflectance + 0.06 * w).clamp(0.0, 1.0),
        emission: base.emission,
    }
}

/// Albedo darkening factor for a wet surface (water film deepens color). 1.0 dry.
pub fn wet_albedo_factor(wetness: u8) -> f32 {
    1.0 - 0.3 * (wetness as f32 / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wetness_level_round_trips_within_one_step() {
        for w in 0..=255u8 {
            let lvl = wet_level(w);
            assert!(lvl <= 15, "level {lvl} out of range for w={w}");
            let back = wet_from_level(lvl);
            // 256 values mapped onto 16 levels: within half a step (~9).
            assert!((back as i16 - w as i16).abs() <= 9, "w={w} -> lvl {lvl} -> {back}");
        }
        assert_eq!(wet_level(DRY), 0);
        assert_eq!(wet_level(SATURATED), 15);
        assert_eq!(wet_from_level(0), 0);
        assert_eq!(wet_from_level(15), 255);
    }

    #[test]
    fn packed_wetness_grid_halves_storage_and_reads_back() {
        let mut f = WetnessField::new();
        f.set((5, 0, 0), SATURATED);
        f.set((6, 0, 0), 128);
        f.set((5, 1, 0), 60);
        // Region (5..9, 0..2, 0) = 8 cells.
        let grid = f.pack_region((5, 0, 0), [4, 2, 1]);
        // 8 cells -> 4 bytes (half a dense u8 grid).
        assert_eq!(grid.byte_len(), 4);
        assert_eq!(grid.get(0, 0, 0), 255); // (5,0,0) saturated
        assert_eq!(grid.level(0, 0, 0), 15);
        assert_eq!(grid.get(3, 1, 0), 0); // (8,1,0) untouched -> dry
        let mid = grid.get(1, 0, 0); // (6,0,0) = 128
        assert!((mid as i16 - 128).abs() <= 9, "mid {mid}");
        let low = grid.get(0, 1, 0); // (5,1,0) = 60
        assert!((low as i16 - 60).abs() <= 9, "low {low}");
    }

    // A world that's all the given block (for absorbency lookups in tests).
    fn uniform(block: BlockId) -> impl Fn(Cell) -> BlockId {
        move |_| block
    }

    #[test]
    fn a_dry_world_costs_nothing() {
        let mut f = WetnessField::new();
        assert!(f.is_empty());
        // No sources, nothing wet → stays empty after stepping.
        f.step(&WetnessParams::default(), 128, &[], uniform(BlockId::Dirt));
        assert!(f.is_empty());
        assert_eq!(f.get((0, 0, 0)), DRY);
    }

    #[test]
    fn water_source_wets_and_wicks_outward_with_falloff() {
        let mut f = WetnessField::new();
        let p = WetnessParams { spread_falloff: 60, evaporation: 0 };
        // Source at origin on porous dirt; step a few times so it can wick out.
        for _ in 0..4 {
            f.step(&p, 128, &[(0, 0, 0)], uniform(BlockId::Dirt));
        }
        assert_eq!(f.get((0, 0, 0)), 220, "source saturated to dirt's capacity");
        let near = f.get((1, 0, 0));
        let far = f.get((3, 0, 0));
        assert!(near > far && far > DRY, "wetness falls off with distance: near {near} far {far}");
        assert!(near < f.get((0, 0, 0)), "neighbor less wet than the source");
    }

    #[test]
    fn absorbency_caps_how_wet_a_block_gets() {
        let p = WetnessParams { spread_falloff: 60, evaporation: 0 };
        // Glass (cap 30) barely wets; dirt (cap 220) holds a lot.
        let mut glass = WetnessField::new();
        let mut dirt = WetnessField::new();
        for _ in 0..3 {
            glass.step(&p, 128, &[(0, 0, 0)], uniform(BlockId::Glass));
            dirt.step(&p, 128, &[(0, 0, 0)], uniform(BlockId::Dirt));
        }
        assert!(glass.get((0, 0, 0)) <= 30, "glass capped near its absorbency");
        assert!(dirt.get((0, 0, 0)) > 200, "dirt holds much more");
    }

    #[test]
    fn it_dries_and_prunes_back_to_empty_when_the_source_is_gone() {
        let mut f = WetnessField::new();
        let p = WetnessParams { spread_falloff: 60, evaporation: 20 };
        for _ in 0..4 {
            f.step(&p, 100, &[(0, 0, 0)], uniform(BlockId::Dirt));
        }
        assert!(!f.is_empty(), "wet while source present");
        // Source removed: keep stepping; everything must dry and prune.
        for _ in 0..40 {
            f.step(&p, 100, &[], uniform(BlockId::Dirt));
        }
        assert!(f.is_empty(), "dried out and self-pruned (cullable), left {}", f.len());
    }

    #[test]
    fn humidity_slows_drying() {
        let mk = || {
            let mut f = WetnessField::new();
            f.set((0, 0, 0), SATURATED);
            f
        };
        let p = WetnessParams { spread_falloff: 255, evaporation: 20 }; // no spread
        let (mut dry_air, mut humid_air) = (mk(), mk());
        for _ in 0..3 {
            dry_air.step(&p, 0, &[], uniform(BlockId::Stone));
            humid_air.step(&p, 230, &[], uniform(BlockId::Stone));
        }
        assert!(
            humid_air.get((0, 0, 0)) > dry_air.get((0, 0, 0)),
            "humid air keeps it wetter: humid {} dry {}",
            humid_air.get((0, 0, 0)),
            dry_air.get((0, 0, 0))
        );
    }

    #[test]
    fn retain_within_culls_far_cells() {
        let mut f = WetnessField::new();
        f.set((0, 0, 0), 100);
        f.set((50, 0, 0), 100);
        f.set((0, 99, 0), 100);
        f.retain_within((-8, -8, -8), (8, 8, 8));
        assert_eq!(f.get((0, 0, 0)), 100, "kept inside region");
        assert_eq!(f.get((50, 0, 0)), DRY, "culled outside region");
        assert_eq!(f.get((0, 99, 0)), DRY, "culled outside region");
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn apply_wetness_makes_surfaces_glossier_and_more_reflective() {
        let base = crate::material::material_for(BlockId::Stone);
        let wet = apply_wetness(base, SATURATED);
        assert!(wet.roughness < base.roughness, "wet stone is glossier");
        assert!(wet.reflectance > base.reflectance, "wet stone reflects more");
        assert_eq!(wet.metalness, base.metalness);
        // Dry input is a no-op, and wet darkens albedo.
        assert_eq!(apply_wetness(base, DRY), base);
        assert!(wet_albedo_factor(SATURATED) < 1.0 && wet_albedo_factor(DRY) == 1.0);
    }

    #[test]
    fn absorbency_is_exhaustive_and_sane() {
        for b in BlockId::ALL {
            let a = absorbency(b);
            // Air and lava never wet; everything else can hold at least a film.
            if matches!(b, BlockId::Air | BlockId::Lava) {
                assert_eq!(a, 0, "{b:?} should not wet");
            } else {
                assert!(a > 0, "{b:?} should take at least a film");
            }
        }
    }
}
