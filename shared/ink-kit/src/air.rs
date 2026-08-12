//! air — pointers that hover instead of touching: tracked hands and gaze.
//!
//! ## The repo situation this module answers
//!
//! * `hover-kit` already defines the shape of a vision-derived pointer: a
//!   normalized `HandPoint { x, y, confidence }`, produced by a camera that a
//!   proximity sensor wakes. Policy only — no camera, no model.
//! * `hand-kit` is the taxonomy + scoring harness for the detector that will
//!   produce those points (the FP5 capture set is the ground truth).
//! * **There is no eye tracking in the tree.** Nothing produces a gaze point
//!   today. But a gaze point, when something does produce one (front-camera
//!   face landmarks are the obvious FP5 path), is the *same data* as a hand
//!   point: a normalized position, a confidence, a frame rate, and no buttons.
//!
//! So this module defines one bridge for both, rather than a hand feature now
//! and an eye feature later that drift apart — the same collapse `menu.rs`
//! performed on right-click vs long-press.
//!
//! ## What an air pointer is missing, and where each gap is filled
//!
//! A finger has contact (down/up for free) and position in screen pixels. An
//! air pointer has neither:
//!
//! 1. **No down/up.** Engagement must be synthesized. For a hand, the natural
//!    signal is a *pinch* (thumb–index distance from the landmark set —
//!    computed upstream, delivered here as a bool). For gaze, continuous
//!    engagement can come from voice ("draw"/"stop") or a switch; **dwell**
//!    covers the discrete actions (click, menu) because you cannot dwell and
//!    draw a line at the same time.
//! 2. **Noise + dropout.** Landmarks vanish for a frame and reappear; a
//!    confidence dip must not split a stroke in half. [`AirPointer`] holds the
//!    contact alive across short dropouts and uses hysteresis on confidence,
//!    exactly the reasoning behind hover-kit's `MIN_CONFIDENCE` — a
//!    low-confidence landmark is not a slightly-wrong pointer, it is a pointer
//!    somewhere else entirely.
//!
//! The output is a plain [`RawPointer`], so everything downstream —
//! [`PointerRouter`](crate::route::PointerRouter), one-euro, Canvas, touchkit
//! nav — is unchanged and already tested. An engaged air pointer is emitted as
//! [`PointerKind::Pen`]: engagement is explicit, so it inks immediately with
//! no finger-style ambiguity grace, and it triggers the same palm rejection.
//!
//! ## The universal context-menu gesture
//!
//! uikit-egui's `menu.rs` established the intent split: *"was I invoked?"*, not
//! *"was I right-clicked?"*. Its `HoldTracker` is already modality-agnostic for
//! anything that can press (touch, pen, mouse, a pinching hand — feed
//! press/moved/tick and it works). The one modality that cannot press is gaze,
//! and [`Dwell`] is its half: rest within a slop circle for `click_ms` → a
//! click intent; keep resting to `menu_ms` → a menu intent, with a progress
//! fraction to paint so the user can see the fuse burning and look away to
//! cancel. Both stages fire once and re-arm only after the gaze leaves the
//! slop circle — a fixation does not machine-gun clicks.
//!
//! The egui wiring (feeding `HoldTracker`, opening `ContextMenu` at the intent
//! position) lives in `sketches/air_egui.rs`; every decision is here.

use crate::route::{PointerKind, RawPointer};
use crate::Vec2f;

/// One vision frame's report for one air pointer, normalized 0..=1 in both
/// axes (hover-kit's `HandPoint` convention; a gaze estimator reports the same
/// way).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AirSample {
    pub x: f32,
    pub y: f32,
    /// 0..=1 from the landmark / gaze model.
    pub confidence: f32,
}

/// Confidence hysteresis: acquire above `enter`, keep until below `exit`.
///
/// Two thresholds because landmark confidence chatters around any single
/// value; a pointer that flickers in and out at the boundary chops strokes.
#[derive(Clone, Copy, Debug)]
pub struct Confidence {
    pub enter: f32,
    pub exit: f32,
}

impl Default for Confidence {
    fn default() -> Self {
        // enter matches hover-kit's MIN_CONFIDENCE; exit is deliberately lower.
        Confidence { enter: 0.5, exit: 0.35 }
    }
}

/// Tunables for one air pointer.
#[derive(Clone, Copy, Debug)]
pub struct AirConfig {
    pub confidence: Confidence,
    /// A tracked point may vanish for up to this long (bad frames, motion
    /// blur) without the synthesized contact lifting; the last good position
    /// is held. ~4 frames at 30 fps.
    pub dropout_grace_ms: f64,
    /// Stable id for the synthesized contact. Must not collide with real
    /// touch ids from the windowing system; defaults are high tag values.
    pub contact_id: u64,
}

impl Default for AirConfig {
    fn default() -> Self {
        AirConfig {
            confidence: Confidence::default(),
            dropout_grace_ms: 140.0,
            contact_id: AIR_HAND_ID,
        }
    }
}

/// Default contact ids (ASCII tags, far outside slot-index touch ids).
pub const AIR_HAND_ID: u64 = 0x4149_5248; // "AIRH"
pub const AIR_GAZE_ID: u64 = 0x4149_5247; // "AIRG"

/// Turns (sample, engaged) frames into an optional synthesized contact.
///
/// Ownership of *engaged* is deliberately outside: pinch distance for a hand,
/// voice/switch for gaze, always-on for a pure cursor. This struct only
/// answers "is the pointer trustworthy, and where" — the same split hover-kit
/// makes between the gate and the pointer.
pub struct AirPointer {
    cfg: AirConfig,
    acquired: bool,
    last_pos: Option<Vec2f>, // viewport px
    last_seen_ms: f64,
}

impl AirPointer {
    pub fn new(cfg: AirConfig) -> Self {
        AirPointer { cfg, acquired: false, last_pos: None, last_seen_ms: 0.0 }
    }

    /// Feed one vision frame. `viewport` maps normalized coords to pixels;
    /// `engaged` is the modality's down-signal. Returns the contact to append
    /// to this frame's `RawPointer` slice for the router — `None` means the
    /// contact is up (the router will emit `End`/nothing accordingly).
    pub fn update(
        &mut self,
        sample: Option<AirSample>,
        engaged: bool,
        viewport: Vec2f,
        t_ms: f64,
    ) -> Option<RawPointer> {
        match sample {
            Some(s) if s.x.is_finite() && s.y.is_finite() => {
                let c = s.confidence;
                if self.acquired {
                    if c < self.cfg.confidence.exit {
                        self.acquired = false;
                    }
                } else if c >= self.cfg.confidence.enter {
                    self.acquired = true;
                }
                if self.acquired {
                    let p = Vec2f::new(
                        s.x.clamp(0.0, 1.0) * viewport.x,
                        s.y.clamp(0.0, 1.0) * viewport.y,
                    );
                    self.last_pos = Some(p);
                    self.last_seen_ms = t_ms;
                }
            }
            _ => {
                // No sample this frame: keep `acquired` as-is until the grace
                // below expires; a single dropped frame must not lift the pen.
            }
        }

        if self.acquired && t_ms - self.last_seen_ms > self.cfg.dropout_grace_ms {
            self.acquired = false;
        }
        if !self.acquired {
            self.last_pos = None;
        }

        match (engaged, self.last_pos) {
            (true, Some(pos)) => Some(RawPointer {
                id: self.cfg.contact_id,
                pos,
                pressure: None,
                kind: PointerKind::Pen,
            }),
            _ => None,
        }
    }

    /// Where the pointer hovers (px) even when not engaged — for drawing the
    /// cursor/reticle, and for feeding [`Dwell`].
    pub fn hover(&self) -> Option<Vec2f> {
        self.last_pos
    }
}

// ---------------------------------------------------------------- dwell ---

/// What a dwell resolved to this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DwellEvent {
    /// First stage: rested `click_ms` — a primary activation.
    Click(Vec2f),
    /// Second stage: kept resting to `menu_ms` — the context-menu intent,
    /// the gaze twin of long-press / right-click.
    Menu(Vec2f),
}

#[derive(Clone, Copy, Debug)]
pub struct DwellConfig {
    /// Radius the point may wander while still "resting".
    pub slop_px: f32,
    pub click_ms: f64,
    pub menu_ms: f64,
}

impl Default for DwellConfig {
    fn default() -> Self {
        // menu_ms matches menu.rs kHoldMs order of magnitude but is longer:
        // a gaze rests on things unintentionally far more than a finger does.
        DwellConfig { slop_px: 24.0, click_ms: 600.0, menu_ms: 1400.0 }
    }
}

/// Two-stage dwell detector for buttonless pointers.
///
/// Feed the hover position every frame (`None` = pointer lost). Each stage
/// fires at most once per rest; re-arming requires leaving the slop circle.
pub struct Dwell {
    cfg: DwellConfig,
    anchor: Option<(Vec2f, f64)>, // (center, since_ms)
    fired_click: bool,
    fired_menu: bool,
}

impl Dwell {
    pub fn new(cfg: DwellConfig) -> Self {
        Dwell { cfg, anchor: None, fired_click: false, fired_menu: false }
    }

    pub fn update(&mut self, pos: Option<Vec2f>, t_ms: f64) -> Option<DwellEvent> {
        let pos = match pos {
            Some(p) => p,
            None => {
                self.anchor = None;
                self.fired_click = false;
                self.fired_menu = false;
                return None;
            }
        };
        match self.anchor {
            Some((c, since)) if dist(c, pos) <= self.cfg.slop_px => {
                let held = t_ms - since;
                if !self.fired_menu && held >= self.cfg.menu_ms {
                    self.fired_menu = true;
                    self.fired_click = true; // menu supersedes click
                    return Some(DwellEvent::Menu(c));
                }
                if !self.fired_click && held >= self.cfg.click_ms {
                    self.fired_click = true;
                    return Some(DwellEvent::Click(c));
                }
                None
            }
            _ => {
                // moved out (or first frame): new rest, re-armed
                self.anchor = Some((pos, t_ms));
                self.fired_click = false;
                self.fired_menu = false;
                None
            }
        }
    }

    /// 0..1 toward the *menu* stage — the visible fuse. 0 when not resting.
    pub fn progress(&self, t_ms: f64) -> f32 {
        match self.anchor {
            Some((_, since)) if !self.fired_menu => {
                (((t_ms - since) / self.cfg.menu_ms) as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

fn dist(a: Vec2f, b: Vec2f) -> f32 {
    let (dx, dy) = (a.x - b.x, a.y - b.y);
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{InkEvent, PointerRouter, RouteConfig};

    const VP: Vec2f = Vec2f { x: 1000.0, y: 1000.0 };

    fn s(x: f32, y: f32, c: f32) -> Option<AirSample> {
        Some(AirSample { x, y, confidence: c })
    }

    #[test]
    fn engage_synthesizes_contact_and_release_lifts_it() {
        let mut ap = AirPointer::new(AirConfig::default());
        assert!(ap.update(s(0.5, 0.5, 0.9), false, VP, 0.0).is_none());
        let p = ap.update(s(0.5, 0.5, 0.9), true, VP, 16.0).unwrap();
        assert_eq!(p.kind, PointerKind::Pen);
        assert_eq!(p.id, AIR_HAND_ID);
        assert!((p.pos.x - 500.0).abs() < 1e-3);
        assert!(ap.update(s(0.5, 0.5, 0.9), false, VP, 32.0).is_none());
    }

    #[test]
    fn confidence_hysteresis_does_not_flicker() {
        let mut ap = AirPointer::new(AirConfig::default());
        assert!(ap.update(s(0.5, 0.5, 0.6), true, VP, 0.0).is_some());
        // dips below enter but above exit: still tracked
        assert!(ap.update(s(0.5, 0.5, 0.4), true, VP, 16.0).is_some());
        // below exit: lost
        assert!(ap.update(s(0.5, 0.5, 0.2), true, VP, 32.0).is_none());
        // between exit and enter: must NOT re-acquire
        assert!(ap.update(s(0.5, 0.5, 0.4), true, VP, 48.0).is_none());
        assert!(ap.update(s(0.5, 0.5, 0.7), true, VP, 64.0).is_some());
    }

    #[test]
    fn short_dropout_bridges_long_dropout_ends() {
        let mut ap = AirPointer::new(AirConfig::default());
        assert!(ap.update(s(0.3, 0.3, 0.9), true, VP, 0.0).is_some());
        // two dropped frames (~66ms): contact held at last position
        assert!(ap.update(None, true, VP, 33.0).is_some());
        let held = ap.update(None, true, VP, 66.0).unwrap();
        assert!((held.pos.x - 300.0).abs() < 1e-3);
        // beyond grace: contact lifts
        assert!(ap.update(None, true, VP, 250.0).is_none());
    }

    #[test]
    fn air_pointer_draws_a_stroke_through_the_router() {
        let mut ap = AirPointer::new(AirConfig::default());
        let mut router = PointerRouter::new(RouteConfig::default());
        let mut got_begin = false;
        let mut moves = 0;
        let mut got_end = false;
        for i in 0..20 {
            let t = i as f64 * 16.0;
            let engaged = (2..18).contains(&i);
            let raw = ap.update(s(0.2 + i as f32 * 0.02, 0.5, 0.9), engaged, VP, t);
            let live: Vec<RawPointer> = raw.into_iter().collect();
            for ev in router.update(&live, t).ink {
                match ev {
                    InkEvent::Begin { .. } => got_begin = true,
                    InkEvent::Move { .. } => moves += 1,
                    InkEvent::End => got_end = true,
                    InkEvent::Cancel => panic!("no cancel expected"),
                }
            }
        }
        assert!(got_begin && got_end);
        assert!(moves >= 10, "moves = {moves}");
    }

    #[test]
    fn dwell_click_then_menu_each_fire_once() {
        let mut d = Dwell::new(DwellConfig::default());
        let p = Vec2f::new(100.0, 100.0);
        let mut clicks = 0;
        let mut menus = 0;
        for i in 0..200 {
            let t = i as f64 * 16.0; // 3.2s total
            match d.update(Some(Vec2f::new(p.x + (i % 3) as f32, p.y)), t) {
                Some(DwellEvent::Click(_)) => clicks += 1,
                Some(DwellEvent::Menu(_)) => menus += 1,
                None => {}
            }
        }
        assert_eq!((clicks, menus), (1, 1));
    }

    #[test]
    fn movement_resets_and_rearms() {
        let mut d = Dwell::new(DwellConfig::default());
        let a = Vec2f::new(100.0, 100.0);
        let b = Vec2f::new(400.0, 400.0);
        // rest at A long enough to click
        let mut fired = 0;
        for i in 0..50 {
            if d.update(Some(a), i as f64 * 16.0).is_some() {
                fired += 1;
            }
        }
        assert_eq!(fired, 1);
        // saccade to B, rest again: fires again (re-armed by leaving slop)
        let mut fired_b = 0;
        for i in 50..100 {
            if matches!(d.update(Some(b), i as f64 * 16.0), Some(DwellEvent::Click(_))) {
                fired_b += 1;
            }
        }
        assert_eq!(fired_b, 1);
        // pointer lost resets cleanly
        assert!(d.update(None, 2000.0).is_none());
        assert_eq!(d.progress(2000.0), 0.0);
    }

    #[test]
    fn progress_is_a_visible_fuse() {
        let mut d = Dwell::new(DwellConfig { slop_px: 24.0, click_ms: 600.0, menu_ms: 1000.0 });
        let p = Vec2f::new(50.0, 50.0);
        d.update(Some(p), 0.0);
        d.update(Some(p), 500.0);
        let half = d.progress(500.0);
        assert!((half - 0.5).abs() < 0.05, "half = {half}");
    }
}
