//! Haversine great-circle distance — ported from LocationTypes.hpp inline fn.
//!
//! Computes the shortest over-the-surface distance between two lat/lon points.
//! Used for "nearby" proximity checks in the UI.

use std::f64::consts::PI;

/// Earth's mean radius in kilometres.
const EARTH_RADIUS_KM: f64 = 6371.0;

/// Haversine great-circle distance between two points in kilometres.
/// Matches `haversineKm()` in LocationTypes.hpp exactly.
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1) * PI / 180.0;
    let d_lon = (lon2 - lon1) * PI / 180.0;
    let a = (d_lat / 2.0).sin().powi(2)
        + (lat1 * PI / 180.0).cos()
        * (lat2 * PI / 180.0).cos()
        * (d_lon / 2.0).sin().powi(2);
    EARTH_RADIUS_KM * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

/// True if two fixes are within `radius_km` of each other.
pub fn is_nearby(lat1: f64, lon1: f64, lat2: f64, lon2: f64, radius_km: f64) -> bool {
    distance_km(lat1, lon1, lat2, lon2) <= radius_km
}

/// Earth's mean radius in metres.
const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Destination point given a start, a bearing (degrees, 0 = north, clockwise),
/// and a distance in metres. The inverse of a bearing+distance computation —
/// used to project a peer's position forward along their heading. Standard
/// great-circle "direct geodesic" (spherical) formula.
pub fn destination(lat: f64, lon: f64, bearing_deg: f64, distance_m: f64) -> (f64, f64) {
    if distance_m <= 0.0 {
        return (lat, lon);
    }
    let ang = distance_m / EARTH_RADIUS_M; // angular distance in radians
    let brg = bearing_deg * PI / 180.0;
    let phi1 = lat * PI / 180.0;
    let lam1 = lon * PI / 180.0;

    let sin_phi2 = phi1.sin() * ang.cos() + phi1.cos() * ang.sin() * brg.cos();
    let phi2 = sin_phi2.asin();
    let y = brg.sin() * ang.sin() * phi1.cos();
    let x = ang.cos() - phi1.sin() * sin_phi2;
    let lam2 = lam1 + y.atan2(x);

    let lat2 = phi2 * 180.0 / PI;
    // normalise longitude to (-180, 180]
    let mut lon2 = lam2 * 180.0 / PI;
    while lon2 > 180.0 { lon2 -= 360.0; }
    while lon2 <= -180.0 { lon2 += 360.0; }
    (lat2, lon2)
}

#[cfg(test)]
mod dest_tests {
    use super::*;
    #[test]
    fn destination_round_trips_distance() {
        // Project 1000m east; the haversine distance back should be ~1000m.
        let (lat2, lon2) = destination(45.0, -73.0, 90.0, 1000.0);
        let d_km = distance_km(45.0, -73.0, lat2, lon2);
        assert!((d_km - 1.0).abs() < 0.01, "got {} km", d_km);
    }
    #[test]
    fn destination_north_increases_lat() {
        let (lat2, _lon2) = destination(45.0, -73.0, 0.0, 1000.0);
        assert!(lat2 > 45.0);
    }
    #[test]
    fn destination_zero_distance_is_identity() {
        assert_eq!(destination(45.0, -73.0, 123.0, 0.0), (45.0, -73.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_point_zero() {
        assert!(distance_km(48.8566, 2.3522, 48.8566, 2.3522) < 1e-9);
    }

    #[test]
    fn paris_to_london_approx() {
        // Paris (48.85, 2.35) to London (51.51, -0.13) ≈ 341 km
        let d = distance_km(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d - 341.0).abs() < 5.0, "got {}", d);
    }

    #[test]
    fn nearby_check() {
        // 1 km radius: same city block should be nearby
        assert!(is_nearby(48.8566, 2.3522, 48.8600, 2.3550, 1.0));
        // Cross-continent should not be
        assert!(!is_nearby(48.8566, 2.3522, 40.7128, -74.0060, 1.0));
    }
}
