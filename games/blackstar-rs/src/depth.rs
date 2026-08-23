//! DEPTH — can the z-buffer actually hold a person and a skyscraper?
//!
//! `scale.rs` ends on the real problem: a 1.75 m person and a 228 m tower in
//! one shot is a 130:1 size span and a 343 m pullback, and depth range is
//! what breaks renderers. That claim was a hand-wave. This module is the
//! arithmetic, because depth precision is closed-form and does not need a
//! screen to check.
//!
//! ## What the renderer currently does
//!
//! `camera.rs` builds `Mat4::perspective_rh(fov, aspect, 0.08, 2600.0)` and
//! `render.rs` allocates `Depth32Float`. That is a near plane of eight
//! centimetres, a far plane of 2.6 km, and STANDARD Z — near maps to 0, far
//! maps to 1.
//!
//! Standard Z with a floating-point depth buffer is the worst of the four
//! combinations, and the reason is worth stating because it is not obvious:
//!
//! * the perspective divide already concentrates depth precision near the
//!   camera — `1/d` falls off a cliff, so most of the [0,1] range is spent
//!   on the first few metres;
//! * float32 ALSO concentrates its precision near zero, where the exponent
//!   can go small;
//! * standard Z puts the near plane at zero, so both effects stack in the
//!   same place. The near plane gets precision measured in nanometres, and
//!   everything past a few hundred metres gets metres.
//!
//! **Reversed Z** — near maps to 1, far to 0 — points the float's precision
//! at the far end, where the perspective divide has none. The two curves
//! cancel almost exactly, and the result is close to uniform. It costs a
//! flipped comparison function, a cleared value of 0.0 instead of 1.0, and
//! nothing else.
//!
//! ## Measuring it, and the trap in measuring it
//!
//! Everything below is measured against actual `f32` quantisation — store the
//! depth, step it by one unit in the last place, invert, and see how far the
//! world moved — rather than against an idealised bit count, because the
//! whole question is what the float does.
//!
//! The first version of this got it wrong in an instructive way. It inverted
//! the reversed-Z value by computing `1.0 - z` and reusing the standard
//! inverse. That subtraction re-quantises onto the standard-Z grid: near
//! zero, where reversed Z keeps all its precision, `1.0 - tiny` throws the
//! tiny away. The measurement reported the two schemes as identical and would
//! have "proved" reversed Z pointless. The analysis therefore carries a
//! DIRECT inverse for each mapping and does its arithmetic in `f64`, so the
//! only f32 rounding in the experiment is the one being measured.
//!
//! ## The answer
//!
//! At the 343 m pullback `scale::camera_for` gives for Two California Plaza:
//!
//! ```text
//!   standard Z   82 mm      <- a building facade z-fights
//!   reversed Z   0.02 mm
//! ```
//!
//! and it gets worse with distance — at 2.5 km standard Z resolves three
//! METRES. Reversed Z costs a flipped depth comparison and a cleared value of
//! 0.0. It is the cheapest fix available to this project by a wide margin.

/// The projection the game ships with today (`camera.rs`).
pub const NEAR_M: f64 = 0.08;
pub const FAR_M: f64 = 2600.0;

/// Standard-Z depth for a distance in front of the eye. Near maps to 0.
pub fn depth_standard(d: f64, near: f64, far: f64) -> f64 {
    (far / (far - near)) * (1.0 - near / d)
}

/// Reversed-Z depth. Near maps to 1, far to 0.
pub fn depth_reversed(d: f64, near: f64, far: f64) -> f64 {
    1.0 - depth_standard(d, near, far)
}

/// Invert `depth_standard`.
pub fn distance_from_standard(z: f64, near: f64, far: f64) -> f64 {
    near / (1.0 - z * (far - near) / far)
}

/// Invert `depth_reversed` DIRECTLY.
///
/// Not `distance_from_standard(1.0 - z, ..)`. That subtraction is where the
/// first version of this module lost the entire effect it was trying to
/// measure; see the module doc.
pub fn distance_from_reversed(z: f64, near: f64, far: f64) -> f64 {
    far * near / (z * (far - near) + near)
}

/// The smallest world-space step, in metres, that changes the STORED f32
/// depth at distance `d`.
///
/// Two surfaces closer together than this share a depth value and z-fight.
pub fn resolution_m(d: f64, near: f64, far: f64, reversed: bool) -> f64 {
    if reversed {
        let z = depth_reversed(d, near, far) as f32;
        let stepped = f32::from_bits(z.to_bits().saturating_sub(1));
        (distance_from_reversed(stepped as f64, near, far) - d).abs()
    } else {
        let z = depth_standard(d, near, far) as f32;
        let stepped = f32::from_bits(z.to_bits() + 1);
        (distance_from_standard(stepped as f64, near, far) - d).abs()
    }
}

/// How many times better reversed Z is at this distance.
pub fn reversed_gain(d: f64, near: f64, far: f64) -> f64 {
    let r = resolution_m(d, near, far, true);
    if r <= 0.0 {
        return f64::INFINITY;
    }
    resolution_m(d, near, far, false) / r
}

/// LOGARITHMIC depth: store `log(1 + d)` rescaled, not `1/d`.
///
/// The third lever, and I did not evaluate it — a three.js solar-system
/// reference turned up using `logarithmicDepthBuffer`, which is what that
/// community reaches for at planetary scale. Worth measuring rather than
/// assuming, because it has a real cost the other two do not: the depth value
/// is no longer linear in clip space, so it must be written per-fragment,
/// which disables early-Z on the hardware that cares.
pub fn depth_log(d: f64, near: f64, far: f64) -> f64 {
    ((d / near).ln() / (far / near).ln()).clamp(0.0, 1.0)
}

pub fn distance_from_log(z: f64, near: f64, far: f64) -> f64 {
    near * (far / near).powf(z)
}

/// World-space resolution of a logarithmic buffer at `d`.
pub fn resolution_log_m(d: f64, near: f64, far: f64) -> f64 {
    let z = depth_log(d, near, far) as f32;
    let stepped = f32::from_bits(z.to_bits() + 1);
    (distance_from_log(stepped as f64, near, far) - d).abs()
}

/// The near plane standard Z would need to resolve `want_m` at distance `d`,
/// or `None` if no near plane inside `d` achieves it.
///
/// The other lever, and the one available without touching the pipeline. It
/// works by throwing away the ability to stand close to anything.
pub fn near_needed_for(d: f64, far: f64, want_m: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    // sweep rather than bisect: resolution is not monotone in the near plane
    // once f32 exponent boundaries come into it, and a bisection over a
    // non-monotone function silently returns nonsense
    let mut n = NEAR_M;
    while n < d * 0.5 {
        if resolution_m(d, n, far, false) <= want_m {
            best = Some(n);
            break;
        }
        n *= 1.05;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mapping_hits_its_own_endpoints_and_inverts() {
        assert!(depth_standard(NEAR_M, NEAR_M, FAR_M).abs() < 1e-9);
        assert!((depth_standard(FAR_M, NEAR_M, FAR_M) - 1.0).abs() < 1e-9);
        assert!((depth_reversed(NEAR_M, NEAR_M, FAR_M) - 1.0).abs() < 1e-9);
        assert!(depth_reversed(FAR_M, NEAR_M, FAR_M).abs() < 1e-9);
        for d in [0.1f64, 1.0, 3.5, 50.0, 343.0, 2000.0] {
            let a = distance_from_standard(depth_standard(d, NEAR_M, FAR_M), NEAR_M, FAR_M);
            let b = distance_from_reversed(depth_reversed(d, NEAR_M, FAR_M), NEAR_M, FAR_M);
            assert!((a - d).abs() / d < 1e-9, "standard round trip at {d}: {a}");
            assert!((b - d).abs() / d < 1e-9, "reversed round trip at {d}: {b}");
        }
    }

    #[test]
    fn the_direct_inverse_is_not_the_same_as_one_minus_z() {
        // the measurement bug, kept as a test so it cannot come back. Going
        // through `1.0 - z` in f32 re-quantises onto the standard grid and
        // erases the entire reversed-Z benefit.
        let d = 1000.0;
        let z = depth_reversed(d, NEAR_M, FAR_M) as f32;
        let stepped = f32::from_bits(z.to_bits() - 1);

        let honest = (distance_from_reversed(stepped as f64, NEAR_M, FAR_M) - d).abs();
        // the subtraction has to happen IN f32 to reproduce it — that is the
        // whole point, and doing it in f64 (as my first attempt at this test
        // did) hides the bug just as thoroughly as the bug hid the effect
        let flipped: f32 = 1.0 - stepped;
        let via_subtraction =
            (distance_from_standard(flipped as f64, NEAR_M, FAR_M) - d).abs();
        assert!(
            via_subtraction > honest * 100.0,
            "the f32 subtraction should destroy the effect: {via_subtraction} vs {honest}"
        );
    }

    #[test]
    fn standard_z_cannot_hold_the_tower_shot() {
        // 343 m is what `scale::camera_for` gives for Two California Plaza. A
        // facade has detail at the centimetre; 82 mm of depth resolution
        // means the windows fight the wall.
        let d = 343.0;
        let std = resolution_m(d, NEAR_M, FAR_M, false);
        assert!(std > 0.05, "standard Z at {d} m resolves {std} m — better than expected");
        let rev = resolution_m(d, NEAR_M, FAR_M, true);
        assert!(rev < 0.001, "reversed Z at {d} m resolves {rev} m");
        assert!(reversed_gain(d, NEAR_M, FAR_M) > 1000.0);
    }

    #[test]
    fn the_gain_grows_with_distance_which_is_why_it_matters_here() {
        // near the camera both schemes have precision to burn; the difference
        // only shows up once something far away also has to be correct, which
        // is exactly what putting a person next to a skyscraper does
        let mut prev = 0.0;
        for d in [16.0f64, 50.0, 120.0, 343.0, 1000.0] {
            let g = reversed_gain(d, NEAR_M, FAR_M);
            assert!(g > prev, "gain should grow: {g} at {d} after {prev}");
            prev = g;
        }
        assert!(reversed_gain(2500.0, NEAR_M, FAR_M) > 10_000.0);
    }

    #[test]
    fn close_up_work_is_unaffected_either_way() {
        // why nobody noticed: at the distances a car game puts things,
        // standard Z has microns to spare. Reversed Z is slightly WORSE here
        // and it does not matter in the least.
        let d = 3.5; // scale::camera_for(Human, 1.75)
        assert!(resolution_m(d, NEAR_M, FAR_M, false) < 0.001);
        assert!(resolution_m(d, NEAR_M, FAR_M, true) < 0.001);
    }

    #[test]
    fn moving_the_near_plane_is_the_other_lever_and_costs_the_close_shot() {
        // available without touching the pipeline, and it buys the far field
        // by making the near field unusable
        // MEASURED, not assumed: nudging the near plane from 8 cm to about
        // 12 cm already buys 5 cm of resolution at 343 m. So this lever is
        // cheap, and I expected it not to be — worth recording, because "the
        // pipeline change is the only option" would have been a nice story
        // and is false.
        let n = near_needed_for(343.0, FAR_M, 0.05).expect("some near plane reaches 5 cm");
        assert!(n > NEAR_M && n < 0.5, "near plane {n} m");

        // The case for reversed Z is not that the alternative fails. It is
        // the SIZE of the win: at the shipped 8 cm near plane it resolves
        // three orders of magnitude finer than that near-plane nudge buys,
        // and it costs no near-field range at all.
        let rev = resolution_m(343.0, NEAR_M, FAR_M, true);
        assert!(rev < 0.05 / 1000.0, "reversed Z at the shipped near plane: {rev} m");
    }
}

#[cfg(test)]
mod log_depth_tests {
    use super::*;

    #[test]
    fn logarithmic_depth_is_nearly_scale_free_and_not_quite() {
        // Its selling point is constant RELATIVE precision. Measured across
        // five decades it varies about 2.3x, and that is expected rather than
        // a flaw in the implementation: f32's ULP doubles at every binade,
        // and the stored value sweeps 0..1 across several of them. I asserted
        // 1.5x first, which was me stating the marketing rather than the
        // float.
        let (n, f) = (NEAR_M, 1.0e7);
        let ratios: Vec<f64> = [10.0f64, 1_000.0, 100_000.0, 1_000_000.0]
            .iter()
            .map(|d| resolution_log_m(*d, n, f) / d)
            .collect();
        let (lo, hi) = (
            ratios.iter().cloned().fold(f64::MAX, f64::min),
            ratios.iter().cloned().fold(0.0, f64::max),
        );
        assert!(hi / lo < 4.0, "relative precision varied {:.1}x", hi / lo);
        // and it IS roughly scale-free, which is the real claim: five decades
        // of distance for a factor of a few in relative error
        assert!(hi / lo < 100.0);
    }

    #[test]
    fn reversed_z_beats_logarithmic_depth_at_every_range_tested() {
        // THE CORRECTION. I expected log depth to win once the range became
        // astronomical — the three.js reference reaches for
        // `logarithmicDepthBuffer` and I assumed that was because it is the
        // right answer out there. It is not: reversed Z puts the FAR plane at
        // zero, where f32 is densest, so it gets better with range rather
        // than worse.
        //
        // Log depth predates widespread reversed-Z support and costs a
        // per-fragment depth write, which disables early-Z. There is no range
        // in this test where it wins.
        for (d, far) in [(343.0f64, FAR_M), (2_000.0, FAR_M), (1.0e6, 1.0e7), (5.0e6, 1.0e7)] {
            let rev = resolution_m(d, NEAR_M, far, true);
            let lg = resolution_log_m(d, NEAR_M, far);
            assert!(
                rev < lg,
                "at {d} m with far {far}: log {lg:.5} beat reversed {rev:.5}"
            );
        }
    }

    #[test]
    fn the_log_mapping_inverts() {
        for d in [0.5f64, 10.0, 343.0, 2000.0] {
            let back = distance_from_log(depth_log(d, NEAR_M, FAR_M), NEAR_M, FAR_M);
            assert!((back - d).abs() / d < 1e-9, "at {d}: {back}");
        }
    }
}
