//! `sim::dragon_observe` — observational behaviour: where the dragon looks. It
//! attends to the most salient nearby stimulus (turning its head toward it,
//! head-tracking), and idle-scans the surroundings otherwise. Composes
//! `head_scan` (idle sweep) and `movement_tune::smooth_angle` (smooth tracking).
//! Pure, deterministic — the caller supplies time + the stimulus bearings.

use crate::sim::head_scan::{scan, ScanState};
use crate::sim::movement_tune::smooth_angle;

/// A thing the dragon might look at, expressed relative to its body heading.
#[derive(Clone, Copy, Debug)]
pub struct Stimulus {
    /// Bearing relative to the body heading (rad; 0 = straight ahead).
    pub bearing: f32,
    /// How interesting it is (0..1): prey, threat, movement, sound source.
    pub salience: f32,
    pub dist2: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct GazeParams {
    pub alertness01: f32,
    /// How far the head can turn off the body axis (rad).
    pub max_yaw: f32,
    /// Head turn speed as a smoothing half-life (s).
    pub track_half_life: f32,
}

impl Default for GazeParams {
    fn default() -> Self {
        GazeParams { alertness01: 0.3, max_yaw: 1.4, track_half_life: 0.18 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gaze {
    pub yaw: f32,
    pub pitch: f32,
    pub neck_bend: f32,
}

impl Gaze {
    pub fn forward() -> Self {
        Gaze { yaw: 0.0, pitch: 0.0, neck_bend: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ObserveStep {
    pub gaze: Gaze,
    /// Index into the stimulus slice the dragon is attending to, if any.
    pub attending: Option<usize>,
}

/// Pick the most salient stimulus within head-turn range (salience, discounted by
/// distance). Returns its index + the bearing to look at.
fn focus(stimuli: &[Stimulus], max_yaw: f32) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32, f32)> = None; // (idx, score, bearing)
    for (i, s) in stimuli.iter().enumerate() {
        if s.salience <= 0.0 || s.bearing.abs() > max_yaw {
            continue;
        }
        let score = s.salience / (1.0 + s.dist2.max(0.0) * 0.01);
        if best.map(|(_, bs, _)| score > bs).unwrap_or(true) {
            best = Some((i, score, s.bearing));
        }
    }
    best.map(|(i, _, b)| (i, b))
}

/// One observation tick. Smoothly turns the gaze toward the most salient stimulus
/// (locking on, narrow sweep), or idle-scans the surroundings when nothing draws
/// attention.
pub fn observe(p: &GazeParams, gaze: Gaze, stimuli: &[Stimulus], t: f32, dt: f32) -> ObserveStep {
    let max_yaw = p.max_yaw.max(0.0);
    match focus(stimuli, max_yaw) {
        Some((idx, bearing)) => {
            // Lock on: small tracking jitter around the target bearing.
            let lock: ScanState = scan(t, 0.12, bearing.clamp(-max_yaw, max_yaw));
            let target = lock.yaw.clamp(-max_yaw, max_yaw);
            let yaw = smooth_angle(gaze.yaw, target, p.track_half_life, dt);
            let pitch = crate::sim::movement_tune::smooth_damp(gaze.pitch, lock.pitch, p.track_half_life, dt);
            ObserveStep {
                gaze: Gaze { yaw, pitch, neck_bend: 0.6 * yaw },
                attending: Some(idx),
            }
        }
        None => {
            // Idle: sweep around dead-ahead, width/speed from alertness.
            let sweep = scan(t, p.alertness01, 0.0);
            let target = sweep.yaw.clamp(-max_yaw, max_yaw);
            let yaw = smooth_angle(gaze.yaw, target, p.track_half_life, dt);
            let pitch = crate::sim::movement_tune::smooth_damp(gaze.pitch, sweep.pitch, p.track_half_life, dt);
            ObserveStep {
                gaze: Gaze { yaw, pitch, neck_bend: 0.6 * yaw },
                attending: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attends_to_most_salient_in_range() {
        let p = GazeParams::default();
        let stimuli = [
            Stimulus { bearing: 0.5, salience: 0.3, dist2: 100.0 },
            Stimulus { bearing: -0.4, salience: 0.9, dist2: 50.0 }, // most salient, close
            Stimulus { bearing: 3.0, salience: 1.0, dist2: 1.0 },   // behind: out of range
        ];
        let step = observe(&p, Gaze::forward(), &stimuli, 0.0, 0.016);
        assert_eq!(step.attending, Some(1), "locks onto the salient in-range stimulus");
    }

    #[test]
    fn turns_toward_the_target_over_time() {
        let p = GazeParams::default();
        let stimuli = [Stimulus { bearing: 1.0, salience: 0.8, dist2: 20.0 }];
        let mut g = Gaze::forward();
        for i in 0..120 {
            g = observe(&p, g, &stimuli, i as f32 * 0.016, 0.016).gaze;
        }
        assert!(g.yaw > 0.6, "head turns toward the stimulus, got {}", g.yaw);
        assert!((g.neck_bend - 0.6 * g.yaw).abs() < 1e-5, "neck follows the head");
    }

    #[test]
    fn idle_scans_when_nothing_salient() {
        let p = GazeParams { alertness01: 1.0, ..GazeParams::default() };
        let none: [Stimulus; 0] = [];
        let a = observe(&p, Gaze::forward(), &none, 0.0, 0.016);
        let b = observe(&p, a.gaze, &none, 1.5, 0.016);
        assert_eq!(a.attending, None);
        assert!(a.gaze.yaw != b.gaze.yaw || a.gaze != b.gaze, "the head sweeps while idle");
    }

    #[test]
    fn behind_or_dull_stimuli_are_ignored() {
        let p = GazeParams::default();
        let stimuli = [
            Stimulus { bearing: 2.5, salience: 1.0, dist2: 1.0 },  // behind
            Stimulus { bearing: 0.2, salience: 0.0, dist2: 1.0 },  // not salient
        ];
        assert_eq!(observe(&p, Gaze::forward(), &stimuli, 0.0, 0.016).attending, None);
    }

    #[test]
    fn gaze_stays_within_range_and_finite() {
        let p = GazeParams::default();
        let stimuli = [Stimulus { bearing: 1.3, salience: 1.0, dist2: 5.0 }];
        let mut g = Gaze::forward();
        for i in 0..300 {
            g = observe(&p, g, &stimuli, i as f32 * 0.016, 0.016).gaze;
            assert!(g.yaw.abs() <= p.max_yaw + 0.05 && g.yaw.is_finite());
        }
    }

    #[test]
    fn deterministic() {
        let p = GazeParams::default();
        let stimuli = [Stimulus { bearing: 0.3, salience: 0.7, dist2: 10.0 }];
        let a = observe(&p, Gaze::forward(), &stimuli, 0.5, 0.016);
        let b = observe(&p, Gaze::forward(), &stimuli, 0.5, 0.016);
        assert_eq!(a.gaze, b.gaze);
    }
}
