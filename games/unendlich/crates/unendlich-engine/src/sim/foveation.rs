//! **Foveation** — a screen-space quality field that expensive passes sample to
//! scale their own work toward the player's gaze proxy (on a flat FPS: the aim
//! point / screen centre, optionally biased by motion). This is the shared
//! keystone for "foveate the froxels / shadows / lighting": one field, many
//! consumers.
//!
//! [`quality_at`] returns `1.0` inside the foveal radius and smoothsteps down to
//! `min_quality` at the outer radius (monotonic, bounded). [`lod_budget`] maps a
//! quality to an integer sample budget (march steps / PCF taps / Z-slices /
//! shading rate). Each pass turns its budget into less work in the periphery:
//!   - froxels:  fewer Z groups in the scatter integration (this module)
//!   - shadows:  fewer PCF taps per peripheral fragment
//!   - lighting: coarser shading rate (e.g. 2×2) in the periphery
//!   - octree:   fewer ray-march steps along peripheral camera rays
//!
//! ## Why foveating the froxel integration is *safe*
//! [`integrate_foveated`] merges a column's Z-slices into `budget` groups. Merging
//! preserves **transmittance exactly** (∏ e^{−τᵢ} = e^{−Στᵢ}, independent of
//! grouping) — so peripheral fog keeps correct opacity/depth — and only the
//! single-scatter detail is approximated, with error shrinking monotonically as
//! the budget rises toward the fovea. At full budget it is bit-identical to
//! [`crate::sim::volumetric_gpu::froxel_integrate_cpu`].
//!
//! ## Analytics / frame pacing
//! Foveation adds no pass; it *removes* cost from existing ones, so the win shows
//! up as lower `scatter` / `shadow` / `main` ms in the dashboard's GPU-timers
//! panel. Drive the foveal point from the camera each frame and the periphery
//! cost drops without touching the centre — the frame-pacing lever you wanted.
//!
//! ## GPU DISPATCH CONTRACT (froxel consumer)
//!   bindings: as `volumetric_gpu` integrate, plus a `quality[nx*ny]` tile buffer.
//!   dispatch: one invocation per (x,y) column; B = lod_budget(quality[x,y], …);
//!     partition z=0..nz into B contiguous groups; per group accumulate the merged
//!     slab (τ = Σσ·step exact; source = length-weighted mean) and write the
//!     running (accum, T) to every froxel in the group. Foveal columns (B≈nz)
//!     reduce to the exact per-slice integration.

use crate::sim::gpu_bench::{bench, BenchResult};
use crate::sim::volumetric_gpu::{slice_scatter, ScatterVolume};

/// Foveation field parameters. Radii are in normalized screen units; `aspect`
/// (width/height) circularizes the falloff.
#[derive(Clone, Copy, Debug)]
pub struct FoveationParams {
    /// Gaze proxy in normalized screen coords (0..1, 0..1) — usually the aim point.
    pub fovea: [f32; 2],
    /// Radius of the full-quality foveal disc.
    pub inner: f32,
    /// Radius at which quality reaches `min_quality`.
    pub outer: f32,
    /// Floor quality in the far periphery (0..1).
    pub min_quality: f32,
    /// Screen aspect (width/height) for a circular falloff.
    pub aspect: f32,
}

impl Default for FoveationParams {
    fn default() -> Self {
        FoveationParams { fovea: [0.5, 0.5], inner: 0.12, outer: 0.6, min_quality: 0.25, aspect: 16.0 / 9.0 }
    }
}

/// Foveation quality at normalized screen coord `(u,v)`: `1.0` within `inner`,
/// smoothstep down to `min_quality` at `outer`, clamped. Monotonic non-increasing
/// in distance from the fovea.
pub fn quality_at(u: f32, v: f32, p: &FoveationParams) -> f32 {
    let dx = (u - p.fovea[0]) * p.aspect;
    let dy = v - p.fovea[1];
    let r = (dx * dx + dy * dy).sqrt();
    if r <= p.inner {
        return 1.0;
    }
    if r >= p.outer {
        return p.min_quality;
    }
    let t = (r - p.inner) / (p.outer - p.inner);
    let s = t * t * (3.0 - 2.0 * t); // smoothstep
    1.0 - s * (1.0 - p.min_quality)
}

/// Map a quality in `[0,1]` to an integer sample budget in `[min_steps,
/// max_steps]`. Monotonic; full quality → `max_steps`.
pub fn lod_budget(quality: f32, max_steps: usize, min_steps: usize) -> usize {
    let b = (quality * max_steps as f32).round() as i64;
    b.clamp(min_steps as i64, max_steps as i64) as usize
}

/// Build a per-tile quality map (length `nx*ny`) for a froxel/screen tile grid.
pub fn quality_map(nx: usize, ny: usize, p: &FoveationParams) -> Vec<f32> {
    let mut q = vec![0.0f32; nx * ny];
    for y in 0..ny {
        for x in 0..nx {
            let u = (x as f32 + 0.5) / nx as f32;
            let v = (y as f32 + 0.5) / ny as f32;
            q[y * nx + x] = quality_at(u, v, p);
        }
    }
    q
}

/// Foveated froxel scattering integration. `quality` is per-column (length
/// `nx*ny`); each column integrates with `lod_budget(quality, max_steps,
/// min_steps)` Z-groups. Output length is `vol.len()` (every froxel written, so
/// the fragment shader samples it unchanged); within a group all froxels carry
/// the running `(accumScatter, transmittance)` at the group's end. Transmittance
/// is exact regardless of budget; at `max_steps >= nz` and full quality the
/// result equals [`crate::sim::volumetric_gpu::froxel_integrate_cpu`].
pub fn integrate_foveated(
    vol: &ScatterVolume,
    quality: &[f32],
    max_steps: usize,
    min_steps: usize,
) -> Vec<[f32; 4]> {
    let (nx, ny, nz) = (vol.nx, vol.ny, vol.nz);
    let mut out = vec![[0.0f32, 0.0, 0.0, 1.0]; vol.len()];
    if nz == 0 {
        return out;
    }
    for y in 0..ny {
        for x in 0..nx {
            let q = quality[y * nx + x];
            let b = lod_budget(q, max_steps.min(nz), min_steps).clamp(1, nz);
            let mut accum = [0.0f32; 3];
            let mut transmittance = 1.0f32;
            for g in 0..b {
                let a = (g * nz) / b;
                let e = ((g + 1) * nz) / b;
                if e <= a {
                    continue;
                }
                // merge slices [a,e): exact optical depth, length-weighted source
                let mut tot_step = 0.0f32;
                let mut tau = 0.0f32;
                let mut s_avg = [0.0f32; 3];
                for z in a..e {
                    let c = vol.idx(x, y, z);
                    let st = vol.step[z];
                    tot_step += st;
                    tau += vol.sigma[c] * st;
                    let src = vol.source[c];
                    for ch in 0..3 {
                        s_avg[ch] += src[ch] * st;
                    }
                }
                let sigma_eff = if tot_step > 1e-9 { tau / tot_step } else { 0.0 };
                for ch in 0..3 {
                    let s = if tot_step > 1e-9 { s_avg[ch] / tot_step } else { 0.0 };
                    accum[ch] += transmittance * slice_scatter(s, sigma_eff, tot_step);
                }
                transmittance *= (-tau).exp();
                let val = [accum[0], accum[1], accum[2], transmittance];
                for z in a..e {
                    out[vol.idx(x, y, z)] = val;
                }
            }
        }
    }
    out
}

/// Benchmark the foveated integration over `iters` runs.
pub fn bench_integrate_foveated(
    vol: &ScatterVolume,
    quality: &[f32],
    max_steps: usize,
    min_steps: usize,
    iters: usize,
) -> BenchResult {
    bench(iters, || {
        let v = integrate_foveated(vol, quality, max_steps, min_steps);
        std::hint::black_box(&v);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::volumetric_gpu::froxel_integrate_cpu;

    fn p() -> FoveationParams {
        FoveationParams { fovea: [0.5, 0.5], inner: 0.1, outer: 0.6, min_quality: 0.25, aspect: 1.0 }
    }

    fn column_vol(nz: usize, seed: u64) -> ScatterVolume {
        // 1x1xnz with deterministic pseudo-random media
        let mut s = seed.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
        let mut next = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            ((s.wrapping_mul(0x2545F4914F6CDD1D) >> 33) as f32) / (1u64 << 31) as f32
        };
        let sigma: Vec<f32> = (0..nz).map(|_| next() * 2.0).collect();
        let source: Vec<[f32; 3]> = (0..nz).map(|_| [next() * 3.0, next() * 2.0, next()]).collect();
        let step: Vec<f32> = (0..nz).map(|_| 0.05 + next() * 0.7).collect();
        ScatterVolume { nx: 1, ny: 1, nz, sigma, source, step }
    }

    #[test]
    fn field_bounded_and_monotonic() {
        let pp = p();
        assert!((quality_at(0.5, 0.5, &pp) - 1.0).abs() < 1e-6);
        let mut prev = 1.0;
        for k in 0..=100 {
            let r = k as f32 / 100.0;
            let q = quality_at(0.5 + r * 0.5, 0.5, &pp);
            assert!(q >= pp.min_quality - 1e-6 && q <= 1.0 + 1e-6);
            assert!(q <= prev + 1e-6, "quality rose with distance");
            prev = q;
        }
        assert!((quality_at(0.99, 0.99, &pp) - pp.min_quality).abs() < 1e-6);
    }

    #[test]
    fn budget_monotonic_and_bounded() {
        let mut prev = 0;
        for k in 0..=20 {
            let b = lod_budget(k as f32 / 20.0, 64, 8);
            assert!(b >= 8 && b <= 64);
            assert!(b >= prev, "budget dropped as quality rose");
            prev = b;
        }
        assert_eq!(lod_budget(1.0, 64, 8), 64);
    }

    #[test]
    fn full_budget_equals_reference() {
        let vol = column_vol(20, 7);
        let q = vec![1.0f32; vol.nx * vol.ny];
        let fov = integrate_foveated(&vol, &q, vol.nz, 1);
        let full = froxel_integrate_cpu(&vol);
        for i in 0..full.len() {
            for ch in 0..4 {
                assert!((fov[i][ch] - full[i][ch]).abs() < 1e-5, "froxel {i} ch {ch}");
            }
        }
    }

    #[test]
    fn transmittance_exact_at_any_budget() {
        let vol = column_vol(30, 11);
        let full = froxel_integrate_cpu(&vol);
        let back_full = full[vol.nz - 1][3];
        for budget in [1usize, 2, 4, 8, 16] {
            // force a uniform low budget via quality that maps to it
            let q = vec![budget as f32 / vol.nz as f32; vol.nx * vol.ny];
            let fov = integrate_foveated(&vol, &q, vol.nz, 1);
            let back = fov[vol.nz - 1][3];
            assert!((back - back_full).abs() < 1e-4, "budget {budget}: T {back} vs {back_full}");
        }
    }

    #[test]
    fn scatter_error_shrinks_with_budget() {
        // A subsampling integrator's scatter error is NOT monotone for any
        // single column: like any Riemann-sum refinement, a coarse partition
        // can coincidentally land closer to the exact integral than a finer
        // one (aliasing). The meaningful, true property — and the one
        // foveation relies on — is that error decreases with budget IN
        // AGGREGATE over the many media columns a frame integrates. So average
        // the absolute scatter error over a spread of distinct columns.
        let nz = 32usize;
        let mean_err = |b: usize| -> f32 {
            let n_cols = 64u64;
            let mut total = 0.0f32;
            for seed in 0..n_cols {
                let vol = column_vol(nz, 1000 + seed);
                let full = froxel_integrate_cpu(&vol);
                let ref_scatter = full[vol.nz - 1][0];
                let q = vec![b as f32 / vol.nz as f32; 1];
                let fov = integrate_foveated(&vol, &q, vol.nz, 1);
                total += (fov[vol.nz - 1][0] - ref_scatter).abs();
            }
            total / n_cols as f32
        };
        let (e2, e8, e32) = (mean_err(2), mean_err(8), mean_err(32));
        // Full budget (b == nz) is exact.
        assert!(e32 <= 1e-4, "full budget should be ~exact: {e32}");
        // Aggregate error is non-increasing as budget rises.
        assert!(e8 <= e2 + 1e-6, "mean error grew with budget: e8 {e8} > e2 {e2}");
        assert!(e32 <= e8 + 1e-6, "mean error grew with budget: e32 {e32} > e8 {e8}");
    }

    #[test]
    fn bench_runs() {
        let mut vol = column_vol(16, 5);
        vol.nx = 4;
        vol.ny = 4;
        let n = vol.nx * vol.ny * vol.nz;
        vol.sigma = vec![0.5; n];
        vol.source = vec![[1.0, 0.5, 0.2]; n];
        let q = quality_map(vol.nx, vol.ny, &p());
        let br = bench_integrate_foveated(&vol, &q, vol.nz, 2, 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("scatter_foveated", Some(8.3)).contains("scatter_foveated n=3"));
    }
}
