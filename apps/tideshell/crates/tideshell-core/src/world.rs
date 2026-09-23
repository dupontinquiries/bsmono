//! The camera pane's "3D world": the phone's orientation from the gyro,
//! kept honest by gravity (accelerometer) and north (compass), and virtual
//! anchors placed in world directions that stay put on screen while the
//! phone turns: the cheap effect. Pure; sensors and pixels arrive from the
//! frontend.
//!
//! * Orientation is a unit quaternion (world -> device). Gyro rates (rad/s,
//!   device axes) integrate it forward every sample; without correction it
//!   drifts, so two slow complementary corrections pull it back: the
//!   measured gravity toward device -Y ... the accelerometer's direction,
//!   and the compass heading toward the yaw it reports. Corrections are
//!   small per sample (GRAVITY_GAIN, HEADING_GAIN), so shaking does not tilt
//!   the world and a brief magnetic disturbance does not swing it.
//! * Projection is a pinhole: an anchor is a unit vector in the world frame;
//!   rotated into the device frame, it is on screen when it is in front of
//!   the rear camera (device -Z), at focal * (x, -y) / depth.
//! * `image_correction` folds in what the picture says: the tracker's
//!   measured pixel motion vs the motion the gyro predicted; the residual
//!   nudges the orientation, which is how the anchors stop sliding when the
//!   gyro bias is off.

use std::f32::consts::PI;

pub const GRAVITY_GAIN: f32 = 0.02;
pub const HEADING_GAIN: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat { w: 1.0, x: 0.0, y: 0.0, z: 0.0 };

    pub fn from_axis_angle(axis: [f32; 3], angle: f32) -> Quat {
        let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt().max(1e-9);
        let (s, c) = ((angle / 2.0).sin(), (angle / 2.0).cos());
        Quat { w: c, x: axis[0] / n * s, y: axis[1] / n * s, z: axis[2] / n * s }
    }

    pub fn mul(self, o: Quat) -> Quat {
        Quat {
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
        }
    }

    pub fn conj(self) -> Quat {
        Quat { w: self.w, x: -self.x, y: -self.y, z: -self.z }
    }

    pub fn normalized(self) -> Quat {
        let n = (self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z).sqrt().max(1e-9);
        Quat { w: self.w / n, x: self.x / n, y: self.y / n, z: self.z / n }
    }

    /// Rotate a vector.
    pub fn rotate(self, v: [f32; 3]) -> [f32; 3] {
        let p = Quat { w: 0.0, x: v[0], y: v[1], z: v[2] };
        let r = self.mul(p).mul(self.conj());
        [r.x, r.y, r.z]
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / n, v[1] / n, v[2] / n]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Device frame, right-handed as Android defines it: X right, Y up (top
/// edge), Z out of the SCREEN toward the user; the rear camera looks along
/// -Z. (The first version put Z out of the back: a left-handed frame, and
/// every yaw came out mirrored; the anchor test caught it.) World frame: X
/// east, Y up, Z south, so a phone held upright facing north has world -Z
/// = north in front of it and q = identity.
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    /// world -> device.
    pub q: Quat,
    /// Focal length in pixels at the pane's size.
    pub focal_px: f32,
    pub anchors: Vec<Anchor>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub label: String,
    /// Unit direction in the world frame.
    pub dir: [f32; 3],
}

impl World {
    pub fn new(focal_px: f32) -> World {
        World { q: Quat::IDENTITY, focal_px, anchors: Vec::new() }
    }

    /// Gyro sample: angular rates about the device axes, rad/s, over `dt` s.
    pub fn gyro(&mut self, rate: [f32; 3], dt: f32) {
        let angle = (rate[0] * rate[0] + rate[1] * rate[1] + rate[2] * rate[2]).sqrt() * dt;
        if angle < 1e-7 {
            return;
        }
        let d = Quat::from_axis_angle(rate, -angle);
        self.q = d.mul(self.q).normalized();
    }

    /// Accelerometer (device axes, any scale): gravity should come out along
    /// world +Y; pull the orientation a little toward that.
    pub fn accel(&mut self, a: [f32; 3]) {
        let measured = normalize(a); // gravity direction in the device frame
        let predicted = self.q.rotate([0.0, 1.0, 0.0]); // world up, in device frame
        self.correct(predicted, measured, GRAVITY_GAIN);
    }

    /// Compass heading in degrees (0 = north, 90 = east): the device's
    /// forward (+Z, the rear camera) points at this bearing when held level.
    pub fn heading(&mut self, degrees: f32) {
        let h = degrees.to_radians();
        // World direction the camera should be looking at (level).
        let want_world = [h.sin(), 0.0, -h.cos()];
        let want_dev = self.q.rotate(want_world); // where that bearing currently sits in the device frame
        // Flatten both onto the device's horizontal plane (ignore tilt).
        let up_dev = self.q.rotate([0.0, 1.0, 0.0]);
        let flat = |v: [f32; 3]| {
            let d = dot(v, up_dev);
            normalize([v[0] - up_dev[0] * d, v[1] - up_dev[1] * d, v[2] - up_dev[2] * d])
        };
        self.correct(flat(want_dev), flat([0.0, 0.0, -1.0]), HEADING_GAIN);
    }

    /// The image moved (px) while the gyro predicted `predicted` (px):
    /// treat the difference as a small rotation error about the screen axes.
    pub fn image_correction(&mut self, measured_px: (f32, f32), predicted_px: (f32, f32), gain: f32) {
        let ex = (measured_px.0 - predicted_px.0) / self.focal_px;
        let ey = (measured_px.1 - predicted_px.1) / self.focal_px;
        // Features moving further right than predicted: the camera turned
        // further LEFT (+Y). Further down: it pitched further UP (+X). A
        // device rotation R updates world->device as q' = R^-1 q, so the
        // axis-angle here carries the negative angle, as in `gyro`.
        let d = Quat::from_axis_angle([1.0, 0.0, 0.0], -ey * gain).mul(Quat::from_axis_angle([0.0, 1.0, 0.0], -ex * gain));
        self.q = d.mul(self.q).normalized();
    }

    fn correct(&mut self, predicted: [f32; 3], measured: [f32; 3], gain: f32) {
        let axis = cross(predicted, measured);
        let s = (dot(axis, axis)).sqrt();
        if s < 1e-6 {
            return;
        }
        let angle = s.atan2(dot(predicted, measured).clamp(-1.0, 1.0));
        let d = Quat::from_axis_angle(axis, angle * gain);
        self.q = d.mul(self.q).normalized();
    }

    /// Screen position of a world direction, relative to the pane centre
    /// (+x right, +y down), or None when it is behind the camera.
    pub fn project(&self, dir: [f32; 3]) -> Option<(f32, f32)> {
        let v = self.q.rotate(normalize(dir));
        let depth = -v[2];
        if depth <= 0.05 {
            return None;
        }
        Some((self.focal_px * v[0] / depth, -self.focal_px * v[1] / depth))
    }

    /// Pixel motion the current orientation predicts for a small yaw/pitch
    /// (rad) change: what the tracker should see if the gyro is right.
    pub fn predicted_shift(&self, yaw: f32, pitch: f32) -> (f32, f32) {
        // Turning left (+yaw) moves the picture right; pitching up (+pitch)
        // moves it down (screen y grows downward).
        (self.focal_px * yaw.tan(), self.focal_px * pitch.tan())
    }

    /// Place an anchor where the camera is looking now.
    pub fn drop_anchor(&mut self, label: &str) {
        let dir = self.q.conj().rotate([0.0, 0.0, -1.0]);
        self.anchors.push(Anchor { label: label.into(), dir });
    }

    /// Yaw (heading) the camera faces, degrees, 0 = north, 90 = east.
    pub fn yaw_deg(&self) -> f32 {
        let f = self.q.conj().rotate([0.0, 0.0, -1.0]);
        let deg = f[0].atan2(-f[2]).to_degrees();
        (deg + 360.0) % 360.0
    }

    /// Pitch of the camera axis above the horizon, degrees.
    pub fn pitch_deg(&self) -> f32 {
        let f = self.q.conj().rotate([0.0, 0.0, -1.0]);
        f[1].asin().to_degrees()
    }
}

pub const DEG: f32 = PI / 180.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn a_dropped_anchor_stays_put_while_the_phone_yaws() {
        let mut w = World::new(400.0);
        w.drop_anchor("door"); // straight ahead
        assert_eq!(w.project(w.anchors[0].dir).map(|p| (p.0.round(), p.1.round())), Some((0.0, 0.0)));
        // Turn left 10 degrees (positive rate about device Y) over 1 s at 100 Hz.
        for _ in 0..100 {
            w.gyro([0.0, 10.0 * DEG, 0.0], 0.01);
        }
        let p = w.project(w.anchors[0].dir).unwrap();
        assert!(close(p.0, 400.0 * (10.0 * DEG).tan(), 1.0), "moved right by focal*tan(10): {}", p.0);
        assert!(close(p.1, 0.0, 0.5));
        assert!(close(w.yaw_deg(), 350.0, 0.5), "{}", w.yaw_deg());
        // Turn back: it returns to centre.
        for _ in 0..100 {
            w.gyro([0.0, -10.0 * DEG, 0.0], 0.01);
        }
        let p = w.project(w.anchors[0].dir).unwrap();
        assert!(close(p.0, 0.0, 0.5) && close(p.1, 0.0, 0.5), "{p:?}");
    }

    #[test]
    fn pitching_moves_anchors_vertically_and_behind_is_hidden() {
        let mut w = World::new(400.0);
        w.drop_anchor("x");
        for _ in 0..100 {
            w.gyro([15.0 * DEG, 0.0, 0.0], 0.01); // tilt the top edge back (look up)
        }
        let p = w.project(w.anchors[0].dir).unwrap();
        assert!(close(p.0, 0.0, 0.5), "{p:?}");
        assert!(close(p.1.abs(), 400.0 * (15.0 * DEG).tan(), 1.0), "{p:?}");
        for _ in 0..1000 {
            w.gyro([0.0, 18.0 * DEG, 0.0], 0.01); // 180 degrees
        }
        assert_eq!(w.project(w.anchors[0].dir), None, "behind the camera");
    }

    #[test]
    fn gravity_pulls_a_drifted_orientation_back_slowly() {
        let mut w = World::new(400.0);
        // Gyro says we rolled 20 degrees; the accelerometer says we did not.
        for _ in 0..100 {
            w.gyro([0.0, 0.0, 20.0 * DEG], 0.01);
        }
        let before = w.q.rotate([0.0, 1.0, 0.0]);
        assert!(!close(before[0], 0.0, 0.05), "rolled: up is no longer device up");
        w.accel([0.0, 9.81, 0.0]);
        let after1 = w.q.rotate([0.0, 1.0, 0.0]);
        assert!(after1[0].abs() < before[0].abs() && after1[0].abs() > before[0].abs() * 0.9, "one sample: a small step ({} -> {})", before[0], after1[0]);
        for _ in 0..600 {
            w.accel([0.0, 9.81, 0.0]);
        }
        let up = w.q.rotate([0.0, 1.0, 0.0]);
        assert!(close(up[0], 0.0, 0.01) && close(up[1], 1.0, 0.01), "settled: {up:?}");
    }

    #[test]
    fn the_compass_sets_the_yaw_without_touching_the_tilt() {
        let mut w = World::new(400.0);
        for _ in 0..1000 {
            w.heading(90.0); // facing east
        }
        assert!(close(w.yaw_deg(), 90.0, 1.0), "{}", w.yaw_deg());
        assert!(close(w.pitch_deg(), 0.0, 0.5));
        // An anchor to the north now sits 90 degrees to the left: not visible.
        assert_eq!(w.project([0.0, 0.0, -1.0]), None);
        // Facing north-east, north is 45 degrees left: on screen at -focal.
        for _ in 0..1000 {
            w.heading(45.0);
        }
        let p = w.project([0.0, 0.0, -1.0]).unwrap();
        assert!(close(p.0, -400.0, 4.0), "{p:?}");
    }

    #[test]
    fn image_motion_corrects_a_gyro_that_under_reports() {
        let mut w = World::new(400.0);
        w.drop_anchor("a");
        // The phone really turned 10 degrees left, the gyro said 8.
        for _ in 0..100 {
            w.gyro([0.0, 8.0 * DEG, 0.0], 0.01);
        }
        let predicted = w.predicted_shift(8.0 * DEG, 0.0);
        let measured = w.predicted_shift(10.0 * DEG, 0.0);
        let before = w.project(w.anchors[0].dir).unwrap().0;
        w.image_correction(measured, predicted, 1.0);
        let after = w.project(w.anchors[0].dir).unwrap().0;
        let truth = 400.0 * (10.0 * DEG).tan();
        assert!((after - truth).abs() < (before - truth).abs(), "{before} -> {after}, truth {truth}");
        assert!((after - truth).abs() < 2.0);
    }
}
