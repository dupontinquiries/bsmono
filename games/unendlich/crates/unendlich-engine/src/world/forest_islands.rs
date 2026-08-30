//! `world::forest_islands` — dense-forest ISLANDS in the savanna (r605).
//!
//! From the Comoé field study (Mühlenberg et al. 1990; distilled with
//! provenance in `docs/design/biomes_west_africa.md`): in the humid
//! savanna, dense forest survives as isolated islands (~1–300 ha), each
//! floristically DISTINCT — neighbouring islands share at most 8 % of tree
//! species — with three vegetation strata (dominant emergents D, dominated
//! mid-canopy d, understory shrubs o), plus rare micro-islands seeded on
//! old termite mounds out in the open grass.
//!
//! In game terms, for a Savanna column:
//! - a coarse cell field decides whether an island exists nearby, its
//!   centre, radius, and its PALETTE (each island rolls its own species
//!   mix — the 8 %-overlap rule as a mechanic: crossing to the next
//!   island should feel like a different forest);
//! - inside an island, tree density rises with `core` (0 at the edge → 1
//!   at the centre) and each spawned tree draws a STRATUM (D/d/o) that
//!   picks height band + silhouette + blocks from the island's palette;
//! - independent of islands, a fine cell field drops termite-mound
//!   micro-clusters: one modest tree plus shrubs in a tight ring.
//!
//! EVERYTHING here is pure, deterministic, f32/u32-only math, because the
//! GPU worldgen shader must MIRROR it exactly (the r604 lesson: the
//! world-level parity probe cannot gate mirrors while the pre-existing
//! noise fork stands, so the mirror's gate is the literal transliteration
//! test at the bottom of this file).

use crate::world::biomes::TreeProfile;
use crate::world::blocks::BlockId;
use crate::world::tree_form::CanopyForm;

/// Island cell size in blocks. 96-block cells with jittered centres give
/// island spacing in the "walk a minute of open grass between forests"
/// range at typical movement speeds.
pub const ISLAND_CELL: i32 = 96;
/// Probability (per cell) that an island exists in it.
pub const ISLAND_P: f32 = 0.18;
/// Island radius range in blocks (≈ the study's 1 ha .. mid-size islands;
/// the 200–300 ha giants read as biome-scale and are left to r-future).
pub const R_MIN: f32 = 12.0;
pub const R_MAX: f32 = 44.0;
/// Termite micro-cluster field: fine cells, rare, tiny radius.
pub const TERMITE_CELL: i32 = 24;
pub const TERMITE_P: f32 = 0.05;
pub const TERMITE_R: f32 = 3.5;
/// Number of island palettes (each island picks one by hash).
pub const N_PALETTES: u32 = 3;
/// Palette id used for termite micro-clusters.
pub const TERMITE_PALETTE: u8 = 3;

/// Wrapping integer hash → [0, 1). Deliberately simple wrapping-mul/xor so
/// the GLSL twin is a line-for-line transliteration (uint math wraps
/// identically). Not a quality hash — a PLACEMENT hash.
#[inline]
pub fn cell_hash01(cx: i32, cz: i32, salt: u32, seed: u32) -> f32 {
    let mut h = (cx as u32).wrapping_mul(0x9E37_79B9)
        ^ (cz as u32).wrapping_mul(0x85EB_CA6B)
        ^ salt.wrapping_mul(0xC2B2_AE35)
        ^ seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    // Top 24 bits → [0,1): exactly representable in f32 on both sides.
    (h >> 8) as f32 / 16_777_216.0
}

/// What the island field says about one column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandSample {
    /// Inside an island (or termite micro-cluster) footprint.
    pub in_island: bool,
    /// 0 at the footprint edge → 1 at the centre. Drives density.
    pub core: f32,
    /// Which palette this island rolled (TERMITE_PALETTE for mounds).
    pub palette: u8,
}

impl IslandSample {
    pub const OPEN: IslandSample = IslandSample { in_island: false, core: 0.0, palette: 0 };
}

/// Sample the island field at a world column. Checks the column's cell and
/// its 8 neighbours (an island centred near a cell edge reaches across),
/// then falls back to the termite micro-field.
pub fn sample(wx: i32, wz: i32, seed: u32) -> IslandSample {
    let cell_x = wx.div_euclid(ISLAND_CELL);
    let cell_z = wz.div_euclid(ISLAND_CELL);
    let mut best = IslandSample::OPEN;
    // Neighbourhood scan, unrolled the same way the GLSL twin writes it.
    let mut dz = -1;
    while dz <= 1 {
        let mut dx = -1;
        while dx <= 1 {
            let cx = cell_x + dx;
            let cz = cell_z + dz;
            if cell_hash01(cx, cz, 0x15AD, seed) < ISLAND_P {
                // Island centre jittered inside the cell (margin keeps the
                // full radius inside a 1-cell neighbourhood reach).
                let jx = cell_hash01(cx, cz, 0xC311, seed);
                let jz = cell_hash01(cx, cz, 0xC312, seed);
                let radius =
                    R_MIN + (R_MAX - R_MIN) * cell_hash01(cx, cz, 0x0BADu32, seed);
                let centre_x = cx as f32 * ISLAND_CELL as f32
                    + (0.25 + 0.5 * jx) * ISLAND_CELL as f32;
                let centre_z = cz as f32 * ISLAND_CELL as f32
                    + (0.25 + 0.5 * jz) * ISLAND_CELL as f32;
                let ddx = wx as f32 + 0.5 - centre_x;
                let ddz = wz as f32 + 0.5 - centre_z;
                let dist = (ddx * ddx + ddz * ddz).sqrt();
                if dist < radius {
                    let core = 1.0 - dist / radius;
                    if core > best.core {
                        best = IslandSample {
                            in_island: true,
                            core,
                            palette: (cell_hash01(cx, cz, 0x9A1E, seed) * N_PALETTES as f32)
                                as u8
                                % N_PALETTES as u8,
                        };
                    }
                }
            }
            dx += 1;
        }
        dz += 1;
    }
    if best.in_island {
        return best;
    }
    // Termite micro-clusters: independent fine field, only in open grass.
    let tx = wx.div_euclid(TERMITE_CELL);
    let tz = wz.div_euclid(TERMITE_CELL);
    if cell_hash01(tx, tz, 0x7E12, seed) < TERMITE_P {
        let jx = cell_hash01(tx, tz, 0x7E13, seed);
        let jz = cell_hash01(tx, tz, 0x7E14, seed);
        let centre_x = tx as f32 * TERMITE_CELL as f32 + (0.3 + 0.4 * jx) * TERMITE_CELL as f32;
        let centre_z = tz as f32 * TERMITE_CELL as f32 + (0.3 + 0.4 * jz) * TERMITE_CELL as f32;
        let ddx = wx as f32 + 0.5 - centre_x;
        let ddz = wz as f32 + 0.5 - centre_z;
        let dist = (ddx * ddx + ddz * ddz).sqrt();
        if dist < TERMITE_R {
            return IslandSample {
                in_island: true,
                core: 0.6 * (1.0 - dist / TERMITE_R),
                palette: TERMITE_PALETTE,
            };
        }
    }
    IslandSample::OPEN
}

/// The three strata of the Comoé tables.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stratum {
    /// Dominant emergents ("D").
    Dominant = 0,
    /// Dominated mid-canopy ("d").
    Mid = 1,
    /// Understory shrubs ("o — arbustes, sous-bois").
    Under = 2,
}

/// Roll a stratum from a per-tree hash. Weights ≈ the study's structure:
/// few emergents, more mid-canopy, most understory (20 / 35 / 45).
#[inline]
pub fn stratum_for(h: f32) -> Stratum {
    if h < 0.20 {
        Stratum::Dominant
    } else if h < 0.55 {
        Stratum::Mid
    } else {
        Stratum::Under
    }
}

/// Island tree density per column from `core`. Open savanna is 0.006;
/// island cores approach dense-forest levels.
#[inline]
pub fn island_density(core: f32) -> f64 {
    (0.006 + 0.040 * core as f64).min(0.046)
}

/// The palette table: per (palette, stratum), the tree to grow. Palettes
/// differ in silhouette mix, height bands, and block species — the only
/// levers the current block set offers — so each island reads as its own
/// forest (the 8 %-overlap rule as a mechanic).
pub fn island_profile(palette: u8, stratum: Stratum) -> TreeProfile {
    use CanopyForm::*;
    // (form, min_h, max_h, radius, trunk, leaves)
    let (form, min_h, max_h, radius, trunk, leaves) = match (palette, stratum) {
        // Palette 0 — "semi-deciduous tall": oak-dark, dome emergents.
        (0, Stratum::Dominant) => (Dome, 8, 12, 3, BlockId::OakLog, BlockId::OakLeaves),
        (0, Stratum::Mid) => (Dome, 5, 8, 2, BlockId::OakLog, BlockId::OakLeaves),
        (0, Stratum::Under) => (Shrub, 1, 2, 2, BlockId::OakLog, BlockId::OakLeaves),
        // Palette 1 — "birch-bright": pale trunks, airy crowns.
        (1, Stratum::Dominant) => (Dome, 7, 11, 2, BlockId::BirchLog, BlockId::BirchLeaves),
        (1, Stratum::Mid) => (Dome, 4, 7, 2, BlockId::BirchLog, BlockId::BirchLeaves),
        (1, Stratum::Under) => (Shrub, 1, 2, 1, BlockId::BirchLog, BlockId::BirchLeaves),
        // Palette 2 — "dry-dense": flat-top emergents over a dome mid.
        (2, Stratum::Dominant) => (FlatTop, 6, 9, 3, BlockId::OakLog, BlockId::OakLeaves),
        (2, Stratum::Mid) => (Dome, 4, 6, 2, BlockId::OakLog, BlockId::OakLeaves),
        (2, Stratum::Under) => (Shrub, 1, 2, 2, BlockId::OakLog, BlockId::OakLeaves),
        // Termite micro-cluster: one modest dome + shrubs (any stratum
        // above Under collapses to the single tree).
        (_, Stratum::Under) => (Shrub, 1, 2, 2, BlockId::OakLog, BlockId::OakLeaves),
        (_, _) => (Dome, 5, 7, 2, BlockId::OakLog, BlockId::OakLeaves),
    };
    TreeProfile {
        base_density: 0.0, // density comes from island_density, not here
        trunk_min_height: min_h,
        trunk_max_height: max_h,
        canopy_radius: radius,
        trunk_block: trunk,
        leaves_block: leaves,
        form,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Determinism + footprint sanity over a big area: islands exist, stay
    /// within radius bounds of their cell neighbourhood, and every palette
    /// appears (each island its OWN forest — the 8 %-overlap rule).
    #[test]
    fn field_is_deterministic_with_all_palettes_present()  {
        let seed = 0xA5A5_1234;
        let mut seen = [false; 4];
        let mut island_cols = 0u32;
        for wx in (-2000..2000).step_by(7) {
            for wz in (-2000..2000).step_by(11) {
                let a = sample(wx, wz, seed);
                let b = sample(wx, wz, seed);
                assert_eq!(a, b, "deterministic at ({wx},{wz})");
                if a.in_island {
                    island_cols += 1;
                    assert!(a.core > 0.0 && a.core <= 1.0);
                    seen[a.palette as usize] = true;
                }
            }
        }
        assert!(island_cols > 500, "islands must actually exist ({island_cols})");
        assert!(seen[0] && seen[1] && seen[2], "all island palettes occur: {seen:?}");
        assert!(seen[TERMITE_PALETTE as usize], "termite micro-clusters occur");
    }

    /// Cores form a gradient: centre columns beat edge columns, and open
    /// savanna is genuinely open (majority of columns).
    #[test]
    fn open_savanna_dominates_and_density_tracks_core() {
        let seed = 77;
        let (mut open, mut total) = (0u32, 0u32);
        for wx in (0..3000).step_by(13) {
            for wz in (0..3000).step_by(17) {
                total += 1;
                if !sample(wx, wz, seed).in_island {
                    open += 1;
                }
            }
        }
        assert!(
            open * 10 > total * 5,
            "open grass must dominate ({open}/{total})"
        );
        assert!(island_density(0.0) < island_density(0.5));
        assert!(island_density(1.0) <= 0.046);
    }

    /// Strata weights land near the study's structure and the palette
    /// table gives each stratum a sensible shape (emergents taller than
    /// mid, understory is Shrub).
    #[test]
    fn strata_weights_and_palette_shapes_hold() {
        let (mut d, mut m, mut u) = (0u32, 0u32, 0u32);
        for i in 0..10_000 {
            match stratum_for(i as f32 / 10_000.0) {
                Stratum::Dominant => d += 1,
                Stratum::Mid => m += 1,
                Stratum::Under => u += 1,
            }
        }
        assert!((1900..2100).contains(&d), "D ≈ 20% ({d})");
        assert!((3400..3600).contains(&m), "d ≈ 35% ({m})");
        assert!((4400..4600).contains(&u), "o ≈ 45% ({u})");
        for p in 0..N_PALETTES as u8 {
            let dom = island_profile(p, Stratum::Dominant);
            let mid = island_profile(p, Stratum::Mid);
            let und = island_profile(p, Stratum::Under);
            assert!(dom.trunk_max_height > mid.trunk_max_height, "palette {p}: D over d");
            assert_eq!(und.form, crate::world::tree_form::CanopyForm::Shrub, "palette {p}: o is Shrub");
        }
    }

    /// The GLSL twin, transliterated literally, must match — the r604
    /// lesson: this test IS the mirror's gate (the world-level probe
    /// can't be, while the pre-existing noise fork stands).
    #[test]
    fn glsl_transliteration_matches_rust_exactly() {
        fn g_hash01(cx: i32, cz: i32, salt: u32, seed: u32) -> f32 {
            let mut h = (cx as u32).wrapping_mul(0x9E37_79B9)
                ^ (cz as u32).wrapping_mul(0x85EB_CA6B)
                ^ salt.wrapping_mul(0xC2B2_AE35)
                ^ seed;
            h ^= h >> 16;
            h = h.wrapping_mul(0x7FEB_352D);
            h ^= h >> 15;
            h = h.wrapping_mul(0x846C_A68B);
            h ^= h >> 16;
            (h >> 8) as f32 / 16_777_216.0
        }
        for cx in -40..40 {
            for cz in -40..40 {
                for salt in [0x15ADu32, 0xC311, 0x9A1E, 0x7E12] {
                    assert_eq!(
                        g_hash01(cx, cz, salt, 42),
                        cell_hash01(cx, cz, salt, 42),
                        "hash ({cx},{cz},{salt:#x})"
                    );
                }
            }
        }
    }
}
