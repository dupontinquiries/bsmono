//! Significant wave height from wind speed and fetch (distance the wind blows
//! over open water): grows ~quadratically with wind, saturating with fetch.

pub fn significant_height(wind_speed: f32, fetch: f32) -> f32 {
    let w = wind_speed.max(0.0);
    let f = fetch.max(0.0);
    let fetch_factor = 1.0 - (-(f / 5000.0)).exp();
    0.001 * w * w * fetch_factor
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_wind_no_waves() { assert_eq!(significant_height(0.0, 5000.0), 0.0); }
    #[test] fn stronger_wind_higher() { assert!(significant_height(20.0, 5000.0) > significant_height(5.0, 5000.0)); }
    #[test] fn longer_fetch_higher() { assert!(significant_height(15.0, 10000.0) > significant_height(15.0, 1000.0)); }
    #[test] fn zero_fetch_zero() { assert_eq!(significant_height(20.0, 0.0), 0.0); }
    #[test] fn nonneg() { assert!(significant_height(20.0, 5000.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(significant_height(15.0, 5000.0).to_bits(), significant_height(15.0, 5000.0).to_bits()); }
}
