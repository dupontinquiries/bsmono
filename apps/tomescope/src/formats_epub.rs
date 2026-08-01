//! formats_epub — EPUB 2/3 via zip + quick-xml + the shared mini-HTML walker.
//!
//! Pipeline: container.xml → OPF path → metadata + manifest + spine →
//! chapters parsed in spine order into one flat block list (with PageBreak
//! hints at spine boundaries) → TOC from the EPUB3 nav doc or the EPUB2 NCX,
//! anchors resolved to block indices where possible.

use crate::content::{Block, BookImage, TocEntry};
use crate::formats::{parse_html_fragment, Format, HtmlSink, LoadedBook};
use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

pub fn open_epub(path: &Path) -> Result<LoadedBook> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).context("not a zip")?;
    let opf_path = container_opf_path(&mut zip)?;
    let opf_xml = read_entry(&mut zip, &opf_path)?;
    let opf_dir = parent_of(&opf_path);
    let opf = parse_opf(&opf_xml)?;

    let mut book = LoadedBook::bare(Format::Epub, opf.title.clone().unwrap_or_else(|| stem_of(path)));
    book.authors = opf.authors.clone();
    book.language = opf.language.clone();
    book.publisher = opf.publisher.clone();
    book.year = opf.date.as_deref().and_then(year_of);
    book.isbn = opf.isbn.clone();
    book.description = opf.description.clone();
    book.series = opf.series.clone();

    // Cover: explicit meta cover id, else an item with properties="cover-image",
    // else a manifest item literally named cover.*
    let cover_href = opf
        .cover_id
        .as_ref()
        .and_then(|id| opf.manifest.get(id))
        .cloned()
        .or_else(|| opf.cover_image_href.clone())
        .or_else(|| {
            opf.manifest
                .values()
                .find(|href| {
                    let h = href.to_ascii_lowercase();
                    h.contains("cover") && (h.ends_with(".jpg") || h.ends_with(".jpeg") || h.ends_with(".png"))
                })
                .cloned()
        });
    if let Some(href) = cover_href {
        let full = join_path(&opf_dir, &href);
        if let Ok(bytes) = read_entry_bytes(&mut zip, &full) {
            book.cover = Some(BookImage { name: href, bytes, size_hint: None });
        }
    }

    // Anchor bookkeeping: map "chapterfile#id" and "chapterfile" → block index.
    let mut anchor_to_block: HashMap<String, usize> = HashMap::new();

    for (spine_no, idref) in opf.spine.iter().enumerate() {
        let Some(href) = opf.manifest.get(idref) else { continue };
        let full = join_path(&opf_dir, href);
        let Ok(html) = read_entry(&mut zip, &full) else { continue };
        let chapter_dir = parent_of(&full);

        if spine_no > 0 {
            book.content.blocks.push(Block::PageBreak { label: None });
        }
        anchor_to_block.insert(file_key(href), book.content.blocks.len());

        let start_block = book.content.blocks.len();
        {
            // Two-phase image handling to keep borrows simple: (1) scan the
            // chapter for src attributes and pull those bytes out of the zip
            // into the book, (2) parse with a closure that only consults the
            // pre-built src → index map.
            let mut src_to_idx: HashMap<String, usize> = HashMap::new();
            for src in scan_img_srcs(&html) {
                let key = join_path(&chapter_dir, &src);
                if src_to_idx.contains_key(&src) {
                    continue;
                }
                if let Ok(bytes) = read_entry_bytes(&mut zip, &key) {
                    let idx = book.content.images.len();
                    book.content.images.push(BookImage { name: src.clone(), bytes, size_hint: None });
                    src_to_idx.insert(src, idx);
                }
            }
            let mut sink = HtmlSink::new(&mut book.content);
            let mut img = |src: &str| -> Option<usize> { src_to_idx.get(src).copied() };
            parse_html_fragment(&html, &mut sink, &mut img);
            sink.finish();
        }

        // Register heading anchors from this chapter as "file#id".
        for (bi, b) in book.content.blocks.iter().enumerate().skip(start_block) {
            if let Block::Heading { anchor: Some(a), .. } = b {
                anchor_to_block.insert(format!("{}#{}", file_key(href), a), bi);
            }
        }
    }

    // TOC: nav doc (EPUB3) first, NCX fallback. Both give (title, href, depth).
    let mut toc_entries: Vec<(String, String, u8)> = Vec::new();
    if let Some(nav_id) = &opf.nav_id {
        if let Some(href) = opf.manifest.get(nav_id) {
            if let Ok(nav) = read_entry(&mut zip, &join_path(&opf_dir, href)) {
                toc_entries = parse_nav_doc(&nav);
            }
        }
    }
    if toc_entries.is_empty() {
        if let Some(ncx_id) = &opf.ncx_id {
            if let Some(href) = opf.manifest.get(ncx_id) {
                if let Ok(ncx) = read_entry(&mut zip, &join_path(&opf_dir, href)) {
                    toc_entries = parse_ncx(&ncx);
                }
            }
        }
    }
    if !toc_entries.is_empty() {
        book.content.toc.clear(); // replace heading-derived toc with the real one
        for (title, href, depth) in toc_entries {
            let key = href.split('#').next().unwrap_or("").to_string();
            let anchored = href.contains('#');
            let block = if anchored {
                anchor_to_block
                    .get(&normalize_href(&href))
                    .or_else(|| anchor_to_block.get(&file_key(&key)))
                    .copied()
            } else {
                anchor_to_block.get(&file_key(&key)).copied()
            };
            if let Some(block) = block {
                book.content.toc.push(TocEntry { title, depth, block });
            }
        }
        book.content.toc.sort_by_key(|t| t.block);
    }

    if book.content.blocks.is_empty() {
        bail!("epub had no readable spine content");
    }
    Ok(book)
}

/// Metadata-only probe: OPF + cover, no chapter parsing. Library scans call this.
pub fn probe_epub(path: &Path) -> Result<LoadedBook> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let opf_path = container_opf_path(&mut zip)?;
    let opf_xml = read_entry(&mut zip, &opf_path)?;
    let opf_dir = parent_of(&opf_path);
    let opf = parse_opf(&opf_xml)?;
    let mut book = LoadedBook::bare(Format::Epub, opf.title.clone().unwrap_or_else(|| stem_of(path)));
    book.authors = opf.authors;
    book.language = opf.language;
    book.publisher = opf.publisher;
    book.year = opf.date.as_deref().and_then(year_of);
    book.isbn = opf.isbn;
    book.description = opf.description;
    book.series = opf.series;
    let cover_href = opf.cover_id.as_ref().and_then(|id| opf.manifest.get(id)).cloned().or(opf.cover_image_href);
    if let Some(href) = cover_href {
        if let Ok(bytes) = read_entry_bytes(&mut zip, &join_path(&opf_dir, &href)) {
            book.cover = Some(BookImage { name: href, bytes, size_hint: None });
        }
    }
    Ok(book)
}

// --- helpers ---------------------------------------------------------------

/// Pre-pass: every src / xlink:href attribute in img/image tags, in order.
fn scan_img_srcs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(pos) = lower[from..].find("<img").map(|o| from + o).or_else(|| {
        lower[from..].find("<image").map(|o| from + o)
    }) {
        let end = lower[pos..].find('>').map(|o| pos + o).unwrap_or(lower.len());
        let tag = &html[pos..end];
        for key in ["src", "xlink:href", "href"] {
            if let Some(v) = extract_attr(tag, key) {
                out.push(v);
                break;
            }
        }
        from = end.min(lower.len().saturating_sub(1)).max(pos + 1);
        if from >= lower.len() {
            break;
        }
    }
    out
}

fn container_opf_path<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<String> {
    let xml = read_entry(zip, "META-INF/container.xml")?;
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"rootfile" {
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"full-path" {
                            return Ok(String::from_utf8_lossy(&attr.value).into_owned());
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    bail!("container.xml missing rootfile")
}

#[derive(Default)]
struct Opf {
    title: Option<String>,
    authors: Vec<String>,
    language: Option<String>,
    publisher: Option<String>,
    date: Option<String>,
    isbn: Option<String>,
    description: Option<String>,
    series: Option<(String, f32)>,
    manifest: HashMap<String, String>, // id -> href
    spine: Vec<String>,                // idrefs in order
    cover_id: Option<String>,
    cover_image_href: Option<String>,
    nav_id: Option<String>,
    ncx_id: Option<String>,
}

fn parse_opf(xml: &str) -> Result<Opf> {
    let mut opf = Opf::default();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut text_target: Option<&'static str> = None;
    let mut series_name: Option<String> = None;
    let mut series_index: Option<f32> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_vec();
                let attr = |k: &str| -> Option<String> {
                    e.attributes().flatten().find_map(|a| {
                        (a.key.local_name().as_ref() == k.as_bytes())
                            .then(|| String::from_utf8_lossy(&a.value).into_owned())
                    })
                };
                match name.as_slice() {
                    b"title" => text_target = Some("title"),
                    b"creator" => text_target = Some("creator"),
                    b"language" => text_target = Some("language"),
                    b"publisher" => text_target = Some("publisher"),
                    b"date" => text_target = Some("date"),
                    b"description" => text_target = Some("description"),
                    b"identifier" => {
                        let scheme = attr("scheme").unwrap_or_default().to_ascii_lowercase();
                        text_target = if scheme.contains("isbn") { Some("isbn") } else { Some("identifier") };
                    }
                    b"meta" => {
                        let mname = attr("name").unwrap_or_default();
                        let prop = attr("property").unwrap_or_default();
                        if mname == "cover" {
                            opf.cover_id = attr("content");
                        }
                        if mname == "calibre:series" {
                            series_name = attr("content");
                        }
                        if mname == "calibre:series_index" {
                            series_index = attr("content").and_then(|v| v.parse().ok());
                        }
                        if prop == "belongs-to-collection" {
                            text_target = Some("series");
                        }
                        if prop == "group-position" {
                            text_target = Some("series_index");
                        }
                    }
                    b"item" => {
                        let id = attr("id").unwrap_or_default();
                        let href = attr("href").unwrap_or_default();
                        let props = attr("properties").unwrap_or_default();
                        let media = attr("media-type").unwrap_or_default();
                        if props.split_whitespace().any(|p| p == "cover-image") {
                            opf.cover_image_href = Some(href.clone());
                        }
                        if props.split_whitespace().any(|p| p == "nav") {
                            opf.nav_id = Some(id.clone());
                        }
                        if media == "application/x-dtbncx+xml" {
                            opf.ncx_id = Some(id.clone());
                        }
                        if !id.is_empty() {
                            opf.manifest.insert(id, href);
                        }
                    }
                    b"itemref" => {
                        if let Some(idref) = attr("idref") {
                            let linear = attr("linear").map(|l| l != "no").unwrap_or(true);
                            if linear {
                                opf.spine.push(idref);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(target) = text_target.take() {
                    let val = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    let val = val.trim().to_string();
                    if val.is_empty() {
                        continue;
                    }
                    match target {
                        "title" => opf.title.get_or_insert(val),
                        "creator" => {
                            opf.authors.push(val);
                            continue;
                        }
                        "language" => opf.language.get_or_insert(val),
                        "publisher" => opf.publisher.get_or_insert(val),
                        "date" => opf.date.get_or_insert(val),
                        "description" => opf.description.get_or_insert(strip_tags(&val)),
                        "isbn" => opf.isbn.get_or_insert(val),
                        "series" => series_name.get_or_insert(val),
                        "series_index" => {
                            series_index = val.parse().ok();
                            continue;
                        }
                        _ => continue,
                    };
                }
            }
            Ok(Event::End(_)) => text_target = None,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if let Some(name) = series_name {
        opf.series = Some((name, series_index.unwrap_or(1.0)));
    }
    Ok(opf)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    crate::formats::decode_entities(out.trim())
}

/// EPUB3 nav doc: nested <ol><li><a href>…; depth from nesting.
fn parse_nav_doc(html: &str) -> Vec<(String, String, u8)> {
    let mut out = Vec::new();
    let mut depth: i32 = -1;
    let mut in_toc_nav = false;
    let mut pending_href: Option<String> = None;
    let mut pending_text = String::new();
    let mut i = 0usize;
    let bytes = html.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let close = match html[i..].find('>') {
                Some(o) => i + o,
                None => break,
            };
            let tag = html[i + 1..close].trim();
            i = close + 1;
            let lower = tag.to_ascii_lowercase();
            if lower.starts_with("nav") {
                in_toc_nav = lower.contains("epub:type=\"toc\"") || lower.contains("epub:type='toc'") || !lower.contains("epub:type");
                depth = -1;
            } else if lower.starts_with("/nav") {
                in_toc_nav = false;
            } else if in_toc_nav && lower.starts_with("ol") {
                depth += 1;
            } else if in_toc_nav && lower.starts_with("/ol") {
                depth -= 1;
            } else if in_toc_nav && lower.starts_with("a ") || in_toc_nav && lower == "a" {
                pending_href = extract_attr(tag, "href");
                pending_text.clear();
            } else if in_toc_nav && lower.starts_with("/a") {
                if let Some(href) = pending_href.take() {
                    let title = crate::formats::decode_entities(pending_text.trim());
                    if !title.is_empty() {
                        out.push((title, href, depth.max(0) as u8));
                    }
                }
            }
        } else {
            let next = html[i..].find('<').map(|o| i + o).unwrap_or(bytes.len());
            pending_text.push_str(&html[i..next]);
            i = next;
        }
    }
    out
}

/// EPUB2 NCX: navPoint nesting with navLabel/text + content src.
fn parse_ncx(xml: &str) -> Vec<(String, String, u8)> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth: i32 = -1;
    let mut label = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"navPoint" => depth += 1,
                b"text" => {
                    in_text = true;
                    label.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"content" {
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"src" {
                            let src = String::from_utf8_lossy(&attr.value).into_owned();
                            if !label.is_empty() {
                                out.push((label.clone(), src, depth.max(0) as u8));
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if in_text {
                    label = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"navPoint" => depth -= 1,
                b"text" => in_text = false,
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn extract_attr(tag: &str, key: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let pos = lower.find(&format!("{key}="))?;
    let rest = &tag[pos + key.len() + 1..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)?;
        Some(crate::formats::decode_entities(&rest[1..1 + end]))
    } else {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<String> {
    Ok(String::from_utf8_lossy(&read_entry_bytes(zip, name)?).into_owned())
}

fn read_entry_bytes<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    // Try exact, then case-insensitive, then URL-decoded.
    let candidates = [name.to_string(), urldecode(name)];
    for cand in &candidates {
        if let Ok(mut e) = zip.by_name(cand) {
            let mut v = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut v)?;
            return Ok(v);
        }
    }
    let lower = name.to_ascii_lowercase();
    let found = (0..zip.len()).find_map(|i| {
        let n = zip.by_index(i).ok()?.name().to_string();
        (n.to_ascii_lowercase() == lower).then_some(n)
    });
    if let Some(n) = found {
        let mut e = zip.by_name(&n)?;
        let mut v = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut v)?;
        return Ok(v);
    }
    bail!("zip entry not found: {name}")
}

fn urldecode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn parent_of(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

fn join_path(dir: &str, rel: &str) -> String {
    let rel = rel.split('#').next().unwrap_or(rel);
    if rel.starts_with('/') {
        return rel.trim_start_matches('/').to_string();
    }
    let mut parts: Vec<&str> = if dir.is_empty() { Vec::new() } else { dir.split('/').collect() };
    for seg in rel.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn file_key(href: &str) -> String {
    href.split('#').next().unwrap_or(href).rsplit('/').next().unwrap_or(href).to_string()
}

fn normalize_href(href: &str) -> String {
    match href.split_once('#') {
        Some((f, a)) => format!("{}#{}", file_key(f), a),
        None => file_key(href),
    }
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".into())
}

fn year_of(date: &str) -> Option<i32> {
    date.get(0..4)?.parse().ok()
}
