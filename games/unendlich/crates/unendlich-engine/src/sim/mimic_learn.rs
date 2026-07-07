//! `sim::mimic_learn` — how the Mimic horror *learns* a voice: not instantly,
//! but by accumulating exposure to the voices it overhears (pairs with
//! `sound_genome`, whose six-field voice this represents as a `[f32; 6]`).
//!
//! [`observe`] folds a heard voice into a [`MimicAcc`], weighted by how long it
//! was heard (`dt`). [`rendered`] reads back the voice the Mimic currently
//! produces: a blend of its blank neutral voice and the exposure-weighted mean
//! of everything it has heard, where the blend leans further toward what it has
//! heard the more total exposure it has had. So a fresh Mimic sounds neutral, a
//! briefly-exposed one only faintly like its quarry, and a long-exposed one
//! nearly indistinguishable. Voices heard more contribute more.
//!
//! Pure and deterministic: integer-free accumulation, no clock, no randomness.

/// A voice as six abstract fields (mirrors `sound_genome::Voice`'s scalars).
pub type Voice6 = [f32; 6];

/// Exposure at which the Mimic is "halfway" convinced (controls how fast the
/// rendered voice leans from neutral toward what it has heard).
pub const HALF_EXPOSURE: f32 = 2.0;

/// The Mimic's blank slate — what it sounds like before it has heard anything.
const NEUTRAL: Voice6 = [0.0; 6];

/// Accumulated exposure: an exposure-weighted running sum of heard voices.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MimicAcc {
    sum: Voice6,
    weight: f32,
}

impl MimicAcc {
    /// A fresh accumulator (no exposure).
    pub fn new() -> Self {
        MimicAcc {
            sum: NEUTRAL,
            weight: 0.0,
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    if t <= 0.0 {
        a
    } else if t >= 1.0 {
        b
    } else {
        a + (b - a) * t
    }
}

/// Fold a heard voice into the accumulator, weighted by `dt` (non-positive `dt`
/// is ignored, so exposure never runs backwards).
pub fn observe(acc: &mut MimicAcc, heard: &Voice6, dt: f32) {
    if dt <= 0.0 {
        return;
    }
    let mut i = 0;
    while i < 6 {
        acc.sum[i] += heard[i] * dt;
        i += 1;
    }
    acc.weight += dt;
}

/// The voice the Mimic currently produces.
///
/// With no exposure this is [`NEUTRAL`]. Otherwise it is the neutral voice
/// blended toward the exposure-weighted mean of everything heard, by a
/// confidence that rises from `0` toward `1` as total exposure grows.
pub fn rendered(acc: &MimicAcc) -> Voice6 {
    if acc.weight <= 0.0 {
        return NEUTRAL;
    }
    let confidence = acc.weight / (acc.weight + HALF_EXPOSURE);
    let mut out = NEUTRAL;
    let mut i = 0;
    while i < 6 {
        let mean = acc.sum[i] / acc.weight;
        out[i] = lerp(NEUTRAL[i], mean, confidence);
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
    fn rand_voice(seed: &mut u32) -> Voice6 {
        let mut v = [0.0f32; 6];
        let mut i = 0;
        while i < 6 {
            v[i] = unit(seed);
            i += 1;
        }
        v
    }
    fn dist(a: &Voice6, b: &Voice6) -> f32 {
        let mut s = 0.0f32;
        let mut i = 0;
        while i < 6 {
            let d = a[i] - b[i];
            s += d * d;
            i += 1;
        }
        s.sqrt()
    }

    #[test]
    fn fresh_acc_is_neutral() {
        let acc = MimicAcc::new();
        assert_eq!(rendered(&acc), NEUTRAL);
    }

    #[test]
    fn no_exposure_leaves_it_unchanged() {
        let mut acc = MimicAcc::new();
        let target = [0.7, 0.2, 0.9, 0.1, 0.5, 0.3];
        observe(&mut acc, &target, 0.0); // dt 0 → ignored
        observe(&mut acc, &target, -5.0); // negative → ignored
        assert_eq!(rendered(&acc), NEUTRAL);
    }

    #[test]
    fn more_exposure_moves_closer_to_target() {
        let target = [0.8, 0.6, 0.4, 0.9, 0.2, 0.5];
        let mut acc = MimicAcc::new();
        let mut prev = dist(&rendered(&acc), &target);
        let mut step = 0;
        while step < 30 {
            observe(&mut acc, &target, 1.0);
            let now = dist(&rendered(&acc), &target);
            assert!(now < prev, "did not get closer at step {}", step);
            prev = now;
            step += 1;
        }
        assert!(prev < 0.1, "should be close after long exposure, got {}", prev);
    }

    #[test]
    fn multiple_voices_blend_by_exposure() {
        let a = [1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let b = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

        // A heard three times as long as B
        let mut a_heavy = MimicAcc::new();
        observe(&mut a_heavy, &a, 3.0);
        observe(&mut a_heavy, &b, 1.0);

        // B heard three times as long as A (same total exposure)
        let mut b_heavy = MimicAcc::new();
        observe(&mut b_heavy, &a, 1.0);
        observe(&mut b_heavy, &b, 3.0);

        let ra = rendered(&a_heavy);
        let rb = rendered(&b_heavy);
        // the A-heavy mimic leans toward A more than the B-heavy one does
        assert!(ra[0] > rb[0], "A-heavy {} should exceed B-heavy {}", ra[0], rb[0]);
        assert!(dist(&ra, &a) < dist(&rb, &a), "A-heavy should sit nearer A");
    }

    #[test]
    fn rendered_is_bounded_within_observed_range() {
        let mut seed = 0x313C_u32;
        for _ in 0..3000 {
            let mut acc = MimicAcc::new();
            let n = 1 + (lcg(&mut seed) % 5);
            let mut k = 0;
            while k < n {
                let v = rand_voice(&mut seed); // components in [0,1]
                observe(&mut acc, &v, unit(&mut seed) * 3.0 + 0.01);
                k += 1;
            }
            let r = rendered(&acc);
            let mut i = 0;
            while i < 6 {
                // neutral is 0 and all heard components are in [0,1] → so is r
                assert!(r[i] >= 0.0 && r[i] <= 1.0, "component {} = {}", i, r[i]);
                i += 1;
            }
        }
    }

    #[test]
    fn deterministic() {
        fn build(seed_init: u32) -> MimicAcc {
            let mut seed = seed_init;
            let mut acc = MimicAcc::new();
            let mut k = 0;
            while k < 50 {
                let v = rand_voice(&mut seed);
                observe(&mut acc, &v, unit(&mut seed) * 2.0);
                k += 1;
            }
            acc
        }
        let a = build(0xAA55);
        let b = build(0xAA55);
        assert_eq!(a, b);
        assert_eq!(rendered(&a), rendered(&b));
    }
}
