//! How well head+torso are shielded by the limbs in a given facing
//! (handoff/COMBAT_STANCE.md). 0 = vitals forward/exposed, 1 = side-on (flank shields torso).

use std::f32::consts::PI;

/// `facing_rad` = mob facing, `threat_rad` = direction to the threat.
/// Head-on (aligned) ⇒ ~0; perpendicular (side-on) ⇒ ~1.
pub fn cover(facing_rad: f32, threat_rad: f32) -> f32 {
    let two_pi = 2.0 * PI;
    let mut d = (facing_rad - threat_rad).abs() % two_pi;
    if d > PI {
        d = two_pi - d;
    }
    d.sin().abs().clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn head_on_low_cover() { assert!(cover(1.0, 1.0) < 0.05); }
    #[test] fn side_on_high_cover() { assert!(cover(0.0, PI / 2.0) > 0.95); }
    #[test] fn bounded() {
        let mut a = -10.0f32;
        while a < 10.0 { let c = cover(a, 0.3); assert!((0.0..=1.0).contains(&c)); a += 0.37; }
    }
    #[test] fn periodic_2pi() {
        let c1 = cover(0.7, 0.2);
        let c2 = cover(0.7 + 2.0 * PI, 0.2);
        assert!((c1 - c2).abs() < 1e-4);
    }
    #[test] fn symmetric() { assert!((cover(0.4, 1.9) - cover(1.9, 0.4)).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(cover(0.5, 1.2).to_bits(), cover(0.5, 1.2).to_bits()); }
}
