//! Weather state — precipitation (rain/snow), lightning, cloud cover,
//! fog, and wind, derived from the climate fields + time. The
//! simulation/state layer for weather; the renderer consumes
//! [`WeatherState`] to modulate the atmosphere (rain darkening, snow
//! whitening, overcast greying, fog, lightning flash, wind). Same
//! "state is authoritative, rendering is a consumer" pattern as
//! [`crate::world::season`]. [`WeatherState::kind`] gives a coarse
//! [`WeatherKind`] for gameplay/HUD/audio.
//!
//! # Scope of this first increment
//!
//! This produces *atmospheric* weather (sky/fog/ambient response) that
//! the existing shader uniforms already drive — no new draw passes, no
//! shaders. Rain *particles/streaks*, lightning *bolt geometry*, surface
//! *wetness*, and *thunder* are downstream consumers that need render
//! work; they'll read this same state when they land. See
//! `docs/CLIMATE_PATH.md`.
//!
//! # Determinism
//!
//! `sample` is a pure function of `(time, humidity, temperature)`. Time
//! is a monotonic unit supplied by the caller (the renderer passes its
//! frame counter), so launcher replays reproduce identical weather.
//!
//! # Tuning
//!
//! Off by default. `UNENDLICH_WEATHER=1` enables the renderer-side
//! atmosphere modulation. Storm cadence / flash decay are consts below.

use std::sync::OnceLock;

/// Monotonic time units per unit of storm noise. Larger = storms evolve
/// more slowly. At ~60 fps with the renderer passing frame count, a
/// storm-noise unit is ~100 s, so storms build and clear over a few
/// minutes of play.
const STORM_PERIOD: f64 = 6000.0;

/// Fraction of the storm-noise range that becomes rain. Noise below
/// `1 - STORM_RAIN_BAND` reads as clear; the top band ramps to a
/// downpour. Keeps most of the time clear.
const STORM_RAIN_BAND: f64 = 0.45;

/// Below this humidity, no rain regardless of storm signal; above the
/// upper bound, full storm signal passes. Smoothstep between.
const HUMIDITY_GATE_LO: f32 = 0.40;
const HUMIDITY_GATE_HI: f32 = 0.70;

/// Precipitation must exceed this for lightning to be possible.
const LIGHTNING_RAIN_THRESHOLD: f32 = 0.5;

/// Time units per potential-strike window, peak strike probability per
/// window at full downpour, and the e-fold decay time of a flash.
const STRIKE_WINDOW: f64 = 90.0;
const MAX_STRIKE_PROB: f64 = 0.5;
const FLASH_DECAY: f64 = 8.0;

/// Lightning flicker: a strike emits a bright main pulse, then a fainter
/// echo `FLICKER_GAP` time units later that decays faster — the
/// characteristic double-flash. `FLICKER_AMP` is the echo's peak relative
/// to the main pulse.
const FLICKER_GAP: f64 = 3.5;
const FLICKER_AMP: f64 = 0.55;
const FLICKER_DECAY: f64 = 3.0;

/// Precipitation freezes to snow below `FREEZE_LO` temperature and is all
/// rain above `FREEZE_HI`, on the climate `[0, 1]` temperature scale
/// (lower = colder). Smoothstep between — sleet/mix in the band.
const FREEZE_LO: f32 = 0.25;
const FREEZE_HI: f32 = 0.38;

// ── Varied weather (cloud cover / fog / wind) ───────────────────────────
// Each derived signal uses `value_noise` on a distinct period and a
// distinct domain offset so the signals don't correlate. Offsets are
// arbitrary primes-ish; only their distinctness matters.

/// Overcast evolves more slowly than storms (a grey day outlasts a shower).
const CLOUD_PERIOD: f64 = 9000.0;
const CLOUD_NOISE_OFFSET: f64 = 113.0;
/// Dry air clears the sky; humid air clouds it. Smoothstep gate.
const CLOUD_HUMIDITY_LO: f32 = 0.30;
const CLOUD_HUMIDITY_HI: f32 = 0.75;

/// Fog/mist rolls in over a medium period, only in cool + humid + calm air.
const FOG_PERIOD: f64 = 5000.0;
const FOG_NOISE_OFFSET: f64 = 401.0;
const FOG_HUMIDITY_LO: f32 = 0.55;
const FOG_HUMIDITY_HI: f32 = 0.85;
/// Colder = mistier (1 - smoothstep, so low temp → full mist potential).
const FOG_TEMP_LO: f32 = 0.20;
const FOG_TEMP_HI: f32 = 0.60;

/// Prevailing wind direction rotates over a very long period; gusts ride
/// a fast period on top of a medium baseline breeze.
const WIND_DIR_PERIOD: f64 = 40000.0;
const WIND_DIR_OFFSET: f64 = 71.0;
const WIND_BASE_PERIOD: f64 = 3000.0;
const WIND_BASE_OFFSET: f64 = 211.0;
const WIND_GUST_PERIOD: f64 = 600.0;
const WIND_GUST_OFFSET: f64 = 307.0;

/// `WeatherState::kind` classification thresholds.
const PRECIP_KIND_THRESHOLD: f32 = 0.12; // any precip above this = Rain/Snow
const STORM_LIQUID_THRESHOLD: f32 = 0.60; // heavy liquid rain = Storm
const FOG_KIND_THRESHOLD: f32 = 0.45;
const OVERCAST_KIND_THRESHOLD: f32 = 0.55;

/// A coarse, **derived** classification of the current weather — the
/// weather analogue of [`crate::world::biomes::Biome`]. The authoritative
/// state is the continuous [`WeatherState`] field set, which is itself a
/// layering of the temperature/humidity climate fields; this enum is a
/// best-fit *compatibility / convenience* surface over those fields,
/// produced at call time by [`classify_weather`] (the weather counterpart
/// of `biomes::determine_biome`) and never stored. It exists so code — and
/// developers used to enum-based weather APIs in other engines — get an
/// intuitive label without the field model losing authority. Consumers
/// that can read the continuous fields (e.g. the renderer's atmosphere
/// modulation) should do so directly rather than branching on the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeatherKind {
    Clear,
    Overcast,
    Fog,
    Rain,
    Snow,
    Storm,
}

/// Simulated weather at an instant. All fields normalized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherState {
    /// 0 = clear, 1 = downpour. Total precipitation, before the
    /// rain/snow split.
    pub precipitation: f32,
    /// 0 = no flash, 1 = peak lightning flash (decays after a strike).
    /// Only liquid (rain) precipitation generates lightning.
    pub lightning_flash: f32,
    /// Frozen fraction of the precipitation, from temperature. 0 = all
    /// rain, 1 = all snow. Cold air freezes the falling precipitation.
    pub snow_fraction: f32,
    /// 0 = clear sky, 1 = full overcast. Independent of rain (you can be
    /// overcast-dry), but rain forces it high. Greys/dims the sky.
    pub cloud_cover: f32,
    /// 0 = clear air, 1 = thick mist. Cool + humid + calm conditions.
    /// Drives visibility (fog density) separately from base humidity.
    pub fog: f32,
    /// Prevailing wind direction in radians `[0, TAU)`.
    pub wind_dir: f32,
    /// Wind strength `[0, 1]` — baseline breeze + gusts, lifted by storms.
    pub wind_speed: f32,
}

impl WeatherState {
    pub const CLEAR: WeatherState = WeatherState {
        precipitation: 0.0,
        lightning_flash: 0.0,
        snow_fraction: 0.0,
        cloud_cover: 0.0,
        fog: 0.0,
        wind_dir: 0.0,
        wind_speed: 0.0,
    };

    /// Liquid (rain) component — darkens the sky, drives lightning.
    #[inline]
    pub fn liquid_rain(&self) -> f32 {
        self.precipitation * (1.0 - self.snow_fraction)
    }

    /// Frozen (snow) component — brightens/whitens the sky.
    #[inline]
    pub fn snow_amount(&self) -> f32 {
        self.precipitation * self.snow_fraction
    }

    /// Coarse classification for gameplay/HUD/audio — a thin convenience
    /// wrapper over [`classify_weather`] on this state's fields. See
    /// [`WeatherKind`] for why this is a derived surface, not state.
    pub fn kind(&self) -> WeatherKind {
        classify_weather(
            self.precipitation,
            self.snow_fraction,
            self.cloud_cover,
            self.fog,
        )
    }
}

/// Process-wide enable gate, read once from `UNENDLICH_WEATHER`.
pub fn weather_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("UNENDLICH_WEATHER")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}

#[inline]
fn hash_u64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

/// Hash to `[0, 1)`.
#[inline]
fn hash01(x: u64) -> f64 {
    (hash_u64(x) >> 11) as f64 / (1u64 << 53) as f64
}

/// Smooth value noise in `[0, 1)`, smoothstep-interpolated between
/// integer lattice points.
fn value_noise(t: f64) -> f64 {
    let i = t.floor();
    let f = t - i;
    let ia = i as i64 as u64;
    let a = hash01(ia);
    let b = hash01(ia.wrapping_add(1));
    let u = f * f * (3.0 - 2.0 * f);
    a + (b - a) * u
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rain intensity for a time + humidity. Storm noise gated by humidity.
pub fn precipitation(time: f64, humidity: f32) -> f32 {
    let storm = value_noise(time / STORM_PERIOD); // [0, 1)
    let storm_rain =
        (((storm - (1.0 - STORM_RAIN_BAND)) / STORM_RAIN_BAND).clamp(0.0, 1.0)) as f32;
    let gate = smoothstep(HUMIDITY_GATE_LO, HUMIDITY_GATE_HI, humidity);
    (storm_rain * gate).clamp(0.0, 1.0)
}

/// Lightning flash intensity given the current *liquid* rain (snow
/// doesn't generate lightning). Strikes are sampled per window; each
/// fired strike contributes a main decaying pulse plus a fainter,
/// faster-decaying echo (the characteristic flicker). We scan the
/// current and two prior windows so a recent strike's decay is visible.
pub fn lightning_flash(time: f64, liquid_rain: f32) -> f32 {
    if liquid_rain < LIGHTNING_RAIN_THRESHOLD {
        return 0.0;
    }
    let prob = ((liquid_rain - LIGHTNING_RAIN_THRESHOLD)
        / (1.0 - LIGHTNING_RAIN_THRESHOLD)) as f64
        * MAX_STRIKE_PROB;
    let bucket = (time / STRIKE_WINDOW).floor() as i64;
    let mut flash = 0.0f64;
    for db in 0..=2i64 {
        let b = bucket - db;
        if b < 0 {
            continue;
        }
        let bh = hash_u64((b as u64) ^ 0x9E37_79B9_7F4A_7C15);
        if hash01(bh) < prob {
            let frac = hash01(bh.wrapping_mul(2654435761));
            let strike_time = (b as f64 + frac) * STRIKE_WINDOW;
            if strike_time <= time {
                let dt = time - strike_time;
                // Main pulse.
                let main = (-dt / FLASH_DECAY).exp();
                // Echo: a second, fainter pulse FLICKER_GAP later.
                let echo = if dt >= FLICKER_GAP {
                    FLICKER_AMP * (-(dt - FLICKER_GAP) / FLICKER_DECAY).exp()
                } else {
                    0.0
                };
                let d = main.max(echo);
                if d > flash {
                    flash = d;
                }
            }
        }
    }
    (flash as f32).clamp(0.0, 1.0)
}

/// Frozen fraction of precipitation from temperature (`[0, 1]` climate
/// scale, lower = colder). 1 = all snow (cold), 0 = all rain (warm),
/// smooth sleet band between `FREEZE_LO` and `FREEZE_HI`.
pub fn snow_fraction(temperature: f32) -> f32 {
    1.0 - smoothstep(FREEZE_LO, FREEZE_HI, temperature)
}

/// Overcast amount `[0, 1]`. A slow-evolving base, gated by humidity
/// (dry air clears), and forced to at least the precipitation level
/// (it can't be raining under a blue sky).
pub fn cloud_cover(time: f64, humidity: f32, precipitation: f32) -> f32 {
    let base = value_noise(time / CLOUD_PERIOD + CLOUD_NOISE_OFFSET) as f32;
    let gate = smoothstep(CLOUD_HUMIDITY_LO, CLOUD_HUMIDITY_HI, humidity);
    (base * gate).max(precipitation).clamp(0.0, 1.0)
}

/// Fog/mist `[0, 1]`. Rolls in on a medium period, but only takes hold
/// in cool, humid, calm air. Heavy precipitation suppresses still mist
/// (a downpour isn't fog) without zeroing low-visibility entirely.
pub fn fog(time: f64, humidity: f32, temperature: f32, precipitation: f32) -> f32 {
    let event = value_noise(time / FOG_PERIOD + FOG_NOISE_OFFSET) as f32;
    let humid = smoothstep(FOG_HUMIDITY_LO, FOG_HUMIDITY_HI, humidity);
    let cool = 1.0 - smoothstep(FOG_TEMP_LO, FOG_TEMP_HI, temperature);
    (event * humid * cool * (1.0 - 0.5 * precipitation)).clamp(0.0, 1.0)
}

/// Prevailing wind as `(direction_radians, speed)`. Direction rotates
/// over a very long period; speed is a baseline breeze plus fast gusts,
/// lifted toward 1 by storms (precipitation).
pub fn wind(time: f64, precipitation: f32) -> (f32, f32) {
    let dir = value_noise(time / WIND_DIR_PERIOD + WIND_DIR_OFFSET) as f32
        * std::f32::consts::TAU;
    let base = value_noise(time / WIND_BASE_PERIOD + WIND_BASE_OFFSET) as f32;
    let gust = value_noise(time / WIND_GUST_PERIOD + WIND_GUST_OFFSET) as f32;
    let speed = (base * 0.5 + gust * 0.3 + precipitation * 0.5).clamp(0.0, 1.0);
    (dir, speed)
}

/// Full weather sample. Temperature splits precipitation into rain vs
/// snow; lightning is gated to the liquid (rain) fraction.
/// Best-fit classification of the authoritative weather field scalars into
/// the compatibility [`WeatherKind`]. The weather analogue of
/// [`crate::world::biomes::determine_biome`]: the fields are authoritative,
/// the enum is a derived convenience surface, and this is a pure runtime
/// translation (no stored mapping). Priority cascade — heavy liquid rain →
/// `Storm`, then any snow → `Snow`, any liquid → `Rain`, thick mist →
/// `Fog`, grey sky → `Overcast`, else `Clear`. Thresholds are the
/// `*_KIND_THRESHOLD` consts; if the biome table ever moves to data
/// (`biomes.json`), this cascade is the natural thing to externalize too.
pub fn classify_weather(
    precipitation: f32,
    snow_fraction: f32,
    cloud_cover: f32,
    fog: f32,
) -> WeatherKind {
    let liquid = precipitation * (1.0 - snow_fraction);
    let snow = precipitation * snow_fraction;
    if liquid >= STORM_LIQUID_THRESHOLD {
        WeatherKind::Storm
    } else if snow >= PRECIP_KIND_THRESHOLD {
        WeatherKind::Snow
    } else if liquid >= PRECIP_KIND_THRESHOLD {
        WeatherKind::Rain
    } else if fog >= FOG_KIND_THRESHOLD {
        WeatherKind::Fog
    } else if cloud_cover >= OVERCAST_KIND_THRESHOLD {
        WeatherKind::Overcast
    } else {
        WeatherKind::Clear
    }
}

pub fn sample(time: f64, humidity: f32, temperature: f32) -> WeatherState {
    let precipitation = precipitation(time, humidity);
    let snow_fraction = snow_fraction(temperature);
    let liquid = precipitation * (1.0 - snow_fraction);
    let lightning_flash = lightning_flash(time, liquid);
    let cloud_cover = cloud_cover(time, humidity, precipitation);
    let fog = fog(time, humidity, temperature, precipitation);
    let (wind_dir, wind_speed) = wind(time, precipitation);
    WeatherState {
        precipitation,
        lightning_flash,
        snow_fraction,
        cloud_cover,
        fog,
        wind_dir,
        wind_speed,
    }
}

// ---- r336-weather: global, seeded weather state machine --------------------
//
// The functions above are the continuous, time-sampled field model. This layer
// adds a discrete *world-wide* weather cycle on top: a spell of one
// [`WeatherKind`] holds for a rolled duration, then transitions to the next,
// with the transition odds gated by temperature (snow only when cold) and
// humidity (rain needs moisture). Deterministic — every roll is a hash of the
// world `seed` and an evolving `phase` counter (no RNG/clock); time enters only
// as the caller-supplied `dt`. Reuses the module's `hash_u64`/`hash01` and the
// `liquid_rain`/`snow_amount` rain/snow split via `WeatherState`.

// climate gates
const SNOW_TEMP_LO: f32 = 0.18; // below → fully "cold" (snow allowed)
const SNOW_TEMP_HI: f32 = 0.34; // above → no snow
const RAIN_HUMIDITY_LO: f32 = 0.30; // below → no rain/storm (too dry)
const RAIN_HUMIDITY_HI: f32 = 0.70;
const STORM_HUMIDITY_LO: f32 = 0.60; // storms need a lot of moisture
const STORM_HUMIDITY_HI: f32 = 0.90;

/// Per-kind spell duration band, in caller ticks (the bin decides tick = second
/// or frame). Mins are well above 1 so weather never flickers per tick.
fn duration_band(kind: WeatherKind) -> (f32, f32) {
    match kind {
        WeatherKind::Clear => (600.0, 1800.0),
        WeatherKind::Overcast => (400.0, 1200.0),
        WeatherKind::Fog => (300.0, 900.0),
        WeatherKind::Rain => (300.0, 1000.0),
        WeatherKind::Snow => (400.0, 1200.0),
        WeatherKind::Storm => (150.0, 500.0), // intense, short-lived
    }
}

/// Per-kind intensity band `[0,1]`.
fn intensity_band(kind: WeatherKind) -> (f32, f32) {
    match kind {
        WeatherKind::Clear => (0.0, 0.0),
        WeatherKind::Overcast => (0.30, 0.60),
        WeatherKind::Fog => (0.40, 0.80),
        WeatherKind::Rain => (0.40, 0.80),
        WeatherKind::Snow => (0.40, 0.80),
        WeatherKind::Storm => (0.80, 1.0),
    }
}

/// All kinds, in a fixed order, for the weighted transition roll.
const KINDS: [WeatherKind; 6] = [
    WeatherKind::Clear,
    WeatherKind::Overcast,
    WeatherKind::Fog,
    WeatherKind::Rain,
    WeatherKind::Snow,
    WeatherKind::Storm,
];

/// Climate-gated transition weight for each candidate kind. Snow weight is 0
/// unless it is cold; rain/storm weights are 0 unless humid; Clear is always a
/// possibility so the sky can always settle.
fn transition_weight(kind: WeatherKind, temp01: f32, humidity01: f32) -> f32 {
    let humid = smoothstep(RAIN_HUMIDITY_LO, RAIN_HUMIDITY_HI, humidity01);
    let stormy = smoothstep(STORM_HUMIDITY_LO, STORM_HUMIDITY_HI, humidity01);
    let cold = 1.0 - smoothstep(SNOW_TEMP_LO, SNOW_TEMP_HI, temp01); // 1 cold .. 0 warm
    let warm = 1.0 - cold;
    match kind {
        WeatherKind::Clear => 1.0,
        WeatherKind::Overcast => 0.6 + 0.4 * humid,
        WeatherKind::Fog => 0.8 * humid * cold,
        WeatherKind::Rain => 1.2 * humid * warm, // moisture + above freezing
        WeatherKind::Snow => 1.2 * humid * cold, // moisture + below freezing
        WeatherKind::Storm => 1.0 * stormy * warm, // heavy moisture, not frozen
    }
}

/// Pick the next kind by a weighted roll on `roll` in `[0,1)`. Falls back to
/// `Clear` if every weight is ~0 (e.g. dry + mid temperature gating others out).
fn choose_kind(temp01: f32, humidity01: f32, roll: f32) -> WeatherKind {
    let mut total = 0.0f32;
    let mut weights = [0.0f32; 6];
    for (i, &k) in KINDS.iter().enumerate() {
        let w = transition_weight(k, temp01, humidity01).max(0.0);
        weights[i] = w;
        total += w;
    }
    if total <= 1e-6 {
        return WeatherKind::Clear;
    }
    let target = roll.clamp(0.0, 1.0) * total;
    let mut acc = 0.0f32;
    for (i, &k) in KINDS.iter().enumerate() {
        acc += weights[i];
        if target < acc {
            return k;
        }
    }
    WeatherKind::Clear
}

#[inline]
fn lerp_band(band: (f32, f32), u: f32) -> f32 {
    band.0 + (band.1 - band.0) * u.clamp(0.0, 1.0)
}

/// Rain/snow split of the current global weather, in `[0,1]`. Reuses the
/// `WeatherState::liquid_rain`/`snow_amount` semantics: a precipitation amount
/// (from the current kind + intensity) split by `snow_fraction(temp)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Precip {
    pub rain01: f32,
    pub snow01: f32,
}

/// A world-wide weather spell with a remaining duration, so it cycles rather
/// than flickering. `phase` advances each transition and (with the world seed)
/// drives every roll deterministically.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalWeather {
    pub current: WeatherKind,
    pub intensity01: f32,
    pub ticks_remaining: f32,
    pub phase: u64,
}

impl GlobalWeather {
    /// The deterministic opening spell for a world `seed`: rolls a first kind
    /// and duration from the climate at genesis. (Equivalent to stepping a
    /// zero-duration Clear once.)
    pub fn new(seed: u64, temp01: f32, humidity01: f32) -> GlobalWeather {
        let start = GlobalWeather {
            current: WeatherKind::Clear,
            intensity01: 0.0,
            ticks_remaining: 0.0,
            phase: 0,
        };
        start.step(0.0, seed, temp01, humidity01)
    }

    /// Advance the spell by `dt` ticks. While the spell has time left, the kind
    /// and intensity hold (no per-tick flicker); when it expires, roll the next
    /// kind/duration/intensity deterministically from `(seed, phase)`, gated by
    /// `temp01`/`humidity01`. `dt < 0`/NaN is treated as `0`.
    pub fn step(self, dt: f32, seed: u64, temp01: f32, humidity01: f32) -> GlobalWeather {
        let dt = if dt.is_nan() || dt < 0.0 { 0.0 } else { dt };
        let remaining = self.ticks_remaining - dt;
        if remaining > 0.0 {
            // still inside the current spell — hold steady.
            return GlobalWeather { ticks_remaining: remaining, ..self };
        }
        // transition: derive three independent rolls from (seed, next phase).
        let next_phase = self.phase.wrapping_add(1);
        let base = seed ^ hash_u64(next_phase);
        let roll_kind = hash01(base) as f32;
        let roll_dur = hash01(hash_u64(base ^ 0xD1B5_4A32_D192_ED03)) as f32;
        let roll_int = hash01(hash_u64(base ^ 0x9E37_79B9_7F4A_7C15)) as f32;

        let kind = choose_kind(temp01, humidity01, roll_kind);
        let dur = lerp_band(duration_band(kind), roll_dur).max(1.0);
        let intensity = lerp_band(intensity_band(kind), roll_int).clamp(0.0, 1.0);
        GlobalWeather {
            current: kind,
            intensity01: intensity,
            ticks_remaining: dur,
            phase: next_phase,
        }
    }
}

/// Rain/snow split for the current global weather at temperature `temp01`.
/// Clear/Overcast/Fog produce no precipitation; Rain/Snow/Storm produce
/// `intensity01`, split into liquid vs frozen by `snow_fraction(temp01)` — so a
/// Snow spell (only chosen when cold ⇒ high snow fraction) reads as snow, and a
/// Rain/Storm spell as rain. Reuses `WeatherState::liquid_rain`/`snow_amount`.
pub fn precip_for(w: &GlobalWeather, temp01: f32) -> Precip {
    let precip = match w.current {
        WeatherKind::Rain | WeatherKind::Snow | WeatherKind::Storm => w.intensity01,
        WeatherKind::Clear | WeatherKind::Overcast | WeatherKind::Fog => 0.0,
    }
    .clamp(0.0, 1.0);
    let ws = WeatherState {
        precipitation: precip,
        snow_fraction: snow_fraction(temp01),
        ..WeatherState::CLEAR
    };
    Precip { rain01: ws.liquid_rain(), snow01: ws.snow_amount() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precipitation_is_bounded() {
        for i in 0..20_000 {
            let t = i as f64 * (STORM_PERIOD / 7.0);
            let p = precipitation(t, 1.0);
            assert!((0.0..=1.0).contains(&p), "precip {} out of range at t={}", p, t);
        }
    }

    #[test]
    fn no_rain_in_dry_air() {
        // Below the humidity gate, no time should produce rain.
        for i in 0..5_000 {
            let t = i as f64 * (STORM_PERIOD / 3.0);
            assert_eq!(precipitation(t, 0.10), 0.0,
                "dry air should never rain (t={})", t);
        }
    }

    #[test]
    fn rain_occurs_in_humid_air_at_some_time() {
        // Over a long window, humid air must hit at least one storm.
        let mut max_p = 0.0f32;
        for i in 0..50_000 {
            let t = i as f64 * (STORM_PERIOD / 4.0);
            max_p = max_p.max(precipitation(t, 1.0));
        }
        assert!(max_p > 0.3, "humid air should storm sometimes; max seen {}", max_p);
    }

    #[test]
    fn no_lightning_without_heavy_rain() {
        assert_eq!(lightning_flash(12345.0, 0.0), 0.0);
        assert_eq!(lightning_flash(12345.0, LIGHTNING_RAIN_THRESHOLD - 0.01), 0.0);
    }

    #[test]
    fn lightning_flash_is_bounded() {
        for i in 0..50_000 {
            let t = i as f64 * 7.0;
            let f = lightning_flash(t, 1.0);
            assert!((0.0..=1.0).contains(&f), "flash {} out of range at t={}", f, t);
        }
    }

    #[test]
    fn lightning_strikes_during_a_downpour() {
        // At full downpour, at least one strike should flash over a span.
        let mut any = false;
        for i in 0..10_000 {
            if lightning_flash(i as f64 * 3.0, 1.0) > 0.05 {
                any = true;
                break;
            }
        }
        assert!(any, "a full downpour should produce lightning over 10k ticks");
    }

    #[test]
    fn sample_is_deterministic() {
        let a = sample(98765.0, 0.8, 0.5);
        let b = sample(98765.0, 0.8, 0.5);
        assert_eq!(a, b);
    }

    #[test]
    fn snow_fraction_goes_cold_to_warm() {
        assert_eq!(snow_fraction(0.0), 1.0, "freezing temp = all snow");
        assert_eq!(snow_fraction(1.0), 0.0, "warm temp = all rain");
        // Monotonic non-increasing as it warms.
        let mut prev = snow_fraction(0.0);
        for i in 1..=100 {
            let s = snow_fraction(i as f32 / 100.0);
            assert!(s <= prev + 1e-6, "snow fraction should not rise with temp");
            assert!((0.0..=1.0).contains(&s));
            prev = s;
        }
    }

    #[test]
    fn liquid_and_snow_split_sums_to_precipitation() {
        let w = WeatherState {
            precipitation: 0.8,
            lightning_flash: 0.0,
            snow_fraction: 0.3,
            cloud_cover: 0.0,
            fog: 0.0,
            wind_dir: 0.0,
            wind_speed: 0.0,
        };
        assert!((w.liquid_rain() + w.snow_amount() - w.precipitation).abs() < 1e-6);
    }

    #[test]
    fn no_lightning_in_a_snowstorm() {
        // Full precipitation but freezing — it's snow, so no lightning at
        // any time.
        for i in 0..10_000 {
            let t = i as f64 * 3.0;
            let w = sample(t, 1.0, 0.0); // humid + freezing
            assert!(w.snow_fraction > 0.99, "should be snowing");
            assert_eq!(w.lightning_flash, 0.0, "snowstorms don't flash (t={})", t);
        }
    }

    #[test]
    fn warm_storms_can_still_flash() {
        // A warm (rain) downpour should still produce lightning. Warm temp
        // => snow_fraction 0 => all precipitation is liquid rain, which is
        // what drives lightning. We must sweep MANY storm periods
        // (STORM_PERIOD=6000) to be sure we actually hit a downpour whose
        // storm noise clears the lightning rain threshold — a short sweep
        // can land entirely between peaks and see no rain at all (this is
        // exactly why the original 5-period range was flaky). The step
        // stays fine enough (~5) to land inside the brief post-strike flash
        // decay (~24 time units) instead of aliasing over it; ~100 periods
        // makes a qualifying downpour effectively certain.
        let mut any = false;
        for i in 0..120_000 {
            if sample(i as f64 * 5.0, 1.0, 1.0).lightning_flash > 0.05 {
                any = true;
                break;
            }
        }
        assert!(any, "warm downpours should produce lightning");
    }

    #[test]
    fn variety_fields_are_bounded() {
        for i in 0..20_000 {
            let t = i as f64 * 7.0;
            let h = ((i % 11) as f32) / 10.0;
            let temp = ((i % 7) as f32) / 6.0;
            let w = sample(t, h, temp);
            assert!((0.0..=1.0).contains(&w.cloud_cover), "cloud {}", w.cloud_cover);
            assert!((0.0..=1.0).contains(&w.fog), "fog {}", w.fog);
            assert!((0.0..=1.0).contains(&w.wind_speed), "wind {}", w.wind_speed);
            assert!(
                (0.0..std::f32::consts::TAU).contains(&w.wind_dir),
                "dir {}", w.wind_dir
            );
        }
    }

    #[test]
    fn weather_shows_variety() {
        use std::collections::HashSet;
        let mut kinds = HashSet::new();
        for ti in 0..4_000 {
            let t = ti as f64 * 45.0;
            for &h in &[0.2f32, 0.45, 0.6, 0.75, 0.9] {
                for &temp in &[0.15f32, 0.3, 0.5, 0.8] {
                    kinds.insert(sample(t, h, temp).kind());
                }
            }
        }
        assert!(kinds.len() >= 4, "expected varied weather, saw {:?}", kinds);
    }

    #[test]
    fn rain_implies_overcast() {
        for i in 0..40_000 {
            let w = sample(i as f64 * 5.0, 1.0, 1.0); // humid + warm => all liquid
            if w.precipitation > 0.5 {
                assert!(
                    w.cloud_cover >= w.precipitation - 1e-4,
                    "raining {} but sky not overcast {}", w.precipitation, w.cloud_cover
                );
            }
        }
    }

    #[test]
    fn clear_state_classifies_clear() {
        assert_eq!(WeatherState::CLEAR.kind(), WeatherKind::Clear);
    }

    #[test]
    fn classify_weather_matches_kind_and_cascade() {
        // The free fn (the determine_biome analogue) agrees with the method.
        let w = sample(7_777.0, 0.8, 0.5);
        assert_eq!(
            w.kind(),
            classify_weather(w.precipitation, w.snow_fraction, w.cloud_cover, w.fog)
        );
        // Cascade spot-checks straight on the authoritative field scalars.
        assert_eq!(classify_weather(0.0, 0.0, 0.0, 0.0), WeatherKind::Clear);
        assert_eq!(classify_weather(0.9, 0.0, 1.0, 0.0), WeatherKind::Storm);
        assert_eq!(classify_weather(0.5, 1.0, 1.0, 0.0), WeatherKind::Snow);
        assert_eq!(classify_weather(0.3, 0.0, 1.0, 0.0), WeatherKind::Rain);
        assert_eq!(classify_weather(0.0, 0.0, 0.0, 0.9), WeatherKind::Fog);
        assert_eq!(classify_weather(0.0, 0.0, 0.9, 0.0), WeatherKind::Overcast);
    }
}

#[cfg(test)]
mod r336_weather_tests {
    use super::*;

    // Drive a long run of transitions at a fixed climate, returning the kinds seen.
    fn run(seed: u64, temp01: f32, humidity01: f32, transitions: usize) -> Vec<GlobalWeather> {
        let mut w = GlobalWeather::new(seed, temp01, humidity01);
        let mut out = vec![w];
        let mut guard = 0;
        while out.len() <= transitions && guard < 5_000_000 {
            // step by the full remaining duration (+1) to force the next transition.
            let dt = w.ticks_remaining + 1.0;
            w = w.step(dt, seed, temp01, humidity01);
            out.push(w);
            guard += 1;
        }
        out
    }

    #[test]
    fn deterministic_for_a_seed() {
        let a = run(0xABCD_1234, 0.6, 0.8, 200);
        let b = run(0xABCD_1234, 0.6, 0.8, 200);
        assert_eq!(a, b, "same seed + climate ⇒ identical weather sequence");
        // different seed ⇒ generally different sequence.
        let c = run(0x9999_0001, 0.6, 0.8, 200);
        assert!(a != c, "different seeds should diverge");
    }

    #[test]
    fn snow_only_when_cold() {
        // Warm + humid: snow must never appear over a long run.
        let warm = run(0x5151, 0.85, 0.9, 400);
        assert!(
            warm.iter().all(|w| w.current != WeatherKind::Snow),
            "snow appeared in warm climate"
        );
        // Cold + humid: snow should appear.
        let cold = run(0x5151, 0.05, 0.9, 400);
        assert!(
            cold.iter().any(|w| w.current == WeatherKind::Snow),
            "snow never appeared in a cold, humid climate"
        );
    }

    #[test]
    fn rain_needs_humidity() {
        // Bone dry: no rain or storm over a long run (only clear/overcast/fog).
        let dry = run(0x7777, 0.6, 0.02, 400);
        assert!(
            dry.iter().all(|w| !matches!(w.current, WeatherKind::Rain | WeatherKind::Storm)),
            "rain/storm appeared in a dry climate"
        );
        // Warm + very humid: rain appears.
        let wet = run(0x7777, 0.7, 0.95, 400);
        assert!(
            wet.iter().any(|w| w.current == WeatherKind::Rain),
            "rain never appeared in a warm, humid climate"
        );
    }

    #[test]
    fn durations_respected_no_one_tick_flips() {
        // every spell starts with at least its kind's minimum duration ...
        let seq = run(0x2024, 0.6, 0.8, 300);
        for w in &seq {
            let (lo, _hi) = duration_band(w.current);
            assert!(w.ticks_remaining >= lo.min(1.0) - 1e-3 || w.ticks_remaining <= 0.0 + 1.0);
            assert!(w.ticks_remaining >= 1.0 - 1e-3, "spell shorter than a tick: {:?}", w);
        }
        // ... and a small dt inside a spell never changes the kind.
        let w0 = GlobalWeather::new(0x2024, 0.6, 0.8);
        let w1 = w0.step(0.5, 0x2024, 0.6, 0.8); // far less than any min duration
        assert_eq!(w0.current, w1.current, "a sub-spell step must not flip the kind");
        assert_eq!(w0.phase, w1.phase, "no transition mid-spell");
        assert!((w1.ticks_remaining - (w0.ticks_remaining - 0.5)).abs() < 1e-3);
    }

    #[test]
    fn intensity_always_bounded() {
        for &(t, h) in &[(0.05f32, 0.9f32), (0.85, 0.95), (0.5, 0.02), (0.6, 0.7)] {
            for w in run(0x1357, t, h, 300) {
                assert!((0.0..=1.0).contains(&w.intensity01) && w.intensity01.is_finite());
                assert!(w.ticks_remaining.is_finite());
            }
        }
        // Clear is always zero-intensity.
        let dry = run(0x1357, 0.5, 0.0, 200);
        for w in dry.iter().filter(|w| w.current == WeatherKind::Clear) {
            assert_eq!(w.intensity01, 0.0);
        }
    }

    #[test]
    fn clear_and_storm_both_reachable() {
        // Warm + very humid climate: a storm must occur, and clear must also occur
        // (the cycle returns to calm), over a long run.
        let seq = run(0xF00D, 0.75, 0.97, 600);
        assert!(seq.iter().any(|w| w.current == WeatherKind::Storm), "storm unreachable");
        assert!(seq.iter().any(|w| w.current == WeatherKind::Clear), "clear unreachable");
    }

    #[test]
    fn precip_split_matches_kind_and_temp() {
        // Clear ⇒ no precip.
        let clear = GlobalWeather { current: WeatherKind::Clear, intensity01: 0.0, ticks_remaining: 100.0, phase: 1 };
        assert_eq!(precip_for(&clear, 0.5), Precip { rain01: 0.0, snow01: 0.0 });
        // Snow spell in the cold ⇒ snow-dominant, ~no rain.
        let snow = GlobalWeather { current: WeatherKind::Snow, intensity01: 0.7, ticks_remaining: 100.0, phase: 2 };
        let p = precip_for(&snow, 0.05);
        assert!(p.snow01 > p.rain01 && p.snow01 > 0.0, "cold snow spell should read as snow: {:?}", p);
        assert!((p.rain01 + p.snow01 - 0.7).abs() < 1e-5, "split conserves total precip");
        // Rain/Storm spell in the warm ⇒ rain-dominant.
        let storm = GlobalWeather { current: WeatherKind::Storm, intensity01: 0.9, ticks_remaining: 100.0, phase: 3 };
        let ps = precip_for(&storm, 0.9);
        assert!(ps.rain01 > ps.snow01 && ps.rain01 > 0.0, "warm storm should read as rain: {:?}", ps);
        // bounded
        for &(t, k, i) in &[(0.0f32, WeatherKind::Snow, 1.0f32), (1.0, WeatherKind::Storm, 1.0)] {
            let w = GlobalWeather { current: k, intensity01: i, ticks_remaining: 10.0, phase: 4 };
            let pr = precip_for(&w, t);
            assert!((0.0..=1.0).contains(&pr.rain01) && (0.0..=1.0).contains(&pr.snow01));
        }
    }

    #[test]
    fn step_handles_zero_and_negative_dt() {
        let w = GlobalWeather { current: WeatherKind::Rain, intensity01: 0.5, ticks_remaining: 200.0, phase: 9 };
        let z = w.step(0.0, 0x1, 0.6, 0.8);
        assert_eq!(z.ticks_remaining, 200.0, "dt=0 ⇒ no time passes");
        assert_eq!(z.current, w.current);
        let n = w.step(-50.0, 0x1, 0.6, 0.8);
        assert_eq!(n.ticks_remaining, 200.0, "negative dt clamped to 0");
        let nan = w.step(f32::NAN, 0x1, 0.6, 0.8);
        assert!(nan.ticks_remaining.is_finite() && nan.ticks_remaining == 200.0);
    }
}
