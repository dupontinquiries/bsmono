//! Head & neck scanning — sweep patterns that widen a dragon's effective vision.
//!
//! A dragon that holds its head still only sees its forward cone; one that
//! sweeps its head/neck covers far more ground over a few seconds. This core
//! produces the head yaw/pitch (and a neck bend that follows the yaw) for a
//! scanning sweep whose width and speed grow with alertness, plus the vision
//! coverage multiplier that sweep earns.
//!
//! Pure and deterministic — the caller passes the time.

/// Head pose for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanState {
    /// Head yaw relative to the body heading (rad).
    pub yaw: f32,
    /// Head pitch — a gentle up/down bob (rad).
    pub pitch: f32,
    /// Neck bend that follows the yaw, so the whole neck curves into the look.
    pub neck_bend: f32,
}

/// Max sweep half-angle (rad) at a given alertness — calm dragons barely look
/// around, alarmed ones sweep wide.
fn sweep_amp(alertness01: f32) -> f32 {
    let a = alertness01.clamp(0.0, 1.0);
    0.25 + a * 1.05 // ~14° calm → ~74° alarmed
}

/// Sweep angular speed (rad/s) — faster when alert.
fn sweep_speed(alertness01: f32) -> f32 {
    let a = alertness01.clamp(0.0, 1.0);
    0.8 + a * 2.2
}

/// Head/neck pose for the scan at time `t` seconds, around a `base_yaw` (e.g.
/// toward a point of interest). Alertness widens and speeds the sweep.
pub fn scan(t: f32, alertness01: f32, base_yaw: f32) -> ScanState {
    let amp = sweep_amp(alertness01);
    let w = sweep_speed(alertness01);
    let yaw = base_yaw + amp * (w * t).sin();
    // A slower vertical bob, a third of the horizontal amplitude.
    let pitch = (amp * 0.33) * (w * 0.5 * t + 1.7).sin();
    // The neck lags the head slightly, curving into the sweep.
    let neck_bend = 0.6 * (yaw - base_yaw);
    ScanState { yaw, pitch, neck_bend }
}

/// Effective vision multiplier from sweeping: a wider sweep covers more area
/// over time, so perception range/area scales up. Always `>= 1`.
pub fn vision_gain(alertness01: f32) -> f32 {
    // Coverage roughly tracks the swept arc width.
    1.0 + sweep_amp(alertness01) * 0.9
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alertness_widens_and_quickens_the_sweep() {
        // Sample the peak of the sine for each alertness.
        let calm_amp = sweep_amp(0.0);
        let alarmed_amp = sweep_amp(1.0);
        assert!(alarmed_amp > calm_amp);
        assert!(sweep_speed(1.0) > sweep_speed(0.0));
    }

    #[test]
    fn vision_gain_rises_with_alertness_and_is_at_least_one() {
        assert!(vision_gain(0.0) >= 1.0);
        assert!(vision_gain(1.0) > vision_gain(0.0));
    }

    #[test]
    fn yaw_stays_within_amplitude_of_base() {
        let amp = sweep_amp(0.5);
        for i in 0..200 {
            let t = i as f32 * 0.05;
            let s = scan(t, 0.5, 0.3);
            assert!((s.yaw - 0.3).abs() <= amp + 1e-4);
        }
    }

    #[test]
    fn neck_follows_head() {
        let s = scan(0.4, 1.0, 0.0);
        // Neck bends the same direction as the head yaw, but less.
        assert!(s.neck_bend.signum() == (s.yaw).signum() || s.yaw.abs() < 1e-6);
        assert!(s.neck_bend.abs() <= s.yaw.abs() + 1e-6);
    }

    #[test]
    fn periodic_and_deterministic() {
        assert_eq!(scan(1.23, 0.7, 0.1), scan(1.23, 0.7, 0.1));
    }
}
