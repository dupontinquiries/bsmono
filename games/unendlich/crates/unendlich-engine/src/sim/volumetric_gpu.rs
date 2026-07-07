//! Kernel: **froxel scattering integration** — the front-to-back Z-integration
//! pass your `render::froxel` docs list as the missing "complete system" step 3,
//! and the thing that turns shadow-sampled froxels into visible **light shafts /
//! godrays** and correct volumetric fog.
//!
//! `froxel_fill.comp` already writes per-froxel single-scatter (density × phase ×
//! sun × shadow) + a per-slice transmittance. This pass composes a froxel column
//! front-to-back along the camera ray: each froxel accumulates the in-scatter of
//! the slices in front of it, attenuated by the transmittance accumulated so far,
//! and stores `(accumScatter_rgb, transmittance)`. Because the per-froxel scatter
//! already encodes sun shadowing, integrating along Z makes the lit slices
//! between shadow gaps add up into shafts — that's the godray.
//!
//! GPU-ready form: one thread per `(x_tile, y_tile)` column; the Z loop is serial
//! *within* a thread (a prefix scan along Z), columns are fully parallel. The CPU
//! reference here is the bit-exact correctness gate and the benchmarked body. Each
//! slice uses the analytic constant-medium integral `s·(1−e^{−σ·step})/σ` (→ `s·step`
//! as σ→0), so transmittance obeys Beer–Lambert exactly.
//!
//! ## Analytics / frame pacing
//! Time this pass with the existing [`render::gpu_timer`] machinery as a new zone
//! (suggest label `"scatter"`, budget alongside `"froxel"`), or report a CPU
//! reference cost via [`BenchResult::metric_line`] with `name = "froxel_scatter"`.
//! It then lands in the dashboard's GPU-timers panel under the 8.3 ms stage budget
//! (feeding the 🚨 stage-p99-over-budget achievement) so its frame-pacing cost is
//! tracked like `shadow`/`froxel`/`cull`/`main`.
//!
//! ## GPU DISPATCH CONTRACT
//!   bindings:
//!     0: sigma   SSBO/image — [f32 extinction; nx*ny*nz]   (from froxel_fill)
//!     1: source  SSBO/image — [vec3 in-scatter; nx*ny*nz]  (from froxel_fill)
//!     2: out     image3D    — [rgba16f (accumScatter.rgb, transmittance); n]
//!   uniforms: nx, ny, nz; step buffer [f32; nz] (log-Z slice thickness)
//!   dispatch: one invocation per (x,y); the thread marches z = 0..nz front-to-
//!     back, keeping `(accum, T)` in registers and writing each froxel. f16 store
//!     ⇒ output matches [`froxel_integrate_cpu`] within ~1e-2 (note the tolerance).

use crate::sim::gpu_bench::{bench, BenchResult};

/// Analytic in-scatter integral across one slice of constant extinction `sigma`
/// and source `s` over thickness `step`: `s·(1−e^{−σ·step})/σ`, with the σ→0
/// limit `s·step`. Never divides by zero.
#[inline]
pub fn slice_scatter(s: f32, sigma: f32, step: f32) -> f32 {
    const EPS: f32 = 1e-6;
    if sigma > EPS {
        s * (1.0 - (-sigma * step).exp()) / sigma
    } else {
        s * step
    }
}

/// Inputs for the integration pass — the froxel_fill output plus the log-Z slice
/// spacing. Row-major, x fastest: `idx(x,y,z) = (z*ny + y)*nx + x`.
#[derive(Clone, Debug)]
pub struct ScatterVolume {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Per-froxel extinction coefficient σ. Length `nx*ny*nz`.
    pub sigma: Vec<f32>,
    /// Per-froxel single-scatter source (rgb). Length `nx*ny*nz`.
    pub source: Vec<[f32; 3]>,
    /// Per-Z-slice thickness (world units along the ray). Length `nz`.
    pub step: Vec<f32>,
}

impl ScatterVolume {
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.ny + y) * self.nx + x
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Integrate every froxel column front-to-back. Output length is `vol.len()`;
/// each froxel stores `[accumScatter_r, accumScatter_g, accumScatter_b,
/// transmittance]`. `transmittance` decreases monotonically along +Z and obeys
/// Beer–Lambert; `accumScatter` increases monotonically.
pub fn froxel_integrate_cpu(vol: &ScatterVolume) -> Vec<[f32; 4]> {
    let (nx, ny, nz) = (vol.nx, vol.ny, vol.nz);
    let mut out = vec![[0.0f32, 0.0, 0.0, 1.0]; vol.len()];
    for y in 0..ny {
        for x in 0..nx {
            let mut accum = [0.0f32; 3];
            let mut transmittance = 1.0f32;
            for z in 0..nz {
                let c = vol.idx(x, y, z);
                let sigma = vol.sigma[c];
                let step = vol.step[z];
                let src = vol.source[c];
                for ch in 0..3 {
                    accum[ch] += transmittance * slice_scatter(src[ch], sigma, step);
                }
                transmittance *= (-sigma * step).exp();
                out[c] = [accum[0], accum[1], accum[2], transmittance];
            }
        }
    }
    out
}

/// Benchmark the integration over `iters` runs. Feed to
/// [`BenchResult::metric_line`] with `name = "froxel_scatter"`.
pub fn bench_integrate(vol: &ScatterVolume, iters: usize) -> BenchResult {
    bench(iters, || {
        let v = froxel_integrate_cpu(vol);
        std::hint::black_box(&v);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1×nz column with constant σ, constant rgb source, constant step.
    fn column(nz: usize, sigma: f32, src: f32, step: f32) -> ScatterVolume {
        ScatterVolume {
            nx: 1,
            ny: 1,
            nz,
            sigma: vec![sigma; nz],
            source: vec![[src, src * 0.5, 0.0]; nz],
            step: vec![step; nz],
        }
    }

    #[test]
    fn transmittance_obeys_beer_lambert() {
        let (sigma, step, nz) = (0.7f32, 0.3f32, 12usize);
        let out = froxel_integrate_cpu(&column(nz, sigma, 1.0, step));
        let back_t = out[nz - 1][3];
        let expect = (-sigma * step * nz as f32).exp();
        assert!((back_t - expect).abs() < 1e-4, "T={back_t} expect={expect}");
    }

    #[test]
    fn scatter_matches_closed_form() {
        let (sigma, step, nz, s) = (0.5f32, 0.4f32, 10usize, 2.0f32);
        let out = froxel_integrate_cpu(&column(nz, sigma, s, step));
        let l = step * nz as f32;
        let analytic = s * (1.0 - (-sigma * l).exp()) / sigma; // red channel
        assert!((out[nz - 1][0] - analytic).abs() < 1e-3, "got {} want {analytic}", out[nz - 1][0]);
        // green channel is half the source → half the accumulated scatter
        assert!((out[nz - 1][1] - analytic * 0.5).abs() < 1e-3);
        assert_eq!(out[nz - 1][2], 0.0); // blue source was 0
    }

    #[test]
    fn zero_extinction_is_linear() {
        let nz = 5;
        let out = froxel_integrate_cpu(&column(nz, 0.0, 1.5, 0.4));
        assert!((out[nz - 1][3] - 1.0).abs() < 1e-7, "no extinction → full transmittance");
        assert!((out[nz - 1][0] - 1.5 * 0.4 * nz as f32).abs() < 1e-5);
    }

    #[test]
    fn monotonic_along_z() {
        let out = froxel_integrate_cpu(&column(20, 0.6, 1.0, 0.25));
        for z in 1..20 {
            assert!(out[z][3] <= out[z - 1][3] + 1e-7, "transmittance must not increase");
            assert!(out[z][0] >= out[z - 1][0] - 1e-7, "accum must not decrease");
        }
    }

    #[test]
    fn slice_scatter_sigma_zero_limit() {
        assert!((slice_scatter(2.0, 0.0, 0.5) - 1.0).abs() < 1e-9);
        // continuity near zero
        let a = slice_scatter(2.0, 1e-7, 0.5);
        let b = slice_scatter(2.0, 0.0, 0.5);
        assert!((a - b).abs() < 1e-3);
    }

    #[test]
    fn columns_independent_and_bench() {
        let mut vol = column(8, 0.4, 1.0, 0.3);
        vol.nx = 2;
        vol.ny = 2;
        let n = vol.nx * vol.ny * vol.nz;
        vol.sigma = vec![0.4; n];
        vol.source = vec![[1.0, 0.5, 0.0]; n];
        let out = froxel_integrate_cpu(&vol);
        assert_eq!(out.len(), n);
        let br = bench_integrate(&vol, 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("froxel_scatter", Some(8.3)).contains("froxel_scatter n=3"));
    }
}
