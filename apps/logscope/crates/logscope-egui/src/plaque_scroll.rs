//! plaque_scroll.rs — impakt-cam-inspired smooth scroll for the physical UI's message-reading plaque.
//!
//! Ported from the Atom/Pulsar package `impakt-cam` (see lib/impakt-cam.js): rather than a momentum/
//! fling model (that's `scroll.rs`, for drag gestures), impakt-cam always TWEENS the camera/scroll
//! position toward a TARGET over a fixed duration with an elastic-out ease — the "smooth follow the
//! cursor" feel from code editors. That's the right model here: `jk` (or a click) sets a target LINE,
//! and the plaque's visible window glides to it instead of snapping, so scrolling through a long
//! message reads as physical motion rather than a jump-cut.
//!
//! Differences from the JS source (deliberate, matching this app's needs):
//!   - No GSAP/TweenMax: a small hand-rolled elastic-out easing (same shape as `Elastic.easeOut.config(.1)`).
//!   - impakt-cam animates PIXELS (line-height × row); this animates LINE units directly, since the
//!     plaque already renders a fixed-size window of wrapped lines — the caller maps `current()` to
//!     whichever lines are visible.
//!   - `maxLinesToScroll`/`lineLimit` (skip animating huge jumps) becomes `snap_if_far`: a jump larger
//!     than a screenful snaps instantly rather than playing a multi-second glide.

/// Elastic-out easing matching GSAP's `Elastic.easeOut.config(amplitude=0.1, period≈0.4)` shape: a couple
/// of decaying overshoots settling on 1.0. `t` in [0,1].
fn elastic_out(t: f32) -> f32 {
    if t <= 0.0 { return 0.0; }
    if t >= 1.0 { return 1.0; }
    let period = 0.4_f32;
    let s = period / 4.0; // quarter-period phase shift, the standard elastic-out derivation
    (2.0_f32.powf(-10.0 * t)) * ((t - s) * (2.0 * std::f32::consts::PI) / period).sin() + 1.0
}

#[derive(Clone, Debug)]
pub struct PlaqueScroll {
    current: f32,        // current scroll position, in LINE units (fractional mid-glide)
    from: f32,           // where THIS glide started (for the ease's t=0 anchor)
    target: f32,         // destination line
    elapsed: f32,        // seconds into the current glide
    // ── tunables (impakt-cam's config, minus the pixel-specific ones) ──
    pub duration: f32,       // impakt-cam `scrollDuration` (default 1.5s in the JS; snappier here — see default())
    pub snap_if_far: f32,    // impakt-cam `maxLinesToScroll`: jumps beyond this snap instantly, no glide
}

impl Default for PlaqueScroll {
    fn default() -> Self {
        // A message plaque is read in a couple of seconds, not scrolled like a code file — 0.45s keeps
        // jk feeling responsive while still gliding (impakt-cam's own default of 1.5s is tuned for
        // full-file cursor jumps, too slow for a per-line nudge).
        Self { current: 0.0, from: 0.0, target: 0.0, elapsed: 0.0, duration: 0.45, snap_if_far: 40.0 }
    }
}

impl PlaqueScroll {
    /// Set a new destination line (e.g. from `jk` or a click). Re-targeting mid-glide is fine — it
    /// restarts the ease from wherever the plaque currently sits, matching impakt-cam's "the animation
    /// re-fires on the newest cursor position" behavior rather than queuing glides.
    pub fn set_target(&mut self, target: f32, max_line: f32) {
        let target = target.clamp(0.0, max_line.max(0.0));
        if (target - self.target).abs() < 1e-4 { return; } // already headed there
        if (target - self.current).abs() > self.snap_if_far {
            self.current = target; // impakt-cam: huge jumps don't animate
        } else {
            self.from = self.current;
        }
        self.target = target;
        self.elapsed = 0.0;
    }

    /// Nudge the target by `delta` lines (jk: -1/+1; a page key could pass a bigger delta), clamped to
    /// the content's line range.
    pub fn nudge(&mut self, delta: f32, max_line: f32) {
        self.set_target(self.target + delta, max_line);
    }

    /// Advance the glide by `dt` seconds; returns the current scroll position (line units, fractional).
    pub fn tick(&mut self, dt: f32) -> f32 {
        if (self.target - self.current).abs() > 1e-4 {
            self.elapsed = (self.elapsed + dt.max(0.0)).min(self.duration);
            let t = if self.duration > 0.0 { self.elapsed / self.duration } else { 1.0 };
            self.current = self.from + (self.target - self.from) * elastic_out(t);
            if t >= 1.0 { self.current = self.target; }
        }
        self.current
    }

    /// Current position without advancing time (for read-only queries, e.g. rendering).
    pub fn current(&self) -> f32 { self.current }
    pub fn target(&self) -> f32 { self.target }
    pub fn at_rest(&self) -> bool { (self.target - self.current).abs() < 1e-4 }

    /// Reset to a fresh line-0 position with no in-flight glide — call when focus switches to a
    /// DIFFERENT message, so the new plaque doesn't inherit the old one's scroll or animate FROM it.
    pub fn reset(&mut self) {
        self.current = 0.0; self.from = 0.0; self.target = 0.0; self.elapsed = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glide_reaches_target_and_stops() {
        let mut s = PlaqueScroll::default();
        s.set_target(5.0, 100.0);
        for _ in 0..200 { s.tick(1.0 / 60.0); } // far more than `duration` worth of frames
        assert!((s.current() - 5.0).abs() < 1e-3, "should settle exactly on target, got {}", s.current());
        assert!(s.at_rest());
    }

    #[test]
    fn target_is_clamped_to_content_range() {
        let mut s = PlaqueScroll::default();
        s.set_target(-3.0, 10.0);
        assert_eq!(s.target(), 0.0, "can't scroll above the top");
        s.set_target(999.0, 10.0);
        assert_eq!(s.target(), 10.0, "can't scroll past the last line");
    }

    #[test]
    fn nudge_moves_by_delta_from_current_target() {
        let mut s = PlaqueScroll::default();
        s.set_target(4.0, 100.0);
        s.nudge(1.0, 100.0); // k / j calls this with ±1
        assert_eq!(s.target(), 5.0);
        s.nudge(-2.0, 100.0);
        assert_eq!(s.target(), 3.0);
    }

    #[test]
    fn huge_jump_snaps_instead_of_animating() {
        // impakt-cam's maxLinesToScroll: don't play a multi-second glide for a jump across a huge doc.
        let mut s = PlaqueScroll::default();
        s.set_target(500.0, 1000.0);
        assert_eq!(s.current(), 500.0, "far jump should snap immediately, not start a glide");
        assert!(s.at_rest());
    }

    #[test]
    fn mid_glide_retarget_restarts_from_current_position_not_from_scratch() {
        let mut s = PlaqueScroll::default();
        s.set_target(10.0, 100.0);
        s.tick(1.0 / 600.0); // one small step — early in the glide, well before any elastic overshoot
        let mid = s.current();
        assert!(mid > 0.0 && mid < 10.0, "should be partway, got {mid}");
        s.set_target(12.0, 100.0); // re-target before settling (another jk press)
        // the glide should continue smoothly from `mid`, not jump back to 0 first
        let just_after = { s.tick(1.0 / 600.0); s.current() }; // a tiny step
        assert!((just_after - mid).abs() < 1.0, "retarget should continue from near {mid}, got {just_after}");
    }

    #[test]
    fn reset_clears_target_and_position() {
        let mut s = PlaqueScroll::default();
        s.set_target(8.0, 100.0);
        s.tick(0.1);
        s.reset();
        assert_eq!(s.current(), 0.0);
        assert_eq!(s.target(), 0.0);
        assert!(s.at_rest());
    }

    #[test]
    fn elastic_out_endpoints_are_exact() {
        assert_eq!(elastic_out(0.0), 0.0);
        assert_eq!(elastic_out(1.0), 1.0);
    }
}
