//! route — the deterministic half of the command palette's "go to via
//! route" search: turn a natural-language ask ("finding the button on the
//! wizard page for account setup") into a short, ranked list of places to
//! jump to, each with a one-line description — instant, before any model
//! is involved.
//!
//! Modeled directly on logscope's `filesummary.rs` two-tier design (its own
//! doc: "a structural digest — pure, instant, deterministic" plus "a model
//! summary — generated in the background"): this module is that first
//! tier for navigation instead of summarization. [`aperture_complete::
//! route_ai`] is the second tier — a prompt built from THIS module's
//! candidates, in the same request-building-without-wiring-a-live-engine
//! shape as `aperture_complete::edit`, for the same reason: an actual model
//! call needs a running provider this crate can't stand up or verify
//! itself.
//!
//! Why not just reuse `project_search`/`workspace_symbols` output directly?
//! Both answer a different question. `project_search` matches one literal
//! substring; a natural-language query is several content words that won't
//! all appear verbatim in one line ("the button" rarely appears near
//! "account setup" in source). `workspace_symbols` ranks by fuzzy match
//! against a single symbol NAME, not a multi-word description. This module
//! is the third question: "which of these keywords show up, and where,
//! scored by how many and how meaningfully" — cheap enough to run
//! synchronously on every keystroke's worth of typed query, unlike a model
//! call.

use crate::workspace_symbols::Symbol;
use std::path::{Path, PathBuf};

/// Where a candidate's score came from — shown so a person can tell "this
/// matched a function named `setup_wizard`" from "this matched because the
/// word `account` appears on this line," which are very different kinds of
/// confidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteKind {
    /// Matched a workspace symbol's name or signature.
    Symbol,
    /// Matched a line of file content (no symbol involved — a string
    /// literal, a comment, a UI label).
    Text,
}

/// One place the query might mean.
#[derive(Clone, Debug)]
pub struct RouteCandidate {
    pub path: PathBuf,
    pub rel: String,
    pub line: usize,
    pub kind: RouteKind,
    /// The symbol name (for `Symbol`) or the matched line, trimmed (for
    /// `Text`) — the short label a picker row shows.
    pub label: String,
    /// One line of extra context: the symbol's signature, or the line
    /// immediately after the text match (often the more informative one for
    /// a UI string — the label line itself is usually just the literal).
    pub snippet: String,
    /// How many distinct query keywords this candidate matched, weighted:
    /// a symbol-name hit counts more than a text hit, since a function
    /// named `account_setup_wizard` is a stronger signal than the phrase
    /// merely appearing in a comment three files away.
    pub score: i32,
}

/// Split a natural-language query into the words worth searching for:
/// lowercased, alphanumeric runs only, common stopwords dropped, short
/// tokens (<3 chars) dropped — "the button on the wizard page for account
/// setup" becomes `["button", "wizard", "page", "account", "setup"]`. Pure
/// and separately tested because query parsing is the part most likely to
/// need tuning as real queries come in.
pub fn keywords(query: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "for", "on", "in", "to", "of", "and", "or", "is", "are", "at", "with",
        "find", "finding", "locate", "show", "where", "me", "that", "this", "it", "into", "from",
    ];
    query
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Score and rank candidates for `query` across `symbols` (workspace symbol
/// index — see `workspace_symbols::index_files`) and `files`' content
/// (already-walked file list — see `file_finder::walk_files`, though this
/// crate doesn't depend on that egui-side module; the caller passes the
/// same list it already has). Capped at `limit`.
///
/// Deliberately conservative about what counts as a match: a candidate
/// needs at least one keyword hit to appear at all — an empty or
/// all-stopword query returns nothing rather than every file in the repo,
/// the same "don't dump everything" choice `project_search::search_files`
/// makes for an empty query.
pub fn candidates(root: &Path, files: &[PathBuf], symbols: &[Symbol], query: &str, limit: usize) -> Vec<RouteCandidate> {
    let kws = keywords(query);
    if kws.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<RouteCandidate> = Vec::new();

    // Symbol matches: a keyword found in the name scores double a keyword
    // found only in the signature — the name is what the code's author
    // chose to call the thing, which is usually the stronger signal.
    for sym in symbols {
        let name_lower = sym.item.name.to_lowercase();
        let sig_lower = sym.item.sig.to_lowercase();
        let mut score = 0i32;
        for kw in &kws {
            if name_lower.contains(kw.as_str()) {
                score += 2;
            } else if sig_lower.contains(kw.as_str()) {
                score += 1;
            }
        }
        if score > 0 {
            out.push(RouteCandidate {
                path: sym.path.clone(),
                rel: sym.rel.clone(),
                line: sym.item.line,
                kind: RouteKind::Symbol,
                label: sym.item.name.clone(),
                snippet: sym.item.sig.clone(),
                score,
            });
        }
    }

    // Text matches: scan each file's lines for keyword hits, one candidate
    // per line that hits at least one keyword, scored by how many distinct
    // keywords it hits (a line hitting 2 of 3 query words is a much
    // stronger candidate than one hitting only 1).
    const MAX_FILES: usize = 800;
    const MAX_HITS_PER_FILE: usize = 5;
    for f in files.iter().take(MAX_FILES) {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().into_owned();
        let lines: Vec<&str> = text.lines().collect();
        let mut hits_this_file = 0;
        for (i, line) in lines.iter().enumerate() {
            if hits_this_file >= MAX_HITS_PER_FILE {
                break;
            }
            let lower = line.to_lowercase();
            let score = kws.iter().filter(|kw| lower.contains(kw.as_str())).count() as i32;
            if score == 0 {
                continue;
            }
            let snippet = lines.get(i + 1).map(|s| s.trim().to_string()).unwrap_or_default();
            out.push(RouteCandidate {
                path: f.clone(),
                rel: rel.clone(),
                line: i,
                kind: RouteKind::Text,
                label: line.trim().to_string(),
                snippet,
                score,
            });
            hits_this_file += 1;
        }
    }

    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.rel.len().cmp(&b.rel.len())).then(a.rel.cmp(&b.rel)));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{OutlineItem, OutlineKind};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aperture-route-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sym(name: &str, sig: &str, path: &Path, rel: &str) -> Symbol {
        Symbol {
            item: OutlineItem { kind: OutlineKind::Fn, name: name.to_string(), sig: sig.to_string(), line: 3, end_line: 10 },
            path: path.to_path_buf(),
            rel: rel.to_string(),
        }
    }

    #[test]
    fn keywords_drops_stopwords_and_short_tokens() {
        let kws = keywords("the button on the wizard page for account setup");
        assert_eq!(kws, vec!["button", "wizard", "page", "account", "setup"]);
    }

    #[test]
    fn keywords_of_an_all_stopword_query_is_empty() {
        assert!(keywords("the for a of").is_empty());
    }

    #[test]
    fn empty_query_returns_no_candidates() {
        let root = tmp("empty-query");
        let out = candidates(&root, &[], &[], "", 5);
        assert!(out.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn symbol_name_match_outranks_a_text_only_match() {
        let root = tmp("symbol-vs-text");
        let sym_path = root.join("wizard.rs");
        let symbols = vec![sym("account_setup_wizard", "fn account_setup_wizard() {}", &sym_path, "wizard.rs")];

        let text_path = root.join("notes.md");
        std::fs::write(&text_path, "some unrelated note about the account setup flow\n").unwrap();
        let files = vec![text_path.clone()];

        let out = candidates(&root, &files, &symbols, "account setup wizard", 5);
        assert!(!out.is_empty());
        assert_eq!(out[0].kind, RouteKind::Symbol, "the symbol name hit should rank first: {out:?}");
        assert_eq!(out[0].rel, "wizard.rs");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_a_ui_label_by_text_match_with_no_matching_symbol() {
        let root = tmp("text-only");
        let path = root.join("wizard_page.rs");
        std::fs::write(&path, "fn build_ui() {\n    let btn = Button::new(\"Set up your account\");\n    btn.show();\n}\n").unwrap();
        let files = vec![path.clone()];

        let out = candidates(&root, &files, &[], "the button for account setup", 5);
        assert!(!out.is_empty(), "should find the button line by text match");
        assert_eq!(out[0].kind, RouteKind::Text);
        assert_eq!(out[0].rel, "wizard_page.rs");
        assert!(out[0].label.contains("Button::new"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn results_are_capped_at_the_limit() {
        let root = tmp("capped");
        let mut files = Vec::new();
        for i in 0..10 {
            let p = root.join(format!("f{i}.rs"));
            std::fs::write(&p, "// account setup wizard\n").unwrap();
            files.push(p);
        }
        let out = candidates(&root, &files, &[], "account setup wizard", 3);
        assert_eq!(out.len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_line_hitting_more_keywords_outranks_one_hitting_fewer() {
        let root = tmp("multi-keyword");
        let path = root.join("a.rs");
        std::fs::write(&path, "// account setup wizard button\n// just an account mention\n").unwrap();
        let files = vec![path.clone()];
        let out = candidates(&root, &files, &[], "account setup wizard button", 5);
        assert!(out[0].label.contains("wizard button"), "the line hitting all 4 keywords should rank first: {out:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
