//! Cloud-shadow ground-darkening factor [0,1]: driven by coverage with a slow
//! drift over time so shadows move. 1 = full sun, lower = shadowed. Pure.

use std::f32::consts::PI;

pub fn factor(coverage: f32, t: f32, drift_speed: f32) -> f32 {
    let cov = coverage.clamp(0.0, 1.0);
    let phase = (2.0 * PI * drift_speed * t).sin() * 0.5 + 0.5;
    (1.0 - cov * phase).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn clear_full_sun() { assert_eq!(factor(0.0, 1.0, 0.1), 1.0); }
    #[test] fn full_coverage_darkens() { assert!(factor(1.0, 2.5, 0.1) < 1.0); }
    #[test] fn drifts_over_time() { assert!((factor(0.8, 0.0, 0.1) - factor(0.8, 2.5, 0.1)).abs() > 1e-3); }
    #[test] fn more_coverage_darker() { assert!(factor(0.8, 2.5, 0.1) < factor(0.2, 2.5, 0.1)); }
    #[test] fn bounded() { let mut t = 0.0f32; while t < 10.0 { assert!((0.0..=1.0).contains(&factor(0.6, t, 0.1))); t += 0.3; } }
    #[test] fn deterministic() { assert_eq!(factor(0.6, 1.0, 0.1).to_bits(), factor(0.6, 1.0, 0.1).to_bits()); }
}
