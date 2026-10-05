//! foliage_depth -- draw a canopy's surface in full and its core cheaply.
//!
//! Every leaf block was stamped from a sub-voxel template, ~800 triangles
//! each, 512 per chunk: one forest chunk meshed to 443,922 triangles against
//! the 90,000 tripwire (mesh_tri_regression; diagnosis 79ab94223). Most of
//! those leaves are INSIDE a canopy, where they cannot be seen.
//!
//! Depth is counted in blocks from the nearest non-leaf cell: 1 touches air,
//! 2 sits behind one layer of leaves. A template has gaps -- you see into a
//! canopy through the first layer -- so depths 1..=FULL_DEPTH keep the full
//! template. Deeper leaves are the FILL: a plain dark-green block, with only
//! its faces toward a non-core neighbour drawn, so what shows through the gaps
//! reads as dense foliage rather than sky, and a core block surrounded by core
//! costs nothing. Cells outside the chunk count as non-leaf, so a leaf on the
//! chunk's edge is always drawn in full: a mistake draws too much, never a hole.

// The emitter writes the mesher's vertices and so exists only with a renderer; the depth
// field and the face test above it are pure and build headless (`--no-default-features`,
// the documented way to run the engine's tests -- this import broke that build for a
// month, every test in the crate with it).
#[cfg(feature = "render-core")]
use crate::render::mesher::Vertex;

/// Leaves this many blocks deep or less keep their full template.
pub const FULL_DEPTH: u8 = 2;

/// The fill's colour before lighting: a canopy interior is darker than its
/// sunlit skin.
pub const FILL_COLOR: [f32; 3] = [0.12, 0.26, 0.09];

fn idx(n: usize, x: usize, y: usize, z: usize) -> usize { x + n * (y + n * z) }

/// Depth of every leaf cell (0 for non-leaf), indexed x + n*(y + n*z).
pub fn leaf_depths(n: usize, is_leaf: impl Fn(usize, usize, usize) -> bool) -> Vec<u8> {
    let mut d = vec![0u8; n * n * n];
    let mut frontier = std::collections::VecDeque::new();
    for z in 0..n { for y in 0..n { for x in 0..n {
        if !is_leaf(x, y, z) { continue; }
        let edge = x == 0 || y == 0 || z == 0 || x + 1 == n || y + 1 == n || z + 1 == n;
        let by_other = edge || [(1i32, 0i32, 0i32), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)].iter().any(|&(dx, dy, dz)| {
            !is_leaf((x as i32 + dx) as usize, (y as i32 + dy) as usize, (z as i32 + dz) as usize)
        });
        if by_other {
            d[idx(n, x, y, z)] = 1;
            frontier.push_back((x, y, z));
        }
    }}}
    while let Some((x, y, z)) = frontier.pop_front() {
        let here = d[idx(n, x, y, z)];
        for (dx, dy, dz) in [(1i32, 0i32, 0i32), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
            if nx < 0 || ny < 0 || nz < 0 || nx as usize >= n || ny as usize >= n || nz as usize >= n { continue; }
            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
            let j = idx(n, nx, ny, nz);
            if d[j] == 0 && is_leaf(nx, ny, nz) {
                d[j] = here.saturating_add(1);
                frontier.push_back((nx, ny, nz));
            }
        }
    }
    d
}

/// Depth of the leaf at (x, y, z) in a `leaf_depths` result (0: not a leaf).
pub fn depth_at(depths: &[u8], n: usize, x: usize, y: usize, z: usize) -> u8 { depths[idx(n, x, y, z)] }

/// Which faces (+X, -X, +Y, -Y, +Z, -Z) of a core leaf to draw: those toward
/// a neighbour that is not itself core (a full-template leaf, or anything
/// else). Core cells are never on the chunk edge (edges are depth 1).
pub fn fill_faces(depths: &[u8], n: usize, x: usize, y: usize, z: usize) -> [bool; 6] {
    let core = |x: usize, y: usize, z: usize| depths[idx(n, x, y, z)] > FULL_DEPTH;
    [
        !core(x + 1, y, z), !core(x - 1, y, z),
        !core(x, y + 1, z), !core(x, y - 1, z),
        !core(x, y, z + 1), !core(x, y, z - 1),
    ]
}

/// Emit the chosen faces of the unit block at `origin`, lit per face like the
/// templates (`brightness(normal)`), tinted by `color_mul` (leaf decay).
#[cfg(feature = "render-core")]
pub fn emit_fill(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, origin: [f32; 3], faces: [bool; 6],
                 color_mul: [f32; 3], brightness: impl Fn([f32; 3]) -> f32) {
    // (normal, four corners counter-clockwise seen from outside)
    const QUADS: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([1.0, 0.0, 0.0], [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]]),
        ([-1.0, 0.0, 0.0], [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]]),
        ([0.0, 1.0, 0.0], [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]]),
        ([0.0, -1.0, 0.0], [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]),
        ([0.0, 0.0, 1.0], [[1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]]),
        ([0.0, 0.0, -1.0], [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]]),
    ];
    for (on, (normal, corners)) in faces.iter().zip(QUADS.iter()) {
        if !on { continue; }
        let b = brightness(*normal);
        let color = [
            (FILL_COLOR[0] * color_mul[0] * b).clamp(0.0, 1.0),
            (FILL_COLOR[1] * color_mul[1] * b).clamp(0.0, 1.0),
            (FILL_COLOR[2] * color_mul[2] * b).clamp(0.0, 1.0),
        ];
        let base = vertices.len() as u32;
        for c in corners {
            vertices.push(Vertex::new([origin[0] + c[0], origin[1] + c[1], origin[2] + c[2]], color));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_counts_blocks_from_the_air_and_the_edge_is_always_surface() {
        // A 7x7x7 solid leaf ball in the middle of a 16^3 chunk.
        let n = 16;
        let ball = |x: usize, y: usize, z: usize| (4..11).contains(&x) && (4..11).contains(&y) && (4..11).contains(&z);
        let d = leaf_depths(n, ball);
        assert_eq!(depth_at(&d, n, 4, 7, 7), 1, "the skin");
        assert_eq!(depth_at(&d, n, 5, 7, 7), 2, "one layer in");
        assert_eq!(depth_at(&d, n, 6, 7, 7), 3, "core");
        assert_eq!(depth_at(&d, n, 7, 7, 7), 4, "the centre");
        assert_eq!(depth_at(&d, n, 0, 0, 0), 0, "air is not a leaf");
        // Leaves filling the whole chunk: the edge counts as air.
        let all = leaf_depths(8, |_, _, _| true);
        assert_eq!(depth_at(&all, 8, 0, 3, 3), 1);
        assert_eq!(depth_at(&all, 8, 3, 3, 3), 4);
    }

    #[test]
    fn a_core_draws_only_its_faces_toward_the_surface_layers() {
        let n = 16;
        let ball = |x: usize, y: usize, z: usize| (4..11).contains(&x) && (4..11).contains(&y) && (4..11).contains(&z);
        let d = leaf_depths(n, ball);
        // The 3x3x3 core (depth >= 3) spans 6..=8; its centre sees only core.
        assert_eq!(fill_faces(&d, n, 7, 7, 7), [false; 6], "surrounded by core: nothing");
        assert_eq!(fill_faces(&d, n, 6, 7, 7), [false, true, false, false, false, false], "only -X faces a depth-2 leaf");
        let mut faces = 0;
        for z in 6..=8 { for y in 6..=8 { for x in 6..=8 {
            faces += fill_faces(&d, n, x, y, z).iter().filter(|b| **b).count();
        }}}
        assert_eq!(faces, 54, "the 3x3x3 core's shell: 6 sides x 9 faces");
    }

    /// The emitter itself: four vertices and six indices per face (with a renderer only).
    #[cfg(feature = "render-core")]
    #[test]
    fn the_emitter_writes_a_quad_per_face() {
        let n = 16;
        let ball = |x: usize, y: usize, z: usize| (4..11).contains(&x) && (4..11).contains(&y) && (4..11).contains(&z);
        let d = leaf_depths(n, ball);
        let (mut v, mut i) = (Vec::new(), Vec::new());
        for z in 6..=8 { for y in 6..=8 { for x in 6..=8 {
            let f = fill_faces(&d, n, x, y, z);
            emit_fill(&mut v, &mut i, [x as f32, y as f32, z as f32], f, [1.0; 3], |_| 1.0);
        }}}
        assert_eq!((v.len(), i.len()), (54 * 4, 54 * 6));
    }

    #[test]
    fn the_budget_arithmetic_for_the_ball() {
        // 343 leaves: 7^3 - 3^3 = 316 in the two surface layers keep their
        // templates; the 27-block core, ~800 triangles each before, now costs
        // its shell: 54 quads = 108 triangles in all.
        let n = 16;
        let ball = |x: usize, y: usize, z: usize| (4..11).contains(&x) && (4..11).contains(&y) && (4..11).contains(&z);
        let d = leaf_depths(n, ball);
        let full = d.iter().filter(|&&v| v >= 1 && v <= FULL_DEPTH).count();
        let core = d.iter().filter(|&&v| v > FULL_DEPTH).count();
        assert_eq!((full, core), (343 - 27, 27));
    }
}
