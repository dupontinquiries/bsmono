//! v0.1.636: shell DIALECT detection — which interpreter is the user at,
//! so the NLP layer can route to the right idiom set. This is the pure
//! foundation the docs/todo.md "per-shell NLP dialect registry" needs:
//! today table_nlp is bash-shaped; nushell, fish, and python REPL each
//! want different natural-language → command templates (nu uses
//! structured pipelines + `where $it =~ ...`, not bash pipe-to-grep).
//!
//! This module ONLY classifies; it has no templates. The lexicon modules
//! (nu idioms etc.) and the table_nlp routing are layered on top. Pure +
//! testable: takes the foreground command name and the current prompt
//! line as &str, returns a Dialect.
//!
//! Detection priority: the foreground command is the strongest signal
//! (you ARE running `nu`); the prompt line is a fallback for when the
//! foreground is unknown (e.g. login shell where comm is just "-bash" or
//! the PTY can't see it). We never GUESS bash — an unrecognized context
//! is Unknown, so the caller falls back to its current generic path
//! rather than misrouting to bash idioms.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// bash / zsh / sh — the POSIX-ish family that shares pipe-to-grep,
    /// `$VAR`, `cmd | grep`, etc. table_nlp's current target.
    BashFamily,
    /// nushell — structured pipelines, `where`, `$it`, columns, no
    /// pipe-to-grep. Needs its own idiom set.
    Nushell,
    /// fish — bash-ish surface but different `set` / `string` semantics.
    Fish,
    /// python interactive (>>> ) — expressions, not shell commands.
    Python,
    /// unrecognized — caller uses its generic/current path, does NOT
    /// assume bash.
    Unknown,
}

impl Dialect {
    pub fn label(self) -> &'static str {
        match self {
            Dialect::BashFamily => "bash",
            Dialect::Nushell => "nushell",
            Dialect::Fish => "fish",
            Dialect::Python => "python",
            Dialect::Unknown => "unknown",
        }
    }
    /// Does this dialect use structured pipelines (vs text/byte streams)?
    /// The one fact the NLP layer most needs: nu is structured, the rest
    /// are text. Drives "filter rows" → `where` vs `grep`.
    pub fn is_structured(self) -> bool {
        matches!(self, Dialect::Nushell)
    }
}

/// Classify from the foreground command name (strongest signal). The name
/// may be login-shell-decorated ("-bash") or path-qualified
/// ("/usr/bin/nu"); we normalize. Returns None if the command isn't a
/// recognized interpreter (caller then tries the prompt).
fn from_command(fg: &str) -> Option<Dialect> {
    let name = fg.trim().trim_start_matches('-'); // login shells: "-bash"
    let name = name.rsplit('/').next().unwrap_or(name); // strip path
    // strip a trailing version-ish suffix only for python ("python3.11")
    match name {
        "bash" | "zsh" | "sh" | "dash" | "ksh" => Some(Dialect::BashFamily),
        "nu" | "nushell" => Some(Dialect::Nushell),
        "fish" => Some(Dialect::Fish),
        _ => {
            if name == "python" || name == "python3" || name.starts_with("python3.")
                || name == "ipython" || name == "bpython" {
                Some(Dialect::Python)
            } else {
                None
            }
        }
    }
}

/// Classify from the prompt line (fallback when the command is unknown).
/// Conservative: only fire on prompts that are unambiguous for a dialect.
fn from_prompt(prompt: &str) -> Option<Dialect> {
    let p = prompt.trim();
    if p.is_empty() {
        return None;
    }
    // python REPL: the bare ">>>" continuation prompt.
    if p == ">>>" || p.starts_with(">>> ") || p == "..." {
        return Some(Dialect::Python);
    }
    // (We intentionally do NOT infer bash vs nu vs fish from a generic
    // "$"/">"/"%" prompt — those overlap across shells and themed prompts
    // make it unreliable. Command name is the only trustworthy shell-vs-
    // shell signal; prompt only rescues the python REPL case here.)
    None
}

/// Detect the shell dialect from the foreground command and the current
/// prompt line. `foreground_cmd` is the PTY foreground process name (may
/// be ""), `prompt_line` is the last non-empty line (may be "").
pub fn detect(foreground_cmd: &str, prompt_line: &str) -> Dialect {
    if let Some(d) = from_command(foreground_cmd) {
        return d;
    }
    if let Some(d) = from_prompt(prompt_line) {
        return d;
    }
    Dialect::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_family_from_command() {
        for c in ["bash", "zsh", "sh", "dash", "ksh", "-bash", "/bin/zsh"] {
            assert_eq!(detect(c, ""), Dialect::BashFamily, "{:?}", c);
        }
    }

    #[test]
    fn nushell_from_command() {
        assert_eq!(detect("nu", ""), Dialect::Nushell);
        assert_eq!(detect("nushell", ""), Dialect::Nushell);
        assert_eq!(detect("/usr/bin/nu", ""), Dialect::Nushell);
        assert_eq!(detect("-nu", ""), Dialect::Nushell); // login nu
    }

    #[test]
    fn fish_from_command() {
        assert_eq!(detect("fish", ""), Dialect::Fish);
        assert_eq!(detect("/usr/local/bin/fish", ""), Dialect::Fish);
    }

    #[test]
    fn python_from_command_and_prompt() {
        assert_eq!(detect("python3", ""), Dialect::Python);
        assert_eq!(detect("python3.11", ""), Dialect::Python);
        assert_eq!(detect("ipython", ""), Dialect::Python);
        // prompt fallback when the foreground is unknown/empty
        assert_eq!(detect("", ">>>"), Dialect::Python);
        assert_eq!(detect("", ">>> import os"), Dialect::Python);
    }

    #[test]
    fn command_beats_prompt() {
        // running nu, but the prompt happens to look pythonic → command wins
        assert_eq!(detect("nu", ">>>"), Dialect::Nushell);
    }

    #[test]
    fn unknown_does_not_assume_bash() {
        // a random foreground program + an ambiguous prompt → Unknown, so
        // the caller falls back to its generic path rather than misrouting.
        assert_eq!(detect("htop", ""), Dialect::Unknown);
        assert_eq!(detect("", "user@host:~$"), Dialect::Unknown);
        assert_eq!(detect("", "❯"), Dialect::Unknown);
        assert_eq!(detect("", ""), Dialect::Unknown);
    }

    #[test]
    fn structured_only_for_nu() {
        assert!(Dialect::Nushell.is_structured());
        assert!(!Dialect::BashFamily.is_structured());
        assert!(!Dialect::Fish.is_structured());
        assert!(!Dialect::Python.is_structured());
    }

    #[test]
    fn nu_does_not_false_match_substrings() {
        // "numfmt", "number-cruncher" must NOT be detected as nushell —
        // we match the whole (normalized) program name, not a prefix.
        assert_eq!(detect("numfmt", ""), Dialect::Unknown);
        assert_eq!(detect("nuget", ""), Dialect::Unknown);
    }
}
