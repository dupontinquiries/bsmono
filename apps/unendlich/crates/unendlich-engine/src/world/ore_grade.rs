//! Ore grade/quality by depth and a noise sample, scaled by a richness factor.
//! Deeper + richer ⇒ better ore, in [0,1]. Pure, deterministic.

pub fn grade(depth: f32, noise: f32, richness: f32) -> f32 {
    let d = depth.max(0.0);
    let n = noise.clamp(0.0, 1.0);
    let r = richness.max(0.0);
    let depth_factor = 1.0 - (-(d / 200.0)).exp();
    (depth_factor * n * r).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn surface_is_zero() { assert_eq!(grade(0.0, 1.0, 1.0), 0.0); }
    #[test] fn deeper_is_more() { assert!(grade(500.0, 1.0, 1.0) > grade(50.0, 1.0, 1.0)); }
    #[test] fn more_noise_more() { assert!(grade(300.0, 0.9, 1.0) > grade(300.0, 0.3, 1.0)); }
    #[test] fn more_richness_more() { assert!(grade(300.0, 0.5, 0.9) > grade(300.0, 0.5, 0.3)); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 2000.0 { let v = grade(d, 0.8, 1.0); assert!((0.0..=1.0).contains(&v)); d += 50.0; }
    }
    #[test] fn deterministic() { assert_eq!(grade(300.0, 0.8, 1.0).to_bits(), grade(300.0, 0.8, 1.0).to_bits()); }
}
