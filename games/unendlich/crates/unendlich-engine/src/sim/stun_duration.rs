//! Stun duration after resistance: power reduced by resistance (capped at 100%),
//! floored at 0 (fully resisted ⇒ no stun). Pure, deterministic.

pub fn duration(power: f32, resistance: f32) -> f32 {
    (power.max(0.0) * (1.0 - resistance.clamp(0.0, 1.0))).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_resist_full() { assert_eq!(duration(3.0, 0.0), 3.0); }
    #[test] fn full_resist_zero() { assert_eq!(duration(3.0, 1.0), 0.0); }
    #[test] fn half_resist_half() { assert!((duration(3.0, 0.5) - 1.5).abs() < 1e-5); }
    #[test] fn bounded() { let mut r = 0.0f32; while r <= 1.0 { let d = duration(3.0, r); assert!((0.0..=3.0).contains(&d)); r += 0.1; } }
    #[test] fn monotone_in_resist() { assert!(duration(3.0, 0.8) < duration(3.0, 0.2)); }
    #[test] fn deterministic() { assert_eq!(duration(3.0, 0.3).to_bits(), duration(3.0, 0.3).to_bits()); }
}
