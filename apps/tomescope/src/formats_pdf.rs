//! formats_pdf — PDF via lopdf.
//!
//! Two lanes:
//!   * text lane (always): per-page text extraction → Paragraph blocks with a
//!     `PageBreak` between source pages and page labels preserved. Heading
//!     heuristics promote short ALL-CAPS / numbered lines so textbooks get a
//!     usable synthetic TOC even without an outline.
//!   * raster lane (feature `poppler-cli`): shell out to `pdftoppm` for page
//!     images so scanned textbooks are readable page-by-page. Off by default;
//!     without it a scanned PDF opens with a "no text layer" placeholder.
//!
//! Real outlines (bookmarks) are read from the catalog when present and win
//! over the heuristic TOC.

use crate::content::{Block, BookContent, Span, SpanStyle, TocEntry};
use crate::formats::{Format, LoadedBook, RasterDoc, RasterPage};
use anyhow::{Context, Result};
use lopdf::{Document, Object, ObjectId};
use std::collections::HashMap;
use std::path::Path;

pub fn open_pdf(path: &Path) -> Result<LoadedBook> {
    let doc = Document::load(path).with_context(|| format!("lopdf load {}", path.display()))?;
    let mut book = LoadedBook::bare(Format::Pdf, stem_of(path));
    read_info(&doc, &mut book);

    let pages = doc.get_pages(); // BTreeMap<u32 pageno, ObjectId>
    let page_ids: Vec<(u32, ObjectId)> = pages.iter().map(|(n, id)| (*n, *id)).collect();
    let labels = page_labels(&doc, page_ids.len());

    let mut empty_pages = 0usize;
    for (i, (page_no, _page_id)) in page_ids.iter().enumerate() {
        let label = labels.get(i).cloned().unwrap_or_else(|| page_no.to_string());
        let break_block = book.content.blocks.len();
        book.content.blocks.push(Block::PageBreak { label: Some(label.clone()) });
        book.content.page_labels.push((break_block, label));

        let text = doc.extract_text(&[*page_no]).unwrap_or_default();
        let text = normalize_pdf_text(&text);
        if text.trim().is_empty() {
            empty_pages += 1;
            continue;
        }
        for para in split_paragraphs(&text) {
            if let Some(level) = heading_guess(&para) {
                let bi = book.content.blocks.len();
                book.content.toc.push(TocEntry { title: para.clone(), depth: level - 1, block: bi });
                book.content.blocks.push(Block::Heading {
                    level: level.min(6) as u8,
                    spans: vec![Span::styled(para, SpanStyle::bold())],
                    anchor: None,
                });
            } else {
                book.content.blocks.push(Block::Paragraph { spans: vec![Span::plain(para)] });
            }
        }
    }

    // Real outline replaces the heuristic TOC when available.
    let outline = read_outline(&doc, &page_ids);
    if !outline.is_empty() {
        book.content.toc.clear();
        // Map page number → the block index of its PageBreak.
        let page_break_blocks: Vec<usize> = book
            .content
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| matches!(b, Block::PageBreak { .. }).then_some(i))
            .collect();
        for (title, page_idx, depth) in outline {
            if let Some(&block) = page_break_blocks.get(page_idx) {
                book.content.toc.push(TocEntry { title, depth, block });
            }
        }
        book.content.toc.sort_by_key(|t| t.block);
    }

    let no_text_layer = empty_pages == page_ids.len();
    if no_text_layer {
        book.content.blocks.clear();
        book.content.toc.clear();
        book.content.page_labels.clear();
    }

    // Raster lane.
    #[allow(unused_mut)]
    let mut raster: Option<RasterDoc> = None;
    #[cfg(feature = "poppler-cli")]
    {
        raster = rasterize_with_poppler(path, page_ids.len()).ok();
    }
    if no_text_layer && raster.is_none() {
        book.content.blocks.push(Block::Heading {
            level: 2,
            spans: vec![Span::plain("Scanned PDF — no text layer")],
            anchor: None,
        });
        book.content.blocks.push(Block::Paragraph {
            spans: vec![Span::plain(
                "This PDF has no extractable text (it is likely a scan). Build tomescope with the \
                 `poppler-cli` feature and install poppler-utils to view rasterized pages, or run \
                 OCR on the file first.",
            )],
        });
    }
    book.content.raster = raster;

    Ok(book)
}

/// Metadata + page count only — cheap for library scans.
pub fn probe_pdf(path: &Path) -> Result<LoadedBook> {
    let doc = Document::load(path)?;
    let mut book = LoadedBook::bare(Format::Pdf, stem_of(path));
    read_info(&doc, &mut book);
    let pages = doc.get_pages().len();
    book.description
        .get_or_insert_with(|| format!("PDF • {pages} pages"));
    Ok(book)
}

fn read_info(doc: &Document, book: &mut LoadedBook) {
    let Some(info_ref) = doc.trailer.get(b"Info").ok() else { return };
    let info = match info_ref {
        Object::Reference(id) => doc.get_object(*id).ok(),
        o => Some(o),
    };
    let Some(Object::Dictionary(dict)) = info else { return };
    let get = |k: &[u8]| -> Option<String> {
        dict.get(k).ok().and_then(|o| match o {
            Object::String(bytes, _) => Some(decode_pdf_string(bytes)),
            _ => None,
        })
    };
    if let Some(t) = get(b"Title") {
        if !t.trim().is_empty() {
            book.title = t;
        }
    }
    if let Some(a) = get(b"Author") {
        book.authors = a
            .split(|c| c == ';' || c == ',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Some(s) = get(b"Subject") {
        book.description = Some(s);
    }
    if let Some(d) = get(b"CreationDate") {
        // D:YYYYMMDD…
        let digits: String = d.chars().filter(|c| c.is_ascii_digit()).collect();
        book.year = digits.get(0..4).and_then(|y| y.parse().ok());
    }
}

fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        // UTF-16BE
        let mut s = String::new();
        let mut i = 2;
        while i + 1 < bytes.len() {
            let u = u16::from_be_bytes([bytes[i], bytes[i + 1]]);
            if let Some(c) = char::from_u32(u as u32) {
                s.push(c);
            }
            i += 2;
        }
        s
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Join hyphen-broken words and drop lone form feeds; keep line structure for
/// the paragraph splitter.
fn normalize_pdf_text(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut lines = t.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim_end_matches('\u{c}');
        if let Some(stripped) = line.strip_suffix('-') {
            // de-hyphenate across the break if the next line starts lowercase
            if lines.peek().map(|n| n.chars().next().map(|c| c.is_lowercase()).unwrap_or(false)).unwrap_or(false) {
                out.push_str(stripped);
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Blank-line paragraphs; consecutive short lines merge (PDF extraction emits
/// one line per text run).
fn split_paragraphs(t: &str) -> Vec<String> {
    let mut paras: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in t.lines() {
        let l = line.trim();
        if l.is_empty() {
            if !cur.trim().is_empty() {
                paras.push(std::mem::take(&mut cur).trim().to_string());
            }
            continue;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(l);
        // Sentence-ending short lines end a paragraph too — catches PDFs with
        // no blank lines at all.
        if l.len() < 45 && (l.ends_with('.') || l.ends_with(':') || l.ends_with('?') || l.ends_with('!')) {
            paras.push(std::mem::take(&mut cur).trim().to_string());
        }
    }
    if !cur.trim().is_empty() {
        paras.push(cur.trim().to_string());
    }
    paras
}

/// Heuristic heading detector for synthetic TOCs: "Chapter 3", "3.2 Title",
/// short ALL CAPS lines. Returns depth 1..=3.
fn heading_guess(p: &str) -> Option<usize> {
    let t = p.trim();
    if t.len() > 80 || t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("chapter ") || lower.starts_with("part ") || lower.starts_with("appendix") {
        return Some(1);
    }
    // "3.2 Something" / "12.4.1 Something"
    let head: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if head.len() >= 1 && head.chars().any(|c| c.is_ascii_digit()) && t.len() > head.len() {
        let dots = head.matches('.').count();
        if t.as_bytes().get(head.len()) == Some(&b' ') && !t.ends_with('.') {
            return Some((dots + 1).min(3));
        }
    }
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() >= 4 && letters.iter().all(|c| c.is_uppercase()) && t.split_whitespace().count() <= 8 {
        return Some(2);
    }
    None
}

/// /PageLabels number tree → a display label per page index ("iv", "3", "A-1").
fn page_labels(doc: &Document, page_count: usize) -> Vec<String> {
    let mut out: Vec<String> = (1..=page_count).map(|n| n.to_string()).collect();
    let Ok(catalog) = doc.catalog() else { return out };
    let Ok(labels_obj) = catalog.get(b"PageLabels") else { return out };
    let labels = resolve(doc, labels_obj);
    let Some(Object::Dictionary(dict)) = labels else { return out };
    let Ok(nums_obj) = dict.get(b"Nums") else { return out };
    let Some(Object::Array(nums)) = resolve(doc, nums_obj) else { return out };
    // pairs of (start_index, spec dict)
    let mut ranges: Vec<(usize, String, i64, Option<String>)> = Vec::new(); // (start, style, first, prefix)
    let mut i = 0;
    while i + 1 < nums.len() {
        let start = match &nums[i] {
            Object::Integer(v) => *v as usize,
            _ => break,
        };
        if let Some(Object::Dictionary(spec)) = resolve(doc, &nums[i + 1]) {
            let style = spec
                .get(b"S")
                .ok()
                .and_then(|o| match o {
                    Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
                    _ => None,
                })
                .unwrap_or_else(|| "D".into());
            let first = spec
                .get(b"St")
                .ok()
                .and_then(|o| match o {
                    Object::Integer(v) => Some(*v),
                    _ => None,
                })
                .unwrap_or(1);
            let prefix = spec.get(b"P").ok().and_then(|o| match o {
                Object::String(b, _) => Some(decode_pdf_string(b)),
                _ => None,
            });
            ranges.push((start, style, first, prefix));
        }
        i += 2;
    }
    for (ri, (start, style, first, prefix)) in ranges.iter().enumerate() {
        let end = ranges.get(ri + 1).map(|r| r.0).unwrap_or(page_count);
        for (offset, slot) in out.iter_mut().enumerate().take(end).skip(*start) {
            let n = first + (offset - start) as i64;
            let body = match style.as_str() {
                "r" => roman(n as u32).to_lowercase(),
                "R" => roman(n as u32),
                "a" => alpha(n as u32).to_lowercase(),
                "A" => alpha(n as u32),
                _ => n.to_string(),
            };
            *slot = format!("{}{}", prefix.clone().unwrap_or_default(), body);
        }
    }
    out
}

fn roman(mut n: u32) -> String {
    const T: [(u32, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: u32) -> String {
    // 1→A, 26→Z, 27→AA …
    let mut n = n;
    let mut s = String::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        s.insert(0, (b'A' + r) as char);
        n = (n - 1) / 26;
    }
    s
}

/// Outline (bookmarks): walk First/Next chains; resolve Dest / GoTo actions to
/// a page index by matching the destination page object id.
fn read_outline(doc: &Document, page_ids: &[(u32, ObjectId)]) -> Vec<(String, usize, u8)> {
    let mut out = Vec::new();
    let Ok(catalog) = doc.catalog() else { return out };
    let Ok(outlines_obj) = catalog.get(b"Outlines") else { return out };
    let Some(Object::Dictionary(outlines)) = resolve(doc, outlines_obj) else { return out };
    let page_index: HashMap<ObjectId, usize> =
        page_ids.iter().enumerate().map(|(i, (_, id))| (*id, i)).collect();
    let first = outlines.get(b"First").ok().cloned();
    if let Some(first) = first {
        walk_outline(doc, &first, 0, &page_index, &mut out, 0);
    }
    out
}

fn walk_outline(
    doc: &Document,
    node_obj: &Object,
    depth: u8,
    page_index: &HashMap<ObjectId, usize>,
    out: &mut Vec<(String, usize, u8)>,
    guard: usize,
) {
    if guard > 2000 || depth > 6 {
        return;
    }
    let mut cur = Some(node_obj.clone());
    let mut steps = 0usize;
    while let Some(obj) = cur {
        steps += 1;
        if steps > 2000 {
            break;
        }
        let Some(Object::Dictionary(dict)) = resolve(doc, &obj) else { break };
        let title = dict
            .get(b"Title")
            .ok()
            .and_then(|o| match resolve(doc, o) {
                Some(Object::String(b, _)) => Some(decode_pdf_string(&b)),
                _ => None,
            })
            .unwrap_or_default();
        let dest_page = outline_dest_page(doc, &dict, page_index);
        if let Some(p) = dest_page {
            if !title.trim().is_empty() {
                out.push((title.trim().to_string(), p, depth));
            }
        }
        if let Ok(first) = dict.get(b"First") {
            walk_outline(doc, first, depth + 1, page_index, out, guard + steps);
        }
        cur = dict.get(b"Next").ok().cloned();
    }
}

fn outline_dest_page(
    doc: &Document,
    dict: &lopdf::Dictionary,
    page_index: &HashMap<ObjectId, usize>,
) -> Option<usize> {
    // Either /Dest directly, or /A << /S /GoTo /D dest >>
    let dest = dict
        .get(b"Dest")
        .ok()
        .cloned()
        .or_else(|| {
            let a = resolve(doc, dict.get(b"A").ok()?)?;
            if let Object::Dictionary(a) = a {
                a.get(b"D").ok().cloned()
            } else {
                None
            }
        })?;
    let dest = resolve(doc, &dest)?;
    let arr = match dest {
        Object::Array(a) => a,
        // Named destinations unsupported here — skip quietly.
        _ => return None,
    };
    match arr.first()? {
        Object::Reference(id) => page_index.get(id).copied(),
        Object::Integer(i) => Some((*i).max(0) as usize),
        _ => None,
    }
}

fn resolve(doc: &Document, o: &Object) -> Option<Object> {
    match o {
        Object::Reference(id) => doc.get_object(*id).ok().cloned(),
        other => Some(other.clone()),
    }
}

#[cfg(feature = "poppler-cli")]
fn rasterize_with_poppler(path: &Path, pages: usize) -> Result<RasterDoc> {
    use std::process::Command;
    let tmp = std::env::temp_dir().join(format!("tomescope-raster-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let status = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg("110")
        .arg(path)
        .arg(tmp.join("p"))
        .status()
        .context("pdftoppm not found — install poppler-utils")?;
    anyhow::ensure!(status.success(), "pdftoppm failed");
    let mut out = Vec::with_capacity(pages);
    let mut entries: Vec<_> = std::fs::read_dir(&tmp)?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.extension().map(|e| e == "png").unwrap_or(false) {
            out.push(RasterPage {
                name: p.file_name().unwrap().to_string_lossy().into_owned(),
                bytes: std::fs::read(&p)?,
            });
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    anyhow::ensure!(!out.is_empty(), "no pages rasterized");
    Ok(RasterDoc { pages: out, rtl: false })
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into())
}

// Silence unused import warnings when the poppler feature is off.
#[allow(unused_imports)]
use crate::formats::RasterPage as _RasterPageUse;
#[allow(unused_imports)]
use crate::content::BookContent as _BookContentUse;
