//! GestureLayer — the adapter that makes `uikit-gesture::TwoTierGesture` work
//! from egui input with zero per-app gesture code.
//!
//! The app owns one `GestureLayer` per swipe surface. Each frame it calls
//! [`GestureLayer::update`] with the surface rect and the surface's at-limit
//! flag; it gets back a [`GestureFrame`] describing what the model decided
//! (detent fired / page jump) plus the live barrier fraction to paint.
//!
//! Mapping (the snapclone convention):
//!   * horizontal pointer drag / two-finger horizontal scroll → the model
//!     (detent steps a sub-page; release decides a page jump),
//!   * "Next" = content moves left (finger left), so we feed `-dx`.
//!
//! This is the seam where the FP5-tuned feel reaches egui: the app never sees
//! raw deltas, only `DetentFired`/`PageJump`/progress.

use egui::{Rect, Ui, Vec2};
use uikit_gesture::{Direction, GestureConfig, Outcome, ReleaseOutcome, TwoTierGesture};

/// Axis the layer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// What happened on the surface this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct GestureFrame {
    /// A sub-page detent fired this frame (step one sub-page).
    pub detent: Option<Direction>,
    /// A whole-page jump committed this frame (at release).
    pub page_jump: Option<Direction>,
    /// Live barrier progress 0..1 (paint the "almost there" indicator).
    pub barrier_fraction: f32,
    /// Which edge the barrier is on.
    pub barrier_edge: Option<Direction>,
    /// True while the post-fire takeover lock is holding (for a HUD).
    pub locked: bool,
    /// Signed accumulator, for diagnostics/HUD.
    pub raw: f32,
}

/// One gesture surface.
pub struct GestureLayer {
    gesture: TwoTierGesture,
    axis: Axis,
    dragging: bool,
    drag_total: f32,
    // px/ms, signed along the chosen axis.
    last_velocity: f32,
}

impl GestureLayer {
    pub fn new(axis: Axis, cfg: GestureConfig) -> Self {
        Self {
            gesture: TwoTierGesture::new(cfg),
            axis,
            dragging: false,
            drag_total: 0.0,
            last_velocity: 0.0,
        }
    }

    /// A horizontal surface sized to `page_px` (the common snapclone case).
    pub fn horizontal_page(page_px: f32) -> Self {
        Self::new(Axis::Horizontal, GestureConfig::for_page_size(page_px as f64))
    }

    /// Live-retune (Settings ▸ Gestures). Returns the mutable config.
    pub fn config_mut(&mut self) -> &mut GestureConfig {
        self.gesture.config_mut()
    }

    fn axis_of(&self, v: Vec2) -> f32 {
        match self.axis {
            Axis::Horizontal => v.x,
            Axis::Vertical => v.y,
        }
    }

    /// Drive the model from this frame's input.
    ///
    /// `surface` bounds the area that should respond. `at_limit` is the app's
    /// answer to "is the inner content scrolled to its edge on this axis?" —
    /// when false, the detent streak is reset so a fresh pull starts clean
    /// (the model never invents a sub-step in the middle of a scroll).
    pub fn update(&mut self, ui: &Ui, surface: Rect, at_limit: bool) -> GestureFrame {
        let mut frame = GestureFrame::default();
        let now_ms = ui.input(|i| i.time) * 1000.0;

        let (pointer_down, hovered, delta, velocity_pps) = ui.input(|i| {
            let p = &i.pointer;
            let hovered = p.hover_pos().map_or(false, |pos| surface.contains(pos));
            (
                p.primary_down(),
                hovered,
                p.delta(),
                p.velocity(),
            )
        });

        // ----- page tier: pointer drag within the surface -----
        if pointer_down && (self.dragging || hovered) {
            if !self.dragging {
                self.dragging = true;
                self.drag_total = 0.0;
                self.gesture.reset_streak();
            }
            let d = self.axis_of(delta);
            self.drag_total += d;
            self.last_velocity = self.axis_of(velocity_pps) / 1000.0; // px/ms

            // Treat a page surface as "at its horizontal limit" (no inner
            // horizontal scroll): accumulate drag toward a sub-page detent.
            // Next = finger toward -axis (content moves left/up) → feed -d.
            let o = self.gesture.on_scroll(-(d as f64), now_ms);
            apply_outcome(o, &mut frame);
        } else if self.dragging {
            // release
            self.dragging = false;
            let rel = self
                .gesture
                .on_release(self.last_velocity as f64 * -1.0, self.drag_total as f64 * -1.0);
            if let ReleaseOutcome::PageJump { dir } = rel {
                frame.page_jump = Some(dir);
            }
        }

        // ----- trackpad: two-finger scroll along the axis when hovered -----
        if hovered && !pointer_down {
            let scroll = ui.input(|i| self.axis_of(i.smooth_scroll_delta));
            if scroll.abs() > 0.01 {
                if at_limit {
                    let o = self.gesture.on_scroll(scroll as f64, now_ms);
                    apply_outcome(o, &mut frame);
                } else {
                    self.gesture.reset_streak();
                }
            }
        }

        frame.locked = self.gesture.locked(now_ms);
        frame.raw = self.gesture.raw() as f32;
        if frame.barrier_edge.is_none() {
            // keep showing residual progress between samples
            let f = (self.gesture.raw().abs() / self.gesture.config().detent_threshold) as f32;
            frame.barrier_fraction = f.clamp(0.0, 1.0);
            if frame.barrier_fraction > 0.0 {
                frame.barrier_edge =
                    Some(if self.gesture.raw() >= 0.0 { Direction::Next } else { Direction::Prev });
            }
        }
        frame
    }
}

fn apply_outcome(o: Outcome, frame: &mut GestureFrame) {
    match o {
        Outcome::None => {}
        Outcome::DetentFired { dir } => {
            frame.detent = Some(dir);
            frame.barrier_fraction = 0.0;
            frame.barrier_edge = None;
        }
        Outcome::DetentProgress { fraction, edge } => {
            frame.barrier_fraction = fraction as f32;
            frame.barrier_edge = Some(edge);
        }
    }
}
