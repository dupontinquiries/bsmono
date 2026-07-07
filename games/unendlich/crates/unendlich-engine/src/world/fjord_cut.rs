//! Flooded glacial valley depth: deepest at the fjord axis near the coast,
//! shoaling inland and to 0 past the half-width (worldgen). Pure, deterministic.

pub fn depth(dist_inland: f32, dist_to_axis: f32, max_depth: f32) -> f32 {
    let md = max_depth.max(0.0);
    let width = 50.0;
    let length = 2000.0;
    let a = dist_to_axis.abs();
    if a >= width {
        return 0.0;
    }
    let across = 1.0 - a / width;
    let inland = (1.0 - dist_inland.max(0.0) / length).max(0.0);
    md * across * inland
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn axis_coast_deep() { assert!((depth(0.0, 0.0, 100.0) - 100.0).abs() < 1e-4); }
    #[test] fn far_inland_shallow() { assert!(depth(2000.0, 0.0, 100.0) < 1.0); }
    #[test] fn beyond_width_zero() { assert_eq!(depth(0.0, 60.0, 100.0), 0.0); }
    #[test] fn bounded() {
        let mut a = 0.0f32;
        while a < 60.0 { let d = depth(100.0, a, 100.0); assert!((0.0..=100.0).contains(&d)); a += 2.0; }
    }
    #[test] fn monotone_in_axis() {
        let (mut prev, mut a) = (f32::MAX, 0.0f32);
        while a < 50.0 { let d = depth(200.0, a, 100.0); assert!(d <= prev + 1e-6); prev = d; a += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(depth(100.0, 10.0, 100.0).to_bits(), depth(100.0, 10.0, 100.0).to_bits()); }
}
