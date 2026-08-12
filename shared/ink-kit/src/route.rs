//! PointerRouter — the answer to "is touchkit compatible with a drawing
//! surface?" It is, but only with a partitioning layer in front of it, because
//! the two consumers want opposite things from the same contacts:
//!
//! * touchkit wants the whole live pointer set each frame and collapses it
//!   into gestures (and drops pressure entirely — `touchkit::Touch` has no
//!   pressure field, by design);
//! * ink wants raw per-pointer polylines *with* pressure and no collapsing.
//!
//! The router keeps touchkit's frame-driven contract intact: the app calls
//! [`PointerRouter::update`] with the complete live set (now including
//! pressure + pointer kind), gets back ink events **plus a nav subset** to
//! feed verbatim into `GestureRecognizer::update`. touchkit never changes.
//!
//! ## Policy (InfiniPaint-informed, tuned for phone-first)
//!
//! * **Pen** inks immediately and unconditionally. While a pen contact is
//!   live, all finger contacts are dropped (palm rejection). Fingers seen
//!   *before* the pen keep whatever role they already had.
//! * **Mouse** (button held) inks immediately — desktop nav goes through
//!   wheel/modifiers, which never enter the router.
//! * **One finger** is ambiguous: it *provisionally* inks (points are
//!   buffered, nothing emitted) until either
//!     - a second finger lands within the grace window and before the first
//!       finger travelled `commit_dist` → the provisional stroke is discarded
//!       and *both* contacts route to nav (two-finger pan/pinch), or
//!     - the grace window elapses or travel exceeds `commit_dist` → the
//!       buffer flushes as `InkBegin` + `InkMove`s and the stroke is committed.
//! * **Second finger during committed inking**: configurable —
//!   [`LateSecondFinger::CancelToNav`] (default; matches the common phone
//!   drawing convention: oops-pan cancels the accidental mark) or
//!   [`LateSecondFinger::EndStroke`] (keep the ink, then navigate).
//!
//! The grace window trades first-ink latency for two-finger reliability.
//! With the default 90 ms + 12 px commit distance, a deliberate stroke
//! commits on the *movement* trigger almost immediately (a drawing finger
//! crosses 12 px in a frame or two), so the latency is only ever paid by a
//! finger that goes down and holds still.

use crate::stroke::{InkPoint, ToolKind};
use crate::{Vec2d, Vec2f};
use std::collections::HashMap;

/// One live pointer this frame — `touchkit::Touch` plus what ink needs.
#[derive(Clone, Copy, Debug)]
pub struct RawPointer {
    pub id: u64,
    /// Screen-space position (the router is pre-camera; the app converts
    /// committed ink points to world space with `Camera::to_world`).
    pub pos: Vec2f,
    /// Some(0..=1) when the backend reports it (pen, some touchscreens).
    pub pressure: Option<f32>,
    pub kind: PointerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Pen,
    Touch,
    Mouse,
}

impl PointerKind {
    /// The stroke [`ToolKind`] this pointer kind produces (adapters use this
    /// when constructing `StrokeStyle`s to match what the router will report).
    pub fn tool(self) -> ToolKind {
        match self {
            PointerKind::Pen => ToolKind::Pen,
            PointerKind::Touch => ToolKind::Finger,
            PointerKind::Mouse => ToolKind::Mouse,
        }
    }
}

/// What to do when a second finger lands while a finger stroke is committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LateSecondFinger {
    /// Discard the in-progress stroke and route both fingers to nav.
    CancelToNav,
    /// End (keep) the stroke, then route both fingers to nav.
    EndStroke,
}

#[derive(Clone, Copy, Debug)]
pub struct RouteConfig {
    /// Ambiguity window for a lone finger (ms).
    pub nav_grace_ms: f64,
    /// Travel (px) that commits a provisional finger stroke early.
    pub commit_dist: f32,
    pub late_second_finger: LateSecondFinger,
}
impl Default for RouteConfig {
    fn default() -> Self {
        RouteConfig {
            nav_grace_ms: 90.0,
            commit_dist: 12.0,
            late_second_finger: LateSecondFinger::CancelToNav,
        }
    }
}

/// Ink-side output. Positions are screen-space [`InkPoint`]s (world x/y hold
/// screen px until the app converts); `tool` says what produced the stroke.
#[derive(Clone, Debug, PartialEq)]
pub enum InkEvent {
    Begin { tool: ToolKind, pt: InkPoint },
    Move { pt: InkPoint },
    End,
    /// The provisional or committed stroke was aborted (route-to-nav).
    Cancel,
}

/// Everything one `update` produced.
#[derive(Clone, Debug, Default)]
pub struct RouteFrame {
    pub ink: Vec<InkEvent>,
    /// Contacts to feed touchkit this frame: `rec.update(&nav_as_touches, t)`.
    /// (id, screen pos). Empty set is meaningful — always forward it.
    pub nav: Vec<(u64, Vec2f)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Idle,
    /// Lone finger buffering (start time, start pos).
    Provisional,
    /// Committed ink via this pointer id.
    Inking,
    /// Contacts belong to touchkit until all lift.
    Nav,
}

pub struct PointerRouter {
    cfg: RouteConfig,
    mode: Mode,
    /// The pointer currently inking / provisionally inking.
    ink_id: Option<u64>,
    ink_tool: ToolKind,
    buf: Vec<InkPoint>,
    start_pos: Vec2f,
    start_t: f64,
    stroke_t0: f64,
    prev: HashMap<u64, RawPointer>,
    pen_live: bool,
}

impl PointerRouter {
    pub fn new(cfg: RouteConfig) -> Self {
        PointerRouter {
            cfg,
            mode: Mode::Idle,
            ink_id: None,
            ink_tool: ToolKind::Finger,
            buf: Vec::new(),
            start_pos: Vec2f::ZERO,
            start_t: 0.0,
            stroke_t0: 0.0,
            prev: HashMap::new(),
            pen_live: false,
        }
    }

    fn pt(&self, p: &RawPointer, t: f64) -> InkPoint {
        InkPoint {
            pos: Vec2d::new(p.pos.x as f64, p.pos.y as f64),
            pressure: p.pressure.unwrap_or(InkPoint::NO_PRESSURE),
            t_ms: t - self.stroke_t0,
        }
    }

    /// Feed the complete live pointer set for this frame. `t_ms` is any
    /// monotonic millisecond clock (egui `i.time * 1000`).
    pub fn update(&mut self, live: &[RawPointer], t_ms: f64) -> RouteFrame {
        let mut out = RouteFrame::default();

        let pen = live.iter().find(|p| p.kind == PointerKind::Pen);
        self.pen_live = pen.is_some();

        // ── Pen has absolute priority ──────────────────────────────────────
        if let Some(pen) = pen {
            match self.mode {
                Mode::Inking if self.ink_id == Some(pen.id) => {
                    out.ink.push(InkEvent::Move { pt: self.pt(pen, t_ms) });
                }
                _ => {
                    // Whatever was happening yields to the pen.
                    self.abort_current(&mut out);
                    self.mode = Mode::Inking;
                    self.ink_id = Some(pen.id);
                    self.ink_tool = ToolKind::Pen;
                    self.stroke_t0 = t_ms;
                    out.ink.push(InkEvent::Begin {
                        tool: ToolKind::Pen,
                        pt: self.pt(pen, t_ms),
                    });
                }
            }
            // Palm rejection: fingers are dropped entirely while pen is live.
            self.remember(live);
            return out;
        }

        // Pen lifted while it was inking → end the stroke.
        if self.mode == Mode::Inking && self.ink_tool == ToolKind::Pen {
            let gone = self.ink_id.map_or(true, |id| !live.iter().any(|p| p.id == id));
            if gone {
                out.ink.push(InkEvent::End);
                self.mode = Mode::Idle;
                self.ink_id = None;
            }
        }

        let touches: Vec<&RawPointer> = live.iter().filter(|p| p.kind == PointerKind::Touch).collect();
        let mouse = live.iter().find(|p| p.kind == PointerKind::Mouse);

        match self.mode {
            // ── Idle: decide what a new contact means ─────────────────────
            Mode::Idle => {
                if let Some(m) = mouse {
                    self.mode = Mode::Inking;
                    self.ink_id = Some(m.id);
                    self.ink_tool = ToolKind::Mouse;
                    self.stroke_t0 = t_ms;
                    out.ink.push(InkEvent::Begin {
                        tool: ToolKind::Mouse,
                        pt: self.pt(m, t_ms),
                    });
                } else if touches.len() >= 2 {
                    self.mode = Mode::Nav;
                } else if touches.len() == 1 {
                    let f = touches[0];
                    self.mode = Mode::Provisional;
                    self.ink_id = Some(f.id);
                    self.ink_tool = ToolKind::Finger;
                    self.start_pos = f.pos;
                    self.start_t = t_ms;
                    self.stroke_t0 = t_ms;
                    self.buf.clear();
                    self.buf.push(self.pt(f, t_ms));
                }
            }

            // ── Provisional lone finger ───────────────────────────────────
            Mode::Provisional => {
                let lead = self.ink_id.and_then(|id| touches.iter().find(|p| p.id == id).copied());
                if touches.len() >= 2 {
                    // Second finger inside the window → it was a nav gesture.
                    self.buf.clear();
                    self.ink_id = None;
                    self.mode = Mode::Nav;
                } else if let Some(f) = lead {
                    self.buf.push(self.pt(f, t_ms));
                    let travelled = f.pos.dist(self.start_pos);
                    let waited = t_ms - self.start_t;
                    if travelled >= self.cfg.commit_dist || waited >= self.cfg.nav_grace_ms {
                        // Commit: flush the buffer.
                        let mut it = self.buf.drain(..);
                        if let Some(first) = it.next() {
                            out.ink.push(InkEvent::Begin {
                                tool: ToolKind::Finger,
                                pt: first,
                            });
                        }
                        for p in it {
                            out.ink.push(InkEvent::Move { pt: p });
                        }
                        self.mode = Mode::Inking;
                    }
                } else {
                    // Finger lifted while provisional: a tap-length stroke —
                    // commit as a dot (ink apps must be able to draw dots;
                    // taps-as-UI belong to widgets outside the canvas).
                    let mut it = self.buf.drain(..);
                    if let Some(first) = it.next() {
                        out.ink.push(InkEvent::Begin {
                            tool: ToolKind::Finger,
                            pt: first,
                        });
                    }
                    for p in it {
                        out.ink.push(InkEvent::Move { pt: p });
                    }
                    out.ink.push(InkEvent::End);
                    self.mode = Mode::Idle;
                    self.ink_id = None;
                }
            }

            // ── Committed inking (finger or mouse; pen handled above) ─────
            Mode::Inking => {
                let lead = self.ink_id.and_then(|id| live.iter().find(|p| p.id == id));
                if self.ink_tool == ToolKind::Finger && touches.len() >= 2 {
                    match self.cfg.late_second_finger {
                        LateSecondFinger::CancelToNav => out.ink.push(InkEvent::Cancel),
                        LateSecondFinger::EndStroke => out.ink.push(InkEvent::End),
                    }
                    self.ink_id = None;
                    self.mode = Mode::Nav;
                } else if let Some(f) = lead {
                    out.ink.push(InkEvent::Move { pt: self.pt(&f.clone(), t_ms) });
                } else if self.ink_tool != ToolKind::Pen {
                    out.ink.push(InkEvent::End);
                    self.mode = Mode::Idle;
                    self.ink_id = None;
                }
            }

            // ── Nav: touchkit owns the contacts until all lift ────────────
            Mode::Nav => {
                if touches.is_empty() && mouse.is_none() {
                    self.mode = Mode::Idle;
                }
            }
        }

        if self.mode == Mode::Nav {
            for p in &touches {
                out.nav.push((p.id, p.pos));
            }
        }
        self.remember(live);
        out
    }

    fn abort_current(&mut self, out: &mut RouteFrame) {
        match self.mode {
            Mode::Provisional => {
                self.buf.clear();
            }
            Mode::Inking => out.ink.push(InkEvent::Cancel),
            _ => {}
        }
        self.ink_id = None;
    }

    fn remember(&mut self, live: &[RawPointer]) {
        self.prev.clear();
        for p in live {
            self.prev.insert(p.id, *p);
        }
    }

    /// True while a pen contact is live (UI can dim finger affordances).
    pub fn pen_active(&self) -> bool {
        self.pen_live
    }
}

impl Default for PointerRouter {
    fn default() -> Self {
        PointerRouter::new(RouteConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finger(id: u64, x: f32, y: f32) -> RawPointer {
        RawPointer {
            id,
            pos: Vec2f::new(x, y),
            pressure: None,
            kind: PointerKind::Touch,
        }
    }
    fn pen(id: u64, x: f32, y: f32, p: f32) -> RawPointer {
        RawPointer {
            id,
            pos: Vec2f::new(x, y),
            pressure: Some(p),
            kind: PointerKind::Pen,
        }
    }

    fn begins(f: &RouteFrame) -> usize {
        f.ink.iter().filter(|e| matches!(e, InkEvent::Begin { .. })).count()
    }

    #[test]
    fn lone_finger_commits_on_movement() {
        let mut r = PointerRouter::default();
        let f0 = r.update(&[finger(1, 0.0, 0.0)], 0.0);
        assert!(f0.ink.is_empty(), "provisional buffers silently");
        let f1 = r.update(&[finger(1, 20.0, 0.0)], 16.0);
        // moved past commit_dist: Begin + the buffered move(s) flush together
        assert_eq!(begins(&f1), 1);
        assert!(f1.ink.iter().any(|e| matches!(e, InkEvent::Move { .. })));
        let f2 = r.update(&[], 32.0);
        assert!(f2.ink.contains(&InkEvent::End));
    }

    #[test]
    fn quick_second_finger_cancels_to_nav_with_no_ink() {
        let mut r = PointerRouter::default();
        let _ = r.update(&[finger(1, 0.0, 0.0)], 0.0);
        let f = r.update(&[finger(1, 2.0, 0.0), finger(2, 100.0, 0.0)], 30.0);
        assert!(f.ink.is_empty(), "no ink ever emitted: {:?}", f.ink);
        assert_eq!(f.nav.len(), 2, "both contacts handed to touchkit");
        // stays nav until all lift
        let f2 = r.update(&[finger(1, 5.0, 0.0), finger(2, 95.0, 0.0)], 46.0);
        assert_eq!(f2.nav.len(), 2);
        let f3 = r.update(&[], 60.0);
        assert!(f3.nav.is_empty());
        // and a fresh finger can ink again
        let _ = r.update(&[finger(3, 0.0, 0.0)], 100.0);
        let f4 = r.update(&[finger(3, 30.0, 0.0)], 116.0);
        assert_eq!(begins(&f4), 1);
    }

    #[test]
    fn late_second_finger_cancels_committed_stroke_by_default() {
        let mut r = PointerRouter::default();
        let _ = r.update(&[finger(1, 0.0, 0.0)], 0.0);
        let _ = r.update(&[finger(1, 30.0, 0.0)], 16.0); // committed
        let f = r.update(&[finger(1, 40.0, 0.0), finger(2, 200.0, 0.0)], 300.0);
        assert!(f.ink.contains(&InkEvent::Cancel));
        assert_eq!(f.nav.len(), 2);
    }

    #[test]
    fn late_second_finger_end_stroke_variant() {
        let mut r = PointerRouter::new(RouteConfig {
            late_second_finger: LateSecondFinger::EndStroke,
            ..RouteConfig::default()
        });
        let _ = r.update(&[finger(1, 0.0, 0.0)], 0.0);
        let _ = r.update(&[finger(1, 30.0, 0.0)], 16.0);
        let f = r.update(&[finger(1, 40.0, 0.0), finger(2, 200.0, 0.0)], 300.0);
        assert!(f.ink.contains(&InkEvent::End));
    }

    #[test]
    fn tap_becomes_a_dot() {
        let mut r = PointerRouter::default();
        let _ = r.update(&[finger(1, 5.0, 5.0)], 0.0);
        let f = r.update(&[], 40.0); // lifted inside the grace window
        assert_eq!(begins(&f), 1);
        assert!(f.ink.contains(&InkEvent::End));
    }

    #[test]
    fn pen_inks_immediately_with_pressure_and_rejects_palm() {
        let mut r = PointerRouter::default();
        let f0 = r.update(&[pen(9, 0.0, 0.0, 0.8)], 0.0);
        assert!(matches!(
            f0.ink[0],
            InkEvent::Begin { tool: ToolKind::Pen, pt } if (pt.pressure - 0.8).abs() < 1e-6
        ));
        // palm lands: finger ignored, pen keeps inking, nothing to nav
        let f1 = r.update(&[pen(9, 10.0, 0.0, 0.9), finger(1, 200.0, 300.0)], 16.0);
        assert_eq!(f1.ink.len(), 1);
        assert!(matches!(f1.ink[0], InkEvent::Move { .. }));
        assert!(f1.nav.is_empty());
        // pen lifts (palm may still be down): stroke ends, palm finger does
        // not get promoted into a provisional stroke this frame
        let f2 = r.update(&[finger(1, 200.0, 300.0)], 32.0);
        assert!(f2.ink.contains(&InkEvent::End));
        assert_eq!(begins(&f2), 0);
    }

    #[test]
    fn pen_interrupts_finger_stroke() {
        let mut r = PointerRouter::default();
        let _ = r.update(&[finger(1, 0.0, 0.0)], 0.0);
        let _ = r.update(&[finger(1, 30.0, 0.0)], 16.0); // committed finger ink
        let f = r.update(&[finger(1, 35.0, 0.0), pen(9, 50.0, 50.0, 0.5)], 32.0);
        assert!(f.ink.contains(&InkEvent::Cancel), "finger stroke aborted");
        assert!(matches!(
            f.ink.last(),
            Some(InkEvent::Begin { tool: ToolKind::Pen, .. })
        ));
    }

    #[test]
    fn slow_hold_commits_after_grace() {
        let mut r = PointerRouter::default();
        let _ = r.update(&[finger(1, 0.0, 0.0)], 0.0);
        let f = r.update(&[finger(1, 1.0, 0.0)], 120.0); // > 90ms, barely moved
        assert_eq!(begins(&f), 1, "grace elapsed => committed as ink");
    }
}
