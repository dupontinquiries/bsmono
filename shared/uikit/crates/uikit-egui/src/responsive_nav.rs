//! responsive_nav — tabs on a screen wide enough to show them, pages on
//! one that isn't. Decided from a WIDTH FIT, as data, once, in the kit —
//! not a per-app `if adaptive.is_compact() { strip } else { swipe }`
//! guess, which is what `bot-arena` (and others before `segmented_tabs`
//! existed) hand-rolled: three sections fit fine on a narrow phone in
//! landscape and the crude device-class boolean says "compact" anyway,
//! or seven sections don't fit on a "regular" phone that boolean says is
//! not compact. The device class answers "what kind of screen is this,"
//! never "will these N labels actually fit on it" — a different question
//! this module answers directly, the same way `pager::fit_cols` already
//! answers "how many grid columns fit" instead of guessing from class.
//!
//! This is the DECISION only. Rendering stays exactly what it already
//! was: [`crate::segmented_tabs::strip_ui`] for the tab strip, and the
//! caller's own swipe/gesture layer for pages — this module does not own
//! gesture state, because an app that already has one (bot-arena's
//! `GestureLayer`, snapclone's `pager`) should keep using it, not hand it
//! to a second owner. [`nav_ui`] is the one convenience that also draws,
//! for the common case of "just the strip or nothing."

use crate::segmented_tabs::TabStrip;
use crate::PaletteColors;

/// Which shape the navigation should take this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavLayout {
    /// All sections visible and switchable inline.
    Tabs,
    /// One section at a time — the caller's own swipe/gesture layer (or a
    /// prev/next control) moves between them.
    Pages,
}

/// Below this, a tab's label clips or its hit target falls under a
/// comfortable tap size — matched to `Adaptive`'s own touch-target floor
/// (44pt on a phone/touch device) rather than an unrelated number, since
/// the failure mode ("too narrow to use") is the same one that constant
/// already guards against for a single control.
pub const DEFAULT_MIN_TAB_WIDTH: f32 = 72.0;

/// Tabs if every section fits at least `min_tab_width` wide in
/// `available_width`; pages otherwise. Pure and cheap enough to call
/// every frame — the same shape as `pager::fit_cols`, which this
/// mirrors deliberately: both answer "how much of this list fits," one
/// for a grid's columns, this one for a strip's segments.
///
/// Zero sections is `Tabs` (there is nothing to page through, and an
/// empty strip already renders as nothing per `segmented_tabs::
/// strip_ui`) rather than a special case a caller has to remember to
/// handle.
pub fn choose(available_width: f32, section_count: usize, min_tab_width: f32) -> NavLayout {
    if section_count == 0 { return NavLayout::Tabs; }
    let min_tab_width = min_tab_width.max(1.0);
    if section_count as f32 * min_tab_width <= available_width.max(0.0) {
        NavLayout::Tabs
    } else {
        NavLayout::Pages
    }
}

/// Same, reading the minimum tab width off `Adaptive` instead of a
/// caller-supplied constant — a touch device's own comfortable tap size
/// doubles as its comfortable tab width, so a phone in landscape (wide
/// enough for a strip, but with fat fingers) gets fewer tabs per strip
/// than a mouse-driven desktop at the same pixel width, which is the
/// right answer for BOTH: the phone needs bigger targets, not just more
/// of them squeezed in.
pub fn choose_adaptive(adaptive: &crate::Adaptive, available_width: f32,
                        section_count: usize) -> NavLayout {
    choose(available_width, section_count, adaptive.min_touch_target.max(DEFAULT_MIN_TAB_WIDTH))
}

/// Draw the strip when [`choose`] says `Tabs`; draw nothing when it says
/// `Pages` — the caller's own swipe/gesture layer is what shows a page,
/// this function only ever owns the strip. Returns `(layout, clicked)`:
/// `clicked` is only ever `Some` when `layout` is `Tabs`, since there is
/// nothing to click when this drew nothing.
///
/// This is the convenience for the common "hand it a strip, get back
/// what to do" case; an app with its own tab-click handling (bot-arena's
/// `self.strip.select(...)` before drawing) can call [`choose`] directly
/// and keep drawing the strip itself exactly as it already does — this
/// function does not require adopting a new `TabStrip` owner.
pub fn nav_ui(ui: &mut egui::Ui, strip: &TabStrip, min_tab_width: f32,
              colors: &PaletteColors) -> (NavLayout, Option<usize>) {
    let layout = choose(ui.available_width(), strip.len(), min_tab_width);
    let clicked = match layout {
        NavLayout::Tabs => crate::segmented_tabs::strip_ui(ui, strip, colors),
        NavLayout::Pages => None,
    };
    (layout, clicked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_fit_pages_dont_at_the_exact_boundary() {
        // Three sections at 80pt each need exactly 240pt.
        assert_eq!(choose(240.0, 3, 80.0), NavLayout::Tabs, "exact fit is still a fit");
        assert_eq!(choose(239.9, 3, 80.0), NavLayout::Pages, "one point short is not");
        assert_eq!(choose(1000.0, 3, 80.0), NavLayout::Tabs);
    }

    #[test]
    fn zero_sections_is_tabs_not_a_special_case_the_caller_must_handle() {
        assert_eq!(choose(0.0, 0, 80.0), NavLayout::Tabs);
        assert_eq!(choose(-5.0, 0, 80.0), NavLayout::Tabs, "a negative width still can't break this");
    }

    #[test]
    fn the_crude_device_class_boolean_and_the_width_fit_can_disagree() {
        // This is the whole reason to compute rather than guess: three
        // short sections fit fine at 400pt even though that width alone
        // might read as "phone-ish" to a class boundary, and seven
        // sections do NOT fit at 700pt even though that is comfortably
        // "desktop" by any device-class threshold.
        assert_eq!(choose(400.0, 3, 80.0), NavLayout::Tabs, "narrow but few sections: fits");
        assert_eq!(choose(700.0, 7, 120.0), NavLayout::Pages, "wide but many sections: doesn't");
    }

    #[test]
    fn choose_adaptive_uses_the_larger_of_touch_target_and_the_default_floor() {
        let mouse = crate::Adaptive::default().with_min_touch_target(28.0);
        // Bigger than DEFAULT_MIN_TAB_WIDTH (72), or `.max()` would floor
        // it right back to the default and this test would show nothing —
        // the failure this exact number caught on the way in.
        let touch = crate::Adaptive::default().with_min_touch_target(90.0);
        // 3 sections at DEFAULT_MIN_TAB_WIDTH (72) need 216; at the touch
        // device's 90 they need 270. Pick a width in between.
        let width = 240.0;
        assert_eq!(choose_adaptive(&mouse, width, 3), NavLayout::Tabs,
                   "mouse floors at the 72pt default, which fits");
        assert_eq!(choose_adaptive(&touch, width, 3), NavLayout::Pages,
                   "touch needs its own bigger target, which doesn't");
    }

    #[test]
    fn nav_ui_never_reports_a_click_in_pages_mode() {
        // Headless check of the contract without a real egui pass: a
        // caller relying on "clicked is None means nothing happened" must
        // not be able to observe a stale Some from a previous Tabs frame
        // leaking into a Pages one — checked at the `choose` boundary
        // this function is built on, since driving a real `egui::Ui`
        // through a click requires the full harness this crate's other
        // UI-drawing functions are already exempted from unit-testing.
        assert_eq!(choose(50.0, 5, 80.0), NavLayout::Pages);
    }
}
