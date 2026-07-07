//! `sim::survival_exposure` — make the environment *felt*. A thermoregulation /
//! exposure layer that FEEDS the existing [`crate::sim::survival`] model: it
//! turns ambient conditions (temperature, wind, wetness, shelter, fire,
//! clothing) into a body temperature + comfort, then bridges that into a
//! [`survival::EnvSample`] so `survival::tick` consumes it. It does **not** fork
//! survival — it produces the `ambient_warmth` that drives `Vitals::warmth`.
//!
//! Cite: `physical_mob_engine.docx` (exposure/thermoregulation);
//! `sim/survival.rs` (existing — `Vitals{hunger,thirst,warmth}`,
//! `EnvSample{ambient_warmth, ate, drank}`, `tick` eases `warmth` toward
//! `ambient_warmth` and raises the `cold` drive as warmth falls).
//!
//! Pure and deterministic: data in, data out. No render/GPU, IO, wall-clock, or
//! RNG; no `.unwrap()`/`.expect()`. Inputs sanitised; outputs finite/in-range.

use crate::sim::survival;

/// Ambient conditions at the creature this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conditions {
    /// Ambient air temperature in °C (from climate/day-night/weather — passed
    /// in, so this module is independent of the climate worker).
    pub ambient_c: f32,
    /// Wind strength `0..1` (more wind ⇒ colder felt temperature).
    pub wind01: f32,
    /// Wetness `0..1` (wet skin/fur ⇒ colder felt temperature).
    pub wet01: f32,
    /// Whether the creature is sheltered (cuts wind, adds a little warmth).
    pub sheltered: bool,
    /// Proximity to a heat source `0..1` (radiant fire warmth).
    pub near_fire01: f32,
    /// Clothing/insulation `0..1` (raises felt temp, blunts wind & wet).
    pub clothing01: f32,
}

/// Body thermal state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thermal {
    /// Body/skin temperature in °C — lags the felt ambient temperature.
    pub body_c: f32,
    /// Thermal comfort: `-1` freezing .. `0` comfortable .. `+1` overheating.
    pub comfort: f32,
}

// ── tunables (°C; tune here, never in the formulas) ─────────────────────────
/// Felt-temperature drop at full wind, exposed, no insulation.
const WIND_CHILL_C: f32 = 12.0;
/// Felt-temperature drop at full wetness.
const WET_CHILL_C: f32 = 8.0;
/// Felt-temperature gain next to a strong fire.
const FIRE_WARM_C: f32 = 20.0;
/// Felt-temperature gain from full clothing/insulation.
const CLOTHING_WARM_C: f32 = 8.0;
/// Felt-temperature gain from being sheltered.
const SHELTER_WARM_C: f32 = 4.0;
/// Fraction of wind chill that still applies when sheltered.
const SHELTER_WIND_FACTOR: f32 = 0.3;
/// How fast body temperature eases toward the felt temperature (per second).
const BODY_EASE_PER_SEC: f32 = 0.05;

/// Comfortable body-temperature band (°C) and the extremes that saturate comfort.
const COMFORT_LO_C: f32 = 18.0;
const COMFORT_HI_C: f32 = 26.0;
const COLD_EXTREME_C: f32 = 0.0;
const HOT_EXTREME_C: f32 = 40.0;

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}
#[inline]
fn clamp01(x: f32) -> f32 {
    san(x).max(0.0).min(1.0)
}

/// Felt ("apparent") temperature in °C: wind-chill + wet cooling lower it; fire,
/// clothing, and shelter raise it. Monotone — colder in `wind01`/`wet01`, warmer
/// in `near_fire01`/`clothing01`/`sheltered`.
pub fn felt_temperature_c(c: &Conditions) -> f32 {
    let ambient = san(c.ambient_c);
    let wind = clamp01(c.wind01);
    let wet = clamp01(c.wet01);
    let fire = clamp01(c.near_fire01);
    let clothing = clamp01(c.clothing01);

    // Wind chill: insulation blunts it, shelter cuts most of it.
    let wind_mult = if c.sheltered { SHELTER_WIND_FACTOR } else { 1.0 };
    let wind_chill = WIND_CHILL_C * wind * (1.0 - clothing) * wind_mult;
    // Wet cooling: clothing halves it at most.
    let wet_chill = WET_CHILL_C * wet * (1.0 - 0.5 * clothing);

    let shelter_warm = if c.sheltered { SHELTER_WARM_C } else { 0.0 };

    ambient - wind_chill - wet_chill
        + FIRE_WARM_C * fire
        + CLOTHING_WARM_C * clothing
        + shelter_warm
}

/// Thermal comfort `[-1, 1]` from a body temperature: `0` inside the comfort
/// band, falling to `-1` at/below `COLD_EXTREME_C`, rising to `+1` at/above
/// `HOT_EXTREME_C`.
fn comfort_from_body(body_c: f32) -> f32 {
    let b = san(body_c);
    if b < COMFORT_LO_C {
        -((COMFORT_LO_C - b) / (COMFORT_LO_C - COLD_EXTREME_C)).max(0.0).min(1.0)
    } else if b > COMFORT_HI_C {
        ((b - COMFORT_HI_C) / (HOT_EXTREME_C - COMFORT_HI_C)).max(0.0).min(1.0)
    } else {
        0.0
    }
}

/// Ease body temperature toward the felt temperature at a bounded rate (never
/// teleports), and recompute comfort. `dt <= 0` / non-finite leaves the body
/// temperature unchanged.
pub fn step_body(prev: Thermal, c: &Conditions, dt: f32) -> Thermal {
    let felt = felt_temperature_c(c);
    let prev_body = san(prev.body_c);
    let step = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
    // ease fraction in [0,1] so a huge dt reaches felt but never overshoots.
    let ease = (BODY_EASE_PER_SEC * step).max(0.0).min(1.0);
    let body_c = prev_body + (felt - prev_body) * ease;
    let body_c = if body_c.is_finite() { body_c } else { felt };
    Thermal { body_c, comfort: comfort_from_body(body_c) }
}

/// Bridge the thermal state into a [`survival::EnvSample`]: map comfort to the
/// `ambient_warmth` warmth-target that `survival::tick` eases `Vitals::warmth`
/// toward (cold ⇒ low warmth ⇒ rising `cold` drive; hot ⇒ high warmth — this
/// model has no heat penalty). `ate`/`drank` are carried through from `base`.
pub fn to_env_sample(th: &Thermal, base: survival::EnvSample) -> survival::EnvSample {
    let comfort = san(th.comfort).max(-1.0).min(1.0);
    // neutral comfort -> a comfortable 0.7 warmth target; full cold -> 0.0;
    // full hot -> 1.0. Monotone increasing in comfort.
    let warmth = if comfort >= 0.0 {
        0.7 + 0.3 * comfort
    } else {
        0.7 + 0.7 * comfort
    };
    survival::EnvSample {
        ambient_warmth: warmth.max(0.0).min(1.0),
        ate: base.ate,
        drank: base.drank,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(ambient: f32, wind: f32, wet: f32, sheltered: bool, fire: f32, clothing: f32) -> Conditions {
        Conditions { ambient_c: ambient, wind01: wind, wet01: wet, sheltered, near_fire01: fire, clothing01: clothing }
    }

    #[test]
    fn wind_and_wet_lower_fire_clothing_shelter_raise() {
        let base = cond(15.0, 0.0, 0.0, false, 0.0, 0.0);
        let f0 = felt_temperature_c(&base);
        // wind lowers
        assert!(felt_temperature_c(&cond(15.0, 1.0, 0.0, false, 0.0, 0.0)) < f0);
        // wet lowers
        assert!(felt_temperature_c(&cond(15.0, 0.0, 1.0, false, 0.0, 0.0)) < f0);
        // fire raises
        assert!(felt_temperature_c(&cond(15.0, 0.0, 0.0, false, 1.0, 0.0)) > f0);
        // clothing raises
        assert!(felt_temperature_c(&cond(15.0, 0.0, 0.0, false, 0.0, 1.0)) > f0);
        // shelter raises
        assert!(felt_temperature_c(&cond(15.0, 0.0, 0.0, true, 0.0, 0.0)) > f0);
        // monotonic in wind (more wind, colder)
        let mut prev = f32::INFINITY;
        let mut w = 0.0;
        while w <= 1.0 {
            let cur = felt_temperature_c(&cond(15.0, w, 0.0, false, 0.0, 0.0));
            assert!(cur <= prev + 1e-5, "wind not monotone");
            prev = cur;
            w += 0.1;
        }
        // clothing also blunts wind chill (clothed windy >= unclothed windy)
        let windy_bare = felt_temperature_c(&cond(15.0, 1.0, 0.0, false, 0.0, 0.0));
        let windy_clothed = felt_temperature_c(&cond(15.0, 1.0, 0.0, false, 0.0, 1.0));
        assert!(windy_clothed > windy_bare);
        assert!(f0.is_finite());
    }

    #[test]
    fn body_eases_toward_felt_without_teleport() {
        let c = cond(-10.0, 0.5, 0.0, false, 0.0, 0.0); // cold
        let felt = felt_temperature_c(&c);
        let start = Thermal { body_c: 22.0, comfort: 0.0 };
        // small dt: moves toward felt but only a fraction (no teleport)
        let one = step_body(start, &c, 1.0);
        assert!(one.body_c < start.body_c, "should cool toward felt");
        assert!(one.body_c > felt, "must not teleport past felt in 1s");
        assert!((one.body_c - start.body_c).abs() <= (felt - start.body_c).abs() + 1e-4);
        // dt = 0 leaves body unchanged
        assert_eq!(step_body(start, &c, 0.0).body_c, start.body_c);
        assert_eq!(step_body(start, &c, f32::NAN).body_c, start.body_c);
        // huge dt approaches felt but never overshoots
        let big = step_body(start, &c, 1.0e6);
        assert!((big.body_c - felt).abs() < 1e-2);
        // monotone approach over many ticks, never crossing felt
        let mut th = start;
        for _ in 0..2000 {
            let next = step_body(th, &c, 1.0);
            assert!(next.body_c >= felt - 1e-3 && next.body_c <= th.body_c + 1e-4);
            th = next;
        }
    }

    #[test]
    fn comfort_range_and_extremes() {
        // comfortable band -> ~0
        assert_eq!(step_body(Thermal { body_c: 22.0, comfort: 0.0 }, &cond(22.0, 0.0, 0.0, false, 0.0, 0.0), 1.0e6).comfort.abs() < 0.2, true);
        // extreme cold -> -1
        let cold = step_body(Thermal { body_c: 22.0, comfort: 0.0 }, &cond(-40.0, 1.0, 1.0, false, 0.0, 0.0), 1.0e6);
        assert!((cold.comfort + 1.0).abs() < 1e-3, "cold comfort {}", cold.comfort);
        // extreme hot -> +1
        let hot = step_body(Thermal { body_c: 22.0, comfort: 0.0 }, &cond(60.0, 0.0, 0.0, false, 1.0, 0.0), 1.0e6);
        assert!((hot.comfort - 1.0).abs() < 1e-3, "hot comfort {}", hot.comfort);
        // always in [-1,1]
        for amb in [-50.0f32, -10.0, 5.0, 22.0, 35.0, 80.0] {
            let th = step_body(Thermal { body_c: 0.0, comfort: 0.0 }, &cond(amb, 0.0, 0.0, false, 0.0, 0.0), 1.0e6);
            assert!((-1.0..=1.0).contains(&th.comfort));
        }
    }

    // ── bridge + survival::tick: cold drives vitals stress up ────────────────
    #[test]
    fn cold_pushes_vitals_colder_than_warm_via_survival_tick() {
        let warm_c = cond(24.0, 0.0, 0.0, true, 0.3, 0.5);   // pleasant
        let cold_c = cond(-15.0, 0.9, 0.6, false, 0.0, 0.0); // brutal

        let run = |c: &Conditions| -> (f32, f32) {
            let mut th = Thermal { body_c: 22.0, comfort: 0.0 };
            let mut v = survival::Vitals::comfortable();
            let base = survival::EnvSample { ambient_warmth: 0.0, ate: false, drank: false };
            let mut drives = survival::Drives { hunger: 0.0, thirst: 0.0, cold: 0.0 };
            for _ in 0..600 {
                th = step_body(th, c, 1.0);
                let env = to_env_sample(&th, base);
                let (nv, nd) = survival::tick(v, env, 1.0);
                v = nv;
                drives = nd;
            }
            (v.warmth, drives.cold)
        };

        let (warm_warmth, warm_cold) = run(&warm_c);
        let (cold_warmth, cold_cold) = run(&cold_c);
        // cold conditions leave the body colder (lower warmth vital) and a
        // higher cold drive than warm conditions.
        assert!(cold_warmth < warm_warmth, "cold {} !< warm {}", cold_warmth, warm_warmth);
        assert!(cold_cold >= warm_cold, "cold drive {} !>= warm drive {}", cold_cold, warm_cold);
        assert!(cold_cold > 0.0, "brutal cold should raise the cold drive");
    }

    #[test]
    fn bridge_preserves_ate_drank_and_warmth_monotone() {
        let base = survival::EnvSample { ambient_warmth: 0.5, ate: true, drank: false };
        let e = to_env_sample(&Thermal { body_c: 22.0, comfort: 0.0 }, base);
        assert!(e.ate && !e.drank);
        // warmth target rises with comfort
        let cold = to_env_sample(&Thermal { body_c: 0.0, comfort: -1.0 }, base).ambient_warmth;
        let neutral = to_env_sample(&Thermal { body_c: 22.0, comfort: 0.0 }, base).ambient_warmth;
        let hot = to_env_sample(&Thermal { body_c: 40.0, comfort: 1.0 }, base).ambient_warmth;
        assert!(cold < neutral && neutral < hot);
        assert!((0.0..=1.0).contains(&cold) && (0.0..=1.0).contains(&hot));
    }

    #[test]
    fn deterministic_and_seeded_sweep() {
        let c = cond(5.0, 0.4, 0.2, false, 0.1, 0.3);
        let a = step_body(Thermal { body_c: 20.0, comfort: 0.0 }, &c, 0.5);
        let b = step_body(Thermal { body_c: 20.0, comfort: 0.0 }, &c, 0.5);
        assert_eq!(a, b);

        // deterministic seeded sweep (R20): all outputs finite/in-range
        let mut x: u64 = 0xE7E0_0078;
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let span = |v: u32, lo: f32, hi: f32| lo + (v % 1000) as f32 / 1000.0 * (hi - lo);
        let mut th = Thermal { body_c: 20.0, comfort: 0.0 };
        for _ in 0..5000 {
            let c = cond(span(next(), -60.0, 60.0), span(next(), -0.2, 1.2), span(next(), -0.2, 1.2),
                         next() % 2 == 0, span(next(), -0.2, 1.2), span(next(), -0.2, 1.2));
            assert!(felt_temperature_c(&c).is_finite());
            th = step_body(th, &c, span(next(), -0.1, 5.0));
            assert!(th.body_c.is_finite());
            assert!((-1.0..=1.0).contains(&th.comfort));
            let e = to_env_sample(&th, survival::EnvSample { ambient_warmth: 0.0, ate: false, drank: false });
            assert!((0.0..=1.0).contains(&e.ambient_warmth));
        }
    }
}
