//! Particle size over its life: lerp from birth_size to death_size by
//! age/lifetime, clamped. Pure, deterministic.

pub fn size(age: f32, lifetime: f32, birth_size: f32, death_size: f32) -> f32 {
    let lt = lifetime.max(1e-6);
    let t = (age.max(0.0) / lt).clamp(0.0, 1.0);
    (birth_size * (1.0 - t) + death_size * t).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn birth_is_birth_size() { assert!((size(0.0, 10.0, 2.0, 0.5) - 2.0).abs() < 1e-6); }
    #[test] fn death_is_death_size() { assert!((size(10.0, 10.0, 2.0, 0.5) - 0.5).abs() < 1e-6); }
    #[test] fn mid_between() { let s = size(5.0, 10.0, 2.0, 0.5); assert!(s < 2.0 && s > 0.5); }
    #[test] fn clamped_past_life() { assert!((size(20.0, 10.0, 2.0, 0.5) - 0.5).abs() < 1e-6); }
    #[test] fn nonneg() { assert!(size(5.0, 10.0, -1.0, -1.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(size(5.0, 10.0, 2.0, 0.5).to_bits(), size(5.0, 10.0, 2.0, 0.5).to_bits()); }
}
