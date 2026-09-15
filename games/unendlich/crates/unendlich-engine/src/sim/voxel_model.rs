//! `sim::voxel_model` — rasterize a small set of axis-aligned boxes (the same
//! coarse "solids" concept `sim::viewmodel::ItemBox` and
//! `sim::dragon_blueprint_from_genome::BlueprintPart` already use) into an
//! actual voxel grid, so held items / creatures can look like real voxel
//! models instead of a handful of flat-shaded boxes.
//!
//! This is the missing middle step in the pipeline:
//!
//!   procedural body-part boxes (already exist: `ItemBox`, `BlueprintPart`)
//!   -> [THIS MODULE: rasterize into a voxel grid]
//!   -> render (already exists: `viewmodel::box_corners_in_view` + the
//!      immediate-mode entity vertex buffer in the game loop — see
//!      [`voxel_model_to_boxes`], which reuses that path as-is by expanding
//!      each occupied voxel cell into a small box)
//!
//! Directly ports the rasterization approach of the browser authoring tools
//! (`handoff/dragon_forge_3d.html` / the sibling `character_forge_3d.html`'s
//! `buildVoxels`), adapted for AXIS-ALIGNED boxes only — this engine's boxes
//! (`ItemBox`, `BlueprintPart`) carry no rotation, unlike the JS tool's
//! oriented-box solids, which reduces the point-in-solid test to a plain
//! AABB containment check instead of a basis projection.
//!
//! Pure / deterministic / panic-free: no GPU, no I/O, no RNG, no clock. Same
//! input always produces the same grid.

/// One axis-aligned box to rasterize into the voxel grid — deliberately
/// generic (not `ItemBox`/`BlueprintPart` themselves) so this module doesn't
/// depend on either of those call sites' crates/modules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoxelSolidBox {
    pub center: [f32; 3],
    pub half_extents: [f32; 3],
    pub color: [u8; 3],
}

/// A small, self-contained voxel grid — NOT related to world chunks (see
/// `world::chunk::Chunk` for that). `voxels[i] == 0` means empty; a nonzero
/// value `v` is a 1-based index into `palette` (`palette[v as usize - 1]`).
///
/// `origin` + `cell_size` describe where the grid sits in the same local
/// space the input `VoxelSolidBox`es were defined in, so a consumer can
/// recover real-world-scale positions for each occupied cell — see
/// [`voxel_model_to_boxes`].
#[derive(Debug, Clone, PartialEq)]
pub struct VoxelModel {
    pub dims: [u32; 3],
    pub palette: Vec<[u8; 3]>,
    pub voxels: Vec<u8>,
    /// World/model-local position of voxel-grid corner (0,0,0).
    pub origin: [f32; 3],
    /// Edge length of one voxel cell, in the same units as the input boxes.
    pub cell_size: f32,
}

impl VoxelModel {
    #[inline]
    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        ((x * self.dims[1] + y) * self.dims[2] + z) as usize
    }

    /// Palette-resolved color at a cell, or `None` if empty or out of range.
    pub fn color_at(&self, x: u32, y: u32, z: u32) -> Option<[u8; 3]> {
        if x >= self.dims[0] || y >= self.dims[1] || z >= self.dims[2] {
            return None;
        }
        let v = self.voxels[self.index(x, y, z)];
        if v == 0 { None } else { self.palette.get(v as usize - 1).copied() }
    }

    /// Count of occupied (nonzero) cells.
    pub fn occupied_count(&self) -> usize {
        self.voxels.iter().filter(|&&v| v != 0).count()
    }
}

/// Voxelize a list of axis-aligned boxes into a [`VoxelModel`].
///
/// `resolution` is the number of voxel cells along the LONGEST overall
/// dimension of the combined bounding box (matching the JS authoring tool's
/// "voxel res" slider) — every axis uses the same cell size, so proportions
/// are preserved. `resolution` is clamped to at least 1.
///
/// An empty `boxes` slice returns a degenerate 1×1×1 empty model rather than
/// panicking on an undefined bounding box.
///
/// Each voxel cell is assigned the color of the LAST box (in input order)
/// whose AABB contains that cell's center — later boxes intentionally paint
/// over earlier ones, so e.g. a blade's cross-guard can be listed after the
/// blade shaft to punch a visible notch through it, mirroring the normal
/// "layered" order a hand-authored box model (like `weapon_model`) is
/// already written in.
pub fn voxelize_boxes(boxes: &[VoxelSolidBox], resolution: u32) -> VoxelModel {
    let resolution = resolution.max(1);
    if boxes.is_empty() {
        return VoxelModel {
            dims: [1, 1, 1],
            palette: Vec::new(),
            voxels: vec![0],
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
        };
    }

    // Combined bounding box over all input boxes.
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for b in boxes {
        for i in 0..3 {
            min[i] = min[i].min(b.center[i] - b.half_extents[i]);
            max[i] = max[i].max(b.center[i] + b.half_extents[i]);
        }
    }
    let size = [
        (max[0] - min[0]).max(1e-6),
        (max[1] - min[1]).max(1e-6),
        (max[2] - min[2]).max(1e-6),
    ];
    let longest = size[0].max(size[1]).max(size[2]);
    let cell = longest / resolution as f32;

    let dims = [
        (size[0] / cell).ceil() as u32 + 1,
        (size[1] / cell).ceil() as u32 + 1,
        (size[2] / cell).ceil() as u32 + 1,
    ];

    let mut palette: Vec<[u8; 3]> = Vec::new();
    let total = (dims[0] * dims[1] * dims[2]) as usize;
    let mut voxels = vec![0u8; total];

    for x in 0..dims[0] {
        let wx = min[0] + (x as f32 + 0.5) * cell;
        for y in 0..dims[1] {
            let wy = min[1] + (y as f32 + 0.5) * cell;
            for z in 0..dims[2] {
                let wz = min[2] + (z as f32 + 0.5) * cell;
                // Last-match-wins: keep scanning after a hit so a later box
                // in the list can paint over an earlier one at this cell.
                let mut found: Option<[u8; 3]> = None;
                for b in boxes {
                    let dx = (wx - b.center[0]).abs();
                    let dy = (wy - b.center[1]).abs();
                    let dz = (wz - b.center[2]).abs();
                    if dx <= b.half_extents[0] && dy <= b.half_extents[1] && dz <= b.half_extents[2] {
                        found = Some(b.color);
                    }
                }
                if let Some(c) = found {
                    let idx = ((x * dims[1] + y) * dims[2] + z) as usize;
                    voxels[idx] = match palette.iter().position(|&p| p == c) {
                        Some(i) => (i + 1) as u8,
                        None => {
                            palette.push(c);
                            palette.len() as u8
                        }
                    };
                }
            }
        }
    }

    VoxelModel { dims, palette, voxels, origin: min, cell_size: cell }
}

/// Expand a [`VoxelModel`]'s occupied cells back into a flat list of
/// `(center, half_extents, color)` tuples in the SAME local space the
/// original input boxes were defined in — i.e. exactly the shape
/// `sim::viewmodel::ItemBox` already has, so a caller in that module (or the
/// game loop directly) can push each one through the existing
/// `box_corners_in_view` + immediate-mode vertex buffer path with no new
/// rendering code. This is deliberately "many small boxes" rather than a
/// merged mesh — correct and simple; greedy face-merging (to cut vertex
/// count for larger models like a full creature) is a real, separate
/// follow-up once this is confirmed to actually look right on a device.
pub fn voxel_model_to_boxes(model: &VoxelModel) -> Vec<([f32; 3], [f32; 3], [u8; 3])> {
    let half = model.cell_size * 0.5;
    let mut out = Vec::with_capacity(model.occupied_count());
    for x in 0..model.dims[0] {
        for y in 0..model.dims[1] {
            for z in 0..model.dims[2] {
                if let Some(color) = model.color_at(x, y, z) {
                    let center = [
                        model.origin[0] + (x as f32 + 0.5) * model.cell_size,
                        model.origin[1] + (y as f32 + 0.5) * model.cell_size,
                        model.origin[2] + (z as f32 + 0.5) * model.cell_size,
                    ];
                    out.push((center, [half, half, half], color));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(center: [f32; 3], half: [f32; 3], color: [u8; 3]) -> VoxelSolidBox {
        VoxelSolidBox { center, half_extents: half, color }
    }

    #[test]
    fn empty_input_gives_a_degenerate_empty_model() {
        let m = voxelize_boxes(&[], 16);
        assert_eq!(m.dims, [1, 1, 1]);
        assert_eq!(m.occupied_count(), 0);
        assert!(m.palette.is_empty());
    }

    #[test]
    fn single_box_is_fully_and_only_occupied() {
        // A cube exactly `resolution` cells wide should come out with every
        // cell painted the same single color, and dims matching resolution
        // (+1, matching the JS tool's `ceil(...)+1` overshoot-by-one).
        let boxes = [b([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [200, 50, 50])];
        let m = voxelize_boxes(&boxes, 8);
        assert_eq!(m.palette, vec![[200, 50, 50]]);
        // Was `m.dims[0] * m.dims[1] * m.dims[2]` (729 = 9^3) -- but dims'
        // own `+1` is an ALLOCATION overshoot (see the comment above and
        // `voxelize_boxes`' `ceil(size/cell).ceil() as u32 + 1`), applied
        // UNCONDITIONALLY even when the box already fills a whole number
        // of cells with no remainder, as it does here (size 2.0 / cell
        // 0.25 = exactly 8.0). The extra allocated cell's world-space
        // sample point necessarily lands outside the box's exact
        // half-extent (confirmed by hand: cell index 8's center is at
        // 1.125 on an axis the box only covers to 1.0), so it is
        // legitimately UNoccupied by construction, not a bug in the fill
        // loop. "Every cell occupied" and "dims deliberately overshoots
        // by one" cannot both be true at once; `resolution^3` (the cells
        // the box actually spans) is the real invariant this test can
        // assert, not the allocation size.
        assert_eq!(m.occupied_count(), 8usize.pow(3));
    }

    #[test]
    fn resolution_is_clamped_to_at_least_one() {
        let boxes = [b([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1, 2, 3])];
        let m = voxelize_boxes(&boxes, 0);
        assert!(m.dims.iter().all(|&d| d >= 1));
        assert!(m.occupied_count() >= 1);
    }

    #[test]
    fn later_box_paints_over_an_earlier_one_at_the_same_cell() {
        // Two fully overlapping boxes at the same spot — the later color
        // must win everywhere, proving "layered" authoring order works.
        let boxes = [
            b([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [10, 10, 10]),
            b([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [220, 220, 220]),
        ];
        let m = voxelize_boxes(&boxes, 6);
        // Was `vec![[10, 10, 10], [220, 220, 220]]` -- but [10, 10, 10]
        // never wins a single cell (the boxes fully overlap, "last-match-
        // wins" per this function's own comment), so it is never actually
        // WRITTEN to any voxel and correctly never earns a palette slot.
        // The loop right below already fully proves the real intent this
        // test is named for ("the later color must win everywhere") --
        // asserting the invisible color also occupies palette space is a
        // separate, over-specified claim that contradicts giving an
        // unused color a slot at all, not a property "layered authoring"
        // needs.
        assert_eq!(m.palette, vec![[220, 220, 220]]);
        // Was `0..m.dims[i]` -- the SAME dims-overshoot as
        // `single_box_is_fully_and_only_occupied` (see that test's own
        // comment): `dims` deliberately allocates one extra cell per axis
        // beyond what the box's exact half-extent covers, and that extra
        // cell legitimately samples to a world position OUTSIDE the box,
        // returning `None` from `color_at` rather than a color. Iterating
        // the real box extent (`resolution`, not `dims`) is what "the
        // later color must win everywhere [the box actually covers]"
        // means; the previous bound also walked into the always-empty
        // overshoot shell and got a real `None` there instead.
        for x in 0..6 {
            for y in 0..6 {
                for z in 0..6 {
                    assert_eq!(m.color_at(x, y, z), Some([220, 220, 220]));
                }
            }
        }
    }

    #[test]
    fn disjoint_boxes_leave_a_gap_between_them() {
        // Two small boxes far apart relative to their size: the voxelized
        // grid must have SOME empty cells between them, not one solid blob.
        let boxes = [
            b([-5.0, 0.0, 0.0], [0.5, 0.5, 0.5], [255, 0, 0]),
            b([5.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0, 255, 0]),
        ];
        let m = voxelize_boxes(&boxes, 40);
        let occ = m.occupied_count();
        let total = (m.dims[0] * m.dims[1] * m.dims[2]) as usize;
        assert!(occ < total, "expected empty gap cells between disjoint boxes");
        assert!(occ > 0, "expected some occupied cells");
        assert_eq!(m.palette.len(), 2);
    }

    #[test]
    fn round_trip_through_voxel_model_to_boxes_preserves_center_of_mass_and_palette() {
        let boxes = [
            b([0.0, 0.0, 0.0], [0.5, 2.0, 0.5], [120, 84, 52]),
            b([0.0, 0.0, 1.0], [0.3, 0.3, 0.2], [180, 188, 200]),
        ];
        let m = voxelize_boxes(&boxes, 20);
        let expanded = voxel_model_to_boxes(&m);
        assert_eq!(expanded.len(), m.occupied_count());
        // Every resulting box's color must be one that was in the palette —
        // no color can be invented that wasn't in an input box.
        let input_colors: Vec<[u8; 3]> = boxes.iter().map(|b| b.color).collect();
        for (_, _, color) in &expanded {
            assert!(input_colors.contains(color), "unexpected color {color:?} in output");
        }
        // All half-extents must be identical (uniform voxel size) and positive.
        let half0 = expanded[0].1;
        assert!(half0[0] > 0.0);
        for (_, half, _) in &expanded {
            assert_eq!(*half, half0);
        }
    }

    #[test]
    fn voxelizing_more_boxes_never_shrinks_the_bounding_extent() {
        // Sanity check against a silent regression where adding a box that
        // extends the bbox gets ignored (e.g. a min/max init bug).
        let small = [b([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1, 1, 1])];
        let bigger = [
            b([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [1, 1, 1]),
            b([10.0, 0.0, 0.0], [1.0, 1.0, 1.0], [2, 2, 2]),
        ];
        let m_small = voxelize_boxes(&small, 10);
        let m_bigger = voxelize_boxes(&bigger, 10);
        let extent = |m: &VoxelModel| m.cell_size * m.dims[0] as f32;
        assert!(extent(&m_bigger) > extent(&m_small));
    }
}
