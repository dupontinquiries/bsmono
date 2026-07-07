//! `sea_level` — sea-level / fill policy + coral-depth gate. **OPTIONAL.**
//!
//! Worker-03 / WORLD-PERF pure core, round 3. Given a terrain height and the
//! world sea level, decide whether a column is submerged and by how much, and
//! gate coral placement to a depth band. Composes with `orogeny` /
//! `bathymetry` (which produce the heights) and `aquatic_materials` (which
//! classifies the exposed seabed).
//!
//! Pure, deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Result of testing a column against sea level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeaLevelResult {
    pub submerged: bool,
    /// Water depth above the terrain (0 when not submerged).
    pub fill_depth: f32,
}

/// Test a terrain height against sea level. Submerged ⇔ height < sea level;
/// when submerged the fill depth is exactly `sea_level - terrain_height`.
pub fn classify(terrain_height: f32, sea_level: f32) -> SeaLevelResult {
    if terrain_height < sea_level {
        SeaLevelResult {
            submerged: true,
            fill_depth: sea_level - terrain_height,
        }
    } else {
        SeaLevelResult {
            submerged: false,
            fill_depth: 0.0,
        }
    }
}

/// Depth band in which coral may be placed (units = fill depth).
#[derive(Clone, Copy, Debug)]
pub struct CoralBand {
    pub min_depth: f32,
    pub max_depth: f32,
}

impl Default for CoralBand {
    fn default() -> Self {
        // Reef-building corals: shallow, sunlit band.
        CoralBand {
            min_depth: 0.5,
            max_depth: 12.0,
        }
    }
}

/// Whether coral may be placed at a given fill depth (within the band).
pub fn coral_allowed(fill_depth: f32, band: &CoralBand) -> bool {
    fill_depth >= band.min_depth && fill_depth <= band.max_depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_sea_level_is_filled() {
        let r = classify(10.0, 30.0);
        assert!(r.submerged);
        assert!(r.fill_depth > 0.0);
    }

    #[test]
    fn fill_depth_is_sea_level_minus_height() {
        for h in [-50.0, 0.0, 10.0, 29.9] {
            let sea = 30.0;
            let r = classify(h, sea);
            assert!(r.submerged);
            assert!((r.fill_depth - (sea - h)).abs() < 1e-6);
        }
    }

    #[test]
    fn at_or_above_sea_level_is_dry() {
        let r = classify(30.0, 30.0);
        assert!(!r.submerged);
        assert_eq!(r.fill_depth, 0.0);
        let r2 = classify(45.0, 30.0);
        assert!(!r2.submerged);
        assert_eq!(r2.fill_depth, 0.0);
    }

    #[test]
    fn coral_gate_respects_band() {
        let band = CoralBand::default();
        assert!(coral_allowed(5.0, &band));
        assert!(!coral_allowed(0.1, &band)); // too shallow
        assert!(!coral_allowed(50.0, &band)); // too deep
        // boundaries inclusive
        assert!(coral_allowed(band.min_depth, &band));
        assert!(coral_allowed(band.max_depth, &band));
    }

    #[test]
    fn prop_deterministic_and_consistent() {
        let mut h = -100.0f32;
        while h <= 100.0 {
            let sea = 25.0;
            let a = classify(h, sea);
            let b = classify(h, sea);
            assert_eq!(a, b);
            // submerged ⇔ fill_depth > 0
            assert_eq!(a.submerged, a.fill_depth > 0.0);
            // fill_depth never negative
            assert!(a.fill_depth >= 0.0);
            h += 1.0;
        }
    }
}
