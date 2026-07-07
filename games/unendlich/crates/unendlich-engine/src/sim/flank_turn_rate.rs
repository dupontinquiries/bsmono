//! Angular speed to rotate toward the chosen stance: snappier under threat,
//! damped as the angular error shrinks so it doesn't overshoot
//! (handoff/COMBAT_STANCE.md).

pub fn turn_rate(threat: f32, ang_err: f32, max_rate: f32) -> f32 {
    let t = threat.clamp(0.0, 1.0);
    let err = ang_err.abs();
    let m = max_rate.max(0.0);
    let damp = err / (err + 0.2); // 0 at err=0, →1 as err grows
    (m * (0.3 + 0.7 * t) * damp).clamp(0.0, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_threat_faster() { assert!(turn_rate(0.9, 1.0, 5.0) > turn_rate(0.1, 1.0, 5.0)); }
    #[test] fn near_zero_err_damped() { assert!(turn_rate(1.0, 0.001, 5.0) < 0.1); }
    #[test] fn capped_at_max() {
        let mut e = 0.0f32;
        while e < 20.0 { assert!(turn_rate(1.0, e, 5.0) <= 5.0 + 1e-6); e += 0.5; }
    }
    #[test] fn zero_err_zero_rate() { assert!(turn_rate(1.0, 0.0, 5.0).abs() < 1e-6); }
    #[test] fn monotone_in_threat() {
        let (mut prev, mut t) = (f32::MIN, 0.0f32);
        while t <= 1.0 { let r = turn_rate(t, 1.0, 5.0); assert!(r >= prev); prev = r; t += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(turn_rate(0.7, 0.5, 5.0).to_bits(), turn_rate(0.7, 0.5, 5.0).to_bits()); }
}
