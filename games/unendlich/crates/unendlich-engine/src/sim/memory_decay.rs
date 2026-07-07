//! Memory strength after time dt with a given half-life: exponential decay. Pure.

pub fn decay(strength: f32, dt: f32, half_life: f32) -> f32 {
    let s = strength.max(0.0);
    let t = dt.max(0.0);
    let hl = half_life.max(1e-4);
    s * (-(std::f32::consts::LN_2 / hl * t)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_dt_unchanged() { assert_eq!(decay(1.0, 0.0, 10.0), 1.0); }
    #[test] fn half_life_halves() { assert!((decay(1.0, 10.0, 10.0) - 0.5).abs() < 1e-4); }
    #[test] fn longer_less() { assert!(decay(1.0, 20.0, 10.0) < decay(1.0, 5.0, 10.0)); }
    #[test] fn zero_strength_zero() { assert_eq!(decay(0.0, 5.0, 10.0), 0.0); }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut t) = (f32::MAX, 0.0f32);
        while t < 50.0 { let v = decay(1.0, t, 10.0); assert!(v <= prev + 1e-6); prev = v; t += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(decay(1.0, 5.0, 10.0).to_bits(), decay(1.0, 5.0, 10.0).to_bits()); }
}
