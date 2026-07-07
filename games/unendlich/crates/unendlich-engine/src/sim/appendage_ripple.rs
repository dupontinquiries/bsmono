//! `sim::appendage_ripple` — cheap secondary motion: the lag and jiggle of ears,
//! tails, fronds, and loose flesh that trail a creature's movement.
//!
//! [`ripple`] returns a signed positional offset for a soft appendage given the
//! creature's speed `vel`, the appendage's `stiffness`, and time `t`. Faster
//! movement throws the appendage further; a stiffer appendage swings less and
//! oscillates faster (a stiff spring settles quicker). At zero velocity there is
//! nothing to trail, so the offset is ~0.
//!
//! Presentation-tier and pure: amplitude `~|vel|/(1+stiffness)` (clamped),
//! frequency `~(1+stiffness)`, shape a sine of `t` — no clock, no randomness, no
//! cross-imports. Output is bounded so a huge velocity can't fling an ear off.

const AMP_SCALE: f32 = 0.1;
const MAX_AMP: f32 = 1.0;
const BASE_FREQ: f32 = 4.0;

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// Secondary-motion offset for a soft appendage.
///
/// Amplitude grows with `|vel|` and shrinks with `stiffness`; frequency grows
/// with `stiffness`. The result is `amp * sin(freq * t)`, bounded to
/// `[-MAX_AMP, MAX_AMP]`.
pub fn ripple(vel: f32, stiffness: f32, t: f32) -> f32 {
    let s = if stiffness > 0.0 { stiffness } else { 0.0 };
    let amp = clampf(vel.abs() * AMP_SCALE / (1.0 + s), 0.0, MAX_AMP);
    let freq = BASE_FREQ * (1.0 + s);
    clampf(amp * (freq * t).sin(), -MAX_AMP, MAX_AMP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    // peak |ripple| over a dense time sweep ≈ the amplitude envelope
    fn envelope(vel: f32, stiffness: f32) -> f32 {
        let mut m = 0.0f32;
        let mut i = 0;
        while i < 4000 {
            let t = i as f32 * 0.01;
            let v = ripple(vel, stiffness, t).abs();
            if v > m {
                m = v;
            }
            i += 1;
        }
        m
    }

    // zero crossings over a fixed window — a proxy for oscillation frequency
    fn crossings(vel: f32, stiffness: f32) -> u32 {
        let mut count = 0;
        let mut prev = ripple(vel, stiffness, 0.0);
        let mut i = 1;
        while i < 2000 {
            let t = i as f32 * 0.01;
            let cur = ripple(vel, stiffness, t);
            if (prev < 0.0 && cur >= 0.0) || (prev > 0.0 && cur <= 0.0) {
                count += 1;
            }
            prev = cur;
            i += 1;
        }
        count
    }

    #[test]
    fn zero_velocity_is_near_zero() {
        let mut i = 0;
        while i < 1000 {
            let t = i as f32 * 0.05;
            assert!(ripple(0.0, 0.5, t).abs() < 1e-6);
            i += 1;
        }
    }

    #[test]
    fn faster_swings_larger() {
        assert!(envelope(5.0, 0.5) > envelope(1.0, 0.5));
        assert!(envelope(2.0, 0.5) > envelope(0.5, 0.5));
    }

    #[test]
    fn stiffer_swings_smaller_and_faster() {
        // smaller amplitude
        assert!(envelope(3.0, 4.0) < envelope(3.0, 0.5), "stiffer should swing less");
        // and oscillates faster (more zero crossings in the same window)
        assert!(crossings(3.0, 4.0) > crossings(3.0, 0.5), "stiffer should be faster");
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let vel = (lcg(&mut seed) % 200_001) as f32 / 100.0 - 1000.0; // huge range
            let stiff = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            let t = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let r = ripple(vel, stiff, t);
            assert!(r >= -MAX_AMP && r <= MAX_AMP, "offset {}", r);
        }
    }

    #[test]
    fn oscillates() {
        // for a moving appendage the offset takes both signs over time
        let mut saw_pos = false;
        let mut saw_neg = false;
        let mut i = 0;
        while i < 4000 {
            let t = i as f32 * 0.01;
            let r = ripple(4.0, 0.5, t);
            if r > 1e-4 {
                saw_pos = true;
            }
            if r < -1e-4 {
                saw_neg = true;
            }
            i += 1;
        }
        assert!(saw_pos && saw_neg, "did not oscillate");
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let vel = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let stiff = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            let t = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            assert_eq!(ripple(vel, stiff, t), ripple(vel, stiff, t));
        }
    }
}
