//! Tree density for a cell from moisture, temperature, and soil quality — peaks
//! in temperate-moist conditions (optimum t≈0.6). In [0,1]. Pure.

pub fn density(moisture: f32, temp_norm: f32, soil: f32) -> f32 {
    let m = moisture.clamp(0.0, 1.0);
    let t = temp_norm.clamp(0.0, 1.0);
    let s = soil.clamp(0.0, 1.0);
    let temp_fit = (-((t - 0.6) * (t - 0.6)) / 0.08).exp();
    (m * temp_fit * s).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dry_is_low() { assert_eq!(density(0.0, 0.6, 1.0), 0.0); }
    #[test] fn no_soil_zero() { assert_eq!(density(0.8, 0.6, 0.0), 0.0); }
    #[test] fn temperate_moist_high() { assert!(density(0.8, 0.6, 1.0) > 0.5); }
    #[test] fn extreme_temp_low() { assert!(density(0.8, 0.05, 1.0) < density(0.8, 0.6, 1.0)); }
    #[test] fn bounded() {
        let mut t = 0.0f32;
        while t <= 1.0 { let v = density(0.7, t, 0.8); assert!((0.0..=1.0).contains(&v)); t += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(density(0.8, 0.6, 1.0).to_bits(), density(0.8, 0.6, 1.0).to_bits()); }
}
