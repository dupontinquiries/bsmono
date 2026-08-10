//! hover-kit — proximity gates the camera; the camera locates the hand.
//!
//! ## What a phone proximity sensor actually is
//!
//! Not a rangefinder. The part in a Fairphone exists to blank the screen
//! against your ear during a call, and that job needs one bit at ~5 cm. On
//! mainline it appears through IIO like the accelerometer
//! (`in_proximity_raw`), and what comes out is a **reflected-IR intensity**
//! that saturates almost immediately and varies with skin, sleeve colour and
//! sunlight.
//!
//! So it cannot be a pointer. Treating it as one produces a cursor that jumps
//! when you wear a dark jumper. What it CAN do is answer *"is something there"*
//! for approximately no power — and that is the expensive question for the
//! thing that can be a pointer.
//!
//! ## The architecture that follows
//!
//! ```text
//!   proximity (µW, ~1 bit)          camera (watts, 21 landmarks)
//!            │                                  │
//!            │  near ──────────────────────────►│ wake
//!            │  far  ──────────────────────────►│ sleep
//!            └──────────── gate ────────────────┘
//! ```
//!
//! This is the same shape as the ToF-locates-then-camera-identifies idea, built
//! from parts a Fairphone already has. It matters more on a phone than on a
//! desk: a front camera at 30 fps is watts and heat, and heat throttles the
//! cores the app is using. **A pointer that costs you the app is not a
//! pointer.**
//!
//! ## What this crate is and is not
//!
//! Policy only — when to wake vision, how to turn a hand position into a
//! pointer, and when to admit it does not know. No sysfs, no camera, no model.
//! That keeps every decision testable on a machine with none of the hardware,
//! which is the same split `iio_accel` already established for the
//! accelerometer.

// ------------------------------------------------------------ proximity ---

/// A raw proximity reading, in whatever units the device reports.
///
/// Deliberately not converted to millimetres. The value is reflected-IR
/// intensity: monotonic-ish in distance but scaled by reflectivity, so a
/// millimetre figure would be a number that looks trustworthy and is not.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Proximity(pub f32);

/// Near/far with hysteresis.
///
/// Two thresholds, not one. A single threshold on a noisy intensity chatters
/// at the boundary, and chatter here means the camera powering on and off many
/// times a second — the worst possible outcome for the thing the gate exists
/// to protect.
#[derive(Clone, Copy, Debug)]
pub struct Hysteresis {
    /// Cross this going up to become near.
    pub enter: f32,
    /// Fall below this to become far again.
    pub exit: f32,
}

impl Default for Hysteresis {
    fn default() -> Self {
        // Wide by design: a narrow band is a threshold with extra steps.
        Hysteresis { enter: 0.60, exit: 0.35 }
    }
}

impl Hysteresis {
    /// Is the band wide enough to actually suppress chatter?
    pub fn is_sane(&self) -> bool { self.enter > self.exit && self.enter - self.exit >= 0.1 }
}

/// What the gate thinks is happening.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    /// Nothing close. Vision should be off.
    Away,
    /// Something is close. Vision should be running.
    Near,
    /// The sensor is pinned at its ceiling — a pocket, a table, a palm resting
    /// on the glass. Distinct from Near because vision would see nothing
    /// useful and should NOT be woken.
    Occluded,
}

/// The gate itself.
#[derive(Clone, Copy, Debug)]
pub struct HoverGate {
    hyst: Hysteresis,
    /// Above this the sensor is saturated rather than tracking.
    saturate_at: f32,
    state: Presence,
    /// Milliseconds the current state has held, for the dwell rules.
    held_ms: f32,
}

/// How long `Near` must hold before vision wakes.
///
/// A hand passing over the phone on its way somewhere else is not an
/// interaction, and waking the camera for it spends the power the gate exists
/// to save.
pub const WAKE_DWELL_MS: f32 = 120.0;

/// How long `Away` must hold before vision sleeps.
///
/// Much longer than the wake dwell, and deliberately asymmetric: a hand that
/// briefly leaves the cone mid-gesture should not drop the pointer, because
/// re-acquiring costs a camera warm-up the user experiences as a stutter.
pub const SLEEP_DWELL_MS: f32 = 900.0;

impl Default for HoverGate {
    fn default() -> Self {
        HoverGate { hyst: Hysteresis::default(), saturate_at: 0.97,
                    state: Presence::Away, held_ms: 0.0 }
    }
}

/// What the gate wants the vision pipeline to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vision { Sleep, Wake }

impl HoverGate {
    pub fn with(hyst: Hysteresis) -> HoverGate {
        HoverGate { hyst, ..Default::default() }
    }

    pub fn presence(&self) -> Presence { self.state }

    /// Feed a reading. Returns the vision state that should hold now.
    pub fn update(&mut self, p: Proximity, dt_ms: f32) -> Vision {
        let v = p.0;
        let next = if !v.is_finite() {
            // A sensor read failure must not be interpreted as a hand.
            Presence::Away
        } else if v >= self.saturate_at {
            Presence::Occluded
        } else {
            match self.state {
                Presence::Near | Presence::Occluded => {
                    if v <= self.hyst.exit { Presence::Away } else { Presence::Near }
                }
                Presence::Away => {
                    if v >= self.hyst.enter { Presence::Near } else { Presence::Away }
                }
            }
        };

        if next == self.state { self.held_ms += dt_ms.max(0.0); }
        else { self.state = next; self.held_ms = 0.0; }

        match self.state {
            // Occluded never wakes vision: a pocket is not an interaction.
            Presence::Occluded => Vision::Sleep,
            Presence::Near if self.held_ms >= WAKE_DWELL_MS => Vision::Wake,
            Presence::Near => Vision::Sleep,
            Presence::Away if self.held_ms >= SLEEP_DWELL_MS => Vision::Sleep,
            // Still inside the grace window — keep vision alive.
            Presence::Away => Vision::Wake,
        }
    }
}

// -------------------------------------------------------------- pointer ---

/// A hand position from the vision layer, in normalised view coordinates.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HandPoint {
    pub x: f32,
    pub y: f32,
    /// 0..=1 from the landmark model.
    pub confidence: f32,
}

/// Below this a landmark is not steered to.
///
/// A low-confidence landmark is not a slightly-wrong pointer; it is a pointer
/// somewhere else entirely, and following it moves the cursor across the screen
/// on a bad frame.
pub const MIN_CONFIDENCE: f32 = 0.5;

/// Pointer smoothing and gating.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pos: Option<(f32, f32)>,
    /// Exponential smoothing factor per frame at 30 fps.
    alpha: f32,
    /// Movement below this is treated as tremor and ignored.
    deadzone: f32,
    lost_ms: f32,
}

/// How long a pointer survives with no confident landmark.
///
/// A hand does not teleport, so a dropped frame should not drop the cursor —
/// but holding a stale pointer forever leaves it stuck on screen after the user
/// has walked away.
pub const POINTER_GRACE_MS: f32 = 250.0;

impl Default for Pointer {
    fn default() -> Self {
        // 0.35: visibly smooth without the lag that makes a pointer feel
        // detached from the hand driving it.
        Pointer { pos: None, alpha: 0.35, deadzone: 0.004, lost_ms: 0.0 }
    }
}

impl Pointer {
    pub fn position(&self) -> Option<(f32, f32)> { self.pos }
    pub fn is_live(&self) -> bool { self.pos.is_some() }

    /// A frame arrived. `None` means vision saw no hand this frame.
    pub fn update(&mut self, hand: Option<HandPoint>, dt_ms: f32) -> Option<(f32, f32)> {
        match hand {
            Some(h) if h.confidence >= MIN_CONFIDENCE && h.x.is_finite() && h.y.is_finite() => {
                self.lost_ms = 0.0;
                let (tx, ty) = (h.x.clamp(0.0, 1.0), h.y.clamp(0.0, 1.0));
                self.pos = Some(match self.pos {
                    None => (tx, ty),
                    Some((px, py)) => {
                        let (dx, dy) = (tx - px, ty - py);
                        // Tremor below the deadzone is held, not followed: a
                        // hand in mid-air never stops moving, and a pointer
                        // that mirrors that is unusable for a small target.
                        if (dx * dx + dy * dy).sqrt() < self.deadzone { (px, py) }
                        else { (px + dx * self.alpha, py + dy * self.alpha) }
                    }
                });
            }
            _ => {
                self.lost_ms += dt_ms.max(0.0);
                if self.lost_ms >= POINTER_GRACE_MS { self.pos = None; }
            }
        }
        self.pos
    }

    /// Vision went to sleep. Drop the pointer immediately rather than letting
    /// it decay — the gate has already decided nobody is there.
    pub fn release(&mut self) { self.pos = None; self.lost_ms = 0.0; }
}

// ----------------------------------------------------------- capability ---

/// What this device can honestly offer.
///
/// Reported rather than assumed, because the same app runs on a desktop with a
/// webcam and no proximity sensor, a Fairphone with both, and a PinePhone whose
/// camera support is partial. A UI that offers hover on a machine that cannot
/// do it is a UI with a dead control on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Capability {
    /// Neither sensor. No hover affordances at all.
    None,
    /// Proximity only: presence and coarse gestures, no pointer.
    PresenceOnly,
    /// Camera only: a pointer, at full power cost whenever it is enabled.
    PointerUngated,
    /// Both: the intended configuration.
    PointerGated,
}

pub fn capability(has_proximity: bool, has_camera: bool) -> Capability {
    match (has_proximity, has_camera) {
        (true, true) => Capability::PointerGated,
        (false, true) => Capability::PointerUngated,
        (true, false) => Capability::PresenceOnly,
        (false, false) => Capability::None,
    }
}

impl Capability {
    /// May the app show a hover pointer?
    pub fn has_pointer(self) -> bool {
        matches!(self, Capability::PointerGated | Capability::PointerUngated)
    }

    /// The line a settings page shows, so the trade is the user's to make.
    pub fn note(self) -> &'static str {
        match self {
            Capability::None => "No proximity sensor or camera — hover is unavailable.",
            Capability::PresenceOnly =>
                "Proximity only: wake-on-approach works, but there is no pointer.",
            Capability::PointerUngated =>
                "No proximity sensor: the camera must run continuously, which costs battery.",
            Capability::PointerGated =>
                "Proximity wakes the camera only when a hand is near.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near() -> Proximity { Proximity(0.8) }
    fn far() -> Proximity { Proximity(0.1) }

    #[test]
    fn a_hand_must_dwell_before_the_camera_wakes() {
        // A hand passing over the phone on its way somewhere else is not an
        // interaction, and waking for it spends the power the gate saves.
        let mut g = HoverGate::default();
        assert_eq!(g.update(near(), 16.0), Vision::Sleep);
        assert_eq!(g.update(near(), 50.0), Vision::Sleep);
        assert_eq!(g.update(near(), WAKE_DWELL_MS), Vision::Wake);
    }

    #[test]
    fn a_brief_departure_does_not_drop_the_pointer() {
        // Re-acquiring costs a camera warm-up the user experiences as a
        // stutter, so the sleep dwell is much longer than the wake dwell.
        let mut g = HoverGate::default();
        g.update(near(), WAKE_DWELL_MS * 2.0);
        assert_eq!(g.update(far(), 100.0), Vision::Wake, "dropped on a blip");
        assert_eq!(g.update(far(), SLEEP_DWELL_MS), Vision::Sleep);
        assert!(SLEEP_DWELL_MS > WAKE_DWELL_MS * 4.0, "the asymmetry is the point");
    }

    #[test]
    fn hysteresis_stops_the_camera_chattering_at_the_boundary() {
        // A single threshold on a noisy intensity toggles many times a second,
        // which is the worst outcome for the thing the gate protects.
        let mut g = HoverGate::default();
        g.update(near(), WAKE_DWELL_MS * 2.0);
        // Sit between the two thresholds and jitter.
        for _ in 0..50 {
            g.update(Proximity(0.50), 16.0);
            assert_eq!(g.presence(), Presence::Near, "fell out of the band");
        }
    }

    #[test]
    fn the_band_must_actually_be_a_band() {
        // A narrow hysteresis is a threshold with extra steps.
        assert!(Hysteresis::default().is_sane());
        assert!(!Hysteresis { enter: 0.5, exit: 0.49 }.is_sane());
        assert!(!Hysteresis { enter: 0.3, exit: 0.6 }.is_sane(), "inverted");
    }

    #[test]
    fn a_saturated_sensor_is_a_pocket_not_a_hand() {
        // Pinned at the ceiling means a pocket, a table or a palm on the glass.
        // Vision would see nothing useful, so it must NOT wake.
        let mut g = HoverGate::default();
        for _ in 0..100 {
            assert_eq!(g.update(Proximity(1.0), 16.0), Vision::Sleep);
        }
        assert_eq!(g.presence(), Presence::Occluded);
    }

    #[test]
    fn a_failed_sensor_read_is_not_a_hand() {
        // A NaN from a disappearing sysfs node must not wake the camera and
        // hold it on.
        let mut g = HoverGate::default();
        assert_eq!(g.update(Proximity(f32::NAN), 16.0), Vision::Wake,
                   "inside the grace window, as any Away is");
        g.update(Proximity(f32::NAN), SLEEP_DWELL_MS);
        assert_eq!(g.presence(), Presence::Away);
    }

    #[test]
    fn a_low_confidence_landmark_does_not_move_the_pointer() {
        // It is not a slightly-wrong pointer; it is a pointer somewhere else
        // entirely, and following it throws the cursor across the screen.
        let mut p = Pointer::default();
        p.update(Some(HandPoint { x: 0.5, y: 0.5, confidence: 0.9 }), 33.0);
        let before = p.position();
        p.update(Some(HandPoint { x: 0.9, y: 0.1, confidence: 0.2 }), 33.0);
        assert_eq!(p.position(), before, "a bad frame moved the cursor");
    }

    #[test]
    fn the_pointer_survives_a_dropped_frame_but_not_a_departure() {
        // A hand does not teleport, so one bad frame should not drop the
        // cursor — but a stale pointer must not stick on screen forever.
        let mut p = Pointer::default();
        p.update(Some(HandPoint { x: 0.5, y: 0.5, confidence: 0.9 }), 33.0);
        p.update(None, 100.0);
        assert!(p.is_live(), "dropped on one missing frame");
        p.update(None, POINTER_GRACE_MS);
        assert!(!p.is_live());
    }

    #[test]
    fn tremor_below_the_deadzone_is_held() {
        // A hand in mid-air never stops moving; a pointer that mirrors that is
        // unusable for a small target.
        let mut p = Pointer::default();
        p.update(Some(HandPoint { x: 0.5, y: 0.5, confidence: 0.9 }), 33.0);
        let settled = p.position().unwrap();
        for i in 0..20 {
            let jitter = if i % 2 == 0 { 0.001 } else { -0.001 };
            p.update(Some(HandPoint { x: 0.5 + jitter, y: 0.5, confidence: 0.9 }), 33.0);
        }
        assert_eq!(p.position().unwrap(), settled, "tremor reached the cursor");
    }

    #[test]
    fn the_pointer_smooths_toward_a_real_move_rather_than_snapping() {
        let mut p = Pointer::default();
        p.update(Some(HandPoint { x: 0.2, y: 0.2, confidence: 0.9 }), 33.0);
        p.update(Some(HandPoint { x: 0.8, y: 0.8, confidence: 0.9 }), 33.0);
        let (x, _) = p.position().unwrap();
        assert!(x > 0.2 && x < 0.8, "snapped rather than smoothed: {x}");
    }

    #[test]
    fn a_landmark_outside_the_view_is_clamped_rather_than_followed() {
        // Models emit landmarks slightly outside the frame when a hand is at
        // the edge; a pointer at -0.3 is off-screen and unreachable.
        let mut p = Pointer::default();
        p.update(Some(HandPoint { x: -0.3, y: 1.4, confidence: 0.9 }), 33.0);
        let (x, y) = p.position().unwrap();
        assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y), "{x},{y}");
    }

    #[test]
    fn sleeping_vision_releases_the_pointer_at_once() {
        // The gate has already decided nobody is there; letting the cursor
        // decay would leave it visible after the screen stopped watching.
        let mut p = Pointer::default();
        p.update(Some(HandPoint { x: 0.5, y: 0.5, confidence: 0.9 }), 33.0);
        p.release();
        assert!(!p.is_live());
    }

    #[test]
    fn capability_is_reported_rather_than_assumed() {
        // The same app runs on a desktop webcam with no proximity sensor, a
        // Fairphone with both, and a PinePhone with partial camera support. A
        // UI offering hover where it cannot work has a dead control on it.
        assert_eq!(capability(true, true), Capability::PointerGated);
        assert_eq!(capability(false, true), Capability::PointerUngated);
        assert_eq!(capability(true, false), Capability::PresenceOnly);
        assert_eq!(capability(false, false), Capability::None);
        assert!(!Capability::PresenceOnly.has_pointer(), "proximity alone is not a pointer");
        assert!(Capability::PointerUngated.has_pointer());
    }

    #[test]
    fn every_capability_says_what_it_costs() {
        // The battery trade is the user's to make, so it has to be on screen.
        for c in [Capability::None, Capability::PresenceOnly,
                  Capability::PointerUngated, Capability::PointerGated] {
            assert!(!c.note().is_empty(), "{c:?}");
        }
        assert!(Capability::PointerUngated.note().contains("battery"),
                "the ungated case must name its cost");
    }
}
