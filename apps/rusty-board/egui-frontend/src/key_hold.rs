//! key_hold — what a key does while it is HELD: repeat, and the spacebar caret.
//!
//! The keys fire on release (`keyboard_ui_styled`), so holding ⌫ deleted one
//! character, and the spacebar's arrow swipe moved the caret one step per
//! swipe. This is the held half, as a pure state machine the frame loop feeds
//! with the pointer position and the clock, and which answers with the
//! keysyms to send now:
//!
//! * **Repeat** (⌫ and the arrow keys): nothing for `delay_ms`, then one press
//!   every `interval_ms` — the GTK keyboard's `key_repeat` /
//!   `repeat_delay_ms` / `repeat_interval_ms`, from the same config.
//!   Letters are not repeated: their long-press is the accent popup.
//! * **Caret** (the spacebar): the caret follows the finger — one Left/Right
//!   per [`CaretCfg::step`] of horizontal travel, Up/Down per `step_y`
//!   vertically, the axis locked by whichever is crossed first so a sideways
//!   drag cannot also wander a line. Past [`CaretCfg::zone`] the spacebar
//!   becomes a joystick: holding there keeps stepping, faster the further
//!   out, until the finger comes back or lifts. That is what makes a long
//!   line reachable from a short spacebar.
//!
//! [`Hold::consumed`] tells the caller the release must not ALSO type: a held
//! ⌫ that repeated, or a spacebar that moved the caret, is not a tap.

use egui::{Pos2, Vec2};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepeatCfg {
    pub enabled: bool,
    pub delay_ms: f64,
    pub interval_ms: f64,
}

impl Default for RepeatCfg {
    fn default() -> Self {
        // config/mod.rs's defaults.
        RepeatCfg { enabled: true, delay_ms: 400.0, interval_ms: 50.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaretCfg {
    /// Travel before the first step, so a tap that wobbles is still a space.
    pub deadzone: f32,
    /// Horizontal travel per Left/Right.
    pub step: f32,
    /// Vertical travel per Up/Down (a line is a bigger jump than a character).
    pub step_y: f32,
    /// Displacement past which a resting finger keeps stepping.
    pub zone: f32,
    /// A finger this still (points per frame window) counts as resting.
    pub rest_ms: f64,
    /// Joystick interval at the zone edge, and at `zone + ramp` and beyond.
    pub slow_ms: f64,
    pub fast_ms: f64,
    pub ramp: f32,
}

impl Default for CaretCfg {
    fn default() -> Self {
        CaretCfg { deadzone: 10.0, step: 12.0, step_y: 24.0, zone: 70.0, rest_ms: 220.0, slow_ms: 140.0, fast_ms: 25.0, ramp: 90.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Repeats this keysym.
    Repeat(String),
    Caret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone)]
pub struct Hold {
    pub kind: Kind,
    origin: Pos2,
    started_ms: f64,
    /// Repeat: when the next press is due. Caret joystick: same.
    next_ms: Option<f64>,
    fired: u32,
    axis: Option<Axis>,
    /// Steps already sent along the locked axis by TRACKING (signed).
    tracked: i32,
    last_pos: Pos2,
    last_move_ms: f64,
}

impl Hold {
    pub fn new(kind: Kind, origin: Pos2, now_ms: f64) -> Hold {
        Hold { kind, origin, started_ms: now_ms, next_ms: None, fired: 0, axis: None, tracked: 0, last_pos: origin, last_move_ms: now_ms }
    }

    /// Whether anything was sent while held: the release must not type too.
    pub fn consumed(&self) -> bool {
        self.fired > 0
    }

    /// Feed the finger's position at `now_ms`; returns keysyms to press now.
    pub fn tick(&mut self, pos: Pos2, now_ms: f64, rep: &RepeatCfg, car: &CaretCfg) -> Vec<&'static str> {
        let out = match self.kind.clone() {
            Kind::Repeat(k) => self.tick_repeat(&k, now_ms, rep),
            Kind::Caret => self.tick_caret(pos, now_ms, car),
        };
        self.fired += out.len() as u32;
        out
    }

    fn tick_repeat(&mut self, keysym: &str, now_ms: f64, rep: &RepeatCfg) -> Vec<&'static str> {
        if !rep.enabled {
            return vec![];
        }
        let Some(k) = static_keysym(keysym) else { return vec![] };
        let due = *self.next_ms.get_or_insert(self.started_ms + rep.delay_ms);
        if now_ms < due {
            return vec![];
        }
        // Catch up if a frame was late, but not a burst: at most 3 per tick.
        let n = (((now_ms - due) / rep.interval_ms.max(1.0)).floor() as u32 + 1).min(3);
        self.next_ms = Some(due + n as f64 * rep.interval_ms.max(1.0));
        // The first repeat is the key's SECOND press: the release would have
        // been the first, and it is swallowed once anything fires, so send one
        // extra up front.
        let n = if self.fired == 0 { n + 1 } else { n };
        vec![k; n as usize]
    }

    fn tick_caret(&mut self, pos: Pos2, now_ms: f64, car: &CaretCfg) -> Vec<&'static str> {
        if (pos - self.last_pos).length() > 1.5 {
            self.last_pos = pos;
            self.last_move_ms = now_ms;
        }
        let d: Vec2 = pos - self.origin;
        if self.axis.is_none() {
            if d.x.abs() >= car.deadzone && d.x.abs() >= d.y.abs() {
                self.axis = Some(Axis::X);
            } else if d.y.abs() >= car.deadzone {
                self.axis = Some(Axis::Y);
            } else {
                return vec![];
            }
        }
        let (along, step, neg, pos_k) = match self.axis.unwrap() {
            Axis::X => (d.x, car.step, "Left", "Right"),
            Axis::Y => (d.y, car.step_y, "Up", "Down"),
        };
        let mut out = Vec::new();
        // TRACKING: the caret follows the finger both ways, relative to where
        // the joystick left it (so coming back from the zone does not undo it).
        let want = ((along.abs() - car.deadzone).max(0.0) / step).floor() as i32 * along.signum() as i32
            + if along.abs() >= car.deadzone { along.signum() as i32 } else { 0 };
        while self.tracked < want {
            self.tracked += 1;
            out.push(pos_k);
        }
        while self.tracked > want {
            self.tracked -= 1;
            out.push(neg);
        }
        // JOYSTICK: resting past the zone keeps going, faster further out.
        let resting = now_ms - self.last_move_ms >= car.rest_ms;
        if along.abs() >= car.zone && resting {
            let t = ((along.abs() - car.zone) / car.ramp.max(1.0)).clamp(0.0, 1.0) as f64;
            let interval = car.slow_ms + (car.fast_ms - car.slow_ms) * t;
            let due = *self.next_ms.get_or_insert(now_ms);
            if now_ms >= due {
                out.push(if along < 0.0 { neg } else { pos_k });
                self.next_ms = Some(now_ms + interval);
            }
        } else {
            self.next_ms = None;
        }
        out
    }
}

/// The keysyms this module sends, as statics (the frame loop borrows them).
fn static_keysym(k: &str) -> Option<&'static str> {
    Some(match k {
        "BackSpace" => "BackSpace",
        "Delete" => "Delete",
        "Left" => "Left",
        "Right" => "Right",
        "Up" => "Up",
        "Down" => "Down",
        _ => return None,
    })
}

/// What holding a key with this emit does, if anything.
pub fn kind_for(emit: &str) -> Option<Kind> {
    use crate::real_layout::{classify, Press};
    match classify(emit) {
        Press::Backspace => Some(Kind::Repeat("BackSpace".into())),
        Press::Text(t) if t == "\u{8}" => Some(Kind::Repeat("BackSpace".into())),
        Press::Arrow(a) => Some(Kind::Repeat(a.keysym().into())),
        Press::Text(t) if t == " " => Some(Kind::Caret),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn run(h: &mut Hold, frames: &[(f32, f32, f64)]) -> Vec<&'static str> {
        let (r, c) = (RepeatCfg::default(), CaretCfg::default());
        frames.iter().flat_map(|&(x, y, t)| h.tick(pos2(x, y), t, &r, &c)).collect()
    }

    #[test]
    fn backspace_waits_then_repeats_at_the_interval() {
        let mut h = Hold::new(Kind::Repeat("BackSpace".into()), pos2(0.0, 0.0), 0.0);
        assert!(run(&mut h, &[(0.0, 0.0, 100.0), (0.0, 0.0, 399.0)]).is_empty(), "a tap is a tap");
        assert!(!h.consumed());
        let got = run(&mut h, &[(0.0, 0.0, 400.0)]);
        assert_eq!(got, ["BackSpace", "BackSpace"], "the swallowed release's press, then the first repeat");
        let got = run(&mut h, &[(0.0, 0.0, 450.0), (0.0, 0.0, 500.0), (0.0, 0.0, 550.0)]);
        assert_eq!(got.len(), 3);
        assert!(h.consumed());
    }

    #[test]
    fn a_late_frame_catches_up_without_a_burst() {
        let mut h = Hold::new(Kind::Repeat("Left".into()), pos2(0.0, 0.0), 0.0);
        run(&mut h, &[(0.0, 0.0, 400.0)]);
        assert_eq!(run(&mut h, &[(0.0, 0.0, 2000.0)]).len(), 3);
    }

    #[test]
    fn the_caret_follows_the_finger_both_ways() {
        let mut h = Hold::new(Kind::Caret, pos2(100.0, 0.0), 0.0);
        assert!(run(&mut h, &[(105.0, 2.0, 16.0)]).is_empty(), "inside the deadzone: still a space");
        let right = run(&mut h, &[(110.0, 0.0, 32.0), (122.0, 0.0, 48.0), (134.0, 0.0, 64.0)]);
        assert_eq!(right, ["Right", "Right", "Right"]);
        let back = run(&mut h, &[(110.0, 0.0, 80.0)]);
        assert_eq!(back, ["Left", "Left"], "dragging back undoes it");
        assert!(h.consumed(), "the release must not type a space");
    }

    #[test]
    fn a_sideways_drag_never_moves_a_line() {
        let mut h = Hold::new(Kind::Caret, pos2(100.0, 50.0), 0.0);
        let got = run(&mut h, &[(115.0, 55.0, 16.0), (140.0, 80.0, 32.0)]);
        assert!(got.iter().all(|k| *k == "Right"), "{got:?}");
    }

    #[test]
    fn resting_past_the_zone_keeps_stepping_faster_further_out() {
        let c = CaretCfg::default();
        let count = |dx: f32| {
            let mut h = Hold::new(Kind::Caret, pos2(0.0, 0.0), 0.0);
            run(&mut h, &[(dx, 0.0, 16.0)]);
            let tracked = h.fired;
            let frames: Vec<(f32, f32, f64)> = (1..=120).map(|i| (dx, 0.0, 16.0 + i as f64 * 16.0)).collect();
            run(&mut h, &frames);
            h.fired - tracked
        };
        assert_eq!(count(c.zone - 5.0), 0, "inside the zone a resting finger is still");
        let near = count(c.zone + 5.0);
        let far = count(c.zone + c.ramp + 20.0);
        assert!(near > 0 && far > near * 2, "near {near}, far {far}");
    }

    #[test]
    fn which_keys_hold() {
        assert_eq!(kind_for(&format!("{}backspace", crate::real_layout::ACT)), Some(Kind::Repeat("BackSpace".into())));
        assert_eq!(kind_for(" "), Some(Kind::Caret));
        assert_eq!(kind_for("a"), None, "a letter's hold is its accents");
    }
}
