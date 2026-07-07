//! wing_opacity.rs — pure wing transparency gene → membrane material.
//!
//! Maps a wing gene to a membrane material: low gene = glassy translucent (low
//! alpha, no leather), high gene = opaque leathery (full alpha, full leather
//! tint). Pure, deterministic, panic-free (R1/R11/R21).

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WingMat {
    /// Membrane opacity `[0,1]` (glassy → opaque).
    pub alpha: f32,
    /// Blend toward the leathery tint `[0,1]` (tracks the gene).
    pub leather_mix: f32,
}

/// Glassy baseline alpha at gene 0 (faintly visible membrane).
const GLASSY_ALPHA: f32 = 0.08;

/// `gene` in `[0,1]`: 0 = glassy translucent, 1 = opaque leathery.
pub fn wing_opacity(gene: f32) -> WingMat {
    let g = gene.max(0.0).min(1.0);
    let alpha = (GLASSY_ALPHA + (1.0 - GLASSY_ALPHA) * g).max(0.0).min(1.0);
    WingMat { alpha, leather_mix: g }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gene_zero_is_near_glassy() {
        let w = wing_opacity(0.0);
        assert!(w.alpha < 0.2, "gene 0 should be glassy, alpha {}", w.alpha);
        assert_eq!(w.leather_mix, 0.0, "no leather at gene 0");
    }

    #[test]
    fn gene_one_is_opaque() {
        let w = wing_opacity(1.0);
        assert!((w.alpha - 1.0).abs() < 1e-6, "gene 1 should be opaque");
        assert!((w.leather_mix - 1.0).abs() < 1e-6, "full leather at gene 1");
    }

    #[test]
    fn alpha_in_unit_range() {
        let mut seed = 0x5160_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..2000 {
            let w = wing_opacity(lcg() * 3.0 - 1.0); // includes out-of-range genes
            assert!((0.0..=1.0).contains(&w.alpha), "alpha oob {}", w.alpha);
            assert!((0.0..=1.0).contains(&w.leather_mix), "leather oob {}", w.leather_mix);
        }
    }

    #[test]
    fn leather_mix_tracks_gene() {
        for &g in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!((wing_opacity(g).leather_mix - g).abs() < 1e-6);
        }
        // clamps out-of-range
        assert_eq!(wing_opacity(2.0).leather_mix, 1.0);
        assert_eq!(wing_opacity(-1.0).leather_mix, 0.0);
    }

    #[test]
    fn monotonic_in_gene() {
        let mut prev_a = -1.0f32;
        let mut prev_l = -1.0f32;
        let mut g = 0.0f32;
        while g <= 1.0 {
            let w = wing_opacity(g);
            assert!(w.alpha >= prev_a - 1e-6, "alpha not monotone");
            assert!(w.leather_mix >= prev_l - 1e-6, "leather not monotone");
            prev_a = w.alpha;
            prev_l = w.leather_mix;
            g += 0.02;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(wing_opacity(0.42), wing_opacity(0.42));
    }
}
