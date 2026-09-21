//! A touchscreen region that behaves like a laptop touchpad, producing
//! relative pointer events for cosmic-comp's EI socket (the transport; see
//! the `0002-dbus-ei-allow-tideshell` patch for why the shell may open one).
//!
//! Semantics, matching libinput's touchpad defaults so it feels familiar:
//!
//! * one finger moving → pointer motion, with a mild speed-dependent gain
//! * one-finger tap (short, still) → left click
//! * two-finger tap → right click
//! * two fingers moving → scroll (natural direction: content follows fingers)
//! * tap, then touch again within [`TAP_DRAG_MS`] and move → left button held
//!   for the drag ("tap-and-drag"), released when the finger lifts

use crate::Millis;

pub const TAP_MS: Millis = 180;
pub const TAP_SLOP: f32 = 10.0;
pub const TAP_DRAG_MS: Millis = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerOut {
    Motion { dx: f32, dy: f32 },
    Button { button: Button, pressed: bool },
    Scroll { dx: f32, dy: f32 },
    /// Absolute position in screen-logical px (fat cursor mode: the shell
    /// owns the position, see `cursor`).
    Absolute { x: f32, y: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Finger {
    slot: i32,
    start: (f32, f32),
    last: (f32, f32),
    t0: Millis,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trackpad {
    pub speed: f32,
    fingers: Vec<Finger>,
    /// Most fingers down at once in this sequence (decides tap kind).
    peak: usize,
    /// Any finger exceeded tap slop in this sequence.
    moved: bool,
    seq_t0: Millis,
    /// When the last one-finger tap completed, for tap-and-drag.
    last_tap: Option<Millis>,
    drag_lock: bool,
}

impl Default for Trackpad {
    fn default() -> Self {
        Trackpad { speed: 1.0, fingers: Vec::new(), peak: 0, moved: false, seq_t0: 0, last_tap: None, drag_lock: false }
    }
}

impl Trackpad {
    /// Pointer gain for a sample moving `dist` px in `dt` ms: 1.0 when slow
    /// (precision), rising to ~2.4 for fast sweeps (reach the far edge of a
    /// large external display without repeated strokes).
    pub fn gain(dist: f32, dt: f32) -> f32 {
        let v = dist / dt.max(1.0); // px/ms
        1.0 + 1.4 * ((v - 0.3) / 1.2).clamp(0.0, 1.0)
    }

    pub fn finger_count(&self) -> usize {
        self.fingers.len()
    }

    pub fn down(&mut self, slot: i32, x: f32, y: f32, t: Millis) -> Vec<PointerOut> {
        let mut out = vec![];
        if self.fingers.is_empty() {
            self.peak = 0;
            self.moved = false;
            self.seq_t0 = t;
            if self.last_tap.is_some_and(|lt| t.saturating_sub(lt) <= TAP_DRAG_MS) {
                // Second touch of tap-and-drag: hold the button now so the
                // drag starts under the pointer, not after the first move.
                self.drag_lock = true;
                self.last_tap = None;
                out.push(PointerOut::Button { button: Button::Left, pressed: true });
            }
        }
        self.fingers.push(Finger { slot, start: (x, y), last: (x, y), t0: t });
        self.peak = self.peak.max(self.fingers.len());
        out
    }

    pub fn motion(&mut self, slot: i32, x: f32, y: f32, t: Millis) -> Vec<PointerOut> {
        let n = self.fingers.len();
        let Some(i) = self.fingers.iter().position(|f| f.slot == slot) else { return vec![] };
        let f = &mut self.fingers[i];
        let (dx, dy) = (x - f.last.0, y - f.last.1);
        let dt = t.saturating_sub(f.t0).max(1) as f32;
        f.last = (x, y);
        f.t0 = t;
        if (x - f.start.0).hypot(y - f.start.1) > TAP_SLOP {
            self.moved = true;
        }
        match n {
            1 => {
                let g = Self::gain(dx.hypot(dy), dt) * self.speed;
                vec![PointerOut::Motion { dx: dx * g, dy: dy * g }]
            }
            // Two fingers: each emits half the delta, so the pair together
            // scrolls by the average of their motion.
            2 => vec![PointerOut::Scroll { dx: dx * 0.5, dy: dy * 0.5 }],
            _ => vec![],
        }
    }

    pub fn up(&mut self, slot: i32, t: Millis) -> Vec<PointerOut> {
        let Some(i) = self.fingers.iter().position(|f| f.slot == slot) else { return vec![] };
        self.fingers.remove(i);
        if !self.fingers.is_empty() {
            return vec![];
        }
        let quick = t.saturating_sub(self.seq_t0) <= TAP_MS;
        if self.drag_lock {
            self.drag_lock = false;
            return vec![PointerOut::Button { button: Button::Left, pressed: false }];
        }
        if quick && !self.moved {
            let button = if self.peak >= 2 { Button::Right } else { Button::Left };
            if button == Button::Left {
                self.last_tap = Some(t);
            }
            return vec![
                PointerOut::Button { button, pressed: true },
                PointerOut::Button { button, pressed: false },
            ];
        }
        vec![]
    }

    /// Release anything held (overlay hidden, sequence cancelled).
    pub fn cancel(&mut self) -> Vec<PointerOut> {
        self.fingers.clear();
        self.last_tap = None;
        if std::mem::take(&mut self.drag_lock) {
            vec![PointerOut::Button { button: Button::Left, pressed: false }]
        } else {
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(b: Button) -> Vec<PointerOut> {
        vec![PointerOut::Button { button: b, pressed: true }, PointerOut::Button { button: b, pressed: false }]
    }

    #[test]
    fn one_finger_tap_is_left_click() {
        let mut p = Trackpad::default();
        p.down(0, 100.0, 100.0, 0);
        assert_eq!(p.up(0, 100), click(Button::Left));
    }

    #[test]
    fn slow_or_moved_touch_is_not_a_click() {
        let mut p = Trackpad::default();
        p.down(0, 100.0, 100.0, 0);
        assert!(p.up(0, TAP_MS + 1).is_empty(), "too long");
        p.down(0, 100.0, 100.0, 1000);
        p.motion(0, 100.0 + TAP_SLOP + 1.0, 100.0, 1050);
        assert!(p.up(0, 1080).is_empty(), "moved");
    }

    #[test]
    fn two_finger_tap_is_right_click() {
        let mut p = Trackpad::default();
        p.down(0, 100.0, 100.0, 0);
        p.down(1, 140.0, 100.0, 20);
        assert!(p.up(1, 80).is_empty());
        assert_eq!(p.up(0, 100), click(Button::Right));
    }

    #[test]
    fn one_finger_moves_pointer_with_gain() {
        let mut p = Trackpad::default();
        p.down(0, 0.0, 0.0, 0);
        // Slow: 4 px in 16 ms = 0.25 px/ms → gain 1.0.
        assert_eq!(p.motion(0, 4.0, 0.0, 16), vec![PointerOut::Motion { dx: 4.0, dy: 0.0 }]);
        // Fast: 40 px in 16 ms = 2.5 px/ms → gain 2.4.
        match p.motion(0, 44.0, 0.0, 32)[0] {
            PointerOut::Motion { dx, .. } => assert!((dx - 96.0).abs() < 0.01, "{dx}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn gain_is_monotonic_and_bounded() {
        let mut prev = 0.0;
        for i in 0..50 {
            let g = Trackpad::gain(i as f32, 10.0);
            assert!(g >= prev && (1.0..=2.4001).contains(&g));
            prev = g;
        }
    }

    #[test]
    fn two_fingers_scroll_by_the_average() {
        let mut p = Trackpad::default();
        p.down(0, 100.0, 100.0, 0);
        p.down(1, 150.0, 100.0, 0);
        let a = p.motion(0, 100.0, 120.0, 16);
        let b = p.motion(1, 150.0, 120.0, 16);
        assert_eq!(a, vec![PointerOut::Scroll { dx: 0.0, dy: 10.0 }]);
        assert_eq!(b, vec![PointerOut::Scroll { dx: 0.0, dy: 10.0 }]);
        p.up(0, 400);
        assert!(p.up(1, 400).is_empty(), "a scroll is not a click");
    }

    #[test]
    fn tap_then_drag_holds_left_until_lift() {
        let mut p = Trackpad::default();
        p.down(0, 100.0, 100.0, 0);
        p.up(0, 80);
        assert_eq!(p.down(0, 100.0, 100.0, 80 + TAP_DRAG_MS), vec![PointerOut::Button { button: Button::Left, pressed: true }]);
        assert!(matches!(p.motion(0, 160.0, 100.0, 400)[0], PointerOut::Motion { .. }));
        assert_eq!(p.up(0, 700), vec![PointerOut::Button { button: Button::Left, pressed: false }]);
        // A late second touch is a fresh touch, not a drag.
        p.down(0, 100.0, 100.0, 1000);
        p.up(0, 1050);
        assert!(p.down(0, 100.0, 100.0, 1050 + TAP_DRAG_MS + 1).is_empty());
    }

    #[test]
    fn cancel_releases_a_held_button() {
        let mut p = Trackpad::default();
        p.down(0, 0.0, 0.0, 0);
        p.up(0, 50);
        p.down(0, 0.0, 0.0, 100);
        assert_eq!(p.cancel(), vec![PointerOut::Button { button: Button::Left, pressed: false }]);
        assert!(p.cancel().is_empty());
    }
}
