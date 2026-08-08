//! patchcheck.rs — refuse to write a patch that breaks the file's structure.
//!
//! Every edit in this project goes through string replacement into an 18000-line file, and the
//! failure mode is not a wrong value — it is an unbalanced brace that turns the next thousand lines
//! into a syntax error the compiler reports somewhere else entirely.
//!
//! ## The trap that makes a naive counter useless here
//!
//! **This codebase is full of WGSL shaders in `r#"..."#` literals**, and shader source is nothing but
//! braces. A counter that does not skip raw strings reports every shader file as catastrophically
//! unbalanced, so it gets disabled, so it protects nothing.
//!
//! Comments have the same problem in the other direction: a `//` line containing `{` is common in
//! doc comments explaining syntax.
//!
//! ## Why balance and not a parse
//!
//! A parse needs the file to be nearly valid, which is exactly what a bad patch destroys. Counting
//! delimiters works on a broken file, which is when the answer is needed.

/// What is wrong with a patched file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Flaw {
    /// More openers than closers, or the reverse.
    Unbalanced { delim: char, opened: i32, closed: i32 },
    /// A raw string that never terminates — usually a mangled `r#"` or a lost `"#`.
    UnterminatedRawString,
    /// The patch changed balance that was already fine. **Reported separately from an
    /// already-broken file**, because a patch is only responsible for what it changed.
    IntroducedImbalance { delim: char, before: i32, after: i32 },
}

impl Flaw {
    pub fn explain(&self) -> String {
        match self {
            Flaw::Unbalanced { delim, opened, closed } => format!(
                "{opened} '{delim}' against {closed} closers — the compiler will report this \
somewhere far from the edit, which is why it is worth catching before the write"),
            Flaw::UnterminatedRawString =>
                "a raw string literal never closes — usually a mangled r#\" or a lost \"#, and in \
this codebase that means a shader body swallowed the rest of the file".into(),
            Flaw::IntroducedImbalance { delim, before, after } => format!(
                "'{delim}' balance went from {before} to {after}: the patch introduced this. The file \
may have had other problems already, but this one is ours"),
        }
    }
}

/// Count delimiter balance, skipping raw strings, normal strings, chars and comments.
///
/// Returns None if a raw string never terminates, because everything after that point is
/// uninterpretable and a count would be fiction.
pub fn balance(src: &str, delim: (char, char)) -> Option<i32> {
    let b = src.as_bytes();
    let (mut i, mut depth) = (0usize, 0i32);
    while i < b.len() {
        let c = b[i] as char;
        // Raw string: r#"..."# with any number of hashes. The reason this module exists.
        if c == 'r' && i + 1 < b.len() && (b[i + 1] == b'#' || b[i + 1] == b'"') {
            let mut h = 0;
            let mut j = i + 1;
            while j < b.len() && b[j] == b'#' { h += 1; j += 1; }
            if j < b.len() && b[j] == b'"' {
                let close: String = std::iter::once('"').chain(std::iter::repeat('#').take(h)).collect();
                match src[j + 1..].find(&close) {
                    Some(k) => { i = j + 1 + k + close.len(); continue }
                    None => return None,
                }
            }
        }
        match c {
            '"' => { // ordinary string
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' { i += 2; continue }
                    if b[i] == b'"' { break }
                    i += 1;
                }
            }
            '/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                // Line comment. Doc comments explaining syntax routinely contain unbalanced braces.
                while i < b.len() && b[i] != b'\n' { i += 1; }
                continue;
            }
            '\'' if i + 2 < b.len() && b[i + 2] == b'\'' => i += 2,   // char literal
            _ => {
                if c == delim.0 { depth += 1 }
                else if c == delim.1 { depth -= 1 }
            }
        }
        i += 1;
    }
    Some(depth)
}

/// Check a patched file, comparing against the original so a pre-existing problem is not blamed on
/// the patch.
pub fn check(before: &str, after: &str) -> Vec<Flaw> {
    let mut v = Vec::new();
    if balance(after, ('{', '}')).is_none() {
        v.push(Flaw::UnterminatedRawString);
        // No point counting anything else: everything past the unterminated literal is fiction.
        return v;
    }
    for d in [('{', '}'), ('(', ')'), ('[', ']')] {
        let (b, a) = (balance(before, d), balance(after, d));
        match (b, a) {
            // UNCHANGED comes first, or it falls through to Unbalanced and every subsequent patch to
            // an already-broken file looks guilty. A patch is responsible for what it changed.
            (Some(b), Some(a)) if a == b => {}
            (Some(b), Some(a)) if a != 0 => {
                v.push(Flaw::IntroducedImbalance { delim: d.0, before: b, after: a });
            }
            (None, Some(a)) if a != 0 => {
                let (o, c) = if a > 0 { (a, 0) } else { (0, -a) };
                v.push(Flaw::Unbalanced { delim: d.0, opened: o, closed: c });
            }
            _ => {}
        }
    }
    v
}

/// How many times to retry a failing edit before stopping.
///
/// **Three, and it is the same three as `longrun::THEORIES_BEFORE_MEASURING`** — arrived at
/// independently from the same evidence. A fourth attempt at an edit that has failed three times is
/// the same reasoning applied harder, and the answer is to look at the file rather than to try again.
pub const MAX_EDIT_ATTEMPTS: u32 = 3;

/// Tracks repeated attempts at one edit.
#[derive(Clone, Debug, Default)]
pub struct EditLoop { attempts: u32, last_flaws: Vec<Flaw> }

impl EditLoop {
    /// Record an attempt. Returns whether to keep trying.
    pub fn attempt(&mut self, flaws: Vec<Flaw>) -> Verdict {
        self.attempts += 1;
        if flaws.is_empty() { return Verdict::Accept }
        // The SAME flaw twice means the fix is not addressing it. That is worth stopping on sooner
        // than three unrelated failures, because repetition is stronger evidence than count.
        let repeated = flaws == self.last_flaws;
        self.last_flaws = flaws;
        if repeated {
            return Verdict::Stop { reason: "the same structural flaw twice — the edit is not \
addressing it, and a third identical attempt will not either" };
        }
        if self.attempts >= MAX_EDIT_ATTEMPTS {
            return Verdict::Stop { reason: "three failed attempts — read the surrounding lines \
instead of editing again; the anchor is probably not where it is assumed to be" };
        }
        Verdict::Retry { attempts: self.attempts }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Verdict { Accept, Retry { attempts: u32 }, Stop { reason: &'static str } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_WGSL_SHADER_LITERAL_does_not_count_as_unbalanced() {
        // The trap that makes a naive counter useless here. Shader source is nothing but braces, and
        // a checker that flags every shader gets turned off and protects nothing.
        let src = r####"
fn real() {
    const S: &str = r#"
        fn main() { if (x > 0u) { y = 1u; } }
    "#;
}
"####;
        assert_eq!(balance(src, ('{', '}')), Some(0));
    }

    #[test]
    fn a_COMMENT_containing_a_brace_is_ignored() {
        // Doc comments explaining syntax routinely carry unbalanced braces.
        assert_eq!(balance("// this opens a block with {\nfn f() {}\n", ('{', '}')), Some(0));
    }

    #[test]
    fn an_UNTERMINATED_raw_string_is_named_specifically() {
        // In this codebase it means a shader body swallowed the rest of the file, which is a very
        // different diagnosis from a missing brace.
        let flaws = check("fn f() {}", "fn f() {}\nconst S: &str = r#\"unclosed");
        assert_eq!(flaws, vec![Flaw::UnterminatedRawString]);
        assert!(flaws[0].explain().contains("swallowed the rest of the file"));
    }

    #[test]
    fn a_PATCH_is_blamed_only_for_what_it_changed() {
        // A file that was already unbalanced should not make every subsequent patch look guilty.
        let before = "fn f() {";                    // already broken
        let after  = "fn f() {\n    let x = 1;";    // still broken, same amount
        assert!(check(before, after).is_empty(), "pre-existing imbalance is not this patch's fault");
    }

    #[test]
    fn an_INTRODUCED_imbalance_is_reported_as_ours() {
        let flaws = check("fn f() {}", "fn f() {");
        assert!(matches!(flaws[0], Flaw::IntroducedImbalance { .. }), "{flaws:?}");
        assert!(flaws[0].explain().contains("this one is ours"));
    }

    #[test]
    fn THREE_failed_attempts_stops_and_says_what_to_do_instead() {
        // Same three as longrun's, reached independently from the same evidence.
        let mut e = EditLoop::default();
        let f1 = vec![Flaw::Unbalanced { delim: '{', opened: 1, closed: 0 }];
        let f2 = vec![Flaw::Unbalanced { delim: '(', opened: 1, closed: 0 }];
        let f3 = vec![Flaw::UnterminatedRawString];
        assert!(matches!(e.attempt(f1), Verdict::Retry { .. }));
        assert!(matches!(e.attempt(f2), Verdict::Retry { .. }));
        match e.attempt(f3) {
            Verdict::Stop { reason } => assert!(reason.contains("read the surrounding lines")),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn the_SAME_flaw_twice_stops_immediately() {
        // Repetition is stronger evidence than count: an identical failure means the fix is not
        // addressing it, and a third identical attempt will not either.
        let mut e = EditLoop::default();
        let f = vec![Flaw::Unbalanced { delim: '{', opened: 1, closed: 0 }];
        assert!(matches!(e.attempt(f.clone()), Verdict::Retry { .. }));
        match e.attempt(f) {
            Verdict::Stop { reason } => assert!(reason.contains("not addressing it")),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn a_clean_patch_is_ACCEPTED() {
        let mut e = EditLoop::default();
        assert_eq!(e.attempt(check("fn f() {}", "fn f() { let x = 1; }")), Verdict::Accept);
    }
}

/// Checking an edit's ANCHOR before its content.
///
/// `EditLoop` grades an attempt by the flaws in what was produced. That is the second question. The
/// first is whether the edit will land where it was meant to — and this session hit the answer twice
/// in two turns, inserting text whose anchor matched inside the text it had just inserted, producing
/// `pub const BUILD_BIAS: &str = pub const BUILD_BIAS: &str =`.
///
/// **`anchor.rs` was written to catch exactly that and was not used**, because it was a tool to
/// reach for rather than a step in the loop. Wiring it here makes it the first thing that happens.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EditPlan {
    /// The anchor is unique; apply it.
    Apply { line: usize },
    /// The anchor matched several places and was widened successfully.
    Widened { from: String, to: String, line: usize },
    /// Cannot be made unique. **Not a failure of the edit** — a failure to say where it goes, which
    /// wants a different response from a bad patch.
    Ambiguous { reason: String, guidance: &'static str },
    /// The anchor matches nothing, usually whitespace or a stale read.
    Absent { guidance: &'static str },
}

impl EditPlan {
    pub fn can_apply(&self) -> bool {
        matches!(self, EditPlan::Apply { .. } | EditPlan::Widened { .. })
    }

    /// One line for the log, and **only when something happened**. An edit that applied cleanly is
    /// the normal case and logging it buries the two that did not.
    pub fn log_line(&self) -> Option<String> {
        match self {
            EditPlan::Apply { .. } => None,
            EditPlan::Widened { from, to, line } => Some(format!(
                "anchor widened from {} to {} lines to reach line {line} — the short form matched \
more than once",
                from.lines().count(), to.lines().count())),
            EditPlan::Ambiguous { reason, guidance } =>
                Some(format!("edit NOT applied: {reason}. {guidance}")),
            EditPlan::Absent { guidance } =>
                Some(format!("edit NOT applied: the anchor matches nothing. {guidance}")),
        }
    }
}

/// Plan an edit against the file it will touch.
///
/// Widening is attempted before giving up, because "your anchor is ambiguous" is a complaint and a
/// wider anchor is an answer.
pub fn plan_edit(haystack: &str, anchor: &str) -> EditPlan {
    match crate::anchor::check(haystack, anchor) {
        Ok(line) => EditPlan::Apply { line },
        Err(crate::anchor::AnchorFault::Absent) =>
            EditPlan::Absent { guidance: crate::anchor::guidance(&crate::anchor::AnchorFault::Absent) },
        Err(f @ crate::anchor::AnchorFault::Ambiguous { .. }) => {
            match crate::anchor::widen(haystack, anchor, 8) {
                Some(w) if w != anchor => match crate::anchor::check(haystack, &w) {
                    Ok(line) => EditPlan::Widened { from: anchor.into(), to: w, line },
                    // Widening produced something that still is not unique. Report the ORIGINAL
                    // fault rather than a confusing second one.
                    Err(_) => EditPlan::Ambiguous { reason: f.explain(),
                        guidance: crate::anchor::guidance(&f) },
                },
                _ => EditPlan::Ambiguous { reason: f.explain(),
                    guidance: crate::anchor::guidance(&f) },
            }
        }
    }
}

#[cfg(test)]
mod edit_plan_tests {
    use super::*;

    const TWO: &str = "fn spawn() {\n    push_car(lane);\n}\n\nfn respawn() {\n    push_car(lane);\n}\n";

    #[test]
    fn an_AMBIGUOUS_anchor_is_widened_rather_than_refused() {
        // "Your anchor is ambiguous" is a complaint; a wider anchor is an answer.
        let p = plan_edit(TWO, "    push_car(lane);");
        assert!(p.can_apply(), "{p:?}");
        assert!(matches!(p, EditPlan::Widened { .. }));
    }

    #[test]
    fn a_CLEAN_edit_logs_NOTHING() {
        // The normal case. Logging it buries the two that did not apply, which is the same rule the
        // run verdict's caveat follows.
        assert_eq!(plan_edit(TWO, "fn respawn() {").log_line(), None);
    }

    #[test]
    fn a_WIDENED_edit_says_so_and_says_WHY() {
        // Silent widening is a change to what the caller asked for, and the log line is what makes it
        // reviewable rather than surprising.
        let l = plan_edit(TWO, "    push_car(lane);").log_line().unwrap();
        assert!(l.contains("matched more than once"));
    }

    #[test]
    fn an_UNFIXABLE_anchor_carries_a_PROCEDURE_not_a_complaint() {
        let src = "x();\n".repeat(20);
        let p = plan_edit(&src, "x();");
        assert!(!p.can_apply());
        assert!(p.log_line().unwrap().contains("enclosing function's signature line"));
    }

    #[test]
    fn an_ABSENT_anchor_is_told_to_RE_READ() {
        // Whitespace or a stale read, and neither is fixed by thinking harder about the intent.
        let p = plan_edit(TWO, "fn nonexistent() {");
        assert!(matches!(p, EditPlan::Absent { .. }));
        assert!(p.log_line().unwrap().contains("copy them verbatim"));
    }

    #[test]
    fn the_SELF_INFLICTED_case_is_covered() {
        // Inserting before an anchor that appears in the inserted text: after the first insert the
        // anchor matches twice, and the second insert lands inside the first. Producing
        // `pub const X = pub const X =`, twice in two turns this session.
        let after_first = "pub const BUILD_BIAS: &str = pub const BUILD_BIAS: &str = \"x\";\n";
        // My first assertion here was that plan_edit REFUSES this, and it does not — widening finds
        // a unique wider anchor, which is correct behaviour and not what I wanted to prove.
        //
        // The real guard is one step earlier: `check` sees the anchor twice BEFORE anything is
        // inserted. That is the signal that would have stopped both incidents, because on the FIRST
        // insert the anchor was still unique and on the second it was not.
        assert!(matches!(crate::anchor::check(after_first, "pub const BUILD_BIAS: &str = "),
                         Err(crate::anchor::AnchorFault::Ambiguous { matches: 2, .. })),
            "a doubled declaration must be visible as ambiguous before a third insert");
        // And the fault is flagged as the SILENT kind, which is why it survived twice.
        let f = crate::anchor::check(after_first, "pub const BUILD_BIAS: &str = ").unwrap_err();
        assert!(f.fails_silently());
    }
}
