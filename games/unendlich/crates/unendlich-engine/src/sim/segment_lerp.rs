//! `sim::segment_lerp` — how much a body section is free to flex to follow the
//! terrain. A long creature's midsection sags and adapts most where it spans
//! *between* its planted legs, and even more where it overhangs a cliff with
//! nothing beneath it.
//!
//! [`seg_amp`] returns a `0..1` follow amplitude from the section's distance to
//! the nearest supporting leg and an air-gap (`cliff_gap`) factor. Right over a
//! leg the section is held firm (low amplitude); out at mid-span the nearest leg
//! is far, so it is free to flex (high amplitude); a cliff gap boosts it further.
//! Both inputs push the amplitude up monotonically, and the result is always
//! bounded to `0..1`.
//!
//! Presentation-tier and pure: a saturating distance term plus a clamped cliff
//! term — no clock, no randomness, no cross-imports.

/// Distance at which the leg-support term reaches half of its range.
const LEG_HALF: f32 = 1.0;
/// How much a full air-gap adds to the amplitude.
const CLIFF_WEIGHT: f32 = 0.5;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Terrain-follow amplitude (`0..1`) for a body section.
///
/// `dist_to_leg` (clamped `>= 0`) raises amplitude with a saturating curve that
/// is `0` at a leg and approaches `1` far from any leg; `cliff_gap` (clamped
/// `0..1`) adds up to `CLIFF_WEIGHT` on top. The sum is clamped to `0..1`.
pub fn seg_amp(dist_to_leg: f32, cliff_gap: f32) -> f32 {
    let d = if dist_to_leg > 0.0 { dist_to_leg } else { 0.0 };
    let leg_term = d / (d + LEG_HALF); // 0 at a leg → →1 far away
    let cliff_term = clamp01(cliff_gap) * CLIFF_WEIGHT;
    clamp01(leg_term + cliff_term)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn at_leg_is_low() {
        assert_eq!(seg_amp(0.0, 0.0), 0.0);
        assert!(seg_amp(0.05, 0.0) < 0.1);
    }

    #[test]
    fn mid_span_is_high() {
        // far from any leg → large follow amplitude
        assert!(seg_amp(10.0, 0.0) > 0.8);
        // and farther is higher than nearer
        assert!(seg_amp(5.0, 0.0) > seg_amp(0.5, 0.0));
    }

    #[test]
    fn cliff_boosts() {
        let no_cliff = seg_amp(1.0, 0.0);
        let cliff = seg_amp(1.0, 1.0);
        assert!(cliff > no_cliff, "{} !> {}", cliff, no_cliff);
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let d = (lcg(&mut seed) % 200_001) as f32 / 1000.0 - 50.0; // incl negative
            let gap = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 0.5;
            let a = seg_amp(d, gap);
            assert!(a >= 0.0 && a <= 1.0, "amp {}", a);
        }
    }

    #[test]
    fn monotone_in_both_inputs() {
        let mut seed = 0x3303_u32;
        for _ in 0..3000 {
            let gap = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let d1 = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let d2 = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let lo = d1.min(d2);
            let hi = d1.max(d2);
            // monotone (non-decreasing) in distance
            assert!(seg_amp(hi, gap) >= seg_amp(lo, gap) - 1e-6);
            // monotone in cliff gap
            let g1 = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let g2 = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let glo = g1.min(g2);
            let ghi = g1.max(g2);
            assert!(seg_amp(lo, ghi) >= seg_amp(lo, glo) - 1e-6);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let d = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let gap = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(seg_amp(d, gap), seg_amp(d, gap));
        }
    }
}
