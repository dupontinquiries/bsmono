//! Dragon MATERIAL gene — `scale_gloss` → micro-specular scale (R28, DRG-dragons).
//!
//! `DRAGON_GENOME.md §MATERIAL`: *"scale_gloss → micro-spec on scales."*
//!
//! Pure: gene value in, a single micro-specular intensity scalar out,
//! monotonically increasing. No RNG, I/O, or GPU — the shader multiplies its
//! per-scale micro-specular term by this; deriving it is all this core does.
//!
//! The output keeps a small non-zero floor: even matte scales catch a faint
//! micro-highlight, so `scale_gloss = 0` is dull-but-not-dead rather than a
//! perfectly flat surface. Glossier scales (gene → 1) scale the term to full.

/// Micro-spec scale at `scale_gloss = 0` (matte scales; faint floor highlight).
pub const MICRO_SPEC_MIN: f32 = 0.05;
/// Micro-spec scale at `scale_gloss = 1` (high-gloss scales; full micro-spec).
pub const MICRO_SPEC_MAX: f32 = 1.0;

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

/// Express the `scale_gloss` gene (nominally 0..1) as a micro-specular scale
/// in [`MICRO_SPEC_MIN`, `MICRO_SPEC_MAX`].
///
/// The gene is clamped to [0,1] first so out-of-range inputs still yield an
/// in-range scale. Monotonically non-decreasing: higher gene = glossier.
pub fn express(scale_gloss: f32) -> f32 {
    let g = scale_gloss.clamp(0.0, 1.0);
    lerp(MICRO_SPEC_MIN, MICRO_SPEC_MAX, g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn endpoints_and_midpoint() {
        assert!(approx(express(0.0), MICRO_SPEC_MIN), "matte floor at 0");
        assert!(approx(express(1.0), MICRO_SPEC_MAX), "full gloss at 1");
        assert!(approx(express(0.5), (MICRO_SPEC_MIN + MICRO_SPEC_MAX) * 0.5));
    }

    #[test]
    fn out_of_range_gene_clamps_into_documented_range() {
        for &g in &[-3.0_f32, -0.001, 1.001, 7.0] {
            let s = express(g);
            assert!(s >= MICRO_SPEC_MIN && s <= MICRO_SPEC_MAX, "micro-spec {s} out of range");
        }
    }

    #[test]
    fn monotonic_finite_and_in_range() {
        let mut prev = express(0.0);
        for i in 0..=1000 {
            let g = i as f32 / 1000.0;
            let s = express(g);
            assert!(s.is_finite());
            assert!(s >= MICRO_SPEC_MIN && s <= MICRO_SPEC_MAX);
            assert!(s + 1e-6 >= prev, "non-monotonic at gene {g}");
            prev = s;
        }
    }
}
