//! Soft sun-disk intensity: 1 inside the angular radius, smooth falloff to 0 in a
//! small penumbra just outside. Angles in radians. Pure, deterministic.

pub fn intensity(angle_to_sun: f32, angular_radius: f32) -> f32 {
    let a = angle_to_sun.abs();
    let r = angular_radius.max(1e-6);
    let penumbra = 0.3 * r;
    if a <= r {
        1.0
    } else if a >= r + penumbra {
        0.0
    } else {
        1.0 - (a - r) / penumbra
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn center_full() { assert_eq!(intensity(0.0, 0.1), 1.0); }
    #[test] fn far_zero() { assert_eq!(intensity(1.0, 0.1), 0.0); }
    #[test] fn penumbra_partial() { let v = intensity(0.115, 0.1); assert!(v > 0.0 && v < 1.0); }
    #[test] fn bounded() {
        let mut a = 0.0f32;
        while a < 0.3 { let v = intensity(a, 0.1); assert!((0.0..=1.0).contains(&v)); a += 0.005; }
    }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut a) = (f32::MAX, 0.0f32);
        while a < 0.3 { let v = intensity(a, 0.1); assert!(v <= prev + 1e-6); prev = v; a += 0.005; }
    }
    #[test] fn deterministic() { assert_eq!(intensity(0.115, 0.1).to_bits(), intensity(0.115, 0.1).to_bits()); }
}
