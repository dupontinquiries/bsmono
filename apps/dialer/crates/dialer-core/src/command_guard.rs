//! The guard — the one place that answers "may this received payload *do*
//! anything, and under what friction?". Kept out of `message` (parsing) and
//! out of the UI (rendering) so the policy is testable in isolation and there
//! is exactly one gate to audit.
//!
//! ## Threat model
//!
//! A [`crate::message::Payload::CommandSuggestion`] is attacker-controlled
//! text from whoever is on the other end of an SMS or Matrix room. Treating
//! it as something to execute is a remote-code-execution channel. So the
//! defaults are hostile-by-assumption:
//!
//! * **Running is off unless the user turns it on.** `allow_run` defaults to
//!   `false`; while it is false, [`Decision::for_command`] never returns
//!   `Run*` — the UI offers copy-to-clipboard and nothing else.
//! * **Even when on, every run is confirmed** showing the full command and
//!   the sender, unless the sender is on the explicit `trusted_senders`
//!   allowlist *and* the command matches an allowed prefix.
//! * **The app never spawns a shell for the command string.** Execution (in
//!   the shell) is `Command::new(program).args(...)` over a *tokenised*
//!   command with no `sh -c`, so shell metacharacters in the payload are
//!   inert. [`tokenize`] is deliberately dumb (whitespace split, honouring
//!   single/double quotes) and rejects anything with control characters.
//!
//! * **File-view** is path-confined: a received `view:` path must resolve
//!   under one of the user's configured roots, canonicalised, or it is
//!   denied — so `view:/etc/shadow` and `view:../../secrets` go nowhere.
//!
//! None of this trusts the transport. Matrix E2EE tells you *who* sent a
//! thing, not that the thing is safe.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// User policy. Serialised next to the other prefs; conservative defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardPolicy {
    /// Master switch. False → received commands are display/copy only.
    pub allow_run: bool,
    /// Senders whose commands may skip the confirmation, IF the command also
    /// matches `allowed_prefixes`. Ids are transport-qualified
    /// ("matrix:@a:x.org", "tel:+15551234").
    pub trusted_senders: Vec<String>,
    /// Command prefixes a trusted sender may run without confirmation
    /// (e.g. "git ", "ls", "cargo test"). Empty → always confirm.
    pub allowed_prefixes: Vec<String>,
    /// Roots a received `view:` path must sit under. Empty → deny all views.
    pub view_roots: Vec<PathBuf>,
}

impl Default for GuardPolicy {
    fn default() -> Self {
        GuardPolicy {
            allow_run: false,
            trusted_senders: Vec::new(),
            allowed_prefixes: Vec::new(),
            view_roots: Vec::new(),
        }
    }
}

/// What the UI may offer for a received command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Running is disabled; only copy/display.
    CopyOnly,
    /// May run, but confirm first (show `command` + sender).
    RunWithConfirm,
    /// Trusted sender + allowed prefix: may run without a modal (still a tap).
    RunAllowed,
    /// The command text itself is unsafe to execute at all (untokenisable);
    /// display only, no run offered even if the user asked.
    Reject(String),
}

impl GuardPolicy {
    pub fn for_command(&self, sender: &str, command: &str) -> Decision {
        if tokenize(command).is_err() {
            return Decision::Reject("command contains unsupported characters".into());
        }
        if !self.allow_run {
            return Decision::CopyOnly;
        }
        let trusted = self.trusted_senders.iter().any(|s| s == sender);
        let prefixed = self
            .allowed_prefixes
            .iter()
            .any(|p| command.trim_start().starts_with(p.trim_start()));
        if trusted && prefixed {
            Decision::RunAllowed
        } else {
            Decision::RunWithConfirm
        }
    }

    /// Resolve a received view path against the roots. `Some(canonical)` only
    /// if it exists and sits under a root; `None` = denied.
    pub fn resolve_view(&self, path: &str) -> Option<PathBuf> {
        let candidate = Path::new(path);
        let canon = candidate.canonicalize().ok()?; // must exist; resolves ..
        for root in &self.view_roots {
            if let Ok(root_canon) = root.canonicalize() {
                if canon.starts_with(&root_canon) {
                    return Some(canon);
                }
            }
        }
        None
    }
}

/// Split a command line into program + args, honouring single and double
/// quotes, with **no** shell semantics: no globbing, no `$VAR`, no `;`/`|`/
/// `&&` handling — those characters are literal argument bytes. Control
/// characters (including newlines and NUL) are rejected outright, so a
/// payload cannot smuggle a second line past a log or a confirmation label.
pub fn tokenize(input: &str) -> Result<Vec<String>, String> {
    if input.chars().any(|c| c.is_control()) {
        return Err("control character in command".into());
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    let mut have_token = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                have_token = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                have_token = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if have_token {
                    out.push(std::mem::take(&mut cur));
                    have_token = false;
                }
            }
            c => {
                cur.push(c);
                have_token = true;
            }
        }
    }
    if in_single || in_double {
        return Err("unbalanced quote".into());
    }
    if have_token {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("empty command".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_is_off_by_default() {
        let g = GuardPolicy::default();
        assert_eq!(g.for_command("matrix:@a:x", "ls"), Decision::CopyOnly);
    }

    #[test]
    fn enabled_still_confirms_unless_trusted_and_prefixed() {
        let g = GuardPolicy {
            allow_run: true,
            trusted_senders: vec!["matrix:@me:x".into()],
            allowed_prefixes: vec!["git ".into()],
            ..Default::default()
        };
        assert_eq!(g.for_command("matrix:@me:x", "git pull"), Decision::RunAllowed);
        // trusted but not an allowed prefix → confirm
        assert_eq!(g.for_command("matrix:@me:x", "rm -rf /"), Decision::RunWithConfirm);
        // allowed prefix but untrusted sender → confirm
        assert_eq!(g.for_command("matrix:@stranger:x", "git pull"), Decision::RunWithConfirm);
    }

    #[test]
    fn dangerous_shell_text_is_rejected_or_inert() {
        let g = GuardPolicy { allow_run: true, ..Default::default() };
        // A newline injection is rejected outright (can't sneak a 2nd command).
        assert!(matches!(g.for_command("s", "ls\nrm -rf ~"), Decision::Reject(_)));
        // Metacharacters tokenize as literal args — no shell, so they're inert,
        // and still gated behind confirm.
        assert_eq!(
            tokenize("echo hi; rm -rf ~").unwrap(),
            vec!["echo", "hi;", "rm", "-rf", "~"]
        );
    }

    #[test]
    fn tokenize_honours_quotes_and_rejects_imbalance() {
        assert_eq!(
            tokenize("git commit -m 'a b c'").unwrap(),
            vec!["git", "commit", "-m", "a b c"]
        );
        assert_eq!(tokenize("say \"hello world\"").unwrap(), vec!["say", "hello world"]);
        assert!(tokenize("oops 'unterminated").is_err());
        assert!(tokenize("   ").is_err());
    }

    #[test]
    fn view_paths_are_confined_to_roots() {
        let dir = std::env::temp_dir().join(format!("dialer-guard-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let inside = dir.join("ok.txt");
        std::fs::write(&inside, b"x").unwrap();

        let g = GuardPolicy { view_roots: vec![dir.clone()], ..Default::default() };
        assert_eq!(g.resolve_view(inside.to_str().unwrap()), Some(inside.canonicalize().unwrap()));
        // Escaping the root via .. is denied (canonicalize resolves it).
        let escape = dir.join("../etc-ish");
        assert!(g.resolve_view(escape.to_str().unwrap()).is_none());
        // A nonexistent path is denied (nothing to view).
        assert!(g.resolve_view("/nope/nope/nope").is_none());
        // No roots configured → everything denied.
        let empty = GuardPolicy::default();
        assert!(empty.resolve_view(inside.to_str().unwrap()).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
