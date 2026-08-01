//! mobilelayout.rs — why controls overlap on a phone, and the width rules that stop it.
//!
//! ## The cause
//!
//! `ui.horizontal()` in egui **does not wrap**. Controls past the right edge are drawn anyway, on top
//! of each other. On a desktop the bar never reaches the edge so nothing looks wrong; on a phone it
//! reaches it immediately, and the failure is invisible in development.
//!
//! `chat.rs` has 17 unwrapped horizontal rows against 10 wrapped ones, so this is a systematic gap
//! rather than one bad row.
//!
//! ## Why wrapping alone is not the answer
//!
//! `horizontal_wrapped` stops the overlap and produces a four-line bar of tiny buttons, which is not
//! usable either. The bar needs to lose controls, not stack them — so below a width threshold the
//! secondary ones move behind an overflow menu and the primary ones keep their place.
//!
//! Deciding by AVAILABLE WIDTH rather than by a platform flag matters: a narrow desktop window and a
//! phone have the same problem, and a split-screen tablet is neither.
//!
//! ## Touch targets
//!
//! A finger is about 9 mm. Buttons sized for a mouse cursor are hit-or-miss on glass, and two adjacent
//! small buttons are worse than one — a mis-hit does something rather than nothing. So on touch the
//! minimum height rises and the spacing rises with it, which also reduces how many fit and feeds back
//! into the overflow decision.

/// Points below which a row must shed controls rather than wrap them.
///
/// 380 is a little above a phone's portrait width in logical points, so the rule fires before the
/// overlap does rather than at the moment of it.
pub const NARROW_PT: f32 = 380.0;

/// Minimum touch target, in points. Below this a finger mis-hits adjacent controls.
pub const TOUCH_MIN_PT: f32 = 44.0;
/// Minimum for a pointer, where the cursor is exact.
pub const POINTER_MIN_PT: f32 = 22.0;

/// How a row should lay itself out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowMode {
    /// Everything fits on one line.
    Inline,
    /// Too many for one line but each still reachable: wrap.
    Wrap,
    /// Not enough width for the secondary controls at a usable size: show primaries, hide the rest.
    Overflow,
}

/// Decide how to lay out a row.
///
/// `n_primary` are controls that must always be visible; `n_secondary` may move behind a menu.
pub fn row_mode(available_pt: f32, n_primary: usize, n_secondary: usize, touch: bool) -> RowMode {
    let unit = if touch { TOUCH_MIN_PT } else { POINTER_MIN_PT } + 8.0;   // control plus spacing
    let need_all = (n_primary + n_secondary) as f32 * unit;
    if need_all <= available_pt { return RowMode::Inline; }
    // Wrapping is only kind when the controls are reachable once wrapped. On a genuinely narrow screen
    // it produces a four-line bar of tiny buttons, which is not better than an overflow menu.
    if available_pt >= NARROW_PT { return RowMode::Wrap; }
    // Primaries plus the overflow button itself must still fit, or hiding things has not helped.
    let need_primary = (n_primary + 1) as f32 * unit;
    if need_primary <= available_pt { RowMode::Overflow } else { RowMode::Wrap }
}

/// The minimum control size for the current input.
pub fn min_control_pt(touch: bool) -> f32 { if touch { TOUCH_MIN_PT } else { POINTER_MIN_PT } }

/// How many controls of the current size fit.
pub fn fits(available_pt: f32, touch: bool) -> usize {
    let unit = min_control_pt(touch) + 8.0;
    if unit <= 0.0 { return 0 }
    (available_pt / unit).floor().max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_width_row_with_many_controls_OVERFLOWS_rather_than_wrapping() {
        // The composer bar's actual shape: two controls that must stay plus five that need not. On a
        // 360pt phone, wrapping produces a four-line bar of tiny buttons — not better than a menu.
        assert_eq!(row_mode(360.0, 2, 5, true), RowMode::Overflow);
    }

    #[test]
    fn a_desktop_width_row_stays_inline() {
        // The reason this bug is invisible in development: the bar never reaches the edge.
        assert_eq!(row_mode(1200.0, 2, 5, false), RowMode::Inline);
    }

    #[test]
    fn a_medium_width_wraps_instead_of_hiding_things() {
        // A narrow desktop window or a split-screen tablet: wide enough that a wrapped row is still
        // usable, so hiding controls would take something away for no gain.
        //
        // The first version of this test asserted Wrap for 7 pointer-sized controls at 500pt, and the
        // code was right to say Inline — seven 30pt controls need 210pt. Corrected to a row that
        // genuinely cannot fit rather than one I assumed could not.
        // A tablet in touch mode: 10 controls at 52pt need 520pt and only 500 is available, but 500 is
        // wide enough that a wrapped row stays reachable.
        assert_eq!(row_mode(500.0, 2, 8, true), RowMode::Wrap);
        assert_eq!(row_mode(500.0, 2, 5, false), RowMode::Inline, "seven fit; do not change the layout");
    }

    #[test]
    fn touch_targets_reduce_how_many_fit() {
        // A finger is about 9mm; a cursor is exact. The same width holds fewer touch controls, and that
        // feeds back into the overflow decision rather than being a separate styling concern.
        assert!(fits(360.0, true) < fits(360.0, false));
        assert!(min_control_pt(true) > min_control_pt(false));
    }

    #[test]
    fn overflow_is_refused_when_even_the_primaries_would_not_fit() {
        // Hiding the secondaries has not helped if what remains still overlaps. Wrapping is then the
        // only honest answer — an overflow menu that itself overlaps is worse than a wrapped row.
        assert_eq!(row_mode(80.0, 4, 5, true), RowMode::Wrap);
    }

    #[test]
    fn the_overflow_BUTTON_is_counted_in_what_must_fit() {
        // Off-by-one that would reintroduce the overlap at exactly the boundary: the menu button takes
        // a slot too. 2 primaries + 1 menu at 52pt each needs 156pt.
        assert_eq!(row_mode(160.0, 2, 5, true), RowMode::Overflow);
        assert_eq!(row_mode(150.0, 2, 5, true), RowMode::Wrap, "one slot short: cannot overflow");
    }

    #[test]
    fn the_threshold_fires_BEFORE_the_overlap_rather_than_at_it() {
        // 380 is above a phone's portrait width, so the layout changes while there is still room —
        // a rule that triggers exactly when controls collide has already failed once.
        assert!(NARROW_PT > 360.0);
    }
}
