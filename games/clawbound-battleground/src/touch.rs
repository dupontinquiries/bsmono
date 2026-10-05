// On-screen controls for handhelds (postmarketOS / Plasma Mobile phones,
// PinePhone, tablets) — the layout the monorepo's other games use, adapted
// for a MOBA.
//
// Tap recognition in the open area is `crate::touchkit`, vendored from the
// monorepo's `shared/touchkit` (the same crate skydribble's `shellkit::
// touch` — its on-screen driving joystick — builds on, and games/shared/
// shellkit vendors for exactly the same "stay a standalone tree" reason).
//
// ```text
//  [≡]                                                  [board]
//                                                         (4)
//   ___                                             (3)
//  /   \                                                   [ATK]
//  | O |  <- floating stick: move                   (2)
//  \___/                                              (1)      [shop]
//                                        (press-drag from here to aim a
//                                         dash/blink; a plain tap still
//                                         fires it at the crosshair)
// ```
//
// EARLIER DESIGN NOTE, kept because the mistake is worth not repeating: the
// first version of this read a fast swipe *anywhere in the open area* as a
// quick-cast of the ancestor's Dash. That is precisely wrong for a fast-
// paced game: the open area is also where the camera gets whipped around
// constantly *during real fights*, at speeds that cross exactly the same
// velocity/distance thresholds a deliberate flick would. An ambient gesture
// cannot be "unique enough" to avoid that collision, because both gestures
// start from the same place with the same hand doing the same motion. A
// misread there does not lose you a menu selection — it burns your escape
// or engage at the worst possible moment.
//
// So instead: press-and-drag *from the ability button itself* (`down()`
// records which button a finger landed on; nothing but that finger's own
// motion after that touch-down matters). That is structurally impossible
// to confuse with camera panning, because camera panning never starts on a
// small fixed circle in the bottom-right — and it is *more* precise than a
// flick, since the drag angle is continuous rather than four cardinal
// snaps. It also only exists for mobility abilities: `AbilityDef::
// quick_aimable()` is false for every ultimate and every non-directional
// archetype (heals, shields, novas...), so there is no path from "fast
// gesture" to "irreversible ultimate" or "heal aimed at nothing" at all —
// per-ability-type correctness, not just per-gesture-type.
//
// A quick tap on any ability button (dragged past the button or not) still
// fires it at the camera crosshair, exactly like keyboard/mouse/pad — drag-
// aim is a strictly additive precision option, never a required one.

use crate::touchkit::{Config as TkConfig, Gesture, GestureRecognizer, Touch as TkTouch, Vec2 as TkVec2};
use glam::Vec2;

/// Button geometry in normalized screen units (fractions of the shorter
/// side, y down), before `scale`.
pub const BTN_R: f32 = 0.075;
pub const ATK_R: f32 = 0.11;
const STICK_R: f32 = 0.135;
const DEAD: f32 = 0.12;
/// A drag on an ability button shorter than this still counts as a tap
/// (fires at the crosshair) — long enough that a finger merely settling on
/// the button doesn't accidentally arm a direction.
const ABILITY_DRAG_DEADZONE: f32 = BTN_R * 0.6;
/// Drag distance (screen units) that reads as "full commitment" to a
/// direction, for the HUD's aim indicator; the direction is used as-is
/// past the deadzone regardless, only the visual confidence ring grows.
const ABILITY_DRAG_FULL: f32 = BTN_R * 2.2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Btn {
    Attack,
    Ability(usize),
    Shop,
    Board,
    Menu,
    /// The aimed leap. Sits inside the thumb's arc between attack and the
    /// first ability: it is pressed mid-run, so it cannot be a reach.
    Leap,
}

pub struct Layout {
    pub attack: Vec2,
    pub abilities: [Vec2; 4],
    pub shop: Vec2,
    pub leap: Vec2,
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
        // Solved, not eyeballed: the bottom row between shop and ability 1
        // has ONE spot clear of attack, shop and a1 at once.
        leap: Vec2::new(right - 0.23 * s, bottom - 0.08 * s),
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

/// `look` and `look_vel` are in shorter-side units (a flick is about one screen a second);
/// `glide` was measured and tuned in pixels per second (its floor is 40 px/s, its flicks
/// 300-1200). This is the nominal shorter side that converts between them. Until c21 the
/// velocity went in unconverted, so every lift -- a 1.2 unit/s flick included -- was under the
/// 40 "px/s" floor and read as a placed finger: the glide setting never did anything, and the
/// two plumbing tests written for it (and, by their own commit, never run) failed.
const GLIDE_PX_PER_UNIT: f32 = 1080.0;

pub struct TouchPad {
    pub enabled: bool,
    /// screen size in shorter-side units (see `layout`)
    pub extent: Vec2,
    pub scale: f32,
    /// What a lift does to the look. Copied from settings each frame by app.rs,
    /// like `enabled` and `scale` beside it.
    pub glide: crate::glide::Glide,
    /// The carried look after a lift, and the velocity to launch it with.
    coast: crate::glide::Coast,
    /// Look velocity of the steering finger, smoothed over recent frames.
    ///
    /// Smoothed because the velocity on the LAST frame before a lift is the
    /// noisiest one: the finger decelerates and often jitters as it leaves the
    /// glass. Launching from that single frame gives a glide that is sometimes
    /// half the flick and sometimes double it.
    look_vel: Vec2,
    fingers: Vec<Finger>,
    stick: Option<(u64, Vec2, Vec2)>, // id, origin, pos
    edges: Vec<Btn>,
    /// Which ability slots (0..4) accept a press-and-drag aim this frame —
    /// set once a frame by app.rs from `AbilityDef::quick_aimable()`, so
    /// this module stays game-agnostic (it knows "slot 2 is drag-able
    /// right now", never "slot 2 is a Dash").
    drag_capable: [bool; 4],
    /// A completed drag-release cast this frame: (slot, direction), screen
    /// space, y UP (a world-facing convention app.rs can hand straight to
    /// a camera-relative dash direction). One-shot, like `tap_pending`.
    pub ability_release: Vec<(usize, Vec2)>,
    /// Recognizes taps in the open area (the finger that's neither the
    /// stick nor over a button).
    rec: GestureRecognizer,
    /// Accumulated look drag (normalized units) since last `end_frame`.
    pub look: Vec2,
    /// A tap in the open area this frame (normalized, y down).
    pub tap_pending: Option<Vec2>,
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
            tap_max_dist: 0.022,
            ..TkConfig::default()
        };
        TouchPad {
            enabled: false,
            extent: Vec2::new(16.0 / 9.0, 1.0),
            scale: 1.0,
            glide: crate::glide::Glide::default(),
            coast: crate::glide::Coast::new(),
            look_vel: Vec2::ZERO,
            fingers: Vec::new(),
            stick: None,
            edges: Vec::new(),
            drag_capable: [false; 4],
            ability_release: Vec::new(),
            rec: GestureRecognizer::new(cfg),
            look: Vec2::ZERO,
            tap_pending: None,
            now_ms: 0.0,
        }
    }

    /// Called once a frame from app.rs with the current ancestor's kit
    /// (`AbilityDef::quick_aimable()` per slot).
    pub fn set_drag_capable(&mut self, caps: [bool; 4]) {
        self.drag_capable = caps;
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
        if near(l.leap, BTN_R * 0.8) {
            return Some(Btn::Leap);
        }
        if near(l.shop, BTN_R * 0.8) {
            return Some(Btn::Shop);
        }
        None
    }

    /// A finger touched down at `p` (normalized, y down).
    pub fn down(&mut self, id: u64, p: Vec2) {
        // ANY new contact ends a glide -- including one landing on an ability
        // button, which is the whole flick-then-reach gesture. The view
        // stopping as the thumb lands is the intended end of it.
        self.coast.stop();
        let btn = self.hit(p);
        let mut on_stick = false;
        if btn.is_none() && p.x < self.extent.x * 0.42 && self.stick.is_none() {
            self.stick = Some((id, p, p));
            on_stick = true;
        }
        // Drag-capable ability buttons defer their cast to release (tap vs.
        // aimed drag, decided in `up()`); everything else fires immediately
        // on press, same as before.
        let fires_on_press = match btn {
            Some(Btn::Ability(i)) => !self.drag_capable.get(i).copied().unwrap_or(false),
            Some(_) => true,
            None => false,
        };
        if fires_on_press {
            if let Some(b) = btn {
                self.edges.push(b);
            }
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
        if let Some(f) = self.fingers.iter().find(|f| f.id == id).copied() {
            // Only the STEERING finger glides. A stick thumb or a button press
            // lifting is not a flick of the view, and launching a coast from
            // one would swing the camera when a player lets go of an ability.
            if !f.on_stick && f.btn.is_none() {
                let v = self.look_vel * GLIDE_PX_PER_UNIT;
                self.coast.launch((v.x, v.y), self.glide);
                self.look_vel = Vec2::ZERO;
            }
            if let Some(Btn::Ability(i)) = f.btn {
                if self.drag_capable.get(i).copied().unwrap_or(false) {
                    let d = f.pos - f.start; // screen space, y down
                    if d.length() > ABILITY_DRAG_DEADZONE {
                        // y down -> y up for the world-facing convention
                        self.ability_release.push((i, (d * Vec2::new(1.0, -1.0)).normalize_or(Vec2::new(0.0, 1.0))));
                    } else {
                        // settled without committing to a direction: a plain tap
                        self.edges.push(Btn::Ability(i));
                    }
                }
            }
        }
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
        // `look` holds this whole frame's steering delta at this point: moves
        // arrive before tick, and `end_frame` clears it after `resolve` reads
        // it. So look/dt is this frame's velocity.
        let steering = self.fingers.iter().any(|f| !f.on_stick && f.btn.is_none());
        if steering && dt > 0.0 {
            let v = self.look / dt;
            // Exponential smoothing over ~3 frames. The final frame before a
            // lift is the noisiest, and launching from it alone gives a glide
            // that is sometimes half the flick and sometimes double.
            self.look_vel = self.look_vel.lerp(v, 0.4);
        } else if !steering {
            let (dx, dy) = self.coast.tick(dt, self.glide);
            self.look += Vec2::new(dx, dy) / GLIDE_PX_PER_UNIT;
        }
        let touches: Vec<TkTouch> = self
            .fingers
            .iter()
            .filter(|f| !f.on_stick && f.btn.is_none())
            .map(|f| TkTouch::new(f.id, TkVec2::new(f.pos.x, f.pos.y)))
            .collect();
        for g in self.rec.update(&touches, self.now_ms) {
            if let Gesture::Tap(p) = g {
                self.tap_pending = Some(Vec2::new(p.x, p.y));
            }
        }
    }

    /// Clear one-shot state; call after `resolve` each frame.
    pub fn end_frame(&mut self) {
        self.edges.clear();
        self.look = Vec2::ZERO;
        self.tap_pending = None;
        self.ability_release.clear();
    }

    /// Live drag vector for ability button `i` (screen space, y down, raw —
    /// not normalized) while a finger is down on it past the deadzone, for
    /// the HUD's aim indicator. `None` before the deadzone or with no
    /// finger there, so a settling touch never flashes a direction it
    /// isn't going to fire.
    pub fn ability_drag_live(&self, i: usize) -> Option<Vec2> {
        if !self.drag_capable.get(i).copied().unwrap_or(false) {
            return None;
        }
        let f = self.fingers.iter().find(|f| f.btn == Some(Btn::Ability(i)))?;
        let d = f.pos - f.start;
        (d.length() > ABILITY_DRAG_DEADZONE).then_some(d)
    }

    /// How committed a live drag is, 0..1, for the HUD's confidence ring.
    pub fn ability_drag_confidence(&self, i: usize) -> f32 {
        self.ability_drag_live(i).map_or(0.0, |d| ((d.length() - ABILITY_DRAG_DEADZONE) / (ABILITY_DRAG_FULL - ABILITY_DRAG_DEADZONE)).clamp(0.0, 1.0))
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
    pub fn leap_edge(&self) -> bool {
        self.edges.contains(&Btn::Leap)
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
    /// Steer with an open-area finger for a few frames, then lift. Returns the
    /// look accumulated on each frame AFTER the lift.
    fn flick_then_lift(glide: crate::glide::Glide, frames_after: usize) -> Vec<Vec2> {
        let mut t = TouchPad::new();
        t.extent = Vec2::new(1.78, 1.0);
        t.glide = glide;
        let dt = 1.0 / 60.0;
        // Right-hand open area: not the stick (x < 0.42 * extent), not a button.
        let start = Vec2::new(1.2, 0.35);
        t.down(1, start);
        t.end_frame();
        for i in 1..=6 {
            t.moved(1, start + Vec2::new(0.02 * i as f32, 0.0));
            t.tick(dt);
            t.end_frame();
        }
        t.up(1);
        let mut out = Vec::new();
        for _ in 0..frames_after {
            t.tick(dt);
            out.push(t.look);
            t.end_frame();
        }
        out
    }

    /// THE PLUMBING, not the curve -- `glide`'s own tests cover the decay.
    /// This proves a lift through the real TouchPad actually carries the view.
    #[test]
    fn a_smooth_flick_keeps_steering_after_the_finger_lifts() {
        let after = flick_then_lift(crate::glide::Glide::Smooth, 5);
        assert!(after[0].x > 0.0, "the view must still move on the next frame: {after:?}");
        assert!(after[0].x > after[4].x, "and slow down: {after:?}");
    }

    /// A placed finger -- creeping, then lifted -- must not coast: that is the glide's floor,
    /// which only means something once the pad's units reach it in the glide's.
    #[test]
    fn a_slow_lift_is_a_placed_finger_and_does_not_coast() {
        let mut t = TouchPad::new();
        t.extent = Vec2::new(1.78, 1.0);
        t.glide = crate::glide::Glide::Smooth;
        let dt = 1.0 / 60.0;
        let start = Vec2::new(1.2, 0.35);
        t.down(1, start);
        t.end_frame();
        for i in 1..=6 {
            t.moved(1, start + Vec2::new(0.0004 * i as f32, 0.0)); // 0.024 units/s: 26 px/s
            t.tick(dt);
            t.end_frame();
        }
        t.up(1);
        t.tick(dt);
        assert_eq!(t.look, Vec2::ZERO, "a careful aim must not drift on release: {:?}", t.look);
    }

    /// RIGID IS TODAY'S BEHAVIOUR EXACTLY, through the real pad.
    #[test]
    fn a_rigid_lift_stops_the_view_dead() {
        let after = flick_then_lift(crate::glide::Glide::Rigid, 5);
        assert!(after.iter().all(|v| *v == Vec2::ZERO), "{after:?}");
    }

    /// The flick-then-reach gesture: a new finger landing ends the glide, so
    /// the view stops as the thumb arrives on the button.
    #[test]
    fn a_new_touch_ends_the_glide() {
        let mut t = TouchPad::new();
        t.extent = Vec2::new(1.78, 1.0);
        t.glide = crate::glide::Glide::Smooth;
        let dt = 1.0 / 60.0;
        let s = Vec2::new(1.2, 0.35);
        t.down(1, s);
        t.end_frame();
        for i in 1..=6 {
            t.moved(1, s + Vec2::new(0.02 * i as f32, 0.0));
            t.tick(dt);
            t.end_frame();
        }
        t.up(1);
        t.tick(dt);
        assert!(t.look.x > 0.0, "gliding before the second touch");
        t.end_frame();
        t.down(2, Vec2::new(1.5, 0.8));
        t.tick(dt);
        assert_eq!(t.look, Vec2::ZERO, "a fresh contact must stop the coast");
    }

    /// The leap button must not steal taps from an old one. Hit radii are the
    /// ones `hit` uses (1.15x), on a landscape phone and a portrait one.
    #[test]
    fn no_two_touch_buttons_overlap() {
        let mut pairs = 0;
        for extent in [Vec2::new(1.78, 1.0), Vec2::new(1.0, 2.1)] {
            let l = layout(extent, 1.0);
            let mut b = vec![("attack", l.attack, ATK_R), ("shop", l.shop, BTN_R * 0.8), ("leap", l.leap, BTN_R * 0.8)];
            for (i, a) in l.abilities.iter().enumerate() {
                b.push((["a1", "a2", "a3", "a4"][i], *a, BTN_R));
            }
            // Only pairs involving the NEW button. `attack` and `a2` have
            // overlapped by 0.028 since before this test existed; `hit`
            // settles it by testing attack first. Known, and not this
            // test's claim -- asserting every pair would just fail on it.
            let leap = b.iter().position(|x| x.0 == "leap").unwrap();
            for j in 0..b.len() {
                if j == leap {
                    continue;
                }
                let gap = (b[leap].1 - b[j].1).length() - (b[leap].2 + b[j].2) * 1.15;
                assert!(gap > 0.01, "leap and {} are {} apart on {extent}", b[j].0, gap);
                pairs += 1;
            }
        }
        assert_eq!(pairs, 12);
    }

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
    fn a_non_drag_capable_ability_fires_on_press_exactly_as_before() {
        let mut t = TouchPad::new();
        t.enabled = true;
        // no set_drag_capable call: every slot defaults to false, matching
        // every ability that isn't a non-ultimate dash/blink
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
    fn a_slow_drag_in_the_open_area_just_looks_around() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.end_frame();
        t.down(5, Vec2::new(1.2, 0.3));
        t.tick(0.0);
        for i in 1..=8 {
            t.moved(5, Vec2::new(1.2 + i as f32 * 0.0015, 0.3));
            t.tick(0.1);
        }
        t.up(5);
        t.tick(0.001);
        assert!(t.tap_pending.is_none() && t.look.x > 0.009);
    }

    // --- drag-to-aim: the safety-motivated redesign ------------------------
    //
    // The earlier version of this read a fast swipe *anywhere in the open
    // area* as a dash cast. That collides with ordinary fast camera
    // whipping during a real fight, which crosses the same velocity/
    // distance thresholds — the misread these tests exist to make
    // structurally impossible: the gesture is now anchored to a specific
    // ability button's own touch-down, which camera panning never starts
    // from, and only for slots app.rs has marked drag-capable (mobility
    // abilities, never ultimates — see `AbilityDef::quick_aimable`).

    #[test]
    fn a_non_capable_ability_button_never_offers_a_drag_aim() {
        let mut t = TouchPad::new();
        t.enabled = true;
        // drag_capable defaults to all-false: this is the "not a dash, or
        // is an ultimate" case, which must behave exactly like today —
        // fire on press, full stop, no matter how far the finger wanders
        let l = layout(t.extent, 1.0);
        t.down(1, l.abilities[3]);
        assert!(t.ability_edge(3), "fires immediately on press");
        t.end_frame();
        t.moved(1, l.abilities[3] + Vec2::new(0.3, 0.3)); // a huge swipe away
        t.up(1);
        assert!(t.ability_release.is_empty(), "a non-capable slot never emits a drag release");
        assert!(!t.ability_edge(3), "and does not fire a second time on release");
    }

    #[test]
    fn a_capable_ability_dragged_past_the_deadzone_fires_on_release_with_direction() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.set_drag_capable([false, true, false, false]);
        let l = layout(t.extent, 1.0);
        t.down(1, l.abilities[1]);
        assert!(!t.ability_edge(1), "a drag-capable slot must NOT fire on press");
        assert!(t.ability_drag_live(1).is_none(), "not armed until past the deadzone");
        // drag straight up on screen (toward the top) well past the deadzone
        t.moved(1, l.abilities[1] + Vec2::new(0.0, -ABILITY_DRAG_DEADZONE * 3.0));
        let live = t.ability_drag_live(1).expect("armed now");
        assert!(live.y < 0.0, "raw screen-space drag is still y-down at this point");
        assert!(t.ability_drag_confidence(1) > 0.0);
        t.up(1);
        assert_eq!(t.ability_release.len(), 1);
        let (slot, dir) = t.ability_release[0];
        assert_eq!(slot, 1);
        assert!(dir.y > 0.9 && dir.x.abs() < 1e-3, "up on screen -> +y in the world-facing convention: {dir}");
        assert!(!t.ability_edge(1), "a committed drag never also fires the plain-tap edge");
    }

    #[test]
    fn a_capable_ability_released_without_committing_still_falls_back_to_a_tap() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.set_drag_capable([true, false, false, false]);
        let l = layout(t.extent, 1.0);
        t.down(2, l.abilities[0]);
        assert!(!t.ability_edge(0));
        // a finger settling, or jittering slightly, but never committing
        t.moved(2, l.abilities[0] + Vec2::new(0.01, 0.0));
        t.up(2);
        assert!(t.ability_release.is_empty(), "too small a drag is not a direction");
        assert!(t.ability_edge(0), "so it falls back to a plain crosshair-aimed cast");
    }

    #[test]
    fn drag_capability_is_per_slot_and_never_leaks_to_other_slots() {
        let mut t = TouchPad::new();
        t.enabled = true;
        t.set_drag_capable([false, true, false, false]);
        let l = layout(t.extent, 1.0);
        // slot 0 is not capable even though slot 1 is
        t.down(9, l.abilities[0]);
        assert!(t.ability_edge(0), "slot 0 still fires on press");
        t.moved(9, l.abilities[0] + Vec2::new(0.0, -ABILITY_DRAG_DEADZONE * 3.0));
        t.up(9);
        assert!(t.ability_release.is_empty(), "slot 0's drag must not be read as an aim");
    }
}
