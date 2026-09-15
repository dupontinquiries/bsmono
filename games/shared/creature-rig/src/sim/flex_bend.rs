//! flex_bend — the S-curve flex of a chain of segments: a serpent's slither, a
//! dragon's neck arcing to track prey, a tail swishing for balance. Given a base
//! heading plus a steady `turn` (steer the chain to a side) and a `swish` drive
//! (a traveling wave along the chain), it returns a per-segment LOCAL yaw — and an
//! optional pitch droop — that the renderer lays out cumulatively from base to
//! tip. It composes with the gait phase: feed `gait_phase` as `phase` so the
//! swish syncs to the stride. Pure: writes into a caller-provided slice (no
//! per-call allocation), std-only, fully deterministic.

use std::f32::consts::PI;

/// Shape limits for a flex chain.
#[derive(Clone, Copy, Debug)]
pub struct FlexParams {
    /// Hard cap on |local angle| per segment (radians).
    pub max_bend: f32,
    /// Amplitude of the swish wave at full `swish` drive (radians).
    pub wave_amp: f32,
    /// Phase advance per segment along the chain (radians) — the wavelength.
    pub wavelength: f32,
    /// Steady bend per segment at full `turn` (radians).
    pub turn_bend: f32,
}

impl Default for FlexParams {
    fn default() -> FlexParams {
        FlexParams { max_bend: 0.5, wave_amp: 0.22, wavelength: PI / 3.0, turn_bend: 0.18 }
    }
}

/// Fill `out` with per-segment LOCAL yaw offsets (radians) for a chain of
/// `out.len()` segments, base at index 0 → tip at the end. `turn` in `[-1, 1]`
/// steers the whole chain to one side (a smooth arc); `swish` in `[0, 1]` scales
/// a traveling wave whose phase advances with `phase` (pass the gait phase); the
/// wave tapers up toward the tip so the end whips most. With
/// `turn == 0 && swish == 0` every offset is exactly `0` (a straight chain). Each
/// offset is clamped to `±max_bend`.
pub fn flex_yaw(out: &mut [f32], turn: f32, swish: f32, phase: f32, p: &FlexParams) {
    let n = out.len();
    if n == 0 {
        return;
    }
    let s = swish.clamp(0.0, 1.0);
    let t = turn.clamp(-1.0, 1.0);
    let inv_n = 1.0 / n as f32;
    for (i, slot) in out.iter_mut().enumerate() {
        let tip = (i as f32 + 1.0) * inv_n; // 0..1, larger toward the tip
        let steady = t * p.turn_bend;
        let wave = s * p.wave_amp * tip * (phase + i as f32 * p.wavelength).sin();
        *slot = (steady + wave).clamp(-p.max_bend, p.max_bend);
    }
}

/// Optional pitch: a gravity droop that grows toward the tip, plus a gentle bob in
/// time with `phase`. `droop` in `[0, 1]` is how heavily the chain hangs. Straight
/// (all `0`) when `droop == 0`. Clamped to `±max_bend`.
pub fn flex_pitch(out: &mut [f32], droop: f32, phase: f32, p: &FlexParams) {
    let n = out.len();
    if n == 0 {
        return;
    }
    let d = droop.clamp(0.0, 1.0);
    let inv_n = 1.0 / n as f32;
    for (i, slot) in out.iter_mut().enumerate() {
        let tip = (i as f32 + 1.0) * inv_n;
        let sag = -d * p.turn_bend * tip; // hangs downward toward the tip
        let bob = d * 0.05 * (phase + i as f32 * p.wavelength).sin();
        *slot = (sag + bob).clamp(-p.max_bend, p.max_bend);
    }
}

/// Turn per-segment LOCAL offsets into cumulative (base-relative) angles — the
/// running sum the renderer applies down the chain. `out.len()` segments; reads
/// `local` up to its length. Straight in ⇒ straight out.
pub fn cumulate(local: &[f32], out: &mut [f32]) {
    let mut acc = 0.0;
    for (i, slot) in out.iter_mut().enumerate() {
        if i < local.len() {
            acc += local[i];
        }
        *slot = acc;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn zero_input_is_a_straight_chain() {
        let p = FlexParams::default();
        let mut yaw = [9.0_f32; 6];
        flex_yaw(&mut yaw, 0.0, 0.0, 0.0, &p);
        assert!(yaw.iter().all(|&v| v == 0.0), "no drive gives a straight chain: {yaw:?}");
        let mut pitch = [9.0_f32; 6];
        flex_pitch(&mut pitch, 0.0, 1.23, &p);
        assert!(pitch.iter().all(|&v| v == 0.0), "no droop gives a straight chain");
        let mut cum = [9.0_f32; 6];
        cumulate(&yaw, &mut cum);
        assert!(cum.iter().all(|&v| v == 0.0), "cumulative of a straight chain is straight");
    }

    #[test]
    fn turn_is_left_right_symmetric() {
        let p = FlexParams::default();
        let (mut l, mut r) = ([0.0_f32; 8], [0.0_f32; 8]);
        flex_yaw(&mut l, 0.7, 0.0, 0.0, &p); // swish off → pure steer
        flex_yaw(&mut r, -0.7, 0.0, 0.0, &p);
        for i in 0..8 {
            assert!((l[i] + r[i]).abs() < 1e-6, "seg {i}: {} vs {}", l[i], r[i]);
        }
    }

    #[test]
    fn a_steady_turn_arcs_one_way() {
        let p = FlexParams::default();
        let mut yaw = [0.0_f32; 8];
        flex_yaw(&mut yaw, 1.0, 0.0, 0.0, &p);
        let mut cum = [0.0_f32; 8];
        cumulate(&yaw, &mut cum);
        for i in 1..8 {
            assert!(cum[i] >= cum[i - 1] - 1e-6, "the arc is monotone");
        }
        assert!(cum[7] > 0.0, "it arcs toward +turn");
    }

    #[test]
    fn the_tip_swishes_more_than_the_base() {
        let p = FlexParams::default();
        let mut yaw = [0.0_f32; 8];
        // phase chosen so sin is positive and rising across the chain
        flex_yaw(&mut yaw, 0.0, 1.0, PI / 6.0, &p);
        let base_amp = yaw[0].abs();
        let tip_amp = yaw[7].abs();
        assert!(tip_amp > base_amp, "tip {tip_amp} should whip more than base {base_amp}");
    }

    #[test]
    fn prop_flex_stays_finite_and_bounded() {
        // Randomized inputs AND params: the model must never produce NaN/Inf and
        // must respect max_bend.
        let mut seed = 0x9E37_79B9u32;
        for _ in 0..5000 {
            let p = FlexParams {
                max_bend: 0.1 + lcg(&mut seed) * 1.5,
                wave_amp: lcg(&mut seed) * 1.0,
                wavelength: lcg(&mut seed) * PI,
                turn_bend: lcg(&mut seed) * 0.5,
            };
            let n = 1 + (lcg(&mut seed) * 16.0) as usize;
            let mut yaw = vec![0.0_f32; n];
            let mut pitch = vec![0.0_f32; n];
            flex_yaw(&mut yaw, lcg(&mut seed) * 6.0 - 3.0, lcg(&mut seed) * 2.0 - 0.5, lcg(&mut seed) * 40.0 - 20.0, &p);
            flex_pitch(&mut pitch, lcg(&mut seed) * 2.0 - 0.5, lcg(&mut seed) * 40.0 - 20.0, &p);
            let mut cum = vec![0.0_f32; n];
            cumulate(&yaw, &mut cum);
            for v in yaw.iter().chain(pitch.iter()).chain(cum.iter()) {
                assert!(v.is_finite(), "all outputs finite");
            }
            for &v in yaw.iter().chain(pitch.iter()) {
                assert!(v.abs() <= p.max_bend + 1e-6, "bounded by max_bend: {v} > {}", p.max_bend);
            }
        }
    }
}
