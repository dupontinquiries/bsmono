//! The fat cursor: an iPadOS-style translucent disc that eases after the
//! finger instead of snapping, shrinks when pressed and fades when idle.
//!
//! It exists while the trackpad drives the pointer, because only then does
//! the shell know where the pointer is: it OWNS the position. Trackpad deltas
//! move `target`; `shown` eases after it (exponential ease-out); the caller
//! sends `shown` to the compositor as an absolute pointer position, so a
//! click lands exactly on the drawn disc, not on where the finger's raw
//! motion would have put it.
//!
//! Ease-out, not a spring: each frame covers `1 - e^(-RATE*dt)` of the
//! remaining distance, so motion starts immediately and decelerates into
//! place. A critically damped spring was the first version; from rest it
//! moves ~2% in the first 16 ms frame (displacement grows from zero
//! velocity), which the tests exposed as start-of-motion lag, wrong for
//! direct manipulation. The exponential is frame-rate independent by
//! construction (pinned by a test) and cannot overshoot.
//!
//! Not possible here: morphing onto other apps' buttons. That needs their
//! widget geometry, which no Wayland protocol exposes.

use crate::Millis;

pub const RADIUS: f32 = 17.0;
pub const RADIUS_PRESSED: f32 = 12.0;
/// Ease-out rate (1/s). 22: ~30% of the way in the first 16 ms frame, ~90%
/// within 100 ms.
pub const RATE: f32 = 22.0;
pub const IDLE_FADE_AFTER: Millis = 1500;
pub const FADE_MS: f32 = 300.0;

#[derive(Debug, Clone, PartialEq)]
pub struct FatCursor {
    pub target: (f32, f32),
    pub shown: (f32, f32),
    pub radius: f32,
    pub opacity: f32,
    pressed: bool,
    last_motion: Millis,
    screen: (f32, f32),
    /// Last position reported by `tick`, so unchanged frames send nothing.
    reported: Option<(f32, f32)>,
}

impl FatCursor {
    /// Starts centred: the shell cannot know where the system pointer was,
    /// so it takes ownership by placing it (the first tick reports this).
    pub fn new(screen: (f32, f32), now: Millis) -> FatCursor {
        let c = (screen.0 / 2.0, screen.1 / 2.0);
        FatCursor { target: c, shown: c, radius: RADIUS, opacity: 1.0, pressed: false, last_motion: now, screen, reported: None }
    }

    pub fn set_screen(&mut self, screen: (f32, f32)) {
        self.screen = screen;
        self.target = self.clamp(self.target);
    }

    fn clamp(&self, p: (f32, f32)) -> (f32, f32) {
        (p.0.clamp(0.0, (self.screen.0 - 1.0).max(0.0)), p.1.clamp(0.0, (self.screen.1 - 1.0).max(0.0)))
    }

    /// Relative motion from the trackpad (already gain-scaled).
    pub fn nudge(&mut self, dx: f32, dy: f32, now: Millis) {
        self.target = self.clamp((self.target.0 + dx, self.target.1 + dy));
        self.last_motion = now;
    }

    /// Absolute position, in px, from a source that knows where on the screen it means (the eye tracker): the
    /// target goes there and the disc eases after it, exactly as it does after a trackpad nudge.
    pub fn place(&mut self, x: f32, y: f32, now: Millis) {
        self.target = self.clamp((x, y));
        self.last_motion = now;
    }

    pub fn press(&mut self, down: bool, now: Millis) {
        self.pressed = down;
        self.last_motion = now;
    }

    pub fn is_settled(&self) -> bool {
        let d = ((self.target.0 - self.shown.0).powi(2) + (self.target.1 - self.shown.1).powi(2)).sqrt();
        d < 0.25
    }

    /// Advance by `dt_ms`. Returns the new shown position when it changed
    /// by at least a quarter pixel since the last report — the absolute
    /// position to send to the compositor.
    pub fn tick(&mut self, now: Millis, dt_ms: f32) -> Option<(f32, f32)> {
        let dt = (dt_ms / 1000.0).clamp(0.0, 0.1);
        let keep = (-RATE * dt).exp();
        self.shown = (self.target.0 + (self.shown.0 - self.target.0) * keep, self.target.1 + (self.shown.1 - self.target.1) * keep);
        if self.is_settled() {
            self.shown = self.target;
        }
        // Radius and opacity ease exponentially toward their targets.
        let k = 1.0 - (-dt_ms / 60.0).exp();
        let r_target = if self.pressed { RADIUS_PRESSED } else { RADIUS };
        self.radius += (r_target - self.radius) * k;
        let idle = now.saturating_sub(self.last_motion);
        self.opacity = if self.pressed || idle < IDLE_FADE_AFTER {
            1.0
        } else {
            (1.0 - (idle - IDLE_FADE_AFTER) as f32 / FADE_MS).clamp(0.0, 1.0)
        };
        let moved = self.reported.is_none_or(|r| (r.0 - self.shown.0).abs() >= 0.25 || (r.1 - self.shown.1).abs() >= 0.25);
        if moved {
            self.reported = Some(self.shown);
            Some(self.shown)
        } else {
            None
        }
    }

    pub fn is_animating(&self) -> bool {
        !self.is_settled() || (self.radius - if self.pressed { RADIUS_PRESSED } else { RADIUS }).abs() > 0.05 || (self.opacity > 0.0 && self.opacity < 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: (f32, f32) = (400.0, 800.0);

    fn run(c: &mut FatCursor, from: Millis, frames: u32, dt: u32) -> Vec<(f32, f32)> {
        (1..=frames).filter_map(|i| c.tick(from + (i * dt) as Millis, dt as f32)).collect()
    }

    #[test]
    fn eases_toward_target_without_overshoot_and_settles() {
        let mut c = FatCursor::new(S, 0);
        c.tick(0, 16.0);
        c.nudge(150.0, 0.0, 0);
        let path = run(&mut c, 0, 60, 16);
        assert!(path.len() > 5, "scan must see intermediate frames: {}", path.len());
        let xs: Vec<f32> = path.iter().map(|p| p.0).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0]), "monotonic, no overshoot: {xs:?}");
        assert!(xs.iter().all(|x| *x <= 350.0 + 1e-3));
        // Ease-out: the first frame already covers ~30% (44.6 of 150 px).
        assert!(xs[0] > 235.0 && xs[0] < 255.0, "first frame moves a real part of the way: {}", xs[0]);
        assert_eq!(c.shown, (350.0, 400.0));
        assert!(c.is_settled());
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = FatCursor::new(S, 0);
        let mut b = FatCursor::new(S, 0);
        a.nudge(200.0, 100.0, 0);
        b.nudge(200.0, 100.0, 0);
        run(&mut a, 0, 6, 16);
        run(&mut b, 0, 12, 8);
        assert!((a.shown.0 - b.shown.0).abs() < 0.01 && (a.shown.1 - b.shown.1).abs() < 0.01, "{:?} vs {:?}", a.shown, b.shown);
    }

    #[test]
    fn clamped_to_screen() {
        let mut c = FatCursor::new(S, 0);
        c.nudge(-5000.0, 9000.0, 0);
        assert_eq!(c.target, (0.0, 799.0));
    }

    #[test]
    fn press_shrinks_and_release_restores() {
        let mut c = FatCursor::new(S, 0);
        c.press(true, 0);
        run(&mut c, 0, 30, 16);
        assert!((c.radius - RADIUS_PRESSED).abs() < 0.1, "{}", c.radius);
        c.press(false, 500);
        run(&mut c, 500, 30, 16);
        assert!((c.radius - RADIUS).abs() < 0.1);
    }

    #[test]
    fn fades_after_idle_and_returns_on_motion() {
        let mut c = FatCursor::new(S, 0);
        c.tick(IDLE_FADE_AFTER - 1, 16.0);
        assert_eq!(c.opacity, 1.0);
        c.tick(IDLE_FADE_AFTER + (FADE_MS as u64) / 2, 16.0);
        assert!(c.opacity > 0.4 && c.opacity < 0.6, "{}", c.opacity);
        c.tick(IDLE_FADE_AFTER + FADE_MS as u64 + 10, 16.0);
        assert_eq!(c.opacity, 0.0);
        c.nudge(1.0, 0.0, 5000);
        c.tick(5016, 16.0);
        assert_eq!(c.opacity, 1.0);
    }

    #[test]
    fn reports_only_when_moved_and_first_tick_takes_ownership() {
        let mut c = FatCursor::new(S, 0);
        assert_eq!(c.tick(16, 16.0), Some((200.0, 400.0)), "first tick places the pointer");
        assert_eq!(c.tick(32, 16.0), None, "settled: nothing to send");
        c.nudge(10.0, 0.0, 40);
        assert!(c.tick(48, 16.0).is_some());
    }

    #[test]
    fn small_nudges_move_on_the_very_next_frame() {
        // The regression that retired the spring: 10 px must show up at once.
        let mut c = FatCursor::new(S, 0);
        c.tick(0, 16.0);
        c.nudge(10.0, 0.0, 0);
        let p = c.tick(16, 16.0).expect("moved");
        assert!(p.0 - 200.0 > 2.0, "{}", p.0);
    }
}
