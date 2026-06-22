//! v0.1.673: syntax highlighting — the foundation for the GUI editor's
//! Pulsar code-editor shape + markdown source highlighting.
//!
//! A language-parameterized lexer that turns source text into typed spans
//! (keyword / string / comment / number / type / ident / punct / heading /
//! emphasis / link). "Language support" = add a LangSpec. The QML editor
//! renders the spans as colored runs.
//!
//! PURE + testable: lexing here; the colored-run rendering is the QML layer.
//! Deliberately lightweight (a real tree-sitter is overkill for a fast
//! editor highlight pass); covers the common token classes per language.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Type,      // capitalized idents / known type words
    String,
    Comment,
    Number,
    Ident,
    Punct,
    Whitespace,
    // markdown-specific
    Heading,
    Emphasis,  // bold/italic markers + text
    CodeSpan,  // `inline` / fenced
    Link,      // [text](url) / [[zbracket]]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: usize, // byte offset
    pub len: usize,
    pub kind: TokenKind,
}

/// A language's lexing rules. Minimal but enough for editor highlighting.
#[derive(Debug, Clone)]
pub struct LangSpec {
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
    pub string_delims: &'static [char],
    pub keywords: &'static [&'static str],
}

pub fn lang_spec(lang: &str) -> Option<LangSpec> {
    let s = match lang {
        "rust" | "rs" => LangSpec {
            line_comment: Some("//"), block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            keywords: &["fn","let","mut","pub","use","mod","struct","enum","impl",
                "trait","for","while","loop","if","else","match","return","self",
                "Self","crate","as","ref","move","async","await","const","static",
                "where","type","dyn","in","break","continue","unsafe"],
        },
        "python" | "py" => LangSpec {
            line_comment: Some("#"), block_comment: None,
            string_delims: &['"', '\''],
            keywords: &["def","class","import","from","return","if","elif","else",
                "for","while","try","except","finally","with","as","lambda","yield",
                "pass","break","continue","global","nonlocal","in","is","not","and",
                "or","None","True","False","async","await","raise"],
        },
        "c" | "h" | "cpp" | "cc" | "hpp" => LangSpec {
            line_comment: Some("//"), block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            keywords: &["int","char","float","double","void","long","short","unsigned",
                "signed","struct","union","enum","typedef","const","static","extern",
                "return","if","else","for","while","do","switch","case","break",
                "continue","sizeof","goto","volatile","register","#include","#define"],
        },
        "js" | "ts" | "javascript" | "typescript" => LangSpec {
            line_comment: Some("//"), block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            keywords: &["function","const","let","var","return","if","else","for",
                "while","class","extends","new","this","import","export","from",
                "default","async","await","try","catch","finally","throw","typeof",
                "instanceof","null","undefined","true","false"],
        },
        "sh" | "bash" | "shell" | "zsh" => LangSpec {
            line_comment: Some("#"), block_comment: None,
            string_delims: &['"', '\''],
            keywords: &["if","then","else","elif","fi","for","while","do","done",
                "case","esac","function","return","export","local","echo","cd","exit"],
        },
        "toml" => LangSpec {
            line_comment: Some("#"), block_comment: None,
            string_delims: &['"', '\''],
            keywords: &["true","false"],
        },
        _ => return None,
    };
    Some(s)
}

/// Highlight code in `lang` into spans. Unknown language → identifiers,
/// numbers, strings, punctuation only (still useful). Markdown uses
/// `highlight_markdown` instead.
pub fn highlight(code: &str, lang: &str) -> Vec<Span> {
    if lang == "md" || lang == "markdown" { return highlight_markdown(code); }
    let spec = lang_spec(lang);
    let mut spans = Vec::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < code.len() {
        let b = bytes[i];
        // line comment
        if let Some(lc) = spec.as_ref().and_then(|s| s.line_comment) {
            if code[i..].starts_with(lc) {
                let end = code[i..].find('\n').map(|n| i + n).unwrap_or(code.len());
                spans.push(Span { start: i, len: end - i, kind: TokenKind::Comment });
                i = end; continue;
            }
        }
        // block comment
        if let Some((open, close)) = spec.as_ref().and_then(|s| s.block_comment) {
            if code[i..].starts_with(open) {
                let end = code[i + open.len()..].find(close)
                    .map(|n| i + open.len() + n + close.len()).unwrap_or(code.len());
                spans.push(Span { start: i, len: end - i, kind: TokenKind::Comment });
                i = end; continue;
            }
        }
        // string
        let ch = code[i..].chars().next().unwrap();
        if spec.as_ref().map(|s| s.string_delims.contains(&ch)).unwrap_or(ch == '"') {
            let end = string_end(code, i, ch);
            spans.push(Span { start: i, len: end - i, kind: TokenKind::String });
            i = end; continue;
        }
        // number
        if b.is_ascii_digit() {
            let mut j = i;
            while j < code.len() && (bytes[j].is_ascii_digit()
                || bytes[j] == b'.' || bytes[j] == b'_'
                || (bytes[j] as char).is_ascii_hexdigit() || bytes[j]==b'x') { j += 1; }
            spans.push(Span { start: i, len: j - i, kind: TokenKind::Number });
            i = j; continue;
        }
        // identifier / keyword / type
        if ch.is_alphabetic() || ch == '_' || ch == '#' {
            let mut j = i;
            while j < code.len() {
                let c = code[j..].chars().next().unwrap();
                if c.is_alphanumeric() || c == '_' || (j == i && c == '#') { j += code[j..].chars().next().unwrap().len_utf8(); }
                else { break; }
            }
            let word = &code[i..j];
            let kind = if spec.as_ref().map(|s| s.keywords.contains(&word)).unwrap_or(false) {
                TokenKind::Keyword
            } else if word.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                TokenKind::Type
            } else {
                TokenKind::Ident
            };
            spans.push(Span { start: i, len: j - i, kind });
            i = j; continue;
        }
        // whitespace run
        if ch.is_whitespace() {
            let mut j = i;
            while j < code.len() && code[j..].chars().next().unwrap().is_whitespace() {
                j += code[j..].chars().next().unwrap().len_utf8();
            }
            spans.push(Span { start: i, len: j - i, kind: TokenKind::Whitespace });
            i = j; continue;
        }
        // punctuation (single char)
        let cl = ch.len_utf8();
        spans.push(Span { start: i, len: cl, kind: TokenKind::Punct });
        i += cl;
    }
    spans
}

fn string_end(code: &str, start: usize, delim: char) -> usize {
    let mut i = start + delim.len_utf8();
    let bytes = code.as_bytes();
    while i < code.len() {
        if bytes[i] == b'\\' { i += 2; continue; } // escape
        let c = code[i..].chars().next().unwrap();
        i += c.len_utf8();
        if c == delim { return i; }
    }
    code.len()
}

/// Markdown source highlighting (for the WYSIWYG editor's source view):
/// headings, emphasis, code spans/fences, links, and ZBrackets.
pub fn highlight_markdown(md: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    let mut offset = 0;
    for line in md.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = body.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            spans.push(Span { start: offset, len: body.len(), kind: TokenKind::CodeSpan });
            offset += line.len(); continue;
        }
        if in_fence {
            spans.push(Span { start: offset, len: body.len(), kind: TokenKind::CodeSpan });
            offset += line.len(); continue;
        }
        if trimmed.starts_with('#') {
            spans.push(Span { start: offset, len: body.len(), kind: TokenKind::Heading });
            offset += line.len(); continue;
        }
        // inline scan: `code`, **bold**/*em*, [[zbracket]], [text](url)
        line_inline_spans(line, offset, &mut spans);
        offset += line.len();
    }
    spans
}

fn line_inline_spans(line: &str, base: usize, spans: &mut Vec<Span>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < line.len() {
        // ZBracket [[...]]
        if i + 1 < line.len() && &line[i..i + 2] == "[[" {
            if let Some(c) = line[i + 2..].find("]]") {
                let len = c + 4;
                spans.push(Span { start: base + i, len, kind: TokenKind::Link });
                i += len; continue;
            }
        }
        // markdown link [text](url)
        if bytes[i] == b'[' {
            if let Some(close) = line[i..].find("](") {
                if let Some(paren) = line[i + close..].find(')') {
                    let len = close + paren + 1;
                    spans.push(Span { start: base + i, len, kind: TokenKind::Link });
                    i += len; continue;
                }
            }
        }
        // inline code `...`
        if bytes[i] == b'`' {
            if let Some(c) = line[i + 1..].find('`') {
                let len = c + 2;
                spans.push(Span { start: base + i, len, kind: TokenKind::CodeSpan });
                i += len; continue;
            }
        }
        // emphasis ** or * or _
        if bytes[i] == b'*' || bytes[i] == b'_' {
            spans.push(Span { start: base + i, len: 1, kind: TokenKind::Emphasis });
            i += 1; continue;
        }
        i += line[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
}

/// CSS-ish class name for a kind (QML maps these to theme colors).
pub fn kind_class(k: TokenKind) -> &'static str {
    match k {
        TokenKind::Keyword => "kw", TokenKind::Type => "ty",
        TokenKind::String => "str", TokenKind::Comment => "com",
        TokenKind::Number => "num", TokenKind::Ident => "id",
        TokenKind::Punct => "punct", TokenKind::Whitespace => "ws",
        TokenKind::Heading => "h", TokenKind::Emphasis => "em",
        TokenKind::CodeSpan => "code", TokenKind::Link => "link",
    }
}

/// Wire format: "start\tlen\tclass" per span, '\n'-joined (whitespace spans
/// omitted to keep it compact — the renderer fills gaps as plain).
pub fn to_wire(spans: &[Span]) -> String {
    spans.iter()
        .filter(|s| s.kind != TokenKind::Whitespace && s.kind != TokenKind::Ident)
        .map(|s| format!("{}\t{}\t{}", s.start, s.len, kind_class(s.kind)))
        .collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn kinds(code: &str, lang: &str) -> Vec<(String, TokenKind)> {
        highlight(code, lang).iter()
            .filter(|s| s.kind != TokenKind::Whitespace)
            .map(|s| (code[s.start..s.start + s.len].to_string(), s.kind))
            .collect()
    }

    #[test]
    fn rust_keywords_strings_comments() {
        let got = kinds("let x = \"hi\"; // c", "rust");
        assert!(got.contains(&("let".into(), TokenKind::Keyword)));
        assert!(got.contains(&("\"hi\"".into(), TokenKind::String)));
        assert!(got.iter().any(|(t, k)| t == "// c" && *k == TokenKind::Comment));
        assert!(got.contains(&("x".into(), TokenKind::Ident)));
    }

    #[test]
    fn numbers_and_types() {
        let got = kinds("let n = 42; let v: Vec = x;", "rust");
        assert!(got.contains(&("42".into(), TokenKind::Number)));
        assert!(got.contains(&("Vec".into(), TokenKind::Type)));
    }

    #[test]
    fn block_comment_spans() {
        let got = kinds("a /* big\ncomment */ b", "c");
        assert!(got.iter().any(|(t, k)| t.contains("big") && *k == TokenKind::Comment));
    }

    #[test]
    fn python_strings_single_and_double() {
        let got = kinds("x = 'a' + \"b\"", "python");
        assert!(got.contains(&("'a'".into(), TokenKind::String)));
        assert!(got.contains(&("\"b\"".into(), TokenKind::String)));
    }

    #[test]
    fn unknown_lang_still_tokenizes() {
        let got = kinds("foo 123 \"s\"", "zzz");
        assert!(got.contains(&("123".into(), TokenKind::Number)));
        assert!(got.contains(&("\"s\"".into(), TokenKind::String)));
    }

    #[test]
    fn markdown_heading_link_code() {
        let spans = highlight_markdown("# Title\nsee [[Note]] and `x` and [a](b)\n");
        let kinds: Vec<TokenKind> = spans.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&TokenKind::Heading));
        assert!(kinds.contains(&TokenKind::Link));    // [[Note]] and [a](b)
        assert!(kinds.contains(&TokenKind::CodeSpan)); // `x`
    }

    #[test]
    fn markdown_fenced_code_is_codespan() {
        let spans = highlight_markdown("```\ncode [[x]]\n```\n");
        // every line inside the fence is CodeSpan (no Link for the [[x]])
        assert!(spans.iter().all(|s| s.kind == TokenKind::CodeSpan));
    }

    #[test]
    fn string_escape_does_not_end_early() {
        let got = kinds("\"a\\\"b\"", "rust");
        assert_eq!(got[0].0, "\"a\\\"b\"", "escaped quote stays inside string");
    }

    #[test]
    fn wire_omits_noise() {
        let spans = highlight("let x = 1", "rust");
        let wire = to_wire(&spans);
        assert!(wire.contains("\tkw")); // keyword present
        assert!(!wire.contains("\tws")); // whitespace omitted
    }
}
