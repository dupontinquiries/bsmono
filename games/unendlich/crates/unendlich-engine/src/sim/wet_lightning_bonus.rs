//! Lightning damage amplified vs a wet target: dmg·(1 + wet·bonus_mult). wet in
//! [0,1] (0 dry, 1 soaked). Pure, deterministic.

pub fn bonus(lightning_dmg: f32, wet: f32, bonus_mult: f32) -> f32 {
    lightning_dmg.max(0.0) * (1.0 + wet.clamp(0.0, 1.0) * bonus_mult.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn dry_base() { assert_eq!(bonus(100.0, 0.0, 1.0), 100.0); }
    #[test] fn soaked_amplified() { assert!((bonus(100.0, 1.0, 1.0) - 200.0).abs() < 1e-4); }
    #[test] fn partial_between() { let v = bonus(100.0, 0.5, 1.0); assert!(v > 100.0 && v < 200.0); }
    #[test] fn zero_dmg_zero() { assert_eq!(bonus(0.0, 1.0, 1.0), 0.0); }
    #[test] fn monotone_in_wet() { assert!(bonus(100.0, 0.8, 1.0) > bonus(100.0, 0.2, 1.0)); }
    #[test] fn deterministic() { assert_eq!(bonus(100.0, 0.5, 1.0).to_bits(), bonus(100.0, 0.5, 1.0).to_bits()); }
}
