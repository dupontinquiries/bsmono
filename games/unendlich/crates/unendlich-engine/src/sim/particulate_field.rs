//! `sim::particulate_field` — a lingering airborne-particulate scalar field
//! (smoke, ash, dust, spores) meant to be registered as a new octree field
//! alongside temperature/moisture/wind/pressure. Unlike the transient spark/
//! ember effects, this *persists*: it diffuses, drifts downwind, settles out
//! under gravity, and decays — so a Fire dragon's ash cloud (a Fire+Sweep
//! technique writing into this field) becomes a real obscurant that drifts and
//! thins over time. It feeds vision/perception (`perception_fog`, `sound_masking`
//! occlusion). Same field-brush pattern as `water_physics`/`erosion`. Pure,
//! deterministic, mass-aware, panic-free.

/// Tunables for one step on a particulate grid (values normalised, 1 = opaque).
#[derive(Clone, Copy, Debug)]
pub struct ParticulateParams {
    /// Lateral spread per second (0..~0.25 stable).
    pub diffuse: f32,
    /// Fraction blown downwind per second per unit wind speed.
    pub advect: f32,
    /// Fraction that settles to the ground (leaves the air) per second.
    pub settle: f32,
    /// Fraction that decays/disperses entirely per second.
    pub decay: f32,
}

impl Default for ParticulateParams {
    fn default() -> Self {
        ParticulateParams { diffuse: 0.12, advect: 0.15, settle: 0.05, decay: 0.04 }
    }
}

#[inline]
fn c01(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.max(0.0).min(1.0) }
}
#[inline]
fn idx(x: usize, y: usize, w: usize) -> usize {
    y * w + x
}

/// Bilinear sample of `grid` at fractional `(sx,sy)` (already clamped in-range).
fn bilinear(grid: &[f32], w: usize, h: usize, sx: f32, sy: f32) -> f32 {
    let x0 = sx.floor() as usize;
    let y0 = sy.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = sx - x0 as f32;
    let fy = sy - y0 as f32;
    let a = grid[idx(x0, y0, w)];
    let b = grid[idx(x1, y0, w)];
    let c = grid[idx(x0, y1, w)];
    let d = grid[idx(x1, y1, w)];
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    top + (bot - top) * fy
}

/// Inject `amount` of particulate at `(cx,cy)` with a soft radius `r` (a brush —
/// e.g. an ash burst). Adds into the grid, clamped to opaque.
pub fn emit(grid: &mut [f32], w: usize, h: usize, cx: i32, cy: i32, r: i32, amount: f32) {
    if w == 0 || h == 0 || r < 0 {
        return;
    }
    let a = amount.max(0.0);
    let rr = (r * r) as f32 + 1.0;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let d2 = ((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f32;
            if d2 <= rr {
                let fall = 1.0 - d2 / rr; // 1 at center → 0 at edge
                let i = idx(x as usize, y as usize, w);
                grid[i] = c01(grid[i] + a * fall);
            }
        }
    }
}

/// Advance the particulate grid one step. `wind` is a per-cell (u,v) flow (cells/
/// sec); a uniform wind drifts the whole cloud. Diffusion is mass-conserving
/// (a 4-neighbour exchange); settle + decay remove mass (it leaves the air).
pub fn step(
    grid: &mut [f32],
    w: usize,
    h: usize,
    wind: (f32, f32),
    p: &ParticulateParams,
    dt: f32,
) {
    if w == 0 || h == 0 || grid.len() < w * h {
        return;
    }
    let dt = dt.max(0.0);
    let diff = (p.diffuse * dt).clamp(0.0, 0.25);
    let src = grid.to_vec();

    // 1. Diffusion: symmetric 4-neighbour exchange (conserves mass; edges are
    //    closed so nothing leaks off-grid).
    for y in 0..h {
        for x in 0..w {
            let here = src[idx(x, y, w)];
            let mut acc = here;
            let nb = |nx: usize, ny: usize, acc: &mut f32| {
                *acc += diff * (src[idx(nx, ny, w)] - here);
            };
            if x > 0 { nb(x - 1, y, &mut acc); }
            if x + 1 < w { nb(x + 1, y, &mut acc); }
            if y > 0 { nb(x, y - 1, &mut acc); }
            if y + 1 < h { nb(x, y + 1, &mut acc); }
            grid[idx(x, y, w)] = acc;
        }
    }

    // 2. Advection: semi-Lagrangian backtrace along the wind (drifts the cloud
    //    downwind). Bilinear sampling so sub-cell drift accumulates instead of
    //    rounding away.
    let (wu, wv) = wind;
    if wu != 0.0 || wv != 0.0 {
        let advect = p.advect.max(0.0) * dt;
        let moved = grid.to_vec();
        for y in 0..h {
            for x in 0..w {
                let sx = (x as f32 - wu * advect).clamp(0.0, (w - 1) as f32);
                let sy = (y as f32 - wv * advect).clamp(0.0, (h - 1) as f32);
                grid[idx(x, y, w)] = bilinear(&moved, w, h, sx, sy);
            }
        }
    }

    // 3. Settle + decay: mass leaves the air. Multiplicative, so it can't go
    //    negative and a still cloud thins toward zero.
    let keep = (1.0 - (p.settle.max(0.0) + p.decay.max(0.0)) * dt).clamp(0.0, 1.0);
    for v in grid.iter_mut() {
        *v = c01(*v * keep);
    }
}

/// Total airborne mass (for tests / HUD).
pub fn total(grid: &[f32]) -> f32 {
    grid.iter().copied().map(|v| v.max(0.0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(w: usize, h: usize) -> Vec<f32> { vec![0.0; w * h] }

    #[test]
    fn emit_deposits_a_blob() {
        let (w, h) = (16, 16);
        let mut g = grid(w, h);
        emit(&mut g, w, h, 8, 8, 3, 1.0);
        assert!(g[idx(8, 8, w)] > g[idx(11, 8, w)], "denser at the center");
        assert!(total(&g) > 0.0);
        assert!(g.iter().all(|&v| (0.0..=1.0).contains(&v)));
    }

    #[test]
    fn a_still_cloud_thins_but_never_negative() {
        let (w, h) = (16, 16);
        let mut g = grid(w, h);
        emit(&mut g, w, h, 8, 8, 4, 1.0);
        let m0 = total(&g);
        for _ in 0..50 {
            step(&mut g, w, h, (0.0, 0.0), &ParticulateParams::default(), 0.1);
        }
        let m1 = total(&g);
        assert!(m1 < m0, "settle + decay remove airborne mass over time");
        assert!(g.iter().all(|&v| v >= 0.0), "never negative");
    }

    #[test]
    fn diffusion_spreads_without_creating_mass() {
        let (w, h) = (21, 21);
        let mut g = grid(w, h);
        g[idx(10, 10, w)] = 1.0;
        let m0 = total(&g);
        // diffusion only (no settle/decay/advect) must conserve mass
        let p = ParticulateParams { diffuse: 0.2, advect: 0.0, settle: 0.0, decay: 0.0 };
        for _ in 0..20 {
            step(&mut g, w, h, (0.0, 0.0), &p, 0.1);
        }
        assert!((total(&g) - m0).abs() < 1e-3, "diffusion conserves mass");
        assert!(g[idx(9, 10, w)] > 0.0 && g[idx(11, 10, w)] > 0.0, "it spread to neighbours");
    }

    #[test]
    fn wind_drifts_the_cloud_downwind() {
        let (w, h) = (32, 8);
        let mut g = grid(w, h);
        emit(&mut g, w, h, 6, 4, 2, 1.0);
        let centroid_x = |g: &[f32]| {
            let mut num = 0.0;
            let mut den = 0.0;
            for y in 0..h {
                for x in 0..w {
                    let v = g[idx(x, y, w)];
                    num += v * x as f32;
                    den += v;
                }
            }
            if den > 0.0 { num / den } else { 0.0 }
        };
        let x0 = centroid_x(&g);
        let p = ParticulateParams { diffuse: 0.02, advect: 0.6, settle: 0.0, decay: 0.0 };
        for _ in 0..30 {
            step(&mut g, w, h, (1.0, 0.0), &p, 0.1); // wind blowing +x
        }
        assert!(centroid_x(&g) > x0 + 0.5, "cloud drifts downwind");
    }

    #[test]
    fn deterministic_and_safe_on_empty() {
        let (w, h) = (8, 8);
        let mut a = grid(w, h);
        let mut b = grid(w, h);
        emit(&mut a, w, h, 4, 4, 2, 0.8);
        emit(&mut b, w, h, 4, 4, 2, 0.8);
        for _ in 0..10 {
            step(&mut a, w, h, (0.3, -0.2), &ParticulateParams::default(), 0.1);
            step(&mut b, w, h, (0.3, -0.2), &ParticulateParams::default(), 0.1);
        }
        assert_eq!(a, b);
        // zero-size / undersized grids don't panic
        step(&mut [], 0, 0, (1.0, 1.0), &ParticulateParams::default(), 0.1);
    }
}

// ── service registration (S3) ──
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

const PF_W: usize = 8;
const PF_H: usize = 8;

// Holds a small closed field + default params; the step kernel is free-standing.
struct ParticulateService {
    grid: Vec<f32>,
    params: ParticulateParams,
}
impl Service for ParticulateService {
    fn name(&self) -> &'static str {
        "particulate_field"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        // neutral: no wind, default diffusion/decay
        step(&mut self.grid, PF_W, PF_H, (0.0, 0.0), &self.params, ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static PARTICULATE_FIELD_SERVICE: ServiceEntry = ServiceEntry {
    name: "particulate_field",
    make: || Box::new(ParticulateService { grid: vec![0.0; PF_W * PF_H], params: ParticulateParams::default() }),
};
