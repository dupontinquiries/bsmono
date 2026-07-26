//! bracecheck.rs — a delimiter check that sits between "it compiled" and "it ran".
//!
//! Rust's own error for an unbalanced file (`unexpected closing delimiter`) points at the LAST brace, not
//! the missing one, so it routinely blames a line hundreds away from the edit that caused it. That is a
//! bad signal to hand an autonomous fix loop: it retries against the wrong location. This scans for the
//! imbalance directly and, where the repair is unambiguous, proposes it.
//!
//! Lexing matters more than the counting. A naive counter breaks on `"{"`, `'}'`, `// {`, `/* { */`, and
//! `r#"{"#`, all of which are common in this codebase — so the scanner tracks Rust string, raw-string,
//! char and (nestable) block-comment state. The classic ambiguity is `'`: a char literal versus a
//! lifetime. `'a'` is a literal, `&'a str` is not, and getting it wrong desynchronises everything after.
//!
//! ## What it will and will not fix
//!
//! Only the unambiguous direction. Closers missing at end of file can be appended with confidence. An
//! UNEXPECTED closer means an opener went missing somewhere earlier, and where to reinsert it is a guess —
//! so that case is reported, never patched. Guessing there would produce a file that compiles and is
//! wrong, which is the failure mode this whole codebase keeps tripping over.

/// Where the delimiters stopped balancing.
#[derive(Clone, Debug, PartialEq)]
pub enum Imbalance {
    /// An opener that was never closed. `line` is 1-based, at the opener.
    Unclosed { ch: char, line: usize },
    /// A closer with no matching opener (or the wrong one). `expected` is what was open, if anything.
    Unexpected { ch: char, line: usize, expected: Option<char> },
}

impl Imbalance {
    pub fn line(&self) -> usize {
        match self { Imbalance::Unclosed { line, .. } | Imbalance::Unexpected { line, .. } => *line }
    }
    /// Can this be repaired without guessing?
    pub fn is_safely_fixable(&self) -> bool { matches!(self, Imbalance::Unclosed { .. }) }
}

fn closer_for(ch: char) -> char { match ch { '(' => ')', '[' => ']', _ => '}' } }

/// Scan Rust source for delimiter imbalance. Empty result means balanced.
///
/// Reports at most one `Unexpected` (the first), because everything after a stray closer is unreliable —
/// continuing would emit a cascade of phantom errors that hide the real one.
pub fn scan(src: &str) -> Vec<Imbalance> {
    let b: Vec<char> = src.chars().collect();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut out = Vec::new();
    let (mut i, mut line) = (0usize, 1usize);
    let mut block_depth = 0usize;

    while i < b.len() {
        let c = b[i];
        if c == '\n' { line += 1; i += 1; continue; }

        // nestable block comments
        if block_depth > 0 {
            if c == '/' && b.get(i + 1) == Some(&'*') { block_depth += 1; i += 2; continue; }
            if c == '*' && b.get(i + 1) == Some(&'/') { block_depth -= 1; i += 2; continue; }
            i += 1; continue;
        }
        if c == '/' && b.get(i + 1) == Some(&'*') { block_depth = 1; i += 2; continue; }
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' { i += 1; }
            continue;
        }

        // raw strings: r"..." / r#"..."# / br#"..."#
        if c == 'r' || (c == 'b' && b.get(i + 1) == Some(&'r')) {
            let mut j = i + if c == 'b' { 2 } else { 1 };
            let hashes = { let s = j; while b.get(j) == Some(&'#') { j += 1; } j - s };
            if b.get(j) == Some(&'"') {
                j += 1;
                loop {
                    match b.get(j) {
                        None => { i = j; break; }
                        Some('\n') => { line += 1; j += 1; }
                        Some('"') => {
                            let mut k = j + 1; let mut n = 0;
                            while n < hashes && b.get(k) == Some(&'#') { k += 1; n += 1; }
                            if n == hashes { i = k; break; }
                            j += 1;
                        }
                        _ => j += 1,
                    }
                }
                continue;
            }
        }

        // normal string
        if c == '"' {
            i += 1;
            while i < b.len() {
                match b[i] {
                    '\\' => i += 2,
                    '"' => { i += 1; break; }
                    '\n' => { line += 1; i += 1; }
                    _ => i += 1,
                }
            }
            continue;
        }

        // char literal vs lifetime. `'a'` and `'\n'` are literals; `&'a str` and `'static` are not.
        // A literal is: quote, then (escape or single char), then quote.
        if c == '\'' {
            let is_literal = match b.get(i + 1) {
                Some('\\') => true,
                Some(_) => b.get(i + 2) == Some(&'\''),
                None => false,
            };
            if is_literal {
                i += 1;
                while i < b.len() {
                    match b[i] { '\\' => i += 2, '\'' => { i += 1; break; } _ => i += 1 }
                }
            } else {
                i += 1; // a lifetime: skip the quote, let the identifier lex normally
            }
            continue;
        }

        match c {
            '(' | '[' | '{' => { stack.push((c, line)); i += 1; }
            ')' | ']' | '}' => {
                match stack.pop() {
                    Some((open, _)) if closer_for(open) == c => {}
                    Some((open, oline)) => {
                        out.push(Imbalance::Unexpected { ch: c, line, expected: Some(closer_for(open)) });
                        // everything after a mismatch is unreliable — report the opener and stop
                        out.push(Imbalance::Unclosed { ch: open, line: oline });
                        return out;
                    }
                    None => {
                        out.push(Imbalance::Unexpected { ch: c, line, expected: None });
                        return out;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    for (ch, line) in stack.into_iter().rev() { out.push(Imbalance::Unclosed { ch, line }); }
    out
}

/// Balanced?
pub fn is_balanced(src: &str) -> bool { scan(src).is_empty() }

/// Repair only the unambiguous case: append the closers for openers left open at end of file, innermost
/// first. Returns None when there is nothing to do, or when any imbalance is an unexpected closer — that
/// means a missing OPENER, whose position cannot be inferred without guessing.
pub fn fix(src: &str) -> Option<String> {
    let issues = scan(src);
    if issues.is_empty() { return None; }
    if !issues.iter().all(|i| i.is_safely_fixable()) { return None; }
    let mut out = src.to_string();
    if !out.ends_with('\n') { out.push('\n'); }
    for i in &issues {
        if let Imbalance::Unclosed { ch, .. } = i { out.push(closer_for(*ch)); out.push('\n'); }
    }
    // the repair must actually balance, or it is not a repair
    if is_balanced(&out) { Some(out) } else { None }
}

/// Outcome of the check → fix → recheck loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Compiled as given; nothing was changed.
    PassedAsIs,
    /// Did not compile, braces were unbalanced, the repair fixed it and it compiled.
    PassedAfterFix { fixed: String, issues: Vec<Imbalance> },
    /// Did not compile and braces were balanced — the failure is something else, so nothing was touched.
    FailedNotBraces,
    /// Braces were unbalanced but the repair was not safe or did not help.
    FailedUnfixable { issues: Vec<Imbalance> },
}

/// compile → (if it failed) brace-check → fix → compile again.
///
/// `compiles` is supplied by the caller so this stays pure and testable; in the devloop it is a real
/// `cargo check`. The order matters: the brace check runs only AFTER a compile failure, because a file can
/// be perfectly balanced and still not compile, and running the fixer speculatively would rewrite files
/// that were never broken.
pub fn verify_with_brace_fix(src: &str, compiles: &dyn Fn(&str) -> bool) -> Verdict {
    if compiles(src) { return Verdict::PassedAsIs; }
    let issues = scan(src);
    if issues.is_empty() { return Verdict::FailedNotBraces; }
    match fix(src) {
        Some(fixed) if compiles(&fixed) => Verdict::PassedAfterFix { fixed, issues },
        _ => Verdict::FailedUnfixable { issues },
    }
}

/// How to present a brace repair that rode along with a model's edit.
///
/// The model produced one patch; the brace fixer produced another. Which to show depends on what the
/// reader is for:
///
/// * `Separate` keeps them distinct, so a reviewer can see that the engine touched the file and exactly
///   what it did. This is the honest default — a repair folded invisibly into someone else's patch is a
///   change nobody agreed to.
/// * `Merged` presents a single original→final diff, which is what you want when recording the result or
///   handing it to something that applies one patch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PatchStyle { Separate, Merged }

/// A brace repair rendered as patch text against `path`.
///
/// `Separate` yields two entries — the model's own patch is the caller's to supply, this is the second —
/// while `Merged` yields one diff from the ORIGINAL text to the final text, so the repair is not
/// distinguishable from the edit. Both describe the same end state.
pub fn repair_patch(path: &str, original: &str, fixed: &str, style: PatchStyle) -> Vec<String> {
    if original == fixed { return Vec::new(); }
    let added: Vec<&str> = fixed.lines().skip(original.lines().count()).collect();
    match style {
        // Only appended closers can reach here (fix() refuses anything else), so the repair is a pure
        // append and can be shown as one.
        PatchStyle::Separate => vec![format!(
            "*** path: {path}
@@
{}", added.iter().map(|l| format!("+{l}
")).collect::<String>())],
        PatchStyle::Merged => vec![format!(
            "*** path: {path}
@@
{}{}",
            original.lines().rev().take(2).collect::<Vec<_>>().into_iter().rev()
                .map(|l| format!(" {l}
")).collect::<String>(),
            added.iter().map(|l| format!("+{l}
")).collect::<String>())],
    }
}

/// A one-line account for the loop's context: what happened, and whether anything was rewritten.
pub fn verdict_line(v: &Verdict) -> String {
    match v {
        Verdict::PassedAsIs => "cargo check passed; braces balanced; nothing rewritten".into(),
        Verdict::PassedAfterFix { issues, .. } => format!(
            "cargo check passed AFTER a brace repair ({}) — the engine appended {} closer(s), which is a \
change the model did not make",
            report(issues).join("; "), issues.len()),
        Verdict::FailedNotBraces =>
            "cargo check failed and braces are balanced — the error is elsewhere, nothing rewritten".into(),
        Verdict::FailedUnfixable { issues } => format!(
            "cargo check failed and braces are unbalanced, but the repair is not safe: {}",
            report(issues).join("; ")),
    }
}

/// Human-readable account, for the loop's feedback and for the operator.
pub fn report(issues: &[Imbalance]) -> Vec<String> {
    issues.iter().map(|i| match i {
        Imbalance::Unclosed { ch, line } =>
            format!("line {line}: '{ch}' opened here is never closed"),
        Imbalance::Unexpected { ch, line, expected } => match expected {
            Some(e) => format!("line {line}: found '{ch}' but '{e}' was expected — an opener is missing earlier; NOT auto-fixable"),
            None => format!("line {line}: '{ch}' closes nothing — an opener is missing earlier; NOT auto-fixable"),
        },
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_code_with_awkward_literals_is_left_alone() {
        // every one of these breaks a naive counter
        let src = r###"
fn f() {
    let a = "{";
    let b = '}';
    let c = '\'';
    // }
    /* } /* nested */ } */
    let d = r#"unclosed { in a raw string"#;
    let e: &'static str = "x";
    let g = vec!['{', '}'];
}
"###;
        assert!(is_balanced(src), "{:?}", scan(src));
        assert!(fix(src).is_none(), "nothing to fix");
    }

    #[test]
    fn a_missing_closer_is_found_and_appended() {
        let src = "fn a() {\n    if true {\n        let x = 1;\n    }\n";  // fn's brace never closed
        let issues = scan(src);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0], Imbalance::Unclosed { ch: '{', line: 1 });
        assert!(issues[0].is_safely_fixable());
        let fixed = fix(src).expect("appendable");
        assert!(is_balanced(&fixed));
        assert!(fixed.starts_with(src), "the original text is preserved verbatim");
    }

    #[test]
    fn a_stray_closer_is_reported_but_never_patched() {
        // This is the real shape of the failure that bit this session: an edit removed a `fn` header,
        // leaving its body's closing brace with nothing to close. Where to reinsert the opener is a
        // guess, so it must NOT be auto-fixed.
        let src = "fn a() {\n    let x = 1;\n}\n}\n";
        let issues = scan(src);
        assert!(!issues.is_empty());
        assert!(issues.iter().any(|i| matches!(i, Imbalance::Unexpected { .. })));
        assert!(!issues[0].is_safely_fixable());
        assert!(fix(src).is_none(), "a missing opener must never be guessed at");
        assert!(report(&issues)[0].contains("NOT auto-fixable"));
    }

    #[test]
    fn a_mismatched_pair_names_what_was_expected() {
        let src = "fn a() {\n    let v = vec![1, 2);\n}\n";
        let issues = scan(src);
        let first = &issues[0];
        assert!(matches!(first, Imbalance::Unexpected { ch: ')', expected: Some(']'), .. }), "{first:?}");
        assert!(report(&issues)[0].contains("but ']' was expected"));
    }

    #[test]
    fn lifetimes_do_not_desynchronise_the_scanner() {
        // `'a` is not a char literal; treating it as one would swallow the following `{`
        let src = "struct S<'a> { x: &'a str }\nfn f<'a>(y: &'a [u8]) -> &'a str { \"z\" }\n";
        assert!(is_balanced(src), "{:?}", scan(src));
    }

    #[test]
    fn the_loop_only_touches_a_file_that_failed_for_brace_reasons() {
        let good = "fn a() {}\n";
        assert_eq!(verify_with_brace_fix(good, &|_| true), Verdict::PassedAsIs);

        // fails to compile, but braces are fine → do not rewrite it
        assert_eq!(verify_with_brace_fix(good, &|_| false), Verdict::FailedNotBraces);

        // fails, braces unbalanced, repair works → passes after fix
        let broken = "fn a() {\n";
        let v = verify_with_brace_fix(broken, &|s: &str| is_balanced(s));
        match v {
            Verdict::PassedAfterFix { fixed, issues } => {
                assert!(is_balanced(&fixed));
                assert_eq!(issues.len(), 1);
            }
            other => panic!("expected PassedAfterFix, got {other:?}"),
        }

        // unbalanced the unfixable way → reported, not patched
        let stray = "fn a() {}\n}\n";
        assert!(matches!(verify_with_brace_fix(stray, &|_| false), Verdict::FailedUnfixable { .. }));
    }

    #[test]
    fn a_repair_is_offered_separately_or_merged_and_both_describe_the_same_end_state() {
        let original = "fn a() {\n    let x = 1;\n";
        let fixed = fix(original).expect("appendable");
        let sep = repair_patch("src/a.rs", original, &fixed, PatchStyle::Separate);
        let mer = repair_patch("src/a.rs", original, &fixed, PatchStyle::Merged);
        assert_eq!(sep.len(), 1);
        assert_eq!(mer.len(), 1);
        // separate is pure additions; merged anchors on trailing context so it applies against the original
        assert!(!sep[0].lines().any(|l| l.starts_with(' ') && !l.trim().is_empty()));
        assert!(mer[0].lines().any(|l| l.starts_with(" ")), "merged carries context: {}", mer[0]);
        assert!(sep[0].contains("+}") && mer[0].contains("+}"));
        // and nothing is emitted when nothing changed
        assert!(repair_patch("src/a.rs", original, original, PatchStyle::Merged).is_empty());
    }

    #[test]
    fn the_verdict_line_says_when_the_engine_rewrote_something() {
        let broken = "fn a() {\n";
        let v = verify_with_brace_fix(broken, &|s: &str| is_balanced(s));
        let line = verdict_line(&v);
        assert!(line.contains("passed AFTER a brace repair"), "{line}");
        assert!(line.contains("the model did not make"), "an engine-made edit must be flagged: {line}");
        // and the clean case says plainly that nothing was touched
        assert!(verdict_line(&Verdict::PassedAsIs).contains("nothing rewritten"));
        assert!(verdict_line(&Verdict::FailedNotBraces).contains("error is elsewhere"));
    }

    #[test]
    fn a_repair_that_does_not_balance_is_rejected() {
        // fix() re-checks its own output; if appending closers would not balance, it declines rather than
        // returning something that merely looks repaired
        let stray = "}\nfn a() {\n";
        assert!(fix(stray).is_none());
    }
}
