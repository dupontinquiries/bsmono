//! `sim::mob_bands` — band-aware mob tick decision (T4).
//!
//! Mirrors `world::leaf_decay`'s catch-up shape for live roamers: a mob's chunk
//! is classified by `world::scheduler::band_for_chunk(coord, player, params)`,
//! whose `ChunkTickBand::divisor(params)` gives the tick cadence — 1 for Render
//! (every tick), `simulation_divisor` for Simulation, 0 for CatchUp. This module
//! turns that cadence + how many ticks since the mob last ran into the decision:
//! tick now? and if so, how many elapsed ticks to integrate (so a Simulation-band
//! mob's movement/forage catches up the gap, like `leaf_decay::advance_n_ticks`).
//!
//! Kept dep-free (takes the already-computed `divisor`) so it's unit-testable
//! without the scheduler/world types or the bin. Pure, deterministic.

/// The per-tick decision for one mob, given its band `divisor` (from
/// `ChunkTickBand::divisor(params)`) and how many game-ticks have elapsed since
/// it last ran a full tick.
///
/// - `divisor == 0` (CatchUp / beyond simulation distance) → `None`: dormant,
///   skip entirely this frame.
/// - `divisor == 1` (Render band) → `Some(1)`: tick every frame, no catch-up.
/// - `divisor >= 2` (Simulation band) → tick only once the gap reaches the
///   divisor, then `Some(divisor)` so the handler integrates the whole skipped
///   interval at once (the catch-up `n` to pass to movement/forage advance).
#[inline]
pub fn mob_band_tick(divisor: u64, ticks_since_last: u64) -> Option<u32> {
    if divisor == 0 {
        None
    } else if divisor == 1 {
        Some(1)
    } else if ticks_since_last >= divisor {
        Some(saturating_u32(divisor))
    } else {
        None
    }
}

/// Convenience name for a band in the per-band count log the bin emits once.
/// `0 → "catchup"`, `1 → "render"`, else `"simulation"`.
pub fn band_label(divisor: u64) -> &'static str {
    match divisor {
        0 => "catchup",
        1 => "render",
        _ => "simulation",
    }
}

#[inline]
fn saturating_u32(v: u64) -> u32 {
    if v > u32::MAX as u64 {
        u32::MAX
    } else {
        v as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_band_ticks_every_frame_no_catchup() {
        // divisor 1 → always tick, integrating exactly one tick.
        for since in 0..5 {
            assert_eq!(mob_band_tick(1, since), Some(1));
        }
    }

    #[test]
    fn catchup_band_is_dormant() {
        for since in 0..100 {
            assert_eq!(mob_band_tick(0, since), None, "CatchUp band must skip");
        }
    }

    #[test]
    fn simulation_band_fires_every_divisor_with_catchup_n() {
        let div = 8u64;
        // skipped until the gap reaches the divisor …
        for since in 0..div {
            assert_eq!(mob_band_tick(div, since), None, "should skip at gap {since}");
        }
        // … then fires, asking to integrate the whole 8-tick interval.
        assert_eq!(mob_band_tick(div, div), Some(8));
        assert_eq!(mob_band_tick(div, div + 3), Some(8));
    }

    #[test]
    fn simulation_band_total_work_is_capped() {
        // Over a window of W frames, a Simulation mob (divisor D) runs ~W/D full
        // ticks vs a Render mob's W — the perf win, asserted as a count.
        let w = 64u64;
        let d = 8u64;
        let render_ticks = (0..w).filter(|&t| mob_band_tick(1, t % 1).is_some()).count();
        // simulate the gap counter resetting on each fire
        let mut gap = 0u64;
        let mut sim_ticks = 0;
        for _ in 0..w {
            gap += 1;
            if let Some(_n) = mob_band_tick(d, gap) {
                sim_ticks += 1;
                gap = 0;
            }
        }
        assert_eq!(render_ticks, w as usize);
        assert!(sim_ticks <= (w / d) as usize + 1, "sim work capped: {sim_ticks}");
        assert!(sim_ticks < render_ticks, "simulation band does less work");
    }

    #[test]
    fn catch_up_n_sums_to_elapsed_time() {
        // Firing every D ticks with catch-up n=D integrates the same total ticks
        // as a Render mob over the same window (time-homogeneity, like leaf_decay).
        let d = 4u64;
        let frames = 40u64;
        let mut gap = 0u64;
        let mut integrated = 0u64;
        for _ in 0..frames {
            gap += 1;
            if let Some(n) = mob_band_tick(d, gap) {
                integrated += n as u64;
                gap = 0;
            }
        }
        // frames divisible by d → exact catch-up
        assert_eq!(integrated, frames, "catch-up must cover every elapsed tick");
    }

    #[test]
    fn labels() {
        assert_eq!(band_label(0), "catchup");
        assert_eq!(band_label(1), "render");
        assert_eq!(band_label(8), "simulation");
    }
}
