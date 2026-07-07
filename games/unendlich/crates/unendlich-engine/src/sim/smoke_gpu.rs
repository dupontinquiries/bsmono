//! Kernel: **smoke from fire** — a buoyant smoke-density field that fire (your
//! `world::lumen` ignite system) feeds, advected and decayed each tick, then
//! injected into the froxel volume so it renders through the
//! [`crate::sim::volumetric_gpu`] scattering-integration pass for free.
//!
//! The field update is: (1) **emit** density at fire cells, (2) build a buoyant
//! velocity (ambient wind + upward rise that grows with local density, i.e. hot
//! dense smoke climbs), (3) **advect** via semi-Lagrangian backtrace with
//! open boundaries (smoke leaving the domain is gone — unconditionally stable),
//! (4) **decay** exponentially. Mass is non-increasing under pure advection and
//! strictly decreasing under pure decay; buoyancy lifts the plume centroid.
//!
//! GPU-ready form: ping-pong density buffers, one thread per cell — emit inline,
//! compute local velocity, trilinearly sample the previous buffer at the
//! backtraced position, decay, write. The CPU reference here is the correctness
//! gate and the benchmarked body.
//!
//! ## Froxel injection (ties into volumetrics)
//! A render pass samples this field at each froxel's world position and adds
//! [`extinction_at`] to the froxel σ and [`emission_at`] to its in-scatter
//! source; the volumetric integration pass then composes it front-to-back. Smoke
//! thus darkens/obscures (extinction) and the embers near fire glow (emission).
//!
//! ## Analytics / frame pacing
//! Time the advect dispatch as a `gpu_timer` zone (suggest `"smoke"`), or report
//! the CPU reference cost via [`BenchResult::metric_line`] with `name =
//! "smoke_step"` — it then appears in the dashboard's GPU-timers panel under the
//! stage budget like the other passes.
//!
//! ## GPU DISPATCH CONTRACT
//!   bindings: 0: density_in [f32; n], 1: density_out [f32; n], 2: fires
//!     [{cell:u32, intensity:f32}; F]; uniforms: nx,ny,nz, dt, wind.xz,
//!     rise_base, rise_density, decay, emit, dmax.
//!   dispatch: one invocation per cell c: d = density_in[c]; for matching fire
//!     d = min(dmax, d + emit*intensity*dt); v = (wind.x, rise_base +
//!     rise_density*d, wind.z); s = trilinear(density_in, c - v*dt) [open
//!     bounds → 0]; density_out[c] = s * exp(-decay*dt). (Emit is folded into the
//!     sampled buffer in a tiny pre-pass or via the fire list; reference does it
//!     before sampling — match that ordering for bit-equality.)

use crate::sim::gpu_bench::{bench, BenchResult};

/// A flat 3D smoke-density field (row-major, x fastest).
#[derive(Clone, Debug)]
pub struct SmokeField {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Smoke density per cell (≥ 0). Length `nx*ny*nz`.
    pub density: Vec<f32>,
}

/// A burning cell feeding the smoke field.
#[derive(Clone, Copy, Debug)]
pub struct FireSource {
    pub cell: usize,
    pub intensity: f32,
}

/// Tunables for one smoke step.
#[derive(Clone, Copy, Debug)]
pub struct SmokeParams {
    /// Ambient wind (x, _, z); y is ignored (buoyancy drives vertical motion).
    pub wind: [f32; 3],
    /// Constant upward rise.
    pub rise_base: f32,
    /// Extra rise per unit density (hotter, denser smoke climbs faster).
    pub rise_density: f32,
    /// Exponential decay rate (dissipation) per unit time.
    pub decay: f32,
    /// Emission rate per unit fire intensity per unit time.
    pub emit: f32,
    /// Saturation cap on emitted density.
    pub dmax: f32,
}

impl SmokeField {
    pub fn new(nx: usize, ny: usize, nz: usize) -> Self {
        SmokeField { nx, ny, nz, density: vec![0.0; nx * ny * nz] }
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

    /// Trilinear sample with **open boundaries**: corners outside the grid
    /// contribute 0 (so advection that backtraces from outside pulls in empty
    /// air, and mass can leave the domain). At integer coordinates this returns
    /// exactly the cell value.
    pub fn sample(&self, px: f32, py: f32, pz: f32) -> f32 {
        let x0 = px.floor() as i32;
        let y0 = py.floor() as i32;
        let z0 = pz.floor() as i32;
        let fx = px - x0 as f32;
        let fy = py - y0 as f32;
        let fz = pz - z0 as f32;
        let mut acc = 0.0f32;
        for dz in 0..2i32 {
            for dy in 0..2i32 {
                for dx in 0..2i32 {
                    let x = x0 + dx;
                    let y = y0 + dy;
                    let z = z0 + dz;
                    if x < 0 || y < 0 || z < 0 {
                        continue;
                    }
                    let (xu, yu, zu) = (x as usize, y as usize, z as usize);
                    if xu >= self.nx || yu >= self.ny || zu >= self.nz {
                        continue;
                    }
                    let wx = if dx == 1 { fx } else { 1.0 - fx };
                    let wy = if dy == 1 { fy } else { 1.0 - fy };
                    let wz = if dz == 1 { fz } else { 1.0 - fz };
                    acc += wx * wy * wz * self.density[self.idx(xu, yu, zu)];
                }
            }
        }
        acc
    }
}

/// Per-cell buoyant velocity: ambient wind in x/z, upward rise that increases
/// with local density.
fn buoyant_velocity(density: &[f32], p: &SmokeParams) -> Vec<[f32; 3]> {
    density
        .iter()
        .map(|&d| [p.wind[0], p.rise_base + p.rise_density * d, p.wind[2]])
        .collect()
}

/// Advance the smoke field one tick: emit → buoyant advect → decay. Returns the
/// new field; the input is untouched (ping-pong friendly).
pub fn step_smoke(field: &SmokeField, p: &SmokeParams, dt: f32, fires: &[FireSource]) -> SmokeField {
    // 1. emit into a working copy
    let mut emitted = field.density.clone();
    for f in fires {
        if f.cell < emitted.len() {
            emitted[f.cell] = (emitted[f.cell] + p.emit * f.intensity * dt).min(p.dmax);
        }
    }
    let src = SmokeField { nx: field.nx, ny: field.ny, nz: field.nz, density: emitted };
    // 2. buoyant velocity from the emitted density
    let vel = buoyant_velocity(&src.density, p);
    // 3. semi-Lagrangian advection (backtrace + sample the emitted buffer)
    let decay = (-p.decay * dt).exp();
    let mut out = vec![0.0f32; src.len()];
    for z in 0..src.nz {
        for y in 0..src.ny {
            for x in 0..src.nx {
                let c = src.idx(x, y, z);
                let v = vel[c];
                let s = src.sample(
                    x as f32 - v[0] * dt,
                    y as f32 - v[1] * dt,
                    z as f32 - v[2] * dt,
                );
                // 4. decay
                out[c] = s * decay;
            }
        }
    }
    SmokeField { nx: field.nx, ny: field.ny, nz: field.nz, density: out }
}

/// Smoke extinction contribution for a froxel (added to σ in the scatter volume).
#[inline]
pub fn extinction_at(density: f32, k_sigma: f32) -> f32 {
    k_sigma * density
}

/// Smoke emission contribution for a froxel (added to the in-scatter source);
/// `glow` is the per-unit-density ember colour near fire.
#[inline]
pub fn emission_at(density: f32, glow: [f32; 3]) -> [f32; 3] {
    [glow[0] * density, glow[1] * density, glow[2] * density]
}

/// Benchmark one smoke step over `iters` runs.
pub fn bench_smoke_step(
    field: &SmokeField,
    p: &SmokeParams,
    dt: f32,
    fires: &[FireSource],
    iters: usize,
) -> BenchResult {
    bench(iters, || {
        let f = step_smoke(field, p, dt, fires);
        std::hint::black_box(&f);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> SmokeParams {
        SmokeParams { wind: [0.0, 0.0, 0.0], rise_base: 0.0, rise_density: 0.0, decay: 0.0, emit: 2.0, dmax: 8.0 }
    }

    fn total(f: &SmokeField) -> f32 {
        f.density.iter().sum()
    }

    fn vcentroid(f: &SmokeField) -> f32 {
        let mut num = 0.0;
        let mut den = 0.0;
        for z in 0..f.nz {
            for y in 0..f.ny {
                for x in 0..f.nx {
                    let w = f.density[f.idx(x, y, z)];
                    num += w * y as f32;
                    den += w;
                }
            }
        }
        if den > 1e-9 {
            num / den
        } else {
            0.0
        }
    }

    #[test]
    fn sample_is_identity_at_grid_points() {
        // guards against the trilinear-weight bug (weight must be 0, not 1, at the
        // far corner when the fractional offset is 0).
        let mut f = SmokeField::new(4, 4, 4);
        for (i, v) in f.density.iter_mut().enumerate() {
            *v = i as f32;
        }
        for z in 0..4 {
            for y in 0..4 {
                for x in 0..4 {
                    let got = f.sample(x as f32, y as f32, z as f32);
                    assert_eq!(got, f.density[f.idx(x, y, z)], "at ({x},{y},{z})");
                }
            }
        }
    }

    #[test]
    fn sample_open_boundary_outside_is_zero() {
        let mut f = SmokeField::new(3, 3, 3);
        for v in f.density.iter_mut() {
            *v = 1.0;
        }
        assert_eq!(f.sample(-5.0, 1.0, 1.0), 0.0);
        assert_eq!(f.sample(1.0, 100.0, 1.0), 0.0);
    }

    #[test]
    fn pure_decay_monotonic_decrease() {
        let mut f = SmokeField::new(5, 5, 5);
        for v in f.density.iter_mut() {
            *v = 1.0;
        }
        let mut p = params();
        p.decay = 0.7;
        let mut prev = total(&f);
        for _ in 0..20 {
            f = step_smoke(&f, &p, 0.3, &[]);
            let t = total(&f);
            assert!(t <= prev + 1e-5, "mass grew under pure decay: {prev}->{t}");
            prev = t;
        }
        assert!(prev < 1.0, "should have decayed substantially");
    }

    #[test]
    fn advection_open_boundary_no_mass_growth() {
        let mut f = SmokeField::new(8, 8, 8);
        let c = f.idx(4, 4, 4);
        f.density[c] = 5.0;
        let n = f.idx(3, 4, 4);
        f.density[n] = 3.0;
        let mut p = params();
        p.wind = [0.3, 0.0, 0.0];
        p.rise_base = 0.5;
        let m0 = total(&f);
        for _ in 0..40 {
            f = step_smoke(&f, &p, 0.4, &[]);
            assert!(total(&f) <= m0 + 1e-3, "advection created mass");
        }
    }

    #[test]
    fn buoyancy_lifts_centroid() {
        let mut f = SmokeField::new(6, 12, 6);
        let mut p = params();
        p.rise_base = 1.0;
        p.rise_density = 0.2;
        p.decay = 0.3;
        let fire = [FireSource { cell: f.idx(3, 0, 3), intensity: 1.0 }];
        let mut early = 0.0;
        for i in 0..25 {
            f = step_smoke(&f, &p, 0.5, &fire);
            if i == 3 {
                early = vcentroid(&f);
            }
        }
        let late = vcentroid(&f);
        assert!(late > early + 0.5, "plume did not rise: {early} -> {late}");
    }

    #[test]
    fn emit_respects_dmax() {
        let mut f = SmokeField::new(3, 3, 3);
        let mut p = params();
        p.dmax = 4.0;
        let fire = [FireSource { cell: f.idx(1, 1, 1), intensity: 100.0 }];
        f = step_smoke(&f, &p, 1.0, &fire);
        assert!(f.density.iter().cloned().fold(0.0f32, f32::max) <= 4.0 + 1e-6);
    }

    #[test]
    fn injection_mappings_and_bench() {
        assert_eq!(extinction_at(2.0, 0.5), 1.0);
        assert_eq!(emission_at(2.0, [0.5, 0.25, 0.0]), [1.0, 0.5, 0.0]);
        let f = SmokeField::new(8, 8, 8);
        let br = bench_smoke_step(&f, &params(), 0.5, &[], 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("smoke_step", Some(8.3)).contains("smoke_step n=3"));
    }
}
