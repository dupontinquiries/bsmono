//! Health-bar fill fraction in [0,1]. Guards max ≤ 0. Pure, deterministic.

pub fn fill(current: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    (current / max).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn full() { assert_eq!(fill(100.0, 100.0), 1.0); }
    #[test] fn empty() { assert_eq!(fill(0.0, 100.0), 0.0); }
    #[test] fn half() { assert!((fill(50.0, 100.0) - 0.5).abs() < 1e-6); }
    #[test] fn overfull_clamped() { assert_eq!(fill(150.0, 100.0), 1.0); }
    #[test] fn zero_max_guarded() { assert_eq!(fill(50.0, 0.0), 0.0); }
    #[test] fn deterministic() { assert_eq!(fill(50.0, 100.0).to_bits(), fill(50.0, 100.0).to_bits()); }
}
