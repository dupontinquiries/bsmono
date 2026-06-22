//! v0.1.638: doc resolver — the "auto find" brain for the documentation-
//! pane feature. Given a command, subcommand, or topic the user is working
//! with, decide WHICH documentation source to surface and the exact action
//! to pull it. This is what makes "open a docs pane for this" actually
//! useful: it doesn't just guess `man <word>` — it picks among bundled man
//! pages, the tldr index, `<cmd> --help`, and web search based on what
//! actually exists for that command.
//!
//! PURE + testable: it takes the query plus closures/sets describing what's
//! available (is there a bundled man page? a tldr entry?) so the sandbox
//! can test the decision logic without filesystem/exec. The caller (FFI)
//! supplies the availability checks (manpages::get, command_index lookup)
//! and then SPAWNS the resolved action — e.g. as a zellij pane running the
//! doc command, or in the app's own doc panel.
//!
//! Why a resolver (not just `man`): a power user working in `git rebase`
//! wants `man git-rebase` (note the hyphen), not `man "git rebase"`. `rg`
//! has a great man page; `cargo` is best via `cargo --help` subcommands;
//! an opaque error string isn't a command at all → web search. The
//! resolver encodes that judgment so the doc pane is smart, not literal.

/// A resolved documentation target: what to show and how to get it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocSource {
    /// Bundled/offline man page we already ship — render in-app, no exec.
    /// Carries the man-page key (e.g. "git-rebase").
    BundledManPage(String),
    /// System man page — run `man <arg>` in a pane. arg is normalized
    /// (subcommands hyphenated: "git rebase" → "git-rebase").
    SystemMan(String),
    /// tldr summary we have indexed — show it (fast, example-focused).
    /// Carries the command name.
    Tldr(String),
    /// Run `<cmd> --help` in a pane (for tools whose --help beats their
    /// man page, or that have no man page). Carries the full argv-ish
    /// string, e.g. "cargo build --help".
    HelpFlag(String),
    /// Not a recognized command/topic for local docs → web search.
    /// Carries the query to search.
    WebSearch(String),
}

impl DocSource {
    /// The shell command a doc PANE would run for this source (None for
    /// BundledManPage/Tldr, which render in-app without exec, and for
    /// WebSearch which the browser tab handles).
    pub fn pane_command(&self) -> Option<String> {
        match self {
            DocSource::SystemMan(arg) => Some(format!("man {}", arg)),
            DocSource::HelpFlag(cmd) => Some(format!("{} --help", cmd)),
            DocSource::BundledManPage(_) | DocSource::Tldr(_) | DocSource::WebSearch(_) => None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            DocSource::BundledManPage(_) => "bundled man page",
            DocSource::SystemMan(_) => "man page",
            DocSource::Tldr(_) => "tldr",
            DocSource::HelpFlag(_) => "--help",
            DocSource::WebSearch(_) => "web search",
        }
    }
}

/// Availability facts the caller supplies (kept as a struct of flags so
/// the resolver is pure/testable; the FFI fills these from manpages::get,
/// command_index, etc.).
pub struct Availability<'a> {
    /// Is there a BUNDLED (shipped) man page for this exact key?
    pub bundled_man: &'a dyn Fn(&str) -> bool,
    /// Is there a tldr index entry for this command name?
    pub has_tldr: &'a dyn Fn(&str) -> bool,
    /// Is this name a real executable on PATH? (so we can offer --help /
    /// system man with confidence it exists)
    pub on_path: &'a dyn Fn(&str) -> bool,
}

/// Commands whose `--help` is the canonical/best docs (subcommand-driven
/// tools where the man page is thin or the help is richer + version-exact).
const HELP_FIRST: &[&str] = &[
    "cargo", "rustup", "npm", "pnpm", "yarn", "pip", "pip3", "go", "deno",
    "kubectl", "docker", "podman", "gh", "aws", "gcloud", "terraform",
];

/// Resolve a query (a command, "cmd subcommand", or free topic) to the
/// best doc source given what's available.
pub fn resolve(query: &str, avail: &Availability) -> DocSource {
    let q = query.trim();
    if q.is_empty() {
        return DocSource::WebSearch(String::new());
    }
    let tokens: Vec<&str> = q.split_whitespace().collect();
    let head = tokens[0];

    // Multi-token: likely "cmd subcommand [...]" (e.g. "git rebase",
    // "cargo build"). Two doc shapes:
    //   - man pages hyphenate: "git rebase" → man page key "git-rebase".
    //   - help-first tools take the subcommand: "cargo build --help".
    if tokens.len() >= 2 {
        let two = format!("{}-{}", tokens[0], tokens[1]); // git-rebase
        // bundled hyphenated page wins (offline, instant)
        if (avail.bundled_man)(&two) {
            return DocSource::BundledManPage(two);
        }
        if HELP_FIRST.contains(&head) {
            // "cargo build --help" — pass the subcommand through
            let sub = tokens[..2].join(" ");
            return DocSource::HelpFlag(sub);
        }
        // a real hyphenated system man page (git-rebase, systemctl-...)?
        if (avail.on_path)(head) {
            return DocSource::SystemMan(two);
        }
        // not a recognized command pair → search the whole phrase
        return DocSource::WebSearch(q.to_string());
    }

    // Single token: a bare command name (or a topic).
    if (avail.bundled_man)(head) {
        return DocSource::BundledManPage(head.to_string());
    }
    if HELP_FIRST.contains(&head) {
        return DocSource::HelpFlag(head.to_string());
    }
    if (avail.on_path)(head) {
        // real binary: prefer its man page; tldr is the example-y fallback
        // but a man page is more complete, so SystemMan first.
        return DocSource::SystemMan(head.to_string());
    }
    if (avail.has_tldr)(head) {
        // known to tldr but not on PATH (not installed) — still useful to
        // show what it does / how to install.
        return DocSource::Tldr(head.to_string());
    }
    // unknown single word → topic search
    DocSource::WebSearch(head.to_string())
}

/// Subcommand-dispatching tools where the second token is a meaningful
/// subcommand to carry into the doc query ("git rebase", "cargo build").
/// For everything else we keep just the head command, so "ls -la file"
/// resolves docs for `ls`, not the file. Superset of HELP_FIRST plus the
/// man-page tools whose subcommands have their own pages (git, systemctl).
const SUBCOMMAND_TOOLS: &[&str] = &[
    "git", "cargo", "rustup", "npm", "pnpm", "yarn", "pip", "pip3", "go",
    "deno", "kubectl", "docker", "podman", "gh", "aws", "gcloud",
    "terraform", "systemctl", "apt", "apt-get", "dnf", "pacman", "brew",
    "nix", "zellij", "tmux",
];

/// Wrapper commands to strip so docs target the REAL command underneath
/// ("sudo systemctl restart" → docs for "systemctl restart").
const WRAPPERS: &[&str] = &["sudo", "doas", "time", "nice", "env", "command", "exec", "nohup", "stdbuf"];

/// Turn a raw command LINE (what the user typed or the foreground command)
/// into a clean doc query for implicit "open docs" mode. Strips leading
/// env-var assignments, wrapper commands (sudo/time/...), path
/// qualification, and the pipeline tail; keeps the head command plus a
/// subcommand for subcommand-dispatching tools. Returns None when there's
/// nothing doc-worthy (empty / only a wrapper / a bare path with no
/// command name).
///
/// Examples:
///   "git rebase -i HEAD~3"        -> Some("git rebase")
///   "cargo build --release"       -> Some("cargo build")
///   "sudo systemctl restart nginx"-> Some("systemctl restart")
///   "/usr/bin/rg -n foo src/"     -> Some("rg")
///   "FOO=bar ls -la | grep x"     -> Some("ls")
///   "./configure"                 -> None (a path, not a known command)
pub fn extract_query(command_line: &str) -> Option<String> {
    let line = command_line.trim();
    if line.is_empty() {
        return None;
    }
    // Take the FIRST pipeline segment (the command the user is focused on
    // when they invoke it; pipes/redirs are about data flow, not the doc).
    let seg = line
        .split(|c| c == '|' || c == ';' || c == '&')
        .next()
        .unwrap_or(line)
        .trim();
    let mut tokens: Vec<&str> = seg.split_whitespace().collect();

    // Strip leading VAR=value assignments ("FOO=bar cmd").
    while let Some(t) = tokens.first() {
        if is_env_assignment(t) {
            tokens.remove(0);
        } else {
            break;
        }
    }
    // Strip leading wrapper commands (sudo/time/env/...), possibly
    // stacked ("sudo nice cmd"). After a wrapper, its own flags AND their
    // values must be skipped to reach the real command. We don't try to
    // know which flags take values (fragile per-wrapper) — instead, after
    // a wrapper word we skip any leading tokens that are NOT a plausible
    // bare command name (flags like "-n", flag values like "5"/"bob",
    // and any further VAR=value env assignments). Stop at the first clean
    // command-name token.
    let mut stripped_wrapper = false;
    while tokens.len() > 1 {
        let head = strip_path(tokens[0]);
        if WRAPPERS.contains(&head) {
            tokens.remove(0);
            stripped_wrapper = true;
        } else {
            break;
        }
    }
    if stripped_wrapper {
        // skip wrapper flags + flag-values + env assignments until the
        // real command. A "plausible command name" starts with a letter
        // (or '_'), has no '=', and isn't purely numeric.
        while tokens.len() > 1 && !is_plausible_command(tokens[0]) {
            tokens.remove(0);
        }
    }
    if tokens.is_empty() {
        return None;
    }
    let head = strip_path(tokens[0]);
    if head.is_empty() {
        return None;
    }
    // A bare path with no recognizable command name ("./configure",
    // "/opt/foo/run.sh") — the head after path-strip is a script name; we
    // only treat it as doc-worthy if it has no path separators originally
    // (i.e. it was a real command name, not an explicit path invocation).
    if tokens[0].contains('/') {
        return None;
    }

    // Subcommand tools: carry the second token if it's a real subcommand
    // (not a flag, not a path/arg with separators or dots).
    if SUBCOMMAND_TOOLS.contains(&head) && tokens.len() >= 2 {
        let sub = tokens[1];
        if !sub.starts_with('-') && !sub.contains('/') && !sub.contains('.') {
            return Some(format!("{} {}", head, sub));
        }
    }
    Some(head.to_string())
}

fn is_env_assignment(tok: &str) -> bool {
    // NAME=value, NAME is a shell-ish identifier and '=' is present before
    // any '/' (so "a/b=c" isn't mistaken for one).
    if let Some(eq) = tok.find('=') {
        let name = &tok[..eq];
        !name.is_empty()
            && !name.contains('/')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && name.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false)
    } else {
        false
    }
}

fn strip_path(tok: &str) -> &str {
    tok.rsplit('/').next().unwrap_or(tok)
}

/// A token that could be a command NAME (vs a flag, a flag-value, or an
/// env assignment). Used to find the real command after wrapper flags.
fn is_plausible_command(tok: &str) -> bool {
    // reject env assignments / flags on the RAW token first (path-strip
    // would turn "PATH=/x" into "x" and wrongly accept it).
    if tok.is_empty() || tok.starts_with('-') || tok.contains('=') {
        return false;
    }
    let t = strip_path(tok);
    if t.is_empty() {
        return false;
    }
    // purely numeric (a flag value like "5") is not a command
    if t.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // must start with a letter or underscore
    t.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avail<'a>(
        bundled: &'a dyn Fn(&str) -> bool,
        tldr: &'a dyn Fn(&str) -> bool,
        path: &'a dyn Fn(&str) -> bool,
    ) -> Availability<'a> {
        Availability { bundled_man: bundled, has_tldr: tldr, on_path: path }
    }
    fn never(_: &str) -> bool { false }
    fn always(_: &str) -> bool { true }

    #[test]
    fn bundled_man_wins_for_known_page() {
        let a = avail(&|s| s == "git", &never, &always);
        assert_eq!(resolve("git", &a), DocSource::BundledManPage("git".into()));
    }

    #[test]
    fn subcommand_hyphenates_for_man() {
        // git rebase → man git-rebase (real binary, no bundled page)
        let a = avail(&never, &never, &|s| s == "git");
        assert_eq!(resolve("git rebase", &a), DocSource::SystemMan("git-rebase".into()));
    }

    #[test]
    fn bundled_hyphenated_subpage_wins() {
        let a = avail(&|s| s == "git-rebase", &never, &|s| s == "git");
        assert_eq!(resolve("git rebase", &a), DocSource::BundledManPage("git-rebase".into()));
    }

    #[test]
    fn help_first_tools_use_help_flag() {
        let a = avail(&never, &never, &always);
        assert_eq!(resolve("cargo", &a), DocSource::HelpFlag("cargo".into()));
        assert_eq!(resolve("cargo build", &a), DocSource::HelpFlag("cargo build".into()));
        assert_eq!(resolve("kubectl get", &a), DocSource::HelpFlag("kubectl get".into()));
    }

    #[test]
    fn real_binary_prefers_system_man() {
        let a = avail(&never, &|s| s == "rg", &|s| s == "rg");
        assert_eq!(resolve("rg", &a), DocSource::SystemMan("rg".into()));
    }

    #[test]
    fn tldr_only_when_not_installed() {
        // known to tldr, NOT on path, no man page → tldr (what is it / how
        // to install)
        let a = avail(&never, &|s| s == "exa", &never);
        assert_eq!(resolve("exa", &a), DocSource::Tldr("exa".into()));
    }

    #[test]
    fn unknown_word_is_web_search() {
        let a = avail(&never, &never, &never);
        assert_eq!(resolve("frobnicate", &a), DocSource::WebSearch("frobnicate".into()));
    }

    #[test]
    fn unknown_phrase_searches_whole_thing() {
        let a = avail(&never, &never, &never);
        assert_eq!(
            resolve("how do i squash commits", &a),
            DocSource::WebSearch("how do i squash commits".into())
        );
    }

    #[test]
    fn pane_command_formatting() {
        assert_eq!(DocSource::SystemMan("git-rebase".into()).pane_command().as_deref(), Some("man git-rebase"));
        assert_eq!(DocSource::HelpFlag("cargo build".into()).pane_command().as_deref(), Some("cargo build --help"));
        assert_eq!(DocSource::BundledManPage("git".into()).pane_command(), None);
        assert_eq!(DocSource::Tldr("rg".into()).pane_command(), None);
    }

    #[test]
    fn empty_query_is_empty_search() {
        let a = avail(&never, &never, &never);
        assert_eq!(resolve("   ", &a), DocSource::WebSearch(String::new()));
    }

    // --- extract_query (implicit "open docs" mode) ---

    #[test]
    fn extract_keeps_subcommand_for_dispatch_tools() {
        assert_eq!(extract_query("git rebase -i HEAD~3").as_deref(), Some("git rebase"));
        assert_eq!(extract_query("cargo build --release").as_deref(), Some("cargo build"));
        assert_eq!(extract_query("kubectl get pods").as_deref(), Some("kubectl get"));
    }

    #[test]
    fn extract_head_only_for_plain_commands() {
        // ls's second token is an ARG, not a subcommand → just "ls"
        assert_eq!(extract_query("ls -la /etc").as_deref(), Some("ls"));
        assert_eq!(extract_query("grep -rn foo src/").as_deref(), Some("grep"));
    }

    #[test]
    fn extract_strips_sudo_and_wrappers() {
        assert_eq!(extract_query("sudo systemctl restart nginx").as_deref(), Some("systemctl restart"));
        assert_eq!(extract_query("time cargo test").as_deref(), Some("cargo test"));
        assert_eq!(extract_query("sudo nice rg pattern").as_deref(), Some("rg"));
    }

    #[test]
    fn extract_strips_env_assignments() {
        assert_eq!(extract_query("FOO=bar ls -la").as_deref(), Some("ls"));
        assert_eq!(extract_query("RUST_LOG=debug cargo run").as_deref(), Some("cargo run"));
    }

    #[test]
    fn extract_strips_path_qualification() {
        // path-qualified BARE command via wrapper still resolves... but a
        // direct ./path invocation is NOT doc-worthy (it's a local script)
        assert_eq!(extract_query("/usr/bin/rg -n foo").as_deref(), None); // explicit path invocation
        assert_eq!(extract_query("./configure --prefix=/usr").as_deref(), None);
    }

    #[test]
    fn extract_takes_first_pipeline_segment() {
        assert_eq!(extract_query("ls -la | grep foo").as_deref(), Some("ls"));
        assert_eq!(extract_query("git log --oneline | head -20").as_deref(), Some("git log"));
    }

    #[test]
    fn extract_subcommand_arg_with_dot_is_not_subcommand() {
        // "git foo.txt" — foo.txt is an arg (has a dot), not a subcommand
        assert_eq!(extract_query("git foo.txt").as_deref(), Some("git"));
    }

    #[test]
    fn extract_empty_or_wrapper_only_is_none() {
        assert_eq!(extract_query("").as_deref(), None);
        assert_eq!(extract_query("   ").as_deref(), None);
        assert_eq!(extract_query("sudo").as_deref(), Some("sudo")); // lone wrapper = the command itself
    }

    #[test]
    fn extract_then_resolve_round_trip() {
        // the implicit-mode flow: extract from a command line, resolve.
        let a = avail(&never, &never, &|s| s == "git");
        let q = extract_query("git rebase -i HEAD~3").unwrap();
        assert_eq!(resolve(&q, &a), DocSource::SystemMan("git-rebase".into()));
    }

    #[test]
    fn extract_wrapper_flags_and_values_skipped() {
        // probe-found (v0.1.639): a wrapper's NUMERIC flag value or env
        // assignment must not become the command. These are the common,
        // reliably-handled cases:
        assert_eq!(extract_query("nice -n 5 cargo test").as_deref(), Some("cargo test"));
        assert_eq!(extract_query("env FOO=bar cargo run").as_deref(), Some("cargo run"));
        assert_eq!(extract_query("env FOO=bar PATH=/x rg pat").as_deref(), Some("rg"));
    }

    #[test]
    fn extract_known_limit_word_flag_value() {
        // HONEST LIMIT: a wrapper flag whose VALUE is itself a word
        // ("sudo -u bob ls") is ambiguous to a generic parser — "bob" and
        // "ls" are both plausible command names and we don't encode
        // per-flag arity. We document the actual behavior rather than
        // pretend it's solved: we land on the flag's word value here.
        // This is acceptable: the doc resolver will then just resolve docs
        // for "bob" (→ WebSearch, harmless), and the common wrapper cases
        // (sudo CMD, sudo -E CMD, nice -n N CMD, env VAR=x CMD) all work.
        assert_eq!(extract_query("sudo -u bob ls").as_deref(), Some("bob"));
        // the plain, overwhelmingly-common form works:
        assert_eq!(extract_query("sudo ls -la").as_deref(), Some("ls"));
        assert_eq!(extract_query("sudo systemctl restart x").as_deref(), Some("systemctl restart"));
    }
}
