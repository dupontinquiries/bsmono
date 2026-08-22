//! Pure search over logged lines — the ranker and snippet builder, no IO.
//!
//! Deliberately the same shape as voiceterm's `note_search.rs`: the host
//! supplies candidate documents, this scores, ranks, and builds the rows. That
//! makes it testable without a filesystem and lets the same ranker serve the
//! terminal, the phone, and the fan-mode command palette.
//!
//! ## Ranking, and why it is not just substring count
//!
//! An IRC log is thousands of short lines by dozens of people over years. The
//! query "avd vulkan" should surface the line where someone explained the
//! staging decision, not the fortieth time the word `avd` appeared in a
//! greeting. So:
//!
//! * **All query terms present outranks any term repeated.** A line matching
//!   both words beats a line saying `avd` five times.
//! * **Nick matches count, but strictly less than body matches.** Searching a
//!   person's name should find their messages, without every line they ever
//!   sent outranking the one that answers the question. A nick-only hit
//!   scores half a body hit, and the two are additive rather than the nick
//!   being a bonus on top.
//! * **Short lines get a mild boost at equal match.** `ack` and a 400-word
//!   paste both containing the terms are not equally informative, and the
//!   short one is cheaper to read.
//! * **Adjacent terms beat scattered ones.** "vulkan video" as a phrase is
//!   almost always what was meant.
//!
//! Scores are relative and have no units; only the ordering is meaningful.

use crate::log::Line;

/// One search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Index into the slice that was searched, so the caller can recover the
    /// full record and its neighbours for context.
    pub index: usize,
    pub score: i64,
    /// A window of the line around the first matched term.
    pub snippet: String,
}

/// Split a query into lowercased terms. Quoted runs stay together as one
/// phrase term, so `"vulkan video"` is searched as a unit.
pub fn terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in query.chars() {
        match c {
            '"' => {
                if in_quote && !cur.is_empty() {
                    out.push(cur.to_lowercase());
                    cur = String::new();
                }
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(cur.to_lowercase());
                    cur = String::new();
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur.to_lowercase());
    }
    out
}

fn count_occurrences(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut n = 0;
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        n += 1;
        from += i + needle.len();
    }
    n
}

/// Score one line against the parsed terms. Returns `None` when nothing
/// matches, so a caller can skip building a snippet.
pub fn score_line(line: &Line, terms: &[String]) -> Option<i64> {
    if terms.is_empty() {
        return None;
    }
    let body = line.text.to_lowercase();
    let nick = line.nick.to_lowercase();

    let mut score = 0i64;
    let mut matched = 0usize;
    for t in terms {
        let in_body = count_occurrences(&body, t);
        let in_nick = count_occurrences(&nick, t);
        if in_body == 0 && in_nick == 0 {
            continue;
        }
        matched += 1;
        if in_body > 0 {
            // First occurrence is worth much more than repeats: a word said
            // twice is not twice as relevant.
            score += 10 + (in_body.saturating_sub(1) as i64).min(5);
        }
        if in_nick > 0 {
            // Strictly less than a body match, and additive rather than a
            // bonus on top of one. Otherwise `avd_fan` saying "ack" ties with
            // a line that actually says "avd" five times, and the recency
            // tiebreak hands the win to the nick.
            score += 5;
        }
    }
    if matched == 0 {
        return None;
    }

    // The dominant term: every query word present.
    if matched == terms.len() {
        score += 25 * matched as i64;
    }

    // Adjacency: terms appearing near each other in the body.
    if terms.len() > 1 {
        let mut positions: Vec<usize> = Vec::new();
        for t in terms {
            if let Some(i) = body.find(t.as_str()) {
                positions.push(i);
            }
        }
        if positions.len() > 1 {
            positions.sort_unstable();
            let span = positions[positions.len() - 1] - positions[0];
            if span < 40 {
                score += (40 - span) as i64 / 2;
            }
        }
    }

    // Mild brevity preference; capped so it can never outweigh a real match.
    let len = line.text.chars().count() as i64;
    score += (200 - len.min(200)) / 25;

    // A notice is usually a bot; still findable, just not preferred.
    if line.notice {
        score -= 5;
    }

    Some(score)
}

/// Rank lines against a query. Ties break toward the more recent line, because
/// in a log the later answer usually supersedes the earlier one.
pub fn search(query: &str, lines: &[Line], max_results: usize) -> Vec<Hit> {
    let t = terms(query);
    if t.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<Hit> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            score_line(l, &t).map(|score| Hit {
                index: i,
                score,
                snippet: make_snippet(&l.text, &t, 160),
            })
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| lines[b.index].ts.cmp(&lines[a.index].ts))
            .then_with(|| a.index.cmp(&b.index))
    });
    hits.truncate(max_results);
    hits
}

/// A window of `text` centred on the first matched term, with ellipses where
/// it was cut. Slicing is done on char boundaries, so a snippet can never
/// split a multi-byte character.
pub fn make_snippet(text: &str, terms: &[String], width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let lower = text.to_lowercase();
    let byte_pos = terms.iter().filter_map(|t| lower.find(t.as_str())).min().unwrap_or(0);
    // Byte offset -> char offset.
    let char_pos = lower[..byte_pos].chars().count();

    let half = width / 2;
    let start = char_pos.saturating_sub(half);
    let end = (start + width).min(chars.len());
    let start = end.saturating_sub(width);

    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.extend(&chars[start..end]);
    if end < chars.len() {
        s.push('…');
    }
    s
}

/// Group hits into conversational runs: consecutive log indices within
/// `gap` of each other become one block. A single matched line out of a
/// four-message exchange is usually unreadable on its own; this is what lets
/// a caller show the exchange.
pub fn cluster(hits: &[Hit], gap: usize) -> Vec<(usize, usize)> {
    if hits.is_empty() {
        return Vec::new();
    }
    let mut idx: Vec<usize> = hits.iter().map(|h| h.index).collect();
    idx.sort_unstable();
    idx.dedup();

    let mut out = Vec::new();
    let mut start = idx[0];
    let mut prev = idx[0];
    for &i in &idx[1..] {
        if i - prev > gap {
            out.push((start, prev));
            start = i;
        }
        prev = i;
    }
    out.push((start, prev));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Origin;

    fn l(ts: u64, nick: &str, text: &str) -> Line {
        Line {
            ts,
            server_time: None,
            network: "oftc".into(),
            channel: "#asahi-dev".into(),
            nick: nick.into(),
            text: text.into(),
            notice: false,
            action: false,
            origin: Origin::Participated,
        }
    }

    fn corpus() -> Vec<Line> {
        vec![
            // 0: one term, many times — the thing all-terms must beat.
            l(10, "alice", "avd avd avd avd avd"),
            // 1: both terms, adjacent.
            l(20, "bob", "avd vulkan is the plan"),
            // 2: no terms at all.
            l(30, "carol", "morning everyone"),
            // 3: both terms, far apart in a long line.
            l(40, "dave", &format!("vulkan {} avd", "filler words here ".repeat(6))),
            // 4: the term appears only in the nick.
            l(50, "avd_fan", "ack"),
        ]
    }

    #[test]
    fn all_terms_present_beats_one_term_repeated() {
        let hits = search("avd vulkan", &corpus(), 10);
        let repeated = hits.iter().position(|h| h.index == 0).unwrap();
        let both_adjacent = hits.iter().position(|h| h.index == 1).unwrap();
        let both_scattered = hits.iter().position(|h| h.index == 3).unwrap();
        assert!(both_adjacent < repeated, "a line with both terms beats one term x5");
        assert!(both_scattered < repeated, "even a scattered both-terms line beats it");
    }

    #[test]
    fn adjacency_beats_scattered_terms() {
        let hits = search("avd vulkan", &corpus(), 10);
        let close = hits.iter().position(|h| h.index == 1).unwrap();
        let far = hits.iter().position(|h| h.index == 3).unwrap();
        assert!(close < far, "adjacent terms should rank above scattered ones");
    }

    #[test]
    fn a_nick_only_hit_loses_to_a_body_hit_outright() {
        // The specific regression: `avd_fan` saying "ack" used to tie a line
        // saying "avd" five times, and won on the recency tiebreak.
        let hits = search("avd", &corpus(), 10);
        let nick_only = hits.iter().position(|h| h.index == 4).unwrap();
        let body = hits.iter().position(|h| h.index == 0).unwrap();
        assert!(body < nick_only);
    }

    #[test]
    fn a_nick_match_is_findable_but_does_not_dominate() {
        let hits = search("avd", &corpus(), 10);
        assert!(hits.iter().any(|h| h.index == 4), "the nick match is findable");
        assert_ne!(hits[0].index, 4, "but 'ack' by avd_fan is not the best hit for 'avd'");
    }

    #[test]
    fn non_matching_lines_are_absent_entirely() {
        let hits = search("avd", &corpus(), 10);
        assert!(!hits.iter().any(|h| h.index == 2), "'morning everyone' matches nothing");
    }

    #[test]
    fn quoted_phrases_are_one_term() {
        assert_eq!(terms(r#"avd "vulkan video" stateless"#), vec!["avd", "vulkan video", "stateless"]);
        let lines = vec![l(1, "a", "vulkan video decode"), l(2, "b", "vulkan and separately video")];
        let hits = search(r#""vulkan video""#, &lines, 10);
        assert_eq!(hits.len(), 1, "the phrase matches only the adjacent spelling");
        assert_eq!(hits[0].index, 0);
    }

    #[test]
    fn an_empty_query_returns_nothing_rather_than_everything() {
        assert!(search("", &corpus(), 10).is_empty());
        assert!(search("   ", &corpus(), 10).is_empty());
    }

    #[test]
    fn ties_break_toward_the_more_recent_line() {
        let lines = vec![l(10, "a", "same words here"), l(99, "b", "same words here")];
        let hits = search("same words", &lines, 10);
        assert_eq!(hits[0].index, 1, "later line first at equal score");
    }

    #[test]
    fn max_results_is_respected() {
        assert_eq!(search("avd", &corpus(), 2).len(), 2);
    }

    #[test]
    fn snippets_centre_on_the_match_and_mark_the_cut() {
        let long = format!("{}NEEDLE{}", "x".repeat(300), "y".repeat(300));
        let s = make_snippet(&long, &["needle".to_string()], 40);
        assert!(s.contains("NEEDLE"), "got {s}");
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.chars().count() <= 42);
    }

    #[test]
    fn a_short_line_is_returned_whole() {
        assert_eq!(make_snippet("short", &["short".into()], 160), "short");
    }

    #[test]
    fn snippets_never_split_a_multibyte_char() {
        let text = format!("{}Ω NEEDLE Ω{}", "日".repeat(200), "本".repeat(200));
        let s = make_snippet(&text, &["needle".to_string()], 40);
        assert!(s.contains("NEEDLE"));
        // Reaching here without a panic is the assertion; slicing on a byte
        // index inside 日 would have aborted.
        assert!(s.chars().count() <= 42);
    }

    #[test]
    fn notices_rank_below_equivalent_messages() {
        let mut a = l(10, "bot", "avd vulkan video");
        a.notice = true;
        let b = l(10, "human", "avd vulkan video");
        let hits = search("avd vulkan", &[a, b], 10);
        assert_eq!(hits[0].index, 1);
    }

    #[test]
    fn clustering_joins_nearby_hits_and_splits_distant_ones() {
        let hits = vec![
            Hit { index: 4, score: 1, snippet: String::new() },
            Hit { index: 5, score: 1, snippet: String::new() },
            Hit { index: 7, score: 1, snippet: String::new() },
            Hit { index: 90, score: 1, snippet: String::new() },
        ];
        assert_eq!(cluster(&hits, 3), vec![(4, 7), (90, 90)]);
        assert_eq!(cluster(&hits, 1), vec![(4, 5), (7, 7), (90, 90)]);
        assert!(cluster(&[], 3).is_empty());
    }
}
