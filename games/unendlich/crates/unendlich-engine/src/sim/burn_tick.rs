//! Burn damage-over-time this tick: base per stack, scaled by dt. Pure.

pub fn burn_tick(stacks: u8, base_per_stack: f32, dt: f32) -> f32 {
    (stacks as f32) * base_per_stack.max(0.0) * dt.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_stacks_no_damage() { assert_eq!(burn_tick(0, 2.0, 0.1), 0.0); }
    #[test] fn more_stacks_more() { assert!(burn_tick(5, 2.0, 0.1) > burn_tick(1, 2.0, 0.1)); }
    #[test] fn longer_dt_more() { assert!(burn_tick(3, 2.0, 0.2) > burn_tick(3, 2.0, 0.1)); }
    #[test] fn linear_in_stacks() { assert!((burn_tick(4, 2.0, 0.1) - 4.0 * burn_tick(1, 2.0, 0.1)).abs() < 1e-5); }
    #[test] fn nonneg() { assert!(burn_tick(3, 2.0, -1.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(burn_tick(3, 2.0, 0.1).to_bits(), burn_tick(3, 2.0, 0.1).to_bits()); }
}
