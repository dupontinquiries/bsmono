//! codefmt.rs — check the code a model just emitted, behind `LOGSCOPE_DEBUG_FORMAT`.
//!
//! Small models format code badly in a few specific, mechanical ways, and every one of them is
//! detectable without understanding the language.
//!
//! ## What this does NOT do, on purpose
//!
//! It does not rewrite the code. A formatter that silently repairs a model's output makes broken code
//! *look* correct — the reader loses the one signal that something went wrong, and a missing brace
//! becomes a missing brace in a file that appears well-formed. Diagnosis and cosmetics are different
//! jobs and only the first is safe to do automatically.
//!
//! So: it reports. The reader decides whether to regenerate, and knows which block to look at.
//!
//! ## The failure that matters most
//!
//! An **unterminated fence** is both the commonest and the worst. A block opened with ``` and never
//! closed swallows the entire rest of the message — prose, next steps, everything after it renders as
//! code. The reply looks truncated when it is merely mis-delimited, and the useful part is sitting
//! there unread.

/// A finding about one code block.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Finding {
    /// A fence was opened and never closed. Everything after it renders as code.
    UnterminatedFence { line: usize, lang: String },
    /// Unbalanced delimiters within a block.
    Unbalanced { line: usize, lang: String, kind: char, opened: i32 },
    /// A closing delimiter with nothing open.
    Stray { line: usize, lang: String, kind: char },
    /// Mixed tabs and spaces in the leading whitespace of one block.
    MixedIndent { line: usize, lang: String },
}

impl Finding {
    pub fn explain(&self) -> String {
        match self {
            Finding::UnterminatedFence { line, lang } => format!(
                "code block opened at line {line}{} is never closed — everything after it renders as \
code, so the rest of the reply is there but invisible",
                if lang.is_empty() { String::new() } else { format!(" ({lang})") }),
            Finding::Unbalanced { line, lang, kind, opened } => format!(
                "block at line {line} ({lang}) has {opened} unclosed '{kind}'"),
            Finding::Stray { line, lang, kind } => format!(
                "block at line {line} ({lang}) closes a '{kind}' that was never opened"),
            Finding::MixedIndent { line, lang } => format!(
                "block at line {line} ({lang}) mixes tabs and spaces in its indentation"),
        }
    }
}

/// Is the checker enabled?
///
/// Off by default: on a well-behaved model every reply would carry an empty check, and a diagnostic
/// that usually says nothing trains people to skip it.
pub fn enabled() -> bool {
    matches!(std::env::var("LOGSCOPE_DEBUG_FORMAT").ok().as_deref(), Some("1") | Some("true"))
}

/// Check every fenced block in a reply.
pub fn check(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String, Vec<char>, bool, bool)> = None; // line, lang, stack, tab, space

    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let t = raw.trim_start();
        if t.starts_with("```") {
            match open.take() {
                // Closing fence: report what the block accumulated.
                Some((start, lang, stack, tab, space)) => {
                    for c in stack.iter().rev().take(1) {
                        out.push(Finding::Unbalanced {
                            line: start, lang: lang.clone(), kind: *c,
                            opened: stack.iter().filter(|x| *x == c).count() as i32,
                        });
                    }
                    if tab && space {
                        out.push(Finding::MixedIndent { line: start, lang });
                    }
                }
                None => {
                    let lang = t.trim_start_matches('`').trim().to_string();
                    open = Some((line, lang, Vec::new(), false, false));
                }
            }
            continue;
        }
        if let Some((_, lang, stack, tab, space)) = open.as_mut() {
            let lead: String = raw.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            if lead.contains('\t') { *tab = true; }
            if lead.contains(' ') { *space = true; }
            // Delimiters only; quotes and comments are language-specific and guessing them wrong
            // produces false findings, which is worse than missing a real one here.
            for c in raw.chars() {
                match c {
                    '{' | '(' | '[' => stack.push(c),
                    '}' | ')' | ']' => {
                        let want = match c { '}' => '{', ')' => '(', _ => '[' };
                        if stack.last() == Some(&want) { stack.pop(); }
                        else { out.push(Finding::Stray { line, lang: lang.clone(), kind: c }); }
                    }
                    _ => {}
                }
            }
        }
    }

    // A fence still open at the end is the important one, and it is reported last so it reads as the
    // conclusion rather than as one item among several.
    if let Some((start, lang, _, _, _)) = open {
        out.push(Finding::UnterminatedFence { line: start, lang });
    }
    out
}

/// One line summarising a check, or None when the reply is clean.
pub fn summary(findings: &[Finding]) -> Option<String> {
    if findings.is_empty() { return None; }
    let unterminated = findings.iter().any(|f| matches!(f, Finding::UnterminatedFence { .. }));
    Some(format!("code check — {} finding{}{}: {}",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" },
        if unterminated { " (one is an UNTERMINATED FENCE, which hides the rest of the reply)" } else { "" },
        findings.iter().map(|f| f.explain()).collect::<Vec<_>>().join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_balanced_block_produces_NOTHING() {
        // The common case must be silent, or the check becomes a line people skip.
        let t = "here:\n```rust\nfn a() { let x = (1 + 2); }\n```\ndone";
        assert!(check(t).is_empty());
        assert!(summary(&check(t)).is_none());
    }

    #[test]
    fn an_unterminated_fence_is_reported_and_named_as_hiding_the_reply() {
        // The worst failure: everything after the fence renders as code, so the reply looks truncated
        // when it is merely mis-delimited.
        let t = "sure:\n```python\ndef f():\n    return 1\n\nand then you would run it";
        let f = check(t);
        assert!(matches!(f.last(), Some(Finding::UnterminatedFence { .. })));
        assert!(summary(&f).unwrap().contains("hides the rest of the reply"));
    }

    #[test]
    fn a_missing_brace_names_the_block_that_has_it() {
        let t = "```rust\nfn a() {\n    if x {\n        y();\n    }\n```";
        let f = check(t);
        assert!(f.iter().any(|x| matches!(x, Finding::Unbalanced { kind: '{', .. })), "{f:?}");
    }

    #[test]
    fn a_stray_closer_is_distinguished_from_a_missing_opener() {
        // Different fixes: one means delete a line, the other means add one. Reporting both as
        // "unbalanced" would leave the reader to work out which.
        let t = "```rust\n    y();\n}\n```";
        let f = check(t);
        assert!(f.iter().any(|x| matches!(x, Finding::Stray { kind: '}', .. })), "{f:?}");
    }

    #[test]
    fn prose_OUTSIDE_a_fence_is_never_checked() {
        // English is full of unbalanced brackets — "(see above" and smileys — and flagging them would
        // make the check useless on the first paragraph it met.
        let t = "as discussed (see the earlier note :) the answer is 42";
        assert!(check(t).is_empty());
    }

    #[test]
    fn mixed_indentation_is_reported_per_BLOCK_not_per_line() {
        // A model that mixes tabs and spaces does it throughout, so one finding is the signal and
        // forty are noise.
        let t = "```python\ndef f():\n\treturn 1\n    # spaces here\n```";
        let f = check(t);
        assert_eq!(f.iter().filter(|x| matches!(x, Finding::MixedIndent { .. })).count(), 1);
    }

    #[test]
    fn it_is_OFF_unless_explicitly_enabled() {
        // On a well-behaved model every reply carries an empty check, and a diagnostic that usually
        // says nothing trains people to skip it.
        std::env::remove_var("LOGSCOPE_DEBUG_FORMAT");
        assert!(!enabled());
        std::env::set_var("LOGSCOPE_DEBUG_FORMAT", "1");
        assert!(enabled());
        std::env::remove_var("LOGSCOPE_DEBUG_FORMAT");
    }
}
