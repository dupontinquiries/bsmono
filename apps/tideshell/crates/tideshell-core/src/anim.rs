//! The one animation primitive behind every drag-driven surface (lock screen
//! reveal, sleep fade, and the drawer/switcher/bar as they move onto it):
//! while a finger is down, progress follows it 1:1; on release it commits to
//! 0 or 1 by distance OR flick velocity, then settles on a critically damped
//! spring (exact per step, frame-rate independent, no overshoot) seeded with
//! the finger's release velocity, so the motion continues out of the gesture
//! instead of restarting from rest.

use crate::ambient::spring_step;
use crate::Millis;

/// Past this, a release commits by distance.
pub const COMMIT_AT: f32 = 0.35;
/// Release speed (progress per second) that commits regardless of distance.
pub const FLICK: f32 = 1.5;
pub const OMEGA: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragTransition {
    pub progress: f32,
    velocity: f32,
    target: f32,
    dragging: bool,
    last: Option<(Millis, f32)>,
}

impl Default for DragTransition {
    fn default() -> Self {
        DragTransition { progress: 0.0, velocity: 0.0, target: 0.0, dragging: false, last: None }
    }
}

impl DragTransition {
    pub fn at(progress: f32) -> Self {
        DragTransition { progress, target: progress, ..Default::default() }
    }

    /// Finger position mapped to progress (the caller owns the geometry).
    pub fn drag(&mut self, progress: f32, now: Millis) {
        let p = progress.clamp(0.0, 1.0);
        if let Some((t, prev)) = self.last {
            let dt = now.saturating_sub(t) as f32 / 1000.0;
            if dt > 0.0 {
                // Smoothed velocity: one noisy sample at lift must not decide.
                self.velocity = 0.5 * self.velocity + 0.5 * (p - prev) / dt;
            }
        }
        self.dragging = true;
        self.progress = p;
        self.target = p;
        self.last = Some((now, p));
    }

    /// Finger lifted: commit to 1 if far enough or flicked forward, to 0 if
    /// flicked back or not far enough. Returns the committed end.
    pub fn release(&mut self) -> f32 {
        self.dragging = false;
        self.last = None;
        self.target = if self.velocity >= FLICK {
            1.0
        } else if self.velocity <= -FLICK {
            0.0
        } else if self.progress >= COMMIT_AT {
            1.0
        } else {
            0.0
        };
        self.target
    }

    /// Animate to an end without a gesture (a button, a timeout).
    pub fn animate_to(&mut self, target: f32) {
        self.dragging = false;
        self.last = None;
        self.target = target.clamp(0.0, 1.0);
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn is_settled(&self) -> bool {
        !self.dragging && self.progress == self.target && self.velocity == 0.0
    }

    pub fn tick(&mut self, dt_ms: f32) {
        if self.dragging || self.is_settled() {
            return;
        }
        let dt = (dt_ms / 1000.0).clamp(0.0, 0.1);
        let (x, v) = spring_step(self.progress - self.target, self.velocity, OMEGA, dt);
        self.progress = (self.target + x).clamp(0.0, 1.0);
        self.velocity = v;
        if x.abs() < 1e-4 && v.abs() < 1e-3 {
            self.progress = self.target;
            self.velocity = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag_to(t: &mut DragTransition, from: f32, to: f32, ms: u64) {
        let n = (ms / 11).max(1);
        for i in 0..=n {
            t.drag(from + (to - from) * i as f32 / n as f32, i * 11);
        }
    }

    fn settle(t: &mut DragTransition) -> Vec<f32> {
        (0..200).map(|_| {
            t.tick(11.1);
            t.progress
        })
        .collect()
    }

    #[test]
    fn follows_the_finger_one_to_one() {
        let mut t = DragTransition::default();
        t.drag(0.42, 0);
        assert_eq!(t.progress, 0.42);
        t.tick(100.0);
        assert_eq!(t.progress, 0.42, "no animation while the finger is down");
    }

    #[test]
    fn commits_by_distance_or_flick_and_springs_back_otherwise() {
        let mut t = DragTransition::default();
        drag_to(&mut t, 0.0, 0.5, 1000); // slow, far
        assert_eq!(t.release(), 1.0);
        let mut t = DragTransition::default();
        drag_to(&mut t, 0.0, 0.2, 60); // short but fast
        assert_eq!(t.release(), 1.0, "flick commits");
        let mut t = DragTransition::default();
        drag_to(&mut t, 0.0, 0.2, 1500); // short and slow
        assert_eq!(t.release(), 0.0, "springs back");
        let mut t = DragTransition::at(1.0);
        drag_to(&mut t, 1.0, 0.8, 60); // flick back from open
        assert_eq!(t.release(), 0.0);
    }

    #[test]
    fn settles_without_overshoot_and_continues_the_release_velocity() {
        let mut t = DragTransition::default();
        drag_to(&mut t, 0.0, 0.3, 80);
        t.release();
        let path = settle(&mut t);
        assert!(path.iter().all(|p| *p <= 1.0 && *p >= 0.3 - 1e-6));
        assert!(path.windows(2).all(|w| w[1] >= w[0] - 1e-6), "monotonic");
        assert!(path[0] - 0.3 > 0.01, "the first frame keeps moving (release velocity carried over)");
        assert_eq!(t.progress, 1.0);
        assert!(t.is_settled());
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = DragTransition::default();
        let mut b = DragTransition::default();
        a.animate_to(1.0);
        b.animate_to(1.0);
        for _ in 0..9 {
            a.tick(11.1);
        }
        for _ in 0..6 {
            b.tick(16.65);
        }
        assert!((a.progress - b.progress).abs() < 1e-3, "{} vs {}", a.progress, b.progress);
    }
}
