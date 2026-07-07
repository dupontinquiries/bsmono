//! `giant_cave` — rare giant-cave structures.
//!
//! Worker-03 / WORLD-WORLDGEN pure core, round 6. A low-probability per-region
//! seeded gate decides whether a region hosts a giant cavern; when it does, a
//! deterministic ellipsoidal cavern (center + per-axis radii, clamped to a max)
//! is produced. Ties to `structure_place` / `structure_template` as the rare,
//! large end of the structure spectrum.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Cave-spawn tuning.
#[derive(Clone, Copy, Debug)]
pub struct GiantCaveParams {
    /// Spawn chance per region, in permille (e.g. 12 = 1.2%). Rare by design.
    pub spawn_chance_permille: u32,
    pub min_radius: u32,
    pub max_radius: u32,
}

impl Default for GiantCaveParams {
    fn default() -> Self {
        GiantCaveParams {
            spawn_chance_permille: 12,
            min_radius: 12,
            max_radius: 48,
        }
    }
}

/// A placed giant cavern: an ellipsoid with per-axis radii.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GiantCave {
    pub center: (i32, i32, i32),
    pub radius: (u32, u32, u32),
}

/// Decide whether a giant cave spawns in the region at `region` (region-grid
/// coords), and if so where/how big. `region_size` is the region edge in
/// blocks. Deterministic in `(region, seed)`.
pub fn cave_at(
    region: (i32, i32, i32),
    region_size: i32,
    seed: u64,
    p: &GiantCaveParams,
) -> Option<GiantCave> {
    let rs = if region_size > 0 { region_size } else { 1 };
    let base = region_hash(region, seed);

    // Rare gate.
    if (base % 1000) >= p.spawn_chance_permille as u64 {
        return None;
    }

    // Center: an offset within the region.
    let ox = (mix(base ^ 0xC0) % rs as u64) as i32;
    let oy = (mix(base ^ 0xC1) % rs as u64) as i32;
    let oz = (mix(base ^ 0xC2) % rs as u64) as i32;
    let center = (
        region.0 * rs + ox,
        region.1 * rs + oy,
        region.2 * rs + oz,
    );

    let radius = (
        clamp_radius(mix(base ^ 0xD0), p),
        clamp_radius(mix(base ^ 0xD1), p),
        clamp_radius(mix(base ^ 0xD2), p),
    );

    Some(GiantCave { center, radius })
}

/// Whether a world cell lies inside the cavern (ellipsoid test; rough shape).
pub fn contains(cave: &GiantCave, x: i32, y: i32, z: i32) -> bool {
    let dx = (x - cave.center.0) as f32;
    let dy = (y - cave.center.1) as f32;
    let dz = (z - cave.center.2) as f32;
    let rx = cave.radius.0.max(1) as f32;
    let ry = cave.radius.1.max(1) as f32;
    let rz = cave.radius.2.max(1) as f32;
    (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry) + (dz * dz) / (rz * rz) <= 1.0
}

fn clamp_radius(h: u64, p: &GiantCaveParams) -> u32 {
    let lo = p.min_radius;
    let hi = p.max_radius.max(p.min_radius);
    let span = (hi - lo).max(1);
    lo + (h % span as u64) as u32
}

fn region_hash(r: (i32, i32, i32), seed: u64) -> u64 {
    let a = mix(seed ^ (r.0 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let b = mix(a ^ (r.1 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    mix(b ^ (r.2 as u64).wrapping_mul(0x1656_67B1_9E37_79F9))
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_spawn_frequency_is_rare() {
        let p = GiantCaveParams::default(); // 12 permille
        let rs = 256;
        let mut spawns = 0u32;
        let total = 20_000u32;
        let mut i = 0u32;
        // Deterministic sweep over many regions.
        for rx in 0..200i32 {
            for ry in 0..100i32 {
                if i >= total {
                    break;
                }
                if cave_at((rx, 0, ry), rs, 0xBEEF, &p).is_some() {
                    spawns += 1;
                }
                i += 1;
            }
        }
        let rate = spawns as f32 / total as f32;
        // Within a loose band of the 1.2% target, and clearly rare.
        assert!(rate > 0.003 && rate < 0.03, "spawn rate {rate} outside rare band");
    }

    #[test]
    fn placement_deterministic() {
        let p = GiantCaveParams::default();
        let a = cave_at((3, 1, 7), 256, 42, &p);
        let b = cave_at((3, 1, 7), 256, 42, &p);
        assert_eq!(a, b);
    }

    #[test]
    fn radii_clamped_to_bounds() {
        let p = GiantCaveParams::default();
        let mut checked = 0;
        for rx in 0..2000i32 {
            if let Some(c) = cave_at((rx, 0, 0), 256, 7, &p) {
                for r in [c.radius.0, c.radius.1, c.radius.2] {
                    assert!(r >= p.min_radius && r <= p.max_radius, "radius {r} out of bounds");
                }
                checked += 1;
            }
        }
        assert!(checked > 0, "no caves spawned to check");
    }

    #[test]
    fn contains_center_not_far() {
        let cave = GiantCave { center: (100, 50, 100), radius: (20, 10, 20) };
        assert!(contains(&cave, 100, 50, 100)); // center
        assert!(contains(&cave, 110, 50, 100)); // within x-radius
        assert!(!contains(&cave, 100, 70, 100)); // beyond y-radius
        assert!(!contains(&cave, 200, 200, 200)); // far away
    }

    #[test]
    fn zero_chance_never_spawns() {
        let p = GiantCaveParams { spawn_chance_permille: 0, ..GiantCaveParams::default() };
        for rx in 0..5000i32 {
            assert!(cave_at((rx, 0, 0), 256, 1, &p).is_none());
        }
    }
}
