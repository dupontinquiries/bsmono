//! `sim::ore_resource_veins` — deterministic ore / resource vein placement.
//!
//! A pure function of world position + seed (NO storage): common ores band high
//! (coal), mid bands carry copper/iron, and the rare-but-rich ores (gold, gems,
//! crystal) only appear deep. Veins are contiguous blobs (a smooth low-frequency
//! value-noise field thresholded per ore), not isolated specks, so digging finds
//! clusters. Biome can bias an ore (emerald favours Mountain) without breaking
//! the depth bands. Pure, deterministic, finite, panic-free (R1/R6/R11);
//! standalone `rustc --test`-able, release-safe.
//!
//! `biome` is taken as a plain `u8` (the id r78-01 produces) so the two threads
//! stay independent — we never import the biome module. The only biome id we act
//! on is [`BIOME_MOUNTAIN`] (documented assumption; emerald boost). Unknown ids
//! simply get no bias.

/// Ore kinds a voxel of solid stone may carry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ore {
    None,
    Coal,
    Copper,
    Iron,
    Gold,
    Redstone,
    Diamond,
    Emerald,
    Crystal,
}

/// A depth band an ore generates within, plus its base spawn fraction.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OreBand {
    pub ore: Ore,
    pub y_min: i32,
    pub y_max: i32,
    /// Approximate fraction of in-band solid voxels that become this ore.
    pub rarity01: f32,
}

/// Documented biome-id assumption (r78-01's Mountain). Emerald is boosted here.
pub const BIOME_MOUNTAIN: u8 = 5;
/// Emerald rarity multiplier inside Mountain biomes.
const EMERALD_MOUNTAIN_BOOST: f32 = 3.0;

/// Vein cell size (world blocks). Each cell either hosts a vein blob of a given
/// ore or doesn't; blobs are contiguous balls, so a hit's neighbours are hits.
const VEIN_CELL: i32 = 6;
/// Blob radius range (blocks) within a hosting cell.
const BLOB_R_MIN: f32 = 1.6;
const BLOB_R_MAX: f32 = 2.6;
/// Approximate fraction of a cell a blob fills (avg ball volume / cell volume);
/// used to convert a target voxel-rarity into a per-cell host probability.
const BLOB_FILL_EST: f32 = 0.176;

/// The depth bands, ordered RAREST-FIRST so that where ranges overlap the
/// rarer/deeper ore is reported (rather than being masked by common coal).
/// Tuned so the union ore fraction stays well under the 8% budget at every depth.
static BANDS: [OreBand; 8] = [
    OreBand { ore: Ore::Emerald, y_min: 0, y_max: 112, rarity01: 0.0040 },
    OreBand { ore: Ore::Diamond, y_min: 0, y_max: 16, rarity01: 0.0050 },
    OreBand { ore: Ore::Crystal, y_min: 0, y_max: 14, rarity01: 0.0060 },
    OreBand { ore: Ore::Gold, y_min: 0, y_max: 34, rarity01: 0.0080 },
    OreBand { ore: Ore::Redstone, y_min: 0, y_max: 24, rarity01: 0.0100 },
    OreBand { ore: Ore::Iron, y_min: 8, y_max: 64, rarity01: 0.0180 },
    OreBand { ore: Ore::Copper, y_min: 16, y_max: 80, rarity01: 0.0180 },
    OreBand { ore: Ore::Coal, y_min: 48, y_max: 128, rarity01: 0.0300 },
];

/// All ore bands (depth ranges + base rarities).
pub fn bands() -> &'static [OreBand] {
    &BANDS
}

/// A distinct salt per ore so each ore's vein field is independent.
fn ore_salt(ore: Ore) -> u64 {
    match ore {
        Ore::None => 0,
        Ore::Coal => 0x00C0_A1_5EED,
        Ore::Copper => 0x0C0_9_9E5_EED,
        Ore::Iron => 0x012_04_15_EED,
        Ore::Gold => 0x0_90_1D_5_EED,
        Ore::Redstone => 0x0_8ED_5_70_E5,
        Ore::Diamond => 0x0D1A_4_0_9D5,
        Ore::Emerald => 0x0E_3E_8A_1D5,
        Ore::Crystal => 0x0C_8_75_7A_15,
    }
}

// ─── deterministic hash ─────────────────────────────────────────────
fn hash3(seed: u64, xi: i64, yi: i64, zi: i64) -> f32 {
    // mix the lattice coordinate into the seed (splitmix64 finalizer)
    let mut h = seed
        ^ (xi as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (yi as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (zi as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    // top 24 bits → [0,1)
    (h >> 40) as f32 / ((1u64 << 24) as f32)
}

/// Effective rarity for an ore at a biome (applies biome bias). Clamped `[0,1]`.
fn effective_rarity(ore: Ore, base: f32, biome: u8) -> f32 {
    let mut r = base;
    if ore == Ore::Emerald && biome == BIOME_MOUNTAIN {
        r *= EMERALD_MOUNTAIN_BOOST;
    }
    r.max(0.0).min(1.0)
}

/// Vein membership: the ore (if any) at this world voxel. Deterministic in
/// `(seed, x, y, z, biome)`. Returns [`Ore::None`] for empty rock. An ore is
/// only ever returned within its band's `[y_min, y_max]`.
///
/// Model: the world is tiled into [`VEIN_CELL`]³ cells. Per ore, a cell hosts a
/// vein with probability `rarity / BLOB_FILL_EST`; a hosting cell carries a
/// contiguous ball (jittered centre, hashed radius), so neighbouring voxels of a
/// hit are also hits — veins cluster. Overall voxel fraction ≈ the band rarity.
pub fn ore_at(seed: u64, x: i32, y: i32, z: i32, biome: u8) -> Ore {
    // cell coordinates (floor division, correct for negatives)
    let cx = x.div_euclid(VEIN_CELL);
    let cy = y.div_euclid(VEIN_CELL);
    let cz = z.div_euclid(VEIN_CELL);
    let (cxi, cyi, czi) = (cx as i64, cy as i64, cz as i64);

    for band in BANDS.iter() {
        if y < band.y_min || y > band.y_max {
            continue;
        }
        let r = effective_rarity(band.ore, band.rarity01, biome);
        if r <= 0.0 {
            continue;
        }
        let salt = ore_salt(band.ore);
        // does this cell host a vein of this ore?
        let host = hash3(seed ^ salt, cxi, cyi, czi);
        let vein_prob = (r / BLOB_FILL_EST).min(1.0);
        if host >= vein_prob {
            continue;
        }
        // blob centre (jittered within the cell) + hashed radius
        let base_x = (cx * VEIN_CELL) as f32;
        let base_y = (cy * VEIN_CELL) as f32;
        let base_z = (cz * VEIN_CELL) as f32;
        let jitter = VEIN_CELL as f32;
        let ccx = base_x + hash3(seed ^ salt ^ 0xA1, cxi, cyi, czi) * jitter;
        let ccy = base_y + hash3(seed ^ salt ^ 0xB2, cxi, cyi, czi) * jitter;
        let ccz = base_z + hash3(seed ^ salt ^ 0xC3, cxi, cyi, czi) * jitter;
        let rad = BLOB_R_MIN + hash3(seed ^ salt ^ 0xD4, cxi, cyi, czi) * (BLOB_R_MAX - BLOB_R_MIN);
        let dx = x as f32 + 0.5 - ccx;
        let dy = y as f32 + 0.5 - ccy;
        let dz = z as f32 + 0.5 - ccz;
        if dx * dx + dy * dy + dz * dz <= rad * rad {
            return band.ore;
        }
    }
    Ore::None
}

/// Expected ore fraction for `ore` at depth `y` (for tuning / validation):
/// its base rarity inside the band, `0.0` outside. Biome-independent (the bias
/// lives in [`ore_at`]); finite and `>= 0`.
pub fn vein_density(ore: Ore, y: i32) -> f32 {
    for band in BANDS.iter() {
        if band.ore == ore {
            if y >= band.y_min && y <= band.y_max {
                return band.rarity01;
            }
            return 0.0;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u64 = 0x78_02_0; // documented seed (R20)

    #[test]
    fn deterministic() {
        for &(x, y, z) in &[(0, 10, 0), (123, 30, -77), (-5, 5, 9)] {
            assert_eq!(ore_at(SEED, x, y, z, 0), ore_at(SEED, x, y, z, 0));
        }
    }

    #[test]
    fn ore_only_within_its_band() {
        // sample a wide volume; every reported ore must be in its y-range
        for y in -8..160 {
            for x in 0..16 {
                for z in 0..16 {
                    let o = ore_at(SEED, x, y, z, 0);
                    if o == Ore::None {
                        continue;
                    }
                    let band = bands().iter().find(|b| b.ore == o).expect("ore has a band");
                    assert!(y >= band.y_min && y <= band.y_max, "{:?} out of band at y={}", o, y);
                }
            }
        }
    }

    #[test]
    fn shallow_common_deep_rare_ordering() {
        // bands place common ores high, rare ores deep
        let coal = bands().iter().find(|b| b.ore == Ore::Coal).unwrap();
        let diamond = bands().iter().find(|b| b.ore == Ore::Diamond).unwrap();
        // coal reaches high, diamond does not
        assert!(coal.y_max > diamond.y_max, "coal should band higher than diamond");
        // rare ores are rarer than common ones
        assert!(diamond.rarity01 < coal.rarity01, "diamond rarer than coal");
        assert!(bands().iter().find(|b| b.ore == Ore::Emerald).unwrap().rarity01 < coal.rarity01);
        // vein_density reflects the bands
        assert_eq!(vein_density(Ore::Coal, 100) > 0.0, true);
        assert_eq!(vein_density(Ore::Diamond, 100), 0.0, "no diamond near the surface");
        assert!(vein_density(Ore::Diamond, 5) > 0.0, "diamond appears deep");
    }

    #[test]
    fn overall_ore_fraction_within_budget() {
        // sample a representative underground volume
        let mut ore = 0u64;
        let mut total = 0u64;
        for y in 0..64 {
            for x in 0..40 {
                for z in 0..40 {
                    total += 1;
                    if ore_at(SEED, x, y, z, 0) != Ore::None {
                        ore += 1;
                    }
                }
            }
        }
        let frac = ore as f64 / total as f64;
        assert!(frac < 0.08, "ore fraction {} exceeds 8% budget", frac);
        assert!(frac > 0.003, "ore fraction {} suspiciously low (nothing to mine)", frac);
    }

    #[test]
    fn veins_are_contiguous_not_specks() {
        // for each ore hit, count same-ore voxels in its 3x3x3 neighbourhood
        // (including itself); a speck would average ~1, a vein averages > 1.
        let mut hits = 0u64;
        let mut cluster_sum = 0u64;
        for y in 2..50 {
            for x in 0..30 {
                for z in 0..30 {
                    let o = ore_at(SEED, x, y, z, 0);
                    if o == Ore::None {
                        continue;
                    }
                    hits += 1;
                    let mut same = 0u64;
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            for dz in -1..=1 {
                                if ore_at(SEED, x + dx, y + dy, z + dz, 0) == o {
                                    same += 1;
                                }
                            }
                        }
                    }
                    cluster_sum += same; // includes the centre (>=1)
                }
            }
        }
        assert!(hits > 50, "need a decent sample of hits, got {}", hits);
        let mean = cluster_sum as f64 / hits as f64;
        assert!(mean > 1.0, "veins look like isolated specks (mean cluster {})", mean);
    }

    #[test]
    fn biome_biases_emerald_without_breaking_bands() {
        // emerald should be more common in Mountain than in a plain biome,
        // sampled within emerald's band, and still never outside the band.
        let count = |biome: u8| -> u64 {
            let mut c = 0;
            for y in 0..100 {
                for x in 0..28 {
                    for z in 0..28 {
                        if ore_at(SEED, x, y, z, biome) == Ore::Emerald {
                            c += 1;
                        }
                    }
                }
            }
            c
        };
        let plain = count(0);
        let mountain = count(BIOME_MOUNTAIN);
        assert!(mountain > plain, "Mountain should yield more emerald ({} vs {})", mountain, plain);
        // band integrity under bias: emerald never appears above its y_max
        let eb = bands().iter().find(|b| b.ore == Ore::Emerald).unwrap();
        for y in (eb.y_max + 1)..(eb.y_max + 20) {
            for x in 0..16 {
                assert_ne!(ore_at(SEED, x, y, 0, BIOME_MOUNTAIN), Ore::Emerald, "emerald leaked above band");
            }
        }
    }

    #[test]
    fn finite_and_total_safe() {
        // vein_density finite & >= 0 everywhere; ore_at never panics on extremes
        for &o in &[Ore::Coal, Ore::Diamond, Ore::Emerald, Ore::None] {
            for y in [-1000, 0, 50, 100000] {
                let d = vein_density(o, y);
                assert!(d.is_finite() && d >= 0.0);
            }
        }
        let _ = ore_at(SEED, i32::MIN, i32::MAX, i32::MIN, 255);
        let _ = ore_at(0, 0, 0, 0, 0);
    }
}
