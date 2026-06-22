//! Glide ratio L/D — horizontal distance per unit altitude lost in a glide.
//! Guards drag→0 (no inf/NaN). Pure, deterministic.

pub fn glide_ratio(lift: f32, drag: f32) -> f32 {
    let l = lift.max(0.0);
    let d = drag.max(1e-6);
    l / d
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_lift_higher() { assert!(glide_ratio(200.0, 20.0) > glide_ratio(100.0, 20.0)); }
    #[test] fn more_drag_lower() { assert!(glide_ratio(100.0, 40.0) < glide_ratio(100.0, 20.0)); }
    #[test] fn zero_drag_finite() { assert!(glide_ratio(100.0, 0.0).is_finite()); }
    #[test] fn zero_lift_zero() { assert_eq!(glide_ratio(0.0, 20.0), 0.0); }
    #[test] fn positive() { assert!(glide_ratio(100.0, 20.0) > 0.0); }
    #[test] fn deterministic() { assert_eq!(glide_ratio(150.0, 20.0).to_bits(), glide_ratio(150.0, 20.0).to_bits()); }
}
