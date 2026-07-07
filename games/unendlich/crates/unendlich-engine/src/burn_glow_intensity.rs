//! Pulsing ember-glow intensity for a burning entity: brighter with more stacks,
//! flickering ~6 Hz over time. In [0,1]. Pure, deterministic.

use std::f32::consts::PI;

pub fn intensity(stacks: u32, t: f32, max_stacks: u32) -> f32 {
    let ms = max_stacks.max(1);
    let s = (stacks.min(ms) as f32) / (ms as f32);
    let flicker = 0.85 + 0.15 * (2.0 * PI * 6.0 * t).sin();
    (s * flicker).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_stacks_dark() { assert_eq!(intensity(0, 0.0, 5), 0.0); }
    #[test] fn more_stacks_brighter() { assert!(intensity(5, 0.0, 5) > intensity(1, 0.0, 5)); }
    #[test] fn flickers_over_time() { assert!((intensity(5, 0.0, 5) - intensity(5, 0.05, 5)).abs() > 1e-3); }
    #[test] fn bounded() { let mut t = 0.0f32; while t < 1.0 { assert!((0.0..=1.0).contains(&intensity(3, t, 5))); t += 0.02; } }
    #[test] fn capped_at_max() { assert_eq!(intensity(100, 0.0, 5), intensity(5, 0.0, 5)); }
    #[test] fn deterministic() { assert_eq!(intensity(3, 0.1, 5).to_bits(), intensity(3, 0.1, 5).to_bits()); }
}
