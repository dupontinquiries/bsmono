//! content — the neutral in-memory book model every format parses into.
//!
//! Formats (epub/pdf/fb2/md/…) produce a [`BookContent`]: a flat list of
//! [`Block`]s plus a table of contents that indexes into it. The layout engine
//! consumes blocks; the reader addresses positions as `(block, char)` so a
//! reading position survives re-pagination when typography changes — the same
//! trick reflowable readers use so "page 141" is a *view*, not an address.

use serde::{Deserialize, Serialize};

/// Inline style bits carried by a [`Span`]. Kept as flags (not a class system)
/// so the layout pass can map straight to egui `TextFormat`s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub small: bool,
    pub sup: bool,
    pub sub: bool,
    pub underline: bool,
    pub strike: bool,
    /// Highlighter-style emphasis from the source document (e.g. <mark>).
    pub marked: bool,
}

impl SpanStyle {
    pub const PLAIN: SpanStyle = SpanStyle {
        bold: false,
        italic: false,
        mono: false,
        small: false,
        sup: false,
        sub: false,
        underline: false,
        strike: false,
        marked: false,
    };

    pub fn bold() -> Self {
        SpanStyle { bold: true, ..Self::PLAIN }
    }
    pub fn italic() -> Self {
        SpanStyle { italic: true, ..Self::PLAIN }
    }
    pub fn mono() -> Self {
        SpanStyle { mono: true, ..Self::PLAIN }
    }
}

/// A run of text with one style, optionally acting as a link.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
    /// Internal link target (`#anchor` resolved to a block index at parse time
    /// when possible) or an external href kept verbatim.
    pub link: Option<LinkTarget>,
    /// Footnote reference: index into `BookContent::footnotes`.
    pub footnote: Option<usize>,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Span {
        Span { text: text.into(), style: SpanStyle::PLAIN, link: None, footnote: None }
    }
    pub fn styled(text: impl Into<String>, style: SpanStyle) -> Span {
        Span { text: text.into(), style, link: None, footnote: None }
    }
    pub fn is_whitespace(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LinkTarget {
    /// Jump to a block in this book.
    Block(usize),
    /// Unresolved internal anchor (resolved lazily by id).
    Anchor(String),
    /// External URL — shown, never auto-opened.
    External(String),
}

/// List flavour for `Block::ListItem`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ListKind {
    Bullet,
    /// Ordinal carried explicitly so partial parses keep numbering.
    Numbered(u32),
}

/// One block-level element. The unit of layout and of addressing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Block {
    /// level 1..=6.
    Heading { level: u8, spans: Vec<Span>, anchor: Option<String> },
    Paragraph { spans: Vec<Span> },
    Quote { spans: Vec<Span> },
    Code { text: String, lang: Option<String> },
    ListItem { kind: ListKind, depth: u8, spans: Vec<Span> },
    /// Index into `BookContent::images`.
    Image { image: usize, caption: Vec<Span> },
    /// Simple grid: rows of cells, each cell a span list. Header row optional.
    Table { header: Option<Vec<Vec<Span>>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
    /// A soft hint from paged sources (PDF page ends, EPUB spine breaks).
    /// The paginator may honour or ignore it depending on settings.
    PageBreak { label: Option<String> },
    /// Numbered display equation kept as source text (LaTeX-ish); rendered
    /// monospace-centered until a real math pass exists.
    Equation { tex: String, number: Option<String> },
}

impl Block {
    pub fn spans(&self) -> Option<&[Span]> {
        match self {
            Block::Heading { spans, .. }
            | Block::Paragraph { spans }
            | Block::Quote { spans }
            | Block::ListItem { spans, .. } => Some(spans),
            _ => None,
        }
    }

    /// Plain text of the block (for search / TTS / export).
    pub fn plain_text(&self) -> String {
        match self {
            Block::Code { text, .. } => text.clone(),
            Block::Equation { tex, .. } => tex.clone(),
            Block::Image { caption, .. } => join_spans(caption),
            Block::Table { header, rows } => {
                let mut out = String::new();
                if let Some(h) = header {
                    for cell in h {
                        out.push_str(&join_spans(cell));
                        out.push('\t');
                    }
                    out.push('\n');
                }
                for row in rows {
                    for cell in row {
                        out.push_str(&join_spans(cell));
                        out.push('\t');
                    }
                    out.push('\n');
                }
                out
            }
            Block::Rule => String::new(),
            Block::PageBreak { label } => label.clone().unwrap_or_default(),
            _ => self.spans().map(join_spans).unwrap_or_default(),
        }
    }

    pub fn char_len(&self) -> usize {
        self.plain_text().chars().count()
    }
}

pub fn join_spans(spans: &[Span]) -> String {
    let mut s = String::new();
    for sp in spans {
        s.push_str(&sp.text);
    }
    s
}

/// A decoded (or decodable) image resource carried by the book.
#[derive(Clone)]
pub struct BookImage {
    pub name: String,
    /// Raw bytes (png/jpeg/…); decoded lazily into a texture by the cover /
    /// figure cache so opening a 400-figure textbook stays fast.
    pub bytes: Vec<u8>,
    /// Pre-known dimensions if the container told us (CBZ does after decode).
    pub size_hint: Option<(u32, u32)>,
}

impl std::fmt::Debug for BookImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BookImage({} • {} bytes)", self.name, self.bytes.len())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TocEntry {
    pub title: String,
    pub depth: u8,
    pub block: usize,
}

/// A footnote body: parsed spans shown in the footnote popover.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Footnote {
    pub label: String,
    pub spans: Vec<Span>,
}

/// Back-of-book index term → blocks it appears at. Textbook feature: powers
/// the index panel and seeds the per-book glossary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexTerm {
    pub term: String,
    pub blocks: Vec<usize>,
}

/// A reading address that survives re-pagination: block index + char offset
/// into that block's plain text.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Locus {
    pub block: usize,
    pub ch: usize,
}

impl Locus {
    pub const START: Locus = Locus { block: 0, ch: 0 };
    pub fn at_block(block: usize) -> Locus {
        Locus { block, ch: 0 }
    }
}

/// An inclusive text range in book coordinates (for highlights/search hits).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LocusRange {
    pub from: Locus,
    pub to: Locus,
}

impl LocusRange {
    pub fn normalized(self) -> LocusRange {
        if (self.to.block, self.to.ch) < (self.from.block, self.from.ch) {
            LocusRange { from: self.to, to: self.from }
        } else {
            self
        }
    }
    pub fn contains(&self, l: Locus) -> bool {
        let a = (self.from.block, self.from.ch);
        let b = (self.to.block, self.to.ch);
        let p = (l.block, l.ch);
        a <= p && p <= b
    }
    pub fn single_block(&self) -> bool {
        self.from.block == self.to.block
    }
}

/// The whole parsed book.
#[derive(Default)]
pub struct BookContent {
    pub blocks: Vec<Block>,
    pub toc: Vec<TocEntry>,
    pub images: Vec<BookImage>,
    pub footnotes: Vec<Footnote>,
    pub index_terms: Vec<IndexTerm>,
    /// Raster pages, for image-first formats (CBZ) or a rasterized PDF.
    pub raster: Option<crate::formats::RasterDoc>,
    /// Page labels from the source (roman-numeral front matter, "A-3", …),
    /// keyed by the block index of the `PageBreak` that starts them.
    pub page_labels: Vec<(usize, String)>,
}

impl BookContent {
    pub fn is_raster_only(&self) -> bool {
        self.raster.is_some() && self.blocks.is_empty()
    }

    /// Total characters, for percent-progress math.
    pub fn total_chars(&self) -> usize {
        self.blocks.iter().map(Block::char_len).sum::<usize>().max(1)
    }

    /// Chars strictly before `l`, for percent-progress math.
    pub fn chars_before(&self, l: Locus) -> usize {
        let mut n = 0usize;
        for (i, b) in self.blocks.iter().enumerate() {
            if i < l.block {
                n += b.char_len();
            } else {
                n += l.ch.min(b.char_len());
                break;
            }
        }
        n
    }

    pub fn percent_at(&self, l: Locus) -> f32 {
        self.chars_before(l) as f32 / self.total_chars() as f32
    }

    /// Resolve a named anchor to a block index (heading anchors, epub ids).
    pub fn resolve_anchor(&self, id: &str) -> Option<usize> {
        self.blocks.iter().position(|b| match b {
            Block::Heading { anchor: Some(a), .. } => a == id,
            _ => false,
        })
    }

    /// The toc entry active at a locus (last entry at or before it).
    pub fn toc_at(&self, l: Locus) -> Option<&TocEntry> {
        self.toc.iter().rev().find(|t| t.block <= l.block)
    }

    /// Case-insensitive full-text search returning ranges, capped.
    pub fn search(&self, needle: &str, cap: usize) -> Vec<LocusRange> {
        let mut out = Vec::new();
        if needle.trim().is_empty() {
            return out;
        }
        let nl = needle.to_lowercase();
        let nlen = nl.chars().count();
        for (bi, b) in self.blocks.iter().enumerate() {
            let text = b.plain_text().to_lowercase();
            let mut start = 0usize; // char offset
            let chars: Vec<char> = text.chars().collect();
            let nchars: Vec<char> = nl.chars().collect();
            while start + nlen <= chars.len() {
                if chars[start..start + nlen] == nchars[..] {
                    out.push(LocusRange {
                        from: Locus { block: bi, ch: start },
                        to: Locus { block: bi, ch: start + nlen },
                    });
                    if out.len() >= cap {
                        return out;
                    }
                    start += nlen.max(1);
                } else {
                    start += 1;
                }
            }
        }
        out
    }

    /// Extract the words around a locus (context snippet for search results
    /// and dictionary lookups).
    pub fn snippet(&self, r: LocusRange, pad: usize) -> String {
        let Some(b) = self.blocks.get(r.from.block) else { return String::new() };
        let chars: Vec<char> = b.plain_text().chars().collect();
        let a = r.from.ch.saturating_sub(pad);
        let z = (r.to.ch + pad).min(chars.len());
        let mut s: String = chars[a..z].iter().collect();
        if a > 0 {
            s.insert(0, '…');
        }
        if z < chars.len() {
            s.push('…');
        }
        s.replace('\n', " ")
    }

    /// The word (letters/digits/apostrophes) under a locus, for tap-to-define.
    pub fn word_at(&self, l: Locus) -> Option<(String, LocusRange)> {
        let b = self.blocks.get(l.block)?;
        let chars: Vec<char> = b.plain_text().chars().collect();
        if chars.is_empty() {
            return None;
        }
        let i = l.ch.min(chars.len() - 1);
        let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '-';
        if !is_word(chars[i]) {
            return None;
        }
        let mut a = i;
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        let mut z = i + 1;
        while z < chars.len() && is_word(chars[z]) {
            z += 1;
        }
        let word: String = chars[a..z].iter().collect();
        Some((word, LocusRange { from: Locus { block: l.block, ch: a }, to: Locus { block: l.block, ch: z } }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book_of(paras: &[&str]) -> BookContent {
        let mut c = BookContent::default();
        for p in paras {
            c.blocks.push(Block::Paragraph { spans: vec![Span::plain(*p)] });
        }
        c
    }

    #[test]
    fn percent_math_monotonic() {
        let c = book_of(&["one two", "three four", "five"]);
        let p0 = c.percent_at(Locus::START);
        let p1 = c.percent_at(Locus::at_block(1));
        let p2 = c.percent_at(Locus { block: 2, ch: 4 });
        assert!(p0 < p1 && p1 < p2);
    }

    #[test]
    fn search_finds_across_case() {
        let c = book_of(&["The Mitochondria is the powerhouse", "mitochondria again"]);
        let hits = c.search("MITOCHONDRIA", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].from.block, 0);
        assert_eq!(hits[1].from.block, 1);
    }

    #[test]
    fn word_at_extracts_whole_word() {
        let c = book_of(&["gradient descent converges"]);
        let (w, r) = c.word_at(Locus { block: 0, ch: 10 }).unwrap();
        assert_eq!(w, "descent");
        assert_eq!(r.from.ch, 9);
    }
}
