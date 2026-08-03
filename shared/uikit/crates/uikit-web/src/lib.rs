//! web — an efficient HTML → egui renderer for voiceterm.
//!
//! **The changed assumption.** The old path rendered web content into QML, which *retains* an
//! element tree in its scene graph. egui is immediate-mode: re-parsing HTML every frame would
//! be wasteful. So the model inverts — we parse the HTML **once** into a flat, render-ready
//! **display list** ([`RichDoc`]: blocks of styled inline runs), and each frame we just walk
//! that list into egui widgets. Layout/measurement is egui's job per frame, but tokenizing and
//! tree-flattening (the expensive part) happen only when the content changes.
//!
//! The parser is a tolerant subset — block tags (p, h1–h6, li, pre/code, blockquote, hr) and
//! inline tags (b/strong, i/em, code, a) with HTML entities — enough for command output, docs,
//! and HTML mail. Unknown tags degrade to their text content rather than breaking.

pub mod doc;

/// Inline text styling (flattened from nested inline tags).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// A run of text sharing one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
}

/// The kind of block (drives block-level layout).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    ListItem,
    Code,
    Quote,
    Rule,
}

/// A block: a kind plus its inline runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub inlines: Vec<Inline>,
}

/// The render-ready display list. Built once from HTML; rendered many times.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RichDoc {
    pub blocks: Vec<Block>,
}

impl RichDoc {
    /// Plain-text projection (handy for tests / a11y / search).
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(|b| b.inlines.iter().map(|i| i.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── tokenizer ────────────────────────────────────────────────────────────────

enum Token {
    Open { name: String, attrs: Vec<(String, String)> },
    Close(String),
    Text(String),
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some(semi) = after.find(';') {
            let ent = &after[1..semi];
            let decoded = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            match decoded {
                Some(c) => {
                    out.push(c);
                    rest = &after[semi + 1..];
                    continue;
                }
                None => {
                    out.push('&');
                    rest = &after[1..];
                    continue;
                }
            }
        } else {
            out.push('&');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(html: &str) -> Vec<Token> {
    let mut toks = Vec::new();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // find the end of the tag
            if let Some(end) = html[i..].find('>') {
                let raw = &html[i + 1..i + end];
                i += end + 1;
                let raw = raw.trim();
                if raw.starts_with('!') || raw.starts_with('?') {
                    continue; // comments / doctype / PIs
                }
                if let Some(name) = raw.strip_prefix('/') {
                    toks.push(Token::Close(name.trim().to_lowercase()));
                } else {
                    let self_close = raw.ends_with('/');
                    let body = raw.trim_end_matches('/').trim();
                    let mut parts = body.splitn(2, char::is_whitespace);
                    let name = parts.next().unwrap_or("").to_lowercase();
                    let attrs = parts.next().map(parse_attrs).unwrap_or_default();
                    toks.push(Token::Open { name: name.clone(), attrs });
                    if self_close {
                        toks.push(Token::Close(name));
                    }
                }
            } else {
                break; // unterminated tag
            }
        } else {
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(bytes.len());
            let text = decode_entities(&html[i..next]);
            if !text.is_empty() {
                toks.push(Token::Text(text));
            }
            i = next;
        }
    }
    toks
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < n && chars[i] != '=' && !chars[i].is_whitespace() {
            i += 1;
        }
        let key: String = chars[ks..i].iter().collect::<String>().to_lowercase();
        if key.is_empty() {
            break;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < n && chars[i] == '=' {
            i += 1;
            while i < n && chars[i].is_whitespace() {
                i += 1;
            }
            if i < n && (chars[i] == '"' || chars[i] == '\'') {
                let q = chars[i];
                i += 1;
                let vs = i;
                while i < n && chars[i] != q {
                    i += 1;
                }
                val = chars[vs..i].iter().collect();
                if i < n {
                    i += 1;
                }
            } else {
                let vs = i;
                while i < n && !chars[i].is_whitespace() {
                    i += 1;
                }
                val = chars[vs..i].iter().collect();
            }
        }
        attrs.push((key, decode_entities(&val)));
    }
    attrs
}

// ── build the display list ───────────────────────────────────────────────────

fn is_block(tag: &str) -> Option<BlockKind> {
    match tag {
        "p" | "div" => Some(BlockKind::Paragraph),
        "h1" => Some(BlockKind::Heading(1)),
        "h2" => Some(BlockKind::Heading(2)),
        "h3" => Some(BlockKind::Heading(3)),
        "h4" => Some(BlockKind::Heading(4)),
        "h5" => Some(BlockKind::Heading(5)),
        "h6" => Some(BlockKind::Heading(6)),
        "li" => Some(BlockKind::ListItem),
        "pre" => Some(BlockKind::Code),
        "blockquote" => Some(BlockKind::Quote),
        _ => None,
    }
}

/// Parse HTML into a flat [`RichDoc`] display list. Do this once, on content change.
/// Tags whose CONTENTS are not content.
///
/// Shared with `doc::parse_page` deliberately. The two parsers here are
/// different algorithms over different models — a flat block list and a tree —
/// and that is fine, but they are not allowed to disagree about what counts as
/// prose. `parse_html` used to have no notion of this at all and rendered
/// `var x = 1;` and `p{color:red}` straight into the document; the browser view
/// dropped them and the manual reader did not, which reads as a broken page
/// rather than as a bug and so goes unreported.
pub(crate) fn is_noise_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "head" | "noscript" | "svg")
}

pub fn parse_html(html: &str) -> RichDoc {
    let toks = tokenize(html);
    let mut blocks: Vec<Block> = Vec::new();
    /// While inside a noise element, text tokens are discarded. Depth-counted
    /// rather than a flag, because `<svg><style>…</style></svg>` would
    /// otherwise switch prose back on at the inner close.
    let mut noise_depth: usize = 0;
    let mut cur: Option<Block> = None;
    let mut style = InlineStyle::default();
    let mut style_stack: Vec<InlineStyle> = Vec::new();

    let flush = |cur: &mut Option<Block>, blocks: &mut Vec<Block>| {
        if let Some(b) = cur.take() {
            if !b.inlines.iter().all(|i| i.text.trim().is_empty()) || b.kind == BlockKind::Rule {
                blocks.push(b);
            }
        }
    };

    for tok in toks {
        // Handled before anything else: a noise element's own attributes and
        // children must not reach the block builder either.
        match &tok {
            Token::Open { name, .. } if is_noise_tag(name) => { noise_depth += 1; continue; }
            Token::Close(name) if is_noise_tag(name) => {
                noise_depth = noise_depth.saturating_sub(1);
                continue;
            }
            _ if noise_depth > 0 => continue,
            _ => {}
        }
        match tok {
            Token::Open { name, attrs } => {
                if name == "hr" {
                    flush(&mut cur, &mut blocks);
                    blocks.push(Block { kind: BlockKind::Rule, inlines: vec![] });
                } else if let Some(kind) = is_block(&name) {
                    flush(&mut cur, &mut blocks);
                    cur = Some(Block { kind, inlines: vec![] });
                } else {
                    // inline styling
                    style_stack.push(style.clone());
                    match name.as_str() {
                        "b" | "strong" => style.bold = true,
                        "i" | "em" => style.italic = true,
                        "code" => style.code = true,
                        "a" => style.link = attrs.iter().find(|(k, _)| k == "href").map(|(_, v)| v.clone()),
                        "br" => {
                            if let Some(b) = cur.as_mut() {
                                b.inlines.push(Inline { text: "\n".into(), style: style.clone() });
                            }
                            style_stack.pop();
                        }
                        _ => {} // unknown inline: keep current style, render its text
                    }
                }
            }
            Token::Close(name) => {
                if is_block(&name).is_some() || name == "hr" {
                    flush(&mut cur, &mut blocks);
                } else if matches!(name.as_str(), "b" | "strong" | "i" | "em" | "code" | "a") {
                    if let Some(prev) = style_stack.pop() {
                        style = prev;
                    }
                }
            }
            Token::Text(text) => {
                if cur.is_none() {
                    cur = Some(Block { kind: BlockKind::Paragraph, inlines: vec![] });
                }
                let preserve = style.code || cur.as_ref().map(|b| b.kind == BlockKind::Code).unwrap_or(false);
                if let Some(b) = cur.as_mut() {
                    let text = if preserve { text } else { collapse_ws(&text) };
                    if !text.is_empty() {
                        b.inlines.push(Inline { text, style: style.clone() });
                    }
                }
            }
        }
    }
    flush(&mut cur, &mut blocks);
    RichDoc { blocks }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

// ── egui render (thin; walks the prebuilt list each frame) ───────────────────

/// Render a prebuilt [`RichDoc`] into egui. Cheap — no parsing here.
pub fn render(ui: &mut egui::Ui, doc: &RichDoc, colors: &uikit_egui::PaletteColors) {
    for block in &doc.blocks {
        match &block.kind {
            BlockKind::Rule => {
                ui.separator();
                continue;
            }
            BlockKind::Heading(level) => {
                let size = match level {
                    1 => 22.0,
                    2 => 19.0,
                    3 => 16.0,
                    _ => 14.0,
                };
                ui.horizontal_wrapped(|ui| {
                    for run in &block.inlines {
                        ui.label(egui::RichText::new(&run.text).size(size).strong().color(colors.text));
                    }
                });
                continue;
            }
            _ => {}
        }
        let quote = block.kind == BlockKind::Quote;
        let bullet = block.kind == BlockKind::ListItem;
        ui.horizontal_wrapped(|ui| {
            if bullet {
                ui.label(egui::RichText::new("• ").color(colors.text_weak));
            }
            if quote {
                ui.label(egui::RichText::new("▏").color(colors.accent_soft));
            }
            for run in &block.inlines {
                let mut rt = egui::RichText::new(&run.text);
                if run.style.bold {
                    rt = rt.strong();
                }
                if run.style.italic {
                    rt = rt.italics();
                }
                if run.style.code {
                    rt = rt.monospace().background_color(colors.bg_raised);
                }
                rt = rt.color(if run.style.link.is_some() { colors.accent } else if quote { colors.text_weak } else { colors.text });
                ui.label(rt);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_and_paragraph() {
        let doc = parse_html("<h1>Title</h1><p>hello world</p>");
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(doc.blocks[0].inlines[0].text, "Title");
        assert_eq!(doc.blocks[1].kind, BlockKind::Paragraph);
        assert_eq!(doc.blocks[1].inlines[0].text, "hello world");
    }

    #[test]
    fn inline_styles_flatten() {
        let doc = parse_html("<p>a <b>bold</b> and <i>italic</i> c</p>");
        let runs = &doc.blocks[0].inlines;
        assert!(runs.iter().any(|r| r.text == "bold" && r.style.bold && !r.style.italic));
        assert!(runs.iter().any(|r| r.text == "italic" && r.style.italic));
        // the plain runs are not styled
        assert!(runs.iter().any(|r| r.text.contains('a') && !r.style.bold));
    }

    #[test]
    fn nested_styles_compose() {
        let doc = parse_html("<p><b><i>x</i></b></p>");
        let r = &doc.blocks[0].inlines[0];
        assert!(r.style.bold && r.style.italic);
    }

    #[test]
    fn links_carry_href() {
        let doc = parse_html(r#"<p><a href="https://x.test">click</a></p>"#);
        let r = &doc.blocks[0].inlines[0];
        assert_eq!(r.text, "click");
        assert_eq!(r.style.link.as_deref(), Some("https://x.test"));
    }

    #[test]
    fn list_items_and_rule() {
        let doc = parse_html("<ul><li>one</li><li>two</li></ul><hr>");
        let kinds: Vec<_> = doc.blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds, vec![BlockKind::ListItem, BlockKind::ListItem, BlockKind::Rule]);
    }

    #[test]
    fn entities_decode() {
        let doc = parse_html("<p>a &amp; b &lt;tag&gt; &#39;q&#39;</p>");
        assert_eq!(doc.blocks[0].inlines[0].text, "a & b <tag> 'q'");
    }

    #[test]
    fn pre_preserves_whitespace_others_collapse() {
        let collapsed = parse_html("<p>a   b\n\tc</p>");
        assert_eq!(collapsed.blocks[0].inlines[0].text, "a b c");
        let pre = parse_html("<pre>a   b</pre>");
        assert_eq!(pre.blocks[0].inlines[0].text, "a   b");
    }

    #[test]
    fn unknown_tags_degrade_to_text() {
        let doc = parse_html("<section><span>kept</span></section>");
        assert!(doc.text().contains("kept"));
    }
}

#[cfg(test)]
mod noise_parity_tests {
    use super::*;

    /// The two parsers in this crate must agree about what is not content.
    ///
    /// `parse_html` (flat blocks, used by the manual reader and the mail view)
    /// and `doc::parse_page` (a tree, used by the browser) are deliberately
    /// different algorithms over different models. They are NOT allowed to
    /// disagree about which tags are noise: the same page rendered in two views
    /// showing JavaScript source in one of them is the kind of inconsistency
    /// nobody reports because it looks like a broken page rather than a bug.
    #[test]
    fn neither_parser_renders_script_or_style_as_prose() {
        let html = "<p>before</p><script>var x = 1;</script>\
                    <style>p{color:red}</style><p>after</p>";
        let flat = parse_html(html);
        let text: String = flat.blocks.iter()
            .flat_map(|b| b.inlines.iter().map(|r| r.text.clone()))
            .collect::<Vec<_>>().join(" ");
        assert!(!text.contains("var x"), "parse_html leaked script source: {text:?}");
        assert!(!text.contains("color:red"), "parse_html leaked CSS: {text:?}");
        assert!(text.contains("before") && text.contains("after"));

        let tree = crate::doc::parse_page(html).root.text();
        assert!(!tree.contains("var x"));
        assert!(!tree.contains("color:red"));
    }
}
