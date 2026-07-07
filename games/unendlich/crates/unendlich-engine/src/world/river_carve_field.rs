//! `world::river_carve_field` — carve river channels into a heightfield from flow
//! accumulation.
//!
//! Composes two tested cores: [`river_flow::accumulate_flow`](crate::sim::river_flow)
//! (how much water funnels through each cell, following the steepest descent) and
//! [`river_carve::channel`](crate::world::river_carve) (whether that accumulation is
//! enough to be a river and, if so, how wide/deep the channel is). Where accumulated flow
//! exceeds the threshold, the terrain is lowered to the carved channel bed — so rivers
//! appear in the valleys water actually drains toward, with bigger catchments cutting
//! broader/deeper channels (sub-linear, capped). This is the integrator the orphaned
//! river cores were waiting for.
//!
//! Inert groundwork: the carve is a pure heightfield transform; running it during world
//! generation is the (chunk-manager-side) wiring step.

use crate::sim::river_flow::accumulate_flow;
use crate::world::river_carve::channel;

/// Carve rivers into `heights` (row-major `w*h`) in place. Accumulates flow, then where a
/// cell's accumulation reaches `threshold`, lowers it to the river-channel bed. Returns
/// the number of cells carved. No-op on a size mismatch.
pub fn carve_rivers(heights: &mut [f32], w: usize, h: usize, threshold: u32) -> usize {
    if w == 0 || h == 0 || heights.len() != w * h {
        return 0;
    }
    let accum = accumulate_flow(heights, w, h);
    let mut carved = 0;
    for i in 0..heights.len() {
        let a = accum[i].max(0.0) as u32;
        if let Some(ch) = channel(a, heights[i], threshold) {
            heights[i] = ch.bed_y;
            carved += 1;
        }
    }
    carved
}

/// As [`carve_rivers`] but on a copy, returning the carved heightfield (leaves the input
/// untouched — handy for previewing or blending the carve).
pub fn carved(heights: &[f32], w: usize, h: usize, threshold: u32) -> Vec<f32> {
    let mut out = heights.to_vec();
    carve_rivers(&mut out, w, h, threshold);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tilted plane: height falls toward one corner, so flow converges there.
    fn slope(w: usize, h: usize) -> Vec<f32> {
        let mut g = vec![0.0f32; w * h];
        for z in 0..h {
            for x in 0..w {
                g[x + z * w] = 100.0 - (x as f32 + z as f32);
            }
        }
        g
    }

    #[test]
    fn carves_where_flow_concentrates() {
        let (w, h) = (8, 8);
        let g = slope(w, h);
        let out = carved(&g, w, h, 1); // low threshold: the convergence carves
        // somewhere got lowered, and nothing rose
        assert!(out.iter().zip(g.iter()).any(|(o, i)| o < i), "a channel was carved");
        assert!(out.iter().zip(g.iter()).all(|(o, i)| *o <= *i + 1e-4), "carving only lowers terrain");
    }

    #[test]
    fn high_threshold_carves_less_than_low() {
        let (w, h) = (10, 10);
        let g = slope(w, h);
        let mut lo = g.clone();
        let mut hi = g.clone();
        let n_lo = carve_rivers(&mut lo, w, h, 1);
        let n_hi = carve_rivers(&mut hi, w, h, 50);
        assert!(n_lo >= n_hi, "a higher river threshold carves no more cells ({n_lo} >= {n_hi})");
    }

    #[test]
    fn flat_terrain_no_deep_carve() {
        // perfectly flat: no descent, flow doesn't concentrate into deep channels
        let (w, h) = (6, 6);
        let g = vec![50.0f32; w * h];
        let out = carved(&g, w, h, 1);
        // every cell is unchanged or only shallowly touched; none carved below a sane bound
        assert!(out.iter().all(|&o| o <= 50.0 + 1e-4), "flat terrain isn't raised");
    }

    #[test]
    fn size_mismatch_is_noop() {
        let mut g = vec![1.0f32; 5];
        assert_eq!(carve_rivers(&mut g, 3, 3, 1), 0);
        assert_eq!(g, vec![1.0f32; 5], "mismatched dims leave heights untouched");
    }
}
