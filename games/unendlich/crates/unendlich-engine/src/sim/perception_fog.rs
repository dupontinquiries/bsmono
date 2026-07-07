//! **Perception fog** — mob eyesight modelled as transmittance through a coarse
//! occlusion field, exposing two layers from one model:
//!
//!   - [`perceive`] → a graded detection signal in `[0,1]` (the *advanced* API):
//!     line-of-sight transmittance × distance falloff × FOV-centredness. Read this
//!     directly in cognition behaviours that want confidence / partial awareness.
//!   - [`can_see`] → a plain `bool` (the *simple* API, a drop-in for the old
//!     `vision::can_see`): the gradient thresholded against a **stable noise**
//!     pattern. Because the noise is uniform in `[0,1)`, `P(can_see) == perceive`
//!     — the boolean is a calibrated, deterministic coin-flip of the gradient. At
//!     the extremes it collapses exactly to the old behaviour (clear+near+centred
//!     ⇒ always seen; blocked / out-of-range / out-of-FOV ⇒ never).
//!
//! The noise is *stable*: keyed by `(observer, target, tick/window)` so detection
//! doesn't strobe frame to frame (it holds for `window` ticks, then re-rolls) and
//! is fully deterministic/replayable.
//!
//! ## The symmetry with the camera
//! This is the same fog math the volumetric renderer uses, sampled differently:
//! the player camera reads it foveated-fine (`sim::foveation` + `volumetric_gpu`),
//! every mob reads it coarse and cheap from one shared low-res [`OcclusionGrid`]
//! (world solidity + smoke + canopy, downsampled once per tick) instead of a
//! per-mob raycast. One model, two fidelities.
//!
//! ## GPU DISPATCH CONTRACT (batched mob perception)
//!   bindings: 0: occlusion grid [f32; n], 1: observers [{eye,facing}; O],
//!     2: targets [{pos,id}; T], 3: out perception [f32; O*T] (or packed bits via
//!     `detect`). uniforms: range, fov_cos, density_k, step, tick, window.
//!   dispatch: one invocation per (observer, target): march the segment for
//!     transmittance, multiply by distance falloff and FOV factor → perception;
//!     `can_see` = perception > stable_noise(obs,tgt,tick,window). Integer hash ⇒
//!     matches the CPU reference; the march is the cost the bench times.

use crate::sim::gpu_bench::{bench, BenchResult};

/// A coarse occlusion field: per-cell opacity in `[0,1]` (0 = clear air, 1 =
/// fully opaque). Row-major, x fastest.
#[derive(Clone, Debug)]
pub struct OcclusionGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub opacity: Vec<f32>,
}

impl OcclusionGrid {
    pub fn clear(nx: usize, ny: usize, nz: usize) -> Self {
        OcclusionGrid { nx, ny, nz, opacity: vec![0.0; nx * ny * nz] }
    }

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

    /// Nearest-cell opacity with clamp-to-edge sampling.
    #[inline]
    fn sample(&self, x: f32, y: f32, z: f32) -> f32 {
        let xi = (x.round() as i32).clamp(0, self.nx as i32 - 1) as usize;
        let yi = (y.round() as i32).clamp(0, self.ny as i32 - 1) as usize;
        let zi = (z.round() as i32).clamp(0, self.nz as i32 - 1) as usize;
        self.opacity[self.idx(xi, yi, zi)]
    }

    /// Transmittance along the segment `a→b`: `exp(-k·∫opacity)`. Returns
    /// `(transmittance, distance)`. 1.0 = clear line of sight, →0 = blocked.
    pub fn transmittance_along(&self, a: [f32; 3], b: [f32; 3], k: f32, step: f32) -> (f32, f32) {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let n = (dist / step).ceil().max(1.0) as usize;
        let seg = dist / n as f32;
        let mut od = 0.0f32;
        for i in 0..n {
            let t = (i as f32 + 0.5) / n as f32;
            od += self.sample(a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t) * seg;
        }
        ((-k * od).exp(), dist)
    }
}

/// Perception tunables.
#[derive(Clone, Copy, Debug)]
pub struct PerceptionParams {
    /// Maximum sight distance (perception is 0 beyond it).
    pub range: f32,
    /// FOV half-angle cosine (1 = straight ahead only, 0 = 90°, -1 = omni).
    pub fov_cos: f32,
    /// Opacity → extinction scale for the transmittance march.
    pub density_k: f32,
    /// March step in cells.
    pub step: f32,
    /// Stable-noise window in ticks (larger = steadier detection).
    pub window: u64,
}

impl Default for PerceptionParams {
    fn default() -> Self {
        PerceptionParams { range: 20.0, fov_cos: 0.5, density_k: 4.0, step: 0.5, window: 16 }
    }
}

/// Distance falloff: 1 at the eye, linearly to 0 at `range`.
#[inline]
pub fn dist_falloff(dist: f32, range: f32) -> f32 {
    (1.0 - dist / range).max(0.0)
}

/// FOV-centredness: 1 dead-ahead, 0 at the cone edge, 0 outside. `facing` and
/// `to_dir` must be unit vectors.
#[inline]
pub fn fov_factor(facing: [f32; 3], to_dir: [f32; 3], fov_cos: f32) -> f32 {
    let c = facing[0] * to_dir[0] + facing[1] * to_dir[1] + facing[2] * to_dir[2];
    if c <= fov_cos {
        0.0
    } else if fov_cos >= 1.0 {
        1.0
    } else {
        (c - fov_cos) / (1.0 - fov_cos)
    }
}

/// Graded perception (advanced API): transmittance × distance falloff × FOV.
/// In `[0,1]`; 0 when blocked, beyond range, or outside the FOV cone.
pub fn perceive(
    grid: &OcclusionGrid,
    eye: [f32; 3],
    facing: [f32; 3],
    target: [f32; 3],
    p: &PerceptionParams,
) -> f32 {
    let (t, dist) = grid.transmittance_along(eye, target, p.density_k, p.step);
    if dist > p.range {
        return 0.0;
    }
    let dv = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
    let dl = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt().max(1e-9);
    let to_dir = [dv[0] / dl, dv[1] / dl, dv[2] / dl];
    t * dist_falloff(dist, p.range) * fov_factor(facing, to_dir, p.fov_cos)
}

/// Stable per-pair noise in `[0,1)`: deterministic, uniform, and constant for
/// `window` ticks (so a thresholded decision doesn't strobe).
pub fn stable_noise(obs_id: u32, tgt_id: u32, tick: u64, window: u64) -> f32 {
    let w = (tick / window.max(1)) as u32;
    let mut n = obs_id
        .wrapping_mul(2_654_435_761)
        ^ tgt_id.wrapping_mul(40_503)
        ^ w.wrapping_mul(668_265_263);
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    ((n ^ (n >> 16)) as f32) / 4_294_967_296.0
}

/// Threshold a graded perception into a boolean detection using stable noise.
/// `P(true) == perception` for `perception ∈ [0,1]`.
#[inline]
pub fn detect(perception: f32, obs_id: u32, tgt_id: u32, tick: u64, window: u64) -> bool {
    perception > stable_noise(obs_id, tgt_id, tick, window)
}

/// Simple API: does observer `obs_id` see target `tgt_id` this `tick`? The
/// gradient thresholded against stable noise — a calibrated, deterministic
/// drop-in for `vision::can_see`.
#[allow(clippy::too_many_arguments)]
pub fn can_see(
    grid: &OcclusionGrid,
    eye: [f32; 3],
    facing: [f32; 3],
    target: [f32; 3],
    p: &PerceptionParams,
    obs_id: u32,
    tgt_id: u32,
    tick: u64,
) -> bool {
    detect(perceive(grid, eye, facing, target, p), obs_id, tgt_id, tick, p.window)
}

/// Benchmark a graded perception query over `iters` runs (the march is the cost).
pub fn bench_perceive(
    grid: &OcclusionGrid,
    eye: [f32; 3],
    facing: [f32; 3],
    target: [f32; 3],
    p: &PerceptionParams,
    iters: usize,
) -> BenchResult {
    bench(iters, || {
        let v = perceive(grid, eye, facing, target, p);
        std::hint::black_box(&v);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> PerceptionParams {
        PerceptionParams { range: 20.0, fov_cos: 0.5, density_k: 8.0, step: 0.5, window: 16 }
    }

    #[test]
    fn transmittance_clear_vs_blocked() {
        let g = OcclusionGrid::clear(12, 6, 12);
        let (tc, _) = g.transmittance_along([0.0, 3.0, 0.0], [11.0, 3.0, 11.0], 8.0, 0.5);
        assert!(tc > 0.95, "clear LOS should transmit ~1, got {tc}");
        let mut wall = OcclusionGrid::clear(12, 6, 12);
        for y in 0..6 {
            for z in 0..12 {
                let i = wall.idx(6, y, z);
                wall.opacity[i] = 1.0; // opaque slab across the path
            }
        }
        let (tw, _) = wall.transmittance_along([0.0, 3.0, 6.0], [11.0, 3.0, 6.0], 8.0, 0.5);
        assert!(tw < 0.05, "blocked LOS should transmit ~0, got {tw}");
    }

    #[test]
    fn perceive_bounds_and_zeroing() {
        let g = OcclusionGrid::clear(12, 6, 12);
        let p = params();
        let near = perceive(&g, [0.0, 3.0, 6.0], [1.0, 0.0, 0.0], [2.0, 3.0, 6.0], &p);
        assert!(near > 0.5 && near <= 1.0, "near/clear/centred should be high: {near}");
        let mut pr = p;
        pr.range = 1.0;
        assert_eq!(perceive(&g, [0.0, 3.0, 6.0], [1.0, 0.0, 0.0], [2.0, 3.0, 6.0], &pr), 0.0, "beyond range");
        let behind = perceive(&g, [0.0, 3.0, 6.0], [1.0, 0.0, 0.0], [-2.0, 3.0, 6.0], &p);
        assert_eq!(behind, 0.0, "behind the observer is out of FOV");
    }

    #[test]
    fn stable_noise_deterministic_window_uniform() {
        assert_eq!(stable_noise(3, 7, 100, 16), stable_noise(3, 7, 100, 16));
        assert_eq!(stable_noise(3, 7, 100, 16), stable_noise(3, 7, 107, 16), "stable within window");
        assert_ne!(stable_noise(3, 7, 100, 16), stable_noise(3, 7, 200, 16), "re-rolls across windows");
        let mut below = 0;
        let mut n = 0;
        for o in 0..20u32 {
            for t in 0..20u32 {
                for tk in (0..160u64).step_by(16) {
                    if stable_noise(o, t, tk, 16) < 0.5 {
                        below += 1;
                    }
                    n += 1;
                }
            }
        }
        let frac = below as f32 / n as f32;
        assert!((frac - 0.5).abs() < 0.05, "noise not ~uniform: {frac}");
    }

    #[test]
    fn detection_is_calibrated() {
        // P(detect) should equal the perception value.
        for &p in &[0.1f32, 0.3, 0.5, 0.7, 0.9] {
            let mut hits = 0;
            let mut n = 0;
            for o in 0..40u32 {
                for t in 0..40u32 {
                    for tk in (0..320u64).step_by(16) {
                        if detect(p, o, t, tk, 16) {
                            hits += 1;
                        }
                        n += 1;
                    }
                }
            }
            let rate = hits as f32 / n as f32;
            assert!((rate - p).abs() < 0.03, "P(detect)={rate} for perceive={p}");
        }
    }

    #[test]
    fn binary_limits_collapse_to_can_see() {
        // perceive == 1 → always; perceive == 0 → never, for any pair/tick.
        for o in 0..10u32 {
            for t in 0..10u32 {
                for tk in (0..80u64).step_by(16) {
                    assert!(detect(1.0, o, t, tk, 16));
                    assert!(!detect(0.0, o, t, tk, 16));
                }
            }
        }
    }

    #[test]
    fn bench_runs() {
        let g = OcclusionGrid::clear(16, 8, 16);
        let br = bench_perceive(&g, [0.0, 4.0, 8.0], [1.0, 0.0, 0.0], [12.0, 4.0, 8.0], &params(), 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("mob_perceive", Some(8.3)).contains("mob_perceive n=3"));
    }
}
