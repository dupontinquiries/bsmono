//! camera — every animated quantity in the editor is one primitive.
//!
//! [`Smooth`] converges on its target by exponential decay with a **framerate-
//! independent** step: `pos += Δ · (1 − e^(−rate·dt))`. The same trajectory at
//! 30 fps and 144 fps, no springs to tune, no overshoot — deceleration only,
//! which is what a camera should do (uikit-egui's `motion::ease` curves are
//! for *timed* transitions; a camera has no duration, it has a destination
//! that keeps moving). Scroll, caret x/y, and font size all ride this one
//! struct, which is why zooming, jumping to a line, and typing all feel like
//! the same instrument.
//!
//! Pure f32 math — no egui types — so the convergence properties are pinned
//! by unit tests, not by eyeballing a build.

#[derive(Clone, Copy, Debug)]
pub struct Smooth {
    pub pos: f32,
    pub target: f32,
    /// Convergence rate in 1/seconds. Higher = snappier. ~12 for scroll,
    /// ~30 for the caret, ~10 for zoom read well on both desktop and phone.
    pub rate: f32,
}

impl Smooth {
    pub fn new(v: f32, rate: f32) -> Self {
        Smooth { pos: v, target: v, rate }
    }

    /// Jump without animating (file load, teleporting the caret on click).
    pub fn snap(&mut self, v: f32) {
        self.pos = v;
        self.target = v;
    }

    /// Advance by `dt` seconds. Returns true while still visibly moving —
    /// the shell's cue to request another repaint.
    pub fn step(&mut self, dt: f32) -> bool {
        let d = self.target - self.pos;
        if d.abs() < 0.1 {
            self.pos = self.target;
            return false;
        }
        // 1 - e^(-rate·dt): the fraction of remaining distance covered this
        // frame. Clamp dt so a hitch (or a debugger pause) can't explode it.
        let k = 1.0 - (-self.rate * dt.clamp(0.0, 0.25)).exp();
        self.pos += d * k;
        true
    }

    pub fn settled(&self) -> bool {
        (self.target - self.pos).abs() < 0.1
    }
}

/// The editor's camera: vertical scroll in px, caret in px, font size in px.
pub struct Cam {
    pub scroll_y: Smooth,
    pub caret_x: Smooth,
    pub caret_y: Smooth,
    pub font_px: Smooth,
}

impl Cam {
    pub fn new(font_px: f32) -> Self {
        Cam {
            scroll_y: Smooth::new(0.0, 12.0),
            caret_x: Smooth::new(0.0, 30.0),
            caret_y: Smooth::new(0.0, 30.0),
            font_px: Smooth::new(font_px, 10.0),
        }
    }

    pub fn step(&mut self, dt: f32) -> bool {
        // `|` not `||`: every channel must advance every frame.
        self.scroll_y.step(dt) | self.caret_x.step(dt) | self.caret_y.step(dt) | self.font_px.step(dt)
    }

    /// Keep `[y0, y1]` (content px) inside the viewport of height `view_h`,
    /// with `margin` px of breathing room — the follow-caret rule.
    pub fn ensure_visible(&mut self, y0: f32, y1: f32, view_h: f32, margin: f32) {
        let top = self.scroll_y.target;
        let bottom = top + view_h;
        if y0 < top + margin {
            self.scroll_y.target = (y0 - margin).max(0.0);
        } else if y1 > bottom - margin {
            self.scroll_y.target = y1 + margin - view_h;
        }
    }

    pub fn clamp_scroll(&mut self, max: f32) {
        self.scroll_y.target = self.scroll_y.target.clamp(0.0, max.max(0.0));
        self.scroll_y.pos = self.scroll_y.pos.clamp(0.0, max.max(0.0));
    }
}

/// impakt-cam, distilled: keep the caret CENTERED, not merely visible — but
/// only move when it's worth moving. Three rules, ported knob-for-knob from
/// the Atom plugin that named this feature, minus its elastic ease (our
/// Smooth is deliberately overshoot-free — a viewport that springs past the
/// line re-reads as jank; that property is unit-tested and stays):
///  • a DEAD ZONE around the current top (impakt-cam `lineCenterThresh`):
///    small drifts — typing along a line, nudging a row — don't move the
///    world. Measured in TOP space, not caret space, so the file's edges
///    (where centering clamps) go quiet for free instead of scrolling
///    forever against the clamp.
///  • GLIDE inside the horizon: the Smooth takes the new target; its
///    rate×error physics already covers longer jumps at higher px/s, which
///    is impakt-cam's `rowModifier` for free.
///  • SNAP beyond it (`maxLinesToScroll`): a 5000-line jump animated is a
///    sickening blur AND a frame-cost cliff; past the horizon, teleport.
#[derive(Clone, Copy, Debug)]
pub struct Follow {
    /// px of top-drift tolerated before the camera bothers moving.
    pub dead_zone: f32,
    /// px beyond which a jump teleports instead of gliding.
    pub snap_beyond: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FollowMove {
    Keep,
    Glide(f32),
    Snap(f32),
}

impl Follow {
    /// Line-denominated construction (the natural units for the knobs).
    pub fn lines(dead: f32, snap: f32, line_h: f32) -> Follow {
        Follow { dead_zone: dead * line_h, snap_beyond: snap * line_h }
    }

    /// Where should the viewport top go so `caret_y` (content px) is
    /// vertically centered — and is the move worth making?
    pub fn target(
        &self,
        caret_y: f32,
        line_h: f32,
        view_h: f32,
        max_scroll: f32,
        cur_target: f32,
    ) -> FollowMove {
        let desired = (caret_y - (view_h - line_h) * 0.5).clamp(0.0, max_scroll.max(0.0));
        let d = (desired - cur_target).abs();
        if d <= self.dead_zone {
            FollowMove::Keep
        } else if d > self.snap_beyond {
            FollowMove::Snap(desired)
        } else {
            FollowMove::Glide(desired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_without_overshoot_and_settles() {
        let mut s = Smooth::new(0.0, 12.0);
        s.target = 100.0;
        let mut last = 0.0f32;
        for _ in 0..240 {
            s.step(1.0 / 60.0);
            assert!(s.pos >= last - 1e-3, "monotone toward target");
            assert!(s.pos <= 100.0 + 1e-3, "never overshoots");
            last = s.pos;
        }
        assert!(s.settled());
        assert_eq!(s.pos, 100.0);
    }

    #[test]
    fn framerate_independent_within_tolerance() {
        let mut a = Smooth::new(0.0, 12.0);
        let mut b = Smooth::new(0.0, 12.0);
        a.target = 100.0;
        b.target = 100.0;
        for _ in 0..30 {
            a.step(1.0 / 30.0);
        }
        for _ in 0..144 {
            b.step(1.0 / 144.0);
        }
        assert!((a.pos - b.pos).abs() < 1.5, "30fps={} 144fps={}", a.pos, b.pos);
    }

    #[test]
    fn hitch_does_not_explode() {
        let mut s = Smooth::new(0.0, 12.0);
        s.target = 100.0;
        s.step(5.0); // a 5-second stall
        assert!(s.pos <= 100.0, "clamped dt keeps the step sane");
    }

    #[test]
    fn ensure_visible_scrolls_both_directions() {
        let mut c = Cam::new(14.0);
        c.ensure_visible(1000.0, 1016.0, 400.0, 40.0);
        assert!((c.scroll_y.target - (1016.0 + 40.0 - 400.0)).abs() < 0.01);
        c.ensure_visible(100.0, 116.0, 400.0, 40.0);
        assert!((c.scroll_y.target - 60.0).abs() < 0.01);
    }
    #[test]
    fn follow_dead_zone_glide_and_snap() {
        let line = 20.0;
        let f = Follow::lines(2.0, 120.0, line);
        let view = 800.0;
        let max = 100_000.0;
        // caret at line 100: an 80-line hop — inside the 120-line horizon → glide
        let caret_y = 100.0 * line;
        let desired = caret_y - (view - line) * 0.5;
        assert_eq!(f.target(caret_y, line, view, max, 0.0), FollowMove::Glide(desired));
        // caret at line 300: past the horizon → teleport
        let far_y = 300.0 * line;
        let far_desired = far_y - (view - line) * 0.5;
        assert_eq!(f.target(far_y, line, view, max, 0.0), FollowMove::Snap(far_desired), "beyond the horizon teleports");
        // one line of drift from a centered target: inside the dead zone
        let m = f.target(caret_y + line, line, view, max, desired);
        assert_eq!(m, FollowMove::Keep, "typing nearby doesn't move the world: {m:?}");
        // five lines out: glides to the new center
        match f.target(caret_y + 5.0 * line, line, view, max, desired) {
            FollowMove::Glide(t) => assert!((t - (desired + 5.0 * line)).abs() < 1e-3),
            other => panic!("expected glide, got {other:?}"),
        }
    }

    #[test]
    fn follow_goes_quiet_at_file_edges() {
        let line = 20.0;
        let f = Follow::lines(2.0, 120.0, line);
        // caret on line 1 of the file: centering is impossible; desired clamps
        // to 0, and once the target is 0 the dead zone holds — no perpetual
        // scroll against the clamp.
        assert_eq!(f.target(1.0 * line, line, 800.0, 5000.0, 0.0), FollowMove::Keep);
        // bottom clamp likewise
        let max = 300.0;
        match f.target(10_000.0, line, 800.0, max, 0.0) {
            FollowMove::Glide(t) | FollowMove::Snap(t) => assert_eq!(t, max),
            m => panic!("{m:?}"),
        }
        assert_eq!(f.target(10_000.0, line, 800.0, max, max), FollowMove::Keep);
    }

}
