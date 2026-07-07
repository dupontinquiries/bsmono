//! `sim::claw_curl` — how a creature's toes/claws curl: gripping pulls them
//! shut, and the gait rolls a little curl through them as the foot loads and
//! unloads.
//!
//! [`claw_curl`] returns a per-toe curl (`0..1`, one entry per toe) from a
//! `grip` (`0..1`, how hard the creature is grasping), a `gait_phase` (`0..1`,
//! where the foot is in its step cycle), and the toe count. Grip dominates: at
//! full grip every toe is fully curled regardless of gait. As grip relaxes, the
//! gait term shows through — toes are spread slightly out of phase with each
//! other so the curl rolls across the foot rather than snapping in unison.
//!
//! Presentation-tier and pure: per-toe `clamp01(grip + (1-grip)·gait_i)`, a sine
//! for the gait wave — no clock, no randomness, no cross-imports. Curl is
//! monotonic in grip (the `(1-grip)` weight is non-negative) and always bounded.

use std::f32::consts::PI;

const TAU: f32 = 2.0 * PI;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Per-toe curl values, one per toe (empty if `toes == 0`).
///
/// Each toe `i` curls `clamp01(grip + (1-grip)·gait_i)`, where `gait_i` is a
/// `0..1` sine of the gait phase offset by the toe's position across the foot,
/// so grip pulls all toes shut while the gait modulates them when grip is loose.
pub fn claw_curl(grip: f32, gait_phase: f32, toes: usize) -> Vec<f32> {
    let g = clamp01(grip);
    let mut out = Vec::with_capacity(toes);
    let mut i = 0;
    while i < toes {
        let offset = i as f32 / toes as f32; // spread toes across the cycle
        let gait_i = 0.5 + 0.5 * (TAU * (gait_phase + offset)).sin();
        out.push(clamp01(g + (1.0 - g) * gait_i));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn unit(seed: &mut u32) -> f32 {
        (lcg(seed) % 100_001) as f32 / 100_000.0
    }

    #[test]
    fn full_grip_fully_curls_every_toe() {
        let mut seed = 0x6217_u32;
        for _ in 0..500 {
            let phase = unit(&mut seed);
            let toes = 1 + (lcg(&mut seed) % 6) as usize;
            for c in claw_curl(1.0, phase, toes) {
                assert_eq!(c, 1.0);
            }
        }
    }

    #[test]
    fn gait_modulates_when_grip_is_loose() {
        let a = claw_curl(0.3, 0.0, 4);
        let b = claw_curl(0.3, 0.25, 4);
        assert_ne!(a, b, "different gait phase should change the curl");
    }

    #[test]
    fn length_matches_toe_count() {
        assert_eq!(claw_curl(0.5, 0.2, 0).len(), 0);
        assert_eq!(claw_curl(0.5, 0.2, 1).len(), 1);
        assert_eq!(claw_curl(0.5, 0.2, 5).len(), 5);
        assert_eq!(claw_curl(0.5, 0.2, 12).len(), 12);
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let grip = unit(&mut seed) * 3.0 - 1.0; // include out-of-range
            let phase = unit(&mut seed) * 4.0 - 2.0;
            let toes = (lcg(&mut seed) % 9) as usize;
            for c in claw_curl(grip, phase, toes) {
                assert!(c >= 0.0 && c <= 1.0, "curl {}", c);
            }
        }
    }

    #[test]
    fn monotonic_in_grip() {
        let mut seed = 0x3303_u32;
        for _ in 0..2000 {
            let phase = unit(&mut seed);
            let toes = 1 + (lcg(&mut seed) % 6) as usize;
            let g1 = unit(&mut seed);
            let g2 = unit(&mut seed);
            let lo = g1.min(g2);
            let hi = g1.max(g2);
            let clo = claw_curl(lo, phase, toes);
            let chi = claw_curl(hi, phase, toes);
            for i in 0..toes {
                assert!(chi[i] >= clo[i] - 1e-6, "toe {} not monotone", i);
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let grip = unit(&mut seed);
            let phase = unit(&mut seed);
            let toes = (lcg(&mut seed) % 8) as usize;
            assert_eq!(
                claw_curl(grip, phase, toes),
                claw_curl(grip, phase, toes)
            );
        }
    }
}
