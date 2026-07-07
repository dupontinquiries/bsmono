//! Breath damage by distance: full at the mouth, linearly to 0 at max_range. Pure.

pub fn damage(distance: f32, max_range: f32, base: f32) -> f32 {
    let d = distance.max(0.0);
    let mr = max_range.max(1e-6);
    let b = base.max(0.0);
    if d >= mr {
        0.0
    } else {
        b * (1.0 - d / mr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn at_mouth_full() { assert!((damage(0.0, 20.0, 50.0) - 50.0).abs() < 1e-4); }
    #[test] fn at_range_zero() { assert_eq!(damage(20.0, 20.0, 50.0), 0.0); }
    #[test] fn beyond_zero() { assert_eq!(damage(25.0, 20.0, 50.0), 0.0); }
    #[test] fn closer_more() { assert!(damage(2.0, 20.0, 50.0) > damage(15.0, 20.0, 50.0)); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 25.0 { let v = damage(d, 20.0, 50.0); assert!((0.0..=50.0).contains(&v)); d += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(damage(5.0, 20.0, 50.0).to_bits(), damage(5.0, 20.0, 50.0).to_bits()); }
}
