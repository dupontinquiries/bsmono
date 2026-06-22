//! Temperature at altitude: base minus lapse-rate cooling (worldgen). Pure.

pub fn temp_at(base_temp: f32, altitude: f32, lapse_rate: f32) -> f32 {
    let alt = altitude.max(0.0);
    let lr = lapse_rate.max(0.0);
    base_temp - alt * lr
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn sea_level_is_base() { assert_eq!(temp_at(20.0, 0.0, 0.0065), 20.0); }
    #[test] fn higher_is_colder() { assert!(temp_at(20.0, 2000.0, 0.0065) < temp_at(20.0, 500.0, 0.0065)); }
    #[test] fn steeper_lapse_colder() { assert!(temp_at(20.0, 1000.0, 0.01) < temp_at(20.0, 1000.0, 0.005)); }
    #[test] fn linear_in_altitude() {
        let d1 = temp_at(20.0, 1000.0, 0.0065) - temp_at(20.0, 2000.0, 0.0065);
        let d2 = temp_at(20.0, 0.0, 0.0065) - temp_at(20.0, 1000.0, 0.0065);
        assert!((d1 - d2).abs() < 1e-3);
    }
    #[test] fn monotone() {
        let (mut prev, mut a) = (f32::MAX, 0.0f32);
        while a < 5000.0 { let t = temp_at(20.0, a, 0.0065); assert!(t <= prev + 1e-3); prev = t; a += 100.0; }
    }
    #[test] fn deterministic() { assert_eq!(temp_at(20.0, 1500.0, 0.0065).to_bits(), temp_at(20.0, 1500.0, 0.0065).to_bits()); }
}
