//! Particle alpha over life: fades in over the first 15%, holds full, fades out
//! over the last 30%. In [0,1]. Pure, deterministic.

pub fn alpha(age: f32, lifetime: f32) -> f32 {
    let lt = lifetime.max(1e-6);
    let t = (age.max(0.0) / lt).clamp(0.0, 1.0);
    let a = if t < 0.15 {
        t / 0.15
    } else if t > 0.7 {
        (1.0 - t) / 0.3
    } else {
        1.0
    };
    a.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn birth_zero() { assert_eq!(alpha(0.0, 10.0), 0.0); }
    #[test] fn mid_full() { assert!((alpha(5.0, 10.0) - 1.0).abs() < 1e-6); }
    #[test] fn death_zero() { assert!(alpha(10.0, 10.0).abs() < 1e-6); }
    #[test] fn fade_in_partial() { let a = alpha(0.75, 10.0); assert!(a > 0.0 && a < 1.0); }
    #[test] fn bounded() {
        let mut age = 0.0f32;
        while age <= 10.0 { assert!((0.0..=1.0).contains(&alpha(age, 10.0))); age += 0.25; }
    }
    #[test] fn deterministic() { assert_eq!(alpha(3.0, 10.0).to_bits(), alpha(3.0, 10.0).to_bits()); }
}
