//! Ability resource cost: base scaled by ability level, reduced by efficiency
//! (capped at 90%). Pure, deterministic.

pub fn mana_cost(base: f32, level: u8, efficiency: f32) -> f32 {
    let b = base.max(0.0);
    let lvl = level as f32;
    let eff = efficiency.clamp(0.0, 0.9);
    (b * (1.0 + 0.2 * lvl) * (1.0 - eff)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn higher_level_more() { assert!(mana_cost(10.0, 5, 0.0) > mana_cost(10.0, 1, 0.0)); }
    #[test] fn more_efficiency_less() { assert!(mana_cost(10.0, 3, 0.8) < mana_cost(10.0, 3, 0.1)); }
    #[test] fn level0_base() { assert!((mana_cost(10.0, 0, 0.0) - 10.0).abs() < 1e-5); }
    #[test] fn nonneg() { assert!(mana_cost(10.0, 5, 0.9) >= 0.0); }
    #[test] fn monotone_in_level() {
        let mut prev = f32::MIN;
        for l in 0..20u8 { let c = mana_cost(10.0, l, 0.0); assert!(c >= prev); prev = c; }
    }
    #[test] fn deterministic() { assert_eq!(mana_cost(10.0, 3, 0.5).to_bits(), mana_cost(10.0, 3, 0.5).to_bits()); }
}
