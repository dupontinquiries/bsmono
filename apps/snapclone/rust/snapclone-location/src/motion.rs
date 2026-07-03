//! Motion modelling — projecting a peer's likely position between fixes.
//!
//! The wayfinder wants to show roughly where a peer is NOW, not only where they
//! were at their last GPS fix. The honest way to do that uses the SENDER's own
//! motion sensors: the sender classifies their activity and either projects
//! their position forward or reports they're stationary, and that enriched state
//! travels with their location. (A receiver cannot sense a remote peer's
//! movement — dead reckoning is self-tracking. So this is a peer-cooperative
//! enrichment, computed sender-side.)
//!
//! ## Why not "double-integrate the accelerometer"
//! Position from raw acceleration is a DOUBLE integral, so any constant bias
//! grows the error as ½·b·t² — a cheap phone's accel drifts tens of metres in
//! under a minute. Phones never navigate that way. The tractable approaches are
//! mode-dependent:
//!   - **Foot** → pedestrian dead reckoning (PDR): count steps × stride along a
//!     heading. Error is bounded *per step* (~linear in distance), not quadratic
//!     in time.
//!   - **Vehicle** → speed is roughly constant between fixes; project along the
//!     last heading at the last known speed. Error dominated by unseen turns.
//!   - **Stationary** → don't move the dot at all (most peers, most of the time).
//!
//! This module is two pure pieces: [`classify_activity`] (still/foot/vehicle
//! from accelerometer summary stats — activity recognition, which IS tractable
//! from cadence + acceleration variance) and [`project`] (mode-dependent forward
//! projection that also grows an honest uncertainty radius). No live sensors, no
//! Qt — the device layer samples the accelerometer and fills [`MotionSummary`];
//! the schema/transport carries [`MotionState`]; this decides the rest.

use crate::haversine;

/// Coarse activity mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Not moving (or only sensor noise). Don't project.
    Stationary,
    /// On foot — walking/running. Project by step count × stride.
    OnFoot,
    /// In a vehicle — project by constant velocity along heading.
    Vehicle,
}

/// A summary of a short accelerometer window, computed device-side. These are
/// the statistics activity recognition actually keys on — not raw samples.
#[derive(Debug, Clone, Copy)]
pub struct MotionSummary {
    /// Std-dev of linear-acceleration magnitude over the window (m/s²). Near 0 =
    /// still; moderate = walking; can be high transient for vehicles on rough
    /// roads but typically lower sustained variance than footfall.
    pub accel_std: f32,
    /// Dominant cadence in steps/min from the accel autocorrelation (0 if none).
    /// Walking ≈ 90–130, running ≈ 150–200, vehicle ≈ 0 (no footfall periodicity).
    pub cadence_spm: f32,
    /// Mean speed estimate over the window (m/s), if the device has one (e.g.
    /// from GPS doppler or wheel/OS speed). <0 if unknown.
    pub speed_mps: f32,
}

/// Thresholds for classification (tunable; device/config can override).
#[derive(Debug, Clone, Copy)]
pub struct ActivityPolicy {
    /// accel_std below this → Stationary.
    pub still_accel_std: f32,
    /// cadence within [foot_cadence_min, foot_cadence_max] → OnFoot.
    pub foot_cadence_min: f32,
    pub foot_cadence_max: f32,
    /// speed above this (m/s) with no footfall cadence → Vehicle (~ >7 m/s = 25km/h).
    pub vehicle_speed_mps: f32,
}

impl Default for ActivityPolicy {
    fn default() -> Self {
        ActivityPolicy {
            still_accel_std: 0.15,
            foot_cadence_min: 70.0,
            foot_cadence_max: 220.0,
            vehicle_speed_mps: 7.0,
        }
    }
}

/// Classify the activity from a motion summary. Order: stillness first (cheap +
/// most common), then footfall cadence (the strong PDR signal), then vehicle by
/// speed, else fall back to still (we'd rather under-project than invent motion).
pub fn classify_activity(policy: &ActivityPolicy, m: &MotionSummary) -> Activity {
    if m.accel_std < policy.still_accel_std && m.cadence_spm < policy.foot_cadence_min {
        return Activity::Stationary;
    }
    if m.cadence_spm >= policy.foot_cadence_min && m.cadence_spm <= policy.foot_cadence_max {
        return Activity::OnFoot;
    }
    if m.speed_mps >= policy.vehicle_speed_mps {
        return Activity::Vehicle;
    }
    // Moving but no clear footfall and not fast → conservatively don't invent a
    // direction; treat as stationary for projection purposes.
    Activity::Stationary
}

/// The motion state a sender attaches to their position so a receiver can
/// project. All fields are the SENDER's own self-measured values.
#[derive(Debug, Clone, Copy)]
pub struct MotionState {
    pub activity: Activity,
    /// Heading in degrees (0 = north, clockwise). Compass/gyro-derived.
    pub heading_deg: f32,
    /// For OnFoot: steps accumulated since the fix. For Vehicle: unused.
    pub steps: u32,
    /// Stride length in metres (per-user calibratable; ~0.7 default).
    pub stride_m: f32,
    /// For Vehicle: speed in m/s at the fix.
    pub speed_mps: f32,
}

/// A projected position with an honest uncertainty radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub lat: f64,
    pub lon: f64,
    /// 1-sigma uncertainty radius in metres — GROWS with projected distance and
    /// elapsed time. The wayfinder should widen the node's halo by this.
    pub uncertainty_m: f32,
    /// How far we projected (m). 0 = didn't move the dot.
    pub projected_m: f32,
}

/// Default per-step PDR error fraction (~8% of distance walked).
pub const PDR_ERROR_FRACTION: f32 = 0.08;
/// Vehicle projection error fraction (turns we can't see) — higher, ~20%.
pub const VEHICLE_ERROR_FRACTION: f32 = 0.20;

/// Project a peer's position forward from their last fix using their motion
/// state and the time elapsed since the fix. `base_accuracy_m` is the fix's own
/// error radius (the projection uncertainty is added on top in quadrature).
pub fn project(
    base_lat: f64,
    base_lon: f64,
    base_accuracy_m: f32,
    motion: &MotionState,
    elapsed_secs: f64,
) -> Projection {
    let dist_m: f64 = match motion.activity {
        Activity::Stationary => 0.0,
        Activity::OnFoot => (motion.steps as f64) * (motion.stride_m.max(0.1) as f64),
        Activity::Vehicle => (motion.speed_mps.max(0.0) as f64) * elapsed_secs.max(0.0),
    };

    if dist_m <= 0.0 {
        // No projection — return the fix as-is, uncertainty = the fix's own.
        return Projection {
            lat: base_lat,
            lon: base_lon,
            uncertainty_m: base_accuracy_m.max(0.0),
            projected_m: 0.0,
        };
    }

    let (plat, plon) = haversine::destination(base_lat, base_lon, motion.heading_deg as f64, dist_m);

    let proj_err_frac = match motion.activity {
        Activity::OnFoot => PDR_ERROR_FRACTION,
        Activity::Vehicle => VEHICLE_ERROR_FRACTION,
        Activity::Stationary => 0.0,
    };
    let proj_err = (dist_m as f32) * proj_err_frac;
    // Combine the fix's own error and the projection error in quadrature.
    let base = base_accuracy_m.max(0.0);
    let uncertainty = (base * base + proj_err * proj_err).sqrt();

    Projection {
        lat: plat,
        lon: plon,
        uncertainty_m: uncertainty,
        projected_m: dist_m as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> ActivityPolicy { ActivityPolicy::default() }

    // ── classification ──────────────────────────────────────────────────────

    #[test]
    fn still_when_quiet() {
        let m = MotionSummary { accel_std: 0.05, cadence_spm: 0.0, speed_mps: 0.0 };
        assert_eq!(classify_activity(&pol(), &m), Activity::Stationary);
    }

    #[test]
    fn walking_cadence_is_foot() {
        let m = MotionSummary { accel_std: 1.2, cadence_spm: 110.0, speed_mps: 1.4 };
        assert_eq!(classify_activity(&pol(), &m), Activity::OnFoot);
    }

    #[test]
    fn running_cadence_is_foot() {
        let m = MotionSummary { accel_std: 3.0, cadence_spm: 175.0, speed_mps: 3.5 };
        assert_eq!(classify_activity(&pol(), &m), Activity::OnFoot);
    }

    #[test]
    fn fast_no_cadence_is_vehicle() {
        // 15 m/s (~54 km/h), no footfall periodicity → vehicle.
        let m = MotionSummary { accel_std: 0.4, cadence_spm: 0.0, speed_mps: 15.0 };
        assert_eq!(classify_activity(&pol(), &m), Activity::Vehicle);
    }

    #[test]
    fn moving_slow_no_cadence_stays_stationary() {
        // Drifting sensor noise / shuffling that isn't clear footfall and isn't
        // fast → don't invent a direction.
        let m = MotionSummary { accel_std: 0.3, cadence_spm: 30.0, speed_mps: 2.0 };
        assert_eq!(classify_activity(&pol(), &m), Activity::Stationary);
    }

    // ── projection ──────────────────────────────────────────────────────────

    #[test]
    fn stationary_does_not_move_the_dot() {
        let motion = MotionState {
            activity: Activity::Stationary, heading_deg: 90.0,
            steps: 0, stride_m: 0.7, speed_mps: 0.0,
        };
        let p = project(45.0, -73.0, 12.0, &motion, 60.0);
        assert_eq!(p.projected_m, 0.0);
        assert_eq!(p.lat, 45.0);
        assert_eq!(p.lon, -73.0);
        assert_eq!(p.uncertainty_m, 12.0); // just the fix's own error
    }

    #[test]
    fn foot_projects_by_steps_and_grows_uncertainty() {
        // 100 steps × 0.7 m = 70 m east.
        let motion = MotionState {
            activity: Activity::OnFoot, heading_deg: 90.0,
            steps: 100, stride_m: 0.7, speed_mps: 0.0,
        };
        let p = project(45.0, -73.0, 10.0, &motion, 60.0);
        assert!((p.projected_m - 70.0).abs() < 0.5);
        // moved roughly east → lon increased, lat ~unchanged
        assert!(p.lon > -73.0);
        assert!((p.lat - 45.0).abs() < 0.001);
        // uncertainty grew beyond the base 10m (PDR error on 70m ≈ 5.6m, in
        // quadrature with 10 → ~11.5m).
        assert!(p.uncertainty_m > 10.0 && p.uncertainty_m < 14.0);
    }

    #[test]
    fn vehicle_projects_by_speed_time_with_bigger_uncertainty() {
        // 20 m/s for 30 s = 600 m north.
        let motion = MotionState {
            activity: Activity::Vehicle, heading_deg: 0.0,
            steps: 0, stride_m: 0.0, speed_mps: 20.0,
        };
        let p = project(45.0, -73.0, 10.0, &motion, 30.0);
        assert!((p.projected_m - 600.0).abs() < 1.0);
        assert!(p.lat > 45.0); // north → lat increased
        // vehicle error fraction is higher: 20% of 600 = 120m, dominates.
        assert!(p.uncertainty_m > 100.0);
    }

    #[test]
    fn uncertainty_grows_with_distance() {
        let near = MotionState { activity: Activity::Vehicle, heading_deg: 0.0,
            steps: 0, stride_m: 0.0, speed_mps: 20.0 };
        let p_short = project(45.0, -73.0, 10.0, &near, 5.0);
        let p_long  = project(45.0, -73.0, 10.0, &near, 60.0);
        assert!(p_long.uncertainty_m > p_short.uncertainty_m,
                "longer projection must be less certain");
    }

    #[test]
    fn negative_elapsed_and_speed_are_safe() {
        let motion = MotionState { activity: Activity::Vehicle, heading_deg: 0.0,
            steps: 0, stride_m: 0.0, speed_mps: -5.0 };
        let p = project(45.0, -73.0, 10.0, &motion, -30.0);
        assert_eq!(p.projected_m, 0.0); // clamped, no projection
    }
}
