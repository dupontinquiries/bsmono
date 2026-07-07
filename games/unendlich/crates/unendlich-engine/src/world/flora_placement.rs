//! `world::flora_placement` — WID r102-WG2-worldgen-plants.
//!
//! Biome-appropriate surface flora distribution: *where* a plant grows, how
//! thickly, and *which species* — with spatial clustering so flora clumps the
//! way it does in nature instead of scattering uniformly. Builds on the
//! existing scatter/lifecycle stack:
//!   - `world::vegetation_scatter::place` — the deterministic per-cell "does a
//!     plant land here?" roll (composed below; biome-agnostic, so reused as-is).
//!   - `world::biomes::Biome` — the biome the species mix is keyed to.
//! It feeds `world::plant_lifecycle`/`world::stem` downstream (this picks the
//! *what + where*; lifecycle ages it, stem grows its geometry).
//!
//! Note: `vegetation_scatter::biome_base` uses an older 5-id biome space, so the
//! biome→density and biome→species tables here are keyed to the real
//! `Biome` enum instead; only the (biome-agnostic) `place` roll is reused.
//!
//! Pure & deterministic: hash-based noise (no RNG), no clock, no IO, no
//! `unwrap`/`expect`/`panic` in non-test code. Same seed ⇒ same world.

use crate::world::biomes::Biome;
use crate::world::vegetation_scatter;

/// Surface flora species the placer can choose. Geometry/blueprints live in
/// `plant_blueprint`; this is just the placement taxonomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloraSpecies {
    Grass,
    Shrub,
    Flower,
    Fern,
    Reed,
    Mushroom,
    Cactus,
    Conifer,
    Broadleaf,
    Palm,
}

impl FloraSpecies {
    pub const ALL: [FloraSpecies; 10] = [
        FloraSpecies::Grass,
        FloraSpecies::Shrub,
        FloraSpecies::Flower,
        FloraSpecies::Fern,
        FloraSpecies::Reed,
        FloraSpecies::Mushroom,
        FloraSpecies::Cactus,
        FloraSpecies::Conifer,
        FloraSpecies::Broadleaf,
        FloraSpecies::Palm,
    ];
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

const N: usize = 10;

/// Per-cell inputs the world generator supplies (synthesized in the example).
#[derive(Clone, Copy, Debug)]
pub struct FloraCtx {
    pub biome: Biome,
    /// Local moisture 0..1.
    pub moisture: f32,
    /// Local temperature 0..1 (0 frigid .. 1 hot).
    pub temperature01: f32,
    /// Surface steepness 0..1 (1 = cliff; suppresses flora).
    pub slope: f32,
    /// Surface height (blocks).
    pub surface_y: i32,
    /// Sea level (blocks); flora never grows at/below it.
    pub sea_level: i32,
    /// Exposed bare rock / no soil → nothing roots here.
    pub is_rock: bool,
}

#[inline]
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

// ── biome → species base weights (0 = absent in that biome) ─────────────────
/// Relative abundance of each species in a biome (unnormalised; 0 ⇒ never).
pub fn species_weights(biome: Biome) -> [f32; N] {
    use FloraSpecies::*;
    let mut w = [0.0f32; N];
    let mut set = |s: FloraSpecies, v: f32| w[s.index()] = v;
    match biome {
        // r336-biomes: new-biome flora (FrozenOcean = no surface flora, ocean-flora intent).
        Biome::Mesa => { set(Cactus, 0.3); set(Shrub, 0.2); set(Grass, 0.05); }
        Biome::VolcanicWastes => {}
        Biome::FrozenOcean => {}
        Biome::Mangrove => { set(Broadleaf, 0.6); set(Fern, 0.4); set(Grass, 0.3); set(Shrub, 0.3); set(Mushroom, 0.2); }
        Biome::Plains => {
            set(Grass, 1.0);
            set(Flower, 0.3);
            set(Shrub, 0.2);
        }
        Biome::HighlandSteppe => {
            // r457: dry grassland — grass-dominant, sparse flowers, occasional shrub.
            set(Grass, 0.8);
            set(Flower, 0.15);
            set(Shrub, 0.15);
        }
        Biome::TemperateForest | Biome::BirchForest | Biome::MapleForest => {
            set(Broadleaf, 0.7);
            set(Fern, 0.5);
            set(Shrub, 0.4);
            set(Grass, 0.5);
            set(Flower, 0.2);
        }
        Biome::CloudForest => {
            set(Broadleaf, 0.6);
            set(Fern, 0.7);
            set(Flower, 0.3);
            set(Mushroom, 0.3);
        }
        Biome::TemperateHighlands | Biome::AlpineHighlands | Biome::MistyMountains => {
            set(Grass, 0.5);
            set(Shrub, 0.4);
            set(Conifer, 0.3);
        }
        Biome::Desert => {
            set(Cactus, 0.6);
            set(Shrub, 0.2);
            set(Grass, 0.1);
        }
        Biome::RedSandstoneCanyon => {
            set(Cactus, 0.4);
            set(Shrub, 0.3);
            set(Grass, 0.1);
        }
        Biome::BloomingCanyon => {
            set(Flower, 0.7);
            set(Grass, 0.5);
            set(Shrub, 0.3);
        }
        Biome::Beach => {
            set(Palm, 0.4);
            set(Grass, 0.3);
            set(Shrub, 0.1);
        }
        Biome::Swamp => {
            set(Reed, 0.8);
            set(Fern, 0.5);
            set(Mushroom, 0.4);
            set(Grass, 0.3);
        }
        Biome::Tundra => {
            set(Grass, 0.15);
            set(Shrub, 0.1);
        }
        Biome::Skylands => {
            set(Grass, 0.4);
            set(Flower, 0.3);
            set(Shrub, 0.2);
        }
        Biome::Savanna => {
            set(Grass, 0.9);
            set(Shrub, 0.3);
            set(Broadleaf, 0.1);
        }
        Biome::Jungle => {
            set(Broadleaf, 0.9);
            set(Fern, 0.8);
            set(Flower, 0.5);
            set(Palm, 0.4);
        }
        Biome::Taiga => {
            set(Conifer, 0.8);
            set(Shrub, 0.3);
            set(Grass, 0.3);
            set(Mushroom, 0.2);
        }
        // Ocean + all cave biomes: no surface flora here.
        Biome::Ocean
        | Biome::IceCave
        | Biome::FungalCave
        | Biome::ThermalCave
        | Biome::CrystalCave
        | Biome::LushCave => {}
    }
    w
}

/// Overall biome lushness (max placement probability before moisture/slope).
fn biome_density_base(biome: Biome) -> f32 {
    // r336-fix: single source of truth on `Biome` (covers the new biomes; Mangrove is lush).
    biome.flora_density()
}

/// How well a species fits the local climate, in `[0,1]` — breaks ties between
/// species the biome allows (e.g. cacti hate moisture, ferns love it).
fn species_env_fit(s: FloraSpecies, moisture: f32, temp: f32) -> f32 {
    use FloraSpecies::*;
    let m = clamp01(moisture);
    let t = clamp01(temp);
    match s {
        Cactus => clamp01(1.0 - m) * clamp01(0.4 + 0.6 * t),
        Reed | Mushroom => clamp01(0.2 + 0.8 * m),
        Fern => clamp01(0.3 + 0.7 * m),
        Broadleaf => clamp01(0.3 + 0.7 * m) * clamp01(0.3 + 0.7 * t),
        Conifer => clamp01(0.3 + 0.7 * (1.0 - t)) * clamp01(0.3 + 0.5 * m),
        Palm => clamp01(0.2 + 0.8 * t),
        Grass | Shrub | Flower => clamp01(0.55 + 0.45 * m),
    }
}

/// Placement probability `[0,1]` at a cell: biome lushness, wetter ⇒ denser,
/// steeper ⇒ sparser.
pub fn flora_density(ctx: &FloraCtx) -> f32 {
    let base = biome_density_base(ctx.biome);
    let m = clamp01(ctx.moisture);
    let s = clamp01(ctx.slope);
    clamp01(base * (0.4 + 0.6 * m) * (1.0 - 0.8 * s))
}

// ── clustering: low-frequency value noise so a species clumps ───────────────
const CLUSTER_CELLS: f32 = 9.0; // ~9 cells per cluster lobe

fn hash01(x: i32, z: i32, salt: u64) -> f32 {
    let mut h = salt ^ 0x9E37_79B9_7F4A_7C15;
    h ^= (x as u32 as u64).wrapping_mul(0x85EB_CA6B);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= (z as u32 as u64).wrapping_mul(0xC2B2_AE35);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Smooth value-noise cluster field in `[0,1]` for a species at a cell.
pub fn cluster_field(cellx: i32, cellz: i32, species_idx: usize, seed: u64) -> f32 {
    let salt = seed ^ (species_idx as u64).wrapping_mul(0xA24B_AED4_963E_E407);
    let fx = cellx as f32 / CLUSTER_CELLS;
    let fz = cellz as f32 / CLUSTER_CELLS;
    let x0 = fx.floor();
    let z0 = fz.floor();
    let tx = fx - x0;
    let tz = fz - z0;
    let (ix, iz) = (x0 as i32, z0 as i32);
    let v00 = hash01(ix, iz, salt);
    let v10 = hash01(ix + 1, iz, salt);
    let v01 = hash01(ix, iz + 1, salt);
    let v11 = hash01(ix + 1, iz + 1, salt);
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sz = tz * tz * (3.0 - 2.0 * tz);
    let a = v00 + (v10 - v00) * sx;
    let b = v01 + (v11 - v01) * sx;
    a + (b - a) * sz
}

/// Decide the flora (if any) at a world surface cell. `None` for ocean, cave
/// biomes, bare rock, underwater cells, or where the scatter roll/weights yield
/// nothing. Deterministic in `(cellx, cellz, seed)`.
pub fn place_at(cellx: i32, cellz: i32, ctx: &FloraCtx, seed: u64) -> Option<FloraSpecies> {
    // hard exclusions first
    if ctx.is_rock
        || ctx.biome == Biome::Ocean
        || ctx.biome.is_cave_biome()
        || ctx.surface_y <= ctx.sea_level
    {
        return None;
    }
    // does anything grow here? (reuse the shared deterministic scatter roll)
    let d = flora_density(ctx);
    if !vegetation_scatter::place(cellx, cellz, d, seed) {
        return None;
    }
    // which species? biome weight × climate fit × clustering.
    let base = species_weights(ctx.biome);
    let mut weights = [0.0f32; N];
    let mut sum = 0.0f32;
    for (i, &s) in FloraSpecies::ALL.iter().enumerate() {
        if base[i] <= 0.0 {
            continue;
        }
        let fit = species_env_fit(s, ctx.moisture, ctx.temperature01);
        let cluster = 0.25 + 0.75 * cluster_field(cellx, cellz, i, seed);
        let w = base[i] * fit * cluster;
        weights[i] = w;
        sum += w;
    }
    if sum <= 0.0 {
        return None;
    }
    // deterministic weighted pick from a second, independent hash.
    let r = hash01(cellx, cellz, seed ^ 0x5DEE_CE5E_F10A_2C17) * sum;
    let mut acc = 0.0f32;
    for (i, &s) in FloraSpecies::ALL.iter().enumerate() {
        acc += weights[i];
        if r < acc {
            return Some(s);
        }
    }
    // floating-point fallthrough: return the heaviest species.
    let mut best = 0usize;
    for i in 1..N {
        if weights[i] > weights[best] {
            best = i;
        }
    }
    Some(FloraSpecies::ALL[best])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(biome: Biome, moisture: f32, temp: f32) -> FloraCtx {
        FloraCtx {
            biome,
            moisture,
            temperature01: temp,
            slope: 0.0,
            surface_y: 80,
            sea_level: 62,
            is_rock: false,
        }
    }

    #[test]
    fn density_in_range_and_monotone() {
        for &b in &[
            Biome::Jungle, Biome::Desert, Biome::Plains, Biome::Tundra, Biome::Swamp,
            Biome::Ocean, Biome::FungalCave,
        ] {
            for mi in 0..=10 {
                for si in 0..=10 {
                    let mut c = ctx(b, mi as f32 / 10.0, 0.6);
                    c.slope = si as f32 / 10.0;
                    let d = flora_density(&c);
                    assert!((0.0..=1.0).contains(&d));
                }
            }
        }
        // wetter denser, steeper sparser
        assert!(flora_density(&ctx(Biome::Plains, 0.9, 0.5)) > flora_density(&ctx(Biome::Plains, 0.1, 0.5)));
        let mut steep = ctx(Biome::Plains, 0.6, 0.5);
        steep.slope = 0.9;
        assert!(flora_density(&ctx(Biome::Plains, 0.6, 0.5)) > flora_density(&steep));
        // ocean + caves never grow
        assert_eq!(flora_density(&ctx(Biome::Ocean, 1.0, 0.5)), 0.0);
        assert_eq!(flora_density(&ctx(Biome::FungalCave, 1.0, 0.5)), 0.0);
    }

    // collect placements over a region for a biome at favourable climate.
    fn sample(biome: Biome, moisture: f32, temp: f32, seed: u64, span: i32) -> (usize, [usize; N]) {
        let c = ctx(biome, moisture, temp);
        let mut placed = 0usize;
        let mut by = [0usize; N];
        for x in 0..span {
            for z in 0..span {
                if let Some(s) = place_at(x, z, &c, seed) {
                    placed += 1;
                    by[s.index()] += 1;
                }
            }
        }
        (placed, by)
    }

    #[test]
    fn right_species_in_right_biome() {
        // Every placed species must be one the biome actually allows.
        for &b in &[
            Biome::Plains, Biome::TemperateForest, Biome::Desert, Biome::Jungle,
            Biome::Taiga, Biome::Swamp, Biome::Savanna, Biome::Beach, Biome::Tundra,
        ] {
            let allowed = species_weights(b);
            let (placed, by) = sample(b, 0.6, 0.6, 11, 100);
            assert!(placed > 0, "{:?} placed nothing", b);
            for i in 0..N {
                if by[i] > 0 {
                    assert!(allowed[i] > 0.0, "{:?} placed disallowed {:?}", b, FloraSpecies::ALL[i]);
                }
            }
        }
        // signature species dominate where expected.
        let (_, desert) = sample(Biome::Desert, 0.15, 0.8, 5, 140);
        assert!(desert[FloraSpecies::Cactus.index()] > 0, "desert should grow cactus");
        // desert never grows palms/ferns/reeds
        assert_eq!(desert[FloraSpecies::Fern.index()], 0);
        assert_eq!(desert[FloraSpecies::Reed.index()], 0);

        let (_, taiga) = sample(Biome::Taiga, 0.5, 0.25, 5, 140);
        let conifer = taiga[FloraSpecies::Conifer.index()];
        assert!(conifer > 0, "taiga should grow conifers");
        assert!(conifer >= *taiga.iter().max().unwrap_or(&0), "conifer should dominate taiga");

        let (_, jungle) = sample(Biome::Jungle, 0.9, 0.85, 5, 140);
        assert!(jungle[FloraSpecies::Broadleaf.index()] > 0 && jungle[FloraSpecies::Fern.index()] > 0);
    }

    #[test]
    fn density_ordering_jungle_denser_than_desert() {
        let (jungle, _) = sample(Biome::Jungle, 0.9, 0.85, 7, 120);
        let (desert, _) = sample(Biome::Desert, 0.15, 0.8, 7, 120);
        let total = (120 * 120) as f32;
        let jf = jungle as f32 / total;
        let df = desert as f32 / total;
        assert!(jf > 0.3, "jungle too sparse: {jf}");
        assert!(df < 0.2, "desert too lush: {df}");
        assert!(jf > df * 3.0, "jungle should be much denser than desert ({jf} vs {df})");
    }

    #[test]
    fn nothing_in_ocean_rock_underwater_or_cave() {
        let span = 60;
        // ocean
        let oc = ctx(Biome::Ocean, 1.0, 0.5);
        // cave
        let cv = ctx(Biome::LushCave, 0.9, 0.5);
        // rock
        let mut rk = ctx(Biome::Plains, 0.8, 0.6);
        rk.is_rock = true;
        // underwater (surface at/below sea level)
        let mut uw = ctx(Biome::Plains, 0.8, 0.6);
        uw.surface_y = 60; // <= sea_level 62
        for x in 0..span {
            for z in 0..span {
                assert!(place_at(x, z, &oc, 3).is_none());
                assert!(place_at(x, z, &cv, 3).is_none());
                assert!(place_at(x, z, &rk, 3).is_none());
                assert!(place_at(x, z, &uw, 3).is_none());
            }
        }
    }

    #[test]
    fn seed_varying() {
        let c = ctx(Biome::Plains, 0.6, 0.6);
        let mut differ = false;
        for x in 0..400 {
            if place_at(x, 0, &c, 1) != place_at(x, 0, &c, 2) {
                differ = true;
                break;
            }
        }
        assert!(differ, "two seeds placed identically");
    }

    #[test]
    fn clustering_is_smooth_and_varied() {
        // adjacent cells have similar cluster values (clumping), but the field
        // spans a wide range across a region (not constant).
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        let mut max_adj = 0.0f32;
        for x in 0..80 {
            for z in 0..80 {
                let a = cluster_field(x, z, 2, 9);
                lo = lo.min(a);
                hi = hi.max(a);
                let b = cluster_field(x + 1, z, 2, 9);
                max_adj = max_adj.max((a - b).abs());
            }
        }
        assert!(hi - lo > 0.5, "cluster field too flat: {lo}..{hi}");
        assert!(max_adj < 0.35, "cluster field not smooth enough (adj jump {max_adj})");
    }

    #[test]
    fn deterministic() {
        let c = ctx(Biome::Jungle, 0.8, 0.8);
        assert_eq!(place_at(13, -4, &c, 77), place_at(13, -4, &c, 77));
        assert_eq!(cluster_field(5, 6, 3, 1), cluster_field(5, 6, 3, 1));
    }
}
