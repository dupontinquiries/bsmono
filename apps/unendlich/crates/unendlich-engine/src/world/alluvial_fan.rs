//! Alluvial-fan deposit where a channel exits a mountain front: thickest at the
//! apex, thinning outward over `spread`, scaled by the gradient drop.

pub fn deposit(dist_from_apex: f32, gradient_drop: f32, spread: f32) -> f32 {
    let s = spread.max(1e-6);
    let g = gradient_drop.max(0.0);
    let d = dist_from_apex.max(0.0);
    g * (-(d / s)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn thickest_at_apex() { assert!((deposit(0.0, 6.0, 10.0) - 6.0).abs() < 1e-5); }
    #[test] fn thins_far() { assert!(deposit(1000.0, 6.0, 10.0) < 0.01); }
    #[test] fn more_drop_more() { assert!(deposit(5.0, 10.0, 10.0) > deposit(5.0, 2.0, 10.0)); }
    #[test] fn spread_widens() { assert!(deposit(20.0, 6.0, 40.0) > deposit(20.0, 6.0, 10.0)); }
    #[test] fn bounded_by_drop() {
        let mut d = 0.0f32;
        while d < 50.0 { let v = deposit(d, 6.0, 10.0); assert!((0.0..=6.0 + 1e-4).contains(&v)); d += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(deposit(4.0, 6.0, 10.0).to_bits(), deposit(4.0, 6.0, 10.0).to_bits()); }
}
