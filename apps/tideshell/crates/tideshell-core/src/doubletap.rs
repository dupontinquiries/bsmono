//! A double tap on the lock screen turns the display off.
//!
//! Strict for the same reason the wake triple tap is: a pocket produces touches. A tap is short and
//! still; two of them must land close together in time and place. A swipe (the unlock gesture), a long
//! press or two taps far apart never count, and the sequence restarts after it fires.

use crate::Millis;

pub const TAP_MAX_MS: Millis = 220;
pub const TAP_SLOP: f32 = 24.0;
/// Press-to-press, the usual double-click feel.
pub const WINDOW_MS: Millis = 380;
pub const CLUSTER: f32 = 70.0;

#[derive(Debug, Default, Clone)]
pub struct DoubleTap {
    /// (slot, down at, x, y, moved)
    down: Option<(i32, Millis, f32, f32, bool)>,
    first: Option<(Millis, f32, f32)>,
}

impl DoubleTap {
    pub fn down(&mut self, slot: i32, x: f32, y: f32, now: Millis) {
        if self.down.is_some() {
            // a second finger: not a deliberate double tap
            self.down = None;
            self.first = None;
            return;
        }
        if self.first.is_some_and(|f| now.saturating_sub(f.0) > WINDOW_MS) {
            self.first = None;
        }
        self.down = Some((slot, now, x, y, false));
    }

    pub fn moved(&mut self, slot: i32, x: f32, y: f32) {
        if let Some((s, _, x0, y0, moved)) = &mut self.down {
            if *s == slot && (x - *x0).hypot(y - *y0) > TAP_SLOP {
                *moved = true;
            }
        }
    }

    /// The finger lifted. True when this completes a double tap.
    pub fn up(&mut self, slot: i32, now: Millis) -> bool {
        let Some((s, t0, x, y, moved)) = self.down else { return false };
        if s != slot {
            return false;
        }
        self.down = None;
        let is_tap = !moved && now.saturating_sub(t0) <= TAP_MAX_MS;
        if !is_tap {
            self.first = None;
            return false;
        }
        match self.first {
            Some((t, fx, fy)) if t0.saturating_sub(t) <= WINDOW_MS && (x - fx).hypot(y - fy) <= CLUSTER => {
                self.first = None;
                true
            }
            _ => {
                self.first = Some((t0, x, y));
                false
            }
        }
    }

    pub fn cancel(&mut self) {
        self.down = None;
        self.first = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(d: &mut DoubleTap, x: f32, y: f32, at: Millis) -> bool {
        d.down(0, x, y, at);
        d.up(0, at + 80)
    }

    #[test]
    fn two_quick_close_taps_fire_and_the_sequence_restarts() {
        let mut d = DoubleTap::default();
        assert!(!tap(&mut d, 100.0, 300.0, 1000));
        assert!(tap(&mut d, 110.0, 305.0, 1250));
        assert!(!tap(&mut d, 110.0, 305.0, 1500), "the next tap starts a new sequence");
        assert!(tap(&mut d, 110.0, 305.0, 1700));
    }

    #[test]
    fn slow_spread_moved_or_long_presses_never_fire() {
        let mut d = DoubleTap::default();
        assert!(!tap(&mut d, 100.0, 300.0, 1000));
        assert!(!tap(&mut d, 100.0, 300.0, 1000 + WINDOW_MS + 50), "too slow");
        let mut d = DoubleTap::default();
        tap(&mut d, 100.0, 300.0, 1000);
        assert!(!tap(&mut d, 400.0, 300.0, 1200), "far apart");
        let mut d = DoubleTap::default();
        tap(&mut d, 100.0, 300.0, 1000);
        d.down(0, 100.0, 300.0, 1200);
        d.moved(0, 100.0, 380.0);
        assert!(!d.up(0, 1280), "a swipe is the unlock gesture");
        let mut d = DoubleTap::default();
        tap(&mut d, 100.0, 300.0, 1000);
        d.down(0, 100.0, 300.0, 1200);
        assert!(!d.up(0, 1200 + TAP_MAX_MS + 100), "a long press");
    }

    #[test]
    fn a_second_finger_cancels() {
        let mut d = DoubleTap::default();
        tap(&mut d, 100.0, 300.0, 1000);
        d.down(0, 100.0, 300.0, 1200);
        d.down(1, 120.0, 300.0, 1210);
        assert!(!d.up(0, 1260));
        assert!(!tap(&mut d, 100.0, 300.0, 1300), "the sequence was dropped");
    }
}
