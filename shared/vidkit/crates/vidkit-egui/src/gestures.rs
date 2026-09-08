//! Gesture routing for the video surface.
//!
//! A pure state machine: pointer positions in, [`Intent`]s out. No egui types
//! beyond `Pos2`/`Rect`, no playback state, no side effects — so every rule
//! below is testable without laying out a frame or decoding anything. The
//! widget feeds it and applies what comes back.
//!
//! # The zones
//!
//! ```text
//!   +-------------------------------+-+
//!   |               |               |R|   R = right EDGE strip: vertical
//!   |   left half   |  right half   |o|       slide rotates 90 degrees
//!   |   = volume    |  = speed      |t|
//!   |               |               |a|
//!   +-------------------------------+-+
//!    horizontal drag anywhere = scrub, at a rate set by height
//! ```
//!
//! "Slide down on the right side" and "swipe down on the right half" cannot
//! both own the same pixels, so they are read as different regions: a narrow
//! edge strip rotates, the rest of the right half is speed. If that is the
//! wrong reading, [`GestureConfig::rotate_edge_fraction`] is the single knob —
//! set it to 0.0 and the rotate zone disappears entirely.
//!
//! # Two rules that stop gestures fighting
//!
//! **The zone is decided at press, not continuously.** Otherwise a volume drag
//! that wanders past the midline turns into a speed drag halfway through, and
//! the user sees two settings move for one gesture.
//!
//! **The axis locks once, after a few pixels of movement.** A scrub that drifts
//! vertically must not start changing the volume, and a volume drag must not
//! seek. Until the lock fires nothing is emitted, which is why the threshold is
//! small.

use egui::{Pos2, Rect};

/// Which region a press landed in.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Zone {
    LeftHalf,
    RightHalf,
    /// The narrow strip at the right edge.
    RightEdge,
}

/// The axis a drag committed to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DragAxis {
    Horizontal,
    Vertical,
}

/// One thing the user asked for. A drag emits a stream of these.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Intent {
    /// Live scrub position change, in seconds. Not committed yet.
    ScrubBy(f64),
    /// Scrub finished; apply the accumulated offset in seconds.
    ScrubCommit(f64),
    /// One detent of playback rate, in +/-5% steps.
    SpeedStep(i32),
    /// One detent of volume, in +/-5% steps.
    VolumeStep(i32),
    /// Quarter turns clockwise; negative is anticlockwise.
    Rotate(i32),
    /// One detent of zoom, in +/-5% steps.
    ZoomStep(i32),
    /// Snap to a locked view instead of free zoom.
    LockView(ViewLock),
    TogglePlay,
}

/// The "screen locked" views a pinch snaps to at its extremes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ViewLock {
    /// Whole frame visible.
    Fit,
    /// Fill the surface, cropping.
    Fill,
    /// 1:1 with the source.
    Actual,
}

#[derive(Copy, Clone, Debug)]
pub struct GestureConfig {
    /// Movement needed before the axis locks, in points.
    pub axis_lock_px: f32,
    /// Width of the rotate strip, as a fraction of the surface. 0 disables it.
    pub rotate_edge_fraction: f32,
    /// Vertical travel per volume/speed detent, in points.
    pub detent_px: f32,
    /// Vertical travel per rotation, in points.
    pub rotate_px: f32,
    /// Pinch scale change per zoom detent. 0.05 = 5%.
    pub zoom_detent: f32,
    /// Pinch beyond this total scale snaps to a locked view.
    pub lock_zoom_in: f32,
    pub lock_zoom_out: f32,
    /// Seconds of media per surface width, when the pointer is at the very
    /// bottom of the surface.
    pub scrub_span_at_bottom: f64,
    /// ...and at the very top.
    ///
    /// Taken literally from "height from bottom determining speed": higher is
    /// faster. Note that mpv, VLC and the phone players do the opposite —
    /// dragging away from the bar gives FINER control — so if this feels
    /// backwards, swap these two numbers and nothing else changes.
    pub scrub_span_at_top: f64,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            axis_lock_px: 8.0,
            rotate_edge_fraction: 0.10,
            detent_px: 28.0,
            rotate_px: 90.0,
            zoom_detent: 0.05,
            lock_zoom_in: 2.5,
            lock_zoom_out: 0.5,
            scrub_span_at_bottom: 30.0,
            scrub_span_at_top: 600.0,
        }
    }
}

/// Which zone a point falls in.
pub fn zone_at(surface: Rect, at: Pos2, cfg: &GestureConfig) -> Zone {
    let w = surface.width().max(1.0);
    let edge = (cfg.rotate_edge_fraction.clamp(0.0, 0.5) * w).max(0.0);
    if edge > 0.0 && at.x >= surface.max.x - edge {
        return Zone::RightEdge;
    }
    if at.x >= surface.center().x {
        Zone::RightHalf
    } else {
        Zone::LeftHalf
    }
}

/// Seconds of media spanned by a full-width drag, given where the pointer is
/// vertically. 0 at the bottom edge, 1 at the top.
pub fn scrub_span(surface: Rect, at: Pos2, cfg: &GestureConfig) -> f64 {
    let h = surface.height().max(1.0);
    let from_bottom = ((surface.max.y - at.y) / h).clamp(0.0, 1.0) as f64;
    cfg.scrub_span_at_bottom + (cfg.scrub_span_at_top - cfg.scrub_span_at_bottom) * from_bottom
}

#[derive(Clone, Debug)]
struct Active {
    zone: Zone,
    axis: Option<DragAxis>,
    origin: Pos2,
    last: Pos2,
    /// Movement since press, for the axis-lock decision.
    total: egui::Vec2,
    /// Accumulated seconds for a scrub.
    scrub_seconds: f64,
    /// Vertical travel not yet spent on a detent.
    detent_residue: f32,
}

/// Routes pointer input to playback intents.
pub struct GestureRouter {
    pub cfg: GestureConfig,
    active: Option<Active>,
    /// Accumulated pinch scale not yet spent on a detent.
    zoom_residue: f32,
    /// Total pinch scale for the current pinch, for the lock thresholds.
    pinch_scale: f32,
}

impl GestureRouter {
    pub fn new(cfg: GestureConfig) -> Self {
        Self {
            cfg,
            active: None,
            zoom_residue: 0.0,
            pinch_scale: 1.0,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.active.is_some()
    }

    /// The zone and axis of the drag in progress, for a HUD.
    pub fn current(&self) -> Option<(Zone, Option<DragAxis>)> {
        self.active.as_ref().map(|a| (a.zone, a.axis))
    }

    pub fn press(&mut self, at: Pos2, surface: Rect) {
        self.active = Some(Active {
            zone: zone_at(surface, at, &self.cfg),
            axis: None,
            origin: at,
            last: at,
            total: egui::Vec2::ZERO,
            scrub_seconds: 0.0,
            detent_residue: 0.0,
        });
    }

    /// Continue a drag. `to` is the pointer's current position.
    pub fn drag_to(&mut self, to: Pos2, surface: Rect) -> Vec<Intent> {
        let cfg = self.cfg;
        let mut out = Vec::new();
        let a = match self.active.as_mut() {
            Some(a) => a,
            // A drag with no press: ignore rather than inventing an origin,
            // which would make the first delta enormous.
            None => return out,
        };

        let delta = to - a.last;
        a.last = to;
        a.total += delta;

        if a.axis.is_none() {
            if a.total.length() < cfg.axis_lock_px {
                return out;
            }
            a.axis = Some(if a.total.x.abs() >= a.total.y.abs() {
                DragAxis::Horizontal
            } else {
                DragAxis::Vertical
            });
        }

        match (a.axis, a.zone) {
            // Horizontal anywhere scrubs, at a rate set by height.
            (Some(DragAxis::Horizontal), _) => {
                let span = scrub_span(surface, to, &cfg);
                let seconds = (delta.x / surface.width().max(1.0)) as f64 * span;
                a.scrub_seconds += seconds;
                out.push(Intent::ScrubBy(seconds));
            }
            // The edge strip rotates: down is clockwise.
            (Some(DragAxis::Vertical), Zone::RightEdge) => {
                a.detent_residue += delta.y;
                while a.detent_residue >= cfg.rotate_px {
                    a.detent_residue -= cfg.rotate_px;
                    out.push(Intent::Rotate(1));
                }
                while a.detent_residue <= -cfg.rotate_px {
                    a.detent_residue += cfg.rotate_px;
                    out.push(Intent::Rotate(-1));
                }
            }
            // Vertical on a half adjusts speed or volume. Up is more, which is
            // the direction every volume control in the world agrees on.
            (Some(DragAxis::Vertical), zone) => {
                a.detent_residue += delta.y;
                let mut emit = |n: i32, out: &mut Vec<Intent>| {
                    out.push(match zone {
                        Zone::LeftHalf => Intent::VolumeStep(n),
                        _ => Intent::SpeedStep(n),
                    });
                };
                while a.detent_residue <= -cfg.detent_px {
                    a.detent_residue += cfg.detent_px;
                    emit(1, &mut out);
                }
                while a.detent_residue >= cfg.detent_px {
                    a.detent_residue -= cfg.detent_px;
                    emit(-1, &mut out);
                }
            }
            (None, _) => {}
        }
        out
    }

    /// Finish a drag. Emits the scrub commit if one was in progress.
    pub fn release(&mut self) -> Vec<Intent> {
        let mut out = Vec::new();
        if let Some(a) = self.active.take() {
            if a.axis == Some(DragAxis::Horizontal) && a.scrub_seconds != 0.0 {
                out.push(Intent::ScrubCommit(a.scrub_seconds));
            }
        }
        out
    }

    /// A tap: press and release with no committed axis.
    ///
    /// Returned separately from `release` because a drag that never locked an
    /// axis is a tap, and a tap toggles playback — but a drag that DID lock
    /// must not also toggle, which is the bug where seeking pauses the video.
    pub fn tap(&mut self) -> Vec<Intent> {
        let was_drag = self
            .active
            .as_ref()
            .map(|a| a.axis.is_some())
            .unwrap_or(false);
        self.active = None;
        if was_drag {
            Vec::new()
        } else {
            vec![Intent::TogglePlay]
        }
    }

    /// Reset the pinch accumulator. Call when a pinch begins.
    pub fn pinch_begin(&mut self) {
        self.zoom_residue = 0.0;
        self.pinch_scale = 1.0;
    }

    /// Feed a pinch. `zoom_delta` is egui's multiplicative factor for the
    /// frame (1.0 = no change).
    ///
    /// Emits 5% detents, or a view lock once the pinch passes the thresholds —
    /// a hard snap rather than more detents, because past a certain point the
    /// user is choosing a mode, not a magnification.
    pub fn pinch(&mut self, zoom_delta: f32) -> Vec<Intent> {
        let mut out = Vec::new();
        if zoom_delta <= 0.0 || (zoom_delta - 1.0).abs() < f32::EPSILON {
            return out;
        }
        self.pinch_scale *= zoom_delta;

        if self.pinch_scale >= self.cfg.lock_zoom_in {
            self.pinch_begin();
            out.push(Intent::LockView(ViewLock::Fill));
            return out;
        }
        if self.pinch_scale <= self.cfg.lock_zoom_out {
            self.pinch_begin();
            out.push(Intent::LockView(ViewLock::Fit));
            return out;
        }

        self.zoom_residue += zoom_delta - 1.0;
        let d = self.cfg.zoom_detent.max(0.001);
        while self.zoom_residue >= d {
            self.zoom_residue -= d;
            out.push(Intent::ZoomStep(1));
        }
        while self.zoom_residue <= -d {
            self.zoom_residue += d;
            out.push(Intent::ZoomStep(-1));
        }
        out
    }
}

impl Default for GestureRouter {
    fn default() -> Self {
        Self::new(GestureConfig::default())
    }
}

/// Apply a speed detent, clamped and quantised to 5%.
///
/// Quantised on the way out so repeated steps cannot accumulate float drift
/// into 1.0499999 and show "104%".
pub fn apply_speed_step(current: f64, steps: i32) -> f64 {
    let stepped = (current * 20.0).round() as i32 + steps;
    (stepped as f64 / 20.0).clamp(0.25, 4.0)
}

/// Apply a volume detent, clamped to 0..1 and quantised to 5%.
pub fn apply_volume_step(current: f32, steps: i32) -> f32 {
    let stepped = (current * 20.0).round() as i32 + steps;
    (stepped as f32 / 20.0).clamp(0.0, 1.0)
}

/// Apply a zoom detent, quantised to 5% and clamped to something sane.
pub fn apply_zoom_step(current: f32, steps: i32) -> f32 {
    let stepped = (current * 20.0).round() as i32 + steps;
    (stepped as f32 / 20.0).clamp(0.25, 8.0)
}

/// Normalise quarter turns to 0..=3.
pub fn apply_rotation(current: i32, quarter_turns: i32) -> i32 {
    (current + quarter_turns).rem_euclid(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};

    fn surface() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 400.0))
    }

    fn router() -> GestureRouter {
        GestureRouter::default()
    }

    /// Drag far enough in one direction to lock the axis, then return the
    /// intents from the movement that follows.
    fn drag(r: &mut GestureRouter, from: Pos2, by: Vec<egui::Vec2>) -> Vec<Intent> {
        r.press(from, surface());
        let mut at = from;
        let mut out = Vec::new();
        for d in by {
            at += d;
            out.extend(r.drag_to(at, surface()));
        }
        out
    }

    // ---- zones ---------------------------------------------------------

    #[test]
    fn zones_split_left_right_and_edge() {
        let cfg = GestureConfig::default();
        let s = surface();
        assert_eq!(zone_at(s, pos2(100.0, 200.0), &cfg), Zone::LeftHalf);
        assert_eq!(zone_at(s, pos2(500.0, 200.0), &cfg), Zone::RightHalf);
        // Edge strip is the last 10% -> x >= 720.
        assert_eq!(zone_at(s, pos2(760.0, 200.0), &cfg), Zone::RightEdge);
        assert_eq!(zone_at(s, pos2(719.0, 200.0), &cfg), Zone::RightHalf);
    }

    /// The escape hatch for the ambiguous "right side" vs "right half"
    /// reading: zero the strip and the rotate zone is gone.
    #[test]
    fn a_zero_edge_fraction_removes_the_rotate_zone() {
        let cfg = GestureConfig {
            rotate_edge_fraction: 0.0,
            ..Default::default()
        };
        assert_eq!(zone_at(surface(), pos2(799.0, 200.0), &cfg), Zone::RightHalf);
    }

    // ---- axis lock ------------------------------------------------------

    #[test]
    fn nothing_is_emitted_before_the_axis_locks() {
        let mut r = router();
        let out = drag(&mut r, pos2(400.0, 200.0), vec![vec2(3.0, 0.0), vec2(2.0, 0.0)]);
        assert!(out.is_empty(), "emitted before lock: {out:?}");
        assert_eq!(r.current().unwrap().1, None);
    }

    #[test]
    fn a_mostly_horizontal_drag_locks_horizontal_and_stays_there() {
        let mut r = router();
        drag(&mut r, pos2(400.0, 200.0), vec![vec2(20.0, 2.0)]);
        assert_eq!(r.current().unwrap().1, Some(DragAxis::Horizontal));
        // A big vertical wobble afterwards must still scrub, not change volume.
        let out = r.drag_to(pos2(420.0, 300.0), surface());
        assert!(
            out.iter().all(|i| matches!(i, Intent::ScrubBy(_))),
            "axis lock leaked: {out:?}"
        );
    }

    #[test]
    fn a_mostly_vertical_drag_locks_vertical() {
        let mut r = router();
        drag(&mut r, pos2(200.0, 200.0), vec![vec2(2.0, -20.0)]);
        assert_eq!(r.current().unwrap().1, Some(DragAxis::Vertical));
    }

    /// The zone is fixed at press. A volume drag that wanders right must not
    /// become a speed drag, or one gesture moves two settings.
    #[test]
    fn the_zone_does_not_change_mid_drag() {
        let mut r = router();
        r.press(pos2(100.0, 200.0), surface()); // left half
        let mut out = r.drag_to(pos2(100.0, 160.0), surface());
        out.extend(r.drag_to(pos2(700.0, 100.0), surface())); // now on the right
        assert!(
            out.iter().all(|i| matches!(i, Intent::VolumeStep(_))),
            "zone changed mid-drag: {out:?}"
        );
    }

    // ---- scrub ----------------------------------------------------------

    #[test]
    fn a_rightward_drag_scrubs_forward_and_commits_on_release() {
        let mut r = router();
        let out = drag(&mut r, pos2(400.0, 200.0), vec![vec2(80.0, 0.0)]);
        let moved: f64 = out
            .iter()
            .filter_map(|i| match i {
                Intent::ScrubBy(s) => Some(*s),
                _ => None,
            })
            .sum();
        assert!(moved > 0.0, "rightward drag went backwards: {moved}");

        let commit = r.release();
        match commit.as_slice() {
            [Intent::ScrubCommit(s)] => assert!((s - moved).abs() < 1e-6),
            other => panic!("expected one commit, got {other:?}"),
        }
    }

    /// The height rule, taken literally from the request: higher is faster.
    #[test]
    fn scrub_speed_rises_with_height_above_the_bottom() {
        let cfg = GestureConfig::default();
        let s = surface();
        let low = scrub_span(s, pos2(400.0, 390.0), &cfg);
        let mid = scrub_span(s, pos2(400.0, 200.0), &cfg);
        let high = scrub_span(s, pos2(400.0, 10.0), &cfg);
        assert!(low < mid && mid < high, "{low} {mid} {high}");
        // Anchors at the actual edges, not near them: y=390 in a 400-tall
        // surface is 2.5% up, which is 44s rather than 30s. Asserting the
        // endpoint value at an interior point is how a correct curve looks
        // broken.
        let at_bottom = scrub_span(s, pos2(400.0, s.max.y), &cfg);
        let at_top = scrub_span(s, pos2(400.0, s.min.y), &cfg);
        assert!((at_bottom - cfg.scrub_span_at_bottom).abs() < 1e-3, "{at_bottom}");
        assert!((at_top - cfg.scrub_span_at_top).abs() < 1e-3, "{at_top}");
    }

    #[test]
    fn the_same_drag_seeks_further_when_started_higher() {
        let far = {
            let mut r = router();
            drag(&mut r, pos2(400.0, 10.0), vec![vec2(80.0, 0.0)]);
            r.release()
        };
        let near = {
            let mut r = router();
            drag(&mut r, pos2(400.0, 390.0), vec![vec2(80.0, 0.0)]);
            r.release()
        };
        let secs = |v: &Vec<Intent>| match v.as_slice() {
            [Intent::ScrubCommit(s)] => *s,
            other => panic!("expected a commit, got {other:?}"),
        };
        assert!(
            secs(&far) > secs(&near) * 5.0,
            "height had little effect: {} vs {}",
            secs(&far),
            secs(&near)
        );
    }

    #[test]
    fn scrub_position_clamps_at_the_surface_edges() {
        let cfg = GestureConfig::default();
        let s = surface();
        // Above and below the surface must not extrapolate past the range.
        let above = scrub_span(s, pos2(400.0, -500.0), &cfg);
        let below = scrub_span(s, pos2(400.0, 900.0), &cfg);
        assert!((above - cfg.scrub_span_at_top).abs() < 1e-3);
        assert!((below - cfg.scrub_span_at_bottom).abs() < 1e-3);
    }

    // ---- volume and speed ------------------------------------------------

    #[test]
    fn dragging_up_on_the_left_raises_volume() {
        let mut r = router();
        let out = drag(&mut r, pos2(100.0, 300.0), vec![vec2(0.0, -100.0)]);
        let steps: i32 = out
            .iter()
            .filter_map(|i| match i {
                Intent::VolumeStep(n) => Some(*n),
                _ => None,
            })
            .sum();
        assert!(steps >= 3, "expected several up-steps, got {steps}");
        assert!(out.iter().all(|i| matches!(i, Intent::VolumeStep(_))));
    }

    #[test]
    fn dragging_down_on_the_left_lowers_volume() {
        let mut r = router();
        let out = drag(&mut r, pos2(100.0, 100.0), vec![vec2(0.0, 100.0)]);
        let steps: i32 = out
            .iter()
            .filter_map(|i| match i {
                Intent::VolumeStep(n) => Some(*n),
                _ => None,
            })
            .sum();
        assert!(steps <= -3, "expected down-steps, got {steps}");
    }

    #[test]
    fn dragging_on_the_right_half_changes_speed_not_volume() {
        let mut r = router();
        let out = drag(&mut r, pos2(500.0, 300.0), vec![vec2(0.0, -100.0)]);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|i| matches!(i, Intent::SpeedStep(_))),
            "right half emitted a volume change: {out:?}"
        );
    }

    /// Detents, not a continuous stream: a 10-pixel twitch must do nothing.
    #[test]
    fn small_movements_do_not_fire_a_detent() {
        let mut r = router();
        let out = drag(&mut r, pos2(100.0, 200.0), vec![vec2(0.0, -10.0)]);
        assert!(out.is_empty(), "twitch fired a detent: {out:?}");
    }

    /// The residue carries across calls, so a slow drag still reaches a detent
    /// instead of losing every partial movement.
    #[test]
    fn a_slow_drag_still_accumulates_to_a_detent() {
        let mut r = router();
        let mut steps = vec![vec2(0.0, -9.0); 1];
        steps.extend(vec![vec2(0.0, -3.0); 10]);
        let out = drag(&mut r, pos2(100.0, 300.0), steps);
        assert!(
            out.iter().any(|i| matches!(i, Intent::VolumeStep(1))),
            "slow drag lost its residue: {out:?}"
        );
    }

    // ---- rotate -----------------------------------------------------------

    #[test]
    fn sliding_down_the_right_edge_rotates_clockwise() {
        let mut r = router();
        let out = drag(&mut r, pos2(770.0, 50.0), vec![vec2(0.0, 100.0)]);
        assert_eq!(out, vec![Intent::Rotate(1)]);
    }

    #[test]
    fn sliding_up_the_right_edge_rotates_the_other_way() {
        let mut r = router();
        let out = drag(&mut r, pos2(770.0, 350.0), vec![vec2(0.0, -100.0)]);
        assert_eq!(out, vec![Intent::Rotate(-1)]);
    }

    /// A horizontal drag that starts on the edge strip is still a scrub. The
    /// strip claims an axis, not the whole corner.
    #[test]
    fn a_horizontal_drag_from_the_edge_strip_still_scrubs() {
        let mut r = router();
        let out = drag(&mut r, pos2(770.0, 200.0), vec![vec2(-80.0, 0.0)]);
        assert!(
            out.iter().all(|i| matches!(i, Intent::ScrubBy(_))),
            "edge strip swallowed a scrub: {out:?}"
        );
    }

    #[test]
    fn rotation_wraps_at_four_quarter_turns() {
        assert_eq!(apply_rotation(0, 1), 1);
        assert_eq!(apply_rotation(3, 1), 0);
        assert_eq!(apply_rotation(0, -1), 3);
        assert_eq!(apply_rotation(2, -5), 1);
    }

    // ---- pinch -------------------------------------------------------------

    #[test]
    fn a_pinch_emits_five_percent_detents() {
        let mut r = router();
        r.pinch_begin();
        let out = r.pinch(1.06);
        assert_eq!(out, vec![Intent::ZoomStep(1)]);
    }

    #[test]
    fn a_tiny_pinch_emits_nothing() {
        let mut r = router();
        r.pinch_begin();
        assert!(r.pinch(1.01).is_empty());
    }

    #[test]
    fn pinching_far_out_locks_to_fit() {
        let mut r = router();
        r.pinch_begin();
        let mut out = Vec::new();
        for _ in 0..20 {
            out.extend(r.pinch(0.9));
        }
        assert!(
            out.contains(&Intent::LockView(ViewLock::Fit)),
            "no lock fired: {out:?}"
        );
    }

    #[test]
    fn pinching_far_in_locks_to_fill() {
        let mut r = router();
        r.pinch_begin();
        let mut out = Vec::new();
        for _ in 0..20 {
            out.extend(r.pinch(1.1));
        }
        assert!(out.contains(&Intent::LockView(ViewLock::Fill)), "{out:?}");
    }

    #[test]
    fn a_zero_or_negative_pinch_factor_is_ignored() {
        let mut r = router();
        r.pinch_begin();
        assert!(r.pinch(0.0).is_empty());
        assert!(r.pinch(-1.0).is_empty());
    }

    // ---- tap ---------------------------------------------------------------

    #[test]
    fn a_tap_toggles_playback() {
        let mut r = router();
        r.press(pos2(400.0, 200.0), surface());
        assert_eq!(r.tap(), vec![Intent::TogglePlay]);
    }

    /// The bug where seeking also pauses: a drag that locked an axis is not a
    /// tap, however it ends.
    #[test]
    fn a_completed_drag_does_not_also_toggle_playback() {
        let mut r = router();
        drag(&mut r, pos2(400.0, 200.0), vec![vec2(80.0, 0.0)]);
        assert!(r.tap().is_empty(), "drag toggled playback");
    }

    #[test]
    fn a_drag_with_no_press_is_ignored() {
        let mut r = router();
        assert!(r.drag_to(pos2(400.0, 200.0), surface()).is_empty());
        assert!(r.release().is_empty());
    }

    // ---- appliers -----------------------------------------------------------

    #[test]
    fn speed_steps_are_five_percent_and_clamped() {
        assert_eq!(apply_speed_step(1.0, 1), 1.05);
        assert_eq!(apply_speed_step(1.0, -1), 0.95);
        assert_eq!(apply_speed_step(4.0, 5), 4.0);
        assert_eq!(apply_speed_step(0.25, -5), 0.25);
    }

    /// Repeated steps must not drift into 1.0499999 and display "104%".
    #[test]
    fn repeated_speed_steps_do_not_accumulate_float_drift() {
        let mut s = 1.0;
        for _ in 0..20 {
            s = apply_speed_step(s, 1);
        }
        for _ in 0..20 {
            s = apply_speed_step(s, -1);
        }
        assert_eq!(s, 1.0, "drifted to {s}");
    }

    #[test]
    fn volume_steps_clamp_at_both_ends() {
        assert_eq!(apply_volume_step(1.0, 1), 1.0);
        assert_eq!(apply_volume_step(0.0, -1), 0.0);
        assert_eq!(apply_volume_step(0.5, 1), 0.55);
    }

    #[test]
    fn zoom_steps_clamp() {
        assert_eq!(apply_zoom_step(1.0, 1), 1.05);
        assert_eq!(apply_zoom_step(8.0, 1), 8.0);
        assert_eq!(apply_zoom_step(0.25, -1), 0.25);
    }
}
