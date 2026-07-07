//! lod_skirt.rs — pure LOD-transition skirt geometry.
//!
//! At an LOD boundary the finer chunk's edge can sit above the coarser
//! neighbour, leaving a visible crack. A "skirt" hides it: a vertical strip
//! hanging from each edge sample down to the neighbour's floor height. Pairs
//! with `mesh_smooth` (which blends the shared ring). Pure, deterministic,
//! panic-free (R1/R11/R21).
//!
//! The edge runs along +X at `z = 0`; sample `i` sits at `x = i * cell`. For
//! each sample we emit two vertices: the top (at the edge height) and the bottom
//! (dropped to `neighbour_floor`), so consecutive pairs form quad strips.

/// Build the skirt strip for one chunk edge. Output length is
/// `edge_heights.len() * 2`: for each sample `i`, `out[2*i]` is the top vertex
/// `[i*cell, edge_heights[i], 0]` and `out[2*i+1]` is the bottom vertex
/// `[i*cell, neighbour_floor, 0]`.
pub fn skirt_strip(edge_heights: &[f32], neighbour_floor: f32, cell: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(edge_heights.len() * 2);
    for (i, &h) in edge_heights.iter().enumerate() {
        let x = i as f32 * cell;
        out.push([x, h, 0.0]); // top: follows the edge
        out.push([x, neighbour_floor, 0.0]); // bottom: sealed to the neighbour
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn output_len_is_double_edge() {
        for n in 0..32usize {
            let heights: Vec<f32> = (0..n).map(|i| i as f32 * 0.5).collect();
            assert_eq!(skirt_strip(&heights, 0.0, 1.0).len(), n * 2);
        }
    }

    #[test]
    fn every_bottom_is_neighbour_floor() {
        let heights = [3.0, 5.0, 1.0, 9.0];
        let floor = -2.5;
        let strip = skirt_strip(&heights, floor, 2.0);
        for (i, v) in strip.iter().enumerate() {
            if i % 2 == 1 {
                assert_eq!(v[1], floor, "bottom vertex not at floor");
            }
        }
    }

    #[test]
    fn tops_match_edge_heights() {
        let heights = [3.0, 5.0, 1.0, 9.0];
        let strip = skirt_strip(&heights, 0.0, 1.0);
        for (i, &h) in heights.iter().enumerate() {
            assert_eq!(strip[2 * i][1], h, "top vertex height mismatch at {}", i);
        }
    }

    #[test]
    fn empty_edge_is_empty() {
        assert!(skirt_strip(&[], 0.0, 1.0).is_empty());
    }

    #[test]
    fn x_spacing_equals_cell() {
        let heights = [0.0; 6];
        let cell = 1.75;
        let strip = skirt_strip(&heights, 0.0, cell);
        for i in 1..heights.len() {
            // top[i].x - top[i-1].x == cell
            let dx = strip[2 * i][0] - strip[2 * (i - 1)][0];
            assert!((dx - cell).abs() < 1e-6, "x spacing {} != cell {}", dx, cell);
            // top and bottom of a pair share x
            assert_eq!(strip[2 * i][0], strip[2 * i + 1][0]);
        }
    }

    #[test]
    fn prop_deterministic_and_finite() {
        let mut seed = 0x5317_7000u32;
        for _ in 0..200 {
            let n = (lcg(&mut seed) * 16.0) as usize;
            let heights: Vec<f32> = (0..n).map(|_| lcg(&mut seed) * 100.0 - 50.0).collect();
            let floor = lcg(&mut seed) * 20.0 - 10.0;
            let cell = lcg(&mut seed) * 4.0 + 0.1;
            let a = skirt_strip(&heights, floor, cell);
            let b = skirt_strip(&heights, floor, cell);
            assert_eq!(a, b, "non-deterministic");
            for v in &a {
                assert!(v[0].is_finite() && v[1].is_finite() && v[2].is_finite());
            }
        }
    }
}
