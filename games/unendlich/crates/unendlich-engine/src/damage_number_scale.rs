//! Floating damage-number render scale: bigger hits render bigger, mapped from
//! damage relative to a reference, clamped to [min_scale, max_scale]. Pure.

pub fn scale(damage: f32, ref_damage: f32, min_scale: f32, max_scale: f32) -> f32 {
    let d = damage.max(0.0);
    let rd = ref_damage.max(1e-4);
    let mn = min_scale.max(0.0);
    let mx = max_scale.max(mn);
    mn + (mx - mn) * (d / rd).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_is_min() { assert_eq!(scale(0.0, 100.0, 0.5, 2.0), 0.5); }
    #[test] fn ref_is_max() { assert!((scale(100.0, 100.0, 0.5, 2.0) - 2.0).abs() < 1e-6); }
    #[test] fn over_capped() { assert_eq!(scale(500.0, 100.0, 0.5, 2.0), 2.0); }
    #[test] fn bigger_bigger() { assert!(scale(80.0, 100.0, 0.5, 2.0) > scale(20.0, 100.0, 0.5, 2.0)); }
    #[test] fn never_below_min() { assert!(scale(0.0, 100.0, 0.5, 2.0) >= 0.5); }
    #[test] fn deterministic() { assert_eq!(scale(80.0, 100.0, 0.5, 2.0).to_bits(), scale(80.0, 100.0, 0.5, 2.0).to_bits()); }
}
