//! Territory conflict in [0,1] from how far two circular territories overlap:
//! 0 = disjoint, 1 = concentric. Pure, deterministic.

pub fn overlap(dist_between_centers: f32, radius_a: f32, radius_b: f32) -> f32 {
    let d = dist_between_centers.max(0.0);
    let sum = radius_a.max(0.0) + radius_b.max(0.0);
    if sum < 1e-6 || d >= sum {
        0.0
    } else {
        ((sum - d) / sum).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn disjoint_zero() { assert_eq!(overlap(100.0, 10.0, 10.0), 0.0); }
    #[test] fn concentric_full() { assert!((overlap(0.0, 10.0, 10.0) - 1.0).abs() < 1e-6); }
    #[test] fn closer_more() { assert!(overlap(5.0, 10.0, 10.0) > overlap(15.0, 10.0, 10.0)); }
    #[test] fn partial() { let v = overlap(10.0, 10.0, 10.0); assert!(v > 0.0 && v < 1.0); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 30.0 { assert!((0.0..=1.0).contains(&overlap(d, 10.0, 10.0))); d += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(overlap(10.0, 10.0, 10.0).to_bits(), overlap(10.0, 10.0, 10.0).to_bits()); }
}
