//! storm_field.rs — pure coarse weather-cell update.
//!
//! One explicit step over a low-resolution grid: humidity condenses into cloud
//! density, dense cloud precipitates out, electrical charge builds where it
//! rains and slowly dissipates, and all fields advect along a wind vector. This
//! is the CPU field the froxel volume will raymarch as volumetric cloud and that
//! the lightning system triggers from. Pure, deterministic, panic-free
//! (R1/R11/R21).
//!
//! Advection is first-order **donor-cell (upwind)**, which is conservative: what
//! leaves a cell enters its neighbour, so total airborne water can only fall
//! (boundary outflow), never spontaneously grow.

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StormParams {
    /// Humidity above which vapour condenses into cloud.
    pub saturation: f32,
    /// Fraction of the excess humidity converted to cloud per step.
    pub condense_rate: f32,
    /// Cloud density above which precipitation occurs.
    pub precip_thresh: f32,
    /// Fraction of the excess cloud removed (rained out) per step.
    pub precip_rate: f32,
    /// Charge gained per unit of precipitation activity.
    pub charge_build: f32,
    /// Fraction of charge lost per step.
    pub charge_dissip: f32,
    /// Metres per cell (for the CFL number).
    pub cell_size: f32,
    /// Seconds per step.
    pub dt: f32,
}

impl Default for StormParams {
    fn default() -> Self {
        StormParams {
            saturation: 0.6,
            condense_rate: 0.25,
            precip_thresh: 0.7,
            precip_rate: 0.15,
            charge_build: 0.5,
            charge_dissip: 0.05,
            cell_size: 64.0,
            dt: 1.0,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct StormField {
    pub w: usize,
    pub h: usize,
    pub humidity: Vec<f32>,
    pub cloud: Vec<f32>,
    pub charge: Vec<f32>,
    tmp: Vec<f32>,
}

impl StormField {
    pub fn new(w: usize, h: usize) -> Self {
        let n = w.max(1) * h.max(1);
        StormField {
            w: w.max(1),
            h: h.max(1),
            humidity: vec![0.0; n],
            cloud: vec![0.0; n],
            charge: vec![0.0; n],
            tmp: vec![0.0; n],
        }
    }

    #[inline]
    pub fn idx(&self, x: usize, z: usize) -> usize {
        z * self.w + x
    }

    /// Total airborne water (humidity + cloud). Used to check conservation.
    pub fn airborne_total(&self) -> f64 {
        let mut s = 0.0f64;
        for i in 0..self.humidity.len() {
            s += self.humidity[i] as f64 + self.cloud[i] as f64;
        }
        s
    }

    /// Advance one step under `wind` (m/s, world XZ).
    pub fn step(&mut self, p: &StormParams, wind: [f32; 2]) {
        let cell = if p.cell_size > 0.0 { p.cell_size } else { 1.0 };
        let cx = (wind[0] * p.dt / cell).clamp(-1.0, 1.0);
        let cz = (wind[1] * p.dt / cell).clamp(-1.0, 1.0);

        // 1) conservative advection of every field.
        Self::advect_x(&mut self.humidity, &mut self.tmp, self.w, self.h, cx);
        Self::advect_z(&mut self.humidity, &mut self.tmp, self.w, self.h, cz);
        Self::advect_x(&mut self.cloud, &mut self.tmp, self.w, self.h, cx);
        Self::advect_z(&mut self.cloud, &mut self.tmp, self.w, self.h, cz);
        Self::advect_x(&mut self.charge, &mut self.tmp, self.w, self.h, cx);
        Self::advect_z(&mut self.charge, &mut self.tmp, self.w, self.h, cz);

        // 2) condensation / precip / charge, per cell.
        for i in 0..self.humidity.len() {
            // humidity -> cloud (conserves humidity + cloud)
            let excess_h = (self.humidity[i] - p.saturation).max(0.0);
            let condensed = (p.condense_rate * excess_h).max(0.0);
            self.humidity[i] -= condensed;
            self.cloud[i] += condensed;

            // cloud -> precip (a sink)
            let excess_c = (self.cloud[i] - p.precip_thresh).max(0.0);
            let rained = (p.precip_rate * excess_c).max(0.0);
            self.cloud[i] -= rained;

            // charge builds on precip activity, then dissipates
            let c = self.charge[i] + p.charge_build * rained;
            self.charge[i] = (c * (1.0 - p.charge_dissip.clamp(0.0, 1.0))).clamp(0.0, 1.0);

            // keep airborne fields in [0,1]
            self.humidity[i] = self.humidity[i].clamp(0.0, 1.0);
            self.cloud[i] = self.cloud[i].clamp(0.0, 1.0);
        }
    }

    /// 1D upwind advection along x by signed CFL fraction `c` in [-1,1].
    /// Conservative except for outflow at the grid boundary.
    fn advect_x(field: &mut [f32], tmp: &mut [f32], w: usize, h: usize, c: f32) {
        let a = c.abs().min(1.0);
        if a == 0.0 {
            return;
        }
        tmp[..field.len()].copy_from_slice(field);
        for z in 0..h {
            for x in 0..w {
                let i = z * w + x;
                let upwind = if c > 0.0 {
                    if x > 0 { tmp[z * w + (x - 1)] } else { 0.0 }
                } else {
                    if x + 1 < w { tmp[z * w + (x + 1)] } else { 0.0 }
                };
                field[i] = tmp[i] * (1.0 - a) + upwind * a;
            }
        }
    }

    /// 1D upwind advection along z by signed CFL fraction `c` in [-1,1].
    fn advect_z(field: &mut [f32], tmp: &mut [f32], w: usize, h: usize, c: f32) {
        let a = c.abs().min(1.0);
        if a == 0.0 {
            return;
        }
        tmp[..field.len()].copy_from_slice(field);
        for z in 0..h {
            for x in 0..w {
                let i = z * w + x;
                let upwind = if c > 0.0 {
                    if z > 0 { tmp[(z - 1) * w + x] } else { 0.0 }
                } else {
                    if z + 1 < h { tmp[(z + 1) * w + x] } else { 0.0 }
                };
                field[i] = tmp[i] * (1.0 - a) + upwind * a;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    fn seeded_field(w: usize, h: usize, seed: &mut u32) -> StormField {
        let mut f = StormField::new(w, h);
        for i in 0..f.humidity.len() {
            f.humidity[i] = lcg(seed);
            f.cloud[i] = lcg(seed) * 0.5;
            f.charge[i] = lcg(seed) * 0.3;
        }
        f
    }

    #[test]
    fn prop_fields_bounded() {
        let mut seed = 0x5701_3322u32;
        let p = StormParams::default();
        for _ in 0..50 {
            let mut f = seeded_field(8, 6, &mut seed);
            let wind = [lcg(&mut seed) * 60.0 - 30.0, lcg(&mut seed) * 60.0 - 30.0];
            for _ in 0..10 {
                f.step(&p, wind);
            }
            for i in 0..f.humidity.len() {
                assert!((0.0..=1.0).contains(&f.humidity[i]), "humidity oob {}", f.humidity[i]);
                assert!((0.0..=1.0).contains(&f.cloud[i]), "cloud oob {}", f.cloud[i]);
                assert!((0.0..=1.0).contains(&f.charge[i]), "charge oob {}", f.charge[i]);
            }
        }
    }

    #[test]
    fn prop_no_spontaneous_airborne_gain() {
        // With no humidity sources, total (humidity + cloud) can only fall:
        // advection is conservative, condensation conserves, precip is a sink.
        let mut seed = 0xA0A0_1234u32;
        let p = StormParams::default();
        for _ in 0..50 {
            let mut f = seeded_field(10, 7, &mut seed);
            let wind = [lcg(&mut seed) * 50.0 - 25.0, lcg(&mut seed) * 50.0 - 25.0];
            for _ in 0..20 {
                let before = f.airborne_total();
                f.step(&p, wind);
                let after = f.airborne_total();
                assert!(after <= before + 1e-4, "airborne grew: {} -> {}", before, after);
            }
        }
    }

    #[test]
    fn calm_dry_input_is_steady_state() {
        // Humidity below saturation, no cloud/charge, no wind -> nothing changes.
        let p = StormParams::default();
        let mut f = StormField::new(6, 6);
        for v in f.humidity.iter_mut() {
            *v = p.saturation * 0.5; // below saturation
        }
        let before = f.clone();
        for _ in 0..25 {
            f.step(&p, [0.0, 0.0]);
        }
        assert_eq!(f.humidity, before.humidity, "humidity drifted at steady state");
        assert_eq!(f.cloud, before.cloud, "cloud appeared at steady state");
        for &c in &f.charge {
            assert!(c <= 1e-6, "charge appeared at steady state");
        }
    }

    #[test]
    fn precip_builds_then_charge_dissipates() {
        let p = StormParams::default();
        let mut f = StormField::new(3, 3);
        // saturate a cell so it condenses + rains -> charge appears.
        for v in f.humidity.iter_mut() {
            *v = 1.0;
        }
        for v in f.cloud.iter_mut() {
            *v = 1.0;
        }
        f.step(&p, [0.0, 0.0]);
        let charged: f32 = f.charge.iter().cloned().fold(0.0, f32::max);
        assert!(charged > 0.0, "no charge built from precip");
        // then let it dry out: charge should decay toward 0.
        for v in f.humidity.iter_mut() {
            *v = 0.0;
        }
        for v in f.cloud.iter_mut() {
            *v = 0.0;
        }
        for _ in 0..200 {
            f.step(&p, [0.0, 0.0]);
        }
        let residual: f32 = f.charge.iter().cloned().fold(0.0, f32::max);
        assert!(residual < charged, "charge did not dissipate");
    }

    #[test]
    fn prop_deterministic() {
        let mut s1 = 0xDEAD_0001u32;
        let mut s2 = 0xDEAD_0001u32;
        let p = StormParams::default();
        let mut a = seeded_field(8, 8, &mut s1);
        let mut b = seeded_field(8, 8, &mut s2);
        for _ in 0..15 {
            a.step(&p, [12.0, -7.0]);
            b.step(&p, [12.0, -7.0]);
        }
        assert_eq!(a.humidity, b.humidity);
        assert_eq!(a.cloud, b.cloud);
        assert_eq!(a.charge, b.charge);
    }
}
