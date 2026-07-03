//! gitmind (`gin`) voice control — the CLI sibling of tmux_voice /
//! zellij_voice / ssh_voice. Maps spoken phrases onto `gin` subcommands so
//! the gitmind checkpoint workflow (snapshot / timeline / search / restore)
//! is drivable by voice when you're inside a gitmind-managed repo.
//!
//! Clean-room from the `gin` CLI surface (apps/gitmind/crates/cli): the
//! subcommands and their short aliases are the public contract. Detection is
//! by the presence of a `.gitmind` directory in the cwd (or an ancestor) —
//! the repo marker that `gin` itself scaffolds — handled in the qt_method
//! wrapper; this module stays pure + testable (no I/O).
//!
//! Action shape mirrors TmuxAction: an argv after the `gin` binary, plus a
//! `confirm` flag for state-overwriting commands (restore/extract) and a
//! `query` flag for read-only inspection (status/timeline/diff/...), so the
//! UI can echo the right hint and gate destructive ones behind a tap.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GinAction {
    /// argv after the `gin` binary, e.g. ["snapshot"] or ["search","auth"].
    pub argv: Vec<String>,
    /// state-overwriting enough to confirm (restore/extract a checkpoint).
    pub confirm: bool,
    /// reads state (status/timeline/diff/where) rather than mutating.
    pub query: bool,
}

impl GinAction {
    fn act(parts: &[&str]) -> Option<GinAction> {
        Some(GinAction {
            argv: parts.iter().map(|s| s.to_string()).collect(),
            confirm: false,
            query: false,
        })
    }
    fn act_query(parts: &[&str]) -> Option<GinAction> {
        Some(GinAction { query: true, ..Self::act(parts)? })
    }
    fn act_confirm(parts: &[&str]) -> Option<GinAction> {
        Some(GinAction { confirm: true, ..Self::act(parts)? })
    }
}

/// The `.gitmind` repo marker `gin` scaffolds. Used by the qt_method wrapper
/// (which walks the cwd + ancestors for it) to decide whether gitmind voice
/// is the active context; kept here so the constant lives with the grammar.
pub const REPO_MARKER: &str = ".gitmind";

/// Vocabulary shown when gitmind is the active context (the help overlay's
/// "commands list" for this grammar). Label → one-line gloss.
pub fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("status", "what changed since the last checkpoint"),
        ("take a snapshot / checkpoint", "snapshot the working tree"),
        ("snapshot as NAME", "labelled checkpoint"),
        ("show timeline", "list checkpoints over time"),
        ("search for X", "search the history / mind"),
        ("diff", "show what differs from the checkpoint"),
        ("review", "review the pending change set"),
        ("maintain / garbage collect", "compact the object store"),
        ("restore checkpoint X", "extract a checkpoint (confirms)"),
        ("where am i / repo root", "print the gitmind repo root"),
    ]
}

/// Map a spoken phrase → a `gin` subcommand. None if not a gitmind command.
pub fn translate(phrase: &str) -> Option<GinAction> {
    let norm = normalize_phrase(phrase);
    let p = norm.as_str();
    if p.is_empty() {
        return None;
    }

    // ── snapshot / checkpoint (mutating, routine) ──
    // Checked early because "checkpoint"/"snapshot" are unambiguous gitmind
    // verbs. Optional trailing label: "snapshot as before-refactor",
    // "checkpoint called wip", "take a snapshot named baseline".
    let is_snapshot = p.contains("snapshot") || p.contains("checkpoint");
    if is_snapshot && !p.contains("restore") && !p.contains("extract")
        && !p.contains("timeline") && !p.contains("list") {
        if let Some(label) = snapshot_label(p) {
            return Some(GinAction {
                argv: vec!["snapshot".into(), label],
                confirm: false,
                query: false,
            });
        }
        return GinAction::act(&["snapshot"]);
    }

    // ── restore / extract a checkpoint (overwrites — confirm) ──
    // "restore checkpoint abc123", "extract abc123", "go back to checkpoint X".
    if p.contains("restore") || p.starts_with("extract") || p.contains("roll back")
        || p.contains("rollback") || (p.contains("go back") && p.contains("checkpoint")) {
        if let Some(id) = checkpoint_id(p) {
            return GinAction::act_confirm(&["extract", id.as_str()]);
        }
        // No id spoken → surface the timeline so the user can pick one.
        return GinAction::act_query(&["timeline"]);
    }

    // ── timeline / history (read-only) ──
    if p.contains("timeline") || p.contains("history")
        || (p.contains("list") && p.contains("checkpoint"))
        || p.contains("show checkpoints") {
        return GinAction::act_query(&["timeline"]);
    }

    // ── search / find (read-only, takes a term) ──
    if let Some(term) = search_term(p) {
        if !term.is_empty() {
            return Some(GinAction {
                argv: vec!["search".into(), term],
                confirm: false,
                query: true,
            });
        }
    }

    // ── diff (read-only) ──
    if p.contains("diff") || p.contains("what changed") || p.contains("whats changed")
        || (p.contains("show") && p.contains("changes")) {
        return GinAction::act_query(&["diff"]);
    }

    // ── status (read-only) ── checked after diff/what-changed so the more
    // specific phrasings win; plain "status" / "are we clean" land here.
    if p == "status" || p.starts_with("status") || p.contains("gitmind status")
        || p.contains("mind status") || p.contains("are we clean") {
        return GinAction::act_query(&["status"]);
    }

    // ── review (read-only) ──
    if p.contains("review") {
        return GinAction::act_query(&["review"]);
    }

    // ── maintain / gc (mutating, routine) ──
    if p.contains("maintain") || p.contains("garbage") || p == "gc"
        || p.contains("compact") || p.contains("clean up the store")
        || p.contains("cleanup") {
        return GinAction::act(&["maintain"]);
    }

    // ── where / repo root (read-only) ── note: normalize_phrase strips the
    // filler "i", so "where am i" arrives as "where am".
    if p == "where" || p.starts_with("where am") || p.starts_with("where are")
        || p.contains("repo root") || p.contains("which repo")
        || p.contains("project root") {
        return GinAction::act_query(&["where"]);
    }

    None
}

/// v0.1: normalize a spoken phrase for matching — lowercase, drop filler
/// words that don't change intent, collapse whitespace. Mirrors the
/// tmux/zellij normalizer so natural phrasings work ("can you take a
/// snapshot please" == "take snapshot"). NOTE: words that can carry a label
/// or search term ("as", "named", "for", "called") are deliberately NOT
/// stripped — `snapshot_label` / `search_term` rely on them.
pub fn normalize_phrase(phrase: &str) -> String {
    const FILLER: &[&str] = &[
        "a", "an", "the", "me", "my", "some", "please", "just", "bit", "to",
        "can", "you", "could", "would", "i", "want", "wanna", "lets", "let",
        "us", "give", "make", "do", "go",
    ];
    phrase
        .trim()
        .to_lowercase()
        .split_whitespace()
        .filter(|w| !FILLER.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pull a label out of a snapshot phrase, if one was spoken. Recognizes
/// "as X", "named X", "called X", "label X" — everything after the keyword
/// becomes the (space-joined) label. Returns None for a bare snapshot.
fn snapshot_label(p: &str) -> Option<String> {
    for kw in [" as ", " named ", " called ", " label ", " labeled ", " labelled "] {
        if let Some(idx) = p.find(kw) {
            let label = p[idx + kw.len()..].trim();
            if !label.is_empty() {
                return Some(label.to_string());
            }
        }
    }
    None
}

/// Pull a checkpoint id/name out of a restore/extract phrase. Takes the last
/// token after "checkpoint"/"extract"/"to", which is where the id lands once
/// fillers are stripped.
fn checkpoint_id(p: &str) -> Option<String> {
    let toks: Vec<&str> = p.split_whitespace().collect();
    // Prefer the token right after an anchor word.
    for anchor in ["checkpoint", "extract", "snapshot"] {
        if let Some(i) = toks.iter().position(|t| *t == anchor) {
            if let Some(id) = toks.get(i + 1) {
                if is_id_like(id) {
                    return Some((*id).to_string());
                }
            }
        }
    }
    // Otherwise the trailing token, if it looks like an id (not a stopword).
    if let Some(last) = toks.last() {
        if is_id_like(last) && !matches!(*last, "restore" | "extract" | "back" | "checkpoint") {
            return Some((*last).to_string());
        }
    }
    None
}

/// An id-like token: a short hash, a number, or a slug (letters/digits/-/_),
/// but not an English stopword we use for routing.
fn is_id_like(t: &str) -> bool {
    !t.is_empty()
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && t.chars().any(|c| c.is_ascii_alphanumeric())
}

/// Pull a search term out of "search for X" / "find X" / "look for X".
/// Returns None when the phrase isn't a search at all.
fn search_term(p: &str) -> Option<String> {
    for kw in ["search for ", "search ", "find ", "look for ", "look up "] {
        if let Some(rest) = p.strip_prefix(kw) {
            let term = rest.trim();
            return Some(term.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(p: &str) -> Vec<String> {
        translate(p).unwrap_or_else(|| panic!("expected a gin command for {p:?}")).argv
    }

    #[test]
    fn snapshot_variants() {
        assert_eq!(argv("take a snapshot"), vec!["snapshot"]);
        assert_eq!(argv("checkpoint"), vec!["snapshot"]);
        assert_eq!(argv("snapshot please"), vec!["snapshot"]);
        assert_eq!(argv("make a checkpoint"), vec!["snapshot"]);
        // labelled
        assert_eq!(argv("snapshot as baseline"), vec!["snapshot", "baseline"]);
        assert_eq!(argv("checkpoint called wip"), vec!["snapshot", "wip"]);
        assert_eq!(argv("take a snapshot named before refactor"),
                   vec!["snapshot", "before refactor"]);
        // snapshot is routine, not destructive
        assert!(!translate("take a snapshot").unwrap().confirm);
    }

    #[test]
    fn read_only_queries() {
        assert_eq!(argv("status"), vec!["status"]);
        assert_eq!(argv("show timeline"), vec!["timeline"]);
        assert_eq!(argv("history"), vec!["timeline"]);
        assert_eq!(argv("diff"), vec!["diff"]);
        assert_eq!(argv("review"), vec!["review"]);
        assert_eq!(argv("where am i"), vec!["where"]);
        assert_eq!(argv("repo root"), vec!["where"]);
        for q in ["status", "show timeline", "diff", "review", "where am i"] {
            assert!(translate(q).unwrap().query, "{q:?} should be a query");
            assert!(!translate(q).unwrap().confirm, "{q:?} shouldn't confirm");
        }
    }

    #[test]
    fn search_takes_a_term() {
        assert_eq!(argv("search for auth"), vec!["search", "auth"]);
        assert_eq!(argv("find the parser bug"), vec!["search", "parser bug"]);
        assert_eq!(argv("look for race condition"), vec!["search", "race condition"]);
        assert!(translate("search for auth").unwrap().query);
    }

    #[test]
    fn restore_extract_confirms() {
        let a = translate("restore checkpoint abc123").unwrap();
        assert_eq!(a.argv, vec!["extract", "abc123"]);
        assert!(a.confirm, "restoring a checkpoint overwrites — must confirm");

        assert_eq!(argv("extract def456"), vec!["extract", "def456"]);
        assert!(translate("extract def456").unwrap().confirm);

        // No id spoken → fall back to the timeline (read-only) so the user
        // can see the options rather than guessing.
        let none_id = translate("restore a checkpoint").unwrap();
        assert_eq!(none_id.argv, vec!["timeline"]);
        assert!(none_id.query && !none_id.confirm);
    }

    #[test]
    fn maintain_variants() {
        assert_eq!(argv("maintain"), vec!["maintain"]);
        assert_eq!(argv("garbage collect"), vec!["maintain"]);
        assert_eq!(argv("compact the store"), vec!["maintain"]);
        assert!(!translate("maintain").unwrap().query);
    }

    #[test]
    fn non_gitmind_is_none() {
        assert_eq!(translate("open google"), None);
        assert_eq!(translate("split the pane"), None);
        assert_eq!(translate("read this section"), None);
        assert_eq!(translate(""), None);
        assert_eq!(translate("the quick brown fox"), None);
    }

    #[test]
    fn commands_list_is_nonempty_and_labelled() {
        let c = commands();
        assert!(c.len() >= 8, "expected a real vocabulary");
        // every entry must be a (label, gloss) with both sides filled
        for (label, gloss) in &c {
            assert!(!label.is_empty() && !gloss.is_empty());
        }
    }

    #[test]
    fn id_like_rejects_words_keeps_hashes() {
        assert!(is_id_like("abc123"));
        assert!(is_id_like("42"));
        assert!(is_id_like("before-refactor"));
        assert!(!is_id_like(""));
        assert!(!is_id_like("!!"));
    }
}
