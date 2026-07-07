//! `sim::mountain_sources` — mountain water sources fed by weather humidity (orographic
//! precipitation), stepped across worker threads.
//!
//! Humid air pushed up high terrain cools and drops its moisture, so rainfall rises
//! with **both humidity and altitude** above a condensation height — this is why
//! mountains feed springs and headwaters. [`orographic_rain`] is that coupling (the
//! new pure core); the resulting rain rate feeds the existing, tested
//! [`river_source`](crate::world::river_source) replenishment so each source level
//! stays bounded. Sources are independent cells, so stepping a whole field of them is
//! a **Class-P** parallel map over the rayon lane
//! ([`util::task_lanes::map_pure`](crate::util::task_lanes)) — order-preserving and
//! identical to the serial result.

use crate::util::task_lanes::{map_pure, map_pure_threshold};
use crate::world::river_source::{self, SourceParams};

/// Tuning for orographic (mountain) precipitation.
#[derive(Clone, Copy, Debug)]
pub struct OrographicParams {
    /// Background rain rate at/below the condensation altitude.
    pub base_rain: f32,
    /// Rain added per unit of `humidity * (altitude − condensation)`.
    pub lift_gain: f32,
    /// Normalized altitude `[0,1]` above which orographic lift begins.
    pub condensation_alt: f32,
}

impl Default for OrographicParams {
    fn default() -> Self {
        OrographicParams { base_rain: 0.2, lift_gain: 6.0, condensation_alt: 0.4 }
    }
}

/// Orographic rain rate at a cell from `humidity01` and normalized `altitude01`
/// (both clamped to `[0,1]`). Monotonically non-decreasing in both above the
/// condensation altitude; never negative.
pub fn orographic_rain(humidity01: f32, altitude01: f32, p: &OrographicParams) -> f32 {
    let hum = humidity01.clamp(0.0, 1.0);
    let alt = altitude01.clamp(0.0, 1.0);
    let lift = (alt - p.condensation_alt).max(0.0);
    (p.base_rain + p.lift_gain * hum * lift).max(0.0)
}

/// One mountain source cell: local humidity, normalized altitude, and current stored
/// water level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MountainSource {
    pub humidity01: f32,
    pub altitude01: f32,
    pub level: f32,
}

impl MountainSource {
    /// Advance this source one tick: orographic rain → `river_source` replenish −
    /// `outflow`, clamped to capacity.
    pub fn stepped(&self, outflow: f32, oro: &OrographicParams, sp: &SourceParams) -> f32 {
        let rain = orographic_rain(self.humidity01, self.altitude01, oro);
        river_source::step(self.level, rain, outflow, sp)
    }
}

/// Step every source in parallel, returning the new levels in input order.
pub fn step_sources(
    sources: &[MountainSource],
    outflow: f32,
    oro: &OrographicParams,
    sp: &SourceParams,
) -> Vec<f32> {
    map_pure(sources, |s| s.stepped(outflow, oro, sp))
}

/// As [`step_sources`], serial below `min_parallel` sources.
pub fn step_sources_threshold(
    sources: &[MountainSource],
    min_parallel: usize,
    outflow: f32,
    oro: &OrographicParams,
    sp: &SourceParams,
) -> Vec<f32> {
    map_pure_threshold(sources, min_parallel, |s| s.stepped(outflow, oro, sp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rain_rises_with_humidity_and_altitude() {
        let p = OrographicParams::default();
        // higher altitude (above condensation) → more rain
        assert!(orographic_rain(0.8, 0.9, &p) > orographic_rain(0.8, 0.5, &p));
        // higher humidity → more rain
        assert!(orographic_rain(0.9, 0.8, &p) > orographic_rain(0.2, 0.8, &p));
        // below condensation altitude → just the base rain, no lift
        assert_eq!(orographic_rain(1.0, 0.3, &p), p.base_rain);
        // never negative
        assert!(orographic_rain(0.0, 0.0, &p) >= 0.0);
    }

    #[test]
    fn parallel_equals_serial_in_order() {
        let oro = OrographicParams::default();
        let sp = SourceParams::default();
        let sources: Vec<MountainSource> = (0..16)
            .map(|i| MountainSource {
                humidity01: (i as f32 / 16.0),
                altitude01: ((i * 7 % 16) as f32 / 16.0),
                level: i as f32,
            })
            .collect();
        let par = step_sources(&sources, 0.5, &oro, &sp);
        let ser: Vec<f32> = sources.iter().map(|s| s.stepped(0.5, &oro, &sp)).collect();
        assert_eq!(par, ser);
        assert_eq!(step_sources_threshold(&sources, 4, 0.5, &oro, &sp), par);
    }

    #[test]
    fn levels_stay_bounded() {
        let oro = OrographicParams::default();
        let sp = SourceParams::default();
        // a humid mountaintop, run many ticks with no outflow → clamps to capacity
        let mut s = MountainSource { humidity01: 1.0, altitude01: 1.0, level: 0.0 };
        for _ in 0..1000 {
            s.level = s.stepped(0.0, &oro, &sp);
        }
        assert!(s.level <= sp.capacity + 1e-3, "level clamps to capacity ({} <= {})", s.level, sp.capacity);
        assert!(s.level > 0.0, "a humid peak accumulates water");
    }

    #[test]
    fn empty_is_empty() {
        assert!(step_sources(&[], 0.0, &OrographicParams::default(), &SourceParams::default()).is_empty());
    }
}
