//! Cloud-cover fraction from humidity, temperature, and altitude: high humidity +
//! cool air ⇒ more cloud; extreme altitude thins it. In [0,1]. Pure.

pub fn coverage(humidity: f32, temp_norm: f32, altitude_norm: f32) -> f32 {
    let h = humidity.clamp(0.0, 1.0);
    let t = temp_norm.clamp(0.0, 1.0);
    let alt = altitude_norm.clamp(0.0, 1.0);
    (h * (1.0 - 0.5 * t) * (1.0 - 0.4 * alt)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dry_is_clear() { assert_eq!(coverage(0.0, 0.5, 0.5), 0.0); }
    #[test] fn humid_cool_cloudy() { assert!((coverage(1.0, 0.0, 0.0) - 1.0).abs() < 1e-6); }
    #[test] fn warmer_less() { assert!(coverage(0.8, 0.9, 0.2) < coverage(0.8, 0.1, 0.2)); }
    #[test] fn altitude_thins() { assert!(coverage(0.8, 0.5, 0.9) < coverage(0.8, 0.5, 0.1)); }
    #[test] fn bounded() {
        let mut h = 0.0f32;
        while h <= 1.0 { assert!((0.0..=1.0).contains(&coverage(h, 0.5, 0.3))); h += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(coverage(0.8, 0.5, 0.3).to_bits(), coverage(0.8, 0.5, 0.3).to_bits()); }
}
