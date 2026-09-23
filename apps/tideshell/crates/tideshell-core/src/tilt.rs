//! Gyro select: with a finger held down (the palette's hold), tilting the
//! phone moves the selection, so one hand chooses without sliding. Roll
//! (about the device's forward axis) steps sideways, pitch (about the
//! horizontal axis) steps up and down. Angle accumulates per axis and a
//! step fires every STEP_DEG, then that axis resets, with a DEAD_DEG_PER_S
//! rate floor so a still hand's noise never steps.

pub const STEP_DEG: f32 = 12.0;
pub const DEAD_DEG_PER_S: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tilt {
    roll_deg: f32,
    pitch_deg: f32,
}

impl Tilt {
    /// Gyro rates about device X (pitch) and Z (roll), rad/s, over dt.
    /// Returns (dx, dy) steps: dx +1 = right, dy +1 = down.
    pub fn feed(&mut self, rate_x: f32, rate_z: f32, dt: f32) -> (i32, i32) {
        let (px, rz) = (rate_x.to_degrees(), rate_z.to_degrees());
        if px.abs() >= DEAD_DEG_PER_S {
            self.pitch_deg += px * dt;
        }
        if rz.abs() >= DEAD_DEG_PER_S {
            self.roll_deg += rz * dt;
        }
        let mut out = (0, 0);
        // Rolling the top edge to the RIGHT is a negative rotation about +Z
        // (Z points at the user): that steps right.
        if self.roll_deg.abs() >= STEP_DEG {
            out.0 = if self.roll_deg < 0.0 { 1 } else { -1 };
            self.roll_deg = 0.0;
        }
        // Tilting the top edge AWAY (looking further down the list) is a
        // negative rotation about +X: that steps down.
        if self.pitch_deg.abs() >= STEP_DEG {
            out.1 = if self.pitch_deg < 0.0 { 1 } else { -1 };
            self.pitch_deg = 0.0;
        }
        out
    }

    pub fn reset(&mut self) {
        *self = Tilt::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deg(d: f32) -> f32 {
        d.to_radians()
    }

    #[test]
    fn a_deliberate_roll_steps_once_per_twelve_degrees_and_noise_never_does() {
        let mut t = Tilt::default();
        let mut steps = 0;
        for _ in 0..30 {
            steps += t.feed(0.0, deg(-40.0), 0.02).0; // rolling right at 40 deg/s for 0.6 s = 24 deg
        }
        assert_eq!(steps, 2, "two steps right");
        let mut t = Tilt::default();
        for _ in 0..500 {
            assert_eq!(t.feed(deg(5.0), deg(-5.0), 0.02), (0, 0), "under the rate floor: nothing, ever");
        }
    }

    #[test]
    fn pitch_steps_vertically_and_directions_are_right() {
        let mut t = Tilt::default();
        let mut dy = 0;
        for _ in 0..20 {
            dy += t.feed(deg(-40.0), 0.0, 0.02).1; // top edge away
        }
        assert_eq!(dy, 1, "one step down");
        let mut t = Tilt::default();
        let mut dx = 0;
        for _ in 0..20 {
            dx += t.feed(0.0, deg(40.0), 0.02).0; // roll left
        }
        assert_eq!(dx, -1);
    }
}
