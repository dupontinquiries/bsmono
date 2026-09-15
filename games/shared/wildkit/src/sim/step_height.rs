//! step_height — how tall a lip a creature can step/clamber over, as a function
//! of body size. The engine baked a single step height for everything, so a huge
//! dragon snagged on a 1-block curb a deer also couldn't pass; this makes the
//! allowance scale with the body (and shrink on a steep approach). Composes with
//! `path_memory`'s clearance estimate and `large_body_grounding`. Reactive tier:
//! a couple of arithmetic ops, pure and deterministic.

/// Step a baseline (size-1) body can clear (blocks).
pub const BASE_STEP: f32 = 0.6;
/// Extra step allowance per unit of body size above 1.
pub const STEP_PER_SIZE: f32 = 0.55;
/// Hard ceiling — nothing clears more than this in one step.
pub const MAX_STEP: f32 = 4.0;

/// Maximum step height for a body of `size` (1 = baseline). Monotone
/// non-decreasing in size, clamped to `[BASE_STEP, MAX_STEP]`.
pub fn max_step(size: f32) -> f32 {
    let s = size.max(1.0);
    (BASE_STEP + STEP_PER_SIZE * (s - 1.0)).clamp(BASE_STEP, MAX_STEP)
}

/// Step height accounting for approach slope. `slope01` in `[0, 1]` (0 = flat
/// head-on, 1 = nearly a wall) shrinks the usable step — you can't hike a lip you
/// can barely lean into. Still monotone non-decreasing in size for any fixed
/// slope, and bounded by `max_step`.
pub fn max_step_on_slope(size: f32, slope01: f32) -> f32 {
    let factor = 1.0 - 0.6 * slope01.clamp(0.0, 1.0);
    (max_step(size) * factor).clamp(0.0, MAX_STEP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn bigger_bodies_clear_taller_lips() {
        assert!(max_step(3.0) > max_step(1.0), "a dragon out-steps a critter");
        assert!(max_step(1.0) >= BASE_STEP);
    }

    #[test]
    fn step_is_bounded() {
        assert!((max_step(1000.0) - MAX_STEP).abs() < 1e-6, "capped at MAX_STEP");
        assert!(max_step(0.1) >= BASE_STEP, "sub-baseline floored");
    }

    #[test]
    fn a_steep_approach_shrinks_the_step() {
        let flat = max_step_on_slope(2.0, 0.0);
        let steep = max_step_on_slope(2.0, 1.0);
        assert!(steep < flat, "steep approach → smaller usable step");
        assert!((flat - max_step(2.0)).abs() < 1e-6, "flat == max_step");
    }

    #[test]
    fn prop_monotone_nondecreasing_and_bounded() {
        let mut seed = 0x57E9_0001u32;
        for _ in 0..5000 {
            let a = lcg(&mut seed) * 8.0;
            let b = a + lcg(&mut seed) * 8.0; // b >= a
            assert!(max_step(b) >= max_step(a) - 1e-6, "monotone in size: {a}->{b}");
            for s in [a, b] {
                let m = max_step(s);
                assert!(m.is_finite() && m >= BASE_STEP - 1e-6 && m <= MAX_STEP + 1e-6, "bounded: {m}");
                let slope = lcg(&mut seed);
                let ms = max_step_on_slope(s, slope);
                assert!(ms.is_finite() && ms >= 0.0 && ms <= MAX_STEP + 1e-6, "sloped bounded: {ms}");
            }
        }
    }
}
