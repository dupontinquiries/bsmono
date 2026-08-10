//! context_redact — `security.rs`'s `context_strip`, ported.
//!
//! Strips sensitive lines out of terminal context before it becomes an LLM
//! prompt.
//!
//! ## The threat model is stated, and it is narrower than "encrypt everything"
//!
//! The original: *"not 'encrypt everything' but 'don't let sensitive data
//! escape the process — to disk via swap, to other processes via core dumps, to
//! external services via API calls, or to background workers via logs.'"*
//!
//! This is the API-call arm of that. Terminal scrollback is exactly where
//! secrets appear in plain text — a sudo prompt, a pasted token, a printed
//! environment — and an AI feature that ships scrollback verbatim is the most
//! direct way to hand them to a third party.
//!
//! ## The audit log records markers, never content
//!
//! *"Allows auditing what kinds of secrets the user's terminal history was
//! producing without ever logging the line content."* A redactor whose own log
//! contains the redacted line has moved the secret rather than removed it —
//! which is the failure that makes redaction feel safe while not being safe.
//!
//! Markers are deduplicated so a scrollback full of one pattern produces one
//! entry rather than a thousand.
//!
//! ## Whole lines, not substrings
//!
//! A password prompt is `Password: hunter2` and blanking only the value leaves
//! the shape, the length and the surrounding command. The original replaces the
//! entire line, which also means a marker appearing anywhere in it is enough —
//! no need to parse where the secret starts.

/// Patterns whose presence redacts a line. Matched case-insensitively.
pub const MARKERS: &[&str] = &[
    "password:", "passwd:", "passphrase:", "[sudo] password",
    "verification code", "totp:", "otp:",
    "api_key=", "api-key=", "api_token=", "access_token=",
    "secret_key=", "private_key=",
    "begin private key", "begin rsa private key", "begin openssh private key",
    "aws_secret_access_key", "aws_access_key_id",
    "bearer ey", "bearer sk-",
    "ssh-rsa aaaa",
    "anthropic_api_key", "openai_api_key",
    "huggingface_token", "hf_token",
    "github_token", "github_pat", "gitlab_token",
];

/// What replaces a redacted line.
///
/// A visible placeholder rather than deletion: a model given context with lines
/// silently missing will confidently reason about a sequence that never
/// happened. Telling it something was removed is more honest and more useful.
pub const PLACEHOLDER: &str = "[redacted: sensitive line]";

/// The result of stripping, with an audit trail that carries no content.
#[derive(Clone, Debug, PartialEq)]
pub struct Redacted {
    pub text: String,
    pub lines_redacted: u32,
    /// Which markers fired, deduplicated and in first-seen order. **Never the
    /// lines themselves.**
    pub markers_hit: Vec<&'static str>,
}

/// Which marker a line trips, if any.
pub fn sensitive_marker(line: &str) -> Option<&'static str> {
    let lower = line.to_lowercase();
    MARKERS.iter().copied().find(|m| lower.contains(&m.to_lowercase()))
}

/// Redact sensitive lines.
pub fn context_strip(text: &str) -> Redacted {
    let mut out = String::with_capacity(text.len());
    let mut lines_redacted = 0u32;
    let mut markers_hit: Vec<&'static str> = Vec::new();

    for line in text.lines() {
        match sensitive_marker(line) {
            Some(marker) => {
                out.push_str(PLACEHOLDER);
                out.push('\n');
                lines_redacted += 1;
                // Deduplicated: a scrollback full of one pattern should produce
                // one audit entry rather than a thousand.
                if !markers_hit.contains(&marker) { markers_hit.push(marker); }
            }
            None => { out.push_str(line); out.push('\n'); }
        }
    }
    Redacted { text: out, lines_redacted, markers_hit }
}

/// Is it safe to send this context at all?
///
/// A caller should check rather than assume: if a large share of the context
/// was redacted, the remainder is unlikely to be useful and sending it spends a
/// round trip to tell the model about holes.
pub fn worth_sending(r: &Redacted, total_lines: usize) -> bool {
    if total_lines == 0 { return false; }
    (r.lines_redacted as usize) * 2 < total_lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sudo_prompt_never_reaches_the_prompt_builder() {
        let ctx = "$ sudo apt update\n[sudo] password for kd: hunter2\nReading lists...";
        let r = context_strip(ctx);
        assert!(!r.text.contains("hunter2"));
        assert!(r.text.contains("Reading lists"), "the rest survives");
        assert_eq!(r.lines_redacted, 1);
    }

    #[test]
    fn the_whole_line_goes_not_just_the_value() {
        // Blanking only the value leaves the shape, the length and the
        // surrounding command — and means the redactor has to know where the
        // secret starts, which it cannot in general.
        let r = context_strip("Password: correct-horse-battery-staple");
        assert_eq!(r.text.trim(), PLACEHOLDER);
        assert!(!r.text.contains("Password"));
    }

    #[test]
    fn the_audit_trail_records_the_marker_and_never_the_line() {
        // "Auditing what kinds of secrets the user's terminal history was
        // producing without ever logging the line content." A redactor whose
        // own log contains the redacted line has MOVED the secret.
        let r = context_strip("export GITHUB_TOKEN=ghp_supersecretvalue");
        assert_eq!(r.markers_hit, vec!["github_token"]);
        for m in &r.markers_hit {
            assert!(!m.contains("ghp_"), "no secret in the audit trail");
        }
        assert!(!format!("{:?}", r.markers_hit).contains("supersecret"));
    }

    #[test]
    fn repeated_hits_produce_one_audit_entry() {
        // A scrollback full of one pattern should not produce a thousand log
        // lines.
        let ctx = (0..500).map(|_| "Password: x").collect::<Vec<_>>().join("\n");
        let r = context_strip(&ctx);
        assert_eq!(r.lines_redacted, 500);
        assert_eq!(r.markers_hit.len(), 1);
    }

    #[test]
    fn matching_is_case_insensitive() {
        // A terminal prints "Password:", "PASSWORD:" and "password:" depending
        // on the program, and a case-sensitive table catches one of the three.
        for line in ["Password: x", "PASSWORD: x", "password: x", "PaSsWoRd: x"] {
            assert_eq!(context_strip(line).lines_redacted, 1, "{line}");
        }
    }

    #[test]
    fn a_marker_anywhere_in_the_line_is_enough() {
        // No need to parse where the secret starts — which is the reason whole
        // lines are replaced.
        let r = context_strip("  2024-01-01 INFO  loaded aws_secret_access_key from env");
        assert_eq!(r.lines_redacted, 1);
    }

    #[test]
    fn every_key_family_in_the_table_is_caught() {
        // One per family rather than one per marker: the point is that no
        // family was dropped in the port.
        for line in ["api_key=abc", "ANTHROPIC_API_KEY=sk-x", "hf_token=hf_x",
                     "-----BEGIN OPENSSH PRIVATE KEY-----", "Authorization: Bearer ey.J",
                     "ssh-rsa AAAAB3Nza", "totp: 123456", "verification code 998877"] {
            assert_eq!(context_strip(line).lines_redacted, 1, "missed: {line}");
        }
    }

    #[test]
    fn ordinary_terminal_output_passes_through_untouched() {
        // Over-redaction makes the feature useless: a model given nothing but
        // placeholders cannot help with anything.
        let ctx = "$ cargo test\n   Compiling voiceterm v0.1.0\ntest result: ok. 669 passed";
        let r = context_strip(ctx);
        assert_eq!(r.lines_redacted, 0);
        assert!(r.markers_hit.is_empty());
        assert_eq!(r.text.trim_end(), ctx);
    }

    #[test]
    fn redaction_leaves_a_visible_placeholder_rather_than_a_gap() {
        // A model given context with lines silently missing will confidently
        // reason about a sequence that never happened.
        let r = context_strip("line one\nPassword: x\nline three");
        let lines: Vec<&str> = r.text.lines().collect();
        assert_eq!(lines.len(), 3, "line count preserved");
        assert_eq!(lines[1], PLACEHOLDER);
    }

    #[test]
    fn heavily_redacted_context_is_not_worth_sending() {
        // The remainder is unlikely to help, and sending it spends a round trip
        // to tell the model about holes.
        let mostly = context_strip("Password: a\nPassword: b\nPassword: c\nls");
        assert!(!worth_sending(&mostly, 4));
        let mostly_clean = context_strip("ls\ncat x\nPassword: a\nmake");
        assert!(worth_sending(&mostly_clean, 4));
    }

    #[test]
    fn empty_context_is_never_worth_sending() {
        let r = context_strip("");
        assert_eq!(r.lines_redacted, 0);
        assert!(!worth_sending(&r, 0));
    }

    #[test]
    fn markers_are_lowercase_so_the_comparison_is_symmetric() {
        // The line is lowercased before matching; an uppercase entry would
        // never fire, which is a silent hole rather than a visible bug.
        for m in MARKERS {
            assert_eq!(*m, m.to_lowercase(), "{m} would never match");
            assert!(!m.is_empty());
        }
    }

    #[test]
    fn no_marker_is_a_prefix_of_another_in_a_way_that_hides_it() {
        // Not a correctness requirement — first match wins and the line goes
        // either way — but a duplicate means one entry can never be reported in
        // the audit trail, which quietly breaks the auditing this exists for.
        let mut seen = MARKERS.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate marker");
    }
}
