//! Wind force on a cloth patch: ∝ how much it faces the wind (|normal·wind_dir|)
//! × wind strength × area. Edge-on patches catch little. Pure, deterministic.

pub fn force(normal_dot_wind: f32, wind_strength: f32, area: f32) -> f32 {
    let facing = normal_dot_wind.abs().clamp(0.0, 1.0);
    facing * wind_strength.max(0.0) * area.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn edge_on_no_force() { assert_eq!(force(0.0, 5.0, 2.0), 0.0); }
    #[test] fn face_on_full() { assert!((force(1.0, 5.0, 2.0) - 10.0).abs() < 1e-6); }
    #[test] fn stronger_wind_more() { assert!(force(1.0, 10.0, 2.0) > force(1.0, 3.0, 2.0)); }
    #[test] fn bigger_area_more() { assert!(force(1.0, 5.0, 4.0) > force(1.0, 5.0, 1.0)); }
    #[test] fn back_face_same() { assert_eq!(force(-1.0, 5.0, 2.0), force(1.0, 5.0, 2.0)); }
    #[test] fn deterministic() { assert_eq!(force(0.7, 5.0, 2.0).to_bits(), force(0.7, 5.0, 2.0).to_bits()); }
}
