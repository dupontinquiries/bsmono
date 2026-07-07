//! Sparse world features — the rare, large set-pieces scattered across the map:
//! **gravity islands** that float and warp the pull around them, **volcanoes**
//! that feed heat and moisture into the climate engine, and great **arches**.
//!
//! Placed by the same determinism-hash scheme flora uses, but on a *coarse*
//! grid — one candidate per big cell, at low probability — so features are rare,
//! roughly one to a region, and every observer computes the same set for the
//! same world seed. A feature carries a per-kind strength: a gravity island's
//! anomaly intensity, a volcano's thermal output. The caller turns these into
//! the actual structures, gravity anomalies (`sim::gravity`), and heat sources.
//!
//! Pure, deterministic. Placement is on the (x, z) plane; `y` is a nominal band
//! per kind (islands ride high, volcanoes and arches sit near the surface) that
//! the worldgen clamps to real terrain.

/// Side of a coarse placement cell, in blocks. One candidate feature per cell.
pub const FEATURE_CELL: i32 = 256;
/// Fraction of cells that host a feature.
pub const FEATURE_DENSITY: f32 = 0.12;

/// The kinds of large feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKind {
    /// A floating mini-planet with a hollow, loot-bearing interior; warps gravity.
    GravityIsland,
    /// A heat/moisture source for the climate engine.
    Volcano,
    /// A great stone arch — a landmark (and a sheltered hollow behind it).
    Arch,
}

/// A placed feature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feature {
    pub kind: FeatureKind,
    pub center: [i32; 3],
    pub radius: i32,
    pub seed: u64,
    /// Kind-specific magnitude: gravity-island anomaly strength (≈0.5..1.5,
    /// >1 inverts), or a volcano's thermal output (heat units). 0 otherwise.
    pub strength: f32,
}

/// splitmix64 of a coarse cell + world seed (mirrors the engine's hashing).
fn cell_hash(cx: i32, cz: i32, seed: u64) -> u64 {
    let mut h = (cx as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (cz as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= seed.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Map the top bits of a hash to `[0,1)`.
fn unit(h: u64) -> f32 {
    ((h >> 40) as f32) / (1u32 << 24) as f32
}

/// Build the feature (if any) hosted by coarse cell `(cx, cz)`.
fn feature_at_cell(cx: i32, cz: i32, world_seed: u64) -> Option<Feature> {
    let h = cell_hash(cx, cz, world_seed);
    if unit(h) >= FEATURE_DENSITY {
        return None; // most cells are empty
    }
    // Decorrelated streams from the one hash.
    let hk = h.rotate_left(17);
    let hx = h.rotate_left(29);
    let hz = h.rotate_left(43);
    let hr = h.rotate_left(7);

    // Kind: arches common, volcanoes less, gravity islands rarest.
    let kind = match hk % 100 {
        0..=44 => FeatureKind::Arch,
        45..=79 => FeatureKind::Volcano,
        _ => FeatureKind::GravityIsland,
    };

    // Jitter the centre within the cell.
    let base_x = cx * FEATURE_CELL;
    let base_z = cz * FEATURE_CELL;
    let x = base_x + (hx % FEATURE_CELL as u64) as i32;
    let z = base_z + (hz % FEATURE_CELL as u64) as i32;

    let (y, radius, strength) = match kind {
        FeatureKind::GravityIsland => {
            let y = 140 + (hr % 100) as i32; // rides high in the sky
            let radius = 24 + (hr % 24) as i32;
            let strength = 0.6 + unit(hr) * 0.9; // 0.6..1.5 (>1 inverts)
            (y, radius, strength)
        }
        FeatureKind::Volcano => {
            let y = 56 + (hr % 24) as i32; // near the surface
            let radius = 36 + (hr % 40) as i32;
            let thermal = 300.0 + unit(hr) * 700.0; // heat units
            (y, radius, thermal)
        }
        FeatureKind::Arch => {
            let y = 60 + (hr % 16) as i32;
            let radius = 16 + (hr % 20) as i32;
            (y, radius, 0.0)
        }
    };

    Some(Feature { kind, center: [x, y, z], radius, seed: h, strength })
}

/// All features whose centre falls within the (x, z) rectangle
/// `[min, min + dims)`. Scans the coarse cells overlapping the region.
pub fn features_in(min: [i32; 2], dims: [i32; 2], world_seed: u64) -> Vec<Feature> {
    let max = [min[0] + dims[0], min[1] + dims[1]];
    // Coarse-cell range (one extra each side, since centres are jittered).
    let cx0 = min[0].div_euclid(FEATURE_CELL) - 1;
    let cx1 = max[0].div_euclid(FEATURE_CELL) + 1;
    let cz0 = min[1].div_euclid(FEATURE_CELL) - 1;
    let cz1 = max[1].div_euclid(FEATURE_CELL) + 1;
    let mut out = Vec::new();
    for cz in cz0..=cz1 {
        for cx in cx0..=cx1 {
            if let Some(f) = feature_at_cell(cx, cz, world_seed) {
                let (x, z) = (f.center[0], f.center[2]);
                if x >= min[0] && x < max[0] && z >= min[1] && z < max[1] {
                    out.push(f);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_is_deterministic() {
        let a = features_in([-1000, -1000], [4000, 4000], 42);
        let b = features_in([-1000, -1000], [4000, 4000], 42);
        assert_eq!(a, b, "same seed + region → identical features");
        assert_ne!(a, features_in([-1000, -1000], [4000, 4000], 43), "different seed differs");
    }

    #[test]
    fn density_is_roughly_as_configured() {
        // Scan a big region: ~FEATURE_DENSITY of coarse cells host a feature.
        let span = 40 * FEATURE_CELL; // 40×40 cells
        let feats = features_in([0, 0], [span, span], 7);
        let cells = 40 * 40;
        let frac = feats.len() as f32 / cells as f32;
        assert!((frac - FEATURE_DENSITY).abs() < 0.05, "density ~{FEATURE_DENSITY}, got {frac}");
    }

    #[test]
    fn features_lie_within_the_queried_region() {
        let feats = features_in([500, -300], [2000, 2000], 11);
        for f in &feats {
            assert!(f.center[0] >= 500 && f.center[0] < 2500);
            assert!(f.center[2] >= -300 && f.center[2] < 1700);
        }
    }

    #[test]
    fn all_kinds_appear_and_carry_sensible_strength() {
        let feats = features_in([-5000, -5000], [10000, 10000], 99);
        let mut saw_island = false;
        let mut saw_volcano = false;
        let mut saw_arch = false;
        for f in &feats {
            match f.kind {
                FeatureKind::GravityIsland => {
                    saw_island = true;
                    assert!(f.strength >= 0.6, "island has anomaly strength");
                    assert!(f.center[1] > 120, "islands ride high");
                }
                FeatureKind::Volcano => {
                    saw_volcano = true;
                    assert!(f.strength >= 300.0, "volcano emits heat");
                }
                FeatureKind::Arch => {
                    saw_arch = true;
                    assert_eq!(f.strength, 0.0);
                }
            }
        }
        assert!(saw_island && saw_volcano && saw_arch, "all three kinds occur over a large map");
    }
}
