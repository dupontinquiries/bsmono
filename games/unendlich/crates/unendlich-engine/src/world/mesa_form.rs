//! Flat-top mesa height profile: cap plateau out to 80% radius, then a cliff
//! ramp to 0 at the radius (worldgen). Pure, deterministic.

pub fn height(dist_from_center: f32, radius: f32, cap_height: f32) -> f32 {
    let r = radius.max(1e-6);
    let ch = cap_height.max(0.0);
    let d = dist_from_center.abs();
    if d >= r {
        return 0.0;
    }
    let inner = 0.8 * r;
    if d <= inner {
        ch
    } else {
        ch * ((r - d) / (r - inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn center_is_cap() { assert_eq!(height(0.0, 100.0, 50.0), 50.0); }
    #[test] fn beyond_radius_zero() { assert_eq!(height(101.0, 100.0, 50.0), 0.0); }
    #[test] fn flat_top_plateau() { assert_eq!(height(0.0, 100.0, 50.0), height(50.0, 100.0, 50.0)); }
    #[test] fn cliff_below_cap() { assert!(height(95.0, 100.0, 50.0) < 50.0 && height(95.0, 100.0, 50.0) > 0.0); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 120.0 { let h = height(d, 100.0, 50.0); assert!((0.0..=50.0).contains(&h)); d += 2.0; }
    }
    #[test] fn deterministic() { assert_eq!(height(60.0, 100.0, 50.0).to_bits(), height(60.0, 100.0, 50.0).to_bits()); }
}
