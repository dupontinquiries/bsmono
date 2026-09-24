//! Edge gestures: a single-finger state machine over raw touch samples.
//!
//! ```text
//!             Down in edge zone            moved > slop
//!   Idle ───────────────────────► Pending ───────────────► Drag(kind)  ──Up──► release event
//!     ▲  Down elsewhere: ignored     │  held still ≥ hold_ms
//!     │                              └──────────────────► Hold ──Up──► HoldRelease
//!     └─────────── Up / Cancel from any state ◄──────────────────────────────
//! ```
//!
//! Only the first finger of a sequence is tracked; extra fingers are ignored
//! until it lifts, so a thumb resting on the edge does not turn a two-finger
//! app gesture into a shell gesture. Hold is detected in [`Recognizer::tick`]
//! as well as on moves, because a finger held perfectly still produces no
//! samples on many digitizers.

use crate::{Millis, TouchEvent};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeConfig {
    /// Depth of each edge zone in px.
    pub edge_px: f32,
    /// Movement that turns Pending into a drag (and cancels a hold).
    pub slop_px: f32,
    pub hold_ms: Millis,
    /// Swipe-up distance, as a fraction of screen height, that opens the switcher.
    pub switcher_commit: f32,
    /// Swipe-up distance that goes past the switcher to the launcher (hatch).
    pub launcher_commit: f32,
    /// Pull-down distance (fraction of drawer travel) that opens the drawer.
    pub drawer_commit: f32,
    /// Travel in px that maps to drawer progress 1.0.
    pub drawer_travel_px: f32,
    /// Horizontal distance along the bottom edge that counts as a quick switch.
    pub quick_switch_px: f32,
    /// Release speed (px/ms) that commits regardless of distance.
    pub flick: f32,
    /// Whether the top edge opens the drawer (off in right-bar layout).
    pub top_enabled: bool,
    /// Whether the right edge reports swipe-ins (on in right-bar layout).
    pub right_enabled: bool,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        EdgeConfig {
            edge_px: 28.0,
            slop_px: 14.0,
            hold_ms: 420,
            switcher_commit: 0.12,
            launcher_commit: 0.55,
            drawer_commit: 0.3,
            drawer_travel_px: 560.0,
            quick_switch_px: 70.0,
            flick: 0.8,
            top_enabled: true,
            right_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Toward the previously used window (finger moved right).
    Back,
    /// Toward the more recent end (finger moved left).
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeUpTarget {
    Cancel,
    Switcher,
    Launcher,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GestureEvent {
    DrawerDrag { progress: f32 },
    DrawerRelease { open: bool },
    SwipeUpDrag { progress: f32 },
    SwipeUpRelease { target: SwipeUpTarget },
    /// Live horizontal offset along the bottom edge (for a peek animation).
    HorizontalDrag { dx: f32 },
    /// Horizontal release; `None` when it did not travel far enough.
    QuickSwitch(Option<Dir>),
    HoldStart { x: f32, y: f32 },
    HoldMove { x: f32, y: f32 },
    HoldRelease { x: f32, y: f32 },
    RightSwipeIn,
    /// The sequence was cancelled mid-gesture; undo any live preview.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    /// A non-edge touch is down; ignore everything until it lifts.
    Foreign { slot: i32 },
    Pending { slot: i32, edge: Edge },
    Drawer { slot: i32 },
    SwipeUp { slot: i32 },
    Horizontal { slot: i32 },
    Hold { slot: i32 },
    /// Right swipe-in already fired; wait for lift.
    Spent { slot: i32 },
}

/// Short, human-readable phase name for the trace overlay.
fn phase_name(p: &Phase) -> &'static str {
    match p {
        Phase::Idle => "idle",
        Phase::Foreign { .. } => "foreign",
        Phase::Pending { .. } => "pending",
        Phase::Drawer { .. } => "drawer",
        Phase::SwipeUp { .. } => "swipe-up",
        Phase::Horizontal { .. } => "horizontal",
        Phase::Hold { .. } => "hold",
        Phase::Spent { .. } => "spent",
    }
}

#[derive(Debug, Clone)]
pub struct Recognizer {
    pub cfg: EdgeConfig,
    screen: (f32, f32),
    phase: Phase,
    start: (f32, f32),
    start_t: Millis,
    last: (f32, f32),
    last_t: Millis,
    /// Smoothed velocity, px/ms, y-down / x-right positive.
    vel: (f32, f32),
    /// Phase transitions since the last `drain_transitions`, for the trace.
    transitions: Vec<String>,
}

impl Recognizer {
    pub fn new(cfg: EdgeConfig, screen: (f32, f32)) -> Self {
        Recognizer {
            cfg,
            screen,
            phase: Phase::Idle,
            start: (0.0, 0.0),
            start_t: 0,
            last: (0.0, 0.0),
            last_t: 0,
            vel: (0.0, 0.0),
            transitions: Vec::new(),
        }
    }

    pub fn set_screen(&mut self, screen: (f32, f32)) {
        self.screen = screen;
    }

    pub fn phase_label(&self) -> &'static str {
        phase_name(&self.phase)
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    pub fn drain_transitions(&mut self) -> Vec<String> {
        std::mem::take(&mut self.transitions)
    }

    /// Which edge zone a point is in. Bottom wins the corners: the home bar is
    /// the gesture people aim for with a thumb, the others are reached for.
    pub fn edge_at(&self, x: f32, y: f32) -> Option<Edge> {
        let (w, h) = self.screen;
        let e = self.cfg.edge_px;
        if y >= h - e {
            Some(Edge::Bottom)
        } else if self.cfg.top_enabled && y <= e {
            Some(Edge::Top)
        } else if self.cfg.right_enabled && x >= w - e {
            Some(Edge::Right)
        } else {
            None
        }
    }

    fn go(&mut self, to: Phase) {
        if std::mem::discriminant(&self.phase) != std::mem::discriminant(&to) {
            self.transitions
                .push(format!("{} -> {}", phase_name(&self.phase), phase_name(&to)));
        }
        self.phase = to;
    }

    fn slot(&self) -> Option<i32> {
        match self.phase {
            Phase::Idle => None,
            Phase::Foreign { slot }
            | Phase::Pending { slot, .. }
            | Phase::Drawer { slot }
            | Phase::SwipeUp { slot }
            | Phase::Horizontal { slot }
            | Phase::Hold { slot }
            | Phase::Spent { slot } => Some(slot),
        }
    }

    /// Where the current gesture began. The palette needs it to tell a
    /// deliberate slide from jitter, and only the recognizer knows it.
    pub fn start(&self) -> (f32, f32) {
        self.start
    }

    pub fn on_event(&mut self, ev: TouchEvent) -> Vec<GestureEvent> {
        match ev {
            TouchEvent::Down { slot, x, y, t } => self.down(slot, x, y, t),
            TouchEvent::Move { slot, x, y, t } => {
                if self.slot() == Some(slot) {
                    self.motion(x, y, t)
                } else {
                    vec![]
                }
            }
            TouchEvent::Up { slot, t } => {
                if self.slot() == Some(slot) {
                    self.up(t)
                } else {
                    vec![]
                }
            }
            TouchEvent::Cancel => {
                let live = !matches!(
                    self.phase,
                    Phase::Idle | Phase::Foreign { .. } | Phase::Pending { .. } | Phase::Spent { .. }
                );
                self.go(Phase::Idle);
                if live {
                    vec![GestureEvent::Cancelled]
                } else {
                    vec![]
                }
            }
        }
    }

    /// Advance time with no new sample: fires a hold on a finger held still.
    pub fn tick(&mut self, now: Millis) -> Vec<GestureEvent> {
        if let Phase::Pending { slot, edge: Edge::Bottom } = self.phase {
            if now.saturating_sub(self.start_t) >= self.cfg.hold_ms {
                self.go(Phase::Hold { slot });
                return vec![GestureEvent::HoldStart { x: self.last.0, y: self.last.1 }];
            }
        }
        vec![]
    }

    fn down(&mut self, slot: i32, x: f32, y: f32, t: Millis) -> Vec<GestureEvent> {
        if !self.is_idle() {
            return vec![];
        }
        self.start = (x, y);
        self.last = (x, y);
        self.start_t = t;
        self.last_t = t;
        self.vel = (0.0, 0.0);
        match self.edge_at(x, y) {
            Some(edge) => self.go(Phase::Pending { slot, edge }),
            None => self.go(Phase::Foreign { slot }),
        }
        vec![]
    }

    fn motion(&mut self, x: f32, y: f32, t: Millis) -> Vec<GestureEvent> {
        let dt = t.saturating_sub(self.last_t).max(1) as f32;
        let inst = ((x - self.last.0) / dt, (y - self.last.1) / dt);
        // EMA: responsive enough for a flick, damped enough that one noisy
        // sample at lift does not decide the outcome.
        self.vel = (self.vel.0 * 0.4 + inst.0 * 0.6, self.vel.1 * 0.4 + inst.1 * 0.6);
        self.last = (x, y);
        self.last_t = t;
        let (dx, dy) = (x - self.start.0, y - self.start.1);
        let moved = dx.hypot(dy) > self.cfg.slop_px;
        let h = self.screen.1.max(1.0);

        match self.phase {
            Phase::Pending { slot, edge } => {
                if !moved {
                    // A hold can also be noticed on a jittery sample.
                    if edge == Edge::Bottom && t.saturating_sub(self.start_t) >= self.cfg.hold_ms {
                        self.go(Phase::Hold { slot });
                        return vec![GestureEvent::HoldStart { x, y }];
                    }
                    return vec![];
                }
                match edge {
                    Edge::Top if dy > 0.0 && dy.abs() >= dx.abs() => {
                        self.go(Phase::Drawer { slot });
                        vec![GestureEvent::DrawerDrag { progress: self.drawer_progress(dy) }]
                    }
                    Edge::Bottom if dy < 0.0 && dy.abs() >= dx.abs() => {
                        self.go(Phase::SwipeUp { slot });
                        vec![GestureEvent::SwipeUpDrag { progress: (-dy / h).clamp(0.0, 1.0) }]
                    }
                    Edge::Bottom if dx.abs() > dy.abs() => {
                        self.go(Phase::Horizontal { slot });
                        vec![GestureEvent::HorizontalDrag { dx }]
                    }
                    Edge::Right if dx < 0.0 && dx.abs() >= dy.abs() => {
                        self.go(Phase::Spent { slot });
                        vec![GestureEvent::RightSwipeIn]
                    }
                    // Moved the wrong way for this edge (e.g. down off the
                    // bottom, sideways along the top): not a shell gesture.
                    _ => {
                        self.go(Phase::Foreign { slot });
                        vec![]
                    }
                }
            }
            Phase::Drawer { .. } => vec![GestureEvent::DrawerDrag { progress: self.drawer_progress(dy) }],
            Phase::SwipeUp { .. } => vec![GestureEvent::SwipeUpDrag { progress: (-dy / h).clamp(0.0, 1.0) }],
            Phase::Horizontal { .. } => vec![GestureEvent::HorizontalDrag { dx }],
            Phase::Hold { .. } => vec![GestureEvent::HoldMove { x, y }],
            _ => vec![],
        }
    }

    fn drawer_progress(&self, dy: f32) -> f32 {
        (dy / self.cfg.drawer_travel_px.max(1.0)).clamp(0.0, 1.0)
    }

    fn up(&mut self, _t: Millis) -> Vec<GestureEvent> {
        let (dx, dy) = (self.last.0 - self.start.0, self.last.1 - self.start.1);
        let h = self.screen.1.max(1.0);
        let out = match self.phase {
            Phase::Drawer { .. } => {
                let p = self.drawer_progress(dy);
                // A flick up closes even from far down; a flick down opens.
                let open = if self.vel.1 <= -self.cfg.flick {
                    false
                } else {
                    p >= self.cfg.drawer_commit || self.vel.1 >= self.cfg.flick
                };
                vec![GestureEvent::DrawerRelease { open }]
            }
            Phase::SwipeUp { .. } => {
                let p = -dy / h;
                let target = if p >= self.cfg.launcher_commit {
                    SwipeUpTarget::Launcher
                } else if p >= self.cfg.switcher_commit || -self.vel.1 >= self.cfg.flick {
                    // Multitask-first: a short flick lands in the switcher,
                    // not "home". Home (hatch) is the long, deliberate swipe.
                    SwipeUpTarget::Switcher
                } else {
                    SwipeUpTarget::Cancel
                };
                vec![GestureEvent::SwipeUpRelease { target }]
            }
            Phase::Horizontal { .. } => {
                let dir = if dx.abs() < self.cfg.quick_switch_px && self.vel.0.abs() < self.cfg.flick {
                    None
                } else if dx > 0.0 {
                    Some(Dir::Back)
                } else {
                    Some(Dir::Forward)
                };
                vec![GestureEvent::QuickSwitch(dir)]
            }
            Phase::Hold { .. } => vec![GestureEvent::HoldRelease { x: self.last.0, y: self.last.1 }],
            _ => vec![],
        };
        self.go(Phase::Idle);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 408.0;
    const H: f32 = 900.0;

    fn rec() -> Recognizer {
        Recognizer::new(EdgeConfig::default(), (W, H))
    }

    fn down(x: f32, y: f32, t: Millis) -> TouchEvent {
        TouchEvent::Down { slot: 0, x, y, t }
    }
    fn mv(x: f32, y: f32, t: Millis) -> TouchEvent {
        TouchEvent::Move { slot: 0, x, y, t }
    }
    fn up(t: Millis) -> TouchEvent {
        TouchEvent::Up { slot: 0, t }
    }

    /// Feed a straight drag in `steps` samples, 16 ms apart, and return every event.
    fn drag(r: &mut Recognizer, from: (f32, f32), to: (f32, f32), steps: u32, t0: Millis) -> Vec<GestureEvent> {
        let mut out = r.on_event(down(from.0, from.1, t0));
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            let p = (from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
            out.extend(r.on_event(mv(p.0, p.1, t0 + 16 * i as u64)));
        }
        out.extend(r.on_event(up(t0 + 16 * steps as u64 + 16)));
        out
    }

    fn last(v: &[GestureEvent]) -> GestureEvent {
        *v.last().expect("no events")
    }

    #[test]
    fn slow_short_swipe_up_opens_switcher() {
        let mut r = rec();
        // 0.2 of screen height over ~40 samples: slow, past switcher_commit.
        let ev = drag(&mut r, (200.0, H - 5.0), (200.0, H - 5.0 - 0.2 * H), 40, 0);
        assert_eq!(last(&ev), GestureEvent::SwipeUpRelease { target: SwipeUpTarget::Switcher });
    }

    #[test]
    fn long_swipe_up_goes_to_launcher() {
        let mut r = rec();
        let ev = drag(&mut r, (200.0, H - 5.0), (200.0, H - 5.0 - 0.7 * H), 40, 0);
        assert_eq!(last(&ev), GestureEvent::SwipeUpRelease { target: SwipeUpTarget::Launcher });
    }

    #[test]
    fn tiny_slow_swipe_up_cancels() {
        let mut r = rec();
        // Past slop (so it IS a swipe-up) but under switcher_commit, and slow.
        let ev = drag(&mut r, (200.0, H - 5.0), (200.0, H - 5.0 - 40.0), 30, 0);
        assert!(ev.iter().any(|e| matches!(e, GestureEvent::SwipeUpDrag { .. })));
        assert_eq!(last(&ev), GestureEvent::SwipeUpRelease { target: SwipeUpTarget::Cancel });
    }

    #[test]
    fn short_fast_flick_up_opens_switcher() {
        let mut r = rec();
        // 60 px in 3 samples = ~1.25 px/ms: under switcher_commit by distance.
        let ev = drag(&mut r, (200.0, H - 5.0), (200.0, H - 65.0), 3, 0);
        assert!(60.0 / H < EdgeConfig::default().switcher_commit, "fixture must be under the distance gate");
        assert_eq!(last(&ev), GestureEvent::SwipeUpRelease { target: SwipeUpTarget::Switcher });
    }

    #[test]
    fn hold_fires_from_tick_without_motion() {
        let mut r = rec();
        r.on_event(down(200.0, H - 4.0, 1000));
        assert!(r.tick(1000 + 419).is_empty(), "must not fire before hold_ms");
        assert_eq!(r.tick(1000 + 420), vec![GestureEvent::HoldStart { x: 200.0, y: H - 4.0 }]);
        assert!(r.tick(2000).is_empty(), "fires once");
        let ev = r.on_event(mv(150.0, H - 120.0, 1500));
        assert_eq!(ev, vec![GestureEvent::HoldMove { x: 150.0, y: H - 120.0 }]);
        assert_eq!(r.on_event(up(1600)), vec![GestureEvent::HoldRelease { x: 150.0, y: H - 120.0 }]);
        assert!(r.is_idle());
    }

    #[test]
    fn moving_before_hold_ms_prevents_hold() {
        let mut r = rec();
        r.on_event(down(200.0, H - 4.0, 0));
        r.on_event(mv(200.0, H - 60.0, 100));
        assert!(r.tick(1000).is_empty());
        assert_eq!(r.phase_label(), "swipe-up");
    }

    #[test]
    fn horizontal_along_bottom_quick_switches_both_ways() {
        let mut r = rec();
        let ev = drag(&mut r, (100.0, H - 6.0), (220.0, H - 8.0), 20, 0);
        assert_eq!(last(&ev), GestureEvent::QuickSwitch(Some(Dir::Back)));
        let ev = drag(&mut r, (300.0, H - 6.0), (180.0, H - 8.0), 20, 5000);
        assert_eq!(last(&ev), GestureEvent::QuickSwitch(Some(Dir::Forward)));
    }

    #[test]
    fn short_slow_horizontal_is_no_switch() {
        let mut r = rec();
        let ev = drag(&mut r, (100.0, H - 6.0), (130.0, H - 6.0), 30, 0);
        assert_eq!(last(&ev), GestureEvent::QuickSwitch(None));
    }

    #[test]
    fn top_pull_opens_and_flick_up_closes() {
        let mut r = rec();
        let ev = drag(&mut r, (200.0, 4.0), (200.0, 300.0), 30, 0);
        assert_eq!(last(&ev), GestureEvent::DrawerRelease { open: true });
        // Pull far, then flick back up before lifting.
        r.on_event(down(200.0, 4.0, 10_000));
        for i in 1..=20 {
            r.on_event(mv(200.0, 4.0 + 20.0 * i as f32, 10_000 + 16 * i));
        }
        r.on_event(mv(200.0, 300.0, 10_400));
        r.on_event(mv(200.0, 200.0, 10_416));
        assert_eq!(r.on_event(up(10_432)), vec![GestureEvent::DrawerRelease { open: false }]);
    }

    #[test]
    fn top_disabled_in_bar_layout_and_right_enabled() {
        let mut r = Recognizer::new(EdgeConfig { top_enabled: false, right_enabled: true, ..Default::default() }, (W, H));
        let ev = drag(&mut r, (200.0, 4.0), (200.0, 300.0), 20, 0);
        assert!(ev.is_empty(), "top edge must be inert: {ev:?}");
        let ev = drag(&mut r, (W - 3.0, 400.0), (W - 90.0, 405.0), 10, 1000);
        assert_eq!(ev.iter().filter(|e| **e == GestureEvent::RightSwipeIn).count(), 1);
    }

    #[test]
    fn non_edge_touch_and_second_finger_are_ignored() {
        let mut r = rec();
        let ev = drag(&mut r, (200.0, 400.0), (200.0, 100.0), 20, 0);
        assert!(ev.is_empty());
        // Edge finger down, then a second finger: the second must not steal it.
        r.on_event(down(200.0, H - 4.0, 0));
        assert!(r.on_event(TouchEvent::Down { slot: 1, x: 50.0, y: 50.0, t: 5 }).is_empty());
        assert!(r.on_event(TouchEvent::Move { slot: 1, x: 50.0, y: 400.0, t: 20 }).is_empty());
        assert_eq!(r.phase_label(), "pending");
    }

    #[test]
    fn wrong_direction_from_edge_is_foreign() {
        let mut r = rec();
        // Sideways along the top edge: not a drawer pull.
        let ev = drag(&mut r, (100.0, 5.0), (250.0, 8.0), 20, 0);
        assert!(ev.is_empty(), "{ev:?}");
    }

    #[test]
    fn cancel_mid_gesture_reports_and_resets() {
        let mut r = rec();
        r.on_event(down(200.0, H - 4.0, 0));
        r.on_event(mv(200.0, H - 80.0, 16));
        assert_eq!(r.on_event(TouchEvent::Cancel), vec![GestureEvent::Cancelled]);
        assert!(r.is_idle());
        assert!(r.on_event(TouchEvent::Cancel).is_empty(), "idle cancel is silent");
    }

    #[test]
    fn transitions_are_recorded_for_the_trace() {
        let mut r = rec();
        drag(&mut r, (200.0, H - 5.0), (200.0, H - 300.0), 10, 0);
        let t = r.drain_transitions();
        assert_eq!(t, vec!["idle -> pending", "pending -> swipe-up", "swipe-up -> idle"]);
        assert!(r.drain_transitions().is_empty());
    }
}
