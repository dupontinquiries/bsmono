//! `sim::plant_propagation` — seed dispersal + germination (WID r54-02).
//!
//! Disperse seeds from a parent plant (by [`DispersalMode`], biased by wind),
//! then test whether each germinates given the local environment. Composes:
//! * `sim::wind_field` — the `wind_xy` passed to [`disperse`] is a wind vector
//!   from `wind_field::wind_at` (drift + spread scale with its magnitude).
//! * `sim::plant_growth_env` — [`germination_chance`] reuses that module's
//!   growth-suitability curve (`growth_multiplier`), so a seed germinates best
//!   exactly where the plant would grow best.
//!
//! Dispersal is seeded-deterministic (local splitmix64, documented salts) — no
//! real RNG. Pure: logic in, data out — no `crate::render`, no GPU, no I/O, no
//! clock, no `.unwrap()`/`.expect()` in non-test code. Release-safe (no
//! `#[cfg]`-gated helpers).

use crate::sim::plant_growth_env::{growth_multiplier, Conditions, Tolerance, MAX_MULT};

/// How a parent plant scatters its seeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispersalMode {
    /// Carried downwind; spread grows with wind speed.
    Wind,
    /// Wide, roughly uniform scatter (eaten and deposited far away).
    Animal,
    /// Drops near the parent in a tight cluster.
    Gravity,
    /// Flung outward in a radial ring.
    Explosive,
}

// ── dispersal tuning (documented; R20 seeds noise from `seed`) ──
const WIND_DRIFT_K: f32 = 0.3; // downwind centroid shift per unit wind speed
const WIND_BASE_SPREAD: f32 = 0.5;
const WIND_SPREAD_GAIN: f32 = 0.2; // extra spread per unit wind speed
const GRAVITY_SPREAD: f32 = 0.4; // tight
const ANIMAL_SPREAD: f32 = 5.0; // wide (>> gravity)
const EXPLOSIVE_R: f32 = 3.0; // ring radius
const EXPLOSIVE_JITTER: f32 = 0.25;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
const SALT_VIABLE: u64 = 0x5EED_11FE_0000_0054;

fn mix64(x: u64) -> u64 {
    // r342 dedup: bit-identical to the canonical finalizer (GOLDEN add) — delegate.
    crate::util::hash_util::mix64(x)
}

/// Deterministic uniform `[0,1)` for `(seed, k)`.
fn unit(seed: u64, k: u64) -> f32 {
    (mix64(seed ^ k.wrapping_mul(GOLDEN)) >> 40) as f32 / (1u64 << 24) as f32
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Scatter `n` seeds from `parent_xy`. Wind drift uses `wind_xy` (a vector from
/// `wind_field`). Deterministic in `seed`. Empty when `n == 0`.
pub fn disperse(parent_xy: [f32; 2], mode: DispersalMode, wind_xy: [f32; 2], seed: u64, n: usize) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(n);
    let (px, py) = (parent_xy[0], parent_xy[1]);
    let wind_speed = (wind_xy[0] * wind_xy[0] + wind_xy[1] * wind_xy[1]).sqrt();

    let mut i = 0usize;
    while i < n {
        // two decorrelated uniforms per seed
        let a = unit(seed, (2 * i) as u64);
        let b = unit(seed, (2 * i + 1) as u64);
        let ox = 2.0 * a - 1.0; // [-1,1)
        let oy = 2.0 * b - 1.0;

        let p = match mode {
            DispersalMode::Wind => {
                let spread = WIND_BASE_SPREAD + WIND_SPREAD_GAIN * wind_speed;
                [
                    px + wind_xy[0] * WIND_DRIFT_K + ox * spread,
                    py + wind_xy[1] * WIND_DRIFT_K + oy * spread,
                ]
            }
            DispersalMode::Gravity => [px + ox * GRAVITY_SPREAD, py + oy * GRAVITY_SPREAD],
            DispersalMode::Animal => [px + ox * ANIMAL_SPREAD, py + oy * ANIMAL_SPREAD],
            DispersalMode::Explosive => {
                let ang = 2.0 * std::f32::consts::PI * a;
                let r = EXPLOSIVE_R + (b - 0.5) * 2.0 * EXPLOSIVE_JITTER;
                [px + ang.cos() * r, py + ang.sin() * r]
            }
        };
        out.push(p);
        i += 1;
    }
    out
}

/// Germination probability `[0,1]` for the local conditions vs the plant's
/// tolerance — the growth-suitability curve normalised to `[0,1]` (≈1 at the
/// optimum, →0 outside the tolerance band). Reuses `plant_growth_env`.
pub fn germination_chance(c: &Conditions, tol: &Tolerance) -> f32 {
    clamp01(growth_multiplier(*c, *tol) / MAX_MULT)
}

/// Deterministic germination test: the seed germinates iff its seeded draw falls
/// under `chance01`. Monotone in `chance01` for a fixed `seed`
/// (`0.0 ⇒ never`, `1.0 ⇒ always`).
pub fn viable(seed: u64, chance01: f32) -> bool {
    unit(seed, SALT_VIABLE) < clamp01(chance01)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(moisture: f32, light: f32, temp: f32, season: f32) -> Conditions {
        Conditions { moisture01: moisture, light01: light, temp_c: temp, season01: season }
    }
    fn tol() -> Tolerance {
        Tolerance { temp_lo: 5.0, temp_hi: 35.0, temp_opt: 20.0, drought_resist01: 0.0 }
    }
    fn centroid(ps: &[[f32; 2]]) -> [f32; 2] {
        let n = ps.len() as f32;
        let mut s = [0.0f32, 0.0];
        for p in ps {
            s[0] += p[0];
            s[1] += p[1];
        }
        [s[0] / n, s[1] / n]
    }
    fn variance(ps: &[[f32; 2]], c: [f32; 2]) -> f32 {
        let n = ps.len() as f32;
        let mut v = 0.0f32;
        for p in ps {
            v += (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2);
        }
        v / n
    }

    #[test]
    fn wind_centroid_shifts_downwind_and_spreads_with_speed() {
        let parent = [0.0f32, 0.0];
        let calm = disperse(parent, DispersalMode::Wind, [0.0, 0.0], 1, 2000);
        let breezy = disperse(parent, DispersalMode::Wind, [10.0, 0.0], 1, 2000);
        let gale = disperse(parent, DispersalMode::Wind, [30.0, 0.0], 1, 2000);
        // centroid moves downwind (+x) with wind
        assert!(centroid(&breezy)[0] > centroid(&calm)[0] + 1.0, "centroid shifts downwind");
        // spread grows with wind speed
        let (cb, cg) = (centroid(&breezy), centroid(&gale));
        assert!(variance(&gale, cg) > variance(&breezy, cb), "spread grows with wind speed");
    }

    #[test]
    fn gravity_is_tighter_than_animal() {
        let parent = [3.0f32, -2.0];
        let g = disperse(parent, DispersalMode::Gravity, [0.0, 0.0], 7, 1500);
        let a = disperse(parent, DispersalMode::Animal, [0.0, 0.0], 7, 1500);
        assert!(variance(&g, centroid(&g)) < variance(&a, centroid(&a)), "gravity cluster tighter than animal scatter");
    }

    #[test]
    fn explosive_forms_a_ring() {
        let parent = [0.0f32, 0.0];
        let ring = disperse(parent, DispersalMode::Explosive, [0.0, 0.0], 5, 500);
        // every seed lands ~EXPLOSIVE_R from the parent (within the jitter band)
        for p in &ring {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - EXPLOSIVE_R).abs() <= EXPLOSIVE_JITTER + 1e-4, "radius {r} off the ring");
        }
    }

    #[test]
    fn disperse_deterministic_and_finite_and_count() {
        let p = [1.0f32, 2.0];
        assert_eq!(disperse(p, DispersalMode::Wind, [4.0, 1.0], 42, 0).len(), 0);
        let a = disperse(p, DispersalMode::Wind, [4.0, 1.0], 42, 64);
        let b = disperse(p, DispersalMode::Wind, [4.0, 1.0], 42, 64);
        assert_eq!(a, b, "deterministic");
        assert_eq!(a.len(), 64);
        assert!(a.iter().all(|q| q[0].is_finite() && q[1].is_finite()));
    }

    #[test]
    fn germination_higher_inside_tolerance_band() {
        let t = tol();
        // ideal: optimum temp, full light/moisture, peak season
        let good = germination_chance(&cond(1.0, 1.0, 20.0, 0.5), &t);
        // out of band: freezing
        let cold = germination_chance(&cond(1.0, 1.0, -10.0, 0.5), &t);
        // out of band: dark + dry
        let poor = germination_chance(&cond(0.0, 0.0, 20.0, 0.5), &t);
        assert!(good > 0.5, "good conditions germinate readily ({good})");
        assert!(cold < good && cold <= 1e-6, "freezing ⇒ ~0");
        assert!(poor < good, "dark+dry far less likely");
        assert!((0.0..=1.0).contains(&good) && (0.0..=1.0).contains(&cold) && (0.0..=1.0).contains(&poor));
    }

    #[test]
    fn viable_flips_at_threshold_deterministically() {
        let seed = 0xCAFE_1234u64;
        assert!(!viable(seed, 0.0), "chance 0 ⇒ never");
        assert!(viable(seed, 1.0), "chance 1 ⇒ always");
        // monotone in chance: once true, stays true as chance rises
        let mut crossed = false;
        let mut prev = false;
        for k in 0..=100 {
            let ch = k as f32 / 100.0;
            let v = viable(seed, ch);
            if prev {
                assert!(v, "viability is monotone in chance");
            }
            if v && !prev {
                crossed = true;
            }
            prev = v;
        }
        assert!(crossed, "there is a deterministic threshold where it flips");
        // deterministic
        assert_eq!(viable(seed, 0.5), viable(seed, 0.5));
    }
}
