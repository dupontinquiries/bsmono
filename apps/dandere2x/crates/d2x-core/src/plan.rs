//! The data the analysis stage hands to the merge stage: for every block of
//! the next frame, where its pixels come from.
//!
//! This is the in-memory form of upstream's `pframe_N.txt` / `residual_N.txt`
//! / `fade_N.txt` triple. Nothing here touches pixels.

use serde::{Deserialize, Serialize};

/// The block lattice over a frame whose dimensions are multiples of
/// `block_size`. The pipeline pads frames up to that before analysis, so
/// blocks never overlap and never run off the edge — upstream instead
/// silently never updates the ragged edge strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub block_size: u32,
    pub cols: u32,
    pub rows: u32,
}

impl Grid {
    pub fn new(width: u32, height: u32, block_size: u32) -> Self {
        assert!(block_size > 0);
        assert!(
            width % block_size == 0 && height % block_size == 0,
            "Grid::new: {width}x{height} is not a multiple of block size {block_size}"
        );
        Grid { block_size, cols: width / block_size, rows: height / block_size }
    }

    #[inline]
    pub fn len(&self) -> usize {
        (self.cols as usize) * (self.rows as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pixel origin of cell `i` (row-major).
    #[inline]
    pub fn origin(&self, i: usize) -> (u32, u32) {
        let bx = (i % self.cols as usize) as u32;
        let by = (i / self.cols as usize) as u32;
        (bx * self.block_size, by * self.block_size)
    }

    pub fn width(&self) -> u32 {
        self.cols * self.block_size
    }

    pub fn height(&self) -> u32 {
        self.rows * self.block_size
    }
}

/// Where one block of the next frame comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockMatch {
    /// Same position in the previous frame passes the quality gate.
    Stationary,
    /// The block at `(src_x, src_y)` in the previous frame passes the gate.
    Moved { src_x: u32, src_y: u32 },
    /// Same position in an older reference (`r` indexes the reference
    /// list: recent frames first, then retained shot starts) passes the gate.
    Recalled { r: u8 },
    /// The block's cell content was upscaled before; the block cache holds
    /// the result under `key`.
    Cached { key: u64 },
    /// The block at `(px, py)` of the window's plate passes the gate. Plate
    /// coordinates; the merge reads the plate's upscale at `(px·s, py·s)`.
    Plated { px: u32, py: u32 },
    /// The block at half-pel position `(sx2 / 2, sy2 / 2)` of the previous
    /// frame passes the gate. In the upscaled domain that is the integer
    /// position `(sx2·s / 2, sy2·s / 2)`, an exact copy for even scales.
    MovedHalf { sx2: u32, sy2: u32 },
    /// Quarter-pel: `(sx4 / 4, sy4 / 4)` in the previous frame; an exact
    /// copy at `(sx4·s / 4, sy4·s / 4)` when `s` is a multiple of 4.
    MovedQuarter { sx4: u32, sy4: u32 },
    /// The block at `(src_x, src_y)` of older reference `r` (the anchor —
    /// the last redrawn frame, whose output is a fresh network pass) passes
    /// the anchor gate. Propagation from a restored frame.
    RecalledMoved { r: u8, src_x: u32, src_y: u32 },
    /// Nothing in any reference passes; the block must be upscaled.
    Missing,
}

/// Number of upscaler tiers a plan can route to: 0 = the main network,
/// 1 = a cheaper network for smooth blocks, 2 = bicubic for flat blocks.
pub const MAX_TIERS: usize = 3;

/// Maps a missing block at `(frame_x, frame_y)` to cell `(cell_x, cell_y)`
/// of the residual image of `tier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidualVector {
    pub frame_x: u32,
    pub frame_y: u32,
    pub cell_x: u32,
    pub cell_y: u32,
    pub tier: u8,
    /// Content hash of the cell, the block cache key once it is upscaled.
    pub key: u64,
}

/// A flat brightness shift applied to a block of the previous frame before
/// matching — catches fades and global lighting changes as one integer per
/// block instead of a residual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FadeBlock {
    pub x: u32,
    pub y: u32,
    pub scalar: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct FrameStats {
    pub psnr: f64,
    pub stationary: u32,
    pub moved: u32,
    pub missing: u32,
    pub fades: u32,
    /// Blocks recovered from older references.
    pub recalled: u32,
    /// Blocks served from the content-keyed block cache.
    pub cached: u32,
    /// Blocks served from the window's plate.
    pub plated: u32,
    /// Blocks matched at a half-pel offset in the previous frame.
    pub moved_half: u32,
    /// Blocks matched at a quarter-pel offset in the previous frame.
    pub moved_quarter: u32,
    /// Blocks propagated from the anchor with motion.
    pub anchored: u32,
    /// Region redraw: the bounding rectangle of missing blocks, in frame
    /// pixels (block-aligned), upscaled as one image instead of packed
    /// cells or the whole frame. `(x, y, w, h)`.
    pub region: Option<(u32, u32, u32, u32)>,
    /// Pixels of the padded region image (0 without a region).
    pub region_px: u64,
    /// Residual image size in cells per tier (0×0 if none). Rectangular:
    /// `cols = ceil(sqrt(n))`, `rows = ceil(n / cols)`, so at most `cols − 1`
    /// cells are wasted instead of up to `2·sqrt(n)` for a square.
    pub residual_tiers: [(u32, u32); MAX_TIERS],
    pub redraw: bool,
}

impl FrameStats {
    /// Tier-0 residual size in cells.
    pub fn residual_cols(&self) -> u32 {
        self.residual_tiers[0].0
    }
    pub fn residual_rows(&self) -> u32 {
        self.residual_tiers[0].1
    }
}

impl FrameStats {
    /// Fraction of the frame's pixels that are re-upscaled this frame, in
    /// terms of the residual image's area relative to the frame. This is the
    /// number dandere2x exists to make small.
    pub fn upscaled_fraction(&self, grid: &Grid, bleed: u32) -> f64 {
        if self.redraw {
            return 1.0;
        }
        let cell = (grid.block_size + 2 * bleed) as f64;
        let frame_px = (grid.width() as f64) * (grid.height() as f64);
        // Tiers 0 and 1 are networks; tier 2 is bicubic and costs nothing worth counting.
        let res_px: f64 = self.residual_tiers.iter().take(2).map(|&(c, r)| (c as f64 * cell) * (r as f64 * cell)).sum();
        (res_px + self.region_px as f64) / frame_px
    }

    /// Fraction of the frame's pixels that went through the *main* network.
    pub fn heavy_fraction(&self, grid: &Grid, bleed: u32) -> f64 {
        if self.redraw {
            return 1.0;
        }
        let cell = (grid.block_size + 2 * bleed) as f64;
        let (c, r) = self.residual_tiers[0];
        ((c as f64 * cell) * (r as f64 * cell) + self.region_px as f64) / ((grid.width() as f64) * (grid.height() as f64))
    }

    /// A frame worth keeping as an anchor: a full redraw, or a region redraw
    /// covering at least half the frame — either way most of its output is
    /// a fresh network pass.
    pub fn is_anchor(&self, grid: &Grid) -> bool {
        self.redraw || self.region.map_or(false, |(_, _, w, h)| (w as u64) * (h as u64) * 2 >= (grid.width() as u64) * (grid.height() as u64))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FramePlan {
    pub grid: Grid,
    /// One entry per grid cell, row-major.
    pub matches: Vec<BlockMatch>,
    pub residuals: Vec<ResidualVector>,
    pub fades: Vec<FadeBlock>,
    /// The whole frame is upscaled and the plan's blocks are ignored.
    pub redraw: bool,
    pub stats: FrameStats,
}

impl FramePlan {
    pub fn redraw(grid: Grid, psnr: f64) -> Self {
        let n = grid.len();
        FramePlan {
            grid,
            matches: vec![BlockMatch::Missing; n],
            residuals: Vec::new(),
            fades: Vec::new(),
            redraw: true,
            stats: FrameStats { psnr, missing: n as u32, redraw: true, ..Default::default() },
        }
    }

    /// Residual image size in cells `(cols, rows)` for `n` missing blocks.
    pub fn residual_dims_for(n: usize) -> (u32, u32) {
        if n == 0 {
            return (0, 0);
        }
        let cols = (n as f64).sqrt().ceil() as u32;
        (cols, (n as u32).div_ceil(cols))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_origins_are_row_major() {
        let g = Grid::new(60, 30, 30);
        assert_eq!(g.len(), 2);
        assert_eq!(g.origin(0), (0, 0));
        assert_eq!(g.origin(1), (30, 0));
        let g = Grid::new(30, 60, 30);
        assert_eq!(g.origin(1), (0, 30));
    }

    #[test]
    fn residual_dims_are_rectangular() {
        assert_eq!(FramePlan::residual_dims_for(0), (0, 0));
        assert_eq!(FramePlan::residual_dims_for(1), (1, 1));
        assert_eq!(FramePlan::residual_dims_for(4), (2, 2));
        assert_eq!(FramePlan::residual_dims_for(5), (3, 2));
        assert_eq!(FramePlan::residual_dims_for(9), (3, 3));
        assert_eq!(FramePlan::residual_dims_for(50), (8, 7));
        for n in 1..500usize {
            let (c, r) = FramePlan::residual_dims_for(n);
            assert!((c * r) as usize >= n && ((c * r) as usize) < n + c as usize, "n={n}");
        }
    }
}
