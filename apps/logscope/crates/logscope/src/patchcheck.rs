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
