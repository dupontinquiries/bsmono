//! Oxbow-lake formation strength: a tight, long-lived meander pinches off into a
//! cutoff lake. Grows with curvature and age. In [0,1].

pub fn oxbow(curvature: f32, age: f32) -> f32 {
    let c = curvature.abs();
    let a = age.max(0.0);
    let sat_c = c / (1.0 + c);
    let sat_a = 1.0 - (-(a * 0.1)).exp();
    (sat_c * sat_a).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn straight_no_oxbow() { assert_eq!(oxbow(0.0, 100.0), 0.0); }
    #[test] fn young_no_oxbow() { assert!(oxbow(5.0, 0.0).abs() < 1e-6); }
    #[test] fn tight_and_old_high() { assert!(oxbow(10.0, 100.0) > 0.7); }
    #[test] fn more_age_more() { assert!(oxbow(3.0, 50.0) > oxbow(3.0, 5.0)); }
    #[test] fn bounded() {
        for &(c, a) in &[(0.0f32, 0.0f32), (100.0, 100.0), (2.0, 20.0)] { let v = oxbow(c, a); assert!((0.0..=1.0).contains(&v)); }
    }
    #[test] fn deterministic() { assert_eq!(oxbow(4.0, 30.0).to_bits(), oxbow(4.0, 30.0).to_bits()); }
}
