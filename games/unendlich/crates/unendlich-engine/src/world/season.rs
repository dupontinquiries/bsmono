//! Seasonal climate axis — the temporal leg of the climate triangle
//! (wind / moisture / temperature), modulated over a year cycle.
//!
//! # Design: render-only, worldgen-safe
//!
//! Worldgen reads `WorldModel::temperature` / `humidity` directly, and
//! those feed `sea_level` → `density` — i.e. terrain shape. Seasons must
//! NOT change terrain (a world can't re-grow its mountains every winter),
//! so this module is deliberately **not** baked into the climate fields.
//! Instead it produces small additive deltas that are applied at the
//! *render-tint consumption point* (the mesher's per-chunk
//! temperature/humidity sample that drives climate-responsive color).
//! The base climate fields — and therefore worldgen — are untouched.
//!
//! Because the renderer's climate-tint already consumes per-chunk
//! temperature/humidity, modulating those at sample time gives seasonal
//! color shifts (greener springs, golden/dry summers, etc.) with **zero
//! shader changes**. Newly-meshed chunks reflect the current season;
//! live transition for already-meshed static chunks needs a periodic
//! re-mesh trigger, which is a separate (filed) increment.
//!
//! # The year
//!
//! Time is the simulation tick (`u64`). One year is `ticks_per_year()`
//! ticks; the phase is `(tick mod ticks_per_year) / ticks_per_year` in
//! `[0, 1)`. Phase landmarks:
//!
//! ```text
//!   phase 0.00  spring begins   (temp neutral, rising)
//!   phase 0.25  summer peak     (warmest, driest)
//!   phase 0.50  autumn begins   (temp neutral, falling)
//!   phase 0.75  winter peak     (coldest, dampest)
//! ```
//!
//! Off by default. `UNENDLICH_SEASONS=1` enables the render-tint
//! modulation; `UNENDLICH_TICKS_PER_YEAR=<n>` overrides the year length.

use std::sync::OnceLock;

/// Default ticks per in-game year. At ~20 ticks/sec this is ~1 hour of
/// play per year — long enough that seasons feel ambient, short enough
/// to be observable in a session. Override with `UNENDLICH_TICKS_PER_YEAR`.
pub const DEFAULT_TICKS_PER_YEAR: u64 = 72_000;

/// Peak seasonal temperature swing, on the `[0, 1]` temperature scale.
/// `±0.15` is a clearly visible tint shift without pushing temperate
/// biomes into desert/tundra extremes.
pub const TEMPERATURE_AMPLITUDE: f64 = 0.15;

/// Humidity tracks inversely to temperature (warm → drier), at a gentler
/// amplitude than temperature.
pub const HUMIDITY_AMPLITUDE: f64 = 0.06;

/// The four seasons, derived from year phase. Quarter-cycle each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

/// Process-wide "seasons enabled" gate. Read once from
/// `UNENDLICH_SEASONS`. Default off → render tint is byte-identical to
/// the pre-seasons behaviour.
pub fn seasons_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("UNENDLICH_SEASONS")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}

/// Ticks per year, read once from `UNENDLICH_TICKS_PER_YEAR` (falls back
/// to [`DEFAULT_TICKS_PER_YEAR`]). A zero or unparseable value falls back
/// to the default so we never divide by zero.
pub fn ticks_per_year() -> u64 {
    static TPY: OnceLock<u64> = OnceLock::new();
    *TPY.get_or_init(|| {
        std::env::var("UNENDLICH_TICKS_PER_YEAR")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_TICKS_PER_YEAR)
    })
}

/// Year phase in `[0, 1)` for a given tick and year length. Pure — takes
/// the year length explicitly so it's testable without env.
pub fn year_phase_with(tick: u64, ticks_per_year: u64) -> f64 {
    debug_assert!(ticks_per_year > 0);
    (tick % ticks_per_year) as f64 / ticks_per_year as f64
}

/// Year phase in `[0, 1)` using the configured year length.
pub fn year_phase(tick: u64) -> f64 {
    year_phase_with(tick, ticks_per_year())
}

/// The season for a given year phase.
pub fn season_for_phase(phase: f64) -> Season {
    // phase is expected in [0, 1); be defensive about rounding.
    let p = phase.rem_euclid(1.0);
    match (p * 4.0) as u32 {
        0 => Season::Spring,
        1 => Season::Summer,
        2 => Season::Autumn,
        _ => Season::Winter,
    }
}

/// Seasonal temperature delta for a year phase, in `[-A, +A]` where
/// `A = TEMPERATURE_AMPLITUDE`. Peaks at summer (phase 0.25), troughs at
/// winter (phase 0.75); continuous across the year boundary (it's a sine).
pub fn temperature_delta(phase: f64) -> f64 {
    TEMPERATURE_AMPLITUDE * (std::f64::consts::TAU * phase).sin()
}

/// Seasonal humidity delta. Inversely tracks temperature at a gentler
/// amplitude — warm summers read slightly drier, cold winters damper.
pub fn humidity_delta(phase: f64) -> f64 {
    -HUMIDITY_AMPLITUDE * (std::f64::consts::TAU * phase).sin()
}

/// Convenience for the render-tint site: `(temperature_delta,
/// humidity_delta)` for the current tick using the configured year
/// length. Apply on top of the base sampled climate, then clamp to
/// `[0, 1]`.
pub fn render_climate_delta(tick: u64) -> (f64, f64) {
    let phase = year_phase(tick);
    (temperature_delta(phase), humidity_delta(phase))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TPY: u64 = 1000; // small year for crisp test arithmetic

    #[test]
    fn year_phase_wraps_to_unit_interval() {
        assert_eq!(year_phase_with(0, TPY), 0.0);
        assert_eq!(year_phase_with(TPY / 4, TPY), 0.25);
        assert_eq!(year_phase_with(TPY, TPY), 0.0); // full year wraps
        assert_eq!(year_phase_with(TPY + TPY / 2, TPY), 0.5); // 1.5 years
        // Always in [0, 1)
        for t in [0u64, 1, 333, 999, 1000, 123_456] {
            let p = year_phase_with(t, TPY);
            assert!((0.0..1.0).contains(&p), "phase {} out of range for tick {}", p, t);
        }
    }

    #[test]
    fn summer_is_warmer_than_winter() {
        let summer = temperature_delta(0.25);
        let winter = temperature_delta(0.75);
        assert!(summer > 0.0, "summer delta should be positive, got {}", summer);
        assert!(winter < 0.0, "winter delta should be negative, got {}", winter);
        assert!(summer > winter);
        // Spring/autumn are the neutral crossings.
        assert!(temperature_delta(0.0).abs() < 1e-9);
        assert!(temperature_delta(0.5).abs() < 1e-9);
    }

    #[test]
    fn temperature_delta_is_bounded() {
        for i in 0..=1000 {
            let phase = i as f64 / 1000.0;
            let d = temperature_delta(phase);
            assert!(d.abs() <= TEMPERATURE_AMPLITUDE + 1e-9,
                "temp delta {} exceeds amplitude at phase {}", d, phase);
        }
    }

    #[test]
    fn humidity_inversely_tracks_temperature() {
        // At summer peak, temp is up and humidity down.
        assert!(temperature_delta(0.25) > 0.0);
        assert!(humidity_delta(0.25) < 0.0);
        // At winter peak, temp down and humidity up.
        assert!(temperature_delta(0.75) < 0.0);
        assert!(humidity_delta(0.75) > 0.0);
    }

    #[test]
    fn season_landmarks_map_correctly() {
        assert_eq!(season_for_phase(0.0), Season::Spring);
        assert_eq!(season_for_phase(0.10), Season::Spring);
        assert_eq!(season_for_phase(0.25), Season::Summer);
        assert_eq!(season_for_phase(0.49), Season::Summer);
        assert_eq!(season_for_phase(0.50), Season::Autumn);
        assert_eq!(season_for_phase(0.75), Season::Winter);
        assert_eq!(season_for_phase(0.99), Season::Winter);
        // Defensive wrap.
        assert_eq!(season_for_phase(1.0), Season::Spring);
        assert_eq!(season_for_phase(1.25), Season::Summer);
    }

    #[test]
    fn delta_is_continuous_across_year_boundary() {
        // End-of-year and start-of-year deltas should nearly match
        // (no discontinuity / popping when the year wraps).
        let end = temperature_delta(year_phase_with(TPY - 1, TPY));
        let start = temperature_delta(year_phase_with(0, TPY));
        assert!((end - start).abs() < 0.01,
            "year-boundary discontinuity: end={} start={}", end, start);
    }
}
