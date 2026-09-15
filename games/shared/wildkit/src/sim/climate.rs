//! climate — temperature and moisture at a world point.
//!
//! A SUBSET of `unendlich-engine/src/sim/biome_climate.rs`, and the only file
//! in this crate taken as a subset rather than whole. What came across:
//! [`Climate`], the two noise salts, [`climate_at`] and [`climate_at_grid`].
//! What did not: `classify`, `biome_id` and `biome_at`.
//!
//! ## Why the split, rather than extracting the module
//!
//! Those three return `world::biomes::Biome`, and following that dependency
//! honestly costs **1,581 lines** — a 19-variant biome palette plus the
//! block-id table underneath it. That is unendlich's WORLD CONTENT, not a
//! shared sensing primitive: clawbound has its own voxel palette and would
//! never name a `FungalCave`. Dragging it in to get a thermometer would make
//! this crate carry another game's block ids, which is the fork this whole
//! kit exists to avoid.
//!
//! The half that IS general is the one above the split: continuous
//! temperature and moisture from a seed and a position, with no opinion about
//! what to call the result. A game wanting seasons, drought, or "the wind is
//! colder up here" needs exactly that and none of the naming.
//!
//! ## The consequence, stated plainly
//!
//! This file will NOT diff cleanly against the engine. Every other module in
//! wildkit is byte-identical (`stamina` excepted, and its header says so);
//! this one is lines 1..160 and 245..257 of the original with the
//! biome-classification block removed and this header prepended. If the
//! engine's climate model changes, the change has to be carried by hand.
//!
//! That is a real cost and the reason it is written down here rather than
//! discovered later. It is accepted because the alternative — a second
//! climate model written from scratch for clawbound — drifts immediately and
//! silently, where this drifts visibly and only when someone edits the noise.


/// Continuous climate at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Climate {
    /// Temperature in degrees Celsius (unbounded; typically ~ −60..+35).
    pub temperature_c: f32,
    /// Moisture, normalised to `[0, 1]`.
    pub moisture01: f32,
}

// ── documented seeds (R20) ──────────────────────────────────────────────────
/// Salt mixed into the world seed for the temperature noise channel.
const SALT_TEMP: u64 = 0xC11A_7E00_7E11_0001;
/// Salt mixed into the world seed for the moisture noise channel.
const SALT_MOIST: u64 = 0xC11A_7E00_3015_0002;

// ── climate model constants ─────────────────────────────────────────────────
const T_EQUATOR_C: f32 = 30.0; // sea-level temperature at the equator (z=0)
const LAT_SCALE: f32 = 2000.0; // world-z units from equator to a temperature "pole"
const LAT_MAX: f32 = 4000.0; // |z| past which latitude cooling saturates
const TEMP_LAT_SWING_C: f32 = 45.0; // °C lost per LAT_SCALE of |latitude|
const LAPSE_PER_M: f32 = 0.0065; // °C lost per metre of elevation (~6.5°C/km)
const TEMP_NOISE_AMP_C: f32 = 5.0; // ± noise on temperature
const TEMP_FREQ: f32 = 1.0 / 900.0; // temperature-noise spatial frequency
const MOIST_FREQ: f32 = 1.0 / 700.0; // moisture-noise spatial frequency
const LOW_ELEV_MOIST_BONUS: f32 = 0.15; // extra moisture at sea level → 0 by HIGH_ELEV_M
const HIGH_ELEV_M: f32 = 160.0;

// ── elevation gates (metres) ────────────────────────────────────────────────
const SEA_LEVEL_M: f32 = 0.0;
const BEACH_MAX_M: f32 = 3.0;
const HIGHLAND_M: f32 = 45.0;
const ALPINE_M: f32 = 90.0;
const MISTY_M: f32 = 140.0;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

// ── seeded value-noise (local, documented) ──────────────────────────────────

/// splitmix64-style hash of a lattice cell → `[0, 1)`.
fn hash01(seed: u64, ix: i64, iz: i64) -> f32 {
    let mut z = seed
        ^ (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iz as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // top 24 bits → [0,1)
    ((z >> 40) as f32) / ((1u32 << 24) as f32)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Bilinear value-noise in `[0, 1]` over the integer lattice.
fn value_noise2(seed: u64, x: f32, z: f32) -> f32 {
    // Non-finite coordinates (NaN/±inf) would saturate the i64 lattice index and
    // overflow on `+ 1`; treat them as the origin so noise stays total + panic-free.
    let x = if x.is_finite() { x } else { 0.0 };
    let z = if z.is_finite() { z } else { 0.0 };
    let xf = x.floor();
    let zf = z.floor();
    let ix = xf as i64;
    let iz = zf as i64;
    let tx = smoothstep(x - xf);
    let tz = smoothstep(z - zf);
    let n00 = hash01(seed, ix, iz);
    let n10 = hash01(seed, ix + 1, iz);
    let n01 = hash01(seed, ix, iz + 1);
    let n11 = hash01(seed, ix + 1, iz + 1);
    let a = n00 + (n10 - n00) * tx;
    let b = n01 + (n11 - n01) * tx;
    a + (b - a) * tz
}

/// Fractional Brownian motion of [`value_noise2`], normalised to `[0, 1]`.
fn fbm2(seed: u64, x: f32, z: f32, octaves: u32) -> f32 {
    let mut value = 0.0;
    let mut amp = 1.0;
    let mut total = 0.0;
    let mut fx = x;
    let mut fz = z;
    let mut o = 0;
    while o < octaves {
        value += value_noise2(seed.wrapping_add(o as u64), fx, fz) * amp;
        total += amp;
        amp *= 0.5;
        fx *= 2.0;
        fz *= 2.0;
        o += 1;
    }
    if total > 0.0 {
        clamp01(value / total)
    } else {
        0.0
    }
}

/// Latitude cooling (°C lost) for a given world-`z`, saturating past [`LAT_MAX`].
fn latitude_cooling_c(z: f32) -> f32 {
    let lat = z.abs().min(LAT_MAX);
    (lat / LAT_SCALE) * TEMP_LAT_SWING_C
}

/// The climate at a world point. Deterministic in `(seed, x, z, elevation_m)`.
pub fn climate_at(seed: u64, x: f32, z: f32, elevation_m: f32) -> Climate {
    let elev = if elevation_m.is_finite() { elevation_m } else { 0.0 };
    let above_sea = elev.max(0.0);

    let tnoise = fbm2(seed ^ SALT_TEMP, x * TEMP_FREQ, z * TEMP_FREQ, 4);
    let temperature_c = T_EQUATOR_C - latitude_cooling_c(z) - LAPSE_PER_M * above_sea
        + (tnoise - 0.5) * 2.0 * TEMP_NOISE_AMP_C;

    let mnoise = fbm2(seed ^ SALT_MOIST, x * MOIST_FREQ, z * MOIST_FREQ, 4);
    let low_bonus = LOW_ELEV_MOIST_BONUS * (1.0 - clamp01(above_sea / HIGH_ELEV_M));
    let moisture01 = clamp01(mnoise + low_bonus);

    Climate { temperature_c, moisture01 }
}


/// via [`climate_at`]. Returns `w*h` entries.
pub fn climate_at_grid(seed: u64, u0: f32, v0: f32, w: usize, h: usize, step: f32) -> Vec<Climate> {
    let mut out = Vec::with_capacity(w.saturating_mul(h));
    for j in 0..h {
        for i in 0..w {
            let x = u0 + i as f32 * step;
            let z = v0 + j as f32 * step;
            out.push(climate_at(seed, x, z, 0.0));
        }
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The whole contract: same seed and place, same climate, forever. A
    /// world that re-rolls its weather on reload is not a world.
    #[test]
    fn climate_is_deterministic_in_seed_and_position() {
        let a = climate_at(0xC0FFEE, 120.5, -33.25, 80.0);
        let b = climate_at(0xC0FFEE, 120.5, -33.25, 80.0);
        assert_eq!(a, b);
        // A different seed gives a different world.
        let c = climate_at(0xBEEF, 120.5, -33.25, 80.0);
        assert!(a != c, "the seed has to matter");
    }

    /// Moisture is normalised; temperature is not, and the doc says so.
    /// Sampling widely is the cheap way to catch a noise change that pushes
    /// moisture out of range.
    #[test]
    fn moisture_stays_normalised_and_temperature_stays_plausible() {
        for i in 0..400 {
            let x = (i as f32) * 37.7 - 5000.0;
            let z = (i as f32) * -19.3 + 2000.0;
            let elev = (i % 40) as f32 * 60.0;
            let c = climate_at(0x5EED, x, z, elev);
            assert!((0.0..=1.0).contains(&c.moisture01),
                "moisture out of range at ({x},{z},{elev}): {}", c.moisture01);
            assert!(c.temperature_c.is_finite());
            assert!((-100.0..=80.0).contains(&c.temperature_c),
                "temperature implausible at ({x},{z},{elev}): {}", c.temperature_c);
        }
    }

    /// Height is the lever the brief cares about -- "colder up here" is what
    /// makes an outcrop feel different from the valley floor.
    #[test]
    fn altitude_cools() {
        let low = climate_at(0x5EED, 10.0, 10.0, 0.0);
        let high = climate_at(0x5EED, 10.0, 10.0, 2500.0);
        assert!(high.temperature_c < low.temperature_c,
            "2500m must be colder than sea level: {} vs {}", high.temperature_c, low.temperature_c);
    }

    /// The grid sampler must agree with point sampling, or a heat map and the
    /// world disagree about the same spot.
    #[test]
    fn the_grid_agrees_with_point_sampling() {
        let (seed, u0, v0, step) = (7u64, -20.0f32, 15.0f32, 5.0f32);
        let (w, h) = (4usize, 3usize);
        let grid = climate_at_grid(seed, u0, v0, w, h, step);
        assert_eq!(grid.len(), w * h);
        for j in 0..h {
            for i in 0..w {
                let want = climate_at(seed, u0 + i as f32 * step, v0 + j as f32 * step, 0.0);
                assert_eq!(grid[j * w + i], want, "grid cell ({i},{j}) disagrees");
            }
        }
    }

    /// A zero-sized grid is empty rather than a panic -- a viewport can be
    /// degenerate for a frame during a resize.
    #[test]
    fn a_degenerate_grid_is_empty() {
        assert!(climate_at_grid(1, 0.0, 0.0, 0, 5, 1.0).is_empty());
        assert!(climate_at_grid(1, 0.0, 0.0, 5, 0, 1.0).is_empty());
    }
}
