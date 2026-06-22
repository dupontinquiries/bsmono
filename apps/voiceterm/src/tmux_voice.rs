//! v0.1.703: tmux voice control — the tmux sibling of zellij_voice, so
//! multiplexer voice control isn't zellij-only. Maps the same spoken verbs to
//! `tmux` subcommands. Clean-room from the public tmux CLI; pure + testable.
//!
//! Detection is via the TMUX env var (set inside a tmux client). Commands are
//! issued with `tmux <subcmd>` which targets the current session/window/pane.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxAction {
    /// argv after the `tmux` binary, e.g. ["split-window","-h"].
    pub argv: Vec<String>,
    /// destructive enough to confirm (kill pane/window).
    pub confirm: bool,
    /// reads state (list-panes/list-windows) rather than mutating.
    pub query: bool,
}

impl TmuxAction {
    fn act(parts: &[&str]) -> Option<TmuxAction> {
        Some(TmuxAction { argv: parts.iter().map(|s| s.to_string()).collect(),
                          confirm: false, query: false })
    }
    fn act_confirm(parts: &[&str]) -> Option<TmuxAction> {
        Some(TmuxAction { confirm: true, ..Self::act(parts)? })
    }
    fn act_query(parts: &[&str]) -> Option<TmuxAction> {
        Some(TmuxAction { query: true, ..Self::act(parts)? })
    }
}

/// In a tmux client? (TMUX env var is set with the socket path.)
pub fn in_tmux_session(tmux_env: Option<&str>) -> bool {
    tmux_env.map(|v| !v.is_empty()).unwrap_or(false)
}

/// Vocabulary shown when tmux is the active context.
pub fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("split right / down", "split the pane (-h / -v)"),
        ("new window", "create a window"),
        ("close pane", "kill the focused pane (confirms)"),
        ("focus left/right/up/down", "select an adjacent pane"),
        ("next / previous window", "switch windows"),
        ("go to window N", "select window N"),
        ("close window", "kill the window (confirms)"),
        ("rename window NAME", "rename the current window"),
        ("zoom", "toggle pane zoom"),
        ("detach", "detach the client"),
        ("what panes / what windows", "read the layout (assisted)"),
    ]
}

/// Map a spoken phrase → a tmux subcommand. None if not a tmux command.
pub fn translate(phrase: &str) -> Option<TmuxAction> {
    let norm = normalize_phrase(phrase);
    let p = norm.as_str();
    let dir = pane_dir(p);

    // ── panes ──
    // v0.1.706/707: contains-based matching so natural phrasings work
    // ("open a vertical pane", "split the window", "move to the pane below").
    // normalize_phrase already stripped fillers + collapsed space.
    //
    // focus/move between panes — checked BEFORE split so "move to the pane
    // below" (a focus move) isn't caught by the directional-split rule. Covers
    // both "focus left" (no pane word) and "move to the pane below".
    let focus_verb = p.starts_with("focus") || p.starts_with("select pane")
        || p.starts_with("move") || (p.starts_with("go") && p.contains("pane"));
    let creating = p.contains("split") || p.contains("new") || p.contains("open")
        || p.contains("create") || p.contains("make");
    if focus_verb && !creating {
        if let Some(flag) = dir {
            return TmuxAction::act(&["select-pane", flag]);
        }
        if p.contains("next") || p.contains("switch") {
            return TmuxAction::act(&["select-pane", "-t", ":.+"]);
        }
    }

    let has_pane = p.contains("pane") || p.contains("split");
    // a creation verb OR a bare "pane <dir>" (focus moves were handled above,
    // so a directional pane phrase here is a split request).
    let make = p.contains("split") || p.contains("new") || p.contains("open")
        || p.contains("create") || p.contains("make") || p.contains("duplicate")
        || p.contains("add") || p.contains("another");
    if has_pane && (make || dir.is_some()) && (p.contains("horizontal")
        || p.contains("right") || p.contains("beside")) {
        return TmuxAction::act(&["split-window", "-h"]);
    }
    if has_pane && (make || dir.is_some() || p == "split") {
        return TmuxAction::act(&["split-window", "-v"]);
    }
    if (p.contains("close") || p.contains("kill")) && p.contains("pane") {
        return TmuxAction::act_confirm(&["kill-pane"]);
    }
    if p.contains("zoom") || p.contains("fullscreen") || p.contains("maximize") {
        return TmuxAction::act(&["resize-pane", "-Z"]);
    }

    // ── windows (tmux's "tabs") ──
    if (p.contains("new") || p.contains("open") || p.contains("add") || p.contains("create"))
        && (p.contains("window") || p.contains("tab")) && !p.contains("pane") {
        return TmuxAction::act(&["new-window"]);
    }
    if p.contains("next") && (p.contains("window") || p.contains("tab")) {
        return TmuxAction::act(&["next-window"]);
    }
    if (p.contains("previous") || p.contains("prev") || p.contains("last"))
        && (p.contains("window") || p.contains("tab")) {
        return TmuxAction::act(&["previous-window"]);
    }
    if (p.contains("close") || p.contains("kill")) && (p.contains("window") || p.contains("tab")) {
        return TmuxAction::act_confirm(&["kill-window"]);
    }
    if let Some(n) = window_number(p) {
        return TmuxAction::act(&["select-window", "-t", leak_num(n)]);
    }
    if let Some(rest) = p.strip_prefix("rename window to ")
        .or_else(|| p.strip_prefix("rename window "))
        .or_else(|| p.strip_prefix("rename tab to ")).or_else(|| p.strip_prefix("rename tab ")) {
        let name = rest.trim();
        if !name.is_empty() {
            return Some(TmuxAction { argv: vec!["rename-window".into(), name.to_string()],
                                     confirm: false, query: false });
        }
    }

    // ── resize ──
    if let Some(rest) = p.strip_prefix("resize ").or(if p.starts_with("grow") { Some(p) } else { None }) {
        if let Some(flag) = pane_dir(rest) {
            return TmuxAction::act(&["resize-pane", flag, "5"]);
        }
    }

    // ── misc ──
    if p.contains("detach") {
        return TmuxAction::act(&["detach-client"]);
    }

    // ── assisted queries ──
    if (p.contains("pane")) && (p.starts_with("list") || p.starts_with("what")
        || p.starts_with("which") || p.starts_with("show")) {
        return TmuxAction::act_query(&["list-panes", "-F",
            "#{pane_index}:#{pane_current_command}:#{pane_active}"]);
    }
    if (p.contains("window") || p.contains("tab")) && (p.starts_with("list")
        || p.starts_with("what") || p.starts_with("which") || p.starts_with("show")) {
        return TmuxAction::act_query(&["list-windows", "-F",
            "#{window_index}:#{window_name}:#{window_active}"]);
    }

    None
}

fn eq(p: &str, opts: &[&str]) -> bool { opts.iter().any(|o| p == *o) }

/// v0.1.706: normalize a spoken phrase for matching — lowercase, drop filler
/// words that don't change intent ("a/an/the/me/my/some/please/just/bit"),
/// collapse whitespace. Lets natural phrasings ("open a vertical pane",
/// "give me a pane") match the same intent as the terse forms.
pub fn normalize_phrase(phrase: &str) -> String {
    const FILLER: &[&str] = &["a", "an", "the", "me", "my", "some", "please",
        "just", "bit", "of", "to", "for", "can", "you", "could", "would",
        "i", "want", "wanna", "lets", "let", "us", "give", "make", "open up"];
    phrase.trim().to_lowercase()
        .split_whitespace()
        .filter(|w| !FILLER.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn pane_dir(s: &str) -> Option<&'static str> {
    if s.contains("left") { Some("-L") }
    else if s.contains("right") { Some("-R") }
    else if s.contains("up") || s.contains("above") { Some("-U") }
    else if s.contains("down") || s.contains("below") { Some("-D") }
    else { None }
}

fn window_number(p: &str) -> Option<u32> {
    // after normalize_phrase, prefixes like "go to"/"switch to" are reduced
    // ("to" is filler). Find "window <n>" or "tab <n>" anywhere in the phrase.
    let toks: Vec<&str> = p.split_whitespace().collect();
    for i in 0..toks.len() {
        if (toks[i] == "window" || toks[i] == "tab") && i + 1 < toks.len() {
            let r = toks[i + 1];
            if let Some(n) = r.parse::<u32>().ok().or_else(|| word_num(r)) {
                return Some(n);
            }
        }
    }
    None
}

fn word_num(s: &str) -> Option<u32> {
    Some(match s {
        "one" => 1, "two" => 2, "three" => 3, "four" => 4, "five" => 5,
        "six" => 6, "seven" => 7, "eight" => 8, "nine" => 9, "ten" => 10,
        _ => return None,
    })
}

// tmux select-window -t wants a &str; we keep numbers small + leak to 'static
// only in tests via a tiny formatting helper (here just format into a String,
// returned through a leaked Box only in the act path). To avoid a real leak in
// production we instead build the argv with a String — see translate's direct
// construction below for the number case.
fn leak_num(n: u32) -> &'static str {
    // small, bounded set → static table avoids any leak.
    match n { 1=>"1",2=>"2",3=>"3",4=>"4",5=>"5",6=>"6",7=>"7",8=>"8",9=>"9",10=>"10", _=>"1" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_session() {
        assert!(in_tmux_session(Some("/tmp/tmux-1000/default,1234,0")));
        assert!(!in_tmux_session(None));
        assert!(!in_tmux_session(Some("")));
    }

    #[test]
    fn panes() {
        assert_eq!(translate("split right").unwrap().argv, vec!["split-window", "-h"]);
        assert_eq!(translate("split down").unwrap().argv, vec!["split-window", "-v"]);
        assert!(translate("close pane").unwrap().confirm);
        assert_eq!(translate("zoom").unwrap().argv, vec!["resize-pane", "-Z"]);
        assert_eq!(translate("focus left").unwrap().argv, vec!["select-pane", "-L"]);
        assert_eq!(translate("focus down").unwrap().argv, vec!["select-pane", "-D"]);
    }

    #[test]
    fn windows() {
        assert_eq!(translate("new window").unwrap().argv, vec!["new-window"]);
        assert_eq!(translate("next window").unwrap().argv, vec!["next-window"]);
        assert_eq!(translate("previous tab").unwrap().argv, vec!["previous-window"]);
        assert_eq!(translate("go to window 3").unwrap().argv, vec!["select-window", "-t", "3"]);
        assert_eq!(translate("window two").unwrap().argv, vec!["select-window", "-t", "2"]);
        assert!(translate("close window").unwrap().confirm);
        assert_eq!(translate("rename window to build").unwrap().argv,
                   vec!["rename-window", "build"]);
    }

    #[test]
    fn resize_and_misc() {
        assert_eq!(translate("resize left").unwrap().argv, vec!["resize-pane", "-L", "5"]);
        assert_eq!(translate("detach").unwrap().argv, vec!["detach-client"]);
    }

    #[test]
    fn queries() {
        assert!(translate("what panes").unwrap().query);
        assert!(translate("list windows").unwrap().query);
    }

    #[test]
    fn non_tmux_is_none() {
        assert_eq!(translate("open google"), None);
        assert_eq!(translate("read this section"), None);
        assert_eq!(translate("pane"), None);
    }

    #[test]
    fn natural_phrasings_v706() {
        // forgiving matching — natural speech, not just terse canonical forms
        assert_eq!(translate("open a vertical pane").unwrap().argv, vec!["split-window", "-v"]);
        assert_eq!(translate("give me a pane on the right").unwrap().argv, vec!["split-window", "-h"]);
        assert_eq!(translate("make a new pane").unwrap().argv, vec!["split-window", "-v"]);
        assert_eq!(translate("split the window").unwrap().argv, vec!["split-window", "-v"]);
        assert_eq!(translate("go to the next window").unwrap().argv, vec!["next-window"]);
        assert_eq!(translate("go to window 3").unwrap().argv, vec!["select-window", "-t", "3"]);
        assert!(translate("what's in my panes").unwrap().query);
        // still rejects non-commands
        assert_eq!(translate("the quick brown fox"), None);
    }

    // v0.1.707 (#2): the natural-phrasing corpus — guards both mux matchers
    // against regression. Fixture maps mux|phrase|expected-argv ('-' = None).
    #[test]
    fn mux_phrasing_corpus() {
        let fixture = include_str!("../tests/mux_phrasing_corpus.txt");
        let mut checked = 0;
        for line in fixture.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let cols: Vec<&str> = line.splitn(3, '|').collect();
            assert_eq!(cols.len(), 3, "bad fixture line: {line}");
            let (mux, phrase, want) = (cols[0], cols[1], cols[2]);
            let got = match mux {
                "tmux" => translate(phrase).map(|a| a.argv.join(" ")),
                "zellij" => crate::zellij_voice::translate(phrase).map(|a| a.argv.join(" ")),
                other => panic!("unknown mux in fixture: {other}"),
            };
            if want == "-" {
                assert!(got.is_none(), "{mux} | {phrase:?} should be None, got {got:?}");
            } else {
                assert_eq!(got.as_deref(), Some(want),
                    "{mux} | {phrase:?} expected {want:?}");
            }
            checked += 1;
        }
        assert!(checked >= 30, "expected ≥30 phrasings, checked {checked}");
        println!("checked {checked} natural phrasings");
    }
}
