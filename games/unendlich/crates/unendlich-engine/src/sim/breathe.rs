//! `sim::breathe` — the slow chest/torso swell that makes an idle creature look
//! alive, and a dragon look enormous as it draws breath.
//!
//! [`breathe`] advances a breathing `phase` (a `0..1` cycle position) by
//! `rate * dt` and returns the new phase together with a `torso_scale` that
//! oscillates smoothly between `1 - depth` and `1 + depth` over the cycle. The
//! phase wraps seamlessly at `1.0` (the oscillation is a sine, so `phase 0` and
//! `phase 1` give the same scale), which keeps the animation continuous with no
//! pop at the loop point.
//!
//! Presentation-tier and pure: a sine, a wrap, and a clamp — no clock, no
//! randomness, no cross-imports; the same inputs always give the same output.

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

/// Torso scale at a given phase for a given (already-clamped) depth.
#[inline]
fn scale_at(phase: f32, depth: f32) -> f32 {
    1.0 + depth * (TAU * phase).sin()
}

/// Advance the breathing cycle one step.
///
/// Returns `(new_phase, torso_scale)`. `new_phase = (phase + rate*dt) mod 1`,
/// and `torso_scale = 1 + depth*sin(2π·new_phase)`, with `depth` clamped to
/// `[0,1]` so the torso never inverts. `rate = 0` (or `dt = 0`) freezes the
/// cycle in place.
pub fn breathe(phase: f32, depth: f32, rate: f32, dt: f32) -> (f32, f32) {
    let depth = clamp01(depth);
    let new_phase = (phase + rate * dt).rem_euclid(1.0);
    (new_phase, scale_at(new_phase, depth))
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
    fn scale_stays_within_depth_band() {
        let mut seed = 0xB3EA_u32;
        for _ in 0..4000 {
            let phase = unit(&mut seed);
            let depth = unit(&mut seed); // [0,1]
            let rate = unit(&mut seed) * 10.0 - 5.0;
            let dt = unit(&mut seed) * 2.0;
            let (_, scale) = breathe(phase, depth, rate, dt);
            assert!(scale >= 1.0 - depth - 1e-5 && scale <= 1.0 + depth + 1e-5);
            assert!(scale >= 0.0); // never inverts
        }
    }

    #[test]
    fn phase_advances_by_rate_times_dt_mod_one() {
        let (np, _) = breathe(0.2, 0.5, 1.0, 0.3);
        assert!((np - 0.5).abs() < 1e-6, "got {}", np);
        // wrapping past 1.0
        let (np2, _) = breathe(0.9, 0.5, 1.0, 0.3); // 1.2 -> 0.2
        assert!((np2 - 0.2).abs() < 1e-5, "got {}", np2);
    }

    #[test]
    fn depth_zero_gives_unit_scale() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let phase = unit(&mut seed);
            let (_, scale) = breathe(phase, 0.0, unit(&mut seed) * 4.0, unit(&mut seed));
            assert_eq!(scale, 1.0);
        }
    }

    #[test]
    fn rate_zero_is_frozen() {
        let phase = 0.37;
        let (np, s0) = breathe(phase, 0.3, 0.0, 1.0);
        assert!((np - phase).abs() < 1e-6);
        // and re-evaluating leaves it identical
        let (np2, s1) = breathe(np, 0.3, 0.0, 5.0);
        assert_eq!((np, s0), (np2, s1));
    }

    #[test]
    fn continuous_across_the_phase_wrap() {
        // scale just before the wrap vs just after a tiny advance over it
        let before = breathe(0.9995, 0.2, 0.0, 0.0).1; // frozen → scale at 0.9995
        let after = breathe(0.9995, 0.2, 1.0, 0.001).1; // advances to ~0.0005
        assert!((after - before).abs() < 0.02, "discontinuity: {} -> {}", before, after);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let phase = unit(&mut seed);
            let depth = unit(&mut seed);
            let rate = unit(&mut seed) * 6.0;
            let dt = unit(&mut seed);
            assert_eq!(
                breathe(phase, depth, rate, dt),
                breathe(phase, depth, rate, dt)
            );
        }
    }
}
