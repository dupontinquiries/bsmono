//! Physical impact amplified vs a frozen target ("shatter"): dmg·(1 + freeze·mult).
//! freeze in [0,1]. Pure, deterministic.

pub fn bonus(physical_dmg: f32, freeze: f32, shatter_mult: f32) -> f32 {
    physical_dmg.max(0.0) * (1.0 + freeze.clamp(0.0, 1.0) * shatter_mult.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn unfrozen_base() { assert_eq!(bonus(80.0, 0.0, 1.5), 80.0); }
    #[test] fn frozen_amplified() { assert!((bonus(80.0, 1.0, 1.5) - 200.0).abs() < 1e-4); }
    #[test] fn partial_between() { let v = bonus(80.0, 0.5, 1.5); assert!(v > 80.0 && v < 200.0); }
    #[test] fn zero_dmg_zero() { assert_eq!(bonus(0.0, 1.0, 1.5), 0.0); }
    #[test] fn monotone_in_freeze() { assert!(bonus(80.0, 0.8, 1.5) > bonus(80.0, 0.2, 1.5)); }
    #[test] fn deterministic() { assert_eq!(bonus(80.0, 0.5, 1.5).to_bits(), bonus(80.0, 0.5, 1.5).to_bits()); }
}
