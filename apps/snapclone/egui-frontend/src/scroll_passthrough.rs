//! scroll_passthrough — `ScrollAreas.cpp`, ported.
//!
//! A horizontal scroll area that **stops consuming the wheel once it is at the
//! edge**, so continued scrolling reaches the page navigator behind it.
//!
//! ## The bug it fixes, in the C++'s own words
//!
//! *"Without this the carousel silently swallowed the wheel at its edge and the
//! gesture died, so you couldn't 'scroll past the end' to the next page."*
//!
//! That is the failure worth naming: not a crash, not an error — the gesture
//! simply stops working at exactly the moment a user is trying hardest. A
//! scroll area that always consumes looks correct in isolation and breaks the
//! surface it sits in.
//!
//! ## Consumption is decided by whether the value MOVED
//!
//! Not by comparing against the limits before applying. Those differ whenever a
//! delta is large enough to hit the edge partway: the value moves *some*, which
//! is a consumed event, while a bounds check beforehand would pass the whole
//! gesture through and skip a page while the carousel still had room.
//!
//! ## The sign convention is inverted, and the C++ warns about it
//!
//! *"delta>0 scrolls content left → toward MAX; delta<0 → toward MIN. setValue
//! uses value-delta, so pushing right/positive delta DECREASES value toward
//! min — match the clamp check to that."* Getting this backwards passes through
//! at the wrong end, which reads as the page flipping when you scroll *into*
//! the carousel rather than out of it.

/// Which wheel deltas a platform reported.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wheel {
    /// High-resolution pixel delta, when the platform provides one.
    pub pixel: Option<(f32, f32)>,
    /// Notch-based angle delta, in eighths of a degree.
    pub angle: (f32, f32),
}

impl Wheel {
    /// The scalar delta to apply.
    ///
    /// Pixel delta wins when present — a touchpad reports it and it is what
    /// makes the scroll track the fingers. Otherwise the angle delta is divided
    /// by eight, as Qt's units require.
    ///
    /// The larger AXIS wins either way: a horizontal carousel must respond to a
    /// vertical wheel, because a mouse has only one and the user expects it to
    /// scroll whatever is under the cursor.
    pub fn delta(&self) -> f32 {
        match self.pixel {
            Some((x, y)) if x != 0.0 || y != 0.0 => if x.abs() > y.abs() { x } else { y },
            _ => {
                let (x, y) = self.angle;
                (if x.abs() > y.abs() { x } else { y }) / 8.0
            }
        }
    }
}

/// What the scroll area did with an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handled {
    /// The value moved; the event is ours.
    Consumed,
    /// At the edge and still pushing — let the page navigator have it.
    PassThrough,
}

/// A horizontal scroll position with limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HScroll {
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

impl HScroll {
    pub fn new(min: f32, max: f32) -> HScroll {
        HScroll { value: min, min, max: max.max(min) }
    }

    pub fn at_min(&self) -> bool { self.value <= self.min }
    pub fn at_max(&self) -> bool { self.value >= self.max }

    /// Apply a wheel event.
    ///
    /// Note the subtraction: positive delta *decreases* the value, matching
    /// Qt's `setValue(value - delta)` — see the module docs on the sign.
    pub fn wheel(&mut self, w: &Wheel) -> Handled {
        let delta = w.delta();
        if delta == 0.0 {
            // A zero delta is a real event on some touchpads at the moment a
            // gesture reverses. Passing it through would flip a page on a
            // gesture that has not moved anything.
            return Handled::Consumed;
        }
        let before = self.value;
        self.value = (before - delta).clamp(self.min, self.max);
        // Whether it MOVED, not whether it was at the edge beforehand.
        if self.value != before { Handled::Consumed } else { Handled::PassThrough }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notch(x: f32) -> Wheel { Wheel { pixel: None, angle: (x * 8.0, 0.0) } }
    fn carousel() -> HScroll { HScroll::new(0.0, 100.0) }

    #[test]
    fn scrolling_with_room_left_consumes_the_event() {
        let mut s = carousel();
        s.value = 50.0;
        assert_eq!(s.wheel(&notch(-10.0)), Handled::Consumed);
        assert_eq!(s.value, 60.0);
    }

    #[test]
    fn pushing_past_the_end_passes_through_to_the_page_navigator() {
        // The bug this exists for: "the carousel silently swallowed the wheel
        // at its edge and the gesture died, so you couldn't scroll past the end
        // to the next page".
        let mut s = carousel();
        s.value = 100.0;
        assert_eq!(s.wheel(&notch(-10.0)), Handled::PassThrough);
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn pushing_back_from_the_end_is_still_consumed() {
        // Only the direction being pushed matters. Passing through here would
        // flip a page when the user is scrolling BACK into a carousel that has
        // plenty of room.
        let mut s = carousel();
        s.value = 100.0;
        assert_eq!(s.wheel(&notch(10.0)), Handled::Consumed);
        assert_eq!(s.value, 90.0);
    }

    #[test]
    fn the_same_holds_at_the_other_end() {
        let mut s = carousel();
        assert!(s.at_min());
        assert_eq!(s.wheel(&notch(10.0)), Handled::PassThrough, "pushing past min");
        assert_eq!(s.wheel(&notch(-10.0)), Handled::Consumed, "back into range");
    }

    #[test]
    fn a_delta_that_reaches_the_edge_partway_is_consumed() {
        // The reason consumption is decided by whether the value MOVED rather
        // than by a bounds check beforehand: a big delta from near the edge
        // moves SOME, and passing it through would skip a page while the
        // carousel still had room.
        let mut s = carousel();
        s.value = 95.0;
        assert_eq!(s.wheel(&notch(-50.0)), Handled::Consumed);
        assert_eq!(s.value, 100.0, "clamped, but it moved");
        assert_eq!(s.wheel(&notch(-50.0)), Handled::PassThrough, "the NEXT one goes through");
    }

    #[test]
    fn the_sign_convention_matches_the_qt_original() {
        // "pushing right/positive delta DECREASES value toward min". Getting
        // this backwards passes through at the wrong end, which reads as the
        // page flipping when you scroll INTO the carousel.
        let mut s = carousel();
        s.value = 50.0;
        s.wheel(&notch(10.0));
        assert!(s.value < 50.0, "positive delta moved toward min");
    }

    #[test]
    fn a_pixel_delta_is_preferred_over_an_angle_delta() {
        // A touchpad reports pixels, and that is what makes the scroll track
        // the fingers rather than jump in notches.
        let w = Wheel { pixel: Some((12.0, 0.0)), angle: (800.0, 0.0) };
        assert_eq!(w.delta(), 12.0, "not 100 from the angle");
    }

    #[test]
    fn a_vertical_wheel_scrolls_a_horizontal_carousel() {
        // A mouse has one wheel, and the user expects it to scroll whatever is
        // under the cursor.
        let mut s = carousel();
        s.value = 50.0;
        let vertical = Wheel { pixel: None, angle: (0.0, -80.0) };
        assert_eq!(s.wheel(&vertical), Handled::Consumed);
        assert_eq!(s.value, 60.0);
    }

    #[test]
    fn the_larger_axis_wins_when_both_are_present() {
        // A diagonal touchpad gesture is one gesture, not two.
        let w = Wheel { pixel: Some((3.0, 20.0)), angle: (0.0, 0.0) };
        assert_eq!(w.delta(), 20.0);
        let w2 = Wheel { pixel: Some((20.0, 3.0)), angle: (0.0, 0.0) };
        assert_eq!(w2.delta(), 20.0);
    }

    #[test]
    fn a_zero_delta_does_not_flip_a_page() {
        // Some touchpads emit one as a gesture reverses; passing it through
        // would turn a hesitation into a page change.
        let mut s = carousel();
        assert_eq!(s.wheel(&notch(0.0)), Handled::Consumed);
        let empty = Wheel { pixel: Some((0.0, 0.0)), angle: (0.0, 0.0) };
        assert_eq!(s.wheel(&empty), Handled::Consumed);
    }

    #[test]
    fn a_carousel_with_nothing_to_scroll_passes_everything_through() {
        // One item, so min == max. Every wheel should reach the navigator
        // rather than dying in a scroll area with no range.
        let mut s = HScroll::new(0.0, 0.0);
        assert_eq!(s.wheel(&notch(10.0)), Handled::PassThrough);
        assert_eq!(s.wheel(&notch(-10.0)), Handled::PassThrough);
    }

    #[test]
    fn an_inverted_range_does_not_produce_a_scroll_area_that_cannot_move() {
        // max < min is reachable from a layout computed before content arrives.
        let s = HScroll::new(50.0, 10.0);
        assert!(s.max >= s.min, "clamped at construction");
    }
}
