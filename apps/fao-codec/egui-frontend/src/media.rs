//! media — a collection of video files from local folders and dufs servers.
//!
//! NOT the "Library" tab. That is the Web Library, a persistent catalog of
//! canonical MESHES the encoder reuses across encodes. This is the collection of
//! VIDEO FILES you point it at -- folders of game matches, local or on a LAN
//! dufs server. Named apart on purpose: two different things called "library"
//! in one app is how someone prunes the wrong one.
//!
//! ## Layers
//!
//! Protocol here, transport elsewhere. The dufs walker takes a `fetch` closure
//! -- give it a URL, get back the response body -- so everything below is pure
//! and tests against a fake server. The HTTP client and the range-request reader
//! that lets the streaming player read a remote `.fao` belong to the transport
//! layer, and are not in this file.
//!
//! ## What was checked about dufs, rather than assumed
//!
//! From dufs's own test suite (v0.46.0): `GET <dir>?json` returns an object
//! with a `paths` array; each item has `name`, `path_type` ("File", "Dir", and
//! symlink variants), `size` and `mtime`. It serves byte ranges (206 with
//! Content-Range), which is what makes remote streaming possible. And it serves
//! names containing emoji, spaces and newlines -- so every path segment must be
//! percent-encoded, which [`encode_path`] does.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Extensions treated as video. `.fao` plays now; the rest are sources the
/// encoder would read.
pub const VIDEO_EXTS: &[&str] = &["fao", "mkv", "mp4", "webm", "mov", "m4v", "avi", "ts", "flv"];

/// Where files come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    /// A dufs base URL including any path, e.g. `http://192.168.1.20:5000/matches`.
    Dufs(String),
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Local(p) => p.display().to_string(),
            Source::Dufs(u) => u.clone(),
        }
    }
}

/// Whether a file can be played now or is a source to encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fao,
    SourceVideo,
}

/// One file in the collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Index into the collection's sources.
    pub source: usize,
    /// Path relative to the source root, '/'-separated on every platform so a
    /// pattern means the same thing locally and over the network.
    pub rel: String,
    pub size: u64,
    pub kind: Kind,
}

impl Entry {
    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }
    /// Stable identity for selection: source plus path.
    pub fn key(&self) -> (usize, String) {
        (self.source, self.rel.clone())
    }
}

/// The kind for a file name, or None if it is not a video.
pub fn kind_of(name: &str) -> Option<Kind> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if !VIDEO_EXTS.contains(&ext.as_str()) {
        return None;
    }
    Some(if ext == "fao" { Kind::Fao } else { Kind::SourceVideo })
}

// ───────────────────────────── local folders ─────────────────────────────

/// What a scan found, and what it could not read.
#[derive(Debug, Default)]
pub struct Scan {
    pub entries: Vec<Entry>,
    /// Directories that could not be read, with why. Reported, not fatal: a
    /// folder of hundreds of GB will have SOME unreadable corner, and one
    /// permission error must not cost the whole scan.
    pub errors: Vec<(String, String)>,
}

/// Recursively scan a local folder for video files.
///
/// Reads METADATA only, never file contents, so hundreds of GB scan in the time
/// it takes to list them. Symlinked directories are not followed: a link back
/// up the tree would loop forever, and following one out of the tree would add
/// files the user never pointed at. Hidden directories (".git", ".cache") are
/// skipped for the same reason.
pub fn scan_local(root: &Path, source: usize) -> Scan {
    let mut scan = Scan::default();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let dir = root.join(&rel);
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                scan.errors.push((dir.display().to_string(), e.to_string()));
                continue;
            }
        };
        for item in rd.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(item.path()) else { continue };
            let child = if rel.as_os_str().is_empty() { PathBuf::from(&name) } else { rel.join(&name) };
            if meta.is_dir() {
                stack.push(child);
            } else if meta.is_file() {
                if let Some(kind) = kind_of(&name) {
                    let rel_str = child.to_string_lossy().replace('\\', "/");
                    scan.entries.push(Entry { source, rel: rel_str, size: meta.len(), kind });
                }
            }
        }
    }
    scan.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    scan
}

// ───────────────────────────── dufs ─────────────────────────────

/// One item from a dufs `?json` listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DufsItem {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Parse a dufs directory listing.
///
/// `path_type` is "File" or "Dir" or a symlink variant ("SymlinkFile",
/// "SymlinkDir"); anything ending in "Dir" is a directory. A missing `size`
/// reads as 0 rather than failing the listing -- one odd item must not hide the
/// rest of a folder.
pub fn parse_dufs_listing(json: &str) -> Result<Vec<DufsItem>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let paths = v.get("paths").and_then(|p| p.as_array())
        .ok_or_else(|| "no `paths` array -- is this a dufs server?".to_string())?;
    Ok(paths.iter().filter_map(|p| {
        let name = p.get("name")?.as_str()?.to_string();
        let kind = p.get("path_type").and_then(|t| t.as_str()).unwrap_or("File");
        Some(DufsItem {
            is_dir: kind.ends_with("Dir"),
            size: p.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
            name,
        })
    }).collect())
}

/// Percent-encode each '/'-separated segment, keeping the slashes.
///
/// dufs serves names with emoji, spaces and newlines (its own tests fetch
/// `%F0%9F%98%80.bin` and `file%0A1.txt`), so a raw name in a URL would request
/// the wrong file or none. Only RFC 3986 unreserved characters pass through.
pub fn encode_path(path: &str) -> String {
    path.split('/').map(|seg| {
        seg.bytes().map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        }).collect::<String>()
    }).collect::<Vec<_>>().join("/")
}

/// The URL for a directory's JSON listing under a dufs base URL.
pub fn listing_url(base: &str, rel_dir: &str) -> String {
    let base = base.trim_end_matches('/');
    if rel_dir.is_empty() {
        format!("{base}/?json")
    } else {
        format!("{base}/{}/?json", encode_path(rel_dir))
    }
}

/// The URL of a file under a dufs base URL.
pub fn file_url(base: &str, rel: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), encode_path(rel))
}

/// Walk a dufs server recursively, through `fetch`.
///
/// `max_dirs` bounds the walk: a LAN share can be enormous, and a UI that hangs
/// listing it is worse than one that says "stopped after N folders".
pub fn scan_dufs(base: &str, source: usize, max_dirs: usize,
                 fetch: &mut dyn FnMut(&str) -> Result<String, String>) -> Scan {
    let mut scan = Scan::default();
    let mut stack = vec![String::new()];
    let mut visited = 0usize;
    while let Some(rel) = stack.pop() {
        if visited >= max_dirs {
            scan.errors.push((base.to_string(), format!("stopped after {max_dirs} folders")));
            break;
        }
        visited += 1;
        let url = listing_url(base, &rel);
        let items = match fetch(&url).and_then(|body| parse_dufs_listing(&body)) {
            Ok(items) => items,
            Err(e) => {
                scan.errors.push((url, e));
                continue;
            }
        };
        for it in items {
            let child = if rel.is_empty() { it.name.clone() } else { format!("{rel}/{}", it.name) };
            if it.is_dir {
                if !it.name.starts_with('.') {
                    stack.push(child);
                }
            } else if let Some(kind) = kind_of(&it.name) {
                scan.entries.push(Entry { source, rel: child, size: it.size, kind });
            }
        }
    }
    scan.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    scan
}

// ───────────────────────────── patterns ─────────────────────────────

/// Does `pattern` match `rel`?
///
/// Glob, case-insensitive (game captures come as "Match.MKV" as often as
/// "match.mkv"):
///   * `*`  -- any run of characters within one folder name
///   * `**` -- any run including '/', so any depth; `**/` also matches none
///   * `?`  -- one character, not '/'
///
/// A pattern with no '/' matches the FILE NAME at any depth, the way a
/// .gitignore line does: `*.mkv` finds `ranked/2026/match.mkv`. A pattern with a
/// '/' matches the whole relative path.
pub fn glob_match(pattern: &str, rel: &str) -> bool {
    let (p, t) = if pattern.contains('/') {
        (pattern.to_lowercase(), rel.to_lowercase())
    } else {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        (pattern.to_lowercase(), name.to_lowercase())
    };
    let p: Vec<char> = p.chars().collect();
    let t: Vec<char> = t.chars().collect();
    let mut memo = vec![vec![None; t.len() + 1]; p.len() + 1];
    glob_at(&p, &t, 0, 0, &mut memo)
}

fn glob_at(p: &[char], t: &[char], i: usize, j: usize, memo: &mut Vec<Vec<Option<bool>>>) -> bool {
    if let Some(r) = memo[i][j] {
        return r;
    }
    let r = if i == p.len() {
        j == t.len()
    } else if p[i] == '*' && p.get(i + 1) == Some(&'*') {
        // `**` matches any run including '/'. For `**/`, skip the slash too:
        // then trying the rest at k == j IS "zero folders", so `ranked/**/x`
        // matches `ranked/x` with no separate case.
        //
        // There WAS a separate zero-folder branch. Removing it failed no test,
        // because the k == j iteration below already does exactly that -- it
        // was dead code, and is gone rather than kept for reassurance.
        let after = if p.get(i + 2) == Some(&'/') { i + 3 } else { i + 2 };
        (j..=t.len()).any(|k| glob_at(p, t, after, k, memo))
    } else if p[i] == '*' {
        let mut k = j;
        loop {
            if glob_at(p, t, i + 1, k, memo) { break true; }
            if k == t.len() || t[k] == '/' { break false; }
            k += 1;
        }
    } else if j < t.len() && (p[i] == t[j] || (p[i] == '?' && t[j] != '/')) {
        glob_at(p, t, i + 1, j + 1, memo)
    } else {
        false
    };
    memo[i][j] = Some(r);
    r
}

// ───────────────────────────── selection ─────────────────────────────

/// Which files are selected: patterns, then individual overrides on top.
///
/// Precedence, so a click always means what it looks like: an individual
/// untick beats everything, an individual tick beats the patterns, and
/// otherwise a file is in if it matches an include and no exclude.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub on: BTreeSet<(usize, String)>,
    pub off: BTreeSet<(usize, String)>,
}

impl Selection {
    pub fn selected(&self, e: &Entry) -> bool {
        let k = e.key();
        if self.off.contains(&k) {
            return false;
        }
        if self.on.contains(&k) {
            return true;
        }
        self.include.iter().any(|p| glob_match(p, &e.rel))
            && !self.exclude.iter().any(|p| glob_match(p, &e.rel))
    }

    /// Flip one file, as a click does: whatever it is now, it becomes the
    /// opposite, and stays that way whatever the patterns say.
    pub fn toggle(&mut self, e: &Entry) {
        let k = e.key();
        let now = self.selected(e);
        self.on.remove(&k);
        self.off.remove(&k);
        if now { self.off.insert(k); } else { self.on.insert(k); }
    }
}

/// Counts and bytes for a set of entries -- the "N files, X GB" line.
pub fn totals<'a>(entries: impl Iterator<Item = &'a Entry>) -> (usize, u64) {
    entries.fold((0, 0), |(n, b), e| (n + 1, b + e.size))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── patterns ──

    #[test]
    fn a_slashless_pattern_matches_the_name_at_any_depth() {
        assert!(glob_match("*.mkv", "match.mkv"));
        assert!(glob_match("*.mkv", "ranked/2026/match.mkv"));
        assert!(!glob_match("*.mkv", "ranked/match.mp4"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(glob_match("*.mkv", "RANKED/Match.MKV"));
        assert!(glob_match("ranked/*", "Ranked/x.mp4"));
    }

    #[test]
    fn a_single_star_stays_inside_one_folder() {
        assert!(glob_match("ranked/*.mkv", "ranked/a.mkv"));
        assert!(!glob_match("ranked/*.mkv", "ranked/2026/a.mkv"), "* must not cross '/'");
    }

    #[test]
    fn a_double_star_crosses_folders_including_none() {
        assert!(glob_match("ranked/**/*.mkv", "ranked/2026/09/a.mkv"));
        assert!(glob_match("ranked/**/*.mkv", "ranked/a.mkv"), "**/ matches zero folders");
        assert!(glob_match("**/finals/*", "a/b/finals/g.mp4"));
        assert!(!glob_match("ranked/**/*.mkv", "casual/a.mkv"));
    }

    #[test]
    fn question_mark_is_one_character() {
        assert!(glob_match("match_?.mkv", "match_7.mkv"));
        assert!(!glob_match("match_?.mkv", "match_17.mkv"));
    }

    #[test]
    fn date_style_patterns_work() {
        assert!(glob_match("2026-09-*", "clips/2026-09-14_ranked.mkv"));
        assert!(!glob_match("2026-09-*", "clips/2026-08-30_ranked.mkv"));
    }

    /// A pathological pattern must not take exponential time -- the memo is
    /// what stops `*a*a*a*...` against a long name from hanging the UI.
    #[test]
    fn a_pathological_pattern_is_still_fast() {
        let p = "*a*a*a*a*a*a*a*a*a*a*b";
        let t = "a".repeat(200);
        let start = std::time::Instant::now();
        assert!(!glob_match(p, &t));
        assert!(start.elapsed().as_millis() < 500, "{:?}", start.elapsed());
    }

    // ── selection ──

    fn e(rel: &str) -> Entry {
        Entry { source: 0, rel: rel.into(), size: 1, kind: kind_of(rel).unwrap() }
    }

    #[test]
    fn include_then_exclude() {
        let s = Selection { include: vec!["*.mkv".into()], exclude: vec!["**/scrims/*".into()], ..Default::default() };
        assert!(s.selected(&e("ranked/a.mkv")));
        assert!(!s.selected(&e("x/scrims/b.mkv")), "excluded");
        assert!(!s.selected(&e("ranked/c.mp4")), "not included");
    }

    /// A click means what it looks like, whatever the patterns say.
    #[test]
    fn an_individual_click_overrides_the_patterns_both_ways() {
        let mut s = Selection { include: vec!["*.mkv".into()], ..Default::default() };
        let a = e("ranked/a.mkv");
        let b = e("ranked/b.mp4");
        s.toggle(&a);
        assert!(!s.selected(&a), "unticked though the pattern includes it");
        s.toggle(&b);
        assert!(s.selected(&b), "ticked though no pattern includes it");
        s.toggle(&a);
        assert!(s.selected(&a), "and a second click restores it");
    }

    #[test]
    fn no_patterns_selects_nothing_but_clicks() {
        let mut s = Selection::default();
        let a = e("a.mkv");
        assert!(!s.selected(&a));
        s.toggle(&a);
        assert!(s.selected(&a));
    }

    // ── dufs ──

    /// The shape dufs's own tests assert: a `paths` array of name / path_type /
    /// size / mtime.
    const LISTING: &str = r#"{"href":"/","kind":"Index","paths":[
        {"path_type":"Dir","name":"ranked","mtime":1726000000000,"size":0},
        {"path_type":"File","name":"match 1.mkv","mtime":1726000000000,"size":4294967296},
        {"path_type":"File","name":"notes.txt","mtime":1726000000000,"size":10},
        {"path_type":"SymlinkDir","name":"linked","mtime":0,"size":0},
        {"path_type":"Dir","name":".cache","mtime":0,"size":0}
    ]}"#;

    #[test]
    fn a_dufs_listing_parses_including_symlink_dirs() {
        let items = parse_dufs_listing(LISTING).unwrap();
        assert_eq!(items.len(), 5);
        assert!(items.iter().any(|i| i.name == "ranked" && i.is_dir));
        assert!(items.iter().any(|i| i.name == "linked" && i.is_dir), "SymlinkDir is a directory");
        assert!(items.iter().any(|i| i.name == "match 1.mkv" && !i.is_dir && i.size == 4_294_967_296),
                "a 4 GB size survives as u64");
    }

    #[test]
    fn a_non_dufs_response_is_a_clear_error() {
        assert!(parse_dufs_listing("<html>").unwrap_err().contains("not JSON"));
        assert!(parse_dufs_listing(r#"{"x":1}"#).unwrap_err().contains("dufs"));
    }

    /// dufs serves emoji, spaces and newlines in names; a raw one in a URL
    /// fetches the wrong file. These are dufs's own test fixtures.
    #[test]
    fn paths_are_percent_encoded_per_segment() {
        assert_eq!(encode_path("😀.bin"), "%F0%9F%98%80.bin");
        assert_eq!(encode_path("file\n1.txt"), "file%0A1.txt");
        assert_eq!(encode_path("ranked/match 1.mkv"), "ranked/match%201.mkv");
        assert_eq!(file_url("http://h:5000/v/", "a b/c.mkv"), "http://h:5000/v/a%20b/c.mkv");
        assert_eq!(listing_url("http://h:5000", ""), "http://h:5000/?json");
        assert_eq!(listing_url("http://h:5000", "a b"), "http://h:5000/a%20b/?json");
    }

    /// A fake dufs: a tree served through `fetch`, recursed, filtered to videos.
    #[test]
    fn a_dufs_tree_is_walked_recursively() {
        let base = "http://nas:5000/matches";
        let mut fetch = |url: &str| -> Result<String, String> {
            match url {
                "http://nas:5000/matches/?json" => Ok(LISTING.into()),
                "http://nas:5000/matches/ranked/?json" => Ok(r#"{"paths":[
                    {"path_type":"File","name":"final.mp4","size":7,"mtime":0},
                    {"path_type":"Dir","name":"2026","size":0,"mtime":0}]}"#.into()),
                "http://nas:5000/matches/ranked/2026/?json" => Ok(r#"{"paths":[
                    {"path_type":"File","name":"deep.fao","size":9,"mtime":0}]}"#.into()),
                "http://nas:5000/matches/linked/?json" => Err("403".into()),
                other => Err(format!("unexpected {other}")),
            }
        };
        let scan = scan_dufs(base, 3, 100, &mut fetch);
        let rels: Vec<_> = scan.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, vec!["match 1.mkv", "ranked/2026/deep.fao", "ranked/final.mp4"]);
        assert!(scan.entries.iter().all(|e| e.source == 3));
        assert_eq!(scan.entries[1].kind, Kind::Fao);
        // The unreadable folder is reported, not fatal; .cache was never asked for.
        assert_eq!(scan.errors.len(), 1, "{:?}", scan.errors);
        assert!(scan.errors[0].0.contains("linked"));
    }

    #[test]
    fn a_huge_share_stops_at_the_folder_limit() {
        let mut fetch = |_: &str| -> Result<String, String> {
            Ok(r#"{"paths":[{"path_type":"Dir","name":"d","size":0,"mtime":0}]}"#.into())
        };
        let scan = scan_dufs("http://nas:5000", 0, 5, &mut fetch);
        assert!(scan.errors.iter().any(|(_, e)| e.contains("stopped after 5")));
    }

    // ── local ──

    #[test]
    fn a_local_folder_is_scanned_recursively_for_videos_only() {
        let root = std::env::temp_dir().join(format!("media_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("ranked/2026")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("a.mkv"), b"12345").unwrap();
        std::fs::write(root.join("ranked/b.MP4"), b"1").unwrap();
        std::fs::write(root.join("ranked/2026/c.fao"), b"1").unwrap();
        std::fs::write(root.join("ranked/readme.txt"), b"1").unwrap();
        std::fs::write(root.join(".hidden/secret.mkv"), b"1").unwrap();
        let scan = scan_local(&root, 0);
        let rels: Vec<_> = scan.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, vec!["a.mkv", "ranked/2026/c.fao", "ranked/b.MP4"]);
        assert_eq!(scan.entries[0].size, 5);
        assert!(scan.errors.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_folder_is_an_error_not_a_panic() {
        let scan = scan_local(Path::new("/definitely/not/here"), 0);
        assert!(scan.entries.is_empty());
        assert_eq!(scan.errors.len(), 1);
    }

    #[test]
    fn totals_add_up_past_four_gigabytes() {
        let es = vec![e("a.mkv"), e("b.mkv")];
        let big: Vec<Entry> = es.into_iter().map(|mut x| { x.size = 300 * 1024 * 1024 * 1024; x }).collect();
        assert_eq!(totals(big.iter()), (2, 600 * 1024 * 1024 * 1024));
    }
}
