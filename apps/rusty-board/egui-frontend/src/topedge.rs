//! topedge — what an upward swipe off the top row means.
//!
//! `docs/single-hand-mode.md` §5 and §6 put two gestures on the same edge, and specifies
//! both precisely enough to implement without guessing:
//!
//! * **Short up-arc from a top-row key → a digit.** Starts on a top-row letter, stays within
//!   about one row-height above the keys, under ~80px of path, under ~250ms.
//! * **Long upward fling → minimize to the tray.** Terminates more than 1.5 row-heights
//!   above the top edge, or within 20px of the screen top, with a predominantly upward gross
//!   direction (`dy < -1.5·|dx|`).
//!
//! ## The tie-break is the interesting part
//!
//! The doc: *"If both digit and minimize conditions could match (short upward arc that
//! happens to also reach high), prefer digit (shorter distance travelled, lower threshold).
//! Minimize requires actual flight off the keyboard."*
//!
//! That ordering matters because the two live on the same edge and a fast flick can satisfy
//! both. Getting it backwards means a user reaching for `3` loses their keyboard — a
//! destructive outcome for a gesture they make constantly, versus a stray digit they can
//! backspace. [`classify`] checks digit first for exactly that reason.
//!
//! ## Column, not letter
//!
//! The doc again: *"column lookup uses the POSITION of the key in the top row at swipe
//! start, not the letter itself, so QWERTY and Dvorak both work without per-layout digit
//! tables."* So `q`→1 on QWERTY and `'`→1 on Dvorak fall out of the same code, and a layout
//! nobody has written yet gets digits for free.
//!
//! ## Pure
//!
//! Geometry in, meaning out. No egui, no clock — the caller passes the press and release
//! points, the elapsed time, and the board's metrics.

/// One row-height above the keys is the digit zone's ceiling (§5).
pub const DIGIT_ZONE_ROWS: f32 = 1.0;
/// Longer than this and it is a swipe, not an arc (§5).
pub const DIGIT_MAX_PATH: f32 = 80.0;
/// Slower than this and it is a drag, not a flick (§5).
pub const DIGIT_MAX_MS: u64 = 250;
/// A fling must end this many row-heights above the top edge (§6).
pub const MINIMIZE_ROWS: f32 = 1.5;
/// …or this close to the screen top (§6).
pub const MINIMIZE_SCREEN_TOP: f32 = 20.0;
/// A fling's gross direction must be this much more vertical than horizontal (§6).
pub const MINIMIZE_SLOPE: f32 = 1.5;

/// What an upward swipe off the top row turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopEdge {
    /// Emit this digit. Already the character, not the column index.
    Digit(char),
    /// Minimize to the tray.
    Minimize,
    /// Neither — the caller should treat it as an ordinary swipe.
    Neither,
}

/// The board metrics a classification needs.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// Y of the top of the top row, in the same space as the points.
    pub top_y: f32,
    /// One key's height.
    pub row_h: f32,
    /// Y of the top of the screen, for the 20px rule.
    pub screen_top_y: f32,
}

/// The digit for a column, 0-indexed from the left: `q`→1 … `p`→0.
///
/// Ten columns. A wider top row (a layout with eleven keys) gets no digit past the tenth
/// rather than a wrong one — there are only ten digits, and inventing an eleventh is worse
/// than declining.
pub fn digit_for_column(col: usize) -> Option<char> {
    const DIGITS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
    DIGITS.get(col).copied()
}

/// Classify an upward swipe.
///
/// `from_top_row_col` is `Some(column)` when the press landed on a top-row key, `None`
/// otherwise — the caller owns hit-testing, which is layout work this module deliberately
/// does not do.
pub fn classify(
    press: egui::Pos2,
    release: egui::Pos2,
    path_len: f32,
    elapsed_ms: u64,
    from_top_row_col: Option<usize>,
    m: Metrics,
) -> TopEdge {
    // DIGIT FIRST — the doc's tie-break. A user reaching for `3` who loses their keyboard
    // has suffered a destructive outcome for a gesture they make constantly; a stray digit
    // is one backspace.
    if let Some(col) = from_top_row_col {
        let above = press.y - release.y;
        let ceiling = m.top_y - DIGIT_ZONE_ROWS * m.row_h;
        let in_zone = release.y <= m.top_y && release.y >= ceiling;
        if above > 0.0 && in_zone && path_len < DIGIT_MAX_PATH && elapsed_ms < DIGIT_MAX_MS {
            if let Some(d) = digit_for_column(col) {
                return TopEdge::Digit(d);
            }
            // A column past the tenth: not a digit, and not a minimize either — the gesture
            // was a short arc, which the doc says is never a minimize.
            return TopEdge::Neither;
        }
    }

    // MINIMIZE: actual flight off the keyboard.
    let (dx, dy) = (release.x - press.x, release.y - press.y);
    let upward = dy < -MINIMIZE_SLOPE * dx.abs();
    let high_enough = release.y < m.top_y - MINIMIZE_ROWS * m.row_h
        || release.y - m.screen_top_y < MINIMIZE_SCREEN_TOP;
    if upward && high_enough {
        return TopEdge::Minimize;
    }
    TopEdge::Neither
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics { top_y: 100.0, row_h: 44.0, screen_top_y: 0.0 }
    }

    fn at(x: f32, y: f32) -> egui::Pos2 {
        egui::pos2(x, y)
    }

    #[test]
    fn a_short_arc_off_a_top_row_key_is_that_columns_digit() {
        let m = metrics();
        // Up from the third column, ending half a row-height above the keys.
        let got = classify(at(120.0, 120.0), at(124.0, 78.0), 45.0, 120, Some(2), m);
        assert_eq!(got, TopEdge::Digit('3'));
        // Column, not letter: the tenth column is 0, and the first is 1.
        assert_eq!(digit_for_column(0), Some('1'));
        assert_eq!(digit_for_column(9), Some('0'));
        // A layout with eleven top-row keys gets no digit past the tenth rather than a
        // wrong one.
        assert_eq!(digit_for_column(10), None);
        assert_eq!(
            classify(at(120.0, 120.0), at(124.0, 78.0), 45.0, 120, Some(10), m),
            TopEdge::Neither
        );
    }

    /// The doc's tie-break, and the reason for it: a user reaching for `3` who loses their
    /// keyboard has suffered a destructive outcome for a gesture they make constantly.
    #[test]
    fn a_short_arc_that_also_reaches_high_is_a_digit_not_a_minimize() {
        let m = metrics();
        // Ends well above the minimize threshold AND satisfies the digit rules… except the
        // digit zone's ceiling, which is what keeps them apart. Inside the zone: digit.
        let inside = classify(at(120.0, 120.0), at(121.0, 60.0), 62.0, 100, Some(4), m);
        assert_eq!(inside, TopEdge::Digit('5'));

        // The same flick with no top-row key under the press is a minimize, so it is the
        // ORIGIN that distinguishes them, not the speed.
        let no_key = classify(at(120.0, 120.0), at(121.0, 10.0), 112.0, 100, None, m);
        assert_eq!(no_key, TopEdge::Minimize);
    }

    /// "Minimize requires actual flight off the keyboard."
    #[test]
    fn a_fling_must_actually_leave_the_keyboard() {
        let m = metrics();
        // Ends 1.5 row-heights above the top edge: 100 - 66 = 34.
        assert_eq!(classify(at(200.0, 140.0), at(200.0, 30.0), 110.0, 90, None, m), TopEdge::Minimize);
        // Just short of it, and not near the screen top: nothing.
        assert_eq!(classify(at(200.0, 140.0), at(200.0, 40.0), 100.0, 90, None, m), TopEdge::Neither);
        // The 20px-from-screen-top rule catches a fling on a short screen.
        let tall = Metrics { top_y: 40.0, row_h: 44.0, screen_top_y: 0.0 };
        assert_eq!(classify(at(200.0, 80.0), at(200.0, 15.0), 65.0, 90, None, tall), TopEdge::Minimize);
    }

    /// `dy < -1.5·|dx|`: a mostly-sideways swipe that drifts up is a swipe, not a fling.
    #[test]
    fn a_sideways_swipe_is_not_a_minimize_however_far_it_goes() {
        let m = metrics();
        // Up 110, across 200: not predominantly upward.
        assert_eq!(classify(at(50.0, 140.0), at(250.0, 30.0), 230.0, 90, None, m), TopEdge::Neither);
        // Same height, much less sideways: a fling.
        assert_eq!(classify(at(50.0, 140.0), at(80.0, 30.0), 115.0, 90, None, m), TopEdge::Minimize);
        // Downward is never either.
        assert_eq!(classify(at(50.0, 30.0), at(55.0, 140.0), 110.0, 90, Some(1), m), TopEdge::Neither);
    }

    /// The digit is an ARC: too long, too slow, or too high and it stops being one.
    #[test]
    fn the_digit_thresholds_are_all_load_bearing() {
        let m = metrics();
        let ok = |path: f32, ms: u64, y: f32| classify(at(120.0, 120.0), at(122.0, y), path, ms, Some(0), m);
        assert_eq!(ok(45.0, 120, 80.0), TopEdge::Digit('1'), "the baseline should pass");
        assert_eq!(ok(DIGIT_MAX_PATH + 1.0, 120, 80.0), TopEdge::Neither, "too long a path");
        assert_eq!(ok(45.0, DIGIT_MAX_MS, 80.0), TopEdge::Neither, "too slow");
        // Above the zone's ceiling (100 - 44 = 56) it is no longer a digit arc.
        assert_eq!(ok(45.0, 120, 50.0), TopEdge::Neither, "past the ceiling");
        // Ending BELOW the top row is not an up-arc at all.
        assert_eq!(ok(45.0, 120, 110.0), TopEdge::Neither, "never left the keys");
    }
}
