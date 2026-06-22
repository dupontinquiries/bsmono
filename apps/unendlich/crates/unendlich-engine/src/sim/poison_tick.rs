//! Poison damage this tick: a percentage of MAX HP per stack, scaled by dt — so
//! poison scales with the target (unlike flat burn). Pure, deterministic.

pub fn poison_tick(stacks: u8, max_hp: f32, pct_per_stack: f32, dt: f32) -> f32 {
    (stacks as f32) * max_hp.max(0.0) * pct_per_stack.clamp(0.0, 1.0) * dt.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_stacks_zero() { assert_eq!(poison_tick(0, 100.0, 0.01, 0.1), 0.0); }
    #[test] fn more_stacks_more() { assert!(poison_tick(5, 100.0, 0.01, 0.1) > poison_tick(1, 100.0, 0.01, 0.1)); }
    #[test] fn scales_with_hp() { assert!(poison_tick(2, 200.0, 0.01, 0.1) > poison_tick(2, 100.0, 0.01, 0.1)); }
    #[test] fn longer_dt_more() { assert!(poison_tick(2, 100.0, 0.01, 0.2) > poison_tick(2, 100.0, 0.01, 0.1)); }
    #[test] fn pct_clamped() { assert_eq!(poison_tick(1, 100.0, 5.0, 0.1).to_bits(), poison_tick(1, 100.0, 1.0, 0.1).to_bits()); }
    #[test] fn deterministic() { assert_eq!(poison_tick(2, 100.0, 0.01, 0.1).to_bits(), poison_tick(2, 100.0, 0.01, 0.1).to_bits()); }
}
