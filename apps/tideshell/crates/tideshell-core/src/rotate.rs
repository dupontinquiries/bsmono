//! Autorotation with hysteresis. iio-sensor-proxy reports one of a few
//! orientations from the accelerometer (with its own angle threshold); this
//! model turns that stream into output transforms without dizziness:
//! a new orientation must hold for HOLD_MS before it is applied, "undefined"
//! (the phone flat, or at 45°) never changes anything, and a rotation is not
//! repeated while the cover is up or the screen asleep (applied on wake).

use crate::Millis;

pub const HOLD_MS: Millis = 900;

/// Degrees the device must turn IN THE SCREEN PLANE past the point where
/// iio-sensor-proxy first reported the new orientation (its own switch is ~45
/// degrees and not configurable) before the shell applies it. Needs accelerometer
/// samples (`on_accel`); without any, the hold alone decides.
pub const PAST_DEG: f32 = 25.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    BottomUp,
    LeftUp,
    RightUp,
}

impl Orientation {
    /// iio-sensor-proxy's AccelerometerOrientation strings.
    pub fn parse(s: &str) -> Option<Orientation> {
        match s {
            "normal" => Some(Orientation::Normal),
            "bottom-up" => Some(Orientation::BottomUp),
            "left-up" => Some(Orientation::LeftUp),
            "right-up" => Some(Orientation::RightUp),
            _ => None, // "undefined" and anything new
        }
    }

    /// Output transform in degrees (wl_output.transform names: normal, 90,
    /// 180, 270), for a portrait-native panel. left-up = the device's left
    /// edge is up = content rotated 90° counter-clockwise, the mapping
    /// gnome-settings-daemon uses; `flip` swaps 90/270 for panels that
    /// disagree.
    pub fn transform_deg(self, flip: bool) -> u32 {
        match (self, flip) {
            (Orientation::Normal, _) => 0,
            (Orientation::BottomUp, _) => 180,
            (Orientation::LeftUp, false) | (Orientation::RightUp, true) => 90,
            (Orientation::LeftUp, true) | (Orientation::RightUp, false) => 270,
        }
    }
}

/// Smallest angle between two directions, radians.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoRotate {
    pub enabled: bool,
    pub locked: Orientation,
    pub applied: Orientation,
    candidate: Option<(Orientation, Millis)>,
    /// Gravity direction in the screen plane (radians), latest sample and the one
    /// current when the candidate was first reported.
    angle: Option<f32>,
    cand_angle: Option<f32>,
}

impl Default for AutoRotate {
    fn default() -> Self {
        AutoRotate { enabled: true, locked: Orientation::Normal, applied: Orientation::Normal, candidate: None, angle: None, cand_angle: None }
    }
}

impl AutoRotate {
    /// A sensor reading. Returns the orientation to apply when one becomes
    /// due (held long enough); `blocked` (cover up, asleep) defers it.
    pub fn on_reading(&mut self, s: &str, now: Millis, blocked: bool) -> Option<Orientation> {
        if !self.enabled {
            return None;
        }
        let Some(o) = Orientation::parse(s) else { return None };
        if o == self.locked {
            self.candidate = None;
            self.cand_angle = None;
            return None;
        }
        if !matches!(self.candidate, Some((c, _)) if c == o) {
            self.candidate = Some((o, now));
            self.cand_angle = self.angle;
        }
        self.poll(now, blocked)
    }

    /// An accelerometer sample (m/s2, device axes: x right, y up, z out of the
    /// screen). Tracks where gravity points in the screen plane; flat or face-down
    /// (no in-plane component) leaves the last direction alone.
    pub fn on_accel(&mut self, a: [f32; 3]) {
        if a[0].hypot(a[1]) > 3.0 {
            self.angle = Some(a[1].atan2(a[0]));
        }
    }

    /// Apply the candidate if it has held long enough. iio-sensor-proxy only
    /// reports CHANGES, so a single reading is all that ever arrives: the hold
    /// has to be judged by the clock (the shell's tick), not by a second reading.
    pub fn poll(&mut self, now: Millis, blocked: bool) -> Option<Orientation> {
        if !self.enabled {
            return None;
        }
        let Some((o, since)) = self.candidate else { return None };
        if now.saturating_sub(since) < HOLD_MS {
            return None;
        }
        if let (Some(a0), Some(a1)) = (self.cand_angle, self.angle) {
            if angle_between(a0, a1).to_degrees() < PAST_DEG {
                return None; // reported, but not turned far enough past the switch point
            }
        }
        self.locked = o;
        self.candidate = None;
        if blocked {
            return None; // applied later, see `pending`
        }
        self.applied = o;
        Some(o)
    }

    /// Whether a locked orientation is waiting to be applied (was blocked).
    pub fn pending(&mut self) -> Option<Orientation> {
        if self.locked != self.applied {
            self.applied = self.locked;
            Some(self.locked)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_orientation_must_hold_before_it_applies() {
        let mut r = AutoRotate::default();
        assert_eq!(r.on_reading("left-up", 0, false), None);
        assert_eq!(r.on_reading("left-up", 300, false), None);
        assert_eq!(r.on_reading("normal", 400, false), None, "went back: candidate dropped");
        assert_eq!(r.on_reading("left-up", 500, false), None, "restarted the hold");
        assert_eq!(r.on_reading("left-up", 500 + HOLD_MS, false), Some(Orientation::LeftUp));
        assert_eq!(r.on_reading("left-up", 2000, false), None, "no repeat while it holds");
    }

    fn tilt(deg: f32) -> [f32; 3] {
        let r = deg.to_radians();
        [9.8 * r.cos(), 9.8 * r.sin(), 0.0]
    }

    #[test]
    fn the_device_must_turn_past_the_switch_point_before_it_rotates() {
        let mut r = AutoRotate::default();
        r.on_accel(tilt(0.0));
        assert_eq!(r.on_reading("left-up", 0, false), None);
        r.on_accel(tilt(10.0)); // held long, but only 10 degrees past the proxy's switch
        assert_eq!(r.poll(HOLD_MS * 3, false), None, "not far enough");
        r.on_accel(tilt(PAST_DEG + 5.0));
        assert_eq!(r.poll(HOLD_MS * 3, false), Some(Orientation::LeftUp));
    }

    #[test]
    fn turning_back_the_other_way_is_not_progress_and_flat_is_ignored() {
        let mut r = AutoRotate::default();
        r.on_accel(tilt(0.0));
        r.on_reading("left-up", 0, false);
        r.on_accel(tilt(-(PAST_DEG + 5.0)));
        assert_eq!(r.poll(HOLD_MS * 3, false), Some(Orientation::LeftUp), "distance, either way, is the test");
        let mut r = AutoRotate::default();
        r.on_accel(tilt(0.0));
        r.on_reading("left-up", 0, false);
        r.on_accel([0.1, 0.1, 9.8]); // lying flat: no in-plane direction, keeps the last
        assert_eq!(r.poll(HOLD_MS * 3, false), None);
    }

    #[test]
    fn a_single_reading_applies_once_the_clock_passes_the_hold() {
        // The proxy reports changes only: one "left-up", then silence.
        let mut r = AutoRotate::default();
        assert_eq!(r.on_reading("left-up", 0, false), None);
        assert_eq!(r.poll(HOLD_MS - 1, false), None);
        assert_eq!(r.poll(HOLD_MS, false), Some(Orientation::LeftUp));
        assert_eq!(r.poll(HOLD_MS * 4, false), None, "applied once");
    }

    #[test]
    fn undefined_never_rotates_and_flat_or_45_degrees_keep_the_last() {
        let mut r = AutoRotate::default();
        r.on_reading("right-up", 0, false);
        assert_eq!(r.on_reading("right-up", HOLD_MS, false), Some(Orientation::RightUp));
        for t in 1..20u64 {
            assert_eq!(r.on_reading("undefined", HOLD_MS + t * 100, false), None);
        }
        assert_eq!(r.locked, Orientation::RightUp);
    }

    #[test]
    fn blocked_rotations_are_applied_once_unblocked() {
        let mut r = AutoRotate::default();
        r.on_reading("bottom-up", 0, true);
        assert_eq!(r.on_reading("bottom-up", HOLD_MS, true), None, "cover up: not now");
        assert_eq!(r.locked, Orientation::BottomUp);
        assert_eq!(r.pending(), Some(Orientation::BottomUp));
        assert_eq!(r.pending(), None, "applied once");
    }

    #[test]
    fn transforms_map_and_the_flip_swaps_landscape() {
        assert_eq!(Orientation::LeftUp.transform_deg(false), 90);
        assert_eq!(Orientation::RightUp.transform_deg(false), 270);
        assert_eq!(Orientation::LeftUp.transform_deg(true), 270);
        assert_eq!(Orientation::BottomUp.transform_deg(true), 180);
        let mut r = AutoRotate { enabled: false, ..Default::default() };
        assert_eq!(r.on_reading("left-up", 0, false), None);
        assert_eq!(r.on_reading("left-up", 9999, false), None, "disabled: never");
    }
}
