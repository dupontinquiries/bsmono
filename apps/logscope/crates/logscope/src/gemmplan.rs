//! gemmplan.rs — pure dispatch planning for the GPU GEMM/attention kernels: output-tile grids, the
//! >65535-workgroups-per-dimension fold, and split-K for skinny matrices.
//!
//! The kernels themselves are the GPU seam, but *how* the host launches them is pure arithmetic that
//! directly affects throughput and correctness: too-small tiles waste memory bandwidth, and a workgroup
//! count above the per-dimension cap (65535 on WebGPU/Vulkan) silently drops work unless it's folded into a
//! 2-D/3-D grid (RULESET K3). Getting this right is a real llama-inference speedup lever; it's also exactly
//! the kind of off-by-one that's miserable to debug on-device, so it's unit-tested here.
#![allow(dead_code)]

pub fn ceil_div(a: u32, b: u32) -> u32 { let b = b.max(1) as u64; ((a as u64 + b - 1) / b) as u32 }

/// Per-dimension dispatch limits. `max_per_dim` is the hardware cap on workgroups in one dimension.
#[derive(Clone, Copy, Debug)]
pub struct Limits { pub max_per_dim: u32, pub max_invocations: u32 }
impl Default for Limits {
    fn default() -> Self { Self { max_per_dim: 65535, max_invocations: 256 } } // WebGPU baseline
}

/// A 3-D workgroup grid to dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid3 { pub x: u32, pub y: u32, pub z: u32 }
impl Grid3 {
    pub fn total(&self) -> u64 { self.x as u64 * self.y as u64 * self.z as u64 }
    /// Reconstruct the flat workgroup id the kernel computes from its 3-D builtins (row-major in x).
    pub fn linear(&self, x: u32, y: u32, z: u32) -> u64 {
        (z as u64 * self.y as u64 + y as u64) * self.x as u64 + x as u64
    }
}

/// Fold a flat workgroup `count` into a ≤3-D grid with every dimension ≤ `max_per_dim`, so dispatches above
/// the per-dim cap still launch (K3). The grid may cover slightly more than `count` (the kernel must guard
/// `linear_id >= count`); padding is kept minimal. `None` if it can't fit even in 3-D.
pub fn fold_grid(count: u32, max_per_dim: u32) -> Option<Grid3> {
    let max = max_per_dim.max(1);
    if count <= max { return Some(Grid3 { x: count.max(1), y: 1, z: 1 }); }
    let x = max;
    let y = ceil_div(count, x);
    if y <= max { return Some(Grid3 { x, y, z: 1 }); }
    let y = max;
    let xy = x as u64 * y as u64;
    let z = ((count as u64 + xy - 1) / xy) as u32;
    if z <= max { return Some(Grid3 { x, y, z }); }
    None
}

/// Output-tile workgroup count for an M×N GEMM at `tile_m`×`tile_n` outputs per group.
pub fn gemm_tile_groups(m: u32, n: u32, tile_m: u32, tile_n: u32) -> u32 {
    ceil_div(m, tile_m) * ceil_div(n, tile_n)
}

/// Pick a split-K factor: when the output is small (few tiles) the GPU is underused, so split the K
/// (contraction) dimension into independent partials that reduce afterward — more parallel groups. Returns
/// 1 when there's already enough work or K is too small to split usefully.
pub fn split_k(tile_groups: u32, k: u32, target_groups: u32, min_k_per_split: u32) -> u32 {
    if tile_groups == 0 || tile_groups >= target_groups { return 1; }
    let want = ceil_div(target_groups, tile_groups);          // factor that would reach the target
    let by_k = (k / min_k_per_split.max(1)).max(1);           // don't make slivers smaller than this
    want.min(by_k).max(1)
}

/// A full dispatch plan for one GEMM.
#[derive(Clone, Copy, Debug)]
pub struct GemmPlan { pub tiles_m: u32, pub tiles_n: u32, pub split_k: u32, pub grid: Grid3 }

/// Plan an M×N×K GEMM: tile the output, optionally split K for occupancy, and fold the total group count
/// (`tiles_m * tiles_n * split_k`) into a launchable grid.
pub fn plan_gemm(m: u32, n: u32, k: u32, tile_m: u32, tile_n: u32, target_groups: u32, limits: &Limits) -> Option<GemmPlan> {
    let tiles_m = ceil_div(m, tile_m);
    let tiles_n = ceil_div(n, tile_n);
    let base = tiles_m * tiles_n;
    let sk = split_k(base, k, target_groups, tile_m.max(tile_n).max(16));
    let grid = fold_grid(base * sk, limits.max_per_dim)?;
    Some(GemmPlan { tiles_m, tiles_n, split_k: sk, grid })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_div_rounds_up() {
        assert_eq!(ceil_div(0, 4), 0);
        assert_eq!(ceil_div(1, 4), 1);
        assert_eq!(ceil_div(8, 4), 2);
        assert_eq!(ceil_div(9, 4), 3);
    }

    #[test]
    fn small_counts_stay_one_dimensional() {
        assert_eq!(fold_grid(1000, 65535), Some(Grid3 { x: 1000, y: 1, z: 1 }));
    }

    #[test]
    fn large_counts_fold_and_cover_every_id() {
        let count = 70_000u32; // > 65535 → must go 2-D
        let g = fold_grid(count, 65535).unwrap();
        assert!(g.x <= 65535 && g.y <= 65535 && g.z <= 65535);
        assert!(g.total() >= count as u64, "grid must cover all work");
        // the padding is small (less than one full extra row)
        assert!(g.total() < count as u64 + g.x as u64);
    }

    #[test]
    fn fold_is_a_bijection_up_to_count() {
        // every workgroup id 0..count maps to a distinct in-range (x,y,z), and the kernel's linear()
        // reproduces the id — i.e. no work is dropped or double-counted
        let count = 200_000u32;
        let g = fold_grid(count, 65535).unwrap();
        let mut seen = std::collections::HashSet::new();
        for id in 0..count as u64 {
            let x = (id % g.x as u64) as u32;
            let y = ((id / g.x as u64) % g.y as u64) as u32;
            let z = (id / (g.x as u64 * g.y as u64)) as u32;
            assert!(x < g.x && y < g.y && z < g.z);
            assert_eq!(g.linear(x, y, z), id);
            assert!(seen.insert(id));
        }
        assert_eq!(seen.len(), count as usize);
    }

    #[test]
    fn huge_dispatch_uses_three_dimensions() {
        // ~5.4e9 groups can't fit in 2-D (65535² ≈ 4.29e9) → needs z
        let count = u32::MAX; // ceil(MAX/65535)=65536 > cap → must spill into z
        let g = fold_grid(count, 65535).unwrap();
        assert!(g.z > 1, "this dispatch can't fit in 2-D");
        assert!(g.total() >= count as u64);
        assert!(g.x <= 65535 && g.y <= 65535 && g.z <= 65535);
    }

    #[test]
    fn split_k_kicks_in_only_when_underutilized() {
        // a 1×1-tile GEMM with big K: few groups → split to reach the target
        assert!(split_k(1, 8192, 64, 128) > 1);
        // plenty of tiles already → no split
        assert_eq!(split_k(256, 8192, 64, 128), 1);
        // tiny K → don't split into slivers
        assert_eq!(split_k(1, 64, 64, 128), 1);
    }

    #[test]
    fn plan_gemm_tiles_and_folds() {
        // a 4096×4096×4096 matmul at 64×64 tiles → 64×64 = 4096 groups, fits 1-D, no split needed
        let p = plan_gemm(4096, 4096, 4096, 64, 64, 512, &Limits::default()).unwrap();
        assert_eq!((p.tiles_m, p.tiles_n), (64, 64));
        assert_eq!(p.split_k, 1);
        assert_eq!(p.grid, Grid3 { x: 4096, y: 1, z: 1 });
    }

    #[test]
    fn plan_gemm_splits_k_for_a_skinny_output() {
        // tiny output (one tile) but large K → split K to fill the GPU
        let p = plan_gemm(64, 64, 16384, 64, 64, 32, &Limits::default()).unwrap();
        assert_eq!((p.tiles_m, p.tiles_n), (1, 1));
        assert!(p.split_k > 1);
        assert_eq!(p.grid.total(), p.split_k as u64); // 1 tile × split_k groups
    }
}
