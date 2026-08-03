//! scroll_areas — `HorizontalScrollArea` and `CarouselAwareScrollArea`.
//!
//! Two small surfaces that exist entirely because a wheel event is ambiguous.
//!
//! ## Why the theme carousel needs its own scroll area
//!
//! A scroll area scrolls vertically on wheel by default, so a mouse wheel over
//! a horizontal carousel does nothing at all. Not "scrolls the wrong way" —
//! nothing, because there is no vertical range to move. `HorizontalScrollArea`
//! maps both a horizontal trackpad swipe and a vertical mouse wheel onto
//! horizontal motion, which is what a user expects from either device over a
//! row of cards.
//!
//! ## The handoff, which is the whole reason this is interesting
//!
//! The carousel sits inside the page swiper. If it consumed every wheel event
//! it would swallow the gesture at its own edge: you scroll to the last theme,
//! keep pushing, and nothing happens — the carousel has no more room and the
//! page never learns you wanted to move on.
//!
//! So consumption is conditional on MOVEMENT. Apply the delta; if the value
//! actually changed, the scroll used it. If it was already clamped at the edge
//! you are pushing toward, do not consume — let it reach the page nav, which
//! turns continued scrolling at the end into a page change. "Scroll past the
//! end to the next page" only works because the inner widget declines.
//!
//! ## The sign convention, written down because it is the easy thing to invert
//!
//! `set_value(before - delta)`. A positive delta scrolls content left, which
//! DECREASES the scrollbar value toward its minimum. The clamp check has to
//! match that or the passthrough fires at the wrong end — and the symptom is
//! the subtlest possible one: paging works at one edge of the carousel and
//! silently dies at the other.

/// A wheel event's deltas. Pixel deltas come from trackpads and are precise;
/// angle deltas come from wheels and are in eighths of a degree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    /// High-resolution delta, when the device provides one.
    pub pixel: Option<(f32, f32)>,
    /// Angle delta in eighths of a degree.
    pub angle: (f32, f32),
}

impl Wheel {
    pub fn pixels(x: f32, y: f32) -> Wheel { Wheel { pixel: Some((x, y)), angle: (0.0, 0.0) } }
    pub fn degrees(x: f32, y: f32) -> Wheel { Wheel { pixel: None, angle: (x * 8.0, y * 8.0) } }

    /// The scalar to scroll by: the dominant axis, so a vertical mouse wheel
    /// and a horizontal trackpad swipe both move the carousel.
    ///
    /// Pixel deltas win when present — they are the finer measurement, and
    /// mixing the two scales would make a trackpad and a wheel move by wildly
    /// different amounts for the same gesture.
    pub fn dominant(&self) -> f32 {
        match self.pixel {
            Some((x, y)) => if x.abs() > y.abs() { x } else { y },
            None => {
                let (x, y) = self.angle;
                if x.abs() > y.abs() { x / 8.0 } else { y / 8.0 }
            }
        }
    }

    /// The horizontal component only, for the outer area's routing — it
    /// forwards sideways gestures to the carousel and leaves vertical ones to
    /// scroll the settings page, which is what the cursor is mostly doing.
    pub fn horizontal(&self) -> f32 {
        match self.pixel {
            Some((x, _)) => x,
            None => {
                let (x, _) = self.angle;
                if x != 0.0 { x / 8.0 } else { 0.0 }
            }
        }
    }
}

/// What the scroll area did with a wheel event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wheeled {
    /// The scroll moved; the event is used up.
    Consumed,
    /// Clamped at the edge and still pushing, or nothing to do. The event must
    /// reach the page nav — this is the handoff.
    PassThrough,
}

impl Wheeled {
    pub fn consumed(self) -> bool { matches!(self, Wheeled::Consumed) }
}

/// A horizontally-scrolling strip.
#[derive(Debug, Clone, PartialEq)]
pub struct HScroll {
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

impl HScroll {
    pub fn new(min: f32, max: f32) -> Self { HScroll { value: min, min, max } }

    /// Apply a wheel event.
    pub fn wheel(&mut self, w: Wheel) -> Wheeled {
        let delta = w.dominant();
        if delta == 0.0 { return Wheeled::PassThrough; }
        let before = self.value;
        // Positive delta scrolls content left, decreasing the value.
        self.value = (before - delta).clamp(self.min, self.max);
        if self.value != before { Wheeled::Consumed } else { Wheeled::PassThrough }
    }

    pub fn at_min(&self) -> bool { self.value <= self.min }
    pub fn at_max(&self) -> bool { self.value >= self.max }
}

/// A rectangle, for the outer area's hit test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }

/// How far outside the carousel a cursor still counts as "over" it.
///
/// Wide vertically (80px) and narrow horizontally (8px) on purpose. The cursor
/// drifts above or below a card while the hand is making a sideways gesture, and
/// a strict hit test drops the scroll mid-swipe. Sideways there is no such
/// slack: past the edge horizontally you are genuinely on the page beside it.
pub const HIT_MARGIN_X: f32 = 8.0;
pub const HIT_MARGIN_Y: f32 = 80.0;

impl Rect {
    /// Is a point inside, allowing for the generous vertical margin?
    pub fn contains_with_margin(&self, px: f32, py: f32) -> bool {
        px >= self.x - HIT_MARGIN_X && px <= self.x + self.w + HIT_MARGIN_X
            && py >= self.y - HIT_MARGIN_Y && py <= self.y + self.h + HIT_MARGIN_Y
    }
}

/// The outer settings area's decision: does this wheel event belong to the
/// carousel?
///
/// Only horizontal gestures, and only near the carousel. It exists to cover the
/// case the inner area cannot — the cursor just outside the strip, on a card's
/// edge, where the inner widget never sees the event at all.
pub fn route_to_carousel(w: Wheel, cursor: (f32, f32), carousel: Option<Rect>) -> bool {
    let Some(r) = carousel else { return false };
    if w.horizontal() == 0.0 { return false; }
    r.contains_with_margin(cursor.0, cursor.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vertical_mouse_wheel_scrolls_the_carousel_sideways() {
        // The whole reason this widget exists: a default scroll area has no
        // vertical range here, so the wheel does NOTHING rather than the wrong
        // thing.
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 250.0;
        assert_eq!(s.wheel(Wheel::degrees(0.0, 15.0)), Wheeled::Consumed);
        assert_ne!(s.value, 250.0);
    }

    #[test]
    fn a_horizontal_trackpad_swipe_scrolls_it_too() {
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 250.0;
        assert_eq!(s.wheel(Wheel::pixels(-40.0, 0.0)), Wheeled::Consumed);
        assert_eq!(s.value, 290.0);
    }

    #[test]
    fn the_dominant_axis_wins() {
        assert_eq!(Wheel::pixels(30.0, 5.0).dominant(), 30.0);
        assert_eq!(Wheel::pixels(5.0, -30.0).dominant(), -30.0);
        assert_eq!(Wheel::degrees(0.0, 15.0).dominant(), 15.0);
        assert_eq!(Wheel::degrees(15.0, 3.0).dominant(), 15.0);
    }

    #[test]
    fn pixel_deltas_are_preferred_over_angle_deltas() {
        // Mixing the scales would make a trackpad and a wheel move by wildly
        // different amounts for the same gesture.
        let w = Wheel { pixel: Some((12.0, 0.0)), angle: (960.0, 0.0) };
        assert_eq!(w.dominant(), 12.0);
    }

    #[test]
    fn angle_deltas_are_eighths_of_a_degree() {
        // A wheel notch is 15 degrees = 120 eighths.
        let w = Wheel { pixel: None, angle: (0.0, 120.0) };
        assert_eq!(w.dominant(), 15.0);
    }

    #[test]
    fn a_positive_delta_moves_the_value_toward_its_minimum() {
        // The easy thing to invert. Getting it backwards makes paging work at
        // one edge of the carousel and silently die at the other.
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 250.0;
        s.wheel(Wheel::pixels(50.0, 0.0));
        assert_eq!(s.value, 200.0, "positive delta decreases the value");
    }

    #[test]
    fn scrolling_past_the_end_hands_off_instead_of_swallowing_the_gesture() {
        // THE bug this exists for: the carousel silently ate the wheel at its
        // edge, so "keep scrolling to reach the next page" did nothing.
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 500.0;
        // Still room to move back.
        assert_eq!(s.wheel(Wheel::pixels(10.0, 0.0)), Wheeled::Consumed);
        s.value = 500.0;
        // Pushing further toward max with nowhere to go.
        assert_eq!(s.wheel(Wheel::pixels(-10.0, 0.0)), Wheeled::PassThrough);
        assert!(!Wheeled::PassThrough.consumed());
    }

    #[test]
    fn the_handoff_works_at_both_edges() {
        // Asymmetry here is exactly the symptom a wrong sign produces.
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 0.0;
        assert_eq!(s.wheel(Wheel::pixels(10.0, 0.0)), Wheeled::PassThrough, "at min");
        s.value = 500.0;
        assert_eq!(s.wheel(Wheel::pixels(-10.0, 0.0)), Wheeled::PassThrough, "at max");
    }

    #[test]
    fn a_partial_move_at_the_edge_still_counts_as_consumed() {
        // 5px of room and a 50px gesture: the carousel used what it had, so it
        // keeps the event. The NEXT one hands off.
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 5.0;
        assert_eq!(s.wheel(Wheel::pixels(50.0, 0.0)), Wheeled::Consumed);
        assert_eq!(s.value, 0.0);
        assert_eq!(s.wheel(Wheel::pixels(50.0, 0.0)), Wheeled::PassThrough);
    }

    #[test]
    fn a_zero_delta_is_not_consumed() {
        // A wheel event with no movement must not eat a gesture.
        let mut s = HScroll::new(0.0, 500.0);
        s.value = 250.0;
        assert_eq!(s.wheel(Wheel::pixels(0.0, 0.0)), Wheeled::PassThrough);
        assert_eq!(s.value, 250.0);
    }

    #[test]
    fn the_value_never_leaves_its_range() {
        let mut s = HScroll::new(0.0, 100.0);
        for _ in 0..50 { s.wheel(Wheel::pixels(-100.0, 0.0)); }
        assert_eq!(s.value, 100.0);
        for _ in 0..50 { s.wheel(Wheel::pixels(100.0, 0.0)); }
        assert_eq!(s.value, 0.0);
    }

    // ── outer routing ───────────────────────────────────────────────────────

    fn carousel() -> Option<Rect> { Some(Rect { x: 20.0, y: 100.0, w: 300.0, h: 90.0 }) }

    #[test]
    fn a_sideways_gesture_over_the_carousel_is_routed_to_it() {
        assert!(route_to_carousel(Wheel::pixels(-30.0, 0.0), (150.0, 140.0), carousel()));
    }

    #[test]
    fn a_vertical_gesture_is_left_to_scroll_the_page() {
        // Which is what the cursor is mostly doing on a settings page.
        assert!(!route_to_carousel(Wheel::pixels(0.0, -30.0), (150.0, 140.0), carousel()));
        assert!(!route_to_carousel(Wheel::degrees(0.0, 15.0), (150.0, 140.0), carousel()));
    }

    #[test]
    fn the_vertical_margin_is_generous_because_a_hand_drifts_mid_swipe() {
        // The cursor wanders above or below a card while the hand makes a
        // sideways gesture; a strict hit test drops the scroll mid-swipe.
        let w = Wheel::pixels(-30.0, 0.0);
        assert!(route_to_carousel(w, (150.0, 100.0 - HIT_MARGIN_Y + 1.0), carousel()),
                "just above the strip still counts");
        assert!(route_to_carousel(w, (150.0, 190.0 + HIT_MARGIN_Y - 1.0), carousel()),
                "just below too");
        assert!(!route_to_carousel(w, (150.0, 0.0), carousel()),
                "but not the whole page");
    }

    #[test]
    fn the_horizontal_margin_is_narrow_because_sideways_means_elsewhere() {
        // Past the edge horizontally you are genuinely on the page beside it.
        let w = Wheel::pixels(-30.0, 0.0);
        assert!(route_to_carousel(w, (20.0 - HIT_MARGIN_X + 1.0, 140.0), carousel()));
        assert!(!route_to_carousel(w, (20.0 - HIT_MARGIN_X - 5.0, 140.0), carousel()));
    }

    #[test]
    fn no_carousel_on_screen_routes_nothing() {
        assert!(!route_to_carousel(Wheel::pixels(-30.0, 0.0), (150.0, 140.0), None));
    }
}
