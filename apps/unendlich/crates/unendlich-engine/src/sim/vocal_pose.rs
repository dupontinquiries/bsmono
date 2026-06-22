//! `sim::vocal_pose` — turning a sound into the pose that produces it: a dragon
//! doesn't just emit audio, it opens its jaw and lifts its neck to speak.
//!
//! [`vocal_pose`] maps a `sound_amp` (`0..1` loudness this instant) and an
//! `expressiveness` (`0..1`, how theatrically the creature moves) into a
//! [`VocalPose`] — `jaw_open` and `neck_raise`, both `0..1`. Silence yields a
//! closed, neutral pose; louder sound opens the jaw and lifts the neck more; and
//! a more expressive creature moves more for the same loudness. The neck lifts a
//! little less than the jaw opens, so speech reads as mostly mouth with a
//! supporting sway.
//!
//! Presentation-tier and pure: clamps and a couple of multiplies — no clock, no
//! randomness, no cross-imports.

/// How much the neck lifts relative to the jaw opening.
const NECK_FACTOR: f32 = 0.8;

/// The pose driven by a vocalization. Both fields are `0..1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VocalPose {
    /// How far the jaw is open.
    pub jaw_open: f32,
    /// How far the neck is raised.
    pub neck_raise: f32,
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Map loudness and expressiveness to a jaw/neck pose.
pub fn vocal_pose(sound_amp: f32, expressiveness: f32) -> VocalPose {
    let a = clamp01(sound_amp);
    let e = clamp01(expressiveness);
    let drive = a * e;
    VocalPose {
        jaw_open: clamp01(drive),
        neck_raise: clamp01(drive * NECK_FACTOR),
    }
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
    fn silence_is_closed_and_neutral() {
        let p = vocal_pose(0.0, 1.0);
        assert_eq!(p.jaw_open, 0.0);
        assert_eq!(p.neck_raise, 0.0);
    }

    #[test]
    fn louder_opens_more() {
        let quiet = vocal_pose(0.2, 1.0);
        let loud = vocal_pose(0.8, 1.0);
        assert!(loud.jaw_open > quiet.jaw_open);
        assert!(loud.neck_raise > quiet.neck_raise);
    }

    #[test]
    fn expressiveness_scales_motion() {
        let flat = vocal_pose(0.7, 0.2);
        let theatrical = vocal_pose(0.7, 0.9);
        assert!(theatrical.jaw_open > flat.jaw_open);
        assert!(theatrical.neck_raise > flat.neck_raise);
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            // include out-of-range inputs to probe the clamps
            let amp = unit(&mut seed) * 3.0 - 1.0;
            let expr = unit(&mut seed) * 3.0 - 1.0;
            let p = vocal_pose(amp, expr);
            assert!(p.jaw_open >= 0.0 && p.jaw_open <= 1.0);
            assert!(p.neck_raise >= 0.0 && p.neck_raise <= 1.0);
        }
    }

    #[test]
    fn monotonic_in_amplitude() {
        let mut seed = 0x3303_u32;
        for _ in 0..3000 {
            let e = unit(&mut seed);
            let a1 = unit(&mut seed);
            let a2 = unit(&mut seed);
            let lo = a1.min(a2);
            let hi = a1.max(a2);
            let plo = vocal_pose(lo, e);
            let phi = vocal_pose(hi, e);
            assert!(phi.jaw_open >= plo.jaw_open);
            assert!(phi.neck_raise >= plo.neck_raise);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let a = unit(&mut seed);
            let e = unit(&mut seed);
            assert_eq!(vocal_pose(a, e), vocal_pose(a, e));
        }
    }
}
