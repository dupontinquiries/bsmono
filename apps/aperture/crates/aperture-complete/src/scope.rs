//! scope — how much of the file a completion is allowed to see.
//!
//! A completion request is mostly a decision about CONTEXT: too little and
//! the model invents an API that does not exist; too much and every
//! keystroke ships kilobytes to a model that re-reads them from scratch.
//! Today aperture makes that decision once, globally, with a character
//! budget. This module makes it a choice with names.
//!
//! ## Why several scopes rather than one better one
//!
//! There is no single right answer, and the right answer changes between
//! keystrokes. Finishing an argument list needs the current line and the
//! signature above it. Writing a new arm needs the whole `match`. Calling
//! a helper defined 300 lines up needs the file. A tool that picks one is
//! wrong a predictable fraction of the time.
//!
//! So: compute SEVERAL, ask for all of them, show what comes back. The
//! reason that is affordable rather than N× the cost is
//! [`crate::prefixcache`] — the scopes share a suffix (they all end at the
//! caret) and differ only in how far back they start, so:
//!
//! * identical scopes collapse (a 12-line function IS its enclosing
//!   block — [`ScopePlan::distinct`] drops the duplicate rather than
//!   asking twice),
//! * every scope is cache-checked independently, so re-triggering after a
//!   caret move usually hits on all of them,
//! * and the misses go out as ONE batch through
//!   [`crate::prefixcache::PrefixCache::plan`], which is exactly the shape
//!   a KV-cached engine wants — several sequences sharing a tail, planned
//!   together.
//!
//! Ordering matters and is deliberate: [`Scope::ALL`] runs narrow →
//! wide. A batched engine that processes in order returns the cheap,
//! usually-sufficient answer first, so the UI can show something before
//! the whole-file answer lands.
//!
//! ## What this module is not
//!
//! It does not parse. Brace matching is lexical, comment- and
//! string-aware enough not to be fooled by the obvious cases, and honest
//! about the rest — a tree-sitter-backed scope would be better and is a
//! natural upgrade (aperture-core already has the grammar), but a
//! completion context that is occasionally a few lines too wide costs
//! nothing, while a parser dependency in this zero-dep crate costs the
//! property that makes it compile in a second.

/// How much context to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// The current line only. Fastest, and right for finishing an
    /// expression that does not depend on anything above it.
    Line,
    /// Back to the start of the enclosing brace block — the body you are
    /// inside, and nothing else.
    Block,
    /// Back to the start of the enclosing top-level item: the `fn`,
    /// `impl`, `struct`. Usually the sweet spot, which is why it is the
    /// default.
    Function,
    /// Everything before the caret, budget-capped. The most context a
    /// single request can carry.
    File,
}

impl Scope {
    /// Narrow → wide. The order a batch should be issued in, so the cheap
    /// answer can be shown before the expensive one arrives.
    pub const ALL: [Scope; 4] = [Scope::Line, Scope::Block, Scope::Function, Scope::File];

    pub fn label(self) -> &'static str {
        match self {
            Scope::Line => "line",
            Scope::Block => "block",
            Scope::Function => "function",
            Scope::File => "file",
        }
    }

    /// One-line description for the palette and the suggestion list.
    pub fn hint(self) -> &'static str {
        match self {
            Scope::Line => "the current line only — fastest",
            Scope::Block => "the enclosing { } body",
            Scope::Function => "the enclosing fn / impl / struct",
            Scope::File => "everything above the caret",
        }
    }
}

impl Default for Scope {
    /// Function scope, not file: it is right far more often than it is
    /// wrong, and it is the one that keeps a long file's completions fast
    /// without losing the signature you are writing against.
    fn default() -> Scope {
        Scope::Function
    }
}

/// Is this byte offset inside a line comment or a string? Used so a brace
/// in `"} not a brace"` or after `//` does not move the scope boundary.
/// Lexical and deliberately simple — see the module note.
fn mask_code(text: &str) -> Vec<bool> {
    let b = text.as_bytes();
    let mut code = vec![true; b.len()];
    let (mut in_str, mut in_char, mut in_line_comment, mut in_block) = (false, false, false, 0u32);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if in_line_comment {
            code[i] = false;
            if c == b'\n' {
                in_line_comment = false;
                code[i] = true;
            }
            i += 1;
            continue;
        }
        if in_block > 0 {
            code[i] = false;
            if c == b'*' && next == Some(b'/') {
                code[i + 1] = false;
                in_block -= 1;
                i += 2;
                continue;
            }
            if c == b'/' && next == Some(b'*') {
                in_block += 1;
                code[i + 1] = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_str || in_char {
            code[i] = false;
            if c == b'\\' {
                if i + 1 < b.len() {
                    code[i + 1] = false;
                }
                i += 2;
                continue;
            }
            if (in_str && c == b'"') || (in_char && c == b'\'') {
                in_str = false;
                in_char = false;
            }
            i += 1;
            continue;
        }
        match (c, next) {
            (b'/', Some(b'/')) => {
                in_line_comment = true;
                code[i] = false;
            }
            (b'/', Some(b'*')) => {
                in_block = 1;
                code[i] = false;
                code[i + 1] = false;
                i += 2;
                continue;
            }
            (b'"', _) => {
                in_str = true;
                code[i] = false;
            }
            (b'\'', _) => {
                in_char = true;
                code[i] = false;
            }
            _ => {}
        }
        i += 1;
    }
    code
}

/// Byte offset where `scope` begins, given everything before the caret.
///
/// Always returns a char boundary, and always <= `before.len()`, so the
/// result is safe to slice with. That is a guarantee rather than a
/// convention: this feeds a slice, and aperture has already been killed
/// once this session by a range that was merely "probably fine".
pub fn scope_start(before: &str, scope: Scope) -> usize {
    let start = match scope {
        Scope::File => 0,
        Scope::Line => before.rfind('\n').map(|i| i + 1).unwrap_or(0),
        Scope::Block => block_start(before),
        Scope::Function => item_start(before),
    };
    let mut s = start.min(before.len());
    while s > 0 && !before.is_char_boundary(s) {
        s -= 1;
    }
    s
}

/// Walk back to the unmatched `{` that opens the block the caret is in.
/// Falls back to the start of the text when there is no enclosing block —
/// at top level, "the block" IS the file so far.
fn block_start(before: &str) -> usize {
    let code = mask_code(before);
    let b = before.as_bytes();
    let mut depth = 0i32;
    let mut i = b.len();
    while i > 0 {
        i -= 1;
        if !code[i] {
            continue;
        }
        match b[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    // Start at the first statement AFTER the opening line.
                    // Including the `if a > 0 {` line would make block
                    // scope a strict superset of the body, blurring it
                    // into function scope — the two are supposed to be
                    // meaningfully different sizes or there is no point
                    // asking both. The condition itself is what function
                    // scope is for.
                    return before[i..]
                        .find('\n')
                        .map(|n| i + n + 1)
                        .unwrap_or(before.len());
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    0
}

/// Walk back to the start of the enclosing top-level item. Finds the
/// outermost unmatched `{`, then rewinds to the beginning of the line its
/// declaration starts on — so `fn foo(` and its doc comment above are
/// both included, which is most of why function scope beats block scope.
fn item_start(before: &str) -> usize {
    let code = mask_code(before);
    let b = before.as_bytes();
    let mut depth = 0i32;
    let mut outermost: Option<usize> = None;
    let mut i = b.len();
    while i > 0 {
        i -= 1;
        if !code[i] {
            continue;
        }
        match b[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    outermost = Some(i);
                } else {
                    depth -= 1;
                }
            }
            _ => {}
        }
    }
    let Some(brace) = outermost else { return 0 };
    // Rewind over the declaration line, then over any immediately
    // preceding doc-comment / attribute lines.
    let mut line_start = before[..brace].rfind('\n').map(|n| n + 1).unwrap_or(0);
    loop {
        if line_start == 0 {
            break;
        }
        let prev_end = line_start - 1;
        let prev_start = before[..prev_end].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let prev = before[prev_start..prev_end].trim_start();
        if prev.starts_with("///") || prev.starts_with("//!") || prev.starts_with("#[") || prev.starts_with("#!") {
            line_start = prev_start;
        } else {
            break;
        }
    }
    line_start
}

/// One scope's slice of the context, ready to become a request.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedPrefix {
    pub scope: Scope,
    pub prefix: String,
}

/// The set of scoped prefixes to ask about, narrow → wide, with
/// duplicates removed.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScopePlan {
    pub items: Vec<ScopedPrefix>,
}

impl ScopePlan {
    /// Build the distinct scopes for this caret position.
    ///
    /// Duplicates collapse: in a short function the block, function and
    /// file scopes are frequently the SAME text, and asking a model the
    /// identical question three times is the cost this whole design
    /// exists to avoid. The widest label wins for an identical prefix,
    /// since that is what the text actually is.
    pub fn distinct(before: &str, scopes: &[Scope]) -> ScopePlan {
        let mut items: Vec<ScopedPrefix> = Vec::new();
        for &s in scopes {
            let start = scope_start(before, s);
            let prefix = before[start..].to_string();
            match items.iter_mut().find(|i| i.prefix == prefix) {
                Some(existing) => {
                    if s > existing.scope {
                        existing.scope = s;
                    }
                }
                None => items.push(ScopedPrefix { scope: s, prefix }),
            }
        }
        ScopePlan { items }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
use std::fmt;

/// Doc line.
#[inline]
fn outer(a: u32) -> u32 {
    let s = \"} not a brace\";
    if a > 0 {
        let x = ";

    #[test]
    fn line_scope_is_the_current_line_only() {
        let p = &SRC[..scope_start(SRC, Scope::Line)];
        let line = &SRC[scope_start(SRC, Scope::Line)..];
        assert_eq!(line, "        let x = ");
        assert!(p.ends_with('\n'));
    }

    #[test]
    fn block_scope_stops_at_the_enclosing_body_not_the_outer_one() {
        let s = &SRC[scope_start(SRC, Scope::Block)..];
        assert!(s.starts_with("        let x = "), "the if-body, {s:?}");
        assert!(!s.contains("fn outer"), "the enclosing fn is a WIDER scope");
    }

    #[test]
    fn function_scope_includes_the_signature_and_its_doc_and_attribute() {
        // The reason function scope beats block scope: the model needs the
        // signature it is writing against, and the doc comment that says
        // what the thing is for.
        let s = &SRC[scope_start(SRC, Scope::Function)..];
        assert!(s.starts_with("/// Doc line."), "doc comment included: {s:?}");
        assert!(s.contains("#[inline]"), "and the attribute");
        assert!(s.contains("fn outer(a: u32) -> u32 {"));
        assert!(!s.contains("use std::fmt"), "but not the whole file");
    }

    #[test]
    fn a_brace_inside_a_string_does_not_move_the_boundary() {
        // `"} not a brace"` sits between the caret and the real `{`. A
        // naive scan would treat it as closing the if-body and walk too
        // far back.
        let s = &SRC[scope_start(SRC, Scope::Block)..];
        assert!(!s.contains("not a brace"), "the string's brace was counted: {s:?}");
    }

    #[test]
    fn a_brace_in_a_comment_does_not_move_the_boundary() {
        let src = "fn f() {\n    // closing } here\n    let x = ";
        let s = &src[scope_start(src, Scope::Block)..];
        assert!(s.starts_with("    // closing"), "{s:?}");
    }

    #[test]
    fn file_scope_is_everything_and_top_level_degrades_gracefully() {
        assert_eq!(scope_start(SRC, Scope::File), 0);
        // At top level there is no enclosing block; "the block" is
        // everything so far rather than an error or an empty context.
        let top = "use std::fmt;\nlet y = ";
        assert_eq!(scope_start(top, Scope::Block), 0);
        assert_eq!(scope_start(top, Scope::Function), 0);
    }

    #[test]
    fn identical_scopes_collapse_so_the_model_is_not_asked_twice() {
        // A short top-level snippet: block, function and file are all the
        // same text. Asking three times is precisely the waste this
        // design exists to avoid.
        let src = "let x = ";
        let plan = ScopePlan::distinct(src, &Scope::ALL);
        assert_eq!(plan.len(), 1, "one distinct question, not four: {plan:?}");
        assert_eq!(plan.items[0].scope, Scope::File, "labelled by the widest that matches");
    }

    #[test]
    fn a_nested_caret_yields_several_distinct_scopes_narrow_first() {
        let plan = ScopePlan::distinct(SRC, &Scope::ALL);
        assert!(plan.len() >= 3, "line/block/function differ here: {plan:?}");
        // Narrow first, so a batched engine returns the cheap answer
        // before the expensive one.
        for w in plan.items.windows(2) {
            assert!(w[0].prefix.len() <= w[1].prefix.len(), "narrow to wide: {plan:?}");
        }
        // The caret here sits on the FIRST line of the if-body, so line
        // and block are the same text and collapse — labelled Block, the
        // wider of the two. That collapse is the feature working, not a
        // miss: it is one fewer request.
        assert_eq!(plan.items[0].scope, Scope::Block);
        // Deeper in a body, line and block genuinely differ and both are
        // asked.
        let deeper = "fn f() {\n    if a {\n        let a = 1;\n        let x = ";
        let p2 = ScopePlan::distinct(deeper, &Scope::ALL);
        assert_eq!(p2.items[0].scope, Scope::Line, "{p2:?}");
        assert!(p2.len() >= 3, "line, block, function all distinct: {p2:?}");
    }

    #[test]
    fn every_scope_start_is_a_safe_slice_index() {
        // Multi-byte characters near a boundary must not produce an index
        // that panics when sliced — the failure mode that already killed
        // the editor once this session.
        let src = "fn f() {\n    let s = \"héllo wörld — em dash\";\n    let x = ";
        for sc in Scope::ALL {
            let i = scope_start(src, sc);
            assert!(src.is_char_boundary(i), "{sc:?} -> {i} is not a char boundary");
            let _ = &src[i..];
        }
    }

    #[test]
    fn an_empty_buffer_is_not_a_panic() {
        for sc in Scope::ALL {
            assert_eq!(scope_start("", sc), 0);
        }
        assert_eq!(ScopePlan::distinct("", &Scope::ALL).len(), 1);
    }
}
