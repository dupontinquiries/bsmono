//! score — fzf's scoring model, ported with its reasoning.
//!
//! The monorepo already has `uikit_egui::fuzzy::subsequence_match`, which
//! answers *does this match*. For a picker over tens of thousands of items that
//! is not enough: everything matches "new", and without a score the list is
//! whatever order the walker happened to produce.
//!
//! fzf's constants are tuned rather than chosen, and its source says how. The
//! boundary bonus is *"specifically chosen that the bonus is cancelled when the
//! gap between the acronyms grows over 8 characters, which is approximately the
//! average length of the words found in web2 dictionary and my file system."*
//! Copying the numbers without that relationship would leave a scorer that
//! looks right and ranks badly, so the relationships are preserved as
//! arithmetic on `SCORE_MATCH` rather than flattened into literals.

pub const SCORE_MATCH: i32 = 16;
pub const SCORE_GAP_START: i32 = -3;
pub const SCORE_GAP_EXTENSION: i32 = -1;
/// Word-boundary bonus. Half a match, so it cancels past an ~8-char gap.
pub const BONUS_BOUNDARY: i32 = SCORE_MATCH / 2;
/// camelCase boundaries carry no single-character gap, so they earn slightly
/// less than a word boundary.
pub const BONUS_CAMEL: i32 = BONUS_BOUNDARY + SCORE_GAP_EXTENSION;
/// Consecutive matches must exactly offset the gap penalties they avoid.
pub const BONUS_CONSECUTIVE: i32 = -(SCORE_GAP_START + SCORE_GAP_EXTENSION);
/// The first pattern character carries extra weight.
pub const BONUS_FIRST_MULTIPLIER: i32 = 2;

fn is_word(c: char) -> bool { c.is_alphanumeric() }

/// Bonus for matching at `cur`, given what preceded it.
fn bonus_for(prev: Option<char>, cur: char) -> i32 {
    match prev {
        // Start of string is the strongest boundary there is.
        None => BONUS_BOUNDARY + 2,
        Some(p) if !is_word(p) && is_word(cur) => {
            // `_` and `::` are the boundaries that matter in Rust paths, and
            // treating them like whitespace is what makes "gr" find
            // `graph_row` ahead of `agrarian`.
            if p == '/' || p == ':' || p == '_' || p == '.' { BONUS_BOUNDARY + 1 }
            else { BONUS_BOUNDARY }
        }
        Some(p) if p.is_lowercase() && cur.is_uppercase() => BONUS_CAMEL,
        Some(p) if !p.is_numeric() && cur.is_numeric() => BONUS_CAMEL,
        _ => 0,
    }
}

/// Score a needle against a haystack, or `None` when it does not match.
///
/// Greedy left-to-right rather than fzf's full dynamic-programming pass. The
/// trade is deliberate and worth naming: DP finds the best alignment, greedy
/// finds a good one in a fraction of the time, and for an interactive picker
/// redrawn on every keystroke over ~50k items the latency matters more than the
/// occasional imperfect tie-break.
pub fn score(needle: &str, hay: &str) -> Option<i32> {
    if needle.is_empty() { return Some(0); }
    let hay_chars: Vec<char> = hay.chars().collect();
    let needle_chars: Vec<char> = needle.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if needle_chars.is_empty() { return Some(0); }

    let mut total = 0;
    let mut hi = 0usize;
    let mut prev_match_idx: Option<usize> = None;
    let mut in_gap = false;

    for (ni, nc) in needle_chars.iter().enumerate() {
        let mut found = None;
        while hi < hay_chars.len() {
            let hc = hay_chars[hi].to_lowercase().next().unwrap_or(hay_chars[hi]);
            if hc == *nc { found = Some(hi); break; }
            hi += 1;
        }
        let idx = found?;
        let prev = if idx == 0 { None } else { Some(hay_chars[idx - 1]) };
        let mut bonus = bonus_for(prev, hay_chars[idx]);
        if ni == 0 { bonus *= BONUS_FIRST_MULTIPLIER; }

        if let Some(p) = prev_match_idx {
            if idx == p + 1 {
                total += BONUS_CONSECUTIVE;
                in_gap = false;
            } else {
                let gap = (idx - p - 1) as i32;
                total += if in_gap { SCORE_GAP_EXTENSION * gap }
                         else { SCORE_GAP_START + SCORE_GAP_EXTENSION * (gap - 1) };
                in_gap = true;
            }
        }
        total += SCORE_MATCH + bonus;
        prev_match_idx = Some(idx);
        hi = idx + 1;
    }

    // Shorter haystacks win ties. Without this, `new` in a 200-character path
    // and `new` in `Foo::new` score identically, and the long one is never what
    // was wanted.
    Some(total - (hay_chars.len() as i32) / 8)
}

/// Does it match at all — the cheap pre-filter.
pub fn matches(needle: &str, hay: &str) -> bool { score(needle, hay).is_some() }

#[cfg(test)]
mod tests {
    use super::*;

    fn better(needle: &str, a: &str, b: &str) {
        let sa = score(needle, a).unwrap_or_else(|| panic!("{a} did not match {needle}"));
        let sb = score(needle, b).unwrap_or_else(|| panic!("{b} did not match {needle}"));
        assert!(sa > sb, "{needle}: expected {a} ({sa}) over {b} ({sb})");
    }

    #[test]
    fn an_empty_needle_matches_everything() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", "anything"), Some(0));
    }

    #[test]
    fn a_non_subsequence_does_not_match() {
        assert_eq!(score("xyz", "alphabet"), None);
        assert_eq!(score("ba", "abc"), None, "order matters");
    }

    #[test]
    fn word_boundaries_beat_the_middle_of_a_word() {
        // The whole reason a scorer exists rather than a filter: everything
        // matches "gr", and the ordering is the answer.
        better("gr", "graph_row", "agrarian");
    }

    #[test]
    fn underscores_and_colons_count_as_boundaries_in_rust_paths() {
        // `_` and `::` are the word separators that matter here; treating them
        // as ordinary characters would rank `punch_addrs` below anything with
        // the letters closer together.
        better("pa", "punch_addrs", "opaque");
        // Same gap either side, so the boundary is the only difference: the `_`
        // earns the bonus and the ordinary letter does not.
        better("sw", "s_wire", "sxwire");
    }

    #[test]
    fn a_long_gap_loses_to_an_adjacent_match_even_across_a_boundary() {
        // My first version of the test above expected "snapcore::wire" to beat
        // "swathe" for "sw". It does not, and should not: ten characters of gap
        // outweighs one boundary bonus, which is precisely the cancellation
        // fzf tuned for. Recording it as its own case so the behaviour is
        // asserted rather than merely not-tested.
        let far = score("sw", "snapcore::wire").unwrap();
        let near = score("sw", "swathe").unwrap();
        assert!(near > far, "adjacent {near} should beat a ten-char gap {far}");
    }

    #[test]
    fn camel_case_boundaries_score() {
        better("tb", "TabBar", "textblob");
    }

    #[test]
    fn consecutive_characters_beat_a_scattered_match() {
        better("chev", "chevron", "cache_hit_every");
    }

    #[test]
    fn the_first_character_carries_extra_weight() {
        // Matching at the start of the name is usually what was meant.
        better("new", "new_session", "renew_token");
    }

    #[test]
    fn a_shorter_haystack_wins_a_tie() {
        // Otherwise `new` in a 200-character path ranks with `Foo::new`, and
        // the long one is never what was wanted.
        better("new", "Foo::new", "some/very/long/path/to/a/module/that/has/new/in/it");
    }

    #[test]
    fn the_boundary_bonus_cancels_over_a_long_gap() {
        // fzf tuned the bonus so it stops dominating past ~8 characters, which
        // is why the relationships are kept as arithmetic on SCORE_MATCH rather
        // than flattened to literals.
        let tight = score("ab", "a_b").unwrap();
        let far = score("ab", "a_ccccccccccccb").unwrap();
        assert!(tight > far, "tight {tight} should beat far {far}");
    }

    #[test]
    fn matching_is_case_insensitive_both_ways() {
        assert!(matches("TABBAR", "tab_bar"));
        assert!(matches("tabbar", "TabBar"));
    }

    #[test]
    fn spaces_in_the_needle_are_ignored() {
        // Typing "tab bar" should find `TabBar` — the space is how a human
        // separates words, not a character to match.
        assert!(matches("tab bar", "TabBar"));
    }

    #[test]
    fn a_realistic_query_ranks_the_obvious_answer_first() {
        let mut items = vec![
            "apps/snapclone/egui-frontend/src/punch_addrs.rs",
            "shared/snapcore/src/punch.rs",
            "apps/voiceterm/src/pty_input.rs",
            "shared/uikit/crates/uikit-egui/src/split_pane.rs",
        ];
        // Sorted by descending score without negating: -i32::MIN overflows,
        // which my first version did on any non-match.
        items.sort_by(|a, b| score("punch", b).unwrap_or(i32::MIN)
            .cmp(&score("punch", a).unwrap_or(i32::MIN)));
        assert_eq!(items[0], "shared/snapcore/src/punch.rs");
    }
}
