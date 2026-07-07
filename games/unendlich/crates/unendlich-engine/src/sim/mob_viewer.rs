//! sim::mob_viewer — orbit / close-up camera state for the specimen viewer.
//!
//! From `mob_dragon_viewer_ui §8` (COLLECTIONS panel): the creature is inspected
//! on a turntable — the camera orbits it, zooms, and on selecting a body region
//! pushes in to a close-up (0.3 units from the surface) over a 280 ms smoothstep
//! transition, then eases back out. This is the pure camera-state model behind
//! that (distinct from `creature_viewer`, which is a 2D orthographic stats
//! projection): orbit yaw/pitch/zoom with clamping, a region focus with an
//! animated in/out transition, and the resulting eye offset. No rendering.
//! Deterministic, std-only.

/// Close-up transition duration (seconds) — the doc's 280 ms.
pub const TRANSITION_SECS: f32 = 0.280;
/// Distance from the specimen when fully zoomed into a region.
pub const CLOSEUP_DISTANCE: f32 = 0.3;

const MIN_DISTANCE: f32 = 0.5;
const MAX_DISTANCE: f32 = 12.0;
// keep the eye out of the poles so the turntable never gimbals over the top
const MAX_PITCH: f32 = 1.4; // ~80°
const TAU: f32 = std::f32::consts::TAU;

/// Cubic smoothstep on `[0,1]`.
pub fn smoothstep(x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The inspection camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobViewer {
    yaw: f32,
    pitch: f32,
    /// Resting orbit distance (what zoom controls).
    orbit_distance: f32,
    /// Close-up transition progress, 0 = full orbit … 1 = at the region.
    t: f32,
    /// True while a region is selected (drives `t` toward 1; clearing drives 0).
    focusing: bool,
    focus_region: Option<u32>,
}

impl Default for MobViewer {
    fn default() -> Self {
        MobViewer {
            yaw: 0.0,
            pitch: 0.3,
            orbit_distance: 5.0,
            t: 0.0,
            focusing: false,
            focus_region: None,
        }
    }
}

impl MobViewer {
    pub fn new() -> MobViewer {
        MobViewer::default()
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }
    pub fn pitch(&self) -> f32 {
        self.pitch
    }
    pub fn transition(&self) -> f32 {
        self.t
    }
    pub fn focus_region(&self) -> Option<u32> {
        self.focus_region
    }

    /// Turntable drag: add to yaw (wraps `0..2π`) and pitch (clamped off the
    /// poles).
    pub fn rotate(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw = (self.yaw + dyaw).rem_euclid(TAU);
        self.pitch = (self.pitch + dpitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Zoom the resting orbit distance (negative = closer), clamped.
    pub fn zoom(&mut self, delta: f32) {
        self.orbit_distance = (self.orbit_distance + delta).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Select a body region → push in to a close-up (animates over
    /// [`TRANSITION_SECS`]).
    pub fn focus(&mut self, region: u32) {
        self.focus_region = Some(region);
        self.focusing = true;
    }

    /// Return to the orbit view (animates back out; region cleared once fully
    /// out — see [`update`]).
    pub fn clear_focus(&mut self) {
        self.focusing = false;
    }

    /// Advance the transition by `dt` seconds toward the current target
    /// (1 while focusing, 0 otherwise). Clears the region once fully zoomed out.
    pub fn update(&mut self, dt: f32) {
        let rate = if TRANSITION_SECS > 0.0 { dt / TRANSITION_SECS } else { 1.0 };
        let target = if self.focusing { 1.0 } else { 0.0 };
        if self.t < target {
            self.t = (self.t + rate).min(target);
        } else if self.t > target {
            self.t = (self.t - rate).max(target);
        }
        if !self.focusing && self.t <= 0.0 {
            self.focus_region = None;
        }
    }

    /// Are we in (or transitioning into) a region close-up?
    pub fn is_inspecting(&self) -> bool {
        self.focus_region.is_some() && self.t > 0.0
    }

    /// The camera's current distance from the specimen — the orbit distance
    /// eased toward the close-up distance by the smoothstepped transition.
    pub fn current_distance(&self) -> f32 {
        let s = smoothstep(self.t);
        self.orbit_distance * (1.0 - s) + CLOSEUP_DISTANCE * s
    }

    /// The eye position offset from the specimen centre, from
    /// (yaw, pitch, current_distance) in spherical coordinates.
    pub fn eye_offset(&self) -> [f32; 3] {
        let d = self.current_distance();
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [d * cp * sy, d * sp, d * cp * cy]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn smoothstep_endpoints_and_midpoint() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert!(approx(smoothstep(0.5), 0.5, 1e-6));
        assert_eq!(smoothstep(-1.0), 0.0, "clamped");
        assert_eq!(smoothstep(2.0), 1.0, "clamped");
    }

    #[test]
    fn rotate_wraps_yaw_and_clamps_pitch() {
        let mut v = MobViewer::new();
        v.rotate(TAU + 0.5, 0.0);
        assert!(approx(v.yaw(), 0.5, 1e-4), "yaw wraps into [0,2π): {}", v.yaw());
        assert!(v.yaw() >= 0.0 && v.yaw() < TAU);
        v.rotate(0.0, 100.0);
        assert!(approx(v.pitch(), MAX_PITCH, 1e-4), "pitch clamped near +80°");
        v.rotate(0.0, -100.0);
        assert!(approx(v.pitch(), -MAX_PITCH, 1e-4));
    }

    #[test]
    fn zoom_clamps_distance() {
        let mut v = MobViewer::new();
        v.zoom(-100.0);
        assert!(approx(v.current_distance(), MIN_DISTANCE, 1e-4));
        v.zoom(1000.0);
        assert!(approx(v.current_distance(), MAX_DISTANCE, 1e-4));
    }

    #[test]
    fn focus_animates_in_over_the_transition_then_reaches_closeup() {
        let mut v = MobViewer::new();
        v.zoom(0.0); // orbit distance 5.0
        v.focus(7);
        assert_eq!(v.focus_region(), Some(7));
        // one full transition worth of dt → t == 1
        v.update(TRANSITION_SECS);
        assert!(approx(v.transition(), 1.0, 1e-4));
        assert!(approx(v.current_distance(), CLOSEUP_DISTANCE, 1e-4), "at the region surface");
        assert!(v.is_inspecting());
    }

    #[test]
    fn clearing_focus_eases_back_and_drops_the_region() {
        let mut v = MobViewer::new();
        v.focus(3);
        v.update(TRANSITION_SECS); // fully in
        v.clear_focus();
        assert_eq!(v.focus_region(), Some(3), "still set mid-transition");
        v.update(TRANSITION_SECS); // fully out
        assert!(approx(v.transition(), 0.0, 1e-4));
        assert_eq!(v.focus_region(), None, "region cleared once fully out");
        assert!(!v.is_inspecting());
    }

    #[test]
    fn partial_updates_accumulate() {
        let mut v = MobViewer::new();
        v.focus(1);
        v.update(TRANSITION_SECS / 2.0);
        assert!(approx(v.transition(), 0.5, 1e-4));
        v.update(TRANSITION_SECS / 2.0);
        assert!(approx(v.transition(), 1.0, 1e-4));
    }

    #[test]
    fn eye_offset_sits_at_the_current_distance() {
        let mut v = MobViewer::new();
        v.rotate(0.7, 0.2);
        let o = v.eye_offset();
        let mag = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
        assert!(approx(mag, v.current_distance(), 1e-3), "eye lies on the orbit sphere");
    }

    #[test]
    fn deterministic() {
        let mut a = MobViewer::new();
        let mut b = MobViewer::new();
        a.rotate(0.3, 0.1);
        a.focus(2);
        a.update(0.1);
        b.rotate(0.3, 0.1);
        b.focus(2);
        b.update(0.1);
        assert_eq!(a, b);
    }
}
