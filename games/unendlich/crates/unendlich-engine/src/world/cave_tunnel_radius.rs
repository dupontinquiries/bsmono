//! Worm-cave tunnel radius from a noise sample [-1,1]: base radius modulated by
//! noise so tunnels pinch and bulge, floored at a minimum. Pure, deterministic.

pub fn radius(noise: f32, base_radius: f32, variation: f32) -> f32 {
    let n = noise.clamp(-1.0, 1.0);
    let b = base_radius.max(0.0);
    let v = variation.clamp(0.0, 1.0);
    (b * (1.0 + n * v)).max(0.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_noise_base() { assert_eq!(radius(0.0, 3.0, 0.5), 3.0); }
    #[test] fn positive_bulges() { assert!(radius(1.0, 3.0, 0.5) > 3.0); }
    #[test] fn negative_pinches() { assert!(radius(-1.0, 3.0, 0.5) < 3.0); }
    #[test] fn floored() { assert!(radius(-1.0, 0.05, 1.0) >= 0.1); }
    #[test] fn no_variation_base() { assert_eq!(radius(1.0, 3.0, 0.0), 3.0); }
    #[test] fn deterministic() { assert_eq!(radius(0.5, 3.0, 0.5).to_bits(), radius(0.5, 3.0, 0.5).to_bits()); }
}
