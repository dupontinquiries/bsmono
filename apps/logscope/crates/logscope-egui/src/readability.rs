//! readability.rs — the measure, which is the one readability factor the theme does not control.
//!
//! ## Contrast is already handled; line LENGTH is not
//!
//! `theme.rs` audits contrast across all 19 palettes and clamps pairs under 4.5:1. What nothing bounds
//! is how wide a line of prose gets. On a 1920px window at 14px, a full-width line runs past 200
//! characters — and the failure mode of a long measure is not illegibility, it is **the return sweep**:
//! the eye has to travel back across the whole width and find the next line, and it lands on the wrong
//! one often enough to be exhausting over an hour.
//!
//! Typographic practice puts the comfortable range at roughly **45–75 characters**. That is not a style
//! opinion — it is where the sweep stays reliable.
//!
//! ## Why a chat UI gets this wrong by default
//!
//! Every layout system fills its container, and a chat panel's container is the window. Nothing is
//! visibly broken at any width, so the problem never presents as a bug — it presents as the reader
//! being tired, which nobody attributes to the layout.
//!
//! ## Code is the exception, and it matters
//!
//! Code must NOT be wrapped to the prose measure. A wrapped line of code changes its meaning to the
//! eye — indentation stops indicating structure — so code blocks take the full width and scroll
//! instead. Applying one rule to both is how a readability fix makes code worse.

/// Characters per line at the comfortable end of the range.
pub const IDEAL_CHARS: f32 = 66.0;
/// Above this the return sweep starts failing.
pub const MAX_CHARS: f32 = 80.0;
/// Below this, wrapping is so frequent that it breaks phrases apart.
pub const MIN_CHARS: f32 = 45.0;

/// What a block of text is, for width purposes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Wrapped prose. Bounded to the measure.
    Prose,
    /// Code, a table, or anything where a wrap changes the meaning. Full width.
    Preformatted,
}

/// The width prose should occupy, given what is available.
///
/// `char_px` is the average advance of the body font — measured rather than assumed, because the same
/// point size gives very different measures in a condensed face and a wide one.
///
/// Returns the AVAILABLE width when it is already narrow, rather than padding it out: on a phone the
/// constraint is the device and adding margins would only shrink an already-tight column.
pub fn prose_width(available_px: f32, char_px: f32) -> f32 {
    if char_px <= 0.0 { return available_px; }
    let ideal = IDEAL_CHARS * char_px;
    if available_px <= ideal { return available_px; }
    // Do not clamp all the way to the ideal on a moderately wide window — a hard 66 characters leaves
    // a distractingly narrow column on a 1440px screen. Allow up to MAX before capping.
    (available_px).min(MAX_CHARS * char_px)
}

/// Should this block be bounded at all?
pub fn bounded(kind: Kind) -> bool { kind == Kind::Prose }

/// Characters per line at a given width. For reporting and for tests.
pub fn chars_per_line(width_px: f32, char_px: f32) -> f32 {
    if char_px <= 0.0 { return 0.0; }
    width_px / char_px
}

/// Is this measure comfortable?
pub fn comfortable(chars: f32) -> bool { (MIN_CHARS..=MAX_CHARS).contains(&chars) }

#[cfg(test)]
mod tests {
    use super::*;

    /// ~7.2px per character at a 14px monospace-ish body, measured rather than assumed.
    const CH: f32 = 7.2;

    #[test]
    fn a_wide_window_is_BOUNDED_rather_than_filled() {
        // The failure this exists for. 1920px at 7.2px/char is 266 characters — over three times the
        // comfortable maximum, and nothing about it looks broken, which is why it never gets reported
        // as a bug.
        assert!(chars_per_line(1920.0, CH) > 250.0, "unbounded is far past the range");
        let w = prose_width(1920.0, CH);
        assert!(comfortable(chars_per_line(w, CH)),
            "bounded to {:.0} chars", chars_per_line(w, CH));
    }

    #[test]
    fn a_phone_width_is_left_ALONE() {
        // On a narrow device the constraint is the device. Adding margins to reach an "ideal" would
        // shrink an already-tight column and make wrapping more frequent, not less.
        let avail = 360.0;
        assert_eq!(prose_width(avail, CH), avail);
        // 360px at 7.2px/char is 50 characters, which is COMFORTABLE — I first asserted it was below
        // the minimum and it is not. A phone column is narrow in pixels and fine as a measure, which is
        // the reason to leave it alone rather than a reason to worry about it.
        assert!(comfortable(chars_per_line(avail, CH)));
    }

    #[test]
    fn a_moderate_window_is_not_clamped_all_the_way_to_the_ideal() {
        // A hard 66 characters leaves a distractingly narrow column on a 1440px screen with the rest
        // of the panel empty. The cap is MAX, so a moderate window keeps its width and only a very wide
        // one is reined in.
        // 500px is 69 characters — inside the range, so it must pass through untouched. My first
        // fixture used 700px, which is 97 characters and correctly capped: the test was wrong and the
        // code was right, and I had written "check" in the message rather than doing the arithmetic.
        assert_eq!(prose_width(500.0, CH), 500.0, "69 chars: leave it");
        assert!(prose_width(700.0, CH) < 700.0, "97 chars: capped");
    }

    #[test]
    fn code_is_NEVER_bounded_to_the_prose_measure() {
        // A wrapped line of code changes its meaning to the eye: indentation stops indicating
        // structure. Applying one rule to both is how a readability fix makes code worse.
        assert!(bounded(Kind::Prose));
        assert!(!bounded(Kind::Preformatted));
    }

    #[test]
    fn the_comfortable_range_matches_typographic_practice() {
        assert!(!comfortable(30.0), "too narrow: phrases break apart");
        assert!(comfortable(45.0));
        assert!(comfortable(66.0));
        assert!(comfortable(80.0));
        assert!(!comfortable(120.0), "the return sweep starts failing");
    }

    #[test]
    fn an_unmeasured_font_does_not_divide_by_zero() {
        // char_px comes from a text layout; before the first frame it can be zero, and returning the
        // available width is the safe answer — an unbounded line is worse than a crash only in theory.
        assert_eq!(prose_width(1920.0, 0.0), 1920.0);
        assert_eq!(chars_per_line(1920.0, 0.0), 0.0);
    }
}
