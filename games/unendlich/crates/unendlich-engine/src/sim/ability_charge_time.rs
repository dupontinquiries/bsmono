//! Charge/cast time for an ability: scales with power, reduced by haste (capped
//! at 90%), floored at 0.1s so nothing is truly instant. Pure, deterministic.

pub fn charge_time(power: f32, haste: f32, base: f32) -> f32 {
    let p = power.max(0.0);
    let h = haste.clamp(0.0, 0.9);
    let b = base.max(0.0);
    (b * (1.0 + p) * (1.0 - h)).max(0.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_power_longer() { assert!(charge_time(2.0, 0.0, 1.0) > charge_time(0.5, 0.0, 1.0)); }
    #[test] fn more_haste_shorter() { assert!(charge_time(1.0, 0.8, 1.0) < charge_time(1.0, 0.1, 1.0)); }
    #[test] fn zero_haste_full() { assert!((charge_time(1.0, 0.0, 1.0) - 2.0).abs() < 1e-5); }
    #[test] fn floored() { assert!(charge_time(0.0, 0.9, 0.1) >= 0.1); }
    #[test] fn monotone_in_haste() {
        let (mut prev, mut h) = (f32::MAX, 0.0f32);
        while h <= 0.9 { let c = charge_time(1.0, h, 1.0); assert!(c <= prev + 1e-6); prev = c; h += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(charge_time(1.0, 0.5, 1.0).to_bits(), charge_time(1.0, 0.5, 1.0).to_bits()); }
}
