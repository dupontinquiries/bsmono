//! # touchkit
//!
//! A small, backend-agnostic gesture recognizer lifted out of snapclone's input
//! layer so it can be shared across the portfolio (the `shared/` kit layer).
//!
//! It takes the set of **currently-down pointers each frame** plus a timestamp,
//! diffs against the previous frame, and emits high-level [`Gesture`]s with
//! velocity and fling **momentum**. That frame-driven shape matches how immediate-
//! mode UIs (egui) expose input: you read all live pointers per frame rather than
//! getting discrete `down`/`move`/`up` callbacks.
//!
//! egui already gives you raw multi-touch zoom/translation; touchkit adds the
//! *semantic* layer egui doesn't — directional swipes, flick-to-momentum, double-
//! tap, long-press — in a way that has **no rendering dependency**, so the exact
//! same recognizer drives snapclone's photo lightbox and logscope's strand timeline.
//!
//! ## Usage shape (per UI frame)
//! ```
//! use touchkit::{GestureRecognizer, Touch, Vec2, Gesture};
//! let mut rec = GestureRecognizer::default();
//!
//! // frame 1: one finger goes down at (100, 100) at t=0ms
//! let _ = rec.update(&[Touch::new(1, Vec2::new(100.0, 100.0))], 0.0);
//! // frame 2: it has moved fast to the left at t=16ms
//! let _ = rec.update(&[Touch::new(1, Vec2::new(40.0, 102.0))], 16.0);
//! // frame 3: finger lifted (not in the set) -> swipe is decided here
//! let gestures = rec.update(&[], 32.0);
//! assert!(gestures.iter().any(|g| matches!(g, Gesture::SwipeLeft { .. })));
//!
//! // after a fling, call `momentum()` each frame to get the glide:
//! for _ in 0..120 {
//!     for g in rec.momentum(now_ms()) {
//!         if let Gesture::MomentumTick { delta } = g { /* scroll by delta */ }
//!     }
//! }
//! # fn now_ms() -> f64 { 0.0 }
//! ```

use std::collections::HashMap;

/// Minimal 2D vector so the core stays dependency-free. With the `egui` feature
/// enabled, `From`/`Into` conversions to `egui::Vec2`/`egui::Pos2` are provided.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    #[inline]
    pub fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }
    #[inline]
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    #[inline]
    pub fn dist(self, o: Vec2) -> f32 {
        (self - o).length()
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}

/// One live pointer/finger this frame. `id` must be stable across frames for the
/// same contact (egui's `TouchId`, a pointer id, or just `0` for the mouse).
#[derive(Clone, Copy, Debug)]
pub struct Touch {
    pub id: u64,
    pub pos: Vec2,
}
impl Touch {
    #[inline]
    pub fn new(id: u64, pos: Vec2) -> Self {
        Touch { id, pos }
    }
}

/// Direction carried by [`Gesture::Fling`] is implicit in its velocity vector;
/// swipes are pre-classified into the four cardinal variants for ergonomic match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// A quick press-release with negligible movement.
    Tap(Vec2),
    /// Two taps in quick succession at roughly the same spot.
    DoubleTap(Vec2),
    /// A single contact held still past the long-press threshold.
    LongPress(Vec2),

    /// Dragging crossed the movement threshold; pan deltas will follow.
    PanStart(Vec2),
    /// Per-frame drag delta while panning (raw, un-thresholded movement).
    Pan { delta: Vec2, pos: Vec2 },
    /// The active drag ended (fires before any Swipe/Fling decision).
    PanEnd(Vec2),

    /// Directional flicks. `vel` is the release speed along the dominant axis (px/s).
    SwipeLeft { vel: f32 },
    SwipeRight { vel: f32 },
    SwipeUp { vel: f32 },
    SwipeDown { vel: f32 },

    /// Two-finger / ctrl+wheel scale. `rel` is the multiplier since the last
    /// Pinch event; `scale` is cumulative since the gesture began.
    Pinch { scale: f32, rel: f32, center: Vec2 },

    /// A release fast enough to coast. `momentum()` should now be polled.
    Fling { vel: Vec2 },
    /// Emitted each frame by [`GestureRecognizer::momentum`] while gliding.
    MomentumTick { delta: Vec2 },
    /// Momentum decayed to rest.
    MomentumEnd,
}

/// Tunable thresholds. Defaults are tuned for finger touch on ~scale-1 displays
/// and match the values in the interactive demo's JS twin.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Min total displacement (px) along an axis to count as a swipe.
    pub swipe_min_dist: f32,
    /// ...or min release velocity (px/s) along that axis.
    pub swipe_min_vel: f32,
    /// Max movement (px) still considered a tap rather than a drag.
    pub tap_max_dist: f32,
    /// Max press duration (ms) for a tap.
    pub tap_max_time: f64,
    /// Max gap (ms) between taps to be a double-tap.
    pub double_tap_gap: f64,
    /// Hold time (ms) before a stationary press becomes a long-press.
    pub long_press_time: f64,
    /// Min release speed (px/s) to start momentum when no swipe fired.
    pub fling_min_vel: f32,
    /// Per-frame velocity multiplier during momentum (0..1). Higher = longer glide.
    pub friction: f32,
    /// Momentum stops when per-frame speed drops below this (px/frame).
    pub momentum_stop: f32,
    /// How dominant one axis must be over the other to lock swipe direction.
    pub axis_lock: f32,
    /// Accumulated trackpad wheel (px) on one axis before emitting a swipe.
    pub wheel_swipe_step: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            swipe_min_dist: 42.0,
            swipe_min_vel: 480.0,
            tap_max_dist: 9.0,
            tap_max_time: 260.0,
            double_tap_gap: 280.0,
            long_press_time: 480.0,
            fling_min_vel: 550.0,
            friction: 0.93,
            momentum_stop: 6.0,
            axis_lock: 1.3,
            wheel_swipe_step: 120.0,
        }
    }
}

/// Sliding-window velocity estimator: weights the most recent samples, ignoring
/// stale ones, so a finger that paused before lifting doesn't report stale speed.
#[derive(Default)]
struct VelocityTracker {
    samples: Vec<(f64, Vec2)>, // (t_ms, pos)
}
impl VelocityTracker {
    const WINDOW_MS: f64 = 90.0;
    fn push(&mut self, t: f64, p: Vec2) {
        self.samples.push((t, p));
        let cutoff = t - Self::WINDOW_MS;
        while self.samples.len() > 1 && self.samples[0].0 < cutoff {
            self.samples.remove(0);
        }
    }
    /// px/s, computed last-vs-first across the retained window.
    fn velocity(&self) -> Vec2 {
        if self.samples.len() < 2 {
            return Vec2::ZERO;
        }
        let (ta, pa) = self.samples[0];
        let (tb, pb) = self.samples[self.samples.len() - 1];
        let dt = ((tb - ta).max(1.0) / 1000.0) as f32;
        Vec2::new((pb.x - pa.x) / dt, (pb.y - pa.y) / dt)
    }
}

struct ContactState {
    pos: Vec2,
    start: Vec2,
    start_t: f64,
    vt: VelocityTracker,
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Press,
    Pan,
    Pinch,
    Long,
}

/// The recognizer. One instance per interactive surface (or one shared instance
/// if surfaces are mutually exclusive, as in the demo). Cheap to construct.
pub struct GestureRecognizer {
    cfg: Config,
    contacts: HashMap<u64, ContactState>,
    phase: Phase,
    did_pan: bool,
    long_fired: bool,
    last_tap_t: f64,
    last_tap_pos: Option<Vec2>,
    pinch_start_dist: f32,
    pinch_last_scale: f32,
    pinch_center: Vec2,
    momentum_vel: Option<Vec2>, // px/frame
    /// Previous reported position of the lone contact, for exact per-frame pan
    /// deltas in the single-finger case. Reset to `None` on lift / pinch so a new
    /// drag starts from a zero delta rather than jumping from a stale position.
    last_single_pos: Option<Vec2>,
    // trackpad wheel accumulation
    wheel_accum: f32,
    wheel_axis_x: Option<bool>,
    wheel_t: f64,
}

impl Default for GestureRecognizer {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

impl GestureRecognizer {
    pub fn new(cfg: Config) -> Self {
        GestureRecognizer {
            cfg,
            contacts: HashMap::new(),
            phase: Phase::Idle,
            did_pan: false,
            long_fired: false,
            last_tap_t: 0.0,
            last_tap_pos: None,
            pinch_start_dist: 0.0,
            pinch_last_scale: 1.0,
            pinch_center: Vec2::ZERO,
            momentum_vel: None,
            last_single_pos: None,
            wheel_accum: 0.0,
            wheel_axis_x: None,
            wheel_t: 0.0,
        }
    }

    /// True while a drag is in progress this frame — handy for suppressing a
    /// click on the element you happened to release over (the demo uses this to
    /// avoid opening a photo at the end of a fling).
    pub fn is_dragging(&self) -> bool {
        self.did_pan
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Feed the **complete set of currently-down pointers** plus `now` (ms).
    /// Returns every gesture recognized this frame. Call once per frame.
    pub fn update(&mut self, touches: &[Touch], now: f64) -> Vec<Gesture> {
        let mut out = Vec::new();

        // Any new input cancels an in-flight glide.
        if !touches.is_empty() {
            self.momentum_vel = None;
        }

        let cur_ids: Vec<u64> = touches.iter().map(|t| t.id).collect();

        // ---- detect lifts (in prev, not in current) ----
        let lifted: Vec<u64> = self
            .contacts
            .keys()
            .copied()
            .filter(|id| !cur_ids.contains(id))
            .collect();
        for id in lifted {
            if let Some(state) = self.contacts.remove(&id) {
                self.on_lift(state, now, &mut out);
            }
        }

        // ---- ingest current touches (new + moved) ----
        let mut became_two = false;
        for t in touches {
            match self.contacts.get_mut(&t.id) {
                None => {
                    let mut vt = VelocityTracker::default();
                    vt.push(now, t.pos);
                    self.contacts.insert(
                        t.id,
                        ContactState {
                            pos: t.pos,
                            start: t.pos,
                            start_t: now,
                            vt,
                        },
                    );
                    if self.contacts.len() == 1 {
                        self.phase = Phase::Press;
                        self.did_pan = false;
                        self.long_fired = false;
                    } else if self.contacts.len() == 2 {
                        became_two = true;
                    }
                }
                Some(state) => {
                    state.pos = t.pos;
                    state.vt.push(now, t.pos);
                }
            }
        }

        if became_two {
            self.begin_pinch();
        }

        // ---- movement interpretation ----
        if self.contacts.len() >= 2 && self.phase == Phase::Pinch {
            self.update_pinch(&mut out);
        } else if self.contacts.len() == 1 {
            // Single-finger pan / pan-start.
            // Copy what we need out of the contact in a tight scope so the borrow
            // of `self.contacts` is released before the `&mut self` call to
            // `frame_delta` and the `self.did_pan` / `self.phase` mutations below.
            let (pos, start) = {
                let s = self.contacts.values().next().unwrap();
                (s.pos, s.start)
            };
            let total = pos - start;
            let pf = self.frame_delta(pos); // exact per-frame delta vs. last pos
            if !self.did_pan && total.length() > self.cfg.tap_max_dist {
                self.did_pan = true;
                self.phase = Phase::Pan;
                out.push(Gesture::PanStart(pos));
            }
            if self.did_pan && (pf.x != 0.0 || pf.y != 0.0) {
                out.push(Gesture::Pan { delta: pf, pos });
            }
        }

        // ---- long press (stationary hold) ----
        if self.contacts.len() == 1 && !self.did_pan && !self.long_fired {
            // Pull the (Copy) fields out in a `let` — whose temporaries drop at the
            // semicolon — so no borrow of `self.contacts` is held across the
            // `self.long_fired` / `self.phase` mutations.
            let held = self
                .contacts
                .values()
                .next()
                .map(|s| (s.start_t, s.pos, s.start));
            if let Some((start_t, pos, start)) = held {
                if (now - start_t) >= self.cfg.long_press_time
                    && (pos - start).length() <= self.cfg.tap_max_dist
                {
                    out.push(Gesture::LongPress(pos));
                    self.long_fired = true;
                    self.phase = Phase::Long;
                }
            }
        }

        if self.contacts.is_empty() && self.momentum_vel.is_none() {
            self.phase = Phase::Idle;
            self.did_pan = false;
        }
        out
    }

    /// Trackpad / wheel input. `ctrl` true means a pinch-zoom gesture (browsers and
    /// most OSes map trackpad pinch to ctrl+wheel). Returns gestures (Pan, Pinch,
    /// and quick-flick Swipes). Call when a scroll event arrives.
    pub fn wheel(&mut self, delta: Vec2, ctrl: bool, at: Vec2, now: f64) -> Vec<Gesture> {
        self.momentum_vel = None;
        let mut out = Vec::new();
        if ctrl {
            let scale = (-delta.y * 0.01).exp();
            out.push(Gesture::Pinch {
                scale,
                rel: scale,
                center: at,
            });
            return out;
        }
        // two-finger scroll => pan (note: natural-scroll sign flip)
        out.push(Gesture::Pan {
            delta: Vec2::new(-delta.x, -delta.y),
            pos: at,
        });
        if now - self.wheel_t > 140.0 {
            self.wheel_accum = 0.0;
            self.wheel_axis_x = None;
        }
        self.wheel_t = now;
        if self.wheel_axis_x.is_none() {
            self.wheel_axis_x = Some(delta.x.abs() > delta.y.abs());
        }
        if self.wheel_axis_x == Some(true) {
            self.wheel_accum += delta.x;
            if self.wheel_accum.abs() > self.cfg.wheel_swipe_step {
                let vel = self.wheel_accum * 9.0;
                out.push(if self.wheel_accum < 0.0 {
                    Gesture::SwipeLeft { vel }
                } else {
                    Gesture::SwipeRight { vel }
                });
                self.wheel_accum = 0.0;
            }
        }
        out
    }

    /// Advance fling momentum. Call every frame after a [`Gesture::Fling`]; returns
    /// `MomentumTick`s until the glide decays, then a single `MomentumEnd`.
    pub fn momentum(&mut self, _now: f64) -> Vec<Gesture> {
        let mut out = Vec::new();
        if let Some(mut v) = self.momentum_vel {
            v = v * self.cfg.friction;
            if v.length() < self.cfg.momentum_stop {
                self.momentum_vel = None;
                out.push(Gesture::MomentumEnd);
                if self.contacts.is_empty() {
                    self.phase = Phase::Idle;
                }
            } else {
                self.momentum_vel = Some(v);
                out.push(Gesture::MomentumTick { delta: v });
            }
        }
        out
    }

    // ---- internals ----

    fn begin_pinch(&mut self) {
        let pts: Vec<Vec2> = self.contacts.values().map(|c| c.pos).take(2).collect();
        if pts.len() == 2 {
            self.pinch_start_dist = pts[0].dist(pts[1]).max(1.0);
            self.pinch_last_scale = 1.0;
            self.pinch_center = (pts[0] + pts[1]) * 0.5;
            self.phase = Phase::Pinch;
        }
    }

    fn update_pinch(&mut self, out: &mut Vec<Gesture>) {
        let pts: Vec<Vec2> = self.contacts.values().map(|c| c.pos).take(2).collect();
        if pts.len() == 2 {
            let d = pts[0].dist(pts[1]).max(1.0);
            let scale = d / self.pinch_start_dist;
            let rel = scale / self.pinch_last_scale;
            self.pinch_last_scale = scale;
            out.push(Gesture::Pinch {
                scale,
                rel,
                center: self.pinch_center,
            });
        }
    }

    /// Exact per-frame pan delta for the single-finger case: the difference between
    /// the current position and the previously reported one, tracked in a dedicated
    /// cell (`last_single_pos`) that is cleared on lift / pinch so a fresh drag
    /// starts at a zero delta rather than jumping from a stale position.
    fn frame_delta(&mut self, cur: Vec2) -> Vec2 {
        let d = cur - self.last_single_pos.unwrap_or(cur);
        self.last_single_pos = Some(cur);
        d
    }

    fn on_lift(&mut self, state: ContactState, now: f64, out: &mut Vec<Gesture>) {
        // a pinch finger lifting just ends the pinch
        if self.phase == Phase::Pinch && self.contacts.len() < 2 {
            self.phase = if self.contacts.is_empty() {
                Phase::Idle
            } else {
                Phase::Pan
            };
            self.last_single_pos = None;
            return;
        }

        let total = state.pos - state.start;
        let dt = now - state.start_t;
        let v = state.vt.velocity();

        if !self.did_pan {
            // tap / double-tap (long-press already emitted live)
            if self.phase != Phase::Long
                && total.length() <= self.cfg.tap_max_dist
                && dt <= self.cfg.tap_max_time
            {
                let is_double = self.last_tap_pos.map_or(false, |lp| {
                    (now - self.last_tap_t) < self.cfg.double_tap_gap
                        && state.pos.dist(lp) < 24.0
                });
                if is_double {
                    out.push(Gesture::DoubleTap(state.pos));
                    self.last_tap_t = 0.0;
                    self.last_tap_pos = None;
                } else {
                    out.push(Gesture::Tap(state.pos));
                    self.last_tap_t = now;
                    self.last_tap_pos = Some(state.pos);
                }
            }
            self.last_single_pos = None;
            return;
        }

        // was panning -> swipe / fling / plain end
        out.push(Gesture::PanEnd(state.pos));
        let (ax, ay) = (total.x.abs(), total.y.abs());
        let (avx, avy) = (v.x.abs(), v.y.abs());
        let horiz = ax > ay * self.cfg.axis_lock || avx > avy * self.cfg.axis_lock;
        let vert = ay > ax * self.cfg.axis_lock || avy > avx * self.cfg.axis_lock;
        let mut fired = false;

        if horiz {
            if ax > self.cfg.swipe_min_dist || avx > self.cfg.swipe_min_vel {
                out.push(if total.x < 0.0 {
                    Gesture::SwipeLeft { vel: v.x }
                } else {
                    Gesture::SwipeRight { vel: v.x }
                });
                fired = true;
            }
        } else if vert {
            if ay > self.cfg.swipe_min_dist || avy > self.cfg.swipe_min_vel {
                out.push(if total.y < 0.0 {
                    Gesture::SwipeUp { vel: v.y }
                } else {
                    Gesture::SwipeDown { vel: v.y }
                });
                fired = true;
            }
        }

        if !fired && v.length() > self.cfg.fling_min_vel {
            out.push(Gesture::Fling { vel: v });
            self.momentum_vel = Some(v * (1.0 / 60.0)); // px/frame seed
        }
        self.last_single_pos = None;
    }
}

/// Conversions between [`Vec2`] and egui's geometry types, compiled only when the
/// `egui` feature is on. This is deliberately tiny — the per-frame input adapter
/// (egui events -> `Touch` set, wheel/zoom -> `wheel()`) lives in
/// `sketches/egui_input.rs` so the default build pulls in no UI dependency at all.
///
/// Targets egui's stable `Vec2 { x, y }` / `Pos2 { x, y }` shape; pin the `egui`
/// dep in Cargo.toml to your workspace version.
#[cfg(feature = "egui")]
mod egui_compat {
    use crate::Vec2;

    impl From<egui::Vec2> for Vec2 {
        #[inline]
        fn from(v: egui::Vec2) -> Self {
            Vec2::new(v.x, v.y)
        }
    }
    impl From<egui::Pos2> for Vec2 {
        #[inline]
        fn from(p: egui::Pos2) -> Self {
            Vec2::new(p.x, p.y)
        }
    }
    // `impl From<LocalType> for ForeignType` is permitted by the orphan rule
    // because the trait's type argument (`Vec2`) is local to this crate.
    impl From<Vec2> for egui::Vec2 {
        #[inline]
        fn from(v: Vec2) -> Self {
            egui::vec2(v.x, v.y)
        }
    }
    impl From<Vec2> for egui::Pos2 {
        #[inline]
        fn from(v: Vec2) -> Self {
            egui::pos2(v.x, v.y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, x: f32, y: f32) -> Touch {
        Touch::new(id, Vec2::new(x, y))
    }

    #[test]
    fn tap_is_recognized() {
        let mut r = GestureRecognizer::default();
        r.update(&[t(1, 50.0, 50.0)], 0.0);
        let g = r.update(&[], 80.0);
        assert!(g.iter().any(|x| matches!(x, Gesture::Tap(_))));
    }

    #[test]
    fn fast_horizontal_drag_is_swipe_left() {
        let mut r = GestureRecognizer::default();
        r.update(&[t(1, 300.0, 100.0)], 0.0);
        r.update(&[t(1, 200.0, 101.0)], 12.0);
        r.update(&[t(1, 90.0, 102.0)], 24.0);
        let g = r.update(&[], 36.0);
        assert!(
            g.iter().any(|x| matches!(x, Gesture::SwipeLeft { .. })),
            "expected SwipeLeft, got {:?}",
            g
        );
    }

    #[test]
    fn small_slow_drag_is_not_a_swipe() {
        let mut r = GestureRecognizer::default();
        r.update(&[t(1, 100.0, 100.0)], 0.0);
        r.update(&[t(1, 104.0, 100.0)], 200.0);
        let g = r.update(&[], 400.0);
        assert!(!g.iter().any(|x| matches!(
            x,
            Gesture::SwipeLeft { .. } | Gesture::SwipeRight { .. }
        )));
    }

    #[test]
    fn two_fingers_spreading_emit_pinch() {
        let mut r = GestureRecognizer::default();
        r.update(&[t(1, 100.0, 100.0), t(2, 200.0, 100.0)], 0.0);
        let g = r.update(&[t(1, 80.0, 100.0), t(2, 220.0, 100.0)], 16.0);
        let scale = g.iter().find_map(|x| match x {
            Gesture::Pinch { scale, .. } => Some(*scale),
            _ => None,
        });
        assert!(scale.map_or(false, |s| s > 1.0), "expected zoom-in pinch");
    }

    #[test]
    fn fling_seeds_momentum_that_decays() {
        let mut r = GestureRecognizer::default();
        // A ~45° flick: neither axis dominates, so it is NOT classified as a
        // cardinal swipe (which would preempt the fling) — but the release is fast,
        // which is exactly the Fling -> momentum path this test exercises.
        r.update(&[t(1, 300.0, 300.0)], 0.0);
        r.update(&[t(1, 200.0, 200.0)], 10.0);
        r.update(&[t(1, 90.0, 90.0)], 20.0);
        let g = r.update(&[], 30.0);
        assert!(
            g.iter().any(|x| matches!(x, Gesture::Fling { .. })),
            "expected Fling for an off-axis flick, got {:?}",
            g
        );
        // momentum should produce ticks then end
        let mut ticks = 0;
        let mut ended = false;
        for f in 0..400 {
            for ev in r.momentum(30.0 + f as f64 * 16.0) {
                match ev {
                    Gesture::MomentumTick { .. } => ticks += 1,
                    Gesture::MomentumEnd => ended = true,
                    _ => {}
                }
            }
            if ended {
                break;
            }
        }
        assert!(ticks > 0 && ended, "momentum should tick then end");
    }
}
