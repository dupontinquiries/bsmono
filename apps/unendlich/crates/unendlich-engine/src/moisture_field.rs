//! 2.5D wind-advected moisture field (turn-2 "vectors -> geometry" groundwork).
//!
//! A world-anchored horizontal grid of moisture that the wind pushes around on
//! the physics tick. The fog pass samples it for density (fog approach 4), and
//! the same Eulerian-advection machinery is what the later water-current phase
//! reuses. This module is the advection CORE only -- no GPU, no Vulkan, no
//! sources/sinks yet -- so it is pure, deterministic, and unit-testable.
//!
//! Scheme: semi-Lagrangian backtrace with an optional MacCormack correction and
//! a monotone clamp (the `clamp` flag, wired to `DEBUG.advect_clamp`). Plain SL
//! is unconditionally stable but diffusive; MacCormack restores second-order
//! sharpness; the clamp limits the corrected value to the source-cell
//! neighbourhood so it can never create a new extremum (no negative density, no
//! ringing). World-anchored sampling means the field never swims with the
//! camera the way a view-aligned froxel volume does.

/// Bilinear sample of `field` at fractional grid coords, clamped to the grid
/// edges. Row-major: index = z * w + x.
#[inline]
fn bilinear(field: &[f32], fx: f32, fz: f32, w: usize, h: usize) -> f32 {
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fz = fz.clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as usize;
    let z0 = fz.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let z1 = (z0 + 1).min(h - 1);
    let tx = fx - x0 as f32;
    let tz = fz - z0 as f32;
    let c00 = field[z0 * w + x0];
    let c10 = field[z0 * w + x1];
    let c01 = field[z1 * w + x0];
    let c11 = field[z1 * w + x1];
    let a = c00 + (c10 - c00) * tx;
    let b = c01 + (c11 - c01) * tx;
    a + (b - a) * tz
}

/// Min/max of the 2x2 source stencil a backtrace to (fx, fz) reads from. Used to
/// clamp the MacCormack correction so it stays monotone (no new extrema).
#[inline]
fn stencil_min_max(field: &[f32], fx: f32, fz: f32, w: usize, h: usize) -> (f32, f32) {
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fz = fz.clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as usize;
    let z0 = fz.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let z1 = (z0 + 1).min(h - 1);
    let c00 = field[z0 * w + x0];
    let c10 = field[z0 * w + x1];
    let c01 = field[z1 * w + x0];
    let c11 = field[z1 * w + x1];
    (
        c00.min(c10).min(c01).min(c11),
        c00.max(c10).max(c01).max(c11),
    )
}

/// World-anchored 2.5D moisture field.
pub struct MoistureField {
    pub w: usize,
    pub h: usize,
    /// Metres per cell.
    pub cell: f32,
    /// World XZ of cell (0,0)'s centre. Anchoring to world space is what keeps
    /// the field from swimming as the camera moves.
    pub origin: [f32; 2],
    m: Vec<f32>,
    fwd: Vec<f32>,
    bwd: Vec<f32>,
    prev: Vec<f32>,
}

impl MoistureField {
    pub fn new(w: usize, h: usize, cell: f32, origin: [f32; 2]) -> Self {
        assert!(w >= 2 && h >= 2, "moisture grid must be at least 2x2");
        assert!(cell > 0.0, "cell size must be positive");
        Self {
            w,
            h,
            cell,
            origin,
            m: vec![0.0; w * h],
            fwd: vec![0.0; w * h],
            bwd: vec![0.0; w * h],
            prev: vec![0.0; w * h],
        }
    }

    #[inline]
    pub fn moisture(&self) -> &[f32] {
        &self.m
    }

    #[inline]
    pub fn at(&self, x: usize, z: usize) -> f32 {
        self.m[z * self.w + x]
    }

    #[inline]
    pub fn set(&mut self, x: usize, z: usize, v: f32) {
        self.m[z * self.w + x] = v;
    }

    /// Re-anchor the window to `new_origin`, snapped to the cell grid, shifting
    /// existing moisture so every cell keeps its WORLD position (the field stays
    /// world-anchored — R10). Cells that scroll into view are zero-filled; cells
    /// that scroll off are dropped. This is the prerequisite for player-following
    /// fog: today the window is fixed, this lets it track the camera without the
    /// pattern swimming.
    ///
    /// Motion is snapped to a whole number of cells (sub-cell motion is ignored
    /// so the grid never resamples / blurs). Both the current field and the
    /// previous-tick snapshot are shifted, so a recenter between ticks doesn't
    /// tear the render interpolation. Returns the integer cell delta applied.
    pub fn recenter(&mut self, new_origin: [f32; 2]) -> [i64; 2] {
        let dx = ((new_origin[0] - self.origin[0]) / self.cell).round() as i64;
        let dz = ((new_origin[1] - self.origin[1]) / self.cell).round() as i64;
        if dx == 0 && dz == 0 {
            return [0, 0];
        }
        let (w, h) = (self.w as i64, self.h as i64);
        // New cell (x,z) inherits old cell (x+dx, z+dz); off-grid -> 0.
        let mut nm = vec![0.0f32; self.m.len()];
        let mut np = vec![0.0f32; self.prev.len()];
        for z in 0..h {
            for x in 0..w {
                let sx = x + dx;
                let sz = z + dz;
                if sx >= 0 && sx < w && sz >= 0 && sz < h {
                    let si = (sz * w + sx) as usize;
                    let di = (z * w + x) as usize;
                    nm[di] = self.m[si];
                    np[di] = self.prev[si];
                }
            }
        }
        self.m = nm;
        self.prev = np;
        // Snap the anchor by exactly the integer cells we shifted.
        self.origin = [
            self.origin[0] + dx as f32 * self.cell,
            self.origin[1] + dz as f32 * self.cell,
        ];
        [dx, dz]
    }

    /// Bilinear sample at a world XZ position -- what the fog pass (and a CPU
    /// parity check) would use. Clamped at the grid edge.
    pub fn sample_world(&self, wx: f32, wz: f32) -> f32 {
        let fx = (wx - self.origin[0]) / self.cell;
        let fz = (wz - self.origin[1]) / self.cell;
        bilinear(&self.m, fx, fz, self.w, self.h)
    }

    /// Snapshot the current field as the "previous tick" state. Call once at the
    /// start of each physics tick, before `advect` / `step_climate`, so the
    /// renderer can interpolate between ticks.
    pub fn begin_tick(&mut self) {
        self.prev.copy_from_slice(&self.m);
    }

    /// Bilinear sample at a world XZ, interpolated between the previous tick and
    /// the current one by `alpha` in [0,1]. Keeps the fog smooth at any
    /// framerate while the sim advances on a fixed tick (fixed-tick sim,
    /// interpolated render).
    pub fn sample_world_interp(&self, wx: f32, wz: f32, alpha: f32) -> f32 {
        let fx = (wx - self.origin[0]) / self.cell;
        let fz = (wz - self.origin[1]) / self.cell;
        let a = bilinear(&self.prev, fx, fz, self.w, self.h);
        let b = bilinear(&self.m, fx, fz, self.w, self.h);
        a + (b - a) * alpha.clamp(0.0, 1.0)
    }

    /// Advance the field one step of `dt` seconds, advecting along `wind`.
    /// `wind(x, z)` returns the world-space wind `[vx, vz]` in m/s at cell
    /// (x, z). `clamp` toggles the monotone MacCormack limiter
    /// (`DEBUG.advect_clamp`): true = stable (no negative density / overshoot),
    /// false = raw MacCormack (sharper, can ring).
    pub fn advect(&mut self, dt: f32, clamp: bool, wind: impl Fn(usize, usize) -> [f32; 2]) {
        let (w, h, cell) = (self.w, self.h, self.cell);
        let k = dt / cell; // wind (m/s) -> displacement in cells

        // Forward semi-Lagrangian: phi_hat^{n+1}(x) = phi^n(x - u*dt).
        for z in 0..h {
            for x in 0..w {
                let [vx, vz] = wind(x, z);
                self.fwd[z * w + x] =
                    bilinear(&self.m, x as f32 - vx * k, z as f32 - vz * k, w, h);
            }
        }

        // Reverse SL on the forward result: phi_hat^n(x) = phi_hat^{n+1}(x + u*dt).
        for z in 0..h {
            for x in 0..w {
                let [vx, vz] = wind(x, z);
                self.bwd[z * w + x] =
                    bilinear(&self.fwd, x as f32 + vx * k, z as f32 + vz * k, w, h);
            }
        }

        // MacCormack correction + optional monotone clamp. Result written into
        // `fwd` (read of `fwd[i]` as the SL fallback happens before the write).
        for z in 0..h {
            for x in 0..w {
                let i = z * w + x;
                let corrected = self.fwd[i] + 0.5 * (self.m[i] - self.bwd[i]);
                if clamp {
                    let [vx, vz] = wind(x, z);
                    let (lo, hi) =
                        stencil_min_max(&self.m, x as f32 - vx * k, z as f32 - vz * k, w, h);
                    // Limiter: if the correction left the source range, fall back
                    // to the plain SL value (always in range); else keep it.
                    if corrected < lo || corrected > hi {
                        // self.fwd[i] already holds the SL value -- leave it.
                    } else {
                        self.fwd[i] = corrected;
                    }
                } else {
                    self.fwd[i] = corrected;
                }
            }
        }

        // Commit: fwd now holds the new field.
        std::mem::swap(&mut self.m, &mut self.fwd);
    }
}

// ─── Wind field (turn-2 "vectors") ──────────────────────────────────────────
//
// The advector takes a wind closure; this builds realistic winds: base
// atmosphere wind + divergence-free curl-noise gusts, deflected around terrain
// so air flows around hills (windward bank, lee thinning) instead of through
// them. Curl noise is divergence-free by construction, so it stirs moisture
// without spuriously creating or destroying it.

#[inline]
fn hash2(ix: i32, iz: i32) -> f32 {
    let mut h =
        (ix.wrapping_mul(374_761_393).wrapping_add(iz.wrapping_mul(668_265_263))) as u32;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}

#[inline]
fn smoothstep01(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise in [0,1], C1-continuous so its gradient (and thus curl) is
/// continuous.
fn value_noise(x: f32, z: f32) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let tx = smoothstep01(x - x0);
    let tz = smoothstep01(z - z0);
    let (ix, iz) = (x0 as i32, z0 as i32);
    let c00 = hash2(ix, iz);
    let c10 = hash2(ix + 1, iz);
    let c01 = hash2(ix, iz + 1);
    let c11 = hash2(ix + 1, iz + 1);
    let a = c00 + (c10 - c00) * tx;
    let b = c01 + (c11 - c01) * tx;
    a + (b - a) * tz
}

/// World-space finite-difference step for the curl. Shared with the divergence
/// test: the discrete divergence of the discrete curl cancels to float zero
/// only when both use the same step.
const CURL_EPS: f32 = 1.0;

/// Divergence-free 2D gust velocity = curl of a scalar noise potential. Returns
/// `[vx, vz]` whose discrete divergence is ~zero, so advecting a scalar through
/// it conserves mass. `scale` sets eddy size (smaller = bigger eddies).
pub fn curl_noise(x: f32, z: f32, scale: f32) -> [f32; 2] {
    let e = CURL_EPS;
    let psi = |wx: f32, wz: f32| value_noise(wx * scale, wz * scale);
    // velocity = (dPsi/dz, -dPsi/dx), central differences in world space.
    let dpsi_dz = (psi(x, z + e) - psi(x, z - e)) / (2.0 * e);
    let dpsi_dx = (psi(x + e, z) - psi(x - e, z)) / (2.0 * e);
    [dpsi_dz, -dpsi_dx]
}

/// Deflect a wind vector around terrain: remove the component pointing uphill
/// (along `grad_h`, which points toward higher ground), scaled by `strength` in
/// [0,1]. Wind blowing across or downhill is left unchanged, so air flows around
/// a hill's windward face and thins in the lee.
pub fn deflect_around_terrain(wind: [f32; 2], grad_h: [f32; 2], strength: f32) -> [f32; 2] {
    let gl = (grad_h[0] * grad_h[0] + grad_h[1] * grad_h[1]).sqrt();
    if gl < 1e-6 {
        return wind;
    }
    let n = [grad_h[0] / gl, grad_h[1] / gl];
    let uphill = wind[0] * n[0] + wind[1] * n[1];
    if uphill <= 0.0 {
        return wind;
    }
    let k = strength * uphill;
    [wind[0] - k * n[0], wind[1] - k * n[1]]
}

/// Build a per-cell wind closure for [`MoistureField::advect`]: base atmosphere
/// wind + curl-noise gusts, deflected by the terrain height gradient. `height`
/// samples terrain height (metres) at a world XZ position; the gradient is a
/// central difference at `cell` spacing. Later this `height` closure is backed
/// by the chunk heightmap; for tests it's any synthetic surface.
pub fn wind_sampler<'a, H: Fn(f32, f32) -> f32>(
    base: [f32; 2],
    gust: f32,
    gust_scale: f32,
    deflect: f32,
    origin: [f32; 2],
    cell: f32,
    height: &'a H,
) -> impl Fn(usize, usize) -> [f32; 2] + 'a {
    move |x, z| {
        let wx = origin[0] + x as f32 * cell;
        let wz = origin[1] + z as f32 * cell;
        let g = curl_noise(wx, wz, gust_scale);
        let mut w = [base[0] + g[0] * gust, base[1] + g[1] * gust];
        if deflect > 0.0 {
            let hl = height(wx - cell, wz);
            let hr = height(wx + cell, wz);
            let hd = height(wx, wz - cell);
            let hu = height(wx, wz + cell);
            let grad_h = [(hr - hl) / (2.0 * cell), (hu - hd) / (2.0 * cell)];
            w = deflect_around_terrain(w, grad_h, deflect);
        }
        w
    }
}

// ─── Sources & sinks (turn-2 climate coupling) ───────────────────────────────
//
// Advection moves total water around; this is where the field couples to the
// world and becomes weather. A cell stores TOTAL water `m`; the air holds up to
// `capacity(T)` of it as invisible vapour, and anything above that is condensed
// = visible fog. So warming the air raises capacity, drops the condensed
// fraction, and burns fog off WITHOUT removing water -- morning fog lifting for
// free. Evaporation adds water over lakes/sea; precipitation removes it when fog
// gets heavy; mixing relaxes toward the ambient air blowing in.

/// Per-cell climate inputs for [`MoistureField::step_climate`]. Backed by the
/// world sim's temperature field + water presence later; anything for tests.
#[derive(Clone, Copy, Debug)]
pub struct ClimateSample {
    /// Normalized temperature, ~[0,1] like the engine's climate T.
    pub temperature: f32,
    /// Fraction of the cell sitting over water (evaporation source), 0..1.
    pub is_water: f32,
}

/// Tunables for the sources/sinks step.
#[derive(Clone, Copy, Debug)]
pub struct ClimateParams {
    /// Saturation capacity at T=0.
    pub capacity_base: f32,
    /// Exponential growth of capacity with temperature (Clausius-Clapeyron-ish).
    pub capacity_temp_k: f32,
    /// Vapour gained per tick over water at full drive.
    pub evap_rate: f32,
    /// Relaxation of `m` toward `ambient` per tick (lateral/vertical exchange).
    pub mix_rate: f32,
    /// Background total-water the air blows in carrying.
    pub ambient: f32,
    /// Fraction of the condensed excess rained out per tick when supersaturated.
    pub rain_rate: f32,
}

impl Default for ClimateParams {
    fn default() -> Self {
        Self {
            capacity_base: 0.30,
            capacity_temp_k: 1.50,
            evap_rate: 0.10,
            mix_rate: 0.02,
            ambient: 0.05,
            rain_rate: 0.10,
        }
    }
}

/// Saturation capacity (max invisible vapour) at a normalized temperature.
/// Exponential, so warm air holds disproportionately more -- the lever behind
/// valley fog at night and burn-off by day.
#[inline]
pub fn capacity(p: &ClimateParams, temperature: f32) -> f32 {
    p.capacity_base * (p.capacity_temp_k * temperature).exp()
}

impl MoistureField {
    /// Apply one tick of sources and sinks. Call after [`Self::advect`].
    /// `climate(x, z)` supplies per-cell temperature + water presence.
    pub fn step_climate(
        &mut self,
        p: &ClimateParams,
        climate: impl Fn(usize, usize) -> ClimateSample,
    ) {
        for z in 0..self.h {
            for x in 0..self.w {
                let s = climate(x, z);
                let cap = capacity(p, s.temperature);
                let mut m = self.at(x, z);
                // Evaporation over water: add vapour, slowing as the cell nears
                // saturation (no source once the air is full).
                let headroom = (cap - m).max(0.0);
                m += p.evap_rate * s.is_water * s.temperature * headroom;
                // Mixing toward the ambient air.
                m += p.mix_rate * (p.ambient - m);
                // Precipitation: when supersaturated, rain out part of the
                // condensed excess (removes water; relaxes m back toward cap).
                if m > cap {
                    m -= p.rain_rate * (m - cap);
                }
                self.set(x, z, m.max(0.0));
            }
        }
    }

    /// Visible fog (condensate) at a cell: total water above the saturation
    /// capacity at that temperature, clamped at 0. This is what the fog pass
    /// samples for density; it falls automatically as temperature (capacity)
    /// rises, so morning fog burns off with no water removed.
    #[inline]
    pub fn fog_density(&self, p: &ClimateParams, x: usize, z: usize, temperature: f32) -> f32 {
        (self.at(x, z) - capacity(p, temperature)).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(w: usize, h: usize, hot: &[(usize, usize, f32)]) -> MoistureField {
        let mut f = MoistureField::new(w, h, 1.0, [0.0, 0.0]);
        for &(x, z, v) in hot {
            f.set(x, z, v);
        }
        f
    }

    fn stats(f: &MoistureField) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in f.moisture() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (lo, hi)
    }

    #[test]
    fn zero_wind_is_identity() {
        let mut f = seeded(8, 8, &[(3, 3, 1.0), (4, 5, 0.5)]);
        let before = f.moisture().to_vec();
        f.advect(1.0, true, |_, _| [0.0, 0.0]);
        for (a, b) in f.moisture().iter().zip(before.iter()) {
            assert!((a - b).abs() < 1e-6, "zero wind must not move anything");
        }
    }

    #[test]
    fn uniform_wind_translates_one_cell() {
        // cell=1, dt=1, vx=1  => exactly one cell of +x displacement.
        let mut f = seeded(8, 8, &[(3, 3, 1.0)]);
        f.advect(1.0, true, |_, _| [1.0, 0.0]);
        assert!(f.at(4, 3) > 0.99, "hot cell should land at x+1, got {}", f.at(4, 3));
        assert!(f.at(3, 3) < 0.01, "origin cell should empty, got {}", f.at(3, 3));
    }

    #[test]
    fn clamp_off_can_ring_negative() {
        // Impulse advected by a fractional (0.5-cell) wind makes MacCormack
        // overshoot into negative density next to the step.
        let mut f = seeded(8, 8, &[(4, 4, 1.0)]);
        f.advect(1.0, false, |_, _| [0.5, 0.0]);
        let (lo, _hi) = stats(&f);
        assert!(lo < -1e-4, "raw MacCormack should undershoot below 0, min={}", lo);
    }

    #[test]
    fn clamp_on_stays_monotone() {
        // Same setup, clamp on: no negative density, no overshoot above the
        // source maximum (1.0).
        let mut f = seeded(8, 8, &[(4, 4, 1.0)]);
        f.advect(1.0, true, |_, _| [0.5, 0.0]);
        let (lo, hi) = stats(&f);
        assert!(lo >= -1e-6, "clamped advection must not go negative, min={}", lo);
        assert!(hi <= 1.0 + 1e-6, "clamped advection must not overshoot, max={}", hi);
    }

    #[test]
    fn sample_world_matches_cells_at_integer_coords() {
        let mut f = MoistureField::new(4, 4, 2.0, [10.0, 20.0]);
        f.set(2, 1, 0.7);
        // cell (2,1) centre is at world (10 + 2*2, 20 + 1*2) = (14, 22).
        assert!((f.sample_world(14.0, 22.0) - 0.7).abs() < 1e-6);
        // halfway to the empty neighbour at world x=16 -> ~0.35.
        assert!((f.sample_world(15.0, 22.0) - 0.35).abs() < 1e-6);
    }

    #[test]
    fn curl_noise_is_divergence_free() {
        let e = CURL_EPS; // same step as the curl -> exact discrete cancellation
        let scale = 0.1_f32;
        let mut worst = 0.0_f32;
        for &(x, z) in &[(3.0, 7.0), (12.0, -4.0), (0.5, 0.5), (-8.0, 20.0)] {
            let vx_px = curl_noise(x + e, z, scale)[0];
            let vx_mx = curl_noise(x - e, z, scale)[0];
            let vz_pz = curl_noise(x, z + e, scale)[1];
            let vz_mz = curl_noise(x, z - e, scale)[1];
            let div = (vx_px - vx_mx) / (2.0 * e) + (vz_pz - vz_mz) / (2.0 * e);
            worst = worst.max(div.abs());
        }
        assert!(worst < 1e-4, "discrete divergence should cancel, worst={}", worst);
    }

    #[test]
    fn deflect_removes_uphill_component() {
        // straight uphill, full strength -> stopped
        let w = deflect_around_terrain([1.0, 0.0], [1.0, 0.0], 1.0);
        assert!(w[0].abs() < 1e-6 && w[1].abs() < 1e-6, "got {:?}", w);
        // across the slope -> unchanged
        assert_eq!(deflect_around_terrain([0.0, 1.0], [1.0, 0.0], 1.0), [0.0, 1.0]);
        // downhill -> unchanged
        assert_eq!(deflect_around_terrain([-1.0, 0.0], [1.0, 0.0], 1.0), [-1.0, 0.0]);
        // flat terrain -> unchanged
        assert_eq!(deflect_around_terrain([1.0, 1.0], [0.0, 0.0], 1.0), [1.0, 1.0]);
    }

    #[test]
    fn wind_sampler_flat_terrain_is_base() {
        let flat = |_x: f32, _z: f32| 0.0_f32;
        let s = wind_sampler([2.0, 0.0], 0.0, 0.1, 1.0, [0.0, 0.0], 1.0, &flat);
        assert_eq!(s(5, 5), [2.0, 0.0]); // gust=0, flat -> exactly base
    }

    #[test]
    fn deflection_banks_moisture_windward_of_a_ridge() {
        // Ridge across x, peaking at z=8. Wind blows +z straight at it. With
        // deflection the uphill push is removed on the windward slope, so the
        // moisture band piles up windward instead of crossing. This is the
        // "geometry from vectors" behaviour, reduced to a measurable assertion.
        let (w, h) = (16usize, 16usize);
        let height = |_wx: f32, wz: f32| (10.0 - (wz - 8.0).abs() * 5.0).max(0.0);
        let crossed = |deflect: f32| -> f32 {
            let mut f = MoistureField::new(w, h, 1.0, [0.0, 0.0]);
            for x in 0..w {
                f.set(x, 2, 1.0); // band on the windward (low-z) side
            }
            let s = wind_sampler([0.0, 1.0], 0.0, 0.1, deflect, [0.0, 0.0], 1.0, &height);
            for _ in 0..8 {
                f.advect(1.0, true, &s);
            }
            let mut past = 0.0;
            for z in 9..h {
                for x in 0..w {
                    past += f.at(x, z);
                }
            }
            past
        };
        let none = crossed(0.0);
        let deflected = crossed(1.0);
        assert!(
            deflected < none,
            "deflection should keep more moisture windward: deflected={} vs none={}",
            deflected, none
        );
    }

    #[test]
    fn capacity_rises_with_temperature() {
        let p = ClimateParams::default();
        assert!(capacity(&p, 0.8) > capacity(&p, 0.2));
    }

    #[test]
    fn evaporation_adds_water_only_over_water() {
        let p = ClimateParams { mix_rate: 0.0, ambient: 0.0, ..ClimateParams::default() };
        let mut f = MoistureField::new(4, 4, 1.0, [0.0, 0.0]);
        f.step_climate(&p, |x, z| ClimateSample {
            temperature: 0.6,
            is_water: if (x, z) == (1, 1) { 1.0 } else { 0.0 },
        });
        assert!(f.at(1, 1) > 0.0, "water cell should gain vapour");
        assert_eq!(f.at(2, 2), 0.0, "dry cell should not");
    }

    #[test]
    fn evaporation_does_not_exceed_capacity() {
        let p = ClimateParams {
            mix_rate: 0.0,
            ambient: 0.0,
            rain_rate: 0.0,
            ..ClimateParams::default()
        };
        let cap = capacity(&p, 0.6);
        let mut f = MoistureField::new(2, 2, 1.0, [0.0, 0.0]);
        for _ in 0..200 {
            f.step_climate(&p, |_, _| ClimateSample { temperature: 0.6, is_water: 1.0 });
        }
        assert!(f.at(0, 0) <= cap + 1e-4, "evaporation must asymptote to capacity");
        assert!(f.at(0, 0) > 0.5 * cap, "should approach capacity, m={}", f.at(0, 0));
    }

    #[test]
    fn morning_burnoff_conserves_water() {
        // Fog at cold temp; warming raises capacity and clears the *visible* fog
        // with no change to the stored water (no source/sink active).
        let p = ClimateParams {
            evap_rate: 0.0,
            mix_rate: 0.0,
            rain_rate: 0.0,
            ..ClimateParams::default()
        };
        let mut f = MoistureField::new(2, 2, 1.0, [0.0, 0.0]);
        let m0 = capacity(&p, 0.1) + 0.2; // supersaturated when cold
        f.set(0, 0, m0);
        assert!(f.fog_density(&p, 0, 0, 0.1) > 0.0, "foggy when cold");
        f.step_climate(&p, |_, _| ClimateSample { temperature: 0.1, is_water: 0.0 });
        assert!((f.at(0, 0) - m0).abs() < 1e-6, "no source/sink -> water unchanged");
        assert_eq!(f.fog_density(&p, 0, 0, 0.9), 0.0, "warm air holds it -> burned off");
    }

    #[test]
    fn precipitation_rains_out_excess_only() {
        let p = ClimateParams {
            evap_rate: 0.0,
            mix_rate: 0.0,
            rain_rate: 0.5,
            ..ClimateParams::default()
        };
        let cap = capacity(&p, 0.3);
        let mut f = MoistureField::new(2, 2, 1.0, [0.0, 0.0]);
        f.set(0, 0, cap + 1.0); // heavily supersaturated
        f.step_climate(&p, |_, _| ClimateSample { temperature: 0.3, is_water: 0.0 });
        let m = f.at(0, 0);
        assert!(m < cap + 1.0, "rain should reduce supersaturated water");
        assert!(m >= cap - 1e-4, "rain removes only condensed excess, not vapour");
    }

    #[test]
    fn tick_interpolation_blends_prev_and_current() {
        let mut f = MoistureField::new(4, 4, 1.0, [0.0, 0.0]);
        f.set(1, 1, 1.0);
        f.begin_tick(); // prev := current (1.0 at cell (1,1))
        f.advect(1.0, true, |_, _| [1.0, 0.0]); // moisture moves (1,1)->(2,1)
        let p0 = f.sample_world_interp(1.0, 1.0, 0.0);
        let p1 = f.sample_world_interp(1.0, 1.0, 1.0);
        assert!(p0 > 0.99, "alpha=0 -> previous state, got {}", p0);
        assert!(p1 < 0.01, "alpha=1 -> current state, got {}", p1);
        let mid = f.sample_world_interp(1.0, 1.0, 0.5);
        assert!((mid - 0.5 * (p0 + p1)).abs() < 1e-6, "alpha=0.5 -> midpoint");
    }

    // ─── Property tests (ruleset invariants over randomized inputs) ──────────
    // A tiny deterministic LCG so these are reproducible without an rng crate.
    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn prop_clamp_keeps_values_in_input_range() {
        // INVARIANT: with the clamp on, advection never creates a value outside
        // the input field's [min,max], for any wind. If a future tweak breaks
        // monotonicity (negative density, ringing), this fires.
        let mut seed = 0x1234_5678u32;
        for _ in 0..200 {
            let mut f = MoistureField::new(8, 8, 1.0, [0.0, 0.0]);
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for z in 0..8 {
                for x in 0..8 {
                    let v = lcg(&mut seed);
                    f.set(x, z, v);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            let (wx, wz) = (lcg(&mut seed) * 4.0 - 2.0, lcg(&mut seed) * 4.0 - 2.0);
            f.advect(1.0, true, |_, _| [wx, wz]);
            for &v in f.moisture() {
                assert!(v >= lo - 1e-5 && v <= hi + 1e-5, "clamp escaped [{},{}] -> {}", lo, hi, v);
            }
        }
    }

    #[test]
    fn prop_curl_noise_divergence_free_random() {
        // INVARIANT: the wind gusts stay divergence-free (mass-conserving) at
        // arbitrary positions and eddy scales.
        let e = CURL_EPS;
        let mut seed = 0x9E37_79B9u32;
        for _ in 0..500 {
            let x = lcg(&mut seed) * 200.0 - 100.0;
            let z = lcg(&mut seed) * 200.0 - 100.0;
            let scale = 0.05 + lcg(&mut seed) * 0.20;
            let div = (curl_noise(x + e, z, scale)[0] - curl_noise(x - e, z, scale)[0]) / (2.0 * e)
                + (curl_noise(x, z + e, scale)[1] - curl_noise(x, z - e, scale)[1]) / (2.0 * e);
            assert!(div.abs() < 1e-3, "divergence {} at ({},{}) scale {}", div, x, z, scale);
        }
    }

    #[test]
    fn prop_outputs_stay_finite() {
        // INVARIANT (R14): advect + step_climate never produce NaN/Inf for
        // bounded inputs, across random grids, cell sizes, winds, and climates.
        let mut seed = 0xCAFE_F00Du32;
        let p = ClimateParams::default();
        for _ in 0..100 {
            let cell = 1.0 + lcg(&mut seed) * 4.0;
            let mut f = MoistureField::new(6, 6, cell, [0.0, 0.0]);
            for z in 0..6 {
                for x in 0..6 {
                    f.set(x, z, lcg(&mut seed));
                }
            }
            let (wx, wz) = (lcg(&mut seed) * 6.0 - 3.0, lcg(&mut seed) * 6.0 - 3.0);
            f.advect(1.0, true, |_, _| [wx, wz]);
            let (t, iw) = (lcg(&mut seed), lcg(&mut seed));
            f.step_climate(&p, |_, _| ClimateSample { temperature: t, is_water: iw });
            for &v in f.moisture() {
                assert!(v.is_finite(), "non-finite cell {}", v);
            }
        }
    }

    #[test]
    fn prop_climate_keeps_m_nonnegative() {
        // INVARIANT (R15): moisture never goes negative after a climate step.
        let mut seed = 0xFEED_BEEFu32;
        let p = ClimateParams::default();
        for _ in 0..100 {
            let mut f = MoistureField::new(4, 4, 1.0, [0.0, 0.0]);
            for z in 0..4 {
                for x in 0..4 {
                    f.set(x, z, lcg(&mut seed) * 2.0 - 0.5); // include negatives
                }
            }
            let (t, iw) = (lcg(&mut seed), lcg(&mut seed));
            f.step_climate(&p, |_, _| ClimateSample { temperature: t, is_water: iw });
            for &v in f.moisture() {
                assert!(v >= 0.0, "negative m {}", v);
            }
        }
    }

    #[test]
    fn recenter_zero_is_identity() {
        let f0 = seeded(8, 8, &[(2, 3, 1.0), (5, 6, 0.5)]);
        let mut f = seeded(8, 8, &[(2, 3, 1.0), (5, 6, 0.5)]);
        let d = f.recenter(f.origin);
        assert_eq!(d, [0, 0]);
        assert_eq!(f.origin, f0.origin);
        assert_eq!(f.moisture(), f0.moisture());
    }

    #[test]
    fn recenter_subcell_snaps_to_no_move() {
        // Less than half a cell rounds to a zero shift: grid + origin unchanged.
        let mut f = seeded(8, 8, &[(4, 4, 2.0)]);
        let before: Vec<f32> = f.moisture().to_vec();
        let d = f.recenter([0.3 * f.cell, -0.2 * f.cell]);
        assert_eq!(d, [0, 0]);
        assert_eq!(f.origin, [0.0, 0.0]);
        assert_eq!(f.moisture(), &before[..]);
    }

    #[test]
    fn recenter_snapping_is_exact() {
        // An exact integer-cell move snaps the origin exactly and shifts cells.
        let mut f = seeded(8, 8, &[(4, 4, 3.0)]);
        let cell = f.cell;
        let d = f.recenter([2.0 * cell, -3.0 * cell]);
        assert_eq!(d, [2, -3]);
        assert_eq!(f.origin, [2.0 * cell, -3.0 * cell]);
        // old (4,4) keeps its world position -> new (4-dx, 4-dz) = (2, 7).
        assert_eq!(f.at(2, 7), 3.0);
    }

    #[test]
    fn recenter_past_window_clears() {
        let mut f = seeded(8, 8, &[(0, 0, 1.0), (7, 7, 1.0), (4, 4, 5.0)]);
        let cell = f.cell;
        let d = f.recenter([100.0 * cell, 0.0]);
        assert_eq!(d, [100, 0]);
        for &v in f.moisture() {
            assert_eq!(v, 0.0, "grid must clear after scrolling fully past the window");
        }
    }

    #[test]
    fn prop_recenter_preserves_overlap_total() {
        // Cells are MOVED, not scaled: the moisture summed over the region that
        // stays in-window is conserved, and nothing becomes non-finite.
        let mut seed = 0x0C0F_FEE5u32;
        for _ in 0..200 {
            let (w, h, cell) = (6usize, 5usize, 1.0f32);
            let mut f = MoistureField::new(w, h, cell, [0.0, 0.0]);
            for z in 0..h {
                for x in 0..w {
                    f.set(x, z, lcg(&mut seed));
                }
            }
            let dx = (lcg(&mut seed) * 16.0) as i64 - 8; // [-8, 7]
            let dz = (lcg(&mut seed) * 14.0) as i64 - 7; // [-7, 6]
            // Old (sx,sz) survives iff its new index (sx-dx, sz-dz) is in-grid.
            let mut expected = 0.0f64;
            for sz in 0..h as i64 {
                for sx in 0..w as i64 {
                    let (nx, nz) = (sx - dx, sz - dz);
                    if nx >= 0 && nx < w as i64 && nz >= 0 && nz < h as i64 {
                        expected += f.at(sx as usize, sz as usize) as f64;
                    }
                }
            }
            f.recenter([dx as f32 * cell, dz as f32 * cell]);
            let mut got = 0.0f64;
            for &v in f.moisture() {
                assert!(v.is_finite(), "non-finite after recenter");
                got += v as f64;
            }
            assert!(
                (got - expected).abs() < 1e-4,
                "overlap total drift: got {} expected {} (dx={}, dz={})",
                got, expected, dx, dz
            );
        }
    }
}
