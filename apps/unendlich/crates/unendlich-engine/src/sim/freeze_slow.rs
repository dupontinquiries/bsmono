//! Movement-speed multiplier from freeze stacks (1.0 = no slow). Each stack adds
//! diminishing slow, saturating toward (1 − max_slow). Floored at 0.05. Pure.

pub fn slow_factor(stacks: u8, max_slow: f32) -> f32 {
    let ms = max_slow.clamp(0.0, 0.95);
    let frac = 1.0 - (-((stacks as f32) * 0.4)).exp();
    (1.0 - ms * frac).clamp(0.05, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_stacks_no_slow() { assert!((slow_factor(0, 0.8) - 1.0).abs() < 1e-6); }
    #[test] fn more_stacks_slower() { assert!(slow_factor(5, 0.8) < slow_factor(1, 0.8)); }
    #[test] fn approaches_cap() { assert!(slow_factor(255, 0.8) < 0.25); }
    #[test] fn bounded() { for s in 0..50u8 { let v = slow_factor(s, 0.8); assert!((0.05..=1.0).contains(&v)); } }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut s) = (f32::MAX, 0u8);
        while s < 30 { let v = slow_factor(s, 0.8); assert!(v <= prev + 1e-6); prev = v; s += 1; }
    }
    #[test] fn deterministic() { assert_eq!(slow_factor(5, 0.8).to_bits(), slow_factor(5, 0.8).to_bits()); }
}
