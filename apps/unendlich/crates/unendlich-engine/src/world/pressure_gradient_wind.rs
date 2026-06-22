//! Wind speed from a pressure gradient (geostrophic simplification): ∝
//! |pressure_difference| / distance. Larger gradient ⇒ stronger wind. Pure.

pub fn wind_speed(pressure_diff: f32, distance: f32, coefficient: f32) -> f32 {
    coefficient.max(0.0) * pressure_diff.abs() / distance.max(1e-4)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_gradient_calm() { assert_eq!(wind_speed(0.0, 100.0, 2.0), 0.0); }
    #[test] fn bigger_diff_more() { assert!(wind_speed(20.0, 100.0, 2.0) > wind_speed(5.0, 100.0, 2.0)); }
    #[test] fn farther_weaker() { assert!(wind_speed(20.0, 200.0, 2.0) < wind_speed(20.0, 50.0, 2.0)); }
    #[test] fn sign_agnostic() { assert_eq!(wind_speed(-20.0, 100.0, 2.0), wind_speed(20.0, 100.0, 2.0)); }
    #[test] fn nonneg() { assert!(wind_speed(20.0, 100.0, 2.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(wind_speed(20.0, 100.0, 2.0).to_bits(), wind_speed(20.0, 100.0, 2.0).to_bits()); }
}
