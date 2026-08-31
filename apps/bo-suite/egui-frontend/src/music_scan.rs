//! music_scan — a catalogue from a directory of music.
//!
//! Two gaps, one module, because they are the same gap.
//!
//! **The library has no source.** `CatalogSource` is a trait with two
//! implementations, both test fakes. Its own header names the real one —
//! bo-controller's SQLite catalogue — and that lives behind a worker
//! channel in another process, so this frontend has never shown a real
//! library.
//!
//! **A track has no file.** `playback` can play a path and `CatalogTrack`
//! carries a title, an album, artists, tags, a bpm and a key. Nothing in
//! the row says where the audio is, so the engine I just wrote had nothing
//! to point at.
//!
//! A filesystem scan answers both, needs no database and no network, and
//! is the catalogue most people's music actually is.
//!
//! ## Metadata comes from the PATH, and that is stated rather than hidden
//!
//! There is no tag reader here — that is a decoder dependency in a widget
//! crate, which this fleet has refused three times for good reasons. So
//! `Artist/Album/04 - Title.flac` becomes exactly that, and a file that
//! does not follow the convention gets its filename as a title and nothing
//! else.
//!
//! That is worse than reading tags and it is HONEST about which it is: the
//! alternative people reach for is guessing an artist from a folder name
//! several levels up, which produces a library that is confidently wrong
//! and impossible to correct from the UI. A missing artist is visibly
//! missing.
//!
//! ## Bounded, because a music directory is not small
//!
//! [`MAX_TRACKS`] and [`MAX_DEPTH`]. A scan that walks an entire home
//! directory because someone pointed it at `~` should stop rather than
//! take a minute and a gigabyte — and the depth limit is what stops a
//! symlink loop, which `read_dir` will happily follow forever.

use std::path::{Path, PathBuf};

use crate::catalog_source::{CatalogSource, CatalogTrack};

/// Extensions taken as music.
///
/// Deliberately short. A scanner that accepts everything and lets the
/// player refuse fills the library with rows that fail when tapped, and
/// "why is my PDF in here" is a worse first impression than a missing
/// format.
pub const AUDIO_EXTS: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "wma", "alac",
];

/// The most files one scan will take.
pub const MAX_TRACKS: usize = 20_000;

/// How deep to walk. Six is `~/Music/Artist/Album/Disc 1/track` with room
/// to spare, and it is what stops a symlink loop.
pub const MAX_DEPTH: usize = 6;

/// Where music is, if this machine has an obvious answer.
///
/// `$XDG_MUSIC_DIR` first because it is the variable that exists to say so;
/// then the two conventional names. No guessing beyond that: scanning a
/// home directory looking for audio is slow, surprising, and finds the
/// voice memos.
pub fn default_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(d) = std::env::var("XDG_MUSIC_DIR") {
        if !d.trim().is_empty() { out.push(PathBuf::from(d)); }
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push(Path::new(&home).join("Music"));
        out.push(Path::new(&home).join("music"));
    }
    out
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| AUDIO_EXTS.contains(&e.as_str()))
}

/// Title, artist and album from a path under `root`.
///
/// `Artist/Album/04 - Title.flac`. A leading track number is stripped
/// because it is not part of the title and sorts wrongly when it is —
/// `10 - A` before `2 - B` in any alphabetical list.
pub fn meta_from_path(root: &Path, file: &Path) -> (String, Option<String>, Vec<String>) {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let title = strip_track_number(stem);
    let rel: Vec<String> = file.strip_prefix(root).unwrap_or(file)
        .parent()
        .map(|p| p.components()
            .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_string()))
            .collect())
        .unwrap_or_default();
    // Read from the FILE upwards: the album is the folder it sits in and
    // the artist the one above. Counting downwards from the root breaks the
    // moment somebody keeps a compilation one level shallower, which
    // everyone does.
    let album = rel.last().cloned();
    let artist = if rel.len() >= 2 { vec![rel[rel.len() - 2].clone()] } else { Vec::new() };
    (title, album, artist)
}

/// `04 - Title`, `04. Title`, `04 Title` → `Title`.
///
/// Only when what follows is not itself a number: `1999` is a title and
/// `07 - 1979` keeps its year.
pub fn strip_track_number(stem: &str) -> String {
    let t = stem.trim();
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 3 {
        return t.to_string();
    }
    let rest = t[digits.len()..].trim_start();
    let rest = rest.trim_start_matches(['-', '.', '_']).trim_start();
    if rest.is_empty() { t.to_string() } else { rest.to_string() }
}

/// One scanned file.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    pub title: String,
    pub album: Option<String>,
    pub artists: Vec<String>,
}

/// Walk a directory for music.
///
/// Sorted by path, so the library is in a stable order across scans —
/// `read_dir` returns whatever the filesystem feels like, and a list that
/// reshuffles on every launch is one nobody can navigate by memory.
pub fn scan(root: &Path) -> Vec<Found> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || out.len() >= MAX_TRACKS {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // `file_type` rather than `metadata`: the first does not follow
            // a symlink, so a link pointing back up the tree is seen as a
            // link and not as the directory it aims at.
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push((p, depth + 1));
            } else if ft.is_file() && is_audio(&p) {
                if out.len() >= MAX_TRACKS { break; }
                let (title, album, artists) = meta_from_path(root, &p);
                out.push(Found { path: p, title, album, artists });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// A `CatalogSource` over a scanned directory.
///
/// Scanned ONCE at construction, not per page. A source that re-walked the
/// disk for every page would make scrolling the library an I/O storm, and
/// the pages would disagree with each other whenever a file changed
/// mid-scroll.
#[derive(Debug, Clone, Default)]
pub struct FsCatalog {
    tracks: Vec<Found>,
    pub root: Option<PathBuf>,
}

impl FsCatalog {
    pub fn new(root: &Path) -> FsCatalog {
        FsCatalog { tracks: scan(root), root: Some(root.to_path_buf()) }
    }

    /// The first root that exists and holds anything.
    pub fn discover() -> FsCatalog {
        for r in default_roots() {
            if r.is_dir() {
                let c = FsCatalog::new(&r);
                if !c.tracks.is_empty() { return c; }
            }
        }
        FsCatalog::default()
    }

    pub fn len(&self) -> usize { self.tracks.len() }
    pub fn is_empty(&self) -> bool { self.tracks.is_empty() }

    /// The file for a track id. Ids are indices into the scan, which is
    /// why the scan is sorted and held: an id that meant "the nth file
    /// read_dir happened to return" would point somewhere else next launch.
    pub fn path_of(&self, id: i64) -> Option<&Path> {
        usize::try_from(id).ok().and_then(|i| self.tracks.get(i)).map(|t| t.path.as_path())
    }

    fn row(&self, i: usize) -> CatalogTrack {
        let t = &self.tracks[i];
        CatalogTrack {
            id: i as i64,
            title: t.title.clone(),
            album: t.album.clone(),
            artists: t.artists.clone(),
            tags: Vec::new(),
            bpm: None,
            key_text: None,
        }
    }
}

impl CatalogSource for FsCatalog {
    fn page(&self, offset: i64, limit: i64, sort: &str) -> Vec<CatalogTrack> {
        let mut idx: Vec<usize> = (0..self.tracks.len()).collect();
        // Sorted here because the trait says the BACKEND sorts: a view that
        // sorted its own rows would order only the page it has, and the
        // order would change as more arrived.
        match sort {
            "title" => idx.sort_by(|a, b| self.tracks[*a].title.to_lowercase()
                                    .cmp(&self.tracks[*b].title.to_lowercase())),
            "artist" => idx.sort_by(|a, b| self.tracks[*a].artists.first().cloned()
                                    .unwrap_or_default().to_lowercase()
                                    .cmp(&self.tracks[*b].artists.first().cloned()
                                         .unwrap_or_default().to_lowercase())),
            "album" => idx.sort_by(|a, b| self.tracks[*a].album.clone().unwrap_or_default()
                                    .to_lowercase()
                                    .cmp(&self.tracks[*b].album.clone().unwrap_or_default()
                                         .to_lowercase())),
            // Unknown sort keeps the scan order rather than refusing: a
            // library in path order is useful, and an empty page is not.
            _ => {}
        }
        let off = offset.max(0) as usize;
        let lim = limit.max(0) as usize;
        idx.into_iter().skip(off).take(lim).map(|i| self.row(i)).collect()
    }

    fn total(&self) -> i64 {
        self.tracks.len() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("bo-scan-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"not really audio").unwrap();
        }
        root
    }

    #[test]
    fn a_conventional_tree_yields_titles_albums_and_artists() {
        let root = tree("conv", &[
            "Ada Lovelace/Engines/01 - Overture.flac",
            "Ada Lovelace/Engines/02. Analytical.mp3",
            "Bo Jensen/Nord/07 Fjord.ogg",
        ]);
        let found = scan(&root);
        assert_eq!(found.len(), 3, "{found:#?}");
        assert_eq!(found[0].title, "Overture");
        assert_eq!(found[0].album.as_deref(), Some("Engines"));
        assert_eq!(found[0].artists, vec!["Ada Lovelace".to_string()]);
        assert_eq!(found[1].title, "Analytical", "the '02.' form was not stripped");
        assert_eq!(found[2].title, "Fjord", "the bare-number form was not stripped");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_number_that_is_the_title_keeps_it() {
        // `1999` is a song. Stripping it leaves a blank row, which is the
        // kind of bug that only shows up in somebody's actual library.
        assert_eq!(strip_track_number("1999"), "1999");
        assert_eq!(strip_track_number("07 - 1979"), "1979");
        assert_eq!(strip_track_number("Untitled"), "Untitled");
        assert_eq!(strip_track_number("04 - Don't Stop"), "Don't Stop");
    }

    #[test]
    fn a_loose_file_gets_a_title_and_no_invented_artist() {
        // The alternative people reach for is guessing an artist from a
        // folder several levels up, which is confidently wrong and cannot
        // be corrected from the UI. Missing is visibly missing.
        let root = tree("loose", &["Stray Recording.wav"]);
        let found = scan(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Stray Recording");
        assert!(found[0].artists.is_empty(), "an artist was invented: {:?}", found[0]);
        assert_eq!(found[0].album, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn non_audio_is_left_out_rather_than_left_to_fail_when_tapped() {
        let root = tree("mixed", &[
            "A/B/01 - Song.flac", "A/B/cover.jpg", "A/B/notes.pdf", "A/B/liner.txt",
        ]);
        let found = scan(&root);
        assert_eq!(found.len(), 1, "{found:#?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_scan_order_is_stable_across_runs() {
        // `read_dir` returns whatever the filesystem feels like. A library
        // that reshuffles on every launch is one nobody can navigate by
        // memory — and ids are indices into this order.
        let root = tree("order", &[
            "Z/late.flac", "A/early.flac", "M/middle.flac",
        ]);
        let a: Vec<_> = scan(&root).into_iter().map(|f| f.path).collect();
        let b: Vec<_> = scan(&root).into_iter().map(|f| f.path).collect();
        assert_eq!(a, b);
        assert!(a[0].ends_with("A/early.flac"), "{a:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_track_id_resolves_to_the_file_the_row_describes() {
        // The whole point: `playback` can play a path, and until now
        // nothing produced one.
        let root = tree("resolve", &["A/B/01 - One.flac", "A/B/02 - Two.flac"]);
        let cat = FsCatalog::new(&root);
        let rows = cat.page(0, 10, "path");
        assert_eq!(rows.len(), 2);
        let p = cat.path_of(rows[1].id).expect("no path for a row that exists");
        assert!(p.ends_with("02 - Two.flac"), "{p:?}");
        assert_eq!(cat.path_of(999), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_backend_sorts_so_a_later_page_agrees_with_the_first() {
        // The trait's own rule. A view sorting its own rows orders only
        // what it has, and the order changes as pages arrive.
        let root = tree("sort", &[
            "A/B/01 - Zulu.flac", "A/B/02 - Alpha.flac", "A/B/03 - Mike.flac",
        ]);
        let cat = FsCatalog::new(&root);
        let titles: Vec<String> = cat.page(0, 10, "title").into_iter()
            .map(|t| t.title).collect();
        assert_eq!(titles, vec!["Alpha".to_string(), "Mike".into(), "Zulu".into()]);
        // And paging into the middle keeps that order.
        let second = cat.page(1, 1, "title");
        assert_eq!(second[0].title, "Mike");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_with_no_music_is_empty_rather_than_an_error() {
        let root = tree("empty", &["readme.txt"]);
        let cat = FsCatalog::new(&root);
        assert!(cat.is_empty());
        assert_eq!(cat.total(), 0);
        assert!(cat.page(0, 10, "title").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_root_scans_to_nothing_and_does_not_panic() {
        let cat = FsCatalog::new(Path::new("/definitely/not/here"));
        assert!(cat.is_empty());
    }
}
