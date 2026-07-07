//! sim::refresh_snap — autotune target + frame-rate snap ladder (r604).
//!
//! The frame autotuner should aim as high as **500 Hz** and settle the present
//! rate onto a small set of stable rungs rather than dithering every frame. This
//! module encodes that policy as pure data + helpers so the tuner (and tests)
//! share one source of truth: the target is [`AUTOTUNE_TARGET_HZ`] and the rungs
//! are [`SNAP_LADDER`]. `snap_at_or_below` picks the highest rung the machine
//! can actually sustain (never promise more than measured); `nearest_snap`
//! rounds to the closest rung. Pure, std-only, deterministic.

/// The ceiling the autotuner aims for.
pub const AUTOTUNE_TARGET_HZ: u32 = 500;

/// The present-rate rungs the tuner snaps onto, ascending. 500 is the target
/// cap; the lower rungs are common high-refresh panel rates.
pub const SNAP_LADDER: [u32; 5] = [120, 240, 360, 480, 500];

/// Snap an achievable frame rate DOWN to the highest ladder rung it can hold, so
/// the tuner never targets more than the machine sustains. Below the lowest rung
/// it returns the lowest rung (that's the floor the ladder offers; the tuner may
/// drop resolution/quality separately to reach it). Capped at the target.
pub fn snap_at_or_below(achievable_fps: f32) -> u32 {
    let f = achievable_fps.max(0.0);
    let mut chosen = SNAP_LADDER[0];
    for &rung in SNAP_LADDER.iter() {
        if (rung as f32) <= f {
            chosen = rung;
        } else {
            break;
        }
    }
    chosen.min(AUTOTUNE_TARGET_HZ)
}

/// Snap to the nearest rung (ties round up), capped at the target.
pub fn nearest_snap(fps: f32) -> u32 {
    let f = fps.max(0.0);
    let mut best = SNAP_LADDER[0];
    let mut best_d = (best as f32 - f).abs();
    for &rung in SNAP_LADDER.iter().skip(1) {
        let d = (rung as f32 - f).abs();
        if d <= best_d {
            best = rung;
            best_d = d;
        }
    }
    best.min(AUTOTUNE_TARGET_HZ)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_and_ladder_are_the_requested_values() {
        assert_eq!(AUTOTUNE_TARGET_HZ, 500);
        assert_eq!(SNAP_LADDER, [120, 240, 360, 480, 500]);
    }

    #[test]
    fn snap_down_holds_only_what_the_machine_sustains() {
        assert_eq!(snap_at_or_below(500.0), 500);
        assert_eq!(snap_at_or_below(499.9), 480, "just under 500 ⇒ hold 480");
        assert_eq!(snap_at_or_below(365.0), 360);
        assert_eq!(snap_at_or_below(240.0), 240);
        assert_eq!(snap_at_or_below(119.0), 120, "below the floor ⇒ lowest rung");
        assert_eq!(snap_at_or_below(9999.0), 500, "capped at the target");
    }

    #[test]
    fn nearest_rounds_to_the_closest_rung() {
        assert_eq!(nearest_snap(130.0), 120);
        assert_eq!(nearest_snap(300.0), 360, "ties round up (300 is 60 from both 240 and 360)");
        assert_eq!(nearest_snap(470.0), 480);
        assert_eq!(nearest_snap(600.0), 500, "capped");
    }

    #[test]
    fn snapping_is_deterministic() {
        for &f in &[100.0_f32, 241.0, 359.0, 481.0, 500.0] {
            assert_eq!(snap_at_or_below(f), snap_at_or_below(f));
            assert_eq!(nearest_snap(f), nearest_snap(f));
        }
    }
}
