//! iOS-style interaction physics — the *decisions*, kept headless so they are
//! testable without a display. The shell reads offsets/commit flags from here
//! and only paints; none of egui's types appear in this module.
//!
//! Three signature iOS behaviours live here, each deliberately NOT the Android
//! equivalent:
//!
//! * **Rubber-banding** ([`rubber_band`]) — the elastic resistance iOS applies
//!   past a boundary (UIScrollView's `1 - 1/(x·c/d + 1)`), where Android
//!   stretches/glows instead. Used for overscroll and for a pop dragged the
//!   "wrong" way.
//! * **Interactive edge-pop** ([`EdgePop`]) — drag from the screen's left edge
//!   and the top view tracks the finger 1:1; release commits if past a
//!   distance threshold *or* flicked hard enough (UIKit's interactive
//!   `popViewController`). Android's back is a button / predictive-back arc.
//! * **Swipe-to-peek** ([`peek_fraction`]) — the iMessage "drag the whole
//!   conversation left to reveal timestamps" gesture, which has no Android
//!   analogue.

/// The classic UIScrollView rubber-band curve. `offset` is how far past the
/// boundary the finger has travelled (same sign as travel); `dim` is the
/// relevant dimension (view width/height). Returns the *displayed* offset,
/// always smaller in magnitude than `offset` and asymptotic to `dim`.
///
/// c = 0.55 is Apple's constant. At `offset == dim` the view has moved ~0.35·dim.
pub fn rubber_band(offset: f32, dim: f32) -> f32 {
    const C: f32 = 0.55;
    if dim <= 0.0 {
        return 0.0;
    }
    let sign = if offset < 0.0 { -1.0 } else { 1.0 };
    let x = offset.abs();
    sign * (1.0 - 1.0 / (x * C / dim + 1.0)) * dim
}

/// Tuning for the interactive pop. Defaults match the iOS feel; a settings
/// screen can widen the edge zone or lower the flick threshold.
#[derive(Debug, Clone, Copy)]
pub struct PopConfig {
    /// How wide, in px from the left edge, a drag may *start* to arm the pop.
    pub edge_zone: f32,
    /// Fraction of width past which a slow release still commits.
    pub commit_fraction: f32,
    /// px/ms rightward velocity that commits regardless of distance (a flick).
    pub flick_velocity: f32,
    /// Below this travel, even a flick is treated as a tap, not a pop.
    pub min_travel: f32,
}

impl Default for PopConfig {
    fn default() -> Self {
        // 0.5 px/ms ≈ 500 px/s, a deliberate flick; 20 px edge like UIKit.
        PopConfig { edge_zone: 20.0, commit_fraction: 0.5, flick_velocity: 0.5, min_travel: 16.0 }
    }
}

/// What a completed drag decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopResult {
    /// Go back / dismiss.
    Commit,
    /// Snap back; nothing happened.
    Cancel,
}

/// One in-progress interactive pop. Pure: the shell feeds it drag deltas and
/// paints [`EdgePop::offset`]; on release it asks [`EdgePop::finish`].
#[derive(Debug, Clone)]
pub struct EdgePop {
    cfg: PopConfig,
    width: f32,
    active: bool,
    /// Rightward travel from the start point (>= 0 while dragging right).
    travel: f32,
}

impl EdgePop {
    pub fn new(width: f32, cfg: PopConfig) -> EdgePop {
        EdgePop { cfg, width: width.max(1.0), active: false, travel: 0.0 }
    }

    /// A press began at `start_x` (px from the surface's left). Arms the pop
    /// only if it started inside the edge zone — a press in the middle of the
    /// screen scrolls the content, it does not pop.
    pub fn begin(&mut self, start_x: f32) -> bool {
        self.active = start_x <= self.cfg.edge_zone;
        self.travel = 0.0;
        self.active
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Feed a horizontal delta (px this frame; right is positive). Returns the
    /// current displayed offset. Dragging left of the origin rubber-bands
    /// (you cannot pop "into" the screen).
    pub fn drag(&mut self, dx: f32) -> f32 {
        if !self.active {
            return 0.0;
        }
        self.travel += dx;
        self.offset()
    }

    /// Displayed x-offset of the leaving view: 1:1 while dragging right, but
    /// rubber-banded once it would go left of home.
    pub fn offset(&self) -> f32 {
        if self.travel >= 0.0 {
            self.travel.min(self.width)
        } else {
            rubber_band(self.travel, self.width)
        }
    }

    /// Live progress 0..1 for cross-fading the view behind.
    pub fn progress(&self) -> f32 {
        (self.offset() / self.width).clamp(0.0, 1.0)
    }

    /// Release. `velocity_x` is px/ms (right positive), from the toolkit.
    pub fn finish(&mut self, velocity_x: f32) -> PopResult {
        let committed = self.travel >= self.cfg.min_travel
            && (self.travel >= self.width * self.cfg.commit_fraction
                || velocity_x >= self.cfg.flick_velocity);
        self.active = false;
        let t = self.travel;
        self.travel = 0.0;
        if committed && t > 0.0 {
            PopResult::Commit
        } else {
            PopResult::Cancel
        }
    }
}

/// The iMessage timestamp peek: drag the conversation left by `dx_left` px
/// (positive = leftward), revealed up to `max` px with rubber-band past it.
/// Returns how far the row content should shift left.
pub fn peek_fraction(dx_left: f32, max: f32) -> f32 {
    if dx_left <= 0.0 {
        return 0.0;
    }
    if dx_left <= max {
        dx_left
    } else {
        max + rubber_band(dx_left - max, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubber_band_resists_and_is_bounded() {
        // No travel → no offset; symmetric; always less than the raw offset.
        assert_eq!(rubber_band(0.0, 300.0), 0.0);
        assert_eq!(rubber_band(-100.0, 300.0), -rubber_band(100.0, 300.0));
        let disp = rubber_band(300.0, 300.0);
        assert!(disp < 300.0 && disp > 0.0);
        // Pulling twice as far moves the view *less than* twice as far.
        assert!(rubber_band(600.0, 300.0) < 2.0 * rubber_band(300.0, 300.0));
    }

    #[test]
    fn pop_only_arms_from_the_edge() {
        let mut p = EdgePop::new(360.0, PopConfig::default());
        assert!(!p.begin(200.0), "a mid-screen press must not arm a pop");
        assert!(p.begin(8.0), "an edge press arms it");
    }

    #[test]
    fn slow_drag_past_half_commits_short_drag_cancels() {
        let cfg = PopConfig::default();
        let mut p = EdgePop::new(300.0, cfg);
        p.begin(5.0);
        p.drag(160.0); // >150 = half width
        assert_eq!(p.finish(0.0), PopResult::Commit);

        let mut p = EdgePop::new(300.0, cfg);
        p.begin(5.0);
        p.drag(40.0); // short, slow
        assert_eq!(p.finish(0.0), PopResult::Cancel);
    }

    #[test]
    fn a_flick_commits_even_when_short_but_not_below_min_travel() {
        let cfg = PopConfig::default();
        let mut p = EdgePop::new(300.0, cfg);
        p.begin(5.0);
        p.drag(30.0); // short but past min_travel(16)
        assert_eq!(p.finish(0.8), PopResult::Commit, "a fast flick commits");

        let mut p = EdgePop::new(300.0, cfg);
        p.begin(5.0);
        p.drag(5.0); // below min_travel — this was a tap, not a pop
        assert_eq!(p.finish(2.0), PopResult::Cancel);
    }

    #[test]
    fn offset_tracks_one_to_one_right_and_rubber_bands_left() {
        let mut p = EdgePop::new(300.0, PopConfig::default());
        p.begin(5.0);
        assert_eq!(p.drag(100.0), 100.0); // 1:1 to the right
        assert_eq!(p.drag(-250.0), rubber_band(-150.0, 300.0)); // now left of home
        assert!(p.offset() > -150.0, "rubber-band resists going left");
    }

    #[test]
    fn peek_reveals_then_resists() {
        assert_eq!(peek_fraction(-10.0, 80.0), 0.0); // wrong direction
        assert_eq!(peek_fraction(50.0, 80.0), 50.0); // within range, 1:1
        let past = peek_fraction(200.0, 80.0);
        assert!(past > 80.0 && past < 200.0, "past max it rubber-bands");
    }
}
