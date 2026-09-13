//! bufsearch — Helix's `/`: incremental search within the CURRENT buffer.
//!
//! Distinct from `project_search`, which greps every file in the workspace
//! for the palette's Ctrl+Shift+F. `/` is narrower and faster to reach for
//! exactly because it is narrower: "find this word again in the file I am
//! already looking at" does not want a workspace walk, a result list, or a
//! tab switch — it wants the caret to jump and the search bar to stay out of
//! the way. Reusing `project_search`'s file-list-and-rank machinery for a
//! single known string would be answering a smaller question with a bigger
//! tool.
//!
//! Pure: takes the buffer's text and a query, returns match positions. No
//! egui, no `Editor`, so the wrapping-and-wraparound rules below are
//! testable without a frame.

/// Every match of `query` in `text`, as `(start_char, end_char)` char-index
/// pairs — not bytes, to match `Buffer::slice_to_string`'s own units and
/// `Selection`'s.
///
/// Case-INSENSITIVE and NON-overlapping, matching Helix's own `/` default.
/// An empty query returns no matches rather than one match per character
/// position, which `str::match_indices("")` would otherwise produce — an
/// empty search bar must show nothing found, not the whole file highlighted.
pub fn find_all(text: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    let hay = text.to_lowercase();
    let needle = query.to_lowercase();
    let mut out = Vec::new();
    // Byte offsets from match_indices, converted to char offsets at the end
    // in one pass — converting per-match would be O(matches * text length).
    let mut byte_hits = Vec::new();
    let mut start = 0usize;
    while let Some(rel) = hay[start..].find(&needle) {
        let b = start + rel;
        byte_hits.push((b, b + needle.len()));
        start = b + needle.len().max(1);
        if start > hay.len() {
            break;
        }
    }
    if byte_hits.is_empty() {
        return out;
    }
    // One left-to-right scan converting byte offsets to char offsets,
    // rather than `text[..b].chars().count()` per hit — that is O(hits *
    // text length) on a file with many matches, which is exactly the file
    // someone reaches for `/` on.
    let mut char_at_byte = std::collections::HashMap::with_capacity(byte_hits.len() * 2);
    let mut chars_seen = 0usize;
    for (bi, _) in text.char_indices() {
        char_at_byte.insert(bi, chars_seen);
        chars_seen += 1;
    }
    char_at_byte.insert(text.len(), chars_seen); // end-of-text is a valid end offset
    for (bs, be) in byte_hits {
        if let (Some(&cs), Some(&ce)) = (char_at_byte.get(&bs), char_at_byte.get(&be)) {
            out.push((cs, ce));
        }
    }
    out
}

/// Which match `next`/`prev` should land on, given the caret's current char
/// position and the sorted match list. Wraps in both directions, matching
/// Helix: search never dead-ends at the top or bottom of the file.
///
/// `matches` must be sorted ascending by start — `find_all` already returns
/// them that way, and this does not re-sort, so a caller with a different
/// order gets a wrong answer rather than a hidden sort cost on every
/// keystroke.
pub fn next_match(matches: &[(usize, usize)], caret: usize, backward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if backward {
        // The last match whose END is at or before the caret — NOT "whose
        // START is before the caret", which was the first version of this
        // and undercounts wrong: if the caret sits INSIDE a match (start <
        // caret < end, the ordinary state after jumping TO a match), that
        // match's start is still < caret, so the start-based rule picked the
        // match the caret is already sitting in instead of the one before
        // it. The forward branch never had this problem — `s > caret`
        // already excludes any match containing the caret, since a
        // contained caret has s <= caret — so backward needs the symmetric
        // condition on the OTHER end of the match to get the same
        // "skip the one I'm on" behaviour.
        matches.iter().rposition(|&(_, e)| e <= caret).or(Some(matches.len() - 1))
    } else {
        // The first match strictly AFTER the caret, or the first match
        // overall if the caret is at or past the last one (wrap).
        matches.iter().position(|&(s, _)| s > caret).or(Some(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_occurrence_case_insensitively() {
        let hits = find_all("Fold fold FOLD folder", "fold");
        assert_eq!(hits, vec![(0, 4), (5, 9), (10, 14), (15, 19)]);
    }

    #[test]
    fn empty_query_matches_nothing() {
        // Not str::match_indices("")'s behaviour, which is one hit per
        // character position — an empty search bar must highlight nothing,
        // not the entire file.
        assert_eq!(find_all("anything", ""), Vec::new());
    }

    #[test]
    fn matches_do_not_overlap() {
        // "aa" in "aaaa" is 2 non-overlapping hits, not 3 sliding-window
        // ones — overlapping hits would double-count and double-highlight.
        assert_eq!(find_all("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn multibyte_text_gives_correct_char_offsets() {
        // "café" is 4 chars / 5 bytes (é is 2 bytes in UTF-8). A byte-offset
        // bug here would report a match one character too far right for
        // every character after the accent, silently corrupting the
        // highlight and the jump-to-match position for any non-ASCII file.
        let hits = find_all("café café", "café");
        assert_eq!(hits, vec![(0, 4), (5, 9)], "char offsets, not byte offsets");
    }

    #[test]
    fn next_match_advances_and_wraps() {
        let m = vec![(0, 3), (10, 13), (20, 23)];
        assert_eq!(next_match(&m, 0, false), Some(1), "caret ON a match advances past it");
        assert_eq!(next_match(&m, 15, false), Some(2));
        assert_eq!(next_match(&m, 25, false), Some(0), "past the last match wraps to the first");
    }

    #[test]
    fn prev_match_retreats_and_wraps() {
        let m = vec![(0, 3), (10, 13), (20, 23)];
        assert_eq!(next_match(&m, 25, true), Some(2), "backward from the end lands on the last");
        assert_eq!(next_match(&m, 12, true), Some(0));
        assert_eq!(next_match(&m, 0, true), Some(2), "before the first match wraps to the last");
    }

    #[test]
    fn prev_match_skips_the_match_the_caret_is_currently_inside() {
        // The exact bug: caret=12 sits INSIDE match (10, 13), not between
        // match 1 and match 2. A start-only rule ("last match whose start is
        // before the caret") still counts (10, 13) itself as "before" 12,
        // since 10 < 12 — so pressing "search backward" while sitting in a
        // match landed on the SAME match instead of the previous one. This
        // is the ordinary post-jump state: land on a match, its caret is
        // somewhere inside the match's range, not before it.
        let m = vec![(0, 3), (10, 13), (20, 23)];
        for caret_inside_match_1 in [10, 11, 12] {
            assert_eq!(
                next_match(&m, caret_inside_match_1, true),
                Some(0),
                "caret={caret_inside_match_1} is inside match 1; backward must reach match 0, \
                 not match 1 again"
            );
        }
        // And landing exactly on a match's END (one past its last char) is
        // NOT "inside" it — the match is now fully behind the caret, so it
        // counts as the previous match, symmetric with the forward branch
        // treating a match's START as the boundary the caret must clear.
        assert_eq!(next_match(&m, 13, true), Some(1));
    }

    #[test]
    fn no_matches_is_none_not_a_panic() {
        assert_eq!(next_match(&[], 0, false), None);
        assert_eq!(next_match(&[], 0, true), None);
    }
}
