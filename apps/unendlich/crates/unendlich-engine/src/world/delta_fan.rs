//! Sediment fan deposit at a river mouth: peaks at the mouth, fades with distance
//! over `spread`, scaled by flow (worldgen).

pub fn deposit(dist_from_mouth: f32, flow: f32, spread: f32) -> f32 {
    let s = spread.max(1e-6);
    let f = flow.max(0.0);
    let d = dist_from_mouth.max(0.0);
    f * (-(d / s)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn peaks_at_mouth() { assert!((deposit(0.0, 8.0, 5.0) - 8.0).abs() < 1e-5); }
    #[test] fn fades_far() { assert!(deposit(1000.0, 8.0, 5.0) < 0.01); }
    #[test] fn more_flow_more() { assert!(deposit(3.0, 10.0, 5.0) > deposit(3.0, 2.0, 5.0)); }
    #[test] fn spread_widens_reach() { assert!(deposit(10.0, 8.0, 20.0) > deposit(10.0, 8.0, 5.0)); }
    #[test] fn bounded_by_flow() {
        let mut d = 0.0f32;
        while d < 50.0 { let v = deposit(d, 8.0, 5.0); assert!((0.0..=8.0 + 1e-4).contains(&v)); d += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(deposit(4.0, 8.0, 5.0).to_bits(), deposit(4.0, 8.0, 5.0).to_bits()); }
}
