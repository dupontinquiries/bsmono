//! Orographic rain shadow: moisture left on the lee side after air rises over a
//! peak and rains out. Higher/drier ⇒ less leeward moisture. In [0, windward].

pub fn leeward_moisture(windward_moisture: f32, peak_height: f32, dryness: f32) -> f32 {
    let m = windward_moisture.max(0.0);
    let h = peak_height.max(0.0);
    let k = dryness.max(0.0);
    m * (-(h * k)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_peak_unchanged() { assert!((leeward_moisture(0.8, 0.0, 0.001) - 0.8).abs() < 1e-6); }
    #[test] fn higher_peak_drier() { assert!(leeward_moisture(0.8, 3000.0, 0.001) < leeward_moisture(0.8, 500.0, 0.001)); }
    #[test] fn more_dryness_drier() { assert!(leeward_moisture(0.8, 1000.0, 0.002) < leeward_moisture(0.8, 1000.0, 0.0005)); }
    #[test] fn bounded_by_windward() {
        let mut h = 0.0f32;
        while h < 4000.0 { let v = leeward_moisture(0.8, h, 0.001); assert!((0.0..=0.8 + 1e-6).contains(&v)); h += 100.0; }
    }
    #[test] fn nonneg() { assert!(leeward_moisture(0.8, 10000.0, 0.01) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(leeward_moisture(0.8, 1000.0, 0.001).to_bits(), leeward_moisture(0.8, 1000.0, 0.001).to_bits()); }
}
