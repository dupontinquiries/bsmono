// On-screen controls for handhelds (postmarketOS / Plasma Mobile phones,
// PinePhone, tablets) — the layout the monorepo's other games use, adapted
// for a MOBA. Self-contained (no gesture crate) so the bundle stands alone;
// the tap/drag recogniser is the small subset of shared/touchkit we need.
//
// ```text
//  [≡]                                                  [board]
//                                                         (4)
//   ___                                             (3)
//  /   \                                                   [ATK]
//  | O |  <- floating stick: move                   (2)
//  \___/                                              (1)      [shop]
// ```
//
// The stick is *floating*: it appears wherever a finger lands in the left
// 42 % of the screen, so there is nothing to aim at. Buttons are fixed on
// the right. A drag anywhere in the open area turns the camera; a tap in the
// open area is a walk-to order (app.rs resolves it to a ground point).

use glam::Vec2;

/// Button geometry in normalized screen units (fractions of the shorter
/// side, y down), before `scale`.
pub const BTN_R: f32 = 0.075;
pub const ATK_R: f32 = 0.11;
const STICK_R: f32 = 0.135;
const DEAD: f32 = 0.12;
const TAP_MAX_DIST: f32 = 0.022;
const TAP_MAX_MS: f64 = 300.0;

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
    t0: f64,
    btn: Option<Btn>,
    on_stick: bool,
    moved: bool,
}

pub struct TouchPad {
    pub enabled: bool,
    /// screen size in shorter-side units (see `layout`)
    pub extent: Vec2,
    pub scale: f32,
    fingers: Vec<Finger>,
    stick: Option<(u64, Vec2, Vec2)>, // id, origin, pos
    edges: Vec<Btn>,
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
        TouchPad { enabled: false, extent: Vec2::new(16.0 / 9.0, 1.0), scale: 1.0, fingers: Vec::new(), stick: None, edges: Vec::new(), look: Vec2::ZERO, tap_pending: None, now_ms: 0.0 }
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
        self.fingers.push(Finger { id, start: p, pos: p, t0: self.now_ms, btn, on_stick, moved: false });
    }

    pub fn moved(&mut self, id: u64, p: Vec2) {
        if let Some(f) = self.fingers.iter_mut().find(|f| f.id == id) {
            let prev = f.pos;
            f.pos = p;
            if (p - f.start).length() > TAP_MAX_DIST {
                f.moved = true;
            }
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
        if let Some(i) = self.fingers.iter().position(|f| f.id == id) {
            let f = self.fingers.remove(i);
            let short = self.now_ms - f.t0 < TAP_MAX_MS;
            if !f.moved && short && f.btn.is_none() && !f.on_stick {
                self.tap_pending = Some(f.pos);
            }
        }
        if self.stick.map(|s| s.0 == id).unwrap_or(false) {
            self.stick = None;
        }
    }

    pub fn cancel(&mut self, id: u64) {
        self.fingers.retain(|f| f.id != id);
        if self.stick.map(|s| s.0 == id).unwrap_or(false) {
            self.stick = None;
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.now_ms += (dt * 1000.0) as f64;
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
        // a quick tap in the open area
        t.down(4, Vec2::new(1.2, 0.3));
        t.tick(0.1);
        t.up(4);
        assert_eq!(t.tap_pending, Some(Vec2::new(1.2, 0.3)));
        // a drag there is a look, not a tap
        t.end_frame();
        t.down(5, Vec2::new(1.2, 0.3));
        t.moved(5, Vec2::new(1.3, 0.3));
        t.up(5);
        assert!(t.tap_pending.is_none() && t.look.x > 0.09);
    }
}
