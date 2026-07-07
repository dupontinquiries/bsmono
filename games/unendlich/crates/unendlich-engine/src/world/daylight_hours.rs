//! Daylight hours from season phase and latitude factor (0 equator, 1 pole).
//! Equator ~12h year-round; higher latitude swings more. In [0,24]. Pure.

pub fn hours(season_phase: f32, latitude_factor: f32) -> f32 {
    let sp = season_phase.clamp(-1.0, 1.0);
    let lat = latitude_factor.clamp(0.0, 1.0);
    (12.0 + sp * lat * 10.0).clamp(0.0, 24.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn equator_twelve() { assert!((hours(1.0, 0.0) - 12.0).abs() < 1e-6 && (hours(-1.0, 0.0) - 12.0).abs() < 1e-6); }
    #[test] fn summer_highlat_long() { assert!(hours(1.0, 0.9) > 12.0); }
    #[test] fn winter_highlat_short() { assert!(hours(-1.0, 0.9) < 12.0); }
    #[test] fn bounded() {
        for sp in [-1.0f32, -0.5, 0.0, 0.5, 1.0] { for lat in [0.0f32, 0.5, 1.0] { let h = hours(sp, lat); assert!((0.0..=24.0).contains(&h)); } }
    }
    #[test] fn equator_invariant() { assert_eq!(hours(1.0, 0.0), hours(-1.0, 0.0)); }
    #[test] fn deterministic() { assert_eq!(hours(0.5, 0.7).to_bits(), hours(0.5, 0.7).to_bits()); }
}
