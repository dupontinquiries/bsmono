//! v0.1.675: content (full-text) search across note/script bodies — the
//! piece the search bar was missing. Searching "cloak" found nothing because
//! search_suggestions only looked at history URLs, bookmarks, and FILENAMES,
//! never file CONTENTS. `rg cloak` finds 159 lines; the app found 0.
//!
//! This is the pure ranker + snippet extractor. The host supplies candidate
//! documents (path + text — from the path index limited to text files, or an
//! rg pre-filter); this scores, ranks, and builds the dropdown rows. PURE +
//! testable; the file IO + dropdown wiring is the host layer.

/// A document to search (a note/script body).
#[derive(Debug, Clone)]
pub struct Doc<'a> {
    pub path: &'a str,
    pub text: &'a str,
}

/// A ranked content match for the suggestion dropdown.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub path: String,
    pub line_no: usize,    // 1-based line of the first/best hit
    pub snippet: String,   // the matched line, trimmed + window around the hit
    pub hits: usize,       // total hits in the doc (relevance signal)
    pub score: f32,
}

/// Case-insensitive substring search by default; multi-word queries match
/// ALL words (AND) somewhere in the line. Returns ranked matches, best first.
pub fn search(query: &str, docs: &[Doc], max_results: usize) -> Vec<Match> {
    let q = query.trim().to_lowercase();
    if q.is_empty() { return Vec::new(); }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut matches: Vec<Match> = Vec::new();

    for doc in docs {
        let lower = doc.text.to_lowercase();
        // total hit count across the doc (first word, the rarest-ish anchor)
        let anchor = words[0];
        let hits = lower.matches(anchor).count();
        if hits == 0 { continue; }

        // find the best line: the first line containing ALL query words
        let mut best: Option<(usize, &str)> = None;
        for (i, line) in doc.text.lines().enumerate() {
            let ll = line.to_lowercase();
            if words.iter().all(|w| ll.contains(w)) {
                best = Some((i + 1, line));
                break;
            }
        }
        // fall back to the first line containing the anchor word
        let (line_no, line) = match best {
            Some(b) => b,
            None => {
                let mut found = None;
                for (i, line) in doc.text.lines().enumerate() {
                    if line.to_lowercase().contains(anchor) { found = Some((i + 1, line)); break; }
                }
                match found { Some(f) => f, None => continue }
            }
        };

        let snippet = make_snippet(line, anchor, 120);
        let all_words = words.iter().all(|w| line.to_lowercase().contains(w));
        let score = score_match(doc.path, &q, hits, all_words);
        matches.push(Match {
            path: doc.path.to_string(), line_no, snippet, hits, score,
        });
    }

    matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.path.cmp(&b.path)));
    matches.truncate(max_results);
    matches
}

/// Trim a matched line to a window centered on the first hit, with ellipses.
pub fn make_snippet(line: &str, needle: &str, width: usize) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= width { return trimmed.to_string(); }
    let lower = trimmed.to_lowercase();
    let hit = lower.find(needle).unwrap_or(0);
    // center a `width`-char window on the hit (by char, not byte)
    let chars: Vec<char> = trimmed.chars().collect();
    let hit_char = trimmed[..hit].chars().count();
    let half = width / 2;
    let start = hit_char.saturating_sub(half);
    let end = (start + width).min(chars.len());
    let start = end.saturating_sub(width);
    let mut s = String::new();
    if start > 0 { s.push('…'); }
    s.extend(&chars[start..end]);
    if end < chars.len() { s.push('…'); }
    s
}

/// Relevance score: more hits → higher; whole-query-on-one-line → boost;
/// a hit in the FILENAME (path) → boost (the note is "about" the term); a
/// shorter path (less nested) is a mild tiebreak toward top-level notes.
fn score_match(path: &str, query: &str, hits: usize, all_words_on_line: bool) -> f32 {
    let mut s = 1.0 + (hits as f32).ln_1p(); // diminishing returns on count
    if all_words_on_line { s *= 1.5; }
    let fname = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if fname.contains(query) { s *= 2.0; } // term is in the filename
    // mild preference for shallower paths
    let depth = path.matches('/').count() as f32;
    s *= 1.0 / (1.0 + depth * 0.03);
    s
}

/// Display name for a path (filename without extension).
pub fn note_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') { Some((n, _)) => n.to_string(), None => base.to_string() }
}

/// v0.1.681: wrap each query-word occurrence in a snippet with «» sentinels
/// so the dropdown can bold the match (e.g. "the «cloak» shimmers"). Case-
/// insensitive match, preserves the original case in the output. Sentinels
/// are chosen to be tab/newline-free and effectively absent from source.
/// Pure + testable.
pub fn highlight_terms(snippet: &str, query: &str) -> String {
    let words: Vec<String> = query.to_lowercase().split_whitespace()
        .filter(|w| w.len() >= 2).map(|w| w.to_string()).collect();
    if words.is_empty() { return snippet.to_string(); }
    let lower = snippet.to_lowercase();
    // collect (start,end) byte ranges of matches, longest words first so a
    // multi-word query doesn't double-wrap overlaps.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for w in &words {
        let mut from = 0;
        while let Some(p) = lower[from..].find(w.as_str()) {
            let s = from + p;
            let e = s + w.len();
            ranges.push((s, e));
            from = e;
        }
    }
    if ranges.is_empty() { return snippet.to_string(); }
    ranges.sort();
    // merge overlaps
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 { last.1 = last.1.max(e); continue; }
        }
        merged.push((s, e));
    }
    let mut out = String::with_capacity(snippet.len() + merged.len() * 2);
    let mut cur = 0;
    for (s, e) in merged {
        out.push_str(&snippet[cur..s]);
        out.push('\u{00AB}'); // «
        out.push_str(&snippet[s..e]);
        out.push('\u{00BB}'); // »
        cur = e;
    }
    out.push_str(&snippet[cur..]);
    out
}

/// Wire format for the dropdown: "path\tline_no\tsnippet" per match,
/// '\n'-joined. (Snippet has tabs/newlines stripped.)
pub fn to_wire_q(matches: &[Match], query: &str) -> String {
    matches.iter()
        .map(|m| {
            let snip = highlight_terms(&m.snippet, query).replace(['\t', '\n'], " ");
            format!("{}\t{}\t{}", m.path, m.line_no, snip)
        })
        .collect::<Vec<_>>().join("\n")
}

/// Wire format (no highlighting) — kept for callers that don't pass a query.
pub fn to_wire(matches: &[Match]) -> String {
    matches.iter()
        .map(|m| format!("{}\t{}\t{}", m.path, m.line_no,
            m.snippet.replace(['\t', '\n'], " ")))
        .collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs() -> Vec<(&'static str, &'static str)> {
        vec![
            ("/n/Clones.md", "The clone wakes.\nIt wears a cloak of shadow.\nNo cloak can hide it."),
            ("/n/Cloak_device.md", "The cloaking field hums.\nA cloak shimmers."),
            ("/n/Beach.md", "He cannot save everyone.\nThe water is cold."),
        ]
    }

    #[test]
    fn finds_content_not_just_filenames() {
        let d = docs(); let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let r = search("cloak", &dv, 10);
        let paths: Vec<&str> = r.iter().map(|m| m.path.as_str()).collect();
        // both cloak-containing notes found; the beach note is not
        assert!(paths.contains(&"/n/Clones.md"));
        assert!(paths.contains(&"/n/Cloak_device.md"));
        assert!(!paths.contains(&"/n/Beach.md"));
    }

    #[test]
    fn filename_match_ranks_higher() {
        let d = docs(); let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let r = search("cloak", &dv, 10);
        // Cloak_device.md has "cloak" in the filename → ranks above Clones.md
        assert_eq!(r[0].path, "/n/Cloak_device.md");
    }

    #[test]
    fn snippet_centers_on_hit() {
        let line = "A very long preamble of words before the important cloak appears and then more trailing words follow after it here";
        let s = make_snippet(line, "cloak", 40);
        assert!(s.contains("cloak"));
        assert!(s.starts_with('…') || s.len() < line.len());
    }

    #[test]
    fn multi_word_and_match() {
        let d = vec![("/n/a.md", "the red cloak\nthe blue hat")];
        let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let r = search("red cloak", &dv, 10);
        assert_eq!(r.len(), 1);
        assert!(r[0].snippet.contains("red cloak"));
    }

    #[test]
    fn line_number_is_one_based() {
        let d = vec![("/n/a.md", "first\nsecond cloak\nthird")];
        let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let r = search("cloak", &dv, 10);
        assert_eq!(r[0].line_no, 2);
    }

    #[test]
    fn empty_query_no_results() {
        let d = docs(); let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        assert!(search("   ", &dv, 10).is_empty());
    }

    #[test]
    fn hit_count_breaks_ties_among_body_matches() {
        // both body-only matches (no filename hit); more hits wins
        let d = vec![
            ("/n/many.md", "cloak cloak cloak everywhere"),
            ("/n/one.md", "just one cloak here"),
        ];
        let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let r = search("cloak", &dv, 10);
        assert_eq!(r[0].path, "/n/many.md");
    }

    #[test]
    fn case_insensitive() {
        let d = vec![("/n/a.md", "The CLOAK of Invisibility")];
        let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        assert_eq!(search("cloak", &dv, 10).len(), 1);
    }

    #[test]
    fn highlight_wraps_matches() {
        assert_eq!(highlight_terms("the cloak shimmers", "cloak"),
                   "the \u{00AB}cloak\u{00BB} shimmers");
        // case-insensitive match, original case preserved
        assert_eq!(highlight_terms("The CLOAK of night", "cloak"),
                   "The \u{00AB}CLOAK\u{00BB} of night");
        // multi-word: each wrapped
        let h = highlight_terms("a red cloak here", "red cloak");
        assert!(h.contains("\u{00AB}red\u{00BB}") && h.contains("\u{00AB}cloak\u{00BB}"));
        // no match → unchanged
        assert_eq!(highlight_terms("nothing here", "cloak"), "nothing here");
        // adjacent matches merge into ONE highlight span (correct)
        let h2 = highlight_terms("cloakcloak", "cloak");
        assert_eq!(h2.matches('\u{00AB}').count(), 1);
        assert_eq!(h2, "\u{00AB}cloakcloak\u{00BB}");
    }

    #[test]
    fn to_wire_q_includes_highlight() {
        let d = vec![("/n/a.md", "x\ncloak line\ny")];
        let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let w = to_wire_q(&search("cloak", &dv, 10), "cloak");
        assert!(w.contains('\u{00AB}'), "wire carries highlight sentinels");
    }

    #[test]
    fn wire_round_trips_fields() {
        let d = vec![("/n/a.md", "x\ncloak line\ny")];
        let dv: Vec<Doc> = d.iter().map(|(p,t)| Doc{path:p,text:t}).collect();
        let w = to_wire(&search("cloak", &dv, 10));
        let parts: Vec<&str> = w.split('\t').collect();
        assert_eq!(parts[0], "/n/a.md");
        assert_eq!(parts[1], "2");
        assert!(parts[2].contains("cloak"));
    }
}
