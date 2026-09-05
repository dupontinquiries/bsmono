//! command_bar — the palette surface, as a thing the kit owns.
//!
//! snapclone's palette drew a translucent card over the page with a 45%
//! scrim. On a dark theme that scrim is invisible — dimming near-black by
//! 45% gives you near-black — so the page behind stayed at full contrast and
//! the suggestions read as loose text floating over a settings form. The fix
//! is not more scrim. It is deciding what the surface IS.
//!
//! Two answers, both here, because they suit different moments:
//!
//! * [`Mode::Bar`] — a slim strip at the bottom edge, the best match
//!   completed INLINE as ghost text, the rest as a short list above it. It
//!   never covers the page, so the page never has to be suppressed.
//! * [`Mode::Pane`] — a full overlay with a heavy scrim. Nothing behind it
//!   competes because nothing behind it is readable.
//!
//! The kit holds the surface and the completion logic; the app supplies the
//! matches and decides what one does. That split is the reusable part —
//! snapclone, logscope and gitmind each have a palette and each hand-rolled
//! one.

/// How the palette presents itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Slim strip at the bottom; the page stays readable behind it.
    Bar,
    /// Full overlay; the page is deliberately suppressed.
    Pane,
}

/// Scrim opacity for [`Mode::Pane`]. High enough that text behind it falls
/// out of reading contrast on a LIGHT palette too — a scrim that only works
/// on one theme is the bug this module exists to stop repeating.
pub const SCRIM_PANE: f32 = 0.78;
/// The bar draws no scrim. Covering nothing is the point.
pub const SCRIM_BAR: f32 = 0.0;

/// Rows the surface shows. Past this a palette stops being a shortcut and
/// becomes a list you read.
pub const MAX_ROWS: usize = 6;
/// Row height, at the touch floor so a thumb can hit one.
pub const ROW_H: f32 = 34.0;
/// The input's own height.
pub const INPUT_H: f32 = 34.0;

/// Pick a mode for the space available.
///
/// A bar is only worth it while the page behind stays useful. Once the bar
/// and its rows would cover half the surface, the pane is the more honest
/// answer: it admits it has taken the screen instead of half-covering it.
pub fn mode_for(surface_h: f32, rows: usize) -> Mode {
    let needed = INPUT_H + rows.min(MAX_ROWS) as f32 * ROW_H + 24.0;
    if needed > surface_h * 0.5 { Mode::Pane } else { Mode::Bar }
}

/// Height the surface wants for `rows` matches.
pub fn wanted_height(rows: usize) -> f32 {
    INPUT_H + rows.min(MAX_ROWS) as f32 * ROW_H + 16.0
}

/// The part of `best` that would be typed next — the ghost suffix shown
/// inline after the cursor.
///
/// `None` when the input is empty (ghosting a whole suggestion into an empty
/// box makes it look pre-filled), when the match adds nothing, or when it is
/// not a prefix at all. The prefix compare ignores case; the suffix comes
/// back in the CANDIDATE's case, because that is what accepting inserts.
pub fn ghost<'a>(input: &str, best: &'a str) -> Option<&'a str> {
    let t = input.trim_start();
    if t.is_empty() || best.len() <= t.len() {
        return None;
    }
    let (head, tail) = best.split_at(t.len());
    head.eq_ignore_ascii_case(t).then_some(tail)
}

/// What the input becomes when the ghost is accepted. What the user typed is
/// preserved verbatim, including its case — rewriting "FL" to "fl" under the
/// cursor is what makes inline completion feel like it is fighting you.
pub fn accept(input: &str, best: &str) -> String {
    match ghost(input, best) {
        Some(rest) => format!("{input}{rest}"),
        None => input.to_string(),
    }
}

/// Move a selection by `delta` over `n` rows, clamped rather than wrapped.
/// The list is short and ordered by score, so wrapping from the last row
/// back to the best one reads as having lost the selection.
pub fn move_cursor(cursor: usize, n: usize, delta: i32) -> usize {
    if n == 0 { return 0; }
    (cursor as i64 + delta as i64).clamp(0, n as i64 - 1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ghost_completes_only_what_the_input_actually_prefixes() {
        assert_eq!(ghost("fl", "flip camera"), Some("ip camera"));
        assert_eq!(ghost("FL", "flip camera"), Some("ip camera"), "prefix ignores case");
        assert_eq!(ghost("", "flip camera"), None, "an empty box must not look pre-filled");
        assert_eq!(ghost("  ", "flip camera"), None);
        assert_eq!(ghost("xyz", "flip camera"), None, "not a prefix");
        assert_eq!(ghost("flip camera", "flip camera"), None, "nothing left to add");
        assert_eq!(ghost("flip camera and more", "flip camera"), None, "input is longer");
    }

    #[test]
    fn accepting_keeps_what_the_user_typed_verbatim() {
        assert_eq!(accept("FL", "flip camera"), "FLip camera");
        assert_eq!(accept("fl", "flip camera"), "flip camera");
        assert_eq!(accept("xyz", "flip camera"), "xyz", "no match, no change");
        assert_eq!(accept("", "flip camera"), "");
    }

    #[test]
    fn a_short_surface_gets_the_pane_and_a_tall_one_gets_the_bar() {
        assert_eq!(mode_for(320.0, 5), Mode::Pane);
        assert_eq!(mode_for(900.0, 5), Mode::Bar);
        assert_eq!(mode_for(320.0, 0), Mode::Bar, "nothing to show keeps the page");
        assert_eq!(mode_for(400.0, 6), Mode::Pane, "the rows decide, not height alone");
    }

    #[test]
    fn the_surface_never_asks_for_more_rows_than_it_shows() {
        let capped = wanted_height(MAX_ROWS);
        assert_eq!(wanted_height(MAX_ROWS + 40), capped, "extra matches do not grow it");
        assert!(wanted_height(0) >= INPUT_H, "the input still needs room");
        assert!(wanted_height(3) < capped);
    }

    #[test]
    fn the_cursor_clamps_rather_than_wrapping_past_the_best_match() {
        assert_eq!(move_cursor(0, 5, -1), 0);
        assert_eq!(move_cursor(4, 5, 1), 4);
        assert_eq!(move_cursor(2, 5, -1), 1);
        assert_eq!(move_cursor(2, 5, 3), 4);
        assert_eq!(move_cursor(3, 0, 1), 0, "no rows is not a panic");
    }

    #[test]
    fn the_pane_scrim_is_strong_enough_to_matter_on_a_dark_theme() {
        // The bug this replaces: 0.45 over a near-black page dims near-black
        // to near-black, and the page behind stays fully legible.
        assert!(SCRIM_PANE >= 0.7, "a weak scrim is the fault being fixed");
        assert_eq!(SCRIM_BAR, 0.0, "the bar covers nothing on purpose");
    }
}
