// src/session_walk.rs
//
// v0.1.426 ("walk me through my last session's webpages"): a
// Prezi-style guided walk across the graph through the pages
// visited in the most recent browsing session. The camera flies
// card→card; each card pops, then crossfades its preview to a
// "word smush" — a cheap, no-LLM salient-word view that works
// instantly even when local LLMs time out.
//
// This module holds the PURE, testable core:
//   - session_boundary detection (gap-based clustering of
//     page timestamps)
//   - last_session_order (the ordered list of node ids to walk)
//   - word_smush (salient-word extraction from preview text)
// The QML side consumes a wire string built from these; the
// camera animation + slider live in main.qml.

#![allow(dead_code)]

/// A gap larger than this (seconds) between consecutive page
/// visits starts a new session. 30 minutes — long enough that a
/// coffee break doesn't split a session, short enough that
/// yesterday's browsing is a different session from today's.
pub const SESSION_GAP_SECS: u64 = 30 * 60;

/// Cap how many pages a single walk includes, so a marathon
/// session doesn't produce a 200-step slog. The most recent N.
pub const MAX_WALK_PAGES: usize = 12;

/// A page in the walk: the node id (to locate its card in the
/// graph) plus the data the preview/smush needs.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkPage {
    pub id: String,
    pub title: String,
    pub domain: String,
    pub url: String,
    pub preview: String,
    pub created_at: u64,
}

/// Given page entries sorted-or-not by time, return the ids (in
/// visit order, oldest→newest within the session) of the pages
/// belonging to the MOST RECENT session — i.e. the trailing run
/// of pages with no >SESSION_GAP_SECS gap between consecutive
/// visits. Capped to the most recent MAX_WALK_PAGES.
pub fn last_session(pages: &[WalkPage]) -> Vec<WalkPage> {
    if pages.is_empty() {
        return Vec::new();
    }
    // Sort by time ascending (clone the refs into a Vec of idx).
    let mut idx: Vec<usize> = (0..pages.len()).collect();
    idx.sort_by_key(|&i| pages[i].created_at);

    // Walk backward from the newest, including pages until we hit
    // a gap larger than SESSION_GAP_SECS.
    let mut session_rev: Vec<usize> = Vec::new();
    let mut prev_ts: Option<u64> = None;
    for &i in idx.iter().rev() {
        let ts = pages[i].created_at;
        if let Some(p) = prev_ts {
            // p is the newer one (we're going backward); gap is
            // p - ts.
            if p.saturating_sub(ts) > SESSION_GAP_SECS {
                break;
            }
        }
        session_rev.push(i);
        prev_ts = Some(ts);
        if session_rev.len() >= MAX_WALK_PAGES {
            break;
        }
    }
    // session_rev is newest→oldest; reverse to oldest→newest for
    // a natural "walk forward through what you did" order.
    session_rev.reverse();
    session_rev.into_iter().map(|i| pages[i].clone()).collect()
}

/// v0.1.438: stopword check delegates to the shared
/// nlp::is_stopword (single source of truth). Was a local list
/// near-identical to nlp's; "newsletter" (the one word this list
/// had that nlp's lacked) was folded into nlp's list so nothing
/// regresses.
fn is_stopword(w: &str) -> bool {
    crate::nlp::is_stopword(w)
}

/// Extract up to `max` salient items from text for the "word
/// smush" view. v0.1.430: prefers RAKE keyPHRASES (e.g. "machine
/// learning safety") over scattered single words — far more
/// glanceable. Falls back to the original frequency-ranked
/// single words when RAKE finds too few phrases (short/choppy
/// text). No LLM — instant and offline.
pub fn word_smush(text: &str, max: usize) -> Vec<String> {
    // RAKE first: multi-word keyphrases read better in the smush.
    let phrases = crate::nlp::rake_keyphrases(text, max);
    // Use RAKE if it found at least one multi-word phrase (the
    // whole point); otherwise fall back to single-word frequency
    // so very choppy text still yields something.
    if phrases.iter().any(|p| p.split_whitespace().count() >= 2) {
        return phrases;
    }
    word_smush_freq(text, max)
}

/// Original frequency-ranked single-word smush (kept as the
/// fallback path + still independently useful/tested).
pub fn word_smush_freq(text: &str, max: usize) -> Vec<String> {
    use std::collections::HashMap;
    // (count, first_index) per word.
    let mut seen: HashMap<String, (u32, usize)> = HashMap::new();
    let mut order = 0usize;
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        let w = raw.to_lowercase();
        if w.chars().count() < 3 {
            continue; // drop 1-2 char tokens (a, of, to, …)
        }
        if is_stopword(&w) {
            continue;
        }
        // Skip pure numbers (years etc. rarely make a good smush).
        if w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let e = seen.entry(w).or_insert((0, order));
        e.0 += 1;
        order += 1;
    }
    let mut words: Vec<(String, u32, usize)> =
        seen.into_iter().map(|(w, (c, o))| (w, c, o)).collect();
    // Higher count first; tie → earlier first-appearance.
    words.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    words.into_iter().take(max).map(|(w, _, _)| w).collect()
}

/// v0.1.427: does this recognized phrase mean "advance the walk
/// / start auto-play"? Matches English ("keep going", "next",
/// "continue", "go on") and Spanish ("vamos", "dale", "sigue",
/// "continúa", "siguiente"). Case-insensitive, tolerant of extra
/// words and punctuation, and of common ASR manglings of "dale"
/// ("dah lay", "dolly"). Pure → testable.
pub fn walk_advance_cue(text: &str) -> bool {
    let t = text.to_lowercase();
    let norm: String = t
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    let joined = tokens.join(" ");

    const PHRASES: &[&str] = &[
        "keep going", "go on", "carry on", "move on",
        "vamos", "continua", "continúa", "sigue", "siguiente",
        "dah lay", // ASR mangling of "dale"
    ];
    for ph in PHRASES {
        if joined.contains(ph) {
            return true;
        }
    }
    // Single-token cues — match a whole token so "nextflix"
    // doesn't trigger "next".
    const WORDS: &[&str] = &["next", "continue", "dale", "dolly", "advance", "forward"];
    tokens.iter().any(|w| WORDS.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, ts: u64) -> WalkPage {
        WalkPage {
            id: id.to_string(),
            title: format!("Title {}", id),
            domain: "example.com".into(),
            url: format!("https://example.com/{}", id),
            preview: String::new(),
            created_at: ts,
        }
    }

    #[test]
    fn last_session_groups_trailing_run() {
        // Three pages close together (newest session), then a big
        // gap, then two older ones.
        let base = 1_000_000u64;
        let pages = vec![
            p("old1", base),
            p("old2", base + 60),
            // 2-hour gap
            p("new1", base + 7200),
            p("new2", base + 7260),
            p("new3", base + 7300),
        ];
        let walk = last_session(&pages);
        let ids: Vec<&str> = walk.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["new1", "new2", "new3"], "only the trailing session, in order");
    }

    #[test]
    fn last_session_single_page() {
        let walk = last_session(&[p("only", 500)]);
        assert_eq!(walk.len(), 1);
        assert_eq!(walk[0].id, "only");
    }

    #[test]
    fn last_session_empty() {
        assert!(last_session(&[]).is_empty());
    }

    #[test]
    fn last_session_all_one_session() {
        let pages = vec![p("a", 0), p("b", 100), p("c", 200)];
        let walk = last_session(&pages);
        assert_eq!(walk.len(), 3, "no gaps → all one session");
        assert_eq!(walk[0].id, "a", "ordered oldest→newest");
        assert_eq!(walk[2].id, "c");
    }

    #[test]
    fn last_session_unsorted_input() {
        // Same as the trailing-run test but input order shuffled.
        let base = 1_000_000u64;
        let pages = vec![
            p("new3", base + 7300),
            p("old1", base),
            p("new1", base + 7200),
            p("old2", base + 60),
            p("new2", base + 7260),
        ];
        let walk = last_session(&pages);
        let ids: Vec<&str> = walk.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["new1", "new2", "new3"]);
    }

    #[test]
    fn last_session_caps_at_max() {
        let pages: Vec<WalkPage> = (0..30).map(|i| p(&format!("p{}", i), i as u64 * 10)).collect();
        let walk = last_session(&pages);
        assert!(walk.len() <= MAX_WALK_PAGES);
        // Should be the most recent MAX_WALK_PAGES, in order.
        assert_eq!(walk.last().unwrap().id, "p29");
    }

    #[test]
    fn word_smush_freq_ranks_by_frequency() {
        let text = "Rust memory safety. Rust ownership. Rust borrow checker. Memory safety matters.";
        let smush = word_smush_freq(text, 3);
        assert_eq!(smush[0], "rust", "most frequent word first");
        assert!(smush.contains(&"memory".to_string()) || smush.contains(&"safety".to_string()));
    }

    #[test]
    fn word_smush_freq_drops_stopwords_and_short() {
        let text = "The cat is on the mat and it is a big cat.";
        let smush = word_smush_freq(text, 5);
        assert!(smush.contains(&"cat".to_string()));
        assert!(!smush.contains(&"the".to_string()), "stopword dropped");
        assert!(!smush.contains(&"is".to_string()));
        assert!(!smush.iter().any(|w| w.chars().count() < 3), "no short tokens");
    }

    #[test]
    fn word_smush_freq_drops_pure_numbers() {
        let text = "release 2024 version 2024 stable release";
        let smush = word_smush_freq(text, 5);
        assert!(!smush.contains(&"2024".to_string()), "pure numbers dropped");
        assert_eq!(smush[0], "release");
    }

    #[test]
    fn word_smush_freq_deterministic_ties() {
        // alpha and beta both appear once; alpha first.
        let smush = word_smush_freq("alpha beta gamma", 3);
        assert_eq!(smush, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn word_smush_empty() {
        assert!(word_smush("", 5).is_empty());
        assert!(word_smush_freq("the a of to", 5).is_empty(), "all stopwords → empty");
    }

    // v0.1.430: word_smush now prefers RAKE multi-word phrases.
    #[test]
    fn word_smush_prefers_phrases() {
        let text = "Machine learning safety is a serious research field. \
                    Machine learning safety needs careful evaluation.";
        let smush = word_smush(text, 5);
        assert!(smush.iter().any(|p| p.split_whitespace().count() >= 2),
            "should yield a multi-word phrase, got {:?}", smush);
    }

    #[test]
    fn word_smush_falls_back_to_freq_when_no_phrases() {
        // Single content words separated by stopwords → no
        // multi-word RAKE phrases → frequency fallback.
        let text = "cats and dogs and cats and dogs";
        let smush = word_smush(text, 3);
        assert!(!smush.is_empty(), "fallback yields single words");
        assert!(smush.iter().all(|p| p.split_whitespace().count() == 1));
    }

    // v0.1.427: voice advance cue.
    #[test]
    fn advance_cue_english() {
        assert!(walk_advance_cue("keep going"));
        assert!(walk_advance_cue("ok, keep going please"));
        assert!(walk_advance_cue("next"));
        assert!(walk_advance_cue("continue"));
        assert!(walk_advance_cue("go on"));
    }

    #[test]
    fn advance_cue_spanish() {
        assert!(walk_advance_cue("vamos"));
        assert!(walk_advance_cue("dale"));
        assert!(walk_advance_cue("sigue"));
        assert!(walk_advance_cue("siguiente"));
        // Common ASR manglings of "dale".
        assert!(walk_advance_cue("dah lay"));
        assert!(walk_advance_cue("dolly"));
    }

    #[test]
    fn advance_cue_negatives() {
        assert!(!walk_advance_cue(""));
        assert!(!walk_advance_cue("stop"));
        assert!(!walk_advance_cue("go back"));
        // Whole-token match: "nextflix" must not trigger "next".
        assert!(!walk_advance_cue("open nextflix"));
        assert!(!walk_advance_cue("what is this page about"));
    }
}
