//! A touchscreen region that behaves like a laptop touchpad, producing
//! relative pointer events for cosmic-comp's EI socket (the transport; see
//! the `0002-dbus-ei-allow-tideshell` patch for why the shell may open one).
//!
//! Semantics, matching libinput's touchpad defaults so it feels familiar:
//!
//! * one finger moving → pointer motion, with a mild speed-dependent gain
//! * one-finger tap (short, still) → left click
//! * two-finger tap → right click
//! * two fingers moving together → scroll (natural: content follows fingers)
//! * two fingers moving apart / together → pinch zoom. The first PINCH_LOCK /
//!   SCROLL_LOCK px of two-finger motion decide which, then the choice locks
//!   for the rest of the gesture (as libinput does), so a scroll never turns
//!   into a zoom half-way. Zoom goes out as ZoomBegin / Zoom{steps} / ZoomEnd:
//!   EI has no gesture interface, so the transport sends Ctrl + wheel notches,
//!   the zoom convention of browsers, viewers, terminals and toolkits.
//! * tap, then touch again within [`TAP_DRAG_MS`] and move → left button held
//!   for the drag ("tap-and-drag"), released when the finger lifts

use crate::Millis;

pub const TAP_MS: Millis = 180;
pub const TAP_SLOP: f32 = 10.0;
pub const TAP_DRAG_MS: Millis = 250;
/// Two-finger distance change that locks the gesture to pinch.
pub const PINCH_LOCK: f32 = 16.0;
/// Two-finger centroid travel that locks it to scroll.
pub const SCROLL_LOCK: f32 = 10.0;
/// Scale change per zoom step (one wheel notch): 12%.
pub const ZOOM_STEP: f32 = 1.12;

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
    /// A pinch started (transport: press Ctrl).
    ZoomBegin,
    /// Zoom by whole steps; positive = zoom in (fingers apart).
    Zoom { steps: i32 },
    /// The pinch ended (transport: release Ctrl). Always paired with ZoomBegin.
    ZoomEnd,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TwoMode {
    Undecided,
    Scroll,
    Pinch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Two {
    mode: TwoMode,
    start_dist: f32,
    start_centroid: (f32, f32),
    last_centroid: (f32, f32),
    /// ln(scale) accumulated but not yet emitted as whole steps.
    zoom_acc: f32,
    last_dist: f32,
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
    two: Option<Two>,
}

impl Default for Trackpad {
    fn default() -> Self {
        Trackpad { speed: 1.0, fingers: Vec::new(), peak: 0, moved: false, seq_t0: 0, last_tap: None, drag_lock: false, two: None }
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
        out.extend(self.retarget_two());
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
            2 => self.two_finger_update(),
            _ => vec![],
        }
    }

    fn geometry(&self) -> Option<(f32, (f32, f32))> {
        let [a, b] = [self.fingers.first()?, self.fingers.get(1)?];
        let d = (a.last.0 - b.last.0).hypot(a.last.1 - b.last.1);
        Some((d, ((a.last.0 + b.last.0) / 2.0, (a.last.1 + b.last.1) / 2.0)))
    }

    /// Finger count changed: end any pinch (always releasing Ctrl), and start
    /// a fresh undecided two-finger gesture when exactly two are down.
    fn retarget_two(&mut self) -> Vec<PointerOut> {
        let mut out = vec![];
        if self.two.is_some_and(|t| t.mode == TwoMode::Pinch) {
            out.push(PointerOut::ZoomEnd);
        }
        self.two = self.geometry().filter(|_| self.fingers.len() == 2).map(|(d, c)| Two {
            mode: TwoMode::Undecided,
            start_dist: d,
            start_centroid: c,
            last_centroid: c,
            zoom_acc: 0.0,
            last_dist: d,
        });
        out
    }

    fn two_finger_update(&mut self) -> Vec<PointerOut> {
        let (Some((d, c)), Some(two)) = (self.geometry(), self.two.as_mut()) else { return vec![] };
        let mut out = vec![];
        if two.mode == TwoMode::Undecided {
            let dd = (d - two.start_dist).abs();
            let dc = (c.0 - two.start_centroid.0).hypot(c.1 - two.start_centroid.1);
            if dd >= PINCH_LOCK && dd > dc {
                two.mode = TwoMode::Pinch;
                two.last_dist = two.start_dist;
                out.push(PointerOut::ZoomBegin);
            } else if dc >= SCROLL_LOCK {
                two.mode = TwoMode::Scroll;
                // Emit the travel from the gesture's start, so the lock window
                // is not a dead zone (a 60 px move scrolled only 52.5 before).
                two.last_centroid = two.start_centroid;
            }
        }
        match two.mode {
            TwoMode::Scroll => out.push(PointerOut::Scroll { dx: c.0 - two.last_centroid.0, dy: c.1 - two.last_centroid.1 }),
            TwoMode::Pinch if two.last_dist > 1.0 && d > 1.0 => {
                two.zoom_acc += (d / two.last_dist).ln();
                two.last_dist = d;
                let step = ZOOM_STEP.ln();
                let steps = (two.zoom_acc / step).trunc() as i32;
                if steps != 0 {
                    two.zoom_acc -= steps as f32 * step;
                    out.push(PointerOut::Zoom { steps });
                }
            }
            _ => {}
        }
        two.last_centroid = c;
        out
    }

    pub fn up(&mut self, slot: i32, t: Millis) -> Vec<PointerOut> {
        let Some(i) = self.fingers.iter().position(|f| f.slot == slot) else { return vec![] };
        self.fingers.remove(i);
        let ended = self.retarget_two();
        if !self.fingers.is_empty() {
            return ended;
        }
        if !ended.is_empty() {
            return ended; // a pinch is not a tap
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
        let mut out = vec![];
        if self.two.take().is_some_and(|t| t.mode == TwoMode::Pinch) {
            out.push(PointerOut::ZoomEnd); // never leave Ctrl held
        }
        self.fingers.clear();
        self.last_tap = None;
        if std::mem::take(&mut self.drag_lock) {
            out.push(PointerOut::Button { button: Button::Left, pressed: false });
        }
        out
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

    /// Two fingers from (x0,y) and (x1,y), moved to (x0',y'),(x1',y') in `n` steps.
    fn two_finger(p: &mut Trackpad, from: [(f32, f32); 2], to: [(f32, f32); 2], n: u32, t0: Millis) -> Vec<PointerOut> {
        let mut out = p.down(0, from[0].0, from[0].1, t0);
        out.extend(p.down(1, from[1].0, from[1].1, t0));
        for i in 1..=n {
            let f = i as f32 / n as f32;
            let lerp = |a: (f32, f32), b: (f32, f32)| (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
            let (a, b) = (lerp(from[0], to[0]), lerp(from[1], to[1]));
            out.extend(p.motion(0, a.0, a.1, t0 + 16 * i as u64));
            out.extend(p.motion(1, b.0, b.1, t0 + 16 * i as u64 + 1));
        }
        out.extend(p.up(0, t0 + 16 * n as u64 + 30));
        out.extend(p.up(1, t0 + 16 * n as u64 + 31));
        out
    }

    #[test]
    fn two_fingers_moving_together_scroll_by_the_centroid() {
        let mut p = Trackpad::default();
        let out = two_finger(&mut p, [(100.0, 100.0), (160.0, 100.0)], [(100.0, 160.0), (160.0, 160.0)], 12, 0);
        let (sx, sy) = out.iter().fold((0.0, 0.0), |(x, y), o| match o {
            PointerOut::Scroll { dx, dy } => (x + dx, y + dy),
            _ => (x, y),
        });
        assert!(sx.abs() < 0.01 && (sy - 60.0).abs() < 0.5, "scrolled ({sx}, {sy}) for a 60 px move: no dead zone");
        assert!(!out.iter().any(|o| matches!(o, PointerOut::ZoomBegin | PointerOut::Zoom { .. } | PointerOut::Button { .. })), "{out:?}");
    }

    #[test]
    fn spreading_fingers_zooms_in_and_ctrl_is_paired() {
        let mut p = Trackpad::default();
        // 60 px apart -> 180 px apart: scale x3 = ln3/ln1.12 ~ 9.7 -> 9 steps in.
        let out = two_finger(&mut p, [(170.0, 300.0), (230.0, 300.0)], [(110.0, 300.0), (290.0, 300.0)], 20, 0);
        assert_eq!(out.first(), Some(&PointerOut::ZoomBegin));
        assert_eq!(out.last(), Some(&PointerOut::ZoomEnd));
        let steps: i32 = out.iter().map(|o| if let PointerOut::Zoom { steps } = o { *steps } else { 0 }).sum();
        assert!((8..=10).contains(&steps), "{steps} steps for x3");
        assert!(!out.iter().any(|o| matches!(o, PointerOut::Scroll { .. } | PointerOut::Button { .. })), "no scroll, no right click: {out:?}");
        // Pinching in zooms out.
        let out = two_finger(&mut p, [(110.0, 300.0), (290.0, 300.0)], [(170.0, 300.0), (230.0, 300.0)], 20, 5000);
        let steps: i32 = out.iter().map(|o| if let PointerOut::Zoom { steps } = o { *steps } else { 0 }).sum();
        assert!(steps <= -8, "{steps}");
    }

    #[test]
    fn the_first_motion_locks_the_gesture() {
        // Starts as a clear scroll, then the fingers spread: stays a scroll.
        let mut p = Trackpad::default();
        let mut out = p.down(0, 100.0, 100.0, 0);
        out.extend(p.down(1, 160.0, 100.0, 0));
        for i in 1..=6 {
            out.extend(p.motion(0, 100.0, 100.0 + 5.0 * i as f32, 16 * i));
            out.extend(p.motion(1, 160.0, 100.0 + 5.0 * i as f32, 16 * i + 1));
        }
        for i in 1..=10 {
            out.extend(p.motion(0, 100.0 - 10.0 * i as f32, 130.0, 200 + 16 * i));
            out.extend(p.motion(1, 160.0 + 10.0 * i as f32, 130.0, 200 + 16 * i + 1));
        }
        assert!(!out.iter().any(|o| matches!(o, PointerOut::ZoomBegin | PointerOut::Zoom { .. })), "locked to scroll: {out:?}");
    }

    #[test]
    fn ctrl_is_released_when_a_finger_lifts_or_the_gesture_is_cancelled() {
        let mut p = Trackpad::default();
        p.down(0, 170.0, 300.0, 0);
        p.down(1, 230.0, 300.0, 0);
        let mut began = false;
        for i in 1..=5 {
            began |= p.motion(0, 170.0 - 8.0 * i as f32, 300.0, 16 * i).contains(&PointerOut::ZoomBegin);
            began |= p.motion(1, 230.0 + 8.0 * i as f32, 300.0, 16 * i + 1).contains(&PointerOut::ZoomBegin);
        }
        assert!(began);
        assert_eq!(p.up(1, 200), vec![PointerOut::ZoomEnd], "one finger up ends the pinch");
        assert!(p.up(0, 210).is_empty(), "and the last lift is not a click");
        // Cancel mid-pinch.
        p.down(0, 170.0, 300.0, 1000);
        p.down(1, 230.0, 300.0, 1000);
        for i in 1..=5 {
            p.motion(0, 170.0 - 8.0 * i as f32, 300.0, 1000 + 16 * i);
            p.motion(1, 230.0 + 8.0 * i as f32, 300.0, 1000 + 16 * i + 1);
        }
        assert!(p.cancel().contains(&PointerOut::ZoomEnd));
        assert!(p.cancel().is_empty());
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
