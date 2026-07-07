//! edge_highlight.rs — pure depth-discontinuity edge factor for outlines.
//!
//! Compares a centre depth to its 4 neighbours; the larger the maximum depth gap
//! relative to `thresh`, the stronger the edge (ramping to 1 at/above the
//! threshold). Pure, deterministic, panic-free (R1/R11/R21). Result in `[0,1]`.

/// Edge strength at a pixel with centre depth `depth_c` and 4 neighbour depths
/// `neigh`, using gap threshold `thresh`. `0` on flat depth, ramping to `1` once
/// the largest neighbour gap reaches `thresh`.
pub fn edge(depth_c: f32, neigh: [f32; 4], thresh: f32) -> f32 {
    let th = thresh.max(1e-6);
    let mut max_gap = 0.0f32;
    for &n in &neigh {
        max_gap = max_gap.max((depth_c - n).abs());
    }
    crate::util::mathx::saturate(max_gap / th)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_depth_is_zero() {
        assert_eq!(edge(5.0, [5.0, 5.0, 5.0, 5.0], 0.1), 0.0);
    }

    #[test]
    fn discontinuity_is_one() {
        assert_eq!(edge(1.0, [1.0, 9.0, 1.0, 1.0], 0.5), 1.0, "big gap -> full edge");
    }

    #[test]
    fn threshold_respected() {
        // a gap below threshold is a partial edge; at/above threshold is full
        let below = edge(1.0, [1.05, 1.0, 1.0, 1.0], 0.2);
        assert!(below > 0.0 && below < 1.0, "sub-threshold should be partial: {}", below);
        assert_eq!(edge(1.0, [1.2, 1.0, 1.0, 1.0], 0.2), 1.0, "at threshold -> 1");
    }

    #[test]
    fn symmetric_in_sign() {
        // a neighbour higher vs lower by the same amount gives the same edge
        let up = edge(1.0, [1.3, 1.0, 1.0, 1.0], 0.5);
        let down = edge(1.0, [0.7, 1.0, 1.0, 1.0], 0.5);
        assert!((up - down).abs() < 1e-6, "edge not symmetric in gap sign");
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2107_0001u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32 << 24) as f32) };
        for _ in 0..2000 {
            let c = lcg() * 100.0;
            let n = [lcg() * 100.0, lcg() * 100.0, lcg() * 100.0, lcg() * 100.0];
            let e = edge(c, n, lcg() * 5.0 + 0.01);
            assert!((0.0..=1.0).contains(&e), "edge oob {}", e);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(edge(2.0, [2.1, 1.9, 2.0, 2.3], 0.5), edge(2.0, [2.1, 1.9, 2.0, 2.3], 0.5));
    }
}
