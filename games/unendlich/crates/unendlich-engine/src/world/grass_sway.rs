//! Grass-blade sway offset from wind, animated over time with a per-blade phase.
//! Horizontal bend in [-wind, wind]. Pure, deterministic.

use std::f32::consts::PI;

pub fn sway(wind: f32, t: f32, phase: f32) -> f32 {
    wind.max(0.0) * (2.0 * PI * 0.5 * t + phase).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn no_wind_still() { assert_eq!(sway(0.0, 1.3, 0.7), 0.0); }
    #[test] fn bounded() {
        let mut t = 0.0f32;
        while t < 8.0 { assert!(sway(2.0, t, 0.5).abs() <= 2.0 + 1e-6); t += 0.2; }
    }
    #[test] fn phase_shifts() { assert!((sway(2.0, 1.0, 0.0) - sway(2.0, 1.0, 1.5)).abs() > 1e-3); }
    #[test] fn periodic() { assert!((sway(2.0, 0.5, 0.3) - sway(2.0, 0.5 + 2.0, 0.3)).abs() < 1e-3); }
    #[test] fn oscillates() {
        let peak = sway(2.0, 0.5, 0.0);   // sin(pi/2) = +1
        let trough = sway(2.0, 1.5, 0.0); // sin(3pi/2) = -1
        assert!((peak - trough).abs() > 1e-3);
    }
    #[test] fn deterministic() { assert_eq!(sway(2.0, 1.0, 0.5).to_bits(), sway(2.0, 1.0, 0.5).to_bits()); }
}
