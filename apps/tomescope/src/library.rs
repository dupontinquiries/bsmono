//! library — the shelf: scan folders, keep book records + reading state,
//! shelves (manual and smart), sorting, fuzzy search, and the cover cache
//! that decodes image bytes into egui textures on demand.

use crate::content::Locus;
use crate::formats::{self, Format};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Stable book identity: fnv1a of the absolute path + file size, so a moved
/// library re-links by re-scan while an edited file gets a fresh id.
pub fn book_id(path: &Path, size: u64) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= size;
    h.wrapping_mul(0x100000001b3)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BookRecord {
    pub id: u64,
    pub path: PathBuf,
    pub format: Format,
    pub title: String,
    pub authors: Vec<String>,
    pub series: Option<(String, f32)>,
    pub year: Option<i32>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub file_size: u64,
    pub added_unix: i64,
    /// Cached cover bytes on disk (under the covers dir), if extracted.
    pub cover_file: Option<PathBuf>,
    // --- reading state ---
    pub last_open_unix: Option<i64>,
    pub position: Locus,
    /// For raster books, the page index instead.
    pub raster_page: usize,
    pub percent: f32,
    pub finished: bool,
    pub rating: u8, // 0..=5
}

impl BookRecord {
    pub fn author_line(&self) -> String {
        if self.authors.is_empty() {
            "unknown author".into()
        } else {
            self.authors.join(", ")
        }
    }
    pub fn series_line(&self) -> Option<String> {
        self.series.as_ref().map(|(n, i)| {
            if (i.fract()).abs() < f32::EPSILON {
                format!("{n} #{}", *i as i64)
            } else {
                format!("{n} #{i}")
            }
        })
    }
    pub fn matches_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    RecentlyRead,
    RecentlyAdded,
    Title,
    Author,
    Series,
    Progress,
    Year,
}

impl SortKey {
    pub fn all() -> [SortKey; 7] {
        [
            SortKey::RecentlyRead,
            SortKey::RecentlyAdded,
            SortKey::Title,
            SortKey::Author,
            SortKey::Series,
            SortKey::Progress,
            SortKey::Year,
        ]
    }
    pub fn label(self) -> &'static str {
        match self {
            SortKey::RecentlyRead => "recently read",
            SortKey::RecentlyAdded => "recently added",
            SortKey::Title => "title",
            SortKey::Author => "author",
            SortKey::Series => "series",
            SortKey::Progress => "progress",
            SortKey::Year => "year",
        }
    }
}

/// A manual shelf: named list of book ids.
#[derive(Clone, Serialize, Deserialize)]
pub struct Shelf {
    pub name: String,
    pub books: Vec<u64>,
}

/// A smart shelf: a saved filter that evaluates live.
#[derive(Clone, Serialize, Deserialize)]
pub struct SmartShelf {
    pub name: String,
    pub rule: SmartRule,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum SmartRule {
    Unread,
    InProgress,
    Finished,
    FormatIs(Format),
    TagIs(String),
    AuthorContains(String),
    RatedAtLeast(u8),
    SeriesAny,
}

impl SmartRule {
    pub fn accepts(&self, b: &BookRecord) -> bool {
        match self {
            SmartRule::Unread => b.percent < 0.005 && !b.finished,
            SmartRule::InProgress => b.percent >= 0.005 && !b.finished,
            SmartRule::Finished => b.finished,
            SmartRule::FormatIs(f) => b.format == *f,
            SmartRule::TagIs(t) => b.matches_tag(t),
            SmartRule::AuthorContains(a) => {
                let a = a.to_lowercase();
                b.authors.iter().any(|x| x.to_lowercase().contains(&a))
            }
            SmartRule::RatedAtLeast(r) => b.rating >= *r,
            SmartRule::SeriesAny => b.series.is_some(),
        }
    }
    pub fn describe(&self) -> String {
        match self {
            SmartRule::Unread => "not started".into(),
            SmartRule::InProgress => "in progress".into(),
            SmartRule::Finished => "finished".into(),
            SmartRule::FormatIs(f) => format!("format = {}", f.label()),
            SmartRule::TagIs(t) => format!("tag = {t}"),
            SmartRule::AuthorContains(a) => format!("author ~ {a}"),
            SmartRule::RatedAtLeast(r) => format!("rating ≥ {r}"),
            SmartRule::SeriesAny => "part of a series".into(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    pub books: Vec<BookRecord>,
    pub shelves: Vec<Shelf>,
    pub smart_shelves: Vec<SmartShelf>,
    pub scan_roots: Vec<PathBuf>,
    #[serde(default)]
    pub sort: Option<SortKey>,
}

pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub errors: Vec<(PathBuf, String)>,
}

impl Library {
    pub fn get(&self, id: u64) -> Option<&BookRecord> {
        self.books.iter().find(|b| b.id == id)
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut BookRecord> {
        self.books.iter_mut().find(|b| b.id == id)
    }

    pub fn default_smart_shelves() -> Vec<SmartShelf> {
        vec![
            SmartShelf { name: "Reading now".into(), rule: SmartRule::InProgress },
            SmartShelf { name: "To read".into(), rule: SmartRule::Unread },
            SmartShelf { name: "Finished".into(), rule: SmartRule::Finished },
            SmartShelf { name: "Textbooks (PDF)".into(), rule: SmartRule::FormatIs(Format::Pdf) },
            SmartShelf { name: "Comics".into(), rule: SmartRule::FormatIs(Format::Cbz) },
            SmartShelf { name: "Series".into(), rule: SmartRule::SeriesAny },
        ]
    }

    /// Walk every scan root, probing new files and re-linking known ones.
    pub fn scan(&mut self, covers_dir: &Path) -> ScanReport {
        let mut report = ScanReport { added: 0, updated: 0, missing: 0, errors: Vec::new() };
        let mut seen: HashMap<PathBuf, u64> = HashMap::new();
        let roots = self.scan_roots.clone();
        for root in roots {
            let mut stack = vec![root];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if !name.starts_with('.') && name != "node_modules" && name != "target" {
                            stack.push(path);
                        }
                        continue;
                    }
                    if Format::from_ext(&path).is_none() {
                        continue;
                    }
                    let Ok(meta) = entry.metadata() else { continue };
                    let size = meta.len();
                    let id = book_id(&path, size);
                    seen.insert(path.clone(), id);
                    if self.get(id).is_some() {
                        continue;
                    }
                    match formats::probe(&path) {
                        Ok(probe) => {
                            let cover_file = probe.cover.as_ref().and_then(|c| {
                                let ext = if c.bytes.starts_with(&[0x89, b'P']) { "png" } else { "jpg" };
                                let file = covers_dir.join(format!("{id:016x}.{ext}"));
                                std::fs::create_dir_all(covers_dir).ok()?;
                                std::fs::write(&file, &c.bytes).ok()?;
                                Some(file)
                            });
                            self.books.push(BookRecord {
                                id,
                                path: path.clone(),
                                format: probe.format,
                                title: probe.title,
                                authors: probe.authors,
                                series: probe.series,
                                year: probe.year,
                                language: probe.language,
                                publisher: probe.publisher,
                                isbn: probe.isbn,
                                description: probe.description,
                                tags: Vec::new(),
                                file_size: size,
                                added_unix: crate::util::now_unix(),
                                cover_file,
                                last_open_unix: None,
                                position: Locus::START,
                                raster_page: 0,
                                percent: 0.0,
                                finished: false,
                                rating: 0,
                            });
                            report.added += 1;
                        }
                        Err(e) => report.errors.push((path.clone(), e.to_string())),
                    }
                }
            }
        }
        // Mark books whose file vanished (kept in the list, flagged missing by
        // path check at render time; here we just count them).
        report.missing = self.books.iter().filter(|b| !b.path.exists()).count();
        report
    }

    pub fn all_tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for b in &self.books {
            for t in &b.tags {
                if !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                    tags.push(t.clone());
                }
            }
        }
        tags.sort();
        tags
    }

    /// Filter + fuzzy search + sort → display ids.
    pub fn view(&self, query: &str, shelf: ShelfSel, sort: SortKey) -> Vec<u64> {
        let mut scored: Vec<(i64, &BookRecord)> = self
            .books
            .iter()
            .filter(|b| match &shelf {
                ShelfSel::All => true,
                ShelfSel::Manual(i) => self.shelves.get(*i).map(|s| s.books.contains(&b.id)).unwrap_or(false),
                ShelfSel::Smart(i) => {
                    self.smart_shelves.get(*i).map(|s| s.rule.accepts(b)).unwrap_or(false)
                }
            })
            .filter_map(|b| {
                if query.trim().is_empty() {
                    return Some((0, b));
                }
                let hay = format!("{} {} {}", b.title, b.author_line(), b.tags.join(" "));
                uikit_egui::fuzzy::score(query, &hay).map(|s| (-(s as i64), b))
            })
            .collect();
        scored.sort_by(|(sa, a), (sb, b)| {
            sa.cmp(sb).then_with(|| match sort {
                SortKey::RecentlyRead => b.last_open_unix.unwrap_or(0).cmp(&a.last_open_unix.unwrap_or(0)),
                SortKey::RecentlyAdded => b.added_unix.cmp(&a.added_unix),
                SortKey::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                SortKey::Author => a.author_line().to_lowercase().cmp(&b.author_line().to_lowercase()),
                SortKey::Series => a
                    .series
                    .as_ref()
                    .map(|(n, i)| (n.to_lowercase(), (i * 100.0) as i64))
                    .cmp(&b.series.as_ref().map(|(n, i)| (n.to_lowercase(), (i * 100.0) as i64))),
                SortKey::Progress => b
                    .percent
                    .partial_cmp(&a.percent)
                    .unwrap_or(std::cmp::Ordering::Equal),
                SortKey::Year => b.year.unwrap_or(0).cmp(&a.year.unwrap_or(0)),
            })
        });
        scored.into_iter().map(|(_, b)| b.id).collect()
    }

    pub fn toggle_shelf(&mut self, shelf_idx: usize, id: u64) {
        if let Some(s) = self.shelves.get_mut(shelf_idx) {
            if let Some(pos) = s.books.iter().position(|&b| b == id) {
                s.books.remove(pos);
            } else {
                s.books.push(id);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ShelfSel {
    All,
    Manual(usize),
    Smart(usize),
}

// ---------------------------------------------------------------------------
// Cover / figure texture cache
// ---------------------------------------------------------------------------

/// Decodes encoded image bytes into egui textures once, keyed by a stable id;
/// evicts nothing (covers are small; figure-heavy books get a cap).
pub struct TextureCache {
    map: HashMap<u64, Option<egui::TextureHandle>>,
    insertions: Vec<u64>,
    cap: usize,
}

impl TextureCache {
    pub fn new(cap: usize) -> TextureCache {
        TextureCache { map: HashMap::new(), insertions: Vec::new(), cap }
    }

    pub fn key(scope: &str, id: u64) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in scope.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^ id
    }

    /// Get-or-decode. `bytes` is only consulted on a miss.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        key: u64,
        name: &str,
        bytes: impl FnOnce() -> Option<Vec<u8>>,
        max_dim: u32,
    ) -> Option<egui::TextureHandle> {
        if let Some(entry) = self.map.get(&key) {
            return entry.clone();
        }
        let decoded = bytes().and_then(|b| decode_to_color_image(&b, max_dim));
        let handle = decoded.map(|img| ctx.load_texture(name, img, egui::TextureOptions::LINEAR));
        if self.insertions.len() >= self.cap {
            if let Some(old) = self.insertions.first().copied() {
                self.map.remove(&old);
                self.insertions.remove(0);
            }
        }
        self.insertions.push(key);
        self.map.insert(key, handle.clone());
        handle
    }

    pub fn forget_scope(&mut self, keys: &[u64]) {
        for k in keys {
            self.map.remove(k);
        }
        self.insertions.retain(|k| !keys.contains(k));
    }
}

pub fn decode_to_color_image(bytes: &[u8], max_dim: u32) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let img = if w.max(h) > max_dim {
        let scale = max_dim as f32 / w.max(h) as f32;
        img.resize(
            (w as f32 * scale) as u32,
            (h as f32 * scale) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(title: &str, author: &str, pct: f32) -> BookRecord {
        BookRecord {
            id: book_id(Path::new(title), 1),
            path: PathBuf::from(title),
            format: Format::Epub,
            title: title.into(),
            authors: vec![author.into()],
            series: None,
            year: None,
            language: None,
            publisher: None,
            isbn: None,
            description: None,
            tags: vec![],
            file_size: 1,
            added_unix: 0,
            cover_file: None,
            last_open_unix: None,
            position: Locus::START,
            raster_page: 0,
            percent: pct,
            finished: false,
            rating: 0,
        }
    }

    #[test]
    fn smart_rules_partition() {
        let a = rec("A", "x", 0.0);
        let b = rec("B", "x", 0.5);
        assert!(SmartRule::Unread.accepts(&a));
        assert!(!SmartRule::Unread.accepts(&b));
        assert!(SmartRule::InProgress.accepts(&b));
    }

    #[test]
    fn view_sorts_by_title() {
        let mut lib = Library::default();
        lib.books.push(rec("Zeta", "a", 0.0));
        lib.books.push(rec("Alpha", "b", 0.0));
        let ids = lib.view("", ShelfSel::All, SortKey::Title);
        assert_eq!(lib.get(ids[0]).unwrap().title, "Alpha");
    }
}
