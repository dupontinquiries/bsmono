// On-screen controls for handhelds (postmarketOS / Plasma Mobile phones,
// PinePhone, tablets) — the layout the monorepo's other games use, adapted
// for a MOBA.
//
// Tap/swipe recognition is `crate::touchkit`, vendored from the monorepo's
// `shared/touchkit` (the same crate skydribble's `shellkit::touch` — its
// on-screen driving joystick — builds on, and games/shared/shellkit vendors
// for exactly the same "stay a standalone tree" reason). That gets us a
// real velocity-aware tap/swipe recognizer instead of a hand-rolled
// distance-and-timer heuristic, for free.
//
// ```text
//  [≡]                                                  [board]
//                                                         (4)
//   ___                                             (3)
//  /   \                                                   [ATK]
//  | O |  <- floating stick: move                   (2)
//  \___/                                              (1)      [shop]
//   (swipe here toward an enemy = quick-dash that way)
// ```
//
// The stick is *floating*: it appears wherever a finger lands in the left
// 42 % of the screen, so there is nothing to aim at. Buttons are fixed on
// the right. In the open area: a drag turns the camera, a tap is a walk-to
// order (app.rs resolves it to a ground point), and — the per-game mod on
// skydribble's flick-to-dodge — a quick swipe fires the ancestor's Dash
// ability in that screen direction, the mobile answer to aiming a dash with
// a mouse. `flick` carries the direction for one frame, same shape as
// `tap_pending`; app.rs (which has the ancestor's kit) decides what to do
// with it, same division of labor as skydribble's `main.rs` reading `tpad`.

use crate::touchkit::{Config as TkConfig, Gesture, GestureRecognizer, Touch as TkTouch, Vec2 as TkVec2};
use glam::Vec2;

/// Button geometry in normalized screen units (fractions of the shorter
/// side, y down), before `scale`.
pub const BTN_R: f32 = 0.075;
pub const ATK_R: f32 = 0.11;
const STICK_R: f32 = 0.135;
const DEAD: f32 = 0.12;
/// How long a flick's direction stays live for app.rs to act on (matches
/// skydribble's `flick_hold`).
const FLICK_HOLD_SECS: f32 = 0.12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Btn {
    Attack,
    Ability(usize),
    Shop,
    Board,
    Menu,
}

pub struct Layout {
    pub attack: Vec2,
    pub abilities: [Vec2; 4],
    pub shop: Vec2,
    pub board: Vec2,
    pub menu: Vec2,
}

/// `extent` is the screen in normalized units of its shorter side, so a
/// landscape phone is (1.78, 1.0) and a portrait one (1.0, 2.1). The cluster
/// anchors to the bottom-right corner either way.
pub fn layout(extent: Vec2, scale: f32) -> Layout {
    let s = scale;
    let right = extent.x - 0.02;
    let bottom = extent.y;
    Layout {
        attack: Vec2::new(right - 0.16 * s, bottom - 0.28 * s),
        abilities: [
            Vec2::new(right - 0.40 * s, bottom - 0.10 * s),
            Vec2::new(right - 0.34 * s, bottom - 0.32 * s),
            Vec2::new(right - 0.24 * s, bottom - 0.50 * s),
            Vec2::new(right - 0.10 * s, bottom - 0.60 * s),
        ],
        shop: Vec2::new(right - 0.06 * s, bottom - 0.08 * s),
        board: Vec2::new(right - 0.05, 0.06),
        menu: Vec2::new(0.055, 0.055),
    }
}

#[derive(Clone, Copy)]
struct Finger {
    id: u64,
    start: Vec2,
    pos: Vec2,
    btn: Option<Btn>,
    on_stick: bool,
}

pub struct TouchPad {
    pub enabled: bool,
    /// screen size in shorter-side units (see `layout`)
    pub extent: Vec2,
    pub scale: f32,
    fingers: Vec<Finger>,
    stick: Option<(u64, Vec2, Vec2)>, // id, origin, pos
    edges: Vec<Btn>,
    /// Recognizes tap/swipe/fling in the open area (the finger that's
    /// neither the stick nor over a button — see `rec_touches`).
    rec: GestureRecognizer,
    /// Accumulated look drag (normalized units) since last `end_frame`.
    pub look: Vec2,
    /// A tap in the open area this frame (normalized, y down).
    pub tap_pending: Option<Vec2>,
    /// A quick swipe in the open area this frame: direction, screen space,
    /// y UP (so it's a world-facing convention app.rs can hand straight to
    /// a camera-relative dash direction, same as skydribble's `flick`).
    pub flick: Option<Vec2>,
    flick_hold: f32,
    now_ms: f64,
}

impl Default for TouchPad {
    fn default() -> Self {
        TouchPad::new()
    }
}

impl TouchPad {
    pub fn new() -> TouchPad {
        let cfg = TkConfig {
            // normalized (0..1) screen units, not pixels: shrink the
            // pixel-tuned defaults to match, same ratios shellkit uses
            swipe_min_dist: 0.055,
            swipe_min_vel: 0.9,
            tap_max_dist: 0.022,
            fling_min_vel: 1.1,
            ..TkConfig::default()
        };
        TouchPad {
            enabled: false,
            extent: Vec2::new(16.0 / 9.0, 1.0),
            scale: 1.0,
            fingers: Vec::new(),
            stick: None,
            edges: Vec::new(),
            rec: GestureRecognizer::new(cfg),
            look: Vec2::ZERO,
            tap_pending: None,
            flick: None,
            flick_hold: 0.0,
            now_ms: 0.0,
        }
    }

    fn hit(&self, p: Vec2) -> Option<Btn> {
        let l = layout(self.extent, self.scale);
        let s = self.scale;
        let near = |c: Vec2, r: f32| (p - c).length() < r * s * 1.15;
        if near(l.menu, BTN_R * 0.8) {
            return Some(Btn::Menu);
        }
        if near(l.board, BTN_R * 0.8) {
            return Some(Btn::Board);
        }
        if near(l.attack, ATK_R) {
            return Some(Btn::Attack);
        }
        for (i, a) in l.abilities.iter().enumerate() {
            if near(*a, BTN_R) {
                return Some(Btn::Ability(i));
            }
        }
        if near(l.shop, BTN_R * 0.8) {
            return Some(Btn::Shop);
        }
        None
    }

    /// A finger touched down at `p` (normalized, y down).
    pub fn down(&mut self, id: u64, p: Vec2) {
        let btn = self.hit(p);
        let mut on_stick = false;
        if btn.is_none() && p.x < self.extent.x * 0.42 && self.stick.is_none() {
            self.stick = Some((id, p, p));
            on_stick = true;
        }
        if let Some(b) = btn {
            self.edges.push(b);
        }
        self.fingers.push(Finger { id, start: p, pos: p, btn, on_stick });
    }

    pub fn moved(&mut self, id: u64, p: Vec2) {
        if let Some(f) = self.fingers.iter_mut().find(|f| f.id == id) {
            let prev = f.pos;
            f.pos = p;
            if !f.on_stick && f.btn.is_none() {
                self.look += p - prev; // open-area drag steers the camera
            }
        }
        if let Some(s) = self.stick.as_mut() {
            if s.0 == id {
                s.2 = p;
            }
        }
    }

    pub fn up(&mut self, id: u64) {
        self.fingers.retain(|f| f.id != id);
        if self.stick.map(|s| s.0 == id).unwrap_or(false) {
            self.stick = None;
        }
    }

    pub fn cancel(&mut self, id: u64) {
        self.up(id);
    }

    /// Feed the recognizer with whichever finger is in the open area (not
    /// the stick, not on a button — there's at most one at a time for our
    /// purposes, matching how the stick already claims the left third).
    pub fn tick(&mut self, dt: f32) {
        self.now_ms += (dt * 1000.0) as f64;
        let touches: Vec<TkTouch> = self
            .fingers
            .iter()
            .filter(|f| !f.on_stick && f.btn.is_none())
            .map(|f| TkTouch::new(f.id, TkVec2::new(f.pos.x, f.pos.y)))
            .collect();
        for g in self.rec.update(&touches, self.now_ms) {
            match g {
                Gesture::Tap(p) => self.tap_pending = Some(Vec2::new(p.x, p.y)),
                Gesture::SwipeUp { .. } => self.set_flick(Vec2::new(0.0, 1.0)),
                Gesture::SwipeDown { .. } => self.set_flick(Vec2::new(0.0, -1.0)),
                Gesture::SwipeLeft { .. } => self.set_flick(Vec2::new(-1.0, 0.0)),
                Gesture::SwipeRight { .. } => self.set_flick(Vec2::new(1.0, 0.0)),
                _ => {}
            }
        }
        self.flick_hold = (self.flick_hold - dt).max(0.0);
        if self.flick_hold <= 0.0 {
            self.flick = None;
        }
    }

    fn set_flick(&mut self, dir: Vec2) {
        self.flick = Some(dir);
        self.flick_hold = FLICK_HOLD_SECS;
    }

    /// Clear one-shot state; call after `resolve` each frame.
    pub fn end_frame(&mut self) {
        self.edges.clear();
        self.look = Vec2::ZERO;
        self.tap_pending = None;
    }

    /// Stick vector, y up, dead-zoned and clamped: the WASD equivalent.
    pub fn stick_vec(&self) -> Vec2 {
        let Some((_, origin, pos)) = self.stick else { return Vec2::ZERO };
        let mut d = pos - origin;
        d.y = -d.y;
        let len = d.length();
        let r = STICK_R * self.scale;
        if len < r * DEAD {
            return Vec2::ZERO;
        }
        let mag = ((len - r * DEAD) / (r * (1.0 - DEAD))).clamp(0.0, 1.0);
        d / len.max(1e-5) * mag
    }

    /// Stick origin and knob for the HUD, if a finger is on it.
    pub fn stick_draw(&self) -> Option<(Vec2, Vec2)> {
        let (_, origin, pos) = self.stick?;
        let d = pos - origin;
        let r = STICK_R * self.scale;
        let knob = if d.length() > r { origin + d / d.length() * r } else { pos };
        Some((origin, knob))
    }

    fn held(&self, b: Btn) -> bool {
        self.fingers.iter().any(|f| f.btn == Some(b))
    }
    pub fn attack_down(&self) -> bool {
        self.held(Btn::Attack)
    }
    pub fn board_down(&self) -> bool {
        self.held(Btn::Board)
    }
    pub fn ability_edge(&self, i: usize) -> bool {
        self.edges.contains(&Btn::Ability(i))
    }
    pub fn shop_edge(&self) -> bool {
        self.edges.contains(&Btn::Shop)
    }
    pub fn menu_edge(&self) -> bool {
        self.edges.contains(&Btn::Menu)
    }
    pub fn ability_down(&self, i: usize) -> bool {
        self.held(Btn::Ability(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_stick_appears_under_the_thumb() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.down(1, Vec2::new(0.2, 0.7));
        assert_eq!(t.stick_vec(), Vec2::ZERO);
        t.moved(1, Vec2::new(0.2, 0.55)); // dragged up on screen = forward
        let v = t.stick_vec();
        assert!(v.y > 0.9 && v.x.abs() < 1e-3, "{v}");
        t.up(1);
        assert_eq!(t.stick_vec(), Vec2::ZERO);
        assert!(t.tap_pending.is_none(), "a stick drag is not a tap");
    }

    #[test]
    fn buttons_edge_and_hold_and_a_tap_walks() {
        let mut t = TouchPad::new();
        t.enabled = true;
        let l = layout(t.extent, 1.0);
        t.down(2, l.abilities[2]);
        assert!(t.ability_edge(2) && t.ability_down(2));
        t.end_frame();
        assert!(!t.ability_edge(2) && t.ability_down(2), "edge once, hold persists");
        t.down(3, l.attack);
        assert!(t.attack_down());
        // a quick tap in the open area: down, one tick, up — same shape as
        // touchkit's own `tap_is_recognized` test
        t.down(4, Vec2::new(1.2, 0.3));
        t.tick(0.05);
        t.up(4);
        t.tick(0.001);
        assert_eq!(t.tap_pending, Some(Vec2::new(1.2, 0.3)));
    }

    #[test]
    fn a_slow_drag_in_the_open_area_looks_around_not_a_tap_or_flick() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.end_frame();
        t.down(5, Vec2::new(1.2, 0.3));
        t.tick(0.0);
        // several slow frames of the same small drag — this is what a real
        // frame loop looks like: one recognizer sample per rendered frame,
        // not one sample for the whole gesture
        for i in 1..=8 {
            t.moved(5, Vec2::new(1.2 + i as f32 * 0.0015, 0.3));
            t.tick(0.1);
        }
        t.up(5);
        t.tick(0.001);
        assert!(t.tap_pending.is_none() && t.flick.is_none() && t.look.x > 0.009);
    }

    #[test]
    fn a_fast_swipe_in_the_open_area_sets_a_flick_that_expires() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.down(6, Vec2::new(1.0, 0.5));
        t.tick(0.0);
        // a fast rightward swipe across three frames, same shape as
        // touchkit's own `fast_horizontal_drag_is_swipe_left` test
        t.moved(6, Vec2::new(1.1, 0.5));
        t.tick(0.012);
        t.moved(6, Vec2::new(1.2, 0.5));
        t.tick(0.012);
        t.moved(6, Vec2::new(1.32, 0.5));
        t.tick(0.012);
        t.up(6);
        t.tick(0.012);
        assert_eq!(t.flick, Some(Vec2::new(1.0, 0.0)), "a rightward swipe flicks right");
        // it's a brief edge, not a held state — it must decay
        for _ in 0..30 {
            t.tick(0.02);
        }
        assert!(t.flick.is_none(), "a flick must not linger");
    }

    #[test]
    fn a_finger_that_starts_on_the_stick_never_reaches_the_swipe_recognizer() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.down(7, Vec2::new(0.1, 0.5)); // inside the stick zone (x < 0.42)
        t.tick(0.0);
        // the same fast motion that reads as a swipe in the open area
        t.moved(7, Vec2::new(0.2, 0.5));
        t.tick(0.012);
        t.moved(7, Vec2::new(0.3, 0.5));
        t.tick(0.012);
        t.moved(7, Vec2::new(0.42, 0.5));
        t.tick(0.012);
        t.up(7);
        t.tick(0.012);
        assert!(t.flick.is_none(), "a stick steer must never read as a dash flick");
    }
}
