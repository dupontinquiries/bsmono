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

/// Typewriter camera: the target that puts `caret_y` at the vertical
/// center of a `view_h` viewport, clamped to scrollable range. Pure, so
/// the centering rule is pinned headless like the rest of this module.
pub fn typewriter_target(caret_y: f32, line_h: f32, view_h: f32, max_scroll: f32) -> f32 {
    (caret_y - (view_h - line_h) * 0.5).clamp(0.0, max_scroll.max(0.0))
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

// ── the caret's own presentation ────────────────────────────────────────────
//
// This used to be one line in `view.rs`: `(now_ms / 530) % 2 == 0`, with an
// unconditional `request_repaint_after(530ms)` under it. Three things were
// wrong with that, and only the third is about looks:
//
// 1. **It never stopped.** The repaint request re-armed itself every frame,
//    so an idle window redrew the whole editor ~1.9 times a second forever —
//    on battery, and with no check on whether the window was even focused. A
//    background aperture blinked a caret nobody could see.
// 2. **The phase was global, not yours.** `now_ms / 530` is anchored to app
//    start, so a keystroke could land in the dark half and the caret you just
//    moved would be invisible for up to 265 ms.
// 3. **A square wave is a flicker.** Which is a taste question, so it is an
//    option rather than a replacement.
//
// The policy below answers all three by returning the alpha AND when to wake
// up next — `None` meaning "nothing here needs another frame", which is the
// part that makes an idle editor genuinely idle.

/// How the caret presents itself at rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaretStyle {
    /// Never animates. Zero wakeups, always legible, and the right answer
    /// for a slow display or anyone who finds movement distracting — an
    /// accessibility setting as much as a taste one.
    Solid,
    /// The classic square wave.
    Blink,
    /// A smooth brightness swell — a raised cosine, no discontinuity at
    /// either end, so it reads as breathing rather than switching.
    #[default]
    Breathe,
}

/// Alpha to draw at, and when the caret next needs a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretDraw {
    pub alpha: f32,
    /// `None` = this caret is in a steady state; do not schedule anything.
    pub repaint_in_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretPolicy {
    pub style: CaretStyle,
    /// Quiet time after which the caret stops animating and holds solid.
    ///
    /// The whole reason animation is affordable: it runs while someone is
    /// demonstrably present and costs nothing once they are not. A caret
    /// that blinks at an empty room for eight hours is the thing being
    /// fixed here, and settling to SOLID (not to invisible) means walking
    /// back to the machine still shows you where you were.
    pub settle_ms: u64,
    /// Half-period of [`CaretStyle::Blink`] — on for this long, off for this
    /// long. 530 ms is what the editor already used.
    pub blink_ms: u64,
    /// Full period of one breath. Slower than a blink on purpose: the point
    /// of a swell is that the eye reads it as continuous.
    pub breath_ms: u64,
    /// Dimmest point of the breath. Not near zero — a swell that reaches
    /// invisible is a blink with extra steps.
    pub dim: f32,
    /// Alpha in a window that does not have focus. Visible enough to find,
    /// clearly not the active caret, and — the point — static.
    pub unfocused: f32,
    /// Frame interval while breathing. 30 fps, not 60: this is a slow
    /// brightness ramp, and the difference is invisible while the wakeup
    /// count halves. On a phone that is the whole argument.
    pub breath_frame_ms: u64,
}

impl CaretStyle {
    /// Every variant, so the palette generates its entries instead of
    /// listing them — the convention `Scope::ALL` already set here.
    pub const ALL: &'static [CaretStyle] = &[CaretStyle::Breathe, CaretStyle::Blink, CaretStyle::Solid];

    pub fn label(self) -> &'static str {
        match self {
            CaretStyle::Solid => "solid",
            CaretStyle::Blink => "blink",
            CaretStyle::Breathe => "breathe",
        }
    }

    /// What choosing it costs, said plainly, because on a phone it is the
    /// deciding factor and nowhere else in the UI would say so.
    pub fn hint(self) -> &'static str {
        match self {
            CaretStyle::Solid => "never animates — no wakeups at all",
            CaretStyle::Blink => "classic square wave, one frame per edge",
            CaretStyle::Breathe => "smooth swell; animates only while you are active",
        }
    }
}

impl Default for CaretPolicy {
    fn default() -> Self {
        CaretPolicy {
            style: CaretStyle::Breathe,
            settle_ms: 8_000,
            blink_ms: 530,
            breath_ms: 1_600,
            dim: 0.2,
            unfocused: 0.35,
            breath_frame_ms: 33,
        }
    }
}

impl CaretPolicy {
    /// `last_input_ms` is when the person last did anything — a keystroke, a
    /// click, a scroll. The animation phase is anchored to it rather than to
    /// the clock, so the caret is at FULL brightness the instant you type and
    /// every keystroke restarts the cycle. That is what makes a blink stop
    /// hiding a caret you just moved.
    ///
    /// `moving` is the follow camera still gliding the caret to its new home;
    /// a caret in motion is always solid and never asks for a frame, because
    /// the camera is already asking for one.
    pub fn draw(&self, now_ms: u64, last_input_ms: u64, focused: bool, moving: bool) -> CaretDraw {
        let steady = |alpha: f32| CaretDraw { alpha, repaint_in_ms: None };
        if moving {
            return steady(1.0);
        }
        if !focused {
            return steady(self.unfocused);
        }
        if self.style == CaretStyle::Solid {
            return steady(1.0);
        }
        let elapsed = now_ms.saturating_sub(last_input_ms);
        let Some(until_settle) = self.settle_ms.checked_sub(elapsed).filter(|r| *r > 0) else {
            return steady(1.0);
        };
        match self.style {
            CaretStyle::Solid => steady(1.0),
            CaretStyle::Blink => {
                let half = self.blink_ms.max(1);
                let phase = elapsed % (half * 2);
                let on = phase < half;
                // Wake at the exact next EDGE rather than a flat 530 ms.
                // A fixed interval drifts against the phase it is trying to
                // track, so the visible on/off boundary lands wherever the
                // last unrelated frame happened to leave it.
                let to_edge = half - (phase % half);
                CaretDraw {
                    alpha: if on { 1.0 } else { 0.0 },
                    repaint_in_ms: Some(to_edge.min(until_settle)),
                }
            }
            CaretStyle::Breathe => {
                let period = self.breath_ms.max(1);
                let u = (elapsed % period) as f32 / period as f32;
                // Starts at 1.0, dips to `dim`, returns — cos, not a
                // triangle: the derivative is zero at both ends, which is
                // the difference between breathing and pulsing.
                let swell = 0.5 + 0.5 * (u * std::f32::consts::TAU).cos();
                CaretDraw {
                    alpha: self.dim + (1.0 - self.dim) * swell,
                    repaint_in_ms: Some(self.breath_frame_ms.max(1).min(until_settle)),
                }
            }
        }
    }
}

#[cfg(test)]
mod caret_tests {
    use super::*;

    /// The bug this whole module section exists for: the old code asked for
    /// a frame every 530 ms forever, whatever the window was doing.
    #[test]
    fn nothing_that_nobody_is_watching_asks_for_a_frame() {
        let p = CaretPolicy::default();
        for style in [CaretStyle::Solid, CaretStyle::Blink, CaretStyle::Breathe] {
            let p = CaretPolicy { style, ..p };
            assert_eq!(
                p.draw(1_000, 900, false, false).repaint_in_ms,
                None,
                "{style:?}: an unfocused window must go completely quiet"
            );
            assert_eq!(
                p.draw(1_000_000, 0, true, false).repaint_in_ms,
                None,
                "{style:?}: past the settle time, nothing more is needed"
            );
            assert_eq!(
                p.draw(1_000, 900, true, true).repaint_in_ms,
                None,
                "{style:?}: while gliding, the camera already owns the frame budget"
            );
        }
        assert_eq!(p.draw(1_000, 900, true, false).repaint_in_ms, Some(33), "…but a live one does");
    }

    #[test]
    fn a_settled_caret_is_solid_and_visible_not_dark() {
        let p = CaretPolicy::default();
        for style in [CaretStyle::Blink, CaretStyle::Breathe] {
            let d = CaretPolicy { style, ..p }.draw(500_000, 0, true, false);
            assert_eq!(d.alpha, 1.0, "{style:?}: walking back to the machine shows where you were");
        }
        // Unfocused is dim but never invisible.
        let d = p.draw(500_000, 0, false, false);
        assert!(d.alpha > 0.15 && d.alpha < 1.0, "{}", d.alpha);
    }

    /// The old `now_ms / 530` was anchored to app start, so a keystroke could
    /// land in the dark half and hide the caret you just moved.
    #[test]
    fn the_phase_is_anchored_to_the_keystroke_not_the_clock() {
        for style in [CaretStyle::Blink, CaretStyle::Breathe] {
            let p = CaretPolicy { style, ..CaretPolicy::default() };
            for odd_clock in [0u64, 1, 265, 531, 799, 12_345, 999_999] {
                let d = p.draw(odd_clock, odd_clock, true, false);
                assert_eq!(d.alpha, 1.0, "{style:?} at {odd_clock}: fully lit the moment you type");
            }
        }
    }

    #[test]
    fn blink_alternates_and_wakes_exactly_on_the_edge() {
        let p = CaretPolicy { style: CaretStyle::Blink, ..CaretPolicy::default() };
        let at = |ms: u64| p.draw(ms, 0, true, false);
        assert_eq!(at(0).alpha, 1.0);
        assert_eq!(at(529).alpha, 1.0);
        assert_eq!(at(530).alpha, 0.0);
        assert_eq!(at(1059).alpha, 0.0);
        assert_eq!(at(1060).alpha, 1.0, "and back");

        // The wake lands ON the transition, not a fixed 530 later.
        assert_eq!(at(0).repaint_in_ms, Some(530));
        assert_eq!(at(500).repaint_in_ms, Some(30));
        assert_eq!(at(529).repaint_in_ms, Some(1));
        // Stepping by the requested amount always arrives exactly at a flip.
        let mut t = 0u64;
        let mut seen = 0;
        for _ in 0..6 {
            let d = at(t);
            let dt = d.repaint_in_ms.expect("still animating");
            t += dt;
            assert_ne!(at(t).alpha, d.alpha, "the scheduled frame is the one that changes something");
            seen += 1;
        }
        assert_eq!(seen, 6);
    }

    #[test]
    fn breathe_is_continuous_bounded_and_actually_swells() {
        let p = CaretPolicy { style: CaretStyle::Breathe, ..CaretPolicy::default() };
        let a = |ms: u64| p.draw(ms, 0, true, false).alpha;

        let mut prev = a(0);
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for ms in 0..(p.breath_ms * 2) {
            let v = a(ms);
            assert!(v >= p.dim - 1e-4 && v <= 1.0 + 1e-4, "at {ms}: {v}");
            // Continuity: no step bigger than the ramp could make in 1 ms.
            // This is the assertion that fails for a square wave, which is
            // the whole point of the style.
            assert!((v - prev).abs() < 0.01, "discontinuity at {ms}: {prev} -> {v}");
            min = min.min(v);
            max = max.max(v);
            prev = v;
        }
        assert!((max - 1.0).abs() < 1e-3, "reaches full: {max}");
        assert!((min - p.dim).abs() < 1e-3, "reaches the dim floor and no further: {min}");
        // And it is a swell, not a sawtooth: the first half only descends.
        let half = p.breath_ms / 2;
        for ms in 1..half {
            assert!(a(ms) <= a(ms - 1) + 1e-6, "not monotone down at {ms}");
        }

        // THE PROPERTY THAT MAKES IT BREATHE, asserted on the mechanism
        // rather than around it. Everything above is equally true of a
        // TRIANGLE wave — verified, by replacing the cosine with one and
        // watching this test still pass. What a triangle does not have is a
        // zero derivative at the turning points, and that corner is exactly
        // what the eye reads as a pulse instead of a breath.
        let slope = |ms: u64| (a(ms + 1) - a(ms)).abs();
        let fastest = slope(p.breath_ms / 4); // steepest point of a cosine
        assert!(fastest > 0.0, "it moves at all");
        for turning in [0u64, half] {
            assert!(
                slope(turning) < fastest * 0.25,
                "corner at {turning}: slope {} vs {fastest} mid-ramp — that is a triangle, not a swell",
                slope(turning)
            );
        }
    }

    #[test]
    fn the_last_scheduled_frame_before_settling_does_not_overshoot_it() {
        // A wake past the settle point would draw the animated alpha once
        // more and only THEN go solid — a visible twitch as you walk away.
        let p = CaretPolicy { settle_ms: 1_000, ..CaretPolicy::default() };
        for style in [CaretStyle::Blink, CaretStyle::Breathe] {
            let p = CaretPolicy { style, ..p };
            let d = p.draw(990, 0, true, false);
            assert_eq!(d.repaint_in_ms, Some(10), "{style:?}: wake exactly at the settle boundary");
            assert_eq!(p.draw(1_000, 0, true, false), CaretDraw { alpha: 1.0, repaint_in_ms: None });
        }
    }

    #[test]
    fn solid_never_animates_at_all() {
        let p = CaretPolicy { style: CaretStyle::Solid, ..CaretPolicy::default() };
        for ms in [0u64, 1, 530, 1_600, 99_999] {
            assert_eq!(p.draw(ms, 0, true, false), CaretDraw { alpha: 1.0, repaint_in_ms: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typewriter_target_centers_and_clamps_at_the_edges() {
        // Mid-file: the caret line sits at the viewport's vertical center.
        let t = typewriter_target(1000.0, 20.0, 600.0, 5000.0);
        assert_eq!(t, 1000.0 - (600.0 - 20.0) * 0.5);
        // Top of file: no negative scroll — the first lines rest naturally.
        assert_eq!(typewriter_target(30.0, 20.0, 600.0, 5000.0), 0.0);
        // Bottom: clamped to max — the last page doesn't overscroll.
        assert_eq!(typewriter_target(4990.0, 20.0, 600.0, 4700.0), 4700.0);
    }

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
