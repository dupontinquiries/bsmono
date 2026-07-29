//! On-screen controls for handhelds, built on the shared `touchkit`
//! recognizer (the same crate the monorepo's other games use, vendored here).
//!
//! Layout, in normalized screen space (0..1, y down):
//!
//! ```text
//!  +---------------------------------------------+
//!  |                                             |
//!  |            (swipe here = flip)              |
//!  |            (tap here   = drive to)          |
//!  |   ___                                       |
//!  |  /   \                             (JUMP)   |
//!  |  | O |  <- stick: WASD                      |
//!  |  \___/                            (BOOST)   |
//!  +---------------------------------------------+
//! ```
//!
//! The stick is *floating*: it appears wherever a finger lands in the left
//! third, so there is nothing to aim at. Buttons are fixed. A flick anywhere
//! in the open area dodges in that direction, and a tap sets a drive-to
//! target that steers for you until you touch the stick again.

use glam::{Vec2, Vec3};
use touchkit::{Config as TkConfig, Gesture, GestureRecognizer, Touch as TkTouch, Vec2 as TkVec2};

/// Fixed button geometry (fractions of the smaller screen dimension).
pub const BTN_R: f32 = 0.115;
const STICK_R: f32 = 0.135; // max knob travel from the origin
const DEAD: f32 = 0.12; // fraction of STICK_R ignored at the centre

pub struct Buttons {
    pub boost: Vec2,
    pub jump: Vec2,
}

/// Where the fixed buttons sit for a given aspect (normalized coords).
pub fn buttons() -> Buttons {
    Buttons { boost: Vec2::new(0.885, 0.80), jump: Vec2::new(0.70, 0.855) }
}

#[derive(Clone, Copy)]
struct Finger {
    id: u64,
    start: Vec2,
    pos: Vec2,
}

/// Live state consumed by `Game::controls`.
pub struct TouchPad {
    pub enabled: bool,
    rec: GestureRecognizer,
    fingers: Vec<Finger>,
    stick: Option<Finger>,
    boost_id: Option<u64>,
    jump_id: Option<u64>,
    /// Filled for exactly one frame when a flick or button press fires.
    pub jump_edge: bool,
    /// Dodge direction for the frame a flick lands (screen space, y up).
    pub flick: Option<Vec2>,
    flick_hold: f32,
    /// Drive-to target set by a tap in the world area, cleared on arrival.
    pub target: Option<Vec3>,
    /// Pending tap position (normalized) — main picks the world point.
    pub tap_pending: Option<Vec2>,
    /// Camera look delta accumulated from open-area drags.
    pub look: Vec2,
    now_ms: f64,
}

impl TouchPad {
    pub fn new() -> TouchPad {
        let cfg = TkConfig {
            // a phone screen in normalized units: thresholds scale with it
            swipe_min_dist: 0.055,
            swipe_min_vel: 0.35,
            tap_max_dist: 0.022,
            ..TkConfig::default()
        };
        TouchPad {
            enabled: false,
            rec: GestureRecognizer::new(cfg),
            fingers: Vec::new(),
            stick: None,
            boost_id: None,
            jump_id: None,
            jump_edge: false,
            flick: None,
            flick_hold: 0.0,
            target: None,
            tap_pending: None,
            look: Vec2::ZERO,
            now_ms: 0.0,
        }
    }

    fn hits(p: Vec2, c: Vec2) -> bool {
        (p - c).length() < BTN_R * 1.15
    }

    /// A finger touched down at `p` (normalized, y down).
    pub fn down(&mut self, id: u64, p: Vec2) {
        let b = buttons();
        if Self::hits(p, b.boost) {
            self.boost_id = Some(id);
        } else if Self::hits(p, b.jump) {
            self.jump_id = Some(id);
            self.jump_edge = true; // press edge, consumed by controls()
        } else if p.x < 0.42 && self.stick.is_none() {
            // floating stick: it materializes under the thumb
            self.stick = Some(Finger { id, start: p, pos: p });
        }
        self.fingers.push(Finger { id, start: p, pos: p });
    }

    pub fn moved(&mut self, id: u64, p: Vec2) {
        if let Some(f) = self.fingers.iter_mut().find(|f| f.id == id) {
            let prev = f.pos;
            f.pos = p;
            // open-area drags steer the camera
            let on_stick = self.stick.map(|s| s.id == id).unwrap_or(false);
            if !on_stick && self.boost_id != Some(id) && self.jump_id != Some(id) {
                self.look += p - prev;
            }
        }
        if let Some(s) = self.stick.as_mut() {
            if s.id == id {
                s.pos = p;
            }
        }
    }

    pub fn up(&mut self, id: u64) {
        self.fingers.retain(|f| f.id != id);
        if self.stick.map(|s| s.id == id).unwrap_or(false) {
            self.stick = None;
        }
        if self.boost_id == Some(id) {
            self.boost_id = None;
        }
        if self.jump_id == Some(id) {
            self.jump_id = None;
        }
    }

    /// Feed the recognizer once per frame and decay one-shot state.
    pub fn tick(&mut self, dt: f32) {
        self.now_ms += (dt * 1000.0) as f64;
        let ts: Vec<TkTouch> = self
            .fingers
            .iter()
            .map(|f| TkTouch::new(f.id, TkVec2::new(f.pos.x, f.pos.y)))
            .collect();
        let gestures = self.rec.update(&ts, self.now_ms);
        for g in gestures {
            match g {
                // a flick in the open area is a DODGE in that direction
                Gesture::SwipeUp { .. } => self.set_flick(Vec2::new(0.0, 1.0)),
                Gesture::SwipeDown { .. } => self.set_flick(Vec2::new(0.0, -1.0)),
                Gesture::SwipeLeft { .. } => self.set_flick(Vec2::new(-1.0, 0.0)),
                Gesture::SwipeRight { .. } => self.set_flick(Vec2::new(1.0, 0.0)),
                Gesture::Tap(p) => {
                    let b = buttons();
                    let pv = Vec2::new(p.x, p.y);
                    let on_ui = Self::hits(pv, b.boost) || Self::hits(pv, b.jump) || pv.x < 0.42;
                    if !on_ui {
                        self.tap_pending = Some(pv); // main resolves to a world point
                    }
                }
                _ => {}
            }
        }
        self.flick_hold = (self.flick_hold - dt).max(0.0);
        if self.flick_hold <= 0.0 {
            self.flick = None;
        }
    }

    fn set_flick(&mut self, dir: Vec2) {
        // a flick only dodges when the stick is not already committed to a
        // direction — otherwise a hard steer would read as a swipe
        if self.stick.is_some() {
            return;
        }
        self.flick = Some(dir);
        self.flick_hold = 0.10;
        self.jump_edge = true;
    }

    /// Stick vector, y up, already dead-zoned and clamped: x = steer,
    /// y = throttle. This is the WASD equivalent.
    pub fn stick_vec(&self) -> Vec2 {
        let Some(s) = self.stick else { return Vec2::ZERO };
        let mut d = s.pos - s.start;
        d.y = -d.y; // screen y is down; throttle is up
        let len = d.length();
        if len < STICK_R * DEAD {
            return Vec2::ZERO;
        }
        let mag = ((len - STICK_R * DEAD) / (STICK_R * (1.0 - DEAD))).clamp(0.0, 1.0);
        d / len.max(1e-5) * mag
    }

    /// Stick origin and knob position for the HUD, if active.
    pub fn stick_draw(&self) -> Option<(Vec2, Vec2)> {
        let s = self.stick?;
        let d = s.pos - s.start;
        let len = d.length();
        let knob = if len > STICK_R { s.start + d / len * STICK_R } else { s.pos };
        Some((s.start, knob))
    }

    pub fn boost_down(&self) -> bool {
        self.boost_id.is_some()
    }
    pub fn jump_down(&self) -> bool {
        self.jump_id.is_some()
    }

    /// True while a drive-to target is being followed.
    pub fn has_target(&self) -> bool {
        self.target.is_some()
    }

    /// Steering toward the drive-to target: returns (throttle, steer).
    /// Cleared automatically on arrival or when the stick takes over.
    pub fn follow(&mut self, car_pos: Vec3, car_fwd: Vec3) -> (f32, f32) {
        if self.stick.is_some() {
            self.target = None;
            return (0.0, 0.0);
        }
        let Some(t) = self.target else { return (0.0, 0.0) };
        let to = t - car_pos;
        let flat = Vec3::new(to.x, 0.0, to.z);
        if flat.length() < 4.0 {
            self.target = None;
            return (0.0, 0.0);
        }
        let dir = flat.normalize();
        let f = Vec3::new(car_fwd.x, 0.0, car_fwd.z).normalize_or(Vec3::Z);
        let right = Vec3::new(-f.z, 0.0, f.x);
        let steer = (dir.dot(right) * 2.4).clamp(-1.0, 1.0);
        let ahead = dir.dot(f);
        // reverse out if the target is behind us and close
        let throttle = if ahead < -0.2 && flat.length() < 14.0 { -0.8 } else { 1.0 };
        (throttle, steer)
    }
}
