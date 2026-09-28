//! Cave air temperature (`cave_biomes.docx`, the cave-type table and each
//! biome's Survival Hazards): underground the surface climate stops mattering.
//! Caves hold a steady temperature of their own, "no seasonal variation
//! underground", and the deep geothermal band is hot enough to kill.
//!
//! | zone (world::underground) | docx biome            | °C      |
//! |---------------------------|-----------------------|---------|
//! | Limestone                 | Twilight Limestone    | 8-15    |
//! | Crystal                   | Crystal Chambers      | 15-40   |
//! | Abyssal                   | Abyssal Chasms        | 12-25   |
//! | Geothermal                | Geothermal Deep       | 30-80   |
//!
//! Inside each range the value moves slowly across the map (limestone,
//! abyssal) or with depth (crystal chambers warm toward their floor; the
//! geothermal band heats toward bedrock). What a player feels blends from the
//! surface climate into the cave's as they go deeper and as the sky closes
//! over them, so a cave mouth or a ravine floor under open sky still reads as
//! the weather outside.
//!
//! Pure: position, the surface height and the seed in; °C out. Not modelled:
//! the docx's steam caverns (40-120 °C, no zone here yet), the thin air of the
//! geothermal deep (its O₂ 5-18%), and CO₂ basins' own 10-20 °C.
use super::noise::noise_2d;
use super::underground::{zone_of, Zone, GEOTHERMAL_Y, SOIL_DEPTH};

/// The docx range for a zone, in °C (`None` for soil: that is the surface).
pub fn range_c(zone: Zone) -> Option<(f32, f32)> {
    match zone {
        Zone::Soil => None,
        Zone::Limestone => Some((8.0, 15.0)),
        Zone::Crystal => Some((15.0, 40.0)),
        Zone::Abyssal => Some((12.0, 25.0)),
        Zone::Geothermal => Some((30.0, 80.0)),
    }
}

/// Depth band of the crystal zone (world::underground's ZONE_BANDS).
const CRYSTAL_TOP: f32 = 26.0;
const CRYSTAL_FLOOR: f32 = 70.0;

const SALT: u32 = 0x0c1f_a7e5;

/// Steady cave-air temperature (°C) at `(x, y, z)`, `depth` blocks below the
/// surface, or `None` above the soil line.
pub fn cave_c(x: i32, y: i32, z: i32, depth: i32, seed: u32) -> Option<f32> {
    let zone = zone_of(x, y, z, depth, seed);
    let (lo, hi) = range_c(zone)?;
    // Slow drift across the map: whole cave systems run warm or cool.
    let drift = noise_2d(x as f64 / 97.0, z as f64 / 97.0, seed ^ SALT) as f32;
    let t = match zone {
        Zone::Soil => return None,
        Zone::Limestone | Zone::Abyssal => drift,
        Zone::Crystal => {
            let d = ((depth as f32 - CRYSTAL_TOP) / (CRYSTAL_FLOOR - CRYSTAL_TOP)).clamp(0.0, 1.0);
            0.8 * d + 0.2 * drift
        }
        Zone::Geothermal => {
            let d = ((GEOTHERMAL_Y - y) as f32 / GEOTHERMAL_Y as f32).clamp(0.0, 1.0);
            0.9 * d + 0.1 * drift
        }
    };
    Some(lo + (hi - lo) * t.clamp(0.0, 1.0))
}

/// How much of the air is cave air rather than weather, 0..1: nothing above
/// the soil line, all of it from 16 blocks down, and less where the sky is
/// open overhead (`sky01` = the light volume's sky level, 0..1).
pub fn enclosure(depth: f32, sky01: f32) -> f32 {
    let t = ((depth - SOIL_DEPTH as f32) / (16.0 - SOIL_DEPTH as f32)).clamp(0.0, 1.0);
    let by_depth = t * t * (3.0 - 2.0 * t);
    by_depth * (1.0 - sky01.clamp(0.0, 1.0))
}

/// The air temperature a player feels before wind, wet, fire and shelter:
/// the surface climate blended into the cave's.
pub fn ambient_c(surface_c: f32, cave_c: Option<f32>, depth: f32, sky01: f32) -> f32 {
    match cave_c {
        Some(c) => surface_c + (c - surface_c) * enclosure(depth, sky01),
        None => surface_c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: u32 = 4242;

    /// Sample a zone's temperatures over a patch of map at a fixed depth/y.
    fn sample(depth: i32, y: i32) -> (Zone, Vec<f32>) {
        let mut out = Vec::new();
        let mut zone = None;
        for x in (-600..600).step_by(37) {
            for z in (-600..600).step_by(41) {
                let zn = zone_of(x, y, z, depth, SEED);
                if zone.is_none() {
                    zone = Some(zn);
                }
                if Some(zn) == zone {
                    out.push(cave_c(x, y, z, depth, SEED).expect("below soil"));
                }
            }
        }
        (zone.unwrap(), out)
    }

    #[test]
    fn every_zone_stays_inside_its_docx_range_and_uses_it() {
        // (depth, y) picked well inside each band, the docx range written out
        // here (not read back from range_c, which would test itself), and the
        // share of the range the map drift owns there (the rest follows
        // depth; see the next test).
        for (depth, y, want, (lo, hi), drift_share) in [
            (12, 50, Zone::Limestone, (8.0, 15.0), 1.0),
            (48, 40, Zone::Crystal, (15.0, 40.0), 0.2),
            (90, 30, Zone::Abyssal, (12.0, 25.0), 1.0),
            (60, 9, Zone::Geothermal, (30.0, 80.0), 0.1),
        ] {
            let (zone, ts) = sample(depth, y);
            assert_eq!(zone, want);
            assert_eq!(range_c(zone), Some((lo, hi)), "{zone:?} range drifted from the docx");
            assert!(ts.len() > 200, "{want:?}: only {} samples", ts.len());
            let (mn, mx) = ts.iter().fold((f32::MAX, f32::MIN), |(a, b), &t| (a.min(t), b.max(t)));
            assert!(mn >= lo && mx <= hi, "{zone:?} {mn}..{mx} outside {lo}..{hi}");
            // Not a constant: the map drift uses most of its share.
            assert!(mx - mn > (hi - lo) * drift_share * 0.5, "{zone:?} barely varies: {mn}..{mx}");
        }
    }

    #[test]
    fn crystal_chambers_warm_downward_and_the_deep_heats_toward_bedrock() {
        let (x, z) = (123, -77);
        let c = |depth| cave_c(x, 40, z, depth, SEED).unwrap();
        assert_eq!(zone_of(x, 40, z, 30, SEED), Zone::Crystal);
        assert!(c(66) > c(30) + 12.0, "crystal {} -> {}", c(30), c(66));
        let g = |y| cave_c(x, y, z, 80, SEED).unwrap();
        assert!(g(1) > 70.0 && g(GEOTHERMAL_Y) < 40.0, "geothermal {} at top, {} at bedrock", g(GEOTHERMAL_Y), g(1));
        assert!(g(1) > g(9) && g(9) > g(17));
    }

    #[test]
    fn soil_is_the_surface_and_depth_and_open_sky_blend_between_them() {
        assert_eq!(cave_c(0, 60, 0, SOIL_DEPTH - 1, SEED), None);
        assert_eq!(ambient_c(30.0, None, 50.0, 0.0), 30.0);
        // A 10 °C cave under a 30 °C day.
        assert_eq!(ambient_c(30.0, Some(10.0), SOIL_DEPTH as f32, 0.0), 30.0, "at the soil line: weather");
        assert_eq!(ambient_c(30.0, Some(10.0), 16.0, 0.0), 10.0, "16 down, roofed: cave air");
        let mid = ambient_c(30.0, Some(10.0), 10.0, 0.0);
        assert!(mid < 30.0 && mid > 10.0, "{mid}");
        // A ravine floor 40 down under open sky is still the weather.
        assert_eq!(ambient_c(30.0, Some(10.0), 40.0, 1.0), 30.0);
        assert!(ambient_c(30.0, Some(10.0), 40.0, 0.5) > 15.0);
        // Monotone in depth.
        let mut prev = 31.0;
        for d in 0..30 {
            let t = ambient_c(30.0, Some(10.0), d as f32, 0.0);
            assert!(t <= prev);
            prev = t;
        }
    }
}
