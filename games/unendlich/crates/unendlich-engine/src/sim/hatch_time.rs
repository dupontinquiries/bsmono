//! Egg incubation time (abstract seconds): a base scaled by genome complexity and
//! shortened by warmth (more heat ⇒ faster hatch). Floored at 10% of base. Pure.

pub fn hatch_time(base: f32, complexity: f32, warmth: f32) -> f32 {
    let b = base.max(0.0);
    let c = complexity.max(0.0);
    let w = warmth.clamp(0.0, 1.0);
    (b * (1.0 + c) * (1.0 - 0.5 * w)).max(b * 0.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_complexity_longer() { assert!(hatch_time(100.0, 2.0, 0.0) > hatch_time(100.0, 0.5, 0.0)); }
    #[test] fn more_warmth_faster() { assert!(hatch_time(100.0, 1.0, 0.9) < hatch_time(100.0, 1.0, 0.1)); }
    #[test] fn zero_warmth_full() { assert!((hatch_time(100.0, 1.0, 0.0) - 200.0).abs() < 1e-3); }
    #[test] fn floored() { assert!(hatch_time(100.0, 0.0, 1.0) >= 10.0); }
    #[test] fn nonneg() { assert!(hatch_time(0.0, 5.0, 0.5) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(hatch_time(100.0, 1.0, 0.5).to_bits(), hatch_time(100.0, 1.0, 0.5).to_bits()); }
}
