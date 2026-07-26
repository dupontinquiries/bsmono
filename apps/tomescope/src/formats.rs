//! formats — sniffing + dispatch, plus the small parsers (txt / md / html).
//!
//! The big containers live in their own modules (`formats_epub`,
//! `formats_pdf`, `formats_cbz`), all producing the same [`BookContent`].
//! Sniffing prefers magic bytes over extensions so a mis-named `.epub` that is
//! really a zip-of-jpegs still opens as a comic.

use crate::content::{Block, BookContent, BookImage, ListKind, Span, SpanStyle, TocEntry};
use anyhow::{bail, Context, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Format {
    Epub,
    Pdf,
    Cbz,
    Fb2,
    Markdown,
    Html,
    Txt,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Epub => "EPUB",
            Format::Pdf => "PDF",
            Format::Cbz => "CBZ",
            Format::Fb2 => "FB2",
            Format::Markdown => "MD",
            Format::Html => "HTML",
            Format::Txt => "TXT",
        }
    }

    pub fn from_ext(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "epub" => Format::Epub,
            "pdf" => Format::Pdf,
            "cbz" | "cbt" | "zip" => Format::Cbz, // zip re-sniffed below
            "fb2" => Format::Fb2,
            "md" | "markdown" => Format::Markdown,
            "html" | "htm" | "xhtml" => Format::Html,
            "txt" | "text" | "log" => Format::Txt,
            _ => return None,
        })
    }
}

/// Sniff magic bytes; fall back to extension. `%PDF` → pdf; `PK\x03\x04` →
/// zip family, disambiguated by looking for `META-INF/container.xml`.
pub fn sniff(path: &Path, head: &[u8]) -> Option<Format> {
    if head.starts_with(b"%PDF") {
        return Some(Format::Pdf);
    }
    if head.starts_with(b"PK\x03\x04") {
        // Zip family: epub carries a mimetype entry / container.xml.
        if let Ok(file) = std::fs::File::open(path) {
            if let Ok(mut z) = zip::ZipArchive::new(file) {
                let mut has_container = false;
                let mut image_count = 0usize;
                for i in 0..z.len() {
                    if let Ok(e) = z.by_index(i) {
                        let name = e.name().to_ascii_lowercase();
                        if name == "meta-inf/container.xml" {
                            has_container = true;
                        }
                        if name.ends_with(".jpg")
                            || name.ends_with(".jpeg")
                            || name.ends_with(".png")
                            || name.ends_with(".webp")
                        {
                            image_count += 1;
                        }
                    }
                }
                if has_container {
                    return Some(Format::Epub);
                }
                if image_count > 0 {
                    return Some(Format::Cbz);
                }
            }
        }
        return Format::from_ext(path).or(Some(Format::Cbz));
    }
    if head.starts_with(b"<?xml") {
        let s = String::from_utf8_lossy(head);
        if s.contains("FictionBook") {
            return Some(Format::Fb2);
        }
        if s.to_ascii_lowercase().contains("<html") {
            return Some(Format::Html);
        }
    }
    let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    if lower.contains("<!doctype html") || lower.contains("<html") {
        return Some(Format::Html);
    }
    Format::from_ext(path)
}

/// Raster page store for image-first formats (CBZ) and rasterized PDFs.
pub struct RasterDoc {
    pub pages: Vec<RasterPage>,
    /// Reading direction: manga (right-to-left) flips the pager.
    pub rtl: bool,
}

pub struct RasterPage {
    pub name: String,
    /// Encoded bytes; decoded lazily to textures by the figure cache.
    pub bytes: Vec<u8>,
}

/// Everything `open` returns: metadata guess + parsed content.
pub struct LoadedBook {
    pub format: Format,
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub year: Option<i32>,
    pub isbn: Option<String>,
    pub series: Option<(String, f32)>,
    pub description: Option<String>,
    pub cover: Option<BookImage>,
    pub content: BookContent,
}

impl LoadedBook {
    pub fn bare(format: Format, title: impl Into<String>) -> LoadedBook {
        LoadedBook {
            format,
            title: title.into(),
            authors: Vec::new(),
            language: None,
            publisher: None,
            year: None,
            isbn: None,
            series: None,
            description: None,
            cover: None,
            content: BookContent::default(),
        }
    }
}

/// Open any supported file. This is the single entry the library and reader use.
pub fn open(path: &Path) -> Result<LoadedBook> {
    let mut head = [0u8; 512];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        f.read(&mut head).unwrap_or(0)
    };
    let fmt = sniff(path, &head[..n]).with_context(|| format!("unrecognised format: {}", path.display()))?;
    match fmt {
        Format::Epub => crate::formats_epub::open_epub(path),
        Format::Pdf => crate::formats_pdf::open_pdf(path),
        Format::Cbz => crate::formats_cbz::open_cbz(path),
        Format::Fb2 => crate::formats_cbz::open_fb2(path),
        Format::Markdown => open_markdown(path),
        Format::Html => open_html(path),
        Format::Txt => open_txt(path),
    }
}

/// Cheap metadata-only probe for library scanning: parses just enough to get
/// title/authors/cover without materialising the whole body.
pub fn probe(path: &Path) -> Result<LoadedBook> {
    let mut head = [0u8; 512];
    let n = {
        use std::io::Read;
        let mut f = std::fs::File::open(path)?;
        f.read(&mut head).unwrap_or(0)
    };
    let fmt = sniff(path, &head[..n]).context("unrecognised format")?;
    match fmt {
        Format::Epub => crate::formats_epub::probe_epub(path),
        Format::Pdf => crate::formats_pdf::probe_pdf(path),
        // The rest are cheap enough to fully open.
        _ => open(path),
    }
}

// ---------------------------------------------------------------------------
// TXT
// ---------------------------------------------------------------------------

pub fn open_txt(path: &Path) -> Result<LoadedBook> {
    let raw = std::fs::read(path)?;
    let text = decode_lossy(&raw);
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
    let mut book = LoadedBook::bare(Format::Txt, title);
    // Blank-line paragraphing; single newlines inside a paragraph become spaces
    // (reflow), unless the block "looks preformatted" (indented / very short
    // lines), which stays as Code so ASCII tables survive.
    for chunk in text.split("\n\n") {
        let chunk = chunk.trim_end();
        if chunk.trim().is_empty() {
            continue;
        }
        let lines: Vec<&str> = chunk.lines().collect();
        let preformatted = lines.len() > 1
            && lines.iter().filter(|l| l.starts_with(' ') || l.starts_with('\t')).count() * 2 > lines.len();
        if preformatted {
            book.content.blocks.push(Block::Code { text: chunk.to_string(), lang: None });
        } else {
            let joined = lines.join(" ");
            book.content.blocks.push(Block::Paragraph { spans: vec![Span::plain(joined)] });
        }
    }
    if book.content.blocks.is_empty() {
        book.content.blocks.push(Block::Paragraph { spans: vec![Span::plain("(empty file)")] });
    }
    Ok(book)
}

fn decode_lossy(raw: &[u8]) -> String {
    // UTF-8 first; then a latin-1 salvage so old TXT files aren't a wall of �.
    match std::str::from_utf8(raw) {
        Ok(s) => s.replace("\r\n", "\n"),
        Err(_) => raw.iter().map(|&b| b as char).collect::<String>().replace("\r\n", "\n"),
    }
}

// ---------------------------------------------------------------------------
// Markdown (a deliberate subset: headings, lists, quotes, fences, rules,
// emphasis, inline code, links, images-by-path)
// ---------------------------------------------------------------------------

pub fn open_markdown(path: &Path) -> Result<LoadedBook> {
    let raw = std::fs::read(path)?;
    let text = decode_lossy(&raw);
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
    let mut book = LoadedBook::bare(Format::Markdown, title);
    let base = path.parent().map(Path::to_path_buf);
    parse_markdown_into(&text, &mut book, base.as_deref());
    // First H1 becomes the title.
    if let Some(Block::Heading { level: 1, spans, .. }) =
        book.content.blocks.iter().find(|b| matches!(b, Block::Heading { level: 1, .. }))
    {
        book.title = crate::content::join_spans(spans);
    }
    Ok(book)
}

pub fn parse_markdown_into(text: &str, book: &mut LoadedBook, image_base: Option<&Path>) {
    let mut lines = text.lines().peekable();
    let mut fence: Option<(String, String)> = None; // (lang, buffer)
    while let Some(line) = lines.next() {
        if let Some((lang, buf)) = fence.as_mut() {
            if line.trim_start().starts_with("```") {
                book.content.blocks.push(Block::Code {
                    text: buf.trim_end().to_string(),
                    lang: if lang.is_empty() { None } else { Some(lang.clone()) },
                });
                fence = None;
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fence = Some((trimmed.trim_start_matches('`').trim().to_string(), String::new()));
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            book.content.blocks.push(Block::Rule);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let mut level = 1u8;
            let mut rest = rest;
            while let Some(r) = rest.strip_prefix('#') {
                level += 1;
                rest = r;
            }
            let title = rest.trim().to_string();
            let anchor = slugify(&title);
            let bi = book.content.blocks.len();
            book.content.toc.push(TocEntry { title: title.clone(), depth: level.saturating_sub(1), block: bi });
            book.content.blocks.push(Block::Heading {
                level: level.min(6),
                spans: parse_inline_md(&title),
                anchor: Some(anchor),
            });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            book.content.blocks.push(Block::Quote { spans: parse_inline_md(rest) });
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            let depth = ((line.len() - trimmed.len()) / 2).min(4) as u8;
            book.content.blocks.push(Block::ListItem { kind: ListKind::Bullet, depth, spans: parse_inline_md(rest) });
            continue;
        }
        if let Some((num, rest)) = split_ordered(trimmed) {
            let depth = ((line.len() - trimmed.len()) / 2).min(4) as u8;
            book.content.blocks.push(Block::ListItem {
                kind: ListKind::Numbered(num),
                depth,
                spans: parse_inline_md(rest),
            });
            continue;
        }
        // Image line: ![alt](path)
        if let Some(img) = parse_md_image(trimmed) {
            if let Some(base) = image_base {
                let p = base.join(&img.1);
                if let Ok(bytes) = std::fs::read(&p) {
                    let idx = book.content.images.len();
                    book.content.images.push(BookImage { name: img.1.clone(), bytes, size_hint: None });
                    book.content.blocks.push(Block::Image { image: idx, caption: vec![Span::plain(img.0)] });
                    continue;
                }
            }
            book.content.blocks.push(Block::Paragraph { spans: vec![Span::plain(format!("[image: {}]", img.1))] });
            continue;
        }
        // Paragraph: greedily absorb following non-blank plain lines.
        let mut para = String::from(trimmed);
        while let Some(next) = lines.peek() {
            let nt = next.trim_start();
            let boundary = nt.is_empty()
                || nt.starts_with('#')
                || nt.starts_with("> ")
                || nt.starts_with("- ")
                || nt.starts_with("* ")
                || nt.starts_with("```")
                || split_ordered(nt).is_some();
            if boundary {
                break;
            }
            para.push(' ');
            para.push_str(nt);
            lines.next();
        }
        book.content.blocks.push(Block::Paragraph { spans: parse_inline_md(&para) });
    }
    if let Some((lang, buf)) = fence {
        book.content.blocks.push(Block::Code { text: buf, lang: if lang.is_empty() { None } else { Some(lang) } });
    }
}

fn split_ordered(s: &str) -> Option<(u32, &str)> {
    let dot = s.find(". ")?;
    if dot == 0 || dot > 4 {
        return None;
    }
    let num: u32 = s[..dot].parse().ok()?;
    Some((num, &s[dot + 2..]))
}

fn parse_md_image(s: &str) -> Option<(String, String)> {
    let s = s.strip_prefix("![")?;
    let close = s.find(']')?;
    let alt = s[..close].to_string();
    let rest = s[close + 1..].strip_prefix('(')?;
    let end = rest.find(')')?;
    Some((alt, rest[..end].to_string()))
}

pub fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Inline markdown: **bold**, *italic* / _italic_, `code`, [text](url).
/// A single pass with a tiny state machine; nesting bold+italic works for the
/// common `***x***` case by toggling both.
pub fn parse_inline_md(s: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut cur = String::new();
    let mut style = SpanStyle::PLAIN;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let flush = |out: &mut Vec<Span>, cur: &mut String, style: SpanStyle| {
        if !cur.is_empty() {
            out.push(Span::styled(std::mem::take(cur), style));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let two = i + 1 < chars.len() && chars[i + 1] == c;
        match c {
            '*' | '_' => {
                flush(&mut out, &mut cur, style);
                if two {
                    style.bold = !style.bold;
                    i += 2;
                } else {
                    style.italic = !style.italic;
                    i += 1;
                }
            }
            '`' => {
                flush(&mut out, &mut cur, style);
                style.mono = !style.mono;
                i += 1;
            }
            '[' => {
                // [text](url)
                if let Some(close) = chars[i..].iter().position(|&x| x == ']') {
                    let after = i + close + 1;
                    if chars.get(after) == Some(&'(') {
                        if let Some(endrel) = chars[after..].iter().position(|&x| x == ')') {
                            flush(&mut out, &mut cur, style);
                            let text: String = chars[i + 1..i + close].iter().collect();
                            let url: String = chars[after + 1..after + endrel].iter().collect();
                            let mut sp = Span::styled(text, SpanStyle { underline: true, ..style });
                            sp.link = Some(if let Some(anchor) = url.strip_prefix('#') {
                                crate::content::LinkTarget::Anchor(anchor.to_string())
                            } else {
                                crate::content::LinkTarget::External(url)
                            });
                            out.push(sp);
                            i = after + endrel + 1;
                            continue;
                        }
                    }
                }
                cur.push(c);
                i += 1;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    flush(&mut out, &mut cur, style);
    if out.is_empty() {
        out.push(Span::plain(""));
    }
    out
}

// ---------------------------------------------------------------------------
// Mini-HTML — shared by the standalone .html opener, EPUB chapters and FB2.
// Not a browser: a forgiving tag-soup walker over the elements books use.
// ---------------------------------------------------------------------------

pub fn open_html(path: &Path) -> Result<LoadedBook> {
    let raw = std::fs::read(path)?;
    let text = decode_lossy(&raw);
    let title = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into());
    let mut book = LoadedBook::bare(Format::Html, title);
    let mut sink = HtmlSink::new(&mut book.content);
    parse_html_fragment(&text, &mut sink, &mut |_src| None);
    sink.finish();
    if let Some(t) = extract_title_tag(&text) {
        book.title = t;
    }
    if book.content.blocks.is_empty() {
        bail!("no readable content in {}", path.display());
    }
    Ok(book)
}

fn extract_title_tag(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let a = lower.find("<title")?;
    let a = lower[a..].find('>').map(|o| a + o + 1)?;
    let z = lower[a..].find("</title").map(|o| a + o)?;
    let t = html[a..z].trim();
    if t.is_empty() { None } else { Some(decode_entities(t)) }
}

/// Accumulates inline spans into blocks as tags open/close.
pub struct HtmlSink<'a> {
    pub(crate) content: &'a mut BookContent,
    spans: Vec<Span>,
    style_stack: Vec<SpanStyle>,
    block: BlockKind,
    list_depth: u8,
    ordinal: u32,
    pending_anchor: Option<String>,
    pub skip_depth: u32, // inside <script>/<style>/<head>
}

#[derive(Clone, Copy, PartialEq)]
enum BlockKind {
    Para,
    Heading(u8),
    Quote,
    ListItem(bool), // ordered?
    Pre,
}

impl<'a> HtmlSink<'a> {
    pub fn new(content: &'a mut BookContent) -> HtmlSink<'a> {
        HtmlSink {
            content,
            spans: Vec::new(),
            style_stack: vec![SpanStyle::PLAIN],
            block: BlockKind::Para,
            list_depth: 0,
            ordinal: 0,
            pending_anchor: None,
            skip_depth: 0,
        }
    }

    fn style(&self) -> SpanStyle {
        *self.style_stack.last().unwrap()
    }

    pub fn text(&mut self, t: &str) {
        if self.skip_depth > 0 {
            return;
        }
        let t = if self.block == BlockKind::Pre { t.to_string() } else { collapse_ws(t) };
        if t.is_empty() {
            return;
        }
        // Merge with the previous span when styles match — keeps span counts sane.
        let st = self.style();
        if let Some(last) = self.spans.last_mut() {
            if last.style == st && last.link.is_none() && last.footnote.is_none() {
                last.text.push_str(&t);
                return;
            }
        }
        self.spans.push(Span::styled(t, st));
    }

    pub fn flush(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        let has_text = spans.iter().any(|s| !s.is_whitespace());
        if !has_text {
            return;
        }
        let anchor = self.pending_anchor.take();
        let block = match self.block {
            BlockKind::Para => Block::Paragraph { spans },
            BlockKind::Heading(l) => {
                let title = crate::content::join_spans(&spans);
                self.content.toc.push(TocEntry {
                    title,
                    depth: l.saturating_sub(1),
                    block: self.content.blocks.len(),
                });
                Block::Heading { level: l, spans, anchor }
            }
            BlockKind::Quote => Block::Quote { spans },
            BlockKind::ListItem(ordered) => {
                let kind = if ordered {
                    self.ordinal += 1;
                    ListKind::Numbered(self.ordinal)
                } else {
                    ListKind::Bullet
                };
                Block::ListItem { kind, depth: self.list_depth.saturating_sub(1), spans }
            }
            BlockKind::Pre => Block::Code { text: crate::content::join_spans(&spans), lang: None },
        };
        self.content.blocks.push(block);
    }

    pub fn open_tag(&mut self, name: &str, attrs: &[(String, String)], images: &mut dyn FnMut(&str) -> Option<usize>) {
        let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        match name {
            "script" | "style" | "head" | "svg" => self.skip_depth += 1,
            "p" | "div" | "section" | "article" => {
                self.flush();
                self.block = BlockKind::Para;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                self.block = BlockKind::Heading(name.as_bytes()[1] - b'0');
                if let Some(id) = get("id") {
                    self.pending_anchor = Some(id);
                }
            }
            "blockquote" => {
                self.flush();
                self.block = BlockKind::Quote;
            }
            "pre" => {
                self.flush();
                self.block = BlockKind::Pre;
            }
            "ul" => {
                self.flush();
                self.list_depth += 1;
            }
            "ol" => {
                self.flush();
                self.list_depth += 1;
                self.ordinal = 0;
            }
            "li" => {
                self.flush();
                self.block = BlockKind::ListItem(self.ordinal_context());
            }
            "br" => self.text("\n"),
            "hr" => {
                self.flush();
                self.content.blocks.push(Block::Rule);
            }
            "b" | "strong" => self.push_style(|s| s.bold = true),
            "i" | "em" | "cite" => self.push_style(|s| s.italic = true),
            "u" => self.push_style(|s| s.underline = true),
            "s" | "del" | "strike" => self.push_style(|s| s.strike = true),
            "code" | "tt" | "kbd" | "samp" => self.push_style(|s| s.mono = true),
            "small" => self.push_style(|s| s.small = true),
            "sup" => self.push_style(|s| s.sup = true),
            "sub" => self.push_style(|s| s.sub = true),
            "mark" => self.push_style(|s| s.marked = true),
            "a" => {
                self.push_style(|s| s.underline = true);
                if let Some(href) = get("href") {
                    // The next text() call gets merged manually below via a
                    // dedicated span so the link survives.
                    self.spans.push(Span {
                        text: String::new(),
                        style: self.style(),
                        link: Some(if let Some(anchor) = href.strip_prefix('#') {
                            crate::content::LinkTarget::Anchor(anchor.to_string())
                        } else {
                            crate::content::LinkTarget::External(href)
                        }),
                        footnote: None,
                    });
                }
                if let Some(id) = get("id") {
                    self.pending_anchor.get_or_insert(id);
                }
            }
            "img" | "image" => {
                if let Some(src) = get("src").or_else(|| get("xlink:href")).or_else(|| get("href")) {
                    if let Some(idx) = images(&src) {
                        self.flush();
                        let caption = get("alt").map(|a| vec![Span::plain(a)]).unwrap_or_default();
                        self.content.blocks.push(Block::Image { image: idx, caption });
                    }
                }
            }
            _ => {}
        }
    }

    fn ordinal_context(&self) -> bool {
        // Approximate: treat as ordered iff we're inside an <ol> (ordinal reset there).
        self.ordinal > 0 || self.list_depth > 0 && self.ordinal == 0 && false
    }

    fn push_style(&mut self, f: impl FnOnce(&mut SpanStyle)) {
        let mut s = self.style();
        f(&mut s);
        self.style_stack.push(s);
    }

    pub fn close_tag(&mut self, name: &str) {
        match name {
            "script" | "style" | "head" | "svg" => self.skip_depth = self.skip_depth.saturating_sub(1),
            "p" | "div" | "section" | "article" | "li" | "blockquote" | "pre" | "h1" | "h2" | "h3" | "h4"
            | "h5" | "h6" => {
                self.flush();
                self.block = BlockKind::Para;
            }
            "ul" | "ol" => {
                self.flush();
                self.list_depth = self.list_depth.saturating_sub(1);
            }
            "b" | "strong" | "i" | "em" | "cite" | "u" | "s" | "del" | "strike" | "code" | "tt" | "kbd"
            | "samp" | "small" | "sup" | "sub" | "mark" | "a" => {
                if self.style_stack.len() > 1 {
                    self.style_stack.pop();
                }
            }
            _ => {}
        }
    }

    pub fn finish(&mut self) {
        self.flush();
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    out
}

pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let rest = &s[i..];
        let semi = rest.find(';');
        match semi {
            Some(end) if end <= 10 => {
                let ent = &rest[1..end];
                let decoded = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "hellip" => Some('…'),
                    "ldquo" => Some('“'),
                    "rdquo" => Some('”'),
                    "lsquo" => Some('‘'),
                    "rsquo" => Some('’'),
                    _ => {
                        if let Some(num) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                            u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
                        } else if let Some(num) = ent.strip_prefix('#') {
                            num.parse::<u32>().ok().and_then(char::from_u32)
                        } else {
                            None
                        }
                    }
                };
                if let Some(d) = decoded {
                    out.push(d);
                    for _ in 0..end {
                        it.next();
                    }
                } else {
                    out.push('&');
                }
            }
            _ => out.push('&'),
        }
    }
    out
}

/// Walk raw HTML text, driving a sink. `images` maps a src to an image index
/// (the caller loads bytes from its container and registers them).
pub fn parse_html_fragment(html: &str, sink: &mut HtmlSink, images: &mut dyn FnMut(&str) -> Option<usize>) {
    let bytes = html.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Comment?
            if html[i..].starts_with("<!--") {
                i = html[i..].find("-->").map(|o| i + o + 3).unwrap_or(bytes.len());
                continue;
            }
            let close = match html[i..].find('>') {
                Some(o) => i + o,
                None => break,
            };
            let inner = &html[i + 1..close];
            i = close + 1;
            let inner = inner.trim();
            if inner.starts_with('!') || inner.starts_with('?') {
                continue;
            }
            let (name_part, closing) = match inner.strip_prefix('/') {
                Some(rest) => (rest, true),
                None => (inner, false),
            };
            let name_end = name_part.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(name_part.len());
            let name = name_part[..name_end].to_ascii_lowercase();
            if closing {
                sink.close_tag(&name);
            } else {
                let attrs = parse_attrs(&name_part[name_end..]);
                let self_closing = inner.ends_with('/')
                    || matches!(name.as_str(), "br" | "hr" | "img" | "image" | "meta" | "link" | "input");
                sink.open_tag(&name, &attrs, images);
                if self_closing && !matches!(name.as_str(), "br" | "hr" | "img" | "image") {
                    sink.close_tag(&name);
                }
            }
        } else {
            let next = html[i..].find('<').map(|o| i + o).unwrap_or(bytes.len());
            let text = decode_entities(&html[i..next]);
            sink.text(&text);
            i = next;
        }
    }
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() && rest != "/" {
        let eq = match rest.find('=') {
            Some(e) => e,
            None => break,
        };
        let key = rest[..eq].trim().trim_start_matches('/').to_ascii_lowercase();
        let after = rest[eq + 1..].trim_start();
        let (val, consumed) = if let Some(q) = after.strip_prefix('"') {
            let end = q.find('"').unwrap_or(q.len());
            (q[..end].to_string(), eq + 1 + (after.len() - q.len()) + end + 1)
        } else if let Some(q) = after.strip_prefix('\'') {
            let end = q.find('\'').unwrap_or(q.len());
            (q[..end].to_string(), eq + 1 + (after.len() - q.len()) + end + 1)
        } else {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            (after[..end].to_string(), eq + 1 + (rest[eq + 1..].len() - after.len()) + end)
        };
        out.push((key, decode_entities(&val)));
        rest = rest.get(consumed..).unwrap_or("").trim_start();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_md_bold_italic_code() {
        let spans = parse_inline_md("a **b** *c* `d`");
        let joined = crate::content::join_spans(&spans);
        assert_eq!(joined, "a b c d");
        assert!(spans.iter().any(|s| s.style.bold && s.text == "b"));
        assert!(spans.iter().any(|s| s.style.italic && s.text == "c"));
        assert!(spans.iter().any(|s| s.style.mono && s.text == "d"));
    }

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a &amp; b &#233; &hellip;"), "a & b é …");
    }

    #[test]
    fn html_headings_build_toc() {
        let mut book = LoadedBook::bare(Format::Html, "t");
        let mut sink = HtmlSink::new(&mut book.content);
        parse_html_fragment("<h1>One</h1><p>body <b>bold</b></p><h2 id=\"two\">Two</h2>", &mut sink, &mut |_| None);
        sink.finish();
        assert_eq!(book.content.toc.len(), 2);
        assert_eq!(book.content.toc[1].title, "Two");
        assert!(matches!(book.content.blocks[0], Block::Heading { level: 1, .. }));
    }
}
