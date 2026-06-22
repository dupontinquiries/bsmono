//! Tree-line elevation: the highest altitude trees grow — rises with warmth and
//! toward the equator (low latitude_factor: 0 equator, 1 pole). Floored at 0.

pub fn elevation(base_line: f32, warmth: f32, latitude_factor: f32) -> f32 {
    let bl = base_line.max(0.0);
    let w = warmth.clamp(0.0, 1.0);
    let lat = latitude_factor.clamp(0.0, 1.0);
    (bl + 1500.0 * w - 2000.0 * lat).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn warmer_is_higher() { assert!(elevation(2000.0, 0.9, 0.5) > elevation(2000.0, 0.1, 0.5)); }
    #[test] fn pole_is_lower() { assert!(elevation(2000.0, 0.5, 0.9) < elevation(2000.0, 0.5, 0.1)); }
    #[test] fn never_negative() { assert!(elevation(0.0, 0.0, 1.0) >= 0.0); }
    #[test] fn equator_warm_high() { assert!(elevation(2000.0, 1.0, 0.0) > 2000.0); }
    #[test] fn bounded_below() {
        let mut lat = 0.0f32;
        while lat <= 1.0 { assert!(elevation(1000.0, 0.5, lat) >= 0.0); lat += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(elevation(2000.0, 0.7, 0.4).to_bits(), elevation(2000.0, 0.7, 0.4).to_bits()); }
}
