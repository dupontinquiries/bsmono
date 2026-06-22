// src/search_suggest.rs
//
// v0.1.420 (search-bar enhancement): aggregate ranked
// suggestions for the URL/search bar from the data sources we
// already have — visited-page history (archive), bookmarks,
// the path-fuzzy file index — plus a couple of always-available
// quick actions (web search, internal pages).
//
// The bar was type-and-go until now. This gives it a dropdown
// like a real browser's omnibox.
//
// Design: this module is PURE — it takes already-collected
// (url, title) pairs and a query, and returns ranked, typed
// suggestions. main.rs gathers the inputs (locking the archive,
// reading bookmarks, fuzzy-matching the path index) and calls
// build(). Keeping the ranking here means it's unit-testable
// without a Backend, an archive on disk, or a PTY.

#![allow(dead_code)]

/// What kind of thing a suggestion points at. QML uses this to
/// pick an icon and decide what tapping does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestKind {
    /// A previously-visited page (from the archive). Tapping
    /// navigates to `target` (a URL).
    History,
    /// A saved bookmark. Tapping navigates to `target`.
    Bookmark,
    /// A local file (from the path index). Tapping navigates to
    /// `target` (a file:// URL or path).
    File,
    /// A web search for the typed text. `target` is the query.
    WebSearch,
    /// An internal web2qml:// page (sessions, recipes, etc.).
    Internal,
    /// v0.1.600: a command's help / man page. Tapping runs `man <cmd>`
    /// (or tldr) in the terminal. `target` is the command name; the
    /// label shows the one-line summary. Surfaces when the typed query
    /// prefixes a known command name.
    ManPage,
}

impl SuggestKind {
    /// Single-letter tag for the QML wire format.
    pub fn tag(self) -> char {
        match self {
            SuggestKind::History => 'h',
            SuggestKind::Bookmark => 'b',
            SuggestKind::File => 'f',
            SuggestKind::WebSearch => 'w',
            SuggestKind::Internal => 'i',
            SuggestKind::ManPage => 'm',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub kind: SuggestKind,
    /// Human-readable label shown in the dropdown.
    pub label: String,
    /// What tapping acts on: a URL, a path, or a search query.
    pub target: String,
    /// Internal ranking score (higher = better). Not sent to QML.
    pub score: i32,
}

/// A candidate (url, title) pair from a data source.
pub struct Entry {
    pub url: String,
    pub title: String,
}

/// Score how well `query` matches a (url, title). Returns None
/// if no match. Higher is better.
///   - title/url starts with query  → strong (prefix match)
///   - word in title starts with query → good
///   - title/url merely contains query → weak
fn match_score(query: &str, url: &str, title: &str) -> Option<i32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        // Empty query: everything is a (low-score) candidate, so
        // recent/bookmarked items can show as the default list.
        return Some(1);
    }
    let url_l = url.to_lowercase();
    let title_l = title.to_lowercase();

    // Prefix on the displayed host or title is the strongest
    // signal — that's what the user is most likely typing toward.
    let host = host_of(&url_l);
    if host.starts_with(&q) {
        return Some(100 - q.len().min(50) as i32); // shorter q ranked a touch lower (less specific)
    }
    if title_l.starts_with(&q) {
        return Some(90);
    }
    // Word-boundary prefix in the title (e.g. "doc" matches
    // "Rust Documentation").
    if title_l.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(&q) && !w.is_empty())
    {
        return Some(70);
    }
    // Substring anywhere.
    if title_l.contains(&q) || url_l.contains(&q) {
        return Some(40);
    }
    None
}

/// Extract the host portion of a URL for prefix matching.
/// Cheap and scheme-tolerant; returns the original string when
/// there's no recognizable host.
fn host_of(url: &str) -> String {
    let after_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

/// Build the ranked suggestion list.
///
/// Inputs are already-collected candidates from each source.
/// `caps` controls how many of each kind survive (after ranking)
/// so one source can't flood the list.
pub fn build(
    query: &str,
    history: &[Entry],
    bookmarks: &[Entry],
    files: &[String],
    cap_per_kind: usize,
    total_cap: usize,
) -> Vec<Suggestion> {
    let q = query.trim();
    let mut out: Vec<Suggestion> = Vec::new();

    // Always offer a web search for non-empty, non-URL-looking
    // queries. Put it first when the query clearly isn't a
    // navigation target; otherwise it sits after matches.
    let looks_like_url = q.contains('.') && !q.contains(' ')
        || q.starts_with("http://") || q.starts_with("https://")
        || q.starts_with("file://") || q.starts_with("web2qml://");

    // Bookmarks (ranked, capped).
    let mut bm: Vec<Suggestion> = bookmarks.iter()
        .filter_map(|e| match_score(q, &e.url, &e.title).map(|s| Suggestion {
            kind: SuggestKind::Bookmark,
            label: if e.title.trim().is_empty() { e.url.clone() } else { e.title.clone() },
            target: e.url.clone(),
            // Bookmarks are user-curated → small boost over history.
            score: s + 10,
        }))
        .collect();
    sort_dedup_cap(&mut bm, cap_per_kind);
    out.append(&mut bm);

    // History (ranked, capped).
    let mut hist: Vec<Suggestion> = history.iter()
        .filter_map(|e| match_score(q, &e.url, &e.title).map(|s| Suggestion {
            kind: SuggestKind::History,
            label: if e.title.trim().is_empty() { e.url.clone() } else { e.title.clone() },
            target: e.url.clone(),
            score: s,
        }))
        .collect();
    sort_dedup_cap(&mut hist, cap_per_kind);
    out.append(&mut hist);

    // Files (path index already fuzzy-matched upstream; preserve
    // its order, just cap + wrap).
    for path in files.iter().take(cap_per_kind) {
        out.push(Suggestion {
            kind: SuggestKind::File,
            label: path.clone(),
            target: if path.starts_with("file://") || path.starts_with('/') {
                path.clone()
            } else {
                format!("file://{}", path)
            },
            score: 50,
        });
    }

    // Internal-page shortcuts when the query prefixes one.
    for (name, url) in INTERNAL_PAGES {
        if !q.is_empty() && name.starts_with(&q.to_lowercase()) {
            out.push(Suggestion {
                kind: SuggestKind::Internal,
                label: format!("{} (web2qml)", name),
                target: url.to_string(),
                score: 60,
            });
        }
    }

    // Cross-source rank: stable sort by score desc. We DON'T
    // re-dedup across kinds (a URL can legitimately be both a
    // bookmark and in history; bookmark wins on score and we
    // drop the history dup here).
    out.sort_by(|a, b| b.score.cmp(&a.score));
    dedup_by_target(&mut out);

    // Web-search suggestion: prepend if the query isn't URL-like,
    // else append. Only for non-empty queries.
    if !q.is_empty() {
        let ws = Suggestion {
            kind: SuggestKind::WebSearch,
            label: format!("Search the web for \u{201C}{}\u{201D}", q),
            target: q.to_string(),
            score: 0,
        };
        if looks_like_url {
            out.push(ws);
        } else {
            out.insert(0, ws);
        }
    }

    out.truncate(total_cap);
    out
}

const INTERNAL_PAGES: &[(&str, &str)] = &[
    ("sessions", "web2qml://sessions"),
    ("recipes", "web2qml://recipes"),
    ("gemini", "web2qml://gemini"),
    ("chat", "web2qml://chat"),
    ("home", "web2qml://home"),
];

fn sort_dedup_cap(v: &mut Vec<Suggestion>, cap: usize) {
    v.sort_by(|a, b| b.score.cmp(&a.score));
    dedup_by_target(v);
    v.truncate(cap);
}

fn dedup_by_target(v: &mut Vec<Suggestion>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.target.clone()));
}

/// v0.1.600: command-help (man page) suggestions. Given the query and a
/// list of (command_name, one_line_summary) pairs, return ManPage
/// suggestions for commands the query prefixes or matches. Kept SEPARATE
/// from build() so it's purely additive — the caller appends these to
/// build()'s output, then re-ranks. This avoids changing build()'s
/// signature (and its FFI call site).
///
/// Matching: a command whose NAME starts with the query ranks highest
/// (you typed "tar" → tar), a name that contains the query ranks lower,
/// and a query that matches a word in the SUMMARY ranks lowest (typed
/// "compress" → finds gzip/tar). Returns at most `cap`, best-first.
/// `target` is the bare command name; the executor decides `man <name>`
/// vs tldr. Empty/very-short queries return nothing (too noisy).
pub fn suggest_commands(
    query: &str,
    commands: &[(String, String)],
    cap: usize,
) -> Vec<Suggestion> {
    let q = query.trim().to_lowercase();
    // Require 2+ chars: a 1-char query would match a huge swath of
    // commands and bury the real navigation suggestions.
    if q.len() < 2 {
        return Vec::new();
    }
    let mut out: Vec<Suggestion> = Vec::new();
    for (name, summary) in commands {
        let name_l = name.to_lowercase();
        let score = if name_l == q {
            120 // exact command name — strongest
        } else if name_l.starts_with(&q) {
            100 // prefix of the command name
        } else if name_l.contains(&q) {
            70 // substring of the name
        } else if summary.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| w == q)
        {
            40 // a whole word in the summary ("compress" → gzip)
        } else {
            continue;
        };
        let label = if summary.trim().is_empty() {
            format!("{} — man page", name)
        } else {
            format!("{} — {}", name, summary.trim())
        };
        out.push(Suggestion {
            kind: SuggestKind::ManPage,
            label,
            target: name.clone(),
            score,
        });
    }
    sort_dedup_cap(&mut out, cap);
    out
}

/// Serialize suggestions for QML: one per line,
/// `tag\x1Flabel\x1Ftarget`.
pub fn to_wire(suggestions: &[Suggestion]) -> String {
    let mut out = String::with_capacity(suggestions.len() * 64);
    for s in suggestions {
        out.push(s.kind.tag());
        out.push('\x1F');
        out.push_str(&s.label);
        out.push('\x1F');
        out.push_str(&s.target);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(url: &str, title: &str) -> Entry {
        Entry { url: url.to_string(), title: title.to_string() }
    }

    fn cmd(n: &str, s: &str) -> (String, String) {
        (n.to_string(), s.to_string())
    }

    #[test]
    fn suggest_commands_prefix_and_exact() {
        let cmds = vec![
            cmd("tar", "archive files"),
            cmd("tac", "concatenate and print files in reverse"),
            cmd("grep", "find lines matching a pattern"),
        ];
        let got = suggest_commands("tar", &cmds, 5);
        // exact "tar" must rank first; "tac" doesn't match "tar" at all
        assert_eq!(got[0].kind, SuggestKind::ManPage);
        assert_eq!(got[0].target, "tar");
        assert!(got[0].label.contains("archive files"));
        // "tac" shares no "tar" substring → not present
        assert!(!got.iter().any(|s| s.target == "tac"));
    }

    #[test]
    fn suggest_commands_matches_summary_word() {
        let cmds = vec![
            cmd("gzip", "compress or expand files"),
            cmd("ls", "list directory contents"),
        ];
        // "compress" matches gzip's summary word, not its name
        let got = suggest_commands("compress", &cmds, 5);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target, "gzip");
    }

    #[test]
    fn suggest_commands_ignores_tiny_query() {
        let cmds = vec![cmd("tar", "archive files")];
        // 1-char query is too noisy → nothing
        assert!(suggest_commands("t", &cmds, 5).is_empty());
        assert!(suggest_commands("", &cmds, 5).is_empty());
    }

    #[test]
    fn suggest_commands_respects_cap() {
        let cmds: Vec<_> = (0..10)
            .map(|i| cmd(&format!("cmd{}", i), "does a thing"))
            .collect();
        // all start with "cmd" → prefix matches, but cap limits
        let got = suggest_commands("cmd", &cmds, 3);
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn manpage_kind_wire_tag() {
        // the new kind must serialize with its own tag
        assert_eq!(SuggestKind::ManPage.tag(), 'm');
    }

    #[test]
    fn host_prefix_ranks_high() {
        let hist = vec![e("https://github.com/x", "GitHub X")];
        let r = build("git", &hist, &[], &[], 5, 10);
        // A history hit + a web search.
        assert!(r.iter().any(|s| s.kind == SuggestKind::History
            && s.target == "https://github.com/x"));
    }

    #[test]
    fn bookmark_beats_history_for_same_query() {
        let hist = vec![e("https://example.com/a", "Example A")];
        let bm = vec![e("https://example.com/b", "Example B")];
        let r = build("example", &hist, &bm, &[], 5, 10);
        // First non-websearch suggestion should be the bookmark
        // (curated boost).
        let first_nav = r.iter().find(|s| s.kind != SuggestKind::WebSearch).unwrap();
        assert_eq!(first_nav.kind, SuggestKind::Bookmark);
    }

    #[test]
    fn websearch_first_for_plain_text() {
        let r = build("how to flatten a list", &[], &[], &[], 5, 10);
        assert_eq!(r[0].kind, SuggestKind::WebSearch);
        assert_eq!(r[0].target, "how to flatten a list");
    }

    #[test]
    fn websearch_last_for_urlish() {
        let hist = vec![e("https://example.com", "Example")];
        let r = build("example.com", &hist, &[], &[], 5, 10);
        // URL-like query → web search is a fallback at the end.
        assert_eq!(r.last().unwrap().kind, SuggestKind::WebSearch);
    }

    #[test]
    fn files_become_file_urls() {
        let files = vec!["/home/u/notes.md".to_string(), "docs/readme.md".to_string()];
        let r = build("note", &[], &[], &files, 5, 10);
        let f = r.iter().find(|s| s.kind == SuggestKind::File).unwrap();
        assert_eq!(f.target, "/home/u/notes.md"); // absolute path kept as-is
        let f2 = r.iter().filter(|s| s.kind == SuggestKind::File)
            .find(|s| s.label == "docs/readme.md").unwrap();
        assert_eq!(f2.target, "file://docs/readme.md"); // relative gets scheme
    }

    #[test]
    fn internal_page_prefix() {
        let r = build("sess", &[], &[], &[], 5, 10);
        assert!(r.iter().any(|s| s.kind == SuggestKind::Internal
            && s.target == "web2qml://sessions"));
    }

    #[test]
    fn dedup_across_sources() {
        // Same URL in both history and bookmarks → one entry.
        let hist = vec![e("https://dup.com", "Dup")];
        let bm = vec![e("https://dup.com", "Dup")];
        let r = build("dup", &hist, &bm, &[], 5, 10);
        let count = r.iter().filter(|s| s.target == "https://dup.com").count();
        assert_eq!(count, 1, "duplicate target should appear once");
    }

    #[test]
    fn empty_query_shows_recents_no_websearch() {
        let hist = vec![e("https://a.com", "A"), e("https://b.com", "B")];
        let r = build("", &hist, &[], &[], 5, 10);
        // No web-search row for an empty query.
        assert!(!r.iter().any(|s| s.kind == SuggestKind::WebSearch));
        // History items present.
        assert!(r.iter().any(|s| s.kind == SuggestKind::History));
    }

    #[test]
    fn total_cap_respected() {
        let hist: Vec<Entry> = (0..50)
            .map(|i| e(&format!("https://s{}.com", i), &format!("Site {}", i)))
            .collect();
        let r = build("site", &hist, &[], &[], 20, 8);
        assert!(r.len() <= 8);
    }

    #[test]
    fn wire_format_roundtrip_shape() {
        let s = vec![Suggestion {
            kind: SuggestKind::History,
            label: "GitHub".to_string(),
            target: "https://github.com".to_string(),
            score: 90,
        }];
        let w = to_wire(&s);
        assert_eq!(w, "h\u{1f}GitHub\u{1f}https://github.com\n");
    }
}
