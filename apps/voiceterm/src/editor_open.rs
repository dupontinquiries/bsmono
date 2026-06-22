//! v0.1.653: editor-open with advanced context (R1). The pure core for
//! "open <thing> [on <host>] [at line N] [in vim|helix]" → a concrete
//! command. Resolves an editor, an optional SSH host (fuzzy-matched
//! against ~/.ssh/config + recent hosts), a path, and a line into the argv
//! to spawn — local `$EDITOR +LINE path` or `ssh host -t '$EDITOR +LINE
//! path'`. The kernel-dev user bounces between machines and deep paths
//! constantly; this turns a spoken intent into the exact open.
//!
//! PURE + testable: host-config PARSING, fuzzy host RESOLUTION, and argv
//! BUILDING are all pure here. The actual ~/.ssh/config read, path_fuzzy
//! resolution, and process spawn are the wiring layer (caller supplies the
//! config text + resolved path).

/// Which editor to open in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor { Vim, Helix }

impl Editor {
    /// The binary name. (vim AND helix: he uses vim; helix matters for the
    /// next wave of devs.)
    pub fn bin(self) -> &'static str {
        match self { Editor::Vim => "vim", Editor::Helix => "hx" }
    }
    /// Parse a voiced editor name; None if unspecified.
    pub fn from_voiced(s: &str) -> Option<Editor> {
        match s.trim().to_lowercase().as_str() {
            "vim" | "nvim" | "neovim" | "vi" => Some(Editor::Vim),
            "helix" | "hx" => Some(Editor::Helix),
            _ => None,
        }
    }
}

/// v0.1.846: pick the default editor when the user didn't name one, honoring
/// `$VISUAL` then `$EDITOR` (the conventional precedence for interactive,
/// full-screen editors). The basename of the first token is mapped via
/// `from_voiced`, so `EDITOR="/usr/bin/nvim -u NONE"` resolves to Vim and a
/// Helix user (`EDITOR=hx`) finally gets Helix instead of a hard-coded Vim.
/// Unknown or unset → Vim (this module's vim+helix scope). Pure: env values are
/// passed in, so it's testable without mutating the process environment.
pub fn default_editor_from(visual: Option<&str>, editor: Option<&str>) -> Editor {
    for val in [visual, editor].into_iter().flatten() {
        if let Some(tok) = val.split_whitespace().next() {
            let base = tok.rsplit(['/', '\\']).next().unwrap_or(tok);
            if let Some(ed) = Editor::from_voiced(base) {
                return ed;
            }
        }
    }
    Editor::Vim
}

/// Convenience wrapper that reads the real `$VISUAL`/`$EDITOR` environment.
pub fn default_editor() -> Editor {
    let v = std::env::var("VISUAL").ok();
    let e = std::env::var("EDITOR").ok();
    default_editor_from(v.as_deref(), e.as_deref())
}

/// A resolved open request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpec {
    pub editor: Editor,
    pub path: String,
    pub line: Option<u32>,
    /// SSH host, or None for local.
    pub host: Option<String>,
}

/// Parse host aliases out of an ssh config file's text. Returns each
/// `Host` alias (skipping wildcard patterns like `*` and negations).
/// Pure — caller reads ~/.ssh/config and passes the text.
pub fn parse_ssh_hosts(config_text: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    for line in config_text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') { continue; }
        // "Host" is case-insensitive in ssh_config.
        let lower = l.to_lowercase();
        if let Some(rest) = lower.strip_prefix("host ") {
            // a Host line can list several aliases: "Host a b c"
            // map back to the ORIGINAL-case tokens (ssh aliases are
            // case-sensitive in use, even though the keyword isn't).
            let orig_rest = l[4..].trim();
            for tok in orig_rest.split_whitespace() {
                if tok.contains('*') || tok.contains('?') || tok.starts_with('!') {
                    continue; // skip patterns/negations
                }
                if !hosts.iter().any(|h: &String| h == tok) {
                    hosts.push(tok.to_string());
                }
            }
            let _ = rest;
        }
    }
    hosts
}

/// Fuzzy-resolve a voiced host phrase against known hosts. Handles fillers
/// ("the m1n1 box" → "m1n1"). Returns the best host + score (0..1), or None
/// if nothing clears the bar.
pub fn resolve_host(voiced: &str, known: &[String]) -> Option<(String, f32)> {
    let v = normalize_host_phrase(voiced);
    if v.is_empty() { return None; }
    let mut best: Option<(String, f32)> = None;
    for h in known {
        let score = host_score(&v, &h.to_lowercase());
        if score > 0.0 && best.as_ref().map(|(_, s)| score > *s).unwrap_or(true) {
            best = Some((h.clone(), score));
        }
    }
    best.filter(|(_, s)| *s >= 0.5)
}

/// Strip common filler words around a spoken host name.
fn normalize_host_phrase(s: &str) -> String {
    let fillers = ["the", "my", "box", "machine", "host", "server", "on", "remote"];
    s.to_lowercase()
        .split_whitespace()
        .filter(|w| !fillers.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Score a normalized voiced phrase against a host alias.
fn host_score(voiced: &str, host: &str) -> f32 {
    if voiced == host { return 1.0; }
    // exact token match (e.g. voiced "m1n1" vs host "m1n1")
    if voiced.split_whitespace().any(|w| w == host) { return 0.95; }
    // host contained as a substring of the voiced phrase (or vice versa)
    let v_nospace = voiced.replace(' ', "");
    if v_nospace == host { return 0.9; }
    if host.contains(&v_nospace) || v_nospace.contains(host) {
        // proportional to overlap length
        let shorter = host.len().min(v_nospace.len()) as f32;
        let longer = host.len().max(v_nospace.len()) as f32;
        return 0.5 + 0.4 * (shorter / longer);
    }
    0.0
}

/// A parsed open request straight from the spoken phrase — slots not yet
/// resolved (thing is still a fuzzy query; host is the voiced phrase). The
/// wiring layer resolves `thing` via path_fuzzy and `host` via
/// resolve_host into a concrete OpenSpec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    /// what to open — a fuzzy path query (e.g. "dcp tracer").
    pub thing: String,
    /// voiced host phrase (e.g. "the m1n1 box"), or None for local.
    pub host: Option<String>,
    pub line: Option<u32>,
    /// editor if explicitly named, else None (caller picks a default).
    pub editor: Option<Editor>,
}

/// Slot-parse a spoken open phrase into an OpenRequest. Handles, in any
/// order: `in vim|helix|nvim` (editor), `at line N` / `line N` (line),
/// `on <host>` (host). Strips a leading `open`/`open up`/`edit` verb. The
/// remainder is the `thing`. Returns None if nothing is left to open.
///
/// Slots are removed before host extraction so "on line 5" isn't mistaken
/// for a host. The host span runs from the first ` on ` (after slot
/// removal) to the end — natural phrasing puts the host after the thing.
pub fn parse_open_request(phrase: &str) -> Option<OpenRequest> {
    let mut t = phrase.trim().to_lowercase();

    // strip a leading verb (or a bare verb with no argument → empty)
    for v in ["open up", "open", "edit", "go edit", "pull up"] {
        if t == v { return None; } // bare verb, nothing to open
        let prefix = format!("{} ", v);
        if let Some(rest) = t.strip_prefix(&prefix) { t = rest.trim_start().to_string(); break; }
    }

    // 1) editor: " in <ed>" / " with <ed>" — only when <ed> is a known
    // editor. Also handle a leading "in <ed>" (phrase began with it).
    let mut editor = None;
    for kw in [" in ", " with ", " using "] {
        if let Some(pos) = t.rfind(kw) {
            let after = t[pos + kw.len()..].trim();
            if let Some(ed) = Editor::from_voiced(after) {
                editor = Some(ed);
                t = t[..pos].trim().to_string();
                break;
            }
        }
    }
    if editor.is_none() {
        for kw in ["in ", "with ", "using "] {
            if let Some(rest) = t.strip_prefix(kw) {
                if let Some(ed) = Editor::from_voiced(rest.trim()) {
                    editor = Some(ed);
                    t = String::new();
                    break;
                }
            }
        }
    }

    // 2) line: " at line N" / " on line N" / " line N" (take the last)
    let mut line = None;
    for kw in [" at line ", " on line ", " line ", " colon " /* "file colon 12" */] {
        if let Some(pos) = t.rfind(kw) {
            let after = t[pos + kw.len()..].trim();
            let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num.parse::<u32>() {
                line = Some(n);
                // remove the "line N" span (keep anything after the number)
                let tail = after[num.len()..].trim().to_string();
                t = format!("{} {}", t[..pos].trim(), tail).trim().to_string();
                break;
            }
        }
    }

    // 3) host: first " on " in what's left (slots already removed)
    let mut host = None;
    if let Some(pos) = t.find(" on ") {
        let h = t[pos + 4..].trim();
        if !h.is_empty() {
            host = Some(h.to_string());
            t = t[..pos].trim().to_string();
        }
    }

    let thing = t.trim().to_string();
    if thing.is_empty() { return None; }
    Some(OpenRequest { thing, host, line, editor })
}

/// Build the argv to open the spec. Local: `<editor> +LINE -- path`.
/// Remote: `ssh -t host <editor> +LINE -- path`. The `--` guards against a
/// path that looks like a flag; `+LINE` is the vim/helix convention both
/// accept.
pub fn build_open_argv(spec: &OpenSpec) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    let editor = spec.editor.bin().to_string();
    let mut editor_cmd: Vec<String> = vec![editor];
    if let Some(l) = spec.line {
        editor_cmd.push(format!("+{}", l));
    }
    editor_cmd.push("--".to_string());
    editor_cmd.push(spec.path.clone());

    match &spec.host {
        None => argv.extend(editor_cmd),
        Some(h) => {
            // ssh -t <host> <editor> +LINE -- path  (-t for a real TTY so
            // the TUI editor works over the connection)
            argv.push("ssh".to_string());
            argv.push("-t".to_string());
            argv.push(h.clone());
            argv.extend(editor_cmd);
        }
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_from_voiced() {
        assert_eq!(Editor::from_voiced("helix"), Some(Editor::Helix));
        assert_eq!(Editor::from_voiced("hx"), Some(Editor::Helix));
        assert_eq!(Editor::from_voiced("neovim"), Some(Editor::Vim));
        assert_eq!(Editor::from_voiced("emacs"), None);
        assert_eq!(Editor::Helix.bin(), "hx");
        assert_eq!(Editor::Vim.bin(), "vim");
    }

    #[test]
    fn parse_hosts_skips_patterns_and_comments() {
        let cfg = "\
# my hosts
Host m1n1
    HostName 10.0.0.5
    User root
Host build-box dev
    HostName 10.0.0.6
Host *
    ForwardAgent yes
Host !secret prod
    HostName 10.0.0.7
";
        let hosts = parse_ssh_hosts(cfg);
        assert!(hosts.contains(&"m1n1".to_string()));
        assert!(hosts.contains(&"build-box".to_string()));
        assert!(hosts.contains(&"dev".to_string()));
        assert!(hosts.contains(&"prod".to_string()));
        assert!(!hosts.iter().any(|h| h.contains('*')), "no wildcard");
        assert!(!hosts.iter().any(|h| h.starts_with('!')), "no negation");
    }

    #[test]
    fn resolve_host_strips_fillers() {
        let known = vec!["m1n1".to_string(), "build-box".to_string()];
        let (h, _) = resolve_host("the m1n1 box", &known).unwrap();
        assert_eq!(h, "m1n1");
    }

    #[test]
    fn resolve_host_exact_and_partial() {
        let known = vec!["m1n1".to_string(), "buildbox".to_string()];
        assert_eq!(resolve_host("m1n1", &known).unwrap().0, "m1n1");
        // "build" is a substring of "buildbox"
        assert_eq!(resolve_host("build", &known).unwrap().0, "buildbox");
    }

    #[test]
    fn resolve_host_no_match() {
        let known = vec!["m1n1".to_string()];
        assert!(resolve_host("some unrelated thing", &known).is_none());
    }

    #[test]
    fn argv_local_with_line() {
        let spec = OpenSpec { editor: Editor::Helix, path: "src/dcp.rs".into(), line: Some(400), host: None };
        assert_eq!(build_open_argv(&spec), vec!["hx", "+400", "--", "src/dcp.rs"]);
    }

    #[test]
    fn argv_local_no_line() {
        let spec = OpenSpec { editor: Editor::Vim, path: "a.txt".into(), line: None, host: None };
        assert_eq!(build_open_argv(&spec), vec!["vim", "--", "a.txt"]);
    }

    #[test]
    fn argv_remote_ssh() {
        let spec = OpenSpec { editor: Editor::Helix, path: "/etc/x".into(), line: Some(12), host: Some("m1n1".into()) };
        assert_eq!(build_open_argv(&spec), vec!["ssh", "-t", "m1n1", "hx", "+12", "--", "/etc/x"]);
    }

    #[test]
    fn path_flag_guard() {
        // a path that looks like a flag is still after the `--`
        let spec = OpenSpec { editor: Editor::Vim, path: "-weird".into(), line: None, host: None };
        let argv = build_open_argv(&spec);
        let dd = argv.iter().position(|a| a == "--").unwrap();
        let p = argv.iter().position(|a| a == "-weird").unwrap();
        assert!(dd < p, "path must be guarded after --");
    }

    #[test]
    fn parse_full_request_all_slots() {
        let r = parse_open_request("open the dcp tracer on the m1n1 box at line 400 in helix").unwrap();
        assert_eq!(r.thing, "the dcp tracer");
        assert_eq!(r.host.as_deref(), Some("the m1n1 box"));
        assert_eq!(r.line, Some(400));
        assert_eq!(r.editor, Some(Editor::Helix));
    }

    #[test]
    fn parse_local_with_line_no_editor() {
        let r = parse_open_request("edit src/dcp.rs at line 42").unwrap();
        assert_eq!(r.thing, "src/dcp.rs");
        assert_eq!(r.host, None);
        assert_eq!(r.line, Some(42));
        assert_eq!(r.editor, None);
    }

    #[test]
    fn parse_just_thing() {
        let r = parse_open_request("open the mailbox tracer").unwrap();
        assert_eq!(r.thing, "the mailbox tracer");
        assert_eq!(r.host, None);
        assert_eq!(r.line, None);
        assert_eq!(r.editor, None);
    }

    #[test]
    fn parse_line_not_mistaken_for_host() {
        // "on line 5" must set line, not host
        let r = parse_open_request("open foo.rs on line 5").unwrap();
        assert_eq!(r.line, Some(5));
        assert_eq!(r.host, None);
        assert_eq!(r.thing, "foo.rs");
    }

    #[test]
    fn parse_host_without_line() {
        let r = parse_open_request("open config.txt on buildbox in vim").unwrap();
        assert_eq!(r.thing, "config.txt");
        assert_eq!(r.host.as_deref(), Some("buildbox"));
        assert_eq!(r.editor, Some(Editor::Vim));
        assert_eq!(r.line, None);
    }

    #[test]
    fn parse_empty_thing_is_none() {
        assert!(parse_open_request("open").is_none());
        assert!(parse_open_request("edit  in vim").is_none());
    }

    #[test]
    fn default_editor_honors_env() {
        use super::{default_editor_from, Editor};
        // VISUAL wins over EDITOR (interactive precedence)
        assert_eq!(default_editor_from(Some("vim"), Some("hx")), Editor::Vim);
        assert_eq!(default_editor_from(Some("hx"), Some("vim")), Editor::Helix);
        // Helix user finally gets Helix instead of the old hard-coded Vim
        assert_eq!(default_editor_from(None, Some("hx")), Editor::Helix);
        assert_eq!(default_editor_from(None, Some("helix")), Editor::Helix);
        // full path + args: basename of the first token is what matters
        assert_eq!(default_editor_from(None, Some("/usr/bin/nvim -u NONE")), Editor::Vim);
        assert_eq!(default_editor_from(Some("/opt/helix/hx"), None), Editor::Helix);
        // unknown editor or unset → Vim (this module's vim+helix scope)
        assert_eq!(default_editor_from(None, Some("nano")), Editor::Vim);
        assert_eq!(default_editor_from(None, Some("code -w")), Editor::Vim);
        assert_eq!(default_editor_from(None, None), Editor::Vim);
        // an empty/whitespace value falls through to the next source
        assert_eq!(default_editor_from(Some("   "), Some("hx")), Editor::Helix);
    }
}
