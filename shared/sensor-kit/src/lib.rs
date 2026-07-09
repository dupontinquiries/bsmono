//! sensor-kit — the shared tilt/gyro control layer every game uses. On postmarketOS/Fairphone the
//! phone's accelerometer + compass are exposed by iio-sensor-proxy; its `monitor-sensor` tool prints
//! coarse tilt/orientation change events. This crate parses that stream and folds the events into an
//! **impulse-based** 2D control vector — the model proven in the gyro-ball prototype: each tilt kicks
//! a velocity impulse that the game loop drains under friction, which handles the "tilted-up is a
//! tiny window" problem better than treating tilt as sustained gravity.
//!
//! Pure + source-agnostic: the platform reader (a piped `monitor-sensor`, a macOS CMMotion bridge,
//! an Android SensorManager bridge) feeds lines/events in; the control math lives here and is tested.
//! Reuses `touchkit::Vec2` — no second vector type in the tree.

pub use touchkit::Vec2;

/// A tilt state from `monitor-sensor --accel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TiltState {
    Normal,
    Up,   // top edge toward you
    Down, // top edge away from you
}

/// An orientation state from `monitor-sensor` ("Accelerometer orientation changed: …").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    LeftUp,
    RightUp,
    BottomUp,
}

/// One parsed sensor event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorEvent {
    Tilt(TiltState),
    Orient(Orientation),
}

/// Parse a line of iio-sensor-proxy `monitor-sensor` output into an event (None for lines that carry
/// no tilt/orientation change — headers, other sensors, blanks).
pub fn parse_monitor_sensor(line: &str) -> Option<SensorEvent> {
    let l = line.trim();
    if let Some(i) = l.find("Tilt changed:") {
        let state = l[i + "Tilt changed:".len()..].trim();
        return Some(SensorEvent::Tilt(match state {
            "tilted-up" => TiltState::Up,
            "tilted-down" => TiltState::Down,
            _ => TiltState::Normal,
        }));
    }
    if let Some(i) = l.find("orientation changed:") {
        let state = l[i + "orientation changed:".len()..].trim();
        return Some(SensorEvent::Orient(match state {
            "left-up" => Orientation::LeftUp,
            "right-up" => Orientation::RightUp,
            "bottom-up" => Orientation::BottomUp,
            _ => Orientation::Normal,
        }));
    }
    None
}

/// The impulse-based tilt controller. Feed it events; each frame `tick` folds the accumulated
/// impulse into the game's velocity and applies friction (lighter while a directional tilt is held,
/// so the object keeps rolling; heavier when neutral, so it settles).
#[derive(Debug, Clone)]
pub struct TiltControl {
    impulse: Vec2,
    active: bool,
    /// Impulse magnitude per tilt event (tune to taste).
    pub impulse_mag: f32,
    /// Friction multiplier applied to velocity each frame when no tilt is held.
    pub friction_neutral: f32,
    /// Friction while a directional tilt is held.
    pub friction_active: f32,
    /// Flip the vertical axis (screen-down is +y in most 2D engines; the gyro-ball default).
    pub flip_vert: bool,
}

impl Default for TiltControl {
    fn default() -> Self {
        TiltControl {
            impulse: Vec2::ZERO,
            active: false,
            impulse_mag: 5.6,
            friction_neutral: 0.80,
            friction_active: 0.92,
            flip_vert: true,
        }
    }
}

impl TiltControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a directional tilt is currently held (drives the lighter friction).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Feed a parsed event: accumulate a directional impulse, or go neutral. Multiple events between
    /// frames accumulate naturally (rapid rocking builds momentum).
    pub fn apply(&mut self, ev: SensorEvent) {
        let m = self.impulse_mag;
        let (dx, dy, active) = match ev {
            SensorEvent::Tilt(TiltState::Down) => (0.0, m, true), // top away → roll down (+y)
            SensorEvent::Tilt(TiltState::Up) => (0.0, -m, true),  // top toward → roll up
            SensorEvent::Orient(Orientation::LeftUp) => (m, 0.0, true),
            SensorEvent::Orient(Orientation::RightUp) => (-m, 0.0, true),
            // Normal / BottomUp → no directional kick; settle under heavier friction.
            _ => (0.0, 0.0, false),
        };
        let dy = if self.flip_vert { -dy } else { dy };
        self.impulse.x += dx;
        self.impulse.y += dy;
        self.active = active;
    }

    /// Convenience: parse a `monitor-sensor` line and apply it if it's an event. Returns true if the
    /// line produced an event.
    pub fn feed_line(&mut self, line: &str) -> bool {
        match parse_monitor_sensor(line) {
            Some(ev) => {
                self.apply(ev);
                true
            }
            None => false,
        }
    }

    /// Advance one frame: add the accumulated impulse to `vel`, apply friction, drain the impulse.
    pub fn tick(&mut self, vel: &mut Vec2) {
        vel.x += self.impulse.x;
        vel.y += self.impulse.y;
        let f = if self.active { self.friction_active } else { self.friction_neutral };
        vel.x *= f;
        vel.y *= f;
        self.impulse = Vec2::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tilt_and_orientation_lines() {
        assert_eq!(parse_monitor_sensor("    Tilt changed: tilted-down"), Some(SensorEvent::Tilt(TiltState::Down)));
        assert_eq!(parse_monitor_sensor("Tilt changed: tilted-up"), Some(SensorEvent::Tilt(TiltState::Up)));
        assert_eq!(parse_monitor_sensor("=== Has accelerometer (yes)"), None);
        assert_eq!(parse_monitor_sensor("Accelerometer orientation changed: left-up"), Some(SensorEvent::Orient(Orientation::LeftUp)));
        assert_eq!(parse_monitor_sensor("Accelerometer orientation changed: normal"), Some(SensorEvent::Orient(Orientation::Normal)));
        assert_eq!(parse_monitor_sensor(""), None);
    }

    #[test]
    fn tilt_down_rolls_the_object_and_neutral_settles_it() {
        let mut c = TiltControl::new();
        c.flip_vert = false; // test in raw axes: down = +y
        let mut vel = Vec2::ZERO;
        c.apply(SensorEvent::Tilt(TiltState::Down));
        c.tick(&mut vel);
        assert!(vel.y > 0.0, "tilted-down kicks the object down (+y): {}", vel.y);
        assert!(c.is_active());
        // go neutral: no new impulse, heavier friction decays the velocity toward rest
        let before = vel.y;
        c.apply(SensorEvent::Tilt(TiltState::Normal));
        c.tick(&mut vel);
        assert!(!c.is_active());
        assert!(vel.y < before && vel.y > 0.0, "neutral decays but doesn't reverse: {} -> {}", before, vel.y);
    }

    #[test]
    fn held_tilt_keeps_rolling_more_than_neutral() {
        // same starting velocity, one frame: active friction retains more speed than neutral.
        let mut held = TiltControl::new();
        let mut neutral = TiltControl::new();
        held.active = true;
        neutral.active = false;
        let (mut vh, mut vn) = (Vec2 { x: 10.0, y: 0.0 }, Vec2 { x: 10.0, y: 0.0 });
        held.tick(&mut vh);
        neutral.tick(&mut vn);
        assert!(vh.x > vn.x, "held tilt (0.92) keeps more speed than neutral (0.80): {} vs {}", vh.x, vn.x);
    }

    #[test]
    fn rapid_rocking_accumulates_impulses() {
        let mut c = TiltControl::new();
        c.flip_vert = false;
        let mut vel = Vec2::ZERO;
        // two down-kicks before a frame → double impulse
        c.apply(SensorEvent::Tilt(TiltState::Down));
        c.apply(SensorEvent::Tilt(TiltState::Down));
        c.tick(&mut vel);
        assert!((vel.y - 2.0 * c.impulse_mag * c.friction_active).abs() < 0.01, "two kicks accumulate: {}", vel.y);
    }
}
