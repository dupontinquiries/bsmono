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

/// `kRubberBandK`.
pub const RUBBER_BAND_K: f32 = 0.40;
/// `kCommitFraction` — 22% of the page.
pub const COMMIT_FRACTION: f32 = 0.22;
/// `kFlickThresholdPxMs`.
pub const FLICK_PX_PER_MS: f32 = 0.30;
/// `kFlickMinPx` — a flick still has to travel.
pub const FLICK_MIN_PX: f32 = 14.0;
/// `kVelWindowMs` — wider than the old 80ms so slow deliberate swipes register.
pub const VELOCITY_WINDOW_MS: f32 = 120.0;
/// Ring buffer depth.
pub const VELOCITY_SAMPLES: usize = 24;

/// One position sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample { pub x: f32, pub t_ms: f32 }

/// Weighted velocity tracker.
#[derive(Debug, Clone, Default)]
pub struct VelocityTracker {
    samples: Vec<Sample>,
}

impl VelocityTracker {
    pub fn push(&mut self, x: f32, t_ms: f32) {
        self.samples.push(Sample { x, t_ms });
        if self.samples.len() > VELOCITY_SAMPLES { self.samples.remove(0); }
    }

    pub fn clear(&mut self) { self.samples.clear(); }
    pub fn len(&self) -> usize { self.samples.len() }
    pub fn is_empty(&self) -> bool { self.samples.is_empty() }

    /// Velocity in px/ms, weighted toward recent samples.
    ///
    /// Recency weighting matters because a swipe that slows before release was
    /// *aimed* at stopping — averaging the whole window flat would report the
    /// speed of the middle of the gesture and fling a page the user had
    /// deliberately decelerated.
    ///
    /// Samples older than the window are discarded rather than weighted to
    /// near-zero: a finger that rested for a second before moving would
    /// otherwise have that pause dragged into the average.
    pub fn velocity(&self, now_ms: f32) -> f32 {
        let recent: Vec<&Sample> = self.samples.iter()
            .filter(|s| now_ms - s.t_ms <= VELOCITY_WINDOW_MS)
            .collect();
        if recent.len() < 2 { return 0.0; }
        let mut num = 0.0;
        let mut den = 0.0;
        for pair in recent.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let dt = b.t_ms - a.t_ms;
            // A zero-duration pair is two events in the same millisecond; its
            // velocity is undefined, not infinite.
            if dt <= 0.0 { continue; }
            let age = now_ms - b.t_ms;
            // Exponential decay: the newest pair counts about e times as much
            // as one a full window older.
            let w = (-age / VELOCITY_WINDOW_MS).exp();
            num += (b.x - a.x) / dt * w;
            den += w;
        }
        if den == 0.0 { 0.0 } else { num / den }
    }
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

/// Rubber-band resistance at an edge.
///
/// Sub-linear so the page never runs away, and scaled by page width so the
/// resistance feels the same at any size. A hard stop reads as a frozen app.
pub fn rubber_band(drag: f32, page_w: f32) -> f32 {
    let d = drag.abs();
    if d == 0.0 || page_w <= 0.0 { return 0.0; }
    let banded = (d * page_w * RUBBER_BAND_K).sqrt().min(d);
    banded * drag.signum()
}

/// A gesture in progress. The page width is captured here and never re-read.
#[derive(Debug, Clone)]
pub struct Swipe {
    page_w: f32,
    bounds: Bounds,
    drag: f32,
    tracker: VelocityTracker,
    start_x: f32,
    active: bool,
}

impl Swipe {
    /// `gestureBegin`: capture the width ONCE.
    pub fn begin(page_w: f32, bounds: Bounds, x: f32, t_ms: f32) -> Self {
        let mut tracker = VelocityTracker::default();
        tracker.push(x, t_ms);
        Swipe { page_w, bounds, drag: 0.0, tracker, start_x: x, active: true }
    }

    pub fn update(&mut self, x: f32, t_ms: f32) {
        if !self.active { return; }
        self.drag = x - self.start_x;
        self.tracker.push(x, t_ms);
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

    /// The distance threshold in pixels for this gesture.
    pub fn commit_distance(&self) -> f32 { self.page_w * COMMIT_FRACTION }

    /// `gestureEnd`: decide.
    pub fn release(&mut self, t_ms: f32) -> Commit {
        self.active = false;
        let v = self.tracker.velocity(t_ms);
        let d = self.drag;
        // Either path can commit. Distance alone makes flicking feel broken;
        // velocity alone makes a careful drag impossible.
        let by_distance = d.abs() > self.commit_distance();
        // The 14px floor is what stops a fast twitch — a tap with a wobble in
        // it — from turning the page.
        let by_flick = v.abs() > FLICK_PX_PER_MS && d.abs() > FLICK_MIN_PX;
        if !(by_distance || by_flick) { return Commit::Cancel; }

        // Direction from the DRAG, not the velocity: a flick that reverses at
        // the last instant still leaves the page where the drag put it, and
        // committing to the velocity's direction would turn the page the
        // opposite way to how it moved on screen.
        let want = if d > 0.0 { Commit::Prev } else { Commit::Next };
        match want {
            Commit::Prev if self.bounds.can_prev => Commit::Prev,
            Commit::Next if self.bounds.can_next => Commit::Next,
            // Committed against an edge: there is nowhere to go, so it springs
            // back. The rubber band already said so on the way.
            _ => Commit::Cancel,
        }
    }

    /// How long the settle animation should run, given the release velocity.
    ///
    /// A fast flick commits quickly and a slow drag settles gently: matching
    /// the animation to the gesture is what makes the page feel like it is
    /// continuing your movement rather than playing a canned transition.
    pub fn settle_ms(&self, t_ms: f32) -> f32 {
        let v = self.tracker.velocity(t_ms).abs();
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
        assert!((s.commit_distance() - 390.0 * COMMIT_FRACTION).abs() < 0.01);
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
        assert!(40.0 < 400.0 * COMMIT_FRACTION, "well under the distance threshold");
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

    // ── velocity ────────────────────────────────────────────────────────────

    #[test]
    fn velocity_needs_two_samples() {
        let mut t = VelocityTracker::default();
        assert_eq!(t.velocity(0.0), 0.0);
        t.push(0.0, 0.0);
        assert_eq!(t.velocity(0.0), 0.0, "one sample is a position, not a speed");
    }

    #[test]
    fn velocity_is_reported_in_pixels_per_millisecond() {
        let mut t = VelocityTracker::default();
        for i in 0..=10 { t.push(i as f32 * 5.0, i as f32 * 10.0); }
        let v = t.velocity(100.0);
        assert!((v - 0.5).abs() < 0.05, "5px per 10ms is 0.5 px/ms, got {v}");
    }

    #[test]
    fn a_swipe_that_slows_before_release_is_not_flung() {
        // It was AIMED at stopping. A flat average would report the speed of
        // the middle of the gesture and fling a page the user decelerated.
        let mut fast = VelocityTracker::default();
        for i in 0..=10 { fast.push(i as f32 * 10.0, i as f32 * 10.0); }
        let mut slowing = VelocityTracker::default();
        for i in 0..=5 { slowing.push(i as f32 * 10.0, i as f32 * 10.0); }
        for i in 1..=5 { slowing.push(50.0 + i as f32 * 0.5, 50.0 + i as f32 * 10.0); }
        assert!(slowing.velocity(100.0) < fast.velocity(100.0) * 0.5,
                "a decelerating swipe should report far less speed");
    }

    #[test]
    fn samples_older_than_the_window_are_discarded_not_merely_down_weighted() {
        // A finger that rested a second before moving would otherwise drag that
        // pause into the average.
        let mut t = VelocityTracker::default();
        t.push(0.0, 0.0);
        t.push(0.0, 1000.0);          // a long pause
        t.push(30.0, 1010.0);
        t.push(60.0, 1020.0);
        let v = t.velocity(1020.0);
        assert!(v > 2.0, "the pause should not damp the real movement, got {v}");
    }

    #[test]
    fn the_ring_buffer_is_bounded() {
        let mut t = VelocityTracker::default();
        for i in 0..200 { t.push(i as f32, i as f32); }
        assert_eq!(t.len(), VELOCITY_SAMPLES);
    }

    #[test]
    fn two_events_in_the_same_millisecond_do_not_produce_infinite_velocity() {
        let mut t = VelocityTracker::default();
        t.push(0.0, 5.0);
        t.push(50.0, 5.0);
        t.push(60.0, 6.0);
        assert!(t.velocity(6.0).is_finite());
    }

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
