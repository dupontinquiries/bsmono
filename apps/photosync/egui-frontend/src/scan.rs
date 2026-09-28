//! scan — walk a folder, decode what is there, and report each photo.
//!
//! On its own thread, because decoding a camera roll takes seconds to
//! minutes and a grid that fills in as photos arrive is the honest way to
//! show that. Every result is an event over a channel; the shell drains
//! them each frame and never blocks.
//!
//! ## What is cached, and why
//!
//! * The [`Index`] carries the hashes and tags, keyed by path + size +
//!   mtime. A photo whose index entry is current is not re-hashed.
//! * Thumbnails go to the XDG cache dir as small PNGs, named by a hash of
//!   the path. A current index entry with a cached thumbnail costs one
//!   small PNG decode instead of one large JPEG decode — which is the
//!   difference between a library that opens in a second and one that
//!   opens in a minute, every time.
//!
//! ## What is decoded
//!
//! JPEG, PNG and WebP: what a phone's camera roll contains. The full image
//! is decoded once, shrunk to at most [`SCAN_PX`] on its long side for the
//! core (its hashes are resolution-invariant by design — see the core's
//! own test), and to [`THUMB_PX`] for the grid.

use crate::library::{Index, Photo, Thumb, THUMB_PX};
use photosync::{Entry, Gray};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

/// The long side the core sees. Its cells are 9×8 and 32×16; anything
/// above a few hundred pixels is decode time spent on nothing.
pub const SCAN_PX: u32 = 512;

/// The file extensions that count as photos.
pub const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// What the scanner reports.
#[derive(Debug)]
pub enum ScanEvent {
    /// The walk finished: this many files will be scanned.
    Found(usize),
    /// One photo, decoded (or restored from the index and the cache).
    Photo(Box<Photo>),
    /// A file that could not be read. Reported, not hidden: a photo that
    /// silently never appears is one the user cannot tell from a lost one.
    Failed { path: String, why: String },
    /// Everything reported. `from_cache` says how many were restored
    /// rather than decoded.
    Done { scanned: usize, from_cache: usize },
}

/// Is this a photo file, by extension?
pub fn is_photo(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Every photo under `root`, recursively, hidden directories skipped.
/// Sorted, so the scan order (and the order photos appear) is the same
/// every launch.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(false);
            if hidden { continue; }
            if p.is_dir() { stack.push(p); } else if is_photo(&p) { out.push(p); }
        }
    }
    out.sort();
    out
}

/// The thumbnail cache the app uses: `$XDG_CACHE_HOME/photosync/thumbs`.
pub fn default_cache_dir() -> PathBuf {
    config_kit::cache_home().join("photosync").join("thumbs")
}

/// Where a photo's cached thumbnail lives under a cache dir.
pub fn thumb_cache_path(cache: &Path, photo_path: &str) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in photo_path.bytes() { h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3); }
    cache.join(format!("{h:016x}.png"))
}

/// Decode a photo fully: the core entry and a thumbnail.
pub fn decode(path: &Path, written_tags: &[&str]) -> Result<(Entry, Thumb, u32, u32), String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 { return Err("empty image".into()); }
    // One shrink for the core, a second for the grid. `thumbnail` is the
    // fast box filter; the core box-averages again on top, which is fine.
    let small = if w.max(h) > SCAN_PX { img.thumbnail(SCAN_PX, SCAN_PX) } else { img.clone() };
    let luma = small.to_luma8();
    let gray = Gray::new(luma.width() as usize, luma.height() as usize, luma.as_raw())
        .ok_or("bad stride")?;
    let entry = Entry::scan(&path.to_string_lossy(), &gray, written_tags);
    let t = small.thumbnail(THUMB_PX, THUMB_PX).to_rgba8();
    let thumb = Thumb { w: t.width() as usize, h: t.height() as usize, rgba: t.into_raw() };
    Ok((entry, thumb, w, h))
}

/// A thumbnail from the cache, if there is one.
fn cached_thumb(cache: &Path, photo_path: &str) -> Option<Thumb> {
    let p = thumb_cache_path(cache, photo_path);
    let img = image::open(&p).ok()?.to_rgba8();
    Some(Thumb { w: img.width() as usize, h: img.height() as usize, rgba: img.into_raw() })
}

fn cache_thumb(cache: &Path, photo_path: &str, t: &Thumb) {
    let p = thumb_cache_path(cache, photo_path);
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    if let Some(buf) = image::RgbaImage::from_raw(t.w as u32, t.h as u32, t.rgba.clone()) {
        let _ = buf.save(&p);
    }
}

/// The file's size and modification time, as the index keys them.
pub fn stat(path: &Path) -> std::io::Result<(u64, i64)> {
    let m = std::fs::metadata(path)?;
    let at = m.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok((m.len(), at))
}

/// Scan one file, through the index and the cache when they are current.
pub fn scan_one(path: &Path, index: &Index, cache: &Path) -> Result<(Photo, bool), String> {
    let key = path.to_string_lossy().to_string();
    let (size, at) = stat(path).map_err(|e| e.to_string())?;
    let prior = index.entries.iter().find(|e| e.path == key);
    let written: Vec<&str> = prior.map(|e| e.tags.iter()
        .filter(|t| !photosync::cv::is_auto(t)).map(String::as_str).collect()).unwrap_or_default();
    if index.is_current(&key, size, at) {
        if let (Some(mut photo), Some(thumb)) = (index.to_library().remove(&key), cached_thumb(cache, &key)) {
            photo.thumb = Some(thumb);
            return Ok((photo, true));
        }
    }
    let (entry, thumb, w, h) = decode(path, &written)?;
    cache_thumb(cache, &key, &thumb);
    Ok((Photo { path: key, at, size, w, h, entry, thumb: Some(thumb) }, false))
}

/// Run the scan on its own thread; events arrive on the returned channel.
pub fn spawn(root: PathBuf, index: Index, cache: PathBuf) -> Receiver<ScanEvent> {
    let (tx, rx) = channel();
    std::thread::Builder::new().name("photosync-scan".into()).spawn(move || run(&root, &index, &cache, &tx)).ok();
    rx
}

fn run(root: &Path, index: &Index, cache: &Path, tx: &Sender<ScanEvent>) {
    let files = walk(root);
    if tx.send(ScanEvent::Found(files.len())).is_err() { return; }
    let (mut scanned, mut from_cache) = (0usize, 0usize);
    for f in &files {
        let ev = match scan_one(f, index, cache) {
            Ok((photo, cached)) => { scanned += 1; if cached { from_cache += 1; } ScanEvent::Photo(Box::new(photo)) }
            Err(why) => ScanEvent::Failed { path: f.to_string_lossy().to_string(), why },
        };
        if tx.send(ev).is_err() { return; }
    }
    let _ = tx.send(ScanEvent::Done { scanned, from_cache });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a small PNG with a gradient, so the decoder has real pixels.
    pub(crate) fn write_png(path: &Path, w: u32, h: u32, seed: u8) {
        // RGB, not RGBA: the JPEG encoder refuses an alpha channel, and the
        // format is chosen by the extension the test asks for.
        let mut img = image::RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = ((x + y) * 255 / (w + h).max(1)) as u8 ^ seed;
            *p = image::Rgb([v, v / 2, 255 - v]);
        }
        img.save(path).unwrap();
    }

    #[test]
    fn the_walk_finds_photos_recursively_and_skips_hidden_dirs() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("trip/.thumbs")).unwrap();
        write_png(&d.path().join("a.png"), 8, 8, 0);
        write_png(&d.path().join("trip/b.PNG"), 8, 8, 1);
        write_png(&d.path().join("trip/.thumbs/c.png"), 8, 8, 2);
        std::fs::write(d.path().join("notes.txt"), "x").unwrap();
        let found = walk(d.path());
        let names: Vec<String> = found.iter().map(|p| p.strip_prefix(d.path()).unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["a.png", "trip/b.PNG"]);
    }

    #[test]
    fn a_decoded_photo_has_a_thumbnail_and_a_real_entry() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("wide.png");
        write_png(&p, 640, 320, 0);
        let (entry, thumb, w, h) = decode(&p, &["trip"]).unwrap();
        assert_eq!((w, h), (640, 320));
        assert!(thumb.w <= THUMB_PX as usize && thumb.h <= THUMB_PX as usize);
        assert_eq!(thumb.rgba.len(), thumb.w * thumb.h * 4);
        assert!(thumb.w > thumb.h, "aspect is kept");
        assert!(entry.tags.contains(&"trip".to_string()));
        assert!(entry.tags.iter().any(|t| t == "auto:landscape"), "{:?}", entry.tags);
        assert_eq!(entry.ascii.lines().count(), photosync::ASCII_H);
    }

    #[test]
    fn a_second_scan_comes_from_the_index_and_the_cache() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("cache");
        let p = d.path().join("one.png");
        write_png(&p, 64, 48, 3);
        let (first, cached) = scan_one(&p, &Index::default(), &cache).unwrap();
        assert!(!cached);
        let mut lib = crate::library::Library { root: d.path().to_string_lossy().to_string(), ..Default::default() };
        lib.upsert(first.clone());
        lib.add_tag(0, "keeper");
        let index = Index::from_library(&lib);
        let (second, cached) = scan_one(&p, &index, &cache).unwrap();
        assert!(cached, "the index was current and the thumbnail cached");
        assert_eq!(second.entry.dhash, first.entry.dhash);
        assert!(second.entry.tags.contains(&"keeper".to_string()), "written tags survive");
        assert!(second.thumb.is_some());
        // Touch the file: the index is stale and the photo is decoded again.
        write_png(&p, 64, 48, 4);
        let (third, cached) = scan_one(&p, &index, &cache).unwrap();
        assert!(!cached || third.entry.dhash != first.entry.dhash);
        assert!(third.entry.tags.contains(&"keeper".to_string()), "written tags carried into the rescan");
    }

    #[test]
    fn the_thread_reports_found_each_photo_and_done() {
        let d = tempfile::tempdir().unwrap();
        write_png(&d.path().join("a.png"), 16, 16, 0);
        write_png(&d.path().join("b.jpg"), 16, 16, 0); // a real JPEG
        std::fs::write(d.path().join("c.png"), b"not a png").unwrap();
        let rx = spawn(d.path().to_path_buf(), Index::default(), d.path().join("cache"));
        let mut photos = 0;
        let mut failed = 0;
        let mut found = None;
        let mut done = None;
        for ev in rx.iter() {
            match ev {
                ScanEvent::Found(n) => found = Some(n),
                ScanEvent::Photo(_) => photos += 1,
                ScanEvent::Failed { .. } => failed += 1,
                ScanEvent::Done { scanned, from_cache } => { done = Some((scanned, from_cache)); }
            }
        }
        assert_eq!(found, Some(3));
        assert_eq!(photos + failed, 3);
        assert_eq!(photos, 2);
        assert!(failed >= 1, "the garbage file must be reported");
        assert_eq!(done.map(|d| d.0), Some(photos));
    }
}
//! scan — walk a folder, decode what is there, and report each photo.
//!
//! On its own thread, because decoding a camera roll takes seconds to
//! minutes and a grid that fills in as photos arrive is the honest way to
//! show that. Every result is an event over a channel; the shell drains
//! them each frame and never blocks.
//!
//! ## What is cached, and why
//!
//! * The [`Index`] carries the hashes and tags, keyed by path + size +
//!   mtime. A photo whose index entry is current is not re-hashed.
//! * Thumbnails go to the XDG cache dir as small PNGs, named by a hash of
//!   the path. A current index entry with a cached thumbnail costs one
//!   small PNG decode instead of one large JPEG decode — which is the
//!   difference between a library that opens in a second and one that
//!   opens in a minute, every time.
//!
//! ## What is decoded
//!
//! JPEG, PNG and WebP: what a phone's camera roll contains. The full image
//! is decoded once, shrunk to at most [`SCAN_PX`] on its long side for the
//! core (its hashes are resolution-invariant by design — see the core's
//! own test), and to [`THUMB_PX`] for the grid.

use crate::library::{Index, Photo, Thumb, THUMB_PX};
use photosync::{Entry, Gray};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

/// The long side the core sees. Its cells are 9×8 and 32×16; anything
/// above a few hundred pixels is decode time spent on nothing.
pub const SCAN_PX: u32 = 512;

/// The file extensions that count as photos.
pub const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

/// What the scanner reports.
#[derive(Debug)]
pub enum ScanEvent {
    /// The walk finished: this many files will be scanned.
    Found(usize),
    /// One photo, decoded (or restored from the index and the cache).
    Photo(Box<Photo>),
    /// A file that could not be read. Reported, not hidden: a photo that
    /// silently never appears is one the user cannot tell from a lost one.
    Failed { path: String, why: String },
    /// Everything reported. `from_cache` says how many were restored
    /// rather than decoded.
    Done { scanned: usize, from_cache: usize },
}

/// Is this a photo file, by extension?
pub fn is_photo(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Every photo under `root`, recursively, hidden directories skipped.
/// Sorted, so the scan order (and the order photos appear) is the same
/// every launch.
pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let hidden = p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.')).unwrap_or(false);
            if hidden { continue; }
            if p.is_dir() { stack.push(p); } else if is_photo(&p) { out.push(p); }
        }
    }
    out.sort();
    out
}

/// The thumbnail cache the app uses: `$XDG_CACHE_HOME/photosync/thumbs`.
pub fn default_cache_dir() -> PathBuf {
    config_kit::cache_home().join("photosync").join("thumbs")
}

/// Where a photo's cached thumbnail lives under a cache dir.
pub fn thumb_cache_path(cache: &Path, photo_path: &str) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in photo_path.bytes() { h ^= b as u64; h = h.wrapping_mul(0x0100_0000_01b3); }
    cache.join(format!("{h:016x}.png"))
}

/// Decode a photo fully: the core entry and a thumbnail.
pub fn decode(path: &Path, written_tags: &[&str]) -> Result<(Entry, Thumb, u32, u32), String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 { return Err("empty image".into()); }
    // One shrink for the core, a second for the grid. `thumbnail` is the
    // fast box filter; the core box-averages again on top, which is fine.
    let small = if w.max(h) > SCAN_PX { img.thumbnail(SCAN_PX, SCAN_PX) } else { img.clone() };
    let luma = small.to_luma8();
    let gray = Gray::new(luma.width() as usize, luma.height() as usize, luma.as_raw())
        .ok_or("bad stride")?;
    let entry = Entry::scan(&path.to_string_lossy(), &gray, written_tags);
    let t = small.thumbnail(THUMB_PX, THUMB_PX).to_rgba8();
    let thumb = Thumb { w: t.width() as usize, h: t.height() as usize, rgba: t.into_raw() };
    Ok((entry, thumb, w, h))
}

/// A thumbnail from the cache, if there is one.
fn cached_thumb(cache: &Path, photo_path: &str) -> Option<Thumb> {
    let p = thumb_cache_path(cache, photo_path);
    let img = image::open(&p).ok()?.to_rgba8();
    Some(Thumb { w: img.width() as usize, h: img.height() as usize, rgba: img.into_raw() })
}

fn cache_thumb(cache: &Path, photo_path: &str, t: &Thumb) {
    let p = thumb_cache_path(cache, photo_path);
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    if let Some(buf) = image::RgbaImage::from_raw(t.w as u32, t.h as u32, t.rgba.clone()) {
        let _ = buf.save(&p);
    }
}

/// The file's size and modification time, as the index keys them.
pub fn stat(path: &Path) -> std::io::Result<(u64, i64)> {
    let m = std::fs::metadata(path)?;
    let at = m.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok((m.len(), at))
}

/// Scan one file, through the index and the cache when they are current.
pub fn scan_one(path: &Path, index: &Index, cache: &Path) -> Result<(Photo, bool), String> {
    let key = path.to_string_lossy().to_string();
    let (size, at) = stat(path).map_err(|e| e.to_string())?;
    let prior = index.entries.iter().find(|e| e.path == key);
    let written: Vec<&str> = prior.map(|e| e.tags.iter()
        .filter(|t| !photosync::cv::is_auto(t)).map(String::as_str).collect()).unwrap_or_default();
    if index.is_current(&key, size, at) {
        if let (Some(mut photo), Some(thumb)) = (index.to_library().remove(&key), cached_thumb(cache, &key)) {
            photo.thumb = Some(thumb);
            return Ok((photo, true));
        }
    }
    let (entry, thumb, w, h) = decode(path, &written)?;
    cache_thumb(cache, &key, &thumb);
    Ok((Photo { path: key, at, size, w, h, entry, thumb: Some(thumb) }, false))
}

/// Run the scan on its own thread; events arrive on the returned channel.
pub fn spawn(root: PathBuf, index: Index, cache: PathBuf) -> Receiver<ScanEvent> {
    let (tx, rx) = channel();
    std::thread::Builder::new().name("photosync-scan".into()).spawn(move || run(&root, &index, &cache, &tx)).ok();
    rx
}

fn run(root: &Path, index: &Index, cache: &Path, tx: &Sender<ScanEvent>) {
    let files = walk(root);
    if tx.send(ScanEvent::Found(files.len())).is_err() { return; }
    let (mut scanned, mut from_cache) = (0usize, 0usize);
    for f in &files {
        let ev = match scan_one(f, index, cache) {
            Ok((photo, cached)) => { scanned += 1; if cached { from_cache += 1; } ScanEvent::Photo(Box::new(photo)) }
            Err(why) => ScanEvent::Failed { path: f.to_string_lossy().to_string(), why },
        };
        if tx.send(ev).is_err() { return; }
    }
    let _ = tx.send(ScanEvent::Done { scanned, from_cache });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a small PNG with a gradient, so the decoder has real pixels.
    pub(crate) fn write_png(path: &Path, w: u32, h: u32, seed: u8) {
        // RGB, not RGBA: the JPEG encoder refuses an alpha channel, and the
        // format is chosen by the extension the test asks for.
        let mut img = image::RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = ((x + y) * 255 / (w + h).max(1)) as u8 ^ seed;
            *p = image::Rgb([v, v / 2, 255 - v]);
        }
        img.save(path).unwrap();
    }

    #[test]
    fn the_walk_finds_photos_recursively_and_skips_hidden_dirs() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("trip/.thumbs")).unwrap();
        write_png(&d.path().join("a.png"), 8, 8, 0);
        write_png(&d.path().join("trip/b.PNG"), 8, 8, 1);
        write_png(&d.path().join("trip/.thumbs/c.png"), 8, 8, 2);
        std::fs::write(d.path().join("notes.txt"), "x").unwrap();
        let found = walk(d.path());
        let names: Vec<String> = found.iter().map(|p| p.strip_prefix(d.path()).unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["a.png", "trip/b.PNG"]);
    }

    #[test]
    fn a_decoded_photo_has_a_thumbnail_and_a_real_entry() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("wide.png");
        write_png(&p, 640, 320, 0);
        let (entry, thumb, w, h) = decode(&p, &["trip"]).unwrap();
        assert_eq!((w, h), (640, 320));
        assert!(thumb.w <= THUMB_PX as usize && thumb.h <= THUMB_PX as usize);
        assert_eq!(thumb.rgba.len(), thumb.w * thumb.h * 4);
        assert!(thumb.w > thumb.h, "aspect is kept");
        assert!(entry.tags.contains(&"trip".to_string()));
        assert!(entry.tags.iter().any(|t| t == "auto:landscape"), "{:?}", entry.tags);
        assert_eq!(entry.ascii.lines().count(), photosync::ASCII_H);
    }

    #[test]
    fn a_second_scan_comes_from_the_index_and_the_cache() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("cache");
        let p = d.path().join("one.png");
        write_png(&p, 64, 48, 3);
        let (first, cached) = scan_one(&p, &Index::default(), &cache).unwrap();
        assert!(!cached);
        let mut lib = crate::library::Library { root: d.path().to_string_lossy().to_string(), ..Default::default() };
        lib.upsert(first.clone());
        lib.add_tag(0, "keeper");
        let index = Index::from_library(&lib);
        let (second, cached) = scan_one(&p, &index, &cache).unwrap();
        assert!(cached, "the index was current and the thumbnail cached");
        assert_eq!(second.entry.dhash, first.entry.dhash);
        assert!(second.entry.tags.contains(&"keeper".to_string()), "written tags survive");
        assert!(second.thumb.is_some());
        // Touch the file: the index is stale and the photo is decoded again.
        write_png(&p, 64, 48, 4);
        let (third, cached) = scan_one(&p, &index, &cache).unwrap();
        assert!(!cached || third.entry.dhash != first.entry.dhash);
        assert!(third.entry.tags.contains(&"keeper".to_string()), "written tags carried into the rescan");
    }

    #[test]
    fn the_thread_reports_found_each_photo_and_done() {
        let d = tempfile::tempdir().unwrap();
        write_png(&d.path().join("a.png"), 16, 16, 0);
        write_png(&d.path().join("b.jpg"), 16, 16, 0); // a real JPEG
        std::fs::write(d.path().join("c.png"), b"not a png").unwrap();
        let rx = spawn(d.path().to_path_buf(), Index::default(), d.path().join("cache"));
        let mut photos = 0;
        let mut failed = 0;
        let mut found = None;
        let mut done = None;
        for ev in rx.iter() {
            match ev {
                ScanEvent::Found(n) => found = Some(n),
                ScanEvent::Photo(_) => photos += 1,
                ScanEvent::Failed { .. } => failed += 1,
                ScanEvent::Done { scanned, from_cache } => { done = Some((scanned, from_cache)); }
            }
        }
        assert_eq!(found, Some(3));
        assert_eq!(photos + failed, 3);
        assert_eq!(photos, 2);
        assert!(failed >= 1, "the garbage file must be reported");
        assert_eq!(done.map(|d| d.0), Some(photos));
    }
}
