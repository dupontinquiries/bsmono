//! fuzzy — subsequence fuzzy matching, the pattern behind voiceterm's navigation palette: type a few
//! characters and everything containing them *in order* stays (type "L09" → "L09.md", "gp" →
//! "gui-egui/pages"). Pure and std-only, so any picker/list can filter with it.

/// True if every char of `needle` appears in `hay` in order, case-insensitively. Spaces in `needle`
/// are ignored; an empty needle matches everything.
pub fn subsequence_match(needle: &str, hay: &str) -> bool {
    let hay: Vec<char> = hay.chars().flat_map(char::to_lowercase).collect();
    let mut hi = 0usize;
    for nc in needle.chars().flat_map(char::to_lowercase) {
        if nc == ' ' {
            continue;
        }
        match hay.get(hi..).and_then(|slice| slice.iter().position(|&h| h == nc)) {
            Some(p) => hi += p + 1,
            None => return false,
        }
    }
    true
}

/// A relevance score for ranking matches (higher = better), or `None` when `needle` isn't a
/// subsequence of `hay`. Rewards a match at the very start, contiguous runs, and penalizes gaps — so
/// "L09" ranks "L09.md" above "aLpha-0-9.md". An empty needle scores 0 (everything ties).
pub fn score(needle: &str, hay: &str) -> Option<i32> {
    let needle: Vec<char> = needle.to_lowercase().chars().filter(|c| *c != ' ').collect();
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    let mut hi = 0usize;
    let mut s = 0i32;
    let mut last: Option<usize> = None;
    for &nc in &needle {
        let rel = hay.get(hi..)?.iter().position(|&h| h == nc)?;
        let p = hi + rel;
        if p == 0 {
            s += 12; // matches at the very start
        }
        if last == Some(p.wrapping_sub(1)) {
            s += 6; // contiguous with the previous match
        }
        s -= (rel as i32) / 3; // penalize how far we had to jump
        last = Some(p);
        hi = p + 1;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches_in_order_case_insensitively() {
        assert!(subsequence_match("l09", "L09.md"));
        assert!(subsequence_match("gp", "gui-egui/pages"));
        assert!(subsequence_match("", "anything")); // empty needle matches
        assert!(subsequence_match("a b", "alpha-beta")); // spaces ignored
        assert!(!subsequence_match("90l", "L09.md")); // wrong order
        assert!(!subsequence_match("xyz", "L09.md"));
    }

    #[test]
    fn score_prefers_early_and_contiguous() {
        // exact prefix outranks a scattered match
        let tight = score("l09", "L09.md").unwrap();
        let loose = score("l09", "aLpha-0-9.md").unwrap();
        assert!(tight > loose, "tight {tight} should beat loose {loose}");
        assert_eq!(score("zzz", "abc"), None);
        assert_eq!(score("", "abc"), Some(0));
    }
}
