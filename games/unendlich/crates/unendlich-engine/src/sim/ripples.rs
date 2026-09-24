//! `sim::ripples` — water that answers back: a height field that carries
//! disturbances as travelling waves.
//!
//! The surface the game draws is analytic (`world::water_waves`: octaves of
//! sines and a swell, a function of position and time), and a body moving
//! through water writes a radial blob into the flow tiles that fades where it
//! is (`water_flow_tiles::splash`). Neither travels: nothing spreads as a ring,
//! reflects off a bank or crosses another wave. This is the part that does.
//!
//! The damped 2D wave equation on a square grid that scrolls with the viewer:
//!
//! ```text
//!   h' = h + keep * (h - h_prev) + (c dt / dx)^2 * laplacian(h)
//! ```
//!
//! - explicit, so stable only while `c dt / dx <= 1/sqrt(2)`: each frame is split
//!   into substeps at half that limit;
//! - DRY cells are held at zero, which reflects a wave (inverted), as a steep
//!   bank does;
//! - the outer `SPONGE` cells damp progressively, so waves reaching the edge of
//!   the simulated patch leave instead of bouncing off an edge that is not there;
//! - `recenter` shifts the grid by whole cells and keeps every wave where it is
//!   in the world.
//!
//! The height is ADDED to the analytic surface by whoever draws or floats on it
//! (`height`, bilinear). Pure: no clock, no RNG, no GPU; deterministic.

/// Tuning, in blocks and seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RippleParams {
    /// Wave speed (blocks/s). Shallow-water waves go at sqrt(g * depth): a
    /// ~0.4 block pond is ~2 blocks/s; this is the speed of small ripples.
    pub speed: f32,
    /// Fraction of wave velocity kept per second (0..1): how long rings live.
    pub keep_per_s: f32,
    /// Width of the absorbing border, in cells.
    pub sponge: usize,
}

impl Default for RippleParams {
    fn default() -> Self {
        RippleParams { speed: 2.5, keep_per_s: 0.55, sponge: 6 }
    }
}

#[derive(Clone, Debug)]
pub struct RippleField {
    pub n: usize,
    pub cell: f32,
    /// World cell index (x, z) of grid cell (0, 0).
    pub origin: (i32, i32),
    h: Vec<f32>,
    prev: Vec<f32>,
    wet: Vec<bool>,
    pub params: RippleParams,
}

impl RippleField {
    /// An `n` x `n` patch of `cell`-block cells, all wet, flat, at the world origin.
    pub fn new(n: usize, cell: f32, params: RippleParams) -> RippleField {
        let n = n.max(8);
        RippleField { n, cell: cell.max(1e-3), origin: (0, 0), h: vec![0.0; n * n], prev: vec![0.0; n * n], wet: vec![true; n * n], params }
    }

    fn at(&self, i: usize, j: usize) -> usize { j * self.n + i }

    /// World (x, z) -> grid cell (possibly outside the grid).
    fn cell_of(&self, wx: f32, wz: f32) -> (i32, i32) {
        ((wx / self.cell).floor() as i32 - self.origin.0, (wz / self.cell).floor() as i32 - self.origin.1)
    }

    /// Centre the patch on world (x, z), keeping every wave where it is in the
    /// world; cells newly covered start flat. `is_water(world cell x, z)` says
    /// which cells hold water (dry ones reflect).
    pub fn recenter(&mut self, wx: f32, wz: f32, is_water: impl Fn(i32, i32) -> bool) {
        let half = (self.n / 2) as i32;
        let new_origin = ((wx / self.cell).floor() as i32 - half, (wz / self.cell).floor() as i32 - half);
        let (dx, dz) = (new_origin.0 - self.origin.0, new_origin.1 - self.origin.1);
        if dx != 0 || dz != 0 {
            let mut h = vec![0.0; self.n * self.n];
            let mut prev = vec![0.0; self.n * self.n];
            for j in 0..self.n as i32 {
                for i in 0..self.n as i32 {
                    let (si, sj) = (i + dx, j + dz);
                    if si >= 0 && sj >= 0 && (si as usize) < self.n && (sj as usize) < self.n {
                        let (d, s) = (self.at(i as usize, j as usize), self.at(si as usize, sj as usize));
                        h[d] = self.h[s];
                        prev[d] = self.prev[s];
                    }
                }
            }
            self.h = h;
            self.prev = prev;
            self.origin = new_origin;
        }
        for j in 0..self.n {
            for i in 0..self.n {
                let k = self.at(i, j);
                self.wet[k] = is_water(self.origin.0 + i as i32, self.origin.1 + j as i32);
                if !self.wet[k] {
                    self.h[k] = 0.0;
                    self.prev[k] = 0.0;
                }
            }
        }
    }

    /// Push the surface at world (x, z): a smooth bump (or dip, `amount` < 0) of
    /// `radius` blocks. A body entering the water, a stone, a wake.
    pub fn splash(&mut self, wx: f32, wz: f32, amount: f32, radius: f32) {
        let r = (radius / self.cell).max(0.5);
        let (ci, cj) = self.cell_of(wx, wz);
        let reach = r.ceil() as i32 + 1;
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (i, j) = (ci + di, cj + dj);
                if i < 0 || j < 0 || i as usize >= self.n || j as usize >= self.n { continue; }
                let d2 = (di * di + dj * dj) as f32 / (r * r);
                if d2 > 1.0 { continue; }
                let k = self.at(i as usize, j as usize);
                if !self.wet[k] { continue; }
                let w = 0.5 * (1.0 + (std::f32::consts::PI * d2.sqrt()).cos()); // smooth, 1 at the centre, 0 at r
                self.h[k] += amount * w;
                self.prev[k] += amount * w; // a displacement at rest: it spreads rather than jumps
            }
        }
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        if !(dt > 0.0) || !dt.is_finite() { return; }
        let c = self.params.speed.max(1e-4);
        let limit = 0.5 * self.cell / c; // half of the 2D stability limit (dx / (c sqrt 2)) ~ margin
        let steps = ((dt / limit).ceil() as u32).clamp(1, 256);
        let h = dt / steps as f32;
        for _ in 0..steps {
            self.substep(h);
        }
    }

    fn substep(&mut self, h: f32) {
        let n = self.n;
        let c2 = (self.params.speed * h / self.cell).powi(2);
        let keep = self.params.keep_per_s.clamp(0.0, 1.0).powf(h);
        let sponge = self.params.sponge.min(n / 2);
        let mut next = vec![0.0f32; n * n];
        for j in 0..n {
            for i in 0..n {
                let k = self.at(i, j);
                if !self.wet[k] { continue; }
                // Neighbours: a dry one counts as zero height (a reflecting bank);
                // past the grid edge, the cell's own height (no flux), and the
                // sponge takes the energy out before it gets there.
                let nb = |ii: isize, jj: isize| -> f32 {
                    if ii < 0 || jj < 0 || ii as usize >= n || jj as usize >= n { return self.h[k]; }
                    let kk = jj as usize * n + ii as usize;
                    if self.wet[kk] { self.h[kk] } else { 0.0 }
                };
                let (ii, jj) = (i as isize, j as isize);
                let lap = nb(ii - 1, jj) + nb(ii + 1, jj) + nb(ii, jj - 1) + nb(ii, jj + 1) - 4.0 * self.h[k];
                let edge = i.min(j).min(n - 1 - i).min(n - 1 - j);
                let sp = if edge < sponge { let t = 1.0 - edge as f32 / sponge as f32; 1.0 - 0.35 * t * t } else { 1.0 };
                next[k] = self.h[k] + keep * sp * (self.h[k] - self.prev[k]) + c2 * lap;
                next[k] *= if edge < sponge { sp.sqrt() } else { 1.0 };
            }
        }
        self.prev = std::mem::replace(&mut self.h, next);
    }

    /// Height at world (x, z), bilinear; 0 outside the patch or over land.
    pub fn height(&self, wx: f32, wz: f32) -> f32 {
        let fx = wx / self.cell - self.origin.0 as f32 - 0.5;
        let fz = wz / self.cell - self.origin.1 as f32 - 0.5;
        let (i0, j0) = (fx.floor(), fz.floor());
        let (tx, tz) = (fx - i0, fz - j0);
        let g = |i: f32, j: f32| -> f32 {
            if i < 0.0 || j < 0.0 || i as usize >= self.n || j as usize >= self.n { return 0.0; }
            self.h[j as usize * self.n + i as usize]
        };
        let a = g(i0, j0) * (1.0 - tx) + g(i0 + 1.0, j0) * tx;
        let b = g(i0, j0 + 1.0) * (1.0 - tx) + g(i0 + 1.0, j0 + 1.0) * tx;
        a * (1.0 - tz) + b * tz
    }

    /// Discrete energy: potential (height gradient) plus kinetic (rate of change).
    pub fn energy(&self, dt_last: f32) -> f32 {
        let n = self.n;
        let mut e = 0.0;
        for j in 0..n {
            for i in 0..n {
                let k = self.at(i, j);
                let v = (self.h[k] - self.prev[k]) / dt_last.max(1e-6);
                e += v * v / (self.params.speed * self.params.speed);
                if i + 1 < n { let d = self.h[k + 1] - self.h[k]; e += d * d / (self.cell * self.cell); }
                if j + 1 < n { let d = self.h[k + n] - self.h[k]; e += d * d / (self.cell * self.cell); }
            }
        }
        e
    }

    /// Largest |height| in the patch.
    pub fn peak(&self) -> f32 { self.h.iter().fold(0.0, |m, x| m.max(x.abs())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> RippleParams { RippleParams { keep_per_s: 1.0, ..Default::default() } }

    /// Radius (blocks) of the strongest crest along +x from (cx, cz).
    fn ring_radius(f: &RippleField, cx: f32, cz: f32) -> f32 {
        let mut best = (0.0f32, 0.0f32);
        let mut r = 0.0;
        while r < f.n as f32 * f.cell * 0.45 {
            let h = f.height(cx + r, cz).abs();
            if h > best.0 { best = (h, r); }
            r += f.cell * 0.25;
        }
        best.1
    }

    #[test]
    fn a_splash_spreads_as_a_ring_at_the_wave_speed() {
        let mut f = RippleField::new(96, 0.25, quiet());
        f.recenter(0.0, 0.0, |_, _| true);
        f.splash(0.0, 0.0, 0.2, 0.75);
        for t in [1.0f32, 2.0, 3.0] {
            let mut g = f.clone();
            g.step(t);
            let r = ring_radius(&g, 0.0, 0.0);
            let expect = g.params.speed * t;
            assert!((r - expect).abs() < 0.9, "after {t} s the crest is {r:.2} blocks out, the wave speed says {expect:.2}");
        }
    }

    #[test]
    fn energy_never_grows_and_damping_takes_it_away() {
        for dt in [1.0f32 / 144.0, 1.0 / 30.0, 0.25] {
            let mut f = RippleField::new(64, 0.25, quiet());
            f.recenter(0.0, 0.0, |_, _| true);
            f.splash(0.3, -0.2, 0.3, 1.0);
            f.step(dt);
            let e0 = f.energy(dt);
            let mut max_e = e0;
            for _ in 0..(4.0 / dt) as usize { f.step(dt); max_e = max_e.max(f.energy(dt)); assert!(f.peak().is_finite()); }
            assert!(max_e < e0 * 1.25 + 1e-6, "dt {dt}: energy grew {e0:.4} -> {max_e:.4}");
        }
        let mut d = RippleField::new(64, 0.25, RippleParams::default());
        d.recenter(0.0, 0.0, |_, _| true);
        d.splash(0.0, 0.0, 0.3, 1.0);
        let p0 = d.peak();
        d.step(6.0);
        assert!(d.peak() < p0 * 0.1, "rings die down: peak {:.4} of {p0:.4}", d.peak());
    }

    #[test]
    fn a_bank_reflects_and_stays_dry() {
        // A wall of land 3 blocks east of the splash.
        let wall_x = 12; // cells of 0.25: x >= 3.0 blocks is land
        let mk = || {
            let mut f = RippleField::new(96, 0.25, quiet());
            f.recenter(0.0, 0.0, move |x, _| x < wall_x);
            f.splash(0.0, 0.0, 0.2, 0.75);
            f
        };
        let mut f = mk();
        let mut open = RippleField::new(96, 0.25, quiet());
        open.recenter(0.0, 0.0, |_, _| true);
        open.splash(0.0, 0.0, 0.2, 0.75);
        // The wave reaches the bank at 3 / c = 1.2 s and is back at the source at ~2.4 s.
        f.step(2.4);
        open.step(2.4);
        assert!(f.height(3.5, 0.0) == 0.0 && f.height(5.0, 1.0) == 0.0, "land stays dry");
        let (at_src_wall, at_src_open) = (f.height(0.8, 0.0).abs(), open.height(0.8, 0.0).abs());
        assert!(at_src_wall > at_src_open + 0.002, "the reflection comes back: {at_src_wall:.4} vs open water {at_src_open:.4}");
    }

    #[test]
    fn the_sponge_lets_waves_leave_the_patch() {
        let run = |sponge: usize| {
            let mut f = RippleField::new(48, 0.25, RippleParams { sponge, ..quiet() });
            f.recenter(0.0, 0.0, |_, _| true);
            f.splash(0.0, 0.0, 0.2, 0.75);
            f.step(8.0); // long enough to reach the edge (6 blocks / 2.5) and come back twice
            f.energy(1.0 / 60.0)
        };
        let (with, without) = (run(6), run(0));
        assert!(with < without * 0.3, "sponge keeps {with:.5} vs {without:.5} without: the edge absorbs");
    }

    #[test]
    fn recentring_keeps_waves_where_they_are_in_the_world() {
        let mut f = RippleField::new(64, 0.25, quiet());
        f.recenter(10.0, -4.0, |_, _| true);
        f.splash(11.0, -3.5, 0.2, 1.0);
        f.step(0.3);
        let probe = [(11.0f32, -3.5f32), (12.0, -3.0), (10.5, -4.0)];
        let before: Vec<f32> = probe.iter().map(|&(x, z)| f.height(x, z)).collect();
        f.recenter(12.3, -2.1, |_, _| true); // shift ~9 x 7 cells
        let after: Vec<f32> = probe.iter().map(|&(x, z)| f.height(x, z)).collect();
        for (b, a) in before.iter().zip(&after) { assert!((b - a).abs() < 1e-6, "{b} vs {a}"); }
        assert!(before[0].abs() > 0.01, "the probe is on the wave");
    }

    #[test]
    fn it_is_deterministic() {
        let run = || { let mut f = RippleField::new(48, 0.25, RippleParams::default()); f.recenter(0.0, 0.0, |x, z| (x + z) % 7 != 0); f.splash(0.4, 0.1, 0.3, 1.2); f.step(2.0); f.h.clone() };
        assert_eq!(run(), run());
    }
}
