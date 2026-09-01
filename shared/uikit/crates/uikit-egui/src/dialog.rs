//! Modal/palette layout that fits a phone, and list selection that stays in
//! bounds.
//!
//! Both halves exist because both were re-implemented per app and got it
//! wrong in different places. [`dialog_width`] had three near-identical
//! copies (logscope's `mobilelayout`, studio's `App::dialog_width`, and by
//! omission tomescope, which hard-coded `set_min_width(400.0)` and simply
//! overflowed a narrow screen). [`Selection`] existed in one app and was
//! missing from the rest, so their palettes could not be driven from the
//! keyboard at all.

/// A dialog's width, given the space it actually has.
///
/// `screen_pt` should come from `ctx.content_rect().width()` — NOT
/// `screen_rect()`, which on a phone includes the area under the system
/// bars and yields a dialog wider than the usable surface.
///
/// The floor is itself clamped by `desired`: a dialog that only wants
/// 180pt should not be forced out to 240.
pub fn dialog_width(screen_pt: f32, desired_pt: f32) -> f32 {
    const MARGIN_PT: f32 = 12.0;
    const FLOOR_PT: f32 = 240.0;
    let usable = screen_pt - 2.0 * MARGIN_PT;
    desired_pt.min(usable).max(FLOOR_PT.min(desired_pt))
}

/// The maximum height a palette-style list should take before scrolling,
/// leaving room for the query box and the window chrome above it.
///
/// Without a cap the window grows past the bottom of a phone screen and the
/// last entries become unreachable — there is nothing to scroll, because the
/// window itself is what overflowed.
pub fn list_max_height(screen_pt: f32) -> f32 {
    (screen_pt * 0.55).clamp(120.0, 420.0)
}

/// Keyboard selection over a list of `len` items.
///
/// Deliberately a tiny value type rather than a widget: the apps that need
/// it all draw their rows differently, and the part they kept getting wrong
/// is the arithmetic, not the painting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub index: usize,
}

impl Selection {
    /// Move by `delta`, clamped to `[0, len)`. Returns whether it moved, so
    /// the caller knows to request `scroll_to_me` only on an actual keyboard
    /// move — scrolling every frame would fight the user's own scrolling.
    pub fn step(&mut self, delta: isize, len: usize) -> bool {
        if len == 0 {
            let moved = self.index != 0;
            self.index = 0;
            return moved;
        }
        let max = len - 1;
        // Saturating rather than wrapping. Wrapping from the last entry back
        // to the first reads as a glitch when you are holding Down to reach
        // the bottom of a long list — you arrive, then silently restart.
        let next = if delta < 0 {
            self.index.saturating_sub(delta.unsigned_abs())
        } else {
            (self.index + delta as usize).min(max)
        };
        let moved = next != self.index;
        self.index = next;
        moved
    }

    /// Clamp after the list changes under it — typing into the query box
    /// shortens the list, and an index left past the end selects nothing,
    /// so Enter appears to do nothing at all.
    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.index = 0;
        } else if self.index >= len {
            self.index = len - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dialog_never_exceeds_the_usable_width() {
        // 360pt phone: a 420-wide palette must come down, not overflow.
        let w = dialog_width(360.0, 420.0);
        assert!(w <= 360.0 - 24.0, "got {w}");
    }

    #[test]
    fn a_narrow_dialog_is_not_inflated_to_the_floor() {
        assert_eq!(dialog_width(1200.0, 180.0), 180.0);
    }

    #[test]
    fn a_wide_screen_gets_what_it_asked_for() {
        assert_eq!(dialog_width(1600.0, 420.0), 420.0);
    }

    #[test]
    fn selection_stops_at_the_ends_instead_of_wrapping() {
        let mut s = Selection::default();
        assert!(!s.step(-1, 5), "already at the top; nothing to do");
        assert_eq!(s.index, 0);
        for _ in 0..10 {
            s.step(1, 5);
        }
        assert_eq!(s.index, 4, "clamped at the last entry, not wrapped to 0");
        assert!(!s.step(1, 5), "no move reported at the end");
    }

    #[test]
    fn selection_reports_movement_so_scroll_to_me_only_fires_on_a_keypress() {
        // The bug this guards: calling scroll_to_me unconditionally pins the
        // view to the selection every frame, so the list cannot be scrolled
        // by hand at all.
        let mut s = Selection::default();
        assert!(s.step(1, 3));
        assert!(!s.step(0, 3));
    }

    #[test]
    fn an_index_past_a_shrinking_list_is_pulled_back() {
        // Type another character, the filtered list shrinks, and an index
        // left past the end means Enter silently does nothing.
        let mut s = Selection { index: 7 };
        s.clamp(3);
        assert_eq!(s.index, 2);
        s.clamp(0);
        assert_eq!(s.index, 0);
    }

    #[test]
    fn an_empty_list_is_not_a_panic() {
        let mut s = Selection { index: 4 };
        assert!(s.step(1, 0) || s.index == 0);
        assert_eq!(s.index, 0);
    }

    #[test]
    fn the_list_cap_leaves_room_on_a_phone_and_does_not_run_away_on_a_desktop() {
        assert!(list_max_height(640.0) < 640.0);
        assert_eq!(list_max_height(4000.0), 420.0);
        assert_eq!(list_max_height(100.0), 120.0);
    }
}
