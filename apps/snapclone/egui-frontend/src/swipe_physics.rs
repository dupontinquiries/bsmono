//! swipe_physics — `SwipeNavigator`'s decision layer.
//!
//! The Qt navigator is 3,647 lines: hit testing, child-widget bookkeeping,
//! scroll-area interception, detents, page pixmaps, animation plumbing. This
//! ports the part that decides what a gesture MEANT — the physics — because
//! that is the part with rules in it, and the rest is Qt's widget tree rather
//! than snapclone's behaviour.
//!
//! ## Width independence is the organising idea
//!
//! Every threshold is a FRACTION of the page width, and the page width is
//! captured when the gesture begins and never updated mid-gesture. Two
//! consequences, and both are the point:
//!
//! * The feel is identical on a 390px phone and a 1280px desktop. A 60px
//!   absolute threshold is a decisive flick on a phone and a twitch on a
//!   monitor.
//! * A window resized mid-drag does not change what the drag means. Recomputing
//!   from a live width would let a rotation partway through a swipe turn a
//!   committed gesture into a cancelled one.
//!
//! ## Committing is adaptive, and it has to be
//!
//! Two ways to turn a page, because there are two ways people swipe:
//!
//! * **Distance** — drag past 22% of the page. The deliberate, slow drag where
//!   you are watching the page follow your finger.
//! * **Velocity** — over 0.30 px/ms with at least 14px of travel. The thumb
//!   flick, which is short and fast and would never reach 22% of anything.
//!
//! Distance alone makes flicking feel broken; velocity alone makes a careful
//! drag impossible. The 14px floor on the flick path is what stops a fast
//! twitch — a tap with a wobble in it — from turning the page.
//!
//! ## Rubber band, and why it is a square root
//!
//! At the first or last page there is nowhere to go, so the page still moves
//! but resists: `sqrt(|drag| * pageW * k)`. Sub-linear, so it never runs away;
//! scaled by page width, so the resistance feels the same at any size. A hard
//! stop reads as a frozen app; linear movement reads as a page that will turn
//! and then does not.

use uikit_gesture::{GestureConfig, ReleaseOutcome, TwoTierGesture};

/// The thresholds are the KIT's, not ours.
///
/// This module originally carried its own `COMMIT_FRACTION = 0.22`,
/// `FLICK_PX_PER_MS = 0.30` and `FLICK_MIN_PX = 14.0` — which are, character for
/// character, the values already sitting in `uikit_gesture::GestureConfig`,
/// whose comment says it mirrors snapclone's own `Theme.hpp`. The port
/// re-derived from the Qt source what the kit had already been given. Reading
/// them from the kit means a retune lands in one place.
pub fn config(page_w: f32) -> GestureConfig { GestureConfig::for_page_size(page_w as f64) }

/// Re-exported so callers do not have to reach into two crates.
pub use uikit_gesture::{rubber_band as kit_rubber_band, VelocityTracker as KitVelocity,
                        RUBBER_BAND_K};

/// Rubber-band resistance at an edge, in pixels.
pub fn rubber_band(drag: f32, page_w: f32) -> f32 {
    kit_rubber_band(drag as f64, page_w as f64, RUBBER_BAND_K) as f32
}

/// Where a gesture can go, given the page it started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds { pub can_prev: bool, pub can_next: bool }

/// What the release decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commit {
    /// Turn to the previous page.
    Prev,
    /// Turn to the next page.
    Next,
    /// Spring back to where it started.
    Cancel,
}


/// A gesture in progress. The page width is captured here and never re-read.
#[derive(Debug, Clone)]
pub struct Swipe {
    page_w: f32,
    bounds: Bounds,
    drag: f32,
    tracker: KitVelocity,
    start_x: f32,
    active: bool,
}

impl Swipe {
    /// `gestureBegin`: capture the width ONCE.
    pub fn begin(page_w: f32, bounds: Bounds, x: f32, t_ms: f32) -> Self {
        let mut tracker = KitVelocity::default();
        tracker.push(x as f64, t_ms as f64);
        Swipe { page_w, bounds, drag: 0.0, tracker, start_x: x, active: true }
    }

    pub fn update(&mut self, x: f32, t_ms: f32) {
        if !self.active { return; }
        self.drag = x - self.start_x;
        self.tracker.push(x as f64, t_ms as f64);
    }

    pub fn drag(&self) -> f32 { self.drag }
    pub fn page_width(&self) -> f32 { self.page_w }

    /// Is this drag pushing against an edge with nowhere to go?
    ///
    /// A positive drag moves the content right, which reveals the PREVIOUS
    /// page — so it is blocked when there is no previous.
    pub fn at_edge(&self) -> bool {
        (self.drag > 0.0 && !self.bounds.can_prev) || (self.drag < 0.0 && !self.bounds.can_next)
    }

    /// How far the content should actually be offset, rubber-banded at edges.
    pub fn offset(&self) -> f32 {
        if self.at_edge() { rubber_band(self.drag, self.page_w) } else { self.drag }
    }

    /// The distance threshold in pixels for this gesture, from the kit's config.
    pub fn commit_distance(&self) -> f32 {
        let c = config(self.page_w);
        (c.page_size * c.commit_fraction) as f32
    }

    /// `gestureEnd`: decide, via the kit.
    ///
    /// The commit rule — distance past 22% OR a flick over 0.30 px/ms with at
    /// least 14px travelled — lives in `TwoTierGesture::on_release`. What stays
    /// here is the EDGE rule, which the kit has no opinion about: a commit
    /// toward a page that does not exist springs back, and the rubber band has
    /// already said so on the way.
    pub fn release(&mut self, t_ms: f32) -> Commit {
        self.active = false;
        let v = self.tracker.velocity(t_ms as f64);
        let d = self.drag as f64;
        let mut g = TwoTierGesture::new(config(self.page_w));
        let want = match g.on_release(v, d) {
            ReleaseOutcome::Stay => return Commit::Cancel,
            // Direction from the DRAG — which is now the kit's rule too, after
            // this port's version of it was folded in.
            ReleaseOutcome::PageJump { .. } =>
                if d > 0.0 { Commit::Prev } else { Commit::Next },
        };
        match want {
            Commit::Prev if self.bounds.can_prev => Commit::Prev,
            Commit::Next if self.bounds.can_next => Commit::Next,
            _ => Commit::Cancel,
        }
    }

    /// How long the settle animation should run, given the release velocity.
    ///
    /// A fast flick commits quickly and a slow drag settles gently: matching
    /// the animation to the gesture is what makes the page feel like it is
    /// continuing your movement rather than playing a canned transition.
    pub fn settle_ms(&self, t_ms: f32) -> f32 {
        let v = self.tracker.velocity(t_ms as f64).abs() as f32;
        // Fast (1 px/ms) → 120ms; still → 320ms.
        (320.0 - v * 200.0).clamp(120.0, 320.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: Bounds = Bounds { can_prev: true, can_next: true };
    const FIRST: Bounds = Bounds { can_prev: false, can_next: true };
    const LAST: Bounds = Bounds { can_prev: true, can_next: false };

    /// A slow deliberate drag at a FIXED velocity well under the flick
    /// threshold, so only the distance rule can decide it.
    ///
    /// The duration scales with the distance on purpose. An earlier version of
    /// this fixture held the duration fixed, which meant a longer drag was also
    /// a FASTER one — so the "too short to commit" case on a wide screen was
    /// travelling at 0.32 px/ms and committed as a flick, correctly. The bug
    /// was in the fixture, and a fixture that changes two variables at once
    /// cannot test either.
    fn slow_drag(page_w: f32, bounds: Bounds, total: f32) -> Swipe {
        const V: f32 = 0.1;                    // px/ms, a third of the flick bar
        let dur = total.abs() / V;
        let mut s = Swipe::begin(page_w, bounds, 0.0, 0.0);
        for i in 1..=20 {
            s.update(total * i as f32 / 20.0, dur * i as f32 / 20.0);
        }
        s
    }

    /// When that drag ended, so `release` sees the right window.
    fn slow_end(total: f32) -> f32 { total.abs() / 0.1 }

    /// A thumb flick: short and fast.
    fn flick(page_w: f32, bounds: Bounds, total: f32) -> Swipe {
        let mut s = Swipe::begin(page_w, bounds, 0.0, 0.0);
        for i in 1..=5 {
            s.update(total * i as f32 / 5.0, i as f32 * 4.0);
        }
        s
    }

    #[test]
    fn the_same_gesture_decides_the_same_way_on_a_phone_and_a_desktop() {
        // Width independence, which is the organising idea. A 60px absolute
        // threshold is a decisive flick on a phone and a twitch on a monitor.
        let phone = slow_drag(390.0, BOTH, -390.0 * 0.3).release(slow_end(390.0 * 0.3));
        let desktop = slow_drag(1280.0, BOTH, -1280.0 * 0.3).release(slow_end(1280.0 * 0.3));
        assert_eq!(phone, Commit::Next);
        assert_eq!(desktop, Commit::Next);

        // Same SPEED, same fraction of the page — only the screen differs.
        let phone_short = slow_drag(390.0, BOTH, -390.0 * 0.1).release(slow_end(390.0 * 0.1));
        let desktop_short = slow_drag(1280.0, BOTH, -1280.0 * 0.1).release(slow_end(1280.0 * 0.1));
        assert_eq!(phone_short, Commit::Cancel);
        assert_eq!(desktop_short, Commit::Cancel);
    }

    #[test]
    fn the_page_width_is_captured_once_and_not_re_read() {
        // A window resized mid-drag must not change what the drag means; a
        // rotation partway through would otherwise turn a committed gesture
        // into a cancelled one.
        let s = slow_drag(390.0, BOTH, -120.0);
        assert_eq!(s.page_width(), 390.0);
        assert!((s.commit_distance() - 390.0 * config(390.0).commit_fraction as f32).abs() < 0.01);
    }

    #[test]
    fn a_deliberate_drag_past_the_distance_threshold_commits() {
        let mut s = slow_drag(400.0, BOTH, -400.0 * 0.25);
        assert_eq!(s.release(slow_end(400.0 * 0.25)), Commit::Next);
    }

    #[test]
    fn a_deliberate_drag_short_of_it_springs_back() {
        let mut s = slow_drag(400.0, BOTH, -400.0 * 0.20);
        assert_eq!(s.release(slow_end(400.0 * 0.20)), Commit::Cancel);
    }

    #[test]
    fn a_short_fast_flick_commits_even_though_it_never_reaches_the_distance() {
        // Distance alone makes flicking feel broken: a thumb flick is short and
        // fast and would never reach 22% of anything.
        let mut s = flick(400.0, BOTH, -40.0);
        assert!(40.0 < 400.0 * config(400.0).commit_fraction as f32, "well under the distance threshold");
        assert_eq!(s.release(20.0), Commit::Next);
    }

    #[test]
    fn a_fast_twitch_shorter_than_the_flick_floor_does_not_turn_the_page() {
        // A tap with a wobble in it. This is what the 14px floor is for.
        let mut s = Swipe::begin(400.0, BOTH, 0.0, 0.0);
        s.update(-8.0, 4.0);
        s.update(-10.0, 8.0);
        assert_eq!(s.release(10.0), Commit::Cancel);
    }

    #[test]
    fn direction_comes_from_the_drag_not_the_velocity() {
        // A flick that reverses at the last instant leaves the page where the
        // drag put it; committing to the velocity's direction would turn the
        // page the opposite way to how it moved on screen.
        let mut s = Swipe::begin(400.0, BOTH, 0.0, 0.0);
        for i in 1..=10 { s.update(-i as f32 * 12.0, i as f32 * 8.0); }
        // A late reversal: the finger snaps back a little before lifting.
        s.update(-100.0, 88.0);
        s.update(-95.0, 92.0);
        assert!(s.drag() < 0.0, "still dragged left overall");
        assert_eq!(s.release(94.0), Commit::Next);
    }

    #[test]
    fn a_positive_drag_reveals_the_previous_page() {
        // Dragging content RIGHT shows what is to the left of it.
        let mut s = slow_drag(400.0, BOTH, 400.0 * 0.3);
        assert_eq!(s.release(slow_end(400.0 * 0.3)), Commit::Prev);
    }

    #[test]
    fn committing_against_an_edge_springs_back_instead() {
        // The rubber band already said there was nowhere to go.
        let mut first = slow_drag(400.0, FIRST, 400.0 * 0.5);
        assert_eq!(first.release(slow_end(400.0 * 0.5)), Commit::Cancel);
        let mut last = slow_drag(400.0, LAST, -400.0 * 0.5);
        assert_eq!(last.release(slow_end(400.0 * 0.5)), Commit::Cancel);
    }

    #[test]
    fn an_edge_drag_still_moves_but_resists() {
        // A hard stop reads as a frozen app; linear movement reads as a page
        // that will turn and then does not.
        let s = slow_drag(400.0, FIRST, 200.0);
        let o = s.offset();
        assert!(o > 0.0, "it moves");
        assert!(o < s.drag(), "but less than the finger did: got {o} for a drag of 200");
    }

    #[test]
    fn the_rubber_band_is_sub_linear_so_it_never_runs_away() {
        let page = 400.0;
        let a = rubber_band(100.0, page);
        let b = rubber_band(400.0, page);
        assert!(b > a, "more drag still moves further");
        assert!(b / a < 4.0, "but not proportionally: {a} -> {b}");
    }

    #[test]
    fn the_rubber_band_never_exceeds_the_drag_itself() {
        // Otherwise the page outruns the finger at the very edge, which reads
        // as the app grabbing the gesture.
        for d in [1.0, 5.0, 50.0, 300.0, 1000.0] {
            assert!(rubber_band(d, 400.0) <= d, "at drag {d}");
        }
    }

    #[test]
    fn the_rubber_band_keeps_the_drags_direction() {
        assert!(rubber_band(-100.0, 400.0) < 0.0);
        assert!(rubber_band(100.0, 400.0) > 0.0);
        assert_eq!(rubber_band(0.0, 400.0), 0.0);
    }

    #[test]
    fn resistance_scales_with_the_page_so_it_feels_the_same_at_any_size() {
        let phone = rubber_band(100.0, 390.0) / 390.0;
        let desktop = rubber_band(100.0 * 1280.0 / 390.0, 1280.0) / 1280.0;
        assert!((phone - desktop).abs() < 0.05, "{phone} vs {desktop}");
    }

    #[test]
    fn a_drag_in_the_middle_of_the_pages_is_not_banded() {
        let s = slow_drag(400.0, BOTH, 150.0);
        assert!((s.offset() - 150.0).abs() < 0.01, "follows the finger exactly");
    }

    // The velocity tracker's own tests moved to uikit-gesture with the code.
    // Keeping copies here would be the duplication this refactor removed,
    // wearing a test's clothes.

    #[test]
    fn a_flick_settles_faster_than_a_slow_drag() {
        // Matching the animation to the gesture is what makes the page feel
        // like it is continuing your movement rather than playing a canned
        // transition.
        let quick = flick(400.0, BOTH, -80.0).settle_ms(20.0);
        let slow = slow_drag(400.0, BOTH, -120.0).settle_ms(slow_end(120.0));
        assert!(quick < slow, "flick {quick}ms vs drag {slow}ms");
        assert!((120.0..=320.0).contains(&quick));
        assert!((120.0..=320.0).contains(&slow));
    }
}
