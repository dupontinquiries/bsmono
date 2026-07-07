//! `water_evaporate` — removes only ISOLATED SHALLOW water (puddles), never lake edges or
//! deep water. Part of the water cycle; pairs with the dynamic settle.
//!
//! The guard is what makes it safe: a column evaporates only if it AND all four of its
//! neighbours are at or below `shallow` depth. A lake's shallow rim is adjacent to its deep
//! interior, so the rim is protected and the lake doesn't slowly erode; a stranded puddle
//! (shallow everywhere around it) dries up. Operates on the sim's per-tick depth grid in
//! place; volume removed is intentional (this is evaporation, not a conservative step).

/// Evaporate isolated shallow water in `depth` (a `side × side` grid, row-major).
/// A column loses up to `rate` depth only when it and every existing neighbour are
/// `<= shallow`. Returns the total depth removed (for diagnostics).
pub fn evaporate(depth: &mut [f32], side: usize, shallow: f32, rate: f32) -> f32 {
    if side == 0 || rate <= 0.0 {
        return 0.0;
    }
    let n = side * side;
    debug_assert_eq!(depth.len(), n);
    // Decide first (read-only), then apply — so a column's evaporation doesn't change the
    // "is my neighbour shallow" answer for the next column within the same pass.
    let mut remove = vec![0.0f32; n];
    for z in 0..side {
        for x in 0..side {
            let i = x + z * side;
            let d = depth[i];
            if d <= 0.0 || d > shallow {
                continue;
            }
            let mut isolated = true;
            if x + 1 < side && depth[i + 1] > shallow {
                isolated = false;
            }
            if x > 0 && depth[i - 1] > shallow {
                isolated = false;
            }
            if z + 1 < side && depth[i + side] > shallow {
                isolated = false;
            }
            if z > 0 && depth[i - side] > shallow {
                isolated = false;
            }
            if isolated {
                remove[i] = rate.min(d);
            }
        }
    }
    let mut total = 0.0;
    for i in 0..n {
        if remove[i] > 0.0 {
            depth[i] -= remove[i];
            total += remove[i];
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dries_an_isolated_puddle() {
        // 3x3, a single shallow cell in the centre, everything else dry.
        let mut d = vec![0.0f32; 9];
        d[4] = 1.0;
        let removed = evaporate(&mut d, 3, 1.5, 0.25);
        assert!((removed - 0.25).abs() < 1e-6);
        assert!((d[4] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn protects_lake_rim_next_to_deep_water() {
        // 3x3, top row [shallow, deep, shallow], rest dry. The shallow cells border the
        // deep centre-top cell, so they're protected; nothing evaporates.
        let mut d = vec![0.0f32; 9];
        d[0] = 1.0; // (0,0) shallow
        d[1] = 5.0; // (1,0) deep
        d[2] = 1.0; // (2,0) shallow
        let removed = evaporate(&mut d, 3, 1.5, 0.25);
        assert_eq!(removed, 0.0);
        assert_eq!(d[0], 1.0);
        assert_eq!(d[2], 1.0);
    }

    #[test]
    fn deep_water_never_evaporates() {
        let mut d = vec![5.0f32; 9];
        let removed = evaporate(&mut d, 3, 1.5, 0.25);
        assert_eq!(removed, 0.0);
    }

    #[test]
    fn rate_capped_by_remaining_depth() {
        let mut d = vec![0.0f32; 9];
        d[4] = 0.1;
        let removed = evaporate(&mut d, 3, 1.5, 0.25);
        assert!((removed - 0.1).abs() < 1e-6);
        assert!(d[4].abs() < 1e-6); // fully dried, not negative
    }

    #[test]
    fn a_block_of_shallow_puddles_dries_together() {
        // 2x2 all shallow, isolated as a block -> all evaporate.
        let mut d = vec![1.0f32; 4];
        let removed = evaporate(&mut d, 2, 1.5, 0.2);
        assert!((removed - 0.8).abs() < 1e-6); // 4 cells * 0.2
        for v in d {
            assert!((v - 0.8).abs() < 1e-6);
        }
    }

    #[test]
    fn zero_rate_noop() {
        let mut d = vec![1.0f32; 4];
        let removed = evaporate(&mut d, 2, 1.5, 0.0);
        assert_eq!(removed, 0.0);
    }
}
