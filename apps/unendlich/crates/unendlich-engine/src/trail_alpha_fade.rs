//! Projectile-trail opacity by age: 1 at spawn, linearly fading to 0 at lifetime.
//! In [0,1]. Pure, deterministic.

pub fn alpha(age: f32, lifetime: f32) -> f32 {
    let a = age.max(0.0);
    let l = lifetime.max(1e-4);
    (1.0 - a / l).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn fresh_opaque() { assert_eq!(alpha(0.0, 2.0), 1.0); }
    #[test] fn expired_clear() { assert_eq!(alpha(2.0, 2.0), 0.0); }
    #[test] fn past_clear() { assert_eq!(alpha(5.0, 2.0), 0.0); }
    #[test] fn mid_half() { assert!((alpha(1.0, 2.0) - 0.5).abs() < 1e-6); }
    #[test] fn monotone() { assert!(alpha(1.5, 2.0) < alpha(0.5, 2.0)); }
    #[test] fn deterministic() { assert_eq!(alpha(1.0, 2.0).to_bits(), alpha(1.0, 2.0).to_bits()); }
}
