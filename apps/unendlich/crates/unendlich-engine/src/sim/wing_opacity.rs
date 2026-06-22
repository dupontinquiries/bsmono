//! Dragon MATERIAL gene — `wing_opacity` → wing-membrane alpha (R28, DRG-dragons).
//!
//! `DRAGON_GENOME.md §MATERIAL`: *"wing_opacity → membrane alpha: 0 =
//! glassy/translucent, 1 = opaque leathery (core `wing_opacity`)."*
//!
//! Pure: gene value in, a single blend alpha out, monotonically increasing.
//! No RNG, I/O, or GPU — the renderer uses the alpha; this core only derives
//! it. The endpoints reach the full [0,1] range exactly so a fully glassy wing
//! (alpha 0) and a fully opaque leathery wing (alpha 1) are both expressible,
//! matching the genome doc.

/// Minimum membrane alpha (`wing_opacity = 0`): fully glassy / translucent.
pub const ALPHA_MIN: f32 = 0.0;
/// Maximum membrane alpha (`wing_opacity = 1`): fully opaque / leathery.
pub const ALPHA_MAX: f32 = 1.0;

/// Express the `wing_opacity` gene (nominally 0..1) as a membrane blend alpha
/// in [`ALPHA_MIN`, `ALPHA_MAX`].
///
/// The gene is clamped to [0,1] first so out-of-range inputs still yield an
/// in-range alpha. Monotonically non-decreasing: higher gene = more opaque.
pub fn express(wing_opacity: f32) -> f32 {
    wing_opacity.clamp(ALPHA_MIN, ALPHA_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn endpoints_and_midpoint() {
        assert!(approx(express(0.0), ALPHA_MIN), "glassy at 0");
        assert!(approx(express(1.0), ALPHA_MAX), "opaque at 1");
        assert!(approx(express(0.5), 0.5), "linear midpoint");
    }

    #[test]
    fn out_of_range_gene_clamps_into_documented_range() {
        for &g in &[-2.0_f32, -0.001, 1.001, 4.0] {
            let a = express(g);
            assert!(a >= ALPHA_MIN && a <= ALPHA_MAX, "alpha {a} out of range");
        }
    }

    #[test]
    fn monotonic_finite_and_in_range() {
        let mut prev = express(0.0);
        for i in 0..=1000 {
            let g = i as f32 / 1000.0;
            let a = express(g);
            assert!(a.is_finite());
            assert!(a >= ALPHA_MIN && a <= ALPHA_MAX);
            assert!(a + 1e-6 >= prev, "non-monotonic at gene {g}");
            prev = a;
        }
    }
}
