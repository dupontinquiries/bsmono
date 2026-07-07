//! Ground-cover (grass/shrub) density from moisture, temperature, and sunlight —
//! warm (opt ≈0.65), moist, and sunny ⇒ lush. Needs all three. In [0,1]. Pure.

pub fn density(moisture: f32, temp_norm: f32, sunlight: f32) -> f32 {
    let m = moisture.clamp(0.0, 1.0);
    let t = temp_norm.clamp(0.0, 1.0);
    let sun = sunlight.clamp(0.0, 1.0);
    let temp_fit = (-((t - 0.65) * (t - 0.65)) / 0.12).exp();
    (m * temp_fit * sun).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dry_barren() { assert_eq!(density(0.0, 0.65, 1.0), 0.0); }
    #[test] fn no_sun_barren() { assert_eq!(density(0.8, 0.65, 0.0), 0.0); }
    #[test] fn warm_moist_sunny_lush() { assert!(density(0.9, 0.65, 1.0) > 0.8); }
    #[test] fn cold_lower() { assert!(density(0.9, 0.05, 1.0) < density(0.9, 0.65, 1.0)); }
    #[test] fn bounded() {
        for t in [0.0f32, 0.3, 0.65, 1.0] { let v = density(0.7, t, 0.8); assert!((0.0..=1.0).contains(&v)); }
    }
    #[test] fn deterministic() { assert_eq!(density(0.9, 0.65, 1.0).to_bits(), density(0.9, 0.65, 1.0).to_bits()); }
}
