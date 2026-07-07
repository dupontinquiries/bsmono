//! Breath resource recharge toward full (1.0) over time: current + rate·dt, capped.
//! Pure, deterministic.

pub fn recharge(current: f32, rate: f32, dt: f32) -> f32 {
    let c = current.clamp(0.0, 1.0);
    (c + rate.max(0.0) * dt.max(0.0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_dt_unchanged() { assert_eq!(recharge(0.4, 0.5, 0.0), 0.4); }
    #[test] fn recharges_up() { assert!(recharge(0.4, 0.5, 0.2) > 0.4); }
    #[test] fn capped_at_full() { assert_eq!(recharge(0.9, 1.0, 5.0), 1.0); }
    #[test] fn from_empty_rises() { assert!(recharge(0.0, 0.5, 0.2) > 0.0); }
    #[test] fn longer_dt_more() { assert!(recharge(0.2, 0.5, 0.4) > recharge(0.2, 0.5, 0.1)); }
    #[test] fn deterministic() { assert_eq!(recharge(0.4, 0.5, 0.2).to_bits(), recharge(0.4, 0.5, 0.2).to_bits()); }
}
