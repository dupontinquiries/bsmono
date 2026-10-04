//! rank — which apps a query surfaces, and in what order.
//!
//! The base match is the same subsequence semantics as `uikit-egui::fuzzy`
//! (early + contiguous rewarded, gaps penalised) — duplicated here rather than
//! depended on, because fuzzy lives inside an egui crate and this core must
//! stay display-free. If a third consumer appears, the right move is lifting
//! fuzzy into a std-only kit and deleting this copy; the semantics were kept
//! compatible so that swap is a re-import, not a re-tune.
//!
//! On top of the base match, launcher-specific boosts:
//!
//! * a NAME hit outranks a description hit at any score,
//! * a whole-word prefix ("fi" → "Files" over "Amphetamine"),
//! * favorites float, recents float by recency rank.

use crate::entry::AppEntry;

/// Subsequence score, `None` when `needle` is not a subsequence of `hay`.
/// Same shape as uikit-egui::fuzzy::score — see module docs for why the copy.
pub fn subseq_score(needle: &str, hay: &str) -> Option<i32> {
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
            s += 12;
        }
        if last == Some(p.wrapping_sub(1)) {
            s += 6;
        }
        s -= (rel as i32) / 3;
        last = Some(p);
        hi = p + 1;
    }
    Some(s)
}

/// Standing an entry has independent of the query.
#[derive(Debug, Clone, Copy, Default)]
pub struct Standing {
    pub favorite: bool,
    /// 0 = most recent launch, 1 = next, …; `None` = never launched.
    pub recent_rank: Option<usize>,
}

/// Total relevance of `entry` for `query`, or `None` to filter it out.
pub fn score(query: &str, entry: &AppEntry, standing: Standing) -> Option<i64> {
    let name = subseq_score(query, &entry.name);
    let kw = if entry.keywords.is_empty() { None } else { subseq_score(query, &entry.keywords) };
    let desc = subseq_score(query, &entry.description);
    // Three tiers, not weights, so no score inflation can cross a line: the NAME, then the app's own KEYWORDS
    // ("browser" finds Firefox), then the description. Keyword and description matches stay findable but never
    // outrank a name match.
    let base: i64 = match (name, kw, desc) {
        (Some(n), _, _) => 10_000 + n as i64,
        (None, Some(k), _) => 2_000 + k as i64,
        (None, None, Some(d)) => d as i64,
        (None, None, None) => return None,
    };
    let q = query.trim().to_lowercase();
    let prefix = if !q.is_empty() && entry.name.to_lowercase().starts_with(&q) { 5_000 } else { 0 };
    let fav = if standing.favorite { 400 } else { 0 };
    let recent = standing.recent_rank.map(|r| 300_i64.saturating_sub(r as i64 * 30).max(0)).unwrap_or(0);
    Some(base + prefix + fav + recent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, desc: &str) -> AppEntry {
        AppEntry {
            id: name.to_lowercase(),
            name: name.into(),
            description: desc.into(),
            icon: String::new(),
            exec: "x".into(),
            terminal: false,
            ..Default::default()
        }
    }

    #[test]
    fn a_name_match_always_beats_a_description_match() {
        let files = e("Files", "Browse the filesystem");
        let editor = e("Editor", "Edit text files quickly and well");
        let s_name = score("fil", &files, Standing::default()).unwrap();
        let s_desc = score("fil", &editor, Standing::default()).unwrap();
        assert!(s_name > s_desc, "{s_name} vs {s_desc}");
    }

    #[test]
    fn a_prefix_beats_a_scattered_subsequence() {
        let fi = score("fi", &e("Files", ""), Standing::default()).unwrap();
        let scattered = score("fi", &e("Ferocious Kiwi", ""), Standing::default()).unwrap();
        assert!(fi > scattered);
    }

    #[test]
    fn favorites_and_recents_break_ties() {
        let a = e("Alpha", "");
        let s_plain = score("al", &a, Standing::default()).unwrap();
        let s_fav = score("al", &a, Standing { favorite: true, recent_rank: None }).unwrap();
        let s_recent = score("al", &a, Standing { favorite: false, recent_rank: Some(0) }).unwrap();
        assert!(s_fav > s_plain && s_recent > s_plain);
        let s_old = score("al", &a, Standing { favorite: false, recent_rank: Some(9) }).unwrap();
        assert!(s_recent > s_old, "fresher launches rank higher");
    }

    #[test]
    fn no_match_anywhere_filters_the_entry_out() {
        assert_eq!(score("zzz", &e("Files", "Browse"), Standing::default()), None);
    }

    #[test]
    fn a_keyword_finds_an_app_by_what_it_is_for_between_a_name_hit_and_a_description_hit() {
        let mut firefox = e("Firefox", "Web Browser");
        firefox.keywords = "web browser internet www".into();
        let mut notes = e("Notes", "Write things down");
        notes.keywords = "memo text editor".into();
        let by_desc = e("Pager", "Reads memo files");
        assert!(score("internet", &firefox, Standing::default()).is_some(), "found by a word in no name or description");
        assert!(score("internet", &notes, Standing::default()).is_none(), "and only the app that lists it");
        let kw = score("memo", &notes, Standing::default()).unwrap();
        let desc = score("memo", &by_desc, Standing::default()).unwrap();
        let name = score("memo", &e("Memo Pad", ""), Standing::default()).unwrap();
        assert!(name > kw && kw > desc, "name {name} > keyword {kw} > description {desc}");
    }
}
