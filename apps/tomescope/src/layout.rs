//! Layout / pagination engine.
//!
//! Tomescope treats "a page" as a derived, cached view over the neutral
//! `BookContent` model rather than something baked in at parse time. This
//! lets the same parsed book be re-paginated instantly when font size,
//! margins, column count, or window size change — which is the whole
//! point of an ebook reader (as opposed to a fixed-layout PDF viewer).
//!
//! The engine here is intentionally egui-agnostic in its *data types* (so
//! it can be unit tested without a GUI context) but its measurement pass
//! takes an egui `Fonts`/`Ui` reference to get real glyph metrics, since
//! that's the only way pagination will actually match what's drawn.

use crate::content::{Block, BookContent, Locus};
use std::collections::HashMap;

/// User-controllable typography knobs. Anything that changes how text is
/// shaped should live here so it can be hashed into the pagination cache
/// key — if it's not in this struct, changing it silently won't
/// repaginate, which is a bug class we want to make structurally hard to
/// hit.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Typography {
    pub font_size: f32,
    pub line_height_mult: f32,
    pub paragraph_spacing_mult: f32,
    pub margin_frac: f32,
    pub columns: u8,
    pub column_gap: f32,
    pub justify: bool,
    pub hyphenate: bool,
    pub font_family: FontFamily,
    pub bold_headings: bool,
    pub indent_paragraphs: bool,
    pub indent_em: f32,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            font_size: 18.0,
            line_height_mult: 1.35,
            paragraph_spacing_mult: 0.6,
            margin_frac: 0.08,
            columns: 1,
            column_gap: 32.0,
            justify: false,
            hyphenate: true,
            font_family: FontFamily::Serif,
            bold_headings: true,
            indent_paragraphs: false,
            indent_em: 1.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FontFamily {
    Serif,
    SansSerif,
    Monospace,
    OpenDyslexic,
}

impl FontFamily {
    pub fn all() -> &'static [FontFamily] {
        &[
            FontFamily::Serif,
            FontFamily::SansSerif,
            FontFamily::Monospace,
            FontFamily::OpenDyslexic,
        ]
    }
    pub fn label(&self) -> &'static str {
        match self {
            FontFamily::Serif => "Serif",
            FontFamily::SansSerif => "Sans-serif",
            FontFamily::Monospace => "Monospace",
            FontFamily::OpenDyslexic => "OpenDyslexic",
        }
    }
}

/// A stable hash of everything that affects pagination, used as a cache
/// key alongside the viewport size and book id.
fn typography_hash(t: &Typography) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |bits: u64| {
        h ^= bits;
        h = h.wrapping_mul(0x100000001b3);
    };
    mix(t.font_size.to_bits() as u64);
    mix(t.line_height_mult.to_bits() as u64);
    mix(t.paragraph_spacing_mult.to_bits() as u64);
    mix(t.margin_frac.to_bits() as u64);
    mix(t.columns as u64);
    mix(t.column_gap.to_bits() as u64);
    mix(t.justify as u64);
    mix(t.hyphenate as u64);
    mix(t.font_family as u64 as u64);
    mix(t.bold_headings as u64);
    mix(t.indent_paragraphs as u64);
    mix(t.indent_em.to_bits() as u64);
    h
}

impl FontFamily {
    // Cheap discriminant for hashing.
    fn discriminant(&self) -> u32 {
        match self {
            FontFamily::Serif => 0,
            FontFamily::SansSerif => 1,
            FontFamily::Monospace => 2,
            FontFamily::OpenDyslexic => 3,
        }
    }
}

impl std::ops::BitXor<u32> for FontFamily {
    type Output = u32;
    fn bitxor(self, rhs: u32) -> u32 {
        self.discriminant() ^ rhs
    }
}

// The `as u64 as u64` above works because FontFamily derives no numeric
// cast directly; provide an explicit From impl to keep things principled.
impl From<FontFamily> for u64 {
    fn from(f: FontFamily) -> u64 {
        f.discriminant() as u64
    }
}

/// One measured line within a page: which block it comes from, and the
/// character range (into that block's plain text) it covers.
#[derive(Clone, Debug)]
pub struct MeasuredLine {
    pub block: usize,
    pub char_start: usize,
    pub char_end: usize,
    pub height: f32,
    pub is_block_start: bool,
}

/// One paginated page: a contiguous run of measured lines plus the locus
/// range it represents, so progress/search/bookmarks can map back into
/// the neutral model without depending on pagination having run yet.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub lines: Vec<MeasuredLine>,
    pub start: Locus,
    pub end: Locus,
    pub column_of_line: Vec<u8>,
}

impl Page {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Result of a full pagination pass over a book at a given viewport size.
#[derive(Clone, Debug, Default)]
pub struct Pagination {
    pub pages: Vec<Page>,
    pub page_of_block: HashMap<usize, usize>,
}

impl Pagination {
    pub fn page_for_locus(&self, locus: Locus) -> usize {
        // Prefer exact containment; fall back to nearest block start.
        for (i, p) in self.pages.iter().enumerate() {
            if locus_in_page(locus, p) {
                return i;
            }
        }
        *self.page_of_block.get(&locus.block).unwrap_or(&0)
    }
}

fn locus_in_page(locus: Locus, p: &Page) -> bool {
    if p.lines.is_empty() {
        return false;
    }
    let after_start = locus.block > p.start.block
        || (locus.block == p.start.block && locus.ch >= p.start.ch);
    let before_end =
        locus.block < p.end.block || (locus.block == p.end.block && locus.ch <= p.end.ch);
    after_start && before_end
}

/// Abstraction over glyph measurement so the pagination algorithm can be
/// unit tested with a fake monospace measurer, and driven for real by
/// egui's `Fonts` at draw/layout time.
pub trait LineMeasurer {
    /// Given a block's plain text starting at `char_start`, and an
    /// available width in points, return the char length of the next
    /// line that fits (greedy word-wrap) and its rendered height.
    fn next_line(
        &self,
        block: &Block,
        text: &str,
        char_start: usize,
        avail_width: f32,
        typo: &Typography,
    ) -> (usize, f32);

    /// Height reserved for a non-text block (image, rule, table, page
    /// break marker) at the given width.
    fn block_height(&self, block: &Block, avail_width: f32, typo: &Typography) -> f32;
}

/// A simple width-estimate measurer usable without any GUI context —
/// good enough for tests and for a first synchronous pass before a real
/// `egui::Fonts`-backed measurer takes over on the next frame.
pub struct ApproxMeasurer {
    pub avg_char_width_em: f32,
}

impl Default for ApproxMeasurer {
    fn default() -> Self {
        Self {
            avg_char_width_em: 0.52,
        }
    }
}

impl LineMeasurer for ApproxMeasurer {
    fn next_line(
        &self,
        _block: &Block,
        text: &str,
        char_start: usize,
        avail_width: f32,
        typo: &Typography,
    ) -> (usize, f32) {
        let chars: Vec<char> = text.chars().collect();
        if char_start >= chars.len() {
            return (char_start, typo.font_size * typo.line_height_mult);
        }
        let char_w = typo.font_size * self.avg_char_width_em;
        let max_chars = ((avail_width / char_w).floor() as usize).max(1);
        let mut end = (char_start + max_chars).min(chars.len());
        // Greedy word-wrap: back off to the last space if we split mid-word
        // and there's more text after this line.
        if end < chars.len() {
            let mut back = end;
            while back > char_start && !chars[back - 1].is_whitespace() {
                back -= 1;
            }
            if back > char_start {
                end = back;
            }
        }
        (end.max(char_start + 1), typo.font_size * typo.line_height_mult)
    }

    fn block_height(&self, block: &Block, _avail_width: f32, typo: &Typography) -> f32 {
        match block {
            Block::Image { .. } => typo.font_size * 12.0,
            Block::Rule => typo.font_size * 1.2,
            Block::PageBreak { .. } => 0.0,
            Block::Table { rows, .. } => (rows.len() as f32 + 1.0) * typo.font_size * 1.6,
            Block::Equation { .. } => typo.font_size * 2.2,
            _ => 0.0,
        }
    }
}

/// Full pagination pass: walks every block, greedily wraps text into
/// lines via the given measurer, and cuts pages once accumulated height
/// exceeds `page_height`.
#[allow(unused_assignments)]
pub fn paginate(
    content: &BookContent,
    measurer: &dyn LineMeasurer,
    typo: &Typography,
    page_width: f32,
    page_height: f32,
) -> Pagination {
    let col_width = if typo.columns > 1 {
        (page_width - typo.column_gap * (typo.columns as f32 - 1.0)) / typo.columns as f32
    } else {
        page_width
    };

    let mut pages = Vec::new();
    let mut page_of_block = HashMap::new();
    let mut cur = Page::default();
    let mut cur_height = 0.0f32;
    let mut cur_col: u8 = 0;
    let para_gap = typo.font_size * typo.paragraph_spacing_mult;

    let mut start_locus = Locus::START;
    let mut first_line_in_doc = true;

    macro_rules! flush_page {
        () => {
            if !cur.lines.is_empty() {
                cur.start = start_locus;
                let last = cur.lines.last().unwrap();
                cur.end = Locus {
                    block: last.block,
                    ch: last.char_end,
                };
                pages.push(std::mem::take(&mut cur));
                cur_height = 0.0;
                cur_col = 0;
            }
        };
    }

    for (bi, block) in content.blocks.iter().enumerate() {
        page_of_block.entry(bi).or_insert(pages.len());

        if let Block::PageBreak { .. } = block {
            flush_page!();
            start_locus = Locus { block: bi + 1, ch: 0 };
            continue;
        }

        let text = block.plain_text();
        let mut is_block_start = true;

        if text.is_empty() {
            let h = measurer.block_height(block, col_width, typo);
            if cur_height + h > page_height && !cur.lines.is_empty() {
                flush_page!();
                start_locus = Locus { block: bi, ch: 0 };
            }
            cur.lines.push(MeasuredLine {
                block: bi,
                char_start: 0,
                char_end: 0,
                height: h,
                is_block_start: true,
            });
            cur.column_of_line.push(cur_col);
            cur_height += h;
            continue;
        }

        let mut pos = 0usize;
        let char_total = text.chars().count();
        while pos < char_total {
            let (next, h) = measurer.next_line(block, &text, pos, col_width, typo);
            let extra = if is_block_start { para_gap } else { 0.0 };
            if cur_height + h + extra > page_height && !cur.lines.is_empty() {
                if typo.columns > 1 && cur_col + 1 < typo.columns {
                    cur_col += 1;
                    cur_height = 0.0;
                } else {
                    flush_page!();
                    start_locus = Locus { block: bi, ch: pos };
                }
            }
            cur.lines.push(MeasuredLine {
                block: bi,
                char_start: pos,
                char_end: next,
                height: h,
                is_block_start,
            });
            cur.column_of_line.push(cur_col);
            cur_height += h + extra;
            is_block_start = false;
            if first_line_in_doc {
                start_locus = Locus { block: bi, ch: pos };
                first_line_in_doc = false;
            }
            pos = next;
        }
    }
    flush_page!();

    // Guarantee at least one (possibly empty) page so the reader has
    // something to show for a book with zero blocks.
    if pages.is_empty() {
        pages.push(Page {
            lines: vec![],
            start: Locus::START,
            end: Locus::START,
            column_of_line: vec![],
        });
    }

    Pagination {
        pages,
        page_of_block,
    }
}

/// Cache of pagination results keyed by (book id, typography hash,
/// viewport width/height rounded to the nearest few points — small
/// resizes shouldn't thrash a full repagination).
pub struct PaginationCache {
    entries: HashMap<(u64, u64, i32, i32), std::rc::Rc<Pagination>>,
    order: Vec<(u64, u64, i32, i32)>,
    cap: usize,
}

impl Default for PaginationCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            cap: 6,
        }
    }
}

fn round_dim(v: f32) -> i32 {
    // Snap to nearest 4pt to avoid thrashing on sub-pixel resizes.
    ((v / 4.0).round() as i32) * 4
}

impl PaginationCache {
    /// Returns an owned, cheaply-cloneable handle to the pagination
    /// result. Deliberately NOT `&Pagination`: an elided-lifetime return
    /// tied to `&mut self` would keep the whole cache (and by extension
    /// the `ReaderState` that owns it) borrowed for as long as the
    /// caller holds the result — which is exactly as long as a reader
    /// screen needs it (across a page-turn call that itself needs
    /// `&mut ReaderState`). Returning `Rc<Pagination>` breaks that tie at
    /// the cost of a refcount bump instead of a borrow.
    pub fn get_or_compute(
        &mut self,
        book_id: u64,
        content: &BookContent,
        measurer: &dyn LineMeasurer,
        typo: &Typography,
        page_width: f32,
        page_height: f32,
    ) -> std::rc::Rc<Pagination> {
        let key = (
            book_id,
            typography_hash(typo),
            round_dim(page_width),
            round_dim(page_height),
        );
        if !self.entries.contains_key(&key) {
            let result = std::rc::Rc::new(paginate(content, measurer, typo, page_width, page_height));
            if self.entries.len() >= self.cap {
                if let Some(oldest) = self.order.first().cloned() {
                    self.entries.remove(&oldest);
                    self.order.remove(0);
                }
            }
            self.entries.insert(key, result);
            self.order.push(key);
        } else {
            // bump recency
            self.order.retain(|k| *k != key);
            self.order.push(key);
        }
        self.entries.get(&key).unwrap().clone()
    }

    pub fn invalidate_book(&mut self, book_id: u64) {
        self.entries.retain(|k, _| k.0 != book_id);
        self.order.retain(|k| k.0 != book_id);
    }
}

/// Very small hyphenation-lite helper: given a long word and an available
/// width, decide a break point using simple vowel/consonant heuristics
/// (not a real dictionary — good enough to avoid ugly overflow on long
/// technical terms in narrow columns).
pub fn soft_hyphenate_point(word: &str, max_chars: usize) -> Option<usize> {
    if word.chars().count() <= max_chars || max_chars < 3 {
        return None;
    }
    let chars: Vec<char> = word.chars().collect();
    let mut best = None;
    for i in 2..chars.len().saturating_sub(2).min(max_chars) {
        let c = chars[i];
        let prev = chars[i - 1];
        let is_vowel = |c: char| "aeiouAEIOU".contains(c);
        if !is_vowel(c) && is_vowel(prev) {
            best = Some(i);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Block, BookContent, Span};

    fn sample_content(paragraphs: usize, words_each: usize) -> BookContent {
        let mut blocks = Vec::new();
        for p in 0..paragraphs {
            let text = (0..words_each)
                .map(|w| format!("word{p}_{w}"))
                .collect::<Vec<_>>()
                .join(" ");
            blocks.push(Block::Paragraph {
                spans: vec![Span::plain(text)],
            });
        }
        BookContent {
            blocks,
            toc: vec![],
            images: vec![],
            footnotes: vec![],
            index_terms: vec![],
            raster: None,
            page_labels: vec![],
        }
    }

    #[test]
    fn paginate_produces_pages_covering_all_blocks() {
        let content = sample_content(20, 30);
        let measurer = ApproxMeasurer::default();
        let typo = Typography::default();
        let result = paginate(&content, &measurer, &typo, 400.0, 500.0);
        assert!(!result.pages.is_empty());
        // every block should be reachable via page_of_block
        for bi in 0..content.blocks.len() {
            assert!(result.page_of_block.contains_key(&bi));
        }
        // pages should be non-decreasing in block coverage
        let mut last_block = 0usize;
        for page in &result.pages {
            if let Some(first) = page.lines.first() {
                assert!(first.block >= last_block);
                last_block = first.block;
            }
        }
    }

    #[test]
    fn cache_reuses_identical_key() {
        let content = sample_content(5, 20);
        let measurer = ApproxMeasurer::default();
        let typo = Typography::default();
        let mut cache = PaginationCache::default();
        let a = cache
            .get_or_compute(1, &content, &measurer, &typo, 300.0, 400.0)
            .pages
            .len();
        let b = cache
            .get_or_compute(1, &content, &measurer, &typo, 300.0, 400.0)
            .pages
            .len();
        assert_eq!(a, b);
        assert_eq!(cache.entries.len(), 1);
    }

    #[test]
    fn hyphenation_hint_reasonable() {
        assert_eq!(soft_hyphenate_point("hi", 10), None);
        let p = soft_hyphenate_point("extraordinary", 8);
        assert!(p.is_some());
    }
}
