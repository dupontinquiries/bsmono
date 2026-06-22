//! `glacier_carve` — valley deepening by glacial ice over time.
//!
//! Worker-03 / WORLD pure core, round 25. Returns how much a glacier deepens the
//! bed in one step: more ice carves deeper, harder rock resists, and the cut
//! scales with time. Slope mildly increases erosion (faster ice flow). No ice
//! means no carving. Pairs with `thermal_erosion` (R16) / `mineral_hardness`
//! (R10). Pure and deterministic.
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const MIN_HARD: f32 = 0.1; // hardness floor (avoid div-by-zero)
const SLOPE_GAIN: f32 = 0.5; // how much slope speeds carving
const CARVE_K: f32 = 1.0; // overall carve constant
const MAX_CARVE: f32 = 500.0; // per-step cap

/// Carve depth for `ice_thickness` over rock of `hardness` on a given `slope`,
/// over `dt`. Zero without ice; rises with ice and dt; falls with hardness;
/// capped.
pub fn carve_depth(ice_thickness: f32, slope: f32, hardness: f32, dt: f32) -> f32 {
    let ice = ice_thickness.max(0.0);
    let s = slope.max(0.0);
    let hard = hardness.max(MIN_HARD);
    let t = dt.max(0.0);
    let slope_term = 1.0 + s * SLOPE_GAIN; // ≥1, mild speed-up with slope
    let depth = CARVE_K * ice * slope_term / hard * t;
    if depth < 0.0 {
        0.0
    } else if depth > MAX_CARVE {
        MAX_CARVE
    } else {
        depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ice_no_carve() {
        assert_eq!(carve_depth(0.0, 1.0, 1.0, 1.0), 0.0);
    }

    #[test]
    fn thicker_carves_deeper() {
        assert!(carve_depth(20.0, 1.0, 1.0, 1.0) > carve_depth(5.0, 1.0, 1.0, 1.0));
    }

    #[test]
    fn harder_rock_carves_less() {
        assert!(carve_depth(10.0, 1.0, 0.5, 1.0) > carve_depth(10.0, 1.0, 5.0, 1.0));
    }

    #[test]
    fn dt_scales() {
        let a = carve_depth(5.0, 1.0, 2.0, 1.0);
        let b = carve_depth(5.0, 1.0, 2.0, 2.0);
        assert!(b > a);
        assert!((b - 2.0 * a).abs() < 1e-3); // linear in dt below the cap
    }

    #[test]
    fn bounded() {
        for ii in 0..30 {
            for hi in 1..10 {
                let v = carve_depth(ii as f32 * 50.0, 2.0, hi as f32 * 0.2, 5.0);
                assert!(v >= 0.0 && v <= MAX_CARVE);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(carve_depth(8.0, 1.5, 2.0, 1.0), carve_depth(8.0, 1.5, 2.0, 1.0));
    }
}
