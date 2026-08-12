//! width — display width for the caret math, per TODO.md's "CJK/double-width:
//! caret x assumes monospace `col * char_w`; wide glyphs need galley-measured
//! x." A full shaping engine is out of scope (egui does none either — a
//! stated limit, not a gap this module tries to close); what this DOES fix
//! is the two places aperture's own arithmetic, not egui's, assumed every
//! character occupies one cell: the caret's rendered x position, and the
//! reverse mapping from a mouse click's x pixel back to a character.
//!
//! Scope, stated honestly: `display_width` classifies the common-case wide
//! ranges (CJK ideographs, Hiragana/Katakana, Hangul, fullwidth forms) that
//! terminal and monospace-font convention renders at 2 cells — the same
//! table shape `wcwidth` implementations use for the BMP. It is not the full
//! Unicode East Asian Width database (astral-plane CJK extensions, emoji
//! width, combining-mark zero-width) — those are real gaps, left for when
//! someone hits them, not silently pretended away.

/// 1 for ordinary glyphs, 2 for characters monospace convention renders at
/// double width. Pure, total (every `char` has an answer), no allocation.
pub fn display_width(c: char) -> usize {
    let cp = c as u32;
    let wide = matches!(cp,
        0x1100..=0x115F   // Hangul Jamo
        | 0x2E80..=0x303E // CJK Radicals, Kangxi, CJK symbols/punctuation
        | 0x3041..=0x33FF // Hiragana .. CJK compatibility
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xA960..=0xA97F // Hangul Jamo Extended-A
        | 0xAC00..=0xD7A3 // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFF01..=0xFF60 // Fullwidth forms
        | 0xFFE0..=0xFFE6 // Fullwidth signs
    );
    if wide { 2 } else { 1 }
}

/// The visual width (in half-width cells) of the first `char_col` characters
/// of `line` — what x pixel (in `char_w` units) the caret sits at.
pub fn char_col_to_visual(line: &str, char_col: usize) -> usize {
    line.chars().take(char_col).map(display_width).sum()
}

/// Like `char_col_to_visual`, but for callers that let `char_col` run past
/// the line's actual length on purpose — full-line selection highlights
/// have always extended one extra cell past end-of-line as a visual "the
/// newline is part of the selection" cue. `char_col_to_visual` alone would
/// silently drop that extension (nothing to sum past the last real char);
/// this adds it back, one narrow cell per char of overrun, so switching a
/// caller from raw char-count math to width-aware math doesn't quietly
/// narrow a highlight rect that used to reach past the line's end.
pub fn char_col_to_visual_clamped(line: &str, char_col: usize) -> usize {
    let len = line.chars().count();
    let base = char_col_to_visual(line, char_col.min(len));
    base + char_col.saturating_sub(len)
}

/// The inverse: given a target visual column (a mouse click's x pixel,
/// already divided by `char_w`), the character-offset it lands on/nearest.
/// Rounds to whichever character boundary is closer when the click falls
/// inside a wide glyph's second cell — the same "nearest" convention
/// `caret_from_click`'s pre-existing `+ 0.5` rounding used for narrow text,
/// so this is a strict generalization, not a behavior change for ASCII.
pub fn visual_to_char_col(line: &str, target_visual: usize) -> usize {
    let mut visual = 0usize;
    for (i, c) in line.chars().enumerate() {
        let w = display_width(c);
        if target_visual < visual + w {
            // Inside this glyph's cell(s): land before it, unless the click
            // was past its own midpoint (only possible for wide glyphs).
            return if w == 2 && target_visual == visual + 1 { i + 1 } else { i };
        }
        visual += w;
    }
    line.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_stays_single_width() {
        for c in ['a', 'Z', '0', ' ', '{', '_'] {
            assert_eq!(display_width(c), 1, "{c:?}");
        }
    }

    #[test]
    fn common_cjk_ranges_are_double_width() {
        // Han, Hiragana, Katakana, Hangul, fullwidth Latin — one from each.
        for c in ['漢', 'ひ', 'カ', '한', 'Ａ'] {
            assert_eq!(display_width(c), 2, "{c:?}");
        }
    }

    #[test]
    fn char_col_to_visual_sums_mixed_width_text() {
        // "a漢b" — 1 + 2 + 1 = 4 visual cells over 3 chars.
        let line = "a漢b";
        assert_eq!(char_col_to_visual(line, 0), 0);
        assert_eq!(char_col_to_visual(line, 1), 1, "before the wide glyph");
        assert_eq!(char_col_to_visual(line, 2), 3, "past the wide glyph: +2");
        assert_eq!(char_col_to_visual(line, 3), 4);
    }

    #[test]
    fn visual_to_char_col_is_the_inverse_and_rounds_inside_wide_glyphs() {
        let line = "a漢b";
        assert_eq!(visual_to_char_col(line, 0), 0);
        assert_eq!(visual_to_char_col(line, 1), 1, "click right at the glyph's left edge");
        assert_eq!(visual_to_char_col(line, 2), 2, "click in the wide glyph's SECOND cell rounds past it");
        assert_eq!(visual_to_char_col(line, 3), 2, "click right at the glyph's right edge");
        assert_eq!(visual_to_char_col(line, 4), 3, "past the end");
        assert_eq!(visual_to_char_col(line, 999), 3, "clamps to the line's length");
    }

    #[test]
    fn pure_ascii_round_trips_exactly_like_the_old_col_times_char_w_math() {
        // The whole point: zero behavior change for the common case.
        let line = "let x = 1;";
        for col in 0..=line.chars().count() {
            assert_eq!(char_col_to_visual(line, col), col, "ascii: visual == char count");
            assert_eq!(visual_to_char_col(line, col), col);
        }
    }
    #[test]
    fn clamped_variant_extends_one_cell_per_char_past_eol() {
        let line = "ab";
        assert_eq!(char_col_to_visual_clamped(line, 0), 0);
        assert_eq!(char_col_to_visual_clamped(line, 2), 2, "exactly at EOL: same as the plain function");
        assert_eq!(char_col_to_visual_clamped(line, 3), 3, "one char past EOL: +1 cell, the selection convention");
        assert_eq!(char_col_to_visual_clamped(line, 5), 5, "further past EOL: +1 per char of overrun");
    }

    #[test]
    fn clamped_variant_matches_plain_within_the_line_even_with_wide_glyphs() {
        let line = "a漢b"; // 1 + 2 + 1 = 4 visual cells, 3 chars
        for col in 0..=3 {
            assert_eq!(char_col_to_visual_clamped(line, col), char_col_to_visual(line, col));
        }
        assert_eq!(char_col_to_visual_clamped(line, 4), 5, "one char past this line's 3 chars: 4 (in-line) + 1");
    }

}
