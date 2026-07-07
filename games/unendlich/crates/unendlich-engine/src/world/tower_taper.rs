//! Tower radius at `height_fraction` in [0,1]: linear taper from base to top. Pure.

pub fn radius(height_fraction: f32, base_radius: f32, top_radius: f32) -> f32 {
    let h = height_fraction.clamp(0.0, 1.0);
    let b = base_radius.max(0.0);
    let t = top_radius.max(0.0);
    (b * (1.0 - h) + t * h).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn base_is_base_radius() { assert_eq!(radius(0.0, 10.0, 4.0), 10.0); }
    #[test] fn top_is_top_radius() { assert!((radius(1.0, 10.0, 4.0) - 4.0).abs() < 1e-6); }
    #[test] fn mid_between() { let r = radius(0.5, 10.0, 4.0); assert!(r < 10.0 && r > 4.0); }
    #[test] fn tapers_inward() { assert!(radius(0.9, 10.0, 4.0) < radius(0.1, 10.0, 4.0)); }
    #[test] fn bounded() {
        let mut h = 0.0f32;
        while h <= 1.0 { let r = radius(h, 10.0, 4.0); assert!((4.0..=10.0).contains(&r)); h += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(radius(0.5, 10.0, 4.0).to_bits(), radius(0.5, 10.0, 4.0).to_bits()); }
}
