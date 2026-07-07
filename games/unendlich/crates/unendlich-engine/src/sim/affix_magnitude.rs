//! Affix stat value: base × level scaling × roll quality. roll spans 50%..100%
//! of the level-scaled value: base·(1 + level/100)·(0.5 + 0.5·roll). Pure.

pub fn magnitude(base: f32, item_level: u32, roll: f32) -> f32 {
    let b = base.max(0.0);
    let lvl = 1.0 + item_level as f32 / 100.0;
    let r = 0.5 + 0.5 * roll.clamp(0.0, 1.0);
    b * lvl * r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn min_roll_half() { assert!((magnitude(100.0, 0, 0.0) - 50.0).abs() < 1e-4); }
    #[test] fn max_roll_full() { assert!((magnitude(100.0, 0, 1.0) - 100.0).abs() < 1e-4); }
    #[test] fn level_scales() { assert!((magnitude(100.0, 100, 1.0) - 200.0).abs() < 1e-4); }
    #[test] fn roll_scales() { assert!(magnitude(100.0, 50, 0.9) > magnitude(100.0, 50, 0.1)); }
    #[test] fn zero_base_zero() { assert_eq!(magnitude(0.0, 50, 1.0), 0.0); }
    #[test] fn deterministic() { assert_eq!(magnitude(100.0, 50, 0.7).to_bits(), magnitude(100.0, 50, 0.7).to_bits()); }
}
