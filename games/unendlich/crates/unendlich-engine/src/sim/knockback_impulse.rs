//! Knockback velocity from a hit: ∝ damage / mass, reduced by knockback
//! resistance. Heavier or more resistant ⇒ less knockback. Pure, deterministic.

pub fn impulse(damage: f32, mass: f32, resist: f32) -> f32 {
    let d = damage.max(0.0);
    let m = mass.max(0.1);
    let r = resist.clamp(0.0, 1.0);
    (d / m) * (1.0 - r)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_damage_more() { assert!(impulse(100.0, 10.0, 0.0) > impulse(20.0, 10.0, 0.0)); }
    #[test] fn heavier_less() { assert!(impulse(100.0, 50.0, 0.0) < impulse(100.0, 10.0, 0.0)); }
    #[test] fn more_resist_less() { assert!(impulse(100.0, 10.0, 0.8) < impulse(100.0, 10.0, 0.1)); }
    #[test] fn full_resist_zero() { assert_eq!(impulse(100.0, 10.0, 1.0), 0.0); }
    #[test] fn nonneg() { assert!(impulse(100.0, 10.0, 0.5) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(impulse(100.0, 10.0, 0.5).to_bits(), impulse(100.0, 10.0, 0.5).to_bits()); }
}
