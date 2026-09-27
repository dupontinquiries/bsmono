//! CO₂ basins (`cave_biomes.docx`, "Deep CO₂ Basins"): lakes of invisible,
//! heavier-than-air gas in the low points of deep caves.
//!
//! The docx: dense gas settles into bowls and fills them to a surface that is
//! "perfectly flat and invisible", full concentration two blocks below it
//! (`gas_density = clamp(depth / 2, 0, 1)`), O₂ from 0 to 15% inside. The only
//! warnings are the silence, a torch that dies at the boundary, and a faint
//! ring of chemosynthetic microbes glowing at the surface level.
//!
//! Here a basin is a disk in plan (a centre and a radius per 64 x 64 region,
//! for a third of the regions) with a flat gas surface low in the world. The
//! gas is a pure function of position and seed, so the world generator (the
//! glowing ring), the light loop (torches that cannot burn) and the survival
//! tick (the player's breath) all agree without sharing state. Gas is only
//! breathed where there is air: inside rock it is never sampled.
//!
//! Simplifications, said plainly: the surface does not follow the real low
//! point of each cave (the docx's `min_height_in_radius`); a basin is where
//! the hash puts it, and a passage that leaves the disk leaves the gas at a
//! vertical boundary. The gas does not move or fill over time.
use super::noise::{hash_2d, noise_3d};

/// Basin grid cell, in blocks.
pub const REGION: i32 = 64;

/// Share of regions that hold a basin, in 1/1000.
pub const BASIN_PERMILLE: u32 = 330;

/// Lowest and highest gas surface (absolute y). The world's surface sits near
/// y 64 over bedrock at 0; basins live in the bottom caves.
pub const SURFACE_MIN: i32 = 12;
pub const SURFACE_MAX: i32 = 30;

/// Breathable-air O₂ fraction.
pub const O2_AIR: f32 = 0.209;

const SALT: u32 = 0xc0_2b_a5_17;

/// One basin: a disk in plan with a flat gas surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basin {
    pub center: [i32; 2],
    pub radius: i32,
    /// Gas fills cave air below this height (the surface cell itself is the
    /// top of the gradient).
    pub surface_y: i32,
}

/// The basin of region `(rx, rz)`, if it has one.
pub fn basin(rx: i32, rz: i32, seed: u32) -> Option<Basin> {
    let h = hash_2d(rx, rz, seed ^ SALT);
    if h % 1000 >= BASIN_PERMILLE {
        return None;
    }
    let h2 = hash_2d(rx, rz, (seed ^ SALT).wrapping_add(1));
    let radius = 14 + (h2 % 16) as i32; // 14..29, always inside the region
    let span = REGION - 2 * radius;
    let cx = rx * REGION + radius + ((h2 >> 8) % span.max(1) as u32) as i32;
    let cz = rz * REGION + radius + ((h2 >> 16) % span.max(1) as u32) as i32;
    let surface_y = SURFACE_MIN + ((h >> 12) % (SURFACE_MAX - SURFACE_MIN + 1) as u32) as i32;
    Some(Basin { center: [cx, cz], radius, surface_y })
}

/// The basin whose disk covers column `(x, z)`, if any.
pub fn basin_at(x: i32, z: i32, seed: u32) -> Option<Basin> {
    let b = basin(x.div_euclid(REGION), z.div_euclid(REGION), seed)?;
    let (dx, dz) = (x - b.center[0], z - b.center[1]);
    (dx * dx + dz * dz <= b.radius * b.radius).then_some(b)
}

/// CO₂ density (0 = clean air, 1 = the basin's full concentration) at the
/// centre of cell `(x, y, z)`: the docx's `clamp(depth / 2, 0, 1)` with depth
/// measured from the top of the surface cell.
pub fn co2_density(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    co2_density_at(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5, seed)
}

/// [`co2_density`] at a point (the player's head, a torch flame).
pub fn co2_density_at(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let Some(b) = basin_at(x.floor() as i32, z.floor() as i32, seed) else { return 0.0 };
    let depth = (b.surface_y as f32 + 1.0) - y;
    (depth / 2.0).clamp(0.0, 1.0)
}

/// O₂ fraction for a CO₂ density: the basin at full concentration leaves
/// under 1% (the docx range is 0-15%).
pub fn o2_for_density(d: f32) -> f32 {
    O2_AIR * (1.0 - 0.97 * d.clamp(0.0, 1.0))
}

/// The CO₂ density that leaves `o2` (the inverse of [`o2_for_density`]).
pub fn density_for_o2(o2: f32) -> f32 {
    ((1.0 - o2 / O2_AIR) / 0.97).clamp(0.0, 1.0)
}

/// What an earth bender feels through the rock: the nearest basin whose gas
/// is within `radius` blocks, and how far off it is (0 inside the gas).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sensed {
    pub basin: Basin,
    pub distance: f32,
}

/// The docx's "seismic sensing detects the gas pressure differential in the
/// rock pores": the nearest gas volume (a basin's disk below its surface)
/// within `radius` of `(x, y, z)`. It senses the gas whether or not a cave
/// passage reaches it, which is the point of sensing through rock.
pub fn sense(x: f32, y: f32, z: f32, seed: u32, radius: f32) -> Option<Sensed> {
    let reach = (radius / REGION as f32).ceil() as i32 + 1;
    let (rx0, rz0) = ((x.floor() as i32).div_euclid(REGION), (z.floor() as i32).div_euclid(REGION));
    let mut best: Option<Sensed> = None;
    for rx in rx0 - reach..=rx0 + reach {
        for rz in rz0 - reach..=rz0 + reach {
            let Some(b) = basin(rx, rz, seed) else { continue };
            // Distance to the gas volume: a cylinder of the disk's radius
            // whose top is the surface cell's top.
            let (dx, dz) = (x - (b.center[0] as f32 + 0.5), z - (b.center[1] as f32 + 0.5));
            let h = ((dx * dx + dz * dz).sqrt() - b.radius as f32).max(0.0);
            let v = (y - (b.surface_y as f32 + 1.0)).max(0.0);
            let distance = (h * h + v * v).sqrt();
            if distance <= radius && best.map_or(true, |s| distance < s.distance) {
                best = Some(Sensed { basin: b, distance });
            }
        }
    }
    best
}

/// Thin air in the geothermal band (`cave_biomes.docx`, Geothermal Deep:
/// "O₂: 5-18%. Chemosynthetic bacteria consume oxygen but also produce it in
/// some zones. O₂ is spatially variable — pockets of adequate O₂ adjacent to
/// depleted zones."). A slow 3D noise over the band below
/// `world::underground::GEOTHERMAL_Y`, eased back to open air over the band's
/// top three blocks. Not CO₂: it does not hush sound or cap the other bending
/// fields (see [`o2_air_at`] for the combined figure).
pub fn deep_o2(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let top = super::underground::GEOTHERMAL_Y as f32 + 1.0;
    if y >= top {
        return O2_AIR;
    }
    let n = noise_3d(x as f64 / 20.0, y as f64 / 14.0, z as f64 / 20.0, seed ^ DEEP_SALT) as f32;
    let t = ((n - 0.08) / 0.72).clamp(0.0, 1.0);
    let deep = DEEP_O2_MIN + (DEEP_O2_MAX - DEEP_O2_MIN) * t * t * (3.0 - 2.0 * t);
    // Ease in over the band's top three blocks.
    let k = ((top - y) / 3.0).clamp(0.0, 1.0);
    O2_AIR + (deep - O2_AIR) * k
}

/// The docx's geothermal O₂ range.
pub const DEEP_O2_MIN: f32 = 0.05;
pub const DEEP_O2_MAX: f32 = 0.18;
const DEEP_SALT: u32 = 0x6e07_4a11;

/// O₂ at a point from every cause: CO₂ basins and the geothermal band's thin
/// air. What breathing and fire use.
pub fn o2_air_at(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    o2_at(x, y, z, seed).min(deep_o2(x, y, z, seed))
}

/// O₂ at a point from CO₂ basins only.
pub fn o2_at(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    o2_for_density(co2_density_at(x, y, z, seed))
}

/// Is the cell `(x, y, z)` on the gas surface of a basin (where the docx's
/// microbial ring glows)?
pub fn on_surface(x: i32, y: i32, z: i32, seed: u32) -> bool {
    basin_at(x, z, seed).map_or(false, |b| b.surface_y == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u32 = 77;

    fn any_basin() -> Basin {
        for rx in 0..40 {
            for rz in 0..40 {
                if let Some(b) = basin(rx, rz, SEED) {
                    return b;
                }
            }
        }
        panic!("no basin in 1600 regions");
    }

    #[test]
    fn about_a_third_of_regions_hold_a_basin_inside_them() {
        let mut n = 0;
        for rx in -30..30 {
            for rz in -30..30 {
                if let Some(b) = basin(rx, rz, SEED) {
                    n += 1;
                    assert!((SURFACE_MIN..=SURFACE_MAX).contains(&b.surface_y));
                    // The whole disk is inside its region, so a column is in
                    // at most one basin.
                    assert!(b.center[0] - b.radius >= rx * REGION && b.center[0] + b.radius < (rx + 1) * REGION);
                    assert!(b.center[1] - b.radius >= rz * REGION && b.center[1] + b.radius < (rz + 1) * REGION);
                }
            }
        }
        let share = n as f32 / 3600.0;
        assert!((0.25..0.42).contains(&share), "basin share {share}");
    }

    #[test]
    fn the_gas_has_a_flat_surface_and_is_full_two_blocks_down() {
        let b = any_basin();
        let (x, z) = (b.center[0], b.center[1]);
        let s = b.surface_y;
        assert_eq!(co2_density(x, s + 1, z, SEED), 0.0, "clean air above the surface");
        assert!(co2_density(x, s, z, SEED) > 0.0 && co2_density(x, s, z, SEED) < 0.5, "the surface cell is the gradient");
        assert_eq!(co2_density(x, s - 2, z, SEED), 1.0, "full two blocks down");
        // Flat: the same at the disk's edge as at its centre.
        let edge = [x + b.radius - 1, z];
        assert_eq!(co2_density(edge[0], s - 1, edge[1], SEED), co2_density(x, s - 1, z, SEED));
        // And none outside the disk.
        assert_eq!(co2_density(x + b.radius + 1, s - 3, z, SEED), 0.0);
    }

    #[test]
    fn oxygen_spans_the_docx_range() {
        assert!((o2_for_density(0.0) - O2_AIR).abs() < 1e-6);
        assert!(o2_for_density(1.0) < 0.01, "a full basin is nearly airless");
        assert!(o2_for_density(0.5) < 0.16 && o2_for_density(0.5) > 0.08);
        // Monotone.
        let mut prev = 1.0;
        for k in 0..=10 {
            let o = o2_for_density(k as f32 / 10.0);
            assert!(o <= prev);
            prev = o;
        }
    }

    #[test]
    fn density_and_oxygen_invert_each_other() {
        for k in 0..=20 {
            let d = k as f32 / 20.0;
            assert!((density_for_o2(o2_for_density(d)) - d).abs() < 1e-5, "{d}");
        }
        assert_eq!(density_for_o2(0.25), 0.0, "richer than air is still no gas");
    }

    #[test]
    fn an_earth_bender_senses_a_basin_from_above_and_nearby_but_not_from_afar() {
        let b = any_basin();
        let (cx, cz) = (b.center[0] as f32 + 0.5, b.center[1] as f32 + 0.5);
        let top = b.surface_y as f32 + 1.0;
        // Inside the gas: distance 0.
        let s = sense(cx, top - 3.0, cz, SEED, 24.0).expect("inside");
        assert_eq!((s.basin, s.distance), (b, 0.0));
        // Ten blocks above the surface, over the centre.
        let s = sense(cx, top + 10.0, cz, SEED, 24.0).expect("above");
        assert!((s.distance - 10.0).abs() < 1e-4, "{}", s.distance);
        // Beside the disk at the gas's own height, 5 blocks out from its edge.
        let s = sense(cx + b.radius as f32 + 5.0, top - 1.0, cz, SEED, 24.0).expect("beside");
        assert!((s.distance - 5.0).abs() < 0.01, "{}", s.distance);
        // Out of range straight up.
        assert!(sense(cx, top + 30.0, cz, SEED, 24.0).is_none());
        // Every sensed distance agrees with the density: 0 exactly where the
        // gas is.
        for dy in -4..6 {
            let p = (cx + 3.0, top + dy as f32 - 0.5, cz);
            let s = sense(p.0, p.1, p.2, SEED, 24.0).unwrap();
            assert_eq!(s.distance == 0.0, co2_density_at(p.0, p.1, p.2, SEED) > 0.0, "dy {dy}");
        }
    }

    #[test]
    fn the_geothermal_band_has_thin_air_in_pockets_and_open_air_above() {
        let top = crate::world::underground::GEOTHERMAL_Y as f32 + 1.0;
        let (mut n, mut adequate, mut weak, mut lethal) = (0, 0, 0, 0);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        for x in (-400..400).step_by(9) {
            for z in (-400..400).step_by(11) {
                for y in [2.0f32, 8.0, 13.0] {
                    let o = deep_o2(x as f32, y, z as f32, SEED);
                    lo = lo.min(o);
                    hi = hi.max(o);
                    n += 1;
                    if o >= 0.16 {
                        adequate += 1;
                    } else if o >= 0.08 {
                        weak += 1;
                    } else {
                        lethal += 1;
                    }
                }
            }
        }
        let share = |k: i32| k as f32 / n as f32;
        eprintln!(
            "deep O2 {lo:.3}..{hi:.3}: adequate {:.2} weak {:.2} lethal {:.2}",
            share(adequate), share(weak), share(lethal)
        );
        assert!(lo >= DEEP_O2_MIN - 1e-6 && hi <= DEEP_O2_MAX + 1e-6, "{lo}..{hi} outside the docx 5-18%");
        assert!(hi - lo > 0.1, "barely varies: {lo}..{hi}");
        // Measured at this seed: adequate 0.29, weak 0.52, lethal 0.19.
        assert!((0.2..0.45).contains(&share(adequate)), "adequate {}", share(adequate));
        assert!((0.1..0.3).contains(&share(lethal)), "lethal {}", share(lethal));
        assert!((0.35..0.7).contains(&share(weak)), "weak {}", share(weak));
        // Above the band: plain air. The band's top eases in.
        assert_eq!(deep_o2(5.0, top, 5.0, SEED), O2_AIR);
        assert_eq!(deep_o2(5.0, 60.0, 5.0, SEED), O2_AIR);
        let near_top = deep_o2(5.0, top - 1.0, 5.0, SEED);
        let deeper = deep_o2(5.0, top - 4.0, 5.0, SEED);
        assert!((near_top - O2_AIR).abs() <= (deeper - O2_AIR).abs() + 1e-6);
        // Pockets are adjacent: from a lethal cell, adequate air is found
        // within 16 blocks somewhere.
        let mut found = 0;
        let mut tried = 0;
        'outer: for x in (-400..400).step_by(37) {
            for z in (-400..400).step_by(41) {
                if deep_o2(x as f32, 8.0, z as f32, SEED) >= 0.08 {
                    continue;
                }
                tried += 1;
                for dx in (-16..=16).step_by(4) {
                    for dz in (-16..=16).step_by(4) {
                        if deep_o2((x + dx) as f32, 8.0, (z + dz) as f32, SEED) >= 0.16 {
                            found += 1;
                            if tried >= 20 {
                                break 'outer;
                            }
                            continue 'outer;
                        }
                    }
                }
            }
        }
        assert!(tried >= 5, "only {tried} lethal cells sampled");
        assert!(found as f32 >= tried as f32 * 0.5, "{found}/{tried} lethal cells have good air nearby");
    }

    #[test]
    fn the_combined_air_is_the_worse_of_basin_and_band() {
        let b = any_basin();
        let (x, z) = (b.center[0] as f32 + 0.5, b.center[1] as f32 + 0.5);
        for y in [5.0f32, b.surface_y as f32 - 1.5, b.surface_y as f32 + 3.0, 40.0] {
            let all = o2_air_at(x, y, z, SEED);
            assert_eq!(all, o2_at(x, y, z, SEED).min(deep_o2(x, y, z, SEED)));
            assert!(all <= o2_at(x, y, z, SEED) && all <= deep_o2(x, y, z, SEED));
        }
        assert_eq!(o2_air_at(0.5, 70.0, 0.5, SEED), O2_AIR, "open air far above everything");
        // Outside every basin, deep in the band: the thin air is what counts.
        let (px, pz) = (-200..200)
            .step_by(13)
            .flat_map(|x| (-200..200).step_by(17).map(move |z| (x as f32 + 0.5, z as f32 + 0.5)))
            .find(|&(x, z)| o2_at(x, 8.5, z, SEED) == O2_AIR && deep_o2(x, 8.5, z, SEED) < 0.12)
            .expect("a thin pocket clear of basins");
        assert!(o2_air_at(px, 8.5, pz, SEED) < 0.12, "the band's thin air is part of the combined O2");
    }

    #[test]
    fn the_ring_is_the_surface_cell_inside_the_disk() {
        let b = any_basin();
        assert!(on_surface(b.center[0], b.surface_y, b.center[1], SEED));
        assert!(!on_surface(b.center[0], b.surface_y + 1, b.center[1], SEED));
        assert!(!on_surface(b.center[0] + b.radius + 2, b.surface_y, b.center[1], SEED));
    }
}
