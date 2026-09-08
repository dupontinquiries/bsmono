//! note_search — finding the notes a piece of text belongs with.
//!
//! `TODO_notes_clip.md` step 4 wants "related clips / create card / search" from a word, and
//! names `plan_click` as the bucketing brain. That planner scores UI ELEMENTS — it takes a
//! zone, a colour and a shape — and a note has none of those, so pointing it at the note
//! corpus would mean filling four fields with blanks and reading the result as if it meant
//! something. This scores what a note actually has: a title and a body.
//!
//! ## Whole words, not substrings
//!
//! `contains` would match "cargo" inside "supercargo" and "arg" inside every note that
//! mentions a flag. A suggestion list built from substring hits is mostly noise, and noise in
//! a suggestion list is worse than an empty one — an empty list is read as "nothing related",
//! a noisy one as "this feature does not work".
//!
//! ## The title counts for more
//!
//! A note called "Linker notes" is about linkers in a way a note that mentions the word once
//! is not. Weighted rather than filtered, so a body-only match still surfaces when nothing
//! better exists.
//!
//! Pure: text in, ranked names out. No filesystem — [`crate::note_store::documents`] supplies
//! the corpus.

/// A scored note.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// The document name, as `note_store` produced it.
    pub name: String,
    /// The note's own title, for showing.
    pub title: String,
    pub score: f32,
    /// How many distinct query words were found.
    pub matched: usize,
}

/// A title hit is worth this many body hits.
pub const TITLE_WEIGHT: f32 = 3.0;

/// Words worth searching for.
///
/// Under three characters, or one of a short stop list, and it is not a query term: "the",
/// "a" and "is" appear in every note and would rank the corpus by length.
pub fn terms(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "was", "were", "this", "that", "with", "from", "into", "then",
        "have", "has", "are", "but", "not", "you", "its", "it's", "our", "their",
    ];
    let mut out: Vec<String> = Vec::new();
    for w in query.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        let w = w.trim_matches('-').to_lowercase();
        if w.chars().count() < 3 || STOP.contains(&w.as_str()) || out.contains(&w) {
            continue;
        }
        out.push(w);
    }
    out
}

/// Does `haystack` contain `word` as a whole word?
///
/// The boundary is "not alphanumeric and not `_`", so `cargo-build` matches `cargo` and
/// `supercargo` does not. Hyphens are boundaries because a hyphenated compound is two words
/// to a reader looking for either.
pub fn has_word(haystack: &str, word: &str) -> bool {
    let h = haystack.to_lowercase();
    let mut from = 0;
    while let Some(i) = h[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before_ok = start == 0
            || !h[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_ok = end >= h.len()
            || !h[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
        from = start + word.len().max(1);
        if from >= h.len() {
            break;
        }
    }
    false
}

/// Rank a corpus of `(name, title, markdown)` against `query`, best first.
///
/// Notes matching NOTHING are dropped rather than returned with a zero score: a suggestion
/// list padded to a fixed length with unrelated notes teaches that the suggestions are
/// unrelated.
pub fn search(query: &str, corpus: &[(String, String, String)], limit: usize) -> Vec<Hit> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = corpus
        .iter()
        .filter_map(|(name, title, body)| {
            let mut score = 0.0;
            let mut matched = 0;
            for t in &terms {
                let in_title = has_word(title, t);
                let in_body = has_word(body, t);
                if in_title {
                    score += TITLE_WEIGHT;
                }
                if in_body {
                    score += 1.0;
                }
                if in_title || in_body {
                    matched += 1;
                }
            }
            (matched > 0).then(|| Hit {
                name: name.clone(),
                title: title.clone(),
                score,
                matched,
            })
        })
        .collect();
    // More DISTINCT terms first, then score. A note hitting two of your words beats one
    // hitting the same word twice — coverage is what makes a suggestion feel relevant, and
    // repetition is often just a long note.
    hits.sort_by(|a, b| {
        b.matched
            .cmp(&a.matched)
            .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.title.cmp(&b.title))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<(String, String, String)> {
        vec![
            ("note:linker".into(), "Linker notes".into(), "the linker was the problem\n".into()),
            ("note:build".into(), "Build log".into(), "cargo build failed on the linker step\n".into()),
            ("note:cook".into(), "Recipes".into(), "supercargo is not a linker\n".into()),
        ]
    }

    /// `contains` would match "cargo" inside "supercargo". A suggestion list built from
    /// substring hits is mostly noise, and noise reads as "this feature does not work" where
    /// an empty list reads as "nothing related".
    #[test]
    fn matching_is_by_whole_word() {
        assert!(has_word("cargo build failed", "cargo"));
        assert!(has_word("run cargo-build now", "cargo"), "a hyphen is a boundary");
        assert!(!has_word("supercargo is here", "cargo"));
        assert!(!has_word("cargos everywhere", "cargo"));
        assert!(has_word("CARGO shouted", "cargo"), "case-insensitive");

        let hits = search("cargo", &corpus(), 5);
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["note:build"], "{hits:?}");
    }

    /// A note CALLED "Linker notes" is about linkers in a way one that mentions the word once
    /// is not — but a body-only match still surfaces when nothing better exists.
    #[test]
    fn a_title_hit_outranks_a_body_hit_without_excluding_it() {
        let hits = search("linker", &corpus(), 5);
        assert_eq!(hits.len(), 3, "every note mentions it: {hits:?}");
        assert_eq!(hits[0].name, "note:linker", "the note named for it should lead");
        assert!(hits[0].score > hits[1].score);
    }

    /// Coverage over repetition: a note hitting two of your words beats one hitting the same
    /// word twice, because repetition is often just a long note.
    #[test]
    fn more_distinct_words_matched_wins() {
        let c = vec![
            ("note:a".into(), "One".into(), "linker linker linker linker".into()),
            ("note:b".into(), "Two".into(), "linker and cargo".into()),
        ];
        let hits = search("linker cargo", &c, 5);
        assert_eq!(hits[0].name, "note:b", "{hits:?}");
        assert_eq!(hits[0].matched, 2);
    }

    /// "the", "a" and "is" appear in every note and would rank the corpus by length.
    #[test]
    fn stop_words_and_fragments_are_not_query_terms() {
        assert!(terms("the and was").is_empty());
        assert_eq!(terms("The Linker Was Broken"), vec!["linker".to_string(), "broken".to_string()]);
        // Under three characters is a fragment, not a term.
        assert!(terms("a to go").is_empty());
        // Repeats are asked once.
        assert_eq!(terms("linker linker"), vec!["linker".to_string()]);
        // A query of nothing but stop words finds nothing rather than everything.
        assert!(search("the and was", &corpus(), 5).is_empty());
        assert!(search("", &corpus(), 5).is_empty());
    }

    /// A list padded to a fixed length with unrelated notes teaches that the suggestions are
    /// unrelated.
    #[test]
    fn unrelated_notes_are_dropped_not_padded() {
        let hits = search("teapot", &corpus(), 5);
        assert!(hits.is_empty(), "{hits:?}");
        // And the limit is a cap, not a target.
        assert_eq!(search("linker", &corpus(), 2).len(), 2);
    }
}
