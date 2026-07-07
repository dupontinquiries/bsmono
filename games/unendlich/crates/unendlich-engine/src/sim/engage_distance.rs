//! Preferred standoff distance: more distance when vitals are exposed or the
//! threat is high; floored at melee reach (handoff/COMBAT_STANCE.md).

pub fn standoff(vital_exposure: f32, threat: f32, base: f32, melee: f32) -> f32 {
    let e = vital_exposure.clamp(0.0, 1.0);
    let t = threat.clamp(0.0, 1.0);
    let d = base.max(0.0) * (1.0 + e + 0.5 * t);
    d.max(melee.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_exposure_farther() { assert!(standoff(0.9, 0.5, 10.0, 2.0) > standoff(0.1, 0.5, 10.0, 2.0)); }
    #[test] fn more_threat_farther() { assert!(standoff(0.5, 0.9, 10.0, 2.0) > standoff(0.5, 0.1, 10.0, 2.0)); }
    #[test] fn floored_at_melee() { assert_eq!(standoff(0.0, 0.0, 1.0, 9.0), 9.0); }
    #[test] fn base_at_zero() { assert!((standoff(0.0, 0.0, 12.0, 0.0) - 12.0).abs() < 1e-6); }
    #[test] fn monotone_in_exposure() {
        let (mut prev, mut e) = (f32::MIN, 0.0f32);
        while e <= 1.0 { let s = standoff(e, 0.3, 10.0, 1.0); assert!(s >= prev); prev = s; e += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(standoff(0.4, 0.6, 8.0, 2.0).to_bits(), standoff(0.4, 0.6, 8.0, 2.0).to_bits()); }
}
