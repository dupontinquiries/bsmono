//! `sim::biome_climate` — deterministic climate→biome classification so the
//! world varies by region instead of one uniform terrain.
//!
//! Two continuous fields — temperature (°C) and moisture (`0..1`) — are derived
//! from seeded value-noise plus latitude (`z`) and elevation, then classified
//! into a biome. The result is the engine's existing
//! [`crate::world::biomes::Biome`] (not a forked palette) so it plugs straight
//! into the systems that already key off biomes: [`biome_id`] yields the `u8`
//! that `sim::mob_spawn_table::pick(biome: u8, …)` consumes, and the returned
//! `Biome` feeds `sim::feeding::graze_rate(Biome)` directly.
//!
//! ## Palette scope (cites `cave_biomes.docx` biome palette)
//! `world::biomes::Biome` carries 19 variants; this *surface* classifier targets
//! the 15 surface biomes and never emits the 3 cave biomes (`IceCave`,
//! `FungalCave`, `ThermalCave` — placed by the underground system, see
//! `Biome::is_cave_biome`) nor `Skylands` (floating-island biome placed by the
//! sky system). Surface targets: `Ocean, Beach, Desert, RedSandstoneCanyon,
//! BloomingCanyon, Plains, Swamp, TemperateForest, BirchForest, MapleForest,
//! CloudForest, TemperateHighlands, AlpineHighlands, MistyMountains, Tundra`.
//!
//! ## Model
//! - **Temperature** = equatorial base − latitude falloff(`|z|`) − lapse·elevation
//!   + small noise. Falls monotonically with elevation (noise is a function of
//!   `(x,z)` only, independent of elevation) and, over any meaningful Δ, with
//!   `|latitude|` (the latitude swing dominates the noise amplitude).
//! - **Moisture** = seeded fbm in `[0,1]`, lifted slightly at low elevation
//!   (coastal/lowland) and trimmed at altitude (rain shadow); always clamped.
//! - **Classify**: elevation gates Ocean/Beach and the highland/mountain tiers;
//!   the lowland band is a Whittaker-style temperature×moisture lookup.
//!
//! Pure + deterministic: data in, data out. Seeded `splitmix64`-style hashing
//! only (documented seeds below) — no `crate::render`, no `ash`/`vk`/GPU, no IO,
//! no clock, no unseeded RNG, no `.unwrap()`/`.expect()` in non-test code.
//! Compiles clean under `cargo check --release` (no `cfg`-gated helpers).

use crate::world::biomes::Biome;

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

/// Classify a climate + elevation into a surface [`Biome`]. Elevation gates
/// ocean/beach and the highland/mountain tiers; the lowland band is a
/// Whittaker-style temperature×moisture lookup. Never returns a cave biome or
/// `Skylands` (see module docs).
pub fn classify(c: Climate, elevation_m: f32) -> Biome {
    let elev = if elevation_m.is_finite() { elevation_m } else { 0.0 };

    // ── elevation gates ──
    if elev < SEA_LEVEL_M {
        return Biome::Ocean;
    }
    if elev < BEACH_MAX_M {
        return Biome::Beach;
    }
    if elev >= MISTY_M {
        return Biome::MistyMountains;
    }
    if elev >= ALPINE_M {
        return Biome::AlpineHighlands;
    }
    if elev >= HIGHLAND_M {
        return Biome::TemperateHighlands;
    }

    // ── lowland Whittaker band (3..45 m) ──
    let t = c.temperature_c;
    let m = c.moisture01;

    if t < 0.0 {
        // Anything below freezing is tundra regardless of moisture.
        return Biome::Tundra;
    }
    if t < 10.0 {
        // Cool.
        return if m > 0.6 {
            Biome::CloudForest
        } else if m < 0.3 {
            Biome::Tundra
        } else {
            Biome::Plains
        };
    }
    if t < 22.0 {
        // Temperate.
        return if m < 0.3 {
            Biome::Plains
        } else if m > 0.7 {
            Biome::Swamp
        } else {
            // moderate-moisture forests, split by temperature sub-band
            if t < 14.0 {
                Biome::BirchForest
            } else if t < 18.0 {
                Biome::TemperateForest
            } else {
                Biome::MapleForest
            }
        };
    }
    // Hot (>= 22 °C).
    if m < 0.25 {
        Biome::Desert
    } else if m < 0.6 {
        Biome::RedSandstoneCanyon
    } else {
        Biome::BloomingCanyon
    }
}

/// Stable `u8` id for spawn tables — equals the `Biome` discriminant
/// (`#[repr(u32)]`, all values `< 256`). This is the `biome` argument
/// `sim::mob_spawn_table::pick` takes.
pub fn biome_id(b: Biome) -> u8 {
    (b as u32) as u8
}

/// Convenience: classify directly from a world point.
pub fn biome_at(seed: u64, x: f32, z: f32, elevation_m: f32) -> Biome {
    classify(climate_at(seed, x, z, elevation_m), elevation_m)
}

/// Sample a `w × h` grid of [`Climate`] (row-major, `x` fastest) at
/// `(u0 + i·step, v0 + j·step)`. Sampled at sea level (`elevation_m = 0`): this
/// is the base climate field for a chunk; callers fold in per-voxel elevation
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

    const SEED: u64 = 0xDEAD_BEEF_1234_5678;

    fn lcg(s: &mut u32) -> u32 {
        *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        *s >> 8
    }
    fn randf(s: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(s) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }

    #[test]
    fn deterministic_climate_and_biome() {
        let mut s = 0x1111u32;
        for _ in 0..5000 {
            let x = randf(&mut s, -5000.0, 5000.0);
            let z = randf(&mut s, -5000.0, 5000.0);
            let e = randf(&mut s, -20.0, 220.0);
            let a = climate_at(SEED, x, z, e);
            let b = climate_at(SEED, x, z, e);
            assert_eq!(a, b, "climate must be deterministic");
            assert_eq!(classify(a, e), classify(b, e));
            assert_eq!(biome_at(SEED, x, z, e), biome_at(SEED, x, z, e));
        }
    }

    #[test]
    fn temperature_falls_with_elevation() {
        let mut s = 0x2222u32;
        for _ in 0..2000 {
            let x = randf(&mut s, -3000.0, 3000.0);
            let z = randf(&mut s, -3000.0, 3000.0);
            // noise is a function of (x,z) only, so raising elevation can only cool.
            let mut prev = climate_at(SEED, x, z, 0.0).temperature_c;
            let mut e = 0.0;
            while e <= 1000.0 {
                let t = climate_at(SEED, x, z, e).temperature_c;
                assert!(t <= prev + 1e-4, "temp must not rise with elevation");
                prev = t;
                e += 50.0;
            }
            // a large delta is strictly cooler
            let t0 = climate_at(SEED, x, z, 0.0).temperature_c;
            let t1 = climate_at(SEED, x, z, 1500.0).temperature_c;
            assert!(t1 < t0, "1500 m must be clearly cooler");
        }
    }

    #[test]
    fn temperature_falls_with_latitude() {
        let mut s = 0x3333u32;
        for _ in 0..2000 {
            let x = randf(&mut s, -3000.0, 3000.0);
            // latitude swing (≈90 °C across the range) dominates the ±5 °C noise.
            let equator = climate_at(SEED, x, 0.0, 0.0).temperature_c;
            let pole = climate_at(SEED, x, 4000.0, 0.0).temperature_c;
            let pole_neg = climate_at(SEED, x, -4000.0, 0.0).temperature_c;
            assert!(pole < equator, "high |latitude| must be cooler (+z)");
            assert!(pole_neg < equator, "high |latitude| must be cooler (−z)");
        }
    }

    #[test]
    fn moisture_in_range() {
        let mut s = 0x4444u32;
        for _ in 0..20000 {
            let x = randf(&mut s, -8000.0, 8000.0);
            let z = randf(&mut s, -8000.0, 8000.0);
            let e = randf(&mut s, -20.0, 220.0);
            let m = climate_at(SEED, x, z, e).moisture01;
            assert!(m >= 0.0 && m <= 1.0 && m.is_finite(), "moisture out of range: {}", m);
        }
    }

    #[test]
    fn ocean_only_at_low_elevation() {
        // classify never yields Ocean at or above sea level, for any climate.
        let mut s = 0x5555u32;
        for _ in 0..20000 {
            let c = Climate { temperature_c: randf(&mut s, -40.0, 45.0), moisture01: randf(&mut s, 0.0, 1.0) };
            let e = randf(&mut s, SEA_LEVEL_M, 250.0); // >= sea level
            assert_ne!(classify(c, e), Biome::Ocean, "no ocean above sea level");
        }
        // and below sea level it IS ocean
        assert_eq!(classify(Climate { temperature_c: 20.0, moisture01: 0.5 }, -1.0), Biome::Ocean);
    }

    #[test]
    fn never_emits_cave_or_sky_biomes() {
        let mut s = 0x6666u32;
        for _ in 0..50000 {
            let c = Climate { temperature_c: randf(&mut s, -60.0, 50.0), moisture01: randf(&mut s, 0.0, 1.0) };
            let e = randf(&mut s, -30.0, 300.0);
            let b = classify(c, e);
            assert!(!b.is_cave_biome(), "surface classifier emitted a cave biome");
            assert_ne!(b, Biome::Skylands, "surface classifier emitted Skylands");
        }
    }

    #[test]
    fn every_surface_biome_reachable() {
        use std::collections::HashSet;
        let mut seen: HashSet<u8> = HashSet::new();
        // sweep the classify input space directly to guarantee coverage
        let mut t = -40.0f32;
        while t <= 45.0 {
            let mut m = 0.0f32;
            while m <= 1.0 {
                let mut e = -10.0f32;
                while e <= 200.0 {
                    seen.insert(biome_id(classify(Climate { temperature_c: t, moisture01: m }, e)));
                    e += 5.0;
                }
                m += 0.05;
            }
            t += 1.0;
        }
        let want = [
            Biome::Ocean, Biome::Beach, Biome::Desert, Biome::RedSandstoneCanyon,
            Biome::BloomingCanyon, Biome::Plains, Biome::Swamp, Biome::TemperateForest,
            Biome::BirchForest, Biome::MapleForest, Biome::CloudForest,
            Biome::TemperateHighlands, Biome::AlpineHighlands, Biome::MistyMountains,
            Biome::Tundra,
        ];
        for b in want {
            assert!(seen.contains(&biome_id(b)), "surface biome {:?} unreachable", b);
        }
    }

    #[test]
    fn biome_boundaries_are_stable_along_a_line() {
        // Walk a long line in small steps; biome should change only a bounded
        // number of times (not flip erratically every sample).
        let step = 4.0f32;
        let n = 4000;
        let mut transitions = 0usize;
        let mut prev = biome_at(SEED, -8000.0, 137.0, 20.0);
        for i in 1..n {
            let x = -8000.0 + i as f32 * step;
            let b = biome_at(SEED, x, 137.0, 20.0);
            if b != prev {
                transitions += 1;
                prev = b;
            }
        }
        // 4000 samples over 16 km: regions, not noise. Generous bound.
        assert!(transitions < n / 8, "too many biome flips: {} in {}", transitions, n);
        assert!(transitions > 0, "a 16 km line should cross at least one biome boundary");
    }

    #[test]
    fn biome_id_matches_discriminant() {
        assert_eq!(biome_id(Biome::Plains), 0);
        assert_eq!(biome_id(Biome::Ocean), 11);
        assert_eq!(biome_id(Biome::ThermalCave), 18);
    }

    #[test]
    fn grid_helper_shape_and_consistency() {
        let g = climate_at_grid(SEED, 100.0, -50.0, 8, 5, 16.0);
        assert_eq!(g.len(), 8 * 5);
        // row-major, x fastest: entry (i,j) == climate_at at that point, elev 0
        let i = 3;
        let j = 2;
        let expect = climate_at(SEED, 100.0 + i as f32 * 16.0, -50.0 + j as f32 * 16.0, 0.0);
        assert_eq!(g[j * 8 + i], expect);
        // empty grid is fine
        assert!(climate_at_grid(SEED, 0.0, 0.0, 0, 0, 1.0).is_empty());
    }

    #[test]
    fn nan_inputs_do_not_panic() {
        let c = climate_at(SEED, f32::NAN, f32::INFINITY, f32::NAN);
        assert!(c.moisture01 >= 0.0 && c.moisture01 <= 1.0);
        let _ = classify(c, f32::NAN);
        let _ = classify(Climate { temperature_c: f32::NAN, moisture01: f32::NAN }, 50.0);
    }
}
