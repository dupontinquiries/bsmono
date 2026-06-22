//! Layered run-length trees with a jump-board index — the CPU-side layout behind
//! the "separate octrees per layer, overlaid, RLE'd, with diving boards" idea.
//! This is the part that decides the access-time tradeoff (the shaders just
//! consume it):
//!
//! - Each layer (surface / subsurface / foliage) is its own run-length stream.
//!   RLE saves memory but makes a point lookup O(runs) — you'd have to walk run
//!   offsets — which is the access-time cost.
//! - A **jump board** (a landing entry every `STRIDE` cells pointing at the run
//!   covering that position) bounds the walk to ≤ `STRIDE` runs, buying back
//!   fast random access. This is the "diving board" that lets a mesher hop in
//!   without swimming through every run.
//! - [`LayeredColumn`] overlays layers with precedence (foliage over surface over
//!   subsurface): a query returns the topmost non-empty layer. Cost scales with
//!   the number of layers, but empty layers miss cheaply.
//!
//! Pure, deterministic. Value 0 = empty/transparent in that layer (defers down).

/// Landing spacing for the jump board. Bounds a lookup's run-walk to ≤ STRIDE.
pub const STRIDE: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub value: u16,
    pub len: u32,
}

/// One run-length layer with a jump-board index for bounded random access.
#[derive(Debug, Clone)]
pub struct RleLayer {
    runs: Vec<Run>,
    starts: Vec<u32>, // starts[i] = first cell index of run i (prefix sum)
    jumps: Vec<u32>,  // jumps[b] = run index covering cell b*STRIDE
    len: u32,
}

impl RleLayer {
    /// Build runs + index from a dense cell slice.
    pub fn from_dense(cells: &[u16]) -> Self {
        let mut runs = Vec::new();
        let mut starts = Vec::new();
        let mut i = 0usize;
        while i < cells.len() {
            let v = cells[i];
            let mut j = i + 1;
            while j < cells.len() && cells[j] == v {
                j += 1;
            }
            starts.push(i as u32);
            runs.push(Run { value: v, len: (j - i) as u32 });
            i = j;
        }
        let len = cells.len() as u32;
        // Jump board: one landing every STRIDE cells.
        let boards = if len == 0 { 0 } else { (len - 1) / STRIDE + 1 };
        let mut jumps = Vec::with_capacity(boards as usize);
        let mut idx = 0usize;
        for b in 0..boards {
            let pos = b * STRIDE;
            while idx + 1 < runs.len() && starts[idx + 1] <= pos {
                idx += 1;
            }
            jumps.push(idx as u32);
        }
        RleLayer { runs, starts, jumps, len }
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Value at `pos`, using the jump board. Returns `(value, runs_walked)` so the
    /// access cost is observable; `runs_walked` is bounded by `STRIDE`.
    pub fn get_counted(&self, pos: u32) -> (u16, u32) {
        debug_assert!(pos < self.len);
        let mut idx = self.jumps[(pos / STRIDE) as usize] as usize;
        let mut walked = 0;
        while idx + 1 < self.runs.len() && self.starts[idx + 1] <= pos {
            idx += 1;
            walked += 1;
        }
        (self.runs[idx].value, walked)
    }

    pub fn get(&self, pos: u32) -> u16 {
        self.get_counted(pos).0
    }

    /// Approximate resident bytes: runs + index.
    pub fn bytes(&self) -> usize {
        self.runs.len() * std::mem::size_of::<Run>()
            + self.starts.len() * 4
            + self.jumps.len() * 4
    }

    pub fn run_count(&self) -> usize {
        self.runs.len()
    }
}

/// Layers stacked by precedence — index 0 = bottom (subsurface), last = top
/// (foliage). A composite query returns the topmost non-empty value.
#[derive(Debug, Clone, Default)]
pub struct LayeredColumn {
    pub layers: Vec<RleLayer>,
}

impl LayeredColumn {
    /// Topmost layer whose value at `pos` is non-zero; 0 if all empty there.
    /// Walks at most `layers.len()` layers (empty hits are cheap).
    pub fn composite_get(&self, pos: u32) -> u16 {
        for layer in self.layers.iter().rev() {
            let v = layer.get(pos);
            if v != 0 {
                return v;
            }
        }
        0
    }

    pub fn total_bytes(&self) -> usize {
        self.layers.iter().map(|l| l.bytes()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_get_matches_the_dense_source_everywhere() {
        let dense: Vec<u16> = vec![
            0, 0, 0, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, // run past a stride boundary
            5, 5, 5, 0, 0, 7, 7, 7, 1, 0, 0, 0, 0, 0, 0, 9,
        ];
        let layer = RleLayer::from_dense(&dense);
        for (p, &want) in dense.iter().enumerate() {
            assert_eq!(layer.get(p as u32), want, "mismatch at {p}");
        }
    }

    #[test]
    fn the_jump_board_bounds_the_walk() {
        // Worst case for RLE: every cell a different value → one run per cell.
        let dense: Vec<u16> = (0..200u16).map(|i| i + 1).collect();
        let layer = RleLayer::from_dense(&dense);
        assert_eq!(layer.run_count(), 200, "no coalescing possible");
        for p in 0..dense.len() as u32 {
            let (v, walked) = layer.get_counted(p);
            assert_eq!(v, dense[p as usize]);
            assert!(walked < STRIDE, "diving board bounds the walk: {walked} < {STRIDE} at {p}");
        }
    }

    #[test]
    fn rle_run_data_is_tiny_but_the_jump_board_is_not_free() {
        // A column that's almost all one value → a handful of runs vs 4096 cells.
        let mut dense = vec![3u16; 4096];
        dense[100] = 9;
        dense[2000] = 9;
        let layer = RleLayer::from_dense(&dense);
        let dense_bytes = dense.len() * 2;
        assert!(layer.run_count() <= 6);
        // The run+offset data is a tiny fraction of dense...
        let run_data = layer.run_count() * (std::mem::size_of::<Run>() + 4);
        assert!(run_data < dense_bytes / 10, "run data {run_data} « dense {dense_bytes}");
        // ...but the jump board adds fixed cost (len/STRIDE entries). Net still
        // beats dense, just not by the run-data ratio — the index isn't free,
        // which is exactly the access-time-vs-memory tradeoff.
        assert!(layer.bytes() < dense_bytes, "total {} < dense {}", layer.bytes(), dense_bytes);
    }

    #[test]
    fn layers_overlay_by_precedence() {
        // subsurface: solid stone (id 2) the whole column
        // surface:    grass (id 3) only at the top a few cells
        // foliage:    a tree trunk (id 7) at two spots, else empty (0)
        let n = 32;
        let sub = RleLayer::from_dense(&vec![2u16; n]);
        let mut surf = vec![0u16; n];
        surf[30] = 3;
        surf[31] = 3;
        let mut foli = vec![0u16; n];
        foli[10] = 7;
        foli[20] = 7;
        let col = LayeredColumn {
            layers: vec![sub, RleLayer::from_dense(&surf), RleLayer::from_dense(&foli)],
        };
        assert_eq!(col.composite_get(10), 7, "foliage wins where present");
        assert_eq!(col.composite_get(20), 7);
        assert_eq!(col.composite_get(31), 3, "surface shows where no foliage");
        assert_eq!(col.composite_get(5), 2, "subsurface shows where nothing above");
    }

    #[test]
    fn layered_storage_beats_one_dense_column_when_layers_are_sparse() {
        // Three sparse layers (mostly empty/uniform) vs a single dense array.
        let n = 4096;
        let sub = RleLayer::from_dense(&vec![2u16; n]); // 1 run
        let mut surf = vec![0u16; n];
        for k in (0..n).step_by(512) {
            surf[k] = 3;
        }
        let mut foli = vec![0u16; n];
        foli[1000] = 7;
        let col = LayeredColumn {
            layers: vec![sub, RleLayer::from_dense(&surf), RleLayer::from_dense(&foli)],
        };
        let dense_bytes = n * 2;
        assert!(col.total_bytes() < dense_bytes, "layered+RLE {} < dense {}", col.total_bytes(), dense_bytes);
    }
}
