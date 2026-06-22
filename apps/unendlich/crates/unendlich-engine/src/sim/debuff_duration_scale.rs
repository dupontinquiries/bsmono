//! Debuff duration scaled by level difference: a higher-level target shrugs it off
//! faster, a lower-level target suffers longer. Scale clamped to [0.25, 2.0]. Pure.

pub fn duration(base_duration: f32, target_level: i32, caster_level: i32) -> f32 {
    let base = base_duration.max(0.0);
    let diff = (target_level - caster_level) as f32;
    let scale = (1.0 - 0.05 * diff).clamp(0.25, 2.0);
    base * scale
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn equal_base() { assert_eq!(duration(10.0, 5, 5), 10.0); }
    #[test] fn higher_target_shorter() { assert!(duration(10.0, 10, 5) < 10.0); }
    #[test] fn lower_target_longer() { assert!(duration(10.0, 1, 5) > 10.0); }
    #[test] fn floored() { assert!((duration(10.0, 100, 5) - 2.5).abs() < 1e-4); }
    #[test] fn capped() { assert!((duration(10.0, 1, 100) - 20.0).abs() < 1e-4); }
    #[test] fn deterministic() { assert_eq!(duration(10.0, 8, 5).to_bits(), duration(10.0, 8, 5).to_bits()); }
}
