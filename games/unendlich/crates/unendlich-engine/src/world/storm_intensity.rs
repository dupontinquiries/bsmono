//! Convective storm intensity [0,1] from humidity and instability: both required
//! (product) — moist, unstable air makes strong storms. Pure, deterministic.

pub fn intensity(humidity: f32, instability: f32) -> f32 {
    (humidity.clamp(0.0, 1.0) * instability.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dry_calm() { assert_eq!(intensity(0.0, 0.9), 0.0); }
    #[test] fn stable_calm() { assert_eq!(intensity(0.9, 0.0), 0.0); }
    #[test] fn moist_unstable_strong() { assert!(intensity(0.9, 0.9) > 0.7); }
    #[test] fn both_max_one() { assert_eq!(intensity(1.0, 1.0), 1.0); }
    #[test] fn monotone() { assert!(intensity(0.8, 0.8) > intensity(0.3, 0.3)); }
    #[test] fn deterministic() { assert_eq!(intensity(0.7, 0.6).to_bits(), intensity(0.7, 0.6).to_bits()); }
}
