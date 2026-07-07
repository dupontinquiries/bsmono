//! `sim::wind_field` — a calm, gusty wind field (the "wind is too fast" fix).
//!
//! Low-frequency seeded value noise gives slow, breathing gusts rather than
//! per-frame jitter. The default is deliberately calm; `calm_scale` rescales an
//! existing brisk wind speed toward that baseline for the bin to drop in.
//! Pure, deterministic (seeded), no wall-clock — time is a parameter.

#[derive(Clone, Copy, Debug)]
pub struct WindParams {
    pub base_speed: f32,
    pub gust_amp: f32,
    pub gust_hz: f32,
    pub dir_rad: f32,
    pub dir_wander: f32,
    pub seed: u64,
}

/// Calm baseline. Prior wind in `air`/`fabric` ran far brisker (it felt "too fast
/// all the time"); this default targets a gentle ~2 m/s breeze with slow gusts.
impl Default for WindParams {
    fn default() -> Self {
        WindParams {
            base_speed: 2.0,
            gust_amp: 1.2,
            gust_hz: 0.08, // slow: a gust cycle every ~12s
            dir_rad: 0.6,
            dir_wander: 0.35,
            seed: 0x5715_C0DE,
        }
    }
}

fn hash01(seed: u64, a: i64, b: i64) -> f32 {
    let mut x = seed
        ^ (a as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    (x >> 40) as f32 / (1u64 << 24) as f32 // [0,1)
}

#[inline]
fn smoothstep(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }

/// 2D value noise in [-1,1], smooth in both args.
fn vnoise(seed: u64, x: f32, y: f32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = smoothstep(x - x0);
    let fy = smoothstep(y - y0);
    let (ix, iy) = (x0 as i64, y0 as i64);
    let c00 = hash01(seed, ix, iy);
    let c10 = hash01(seed, ix + 1, iy);
    let c01 = hash01(seed, ix, iy + 1);
    let c11 = hash01(seed, ix + 1, iy + 1);
    let a = c00 + (c10 - c00) * fx;
    let b = c01 + (c11 - c01) * fx;
    (a + (b - a) * fy) * 2.0 - 1.0
}

/// Ground-plane wind vector (x,z) at a world position and time. Magnitude bounded
/// by base_speed + gust_amp; finite; varies slowly in time.
pub fn wind_at(p: &WindParams, world_x: f32, world_z: f32, t_seconds: f32) -> [f32; 2] {
    let base = p.base_speed.max(0.0);
    let amp = p.gust_amp.max(0.0);
    let hz = p.gust_hz.max(0.0);
    // gust: low-frequency in time, gentle spatial variation
    let g = vnoise(p.seed, world_x * 0.02 + t_seconds * hz, world_z * 0.02);
    let speed = (base + amp * g).clamp(0.0, base + amp);
    let dir = p.dir_rad + p.dir_wander * vnoise(p.seed ^ 0xABCD, t_seconds * hz * 0.5, 0.0);
    [dir.cos() * speed, dir.sin() * speed]
}

/// Foliage sway strength in [0,1]: normalized wind magnitude at a point/time.
pub fn sway01(p: &WindParams, world_x: f32, world_z: f32, t_seconds: f32) -> f32 {
    let w = wind_at(p, world_x, world_z, t_seconds);
    let mag = (w[0] * w[0] + w[1] * w[1]).sqrt();
    let denom = (p.base_speed.max(0.0) + p.gust_amp.max(0.0)).max(1e-3);
    (mag / denom).clamp(0.0, 1.0)
}

/// Rescale an existing (brisk) wind speed toward the calm baseline — a multiplier
/// in (0,1] (1 when the input is already ≤ the calm target).
pub fn calm_scale(old_speed: f32) -> f32 {
    const CALM_TARGET: f32 = 2.0;
    (CALM_TARGET / old_speed.max(0.01)).clamp(0.05, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mag(w: [f32; 2]) -> f32 { (w[0] * w[0] + w[1] * w[1]).sqrt() }

    #[test]
    fn magnitude_bounded_everywhere() {
        let p = WindParams::default();
        let cap = p.base_speed + p.gust_amp;
        for x in [-1e5, 0.0, 13.7, 1e5] {
            for z in [-1e5, 0.0, 4.2, 1e5] {
                for t in [0.0, 1.0, 1234.5, 1e6] {
                    let m = mag(wind_at(&p, x, z, t));
                    assert!(m.is_finite() && m <= cap + 1e-3, "m={m} cap={cap}");
                }
            }
        }
    }

    #[test]
    fn calmer_than_brisk_reference() {
        let p = WindParams::default();
        // a brisk reference wind would be ~8 m/s; our peak is base+gust = 3.2
        assert!(p.base_speed + p.gust_amp < 8.0);
    }

    #[test]
    fn low_frequency_in_time() {
        let p = WindParams::default();
        let a = wind_at(&p, 5.0, 5.0, 100.0);
        let b = wind_at(&p, 5.0, 5.0, 100.0 + 0.01); // tiny time step
        assert!((mag(a) - mag(b)).abs() < 0.05, "wind changes slowly, not jittery");
    }

    #[test]
    fn sway_in_unit() {
        let p = WindParams::default();
        for t in [0.0, 50.0, 999.0] {
            let s = sway01(&p, 2.0, 3.0, t);
            assert!((0.0..=1.0).contains(&s));
        }
    }

    #[test]
    fn calm_scale_reduces_brisk() {
        assert!(calm_scale(10.0) > 0.0 && calm_scale(10.0) < 1.0);
        assert!(calm_scale(1.0) <= 1.0);
        assert!(calm_scale(10.0) < calm_scale(4.0), "brisker -> smaller multiplier");
    }

    #[test]
    fn deterministic() {
        let p = WindParams::default();
        assert_eq!(wind_at(&p, 1.0, 2.0, 3.0), wind_at(&p, 1.0, 2.0, 3.0));
    }
}
