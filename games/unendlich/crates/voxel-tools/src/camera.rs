//! Camera: showing a build the way a builder shows one.
//!
//! A build video never cuts to a structure from wherever the camera
//! happens to be. It frames the thing — three-quarter view, slightly
//! elevated, the whole silhouette in shot with air around it — then
//! moves: an orbit to show all sides, a reveal that pulls back from a
//! detail to the whole, a fly-through that runs along the paths.
//!
//! None of that is the engine's business to invent per build, and none
//! of it needs the model. Given a bounding box and a field of view the
//! framing is trigonometry, and the paths are interpolation.
//!
//! This module computes poses; the game plays them. Poses are in
//! world units with a look-at target, which is the one representation
//! every camera can consume without knowing the engine's yaw/pitch
//! conventions.

/// Where the camera is and what it looks at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub eye: [f32; 3],
    pub look_at: [f32; 3],
    /// Seconds to hold or to reach this pose from the previous one.
    pub seconds: f32,
}

impl Pose {
    pub fn forward(&self) -> [f32; 3] {
        let d = [self.look_at[0] - self.eye[0], self.look_at[1] - self.eye[1],
                 self.look_at[2] - self.eye[2]];
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        [d[0] / n, d[1] / n, d[2] / n]
    }
    pub fn distance(&self) -> f32 {
        let d = [self.look_at[0] - self.eye[0], self.look_at[1] - self.eye[1],
                 self.look_at[2] - self.eye[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }
}

/// A box in world blocks, inclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subject {
    pub min: (i32, i32, i32),
    pub max: (i32, i32, i32),
}

impl Subject {
    pub fn centre(&self) -> [f32; 3] {
        [(self.min.0 + self.max.0 + 1) as f32 / 2.0,
         (self.min.1 + self.max.1 + 1) as f32 / 2.0,
         (self.min.2 + self.max.2 + 1) as f32 / 2.0]
    }
    /// Half the diagonal: the radius of the sphere the box fits in,
    /// which is what a framing distance is computed from so the
    /// subject fits at ANY orientation.
    pub fn radius(&self) -> f32 {
        let w = (self.max.0 - self.min.0 + 1) as f32;
        let h = (self.max.1 - self.min.1 + 1) as f32;
        let d = (self.max.2 - self.min.2 + 1) as f32;
        (w * w + h * h + d * d).sqrt() / 2.0
    }
    pub fn base_y(&self) -> f32 { self.min.1 as f32 }
}

/// The distance at which a sphere of `radius` fills `fraction` of the
/// vertical field of view. Standard framing: r / sin(fov/2), scaled so
/// the subject does not touch the frame edge.
pub fn framing_distance(radius: f32, fov_y_radians: f32, fraction: f32) -> f32 {
    let half = (fov_y_radians / 2.0).clamp(0.05, 1.5);
    (radius / half.sin()) / fraction.clamp(0.2, 1.0)
}

/// The hero shot: three-quarter view, elevated, from the side the
/// player is on so the camera does not swing round behind the subject.
///
/// Elevation of ~25 degrees is the builder's convention. Lower and the
/// roof disappears; higher and it becomes a plan.
pub fn frame(s: &Subject, fov_y: f32, from: [f32; 3]) -> Pose {
    let c = s.centre();
    let dist = framing_distance(s.radius(), fov_y, 0.8);
    // Azimuth toward the player, then offset 45 degrees for the
    // three-quarter view.
    let dx = from[0] - c[0];
    let dz = from[2] - c[2];
    let base_az = dz.atan2(dx);
    let az = base_az + std::f32::consts::FRAC_PI_4;
    let elev = 25.0f32.to_radians();
    let eye = [
        c[0] + dist * elev.cos() * az.cos(),
        c[1] + dist * elev.sin(),
        c[2] + dist * elev.cos() * az.sin(),
    ];
    Pose { eye, look_at: c, seconds: 0.0 }
}

/// A full orbit at hero elevation. `steps` poses, evenly spaced.
pub fn orbit(s: &Subject, fov_y: f32, steps: usize, seconds: f32) -> Vec<Pose> {
    let c = s.centre();
    let dist = framing_distance(s.radius(), fov_y, 0.8);
    let elev = 25.0f32.to_radians();
    let steps = steps.max(4);
    (0..=steps).map(|i| {
        let az = std::f32::consts::TAU * i as f32 / steps as f32;
        Pose {
            eye: [c[0] + dist * elev.cos() * az.cos(),
                  c[1] + dist * elev.sin(),
                  c[2] + dist * elev.cos() * az.sin()],
            look_at: c,
            // The first pose is where the orbit STARTS; it takes no time
            // to reach. Charging it a segment made a 6-second orbit take
            // 6.5, which is the kind of drift nobody notices until a
            // reveal and an orbit are chained and the cut lands late.
            seconds: if i == 0 { 0.0 } else { seconds / steps as f32 },
        }
    }).collect()
}

/// The reveal: start close and low on a detail, pull back and up until
/// the whole thing is in frame. Three poses; the game eases between.
pub fn reveal(s: &Subject, fov_y: f32, from: [f32; 3], seconds: f32) -> Vec<Pose> {
    let end = frame(s, fov_y, from);
    let c = s.centre();
    let f = end.forward();
    // Start a few blocks off the subject's base on the same line,
    // looking at its foot rather than its centre.
    let near = s.radius() * 0.35 + 3.0;
    let start = Pose {
        eye: [c[0] - f[0] * near, s.base_y() + 2.0, c[2] - f[2] * near],
        look_at: [c[0], s.base_y() + 1.0, c[2]],
        seconds: 0.0,
    };
    let mid = Pose {
        eye: [(start.eye[0] + end.eye[0]) / 2.0,
              (start.eye[1] + end.eye[1]) / 2.0 + s.radius() * 0.2,   // arc, not a line
              (start.eye[2] + end.eye[2]) / 2.0],
        look_at: [(start.look_at[0] + c[0]) / 2.0, (start.look_at[1] + c[1]) / 2.0,
                  (start.look_at[2] + c[2]) / 2.0],
        seconds: seconds * 0.5,
    };
    vec![start, mid, Pose { seconds: seconds * 0.5, ..end }]
}

/// A path through waypoints at a fixed height above them, looking
/// ahead. For a village this is the route along its own paths; for a
/// hall it is the walk from the door to the far end.
///
/// The look-at leads the eye by one waypoint so the camera turns into
/// corners rather than after them.
pub fn flythrough(waypoints: &[(i32, i32, i32)], height: f32, seconds: f32) -> Vec<Pose> {
    if waypoints.len() < 2 { return Vec::new(); }
    let per = seconds / (waypoints.len() - 1) as f32;
    let pt = |p: (i32, i32, i32)| [p.0 as f32 + 0.5, p.1 as f32 + height, p.2 as f32 + 0.5];
    waypoints.iter().enumerate().map(|(i, &w)| {
        let eye = pt(w);
        let ahead = waypoints.get(i + 1).copied().unwrap_or(w);
        let mut look = pt(ahead);
        look[1] -= height * 0.5;    // look slightly down at the path
        Pose { eye, look_at: look, seconds: if i == 0 { 0.0 } else { per } }
    }).collect()
}

/// Ease-in-out between two poses. The game calls this per frame with
/// t in 0..1; it is here so every consumer eases the same way.
pub fn lerp(a: &Pose, b: &Pose, t: f32) -> Pose {
    let t = t.clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    let mix = |p: [f32; 3], q: [f32; 3]| [p[0] + (q[0] - p[0]) * s,
                                          p[1] + (q[1] - p[1]) * s,
                                          p[2] + (q[2] - p[2]) * s];
    Pose { eye: mix(a.eye, b.eye), look_at: mix(a.look_at, b.look_at), seconds: b.seconds }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tower() -> Subject { Subject { min: (0, 64, 0), max: (8, 80, 8) } }
    const FOV: f32 = 1.2;  // ~70 degrees

    #[test]
    fn the_hero_shot_fits_the_whole_subject_in_frame() {
        let s = tower();
        let p = frame(&s, FOV, [-20.0, 66.0, -20.0]);
        // Every corner of the box must be within the view cone: angle
        // from the forward vector less than half the FOV. Uses the
        // diagonal-based radius, so this holds at any orientation.
        let f = p.forward();
        for &(x, y, z) in &[(s.min.0, s.min.1, s.min.2), (s.max.0 + 1, s.max.1 + 1, s.max.2 + 1),
                            (s.min.0, s.max.1 + 1, s.max.2 + 1), (s.max.0 + 1, s.min.1, s.min.2)] {
            let d = [x as f32 - p.eye[0], y as f32 - p.eye[1], z as f32 - p.eye[2]];
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let cos = (d[0] * f[0] + d[1] * f[1] + d[2] * f[2]) / n;
            let ang = cos.clamp(-1.0, 1.0).acos();
            assert!(ang < FOV / 2.0, "corner ({x},{y},{z}) is outside the frame: {ang:.2} rad");
        }
    }

    #[test]
    fn the_hero_shot_is_elevated_and_on_the_players_side() {
        let s = tower();
        let p = frame(&s, FOV, [-30.0, 66.0, 0.0]);
        assert!(p.eye[1] > s.centre()[1], "above the centre: the roof must be visible");
        assert!(p.eye[1] < s.centre()[1] + p.distance() * 0.6, "but not a plan view");
        // the player is west; the camera stays on the western half so
        // it does not swing round behind the subject
        assert!(p.eye[0] < s.centre()[0], "camera on the player's side: eye x {}", p.eye[0]);
    }

    #[test]
    fn a_bigger_subject_gets_a_longer_shot() {
        let small = frame(&Subject { min: (0, 64, 0), max: (4, 68, 4) }, FOV, [-10.0, 66.0, -10.0]);
        let big = frame(&Subject { min: (0, 64, 0), max: (40, 90, 40) }, FOV, [-10.0, 66.0, -10.0]);
        assert!(big.distance() > small.distance() * 3.0);
    }

    #[test]
    fn an_orbit_closes_and_keeps_its_distance() {
        let s = tower();
        let o = orbit(&s, FOV, 12, 6.0);
        assert_eq!(o.len(), 13, "steps + 1 so the loop closes");
        let d0 = o[0].distance();
        for p in &o {
            assert!((p.distance() - d0).abs() < 0.01, "constant radius");
            assert_eq!(p.look_at, s.centre(), "always looking at the subject");
        }
        assert!((o[0].eye[0] - o[12].eye[0]).abs() < 0.01, "ends where it began");
        let total: f32 = o.iter().map(|p| p.seconds).sum();
        assert!((total - 6.0).abs() < 0.01, "timing adds up to what was asked");
    }

    #[test]
    fn the_reveal_starts_close_and_low_and_ends_on_the_hero_shot() {
        let s = tower();
        let r = reveal(&s, FOV, [-20.0, 66.0, -20.0], 4.0);
        assert_eq!(r.len(), 3);
        assert!(r[0].distance() < r[2].distance(), "pulls back");
        assert!(r[0].eye[1] < r[2].eye[1], "and rises");
        assert!(r[0].eye[1] < s.base_y() + 4.0, "starts near the foot");
        assert_eq!(r[2].look_at, s.centre(), "ends framing the whole");
        assert!(r[1].eye[1] > (r[0].eye[1] + r[2].eye[1]) / 2.0, "the middle arcs up, not a straight line");
    }

    #[test]
    fn a_flythrough_looks_ahead_into_corners() {
        let path = [(0, 64, 0), (10, 64, 0), (10, 64, 10), (20, 64, 10)];
        let f = flythrough(&path, 2.0, 6.0);
        assert_eq!(f.len(), 4);
        // at the first corner the camera is at (10,_,0) and already
        // looking toward (10,_,10): it turns INTO the corner
        let fwd = f[1].forward();
        assert!(fwd[2] > 0.5, "looking down the next leg: {fwd:?}");
        assert!(f[0].seconds == 0.0 && f[1].seconds > 0.0);
        assert!(flythrough(&[(0, 0, 0)], 2.0, 1.0).is_empty(), "one point is not a path");
    }

    #[test]
    fn lerp_eases_and_stays_between_its_ends() {
        let a = Pose { eye: [0.0, 0.0, 0.0], look_at: [1.0, 0.0, 0.0], seconds: 0.0 };
        let b = Pose { eye: [10.0, 0.0, 0.0], look_at: [11.0, 0.0, 0.0], seconds: 1.0 };
        assert_eq!(lerp(&a, &b, 0.0).eye, a.eye);
        assert_eq!(lerp(&a, &b, 1.0).eye, b.eye);
        let q = lerp(&a, &b, 0.25).eye[0];
        assert!(q < 2.5, "eased in: slower at the start ({q})");
        assert!(lerp(&a, &b, 0.5).eye[0] == 5.0, "symmetric at the middle");
    }
}
