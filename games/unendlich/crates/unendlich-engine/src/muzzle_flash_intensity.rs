//! Muzzle-flash brightness over its short life: fast exponential decay from 1.
//! In [0,1]. Pure, deterministic.

pub fn intensity(age: f32, decay_rate: f32) -> f32 {
    let a = age.max(0.0);
    let k = decay_rate.max(0.0);
    (-(k * a)).exp().clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn spawn_full() { assert_eq!(intensity(0.0, 8.0), 1.0); }
    #[test] fn later_dimmer() { assert!(intensity(0.3, 8.0) < 1.0); }
    #[test] fn faster_decay_dimmer() { assert!(intensity(0.2, 12.0) < intensity(0.2, 4.0)); }
    #[test] fn bounded() { let mut a = 0.0f32; while a < 2.0 { assert!((0.0..=1.0).contains(&intensity(a, 8.0))); a += 0.1; } }
    #[test] fn monotone() { assert!(intensity(0.5, 8.0) < intensity(0.1, 8.0)); }
    #[test] fn deterministic() { assert_eq!(intensity(0.3, 8.0).to_bits(), intensity(0.3, 8.0).to_bits()); }
}
