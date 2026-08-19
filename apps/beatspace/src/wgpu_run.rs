//! wgpu_run — the device, the buffers, and the dispatch.
//!
//! `wgpu_train` holds the kernels and the index arithmetic, proven on the CPU.
//! This is what puts them on hardware. Split that way because the maths is
//! checkable anywhere and this file is not: it needs a GPU to say anything at
//! all, so keeping the two apart is what let the arithmetic be tested at all.
//!
//! ## The limits that have already cost this project time
//!
//! Both are recorded in `gpu-gemm` after being hit on real hardware, and both
//! are enforced here rather than discovered again:
//!
//! * **Every dispatch dimension must be ≤ 65535.** A 220,960-workgroup grid
//!   was silently accepted by the API and produced wrong results; the fix was
//!   to spill into `y`. `Grid::for_items` does that spilling by construction,
//!   so no caller can build the bad grid.
//! * **The single-allocation ceiling is not the concurrent one.** gpu-gemm's
//!   loader clamps a 48 GiB budget to 31 GiB because this driver "accepts a
//!   large single allocation but will not hold more than that in buffers at
//!   once" — and a plan above it fails PART WAY THROUGH a load rather than at
//!   planning time, which is the expensive way to find out. Allocation here
//!   goes through one accounting point that refuses up front.
//!
//! ## Not run
//!
//! There is no GPU in the container this was written in. The code compiles and
//! its bookkeeping — grid shapes, allocation accounting, bind-group layout
//! arity — is unit-tested; whether it produces correct numbers on an M2 is
//! unknown until someone runs `beatspace gpu-selftest` on one. That command
//! exists for exactly that, and it compares against the CPU reference rather
//! than against a number somebody wrote down.

#![cfg(feature = "gpu")]

use crate::wgpu_train::{ConvShape, WORKGROUP};

/// The per-dimension dispatch limit. Not a tunable.
pub const MAX_DIM: u32 = 65535;

/// What this driver will hold in buffers at once, in bytes.
///
/// gpu-gemm's figure, and its warning travels with it: this is a COMPILED-IN
/// CONSTANT measured once on an M2 Ultra, not a property of the device in
/// front of you. On different hardware it is wrong in either direction. It is
/// used as a budget to refuse plans early, never as a claim about the machine.
pub const CONCURRENT_BUFFER_CEILING: u64 = 31 * 1024 * 1024 * 1024;

/// A dispatch grid that cannot exceed the per-dimension limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl Grid {
    /// One workgroup per `WORKGROUP` items, spilling into `y` past the limit.
    ///
    /// The spill is not an optimisation. A flat `items / WORKGROUP` grid for a
    /// large tensor exceeds 65535 and the resulting kernel writes garbage into
    /// part of its output — which looks like a training bug, not a dispatch
    /// bug, and is why this returns a type rather than a number.
    ///
    /// A kernel launched this way MUST derive its index from
    /// `num_workgroups`, because the y dimension is only meaningful if it is
    /// read. `index_expr()` is the snippet that does it.
    pub fn for_items(items: usize) -> Grid {
        let groups = items.div_ceil(WORKGROUP) as u64;
        if groups <= MAX_DIM as u64 {
            return Grid { x: groups as u32, y: 1, z: 1 };
        }
        let x = MAX_DIM;
        let y = groups.div_ceil(x as u64);
        assert!(
            y <= MAX_DIM as u64,
            "dispatch of {items} items needs a {x}x{y} grid, past the 65535 limit in both \
             dimensions — split the work"
        );
        Grid { x, y: y as u32, z: 1 }
    }

    /// A 2-D tile grid, for the GEMM.
    pub fn for_tiles(m: usize, n: usize, tile: usize) -> Grid {
        let x = n.div_ceil(tile) as u64;
        let y = m.div_ceil(tile) as u64;
        assert!(
            x <= MAX_DIM as u64 && y <= MAX_DIM as u64,
            "GEMM {m}x{n} needs a {x}x{y} tile grid, past the 65535 limit"
        );
        Grid { x: x as u32, y: y as u32, z: 1 }
    }

    pub fn total(&self) -> u64 {
        self.x as u64 * self.y as u64 * self.z as u64
    }
}

/// The WGSL a spilled kernel must use to find its item.
///
/// Provided as a constant so the shader and `Grid::for_items` cannot disagree:
/// a kernel that computes `gid.x` alone silently processes the first 65535
/// groups and ignores the rest.
pub const SPILL_INDEX_WGSL: &str = "\
let gidx = gid.x + gid.y * num_wg.x * WGSIZE;";

/// Running total of what has been allocated, so a plan can be refused early.
#[derive(Debug, Default, Clone)]
pub struct BufferBudget {
    allocated: u64,
    ceiling: u64,
}

impl BufferBudget {
    pub fn new(ceiling: u64) -> Self {
        BufferBudget { allocated: 0, ceiling }
    }

    /// Reserve `bytes`, or explain why not.
    ///
    /// Refusing here is the whole point: the failure this prevents is a load
    /// that dies part way through, having already spent minutes decoding, with
    /// an out-of-memory from the driver that names no budget.
    pub fn reserve(&mut self, bytes: u64, what: &str) -> Result<(), String> {
        if self.allocated + bytes > self.ceiling {
            return Err(format!(
                "{what} needs {:.1} MiB and only {:.1} MiB of the {:.1} GiB buffer budget is \
                 left. This ceiling is a compiled-in constant measured on an M2 Ultra, not a \
                 reading from this device — if the hardware is larger, raise it; if a load \
                 still dies part way through, it is lower.",
                bytes as f64 / 1e6,
                (self.ceiling - self.allocated) as f64 / 1e6,
                self.ceiling as f64 / 1e9,
            ));
        }
        self.allocated += bytes;
        Ok(())
    }

    pub fn used(&self) -> u64 {
        self.allocated
    }

    /// Give bytes back. Training frees activations every step, and a budget
    /// that only counted up would refuse the second epoch.
    pub fn release(&mut self, bytes: u64) {
        self.allocated = self.allocated.saturating_sub(bytes);
    }
}

/// Bytes needed to lower one convolution: the column matrix dominates.
///
/// Worth computing before allocating, because im2col expands the input by
/// `kernel²` — a 3×3 lowering is nine times the activation, and that
/// multiplication is where a plan that looked fine stops fitting.
pub fn im2col_bytes(s: &ConvShape) -> u64 {
    let (_, k, n) = s.gemm_dims();
    (k as u64) * (n as u64) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_dispatch_is_one_row() {
        let g = Grid::for_items(1000);
        assert_eq!(g, Grid { x: 4, y: 1, z: 1 }, "1000 items at 256 per group");
    }

    #[test]
    fn a_large_dispatch_spills_into_y_rather_than_overflowing() {
        // The bug this project already hit: a 220,960-group grid was accepted
        // by the API and produced wrong results in part of the output, which
        // reads as a training bug rather than a dispatch bug.
        let items = 220_960 * WORKGROUP;
        let g = Grid::for_items(items);
        assert!(g.x <= MAX_DIM && g.y <= MAX_DIM, "{g:?} still exceeds the limit");
        assert!(g.y > 1, "the grid did not spill");
        // And it still covers every item.
        assert!(g.total() * WORKGROUP as u64 >= items as u64, "{g:?} covers too little");
    }

    #[test]
    fn a_grid_that_cannot_fit_even_spilled_says_so() {
        // 65535² groups is the ceiling of this scheme. Past it the answer is
        // to split the work, and a panic naming that is better than a silently
        // truncated dispatch.
        let too_many = (MAX_DIM as usize + 1) * MAX_DIM as usize * WORKGROUP;
        assert!(std::panic::catch_unwind(|| Grid::for_items(too_many)).is_err());
    }

    #[test]
    fn the_gemm_grid_is_tiles_not_threads() {
        // A GEMM grid counts TILES. Computing it in threads is a 256×
        // overshoot that blows the limit on a matrix that would have fitted.
        let g = Grid::for_tiles(1024, 2048, 16);
        assert_eq!(g, Grid { x: 128, y: 64, z: 1 });
    }

    #[test]
    fn the_budget_refuses_before_the_allocation_rather_than_during() {
        // The failure being prevented: a load that dies part way through,
        // after minutes of decoding, with a driver OOM that names no budget.
        let mut b = BufferBudget::new(1000);
        assert!(b.reserve(600, "weights").is_ok());
        let err = b.reserve(600, "activations").unwrap_err();
        assert!(err.contains("activations"), "{err}");
        assert!(err.contains("compiled-in constant"),
                "the message must not claim to have measured this device: {err}");
        assert_eq!(b.used(), 600, "a refused reservation must not be counted");
    }

    #[test]
    fn freeing_lets_the_next_step_run() {
        // Training frees activations every step. A budget that only counted up
        // would refuse the second epoch.
        let mut b = BufferBudget::new(1000);
        b.reserve(900, "step 1").unwrap();
        b.release(900);
        assert!(b.reserve(900, "step 2").is_ok());
    }

    #[test]
    fn im2col_expansion_is_visible_before_it_is_allocated() {
        // A 4x4 kernel expands the activation sixteenfold, and that is where a
        // plan that looked fine stops fitting.
        let s = ConvShape {
            batch: 32, in_ch: 128, out_ch: 64,
            in_h: 16, in_w: 16, kernel: 4, stride: 2, pad: 1,
        };
        let raw = (s.batch * s.in_ch * s.in_h * s.in_w * 4) as u64;
        let col = im2col_bytes(&s);
        assert!(col > raw * 3, "the expansion is not being reported: {col} vs {raw}");
    }
}
