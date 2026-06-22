//! REPL voice mode (TODO #3): when the focused PTY is running an
//! interactive debugger/REPL, route voice through to that REPL's
//! command vocabulary instead of treating the words as a shell
//! command or web action.
//!
//! This module is the PURE core: it detects which REPL (if any) is
//! active from the last output line's prompt, and translates a
//! spoken phrase into the REPL's command string. It does no IO and
//! holds no state, so it's fully unit-testable. The caller (main.rs)
//! is responsible for: sampling the last PTY line, calling
//! detect_repl + translate, and writing "<cmd>\r" to the PTY.
//!
//! Generalizable per the TODO: each REPL is a (prompt-matcher,
//! verb-table) pair. gdb is the first; lldb / python / sqlite /
//! redis follow the same shape.

/// The interactive REPLs we recognize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repl {
    Gdb,
    Lldb,
    Python,
    Sqlite,
    Redis,
}

impl Repl {
    /// A short human label (for the "voice → gdb" UI hint).
    pub fn label(&self) -> &'static str {
        match self {
            Repl::Gdb => "gdb",
            Repl::Lldb => "lldb",
            Repl::Python => "python",
            Repl::Sqlite => "sqlite",
            Repl::Redis => "redis",
        }
    }
}

/// v0.1.484: self-surfacing (the man-pages principle). The speakable
/// phrases for this REPL, so the UI can answer "what can I say right
/// now?" the moment a REPL prompt is detected — making the
/// integration point announce itself instead of being a hidden
/// capability the user must already know.
pub fn commands(repl: Repl) -> Vec<(&'static str, &'static str)> {
    match repl {
        Repl::Gdb | Repl::Lldb => vec![
            ("step into", "step into the call"),
            ("step over", "next line, over calls"),
            ("step out", "finish the current function"),
            ("continue", "resume execution"),
            ("break at line 42", "set a breakpoint"),
            ("print x", "print a variable"),
            ("backtrace", "show the call stack"),
            ("run", "start/restart"),
            ("quit", "exit the debugger"),
        ],
        Repl::Python => vec![
            ("print x", "print a value"),
            ("quit", "exit the REPL"),
        ],
        Repl::Sqlite => vec![
            ("show tables", "list tables"),
            ("show schema", "show the schema"),
            ("quit", "exit"),
        ],
        Repl::Redis => vec![
            ("ping", "PING"),
            ("show keys", "KEYS *"),
            ("get session", "GET <key>"),
            ("info", "server INFO"),
            ("quit", "exit"),
        ],
    }
}

/// Detect which REPL is active from the LAST line of PTY output
/// (the current prompt). Returns None if it doesn't look like any
/// known REPL prompt — in which case the caller routes voice
/// normally (shell/web).
///
/// SAFETY-CRITICAL matching. A false positive routes voice into a
/// debugger/REPL that may execute commands — far worse than a false
/// negative (which just routes voice normally). So detection is
/// STRICT: the prompt must be effectively the ENTIRE trimmed last
/// line (a real REPL prompt line is just the prompt awaiting input),
/// not merely a suffix. This rejects output that happens to END with
/// "(gdb)" (e.g. a comment mentioning it) or a URL like
/// "host:8080>" that the loose suffix match would have mis-flagged.
pub fn detect_repl(last_line: &str) -> Option<Repl> {
    // A prompt line is the prompt and (at most) the text the user has
    // begun typing after it. We match on the line with leading
    // whitespace removed; the prompt must be at the START.
    let line = last_line.trim();
    if line.is_empty() { return None; }

    // gdb / lldb / sqlite: the prompt is a fixed token at line start,
    // optionally followed by a space (and possibly user-typed text,
    // but we only auto-route when the line IS just the prompt — see
    // prompt_is_bare). pwndbg/gef theme the prompt but it still
    // starts with the parenthesized token at the beginning of the
    // input region, so anchoring at start is correct.
    if prompt_is_bare(line, "(gdb)") { return Some(Repl::Gdb); }
    if prompt_is_bare(line, "(lldb)") { return Some(Repl::Lldb); }
    if prompt_is_bare(line, "sqlite>") { return Some(Repl::Sqlite); }
    // python: the prompt IS ">>>" (continuation is "..."). Must be the
    // whole bare line — rejects "print('>>>')" and "x >>> y".
    if prompt_is_bare(line, ">>>") { return Some(Repl::Python); }
    // redis: "host:port>" possibly with "[db]". Must be the whole
    // bare line — rejects a URL ending in ":8080>".
    if is_redis_prompt_bare(line) { return Some(Repl::Redis); }
    None
}

/// True iff the (already-trimmed) `line` is exactly the bare prompt
/// `prompt`. Since the caller trims the line, a real idle prompt
/// like "(gdb) " becomes "(gdb)" and matches exactly. We require an
/// EXACT match (not a prefix): if the user has typed text after the
/// prompt, the line is longer than the prompt and we route normally
/// rather than risk clobbering their partial input. This exactness
/// is what rejects "# comment about (gdb)" and "x >>> y".
fn prompt_is_bare(line: &str, prompt: &str) -> bool {
    line == prompt
}

/// redis variant of prompt_is_bare: the WHOLE bare line must be a
/// "host:port>" (optionally "host:port[db]>") prompt.
fn is_redis_prompt_bare(line: &str) -> bool {
    let l = line.trim_end();
    let core = match l.strip_suffix('>') {
        Some(c) => c,
        None => return false, // must end in '>'
    };
    // optional [N] db-index segment immediately before '>'
    let core = if core.ends_with(']') {
        match core.rfind('[') {
            Some(open) => &core[..open],
            None => return false,
        }
    } else { core };
    // The remainder must be exactly host:port with NO scheme/path —
    // i.e. it must not contain '/', and must split as <nonempty>:<digits>.
    if core.contains('/') { return false; } // rejects URLs
    match core.rfind(':') {
        Some(colon) if colon > 0 => {
            let host = &core[..colon];
            let port = &core[colon + 1..];
            // host must be plausible (no spaces — a real prompt host
            // has none), port all digits and non-empty.
            !host.is_empty()
                && !host.contains(' ')
                && !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Translate a spoken phrase into a REPL command string for the
/// given REPL. Returns None if the phrase doesn't map to a known
/// command (caller can then fall back to dictation or ignore).
///
/// The phrase is the transcribed utterance, already lowercased-ish;
/// we normalize internally. Numeric arguments (line numbers,
/// variable names) are extracted where the verb takes one.
pub fn translate(repl: Repl, phrase: &str) -> Option<String> {
    let p = phrase.trim().to_lowercase();
    if p.is_empty() { return None; }
    match repl {
        Repl::Gdb => translate_gdb(&p),
        Repl::Lldb => translate_lldb(&p),
        Repl::Python => translate_python(&p),
        Repl::Sqlite => translate_sqlite(&p),
        Repl::Redis => translate_redis(&p),
    }
}

/// Strip common polite/filler lead-ins so "please step into" and
/// "now continue" map the same as the bare verb.
fn strip_filler(p: &str) -> &str {
    let mut s = p;
    for lead in ["please ", "now ", "okay ", "ok ", "can you ", "let's ", "lets "] {
        if let Some(rest) = s.strip_prefix(lead) {
            s = rest;
        }
    }
    s.trim()
}

fn translate_gdb(p: &str) -> Option<String> {
    let p = strip_filler(p);
    // Exact / phrase verbs first.
    let cmd = match p {
        "step" | "step into" | "step in" => "step",
        "next" | "next line" | "step over" => "next",
        "continue" | "go" | "resume" => "continue",
        "finish" | "step out" | "return" => "finish",
        "list" | "show source" | "show code" => "list",
        "backtrace" | "back trace" | "stack" | "where" => "backtrace",
        "info locals" | "locals" | "local variables" => "info locals",
        "info args" | "arguments" | "args" => "info args",
        "run" | "start" => "run",
        "quit" | "quit gdb" | "exit" | "exit gdb" => "quit",
        _ => "",
    };
    if !cmd.is_empty() {
        return Some(cmd.to_string());
    }
    // Parametric verbs.
    // "break at line 42" / "breakpoint at 42" / "break 42"
    if let Some(n) = after_any(p, &["break at line ", "breakpoint at line ",
        "break at ", "breakpoint at ", "break ", "breakpoint "]) {
        let n = first_int(n)?;
        return Some(format!("break {}", n));
    }
    // "print X" / "print variable X" / "show me X"
    if let Some(rest) = after_any(p, &["print variable ", "print ", "show me ", "evaluate "]) {
        let var = sanitize_symbol(rest)?;
        return Some(format!("print {}", var));
    }
    None
}

fn translate_lldb(p: &str) -> Option<String> {
    let p = strip_filler(p);
    let cmd = match p {
        "step" | "step into" | "step in" => "thread step-in",
        "next" | "next line" | "step over" => "thread step-over",
        "continue" | "go" | "resume" => "thread continue",
        "finish" | "step out" => "thread step-out",
        "backtrace" | "stack" | "where" => "thread backtrace",
        "run" | "start" => "run",
        "quit" | "exit" | "quit lldb" => "quit",
        _ => "",
    };
    if !cmd.is_empty() { return Some(cmd.to_string()); }
    if let Some(n) = after_any(p, &["break at line ", "break at ", "breakpoint at ", "break "]) {
        let n = first_int(n)?;
        return Some(format!("breakpoint set --line {}", n));
    }
    if let Some(rest) = after_any(p, &["print ", "show me "]) {
        let var = sanitize_symbol(rest)?;
        return Some(format!("frame variable {}", var));
    }
    None
}

fn translate_python(p: &str) -> Option<String> {
    let p = strip_filler(p);
    // Python REPL voice is mostly meta; limited safe verbs.
    let cmd = match p {
        "quit" | "exit" | "quit python" => "exit()",
        "clear" => "",  // no portable clear; ignore
        _ => "",
    };
    if !cmd.is_empty() { return Some(cmd.to_string()); }
    // "print X"
    if let Some(rest) = after_any(p, &["print ", "show me "]) {
        let var = sanitize_symbol(rest)?;
        return Some(format!("print({})", var));
    }
    None
}

fn translate_sqlite(p: &str) -> Option<String> {
    let p = strip_filler(p);
    let cmd = match p {
        "show tables" | "list tables" | "tables" => ".tables",
        "show schema" | "schema" => ".schema",
        "quit" | "exit" | "quit sqlite" => ".quit",
        _ => "",
    };
    if !cmd.is_empty() { return Some(cmd.to_string()); }
    None
}

fn translate_redis(p: &str) -> Option<String> {
    let p = strip_filler(p);
    let cmd = match p {
        "ping" => "PING",
        "show keys" | "list keys" | "all keys" => "KEYS *",
        "info" => "INFO",
        "flush" | "flush all" => "",  // destructive — never auto-issue
        "quit" | "exit" => "QUIT",
        _ => "",
    };
    if !cmd.is_empty() { return Some(cmd.to_string()); }
    if let Some(rest) = after_any(p, &["get ", "show me "]) {
        let key = sanitize_symbol(rest)?;
        return Some(format!("GET {}", key));
    }
    None
}

/// If `p` starts with any of the given prefixes, return the
/// remainder after the first match. None otherwise.
fn after_any<'a>(p: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for pre in prefixes {
        if let Some(rest) = p.strip_prefix(pre) {
            return Some(rest.trim());
        }
    }
    None
}

/// Extract the first integer found in `s` (handles "line 42",
/// "42", "forty"... no — digits only; spoken-number words are out
/// of scope here). Returns None if no digits.
fn first_int(s: &str) -> Option<u32> {
    let mut digits = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else if !digits.is_empty() {
            break;
        }
    }
    digits.parse().ok()
}

/// Sanitize a spoken symbol/variable name into something safe to
/// inject into a REPL command: keep identifier chars, dots, and
/// brackets/indexing (so `node->next`, `arr[0]`, `my.field` survive);
/// reject if it's empty, contains shell/command metacharacters, is
/// pure punctuation, or looks like a flag. Defends against a
/// transcript like "print; rm -rf" or "print --rf" reaching the PTY.
fn sanitize_symbol(s: &str) -> Option<String> {
    // Take the first whitespace-delimited token — variable names
    // don't contain spaces, and this drops trailing dictation.
    let tok = s.split_whitespace().next()?;
    if tok.is_empty() { return None; }
    // v0.1.480: reject a leading '-' so a transcript can't produce a
    // flag-like token (e.g. "--rf", "-x") that a REPL might interpret
    // as an option rather than a symbol.
    if tok.starts_with('-') { return None; }
    let charset_ok = tok.chars().all(|c|
        c.is_ascii_alphanumeric() || c == '_' || c == '.'
        || c == '[' || c == ']' || c == '-' || c == '>');
    if !charset_ok { return None; }
    // v0.1.480: a real identifier has at least one alphanumeric char.
    // Pure punctuation ("-", ">", "->", "[]") is never a variable
    // name — reject rather than inject nonsense into the debugger.
    if !tok.chars().any(|c| c.is_ascii_alphanumeric()) { return None; }
    Some(tok.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_gdb_prompt() {
        assert_eq!(detect_repl("(gdb) "), Some(Repl::Gdb));
        assert_eq!(detect_repl("(gdb)"), Some(Repl::Gdb));
        // leading/trailing whitespace tolerated (caller passes the
        // last line; we trim it).
        assert_eq!(detect_repl("  (gdb)  "), Some(Repl::Gdb));
    }

    #[test]
    fn detect_other_repls() {
        assert_eq!(detect_repl("(lldb) "), Some(Repl::Lldb));
        assert_eq!(detect_repl(">>> "), Some(Repl::Python));
        assert_eq!(detect_repl("sqlite> "), Some(Repl::Sqlite));
        assert_eq!(detect_repl("127.0.0.1:6379> "), Some(Repl::Redis));
        assert_eq!(detect_repl("127.0.0.1:6379[2]> "), Some(Repl::Redis));
    }

    #[test]
    fn detect_none_for_shell() {
        // ordinary shell prompts must NOT match
        assert_eq!(detect_repl("$ "), None);
        assert_eq!(detect_repl("user@host:~$ "), None);
        assert_eq!(detect_repl("> "), None);          // bare > is not python
        assert_eq!(detect_repl("PS C:\\> "), None);
        assert_eq!(detect_repl(""), None);
    }

    #[test]
    fn detect_rejects_false_positives() {
        // v0.1.479: these all END with a prompt-like token but are
        // NOT bare prompts — the strict (whole-line) match must
        // reject them. A false positive here would route voice into
        // a debugger that isn't waiting, which is dangerous.
        // A comment / output line mentioning the prompt token:
        assert_eq!(detect_repl("# some comment about (gdb)"), None);
        assert_eq!(detect_repl("error near sqlite>"), None);
        // python chevrons inside printed text:
        assert_eq!(detect_repl("x >>> y"), None);
        assert_eq!(detect_repl("print('>>>')"), None);
        // a URL with a port that the old suffix match flagged as redis:
        assert_eq!(detect_repl("https://example.com:8080>"), None);
        assert_eq!(detect_repl("see http://host:443>"), None);
        // gdb prompt with text already typed after it → route
        // normally, don't clobber the user's partial input:
        assert_eq!(detect_repl("(gdb) print x"), None);
    }

    #[test]
    fn redis_prompt_discrimination() {
        // a real redis prompt
        assert!(is_redis_prompt_bare("127.0.0.1:6379>"));
        assert!(is_redis_prompt_bare("127.0.0.1:6379[2]>"));
        // not redis: no port digits
        assert!(!is_redis_prompt_bare("host:abc>"));
        // not redis: plain text
        assert!(!is_redis_prompt_bare("hello>"));
        // not redis: a URL (has a scheme/path slash) even with :port>
        assert!(!is_redis_prompt_bare("https://example.com:8080>"));
        assert!(!is_redis_prompt_bare("host:443/path>"));
    }

    #[test]
    fn gdb_simple_verbs() {
        assert_eq!(translate(Repl::Gdb, "step into"), Some("step".to_string()));
        assert_eq!(translate(Repl::Gdb, "step over"), Some("next".to_string()));
        assert_eq!(translate(Repl::Gdb, "continue"), Some("continue".to_string()));
        assert_eq!(translate(Repl::Gdb, "step out"), Some("finish".to_string()));
        assert_eq!(translate(Repl::Gdb, "backtrace"), Some("backtrace".to_string()));
        assert_eq!(translate(Repl::Gdb, "quit gdb"), Some("quit".to_string()));
    }

    #[test]
    fn gdb_filler_tolerance() {
        assert_eq!(translate(Repl::Gdb, "please step into"), Some("step".to_string()));
        assert_eq!(translate(Repl::Gdb, "now continue"), Some("continue".to_string()));
        assert_eq!(translate(Repl::Gdb, "okay next line"), Some("next".to_string()));
    }

    #[test]
    fn gdb_breakpoint_with_line() {
        assert_eq!(translate(Repl::Gdb, "break at line 42"), Some("break 42".to_string()));
        assert_eq!(translate(Repl::Gdb, "breakpoint at 100"), Some("break 100".to_string()));
        assert_eq!(translate(Repl::Gdb, "break 7"), Some("break 7".to_string()));
    }

    #[test]
    fn gdb_print_variable() {
        assert_eq!(translate(Repl::Gdb, "print x"), Some("print x".to_string()));
        assert_eq!(translate(Repl::Gdb, "print my_var"), Some("print my_var".to_string()));
        assert_eq!(translate(Repl::Gdb, "show me counter"), Some("print counter".to_string()));
        // struct field access survives sanitize
        assert_eq!(translate(Repl::Gdb, "print node.next"), Some("print node.next".to_string()));
    }

    #[test]
    fn print_rejects_injection() {
        // A metacharacter glued to the symbol → the whole token is
        // rejected (safer to drop than to guess). "x;" fails charset.
        assert_eq!(translate(Repl::Gdb, "print x; quit"), None);
        // Space-separated trailing dictation IS dropped, keeping the
        // clean first token — "print x and stuff" → "print x".
        assert_eq!(translate(Repl::Gdb, "print x and then quit"),
            Some("print x".to_string()));
        // leading metachar token → rejected
        assert_eq!(translate(Repl::Gdb, "print ;rm"), None);
    }

    #[test]
    fn lldb_maps_to_thread_commands() {
        assert_eq!(translate(Repl::Lldb, "step into"), Some("thread step-in".to_string()));
        assert_eq!(translate(Repl::Lldb, "step over"), Some("thread step-over".to_string()));
        assert_eq!(translate(Repl::Lldb, "break at line 12"),
            Some("breakpoint set --line 12".to_string()));
    }

    #[test]
    fn python_print() {
        assert_eq!(translate(Repl::Python, "print x"), Some("print(x)".to_string()));
        assert_eq!(translate(Repl::Python, "quit"), Some("exit()".to_string()));
    }

    #[test]
    fn sqlite_and_redis_verbs() {
        assert_eq!(translate(Repl::Sqlite, "show tables"), Some(".tables".to_string()));
        assert_eq!(translate(Repl::Redis, "ping"), Some("PING".to_string()));
        assert_eq!(translate(Repl::Redis, "get session"), Some("GET session".to_string()));
    }

    #[test]
    fn destructive_redis_not_auto_issued() {
        // "flush all" maps to nothing — we never auto-issue a flush
        assert_eq!(translate(Repl::Redis, "flush all"), None);
    }

    #[test]
    fn unknown_phrase_returns_none() {
        assert_eq!(translate(Repl::Gdb, "make me a sandwich"), None);
        assert_eq!(translate(Repl::Gdb, ""), None);
    }

    #[test]
    fn commands_lists_are_truthful() {
        // Every advertised command must actually translate, for each
        // REPL — the discoverable list can't lie about what works.
        for repl in [Repl::Gdb, Repl::Lldb, Repl::Python, Repl::Sqlite, Repl::Redis] {
            let c = commands(repl);
            assert!(!c.is_empty(), "{:?} should list commands", repl);
            for (say, _does) in &c {
                assert!(translate(repl, say).is_some(),
                    "{:?} lists {:?} but it doesn't translate", repl, say);
            }
        }
    }

    #[test]
    fn symbol_sanitization_hardening() {
        // v0.1.480: legitimate identifiers (incl. pointer deref and
        // indexing) still pass.
        assert_eq!(translate(Repl::Gdb, "print node->next"),
            Some("print node->next".to_string()));
        assert_eq!(translate(Repl::Gdb, "print arr[0]"),
            Some("print arr[0]".to_string()));
        assert_eq!(translate(Repl::Gdb, "print my.field"),
            Some("print my.field".to_string()));
        // Flag-like and pure-punctuation tokens are REJECTED so they
        // can't reach the debugger as options/garbage.
        assert_eq!(translate(Repl::Gdb, "print --rf"), None);
        assert_eq!(translate(Repl::Gdb, "print -x"), None);
        assert_eq!(translate(Repl::Gdb, "print -"), None);
        assert_eq!(translate(Repl::Gdb, "print >"), None);
        assert_eq!(translate(Repl::Gdb, "print ->"), None);
    }
}
