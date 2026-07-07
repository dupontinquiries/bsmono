//! Weather — the atmospheric layer that sits on top of [`crate::sky::Sky`].
//!
//! `Sky` owns the diurnal cycle (sun direction, day/night sky color, ambient).
//! `Weather` owns the *conditions*: how cloudy, how wet, how foggy, how windy.
//! Neither renders anything; together they produce [`AtmosphereUniforms`] — the
//! single struct the cinematic render path (clear color, fog, directional sun,
//! cloud/rain shaders) is meant to consume. Keeping it all CPU-side and pure
//! means the look is unit-testable before a line of shader exists.
//!
//! ## Design
//!
//! Weather is a *deterministic function of time + seed*, not a random walk.
//! Time is divided into epochs of `EPOCH_SECONDS`; each epoch hashes to a
//! [`Condition`], and the live parameters cross-fade from the previous epoch's
//! condition to the current one over the first `TRANSITION_FRAC` of the epoch
//! (smoothstepped, so no visible kink). Because it's a pure function of
//! `time_seconds`, a snapshot is just the time + seed, and replay is exact.
//!
//! ## Rollback note
//!
//! Like `Sky`, weather currently advances on wall-clock `real_dt` from the bin,
//! so it is not bit-identical across machines with different frame timing. That
//! is fine for a purely *visual* consumer. The moment a simulation consumer
//! appears (rain wetting the ground, storms scaring mobs), switch `tick` to
//! advance on `TickId × period` instead — the param math here doesn't change.

use crate::sky::Sky;

/// Real seconds per weather epoch. One condition holds (after its fade-in) for
/// the rest of the epoch. 180s ≈ a few minutes of, say, rain before it clears.
pub const EPOCH_SECONDS: f32 = 180.0;

/// Fraction of an epoch spent cross-fading from the previous condition to the
/// current one. 0.25 → a ~45s transition at the default epoch length, slow
/// enough to read as weather rolling in rather than snapping.
pub const TRANSITION_FRAC: f32 = 0.25;

/// Named weather conditions. The live state is continuous (see
/// [`WeatherParams`]); these are just the keyframes it interpolates between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Clear,
    Cloudy,
    Overcast,
    Rain,
    Storm,
    Fog,
}

impl Condition {
    /// The continuous parameters this condition settles to.
    pub fn params(self) -> WeatherParams {
        // cloudiness, precipitation, fog, wind — all in [0, 1].
        let (cloudiness, precipitation, fog, wind) = match self {
            Condition::Clear => (0.00, 0.00, 0.02, 0.10),
            Condition::Cloudy => (0.45, 0.00, 0.08, 0.30),
            Condition::Overcast => (0.85, 0.00, 0.18, 0.40),
            Condition::Rain => (0.80, 0.60, 0.35, 0.55),
            Condition::Storm => (0.97, 0.95, 0.45, 0.90),
            Condition::Fog => (0.40, 0.00, 0.90, 0.05),
        };
        WeatherParams { cloudiness, precipitation, fog, wind }
    }

    /// Deterministic per-epoch selection. Clear/cloudy dominate; storms are
    /// rare. The weights are a small fixed table indexed by a hashed epoch.
    fn for_epoch(epoch: u64, seed: u64) -> Condition {
        // Weighted bag (20 slots). Clear is most common, Storm least.
        const BAG: [Condition; 20] = {
            use Condition::*;
            [
                Clear, Clear, Clear, Clear, Clear, Clear, Clear, // 7 Clear
                Cloudy, Cloudy, Cloudy, Cloudy, // 4 Cloudy
                Overcast, Overcast, Overcast, // 3 Overcast
                Rain, Rain, Rain, // 3 Rain
                Fog, Fog, // 2 Fog
                Storm, // 1 Storm
            ]
        };
        let h = hash2(epoch, seed);
        BAG[(h % BAG.len() as u64) as usize]
    }
}

/// Continuous atmospheric parameters, each in `[0, 1]`. The render path reads
/// THESE (never the discrete [`Condition`]) so weather changes are smooth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherParams {
    /// 0 = clear sky, 1 = full overcast. Greys the sky and dims the sun.
    pub cloudiness: f32,
    /// 0 = dry, 1 = heavy rain. Drives the rain shader and darkens light.
    pub precipitation: f32,
    /// 0 = clear air, 1 = thick fog. Drives fog density directly.
    pub fog: f32,
    /// 0 = still, 1 = gale. Drives cloud/rain motion + foliage sway in shader.
    pub wind: f32,
}

impl WeatherParams {
    pub const CLEAR: WeatherParams =
        WeatherParams { cloudiness: 0.0, precipitation: 0.0, fog: 0.0, wind: 0.0 };

    fn lerp(a: WeatherParams, b: WeatherParams, t: f32) -> WeatherParams {
        let l = |x: f32, y: f32| x + (y - x) * t;
        WeatherParams {
            cloudiness: l(a.cloudiness, b.cloudiness),
            precipitation: l(a.precipitation, b.precipitation),
            fog: l(a.fog, b.fog),
            wind: l(a.wind, b.wind),
        }
    }

    /// Multiplier on the sun's directional contribution. Clouds block most of
    /// it; rain a little more. Clear → 1.0, storm → ~0.15. Never fully zero so
    /// daytime keeps some shape even under storm.
    pub fn sun_dimming(self) -> f32 {
        (1.0 - 0.6 * self.cloudiness - 0.3 * self.precipitation).clamp(0.12, 1.0)
    }

    /// Total view-distance fog in `[0, 1]`. Fog condition dominates; rain and
    /// heavy cloud add a haze floor.
    pub fn fog_density(self) -> f32 {
        crate::util::mathx::clampf(self.fog + 0.30 * self.precipitation + 0.12 * self.cloudiness, 0.0, 1.0)
    }

    /// Per-channel light tint multiplier. Clear light is warm white; overcast
    /// and rain pull it toward a cool, desaturated grey-blue.
    pub fn light_tint(self) -> [f32; 3] {
        let warm = [1.00_f32, 0.98, 0.92];
        let cool = [0.66_f32, 0.70, 0.78];
        let t = crate::util::mathx::clampf(self.cloudiness.max(self.precipitation), 0.0, 1.0);
        [
            crate::util::mathx::lerp(warm[0], cool[0], t),
            crate::util::mathx::lerp(warm[1], cool[1], t),
            crate::util::mathx::lerp(warm[2], cool[2], t),
        ]
    }

    /// How much to grey/desaturate the sky toward overcast. Mostly cloudiness,
    /// with fog adding a little wash-out.
    pub fn sky_grey(self) -> f32 {
        crate::util::mathx::clampf(self.cloudiness * 0.85 + self.fog * 0.3, 0.0, 1.0)
    }
}

/// Weather resource. Cheap to clone; advances on real time like [`Sky`].
#[derive(Debug, Clone, Copy)]
pub struct Weather {
    /// World time in seconds (independent accumulator so weather and sky can
    /// run at different scales if desired).
    pub time_seconds: f64,
    /// Per-world seed; different seeds give different weather sequences.
    pub seed: u64,
    /// Real-time → weather-time scale (1.0 = real time).
    pub time_scale: f32,
}

impl Default for Weather {
    fn default() -> Self {
        Self { time_seconds: 0.0, seed: 0x5eed_1234_abcd_0001, time_scale: 1.0 }
    }
}

impl Weather {
    pub fn new(seed: u64) -> Self {
        Self { seed, ..Self::default() }
    }

    /// Advance weather time by `real_dt` real seconds.
    pub fn tick(&mut self, real_dt: f32) {
        self.time_seconds += (real_dt * self.time_scale) as f64;
    }

    /// Live continuous parameters at the current time — a pure function of
    /// `time_seconds` + `seed`, cross-faded across the epoch boundary.
    pub fn params(self) -> WeatherParams {
        let t = self.time_seconds.max(0.0) as f32;
        let epoch = (t / EPOCH_SECONDS) as u64;
        let frac = (t / EPOCH_SECONDS) - epoch as f32; // [0,1) within epoch

        let current = Condition::for_epoch(epoch, self.seed).params();
        if frac >= TRANSITION_FRAC || epoch == 0 {
            // Past the fade-in (or the very first epoch, which has no
            // predecessor to fade from): hold the current condition.
            return current;
        }
        // Cross-fade from the previous epoch's condition.
        let prev = Condition::for_epoch(epoch - 1, self.seed).params();
        let s = smoothstep(frac / TRANSITION_FRAC);
        WeatherParams::lerp(prev, current, s)
    }

    /// The discrete condition the current epoch settles to (for UI/debug; the
    /// render path should prefer [`Weather::params`]).
    pub fn condition(self) -> Condition {
        let epoch = (self.time_seconds.max(0.0) as f32 / EPOCH_SECONDS) as u64;
        Condition::for_epoch(epoch, self.seed)
    }
}

/// The composed atmosphere — everything the cinematic render path needs in one
/// struct. Produced by [`compose`] from a `Sky` (time of day) and the live
/// `WeatherParams`. This is the intended CPU→shader uniform payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtmosphereUniforms {
    /// Unit vector: direction sunlight travels (from `Sky`).
    pub sun_direction: [f32; 3],
    /// Directional sun strength after weather dimming, in `[0, 1]`.
    pub sun_intensity: f32,
    /// Clear color / horizon, greyed toward overcast by cloud cover. RGBA.
    pub sky_color: [f32; 4],
    /// Fog color (matches the greyed horizon so geometry fades into sky). RGB.
    pub fog_color: [f32; 3],
    /// Fog thickness in `[0, 1]`.
    pub fog_density: f32,
    /// Ambient light color after weather tint. RGB.
    pub ambient_color: [f32; 3],
    /// Wind strength in `[0, 1]` (cloud/rain/foliage motion in shader).
    pub wind: f32,
}

/// Overcast grey the sky and fog blend toward as cloud cover rises.
const OVERCAST_GREY: [f32; 3] = [0.70, 0.72, 0.75];

/// Fold time-of-day (`sky`) and `weather` into one uniform payload. Pure.
pub fn compose(sky: &Sky, weather: WeatherParams) -> AtmosphereUniforms {
    let grey = weather.sky_grey();

    // Sky color greyed toward overcast. At night the sky is already dark, so
    // greying toward a bright grey would wrongly brighten it — scale the grey
    // target by the sky's own luminance so night stays dark and overcast.
    let base_sky = sky.sky_color_rgba();
    let sky_lum = (base_sky[0] + base_sky[1] + base_sky[2]) / 3.0;
    let grey_target = [
        OVERCAST_GREY[0] * (0.35 + 0.65 * sky_lum),
        OVERCAST_GREY[1] * (0.35 + 0.65 * sky_lum),
        OVERCAST_GREY[2] * (0.35 + 0.65 * sky_lum),
    ];
    let sky_color = [
        base_sky[0] + (grey_target[0] - base_sky[0]) * grey,
        base_sky[1] + (grey_target[1] - base_sky[1]) * grey,
        base_sky[2] + (grey_target[2] - base_sky[2]) * grey,
        1.0,
    ];

    // Fog takes the horizon color so distant geometry dissolves into the sky.
    let fog_color = [sky_color[0], sky_color[1], sky_color[2]];

    // Ambient from sky, tinted by weather.
    let amb = sky.ambient_color();
    let tint = weather.light_tint();
    let ambient_color = [amb[0] * tint[0], amb[1] * tint[1], amb[2] * tint[2]];

    AtmosphereUniforms {
        sun_direction: sky.sun_direction(),
        sun_intensity: crate::util::mathx::clampf(sky.directional_intensity() * weather.sun_dimming(), 0.0, 1.0),
        sky_color,
        fog_color,
        fog_density: weather.fog_density(),
        ambient_color,
        wind: weather.wind,
    }
}

/// Smoothstep on `[0,1]` (3t² − 2t³). Eases the epoch cross-fade.
fn smoothstep(t: f32) -> f32 {
    crate::util::mathx::smoothstep(0.0, 1.0, t) // r330-refactor-math
}

/// Tiny deterministic 2-input integer hash (splitmix-ish). Stable across runs
/// and platforms so weather is reproducible from `(epoch, seed)`.
fn hash2(a: u64, b: u64) -> u64 {
    let mut x = a
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(b.wrapping_mul(0xD1B5_4A32_D192_ED03))
        .wrapping_add(0x1234_5678_9ABC_DEF0);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_params_in_range() {
        for c in [
            Condition::Clear, Condition::Cloudy, Condition::Overcast,
            Condition::Rain, Condition::Storm, Condition::Fog,
        ] {
            let p = c.params();
            for v in [p.cloudiness, p.precipitation, p.fog, p.wind] {
                assert!((0.0..=1.0).contains(&v), "{c:?} param out of range: {v}");
            }
        }
    }

    #[test]
    fn clear_is_bright_storm_is_dim() {
        let clear = Condition::Clear.params();
        let storm = Condition::Storm.params();
        assert!(clear.sun_dimming() > 0.95, "clear should barely dim: {}", clear.sun_dimming());
        assert!(storm.sun_dimming() < 0.35, "storm should dim hard: {}", storm.sun_dimming());
        assert!(storm.fog_density() > clear.fog_density());
    }

    #[test]
    fn fog_condition_has_the_densest_fog() {
        let fog = Condition::Fog.params().fog_density();
        for c in [Condition::Clear, Condition::Cloudy, Condition::Overcast, Condition::Rain] {
            assert!(fog > c.params().fog_density(), "Fog should beat {c:?}");
        }
    }

    #[test]
    fn light_tint_cools_with_cloud() {
        let clear = Condition::Clear.params().light_tint();
        let overcast = Condition::Overcast.params().light_tint();
        // Clear light is warmer: red/blue ratio higher than overcast.
        assert!(clear[0] / clear[2] > overcast[0] / overcast[2],
            "overcast should be cooler: clear={clear:?} overcast={overcast:?}");
    }

    #[test]
    fn params_are_deterministic_from_time_and_seed() {
        let a = Weather { time_seconds: 1234.5, seed: 42, time_scale: 1.0 };
        let b = Weather { time_seconds: 1234.5, seed: 42, time_scale: 1.0 };
        assert_eq!(a.params(), b.params());
        // Different seed → (very likely) different weather sequence somewhere.
        let differ = (0..50).any(|e| {
            Condition::for_epoch(e, 42) != Condition::for_epoch(e, 7)
        });
        assert!(differ, "different seeds should diverge within 50 epochs");
    }

    #[test]
    fn params_cross_fade_is_continuous() {
        // Walk across an epoch boundary; adjacent samples shouldn't jump.
        let seed = 99;
        let mut prev: Option<WeatherParams> = None;
        let mut max_jump = 0.0_f32;
        let step = 0.5_f32; // seconds
        let mut t = 0.0_f32;
        while t < EPOCH_SECONDS * 4.0 {
            let w = Weather { time_seconds: t as f64, seed, time_scale: 1.0 };
            let p = w.params();
            if let Some(pp) = prev {
                let j = (p.cloudiness - pp.cloudiness).abs()
                    + (p.precipitation - pp.precipitation).abs()
                    + (p.fog - pp.fog).abs();
                max_jump = max_jump.max(j);
            }
            prev = Some(p);
            t += step;
        }
        // Over a 0.5s step the fade should move each param only a little.
        assert!(max_jump < 0.05, "weather params jumped: {max_jump}");
    }

    #[test]
    fn first_epoch_holds_without_predecessor() {
        // Epoch 0 has no previous condition to fade from; params must equal the
        // epoch-0 condition for the whole epoch (no NaN / no fade from nothing).
        let seed = 3;
        let cond0 = Condition::for_epoch(0, seed);
        for &frac in &[0.0_f32, 0.1, 0.24, 0.5, 0.99] {
            let w = Weather { time_seconds: (EPOCH_SECONDS * frac) as f64, seed, time_scale: 1.0 };
            assert_eq!(w.params(), cond0.params(), "epoch 0 should hold at frac {frac}");
        }
    }

    #[test]
    fn compose_clear_matches_sky_sun() {
        // Under clear weather, sun intensity should equal the sky's own
        // directional intensity (dimming ~1.0) and the sky color should be
        // essentially unchanged (grey ~0).
        let mut sky = Sky::new();
        sky.time_seconds = (crate::sky::DAY_LENGTH_SECONDS as f64) * 0.25; // noon
        let atmo = compose(&sky, WeatherParams::CLEAR);
        let bare = sky.directional_intensity();
        assert!((atmo.sun_intensity - bare).abs() < 1e-3,
            "clear sun_intensity {} should match sky {}", atmo.sun_intensity, bare);
        let sky_rgba = sky.sky_color_rgba();
        for ch in 0..3 {
            assert!((atmo.sky_color[ch] - sky_rgba[ch]).abs() < 1e-2,
                "clear sky color should be unchanged on ch {ch}");
        }
        assert_eq!(atmo.fog_density, 0.0);
    }

    #[test]
    fn compose_storm_dims_and_fogs() {
        let mut sky = Sky::new();
        sky.time_seconds = (crate::sky::DAY_LENGTH_SECONDS as f64) * 0.25; // noon
        let clear = compose(&sky, WeatherParams::CLEAR);
        let storm = compose(&sky, Condition::Storm.params());
        assert!(storm.sun_intensity < clear.sun_intensity * 0.5,
            "storm should cut sun intensity sharply");
        assert!(storm.fog_density > 0.5, "storm fog should be heavy: {}", storm.fog_density);
        // Fog color tracks the (greyed) sky color.
        assert_eq!(storm.fog_color, [storm.sky_color[0], storm.sky_color[1], storm.sky_color[2]]);
    }

    #[test]
    fn compose_keeps_night_dark_under_overcast() {
        // Greying the sky must not brighten a midnight sky.
        let mut sky = Sky::new();
        sky.time_seconds = (crate::sky::DAY_LENGTH_SECONDS as f64) * 0.75; // midnight
        let overcast = compose(&sky, Condition::Overcast.params());
        let lum = (overcast.sky_color[0] + overcast.sky_color[1] + overcast.sky_color[2]) / 3.0;
        assert!(lum < 0.4, "overcast midnight sky should stay dark, lum={lum}");
    }

    #[test]
    fn all_uniform_channels_finite_and_bounded() {
        let mut sky = Sky::new();
        for phase_step in 0..24 {
            sky.time_seconds =
                (crate::sky::DAY_LENGTH_SECONDS as f64) * (phase_step as f64 / 24.0);
            for c in [Condition::Clear, Condition::Rain, Condition::Storm, Condition::Fog] {
                let a = compose(&sky, c.params());
                let chans = [
                    a.sun_intensity, a.fog_density, a.wind,
                    a.sky_color[0], a.sky_color[1], a.sky_color[2],
                    a.ambient_color[0], a.ambient_color[1], a.ambient_color[2],
                ];
                for v in chans {
                    assert!(v.is_finite(), "non-finite uniform under {c:?}");
                    assert!((0.0..=1.5).contains(&v), "uniform {v} out of bounds under {c:?}");
                }
            }
        }
    }
}
