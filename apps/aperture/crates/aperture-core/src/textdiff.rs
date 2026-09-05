//! textdiff — the minimal text-diffing this crate has needed and not had:
//! given an OLD and NEW full-document string, produce the single smallest
//! contiguous edit that turns one into the other, as a `(start, end, text)`
//! triple in this crate's own char-index [`Pos`] convention.
//!
//! Not a general Myers diff (multiple hunks, globally minimal edit
//! distance) — deliberately. The one real caller this exists for is LSP's
//! incremental `didChange` (see `aperture-lsp`'s own doc on why sending the
//! whole document was the honest default until a real diff utility
//! existed), and that caller only ever has ONE edit to describe at a time:
//! a single keystroke, or a single programmatic `replace_range`/
//! `replace_many` call, both already contiguous by construction on the
//! `aperture-core` side before this module ever sees them. A common-
//! prefix/common-suffix diff finds that one edit exactly, in O(n) with no
//! allocation beyond the returned strings — a general diff would be doing
//! multi-hunk work no caller here needs, and would be a second thing to
//! keep correct for no behavioral gain over the buffer's own guarantee
//! that its callers already produce one contiguous change per edit.

use crate::buffer::Pos;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// Start of the replaced range, inclusive.
    pub start: Pos,
    /// End of the replaced range, exclusive.
    pub end: Pos,
    /// What replaces `[start, end)`.
    pub text: String,
}

/// Compute the edit that turns `old` into `new`. Trims the shared prefix
/// and shared suffix (by CHAR, never by byte, so a computed boundary can
/// never land inside a multi-byte codepoint) off both ends; whatever
/// remains in the middle of each string is the edit.
///
/// Returns `None` when `old == new` — "no edit" is a real, common answer
/// (an edit that round-tripped, or a caller diffing against itself), not a
/// degenerate zero-length `TextEdit` every caller would have to
/// special-case separately from "there was genuinely nothing to send".
pub fn diff(old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();

    let mut prefix = 0;
    let max_prefix = old_chars.len().min(new_chars.len());
    while prefix < max_prefix && old_chars[prefix] == new_chars[prefix] {
        prefix += 1;
    }

    // Bounded by `len - prefix` on EACH side independently, not by 0 —
    // otherwise an edit entirely inside a repeated pattern (replacing the
    // middle "a" in "aaa" with "b") could let the prefix and suffix scans
    // run past each other and "consume" the very characters that changed,
    // reporting a zero-width edit that describes nothing real.
    let mut suffix = 0;
    let max_suffix = (old_chars.len() - prefix).min(new_chars.len() - prefix);
    while suffix < max_suffix
        && old_chars[old_chars.len() - 1 - suffix] == new_chars[new_chars.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let old_end = old_chars.len() - suffix;
    let new_end = new_chars.len() - suffix;

    let start = char_index_to_pos(&old_chars, prefix);
    let end = char_index_to_pos(&old_chars, old_end);
    let text: String = new_chars[prefix..new_end].iter().collect();

    Some(TextEdit { start, end, text })
}

/// Char index -> `(line, col)` by counting newlines up to that index. O(n)
/// per call, which is fine for the one or two calls `diff` makes per
/// invocation — this walks the changed boundary once, not once per
/// character typed.
fn char_index_to_pos(chars: &[char], index: usize) -> Pos {
    let mut line = 0;
    let mut col = 0;
    for &c in &chars[..index] {
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Pos::new(line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_strings_produce_no_edit() {
        assert_eq!(diff("same", "same"), None);
    }

    #[test]
    fn a_single_char_insert_mid_line() {
        // "ab" -> "axb": prefix "a", suffix "b", inserted "x" in between.
        let e = diff("ab", "axb").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 1), end: Pos::new(0, 1), text: "x".into() });
    }

    #[test]
    fn a_single_char_delete_mid_line() {
        let e = diff("axb", "ab").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 1), end: Pos::new(0, 2), text: "".into() });
    }

    #[test]
    fn appending_at_the_very_end() {
        let e = diff("abc", "abcd").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 3), end: Pos::new(0, 3), text: "d".into() });
    }

    #[test]
    fn inserting_at_the_very_start() {
        let e = diff("bcd", "abcd").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 0), end: Pos::new(0, 0), text: "a".into() });
    }

    #[test]
    fn replacing_the_whole_string_with_no_shared_prefix_or_suffix() {
        let e = diff("hello", "world").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 0), end: Pos::new(0, 5), text: "world".into() });
    }

    #[test]
    fn an_edit_inside_a_repeated_pattern_does_not_let_prefix_and_suffix_overlap() {
        // The exact case the prefix/suffix bound-by-remaining-length guard
        // exists for: replacing the MIDDLE "a" in "aaa" with "b". A naive
        // unbounded suffix scan (matching from the end with no bound tied
        // to where the prefix scan stopped) would keep matching "a" == "a"
        // past the real edit and report something nonsensical.
        let e = diff("aaa", "aba").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 1), end: Pos::new(0, 2), text: "b".into() });
    }

    #[test]
    fn a_multiline_insert_reports_the_correct_end_line_and_column() {
        let old = "fn main() {\n}\n";
        let new = "fn main() {\n    let x = 1;\n}\n";
        let e = diff(old, new).unwrap();
        assert_eq!(e.start, Pos::new(1, 0), "the insert starts right after the first newline");
        assert_eq!(e.end, Pos::new(1, 0), "a pure insert: start == end");
        assert_eq!(e.text, "    let x = 1;\n");
    }

    #[test]
    fn deleting_an_entire_line_reports_a_range_spanning_two_lines() {
        let old = "a\nb\nc\n";
        let new = "a\nc\n";
        let e = diff(old, new).unwrap();
        // The shared suffix is "c\n" (2 chars); the deleted "b\n" sits
        // between the shared prefix "a\n" and that suffix.
        assert_eq!(e, TextEdit { start: Pos::new(1, 0), end: Pos::new(2, 0), text: "".into() });
    }

    #[test]
    fn multi_byte_codepoints_never_split_a_boundary() {
        // "é" and "漢" are each one CHAR but multiple UTF-8 bytes; a
        // byte-indexed diff could slice mid-codepoint and panic (or worse,
        // silently corrupt). This only passes if diff() is operating on
        // chars throughout, which `.chars().collect()` up front guarantees.
        let e = diff("aé漢b", "aX漢b").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 1), end: Pos::new(0, 2), text: "X".into() });
    }

    #[test]
    fn an_edit_that_touches_the_final_newline_reports_the_line_after_it() {
        // Deleting the trailing newline changes the LINE COUNT the edit's
        // own end position needs to reflect correctly.
        let e = diff("abc\n", "abc").unwrap();
        assert_eq!(e, TextEdit { start: Pos::new(0, 3), end: Pos::new(1, 0), text: "".into() });
    }
}
