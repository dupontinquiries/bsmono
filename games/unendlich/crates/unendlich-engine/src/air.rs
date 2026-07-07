//! Lightweight air-flow field for driving *visual* motion — hanging vines,
//! banners, foliage, drifting particles. This is deliberately NOT part of the
//! authoritative simulation: it's a cheap, sampleable, deterministic flow that
//! the renderer reads to push soft bodies around. No grid solve, no storage —
//! a base wind vector plus layered hash value-noise gives gusts and lazy swirls
//! as a pure function of position + time.
//!
//! Pairs with [`crate::fabric`] (the air velocity becomes the wind force on a
//! vine/cloth strand) and with `wind.rs`/weather (which supply the base wind).
//! All `f32`; deterministic for a given platform; unit-tested.

/// Spatial frequency of gusts — smaller = larger, lazier swirls.
pub const TURBULENCE_SCALE: f32 = 0.08;
/// How fast the gust pattern drifts through time.
pub const GUST_SPEED: f32 = 0.35;
/// Turbulence magnitude as a fraction of... itself (absolute units, blocks/sec
/// equivalent). The swirl never exceeds this in any axis.
pub const TURBULENCE_STRENGTH: f32 = 1.5;

#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Hash a lattice point to a pseudo-random scalar in `[0,1)`. Cheap, stable,
/// no allocation — the standard sin-fract trick with large irrational-ish
/// multipliers so adjacent cells decorrelate.
fn hash3(xi: f32, yi: f32, zi: f32) -> f32 {
    let h = xi * 127.1 + yi * 311.7 + zi * 74.7;
    fract(h.sin() * 43758.5453)
}

/// Trilinear-interpolated 3D value noise in `[0,1]`.
fn value_noise_3d(p: [f32; 3]) -> f32 {
    let (xf, yf, zf) = (p[0].floor(), p[1].floor(), p[2].floor());
    let (tx, ty, tz) = (p[0] - xf, p[1] - yf, p[2] - zf);
    // Smoothstep weights for C1 continuity (no faceting).
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);
    let sz = tz * tz * (3.0 - 2.0 * tz);

    let c = |dx: f32, dy: f32, dz: f32| hash3(xf + dx, yf + dy, zf + dz);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;

    let x00 = lerp(c(0.0, 0.0, 0.0), c(1.0, 0.0, 0.0), sx);
    let x10 = lerp(c(0.0, 1.0, 0.0), c(1.0, 1.0, 0.0), sx);
    let x01 = lerp(c(0.0, 0.0, 1.0), c(1.0, 0.0, 1.0), sx);
    let x11 = lerp(c(0.0, 1.0, 1.0), c(1.0, 1.0, 1.0), sx);
    let y0 = lerp(x00, x10, sy);
    let y1 = lerp(x01, x11, sy);
    lerp(y0, y1, sz)
}

/// A signed turbulence component in `[-1,1]`, offset per output axis so the
/// three components decorrelate (giving a swirl rather than three copies of
/// the same scalar pushing in lockstep).
fn turb(pos: [f32; 3], time: f32, axis_offset: f32) -> f32 {
    let s = TURBULENCE_SCALE;
    let p = [
        pos[0] * s + axis_offset,
        pos[1] * s + axis_offset * 0.5,
        pos[2] * s + time * GUST_SPEED,
    ];
    value_noise_3d(p) * 2.0 - 1.0
}

/// Air velocity at a world `pos` and `time`, given the prevailing `base_wind`.
/// The base wind dominates the average; turbulence adds a bounded swirl on top
/// so cloth and vines flutter instead of merely leaning. Deterministic.
pub fn air_velocity_at(pos: [f32; 3], time: f32, base_wind: [f32; 3]) -> [f32; 3] {
    [
        base_wind[0] + turb(pos, time, 0.0) * TURBULENCE_STRENGTH,
        // Vertical gusts are gentler — air mostly moves horizontally.
        base_wind[1] + turb(pos, time, 13.7) * TURBULENCE_STRENGTH * 0.35,
        base_wind[2] + turb(pos, time, 71.3) * TURBULENCE_STRENGTH,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn value_noise_in_unit_range() {
        for i in 0..50 {
            let f = i as f32 * 0.37;
            let v = value_noise_3d([f, f * 1.3, f * 0.7]);
            assert!((0.0..=1.0).contains(&v), "noise {v} out of [0,1]");
        }
    }

    #[test]
    fn deterministic_for_same_inputs() {
        let a = air_velocity_at([3.0, 4.0, 5.0], 1.25, [2.0, 0.0, 0.0]);
        let b = air_velocity_at([3.0, 4.0, 5.0], 1.25, [2.0, 0.0, 0.0]);
        assert_eq!(a, b, "same pos/time/wind → identical velocity");
    }

    #[test]
    fn turbulence_is_bounded() {
        // Sample a lattice; every component stays within base ± strength.
        for xi in 0..12 {
            for zi in 0..12 {
                let pos = [xi as f32 * 2.3, 6.0, zi as f32 * 1.7];
                let v = air_velocity_at(pos, xi as f32 * 0.1, [5.0, 0.0, -3.0]);
                assert!(v[0] >= 5.0 - TURBULENCE_STRENGTH - 1e-3);
                assert!(v[0] <= 5.0 + TURBULENCE_STRENGTH + 1e-3);
                assert!(v[1].abs() <= TURBULENCE_STRENGTH * 0.35 + 1e-3, "vertical gust gentle");
                assert!(v[2] >= -3.0 - TURBULENCE_STRENGTH - 1e-3);
                assert!(v[2] <= -3.0 + TURBULENCE_STRENGTH + 1e-3);
            }
        }
    }

    #[test]
    fn base_wind_dominates_the_average() {
        // Averaged over many cells, turbulence cancels and the mean ≈ base wind.
        let base = [4.0, 0.0, 2.0];
        let mut sum = [0.0f32; 3];
        let mut n = 0.0f32;
        for xi in 0..40 {
            for zi in 0..40 {
                let v = air_velocity_at([xi as f32, 6.0, zi as f32], 0.0, base);
                sum[0] += v[0];
                sum[2] += v[2];
                n += 1.0;
            }
        }
        assert!(approx(sum[0] / n, base[0], 0.25), "mean x ≈ base x");
        assert!(approx(sum[2] / n, base[2], 0.25), "mean z ≈ base z");
    }

    #[test]
    fn calm_air_still_has_gentle_motion() {
        // Zero base wind: the field isn't dead, but it's bounded.
        let v = air_velocity_at([10.0, 6.0, 10.0], 2.0, [0.0, 0.0, 0.0]);
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        assert!(speed <= TURBULENCE_STRENGTH * 2.5, "calm-air swirl stays small");
    }
}
