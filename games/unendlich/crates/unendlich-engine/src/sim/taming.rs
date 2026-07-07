//! `sim::taming` — how much a creature warms to being tamed when you feed it.
//!
//! [`progress`] advances a taming meter (`0` wild .. `1` tamed) by one feed
//! attempt: the gain is the creature's `affinity` for you times the
//! `feed_quality` of the offering, added to the current progress and clamped to
//! `[0, 1]`. A higher affinity or a better treat tames faster; feeding with zero
//! affinity or zero-quality food makes no progress; once the meter reaches 1 the
//! creature is tamed.
//!
//! Pure and deterministic: a multiply-add and a clamp — no clock, no randomness,
//! no cross-imports.

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}
fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Taming progress after one feed.
///
/// `clamp01(cur + affinity·feed_quality)`, with `affinity` and `feed_quality`
/// floored at 0.
pub fn progress(cur: f32, affinity: f32, feed_quality: f32) -> f32 {
    clamp01(clamp01(cur) + nonneg(affinity) * nonneg(feed_quality))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn higher_affinity_tames_faster() {
        let lo = progress(0.2, 0.3, 0.5);
        let hi = progress(0.2, 0.8, 0.5);
        assert!(hi > lo, "{} !> {}", hi, lo);
    }

    #[test]
    fn quality_scales() {
        let lo = progress(0.2, 0.5, 0.2);
        let hi = progress(0.2, 0.5, 0.8);
        assert!(hi > lo, "{} !> {}", hi, lo);
    }

    #[test]
    fn cur_kept_on_zero_inputs() {
        // zero affinity → no progress
        assert_eq!(progress(0.4, 0.0, 0.9), 0.4);
        // zero quality → no progress
        assert_eq!(progress(0.4, 0.9, 0.0), 0.4);
    }

    #[test]
    fn reaches_one() {
        let mut p = 0.0;
        let mut feeds = 0;
        while p < 1.0 && feeds < 1000 {
            p = progress(p, 0.5, 0.5);
            feeds += 1;
        }
        assert_eq!(p, 1.0);
        assert!(feeds < 1000);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let cur = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0; // incl out-of-range
            let aff = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0;
            let q = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0;
            let p = progress(cur, aff, q);
            assert!(p >= 0.0 && p <= 1.0, "progress {}", p);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let cur = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let aff = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let q = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(progress(cur, aff, q), progress(cur, aff, q));
        }
    }
}
