//! Beer–Lambert transmittance through a participating medium: exp(−density·distance).
//! In [0,1]. Pure, deterministic.

pub fn transmittance(density: f32, distance: f32) -> f32 {
    let d = density.max(0.0);
    let dist = distance.max(0.0);
    (-(d * dist)).exp().clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_density_clear() { assert_eq!(transmittance(0.0, 100.0), 1.0); }
    #[test] fn dense_far_opaque() { assert!(transmittance(1.0, 100.0) < 0.01); }
    #[test] fn monotone_in_distance() {
        let (mut prev, mut x) = (f32::MAX, 0.0f32);
        while x < 20.0 { let v = transmittance(0.3, x); assert!(v <= prev + 1e-6); prev = v; x += 1.0; }
    }
    #[test] fn bounded() {
        let mut x = 0.0f32;
        while x < 50.0 { assert!((0.0..=1.0).contains(&transmittance(0.2, x))); x += 2.0; }
    }
    #[test] fn zero_distance_clear() { assert_eq!(transmittance(5.0, 0.0), 1.0); }
    #[test] fn deterministic() { assert_eq!(transmittance(0.3, 10.0).to_bits(), transmittance(0.3, 10.0).to_bits()); }
}
