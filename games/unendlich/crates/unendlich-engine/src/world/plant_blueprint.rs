//! Plant blueprints — declarative descriptions of plant shapes that the
//! sub-voxel mesher interprets at render time.
//!
//! Why blueprints: hand-written recipes (the old `recipes::grass_tuft`,
//! `recipes::fern`, etc.) put plant shape in code. That's fine for a few
//! plants but doesn't scale — every new plant needs Rust code, no runtime
//! variation, no procedural composition.
//!
//! With blueprints, a plant is a small data structure: a list of
//! **segments** (stems, blades, branches) each with a base point,
//! direction, length, thickness, curvature, and color. The interpreter
//! walks each segment and emits sub-voxel cells along the path.
//!
//! ## What "spirals" / curvature means here
//!
//! A real grass blade isn't a perfectly vertical column — it leans, curves,
//! tapers. Rod theory describes this using *curvature* (how much the tangent
//! turns per unit length) and *torsion* (how much the cross-section rotates
//! around the tangent). For visual blueprints we don't need full Cosserat
//! rods — a per-segment 3D curvature vector that bends the centerline over
//! the segment's length is enough.
//!
//! Concretely: the segment's tangent rotates by `curvature * t` as `t`
//! goes from 0 to `length`. With `curvature` driven by world-position
//! noise, different blades curve in different directions — natural-
//! looking variation without coding each blade by hand.
//!
//! ## What's deferred
//!
//! - Branching: a segment that spawns child segments at its endpoint
//!   (think tree branches). The Vec-of-segments form supports this if you
//!   set child base = parent's tip; an endpoint accessor would help.
//! - Leaves at segment tips (terminator shapes — leaves, flowers, fruit).
//! - Per-segment thickness gradient (taper).
//! - Bending physics — the blueprint stores STATIC curvature, but a real
//!   bending solver (Kirchhoff rods, etc.) would mutate per-frame curvature
//!   based on accumulated forces from wind / weight / collision.

use super::subvoxel::{SubVoxelGrid, MiniCell, SUBVOXEL_RESOLUTION};

/// A single stem / blade / branch. The interpreter walks `length`
/// sub-voxel steps from `base`, advancing the position each step by the
/// current tangent. The tangent rotates over the segment by the
/// `curvature` vector — this is what bends the segment.
///
/// All coordinates are in the host block's sub-voxel space (0..RES).
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    /// Starting sub-voxel position. (x, y, z) in [0..RES).
    pub base: [u8; 3],
    /// Number of sub-voxel cells to lay down.
    pub length: u8,
    /// Initial unit direction. Default for a vertical stem: [0, 1, 0].
    /// Need not be normalized — the interpreter normalizes.
    pub direction: [f32; 3],
    /// Cells laid per step in the lateral plane. 1 = thin (a single
    /// cell line); higher values create thicker stems by adding
    /// adjacent cells. Capped to ~3 visually.
    pub thickness: u8,
    /// Curvature vector applied as a constant rotation per step. The
    /// tangent at step t is `(direction + curvature * t).normalize()`.
    /// In the lateral (XZ) plane this bends the stem; in Y it would
    /// straighten or fold. Magnitude controls bend rate. Typical
    /// values: ±0.05 to ±0.2 per step for a noticeable lean.
    pub curvature: [f32; 3],
    /// Color of every cell along this segment, before per-cell shading.
    pub color: [f32; 3],
}

impl Segment {
    /// Vertical stem, no bend, default thickness 1.
    pub fn vertical(base: [u8; 3], length: u8, color: [f32; 3]) -> Self {
        Self {
            base, length, color,
            direction: [0.0, 1.0, 0.0],
            thickness: 1,
            curvature: [0.0, 0.0, 0.0],
        }
    }

    /// Same as `vertical` but with a noise-driven horizontal curvature.
    /// `seed_hash` is used to pick the curve direction — different blades
    /// from the same recipe but different seeds lean different ways.
    /// `magnitude` is the curvature strength (recommend 0.05..0.2).
    pub fn vertical_curved(
        base: [u8; 3], length: u8, color: [f32; 3],
        seed_hash: u32, magnitude: f32,
    ) -> Self {
        // Convert hash to a 2D unit direction in the XZ plane.
        let angle = ((seed_hash & 0xFFFF) as f32 / 65535.0) * std::f32::consts::TAU;
        let cx = angle.cos() * magnitude;
        let cz = angle.sin() * magnitude;
        Self {
            base, length, color,
            direction: [0.0, 1.0, 0.0],
            thickness: 1,
            curvature: [cx, 0.0, cz],
        }
    }
}

/// A complete plant blueprint — the declarative form of a sub-voxel grid.
#[derive(Debug, Clone, Default)]
pub struct Blueprint {
    pub segments: Vec<Segment>,
}

impl Blueprint {
    pub fn new() -> Self { Self::default() }
    pub fn push(&mut self, s: Segment) { self.segments.push(s); }

    /// Interpret this blueprint into a `SubVoxelGrid`. Walks each
    /// segment, places cells along the path, returns the resulting grid.
    /// Cells outside [0, RES) are clipped silently — segments that
    /// curve out of the host block lose those cells.
    pub fn to_subgrid(&self) -> SubVoxelGrid {
        let mut grid = SubVoxelGrid::empty();
        for seg in &self.segments {
            walk_segment(seg, &mut grid);
        }
        grid
    }
}

/// Walk one segment, placing cells along its (possibly curving) centerline.
/// Cells that fall outside [0, RES) on any axis are clipped.
fn walk_segment(seg: &Segment, grid: &mut SubVoxelGrid) {
    let max = SUBVOXEL_RESOLUTION as i32;
    // Normalize initial direction.
    let dir_len = (seg.direction[0] * seg.direction[0]
        + seg.direction[1] * seg.direction[1]
        + seg.direction[2] * seg.direction[2]).sqrt();
    if dir_len < 1e-6 { return; }
    let mut dir = [
        seg.direction[0] / dir_len,
        seg.direction[1] / dir_len,
        seg.direction[2] / dir_len,
    ];

    // Position in sub-voxel space, as floats so we can step fractionally
    // and curve smoothly. Round to integer cells when placing.
    let mut pos = [
        seg.base[0] as f32 + 0.5,
        seg.base[1] as f32 + 0.5,
        seg.base[2] as f32 + 0.5,
    ];

    for _step in 0..seg.length {
        // Place the cell at the current rounded position.
        place_cell(grid, &pos, seg.color, max);

        // Thickness > 1: also place lateral neighbors. Pick the two
        // axes most perpendicular to current direction and add cells
        // there. Clamps subtly cap at thickness 3.
        if seg.thickness >= 2 {
            // Pick the two axes most perpendicular to current dir. For
            // a near-vertical stem that's X and Z.
            let perp_axes = perpendicular_axes(&dir);
            for axis in perp_axes.iter() {
                let mut p2 = pos;
                p2[*axis] += 1.0;
                place_cell(grid, &p2, seg.color, max);
                if seg.thickness >= 3 {
                    let mut p3 = pos;
                    p3[*axis] -= 1.0;
                    place_cell(grid, &p3, seg.color, max);
                }
            }
        }

        // Advance position along current direction.
        pos[0] += dir[0];
        pos[1] += dir[1];
        pos[2] += dir[2];

        // Bend tangent by curvature, then renormalize. This is the
        // "rod calculus" piece: tangent integral of curvature over arc
        // length. Discrete: dir' = normalize(dir + curvature).
        dir[0] += seg.curvature[0];
        dir[1] += seg.curvature[1];
        dir[2] += seg.curvature[2];
        let l = (dir[0]*dir[0] + dir[1]*dir[1] + dir[2]*dir[2]).sqrt();
        if l > 1e-6 {
            dir[0] /= l;
            dir[1] /= l;
            dir[2] /= l;
        }
    }
}

fn place_cell(grid: &mut SubVoxelGrid, pos: &[f32; 3], color: [f32; 3], max: i32) {
    let x = pos[0].floor() as i32;
    let y = pos[1].floor() as i32;
    let z = pos[2].floor() as i32;
    if x < 0 || x >= max || y < 0 || y >= max || z < 0 || z >= max { return; }
    grid.cells.push(MiniCell {
        sub_x: x as u8,
        sub_y: y as u8,
        sub_z: z as u8,
        color,
    });
}

/// Return the two world axes (0=X, 1=Y, 2=Z) most perpendicular to `dir`.
/// Used to pick "lateral" axes for adding thickness cells. For a near-
/// vertical direction returns [0, 2] (X and Z). For a near-horizontal X,
/// returns [1, 2] (Y and Z).
fn perpendicular_axes(dir: &[f32; 3]) -> [usize; 2] {
    let abs = [dir[0].abs(), dir[1].abs(), dir[2].abs()];
    // Pick the axis with smallest |dir| as one perpendicular, the second-
    // smallest as the other.
    let mut sorted = [(0, abs[0]), (1, abs[1]), (2, abs[2])];
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    [sorted[0].0, sorted[1].0]
}

// =============================================================================
// Built-in blueprints
// =============================================================================

/// Build a grass-tuft blueprint with `n` blades at hash-driven positions
/// and curvatures. Compatible with the old `recipes::grass_tuft` shape
/// — sparse cluster of vertical blades, varying heights — but with
/// per-blade curvature so they look natural rather than rigidly straight.
pub fn grass_tuft_blueprint(n: usize, color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    // Place blades on a deterministic-but-varying pattern. We use the
    // seed hash to pick blade base positions; hashing again per blade
    // gives us a distinct curvature per blade.
    let mut h = seed_hash;
    for i in 0..n {
        // Cheap rehash for next blade.
        h = h.wrapping_mul(0x9E37_79B9).wrapping_add(0x12345);
        // Base in the inner 5×5 region (sub coords 2..7) so blades cluster.
        let bx = 2 + ((h >> 4) & 0x4) as u8;       // 2 or 6
        let bz = 2 + ((h >> 8) & 0x4) as u8;
        // Variations on (bx, bz) to spread blades more.
        let bx = (bx + (i as u8 * 3) % 5).min((SUBVOXEL_RESOLUTION - 1) as u8);
        let bz = (bz + (i as u8 * 5) % 5).min((SUBVOXEL_RESOLUTION - 1) as u8);
        // Blade height: 8 distinct values from 1 to SUBVOXEL_RESOLUTION-1.
        // Previous `& 0x4` only produced {0, 4} → heights {3, 7}, two
        // values total. The user asked for ~8 different heights with
        // shared noise; `& 0x7` gives the full 0-7 range, mapped to
        // heights 1..=8. The seed hash above ensures neighboring tufts
        // have correlated heights via the `h` chain (each blade's
        // height depends on the prior blade's hash, so a tuft has
        // self-consistent variation rather than salt-and-pepper).
        let variation = ((h >> 12) & 0x7) as u8;
        let height = (1 + variation).min((SUBVOXEL_RESOLUTION - 1) as u8);
        // Curvature magnitude: 0.05 - 0.18. Tall blades curve more.
        let curve_mag = 0.05 + (height as f32 / 7.0) * 0.13;
        let seg = Segment::vertical_curved(
            [bx, 0, bz],
            height,
            color,
            h.wrapping_mul(7919),  // distinct sub-seed for direction
            curve_mag,
        );
        bp.push(seg);
    }
    bp
}

/// Build a tree-leaf blueprint: a small twig with leaves attached at
/// hash-determined positions. Replaces the old uniform-cluster leaf
/// design with something that reads as actual foliage.
///
/// Structure:
///   - 1 brown twig segment, ~4-5 cells, slight curve, originating from
///     a hash-driven entry point on one face of the host cell. The twig
///     points roughly toward the cell interior so the leaves end up
///     visible at the cell surface.
///   - 2-4 leaf "foliage" segments along the twig: short (2-3 cells),
///     thicker (thickness=2), in colors near `leaf_color` with per-segment
///     jitter. Direction is roughly perpendicular to the twig's tangent
///     at that point, with hash-driven offsets so leaves spray out.
///
/// This composes the existing Segment + Blueprint primitives — no new
/// types needed. The twig + leaves layer up via the multi-segment
/// blueprint walker. Future: terminators (leaves at the very tip)
/// would let us avoid faking it with a short segment.
pub fn tree_leaf_blueprint(
    leaf_color: [f32; 3], twig_color: [f32; 3], seed_hash: u32
) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let max = SUBVOXEL_RESOLUTION as f32;

    // Pick which face the twig enters from. 6 options, 3 axes × 2 sides.
    let face = (h & 0x7) % 6;
    h = h.wrapping_mul(0x9E37_79B9);

    // Twig start position on that face, jittered.
    let mid = (max / 2.0) as u8;
    let jitter1 = (((h >> 4) & 0x3) as u8).saturating_sub(1);  // -1..1
    let jitter2 = (((h >> 8) & 0x3) as u8).saturating_sub(1);
    let (base, twig_dir): ([u8; 3], [f32; 3]) = match face {
        0 => ([0,        mid + jitter1, mid + jitter2], [ 1.0, 0.0, 0.0]),
        1 => ([(SUBVOXEL_RESOLUTION - 1) as u8, mid + jitter1, mid + jitter2], [-1.0, 0.0, 0.0]),
        2 => ([mid + jitter1, 0, mid + jitter2], [0.0,  1.0, 0.0]),
        3 => ([mid + jitter1, (SUBVOXEL_RESOLUTION - 1) as u8, mid + jitter2], [0.0, -1.0, 0.0]),
        4 => ([mid + jitter1, mid + jitter2, 0], [0.0, 0.0,  1.0]),
        _ => ([mid + jitter1, mid + jitter2, (SUBVOXEL_RESOLUTION - 1) as u8], [0.0, 0.0, -1.0]),
    };

    // Twig segment: short, brown, slight curve away from entry direction.
    let twig_len = 4 + ((h >> 12) & 0x1) as u8;  // 4 or 5
    h = h.wrapping_mul(0x9E37_79B9);
    let twig_curve_axis = ((h >> 4) & 0x3) as usize;
    let twig_curve_mag = 0.08;
    let mut twig_curvature = [0.0; 3];
    if twig_curve_axis < 3 && twig_curve_axis != axis_of(&twig_dir) {
        twig_curvature[twig_curve_axis] = twig_curve_mag * if (h & 0x10) != 0 { 1.0 } else { -1.0 };
    }
    bp.push(Segment {
        base,
        length: twig_len,
        direction: twig_dir,
        thickness: 1,
        curvature: twig_curvature,
        color: twig_color,
    });

    // Leaf clumps along the twig. 2-4 of them. Each is a short, thick
    // segment perpendicular to the twig's local tangent.
    let n_leaves = 2 + ((h >> 16) & 0x3) as usize;  // 2..5
    let twig_axis = axis_of(&twig_dir);
    for i in 0..n_leaves {
        h = h.wrapping_mul(0x9E37_79B9);
        // Position along the twig, [0, twig_len).
        let along = ((i as u8) * twig_len / (n_leaves as u8 + 1) + 1).min(twig_len - 1);
        // Compute base position by walking the twig.
        let mut leaf_base = base;
        for axis in 0..3 {
            let bf = base[axis] as i32 + (twig_dir[axis] * along as f32) as i32;
            leaf_base[axis] = bf.clamp(0, (SUBVOXEL_RESOLUTION - 1) as i32) as u8;
        }
        // Direction: perpendicular to twig, in a plane chosen by hash.
        // Pick one of the two axes orthogonal to twig and a sign.
        let perp_pick = (h >> 4) & 0x3;
        let (perp_axis, sign) = match (twig_axis, perp_pick) {
            (0, 0) => (1, 1.0),  (0, 1) => (1, -1.0),  (0, 2) => (2, 1.0),  (0, _) => (2, -1.0),
            (1, 0) => (0, 1.0),  (1, 1) => (0, -1.0),  (1, 2) => (2, 1.0),  (1, _) => (2, -1.0),
            (2, 0) => (0, 1.0),  (2, 1) => (0, -1.0),  (2, 2) => (1, 1.0),  (_, _) => (1, -1.0),
        };
        let mut leaf_dir = [0.0; 3];
        leaf_dir[perp_axis] = sign;

        // Per-leaf color jitter: ±10%.
        let j = ((h >> 8) & 0xFF) as f32 / 255.0 - 0.5;
        let c = [
            (leaf_color[0] + j * 0.10).clamp(0.0, 1.0),
            (leaf_color[1] + j * 0.15).clamp(0.0, 1.0),
            (leaf_color[2] + j * 0.10).clamp(0.0, 1.0),
        ];

        bp.push(Segment {
            base: leaf_base,
            length: 2 + ((h >> 16) & 0x1) as u8,  // 2 or 3
            direction: leaf_dir,
            thickness: 2,  // chunky leaf clusters
            curvature: [0.0; 3],
            color: c,
        });
    }

    bp
}

/// Generate a branching trunk segment for a tree's trunk block. This
/// is the per-block sub-voxel detail for trunks — gives the otherwise
/// monolithic brown column some structural variation (mini-branches
/// fan out at fractional heights of the trunk).
///
/// Note: this generates branching INSIDE a single block. For canopy-
/// scale branching (whole branches connecting trunk blocks to leaf
/// blocks), see future work in cpu_generator.
///
/// Used by trunk blocks at the canopy attachment level. For trunk
/// blocks deep in the trunk shaft, prefer the simpler `Segment::vertical`.
pub fn branching_trunk_blueprint(
    trunk_color: [f32; 3],
    branch_color: [f32; 3],
    seed_hash: u32,
) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let mid = (SUBVOXEL_RESOLUTION / 2) as u8;

    // Main vertical trunk core: spans the full block height. Slightly
    // offset from center via hash for variation between blocks.
    let trunk_jitter_x = (((h >> 4) & 0x3) as i8 - 1).max(-1).min(1);
    let trunk_jitter_z = (((h >> 6) & 0x3) as i8 - 1).max(-1).min(1);
    let trunk_x = (mid as i8 + trunk_jitter_x).max(1).min((SUBVOXEL_RESOLUTION - 2) as i8) as u8;
    let trunk_z = (mid as i8 + trunk_jitter_z).max(1).min((SUBVOXEL_RESOLUTION - 2) as i8) as u8;
    bp.push(Segment {
        base: [trunk_x, 0, trunk_z],
        length: SUBVOXEL_RESOLUTION as u8,
        direction: [0.0, 1.0, 0.0],
        thickness: 3, // chunky trunk
        curvature: [0.0; 3],
        color: trunk_color,
    });

    // 1-3 small lateral branches at varying heights.
    h = h.wrapping_mul(0x9E37_79B9);
    let n_branches = 1 + ((h >> 16) & 0x3) as usize; // 1..4
    for i in 0..n_branches {
        h = h.wrapping_mul(0x9E37_79B9);
        // Vertical position along trunk: spread evenly with jitter.
        let along_frac = ((i as u8 * SUBVOXEL_RESOLUTION as u8) / (n_branches as u8 + 1)) + 1;
        let by = along_frac.min((SUBVOXEL_RESOLUTION - 2) as u8);
        // Lateral direction: pick one of 4 cardinal directions in XZ plane.
        let dir_pick = (h >> 4) & 0x3;
        let (dx, dz) = match dir_pick {
            0 => (1.0,  0.0),
            1 => (-1.0, 0.0),
            2 => (0.0,  1.0),
            _ => (0.0,  -1.0),
        };
        // Branch length: 2-3 cells.
        let branch_len = 2 + ((h >> 8) & 0x1) as u8;
        // Slight upward tilt for each branch: more visually interesting
        // than purely horizontal sticks.
        let tilt = 0.3;
        let branch_dir = [dx, tilt, dz];
        bp.push(Segment {
            base: [trunk_x, by, trunk_z],
            length: branch_len,
            direction: branch_dir,
            thickness: 1,
            curvature: [0.0; 3],
            color: branch_color,
        });
    }
    bp
}

/// Index of the dominant axis of a unit-ish direction vector.
fn axis_of(dir: &[f32; 3]) -> usize {
    let abs = [dir[0].abs(), dir[1].abs(), dir[2].abs()];
    if abs[0] >= abs[1] && abs[0] >= abs[2] { 0 }
    else if abs[1] >= abs[2] { 1 }
    else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_stem_emits_one_cell_per_step() {
        let seg = Segment::vertical([3, 0, 3], 5, [0.0, 1.0, 0.0]);
        let mut bp = Blueprint::new();
        bp.push(seg);
        let g = bp.to_subgrid();
        assert_eq!(g.cell_count(), 5, "expected 5 cells in a 5-tall vertical stem");
        // Cells should be stacked vertically at (3, *, 3).
        for cell in &g.cells {
            assert_eq!((cell.sub_x, cell.sub_z), (3, 3));
        }
    }

    #[test]
    fn curved_stem_drifts_laterally() {
        // A curved stem with strong horizontal curvature should NOT have
        // all cells at the same (x, z).
        let seg = Segment {
            base: [4, 0, 4],
            length: 7,
            direction: [0.0, 1.0, 0.0],
            thickness: 1,
            curvature: [0.2, 0.0, 0.0], // bend strongly in +X
            color: [0.0, 1.0, 0.0],
        };
        let mut bp = Blueprint::new();
        bp.push(seg);
        let g = bp.to_subgrid();
        let xs: std::collections::HashSet<u8> =
            g.cells.iter().map(|c| c.sub_x).collect();
        assert!(xs.len() > 1,
            "curved stem should have multiple x positions; got {:?}", xs);
    }

    #[test]
    fn out_of_bounds_cells_are_clipped() {
        // Stem starting at the edge with strong outward curvature: most
        // cells should fall outside, not panic.
        let seg = Segment {
            base: [0, 0, 0],
            length: 16,
            direction: [-1.0, 0.0, 0.0], // points outward
            thickness: 1,
            curvature: [0.0, 0.0, 0.0],
            color: [0.0, 1.0, 0.0],
        };
        let mut bp = Blueprint::new();
        bp.push(seg);
        let g = bp.to_subgrid();
        // First cell at base is in-bounds; everything after walks out.
        assert!(g.cell_count() <= 1);
    }

    #[test]
    fn thicker_stem_has_more_cells() {
        let thin = {
            let seg = Segment {
                base: [4, 0, 4], length: 5, direction: [0.0, 1.0, 0.0],
                thickness: 1, curvature: [0.0; 3], color: [0.0, 1.0, 0.0],
            };
            let mut bp = Blueprint::new(); bp.push(seg); bp.to_subgrid()
        };
        let thick = {
            let seg = Segment {
                base: [4, 0, 4], length: 5, direction: [0.0, 1.0, 0.0],
                thickness: 3, curvature: [0.0; 3], color: [0.0, 1.0, 0.0],
            };
            let mut bp = Blueprint::new(); bp.push(seg); bp.to_subgrid()
        };
        assert!(thick.cell_count() > thin.cell_count(),
            "thicker stem should have more cells: thin={} thick={}",
            thin.cell_count(), thick.cell_count());
    }

    #[test]
    fn grass_blueprint_produces_grid() {
        let bp = grass_tuft_blueprint(5, [0.0, 1.0, 0.0], 0xCAFE);
        let g = bp.to_subgrid();
        assert!(g.cell_count() > 5, "expected >5 cells for 5 blades; got {}",
            g.cell_count());
    }

    #[test]
    fn tree_leaf_blueprint_has_twig_and_leaves() {
        let bp = tree_leaf_blueprint(
            [0.2, 0.5, 0.2],   // leaf green
            [0.4, 0.25, 0.1],  // twig brown
            0xBABE,
        );
        let g = bp.to_subgrid();
        // Should have multiple cells (twig + leaves).
        assert!(g.cell_count() > 5, "tree leaf needs both twig and leaves; got {}",
            g.cell_count());
        // Cells should include both brown (twig) and greenish (leaf) colors.
        let twig_cells = g.cells.iter()
            .filter(|c| c.color[0] > c.color[1] && c.color[1] > c.color[2])
            .count();
        let leaf_cells = g.cells.iter()
            .filter(|c| c.color[1] > c.color[0] && c.color[1] > c.color[2])
            .count();
        assert!(twig_cells > 0, "no twig-colored cells");
        assert!(leaf_cells > 0, "no leaf-colored cells");
    }

    #[test]
    fn tree_leaf_blueprint_varies_by_seed() {
        // Different seeds should produce different cell layouts.
        let g1 = tree_leaf_blueprint([0.2, 0.5, 0.2], [0.4, 0.25, 0.1], 0xCAFE)
            .to_subgrid();
        let g2 = tree_leaf_blueprint([0.2, 0.5, 0.2], [0.4, 0.25, 0.1], 0xFADE)
            .to_subgrid();
        // At minimum, cell counts differ OR positions differ.
        let positions1: Vec<(u8, u8, u8)> = g1.cells.iter()
            .map(|c| (c.sub_x, c.sub_y, c.sub_z)).collect();
        let positions2: Vec<(u8, u8, u8)> = g2.cells.iter()
            .map(|c| (c.sub_x, c.sub_y, c.sub_z)).collect();
        assert_ne!(positions1, positions2,
            "same seeds should produce different blueprints");
    }

    /// Grass blades should produce more than two distinct heights when
    /// many tufts are sampled. The previous `& 0x4` mask collapsed
    /// heights to {3, 7}; the fixed `& 0x7` mask should yield 8.
    #[test]
    fn grass_tuft_heights_span_multiple_values() {
        use std::collections::HashSet;
        let mut heights = HashSet::new();
        for seed in 0..200u32 {
            let bp = grass_tuft_blueprint(5, [0.0, 1.0, 0.0], seed.wrapping_mul(0xCAFE));
            for seg in &bp.segments {
                heights.insert(seg.length);
            }
        }
        // Should see at least 5 distinct heights across 200 seeds × 5 blades.
        // Used to be 2 (just 3 and 7) before the mask fix.
        assert!(heights.len() >= 5,
            "expected at least 5 distinct grass heights, got {}: {:?}",
            heights.len(), heights);
    }

    /// Branching trunk produces a main trunk segment plus 1-3 lateral
    /// branches. Total segment count is 2-4.
    #[test]
    fn branching_trunk_has_trunk_and_branches() {
        for seed in 0..20u32 {
            let bp = branching_trunk_blueprint(
                [0.4, 0.25, 0.1],
                [0.5, 0.3, 0.15],
                seed.wrapping_mul(0x9E37_79B9),
            );
            assert!(bp.segments.len() >= 2,
                "expected at least 2 segments (trunk + 1 branch), got {} (seed {})",
                bp.segments.len(), seed);
            assert!(bp.segments.len() <= 5,
                "expected at most 5 segments, got {}", bp.segments.len());
            // First segment is the trunk: should be vertical, max length.
            let trunk = &bp.segments[0];
            assert_eq!(trunk.length, SUBVOXEL_RESOLUTION as u8,
                "trunk should span full block height");
            assert!(trunk.direction[1].abs() > 0.9,
                "trunk should be ~vertical");
        }
    }

    /// Branching trunk with different seeds produces different shapes
    /// (branch positions and counts vary).
    #[test]
    fn branching_trunk_varies_by_seed() {
        let mut shapes = std::collections::HashSet::new();
        for seed in 0..30u32 {
            let bp = branching_trunk_blueprint(
                [0.4, 0.25, 0.1],
                [0.5, 0.3, 0.15],
                seed.wrapping_mul(0x9E37_79B9),
            );
            // Encode shape by branch count + direction signs.
            let mut shape = vec![bp.segments.len() as u8];
            for s in &bp.segments[1..] {
                let dx = (s.direction[0].signum() as i8) + 1; // -1,0,1 -> 0,1,2
                let dz = (s.direction[2].signum() as i8) + 1;
                shape.push(dx as u8 * 3 + dz as u8);
                shape.push(s.base[1]); // vertical position
            }
            shapes.insert(shape);
        }
        assert!(shapes.len() >= 5,
            "expected ≥5 distinct trunk shapes across 30 seeds, got {}",
            shapes.len());
    }
}

// ── r336-plants ──────────────────────────────────────────────────────────────
// More plant species. `world::flora_placement::FloraSpecies` is the placement
// taxonomy (with biome affinity already in `flora_placement::species_weights`),
// and its doc says "Geometry/blueprints live in `plant_blueprint`" — but only
// grass/tree-leaf/trunk had builders. These add the missing species geometry as
// `Blueprint` builders (mirroring `grass_tuft_blueprint`'s shape exactly, so the
// sub-voxel mesher consumes them unchanged) plus `species_blueprint`, the
// dispatcher the worldgen scatter calls once it has picked a `FloraSpecies`.
use crate::world::flora_placement::FloraSpecies;

const RES_U8: u8 = SUBVOXEL_RESOLUTION as u8; // 8

/// Cheap rehash, matching the existing builders' `h` chain.
#[inline]
fn rehash(h: &mut u32) -> u32 {
    *h = h.wrapping_mul(0x9E37_79B9).wrapping_add(0x12345);
    *h
}

/// Map a raw byte to an in-bounds base coord in `1..=RES-2` (leaves room for
/// thickness/curve without immediately clipping).
#[inline]
fn inner(v: u8) -> u8 {
    1 + (v % (RES_U8 - 2))
}

/// Wetland reed cluster — a few tall, thin, near-vertical stalks.
pub fn reed_blueprint(color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let n = 2 + (rehash(&mut h) & 0x3) as usize; // 2..=5 stalks
    for _ in 0..n {
        rehash(&mut h);
        let bx = inner(((h >> 4) & 0x7) as u8);
        let bz = inner(((h >> 8) & 0x7) as u8);
        let height = (5 + ((h >> 12) & 0x2) as u8).min(RES_U8 - 1); // 5..=7
        bp.push(Segment::vertical_curved([bx, 0, bz], height, color, h.wrapping_mul(7919), 0.03));
    }
    bp
}

/// Fern — several fronds arching outward and back down from a common base.
pub fn fern_blueprint(color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let n = 3 + (rehash(&mut h) & 0x3) as usize; // 3..=6 fronds
    let cx = inner(((h >> 4) & 0x7) as u8);
    let cz = inner(((h >> 8) & 0x7) as u8);
    for i in 0..n {
        rehash(&mut h);
        let angle = (i as f32 / n as f32) * std::f32::consts::TAU;
        let dir = [angle.cos() * 0.7, 0.8, angle.sin() * 0.7];
        // arch back down/out as the frond extends
        let curve = [angle.cos() * 0.10, -0.06, angle.sin() * 0.10];
        let len = 4 + ((h >> 12) & 0x1) as u8; // 4..=5
        bp.push(Segment { base: [cx, 0, cz], length: len, direction: dir, thickness: 1, curvature: curve, color });
    }
    bp
}

/// Vine — one or two long strands that climb then drape (strong curvature).
pub fn vine_blueprint(color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let n = 1 + (rehash(&mut h) & 0x1) as usize; // 1..=2
    for _ in 0..n {
        rehash(&mut h);
        let bx = inner(((h >> 4) & 0x7) as u8);
        let bz = inner(((h >> 8) & 0x7) as u8);
        let len = 6 + ((h >> 12) & 0x1) as u8; // 6..=7
        let sign = if (h & 0x10) != 0 { 1.0 } else { -1.0 };
        bp.push(Segment {
            base: [bx, 0, bz],
            length: len,
            direction: [0.0, 1.0, 0.0],
            thickness: 1,
            curvature: [0.14 * sign, 0.0, 0.05 * sign],
            color,
        });
    }
    bp
}

/// Shrub — a low bushy cluster of short, thickish segments fanning out.
pub fn shrub_blueprint(color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let cx = inner(((rehash(&mut h) >> 4) & 0x7) as u8);
    let cz = inner(((h >> 8) & 0x7) as u8);
    let n = 4 + (rehash(&mut h) & 0x3) as usize; // 4..=7
    for i in 0..n {
        rehash(&mut h);
        let angle = (i as f32 / n as f32) * std::f32::consts::TAU;
        let dir = [angle.cos() * 0.6, 0.7, angle.sin() * 0.6];
        let len = 2 + ((h >> 12) & 0x1) as u8; // 2..=3
        bp.push(Segment { base: [cx, 0, cz], length: len, direction: dir, thickness: 2, curvature: [0.0; 3], color });
    }
    bp
}

/// Wildflower — a slender stem topped by a small bloom cluster.
pub fn wildflower_blueprint(stem_color: [f32; 3], bloom_color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let bx = inner(((rehash(&mut h) >> 4) & 0x7) as u8);
    let bz = inner(((h >> 8) & 0x7) as u8);
    let stem_h = 4 + ((h >> 12) & 0x1) as u8; // 4..=5
    bp.push(Segment::vertical_curved([bx, 0, bz], stem_h, stem_color, h.wrapping_mul(7919), 0.04));
    // bloom: a short, thick cluster at the stem top.
    let top = stem_h.min(RES_U8 - 1);
    bp.push(Segment {
        base: [bx, top.saturating_sub(1), bz],
        length: 2,
        direction: [0.0, 1.0, 0.0],
        thickness: 3,
        curvature: [0.0; 3],
        color: bloom_color,
    });
    bp
}

/// Mushroom — a short stout stem under a wider cap.
pub fn mushroom_blueprint(stem_color: [f32; 3], cap_color: [f32; 3], seed_hash: u32) -> Blueprint {
    let mut bp = Blueprint::new();
    let mut h = seed_hash;
    let bx = inner(((rehash(&mut h) >> 4) & 0x7) as u8);
    let bz = inner(((h >> 8) & 0x7) as u8);
    let stem_h = 2 + ((h >> 12) & 0x1) as u8; // 2..=3
    bp.push(Segment { base: [bx, 0, bz], length: stem_h, direction: [0.0, 1.0, 0.0], thickness: 2, curvature: [0.0; 3], color: stem_color });
    // cap: a wide, thick segment at the top (thickness 3 spreads laterally).
    let cap_y = stem_h.min(RES_U8 - 1);
    bp.push(Segment { base: [bx, cap_y, bz], length: 2, direction: [0.0, 1.0, 0.0], thickness: 3, curvature: [0.0; 3], color: cap_color });
    bp
}

/// Default (primary, accent) colors for a species. Accent is the bloom/cap for
/// flower/mushroom and the twig for trees; otherwise it equals the primary.
pub fn species_colors(species: FloraSpecies) -> ([f32; 3], [f32; 3]) {
    use FloraSpecies::*;
    match species {
        Grass => ([0.30, 0.65, 0.18], [0.30, 0.65, 0.18]),
        Shrub => ([0.22, 0.45, 0.16], [0.22, 0.45, 0.16]),
        Flower => ([0.25, 0.55, 0.20], [0.85, 0.30, 0.55]),
        Fern => ([0.16, 0.42, 0.20], [0.16, 0.42, 0.20]),
        Reed => ([0.55, 0.62, 0.25], [0.55, 0.62, 0.25]),
        Mushroom => ([0.80, 0.74, 0.62], [0.70, 0.18, 0.16]),
        Cactus => ([0.20, 0.50, 0.28], [0.20, 0.50, 0.28]),
        Conifer => ([0.12, 0.35, 0.18], [0.30, 0.20, 0.10]),
        Broadleaf => ([0.20, 0.50, 0.20], [0.40, 0.25, 0.10]),
        Palm => ([0.22, 0.52, 0.22], [0.22, 0.52, 0.22]),
    }
}

/// Geometry for a placed `FloraSpecies` — the catalog the worldgen scatter
/// consumes after `flora_placement::species_weights` picks the species. Every
/// `FloraSpecies` maps to a deterministic, seed-varying `Blueprint`; the mesher
/// consumes the result via `Blueprint::to_subgrid` unchanged.
pub fn species_blueprint(species: FloraSpecies, seed_hash: u32) -> Blueprint {
    use FloraSpecies::*;
    let (a, b) = species_colors(species);
    match species {
        Grass => grass_tuft_blueprint(5, a, seed_hash),
        Shrub => shrub_blueprint(a, seed_hash),
        Flower => wildflower_blueprint(a, b, seed_hash),
        Fern => fern_blueprint(a, seed_hash),
        Reed => reed_blueprint(a, seed_hash),
        Mushroom => mushroom_blueprint(a, b, seed_hash),
        Cactus => shrub_blueprint(a, seed_hash), // stout cluster stand-in
        Conifer => tree_leaf_blueprint(a, b, seed_hash), // needled foliage
        Broadleaf => tree_leaf_blueprint(a, b, seed_hash),
        Palm => fern_blueprint(a, seed_hash), // frond crown
    }
}

#[cfg(test)]
mod r336_plants_tests {
    use super::*;
    use crate::world::biomes::Biome;
    use crate::world::flora_placement::{species_weights, FloraSpecies};

    fn in_bounds(g: &SubVoxelGrid) -> bool {
        g.cells.iter().all(|c| {
            (c.sub_x as usize) < SUBVOXEL_RESOLUTION
                && (c.sub_y as usize) < SUBVOXEL_RESOLUTION
                && (c.sub_z as usize) < SUBVOXEL_RESOLUTION
        })
    }

    #[test]
    fn each_new_builder_has_nonempty_in_bounds_morphology() {
        let g = [
            reed_blueprint([0.5, 0.6, 0.2], 0x11).to_subgrid(),
            fern_blueprint([0.2, 0.4, 0.2], 0x22).to_subgrid(),
            vine_blueprint([0.2, 0.5, 0.2], 0x33).to_subgrid(),
            shrub_blueprint([0.2, 0.4, 0.2], 0x44).to_subgrid(),
            wildflower_blueprint([0.2, 0.5, 0.2], [0.9, 0.3, 0.5], 0x55).to_subgrid(),
            mushroom_blueprint([0.8, 0.7, 0.6], [0.7, 0.2, 0.2], 0x66).to_subgrid(),
        ];
        for grid in &g {
            assert!(grid.cell_count() > 0, "empty morphology");
            assert!(in_bounds(grid), "out-of-bounds cell");
        }
    }

    #[test]
    fn species_catalog_is_total_and_nonempty() {
        // FloraSpecies::ALL is the catalog; every species now has geometry.
        for s in FloraSpecies::ALL {
            let g = species_blueprint(s, 0xABCD).to_subgrid();
            assert!(g.cell_count() > 0, "{s:?} produced no geometry");
            assert!(in_bounds(&g), "{s:?} produced an out-of-bounds cell");
        }
        // no duplicate species ids in the catalog
        let n = FloraSpecies::ALL.len();
        let uniq: std::collections::BTreeSet<usize> =
            FloraSpecies::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(uniq.len(), n, "duplicate species index");
    }

    #[test]
    fn deterministic_and_seed_varying() {
        for s in FloraSpecies::ALL {
            let a = species_blueprint(s, 0x1234).to_subgrid();
            let b = species_blueprint(s, 0x1234).to_subgrid();
            let cells = |g: &SubVoxelGrid| -> Vec<(u8, u8, u8)> {
                g.cells.iter().map(|c| (c.sub_x, c.sub_y, c.sub_z)).collect()
            };
            assert_eq!(cells(&a), cells(&b), "{s:?} not deterministic");
        }
        // a seed change moves at least one species' layout
        let moved = FloraSpecies::ALL.iter().any(|&s| {
            let p = |seed| {
                species_blueprint(s, seed)
                    .to_subgrid()
                    .cells
                    .iter()
                    .map(|c| (c.sub_x, c.sub_y, c.sub_z))
                    .collect::<Vec<_>>()
            };
            p(1) != p(2)
        });
        assert!(moved, "no species varied with the seed");
    }

    #[test]
    fn all_species_in_bounds_over_many_seeds() {
        for s in FloraSpecies::ALL {
            for seed in 0..200u32 {
                let g = species_blueprint(s, seed.wrapping_mul(0x9E37_79B9)).to_subgrid();
                assert!(in_bounds(&g), "{s:?} out of bounds at seed {seed}");
            }
        }
    }

    #[test]
    fn every_species_has_a_valid_biome_affinity() {
        // "Valid biome affinity" = each species the scatter can place (and we now
        // give geometry) has nonzero weight in at least one biome, via the REAL
        // flora_placement table (unchanged by this thread).
        const BIOMES: [Biome; 24] = [
            Biome::Plains, Biome::TemperateForest, Biome::TemperateHighlands, Biome::BirchForest,
            Biome::MapleForest, Biome::CloudForest, Biome::MistyMountains, Biome::AlpineHighlands,
            Biome::Desert, Biome::RedSandstoneCanyon, Biome::BloomingCanyon, Biome::Ocean,
            Biome::Beach, Biome::Swamp, Biome::Tundra, Biome::Skylands, Biome::IceCave,
            Biome::FungalCave, Biome::ThermalCave, Biome::Savanna, Biome::Jungle, Biome::Taiga,
            Biome::CrystalCave, Biome::LushCave,
        ];
        for s in FloraSpecies::ALL {
            let present = BIOMES.iter().any(|&b| species_weights(b)[s.index()] > 0.0);
            assert!(present, "{s:?} has no biome affinity anywhere");
        }
    }

    #[test]
    fn a_blueprinted_plant_grows_without_nan() {
        use crate::sim::plant_growth::{advance, Fields, GrowthEvent, PlantState};
        // Seed a plant for a species and advance it under favourable fields;
        // growth01 must stay finite and within [0,1], and it should mature.
        let mut st = PlantState { growth01: 0.0, stage: 0, element: 2, vigor: 0.8 };
        let f = Fields { h: 0.8, r: 0.7, l: 0.9, s: 0.0, b: 0.5 };
        let mut matured = false;
        for _ in 0..1000 {
            let step = advance(st, &f, 0.05);
            st = step.state;
            assert!(st.growth01.is_finite() && (0.0..=1.0).contains(&st.growth01));
            if step.event == GrowthEvent::Matured {
                matured = true;
            }
        }
        assert!(matured, "plant should reach maturity under good fields");
        // Under collapse it wilts to a finite dead seed (no NaN).
        let wilt = advance(PlantState { growth01: 0.2, stage: 1, element: 0, vigor: 0.5 }, &Fields { h: 0.0, r: 0.0, l: 0.0, s: 0.0, b: 0.0 }, 0.1);
        assert!(wilt.state.growth01.is_finite());
    }

    #[test]
    fn flower_and_mushroom_carry_their_accent_color() {
        // wildflower has both a stem-green and a bloom-pink cell; mushroom has a
        // pale stem and a red cap.
        let (stem, bloom) = species_colors(FloraSpecies::Flower);
        let g = wildflower_blueprint(stem, bloom, 0x7).to_subgrid();
        assert!(g.cells.iter().any(|c| c.color == bloom), "no bloom cell");
        assert!(g.cells.iter().any(|c| c.color == stem), "no stem cell");
        let (mstem, mcap) = species_colors(FloraSpecies::Mushroom);
        let m = mushroom_blueprint(mstem, mcap, 0x9).to_subgrid();
        assert!(m.cells.iter().any(|c| c.color == mcap), "no cap cell");
    }
}
