//! fxaa_edge.rs — pure FXAA luma edge factor from a 3x3 cross neighbourhood.
//!
//! Local luma contrast (max-min over centre + 4 neighbours); below `thresh` it's
//! 0, otherwise it ramps to 1. Pure, deterministic, panic-free (R1/R11/R21).
//! Result in `[0,1]`. Lumas are expected in `[0,1]`.

/// Edge factor for centre luma `l_c` and N/E/S/W neighbours, with contrast
/// threshold `thresh`. `0` on flat luma, ramping to `1` for a full-contrast edge.
pub fn edge(l_c: f32, l_n: f32, l_e: f32, l_s: f32, l_w: f32, thresh: f32) -> f32 {
    let lo = l_c.min(l_n).min(l_e).min(l_s).min(l_w);
    let hi = l_c.max(l_n).max(l_e).max(l_s).max(l_w);
    let range = hi - lo;
    let th = thresh.max(0.0).min(1.0);
    if range <= th {
        return 0.0;
    }
    let denom = (1.0 - th).max(1e-4);
    ((range - th) / denom).max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_zero() {
        assert_eq!(edge(0.5, 0.5, 0.5, 0.5, 0.5, 0.1), 0.0);
    }

    #[test]
    fn strong_edge_near_one() {
        // full contrast 0..1 with a low threshold -> ~1
        assert!((edge(1.0, 0.0, 1.0, 0.0, 1.0, 0.05) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn threshold_respected() {
        // small contrast below threshold -> 0
        assert_eq!(edge(0.5, 0.52, 0.5, 0.48, 0.5, 0.1), 0.0, "below thresh should be 0");
        // contrast above threshold -> nonzero
        assert!(edge(0.5, 0.9, 0.5, 0.1, 0.5, 0.1) > 0.0);
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2517_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let e = edge(lcg(), lcg(), lcg(), lcg(), lcg(), lcg());
            assert!((0.0..=1.0).contains(&e), "edge oob {}", e);
        }
    }

    #[test]
    fn symmetric_in_neighbours() {
        // permuting the 4 neighbours doesn't change the result (uses min/max)
        let a = edge(0.5, 0.1, 0.9, 0.3, 0.7, 0.1);
        let b = edge(0.5, 0.7, 0.3, 0.9, 0.1, 0.1);
        assert!((a - b).abs() < 1e-6, "edge not permutation-symmetric");
    }

    #[test]
    fn deterministic() {
        assert_eq!(edge(0.5, 0.6, 0.4, 0.55, 0.45, 0.1), edge(0.5, 0.6, 0.4, 0.55, 0.45, 0.1));
    }
}
