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

/// Points of breathing room left on EACH side of a dialog, so a clamped window reads as a window
/// rather than as content jammed edge to edge.
pub const DIALOG_MARGIN_PT: f32 = 12.0;

/// The width a dialog should ask for, given the screen it has to live on.
///
/// Same failure as the toolbars, different widget: `Window::fixed_size([560, 0])` and
/// `default_size([660, 480])` are desktop numbers, and on a phone (about 380pt wide) they put the
/// window's own edges off-screen — controls near the left and right rims become unreachable, which
/// is the "UI out of app bounds" the operator sees. egui's `constrain` keeps a window's POSITION
/// inside the viewport but does not shrink a size the caller insisted on.
///
/// The rule: never wider than the screen minus a margin on each side, never wider than desired
/// (a phone-sized dialog must not stretch across a desktop), and never below a floor — a window
/// squeezed to 40pt is not more usable than one that overflows, so on an absurdly small viewport
/// the dialog stops shrinking and its own scrolling takes over.
pub fn dialog_width(screen_pt: f32, desired_pt: f32) -> f32 {
    const FLOOR_PT: f32 = 240.0;
    let usable = screen_pt - 2.0 * DIALOG_MARGIN_PT;
    desired_pt.min(usable).max(FLOOR_PT.min(desired_pt))
}

/// One fan row's height. Mirrors `holdmenu::ROW_PT`, pinned equal by test.
pub const FAN_ROW_PT: f32 = 44.0;

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

/// How many secondary controls it is reasonable to shed into a hold-to-fan button.
///
/// `row_mode` decides WHETHER to shed; this decides how much shedding the destination can carry.
/// The reason the two are separate: a tap-to-open overflow menu costs a second tap per control, so
/// a large shed set is a punishment, while `holdmenu`'s press-slide-release costs ONE gesture
/// however many options are in the fan. What limits the fan is not taps but REACH and MEMORY —
/// rows stack upward at 44pt each, and a fan taller than the thumb's arc (or longer than a person
/// will read while holding a finger down) is a list that wants a real menu instead.
///
/// Six is the arc: ~264pt of stack, about the reach of a thumb from the bottom bar of a phone held
/// one-handed, and short enough to scan without releasing. Beyond that, `row_mode`'s Overflow (a
/// real, tappable, scrollable menu) is the honest answer, and this returns None to say so.
pub fn fan_capacity(available_height_pt: f32) -> Option<usize> {
    const MAX_ROWS: usize = 6;
    // FAN_ROW_PT rather than holdmenu::ROW_PT: this rule is std-only and must survive a headless
    // build, while holdmenu is egui drawing. The two are pinned equal by a test below — a silent
    // divergence would make this function's answer wrong rather than merely stale.
    let rows = (available_height_pt / FAN_ROW_PT).floor().max(0.0) as usize;
    let n = rows.min(MAX_ROWS);
    if n >= 2 { Some(n) } else { None }
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
    #[test]
    fn a_desktop_dialog_keeps_its_desired_width() {
        assert_eq!(dialog_width(1440.0, 560.0), 560.0, "a wide screen must not stretch the dialog");
    }

    #[test]
    fn a_PHONE_shrinks_the_dialog_inside_its_own_edges() {
        // The actual bug: 560pt of wizard on a 380pt phone put both rims off-screen.
        let w = dialog_width(380.0, 560.0);
        assert!(w <= 380.0 - 2.0 * DIALOG_MARGIN_PT, "got {w}");
        assert!(w > 300.0, "shrunk further than it needed to: {w}");
    }

    #[test]
    fn an_ABSURD_viewport_stops_at_the_floor_rather_than_vanishing() {
        // Below the floor the dialog keeps a usable width and scrolls its own content; a 40pt
        // window is not a better answer than an overflowing one.
        assert_eq!(dialog_width(100.0, 560.0), 240.0);
        // ...but a dialog that only WANTED 180 still gets 180 — the floor must not inflate it.
        assert_eq!(dialog_width(100.0, 180.0), 180.0);
    }

    #[cfg(feature = "gui")]
    #[test]
    fn the_fan_row_height_matches_the_widget_that_draws_it() {
        assert_eq!(FAN_ROW_PT, crate::holdmenu::ROW_PT,
                   "the layout rule and the drawing must agree on a row's height");
    }

    #[test]
    fn a_fan_carries_up_to_the_thumbs_reach_and_then_defers_to_a_real_menu() {
        // Six rows is ~264pt: about a thumb's arc from a phone's bottom bar, and short enough to
        // scan without releasing. A taller list is not a fan, it is a menu.
        assert_eq!(fan_capacity(1000.0), Some(6));
        assert_eq!(fan_capacity(FAN_ROW_PT * 3.5), Some(3));
        // Not enough room for two rows: a "fan" of one is just a button that hides itself.
        assert_eq!(fan_capacity(FAN_ROW_PT), None);
        assert_eq!(fan_capacity(0.0), None);
    }

}
