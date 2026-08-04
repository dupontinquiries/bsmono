//! symbolfind.rs — telling a definition from a use, so a window anchors on the right line.
//!
//! `filewindow` plans which lines to read once the interesting ones are known. Finding them is grep,
//! and grep returns everything: the definition, every call site, the doc comment that mentions it,
//! and the test fixture that names it.
//!
//! **Reading a use while believing it is the definition is a specific, repeated failure here.**
//! Searching for `MATVEC_IQ1_S` in one session returned a dispatch call at 10833 and a lookup-table
//! entry at 13434 before the actual `const` — and both looked like answers.
//!
//! ## Ranking, not filtering
//!
//! A filter that dropped uses would break the other common question: *where is this called from?* So
//! every hit is kept and classified, and the caller says which kind it wanted.

/// What a matching line actually is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Hit {
    /// `fn foo`, `struct Foo`, `const FOO:` — the thing itself.
    Definition,
    /// A call, a reference, a field access.
    Use,
    /// Inside a `#[cfg(test)]` region. **Ranked below a use on purpose**: a test naming a symbol is
    /// evidence it exists, not evidence about how production calls it.
    TestUse,
    /// A doc or line comment mentioning the name. Often the most *readable* explanation and the least
    /// reliable — comments outlive the code they describe, which this codebase has demonstrated.
    Comment,
    /// A string literal. Almost always a shader body, a log line, or a lookup table.
    StringLiteral,
}

/// Classify one matching line.
///
/// Deliberately syntactic rather than semantic: a real parse would be better and would also mean the
/// answer depends on the file compiling, which is exactly when this is most needed.
pub fn classify(line: &str, name: &str, in_test_mod: bool) -> Hit {
    let t = line.trim_start();
    if t.starts_with("//") { return Hit::Comment }
    // A definition keyword immediately before the name. Checked before the test flag, because a
    // helper defined inside a test module is still a definition and pretending otherwise sends the
    // reader hunting for one that does not exist.
    for kw in ["fn ", "struct ", "enum ", "const ", "static ", "trait ", "type ", "mod ", "macro_rules!"] {
        if let Some(p) = t.find(kw) {
            let after = &t[p + kw.len()..];
            let head = &t[..p];
            let is_decl = head.is_empty() || head.split_whitespace()
                .all(|w| matches!(w, "pub" | "async" | "unsafe" | "extern" | "pub(crate)" | "impl"));
            if is_decl && after.trim_start().starts_with(name) { return Hit::Definition }
        }
    }
    // A quoted occurrence: shader source, a log message, a name table.
    if let Some(q) = t.find('"') {
        if t[q..].contains(name) && !t[..q].contains(name) { return Hit::StringLiteral }
    }
    if in_test_mod { return Hit::TestUse }
    Hit::Use
}

/// One classified match.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Match { pub line: usize, pub kind: Hit }

/// Classify every hit and order them so the definition comes first.
///
/// `test_regions` are line ranges inside `#[cfg(test)]`, which the caller knows from a cheap scan.
pub fn rank(lines: &[(usize, String)], name: &str, test_regions: &[(usize, usize)]) -> Vec<Match> {
    let mut v: Vec<Match> = lines.iter().map(|(n, l)| {
        let in_test = test_regions.iter().any(|(a, b)| n >= a && n <= b);
        Match { line: *n, kind: classify(l, name, in_test) }
    }).collect();
    // Stable by kind then line, so the definition leads and the remaining order still reflects the
    // file — a reader scanning uses expects them in the order they appear.
    v.sort_by_key(|m| (m.kind, m.line));
    v
}

/// The lines a `filewindow` should anchor on for "where is this defined".
///
/// **Returns every definition, not the first.** Two definitions of one name is the shape of the
/// duplicate-implementation bug this codebase has hit four times — `Peer`, `Beacon`, two Q4_K dequant
/// paths, two CPU quantizers — and returning only the first would hide exactly that.
pub fn definition_anchors(ranked: &[Match]) -> Vec<usize> {
    ranked.iter().filter(|m| m.kind == Hit::Definition).map(|m| m.line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_DEFINITION_outranks_a_use() {
        // The repeated failure: searching for MATVEC_IQ1_S returned a dispatch call and a table entry
        // before the const, and both looked like answers.
        assert_eq!(classify("pub const MATVEC_IQ1_S: &str = r#\"", "MATVEC_IQ1_S", false),
                   Hit::Definition);
        assert_eq!(classify("        self.dispatch(MATVEC_IQ1_S, &[&at], &out,", "MATVEC_IQ1_S", false),
                   Hit::Use);
    }

    #[test]
    fn a_STRING_LITERAL_is_not_a_use() {
        // The lookup-table entry that came back second in that same search. It names the symbol and
        // tells you nothing about it.
        assert_eq!(classify("        (\"fn wbyte\", \"MATVEC_IQ1_S\"),", "MATVEC_IQ1_S", false),
                   Hit::StringLiteral);
    }

    #[test]
    fn a_COMMENT_is_ranked_lowest_because_comments_outlive_their_code() {
        // This codebase has a comment claiming a binding fixed an FMA issue that it did not fix.
        assert_eq!(classify("    // see MATVEC_IQ1_S for the decode", "MATVEC_IQ1_S", false),
                   Hit::Comment);
        assert!(Hit::Comment > Hit::Use);
    }

    #[test]
    fn a_definition_INSIDE_a_test_module_is_still_a_definition() {
        // Pretending otherwise sends the reader hunting for one that does not exist.
        assert_eq!(classify("    fn helper() -> Scene {", "helper", true), Hit::Definition);
    }

    #[test]
    fn a_TEST_USE_ranks_below_a_production_use() {
        // A test naming a symbol is evidence it exists, not evidence about how production calls it.
        assert_eq!(classify("        assert!(check(&thing));", "check", true), Hit::TestUse);
        assert!(Hit::TestUse > Hit::Use);
    }

    #[test]
    fn EVERY_definition_is_returned_not_just_the_first() {
        // Two definitions of one name is the shape of the duplicate-implementation bug hit four times
        // here — Peer, Beacon, two Q4_K dequant paths, two CPU quantizers. Returning the first would
        // hide precisely that.
        let lines = vec![
            (100, "pub struct Peer {".to_string()),
            (200, "    let p = Peer { age_s: 0.0 };".to_string()),
            (300, "pub struct Peer {".to_string()),
        ];
        let r = rank(&lines, "Peer", &[]);
        assert_eq!(definition_anchors(&r), vec![100, 300], "both, or the duplicate stays hidden");
    }

    #[test]
    fn ranking_keeps_FILE_ORDER_within_a_kind() {
        // A reader scanning uses expects them in the order they appear.
        let lines = vec![
            (50, "    foo();".to_string()),
            (10, "fn foo() {}".to_string()),
            (30, "    foo();".to_string()),
        ];
        let r = rank(&lines, "foo", &[]);
        assert_eq!(r[0].line, 10);
        assert_eq!((r[1].line, r[2].line), (30, 50));
    }
}
