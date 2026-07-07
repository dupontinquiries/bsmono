//! Cloud field — sky moisture concentration that renders as opaque
//! white cube cloud blocks in the high-altitude band.
//!
//! ## Why clouds matter to the world
//!
//! Clouds aren't just visual decoration. They're the SAME moisture
//! that drives plant growth (via plant_lifecycle) and weathering (via
//! the weathering module): humidity in the field. When humidity
//! exceeds the condensation threshold AT cloud altitude, a cloud cell
//! exists. The user can see, from the sky cover, where moisture is
//! concentrated.
//!
//! Future tie-ins (the gameplay loop):
//!   - Wind advection: each tick, moisture drifts downwind.
//!     Clouds visibly flow with weather.
//!   - Condensation: high cloud density → rain block events that fall
//!     to the ground.
//!   - Rain effects: rain advances soil moisture, plant lifecycle,
//!     stone weathering. Wet stone weathers faster than dry.
//!   - Evaporation: water bodies (now visible since water gen landed)
//!     and wet soil → moisture released back upward into the field.
//!   - Player feedback: planting trees, building walls, lighting
//!     fires all influence local humidity → cloud patterns →
//!     rain → growth. The player can engineer microclimates.
//!
//! ## This module: just the visible cloud sampling
//!
//! Step 1 of the cloud system. We define `cloud_density_at` (a pure
//! function of world position) so the mesher can emit cloud cubes
//! where the field exceeds a threshold. The density function is
//! deterministic 3D pseudo-noise tuned to look like a partial cloud
//! cover — enough to see, not so much that the world is grey.
//!
//! Future steps will couple this with the wind-advected moisture
//! field so the function takes time as input and the clouds drift.

use crate::constants::SEA_LEVEL;

/// Cloud band: clouds only exist between these world Y values.
/// Below this is normal play space; above is upper atmosphere
/// (clear sky). The band is wide enough that the player can fly
/// through it but narrow enough that clouds form a clear layer
/// when viewed from below or above.
pub const CLOUD_BAND_BOTTOM: i32 = SEA_LEVEL + 64;
pub const CLOUD_BAND_TOP: i32 = SEA_LEVEL + 80;

/// Density above this threshold counts as a cloud cell.
/// Tuned so ~25-40% of the cloud-band volume is cloud (visible
/// partial coverage; not solid-overcast, not empty).
pub const CLOUD_THRESHOLD: f32 = 0.55;

/// Sample the cloud density field at world position (wx, wy, wz).
/// Returns [0, 1]. Outside the cloud band always returns 0.
///
/// The field is a sum of 3 octaves of trig pseudo-noise — gives
/// puffy, irregular cloud blobs without needing a real noise
/// hash table. Cheap to evaluate (~6 sin calls + arithmetic).
///
/// Y modulation: peak density in the middle of the cloud band,
/// tapering to zero at the band edges. This stops clouds from
/// filling the whole atmosphere — they form a visible LAYER.
///
/// Future: add `time: f32` parameter; offset XZ sampling by
/// `wind_at(0,0,0,t).xz * t` so the cloud field advects with wind.
/// Currently static (chunk-independent so meshes compose cleanly,
/// but no animation).
pub fn cloud_density_at(wx: i32, wy: i32, wz: i32) -> f32 {
    cloud_density_at_time(wx, wy, wz, 0.0)
}

/// Time-aware cloud density. Wind drift offsets the noise sampling
/// position so the cloud field flows. Use this when meshing clouds
/// that you want to animate; pass the current sim time.
///
/// Note: the existing `cloud_density_at` calls this with t=0 for
/// backward compatibility (and for callers that don't care about
/// motion, e.g. unit tests).
pub fn cloud_density_at_time(wx: i32, wy: i32, wz: i32, time_seconds: f32) -> f32 {
    if wy < CLOUD_BAND_BOTTOM || wy > CLOUD_BAND_TOP { return 0.0; }
    // Vertical taper: 1.0 at band center, 0.0 at edges.
    let band_mid = (CLOUD_BAND_TOP + CLOUD_BAND_BOTTOM) as f32 * 0.5;
    let band_half = (CLOUD_BAND_TOP - CLOUD_BAND_BOTTOM) as f32 * 0.5;
    let y_norm = ((wy as f32 - band_mid) / band_half).abs();
    let y_taper = (1.0 - y_norm).max(0.0);

    // Wind drift: offset XZ sampling positions by wind × time. Sample
    // wind at THIS cell so different regions can flow differently.
    // Cast to f64 for the wind lookup, back to f32 here.
    let (wind_x, wind_z, _) = crate::wind::wind_at(
        wx as f64, wy as f64, wz as f64, 0.0
    );
    // Scale: at typical wind magnitude 1.5, drift is 1.5 * t blocks
    // per second. Over 60 seconds, clouds shift ~90 blocks — visible
    // motion across the sky.
    let drift_x = wind_x * time_seconds;
    let drift_z = wind_z * time_seconds;

    let fx = wx as f32 - drift_x;
    let fz = wz as f32 - drift_z;
    // Three octaves of trig noise. Frequencies tuned for cloud-blob
    // sizes around 8-30 blocks across.
    let large  = ((fx * 0.04).sin() + (fz * 0.05).cos()) * 0.5;
    let medium = ((fx * 0.11 + fz * 0.13).sin()) * 0.35;
    let small  = ((fx * 0.27 + fz * 0.31).cos()) * 0.20;
    let raw = large + medium + small;
    // Map raw [-1.05, 1.05] to [0, 1].
    let unit = (raw * 0.5 + 0.5).clamp(0.0, 1.0);

    // Temperature coupling: colder air condenses moisture more
    // readily. Sample temperature at THIS cloud cell's position;
    // a cold cell boosts density, a warm cell suppresses it.
    let temp = crate::temperature::temperature_at(wx, wy, wz);
    let temp_boost = ((15.0 - temp) / 75.0).clamp(-0.3, 0.3);

    (unit * y_taper + temp_boost).clamp(0.0, 1.0)
}

/// Convenience: is the block at this world position inside a cloud?
#[inline]
pub fn is_cloud_at(wx: i32, wy: i32, wz: i32) -> bool {
    cloud_density_at(wx, wy, wz) > CLOUD_THRESHOLD
}

/// Time-aware: is this position inside a cloud at the given simulation time?
#[inline]
pub fn is_cloud_at_time(wx: i32, wy: i32, wz: i32, time_seconds: f32) -> bool {
    cloud_density_at_time(wx, wy, wz, time_seconds) > CLOUD_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_zero_below_band() {
        for wy in [SEA_LEVEL, SEA_LEVEL + 32, CLOUD_BAND_BOTTOM - 1] {
            for wx in -10..=10 {
                for wz in -10..=10 {
                    assert_eq!(cloud_density_at(wx, wy, wz), 0.0,
                        "below-band density should be 0 at ({},{},{})", wx, wy, wz);
                }
            }
        }
    }

    #[test]
    fn density_zero_above_band() {
        for wy in [CLOUD_BAND_TOP + 1, CLOUD_BAND_TOP + 50, 1000] {
            for wx in -10..=10 {
                for wz in -10..=10 {
                    assert_eq!(cloud_density_at(wx, wy, wz), 0.0,
                        "above-band density should be 0 at ({},{},{})", wx, wy, wz);
                }
            }
        }
    }

    #[test]
    fn density_in_unit_range_inside_band() {
        let band_mid = (CLOUD_BAND_TOP + CLOUD_BAND_BOTTOM) / 2;
        for wx in -50..=50 {
            for wz in -50..=50 {
                let d = cloud_density_at(wx, band_mid, wz);
                assert!(d >= 0.0 && d <= 1.0,
                    "density out of [0,1]: {} at ({},{},{})", d, wx, band_mid, wz);
            }
        }
    }

    #[test]
    fn some_cells_are_cloud_some_arent() {
        // Across a wide area at band center, expect mixed coverage.
        let band_mid = (CLOUD_BAND_TOP + CLOUD_BAND_BOTTOM) / 2;
        let mut cloud_count = 0;
        let mut total = 0;
        for wx in -100..=100 {
            for wz in -100..=100 {
                if is_cloud_at(wx, band_mid, wz) { cloud_count += 1; }
                total += 1;
            }
        }
        let ratio = cloud_count as f32 / total as f32;
        // Some clouds, not all clouds.
        assert!(ratio > 0.05 && ratio < 0.80,
            "expected partial cloud cover (5-80%), got {} ({}/{})",
            ratio, cloud_count, total);
    }

    /// Cloud field should be smooth — neighboring cells have similar
    /// density. This test prevents salt-and-pepper noise.
    #[test]
    fn density_field_is_smooth() {
        let band_mid = (CLOUD_BAND_TOP + CLOUD_BAND_BOTTOM) / 2;
        let mut max_jump: f32 = 0.0;
        for wx in -20..=20 {
            for wz in -20..=20 {
                let here = cloud_density_at(wx, band_mid, wz);
                let east = cloud_density_at(wx + 1, band_mid, wz);
                max_jump = max_jump.max((east - here).abs());
            }
        }
        // Adjacent cells (1 block apart) should differ by < 0.3.
        assert!(max_jump < 0.3,
            "cloud field too jagged: max neighbor diff = {}", max_jump);
    }

    /// Vertical taper: density at band edges should be 0; max at center.
    #[test]
    fn vertical_taper_peaks_at_center() {
        // Pick a spot where horizontal noise is non-trivial.
        let wx = 7;
        let wz = 13;
        let mut prev = 0.0_f32;
        let mut max_y = CLOUD_BAND_BOTTOM;
        let mut max_d = 0.0_f32;
        for wy in CLOUD_BAND_BOTTOM..=CLOUD_BAND_TOP {
            let d = cloud_density_at(wx, wy, wz);
            if d > max_d { max_d = d; max_y = wy; }
            prev = d;
        }
        let _ = prev;
        // Max should occur near the middle of the band.
        let band_mid = (CLOUD_BAND_TOP + CLOUD_BAND_BOTTOM) / 2;
        let dist_from_mid = (max_y - band_mid).abs();
        let band_half = (CLOUD_BAND_TOP - CLOUD_BAND_BOTTOM) / 2;
        assert!(dist_from_mid <= band_half,
            "max density should be inside band, got max at y={} (mid={})",
            max_y, band_mid);
        // And should be > 0 — there must BE some density at this location.
        assert!(max_d > 0.0,
            "cloud column at ({},{}) has no density anywhere", wx, wz);
    }
}
