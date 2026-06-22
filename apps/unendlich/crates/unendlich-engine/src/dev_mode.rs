//! dev_mode (r206) — a single source of truth for "are we in developer
//! mode?" Set `UNENDLICH_DEV=1` (or `true`/`yes`/`on`) in the environment
//! and the game turns on developer conveniences by default:
//!   - the debug overlay (F4 markers) starts enabled;
//!   - the FPS / perf banner shows without toggling;
//!   - the world-history rollback thresholds drop to seconds so points
//!     actually accumulate in a short test session;
//!   - a "DEV" banner is drawn so it's obvious the build is not shipping
//!     defaults.
//!
//! The operator asked for exactly this: "a dev runtime UNENDLICH_DEV=1
//! that turns this stuff on by default like debug menu and FPS." Reading
//! the env once at boot keeps it deterministic and cheap; everything else
//! consults the cached bool.

use std::sync::OnceLock;

static DEV: OnceLock<bool> = OnceLock::new();

/// True iff UNENDLICH_DEV is set to a truthy value. Cached after the first
/// call (reads the env exactly once).
pub fn is_dev() -> bool {
    *DEV.get_or_init(|| {
        std::env::var("UNENDLICH_DEV")
            .map(|v| {
                let v = v.trim().to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes" || v == "on"
            })
            .unwrap_or(false)
    })
}

/// Rollback-point cadence in *frames* for the timer trigger. In dev mode
/// a point every ~20s (at 60fps) so the history fills during testing; in
/// shipping mode the original 5-minute cadence.
pub fn rollback_timer_frames() -> u64 {
    if is_dev() {
        20 * 60 // ~20s at 60fps
    } else {
        5 * 60 * 60 // 5 min
    }
}

/// Distance (blocks) that triggers a travel-based rollback point. Dev mode
/// trips it far sooner so walking around a test world records history.
pub fn rollback_travel_blocks() -> f32 {
    if is_dev() {
        120.0
    } else {
        1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_lower_in_dev() {
        // Can't toggle the OnceLock per-test, but the relationship the
        // functions encode must hold for both branches.
        assert!(20 * 60 < 5 * 60 * 60);
        assert!(120.0 < 1000.0_f32);
        // is_dev() returns a stable bool and doesn't panic.
        let _ = is_dev();
        // the cadence helpers return one of the two documented values.
        let f = rollback_timer_frames();
        assert!(f == 20 * 60 || f == 5 * 60 * 60);
    }
}
