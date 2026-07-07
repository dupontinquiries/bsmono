//! Sediment exchange this step: if suspended exceeds capacity, deposit the surplus
//! (positive); if under, erode toward capacity (negative). Scaled by rate. Pure.

pub fn exchange(suspended: f32, capacity: f32, rate: f32) -> f32 {
    let s = suspended.max(0.0);
    let c = capacity.max(0.0);
    let r = rate.clamp(0.0, 1.0);
    (s - c) * r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn over_deposits() { assert!(exchange(10.0, 5.0, 0.5) > 0.0); }
    #[test] fn under_erodes() { assert!(exchange(2.0, 5.0, 0.5) < 0.0); }
    #[test] fn balanced_zero() { assert_eq!(exchange(5.0, 5.0, 0.5), 0.0); }
    #[test] fn rate_scales() { assert!(exchange(10.0, 5.0, 1.0) > exchange(10.0, 5.0, 0.5)); }
    #[test] fn magnitude_bounded() { assert!(exchange(10.0, 5.0, 0.5).abs() <= (10.0f32 - 5.0).abs()); }
    #[test] fn deterministic() { assert_eq!(exchange(10.0, 5.0, 0.5).to_bits(), exchange(10.0, 5.0, 0.5).to_bits()); }
}
