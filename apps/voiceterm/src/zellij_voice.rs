//! v0.1.634: zellij voice/assisted control — PURE core.
//!
//! Design (confirmed against the zellij docs + the prior power-features.md
//! note): drive zellij through its `zellij action <subcommand>` CLI, NOT
//! keystrokes. The CLI is zellij's entire control surface, it's
//! mode-independent (keystrokes depend on which input mode zellij is in
//! and on the user's custom keybinds), and the query actions
//! (list-panes/list-tabs/dump-screen) return structured data for the
//! "assisted" side. This costs a non-zellij user nothing and lets a power
//! user opt in; even a user who prefers keys for navigation benefits from
//! the assisted READ side (which keys can't do).
//!
//! This module is the PURE brain: detection (am I in a zellij session?)
//! and phrase → argv mapping. It has NO side effects — it returns the argv
//! a caller would spawn (`zellij action ...`), so it's fully unit-testable.
//! Spawning the subprocess + wiring into the voice intent path is the
//! build-gated layer on top (see TODO_zellij_voice.md).

/// Is the terminal currently inside a zellij session? Detected via the
/// ZELLIJ env var (set inside a session) rather than the foreground
/// command — because inside zellij the foreground process is your shell or
/// editor, not `zellij`. Mirrors the prior power-features.md note.
///
/// `zellij_env` is the value of std::env::var("ZELLIJ").ok() captured by
/// the caller (kept as a param so this stays pure/testable).
pub fn in_zellij_session(zellij_env: Option<&str>) -> bool {
    // ZELLIJ is set to "0" (the client id) inside a session; any present,
    // non-empty value means we're inside one.
    matches!(zellij_env, Some(v) if !v.is_empty())
}

/// A translated zellij action: the argv to pass after `zellij` (i.e.
/// `["action", "new-pane", ...]`), plus whether the caller may want to
/// confirm first (destructive ops like closing a pane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijAction {
    /// argv AFTER the `zellij` binary. e.g. ["action","new-pane","-d","right"].
    pub argv: Vec<String>,
    /// True if destructive enough to confirm (close pane/tab).
    pub confirm: bool,
    /// True if this is a QUERY (reads state, returns JSON/text on stdout)
    /// rather than a mutation — the caller routes the output to the
    /// assisted layer instead of just firing and forgetting.
    pub query: bool,
}

impl ZellijAction {
    fn act(parts: &[&str]) -> Option<ZellijAction> {
        Some(ZellijAction {
            argv: std::iter::once("action").chain(parts.iter().copied())
                .map(|s| s.to_string()).collect(),
            confirm: false,
            query: false,
        })
    }
    fn act_confirm(parts: &[&str]) -> Option<ZellijAction> {
        Some(ZellijAction { confirm: true, ..Self::act(parts)? })
    }
    fn act_query(parts: &[&str]) -> Option<ZellijAction> {
        Some(ZellijAction { query: true, ..Self::act(parts)? })
    }
}

/// The voice-command vocabulary shown to the user when zellij is the
/// active context (label, example phrase).
pub fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("new pane", "split a new pane"),
        ("new pane down/right", "split in a direction"),
        ("floating pane", "new floating pane"),
        ("close pane", "close the focused pane (confirms)"),
        ("focus left/right/up/down", "move focus between panes"),
        ("new tab", "open a new tab"),
        ("next/previous tab", "switch tabs"),
        ("go to tab N", "jump to tab number N"),
        ("close tab", "close the current tab (confirms)"),
        ("rename tab NAME", "rename the current tab"),
        ("resize left/right/up/down", "resize the focused pane"),
        ("toggle fullscreen", "maximize/restore the pane"),
        ("toggle floating", "show/hide floating panes"),
        ("what panes / list panes", "read the layout (assisted)"),
        ("what tabs / list tabs", "read the tabs (assisted)"),
    ]
}

/// Map a spoken phrase to a `zellij action` invocation. Returns None if
/// the phrase isn't a recognized zellij command (caller falls through to
/// normal handling). Phrase is expected already lowercased/trimmed by the
/// caller, but we normalize defensively.
pub fn translate(phrase: &str) -> Option<ZellijAction> {
    // v0.1.706: strip filler words ("a/the/me/my/please/just…") and collapse
    // whitespace up front, so natural phrasings ("open a new pane", "give me a
    // pane") reach the canonical matchers below. Mirrors tmux_voice.
    let cleaned = {
        const FILLER: &[&str] = &["a", "an", "the", "me", "my", "some", "please",
            "just", "bit", "can", "you", "could", "would", "i", "want", "wanna",
            "lets", "let", "us", "give", "to"];
        phrase.trim().to_lowercase().split_whitespace()
            .filter(|w| !FILLER.contains(w))
            .collect::<Vec<_>>().join(" ")
    };
    let p = cleaned.as_str();

    // v0.1.707: contains-based intent matching (mirrors tmux_voice). Order
    // matters: floating-toggle before floating-create; explicit focus verbs
    // before splits; bare "pane" alone is NOT a command.
    let dir = pane_dir(p);

    // --- floating (toggle vs create) ---
    if p.contains("floating") && (p.contains("toggle") || p.contains("show") || p.contains("hide")) {
        return ZellijAction::act(&["toggle-floating-panes"]);
    }
    if p.contains("float") {
        return ZellijAction::act(&["new-pane", "-f"]);
    }

    // --- panes: focus movement — needs an explicit focus/move verb + dir ---
    let has_focus_verb = p.starts_with("focus") || p.starts_with("move focus")
        || p.starts_with("move ") || p.starts_with("select") || p.starts_with("go ");
    if has_focus_verb && !p.contains("split") && !p.contains("new") && !p.contains("open")
        && !p.contains("create") && !p.contains("tab") {
        if let Some(d) = dir {
            return ZellijAction::act(&["move-focus", d]);
        }
    }

    // --- panes: creation (split / new pane, optionally directional) ---
    let is_split = p.contains("split")
        || (p.contains("pane") && (p.contains("new") || p.contains("open")
            || p.contains("add") || p.contains("create") || p.contains("another")
            || p.contains("duplicate") || dir.is_some()));
    if is_split {
        return match dir {
            Some("left") | Some("right") => ZellijAction::act(&["new-pane", "-d", "right"]),
            Some(_) => ZellijAction::act(&["new-pane", "-d", "down"]),
            None => ZellijAction::act(&["new-pane"]),
        };
    }
    if (p.contains("close") || p.contains("kill") || p.contains("remove")) && p.contains("pane") {
        return ZellijAction::act_confirm(&["close-pane"]);
    }

    // --- panes: state ---
    if p.contains("fullscreen") || p.contains("maximize") || p.contains("zoom") {
        return ZellijAction::act(&["toggle-fullscreen"]);
    }
    if p.contains("floating") {
        return ZellijAction::act(&["toggle-floating-panes"]);
    }

    // --- panes: resize (grow/shrink/resize + direction) ---
    if (p.contains("resize") || p.contains("grow") || p.contains("wider")
        || p.contains("taller") || p.contains("bigger")) && !p.contains("window") {
        if let Some(d) = dir {
            return ZellijAction::act(&["resize", d]);
        }
    }

    // --- tabs ---
    if (p.contains("new") || p.contains("add") || p.contains("open") || p.contains("create"))
        && p.contains("tab") {
        return ZellijAction::act(&["new-tab"]);
    }
    if p.contains("next tab") || (p.contains("tab") && p.contains("next")) {
        return ZellijAction::act(&["go-to-next-tab"]);
    }
    if p.contains("tab") && (p.contains("previous") || p.contains("prev") || p.contains("last")) {
        return ZellijAction::act(&["go-to-previous-tab"]);
    }
    if (p.contains("close") || p.contains("kill")) && p.contains("tab") {
        return ZellijAction::act_confirm(&["close-tab"]);
    }
    // "go to tab N" / "tab N" / "switch to tab N"
    if let Some(n) = parse_tab_number(p) {
        return Some(ZellijAction {
            argv: vec!["action".into(), "go-to-tab".into(), n.to_string()],
            confirm: false,
            query: false,
        });
    }

    // --- assisted: queries (read state) ---
    let is_query = p.starts_with("list") || p.starts_with("what") || p.starts_with("which")
        || p.starts_with("show");
    if is_query && p.contains("pane") {
        return ZellijAction::act_query(&["list-panes", "--json"]);
    }
    if is_query && p.contains("tab") {
        return ZellijAction::act_query(&["query-tab-names"]);
    }

    None
}

/// Exact-or-contains match against a small set of phrasings.
#[allow(dead_code)]
fn matches(p: &str, options: &[&str]) -> bool {
    options.iter().any(|o| p == *o)
}

/// v0.1.707: spoken direction → zellij direction word. "to the left", "leftward",
/// "below"/"down", "above"/"up", "beside"/"right". None if no direction.
fn pane_dir(p: &str) -> Option<&'static str> {
    if p.contains("left") { Some("left") }
    else if p.contains("right") || p.contains("beside") { Some("right") }
    else if p.contains("up") || p.contains("above") { Some("up") }
    else if p.contains("down") || p.contains("below") { Some("down") }
    else { None }
}

/// v0.1.703 (#1 assisted read): summarize `zellij action query-tab-names`
/// output (one tab name per line) into a spoken sentence.
pub fn describe_tabs(output: &str) -> String {
    let names: Vec<&str> = output.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    match names.len() {
        0 => "No tabs open.".to_string(),
        1 => format!("One tab: {}.", names[0]),
        n => format!("{} tabs: {}.", n, names.join(", ")),
    }
}

/// v0.1.703 (#1): summarize `zellij action list-panes --json` output. The JSON
/// is a map of tab→[pane objects]; we don't pull a full JSON dep here — count
/// the pane records (objects with an "id" field) tolerantly and report a
/// count, naming any titled panes we can cheaply extract.
pub fn describe_panes(json: &str) -> String {
    // count "id": occurrences as a pane proxy (each pane object has one).
    let count = json.matches("\"id\"").count();
    // cheap title scrape: "title":"..." values
    let mut titles: Vec<String> = Vec::new();
    let mut rest = json;
    while let Some(k) = rest.find("\"title\"") {
        rest = &rest[k + 7..];
        if let Some(q1) = rest.find('"') {
            let after = &rest[q1 + 1..];
            if let Some(q2) = after.find('"') {
                let t = after[..q2].trim();
                if !t.is_empty() && t.len() < 40 { titles.push(t.to_string()); }
                rest = &after[q2 + 1..];
                continue;
            }
        }
        break;
    }
    match count {
        0 => "No panes found.".to_string(),
        1 => "One pane.".to_string(),
        n if titles.is_empty() => format!("{} panes.", n),
        n => format!("{} panes: {}.", n, titles.join(", ")),
    }
}

/// Parse "go to tab 3" / "tab 3" / "switch to tab 3" → Some(3). Only a
/// trailing integer after a tab keyword; returns None otherwise.
fn parse_tab_number(p: &str) -> Option<u32> {
    // v0.1.706/707: after filler-stripping, find "tab <n>" anywhere; accept a
    // digit or a number word ("tab three").
    let toks: Vec<&str> = p.split_whitespace().collect();
    for i in 0..toks.len() {
        if toks[i] == "tab" && i + 1 < toks.len() {
            let r = toks[i + 1];
            let n = r.parse::<u32>().ok().or_else(|| word_num(r));
            if let Some(n) = n.filter(|n| *n >= 1) {
                return Some(n);
            }
        }
    }
    None
}

/// number word → u32 (1-10), for "tab three".
fn word_num(s: &str) -> Option<u32> {
    Some(match s {
        "one" => 1, "two" => 2, "three" => 3, "four" => 4, "five" => 5,
        "six" => 6, "seven" => 7, "eight" => 8, "nine" => 9, "ten" => 10,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &Option<ZellijAction>) -> Vec<String> {
        a.as_ref().unwrap().argv.clone()
    }

    #[test]
    fn detects_session_via_env() {
        assert!(in_zellij_session(Some("0")));
        assert!(in_zellij_session(Some("session-name")));
        assert!(!in_zellij_session(Some("")));
        assert!(!in_zellij_session(None));
    }

    #[test]
    fn new_pane_variants() {
        assert_eq!(argv(&translate("new pane")), vec!["action", "new-pane"]);
        assert_eq!(argv(&translate("new pane down")), vec!["action", "new-pane", "-d", "down"]);
        assert_eq!(argv(&translate("new pane right")), vec!["action", "new-pane", "-d", "right"]);
        assert_eq!(argv(&translate("floating pane")), vec!["action", "new-pane", "-f"]);
    }

    #[test]
    fn close_pane_confirms() {
        let a = translate("close pane").unwrap();
        assert_eq!(a.argv, vec!["action", "close-pane"]);
        assert!(a.confirm, "closing a pane should be flagged to confirm");
    }

    #[test]
    fn focus_movement() {
        assert_eq!(argv(&translate("focus left")), vec!["action", "move-focus", "left"]);
        assert_eq!(argv(&translate("move focus up")), vec!["action", "move-focus", "up"]);
    }

    #[test]
    fn tabs() {
        assert_eq!(argv(&translate("new tab")), vec!["action", "new-tab"]);
        assert_eq!(argv(&translate("next tab")), vec!["action", "go-to-next-tab"]);
        assert_eq!(argv(&translate("previous tab")), vec!["action", "go-to-previous-tab"]);
    }

    #[test]
    fn go_to_tab_number() {
        assert_eq!(argv(&translate("go to tab 3")), vec!["action", "go-to-tab", "3"]);
        assert_eq!(argv(&translate("tab 2")), vec!["action", "go-to-tab", "2"]);
        assert_eq!(argv(&translate("switch to tab 5")), vec!["action", "go-to-tab", "5"]);
        // tab 0 is invalid (1-based)
        assert!(translate("tab 0").is_none() || !argv(&translate("tab 0")).contains(&"0".to_string()));
    }

    #[test]
    fn resize() {
        assert_eq!(argv(&translate("resize left")), vec!["action", "resize", "left"]);
        assert_eq!(argv(&translate("resize down")), vec!["action", "resize", "down"]);
    }

    #[test]
    fn queries_are_flagged() {
        let t = translate("what tabs").unwrap();
        assert!(t.query, "a read should be flagged as query");
        assert_eq!(t.argv, vec!["action", "query-tab-names"]);
        let p = translate("what panes").unwrap();
        assert!(p.query);
        assert_eq!(p.argv, vec!["action", "list-panes", "--json"]);
    }

    #[test]
    fn unknown_phrase_is_none() {
        assert!(translate("make me a sandwich").is_none());
        assert!(translate("git commit").is_none());
        // a partial/ambiguous word shouldn't fire a destructive action
        assert!(translate("pane").is_none());
    }

    #[test]
    fn close_tab_confirms() {
        let a = translate("close tab").unwrap();
        assert!(a.confirm);
        assert_eq!(a.argv, vec!["action", "close-tab"]);
    }

    #[test]
    fn describe_tabs_speaks() {
        assert_eq!(describe_tabs(""), "No tabs open.");
        assert_eq!(describe_tabs("main"), "One tab: main.");
        assert_eq!(describe_tabs("main\nlogs\nedit"), "3 tabs: main, logs, edit.");
    }

    #[test]
    fn natural_phrasings_v707() {
        // contains-based matching — natural speech, not just canonical forms
        let argv = |p: &str| translate(p).map(|a| a.argv.join(" ")).unwrap_or_default();
        assert_eq!(argv("open a new pane"), "action new-pane");
        assert_eq!(argv("give me a pane on the right"), "action new-pane -d right");
        assert_eq!(argv("focus left"), "action move-focus left");
        assert_eq!(argv("move focus down"), "action move-focus down");
        assert_eq!(argv("close the pane"), "action close-pane");
        assert_eq!(argv("what panes do i have"), "action list-panes --json");
        assert_eq!(argv("go to tab 2"), "action go-to-tab 2");
        assert_eq!(argv("toggle floating"), "action toggle-floating-panes");
        assert_eq!(argv("grow left"), "action resize left");
        // still rejects non-commands + bare noun
        assert!(translate("the quick brown fox").is_none());
        assert!(translate("pane").is_none());
    }
}
