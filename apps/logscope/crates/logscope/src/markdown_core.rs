//! markdown_core.rs — the PURE Markdown parsing/inline-formatting logic, with no GTK dependency.
//!
//! Split out of `ui/markdown.rs` (which is gated behind the `gui` feature and pulls in gtk4/pango) so the
//! grammar can be unit-tested and reused without a display stack. The GTK renderer builds widgets from
//! [`parse_blocks`] and turns inline spans into Pango markup via [`inline_to_pango`]; both are here.
//!
//! Inline formatting is UTF-8 correct: it walks the text by CHAR, not byte — the previous version mixed a
//! char cursor with byte slices (`text[i+2..]`), so any non-ASCII character before a `**bold**` / `*italic*`
//! / `~~strike~~` marker sliced at the wrong offset (corrupt output, or a panic on a non-char boundary).

/// A parsed Markdown block. Field-for-field what the GTK renderer switches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: String },
    Para(String),
    Code { lang: String, text: String },
    Blockquote(String),
    ListItem { ordered: bool, index: usize, text: String },
    HRule,
}

/// Parse Markdown into a flat list of blocks. Single-pass state machine over the grammar that actually
/// appears in LLM output (fenced code, headings, rules, blockquotes, ordered/unordered lists, paragraphs).
/// Consecutive plain paragraphs are merged. Prefix slices here are all ASCII (`#`, `> `, `- `), so byte
/// indexing is safe; only the inline pass needed the char-cursor fix.
pub fn parse_blocks(md: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut lines = md.lines().peekable();
    let mut list_idx = 0usize;

    while let Some(line) = lines.next() {
        // Fenced code block
        if line.starts_with("```") {
            let lang = line.trim_start_matches('`').trim().to_string();
            let mut code = String::new();
            for l in lines.by_ref() {
                if l.starts_with("```") { break; }
                code.push_str(l);
                code.push('\n');
            }
            blocks.push(Block::Code { lang, text: code });
            list_idx = 0;
            continue;
        }
        // Horizontal rule
        if line.trim() == "---" || line.trim() == "***" || line.trim() == "___" {
            blocks.push(Block::HRule);
            continue;
        }
        // Heading
        let hashes = line.chars().take_while(|&c| c == '#').count();
        if hashes > 0 && hashes <= 6 && line.chars().nth(hashes) == Some(' ') {
            blocks.push(Block::Heading { level: hashes as u8, text: line[hashes + 1..].to_string() });
            list_idx = 0;
            continue;
        }
        // Blockquote
        if let Some(rest) = line.strip_prefix("> ") {
            blocks.push(Block::Blockquote(rest.to_string()));
            continue;
        }
        // Ordered list item ("1. " / "1) ")
        if let Some(rest) = line.strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|s| s.strip_prefix(". ").or_else(|| s.strip_prefix(") "))) {
            list_idx += 1;
            blocks.push(Block::ListItem { ordered: true, index: list_idx, text: rest.to_string() });
            continue;
        }
        // Unordered list item
        if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")).or_else(|| line.strip_prefix("+ ")) {
            blocks.push(Block::ListItem { ordered: false, index: 0, text: rest.to_string() });
            list_idx = 0;
            continue;
        }
        // Empty line
        if line.trim().is_empty() { list_idx = 0; continue; }
        // Regular paragraph
        blocks.push(Block::Para(line.to_string()));
        list_idx = 0;
    }

    // Merge consecutive Para blocks into single paragraphs.
    let mut merged: Vec<Block> = Vec::new();
    for block in blocks {
        if let Block::Para(ref text) = block {
            if let Some(Block::Para(prev)) = merged.last_mut() {
                prev.push(' ');
                prev.push_str(text);
                continue;
            }
        }
        merged.push(block);
    }
    merged
}

/// Escape the three XML/Pango-significant characters in literal text.
fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }

/// Convert one line of inline Markdown into Pango markup (`<b>`, `<i>`, `<tt>`, `<s>`), escaping literal
/// `& < >`. Walks by CHAR so multi-byte text can't desync the cursor from the slices.
pub fn inline_to_pango(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len() + 32);
    let mut i = 0;

    // Offset (relative to `start`) of the first occurrence of `needle` in `chars[start..]`, by char index.
    let find_from = |start: usize, needle: &[char]| -> Option<usize> {
        if needle.is_empty() || start >= n || needle.len() > n - start { return None; }
        (start..=n - needle.len()).find(|&j| chars[j..j + needle.len()] == *needle).map(|j| j - start)
    };
    let collect = |a: usize, b: usize| -> String { chars[a..b].iter().collect() };

    while i < n {
        match chars[i] {
            '&' => { out.push_str("&amp;"); i += 1; continue; }
            '<' => { out.push_str("&lt;"); i += 1; continue; }
            '>' => { out.push_str("&gt;"); i += 1; continue; }
            _ => {}
        }
        // Inline code `code` — literal (escaped, not re-parsed).
        if chars[i] == '`' {
            if let Some(rel) = find_from(i + 1, &['`']) {
                out.push_str("<tt>");
                out.push_str(&esc(&collect(i + 1, i + 1 + rel)));
                out.push_str("</tt>");
                i += rel + 2;
                continue;
            }
        }
        // Bold+italic ***text***
        if i + 2 < n && chars[i] == '*' && chars[i + 1] == '*' && chars[i + 2] == '*' {
            if let Some(rel) = find_from(i + 3, &['*', '*', '*']) {
                out.push_str("<b><i>");
                out.push_str(&inline_to_pango(&collect(i + 3, i + 3 + rel)));
                out.push_str("</i></b>");
                i += rel + 6;
                continue;
            }
        }
        // Bold **text**
        if i + 1 < n && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(rel) = find_from(i + 2, &['*', '*']) {
                out.push_str("<b>");
                out.push_str(&inline_to_pango(&collect(i + 2, i + 2 + rel)));
                out.push_str("</b>");
                i += rel + 4;
                continue;
            }
        }
        // Italic *text* / _text_ — only when opened at a word boundary (start or after a space).
        if (chars[i] == '*' || chars[i] == '_') && (i == 0 || chars[i - 1] == ' ') {
            let delim = chars[i];
            if let Some(rel) = find_from(i + 1, &[delim]) {
                out.push_str("<i>");
                out.push_str(&inline_to_pango(&collect(i + 1, i + 1 + rel)));
                out.push_str("</i>");
                i += rel + 2;
                continue;
            }
        }
        // Strikethrough ~~text~~
        if i + 1 < n && chars[i] == '~' && chars[i + 1] == '~' {
            if let Some(rel) = find_from(i + 2, &['~', '~']) {
                out.push_str("<s>");
                out.push_str(&inline_to_pango(&collect(i + 2, i + 2 + rel)));
                out.push_str("</s>");
                i += rel + 4;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Whether text has meaningful Markdown formatting (so plain chat isn't run through the renderer).
pub fn has_markdown(text: &str) -> bool {
    text.contains("```")
        || text.contains("**")
        || text.contains("# ")
        || text.contains("\n- ")
        || text.contains("\n* ")
        || text.contains("\n> ")
        || text.contains("> ")
        || text.contains("~~")
        || (text.contains('`') && text.matches('`').count() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_italic_code_strike_basic() {
        assert_eq!(inline_to_pango("a **b** c"), "a <b>b</b> c");
        assert_eq!(inline_to_pango("say `x=1` ok"), "say <tt>x=1</tt> ok");
        assert_eq!(inline_to_pango("~~no~~"), "<s>no</s>");
        assert_eq!(inline_to_pango("***wow***"), "<b><i>wow</i></b>");
        // italic only opens at a word boundary
        assert_eq!(inline_to_pango("start *em* end"), "start <i>em</i> end");
        assert_eq!(inline_to_pango("a_b_c"), "a_b_c"); // mid-word underscores are literal
    }

    #[test]
    fn escapes_xml_specials() {
        assert_eq!(inline_to_pango("1 < 2 && 3 > 0"), "1 &lt; 2 &amp;&amp; 3 &gt; 0");
        // code content is escaped too
        assert_eq!(inline_to_pango("`<T>`"), "<tt>&lt;T&gt;</tt>");
    }

    #[test]
    fn utf8_before_marker_does_not_corrupt_or_panic() {
        // REGRESSION: a multi-byte char before a marker used to desync the byte slices from the char
        // cursor — producing "*bold"/garbage, or panicking on a non-char boundary.
        assert_eq!(inline_to_pango("café **bold**"), "café <b>bold</b>");
        assert_eq!(inline_to_pango("naïve *it* über"), "naïve <i>it</i> über");
        assert_eq!(inline_to_pango("日本語 `code` 中文"), "日本語 <tt>code</tt> 中文");
        assert_eq!(inline_to_pango("emoji 😀 ~~gone~~"), "emoji 😀 <s>gone</s>");
        // multi-byte INSIDE the emphasized span
        assert_eq!(inline_to_pango("**café**"), "<b>café</b>");
        // and it must never panic on an unterminated marker after multibyte text
        let _ = inline_to_pango("café **oops");
    }

    #[test]
    fn nested_inline() {
        assert_eq!(inline_to_pango("**bold `code` in**"), "<b>bold <tt>code</tt> in</b>");
        // NOTE: the simple parser closes an italic at the first following delimiter, so an italic that
        // *contains* a bold isn't nested (a known limitation, unchanged by the UTF-8 fix). Bold-containing-
        // italic works because ** is matched as a pair before single *.
        assert_eq!(inline_to_pango("**a *b* c**"), "<b>a <i>b</i> c</b>");
    }

    #[test]
    fn parse_blocks_recognizes_grammar() {
        let md = "# Title\n\nsome text here\nmore text\n\n```rust\nfn x() {}\n```\n\n- a\n- b\n\n1. one\n2. two\n\n> quote\n\n---";
        let bs = parse_blocks(md);
        assert!(matches!(bs[0], Block::Heading { level: 1, .. }));
        // the two prose lines merge into one paragraph
        assert!(matches!(&bs[1], Block::Para(t) if t == "some text here more text"));
        assert!(matches!(&bs[2], Block::Code { lang, .. } if lang == "rust"));
        assert!(matches!(bs[3], Block::ListItem { ordered: false, .. }));
        assert!(bs.iter().any(|b| matches!(b, Block::ListItem { ordered: true, index: 2, .. })));
        assert!(bs.iter().any(|b| matches!(b, Block::Blockquote(_))));
        assert!(bs.iter().any(|b| matches!(b, Block::HRule)));
    }

    #[test]
    fn heading_after_multibyte_line() {
        // headings slice by byte but only over the ASCII `#`+space prefix, so multibyte content is fine
        let bs = parse_blocks("## Café ☕");
        assert_eq!(bs, vec![Block::Heading { level: 2, text: "Café ☕".into() }]);
    }

    #[test]
    fn has_markdown_detects_and_ignores() {
        assert!(has_markdown("this has **bold**"));
        assert!(has_markdown("```\ncode\n```"));
        assert!(has_markdown("intro\n- a list"));
        assert!(!has_markdown("just a plain sentence with no formatting"));
        assert!(!has_markdown("a single ` backtick"));
    }
}
