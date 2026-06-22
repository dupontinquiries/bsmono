//! `biome_temp_map` — classify a biome from temperature + moisture.
//!
//! Worker-03 / WORLD pure core, round 18. A Whittaker-style 3×3 lookup: split
//! temperature and moisture each into cold/temperate/hot and dry/mid/wet, giving
//! nine biome ids. Boundaries are at the thirds. Feeds `biome_blend` (R8) and
//! `biome_transition` (R12), which smooth across the seams these ids define.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Biome id 0..=8 from `temp` and `moisture` (both ≈[0,1]). Row = temperature
/// band (cold/temperate/hot), column = moisture band (dry/mid/wet):
/// `id = temp_band*3 + moisture_band`.
pub fn biome(temp: f32, moisture: f32) -> u8 {
    let t = band3(clamp01(temp));
    let m = band3(clamp01(moisture));
    (t * 3 + m) as u8
}

fn band3(v: f32) -> u32 {
    if v < 1.0 / 3.0 {
        0
    } else if v < 2.0 / 3.0 {
        1
    } else {
        2
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_dry_differs_from_cold_wet() {
        assert_ne!(biome(0.9, 0.1), biome(0.1, 0.9));
    }

    #[test]
    fn boundaries_map() {
        assert_eq!(biome(0.0, 0.0), 0); // cold dry
        assert_eq!(biome(0.34, 0.0), 3); // temperate dry (just past 1/3)
        assert_eq!(biome(0.7, 0.0), 6); // hot dry (just past 2/3)
        assert_eq!(biome(0.0, 0.34), 1); // cold mid
        assert_eq!(biome(0.0, 0.7), 2); // cold wet
    }

    #[test]
    fn id_bounded() {
        for ti in 0..=20 {
            for mi in 0..=20 {
                let id = biome(ti as f32 / 20.0, mi as f32 / 20.0);
                assert!(id <= 8);
            }
        }
    }

    #[test]
    fn corners_distinct() {
        let c00 = biome(0.0, 0.0);
        let c10 = biome(1.0, 0.0);
        let c01 = biome(0.0, 1.0);
        let c11 = biome(1.0, 1.0);
        let mut set = [c00, c10, c01, c11];
        set.sort();
        // all four corners are different biomes
        assert_eq!(set, [0, 2, 6, 8]);
    }

    #[test]
    fn stable_within_a_cell() {
        // Two points in the same band cell map to the same biome.
        assert_eq!(biome(0.1, 0.1), biome(0.2, 0.2));
    }

    #[test]
    fn deterministic() {
        assert_eq!(biome(0.55, 0.45), biome(0.55, 0.45));
    }
}
