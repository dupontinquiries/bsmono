//! Precipitation rate from cloud coverage and humidity: needs both clouds and
//! moisture, ramping superlinearly. In [0,1]. Pure, deterministic.

pub fn rate(cloud_coverage: f32, humidity: f32) -> f32 {
    let c = cloud_coverage.clamp(0.0, 1.0);
    let h = humidity.clamp(0.0, 1.0);
    let trig = (c * h).clamp(0.0, 1.0);
    (trig * trig).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_cloud_no_rain() { assert_eq!(rate(0.0, 1.0), 0.0); }
    #[test] fn no_humidity_no_rain() { assert_eq!(rate(1.0, 0.0), 0.0); }
    #[test] fn full_both_max() { assert!((rate(1.0, 1.0) - 1.0).abs() < 1e-6); }
    #[test] fn more_cloud_more() { assert!(rate(0.9, 0.8) > rate(0.3, 0.8)); }
    #[test] fn bounded() {
        let mut c = 0.0f32;
        while c <= 1.0 { assert!((0.0..=1.0).contains(&rate(c, 0.7))); c += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(rate(0.8, 0.7).to_bits(), rate(0.8, 0.7).to_bits()); }
}
