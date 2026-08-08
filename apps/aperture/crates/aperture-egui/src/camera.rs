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
}
