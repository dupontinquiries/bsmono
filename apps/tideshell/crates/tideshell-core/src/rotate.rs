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

/// The phone is "resting" (on a table, face up or down) when the screen normal is within this many
/// degrees of vertical, and stops resting once past the larger one (hysteresis, so a table that is
/// not quite level, or a hand on the edge, does not flicker it). While resting, a reported
/// orientation is noise (the in-plane gravity is a few degrees of table tilt), so nothing rotates.
pub const REST_ENTER_DEG: f32 = 28.0;
pub const REST_EXIT_DEG: f32 = 38.0;
/// A phone propped or held mostly flat (screen normal within this of vertical) is slower to commit
/// to a turn: twice the hold, and 10 degrees further past the switch point.
pub const LOW_TILT_DEG: f32 = 55.0;

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
    /// Screen-normal tilt from vertical, degrees (0 face up, 180 face down), from the latest sample.
    tilt: Option<f32>,
    resting: bool,
    /// The shell's own flat verdict (`clock3d::FlatDetector`, from the orientation string on a\n    /// phone with no raw accelerometer): also counts as resting.
    flat_hint: bool,
}

impl Default for AutoRotate {
    fn default() -> Self {
        AutoRotate { enabled: true, locked: Orientation::Normal, applied: Orientation::Normal, candidate: None, angle: None, cand_angle: None, tilt: None, resting: false, flat_hint: false }
    }
}

impl AutoRotate {
    /// A sensor reading. Returns the orientation to apply when one becomes
    /// due (held long enough); `blocked` (cover up, asleep) defers it.
    pub fn on_reading(&mut self, s: &str, now: Millis, blocked: bool) -> Option<Orientation> {
        if !self.enabled {
            return None;
        }
        let Some(o) = Orientation::parse(s) else {
            // "undefined": the phone is flat or mid-turn. Whatever was building (a transient reading
            // on the way down to the table) is not a turn.
            self.candidate = None;
            self.cand_angle = None;
            return None;
        };
        if self.is_resting() {
            return None; // lying on a table: the reading is table tilt, not a turn
        }
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
        if let Some(t) = crate::clock3d::tilt_from_up_deg(a) {
            self.tilt = Some(t);
            let off_vertical = t.min(180.0 - t); // face up or face down
            let limit = if self.resting { REST_EXIT_DEG } else { REST_ENTER_DEG };
            let now_resting = off_vertical <= limit;
            if now_resting && !self.resting {
                self.candidate = None; // a turn that was building is not one
                self.cand_angle = None;
            }
            self.resting = now_resting;
        }
    }

    /// Is the phone lying on a surface (so nothing rotates)?
    pub fn is_resting(&self) -> bool {
        self.resting || self.flat_hint
    }

    /// The shell's flat verdict (on a table, face up or down), for devices that report orientation
    /// only as a string. Becoming flat drops a turn that was building.
    pub fn set_flat_hint(&mut self, flat: bool) {
        if flat && !self.flat_hint {
            self.candidate = None;
            self.cand_angle = None;
        }
        self.flat_hint = flat;
    }

    /// (hold ms, degrees past the switch point) for the current tilt: stricter when the phone is
    /// mostly flat but not resting.
    fn params(&self) -> (Millis, f32) {
        match self.tilt {
            Some(t) if t.min(180.0 - t) < LOW_TILT_DEG => (HOLD_MS * 2, PAST_DEG + 10.0),
            _ => (HOLD_MS, PAST_DEG),
        }
    }

    /// Apply the candidate if it has held long enough. iio-sensor-proxy only
    /// reports CHANGES, so a single reading is all that ever arrives: the hold
    /// has to be judged by the clock (the shell's tick), not by a second reading.
    pub fn poll(&mut self, now: Millis, blocked: bool) -> Option<Orientation> {
        if !self.enabled || self.is_resting() {
            return None;
        }
        let Some((o, since)) = self.candidate else { return None };
        let (hold, past) = self.params();
        if now.saturating_sub(since) < hold {
            return None;
        }
        if let (Some(a0), Some(a1)) = (self.cand_angle, self.angle) {
            if angle_between(a0, a1).to_degrees() < past {
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

    // gravity in device axes: on a table (0,0,9.8); upright in the hand (0,9.8,0); a landscape hold (9.8,0,0)
    const TABLE: [f32; 3] = [0.0, 0.0, 9.8];
    const UPRIGHT_SIDE: [f32; 3] = [9.5, 0.0, 2.0];
    const PORTRAIT: [f32; 3] = [0.0, 9.5, 2.0];

    /// On a table nothing rotates, however the orientation string flaps: a reading that would have
    /// turned the screen when held does nothing, and a turn that was building is dropped.
    #[test]
    fn a_phone_lying_on_a_table_never_rotates() {
        let mut held = AutoRotate::default();
        held.on_accel(PORTRAIT);
        let mut got = held.on_reading("left-up", 0, false);
        held.on_accel(UPRIGHT_SIDE); // and it really is turned
        for t in (100..3000).step_by(100) {
            got = got.or(held.on_reading("left-up", t, false));
        }
        assert_eq!(got, Some(Orientation::LeftUp), "held in the hand, the same reading rotates");

        let mut table = AutoRotate::default();
        table.on_accel(TABLE);
        assert!(table.is_resting());
        for t in (0..6000).step_by(100) {
            // the string flaps between orientations, as a slightly tilted table makes it
            let s = ["left-up", "bottom-up", "right-up", "normal"][(t / 700) as usize % 4];
            assert_eq!(table.on_reading(s, t, false), None, "no rotation at {t}");
            assert_eq!(table.poll(t, false), None);
        }
        assert_eq!(table.applied, Orientation::Normal);
        // and a STEADY wrong reading (the table is simply a bit tilted) is no turn either
        let mut tilted_table = AutoRotate::default();
        tilted_table.on_accel([0.0, 2.0, 9.6]); // a table 12 degrees off level
        for t in (0..8000).step_by(100) {
            assert_eq!(tilted_table.on_reading("bottom-up", t, false), None, "steady reading, still resting, at {t}");
            assert_eq!(tilted_table.poll(t, false), None);
        }
    }

    /// A turn that was building when the phone is set down is dropped, not applied a moment later.
    #[test]
    fn setting_the_phone_down_drops_a_turn_in_progress() {
        let mut r = AutoRotate::default();
        r.on_accel(PORTRAIT);
        assert_eq!(r.on_reading("left-up", 0, false), None);
        r.on_accel(UPRIGHT_SIDE);
        r.on_accel(TABLE);
        assert_eq!(r.poll(5000, false), None, "set down: the pending turn never lands");
        r.on_accel(UPRIGHT_SIDE);
        assert_eq!(r.poll(5100, false), None, "and picking it up does not resurrect it");
    }

    /// Resting has hysteresis: 30 degrees off flat still rests (a not-quite-level table), 45 does not.
    #[test]
    fn resting_has_hysteresis_and_face_down_counts() {
        let tilt = |deg: f32| [0.0, 9.8 * deg.to_radians().sin(), 9.8 * deg.to_radians().cos()];
        let mut r = AutoRotate::default();
        r.on_accel(tilt(10.0));
        assert!(r.is_resting());
        r.on_accel(tilt(33.0));
        assert!(r.is_resting(), "33 is past the 28 entry but inside the 38 exit");
        r.on_accel(tilt(45.0));
        assert!(!r.is_resting(), "45 is picked up");
        r.on_accel(tilt(33.0));
        assert!(!r.is_resting(), "and 33 does not re-enter (28)");
        let mut down = AutoRotate::default();
        down.on_accel([0.0, 0.0, -9.8]);
        assert!(down.is_resting(), "face down on a table rests too");
    }

    /// Held mostly flat (not resting), a turn needs a longer hold than upright.
    #[test]
    fn a_mostly_flat_hold_is_slower_to_turn() {
        // (portrait pose, turned pose): the reading arrives at the first, the device ends up at the second
        let at = |from: [f32; 3], to: [f32; 3]| {
            let mut r = AutoRotate::default();
            r.on_accel(from);
            let _ = r.on_reading("left-up", 0, false);
            r.on_accel(to);
            (r.poll(HOLD_MS + 50, false), r.poll(HOLD_MS * 2 + 50, false))
        };
        let tilted = ([0.0, 9.0, 4.0], [9.0, 0.0, 4.0]); // 66 deg from vertical: not low
        let low = ([0.0, 6.0, 7.7], [6.0, 0.0, 7.7]); // 38 deg: low tilt (resting only inside 28 on the way in)
        let (early, _) = at(tilted.0, tilted.1);
        assert_eq!(early, Some(Orientation::LeftUp), "upright-ish: commits after the normal hold");
        let (early, late) = at(low.0, low.1);
        assert_eq!(early, None, "mostly flat: not yet at the normal hold");
        assert_eq!(late, Some(Orientation::LeftUp), "but at twice the hold");
    }


    /// No accelerometer vector on the phone (only strings): the reading "undefined" (flat, or mid-turn)
    /// drops a candidate that was building, so a transient on the way down to the table never lands.
    #[test]
    fn undefined_drops_a_turn_that_was_building() {
        let mut r = AutoRotate::default();
        assert_eq!(r.on_reading("left-up", 0, false), None);
        assert_eq!(r.on_reading("undefined", 400, false), None);
        assert_eq!(r.poll(5000, false), None, "the transient never lands");
        assert_eq!(r.applied, Orientation::Normal);
        // a real turn afterwards still works
        assert_eq!(r.on_reading("right-up", 6000, false), None);
        assert_eq!(r.poll(6000 + HOLD_MS + 10, false), Some(Orientation::RightUp));
    }

    /// The shell's flat verdict freezes rotation just as a resting accelerometer does.
    #[test]
    fn the_shells_flat_verdict_freezes_rotation_and_drops_a_pending_turn() {
        let mut r = AutoRotate::default();
        assert_eq!(r.on_reading("left-up", 0, false), None);
        r.set_flat_hint(true);
        assert!(r.is_resting());
        assert_eq!(r.poll(5000, false), None, "the pending turn is gone");
        for t in (6000..9000).step_by(100) {
            assert_eq!(r.on_reading("bottom-up", t, false), None, "flat: nothing rotates at {t}");
        }
        r.set_flat_hint(false);
        assert_eq!(r.on_reading("bottom-up", 10_000, false), None);
        assert_eq!(r.poll(10_000 + HOLD_MS + 10, false), Some(Orientation::BottomUp), "picked up: rotation works again");
    }

}
