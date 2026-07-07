//! Damage after elemental resistance: resist in [0,1] reduces damage (1 = immune).

pub fn after_resist(damage: f32, resist: f32) -> f32 {
    damage.max(0.0) * (1.0 - resist.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_resist_full() { assert_eq!(after_resist(100.0, 0.0), 100.0); }
    #[test] fn full_resist_zero() { assert_eq!(after_resist(100.0, 1.0), 0.0); }
    #[test] fn half_resist_half() { assert!((after_resist(100.0, 0.5) - 50.0).abs() < 1e-4); }
    #[test] fn bounded() {
        let mut r = 0.0f32;
        while r <= 1.0 { let v = after_resist(100.0, r); assert!((0.0..=100.0).contains(&v)); r += 0.05; }
    }
    #[test] fn monotone_decreasing() { assert!(after_resist(100.0, 0.8) < after_resist(100.0, 0.2)); }
    #[test] fn deterministic() { assert_eq!(after_resist(100.0, 0.3).to_bits(), after_resist(100.0, 0.3).to_bits()); }
}
