//! keyboard_inset — `VirtualKeyboardWatcher`, and what consumes it.
//!
//! An on-screen keyboard covers the bottom of the screen, which is where a
//! composer's text field and Send button live. Without this the keyboard opens
//! over the thing you are typing into — the defining mobile bug, and the reason
//! this is the port worth doing before anything cosmetic.
//!
//! ## Reported as a rectangle, not a height
//!
//! The Qt watcher tracks `keyboardRectangle()` in **screen coordinates**, and
//! `applyKeyboardInset` computes the overlap against the window's own bottom
//! edge rather than using the keyboard's height. Those differ whenever the
//! window is not full-height — a floating composer, a split-screen phone, a
//! desktop window with the panel showing — and using the height there insets a
//! window that the keyboard never covered.
//!
//! ## Two details from the C++ that are easy to drop
//!
//! * **Identical reports are de-duped.** Compositors re-send the same rectangle;
//!   acting on every one relayouts continuously while the keyboard sits still.
//! * **The rectangle is aligned to whole pixels.** The comment says why: it
//!   "avoids sub-pixel jitter from compositor reporting", which otherwise
//!   shows up as a composer that shivers.

/// A rectangle in screen coordinates.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect { Rect { x, y, w, h } }
    pub fn bottom(&self) -> f32 { self.y + self.h }

    /// Empty means no keyboard, which is how the watcher reports "hidden" —
    /// there is no separate visibility flag to fall out of sync with.
    pub fn is_empty(&self) -> bool { self.w <= 0.0 || self.h <= 0.0 }

    /// Round outward to whole pixels.
    ///
    /// The C++ uses `toAlignedRect` for the reason its comment gives: sub-pixel
    /// compositor reports make the composer shiver. Outward rather than nearest
    /// so the inset never lands a fraction of a pixel short and leaves a seam.
    pub fn aligned(&self) -> Rect {
        let x = self.x.floor();
        let y = self.y.floor();
        Rect { x, y, w: (self.x + self.w).ceil() - x, h: (self.y + self.h).ceil() - y }
    }
}

/// Tracks the on-screen keyboard.
#[derive(Clone, Debug, Default)]
pub struct KeyboardWatcher {
    rect: Rect,
    /// Incremented only when the rectangle actually changed, so a caller can
    /// relayout on a change rather than on a report.
    generation: u64,
}

impl KeyboardWatcher {
    pub fn is_visible(&self) -> bool { !self.rect.is_empty() }
    pub fn rect(&self) -> Rect { self.rect }
    pub fn generation(&self) -> u64 { self.generation }

    /// Feed a report from the platform. Returns true when something changed.
    ///
    /// Compositors re-send identical rectangles; acting on every report
    /// relayouts continuously while the keyboard sits perfectly still.
    pub fn report(&mut self, rect: Rect) -> bool {
        let next = rect.aligned();
        if next == self.rect { return false; }
        self.rect = next;
        self.generation += 1;
        true
    }

    /// Report that the keyboard closed.
    pub fn hide(&mut self) -> bool { self.report(Rect::default()) }

    /// How far to inset a window's bottom edge.
    ///
    /// Computed as the OVERLAP between the keyboard and this window, not the
    /// keyboard's height. They differ whenever the window is not full-height —
    /// a floating composer, split-screen, a desktop window above a panel — and
    /// using the height insets a window the keyboard never covered.
    pub fn inset_for(&self, window: Rect) -> f32 {
        if self.rect.is_empty() { return 0.0; }
        (window.bottom() - self.rect.y).max(0.0).min(window.h)
    }

    /// Is the keyboard covering this rectangle at all?
    ///
    /// Asked before scrolling a field into view: scrolling one that was never
    /// covered moves the page under the user for no reason.
    pub fn covers(&self, r: Rect) -> bool {
        !self.rect.is_empty() && r.bottom() > self.rect.y && r.y < self.rect.bottom()
    }

    /// How far to scroll so `field` sits above the keyboard, plus `margin`.
    ///
    /// Zero when it already fits. Never negative: a field above the keyboard
    /// must not scroll *down* to meet it, which is what an unclamped difference
    /// would do.
    pub fn scroll_to_clear(&self, field: Rect, margin: f32) -> f32 {
        if !self.covers(field) { return 0.0; }
        (field.bottom() + margin - self.rect.y).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1080x2400 phone with a 900px keyboard at the bottom.
    fn phone_kb() -> Rect { Rect::new(0.0, 1500.0, 1080.0, 900.0) }
    fn full_window() -> Rect { Rect::new(0.0, 0.0, 1080.0, 2400.0) }

    #[test]
    fn an_empty_rectangle_is_how_hidden_is_reported() {
        // No separate visibility flag to fall out of sync with the rectangle.
        let mut w = KeyboardWatcher::default();
        assert!(!w.is_visible());
        w.report(phone_kb());
        assert!(w.is_visible());
        w.hide();
        assert!(!w.is_visible());
        assert_eq!(w.inset_for(full_window()), 0.0);
    }

    #[test]
    fn an_identical_report_changes_nothing() {
        // Compositors re-send the same rectangle; acting on every one relayouts
        // continuously while the keyboard sits still.
        let mut w = KeyboardWatcher::default();
        assert!(w.report(phone_kb()));
        let gen = w.generation();
        assert!(!w.report(phone_kb()), "the second report is a no-op");
        assert_eq!(w.generation(), gen);
    }

    #[test]
    fn sub_pixel_jitter_does_not_count_as_a_change() {
        // The reason the C++ aligns the rect: otherwise the composer shivers.
        let mut w = KeyboardWatcher::default();
        w.report(Rect::new(0.0, 1500.0, 1080.0, 900.0));
        let gen = w.generation();
        assert!(!w.report(Rect::new(0.0, 1500.2, 1080.0, 899.8)),
                "a fraction of a pixel is not a move");
        assert_eq!(w.generation(), gen);
    }

    #[test]
    fn a_real_move_does_register() {
        // Switching between a compact and a full keyboard layout is a real
        // change, and de-duping must not swallow it.
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        assert!(w.report(Rect::new(0.0, 1800.0, 1080.0, 600.0)));
    }

    #[test]
    fn a_full_height_window_is_inset_by_the_keyboard_height() {
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        assert_eq!(w.inset_for(full_window()), 900.0);
    }

    #[test]
    fn a_window_the_keyboard_does_not_reach_is_not_inset() {
        // The case that makes this an overlap rather than a height: a floating
        // composer in the upper half of the screen. Using the keyboard height
        // would inset a window it never covered.
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        let floating = Rect::new(100.0, 100.0, 880.0, 800.0);   // bottom at 900
        assert_eq!(w.inset_for(floating), 0.0);
    }

    #[test]
    fn a_partially_covered_window_is_inset_by_the_overlap_only() {
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        let split = Rect::new(0.0, 1000.0, 1080.0, 700.0);      // bottom at 1700
        assert_eq!(w.inset_for(split), 200.0, "1700 - 1500");
    }

    #[test]
    fn the_inset_never_exceeds_the_window_itself() {
        // A keyboard taller than a small window would otherwise inset it past
        // its own height and produce a negative content area.
        let mut w = KeyboardWatcher::default();
        w.report(Rect::new(0.0, 100.0, 1080.0, 2300.0));
        let small = Rect::new(0.0, 1000.0, 1080.0, 300.0);
        assert_eq!(w.inset_for(small), 300.0);
    }

    #[test]
    fn a_field_above_the_keyboard_needs_no_scroll() {
        // Scrolling one that was never covered moves the page under the user
        // for no reason.
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        let field = Rect::new(40.0, 400.0, 1000.0, 60.0);
        assert!(!w.covers(field));
        assert_eq!(w.scroll_to_clear(field, 16.0), 0.0);
    }

    #[test]
    fn a_covered_field_scrolls_just_clear_with_its_margin() {
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        let field = Rect::new(40.0, 1450.0, 1000.0, 60.0);      // bottom 1510
        assert_eq!(w.scroll_to_clear(field, 16.0), 26.0, "1510 + 16 - 1500");
    }

    #[test]
    fn scrolling_is_never_negative() {
        // An unclamped difference would scroll a field DOWN to meet the
        // keyboard, which is the opposite of the intent.
        let mut w = KeyboardWatcher::default();
        w.report(phone_kb());
        for y in [0.0, 500.0, 1400.0, 1499.0] {
            let f = Rect::new(0.0, y, 100.0, 20.0);
            assert!(w.scroll_to_clear(f, 16.0) >= 0.0, "field at {y}");
        }
    }

    #[test]
    fn nothing_scrolls_or_insets_while_the_keyboard_is_hidden() {
        let w = KeyboardWatcher::default();
        let field = Rect::new(40.0, 2300.0, 1000.0, 60.0);
        assert!(!w.covers(field));
        assert_eq!(w.scroll_to_clear(field, 16.0), 0.0);
        assert_eq!(w.inset_for(full_window()), 0.0);
    }

    #[test]
    fn alignment_rounds_outward_so_no_seam_is_left() {
        // Nearest rounding can land the inset a fraction short, leaving a
        // hairline of keyboard visible over the field.
        let r = Rect::new(0.4, 1499.6, 1079.2, 900.9).aligned();
        assert_eq!(r.y, 1499.0);
        assert!(r.bottom() >= 1499.6 + 900.9, "covers the original");
    }
}
