//! Soil moisture near a water body in [0,1]: full at the shore, decaying linearly
//! to 0 at max_dist. Pure, deterministic.

pub fn moisture(distance_to_water: f32, max_dist: f32) -> f32 {
    let d = distance_to_water.max(0.0);
    let md = max_dist.max(1e-4);
    (1.0 - d / md).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn shore_full() { assert_eq!(moisture(0.0, 20.0), 1.0); }
    #[test] fn edge_zero() { assert_eq!(moisture(20.0, 20.0), 0.0); }
    #[test] fn beyond_zero() { assert_eq!(moisture(50.0, 20.0), 0.0); }
    #[test] fn mid_half() { assert!((moisture(10.0, 20.0) - 0.5).abs() < 1e-6); }
    #[test] fn monotone() { assert!(moisture(15.0, 20.0) < moisture(5.0, 20.0)); }
    #[test] fn deterministic() { assert_eq!(moisture(10.0, 20.0).to_bits(), moisture(10.0, 20.0).to_bits()); }
}
