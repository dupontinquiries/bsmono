//! formats_cbz — the image-first comic container (CBZ) and, because they're
//! both "one more small parser", FB2 (FictionBook 2 XML).
//!
//! CBZ: natural-sort the image entries (2 < 10), keep encoded bytes for lazy
//! texture decode, sniff manga right-to-left from a ComicInfo.xml if present.
//! FB2: sections → headings/paragraphs, binary elements → images, with the
//! usual emphasis tags mapped onto SpanStyle.

use crate::content::{Block, BookImage, Span, SpanStyle, TocEntry};
use crate::formats::{decode_entities, Format, LoadedBook, RasterDoc, RasterPage};
use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;
use std::path::Path;

// ---------------------------------------------------------------------------
// CBZ / CBT
// ---------------------------------------------------------------------------

pub fn open_cbz(path: &Path) -> Result<LoadedBook> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).context("not a zip archive")?;
    let mut names: Vec<String> = Vec::new();
    let mut comic_info: Option<String> = None;
    for i in 0..zip.len() {
        let e = zip.by_index(i)?;
        let name = e.name().to_string();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png") || lower.ends_with(".webp") || lower.ends_with(".gif")
        {
            if !lower.contains("__macosx") {
                names.push(name);
            }
        } else if lower.ends_with("comicinfo.xml") {
            comic_info = Some(name);
        }
    }
    if names.is_empty() {
        bail!("no images in archive");
    }
    natural_sort(&mut names);

    let mut pages = Vec::with_capacity(names.len());
    for name in &names {
        let mut e = zip.by_name(name)?;
        let mut bytes = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut bytes)?;
        pages.push(RasterPage { name: name.clone(), bytes });
    }

    let mut book = LoadedBook::bare(Format::Cbz, stem_of(path));
    let mut rtl = false;
    if let Some(ci) = comic_info {
        if let Ok(mut e) = zip.by_name(&ci) {
            let mut xml = String::new();
            let _ = e.read_to_string(&mut xml);
            apply_comic_info(&xml, &mut book, &mut rtl);
        }
    }
    // Cover = first page.
    if let Some(first) = pages.first() {
        book.cover = Some(BookImage { name: first.name.clone(), bytes: first.bytes.clone(), size_hint: None });
    }
    book.content.raster = Some(RasterDoc { pages, rtl });
    Ok(book)
}

fn apply_comic_info(xml: &str, book: &mut LoadedBook, rtl: &mut bool) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut target: Option<String> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                target = Some(String::from_utf8_lossy(e.local_name().as_ref()).into_owned());
            }
            Ok(Event::Text(t)) => {
                if let Some(k) = target.take() {
                    let v = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    match k.as_str() {
                        "Title" => book.title = v,
                        "Series" => {
                            let idx = book.series.as_ref().map(|s| s.1).unwrap_or(1.0);
                            book.series = Some((v, idx));
                        }
                        "Number" => {
                            if let Ok(n) = v.parse::<f32>() {
                                let name =
                                    book.series.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| book.title.clone());
                                book.series = Some((name, n));
                            }
                        }
                        "Writer" | "Penciller" => book.authors.push(v),
                        "Publisher" => book.publisher = Some(v),
                        "Year" => book.year = v.parse().ok(),
                        "Summary" => book.description = Some(v),
                        "Manga" => *rtl = v.eq_ignore_ascii_case("yesandrighttoleft") || v.eq_ignore_ascii_case("yes"),
                        "LanguageISO" => book.language = Some(v),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(_)) => target = None,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// "p2" < "p10": split into digit/non-digit runs and compare runs.
pub fn natural_sort(names: &mut [String]) {
    names.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));
}

fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut key = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n = 0u64;
            while let Some(&d) = chars.peek() {
                if let Some(v) = d.to_digit(10) {
                    n = n.saturating_mul(10).saturating_add(v as u64);
                    chars.next();
                } else {
                    break;
                }
            }
            key.push((0, n, String::new()));
        } else {
            let mut run = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    break;
                }
                run.push(d.to_ascii_lowercase());
                chars.next();
            }
            key.push((1, 0, run));
        }
    }
    key
}

// ---------------------------------------------------------------------------
// FB2
// ---------------------------------------------------------------------------

pub fn open_fb2(path: &Path) -> Result<LoadedBook> {
    let xml = std::fs::read_to_string(path).or_else(|_| {
        // Some FB2s are windows-1251; salvage byte-wise.
        std::fs::read(path).map(|b| b.iter().map(|&x| x as char).collect::<String>())
    })?;
    let mut book = LoadedBook::bare(Format::Fb2, stem_of(path));
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    let mut style = SpanStyle::PLAIN;
    let mut style_stack: Vec<SpanStyle> = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut section_depth: u8 = 0;
    let mut in_title = false;
    let mut in_body = false;
    let mut in_binary: Option<(String, String)> = None; // (id, b64 buffer)
    let mut title_first_name = String::new();
    let mut title_last_name = String::new();
    let mut meta_target: Option<String> = None;
    let mut cover_id: Option<String> = None;

    macro_rules! flush_para {
        ($book:ident, $spans:ident, $in_title:ident, $section_depth:ident) => {
            if $spans.iter().any(|s: &Span| !s.is_whitespace()) {
                let taken = std::mem::take(&mut $spans);
                if $in_title {
                    let title = crate::content::join_spans(&taken);
                    let bi = $book.content.blocks.len();
                    $book.content.toc.push(TocEntry {
                        title,
                        depth: $section_depth.saturating_sub(1),
                        block: bi,
                    });
                    $book.content.blocks.push(Block::Heading {
                        level: $section_depth.max(1).min(6),
                        spans: taken,
                        anchor: None,
                    });
                } else {
                    $book.content.blocks.push(Block::Paragraph { spans: taken });
                }
            } else {
                $spans.clear();
            }
        };
    }

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"body" => in_body = true,
                    b"section" => section_depth += 1,
                    b"title" => in_title = true,
                    b"p" | b"v" | b"subtitle" => { /* paragraph collects below */ }
                    b"emphasis" => {
                        style_stack.push(style);
                        style.italic = true;
                    }
                    b"strong" => {
                        style_stack.push(style);
                        style.bold = true;
                    }
                    b"code" => {
                        style_stack.push(style);
                        style.mono = true;
                    }
                    b"strikethrough" => {
                        style_stack.push(style);
                        style.strike = true;
                    }
                    b"sup" => {
                        style_stack.push(style);
                        style.sup = true;
                    }
                    b"sub" => {
                        style_stack.push(style);
                        style.sub = true;
                    }
                    b"cite" | b"epigraph" => {
                        style_stack.push(style);
                        style.italic = true;
                    }
                    b"binary" => {
                        let id = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.local_name().as_ref() == b"id")
                            .map(|a| String::from_utf8_lossy(&a.value).into_owned())
                            .unwrap_or_default();
                        in_binary = Some((id, String::new()));
                    }
                    b"first-name" => meta_target = Some("first".into()),
                    b"last-name" => meta_target = Some("last".into()),
                    b"book-title" => meta_target = Some("title".into()),
                    b"lang" => meta_target = Some("lang".into()),
                    b"year" => meta_target = Some("year".into()),
                    b"publisher" if !in_body => meta_target = Some("publisher".into()),
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_vec();
                if name.as_slice() == b"image" {
                    let href = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == b"href")
                        .map(|a| String::from_utf8_lossy(&a.value).into_owned());
                    if let Some(h) = href {
                        let id = h.trim_start_matches('#').to_string();
                        if !in_body {
                            cover_id = Some(id);
                        } else {
                            // Placeholder — patched to real index in the fixup
                            // pass once binaries are decoded.
                            flush_para!(book, spans, in_title, section_depth);
                            book.content.blocks.push(Block::Image {
                                image: usize::MAX,
                                caption: vec![Span::plain(id)],
                            });
                        }
                    }
                }
                if name.as_slice() == b"empty-line" {
                    flush_para!(book, spans, in_title, section_depth);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some((_, b64)) = in_binary.as_mut() {
                    b64.push_str(&String::from_utf8_lossy(t.as_ref()));
                } else if let Some(k) = meta_target.as_deref() {
                    let v = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    let v = v.trim().to_string();
                    if !v.is_empty() {
                        match k {
                            "first" => title_first_name = v,
                            "last" => title_last_name = v,
                            "title" => book.title = v,
                            "lang" => book.language = Some(v),
                            "year" => book.year = v.parse().ok(),
                            "publisher" => book.publisher = Some(v),
                            _ => {}
                        }
                    }
                } else if in_body {
                    let v = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    if !v.is_empty() {
                        spans.push(Span::styled(decode_entities(&v), style));
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"body" => in_body = false,
                    b"section" => section_depth = section_depth.saturating_sub(1),
                    b"title" => {
                        flush_para!(book, spans, in_title, section_depth);
                        in_title = false;
                    }
                    b"p" | b"v" | b"subtitle" => flush_para!(book, spans, in_title, section_depth),
                    b"emphasis" | b"strong" | b"code" | b"strikethrough" | b"sup" | b"sub" | b"cite"
                    | b"epigraph" => {
                        style = style_stack.pop().unwrap_or(SpanStyle::PLAIN);
                    }
                    b"binary" => {
                        if let Some((id, b64)) = in_binary.take() {
                            if let Some(bytes) = b64_decode(&b64) {
                                book.content.images.push(BookImage { name: id, bytes, size_hint: None });
                            }
                        }
                    }
                    b"first-name" | b"last-name" | b"book-title" | b"lang" | b"year" | b"publisher" => {
                        meta_target = None;
                        if !title_first_name.is_empty() || !title_last_name.is_empty() {
                            let full = format!("{} {}", title_first_name, title_last_name).trim().to_string();
                            if !full.is_empty() && !book.authors.contains(&full) {
                                book.authors.push(full);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    // Fixup: image placeholders reference binaries by id in the caption.
    let ids: Vec<String> = book.content.images.iter().map(|i| i.name.clone()).collect();
    for b in &mut book.content.blocks {
        if let Block::Image { image, caption } = b {
            if *image == usize::MAX {
                let want = crate::content::join_spans(caption);
                if let Some(idx) = ids.iter().position(|n| *n == want) {
                    *image = idx;
                    caption.clear();
                } else {
                    *b = Block::Paragraph { spans: vec![Span::plain(format!("[image {want}]"))] };
                }
            }
        }
    }
    if let Some(cid) = cover_id {
        if let Some(img) = book.content.images.iter().find(|i| i.name == cid) {
            book.cover = Some(img.clone());
        }
    }
    if book.content.blocks.is_empty() {
        bail!("fb2 had no body content");
    }
    Ok(book)
}

/// Minimal base64 (standard alphabet, ignores whitespace / padding).
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c.is_ascii_whitespace() || c == b'=' {
            continue;
        }
        let v = val(c)?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["p10.png".to_string(), "p2.png".to_string(), "p1.png".to_string()];
        natural_sort(&mut v);
        assert_eq!(v, vec!["p1.png", "p2.png", "p10.png"]);
    }

    #[test]
    fn b64_roundtrip_ascii() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVs\nbG8=").unwrap(), b"hello");
    }
}
