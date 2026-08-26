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

// ── beyond text scopes: call-graph depth (designed, not built) ─────────────
//
// The request that led to this module also asked for something structural:
// jump-to-definition-and-pull-in-the-body, depth-limited (3/6/9 in the
// palette), so a completion inside `handle_request` can see not just its
// own text but the SIGNATURE and RETURN TYPE of every function it calls,
// recursively, out to a bound. This is a real and good idea and a
// genuinely different mechanism from everything in this file — worth
// writing down precisely rather than half-building, because it needs a
// piece this module does not have: a call graph, not a text scan.
//
// WHAT IT NEEDS. `aperture-core::ts` already builds an outline
// (`OutlineItem`, function spans) via tree-sitter. A call-graph pass is an
// extension of the same query: for each function body, find `Call` nodes
// and resolve their callee names against the outline's own symbol table
// (same-file first; a further version would want `workspace_symbols` for
// cross-file resolution, which already exists for Ctrl+T). Depth N then
// means: start at the enclosing function, add everything it calls, then
// everything THOSE call, N times, collecting each visited function's
// SIGNATURE AND RETURN TYPE (not its body — a completion needs to know
// what `parse_gguf` returns, not how it is implemented, and that
// difference is most of the token budget). No repeats: a call graph in
// real code is not a tree, it has cycles and diamonds, and visiting a
// node twice wastes budget for nothing new.
//
// THE PALETTE SHAPE: three fixed depths (3, 6, 9) rather than an arbitrary
// dial, matching how `Scope` below offers named choices instead of a
// slider — a person picks "enough" or "more" faster than they pick a
// number, and three concrete choices are cache-friendly the same way
// `Scope::ALL` is: depth 3's collected set is very often a strict subset
// of depth 6's, which is again the collapse `ScopePlan::distinct` already
// exploits for text scopes.
//
// THE HARDER, LATER IDEA, recorded because it is the right shape for when
// this exists: a token-BUDGETED walk with no depth cap and no repeats,
// rather than a depth cap with no budget. Depth alone treats a
// three-line shell-out (`fn run(cmd: &str) -> io::Result<Output>`) the
// same as a three-hundred-line state machine that happens to be three
// calls away — both cost "one hop," but they do not cost the same
// TOKENS, and a fixed depth either starves a well-behaved recursive
// function (which revisits the same small set of neighbours over and
// over — no-repeats already handles that part) or bloats the budget on
// one large function that happened to be shallow. A walk that stops on a
// token count instead of a hop count, visiting the graph breadth-first
// so nearer calls are preferred when the budget runs out, is the fix —
// and it composes with everything above unchanged, since it is still
// "which functions' signatures to collect," just governed by a
// different stopping rule.
//
// WHY NOT NOW. This needs the call-graph extraction to exist first (real,
// separate work in aperture-core, since it needs the grammar), and it
// deserves its own tests against real recursive and mutually-recursive
// code before it is trusted. Text scoping was buildable and testable in
// one pass because it never needs more than the buffer already in
// memory; call-graph scoping needs a symbol table and is worth doing
// right rather than fast.

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
/// `line_count` is how many lines `Scope::Line` includes — a single line
/// is often too little (the request in flight named this directly: a
/// one-liner rarely carries the loop or the `if` guard the completion
/// actually depends on), and how many is a matter of taste rather than
/// correctness, so it is a parameter rather than a constant. Ignored by
/// every other scope.
///
/// Always returns a char boundary, and always <= `before.len()`, so the
/// result is safe to slice with. That is a guarantee rather than a
/// convention: this feeds a slice, and aperture has already been killed
/// once this session by a range that was merely "probably fine".
pub fn scope_start(before: &str, scope: Scope, line_count: usize) -> usize {
    let start = match scope {
        Scope::File => 0,
        Scope::Line => {
            let mut i = before.len();
            for _ in 0..line_count.max(1) {
                i = before[..i].rfind('\n').unwrap_or(0);
                if i == 0 {
                    break;
                }
            }
            if i == 0 { 0 } else { i + 1 }
        }
        Scope::Block => block_start(before),
        Scope::Function => item_start(before),
    };
    let mut s = start.min(before.len());
    while s > 0 && !before.is_char_boundary(s) {
        s -= 1;
    }
    s
}

/// The default line count for `Scope::Line` — three, not one. A single
/// line misses the `if` guard or the loop header sitting right above the
/// caret, which is usually the whole reason the completion needs any
/// scope narrower than the block at all.
pub const DEFAULT_LINE_COUNT: usize = 3;

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
        // A PLAIN `//` COMMENT RIGHT ABOVE THE SIGNATURE WAS BEING MISSED.
        // Real code explains a function with an ordinary `//` line at
        // least as often as with `///` — not every explanatory comment
        // is doc-comment-styled, and the reader does not care which
        // slash-count it used. Excluding it meant function scope handed
        // the model everything the signature promises except the one
        // line most likely to say what the function is actually FOR.
        if prev.starts_with("//") || prev.starts_with("#[") || prev.starts_with("#!") {
            line_start = prev_start;
        } else {
            break;
        }
    }
    line_start
}

// ── quick signals ────────────────────────────────────────────────────────
//
// Cheap, text-only facts about where the caret sits — not a scope (they do
// not decide how much text to send), but the kind of thing worth telling
// the model ALONGSIDE whatever scope was chosen, because they cost a
// regex-free scan rather than a parse and they are exactly the things a
// person glances at before writing the next line: what function am I in,
// how deep in loops, how big is this file, what does it import.

/// The name of the function enclosing the caret, if any. Reads it off the
/// same signature `item_start` already finds — this does not re-scan, it
/// just extracts the identifier between `fn` and `(` from that line.
pub fn enclosing_function_name(before: &str) -> Option<String> {
    let start = item_start(before);
    let sig_region = &before[start..];
    let fn_at = sig_region.find("fn ")?;
    let after_fn = &sig_region[fn_at + 3..];
    let name: String = after_fn
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// How many `for`/`while`/`loop` bodies enclose the caret. A cheap proxy
/// for "is this hot code" and "how many break/continue targets are live" —
/// both change what a reasonable completion looks like (an early return
/// inside three nested loops reads differently than one at top level).
///
/// Lexical, like the rest of this module: walks outward brace by brace,
/// and for each enclosing `{` checks whether the text immediately before
/// it names a loop. Comments and strings are masked first so a loop
/// keyword mentioned in a doc comment does not count.
pub fn loop_depth(before: &str) -> usize {
    let code = mask_code(before);
    let b = before.as_bytes();
    let mut depth = 0i32;
    let mut loops = 0usize;
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
                    let head = before[..i].trim_end();
                    if ends_with_loop_keyword(head) {
                        loops += 1;
                    }
                } else {
                    depth -= 1;
                }
            }
            _ => {}
        }
    }
    loops
}

fn ends_with_loop_keyword(head: &str) -> bool {
    // The brace of `for x in y {` / `while c {` / `loop {` is preceded by
    // more than the bare keyword (the binding, the condition), so this
    // checks the line's own leading word rather than the character right
    // before the brace.
    let line = head.rsplit('\n').next().unwrap_or(head).trim_start();
    line.starts_with("for ") || line.starts_with("while ") || line.starts_with("loop") && {
        let rest = line[4..].trim_start();
        rest.is_empty() || rest.starts_with('{')
    }
}

/// Root crate names from every `use` line in the file, deduplicated and
/// sorted, `crate`/`self`/`super` excluded — those name this file's own
/// tree, not a library. This is "what does this module depend on", read
/// straight off the source with no parser: a `use` statement's root
/// segment is a crate name in every edition this codebase targets.
pub fn crates_used(file_text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in file_text.lines() {
        let line = raw.trim_start();
        let Some(rest) = line.strip_prefix("use ") else { continue };
        let rest = rest.trim_start_matches("pub(crate) ").trim_start_matches("pub ");
        let root: String = rest
            .trim_start_matches("::")
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if root.is_empty() || matches!(root.as_str(), "crate" | "self" | "super") {
            continue;
        }
        if !out.contains(&root) {
            out.push(root);
        }
    }
    out.sort();
    out
}

/// The quick facts, gathered and formatted as one `//`-commented line so
/// it can be prepended to any Rust prefix without looking like anything
/// other than a comment a human might have left there.
///
/// `file_loc` and `crates_used` need the WHOLE file, which the scoped
/// prefix by definition may not contain (that is the point of scoping) —
/// so they are computed from `full_text` and passed in as facts, not
/// derived from the scope itself. `enclosing_function_name` and
/// `loop_depth` DO come from the scope's own text, since both are true of
/// "everything before the caret" regardless of how much of it was kept.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuickContext {
    pub function_name: Option<String>,
    pub loop_depth: usize,
    pub file_loc: usize,
    pub crates_used: Vec<String>,
}

impl QuickContext {
    pub fn gather(before: &str, full_text: &str) -> QuickContext {
        QuickContext {
            function_name: enclosing_function_name(before),
            loop_depth: loop_depth(before),
            file_loc: full_text.lines().count(),
            crates_used: crates_used(full_text),
        }
    }

    /// Empty facts render as nothing — a comment line that says nothing
    /// (no function, at top level, no imports) is noise, not context.
    pub fn is_empty(&self) -> bool {
        self.function_name.is_none() && self.loop_depth == 0 && self.crates_used.is_empty()
    }

    /// One `// ` line, or empty when there is nothing worth saying.
    pub fn render(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        if let Some(f) = &self.function_name {
            parts.push(format!("in `{f}`"));
        }
        if self.loop_depth > 0 {
            parts.push(format!("loop-depth {}", self.loop_depth));
        }
        parts.push(format!("{} LOC", self.file_loc));
        if !self.crates_used.is_empty() {
            parts.push(format!("uses: {}", self.crates_used.join(", ")));
        }
        format!("// context: {}\n", parts.join(", "))
    }
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
    pub fn distinct(before: &str, scopes: &[Scope], line_count: usize) -> ScopePlan {
        let mut items: Vec<ScopedPrefix> = Vec::new();
        for &s in scopes {
            let start = scope_start(before, s, line_count);
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
        let p = &SRC[..scope_start(SRC, Scope::Line, 1)];
        let line = &SRC[scope_start(SRC, Scope::Line, 1)..];
        assert_eq!(line, "        let x = ");
        assert!(p.ends_with('\n'));
    }

    #[test]
    fn block_scope_stops_at_the_enclosing_body_not_the_outer_one() {
        let s = &SRC[scope_start(SRC, Scope::Block, DEFAULT_LINE_COUNT)..];
        assert!(s.starts_with("        let x = "), "the if-body, {s:?}");
        assert!(!s.contains("fn outer"), "the enclosing fn is a WIDER scope");
    }

    #[test]
    fn function_scope_includes_a_plain_slash_slash_comment_too() {
        // The bug: only `///`/`//!` rewound past the signature line. A
        // plain `//` explanatory comment right above a function is at
        // least as common in real code and is often the ONE line that
        // says what the function is for — dropping it left function
        // scope carrying the signature but not the reason for it.
        let src = "// Parse every Rust file and collect their outlines.\nfn index_files(root: &Path) -> Vec<Symbol> {\n    let x = ";
        let s = &src[scope_start(src, Scope::Function, DEFAULT_LINE_COUNT)..];
        assert!(s.starts_with("// Parse every"), "{s:?}");
    }

    #[test]
    fn line_scope_defaults_to_three_lines_not_one() {
        // A one-line scope routinely misses the `if` guard or loop header
        // sitting right above the caret — usually the whole reason a
        // narrower-than-block scope is worth having at all.
        let src = "fn f() {\n    if ready {\n        for x in xs {\n            call(";
        let s = &src[scope_start(src, Scope::Line, DEFAULT_LINE_COUNT)..];
        assert!(s.contains("if ready"), "{s:?}");
        assert!(s.contains("for x in xs"), "{s:?}");
        // But not the function signature two lines further back — three
        // means three, not "however far it takes to find something useful".
        assert!(!s.contains("fn f()"), "{s:?}");
    }

    #[test]
    fn function_scope_includes_the_signature_and_its_doc_and_attribute() {
        // The reason function scope beats block scope: the model needs the
        // signature it is writing against, and the doc comment that says
        // what the thing is for.
        let s = &SRC[scope_start(SRC, Scope::Function, DEFAULT_LINE_COUNT)..];
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
        let s = &SRC[scope_start(SRC, Scope::Block, DEFAULT_LINE_COUNT)..];
        assert!(!s.contains("not a brace"), "the string's brace was counted: {s:?}");
    }

    #[test]
    fn a_brace_in_a_comment_does_not_move_the_boundary() {
        let src = "fn f() {\n    // closing } here\n    let x = ";
        let s = &src[scope_start(src, Scope::Block, DEFAULT_LINE_COUNT)..];
        assert!(s.starts_with("    // closing"), "{s:?}");
    }

    #[test]
    fn file_scope_is_everything_and_top_level_degrades_gracefully() {
        assert_eq!(scope_start(SRC, Scope::File, DEFAULT_LINE_COUNT), 0);
        // At top level there is no enclosing block; "the block" is
        // everything so far rather than an error or an empty context.
        let top = "use std::fmt;\nlet y = ";
        assert_eq!(scope_start(top, Scope::Block, DEFAULT_LINE_COUNT), 0);
        assert_eq!(scope_start(top, Scope::Function, DEFAULT_LINE_COUNT), 0);
    }

    #[test]
    fn identical_scopes_collapse_so_the_model_is_not_asked_twice() {
        // A short top-level snippet: block, function and file are all the
        // same text. Asking three times is precisely the waste this
        // design exists to avoid.
        let src = "let x = ";
        let plan = ScopePlan::distinct(src, &Scope::ALL, DEFAULT_LINE_COUNT);
        assert_eq!(plan.len(), 1, "one distinct question, not four: {plan:?}");
        assert_eq!(plan.items[0].scope, Scope::File, "labelled by the widest that matches");
    }

    #[test]
    fn a_nested_caret_yields_several_distinct_scopes_narrow_first() {
        let plan = ScopePlan::distinct(SRC, &Scope::ALL, DEFAULT_LINE_COUNT);
        assert!(plan.len() >= 3, "line/block/function differ here: {plan:?}");
        // Function and File are always narrow-to-wide relative to each
        // other, since File is a strict superset of Function by
        // construction (both walk back from the caret; File just keeps
        // going). Line and Block do NOT have that guarantee once Line is
        // multi-line — a 3-line window can reach further back than a
        // shallow block, which is real and fine, not a bug the ordering
        // needs to hide.
        let function_i = plan.items.iter().position(|i| i.scope == Scope::Function).unwrap();
        let file_i = plan.items.iter().position(|i| i.scope == Scope::File).unwrap();
        assert!(function_i < file_i, "Function must come before File: {plan:?}");

        // A one-line-deep body: Line's 3-line window reaches OUT of the
        // if-body into the signature, past what Block (the if-body only)
        // covers. Line is not "the smallest scope" any more once it is
        // configurable — it is "recent lines", which can outgrow a
        // shallow block. Both are still independently useful and both
        // are still asked; the plan just does not force one to be a
        // subset of the other.
        assert!(
            plan.items[0].prefix.len() >= plan.items[1].prefix.len(),
            "Line's 3-line window reaching past a 1-line block: {plan:?}"
        );

        // Deep enough that a 3-line window stays INSIDE the block (five
        // statement lines between the brace and the caret), the natural
        // narrow-to-wide order holds — this is the common case; the
        // shallow one above is the edge case worth naming, not the norm.
        let deeper = "fn f() {\n    if a {\n        let a = 1;\n        let b = 2;\n        let c = 3;\n        let d = 4;\n        let x = ";
        let p2 = ScopePlan::distinct(deeper, &Scope::ALL, DEFAULT_LINE_COUNT);
        assert_eq!(p2.items[0].scope, Scope::Line, "{p2:?}");
        assert!(p2.len() >= 3, "line, block, function all distinct: {p2:?}");
        for w in p2.items.windows(2) {
            assert!(w[0].prefix.len() <= w[1].prefix.len(), "{p2:?}");
        }
    }

    #[test]
    fn every_scope_start_is_a_safe_slice_index() {
        // Multi-byte characters near a boundary must not produce an index
        // that panics when sliced — the failure mode that already killed
        // the editor once this session.
        let src = "fn f() {\n    let s = \"héllo wörld — em dash\";\n    let x = ";
        for sc in Scope::ALL {
            let i = scope_start(src, sc, DEFAULT_LINE_COUNT);
            assert!(src.is_char_boundary(i), "{sc:?} -> {i} is not a char boundary");
            let _ = &src[i..];
        }
    }

    #[test]
    fn enclosing_function_name_reads_the_identifier_not_the_whole_signature() {
        let src = "fn index_files(root: &Path, files: &[PathBuf]) -> Vec<Symbol> {\n    let x = ";
        assert_eq!(enclosing_function_name(src).as_deref(), Some("index_files"));
        assert_eq!(enclosing_function_name("let x = "), None, "top level: no enclosing fn");
    }

    #[test]
    fn loop_depth_counts_enclosing_loops_and_ignores_the_word_in_comments() {
        assert_eq!(loop_depth("let x = "), 0);
        assert_eq!(loop_depth("for x in xs {\n    let y = "), 1);
        assert_eq!(loop_depth("for x in xs {\n    while c {\n        let y = "), 2);
        // A sibling loop that already CLOSED must not count.
        assert_eq!(loop_depth("for x in xs {}\nfor y in ys {\n    let z = "), 1);
        // The word "loop" in a comment or a string is not a loop.
        assert_eq!(loop_depth("// loop over items\nfn f() {\n    let x = "), 0);
        assert_eq!(loop_depth("let s = \"a for loop\";\nlet x = "), 0);
        // `loop {}` itself, with no condition.
        assert_eq!(loop_depth("loop {\n    let x = "), 1);
    }

    #[test]
    fn crates_used_reads_use_roots_and_drops_this_files_own_tree() {
        let src = "use std::fmt;\nuse aperture_core::Buffer;\nuse crate::state::AppState;\nuse super::helpers;\nuse std::io::Read;\n";
        let mut got = crates_used(src);
        got.sort();
        assert_eq!(got, vec!["aperture_core".to_string(), "std".to_string()],
                   "crate/super excluded, std deduplicated");
    }

    #[test]
    fn quick_context_renders_nothing_when_there_is_nothing_to_say() {
        // Top level, no loop, no imports: a context line here would be
        // noise, not information.
        let ctx = QuickContext { file_loc: 40, ..Default::default() };
        assert!(ctx.is_empty());
        assert_eq!(ctx.render(), "");
    }

    #[test]
    fn quick_context_renders_one_comment_line_with_what_is_known() {
        let before = "for x in xs {\n    fn helper() {\n        let v = ";
        let full = "use std::fmt;\nuse aperture_core::Buffer;\n\nfor x in xs {\n    fn helper() {\n        let v = 1;\n    }\n}\n";
        let ctx = QuickContext::gather(before, full);
        let line = ctx.render();
        assert!(line.starts_with("// context: "), "{line:?}");
        assert!(line.contains("in `helper`"), "{line:?}");
        assert!(line.contains("loop-depth 1"), "{line:?}");
        assert!(line.contains("LOC"), "{line:?}");
        assert!(line.contains("aperture_core"), "{line:?}");
        assert!(line.ends_with('\n'), "a whole extra line, not a fragment appended to the prefix");
    }

    #[test]
    fn an_empty_buffer_is_not_a_panic() {
        for sc in Scope::ALL {
            assert_eq!(scope_start("", sc, DEFAULT_LINE_COUNT), 0);
        }
        assert_eq!(ScopePlan::distinct("", &Scope::ALL, DEFAULT_LINE_COUNT).len(), 1);
    }
}
