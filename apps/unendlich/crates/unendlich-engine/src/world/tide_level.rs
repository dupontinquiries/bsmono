//! Tidal height over time: a sinusoid about 0 with the given period and amplitude.

use std::f32::consts::PI;

pub fn level(time: f32, period: f32, amplitude: f32) -> f32 {
    let p = period.max(1e-6);
    amplitude.max(0.0) * (2.0 * PI * time / p).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_amplitude_flat() { assert_eq!(level(3.0, 12.0, 0.0), 0.0); }
    #[test] fn bounded() {
        let mut t = 0.0f32;
        while t < 50.0 { assert!(level(t, 12.0, 2.0).abs() <= 2.0 + 1e-6); t += 0.5; }
    }
    #[test] fn periodic() { assert!((level(3.0, 12.0, 2.0) - level(3.0 + 12.0, 12.0, 2.0)).abs() < 1e-3); }
    #[test] fn zero_time_zero() { assert!(level(0.0, 12.0, 2.0).abs() < 1e-6); }
    #[test] fn amplitude_scales() {
        let a = level(3.0, 12.0, 4.0); let b = level(3.0, 12.0, 2.0);
        assert!((a - 2.0 * b).abs() < 1e-4);
    }
    #[test] fn deterministic() { assert_eq!(level(3.0, 12.0, 2.0).to_bits(), level(3.0, 12.0, 2.0).to_bits()); }
}
