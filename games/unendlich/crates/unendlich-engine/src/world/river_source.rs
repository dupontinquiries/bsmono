//! `river_source` — weather-fed water source with a replenish cap.
//!
//! Worker-03 / WORLD-PERF pure core, round 5. Converts a rain rate into a
//! bounded replenish for a river/spring source so rivers stay fed during rain
//! without building infinite pressure (the gameplay-friendly water option):
//! replenish is a fraction of the rain rate, capped, and the source level is
//! clamped to a finite capacity. Pairs with `water_cell` (the source feeds a
//! tagged source cell) and a weather/rain input.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Source tuning.
#[derive(Clone, Copy, Debug)]
pub struct SourceParams {
    /// Fraction of the rain rate captured as replenish (clamped to [0,1]).
    pub efficiency: f32,
    /// Hard cap on replenish per tick.
    pub max_rate: f32,
    /// Maximum stored level (prevents unbounded pressure).
    pub capacity: f32,
}

impl Default for SourceParams {
    fn default() -> Self {
        SourceParams {
            efficiency: 0.5,
            max_rate: 4.0,
            capacity: 64.0,
        }
    }
}

/// Replenish this tick from `rain_rate`: `min(rain_rate * efficiency, max_rate)`,
/// never negative. Because `efficiency ≤ 1`, the result never exceeds
/// `rain_rate`, and it never exceeds `max_rate`.
pub fn replenish(rain_rate: f32, p: &SourceParams) -> f32 {
    let rain = nonneg(rain_rate);
    let eff = clamp01(p.efficiency);
    let cap = nonneg(p.max_rate);
    let by_rain = rain * eff;
    if by_rain < cap {
        by_rain
    } else {
        cap
    }
}

/// Advance a source level: add the rain-driven replenish, subtract `outflow`,
/// and clamp to `[0, capacity]`. The level never exceeds capacity (no infinite
/// pressure) and never goes negative.
pub fn step(level: f32, rain_rate: f32, outflow: f32, p: &SourceParams) -> f32 {
    let cap = nonneg(p.capacity);
    let next = nonneg(level) + replenish(rain_rate, p) - nonneg(outflow);
    if next.is_nan() || next < 0.0 {
        0.0
    } else if next > cap {
        cap
    } else {
        next
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

fn nonneg(v: f32) -> f32 {
    if v.is_nan() || v < 0.0 {
        0.0
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_replenish_bounded_by_rain_and_cap() {
        let p = SourceParams::default();
        let mut rain = 0.0f32;
        while rain <= 50.0 {
            let r = replenish(rain, &p);
            assert!(r >= 0.0);
            assert!(r <= rain + 1e-6, "replenish exceeded rain rate");
            assert!(r <= p.max_rate + 1e-6, "replenish exceeded cap");
            rain += 0.5;
        }
    }

    #[test]
    fn efficiency_one_still_bounded_by_rain() {
        let p = SourceParams { efficiency: 1.0, max_rate: 1e9, capacity: 1e9 };
        for rain in [0.0, 1.0, 5.0, 100.0] {
            assert!((replenish(rain, &p) - rain).abs() < 1e-4);
        }
    }

    #[test]
    fn prop_level_never_exceeds_capacity_or_goes_negative() {
        let p = SourceParams::default();
        let mut level = 0.0f32;
        // Heavy rain, no outflow → fills to capacity and stays.
        for _ in 0..1000 {
            level = step(level, 100.0, 0.0, &p);
            assert!(level >= 0.0 && level <= p.capacity + 1e-6);
        }
        assert!((level - p.capacity).abs() < 1e-3);
        // Heavy outflow, no rain → drains to zero, never negative.
        for _ in 0..1000 {
            level = step(level, 0.0, 100.0, &p);
            assert!(level >= 0.0);
        }
        assert_eq!(level, 0.0);
    }

    #[test]
    fn deterministic() {
        let p = SourceParams::default();
        assert_eq!(replenish(3.3, &p), replenish(3.3, &p));
        assert_eq!(step(10.0, 2.0, 1.0, &p), step(10.0, 2.0, 1.0, &p));
    }

    #[test]
    fn negative_and_nan_inputs_are_safe() {
        let p = SourceParams::default();
        assert_eq!(replenish(-5.0, &p), 0.0);
        assert!(step(f32::NAN, f32::NAN, f32::NAN, &p).is_finite());
    }
}
