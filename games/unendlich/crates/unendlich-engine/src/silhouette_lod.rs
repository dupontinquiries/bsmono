//! silhouette_lod.rs — pure large-entity silhouette + breath legibility.
//!
//! Decides how a large creature is drawn with distance: up close it keeps its
//! real parts; far away it collapses to a flat impostor and its breathing pulse
//! is exaggerated so the silhouette still reads at range. Pure, deterministic,
//! panic-free (R1/R11/R21).

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Silh {
    /// 0 = no impostor (full parts), 1 = fully collapsed to an impostor card.
    pub impostor_scale: f32,
    /// Breathing amplitude; grows with distance so motion stays legible. In
    /// `[0.05, 0.5]`.
    pub breath_pulse: f32,
    /// Whether to render the real articulated parts (true near, false far).
    pub keep_parts: bool,
}

/// Size-normalized distance thresholds: a larger `size` keeps parts / reads at a
/// longer absolute distance because distance is divided by size.
const KEEP: f32 = 20.0; // keep real parts within this normalized distance
const FULL: f32 = 60.0; // fully impostor beyond this

pub fn silhouette(dist: f32, size: f32) -> Silh {
    let d = dist.max(0.0) / size.max(1e-3); // size-normalized distance
    let keep_parts = d < KEEP;
    let impostor_scale = ((d - KEEP) / (FULL - KEEP)).clamp(0.0, 1.0);
    // exaggerate the breath as the body collapses to an impostor
    let breath_pulse = (0.05 + 0.45 * impostor_scale).min(0.5);
    Silh { impostor_scale, breath_pulse, keep_parts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn near_keeps_parts() {
        let s = silhouette(1.0, 1.0);
        assert!(s.keep_parts, "near entity should keep parts");
        assert_eq!(s.impostor_scale, 0.0, "no impostor up close");
        // a bigger creature keeps parts even farther out
        assert!(silhouette(40.0, 5.0).keep_parts, "big creature keeps parts longer");
    }

    #[test]
    fn far_collapses_to_impostor() {
        let s = silhouette(1000.0, 1.0);
        assert!(!s.keep_parts, "far entity drops parts");
        assert!((s.impostor_scale - 1.0).abs() < 1e-6, "far -> full impostor");
    }

    #[test]
    fn breath_pulse_positive_at_distance() {
        for &d in &[100.0f32, 300.0, 1000.0] {
            assert!(silhouette(d, 1.0).breath_pulse > 0.0, "breath must be > 0 at d={}", d);
        }
        // and exaggerated vs near
        assert!(silhouette(1000.0, 1.0).breath_pulse > silhouette(1.0, 1.0).breath_pulse);
    }

    #[test]
    fn prop_monotonic_with_distance() {
        let mut prev_imp = -1.0f32;
        let mut prev_breath = -1.0f32;
        let mut saw_drop = false;
        let mut last_keep = true;
        let mut d = 0.0f32;
        while d <= 200.0 {
            let s = silhouette(d, 1.0);
            assert!(s.impostor_scale >= prev_imp - 1e-6, "impostor not monotone at d={}", d);
            assert!(s.breath_pulse >= prev_breath - 1e-6, "breath not monotone at d={}", d);
            // keep_parts only ever transitions true -> false once
            if last_keep && !s.keep_parts {
                saw_drop = true;
            }
            if saw_drop {
                assert!(!s.keep_parts, "keep_parts flipped back on at d={}", d);
            }
            last_keep = s.keep_parts;
            prev_imp = s.impostor_scale;
            prev_breath = s.breath_pulse;
            d += 1.0;
        }
    }

    #[test]
    fn prop_bounded() {
        let mut seed = 0x5117_0009u32;
        for _ in 0..500 {
            let dist = lcg(&mut seed) * 5000.0 - 100.0; // includes negatives
            let size = lcg(&mut seed) * 20.0;
            let s = silhouette(dist, size);
            assert!((0.0..=1.0).contains(&s.impostor_scale), "impostor oob {}", s.impostor_scale);
            assert!((0.05..=0.5).contains(&s.breath_pulse), "breath oob {}", s.breath_pulse);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(silhouette(123.0, 2.0), silhouette(123.0, 2.0));
    }
}
