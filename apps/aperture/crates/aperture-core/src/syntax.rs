//! syntax — the highlighting seam, shaped like the thing that replaces it.
//!
//! Highlighting an edited file has exactly one hard part: line N's meaning can
//! depend on state opened arbitrarily far above (an unclosed `/*`). So the
//! interface is a **line lexer**: `(line_text, entry_state) → (spans, exit_state)`,
//! and [`SyntaxCache`] owns the per-line entry states, computed lazily down to
//! whatever the viewport asks for and truncated from the first edited line.
//! Editing at the top of a huge file costs one state-threading pass down to
//! the visible region — nothing more, because nothing below is ever computed
//! until looked at.
//!
//! The shipped [`Lexer`] is a deliberately small hand-rolled Rust tokenizer —
//! keywords, strings, char-vs-lifetime, nested block comments, attrs, numbers.
//! It is a placeholder with honest limits (strings don't span lines; no type
//! vs. value distinction). The seam is the point: tree-sitter drops in behind
//! this exact interface later (its incremental state is richer, the shape is
//! the same), and nothing in the shell changes. See DESIGN.md §syntax for why
//! tree-sitter is *not* in v0 (grammar-crate version churn).
//!
//! Spans are **byte** ranges into the line — what `egui::text::LayoutJob`
//! sections take — and only non-plain runs are emitted; the renderer gap-fills.

use std::ops::Range;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Plain,
    Keyword,
    Number,
    Str,
    Comment,
    Attr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>, // bytes into the line
    pub kind: TokKind,
}

/// Cross-line lexer state. Only nested block-comment depth carries for Rust.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LexState {
    pub block_comment: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    Plain,
}

impl Language {
    pub fn from_path(path: &Path) -> Language {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => Language::Rust,
            _ => Language::Plain,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Lexer {
    pub lang: Language,
}

impl Lexer {
    pub fn new(lang: Language) -> Self {
        Lexer { lang }
    }

    pub fn lex(&self, line: &str, entry: LexState) -> (Vec<Span>, LexState) {
        match self.lang {
            Language::Plain => (Vec::new(), entry),
            Language::Rust => lex_rust(line, entry),
        }
    }
}

/// Entry states per line, lazy and truncate-on-edit.
pub struct SyntaxCache {
    lexer: Lexer,
    /// `states[i]` = state at the start of line `i`. `states[0]` is Default and
    /// always present; the vec grows as lines are demanded.
    states: Vec<LexState>,
}

impl SyntaxCache {
    pub fn new(lexer: Lexer) -> Self {
        SyntaxCache { lexer, states: vec![LexState::default()] }
    }

    pub fn set_language(&mut self, lang: Language) {
        self.lexer = Lexer::new(lang);
        self.invalidate_from(0);
    }

    /// Call with the first line an edit touched. Cheap: a truncate.
    pub fn invalidate_from(&mut self, line: usize) {
        self.states.truncate(line.max(0) + 1);
        if self.states.is_empty() {
            self.states.push(LexState::default());
        }
        self.states.truncate(line + 1);
    }

    /// Spans for `line`, threading state from the last known point.
    /// `get_line(i)` supplies line text (newline-free).
    pub fn spans_for(&mut self, line: usize, mut get_line: impl FnMut(usize) -> String) -> Vec<Span> {
        while self.states.len() <= line {
            let i = self.states.len() - 1;
            let (_, exit) = self.lexer.lex(&get_line(i), self.states[i]);
            self.states.push(exit);
        }
        let (spans, exit) = self.lexer.lex(&get_line(line), self.states[line]);
        if self.states.len() == line + 1 {
            self.states.push(exit);
        }
        spans
    }
}

// ── the Rust line lexer ─────────────────────────────────────────────────────

const KEYWORDS: &[&str] = &[
    "Self", "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

fn is_kw(word: &str) -> bool {
    KEYWORDS.binary_search(&word).is_ok()
}

fn lex_rust(line: &str, entry: LexState) -> (Vec<Span>, LexState) {
    let b = line.as_bytes();
    let n = b.len();
    let mut spans = Vec::new();
    let mut st = entry;
    let mut i = 0usize;

    // Resume an open block comment from above.
    if st.block_comment > 0 {
        let start = 0;
        while i < n && st.block_comment > 0 {
            if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
                st.block_comment += 1;
                i += 2;
            } else if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
                st.block_comment -= 1;
                i += 2;
            } else {
                i += 1;
            }
        }
        spans.push(Span { range: start..i, kind: TokKind::Comment });
        if st.block_comment > 0 {
            return (spans, st); // whole line was comment
        }
    }

    while i < n {
        let c = b[i];
        // line comment
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            spans.push(Span { range: i..n, kind: TokKind::Comment });
            break;
        }
        // block comment (possibly closing on this line, possibly nesting)
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let start = i;
            st.block_comment = 1;
            i += 2;
            while i < n && st.block_comment > 0 {
                if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
                    st.block_comment += 1;
                    i += 2;
                } else if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
                    st.block_comment -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            spans.push(Span { range: start..i, kind: TokKind::Comment });
            continue;
        }
        // raw string r"…" / r#"…"# (intra-line only — honest limit)
        if c == b'r' && i + 1 < n && (b[i + 1] == b'"' || b[i + 1] == b'#') {
            let start = i;
            let mut j = i + 1;
            let mut hashes = 0;
            while j < n && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < n && b[j] == b'"' {
                j += 1;
                'scan: while j < n {
                    if b[j] == b'"' {
                        let mut k = j + 1;
                        let mut h = 0;
                        while k < n && b[k] == b'#' && h < hashes {
                            h += 1;
                            k += 1;
                        }
                        if h == hashes {
                            j = k;
                            break 'scan;
                        }
                    }
                    j += 1;
                }
                spans.push(Span { range: start..j.min(n), kind: TokKind::Str });
                i = j.min(n).max(i + 1);
                continue;
            }
        }
        // string
        if c == b'"' {
            let start = i;
            i += 1;
            while i < n {
                match b[i] {
                    b'\\' => i = (i + 2).min(n),
                    b'"' => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            spans.push(Span { range: start..i, kind: TokKind::Str });
            continue;
        }
        // char literal vs lifetime: 'a' is Str, 'a (no close nearby) is plain
        if c == b'\'' {
            let close = if i + 2 < n && b[i + 1] == b'\\' {
                // escaped char: find the ' after the escape
                (i + 3..n.min(i + 8)).find(|&k| b[k] == b'\'')
            } else if i + 2 < n && b[i + 2] == b'\'' {
                Some(i + 2)
            } else {
                None
            };
            if let Some(end) = close {
                spans.push(Span { range: i..end + 1, kind: TokKind::Str });
                i = end + 1;
                continue;
            }
            i += 1; // lifetime tick: plain
            continue;
        }
        // attribute #[…] / #![…]
        if c == b'#' && i + 1 < n && (b[i + 1] == b'[' || (b[i + 1] == b'!' && i + 2 < n && b[i + 2] == b'[')) {
            let start = i;
            let mut depth = 0i32;
            while i < n {
                match b[i] {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            spans.push(Span { range: start..i, kind: TokKind::Attr });
            continue;
        }
        // number
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                // "1..10" — don't eat the range operator
                if b[i] == b'.' && i + 1 < n && b[i + 1] == b'.' {
                    break;
                }
                i += 1;
            }
            spans.push(Span { range: start..i, kind: TokKind::Number });
            continue;
        }
        // identifier / keyword
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if is_kw(&line[start..i]) {
                spans.push(Span { range: start..i, kind: TokKind::Keyword });
            }
            continue;
        }
        i += 1;
    }
    (spans, st)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(TokKind, String)> {
        let (spans, _) = Lexer::new(Language::Rust).lex(line, LexState::default());
        spans.into_iter().map(|s| (s.kind, line[s.range].to_string())).collect()
    }

    #[test]
    fn keywords_strings_numbers_comments() {
        let toks = kinds(r#"let x = "a // not comment"; // real"#);
        assert_eq!(toks[0], (TokKind::Keyword, "let".into()));
        assert_eq!(toks[1], (TokKind::Str, r#""a // not comment""#.into()));
        assert_eq!(toks[2], (TokKind::Comment, "// real".into()));
        assert_eq!(kinds("x + 0xFF_u8")[0], (TokKind::Number, "0xFF_u8".into()));
        assert_eq!(kinds("for i in 1..10 {")[2], (TokKind::Number, "1".into()));
    }

    #[test]
    fn char_literal_vs_lifetime() {
        assert_eq!(kinds("let c = 'x';")[1], (TokKind::Str, "'x'".into()));
        assert_eq!(kinds(r"let c = '\n';")[1], (TokKind::Str, r"'\n'".into()));
        let toks = kinds("fn f<'a>(x: &'a str)");
        assert!(
            toks.iter().all(|(k, _)| *k != TokKind::Str),
            "lifetimes are not char literals: {toks:?}"
        );
    }

    #[test]
    fn nested_block_comments_carry_state_across_lines() {
        let lx = Lexer::new(Language::Rust);
        let (s1, st1) = lx.lex("code /* open /* deeper", LexState::default());
        assert_eq!(st1.block_comment, 2);
        assert_eq!(s1.last().unwrap().kind, TokKind::Comment);
        let (s2, st2) = lx.lex("still */ inside */ let x = 1;", st1);
        assert_eq!(st2.block_comment, 0);
        assert_eq!(s2[0].kind, TokKind::Comment);
        assert!(s2.iter().any(|s| s.kind == TokKind::Keyword), "code after close lexes: {s2:?}");
    }

    #[test]
    fn attr_and_raw_string() {
        assert_eq!(kinds("#[derive(Debug)]")[0].0, TokKind::Attr);
        assert_eq!(kinds(r####"let p = r#"a "quoted" b"#;"####)[1], (TokKind::Str, r####"r#"a "quoted" b"#"####.into()));
    }

    #[test]
    fn cache_recolors_downstream_after_upstream_edit() {
        let lines_v1 = ["fn a() {}", "fn b() {}", "fn c() {}"];
        let lines_v2 = ["/* opened", "fn b() {}", "fn c() {}"];
        let mut cache = SyntaxCache::new(Lexer::new(Language::Rust));
        let s = cache.spans_for(2, |i| lines_v1[i].to_string());
        assert_eq!(s[0].kind, TokKind::Keyword);
        // Edit line 0: it now opens a block comment. Invalidate from there.
        cache.invalidate_from(0);
        let s = cache.spans_for(2, |i| lines_v2[i].to_string());
        assert_eq!(s[0].kind, TokKind::Comment, "line 2 is inside the comment now");
    }

    #[test]
    fn keyword_table_is_sorted_for_binary_search() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS);
    }
}
