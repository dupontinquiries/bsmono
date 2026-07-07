//! White hit-flash intensity over a short duration: linear 1→0. In [0,1]. Pure.

pub fn intensity(age: f32, duration: f32) -> f32 {
    let a = age.max(0.0);
    let d = duration.max(1e-4);
    (1.0 - a / d).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fresh_full() { assert_eq!(intensity(0.0, 0.2), 1.0); }
    #[test] fn expired_zero() { assert_eq!(intensity(0.2, 0.2), 0.0); }
    #[test] fn past_zero() { assert_eq!(intensity(0.5, 0.2), 0.0); }
    #[test] fn mid_half() { assert!((intensity(0.1, 0.2) - 0.5).abs() < 1e-6); }
    #[test] fn monotone() { assert!(intensity(0.15, 0.2) < intensity(0.05, 0.2)); }
    #[test] fn deterministic() { assert_eq!(intensity(0.1, 0.2).to_bits(), intensity(0.1, 0.2).to_bits()); }
}
