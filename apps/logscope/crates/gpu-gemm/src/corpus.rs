//! corpus.rs — the cheap mid-generation kill check, from symbols the repository already has.
//!
//! `reap` needs signals available BEFORE a compile, because a branch that gets to compile has
//! already spent its slot. Its yield table says the difference between reaping at 80% of budget and
//! at 20% is the difference between 11% and 3.57x, and the whole case rests on `at_fraction`, which
//! had nothing behind it. This is what puts something behind it: a call to a name that exists
//! nowhere in the repository and nowhere in the branch's own output is decidable at the token it is
//! written, not at the compile it would have failed.
//!
//! ## Why absence of a name is not, on its own, evidence
//!
//! The obvious version of this check condemns every branch immediately, because every branch is
//! writing code that does not exist yet. A generated `fn reap_slot(...)` calling a generated
//! `fn slot_pages(...)` uses two names the corpus has never seen and both are fine.
//!
//! So the scanner tracks what the branch DEFINES as it goes and resolves against corpus ∪ branch.
//! Getting that tracker wrong does not degrade the check, it inverts it — a missed definition kills
//! a good branch. `examples/corpus_falsekill.rs` runs the whole thing over the monorepo's own source
//! leave-one-file-out and counts how many known-good files it would have condemned. That number is
//! the only thing that says whether this is usable.
//!
//! ## Frequency is a prior, never a gate
//!
//! `familiarity` scores how conventional a branch's vocabulary is against corpus usage counts. It
//! is a tie-breaker among branches that already passed the compile gate and it must not be anything
//! more, for two reasons. The most-called API in a codebase is often the most-deprecated one — this
//! repository's own history has a revert six commits back for porting something that already
//! existed. And unlike a compiler, a conformity score is trivially gamed by copying existing code,
//! so a search that optimises it will never produce the abstraction the repository needs and does
//! not yet have.
//!
//! ## What this cannot see
//!
//! Only hallucinated names. A branch with correct names, wrong types, wrong lifetimes, wrong logic
//! or wrong architecture passes cleanly. How much of `at_fraction` this actually moves depends on
//! what share of doomed branches die by inventing an API, and that share is unmeasured.
//!
//! Pure: no wgpu, no model, no compiler. Lexical, not a parser — a parse would be the wrong cost for
//! something that runs every few tokens.

use std::collections::{HashMap, HashSet};

/// Identifiers the corpus contains, with how often each appears.
#[derive(Clone, Debug, Default)]
pub struct SymbolIndex {
    counts: HashMap<String, u32>,
    total: u64,
}

impl SymbolIndex {
    pub fn new() -> Self { Self::default() }

    /// Fold one source file in. Deliberately indiscriminate: every identifier anywhere, including
    /// locals and comments' worth of prose that happens to lex as identifiers. An over-inclusive
    /// corpus produces false NEGATIVES (a hallucinated name that happens to appear somewhere is let
    /// through), and this check should err that way — a missed kill costs a slot, a false kill costs
    /// a hypothesis.
    pub fn add_source(&mut self, src: &str) {
        for id in identifiers(&strip_noncode(src)) {
            *self.counts.entry(id).or_insert(0) += 1;
            self.total += 1;
        }
    }

    pub fn contains(&self, name: &str) -> bool { self.counts.contains_key(name) }
    pub fn count(&self, name: &str) -> u32 { self.counts.get(name).copied().unwrap_or(0) }
    pub fn distinct(&self) -> usize { self.counts.len() }
    pub fn total(&self) -> u64 { self.total }
}

/// Why a branch was condemned.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unresolved {
    /// A call to a name that is in neither the corpus nor the branch's own definitions.
    Call(String),
}

/// Incremental scan over a branch's growing output.
///
/// `feed` is called with the whole buffer so far and only examines what is new. It holds back the
/// final token of every feed, because a buffer ending mid-identifier (`Vec<u3`, `self.slot_pag`)
/// would otherwise be condemned for a name the branch has not finished writing. That hold-back is
/// the difference between a detector and a branch shredder.
pub struct Scanner {
    cursor: usize,
    defined: HashSet<String>,
    unresolved: Vec<Unresolved>,
    /// Names seen in call position that were not resolvable YET. Re-checked on each feed, because
    /// Rust lets a function be called above its definition.
    pending: Vec<String>,
}

impl Scanner {
    pub fn new() -> Self {
        Scanner { cursor: 0, defined: HashSet::new(), unresolved: Vec::new(), pending: Vec::new() }
    }

    pub fn defined(&self) -> usize { self.defined.len() }
    pub fn pending(&self) -> usize { self.pending.len() }

    /// Everything condemned so far. Empty is the common case and the one that must stay common.
    pub fn unresolved(&self) -> &[Unresolved] { &self.unresolved }

    /// Scan the new suffix of `buf`. Returns how many call sites are now awaiting resolution.
    ///
    /// `finished` only controls the hold-back: while a branch is still generating, the final token
    /// may be a half-written name. It does NOT condemn — `settle` is the only path that does.
    ///
    /// The first version condemned here too, using branch definitions alone and consulting neither
    /// the corpus nor the builtin list, so a call to a perfectly ordinary repository function was
    /// condemned by `feed` and then found nothing left to check by `settle`. Two paths draining the
    /// same queue under different rules is the shape of that bug; there is one path now.
    pub fn feed(&mut self, buf: &str, finished: bool) -> usize {
        let end = if finished { buf.len() } else { hold_back(buf) };
        if end > self.cursor {
            let chunk = &buf[self.cursor..end];
            for d in definitions(chunk) { self.defined.insert(d); }
            for c in calls(chunk) { self.pending.push(c); }
            self.cursor = end;
        }
        self.pending.len()
    }

    /// Call sites that cannot currently be resolved, without condemning them.
    ///
    /// This is the mid-generation signal: a rising count is a branch inventing vocabulary, and it is
    /// readable at the token it happens rather than at the compile it would have failed. It is a
    /// suspicion and not a verdict, because Rust resolves forward references and the definition may
    /// be three lines away.
    pub fn suspects(&self, idx: &SymbolIndex) -> usize {
        self.pending.iter()
            .filter(|n| !self.defined.contains(*n) && !idx.contains(n) && !is_builtin(n))
            .count()
    }

    /// Resolve pending names against a corpus and condemn what neither side has. Call once, when
    /// the branch has stopped generating — before that, an unknown call is a forward reference.
    pub fn settle(&mut self, idx: &SymbolIndex) -> bool {
        let before = self.unresolved.len();
        let (defined, pending) = (&self.defined, std::mem::take(&mut self.pending));
        for name in pending {
            if !defined.contains(&name) && !idx.contains(&name) && !is_builtin(&name) {
                self.unresolved.push(Unresolved::Call(name));
            }
        }
        self.unresolved.len() > before
    }

    /// Mean log-frequency of the identifiers this branch used, in [0, 1] after squashing.
    ///
    /// A tie-breaker. See the module note on why it is not a fitness function.
    pub fn familiarity(&self, buf: &str, idx: &SymbolIndex) -> f32 {
        let ids: Vec<String> = identifiers(buf).into_iter()
            .filter(|i| !is_builtin(i) && !self.defined.contains(i)).collect();
        if ids.is_empty() { return 0.5; }
        let acc: f64 = ids.iter().map(|i| ((idx.count(i) + 1) as f64).ln()).sum();
        let mean = acc / ids.len() as f64;
        // ln(1) = 0 for a name the corpus has never seen; ln(1000) ~ 6.9 for a very common one.
        (mean / 7.0).min(1.0) as f32
    }
}

impl Default for Scanner { fn default() -> Self { Self::new() } }

/// Index of the last byte safe to scan: everything up to the start of the trailing identifier.
fn hold_back(buf: &str) -> usize {
    let b = buf.as_bytes();
    let mut i = b.len();
    while i > 0 && is_ident_byte(b[i - 1]) { i -= 1; }
    i
}

fn is_ident_byte(c: u8) -> bool { c.is_ascii_alphanumeric() || c == b'_' }

/// Blank out comments and string literals, preserving byte offsets.
///
/// Without this the detector reads prose. Measured on 120 real files it condemned `illuminated`,
/// `isosceles`, `hallucinations`, `springing` and `backbone` — all ordinary English words that
/// happened to be followed by an opening parenthesis inside a doc comment. Offsets are preserved
/// rather than removed so `calls` can still look at the byte before an identifier.
pub fn strip_noncode(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = vec![b' '; b.len()];
    let (mut i, mut n) = (0usize, b.len());
    while i < n {
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' { i += 1; }
        } else if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let mut depth = 1; i += 2;
            while i < n && depth > 0 {
                if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' { depth += 1; i += 2; }
                else if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' { depth -= 1; i += 2; }
                else { i += 1; }
            }
        } else if b[i] == b'"' {
            i += 1;
            while i < n { if b[i] == b'\\' { i += 2; } else if b[i] == b'"' { i += 1; break } else { i += 1 } }
        } else if b[i] == b'\'' && i + 2 < n && b[i + 2] == b'\'' {
            i += 3; // char literal; lifetimes have no closing quote and fall through
        } else {
            out[i] = b[i]; i += 1;
        }
        let _ = &mut n;
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

/// Every identifier in `src`, in order, with duplicates.
pub fn identifiers(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0usize);
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let s = i;
            while i < b.len() && is_ident_byte(b[i]) { i += 1; }
            out.push(src[s..i].to_string());
        } else { i += 1; }
    }
    out
}

/// Keywords that introduce a binding whose name is the next identifier.
const INTRODUCERS: &[&str] = &[
    "fn", "struct", "enum", "trait", "type", "const", "static", "mod", "union", "macro_rules",
];

/// Names this chunk defines. Over-inclusive on purpose — a name wrongly counted as defined causes
/// a missed kill, a name wrongly omitted causes a false kill, and only one of those is expensive.
pub fn definitions(src: &str) -> Vec<String> {
    let src = &strip_noncode(src);
    let ids = identifiers(src);
    let mut out = Vec::new();
    for w in ids.windows(2) {
        if INTRODUCERS.contains(&w[0].as_str()) { out.push(w[1].clone()); }
        // `let x`, `let mut x`, and `for x in` all bind; so does `use path::As as Name`.
        if w[0] == "let" || w[0] == "for" || w[0] == "as" { out.push(w[1].clone()); }
    }
    for w in ids.windows(3) {
        if w[0] == "let" && w[1] == "mut" { out.push(w[2].clone()); }
    }
    // A parameter binds its name: `fn take(pred: impl Fn(u8) -> bool)` then `pred(x)`. Every
    // remaining false kill in the 120-file run was one of these — `pred`, `keyfn`, `is_srgb`.
    // The rule used is "identifier immediately followed by `:`", which also catches struct-literal
    // fields and match guards. Over-inclusive, which is the safe direction here.
    let b = src.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let st = i;
            while i < b.len() && is_ident_byte(b[i]) { i += 1; }
            let mut j = i;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') { j += 1; }
            // `:` but not `::`, which is a path rather than a binding.
            if j < b.len() && b[j] == b':' && !(j + 1 < b.len() && b[j + 1] == b':') {
                out.push(src[st..i].to_string());
            }
        } else { i += 1; }
    }

    // Anything imported is resolvable by definition; take every segment of a `use` line.
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("use ") || t.starts_with("pub use ") {
            out.extend(identifiers(t));
        }
    }
    out
}

/// Identifiers appearing in FREE call position: followed by `(`, not preceded by `.`, not a
/// definition's own name, and not a keyword taking a parenthesised clause.
///
/// The exclusions are not tidying, they are the difference between a detector and a shredder.
/// Measured on 120 real files from this monorepo, leave-one-file-out, condemning every name in
/// lexical call position flagged 70.8% of known-good files. The condemned names were overwhelmingly
/// `as_mut_ptr`, `is_null`, `bind_texture`, `get_uniform_location` — METHOD calls, whose resolution
/// needs the receiver's type and therefore a compiler. Nothing lexical can check them, so nothing
/// lexical should judge them.
///
/// Capitalised names are skipped for the same reason one step removed: `Tilt(x)` and `vec4(..)` are
/// tuple-variant and constructor calls, resolvable only through the type namespace and through
/// dependencies whose source the corpus does not contain.
///
/// What survives is free-function and path calls, which is where a hallucinated API most often
/// shows up anyway.
pub fn calls(src: &str) -> Vec<String> {
    const CLAUSE: &[&str] = &["if", "while", "for", "match", "return", "fn"];
    // A DEFINITION puts its own name in lexical call position: `fn f()`, `struct Pair(u8, u8)`.
    // Counting those as calls made every function look like a call to itself, which is both a
    // guaranteed false positive on well-formed code and the reason the held-back-token test
    // reported a call in `fn f() { let x = `.
    const DEFINES_NEXT: &[&str] = &["fn", "struct", "enum", "union", "macro_rules"];
    let src = &strip_noncode(src);
    let b = src.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0usize);
    let mut prev = String::new();
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let s = i;
            while i < b.len() && is_ident_byte(b[i]) { i += 1; }
            let name = &src[s..i];
            let was_defined = DEFINES_NEXT.contains(&prev.as_str());
            let is_method = s > 0 && b[s - 1] == b'.';
            // `slice::from_raw_parts_mut(..)` — the qualifier names the namespace, which the corpus
            // has no view of when it is an external crate or std. Judging the final segment
            // condemned std and glam calls across a third of the files.
            let is_qualified = s > 1 && b[s - 1] == b':' && b[s - 2] == b':';
            let is_ctor = name.starts_with(|c: char| c.is_ascii_uppercase());
            prev = name.to_string();
            if was_defined || is_method || is_ctor || is_qualified { continue; }
            let mut j = i;
            while j < b.len() && (b[j] == b' ' || b[j] == b'\t') { j += 1; }
            // A generic call `foo::<T>(x)` still counts.
            if j + 1 < b.len() && b[j] == b':' && b[j + 1] == b':' {
                let mut k = j + 2;
                while k < b.len() && b[k] != b'(' && b[k] != b';' && b[k] != b'\n' { k += 1; }
                let _ = k; // a path call's qualifier is not the thing being called
                continue;
            }
            if j < b.len() && b[j] == b'(' && !CLAUSE.contains(&name) {
                out.push(name.to_string());
            }
        } else {
            // Only whitespace may sit between an introducer and the name it introduces.
            if !b[i].is_ascii_whitespace() { prev.clear(); }
            i += 1;
        }
    }
    out
}

/// Language items and prelude names no corpus has to contain.
fn is_builtin(name: &str) -> bool {
    const B: &[&str] = &[
        "fn", "let", "mut", "if", "else", "while", "for", "in", "match", "return", "struct", "enum",
        "impl", "trait", "pub", "use", "mod", "const", "static", "self", "Self", "super", "crate",
        "where", "as", "ref", "move", "dyn", "type", "unsafe", "async", "await", "loop", "break",
        "continue", "true", "false",
        "Some", "None", "Ok", "Err", "Option", "Result", "Vec", "String", "Box", "Rc", "Arc", "Cell",
        "RefCell", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque", "Iterator", "Into",
        "From", "Default", "Clone", "Copy", "Debug", "Display", "Drop", "Sized", "Send", "Sync",
        "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64",
        "bool", "char", "str",
        "println", "eprintln", "format", "vec", "panic", "assert", "assert_eq", "assert_ne",
        "write", "writeln", "matches", "todo", "unimplemented", "unreachable", "dbg",
    ];
    B.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx_of(src: &str) -> SymbolIndex { let mut i = SymbolIndex::new(); i.add_source(src); i }

    #[test]
    fn a_branch_calling_only_what_it_defines_is_never_condemned() {
        let src = "fn outer() { inner(); }\nfn inner() { }\n";
        let mut s = Scanner::new();
        s.feed(src, true);
        s.settle(&SymbolIndex::new());
        assert!(s.unresolved().is_empty(), "{:?}", s.unresolved());
    }

    #[test]
    fn forward_references_resolve_because_rust_allows_them() {
        // `inner` is called before it is defined. A scanner that condemned on first sight would kill
        // most well-formed Rust.
        let src = "fn outer() { inner(); }\nfn inner() {}";
        let mut s = Scanner::new();
        // Feed in two chunks so the call is seen strictly before the definition.
        s.feed("fn outer() { inner(); }\n", false);
        s.feed(src, true);
        s.settle(&SymbolIndex::new());
        assert!(s.unresolved().is_empty(), "{:?}", s.unresolved());
    }

    #[test]
    fn a_call_to_something_nobody_has_is_condemned() {
        let mut s = Scanner::new();
        s.feed("fn f() { totally_invented_helper(3); }", true);
        s.settle(&idx_of("fn real_helper() {}"));
        assert_eq!(s.unresolved(), &[Unresolved::Call("totally_invented_helper".into())]);
    }

    #[test]
    fn a_call_the_corpus_knows_is_fine_even_though_the_branch_never_defined_it() {
        let mut s = Scanner::new();
        s.feed("fn f() { existing_api(1); }", true);
        s.settle(&idx_of("pub fn existing_api(x: u32) {}"));
        assert!(s.unresolved().is_empty());
    }

    #[test]
    fn a_partial_trailing_identifier_is_held_back_rather_than_condemned() {
        // The branch shredder case: mid-token buffers must not be scanned.
        let mut s = Scanner::new();
        assert_eq!(s.feed("fn f() { let x = comp", false), 0);
        s.settle(&SymbolIndex::new());
        assert!(s.unresolved().is_empty(), "condemned a half-written name: {:?}", s.unresolved());
        assert_eq!(hold_back("let y = foo"), "let y = ".len());
        assert_eq!(hold_back("let y = foo("), "let y = foo(".len());
    }

    #[test]
    fn nothing_is_condemned_until_the_branch_has_finished() {
        // While generating, an unknown call is a forward reference until proven otherwise.
        let mut s = Scanner::new();
        assert_eq!(s.feed("fn f() { mystery(); ", false), 1);
        assert!(s.unresolved().is_empty());
        assert_eq!(s.suspects(&SymbolIndex::new()), 1, "suspected, not condemned");
    }

    #[test]
    fn suspicion_clears_when_the_definition_arrives() {
        // The mid-generation signal has to fall as well as rise, or it is a countdown to a kill
        // rather than a measurement.
        let mut s = Scanner::new();
        s.feed("fn f() { helper(); ", false);
        assert_eq!(s.suspects(&SymbolIndex::new()), 1);
        s.feed("fn f() { helper(); }\nfn helper() {}\n", true);
        assert_eq!(s.suspects(&SymbolIndex::new()), 0);
        s.settle(&SymbolIndex::new());
        assert!(s.unresolved().is_empty());
    }

    #[test]
    fn control_flow_keywords_are_not_calls() {
        let c = calls("if (a) { while (b) { match (c) {} } } for (x) in y {}");
        assert!(c.is_empty(), "{c:?}");
    }

    #[test]
    fn method_calls_are_not_judged_because_resolving_them_needs_types() {
        let c = calls("x.as_mut_ptr(); self.bind_texture(1); it.collect::<Vec<_>>()");
        assert!(c.is_empty(), "{c:?}");
    }

    #[test]
    fn constructors_and_tuple_variants_are_not_judged() {
        let c = calls("let a = Tilt(3); let b = vec4(1.0, 2.0, 3.0, 4.0); Some(9);");
        assert!(!c.contains(&"Tilt".to_string()), "{c:?}");
        assert!(!c.contains(&"Some".to_string()), "{c:?}");
        assert!(c.contains(&"vec4".to_string()), "lowercase free calls are still judged: {c:?}");
    }

    #[test]
    fn a_definition_name_is_not_a_call_to_itself() {
        // `fn f()` puts f in lexical call position; counting it made every function look like a
        // self-call, which is a guaranteed false positive on well-formed code.
        assert!(calls("fn f() {}").is_empty());
        assert!(calls("struct Pair(u8, u8);").is_empty());
    }

    #[test]
    fn bare_free_calls_are_judged_and_qualified_ones_are_not() {
        let c = calls("let v = slice::from_raw_parts_mut(p, n); helper(1);");
        assert!(c.contains(&"helper".to_string()), "{c:?}");
        assert!(!c.contains(&"from_raw_parts_mut".to_string()), "qualified: {c:?}");
        assert!(!c.contains(&"slice".to_string()), "the qualifier is not the callee: {c:?}");
    }

    #[test]
    fn comments_and_strings_are_not_read_as_code() {
        let c = calls("// see springing (below)\nlet s = \"call illuminated(x) here\"; real(1);");
        assert_eq!(c, vec!["real".to_string()], "{c:?}");
    }

    #[test]
    fn a_parameter_bound_by_the_signature_can_be_called() {
        let mut sc = Scanner::new();
        sc.feed("fn take(pred: impl Fn(u8) -> bool) -> bool { pred(3) }", true);
        sc.settle(&SymbolIndex::new());
        assert!(sc.unresolved().is_empty(), "{:?}", sc.unresolved());
    }

    #[test]
    fn bindings_and_imports_count_as_definitions() {
        let d = definitions("use crate::thing::Widget;\nlet mut gadget = 1;\nstruct Doohickey;");
        for want in ["Widget", "gadget", "Doohickey"] {
            assert!(d.contains(&want.to_string()), "missing {want} in {d:?}");
        }
    }

    #[test]
    fn prelude_names_need_no_corpus() {
        let mut s = Scanner::new();
        s.feed("fn f() { let v = Vec(); println(); Some(1); }", true);
        s.settle(&SymbolIndex::new());
        assert!(s.unresolved().is_empty(), "{:?}", s.unresolved());
    }

    #[test]
    fn familiarity_ranks_conventional_vocabulary_above_invented_vocabulary() {
        let mut idx = SymbolIndex::new();
        for _ in 0..50 { idx.add_source("fn decode_step() { activate_model(); begin_frame(); }"); }
        let s = Scanner::new();
        let common = s.familiarity("decode_step(); activate_model(); begin_frame();", &idx);
        let novel = s.familiarity("zorble(); quux_frobnicate(); wibble_wobble();", &idx);
        assert!(common > novel, "common {common} novel {novel}");
        assert!(novel < 0.2, "an entirely invented vocabulary should score near zero, got {novel}");
    }

    #[test]
    fn an_empty_corpus_condemns_nothing_it_was_not_asked_to() {
        // Degenerate but worth pinning: with no corpus the check reduces to branch-internal
        // consistency, which is still useful and must not become "condemn everything".
        let mut s = Scanner::new();
        s.feed("struct A; impl A { fn go(&self) { self.go(); } }\nfn main() { }", true);
        s.settle(&SymbolIndex::new());
        assert!(s.unresolved().is_empty(), "{:?}", s.unresolved());
    }
}
