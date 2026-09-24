//! Downloads kept in /tmp, keyed by the page URL.
//!
//! A downloaded video used to live exactly as long as its playback: closed,
//! it was deleted, and watching it again -- or opening it from "where did I
//! see that" -- meant downloading it again. Now it stays, under
//! `$TMPDIR/vidkit-cache/<key>/`, until the cache needs the room.
//!
//! # /tmp is RAM on a phone
//!
//! postmarketOS mounts /tmp as tmpfs. Every byte kept there is a byte of
//! memory, so the budget is derived from /tmp's own size rather than fixed:
//! a quarter of it, capped at 2 GiB, and never so much that /tmp drops below
//! a tenth free. It goes away at reboot, which on tmpfs is the point.
//! `VIDKIT_CACHE_MB` overrides; `0` turns keeping off (the old behaviour).
//!
//! # Layout
//!
//! ```text
//! vidkit-cache/<fnv(url)>-<height>/
//!     media.<ext>      what yt-dlp wrote
//!     url              the page URL (for eviction logs and for humans)
//!     done             present only once the download finished and checked
//!     lock-<pid>       held while a process plays it; never evicted then
//! ```
//! A download goes to `<key>.partial-<pid>` and is renamed into place, so a
//! killed download never looks finished.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub fn root() -> PathBuf {
    std::env::temp_dir().join("vidkit-cache")
}

/// The key for a page URL at a quality ceiling: the same video at 480p and
/// 1080p are different files.
pub fn key(url: &str, max_height: Option<u32>) -> String {
    let h = crate::transcribe::fnv1a(url.trim().as_bytes());
    format!("{h:016x}-{}", max_height.map(|m| m.to_string()).unwrap_or_else(|| "best".into()))
}

/// One cache entry as eviction sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub dir: PathBuf,
    pub bytes: u64,
    /// Last time it was opened.
    pub used: SystemTime,
    /// A live process holds a lock on it.
    pub in_use: bool,
    /// An abandoned partial download (its process is gone).
    pub stale_partial: bool,
}

/// Bytes to keep, from /tmp's size and free space. Pure.
pub fn budget(fs_total: u64, fs_free: u64, cached: u64, env_mb: Option<u64>) -> u64 {
    if let Some(mb) = env_mb {
        return mb * 1024 * 1024;
    }
    let quarter = (fs_total / 4).min(2 << 30);
    // What the cache may occupy while leaving a tenth of /tmp free: what it
    // holds now plus the free space above that floor.
    let floor = fs_total / 10;
    let room = cached + fs_free.saturating_sub(floor);
    quarter.min(room)
}

/// Which entries to delete: abandoned partials always; then least recently
/// used first until the total fits. Never one in use. Pure.
pub fn evict(entries: &[Entry], budget: u64) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = entries.iter().filter(|e| e.stale_partial && !e.in_use).map(|e| e.dir.clone()).collect();
    let mut live: Vec<&Entry> = entries.iter().filter(|e| !e.stale_partial).collect();
    let mut total: u64 = live.iter().map(|e| e.bytes).sum();
    live.sort_by_key(|e| e.used);
    for e in live {
        if total <= budget {
            break;
        }
        if e.in_use {
            continue;
        }
        total -= e.bytes;
        out.push(e.dir.clone());
    }
    out
}

fn dir_bytes(d: &Path) -> u64 {
    std::fs::read_dir(d)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

fn locked_by_live(d: &Path) -> bool {
    std::fs::read_dir(d)
        .map(|rd| {
            rd.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("lock-"))
                    .and_then(|p| p.parse::<u32>().ok())
                    .is_some_and(crate::io::pid_is_running)
            })
        })
        .unwrap_or(false)
}

/// Scan the cache directory.
pub fn scan(root: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let dir = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let partial_pid = name.split_once(".partial-").and_then(|(_, p)| p.parse::<u32>().ok());
            let used = std::fs::metadata(dir.join("done"))
                .and_then(|m| m.modified())
                .or_else(|_| e.metadata().and_then(|m| m.modified()))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Entry {
                bytes: dir_bytes(&dir),
                used,
                in_use: locked_by_live(&dir) || partial_pid.is_some_and(crate::io::pid_is_running),
                stale_partial: partial_pid.is_some_and(|p| !crate::io::pid_is_running(p))
                    || (partial_pid.is_none() && !dir.join("done").exists() && !locked_by_live(&dir)),
                dir,
            }
        })
        .collect()
}

/// statvfs of the cache's filesystem: (total, free) bytes.
fn fs_space(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: a valid NUL-terminated path and a zeroed out-struct.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    Some((s.f_blocks as u64 * f, s.f_bavail as u64 * f))
}

fn env_mb() -> Option<u64> {
    std::env::var("VIDKIT_CACHE_MB").ok().and_then(|v| v.trim().parse().ok())
}

/// Trim the cache to its budget. Returns what was deleted.
pub fn trim(root: &Path) -> Vec<PathBuf> {
    let entries = scan(root);
    let cached: u64 = entries.iter().map(|e| e.bytes).sum();
    let (total, free) = fs_space(root).or_else(|| fs_space(&std::env::temp_dir())).unwrap_or((0, 0));
    let b = budget(total, free, cached, env_mb());
    let gone = evict(&entries, b);
    for d in &gone {
        let _ = std::fs::remove_dir_all(d);
    }
    gone
}

/// A finished download for this URL, if one is cached.
pub fn lookup(root: &Path, url: &str, max_height: Option<u32>) -> Option<PathBuf> {
    let dir = root.join(key(url, max_height));
    if !dir.join("done").is_file() {
        return None;
    }
    let media = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_stem().and_then(|s| s.to_str()) == Some("media") && p.is_file()
    })?;
    // Freshen: eviction orders by the marker's mtime.
    let _ = std::fs::write(dir.join("done"), b"");
    Some(media)
}

/// Held while playing: a lock file that eviction respects, removed on drop.
pub struct Held {
    pub file: PathBuf,
    lock: PathBuf,
}

impl Held {
    pub fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock);
    }
}

pub fn hold(media: PathBuf) -> Held {
    let lock = media.parent().unwrap_or(Path::new(".")).join(format!("lock-{}", std::process::id()));
    let _ = std::fs::write(&lock, b"");
    Held { file: media, lock }
}

/// The cached file for `url`, downloading it if needed.
///
/// `download(dir)` fills `dir` (yt-dlp with a `media.%(ext)s` template) and
/// returns the file it wrote; it is `io::fetch_into` in the app and a stub
/// in tests.
pub fn get_or_fetch(
    root: &Path,
    url: &str,
    max_height: Option<u32>,
    download: impl FnOnce(&Path) -> Result<PathBuf, String>,
) -> Result<(Held, bool), String> {
    if let Some(m) = lookup(root, url, max_height) {
        return Ok((hold(m), true));
    }
    let final_dir = root.join(key(url, max_height));
    let partial = root.join(format!("{}.partial-{}", key(url, max_height), std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let file = match download(&partial) {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(e);
        }
    };
    let _ = std::fs::write(partial.join("url"), url);
    let _ = std::fs::write(partial.join("done"), b"");
    // Another instance may have finished the same URL meanwhile: theirs wins,
    // ours is dropped.
    if final_dir.join("done").is_file() {
        let _ = std::fs::remove_dir_all(&partial);
    } else {
        let _ = std::fs::remove_dir_all(&final_dir);
        std::fs::rename(&partial, &final_dir).map_err(|e| format!("{}: {e}", final_dir.display()))?;
    }
    let name = file.file_name().ok_or("download has no file name")?;
    let media = final_dir.join(name);
    let held = hold(media);
    // Trim AFTER holding, so the new entry is never the one evicted.
    trim(root);
    Ok((held, false))
}

/// How long ago, for a status line.
pub fn age(t: SystemTime) -> Duration {
    SystemTime::now().duration_since(t).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, bytes: u64, used: u64, in_use: bool, stale: bool) -> Entry {
        Entry { dir: PathBuf::from(name), bytes, used: SystemTime::UNIX_EPOCH + Duration::from_secs(used), in_use, stale_partial: stale }
    }

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_budget_follows_tmpfs_size_and_leaves_room() {
        // A phone: 3.8 GiB tmpfs (half of RAM), mostly free.
        assert_eq!(budget(38 * GIB / 10, 35 * GIB / 10, 0, None), 38 * GIB / 40, "a quarter");
        // A desktop /tmp on a big disk: capped at 2 GiB.
        assert_eq!(budget(500 * GIB, 400 * GIB, 0, None), 2 * GIB);
        // Nearly full /tmp: only what keeps a tenth free.
        let b = budget(4 * GIB, GIB / 2, GIB / 4, None);
        assert_eq!(b, GIB / 4 + (GIB / 2 - 4 * GIB / 10));
        assert_eq!(budget(4 * GIB, 0, 0, None), 0, "nothing free, nothing kept");
        assert_eq!(budget(4 * GIB, 4 * GIB, 0, Some(0)), 0, "0 turns keeping off");
        assert_eq!(budget(4 * GIB, 4 * GIB, 0, Some(100)), 100 << 20);
    }

    #[test]
    fn eviction_is_least_recent_first_and_never_touches_what_plays() {
        let es = [e("old", 400, 1, false, false), e("playing", 500, 0, true, false), e("mid", 300, 5, false, false), e("new", 200, 9, false, false)];
        // 1400 total, budget 700: drop old (1000 left), skip playing, drop mid (700).
        assert_eq!(evict(&es, 700), vec![PathBuf::from("old"), PathBuf::from("mid")]);
        assert!(evict(&es, 5000).is_empty());
        assert!(!evict(&es, 0).contains(&PathBuf::from("playing")), "in use survives even a zero budget");
    }

    #[test]
    fn abandoned_partials_always_go() {
        let es = [e("done", 100, 1, false, false), e("x.partial-1", 900, 2, false, true), e("y.partial-2", 50, 3, true, false)];
        assert_eq!(evict(&es, 10_000), vec![PathBuf::from("x.partial-1")]);
    }

    #[test]
    fn keys_separate_urls_and_qualities_and_ignore_whitespace() {
        assert_ne!(key("https://y/a", Some(720)), key("https://y/b", Some(720)));
        assert_ne!(key("https://y/a", Some(720)), key("https://y/a", Some(1080)));
        assert_eq!(key(" https://y/a\n", None), key("https://y/a", None));
    }

    /// The real thing on disk: first open downloads, second is a hit and
    /// downloads nothing, a failed download leaves nothing behind, and the
    /// lock stops eviction until the holder is dropped.
    #[test]
    fn a_second_open_is_a_hit_and_a_failure_leaves_no_trace() {
        let root = std::env::temp_dir().join(format!("vk-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dl = |d: &Path| {
            let f = d.join("media.mkv");
            std::fs::write(&f, vec![7u8; 1000]).unwrap();
            Ok(f)
        };
        let (h, hit) = get_or_fetch(&root, "https://site/v", Some(720), dl).unwrap();
        assert!(!hit && h.path().is_file());
        let mut called = false;
        let (h2, hit2) = get_or_fetch(&root, "https://site/v", Some(720), |_| { called = true; Err("no".into()) }).unwrap();
        assert!(hit2 && !called, "cached: no download");
        assert_eq!(h2.path(), h.path());

        assert!(get_or_fetch(&root, "https://site/other", None, |_| Err("403".into())).is_err());
        let names: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 1, "a failed download leaves no partial: {names:?}");

        let entries = scan(&root);
        assert!(entries[0].in_use, "held while playing");
        assert!(evict(&entries, 0).is_empty());
        drop(h);
        drop(h2);
        let entries = scan(&root);
        assert!(!entries[0].in_use);
        assert_eq!(evict(&entries, 0).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A download killed mid-way is a partial of a dead process: never
    /// served, always swept. pid 1 exists; a huge pid does not.
    #[test]
    fn a_killed_download_is_never_served() {
        let root = std::env::temp_dir().join(format!("vk-cache-k-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dead = root.join(format!("{}.partial-{}", key("https://s/k", None), 3_999_999));
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::write(dead.join("media.mkv"), b"half").unwrap();
        assert_eq!(lookup(&root, "https://s/k", None), None);
        let es = scan(&root);
        assert!(es[0].stale_partial);
        assert_eq!(evict(&es, u64::MAX), vec![dead]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
